//! Passage-budget admission for `_bulk` requests that embed text.
//!
//! The embedding model is the bottleneck of a semantic bulk. Without a bound
//! on the work already accepted, a client keeps sending bulks faster than the
//! model drains them, each one waits behind the others, and the client times
//! out and re-sends. This module counts passages that were reserved at
//! admission and not yet embedded (`L`), and refuses a new bulk with a
//! whole-request 429 when `L + cost` would exceed `budget_secs * mu`, where
//! `mu` is the measured model throughput in passages per second.
//!
//! Rules that keep it safe:
//!
//! * No measured `mu` (cold start, or a backend that does not feed the neural
//!   rate) admits everything. The module never throttles on a guess.
//! * An empty backlog always admits, so a bulk larger than the whole budget
//!   cannot be refused forever.
//! * A bulk that fits the rope is admitted even when the arrival average puts
//!   utilization at or above 100 %; utilization only names the reason when the
//!   rope is also exceeded.
//! * The check and the reservation happen under one lock, so concurrent bulks
//!   cannot both pass on the same stale `L`.
//! * A [`PassageReservation`] releases what it still holds when it drops, so a
//!   cancelled or failed bulk cannot leak backlog.

pub mod dbr;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use dbr::{DbrAdmission, DbrFlowMetrics, DbrInput};

/// Bytes per passage used to estimate a document's cost before it is chunked.
/// The semantic chunker targets 512 characters with 64 of overlap, so each
/// chunk after the first advances by 448.
pub const BYTES_PER_PASSAGE: u64 = 448;

/// Time constant, in seconds, of the arrival-rate average.
const ARRIVAL_TAU_SECS: f64 = 30.0;

/// Estimated passages for `semantic_bytes` of semantic text in one document:
/// `max(1, ceil(bytes / 448))`.
pub fn estimate_passages(semantic_bytes: u64) -> u64 {
    semantic_bytes.div_ceil(BYTES_PER_PASSAGE).max(1)
}

/// Exponentially decayed arrival rate of admitted passages per second. A
/// steady `x` passages/s converges to `x`.
#[derive(Debug)]
struct ArrivalRate {
    rate: f64,
    at: Instant,
}

impl ArrivalRate {
    fn rate_at(&self, now: Instant) -> f64 {
        let dt = now.saturating_duration_since(self.at).as_secs_f64();
        self.rate * (-dt / ARRIVAL_TAU_SECS).exp()
    }

    fn record(&mut self, passages: u64, now: Instant) {
        self.rate = self.rate_at(now) + passages as f64 / ARRIVAL_TAU_SECS;
        self.at = now;
    }
}

/// A refused bulk: the text for the 429 reason and the `Retry-After` seconds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmissionHold {
    pub reason: String,
    pub retry_after_secs: u64,
}

/// Process-wide backlog of reserved, not-yet-embedded passages.
#[derive(Debug)]
pub struct PassageAdmission {
    queued: AtomicU64,
    arrivals: Mutex<ArrivalRate>,
    /// Test-only service rate in f64 bits; `0` means "ask the embedder".
    mu_override_bits: AtomicU64,
}

impl Default for PassageAdmission {
    fn default() -> Self {
        Self {
            queued: AtomicU64::new(0),
            arrivals: Mutex::new(ArrivalRate {
                rate: 0.0,
                at: Instant::now(),
            }),
            mu_override_bits: AtomicU64::new(0),
        }
    }
}

impl PassageAdmission {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Passages reserved and not yet embedded (`L`).
    pub fn queued(&self) -> u64 {
        self.queued.load(Ordering::Relaxed)
    }

    /// Pin the service rate instead of reading the neural EWMA. Tests only.
    #[doc(hidden)]
    pub fn set_service_rate_for_test(&self, passages_per_second: f64) {
        self.mu_override_bits
            .store(passages_per_second.to_bits(), Ordering::Relaxed);
    }

    fn service_rate(&self) -> f64 {
        match self.mu_override_bits.load(Ordering::Relaxed) {
            0 => xerj_ai::embedder::neural_passages_per_second(),
            bits => f64::from_bits(bits),
        }
    }

    fn publish(&self) {
        if let Some(m) = xerj_common::metrics::global_metrics() {
            m.embed_passages_queued
                .set(i64::try_from(self.queued()).unwrap_or(i64::MAX));
        }
    }

