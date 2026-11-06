// SPDX-License-Identifier: Apache-2.0 OR MIT
//! Wes Brown's Engineering Leadership Canon: pure domain evaluations for the Top 3
//! Engineering Principles, neurodivergent executive scaffolding, interoceptive state
//! debugging, individualized communication archetypes, and team scale bounds.
//!
//! Contains zero system I/O. Grounded in the `xc-iopsych-corpus` documents:
//! - `iopsych-wes-brown-leadership-canon`
//! - `iopsych-wes-brown-neurodivergent-scaffolding`
//! - `iopsych-wes-brown-management-practices`

use serde::{Deserialize, Serialize};

/// Hard ceiling on direct engineering lanes or reports before technical management
/// depth degrades and a team split or subdomain lead is required (5-6 engineers).
pub const WES_BROWN_MAX_SPAN_OF_CONTROL: usize = 6;

/// Continuous work duration (in minutes) at which interoceptive dysregulation
/// ("6 hours without a break") requires an immediate timeout before debating technical merits.
pub const INTEROCEPTIVE_TIMEOUT_MINUTES: u32 = 240;

/// Verdict from evaluating an engineering proposal or change against Wes Brown's
/// Top 3 Engineering Principles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineeringPrincipleVerdict {
    /// Solves the highest-impact root problem at bounded cost and ships the 80% "good enough" point.
    ShipEightyTwenty,
    /// Over-scoped perfectionism (>92% completeness target with high complexity/time cost);
    /// violates Principle 3 ("perfection is the enemy of good").
    PerfectionismTrap,
    /// Addresses a surface symptom rather than the highest-impact root problem (violates Principle 1).
    SymptomWorkaround,
    /// Time, complexity, or ongoing maintenance cost outweighs the impact (violates Principle 2).
    ExcessiveMaintenanceBurden,
}

impl EngineeringPrincipleVerdict {
    /// Canonical snake_case wire label for this verdict.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ShipEightyTwenty => "ship_eighty_twenty",
            Self::PerfectionismTrap => "perfectionism_trap",
            Self::SymptomWorkaround => "symptom_workaround",
            Self::ExcessiveMaintenanceBurden => "excessive_maintenance_burden",
        }
    }
}

/// Evaluation of a design or code change against Wes Brown's Top 3 Engineering Principles:
/// 1. Determine the actual root problem being solved that has the most impact.
/// 2. Be cognizant of the cost of an approach or solution in terms of time, complexity,
///    and maintenance burden.
/// 3. Determine the "good enough" point (the 80% solution to 80% of the problem) and
///    recognize that perfection is the enemy of good.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TopThreePrinciplesEval {
    /// Principle 1: Root-problem impact score in `[0.0, 1.0]`.
    pub root_problem_impact: f64,
    /// Principle 2: Normalized cost burden (time, complexity, maintenance) in `[0.0, 1.0]`.
    pub cost_burden_ratio: f64,
    /// Principle 3: Targeted completeness / problem-coverage fraction in `[0.0, 1.0]`.
    /// The pragmatic sweet spot is `[0.75, 0.92]` (the 80% solution to 80% of the problem).
    pub target_completeness: f64,
    /// Net leverage ratio: `root_problem_impact / max(cost_burden_ratio, 0.05)`.
    pub net_leverage_ratio: f64,
    /// Resulting engineering verdict.
    pub verdict: EngineeringPrincipleVerdict,
}

impl TopThreePrinciplesEval {
    /// Evaluate a candidate engineering approach against the Top 3 Engineering Principles.
    #[must_use]
    pub fn evaluate(
        root_problem_impact: f64,
        cost_burden_ratio: f64,
        target_completeness: f64,
    ) -> Self {
        let impact = root_problem_impact.clamp(0.0, 1.0);
        let cost = cost_burden_ratio.clamp(0.0, 1.0);
        let completeness = target_completeness.clamp(0.0, 1.0);
        let net_leverage_ratio = impact / cost.max(0.05);

        let verdict = if impact < 0.50 {
            EngineeringPrincipleVerdict::SymptomWorkaround
        } else if completeness > 0.92 && cost > 0.45 {
            EngineeringPrincipleVerdict::PerfectionismTrap
        } else if cost > 0.65 || net_leverage_ratio < 1.15 {
            EngineeringPrincipleVerdict::ExcessiveMaintenanceBurden
        } else {
            EngineeringPrincipleVerdict::ShipEightyTwenty
        };

        Self {
            root_problem_impact: impact,
            cost_burden_ratio: cost,
            target_completeness: completeness,
            net_leverage_ratio,
            verdict,
        }
    }
}

