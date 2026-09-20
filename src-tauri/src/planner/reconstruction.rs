use std::cmp::Ordering;

use super::{
    ReconstructionCandidate, ReconstructionDecision, ReconstructionMetrics, ReconstructionViability,
};

pub fn parse_model_analyzer(
    output: &str,
    input_images: u64,
    model_count: u32,
) -> ReconstructionMetrics {
    let value = |label: &str| -> Option<String> {
        output.lines().find_map(|line| {
            let (_, tail) = line.split_once(label)?;
            Some(tail.trim().trim_end_matches("px").trim().to_owned())
        })
    };
    let registered_images = value("Registered images:")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let points_3d = value("Points:").and_then(|v| v.parse().ok()).unwrap_or(0);
    ReconstructionMetrics {
        input_images,
        registered_images,
        registered_ratio: if input_images == 0 {
            0.0
        } else {
            registered_images as f64 / input_images as f64
        },
        points_3d,
        observations: value("Observations:")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0),
        mean_track_length: value("Mean track length:").and_then(|v| v.parse().ok()),
        mean_observations_per_image: value("Mean observations per image:")
            .and_then(|v| v.parse().ok()),
        mean_reprojection_error: value("Mean reprojection error:").and_then(|v| v.parse().ok()),
        model_count,
        finite_geometry: registered_images > 0
            && registered_images <= input_images
            && points_3d > 0,
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PlannerReconstructionValidator;

impl PlannerReconstructionValidator {
    pub fn classify(
        &self,
        metrics: &ReconstructionMetrics,
    ) -> (ReconstructionDecision, ReconstructionViability) {
        if !metrics.finite_geometry || metrics.registered_images == 0 || metrics.points_3d == 0 {
            return (
                ReconstructionDecision::Critical,
                ReconstructionViability::NotViable,
            );
        }
        let reprojection = metrics.mean_reprojection_error.unwrap_or(2.5);
        let track = metrics.mean_track_length.unwrap_or(1.0);
        let geometry_good = reprojection <= 4.0 && track >= 1.5 && metrics.points_3d >= 100;
        let viable = if geometry_good && metrics.registered_images >= 12 {
            if metrics.registered_ratio >= 0.60 {
                ReconstructionViability::Viable
            } else {
                ReconstructionViability::DegradedButViable
            }
        } else {
            ReconstructionViability::NotViable
        };
        let decision = if metrics.registered_ratio >= 0.80 && reprojection <= 2.0 && track >= 2.0 {
            ReconstructionDecision::Pass
        } else if metrics.registered_ratio >= 0.60 && geometry_good {
            ReconstructionDecision::Warning
        } else if viable != ReconstructionViability::NotViable {
            ReconstructionDecision::NeedRescue
        } else {
            ReconstructionDecision::Critical
        };
        (decision, viable)
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ReconstructionComparator;

impl ReconstructionComparator {
    pub fn best<'a>(
        &self,
        candidates: &'a [ReconstructionCandidate],
    ) -> Option<&'a ReconstructionCandidate> {
        candidates
            .iter()
            .filter(|candidate| candidate.viability != ReconstructionViability::NotViable)
            .max_by(|left, right| compare(left, right))
    }
}

fn compare(left: &ReconstructionCandidate, right: &ReconstructionCandidate) -> Ordering {
    viability_rank(left.viability)
        .cmp(&viability_rank(right.viability))
        .then_with(|| decision_rank(left.decision).cmp(&decision_rank(right.decision)))
        .then_with(|| {
            let l = left
                .metrics
                .mean_reprojection_error
                .unwrap_or(f64::INFINITY);
            let r = right
                .metrics
                .mean_reprojection_error
                .unwrap_or(f64::INFINITY);
            r.partial_cmp(&l).unwrap_or(Ordering::Equal)
        })
        .then_with(|| {
            left.metrics
                .mean_track_length
                .unwrap_or(0.0)
                .partial_cmp(&right.metrics.mean_track_length.unwrap_or(0.0))
                .unwrap_or(Ordering::Equal)
        })
        .then_with(|| {
            left.metrics
                .registered_images
                .cmp(&right.metrics.registered_images)
        })
        .then_with(|| left.metrics.points_3d.cmp(&right.metrics.points_3d))
}

fn viability_rank(value: ReconstructionViability) -> u8 {
    match value {
        ReconstructionViability::NotViable => 0,
        ReconstructionViability::DegradedButViable => 1,
        ReconstructionViability::Viable => 2,
    }
}

fn decision_rank(value: ReconstructionDecision) -> u8 {
    match value {
        ReconstructionDecision::Critical => 0,
        ReconstructionDecision::NeedRescue => 1,
        ReconstructionDecision::Warning => 2,
        ReconstructionDecision::Pass => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planner::MapperBackend;

    fn candidate(id: &str, registered: u64, reprojection: f64) -> ReconstructionCandidate {
        let metrics = ReconstructionMetrics {
            input_images: 100,
            registered_images: registered,
            registered_ratio: registered as f64 / 100.0,
            points_3d: 1_000,
            mean_track_length: Some(3.0),
            mean_reprojection_error: Some(reprojection),
            finite_geometry: true,
            ..ReconstructionMetrics::default()
        };
        let (decision, viability) = PlannerReconstructionValidator.classify(&metrics);
        ReconstructionCandidate {
            id: id.into(),
            mapper: MapperBackend::Incremental,
            model_path: id.into(),
            metrics,
            decision,
            viability,
            rescue_round: 0,
        }
    }

    #[test]
    fn comparator_is_not_registration_only_or_last_wins() {
        let values = vec![candidate("good", 75, 0.8), candidate("more", 90, 8.0)];
        assert_eq!(ReconstructionComparator.best(&values).unwrap().id, "good");
    }

    #[test]
    fn below_sixty_percent_can_be_degraded_but_viable() {
        let value = candidate("degraded", 55, 1.0);
        assert_eq!(value.viability, ReconstructionViability::DegradedButViable);
        assert_eq!(value.decision, ReconstructionDecision::NeedRescue);
    }
}
