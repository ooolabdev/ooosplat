use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

use super::pipeline_config::pipeline_optimization_config;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Quality {
    Fast,
    #[default]
    Balanced,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrushDensificationPreset {
    pub growth_grad_threshold: f32,
    pub growth_select_fraction: f32,
    pub growth_stop_iter: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrushTrainingPreset {
    pub total_steps: usize,
    pub max_resolution: u32,
    pub refine_every: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_splats: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub densification: Option<BrushDensificationPreset>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BrushTrainingProfile {
    #[default]
    Legacy,
    Fast,
    Balanced,
    HighLow,
    HighStandard,
    HighLarge,
    HighEmergency,
}

impl BrushTrainingProfile {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Legacy => "legacy",
            Self::Fast => "fast",
            Self::Balanced => "balanced",
            Self::HighLow => "high-low",
            Self::HighStandard => "high-standard",
            Self::HighLarge => "high-large",
            Self::HighEmergency => "high-emergency",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedBrushTrainingPreset {
    pub profile: BrushTrainingProfile,
    pub detected_total_memory_mb: Option<u64>,
    pub configured_max_splats: Option<u32>,
    pub preset: BrushTrainingPreset,
}

pub const PLANNER_RESOLUTION_POLICY_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannerResolutionPlan {
    pub policy_version: u32,
    pub working_width: u32,
    pub working_height: u32,
    pub sfm_max_image_size: u32,
    pub brush_initial_max_resolution: u32,
    pub brush_initial_profile: BrushTrainingProfile,
}

impl PlannerResolutionPlan {
    pub const fn working_long_edge(self) -> u32 {
        if self.working_width > self.working_height {
            self.working_width
        } else {
            self.working_height
        }
    }
}

impl ResolvedBrushTrainingPreset {
    pub fn downgrade_after_oom(self, initial_sfm_points: u64) -> Option<Self> {
        if self.profile == BrushTrainingProfile::Legacy {
            return None;
        }
        let profile = pipeline_optimization_config()
            .brush_profiles
            .get(self.profile)
            .oom_fallback?;
        Some(resolve_high_profile(
            profile,
            self.detected_total_memory_mb,
            self.preset.max_resolution,
            initial_sfm_points,
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QualityPreset {
    pub frame_retention_ratio: f64,
    pub initial_fps: Option<f64>,
    pub rescue_max_fps: Option<f64>,
    pub sfm_max_image_size: u32,
    pub planned_sfm_max_image_size: u32,
    pub sfm_max_features: u32,
    pub planned_sfm_max_features: u32,
    pub sfm_allow_two_view_tracks: bool,
    pub brush_iterations: usize,
    pub brush_max_resolution: u32,
    pub brush_densification: Option<BrushDensificationPreset>,
}

impl Quality {
    pub fn preset(self) -> QualityPreset {
        let config = pipeline_optimization_config();
        let quality = config.qualities.get(self);
        let legacy = &quality.automatic_optimization_off;
        let automatic = &quality.automatic_optimization_on;
        let automatic_brush = config.brush_profiles.get(automatic.initial_brush_profile);
        QualityPreset {
            frame_retention_ratio: legacy.frame_retention_ratio,
            initial_fps: Some(automatic.initial_fps),
            rescue_max_fps: Some(automatic.rescue_max_fps),
            sfm_max_image_size: legacy.sfm_max_image_size,
            planned_sfm_max_image_size: automatic_brush.sfm_max_image_size,
            sfm_max_features: legacy.sfm_max_features,
            planned_sfm_max_features: automatic.sfm_max_features,
            sfm_allow_two_view_tracks: automatic.allow_two_view_tracks,
            brush_iterations: legacy.brush.total_steps,
            brush_max_resolution: legacy.brush.max_resolution,
            brush_densification: automatic_brush.densification,
        }
    }
}

pub fn resolve_brush_training_preset(
    quality: Quality,
    planner_enabled: bool,
    detected_total_memory_mb: Option<u64>,
    source_long_edge: u32,
    initial_sfm_points: u64,
) -> ResolvedBrushTrainingPreset {
    let base = quality.preset();
    if !planner_enabled {
        let config = pipeline_optimization_config();
        return ResolvedBrushTrainingPreset {
            profile: BrushTrainingProfile::Legacy,
            detected_total_memory_mb,
            configured_max_splats: None,
            preset: BrushTrainingPreset {
                total_steps: base.brush_iterations,
                max_resolution: base.brush_max_resolution,
                refine_every: config.shared.brush_refine_every,
                max_splats: None,
                densification: config
                    .qualities
                    .get(quality)
                    .automatic_optimization_off
                    .brush
                    .densification,
            },
        };
    }

    match quality {
        Quality::Fast | Quality::Balanced => resolved_quality_profile(
            pipeline_optimization_config()
                .qualities
                .get(quality)
                .automatic_optimization_on
                .initial_brush_profile,
            detected_total_memory_mb,
            base.brush_max_resolution,
            initial_sfm_points,
        ),
        Quality::High => {
            let profile = high_profile_for_memory(detected_total_memory_mb);
            resolve_high_profile(
                profile,
                detected_total_memory_mb,
                source_long_edge,
                initial_sfm_points,
            )
        }
    }
}

pub fn resolve_planner_resolution_plan(
    quality: Quality,
    detected_total_memory_mb: Option<u64>,
    source_width: u32,
    source_height: u32,
    video_input: bool,
) -> PlannerResolutionPlan {
    let source_long_edge = source_width.max(source_height).max(1);
    let profile = match quality {
        Quality::High => high_profile_for_memory(detected_total_memory_mb),
        Quality::Fast | Quality::Balanced => {
            pipeline_optimization_config()
                .qualities
                .get(quality)
                .automatic_optimization_on
                .initial_brush_profile
        }
    };
    let config = pipeline_optimization_config();
    let profile_config = config.brush_profiles.get(profile);
    let working_limit = profile_config
        .working_max_long_edge
        .resolve(source_long_edge);
    let sfm_max_image_size = profile_config.sfm_max_image_size;
    let brush_initial_max_resolution = profile_config.max_resolution.resolve(source_long_edge);
    let (working_width, working_height) = if video_input {
        scaled_dimensions(source_width, source_height, working_limit)
    } else {
        (source_width.max(1), source_height.max(1))
    };
    PlannerResolutionPlan {
        policy_version: PLANNER_RESOLUTION_POLICY_VERSION,
        working_width,
        working_height,
        sfm_max_image_size,
        brush_initial_max_resolution,
        brush_initial_profile: profile,
    }
}

pub fn resolve_brush_training_preset_for_plan(
    quality: Quality,
    detected_total_memory_mb: Option<u64>,
    source_long_edge: u32,
    initial_sfm_points: u64,
    resolution: &PlannerResolutionPlan,
) -> ResolvedBrushTrainingPreset {
    if quality == Quality::High {
        return resolve_high_profile(
            resolution.brush_initial_profile,
            detected_total_memory_mb,
            source_long_edge,
            initial_sfm_points,
        );
    }
    let mut resolved = resolve_brush_training_preset(
        quality,
        true,
        detected_total_memory_mb,
        source_long_edge,
        initial_sfm_points,
    );
    resolved.profile = resolution.brush_initial_profile;
    resolved.preset.max_resolution = resolution.brush_initial_max_resolution.max(1);
    resolved
}

fn high_profile_for_memory(detected_total_memory_mb: Option<u64>) -> BrushTrainingProfile {
    let config = pipeline_optimization_config();
    let thresholds = &config.high_vram;
    match detected_total_memory_mb {
        Some(memory) if memory >= thresholds.large_minimum_mi_b => BrushTrainingProfile::HighLarge,
        Some(memory) if memory >= thresholds.standard_minimum_mi_b => {
            BrushTrainingProfile::HighStandard
        }
        _ => BrushTrainingProfile::HighLow,
    }
}

fn scaled_dimensions(width: u32, height: u32, max_long_edge: u32) -> (u32, u32) {
    let width = width.max(1);
    let height = height.max(1);
    let long_edge = width.max(height);
    if long_edge <= max_long_edge.max(1) {
        return (width, height);
    }
    let scale = max_long_edge.max(1) as f64 / long_edge as f64;
    (
        (width as f64 * scale).round().max(1.0) as u32,
        (height as f64 * scale).round().max(1.0) as u32,
    )
}

fn resolved_quality_profile(
    profile: BrushTrainingProfile,
    detected_total_memory_mb: Option<u64>,
    initial_max_resolution: u32,
    initial_sfm_points: u64,
) -> ResolvedBrushTrainingPreset {
    let config = pipeline_optimization_config();
    let profile_config = config.brush_profiles.get(profile);
    let initial_sfm_points = initial_sfm_points.min(u32::MAX as u64) as u32;
    ResolvedBrushTrainingPreset {
        profile,
        detected_total_memory_mb,
        configured_max_splats: profile_config.max_splats,
        preset: BrushTrainingPreset {
            total_steps: profile_config.total_steps,
            max_resolution: initial_max_resolution.max(1),
            refine_every: config.shared.brush_refine_every,
            max_splats: profile_config
                .max_splats
                .map(|cap| cap.max(initial_sfm_points)),
            densification: profile_config.densification,
        },
    }
}

fn resolve_high_profile(
    profile: BrushTrainingProfile,
    detected_total_memory_mb: Option<u64>,
    source_long_edge: u32,
    initial_sfm_points: u64,
) -> ResolvedBrushTrainingPreset {
    let config = pipeline_optimization_config();
    let profile_config = config.brush_profiles.get(profile);
    let cap = profile_config
        .max_splats
        .expect("High profiles must define maxSplats");
    let initial_sfm_points = initial_sfm_points.min(u32::MAX as u64) as u32;
    ResolvedBrushTrainingPreset {
        profile,
        detected_total_memory_mb,
        configured_max_splats: Some(cap),
        preset: BrushTrainingPreset {
            total_steps: profile_config.total_steps,
            max_resolution: profile_config.max_resolution.resolve(source_long_edge),
            refine_every: config.shared.brush_refine_every,
            max_splats: Some(cap.max(initial_sfm_points)),
            densification: profile_config.densification,
        },
    }
}

impl fmt::Display for Quality {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Fast => "fast",
            Self::Balanced => "balanced",
            Self::High => "high",
        })
    }
}

impl FromStr for Quality {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "fast" => Ok(Self::Fast),
            "balanced" => Ok(Self::Balanced),
            "high" => Ok(Self::High),
            _ => Err(format!("unknown quality preset: {value}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quality_v2_presets_are_exact() {
        let fast = Quality::Fast.preset();
        let balanced = Quality::Balanced.preset();
        let high = Quality::High.preset();
        assert_eq!(
            (fast.initial_fps, fast.rescue_max_fps),
            (Some(6.0), Some(9.0))
        );
        assert_eq!(
            (balanced.initial_fps, balanced.rescue_max_fps),
            (Some(8.0), Some(12.0))
        );
        assert_eq!(
            (high.initial_fps, high.rescue_max_fps),
            (Some(12.0), Some(15.0))
        );
        assert_eq!(fast.sfm_max_image_size, 1_200);
        assert_eq!(balanced.sfm_max_image_size, 1_600);
        assert_eq!(high.sfm_max_image_size, 2_000);
        assert_eq!(fast.planned_sfm_max_image_size, 1_200);
        assert_eq!(balanced.planned_sfm_max_image_size, 1_600);
        assert_eq!(high.planned_sfm_max_image_size, 3_200);
        assert_eq!(fast.sfm_max_features, 8_192);
        assert_eq!(balanced.sfm_max_features, 8_192);
        assert_eq!(high.sfm_max_features, 8_192);
        assert_eq!(fast.planned_sfm_max_features, 4_096);
        assert_eq!(balanced.planned_sfm_max_features, 8_192);
        assert_eq!(high.planned_sfm_max_features, 16_384);
        assert_eq!(fast.brush_densification.unwrap().growth_stop_iter, 6_000);
        assert_eq!(
            balanced.brush_densification.unwrap().growth_grad_threshold,
            0.00003
        );
        assert!(high.sfm_allow_two_view_tracks);
    }

    #[test]
    fn high_vram_boundaries_and_native_resolution_are_exact() {
        let low = resolve_brush_training_preset(Quality::High, true, Some(8_191), 7_680, 50_000);
        let standard =
            resolve_brush_training_preset(Quality::High, true, Some(8_192), 7_680, 50_000);
        let standard_max =
            resolve_brush_training_preset(Quality::High, true, Some(12_287), 7_680, 50_000);
        let large = resolve_brush_training_preset(Quality::High, true, Some(12_288), 7_680, 50_000);
        let unknown = resolve_brush_training_preset(Quality::High, true, None, 7_680, 50_000);

        assert_eq!(low.profile, BrushTrainingProfile::HighLow);
        assert_eq!(low.preset.max_resolution, 3_200);
        assert_eq!(standard.profile, BrushTrainingProfile::HighStandard);
        assert_eq!(standard.preset.max_resolution, 3_840);
        assert_eq!(standard_max.profile, BrushTrainingProfile::HighStandard);
        assert_eq!(large.profile, BrushTrainingProfile::HighLarge);
        assert_eq!(large.preset.max_resolution, 7_680);
        assert_eq!(unknown.profile, BrushTrainingProfile::HighLow);
    }

    #[test]
    fn staged_resolution_policy_matches_all_quality_tiers() {
        let fast = resolve_planner_resolution_plan(Quality::Fast, Some(4_096), 3_840, 2_160, true);
        assert_eq!((fast.working_width, fast.working_height), (1_600, 900));
        assert_eq!(fast.sfm_max_image_size, 1_200);
        assert_eq!(fast.brush_initial_max_resolution, 1_600);

        let balanced =
            resolve_planner_resolution_plan(Quality::Balanced, Some(4_096), 3_840, 2_160, true);
        assert_eq!(
            (balanced.working_width, balanced.working_height),
            (1_920, 1_080)
        );
        assert_eq!(balanced.sfm_max_image_size, 1_600);
        assert_eq!(balanced.brush_initial_max_resolution, 1_920);

        let low = resolve_planner_resolution_plan(Quality::High, Some(8_191), 7_680, 4_320, true);
        let standard =
            resolve_planner_resolution_plan(Quality::High, Some(8_192), 7_680, 4_320, true);
        let standard_max =
            resolve_planner_resolution_plan(Quality::High, Some(12_287), 7_680, 4_320, true);
        let large =
            resolve_planner_resolution_plan(Quality::High, Some(12_288), 7_680, 4_320, true);
        assert_eq!(low.working_long_edge(), 3_200);
        assert_eq!(standard.working_long_edge(), 3_840);
        assert_eq!(standard_max.working_long_edge(), 3_840);
        assert_eq!(large.working_long_edge(), 7_680);
        assert_eq!(low.sfm_max_image_size, 3_200);
        assert_eq!(standard.sfm_max_image_size, 3_200);
        assert_eq!(large.sfm_max_image_size, 3_200);
    }

    #[test]
    fn staged_resolution_never_upscales_and_keeps_image_sources_native() {
        let video = resolve_planner_resolution_plan(Quality::High, Some(12_288), 1_280, 720, true);
        assert_eq!((video.working_width, video.working_height), (1_280, 720));
        let images =
            resolve_planner_resolution_plan(Quality::High, Some(8_192), 2_560, 3_840, false);
        assert_eq!(
            (images.working_width, images.working_height),
            (2_560, 3_840)
        );
        assert_eq!(images.sfm_max_image_size, 3_200);
        assert_eq!(images.brush_initial_max_resolution, 3_840);
    }

    #[test]
    fn planner_brush_resolution_does_not_change_legacy_defaults() {
        let fast_plan =
            resolve_planner_resolution_plan(Quality::Fast, Some(8_192), 3_840, 2_160, true);
        let fast = resolve_brush_training_preset_for_plan(
            Quality::Fast,
            Some(8_192),
            3_840,
            0,
            &fast_plan,
        );
        assert_eq!(fast.preset.max_resolution, 1_600);
        let legacy = resolve_brush_training_preset(Quality::Fast, false, Some(8_192), 3_840, 0);
        assert_eq!(legacy.preset.max_resolution, 1_200);
    }

    #[test]
    fn high_splat_cap_never_falls_below_initial_geometry() {
        let resolved =
            resolve_brush_training_preset(Quality::High, true, Some(4_096), 3_840, 350_000);
        let cap = pipeline_optimization_config()
            .brush_profiles
            .get(resolved.profile)
            .max_splats
            .unwrap();
        assert_eq!(resolved.configured_max_splats, Some(cap));
        assert_eq!(resolved.preset.max_splats, Some(cap.max(350_000)));
    }

    #[test]
    fn fast_and_balanced_splat_caps_never_fall_below_initial_geometry() {
        let fast_below_cap =
            resolve_brush_training_preset(Quality::Fast, true, Some(4_096), 1_600, 250_000);
        assert_eq!(fast_below_cap.configured_max_splats, Some(500_000));
        assert_eq!(fast_below_cap.preset.max_splats, Some(500_000));
        assert!(fast_below_cap.downgrade_after_oom(250_000).is_none());

        let fast_above_cap =
            resolve_brush_training_preset(Quality::Fast, true, Some(4_096), 1_600, 600_000);
        assert_eq!(fast_above_cap.configured_max_splats, Some(500_000));
        assert_eq!(fast_above_cap.preset.max_splats, Some(600_000));

        let balanced =
            resolve_brush_training_preset(Quality::Balanced, true, Some(8_192), 1_920, 1_100_000);
        assert_eq!(balanced.configured_max_splats, Some(1_000_000));
        assert_eq!(balanced.preset.max_splats, Some(1_100_000));
        assert!(balanced.downgrade_after_oom(1_100_000).is_none());
    }

    #[test]
    fn high_oom_downgrade_is_bounded() {
        let large = resolve_brush_training_preset(Quality::High, true, Some(12_288), 7_680, 50_000);
        let standard = large.downgrade_after_oom(50_000).unwrap();
        let low = standard.downgrade_after_oom(50_000).unwrap();
        let emergency = low.downgrade_after_oom(50_000).unwrap();
        assert_eq!(standard.profile, BrushTrainingProfile::HighStandard);
        assert_eq!(low.profile, BrushTrainingProfile::HighLow);
        assert_eq!(emergency.profile, BrushTrainingProfile::HighEmergency);
        assert_eq!(emergency.preset.max_resolution, 2_000);
        assert!(emergency.downgrade_after_oom(50_000).is_none());
    }

    #[test]
    fn planner_disabled_uses_compatible_densification_with_fixed_training_settings() {
        for (quality, total_steps, max_resolution) in [
            (Quality::Fast, 8_000, 1_200),
            (Quality::Balanced, 15_000, 1_600),
            (Quality::High, 30_000, 2_000),
        ] {
            let resolved =
                resolve_brush_training_preset(quality, false, Some(24_576), 7_680, 50_000);
            assert_eq!(resolved.profile, BrushTrainingProfile::Legacy);
            assert_eq!(resolved.preset.total_steps, total_steps);
            assert_eq!(resolved.preset.max_resolution, max_resolution);
            assert_eq!(resolved.preset.refine_every, 200);
            assert_eq!(
                resolved.preset.densification,
                Some(BrushDensificationPreset {
                    growth_grad_threshold: 0.0025,
                    growth_select_fraction: 0.1,
                    growth_stop_iter: 15_000,
                })
            );
            assert!(resolved.preset.max_splats.is_none());
            assert!(resolved.downgrade_after_oom(50_000).is_none());
        }
    }

    #[test]
    fn balanced_is_default() {
        assert_eq!(Quality::default(), Quality::Balanced);
    }
}