/// Diagnosis from Wes Brown's conflict and stall resolution protocol:
/// "Debug the State, Not the Argument."
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StateDiagnosis {
    /// Physiological, cognitive, and runtime resource states are regulated; safe to debate technical merits.
    Regulated,
    /// Dysregulated state detected (hyperfocus exhaustion, >=4-6 hours without a break,
    /// context window/RSS saturation, or repeated retry friction). Call an immediate timeout
    /// and reset state before continuing technical debate.
    InteroceptiveTimeoutRequired,
}

/// Evaluation of operator/agent interoceptive and runtime state ("Debug the State, Not the Argument").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InteroceptiveStateEval {
    /// Continuous minutes spent in hyperfocus or debugging without a break or checkpoint.
    pub continuous_minutes: u32,
    /// Context window or node memory utilization ratio in `[0.0, 1.0]`.
    pub resource_saturation_ratio: f64,
    /// Consecutive failed retries or circular argument turns on the same issue.
    pub consecutive_friction_turns: usize,
    /// Resulting state diagnosis.
    pub diagnosis: StateDiagnosis,
}

impl InteroceptiveStateEval {
    /// Evaluate whether friction or a stall requires an immediate state-reset timeout.
    #[must_use]
    pub fn evaluate(
        continuous_minutes: u32,
        resource_saturation_ratio: f64,
        consecutive_friction_turns: usize,
    ) -> Self {
        let sat = resource_saturation_ratio.clamp(0.0, 1.0);
        let diagnosis = if continuous_minutes >= INTEROCEPTIVE_TIMEOUT_MINUTES
            || sat >= 0.85
            || consecutive_friction_turns >= 3
        {
            StateDiagnosis::InteroceptiveTimeoutRequired
        } else {
            StateDiagnosis::Regulated
        };

        Self {
            continuous_minutes,
            resource_saturation_ratio: sat,
            consecutive_friction_turns,
            diagnosis,
        }
    }
}

/// Wes Brown's 8 individualized communication archetypes for adapting technical management
/// and agent-operator feedback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommunicationArchetype {
    /// Thinks out loud; needs synchronous verbal/dialogue bounce to crystallize architecture.
    HyperverbalProcessor,
    /// Needs structured written documents and time to read before responding.
    WrittenProcessor,
    /// "Just tell me what to do": wants concrete, unambiguous action items during crunch or ambiguity.
    DirectInstruction,
    /// "Let me figure it out": wants the root goal and constraints, then uninterrupted autonomy.
    AutonomousExplorer,
    /// High capability with imposter syndrome; benefits from explicit validation of approach before critique.
    ValidationSeeker,
    /// "Just the facts": wants a <=15-line evidence-backed verdict with zero preamble.
    JustTheFacts,
    /// Prefers blunt, direct, unvarnished critique with zero softening.
    BluntDirect,
    /// Sensitive to tone; requires framing feedback around shared system goals rather than personal fault.
    ContextSensitive,
}

impl CommunicationArchetype {
    /// Recommended maximum summary lines for this communication archetype.
    #[must_use]
    pub const fn recommended_max_lines(self) -> usize {
        match self {
            Self::JustTheFacts | Self::DirectInstruction | Self::BluntDirect => 15,
            Self::AutonomousExplorer | Self::ValidationSeeker | Self::ContextSensitive => 25,
            Self::HyperverbalProcessor | Self::WrittenProcessor => 40,
        }
    }
}

/// Structural audit of a team or multi-agent wave against Wes Brown's operating practices.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TeamRhythmAndScaleAudit {
    /// Number of active direct reports or concurrent execution lanes under one lead/coordinator.
    pub active_lanes: usize,
    /// Whether the team respects the 5-6 engineer/lane hard ceiling.
    pub within_span_ceiling: bool,
    /// Whether synchronous exploratory meetings/chats obey the "Never Recorded" rule.
    pub never_recorded_compliant: bool,
    /// Fraction of work discussions and decisions occurring in public searchable channels
    /// (`#ml-dev` / shared XERJ indices) rather than private 1:1 DMs.
    pub public_channel_ratio: f64,
    /// Whether deadlines are self-estimated by the executor ("When can YOU do it?").
    pub self_driven_deadlines: bool,
    /// Whether upward status is generated from shipped artifacts (PRs, commits, receipts)
    /// rather than manual ticket-system maintenance ("Fuck Jira").
    pub artifact_derived_status: bool,
    /// Principle evaluation for the current workstream.
    pub principles: TopThreePrinciplesEval,
    /// Interoceptive / runtime state diagnosis.
    pub state: InteroceptiveStateEval,
}

