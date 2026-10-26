// SPDX-License-Identifier: Apache-2.0
// Vendored from cerebral-work/scorcher scorcher-admission/src/solver.rs (derived from
// unsigned-gg/glaeda eb7088c), 2026-10-26. See NOTICE.
//
//! Drum-Buffer-Rope admission arithmetic for the embedding backlog.
//!
//! Only the pure `WorkerPoolFlowMetrics::evaluate` function is vendored, cut
//! down to what a single-server passage queue needs:
//!
//! * Pool utilization `rho = lambda * tau_eff / m` in basis points, here with
//!   `m = 1` (one model) and `tau_eff = 1 / mu` per passage.
//! * Little's Law cycle time `W = L / lambda`.
//! * The admission decision: unstable utilization or a full buffer (the rope)
//!   holds the request.
//!
//! Changes from the original, all deliberate:
//!
//! * `RejectUnstableQueue` is dropped. `rho >= 100%` maps to a hold, with the
//!   same retry arithmetic as a full buffer.
//! * Kingman, Cobham, Sweller reload, the memory bound and the CPU bound are
//!   dropped. They need `WorkerPoolConfig`, `ExecutionResourceLimits` and
//!   `PersonalWorkerPriority` from the source crate's `types.rs`, which has no
//!   licence header and is not copied.
//! * The inputs are xerj's own small struct, with no `sha2`, `bon` or `serde`.
//! * The decision carries a `Retry-After` in whole seconds: the excess backlog
//!   divided by the service rate `mu`, rounded up and clamped to 1..=60.

/// Basis points in 100 %.
const BPS_SCALE: u64 = 10_000;

/// Upper bound of the advised `Retry-After`, in seconds.
pub const MAX_RETRY_AFTER_SECS: u64 = 60;

/// What the evaluator needs to know about the backlog and the candidate bulk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DbrInput {
    /// Passages reserved and not yet embedded (`L`).
    pub queued_passages: u64,
    /// Passages the candidate bulk would add.
    pub candidate_passages: u64,
    /// Service rate `mu` in milli-passages per second (`1_000` = 1 passage/s).
    /// Must be non-zero: a cold start has no rate and is admitted by the caller.
    pub service_rate_milli_per_sec: u64,
    /// Arrival rate `lambda` of admitted passages, in milli-passages per second.
    pub arrival_rate_milli_per_sec: u64,
    /// The rope: the largest backlog, in passages, the pipeline may hold.
    pub rope_passages: u64,
}

/// The decision, with the reason a hold was issued.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbrAdmission {
    Admit,
    /// Utilization is at or above 100 % (the source's `RejectUnstableQueue`,
    /// mapped to a hold).
    HoldUnstableQueue,
    /// The backlog plus the candidate would exceed the rope.
    HoldBufferFull,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DbrFlowMetrics {
    /// Total work-in-progress `L`.
    pub total_wip: u64,
    /// Per-passage service time `tau_eff = 1 / mu` in milliseconds, at least 1.
    pub effective_service_time_ms: u64,
    /// `rho = lambda * tau_eff` in basis points (`10_000` = 100 %).
    pub utilization_bps: u32,
    /// Little's Law cycle time `W = L / lambda` in milliseconds (`u64::MAX`
    /// when `lambda == 0` and `L > 0`).
    pub littles_law_cycle_time_ms: u64,
    pub admission: DbrAdmission,
    /// Seconds a held client should wait, `ceil((L + cost - rope) / mu)`
    /// clamped to `1..=60`. Zero when admitted.
    pub retry_after_secs: u64,
}