    /// Admit a bulk of `cost` passages or refuse it. `budget_secs` is the rope
    /// in seconds of model work.
    pub fn admit(
        self: &Arc<Self>,
        cost: u64,
        budget_secs: u64,
    ) -> Result<PassageReservation, AdmissionHold> {
        self.admit_at(cost, budget_secs, Instant::now())
    }

    fn admit_at(
        self: &Arc<Self>,
        cost: u64,
        budget_secs: u64,
        now: Instant,
    ) -> Result<PassageReservation, AdmissionHold> {
        let mu = self.service_rate();
        let mut arrivals = self
            .arrivals
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let queued = self.queued();
        let mu_milli = (mu * 1_000.0) as u64;
        // Cold start, or an idle pipeline: nothing to protect or nothing to
        // wait behind.
        if mu.is_finite() && mu > 0.0 && mu_milli > 0 && queued > 0 {
            let rope = (mu * budget_secs as f64) as u64;
            let metrics = DbrFlowMetrics::evaluate(DbrInput {
                queued_passages: queued,
                candidate_passages: cost,
                service_rate_milli_per_sec: mu_milli,
                arrival_rate_milli_per_sec: (arrivals.rate_at(now) * 1_000.0) as u64,
                rope_passages: rope,
            });
            // Utilization alone does not hold a bulk that fits the rope: the
            // arrival average is an impulse estimate (one admit of `n`
            // passages adds `n / 30` passages/s at once), so a single bulk
            // of 30 * mu passages would otherwise block the next small one
            // although the budget has room. Utilization decides the reason
            // only when the rope is also exceeded.
            let fits_rope = queued.saturating_add(cost) <= rope;
            if metrics.admission != DbrAdmission::Admit
                && !(metrics.admission == DbrAdmission::HoldUnstableQueue && fits_rope)
            {
                let why = match metrics.admission {
                    DbrAdmission::HoldUnstableQueue => {
                        "admitted passages arrive at or above the embedding rate"
                    }
                    _ => "the backlog would exceed its budget",
                };
                return Err(AdmissionHold {
                    reason: format!(
                        "embedding backlog: {queued} passages are queued and this bulk adds \
                         about {cost}; {why} ({budget_secs} s at {mu:.1} passages/s is {rope} \
                         passages). Retry the whole request in {} s",
                        metrics.retry_after_secs
                    ),
                    retry_after_secs: metrics.retry_after_secs,
                });
            }
        }
        self.queued.fetch_add(cost, Ordering::Relaxed);
        arrivals.record(cost, now);
        drop(arrivals);
        self.publish();
        Ok(PassageReservation {
            owner: Arc::clone(self),
            held: AtomicU64::new(cost),
        })
    }
}

/// Passages a bulk reserved and has not yet seen embedded. Dropping it
/// releases the remainder.
#[derive(Debug)]
pub struct PassageReservation {
    owner: Arc<PassageAdmission>,
    held: AtomicU64,
}

impl PassageReservation {
    /// Passages still held.
    pub fn held(&self) -> u64 {
        self.held.load(Ordering::Relaxed)
    }

    /// Replace an estimate with the real passage count once the documents are
    /// chunked: add or return the difference.
    pub fn settle(&self, estimated: u64, actual: u64) {
        if actual > estimated {
            let extra = actual - estimated;
            self.held.fetch_add(extra, Ordering::Relaxed);
            self.owner.queued.fetch_add(extra, Ordering::Relaxed);
            self.owner.publish();
        } else {
            self.release(estimated - actual);
        }
    }

    /// Release up to `passages` (a finished embedding window). Never releases
    /// more than the reservation holds.
    pub fn release(&self, passages: u64) {
        let Ok(prev) = self
            .held
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |h| {
                Some(h.saturating_sub(passages))
            })
        else {
            return;
        };
        let released = prev.min(passages);
        if released == 0 {
            return;
        }
        let _ = self
            .owner
            .queued
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |q| {
                Some(q.saturating_sub(released))
            });
        self.owner.publish();
    }
}