impl TeamRhythmAndScaleAudit {
    /// Evaluate full compliance with Wes Brown's engineering leadership practices.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn evaluate(
        active_lanes: usize,
        synchronous_meetings_recorded: bool,
        public_channel_ratio: f64,
        self_driven_deadlines: bool,
        artifact_derived_status: bool,
        principles: TopThreePrinciplesEval,
        state: InteroceptiveStateEval,
    ) -> Self {
        let pub_ratio = public_channel_ratio.clamp(0.0, 1.0);
        Self {
            active_lanes,
            within_span_ceiling: active_lanes <= WES_BROWN_MAX_SPAN_OF_CONTROL,
            never_recorded_compliant: !synchronous_meetings_recorded,
            public_channel_ratio: pub_ratio,
            self_driven_deadlines,
            artifact_derived_status,
            principles,
            state,
        }
    }

    /// Return `true` when all structural practices, span-of-control bounds, state regulation,
    /// and the Top 3 Engineering Principles are satisfied.
    #[must_use]
    pub fn is_healthy(&self) -> bool {
        self.within_span_ceiling
            && self.never_recorded_compliant
            && self.public_channel_ratio >= 0.90
            && self.self_driven_deadlines
            && self.artifact_derived_status
            && self.principles.verdict == EngineeringPrincipleVerdict::ShipEightyTwenty
            && self.state.diagnosis == StateDiagnosis::Regulated
    }
}

/// Principle 3 of the 7-Principle Sales & Problem-Solving Canon (`xc-iopsych-corpus`:
/// `iopsych-sales-problem-solving-canon`):
/// "People buy 4 things and 4 things only. Ever. Those 4 things are time, money, sex,
/// and approval/peace of mind. If you try selling something other than those 4 things
/// you will fail."
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuyerDriver {
    /// Saves human or agent execution time (e.g., zero ingestion pipeline authoring, 1.5x-2.7x fewer turns).
    Time,
    /// Saves or generates money (e.g., lower compute/token spend, single static binary replacing a JVM cluster).
    Money,
    /// Elevates status, attraction, or prestige for the buyer.
    Sex,
    /// Delivers approval, certainty, compliance, and peace of mind (e.g., `ax_path:match_line` ground-truth citations).
    ApprovalPeaceOfMind,
}

impl BuyerDriver {
    /// Canonical snake_case wire label for this buyer driver.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Time => "time",
            Self::Money => "money",
            Self::Sex => "sex",
            Self::ApprovalPeaceOfMind => "approval_peace_of_mind",
        }
    }
}

/// Principle 4 of the 7-Principle Sales & Problem-Solving Canon:
/// "People buy aspirin always. They buy vitamins only occasionally and at unpredictable times. Sell aspirin."
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PainClass {
    /// Acute, immediate operational pain that buyers purchase predictably in every cycle.
    Aspirin,
    /// Optional nice-to-have improvement bought only occasionally and at unpredictable times.
    Vitamin,
}

impl PainClass {
    /// Canonical snake_case wire label for this pain classification.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Aspirin => "aspirin",
            Self::Vitamin => "vitamin",
        }
    }
}

/// Verdict from evaluating a product positioning, demo flow, or commercial offer against
/// the 7-Principle Sales & Problem-Solving Canon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SalesCanonVerdict {
    /// Satisfies all 7 principles: walks up and hits the ball, focuses on people and problem solving,
    /// anchors in the 4 buyer drivers, sells Aspirin, establishes product parity + friendship,
    /// practices unconditional value-first generosity, and leaks zero internal metrics.
    HitTheBallAspirin,
    /// Violates Principle 7: leaks internal quota, payroll, opex, or burn rate to the buyer.
    InternalMetricLeakage,
    /// Violates Principle 2: pitches technology, lines of code, or jargon instead of the human problem.
    TechJargonOverProblem,
    /// Violates Principle 3: fails to anchor in Time, Money, Sex, or Approval/Peace of Mind.
    MissingBuyerDriver,
    /// Violates Principle 4: pitches an optional Vitamin instead of acute Aspirin pain relief.
    VitaminPitch,
    /// Violates Principle 1: overcomplicates execution instead of walking up and hitting the ball.
    OvercomplicatedFunnel,
    /// Violates Principle 5 or 6: lacks baseline product parity ("make everything else equal")
    /// or gates help transactionally instead of practicing unconditional value-first generosity.
    TransactionalOrLowParity,
}

