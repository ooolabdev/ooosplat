use serde::Serialize;

use crate::{
    presets::Quality,
    video::{FramePlan, VideoInfo},
};

#[derive(Debug, Clone)]
pub struct RuntimeSample {
    pub quality: Quality,
    pub extracted_frames: u64,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EstimateConfidence {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeEstimate {
    pub estimated_ms: u64,
    pub lower_bound_ms: u64,
    pub upper_bound_ms: u64,
    pub confidence: EstimateConfidence,
    pub sample_count: usize,
    pub basis: String,
}

pub fn estimate_runtime(
    video: &VideoInfo,
    plan: &FramePlan,
    quality: Quality,
    samples: &[RuntimeSample],
) -> RuntimeEstimate {
    let base = base_estimate_ms(plan.estimated_frames, quality);
    let valid_samples = samples
        .iter()
        .filter(|sample| sample.duration_ms >= 10_000 && sample.extracted_frames > 0)
        .collect::<Vec<_>>();
    let same_quality = valid_samples
        .iter()
        .copied()
        .filter(|sample| sample.quality == quality)
        .collect::<Vec<_>>();
    let nearby_same_quality = same_quality
        .iter()
        .copied()
        .filter(|sample| {
            let smaller = sample.extracted_frames.min(plan.estimated_frames).max(1) as f64;
            let larger = sample.extracted_frames.max(plan.estimated_frames).max(1) as f64;
            larger / smaller <= 1.25
        })
        .collect::<Vec<_>>();
    let (calibration_source, calibration_label) = if !nearby_same_quality.is_empty() {
        (nearby_same_quality, "同档位、相近帧数")
    } else if !same_quality.is_empty() {
        (same_quality, "同档位")
    } else {
        (valid_samples, "跨档位")
    };
    let mut calibration = calibration_source
        .into_iter()
        .map(|sample| {
            let expected = base_estimate_ms(sample.extracted_frames, sample.quality);
            (sample.duration_ms as f64 / expected.max(1) as f64).clamp(0.15, 5.0)
        })
        .collect::<Vec<_>>();
    calibration.sort_by(f64::total_cmp);
    let sample_count = calibration.len();
    let factor = median(&calibration).unwrap_or(1.0);
    let estimated_ms = (base as f64 * factor).round().max(1_000.0) as u64;
    let (confidence, lower_factor, upper_factor) = match sample_count {
        0 => (EstimateConfidence::Low, 0.55, 1.75),
        1..=2 => (EstimateConfidence::Low, 0.60, 1.60),
        3..=5 => (EstimateConfidence::Medium, 0.72, 1.38),
        _ => (EstimateConfidence::High, 0.82, 1.22),
    };
    RuntimeEstimate {
        estimated_ms,
        lower_bound_ms: (estimated_ms as f64 * lower_factor).round() as u64,
        upper_bound_ms: (estimated_ms as f64 * upper_factor).round() as u64,
        confidence,
        sample_count,
        basis: if sample_count == 0 {
            format!(
                "根据输入 {} 总帧、预计处理 {} 帧和质量档位估算；完成任务后会自动校准",
                video.total_frames, plan.estimated_frames
            )
        } else {
            format!(
                "根据输入 {} 总帧、预计处理 {} 帧、质量档位和本机 {sample_count} 个{calibration_label}任务校准",
                video.total_frames, plan.estimated_frames,
            )
        },
    }
}

fn base_estimate_ms(frames: u64, quality: Quality) -> u64 {
    let frame_count = frames.max(1) as f64;

    // Feature work grows roughly linearly with frames and pixels, while camera
    // reconstruction and bundle adjustment grow faster as more views are added.
    let preparation_ms = 8_000.0 + frame_count * 55.0;
    let reconstruction_ms = 176.0 * frame_count.powf(1.5);
    let brush_ms = estimate_brush_stage_ms(quality) as f64;
    (preparation_ms + reconstruction_ms + brush_ms).round() as u64
}

/// Brush v0.3.0 does not expose its current training step on stdout/stderr.
/// This duration model is therefore used only to provide a clearly labelled,
/// best-effort progress indicator while the process is alive.
pub(crate) fn estimate_brush_stage_ms(quality: Quality) -> u64 {
    let preset = quality.preset();
    let resolution_factor = (preset.brush_max_resolution as f64 / 960.0).powf(1.35);
    let iteration_factor = preset.brush_iterations as f64 / 6_000.0;
    (80_000.0 * resolution_factor * iteration_factor)
        .round()
        .max(1_000.0) as u64
}

pub(crate) fn estimate_calibrated_brush_stage_ms(
    video: &VideoInfo,
    plan: &FramePlan,
    quality: Quality,
    samples: &[RuntimeSample],
) -> u64 {
    let base_total_ms = base_estimate_ms(plan.estimated_frames, quality).max(1);
    let calibrated_total_ms = estimate_runtime(video, plan, quality, samples).estimated_ms;
    let calibration = calibrated_total_ms as f64 / base_total_ms as f64;
    (estimate_brush_stage_ms(quality) as f64 * calibration)
        .round()
        .max(1_000.0) as u64
}

fn median(values: &[f64]) -> Option<f64> {
    match values.len() {
        0 => None,
        length if length % 2 == 1 => Some(values[length / 2]),
        length => Some((values[length / 2 - 1] + values[length / 2]) / 2.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn video() -> VideoInfo {
        VideoInfo {
            duration: 12.52,
            width: 3840,
            height: 2160,
            fps: 60.0,
            total_frames: 752,
            codec: "hevc".into(),
            rotation: 0,
            pixel_format: "yuv420p".into(),
            has_alpha: false,
        }
    }

    #[test]
    fn quality_and_frame_count_increase_the_estimate() {
        let fast = base_estimate_ms(48, Quality::Fast);
        let balanced = base_estimate_ms(72, Quality::Balanced);
        let high = base_estimate_ms(120, Quality::High);
        assert!(fast < balanced && balanced < high);
    }

    #[test]
    fn brush_stage_estimate_increases_with_the_quality_preset() {
        assert!(
            estimate_brush_stage_ms(Quality::Fast) < estimate_brush_stage_ms(Quality::Balanced)
        );
        assert!(
            estimate_brush_stage_ms(Quality::Balanced) < estimate_brush_stage_ms(Quality::High)
        );
    }

    #[test]
    fn brush_stage_estimate_uses_the_same_local_history_calibration() {
        let video = video();
        let plan = FramePlan {
            retention_ratio: 0.5,
            sampling_fps: 30.0,
            estimated_frames: 533,
        };
        let base_total = base_estimate_ms(plan.estimated_frames, Quality::Balanced);
        let sample = RuntimeSample {
            quality: Quality::Balanced,
            extracted_frames: plan.estimated_frames,
            duration_ms: base_total * 2,
        };

        let calibrated =
            estimate_calibrated_brush_stage_ms(&video, &plan, Quality::Balanced, &[sample]);
        assert_eq!(calibrated, estimate_brush_stage_ms(Quality::Balanced) * 2);
    }

    #[test]
    fn completed_local_runs_calibrate_and_narrow_the_range() {
        let video = video();
        let plan = FramePlan {
            retention_ratio: 0.064,
            sampling_fps: 3.83,
            estimated_frames: 48,
        };
        let sample = RuntimeSample {
            quality: Quality::Fast,
            extracted_frames: 226,
            duration_ms: 858_613,
        };
        let estimate = estimate_runtime(
            &video,
            &plan,
            Quality::Fast,
            &[sample.clone(), sample.clone(), sample],
        );
        assert_eq!(estimate.confidence, EstimateConfidence::Medium);
        assert_eq!(estimate.sample_count, 3);
        assert!(estimate.lower_bound_ms < estimate.estimated_ms);
        assert!(estimate.upper_bound_ms > estimate.estimated_ms);
    }

    #[test]
    fn same_quality_samples_take_priority_and_use_the_median() {
        let video = video();
        let plan = FramePlan {
            retention_ratio: 0.5,
            sampling_fps: 30.0,
            estimated_frames: 533,
        };
        let samples = [
            RuntimeSample {
                quality: Quality::Balanced,
                extracted_frames: 533,
                duration_ms: 3_954_000,
            },
            RuntimeSample {
                quality: Quality::Fast,
                extracted_frames: 320,
                duration_ms: 374_000,
            },
            RuntimeSample {
                quality: Quality::High,
                extracted_frames: 416,
                duration_ms: 10_464_000,
            },
        ];
        let estimate = estimate_runtime(&video, &plan, Quality::Balanced, &samples);
        assert_eq!(estimate.sample_count, 1);
        assert_eq!(estimate.estimated_ms, 3_954_000);
        assert!(estimate.basis.contains("1 个同档位、相近帧数任务"));
    }

    #[test]
    fn nearby_frame_counts_do_not_mix_unrelated_runs() {
        let video = video();
        let plan = FramePlan {
            retention_ratio: 0.3,
            sampling_fps: 9.0,
            estimated_frames: 320,
        };
        let samples = [
            RuntimeSample {
                quality: Quality::Fast,
                extracted_frames: 320,
                duration_ms: 840_000,
            },
            RuntimeSample {
                quality: Quality::Fast,
                extracted_frames: 320,
                duration_ms: 960_000,
            },
            RuntimeSample {
                quality: Quality::Fast,
                extracted_frames: 506,
                duration_ms: 374_000,
            },
        ];
        let estimate = estimate_runtime(&video, &plan, Quality::Fast, &samples);
        assert_eq!(estimate.sample_count, 2);
        assert_eq!(estimate.estimated_ms, 900_000);
        assert!(estimate.basis.contains("相近帧数"));
    }
}
