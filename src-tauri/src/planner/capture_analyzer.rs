use super::{CapturePrior, CaptureType, FrameCandidate, MotionLevel};

#[derive(Debug, Clone, Default)]
pub struct CaptureAnalyzer;

impl CaptureAnalyzer {
    pub fn analyze_video(&self, candidates: &[FrameCandidate]) -> CapturePrior {
        if candidates.is_empty() {
            return CapturePrior::default();
        }
        let motions: Vec<f32> = candidates.iter().map(|value| value.motion_score).collect();
        let exposures: Vec<f32> = candidates
            .iter()
            .map(|value| value.exposure_score)
            .collect();
        let mean_motion = mean(&motions);
        let motion_variance = variance(&motions, mean_motion);
        let exposure_variance = variance(&exposures, mean(&exposures));
        let blur_level = 1.0
            - candidates
                .iter()
                .map(|value| value.sharpness_score)
                .sum::<f32>()
                / candidates.len() as f32;
        let endpoints = candidates
            .first()
            .zip(candidates.last())
            .map(|(first, last)| {
                1.0 - (first.exposure_score - last.exposure_score)
                    .abs()
                    .clamp(0.0, 1.0)
            })
            .unwrap_or(0.0);
        let loop_prior = (endpoints * 0.45
            + (1.0 - motion_variance.clamp(0.0, 1.0)) * 0.25
            + candidates
                .last()
                .map(|value| value.view_change_score)
                .unwrap_or(0.0)
                * 0.30)
            .clamp(0.0, 1.0);
        let motion_level = if mean_motion < 0.25 {
            MotionLevel::Low
        } else if mean_motion > 0.62 {
            MotionLevel::High
        } else {
            MotionLevel::Medium
        };
        let capture_type = if loop_prior > 0.68 {
            CaptureType::ObjectOrbit
        } else if mean_motion < 0.18 && motion_variance < 0.08 {
            CaptureType::Turntable
        } else {
            CaptureType::SceneWalkthrough
        };
        CapturePrior {
            capture_type,
            temporal_order_confidence: 0.95,
            loop_prior,
            motion_level,
            motion_variance,
            blur_level: blur_level.clamp(0.0, 1.0),
            exposure_variance,
            confidence: (0.45 + candidates.len().min(100) as f32 / 200.0).min(0.95),
        }
    }

    pub fn unordered_images(&self, image_count: u64) -> CapturePrior {
        CapturePrior {
            capture_type: CaptureType::UnorderedPhotos,
            temporal_order_confidence: 0.0,
            confidence: if image_count >= 3 { 0.9 } else { 0.4 },
            ..CapturePrior::default()
        }
    }
}

fn mean(values: &[f32]) -> f32 {
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f32>() / values.len() as f32
    }
}

fn variance(values: &[f32], mean: f32) -> f32 {
    if values.is_empty() {
        0.0
    } else {
        values
            .iter()
            .map(|value| (value - mean).powi(2))
            .sum::<f32>()
            / values.len() as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unordered_images_are_only_a_weak_prior() {
        let prior = CaptureAnalyzer.unordered_images(20);
        assert_eq!(prior.capture_type, CaptureType::UnorderedPhotos);
        assert_eq!(prior.temporal_order_confidence, 0.0);
    }
}