impl SalesCanonVerdict {
    /// Canonical snake_case wire label for this verdict.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::HitTheBallAspirin => "hit_the_ball_aspirin",
            Self::InternalMetricLeakage => "internal_metric_leakage",
            Self::TechJargonOverProblem => "tech_jargon_over_problem",
            Self::MissingBuyerDriver => "missing_buyer_driver",
            Self::VitaminPitch => "vitamin_pitch",
            Self::OvercomplicatedFunnel => "overcomplicated_funnel",
            Self::TransactionalOrLowParity => "transactional_or_low_parity",
        }
    }
}

/// Pure domain evaluation of a product surface, GTM motion, or customer interaction against
/// the 7-Principle Sales & Problem-Solving Canon (`xc-iopsych-corpus`: `iopsych-sales-problem-solving-canon`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SalesCanonEval {
    /// Principle 1 ("Walk up and hit the ball"): direct execution simplicity in `[0.0, 1.0]`.
    pub direct_execution_score: f64,
    /// Principle 2 ("People and problem solving, not lines of code"): human problem focus in `[0.0, 1.0]`.
    pub problem_over_tech_ratio: f64,
    /// Principle 3 ("People buy 4 things and 4 things only"): anchored buyer drivers.
    pub buyer_drivers: Vec<BuyerDriver>,
    /// Principle 4 ("Sell aspirin, not vitamins"): pain classification.
    pub pain_class: PainClass,
    /// Principle 5 ("Make everything else equal, then go make a lot of friends"):
    /// product parity/quality score in `[0.0, 1.0]`.
    pub product_parity_score: f64,
    /// Principle 5 & 6 ("Go make a lot of friends" + "Being valuable and useful expecting nothing in return"):
    /// authentic value-first generosity and relationship warmth in `[0.0, 1.0]`.
    pub value_first_generosity_score: f64,
    /// Principle 7 ("No one cares about your quota, payroll, opex, burn rate"):
    /// `true` if any internal company metric is leaked into the external pitch.
    pub leaks_internal_metrics: bool,
    /// Resulting 7-Principle Sales Canon verdict.
    pub verdict: SalesCanonVerdict,
}

