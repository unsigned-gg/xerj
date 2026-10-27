//! Passage-budget admission for `_bulk` requests that embed text.
//!
//! The embedding model drains passages at a measured rate `mu` (passages per
//! second). `L` counts passages that admitted requests have reserved and the
//! model has not finished. A request that would cost `cost` passages is
//! admitted when `L + cost <= budget_secs * mu`, and refused whole otherwise.
//! Until `mu` has been measured, and while `L == 0`, every request is
//! admitted: the budget bounds the queue, not one request.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Bytes of document per estimated passage.
pub const BYTES_PER_PASSAGE: u64 = 448;

/// Largest `Retry-After` value a refusal will ever send, in seconds.
pub const RETRY_AFTER_CEILING_SECS: u64 = 60;

/// Estimated passages for one document of `doc_bytes` bytes.
pub fn estimate_passages(doc_bytes: usize) -> u64 {
    (doc_bytes as u64).div_ceil(BYTES_PER_PASSAGE)
}

/// Why a request was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejection {
    /// Passages already reserved.
    pub queued: u64,
    /// Passages the request asked for.
    pub cost: u64,
    /// The budget in passages.
    pub budget: u64,
    /// `Retry-After` in seconds, within `1..=60`.
    pub retry_after_secs: u64,
}

/// Passages reserved and not yet embedded.
#[derive(Debug)]
pub struct PassageLedger {
    queued: AtomicU64,
    publish: bool,
}

impl PassageLedger {
    /// A ledger that mirrors its count into `xerj_embed_passages_queued`.
    fn process() -> Self {
        Self {
            queued: AtomicU64::new(0),
            publish: true,
        }
    }

    /// A ledger that leaves process metrics alone.
    pub fn isolated() -> Self {
        Self {
            queued: AtomicU64::new(0),
            publish: false,
        }
    }

    /// Passages reserved and not yet embedded.
    pub fn queued(&self) -> u64 {
        self.queued.load(Ordering::Acquire)
    }

    fn publish(&self) {
        if self.publish {
            if let Some(m) = xerj_common::metrics::global_metrics() {
                m.embed_passages_queued
                    .set(i64::try_from(self.queued()).unwrap_or(i64::MAX));
            }
        }
    }

    /// Decide on a request of `cost` passages. `budget_secs == 0` disables the
    /// rule, so nothing is reserved. On admission the cost is added to `L`.
    pub fn admit(
        self: &Arc<Self>,
        cost: u64,
        mu: f64,
        budget_secs: u64,
    ) -> Result<PassageGuard, Rejection> {
        if budget_secs == 0 {
            return Ok(PassageGuard::new(Arc::clone(self), 0));
        }
        let measured = mu.is_finite() && mu > 0.0;
        let budget = budget_secs as f64 * mu;
        let mut queued = self.queued.load(Ordering::Acquire);
        loop {
            let total = queued.saturating_add(cost);
            if measured && queued > 0 && total as f64 > budget {
                let excess_secs = (total as f64 - budget) / mu;
                return Err(Rejection {
                    queued,
                    cost,
                    budget: budget as u64,
                    retry_after_secs: (excess_secs.ceil() as u64)
                        .clamp(1, RETRY_AFTER_CEILING_SECS),
                });
            }
            match self.queued.compare_exchange_weak(
                queued,
                total,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(seen) => queued = seen,
            }
        }
        self.publish();
        Ok(PassageGuard::new(Arc::clone(self), cost))
    }

    fn sub(&self, n: u64) {
        self.queued.fetch_sub(n, Ordering::AcqRel);
        self.publish();
    }
}

/// Passages one request still holds. Dropping it returns what is left, so an
/// error or a cancelled request cannot leak.
#[derive(Debug)]
pub struct PassageGuard {
    ledger: Arc<PassageLedger>,
    held: AtomicU64,
}

impl PassageGuard {
    fn new(ledger: Arc<PassageLedger>, held: u64) -> Self {
        Self {
            ledger,
            held: AtomicU64::new(held),
        }
    }

    /// Passages still held.
    pub fn held(&self) -> u64 {
        self.held.load(Ordering::Acquire)
    }

    /// Take up to `n` out of `held`, returning how many were taken.
    fn take(&self, n: u64) -> u64 {
        let mut held = self.held.load(Ordering::Acquire);
        loop {
            let taken = n.min(held);
            match self.held.compare_exchange_weak(
                held,
                held - taken,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return taken,
                Err(seen) => held = seen,
            }
        }
    }

    /// Return up to `n` passages, for example when an embedding window ends.
    pub fn release(&self, n: u64) {
        let taken = self.take(n);
        if taken > 0 {
            self.ledger.sub(taken);
        }
    }

    /// Move up to `n` held passages into a guard of their own. The ledger
    /// count does not change.
    pub fn split(&self, n: u64) -> PassageGuard {
        PassageGuard::new(Arc::clone(&self.ledger), self.take(n))
    }
}

impl Drop for PassageGuard {
    fn drop(&mut self) {
        self.release(u64::MAX);
    }
}

/// The ledger shared by every `_bulk` in this process.
pub fn process_ledger() -> Arc<PassageLedger> {
    static LEDGER: std::sync::OnceLock<Arc<PassageLedger>> = std::sync::OnceLock::new();
    Arc::clone(LEDGER.get_or_init(|| Arc::new(PassageLedger::process())))
}

