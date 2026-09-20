use crate::presets::MatchingBudget;

use super::{CapturePrior, CaptureType, PairingPlan, PairingStrategy};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EstimatedMatchingCost {
    pub image_count: u64,
    pub exhaustive_pairs: u64,
    pub sequential_pairs: u64,
}

impl EstimatedMatchingCost {
    pub fn new(image_count: u64, overlap: u32) -> Self {
        Self {
            image_count,
            exhaustive_pairs: image_count.saturating_mul(image_count.saturating_sub(1)) / 2,
            sequential_pairs: image_count.saturating_mul(overlap as u64),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PairingThresholds {
    pub exhaustive_pair_limit: u64,
    pub temporal_confidence: f32,
    pub loop_prior: f32,
}

impl Default for PairingThresholds {
    fn default() -> Self {
        Self {
            exhaustive_pair_limit: 12_000,
            temporal_confidence: 0.62,
            loop_prior: 0.48,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PairingPlanner {
    pub thresholds: PairingThresholds,
}

impl PairingPlanner {
    pub fn plan(
        &self,
        prior: &CapturePrior,
        image_count: u64,
        budget: MatchingBudget,
    ) -> PairingPlan {
        let cost = EstimatedMatchingCost::new(image_count, budget.sequential_overlap);
        let (strategy, reason) = if prior.capture_type == CaptureType::UnorderedPhotos
            && cost.exhaustive_pairs <= self.thresholds.exhaustive_pair_limit
        {
            (PairingStrategy::Exhaustive, "small_unordered_capture")
        } else if prior.temporal_order_confidence >= self.thresholds.temporal_confidence
            && prior.loop_prior >= self.thresholds.loop_prior
        {
            (
                PairingStrategy::SequentialWithLoopClosure,
                "ordered_capture_with_loop_prior",
            )
        } else if prior.temporal_order_confidence >= self.thresholds.temporal_confidence {
            (PairingStrategy::Sequential, "ordered_capture")
        } else if cost.exhaustive_pairs > self.thresholds.exhaustive_pair_limit {
            (PairingStrategy::Prefilter, "expensive_pair_space")
        } else {
            (PairingStrategy::Exhaustive, "affordable_pair_space")
        };
        let estimated_pairs = match strategy {
            PairingStrategy::Exhaustive => cost.exhaustive_pairs,
            PairingStrategy::Sequential | PairingStrategy::SequentialWithLoopClosure => {
                cost.sequential_pairs
            }
            PairingStrategy::Prefilter => {
                image_count.saturating_mul(budget.prefilter_neighbors as u64)
            }
        };
        PairingPlan {
            strategy,
            sequential_overlap: budget.sequential_overlap,
            prefilter_neighbors: budget.prefilter_neighbors,
            estimated_pairs,
            reason_codes: vec![reason.into()],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planner::CaptureAnalyzer;

    fn budget() -> MatchingBudget {
        MatchingBudget {
            sequential_overlap: 15,
            prefilter_neighbors: 24,
        }
    }

    #[test]
    fn quality_budget_does_not_hardcode_strategy() {
        let unordered = CaptureAnalyzer.unordered_images(30);
        assert_eq!(
            PairingPlanner::default()
                .plan(&unordered, 30, budget())
                .strategy,
            PairingStrategy::Exhaustive
        );
        let expensive = CaptureAnalyzer.unordered_images(1_000);
        assert_eq!(
            PairingPlanner::default()
                .plan(&expensive, 1_000, budget())
                .strategy,
            PairingStrategy::Prefilter
        );
    }
}