impl SalesCanonEval {
    /// Evaluate a product pitch, demo, or commercial workflow against all 7 principles.
    #[must_use]
    pub fn evaluate(
        direct_execution_score: f64,
        problem_over_tech_ratio: f64,
        buyer_drivers: Vec<BuyerDriver>,
        pain_class: PainClass,
        product_parity_score: f64,
        value_first_generosity_score: f64,
        leaks_internal_metrics: bool,
    ) -> Self {
        let direct = direct_execution_score.clamp(0.0, 1.0);
        let problem = problem_over_tech_ratio.clamp(0.0, 1.0);
        let parity = product_parity_score.clamp(0.0, 1.0);
        let generosity = value_first_generosity_score.clamp(0.0, 1.0);

        let verdict = if leaks_internal_metrics {
            SalesCanonVerdict::InternalMetricLeakage
        } else if problem < 0.60 {
            SalesCanonVerdict::TechJargonOverProblem
        } else if buyer_drivers.is_empty() {
            SalesCanonVerdict::MissingBuyerDriver
        } else if pain_class == PainClass::Vitamin {
            SalesCanonVerdict::VitaminPitch
        } else if direct < 0.60 {
            SalesCanonVerdict::OvercomplicatedFunnel
        } else if parity < 0.75 || generosity < 0.70 {
            SalesCanonVerdict::TransactionalOrLowParity
        } else {
            SalesCanonVerdict::HitTheBallAspirin
        };

        Self {
            direct_execution_score: direct,
            problem_over_tech_ratio: problem,
            buyer_drivers,
            pain_class,
            product_parity_score: parity,
            value_first_generosity_score: generosity,
            leaks_internal_metrics,
            verdict,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_three_principles_reward_80_20_root_fix_and_flag_perfectionism_or_symptom_chasing() {
        let good = TopThreePrinciplesEval::evaluate(0.88, 0.25, 0.82);
        assert_eq!(good.verdict, EngineeringPrincipleVerdict::ShipEightyTwenty);
        assert!(good.net_leverage_ratio > 3.0);

        let perfectionism = TopThreePrinciplesEval::evaluate(0.85, 0.60, 0.98);
        assert_eq!(
            perfectionism.verdict,
            EngineeringPrincipleVerdict::PerfectionismTrap
        );

        let symptom = TopThreePrinciplesEval::evaluate(0.30, 0.20, 0.80);
        assert_eq!(
            symptom.verdict,
            EngineeringPrincipleVerdict::SymptomWorkaround
        );

        let high_burden = TopThreePrinciplesEval::evaluate(0.75, 0.72, 0.80);
        assert_eq!(
            high_burden.verdict,
            EngineeringPrincipleVerdict::ExcessiveMaintenanceBurden
        );
    }

    #[test]
    fn interoceptive_state_debugs_the_state_not_the_argument() {
        let regulated = InteroceptiveStateEval::evaluate(90, 0.55, 1);
        assert_eq!(regulated.diagnosis, StateDiagnosis::Regulated);

        let six_hours_no_break = InteroceptiveStateEval::evaluate(360, 0.40, 0);
        assert_eq!(
            six_hours_no_break.diagnosis,
            StateDiagnosis::InteroceptiveTimeoutRequired
        );

        let rss_circuit_breaker = InteroceptiveStateEval::evaluate(45, 0.94, 0);
        assert_eq!(
            rss_circuit_breaker.diagnosis,
            StateDiagnosis::InteroceptiveTimeoutRequired
        );

        let retry_spiral = InteroceptiveStateEval::evaluate(60, 0.50, 3);
        assert_eq!(
            retry_spiral.diagnosis,
            StateDiagnosis::InteroceptiveTimeoutRequired
        );
    }

    #[test]
    fn team_rhythm_and_scale_audit_enforces_span_ceiling_never_recorded_and_public_channel() {
        let principles = TopThreePrinciplesEval::evaluate(0.90, 0.22, 0.80);
        let state = InteroceptiveStateEval::evaluate(75, 0.48, 0);

        let healthy = TeamRhythmAndScaleAudit::evaluate(
            5,
            false,
            0.98,
            true,
            true,
            principles.clone(),
            state.clone(),
        );
        assert!(healthy.is_healthy());

        let overloaded_span = TeamRhythmAndScaleAudit::evaluate(
            8,
            false,
            0.98,
            true,
            true,
            principles.clone(),
            state.clone(),
        );
        assert!(!overloaded_span.within_span_ceiling);
        assert!(!overloaded_span.is_healthy());

        let recorded_meeting =
            TeamRhythmAndScaleAudit::evaluate(5, true, 0.98, true, true, principles, state);
        assert!(!recorded_meeting.never_recorded_compliant);
        assert!(!recorded_meeting.is_healthy());
    }

    #[test]
    fn sales_canon_enforces_seven_principles_aspirin_four_drivers_and_zero_metric_leakage() {
        let xerj_flagship = SalesCanonEval::evaluate(
            0.92,
            0.90,
            vec![
                BuyerDriver::Time,
                BuyerDriver::Money,
                BuyerDriver::ApprovalPeaceOfMind,
            ],
            PainClass::Aspirin,
            0.95,
            0.92,
            false,
        );
        assert_eq!(xerj_flagship.verdict, SalesCanonVerdict::HitTheBallAspirin);

        let leaked_quota = SalesCanonEval::evaluate(
            0.90,
            0.90,
            vec![BuyerDriver::Time],
            PainClass::Aspirin,
            0.90,
            0.90,
            true,
        );
        assert_eq!(
            leaked_quota.verdict,
            SalesCanonVerdict::InternalMetricLeakage
        );

        let vitamin_pitch = SalesCanonEval::evaluate(
            0.85,
            0.85,
            vec![BuyerDriver::Time],
            PainClass::Vitamin,
            0.90,
            0.85,
            false,
        );
        assert_eq!(vitamin_pitch.verdict, SalesCanonVerdict::VitaminPitch);

        let code_jargon = SalesCanonEval::evaluate(
            0.85,
            0.35,
            vec![BuyerDriver::Money],
            PainClass::Aspirin,
            0.90,
            0.85,
            false,
        );
        assert_eq!(
            code_jargon.verdict,
            SalesCanonVerdict::TechJargonOverProblem
        );
    }
}
