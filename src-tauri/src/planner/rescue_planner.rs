use crate::presets::RescueBudget;

use super::{
    GraphDecision, MapperBackend, PlannerRecoveryMode, ReconstructionDecision, RescueAction,
    SuccessRecoveryPolicy, ViewGraphReport,
};

#[derive(Debug, Clone, Copy, Default)]
pub struct RescuePlanner;

impl RescuePlanner {
    pub fn next_normal(
        &self,
        round: u32,
        budget: RescueBudget,
        graph: &ViewGraphReport,
        decision: GraphDecision,
    ) -> Option<RescueAction> {
        if round >= budget.max_rounds {
            return None;
        }
        if budget.allow_frame_backfill
            && (!graph.weak_runs.is_empty() || decision == GraphDecision::NeedRescue)
        {
            Some(RescueAction::FrameBackfill)
        } else if budget.allow_local_exhaustive {
            Some(RescueAction::LocalExhaustive)
        } else {
            Some(RescueAction::ExpandSequential)
        }
    }

    pub fn next_recovery(
        &self,
        round: u32,
        policy: SuccessRecoveryPolicy,
        current_mapper: MapperBackend,
        last_decision: ReconstructionDecision,
    ) -> Option<RescueAction> {
        if round >= policy.max_rounds {
            return None;
        }
        let action = match round {
            0 if policy.allow_pairing_escalation_beyond_quality => RescueAction::LocalExhaustive,
            1 if policy.allow_sfm_escalation_beyond_quality => RescueAction::SfmEscalation,
            2 if policy.allow_alternate_mapper => RescueAction::AlternateMapper,
            3 if policy.allow_frame_backfill_beyond_quality => RescueAction::FrameBackfill,
            _ if policy.allow_legacy_safe_fallback
                && last_decision == ReconstructionDecision::Critical =>
            {
                RescueAction::LegacySafeFallback
            }
            _ => return None,
        };
        let _ = current_mapper;
        Some(action)
    }

    pub const fn mode(normal_budget_exhausted: bool) -> PlannerRecoveryMode {
        if normal_budget_exhausted {
            PlannerRecoveryMode::SuccessRecovery
        } else {
            PlannerRecoveryMode::Normal
        }
    }
}