#[cfg(test)]
tokio::task_local! {
    static TEST_SCOPE: (Arc<PassageLedger>, f64);
}

/// The ledger this task reserves against. A test can scope its own.
pub(crate) fn current_ledger() -> Arc<PassageLedger> {
    #[cfg(test)]
    if let Ok(ledger) = TEST_SCOPE.try_with(|(ledger, _)| Arc::clone(ledger)) {
        return ledger;
    }
    process_ledger()
}

/// Measured embedding throughput in passages per second, `0.0` if unknown.
pub(crate) fn current_throughput() -> f64 {
    #[cfg(test)]
    if let Ok(mu) = TEST_SCOPE.try_with(|(_, mu)| *mu) {
        return mu;
    }
    xerj_ai::embedder::neural_passages_per_second()
}

/// Run `future` against an isolated ledger and a fixed throughput.
#[cfg(test)]
pub(crate) async fn with_test_scope<F: std::future::Future>(
    ledger: Arc<PassageLedger>,
    mu: f64,
    future: F,
) -> F::Output {
    TEST_SCOPE.scope((ledger, mu), future).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ledger() -> Arc<PassageLedger> {
        Arc::new(PassageLedger::isolated())
    }

    #[test]
    fn estimate_is_ceil_of_bytes_over_448() {
        assert_eq!(estimate_passages(0), 0);
        assert_eq!(estimate_passages(1), 1);
        assert_eq!(estimate_passages(448), 1);
        assert_eq!(estimate_passages(449), 2);
        assert_eq!(estimate_passages(4480), 10);
    }

    #[test]
    fn cold_start_admits_and_still_counts() {
        let l = ledger();
        let a = l.admit(1_000_000, 0.0, 60).unwrap();
        let b = l.admit(1_000_000, f64::NAN, 60).unwrap();
        assert_eq!(l.queued(), 2_000_000);
        drop((a, b));
        assert_eq!(l.queued(), 0);
    }

    #[test]
    fn exact_budget_admits_and_one_over_refuses() {
        let l = ledger();
        // mu 10/s for 60 s: a budget of 600 passages.
        let a = l.admit(500, 10.0, 60).unwrap();
        let err = l.admit(101, 10.0, 60).unwrap_err();
        assert_eq!((err.queued, err.cost, err.budget), (500, 101, 600));
        assert_eq!(l.queued(), 500, "a refusal reserves nothing");
        let b = l.admit(100, 10.0, 60).unwrap();
        assert_eq!(l.queued(), 600);
        drop((a, b));
    }

    #[test]
    fn a_request_larger_than_the_whole_budget_is_admitted_when_nothing_is_queued() {
        // The budget bounds the queue, not one request: an idle node must
        // never refuse the same bulk forever. The embed deadline bounds it.
        let l = ledger();
        let g = l.admit(601, 10.0, 60).unwrap();
        assert_eq!(l.queued(), 601);
        // With that in flight, even a small request waits its turn.
        let err = l.admit(1, 10.0, 60).unwrap_err();
        assert_eq!(err.queued, 601);
        drop(g);
        assert_eq!(l.queued(), 0);
    }

    #[test]
    fn retry_after_is_ceil_of_excess_over_mu_clamped_to_1_through_60() {
        let l = ledger();
        let _held = l.admit(600, 10.0, 60).unwrap();
        // One passage short of fitting needs a tenth of a second; the floor is 1.
        assert_eq!(l.admit(1, 10.0, 60).unwrap_err().retry_after_secs, 1);
        // 250 passages short at 10 per second is exactly 25 seconds.
        assert_eq!(l.admit(250, 10.0, 60).unwrap_err().retry_after_secs, 25);
        // 251 short is 25.1 seconds; a fraction is rounded up to a whole second.
        assert_eq!(l.admit(251, 10.0, 60).unwrap_err().retry_after_secs, 26);
        // A huge shortfall is held at the ceiling.
        assert_eq!(
            l.admit(1_000_000, 10.0, 60).unwrap_err().retry_after_secs,
            60
        );
    }

    #[test]
    fn zero_budget_disables_the_rule() {
        let l = ledger();
        let g = l.admit(1_000_000, 10.0, 0).unwrap();
        assert_eq!((g.held(), l.queued()), (0, 0));
    }

    #[test]
    fn guard_releases_on_drop_and_partial_release_is_capped() {
        let l = ledger();
        let g = l.admit(10, 0.0, 60).unwrap();
        g.release(3);
        assert_eq!((g.held(), l.queued()), (7, 7));
        g.release(100);
        assert_eq!((g.held(), l.queued()), (0, 0));
        let g = l.admit(10, 0.0, 60).unwrap();
        drop(g);
        assert_eq!(l.queued(), 0);
    }

    #[test]
    fn split_moves_passages_between_guards_without_touching_the_ledger() {
        let l = ledger();
        let whole = l.admit(10, 0.0, 60).unwrap();
        let part = whole.split(4);
        assert_eq!((whole.held(), part.held(), l.queued()), (6, 4, 10));
        drop(part);
        assert_eq!(l.queued(), 6);
        drop(whole);
        assert_eq!(l.queued(), 0);
    }
}
