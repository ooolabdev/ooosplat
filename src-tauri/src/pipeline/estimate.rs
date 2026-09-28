use serde::Serialize;

use crate::{
    presets::{BrushTrainingPreset, BrushTrainingProfile, Quality, ResolvedBrushTrainingPreset},
    video::{FramePlan, VideoInfo},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeInputKind {
    Video,
    Images,
}

#[derive(Debug, Clone)]
pub struct RuntimeSample {
    pub quality: Quality,
    pub input_kind: RuntimeInputKind,
    pub source_long_edge: u32,
    pub working_long_edge: Option<u32>,
    pub resolution_policy_version: Option<u32>,
    pub extracted_frames: u64,
    pub duration_ms: u64,
    pub brush: Option<ResolvedBrushTrainingPreset>,
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
    estimate_runtime_with_brush(video, plan, quality, samples, None)
}

pub fn estimate_runtime_with_brush(
    video: &VideoInfo,
    plan: &FramePlan,
    quality: Quality,
    samples: &[RuntimeSample],
    brush: Option<&ResolvedBrushTrainingPreset>,
) -> RuntimeEstimate {
    estimate_runtime_with_brush_and_resolution(
        video,
        plan,
        quality,
        samples,
        brush,
        None,
        video.width.max(video.height),
    )
}

pub fn estimate_runtime_with_brush_and_resolution(
    video: &VideoInfo,
    plan: &FramePlan,
    quality: Quality,
    samples: &[RuntimeSample],
    brush: Option<&ResolvedBrushTrainingPreset>,
    resolution_policy_version: Option<u32>,
    working_long_edge: u32,
) -> RuntimeEstimate {
    estimate_runtime_for_input(
        video.total_frames,
        plan,
        quality,
        samples,
        RuntimeInputKind::Video,
        working_long_edge,
        brush,
        resolution_policy_version,
    )
}

pub fn estimate_runtime_for_images(
    image_count: u64,
    source_long_edge: u32,
    plan: &FramePlan,
    quality: Quality,
    samples: &[RuntimeSample],
) -> RuntimeEstimate {
    estimate_runtime_for_images_with_brush(
        image_count,
        source_long_edge,
        plan,
        quality,
        samples,
        None,
    )
}

pub fn estimate_runtime_for_images_with_brush(
    image_count: u64,
    source_long_edge: u32,
    plan: &FramePlan,
    quality: Quality,
    samples: &[RuntimeSample],
    brush: Option<&ResolvedBrushTrainingPreset>,
) -> RuntimeEstimate {
    estimate_runtime_for_images_with_brush_and_resolution(
        image_count,
        source_long_edge,
        plan,
        quality,
        samples,
        brush,
        None,
        source_long_edge,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn estimate_runtime_for_images_with_brush_and_resolution(
    image_count: u64,
    _source_long_edge: u32,
    plan: &FramePlan,
    quality: Quality,
    samples: &[RuntimeSample],
    brush: Option<&ResolvedBrushTrainingPreset>,
    resolution_policy_version: Option<u32>,
    working_long_edge: u32,
) -> RuntimeEstimate {
    let mut estimate = estimate_runtime_for_input(
        image_count,
        plan,
        quality,
        samples,
        RuntimeInputKind::Images,
        working_long_edge,
        brush,
        resolution_policy_version,
    );
    estimate.basis = if estimate.sample_count == 0 {
        format!("根据 {image_count} 张输入图片和质量档位估算；完成任务后会自动校准")
    } else {
        format!(
            "根据 {image_count} 张输入图片、质量档位和本机 {} 个历史任务校准",
            estimate.sample_count
        )
    };
    estimate
}

#[allow(clippy::too_many_arguments)]
fn estimate_runtime_for_input(
    source_count: u64,
    plan: &FramePlan,
    quality: Quality,
    samples: &[RuntimeSample],
    input_kind: RuntimeInputKind,
    source_long_edge: u32,
    brush: Option<&ResolvedBrushTrainingPreset>,
    resolution_policy_version: Option<u32>,
) -> RuntimeEstimate {
    let base = base_estimate_ms_with_brush(
        plan.estimated_frames,
        quality,
        input_kind,
        source_long_edge,
        brush,
    );
    let valid_samples = samples
        .iter()
        .filter(|sample| {
            sample.duration_ms >= 10_000
                && sample.extracted_frames > 0
                && sample.input_kind == input_kind
        })
        .collect::<Vec<_>>();
    let same_quality = valid_samples
        .iter()
        .copied()
        .filter(|sample| sample.quality == quality)
        .collect::<Vec<_>>();
    let same_profile = brush
        .map(|current| {
            same_quality
                .iter()
                .copied()
                .filter(|sample| {
                    sample
                        .brush
                        .as_ref()
                        .is_some_and(|saved| saved.profile == current.profile)
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let same_profile_and_policy = same_profile
        .iter()
        .copied()
        .filter(|sample| sample.resolution_policy_version == resolution_policy_version)
        .collect::<Vec<_>>();
    let profile_calibration_required =
        brush.is_some_and(|current| current.profile != BrushTrainingProfile::Legacy);
    let exact_profile_calibration =
        !profile_calibration_required || !same_profile_and_policy.is_empty();
    let same_quality = if profile_calibration_required {
        if same_profile_and_policy.is_empty() {
            same_profile
        } else {
            same_profile_and_policy
        }
    } else if same_profile.is_empty() {
        same_quality
    } else {
        same_profile
    };
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
            let expected = base_estimate_ms_with_brush(
                sample.extracted_frames,
                sample.quality,
                sample.input_kind,
                sample.working_long_edge.unwrap_or(sample.source_long_edge),
                sample.brush.as_ref(),
            );
            (sample.duration_ms as f64 / expected.max(1) as f64).clamp(0.15, 5.0)
        })
        .collect::<Vec<_>>();
    calibration.sort_by(f64::total_cmp);
    let sample_count = calibration.len();
    let factor = median(&calibration).unwrap_or(1.0);
    let estimated_ms = (base as f64 * factor).round().max(1_000.0) as u64;
    let (mut confidence, lower_factor, upper_factor) = match sample_count {
        0 => (EstimateConfidence::Low, 0.55, 1.75),
        1..=2 => (EstimateConfidence::Low, 0.60, 1.60),
        3..=5 => (EstimateConfidence::Medium, 0.72, 1.38),
        _ => (EstimateConfidence::High, 0.82, 1.22),
    };
    if !exact_profile_calibration {
        confidence = EstimateConfidence::Low;
    }
    RuntimeEstimate {
        estimated_ms,
        lower_bound_ms: (estimated_ms as f64 * lower_factor).round() as u64,
        upper_bound_ms: (estimated_ms as f64 * upper_factor).round() as u64,
        confidence,
        sample_count,
        basis: if sample_count == 0 {
            format!(
                "根据输入 {} 总帧、预计处理 {} 帧和质量档位估算；完成任务后会自动校准",
                source_count, plan.estimated_frames
            )
        } else {
            format!(
                "根据输入 {} 总帧、预计处理 {} 帧、质量档位和本机 {sample_count} 个{calibration_label}任务校准",
                source_count, plan.estimated_frames,
            )
        },
    }
}

fn base_estimate_ms_with_brush(
    frames: u64,
    quality: Quality,
    input_kind: RuntimeInputKind,
    source_long_edge: u32,
    brush: Option<&ResolvedBrushTrainingPreset>,
) -> u64 {
    let frame_count = frames.max(1) as f64;
    let planner_v2_profile =
        brush.is_some_and(|resolved| resolved.profile != BrushTrainingProfile::Legacy);
    let non_brush_ms = if planner_v2_profile {
        estimate_non_brush_ms(frame_count, quality, input_kind)
    } else {
        // Preserve the legacy/Planner-disabled estimate model.
        8_000.0 + frame_count * 55.0 + 176.0 * frame_count.powf(1.5)
    };
    let brush_ms = brush
        .map(|resolved| estimate_brush_stage_ms_for_resolved(resolved, source_long_edge))
        .unwrap_or_else(|| estimate_brush_stage_ms(quality)) as f64;
    (non_brush_ms + brush_ms).round() as u64
}

fn estimate_non_brush_ms(frame_count: f64, quality: Quality, input_kind: RuntimeInputKind) -> f64 {
    // Benchmark anchors collected on 2026-09-27. They include material
    // preparation, feature extraction, matching and Incremental Mapper, but
    // exclude Brush. Scaling remains deliberately conservative because scene
    // connectivity can dominate Mapper time even at the same frame count.
    let (anchor_frames, anchor_ms, exponent): (f64, f64, f64) = match (input_kind, quality) {
        (RuntimeInputKind::Video, Quality::Fast) => (251.0, 122_434.0, 1.35),
        (RuntimeInputKind::Video, Quality::Balanced) => (285.0, 553_811.0, 1.35),
        (RuntimeInputKind::Video, Quality::High) => (674.0, 600_916.0, 1.35),
        (RuntimeInputKind::Images, Quality::Fast) => (90.0, 437_578.0, 1.25),
        // Balanced has no same-material image benchmark yet, so interpolate
        // between the measured Fast and High non-Brush costs.
        (RuntimeInputKind::Images, Quality::Balanced) => (90.0, 740_000.0, 1.25),
        (RuntimeInputKind::Images, Quality::High) => (90.0, 1_143_657.0, 1.25),
    };
    anchor_ms * (frame_count / anchor_frames).powf(exponent)
}

/// Brush v0.3.0 does not expose its current training step on stdout/stderr.
/// This duration model is therefore used only to provide a clearly labelled,
/// best-effort progress indicator while the process is alive.
pub(crate) fn estimate_brush_stage_ms(quality: Quality) -> u64 {
    let preset = quality.preset();
    estimate_brush_stage_ms_for_preset(&BrushTrainingPreset {
        total_steps: preset.brush_iterations,
        max_resolution: preset.brush_max_resolution,
        refine_every: 200,
        max_splats: None,
        densification: None,
    })
}

fn estimate_brush_stage_ms_for_preset(preset: &BrushTrainingPreset) -> u64 {
    let resolution_factor = (preset.max_resolution as f64 / 960.0).powf(1.35);
    let iteration_factor = preset.total_steps as f64 / 6_000.0;
    (80_000.0 * resolution_factor * iteration_factor)
        .round()
        .max(1_000.0) as u64
}

fn estimate_brush_stage_ms_for_resolved(
    resolved: &ResolvedBrushTrainingPreset,
    source_long_edge: u32,
) -> u64 {
    let effective_resolution = resolved.preset.max_resolution.min(source_long_edge.max(1)) as f64;
    let estimate = match resolved.profile {
        BrushTrainingProfile::Fast => 138_530.0 * (effective_resolution / 1_200.0).powf(1.7),
        BrushTrainingProfile::Balanced => 621_603.0 * (effective_resolution / 1_600.0).powf(1.7),
        BrushTrainingProfile::HighStandard | BrushTrainingProfile::HighLarge => {
            // Fits both measured High points: 1,212.408 s at an effective
            // 1920 edge and 7,544.882 s at 3840 with the aggressive profile.
            1_212_408.0 * (effective_resolution / 1_920.0).powf(2.638)
        }
        BrushTrainingProfile::HighLow => 1_500_000.0 * (effective_resolution / 2_000.0).powf(2.0),
        BrushTrainingProfile::HighEmergency => {
            1_500_000.0 * (effective_resolution / 2_000.0).powf(1.8)
        }
        BrushTrainingProfile::Legacy => {
            return estimate_brush_stage_ms_for_preset(&resolved.preset);
        }
    };
    estimate.round().max(1_000.0) as u64
}

pub(crate) fn estimate_calibrated_brush_stage_ms(
    video: &VideoInfo,
    plan: &FramePlan,
    quality: Quality,
    samples: &[RuntimeSample],
    brush: Option<&ResolvedBrushTrainingPreset>,
    resolution_policy_version: Option<u32>,
    working_long_edge: u32,
) -> u64 {
    let source_long_edge = working_long_edge.max(1);
    let base_total_ms = base_estimate_ms_with_brush(
        plan.estimated_frames,
        quality,
        RuntimeInputKind::Video,
        source_long_edge,
        brush,
    )
    .max(1);
    let calibrated_total_ms = estimate_runtime_with_brush_and_resolution(
        video,
        plan,
        quality,
        samples,
        brush,
        resolution_policy_version,
        working_long_edge,
    )
    .estimated_ms;
    let calibration = calibrated_total_ms as f64 / base_total_ms as f64;
    let brush_ms = brush
        .map(|resolved| estimate_brush_stage_ms_for_resolved(resolved, source_long_edge))
        .unwrap_or_else(|| estimate_brush_stage_ms(quality));
    (brush_ms as f64 * calibration).round().max(1_000.0) as u64
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn estimate_calibrated_brush_stage_ms_for_images(
    image_count: u64,
    source_long_edge: u32,
    plan: &FramePlan,
    quality: Quality,
    samples: &[RuntimeSample],
    brush: Option<&ResolvedBrushTrainingPreset>,
    resolution_policy_version: Option<u32>,
    working_long_edge: u32,
) -> u64 {
    let base_total_ms = base_estimate_ms_with_brush(
        plan.estimated_frames,
        quality,
        RuntimeInputKind::Images,
        working_long_edge,
        brush,
    )
    .max(1);
    let calibrated_total_ms = estimate_runtime_for_images_with_brush_and_resolution(
        image_count,
        source_long_edge,
        plan,
        quality,
        samples,
        brush,
        resolution_policy_version,
        working_long_edge,
    )
    .estimated_ms;
    let calibration = calibrated_total_ms as f64 / base_total_ms as f64;
    let brush_ms = brush
        .map(|resolved| estimate_brush_stage_ms_for_resolved(resolved, working_long_edge))
        .unwrap_or_else(|| estimate_brush_stage_ms(quality));
    (brush_ms as f64 * calibration).round().max(1_000.0) as u64
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
        let fast =
            base_estimate_ms_with_brush(48, Quality::Fast, RuntimeInputKind::Video, 3_840, None);
        let balanced = base_estimate_ms_with_brush(
            72,
            Quality::Balanced,
            RuntimeInputKind::Video,
            3_840,
            None,
        );
        let high =
            base_estimate_ms_with_brush(120, Quality::High, RuntimeInputKind::Video, 3_840, None);
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
            ..FramePlan::default()
        };
        let base_total = base_estimate_ms_with_brush(
            plan.estimated_frames,
            Quality::Balanced,
            RuntimeInputKind::Video,
            3_840,
            None,
        );
        let sample = RuntimeSample {
            quality: Quality::Balanced,
            input_kind: RuntimeInputKind::Video,
            source_long_edge: 3_840,
            working_long_edge: None,
            resolution_policy_version: None,
            extracted_frames: plan.estimated_frames,
            duration_ms: base_total * 2,
            brush: None,
        };

        let calibrated = estimate_calibrated_brush_stage_ms(
            &video,
            &plan,
            Quality::Balanced,
            &[sample],
            None,
            None,
            3_840,
        );
        assert_eq!(calibrated, estimate_brush_stage_ms(Quality::Balanced) * 2);
    }

    #[test]
    fn completed_local_runs_calibrate_and_narrow_the_range() {
        let video = video();
        let plan = FramePlan {
            retention_ratio: 0.064,
            sampling_fps: 3.83,
            estimated_frames: 48,
            ..FramePlan::default()
        };
        let sample = RuntimeSample {
            quality: Quality::Fast,
            input_kind: RuntimeInputKind::Video,
            source_long_edge: 3_840,
            working_long_edge: None,
            resolution_policy_version: None,
            extracted_frames: 226,
            duration_ms: 858_613,
            brush: None,
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
            ..FramePlan::default()
        };
        let samples = [
            RuntimeSample {
                quality: Quality::Balanced,
                input_kind: RuntimeInputKind::Video,
                source_long_edge: 3_840,
                working_long_edge: None,
                resolution_policy_version: None,
                extracted_frames: 533,
                duration_ms: 3_954_000,
                brush: None,
            },
            RuntimeSample {
                quality: Quality::Fast,
                input_kind: RuntimeInputKind::Video,
                source_long_edge: 3_840,
                working_long_edge: None,
                resolution_policy_version: None,
                extracted_frames: 320,
                duration_ms: 374_000,
                brush: None,
            },
            RuntimeSample {
                quality: Quality::High,
                input_kind: RuntimeInputKind::Video,
                source_long_edge: 3_840,
                working_long_edge: None,
                resolution_policy_version: None,
                extracted_frames: 416,
                duration_ms: 10_464_000,
                brush: None,
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
            ..FramePlan::default()
        };
        let samples = [
            RuntimeSample {
                quality: Quality::Fast,
                input_kind: RuntimeInputKind::Video,
                source_long_edge: 3_840,
                working_long_edge: None,
                resolution_policy_version: None,
                extracted_frames: 320,
                duration_ms: 840_000,
                brush: None,
            },
            RuntimeSample {
                quality: Quality::Fast,
                input_kind: RuntimeInputKind::Video,
                source_long_edge: 3_840,
                working_long_edge: None,
                resolution_policy_version: None,
                extracted_frames: 320,
                duration_ms: 960_000,
                brush: None,
            },
            RuntimeSample {
                quality: Quality::Fast,
                input_kind: RuntimeInputKind::Video,
                source_long_edge: 3_840,
                working_long_edge: None,
                resolution_policy_version: None,
                extracted_frames: 506,
                duration_ms: 374_000,
                brush: None,
            },
        ];
        let estimate = estimate_runtime(&video, &plan, Quality::Fast, &samples);
        assert_eq!(estimate.sample_count, 2);
        assert_eq!(estimate.estimated_ms, 900_000);
        assert!(estimate.basis.contains("相近帧数"));
    }

    #[test]
    fn incompatible_high_profile_history_stays_low_confidence() {
        let video = video();
        let plan = FramePlan {
            retention_ratio: 0.4,
            sampling_fps: 12.0,
            estimated_frames: 150,
            ..FramePlan::default()
        };
        let current = crate::presets::resolve_brush_training_preset(
            Quality::High,
            true,
            Some(8_192),
            3_840,
            60_000,
        );
        let legacy_samples = (0..6)
            .map(|_| RuntimeSample {
                quality: Quality::High,
                input_kind: RuntimeInputKind::Video,
                source_long_edge: 3_840,
                working_long_edge: None,
                resolution_policy_version: None,
                extracted_frames: 150,
                duration_ms: 1_000_000,
                brush: None,
            })
            .collect::<Vec<_>>();

        let estimate = estimate_runtime_with_brush(
            &video,
            &plan,
            Quality::High,
            &legacy_samples,
            Some(&current),
        );
        assert_eq!(estimate.confidence, EstimateConfidence::Low);
    }

    #[test]
    fn same_profile_from_an_old_resolution_policy_is_only_a_fallback() {
        let video = video();
        let plan = FramePlan {
            estimated_frames: 251,
            ..FramePlan::default()
        };
        let resolution = crate::presets::resolve_planner_resolution_plan(
            Quality::Fast,
            Some(8_192),
            3_840,
            2_160,
            true,
        );
        let brush = crate::presets::resolve_brush_training_preset_for_plan(
            Quality::Fast,
            Some(8_192),
            3_840,
            0,
            &resolution,
        );
        let samples = (0..6)
            .map(|_| RuntimeSample {
                quality: Quality::Fast,
                input_kind: RuntimeInputKind::Video,
                source_long_edge: 3_840,
                working_long_edge: Some(1_600),
                resolution_policy_version: None,
                extracted_frames: 251,
                duration_ms: 350_000,
                brush: Some(brush),
            })
            .collect::<Vec<_>>();
        let estimate = estimate_runtime_with_brush_and_resolution(
            &video,
            &plan,
            Quality::Fast,
            &samples,
            Some(&brush),
            Some(resolution.policy_version),
            resolution.working_long_edge(),
        );
        assert_eq!(estimate.confidence, EstimateConfidence::Low);
        assert_eq!(estimate.sample_count, 6);
    }

    #[test]
    fn planner_video_baselines_match_the_three_latest_benchmarks() {
        let cases = [
            (Quality::Fast, 251, 260_964_u64),
            (Quality::Balanced, 285, 1_175_414_u64),
            (Quality::High, 674, 1_813_324_u64),
        ];
        for (quality, frames, measured_ms) in cases {
            let brush = crate::presets::resolve_brush_training_preset(
                quality,
                true,
                Some(8_192),
                1_920,
                60_000,
            );
            let estimated = base_estimate_ms_with_brush(
                frames,
                quality,
                RuntimeInputKind::Video,
                1_920,
                Some(&brush),
            );
            assert!(estimated.abs_diff(measured_ms) <= 1_000);
        }
    }

    #[test]
    fn staged_fast_and_balanced_brush_limits_raise_the_new_policy_estimate() {
        for (quality, frames, expected_ms) in [
            (Quality::Fast, 251, 348_346_u64),
            (Quality::Balanced, 285, 1_401_275_u64),
        ] {
            let resolution = crate::presets::resolve_planner_resolution_plan(
                quality,
                Some(8_192),
                3_840,
                2_160,
                true,
            );
            let brush = crate::presets::resolve_brush_training_preset_for_plan(
                quality,
                Some(8_192),
                3_840,
                0,
                &resolution,
            );
            let estimated = base_estimate_ms_with_brush(
                frames,
                quality,
                RuntimeInputKind::Video,
                resolution.working_long_edge(),
                Some(&brush),
            );
            assert!(estimated.abs_diff(expected_ms) <= 1_000);
        }
    }

    #[test]
    fn high_standard_full_resolution_image_baseline_matches_experiment() {
        let brush = crate::presets::resolve_brush_training_preset(
            Quality::High,
            true,
            Some(8_192),
            3_840,
            60_000,
        );
        let estimated = base_estimate_ms_with_brush(
            90,
            Quality::High,
            RuntimeInputKind::Images,
            3_840,
            Some(&brush),
        );
        let measured = 1_143_657 + 7_544_882;
        assert!((estimated as f64 / measured as f64 - 1.0).abs() < 0.01);
    }

    #[test]
    fn video_history_does_not_calibrate_image_estimates() {
        let brush = crate::presets::resolve_brush_training_preset(
            Quality::High,
            true,
            Some(8_192),
            3_840,
            60_000,
        );
        let video_sample = RuntimeSample {
            quality: Quality::High,
            input_kind: RuntimeInputKind::Video,
            source_long_edge: 1_920,
            working_long_edge: None,
            resolution_policy_version: None,
            extracted_frames: 674,
            duration_ms: 1_813_324,
            brush: Some(brush),
        };
        let plan = FramePlan {
            estimated_frames: 90,
            ..FramePlan::default()
        };
        let estimate = estimate_runtime_for_images_with_brush(
            90,
            3_840,
            &plan,
            Quality::High,
            &[video_sample],
            Some(&brush),
        );
        assert_eq!(estimate.sample_count, 0);
        assert!(estimate.estimated_ms > 8_000_000);
    }
}