impl Drop for PassageReservation {
    fn drop(&mut self) {
        self.release(self.held());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn controller(mu: f64) -> Arc<PassageAdmission> {
        let c = PassageAdmission::new();
        c.set_service_rate_for_test(mu);
        c
    }

    #[test]
    fn passage_estimate_is_ceil_of_bytes_over_448_and_at_least_one() {
        assert_eq!(estimate_passages(0), 1);
        assert_eq!(estimate_passages(1), 1);
        assert_eq!(estimate_passages(448), 1);
        assert_eq!(estimate_passages(449), 2);
        assert_eq!(estimate_passages(4_480), 10);
    }

    #[test]
    fn cold_start_without_a_service_rate_admits_everything() {
        let c = PassageAdmission::new(); // reads the neural EWMA: unset here
        c.set_service_rate_for_test(0.0);
        let a = c.admit(1_000_000, 60).unwrap();
        let b = c.admit(1_000_000, 60).unwrap();
        assert_eq!(c.queued(), 2_000_000);
        drop((a, b));
        assert_eq!(c.queued(), 0);
    }

    #[test]
    fn second_bulk_is_held_while_the_first_is_queued_and_an_empty_queue_admits() {
        // mu = 10/s, budget 1 s: rope = 10 passages.
        let c = controller(10.0);
        // 12 > rope, but the queue is empty, so it is admitted.
        let first = c.admit(12, 1).unwrap();
        assert_eq!(c.queued(), 12);
        // 12 queued + 12 asked = 24; 14 over the rope at 10/s -> ceil(1.4) = 2 s.
        let hold = c.admit(12, 1).unwrap_err();
        assert_eq!(hold.retry_after_secs, 2);
        assert!(
            hold.reason.starts_with("embedding backlog"),
            "{}",
            hold.reason
        );
        assert!(!hold.reason.contains("circuit breaker"));
        assert_eq!(c.queued(), 12, "a hold must reserve nothing");
        drop(first);
        assert_eq!(c.queued(), 0);
        assert!(c.admit(12, 1).is_ok());
    }

    #[test]
    fn a_bulk_that_fits_the_remaining_budget_is_admitted() {
        let c = controller(10.0);
        let _a = c.admit(4, 1).unwrap();
        let _b = c.admit(6, 1).unwrap(); // 4 + 6 = 10 <= 10
        assert_eq!(c.queued(), 10);
        assert!(c.admit(1, 1).is_err());
    }

    #[test]
    fn release_is_per_window_and_never_more_than_held() {
        let c = controller(10.0);
        let r = c.admit(10, 1).unwrap();
        r.release(4);
        assert_eq!((r.held(), c.queued()), (6, 6));
        r.release(100);
        assert_eq!((r.held(), c.queued()), (0, 0));
    }

    #[test]
    fn settle_applies_the_delta_in_both_directions() {
        let c = controller(10.0);
        let r = c.admit(10, 1).unwrap();
        r.settle(10, 25); // more chunks than estimated
        assert_eq!((r.held(), c.queued()), (25, 25));
        r.settle(25, 5); // fewer
        assert_eq!((r.held(), c.queued()), (5, 5));
    }

    /// A reservation dropped without a single window completing (a cancelled
    /// handler, a failed group) returns the backlog to zero.
    #[test]
    fn dropped_reservation_returns_the_backlog_to_zero() {
        let c = controller(10.0);
        {
            let r = c.admit(40, 60).unwrap();
            r.settle(40, 55);
            r.release(5);
            assert_eq!(c.queued(), 50);
        }
        assert_eq!(c.queued(), 0);
    }

    #[test]
    fn utilization_alone_does_not_hold_a_bulk_that_fits_the_rope() {
        // mu = 10/s, rope 600. One admit of 300 passages makes lambda = 300 /
        // 30 s = 10/s, which is rho = 100 %. The next bulk still fits the rope
        // (301 <= 600), so it is admitted.
        let c = controller(10.0);
        let now = Instant::now();
        let _a = c.admit_at(300, 60, now).unwrap();
        let _b = c.admit_at(1, 60, now).unwrap();
        assert_eq!(c.queued(), 301);
    }

    #[test]
    fn utilization_at_the_service_rate_names_itself_when_the_rope_is_also_exceeded() {
        let c = controller(10.0);
        let now = Instant::now();
        let _a = c.admit_at(300, 60, now).unwrap();
        // 300 + 400 = 700 > 600: 100 over at 10/s is 10 s.
        let hold = c.admit_at(400, 60, now).unwrap_err();
        assert_eq!(hold.retry_after_secs, 10);
        assert!(hold.reason.contains("at or above"), "{}", hold.reason);
    }
}