impl DbrFlowMetrics {
    /// Evaluate Little's Law, utilization and the rope for a candidate bulk.
    #[must_use]
    pub fn evaluate(input: DbrInput) -> Self {
        let total_wip = input.queued_passages;
        let mu_milli = input.service_rate_milli_per_sec;

        // tau_eff in ms: 1 / mu, with mu in milli-passages/s.
        let effective_service_time_ms = 1_000_000_u64.checked_div(mu_milli).unwrap_or(1).max(1);

        // rho = (lambda * tau_eff) in basis points. lambda is lambda*1000 and
        // tau_eff is in ms, so (lambda_milli * tau_ms * 10_000) / 1_000_000
        // reduces to (lambda_milli * tau_ms) / 100.
        let utilization_u64 = input
            .arrival_rate_milli_per_sec
            .saturating_mul(effective_service_time_ms)
            / 100;
        let utilization_bps = u32::try_from(utilization_u64).unwrap_or(u32::MAX);

        // Little's Law: W (ms) = (L * 1_000_000) / lambda_milli.
        let littles_law_cycle_time_ms = if total_wip == 0 {
            0
        } else {
            total_wip
                .saturating_mul(1_000_000)
                .checked_div(input.arrival_rate_milli_per_sec)
                .unwrap_or(u64::MAX)
        };

        let demand = total_wip.saturating_add(input.candidate_passages);
        let admission = if u64::from(utilization_bps) >= BPS_SCALE {
            DbrAdmission::HoldUnstableQueue
        } else if demand > input.rope_passages {
            DbrAdmission::HoldBufferFull
        } else {
            DbrAdmission::Admit
        };

        let retry_after_secs = if admission == DbrAdmission::Admit {
            0
        } else {
            // ceil(excess / mu) with mu in milli-passages/s.
            let excess = demand.saturating_sub(input.rope_passages);
            excess
                .saturating_mul(1_000)
                .div_ceil(mu_milli.max(1))
                .clamp(1, MAX_RETRY_AFTER_SECS)
        };

        Self {
            total_wip,
            effective_service_time_ms,
            utilization_bps,
            littles_law_cycle_time_ms,
            admission,
            retry_after_secs,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// mu = 100 passages/s, so a 60 s rope is 6000 passages.
    fn input(queued: u64, cost: u64, lambda_per_sec: u64) -> DbrInput {
        DbrInput {
            queued_passages: queued,
            candidate_passages: cost,
            service_rate_milli_per_sec: 100_000,
            arrival_rate_milli_per_sec: lambda_per_sec * 1_000,
            rope_passages: 6_000,
        }
    }

    #[test]
    fn admits_when_backlog_plus_cost_fits_the_rope() {
        let m = DbrFlowMetrics::evaluate(input(5_000, 1_000, 10));
        assert_eq!(m.admission, DbrAdmission::Admit);
        assert_eq!(m.retry_after_secs, 0);
        assert_eq!(m.total_wip, 5_000);
        // lambda = 10/s, mu = 100/s -> rho = 10 %.
        assert_eq!(m.utilization_bps, 1_000);
        assert_eq!(m.effective_service_time_ms, 10);
    }

    #[test]
    fn holds_with_the_littles_law_retry_when_the_rope_is_exceeded() {
        // 5_900 queued + 300 asked = 6_200; 200 over the rope at 100/s = 2 s.
        let m = DbrFlowMetrics::evaluate(input(5_900, 300, 10));
        assert_eq!(m.admission, DbrAdmission::HoldBufferFull);
        assert_eq!(m.retry_after_secs, 2);
        // 201 over rounds up to 3 s.
        let m = DbrFlowMetrics::evaluate(input(5_900, 301, 10));
        assert_eq!(m.retry_after_secs, 3);
    }

    #[test]
    fn retry_after_is_clamped_to_one_through_sixty_seconds() {
        let m = DbrFlowMetrics::evaluate(input(5_999, 2, 10));
        assert_eq!(m.admission, DbrAdmission::HoldBufferFull);
        assert_eq!(m.retry_after_secs, 1, "1 over the rope is under a second");
        let m = DbrFlowMetrics::evaluate(input(0, 1_000_000, 10));
        assert_eq!(m.retry_after_secs, MAX_RETRY_AFTER_SECS);
    }

    #[test]
    fn utilization_at_or_above_one_hundred_percent_holds_instead_of_rejecting() {
        // lambda = mu = 100/s -> rho = 100 %. The source rejected here; xerj holds.
        let m = DbrFlowMetrics::evaluate(input(10, 10, 100));
        assert_eq!(m.utilization_bps, 10_000);
        assert_eq!(m.admission, DbrAdmission::HoldUnstableQueue);
        // Inside the rope the excess is zero, so the clamp gives 1 s.
        assert_eq!(m.retry_after_secs, 1);
        // Over the rope as well: same arithmetic as a full buffer.
        let m = DbrFlowMetrics::evaluate(input(5_900, 300, 150));
        assert_eq!(m.admission, DbrAdmission::HoldUnstableQueue);
        assert_eq!(m.retry_after_secs, 2);
        // Just under 100 % is still admitted.
        let m = DbrFlowMetrics::evaluate(input(10, 10, 99));
        assert_eq!(m.admission, DbrAdmission::Admit);
    }

    #[test]
    fn littles_law_cycle_time_is_backlog_over_arrival_rate() {
        // L = 6_000 at lambda = 100/s -> W = 60 s.
        let m = DbrFlowMetrics::evaluate(input(6_000, 0, 100));
        assert_eq!(m.littles_law_cycle_time_ms, 60_000);
        let idle = DbrFlowMetrics::evaluate(input(0, 5, 100));
        assert_eq!(idle.littles_law_cycle_time_ms, 0);
        let no_arrivals = DbrFlowMetrics::evaluate(input(5, 5, 0));
        assert_eq!(no_arrivals.littles_law_cycle_time_ms, u64::MAX);
    }
}
