use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

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

impl ResolvedBrushTrainingPreset {
    pub fn downgrade_after_oom(self, initial_sfm_points: u64) -> Option<Self> {
        let profile = match self.profile {
            BrushTrainingProfile::HighLarge => BrushTrainingProfile::HighStandard,
            BrushTrainingProfile::HighStandard => BrushTrainingProfile::HighLow,
            BrushTrainingProfile::HighLow => BrushTrainingProfile::HighEmergency,
            BrushTrainingProfile::Legacy
            | BrushTrainingProfile::Fast
            | BrushTrainingProfile::Balanced
            | BrushTrainingProfile::HighEmergency => return None,
        };
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
    pub sfm_max_features: u32,
    pub sfm_allow_two_view_tracks: bool,
    pub brush_iterations: usize,
    pub brush_max_resolution: u32,
    pub brush_densification: Option<BrushDensificationPreset>,
}

impl Quality {
    pub const fn preset(self) -> QualityPreset {
        match self {
            Self::Fast => QualityPreset {
                frame_retention_ratio: 0.30,
                initial_fps: Some(6.0),
                rescue_max_fps: Some(9.0),
                sfm_max_image_size: 1_600,
                sfm_max_features: 4_096,
                sfm_allow_two_view_tracks: false,
                brush_iterations: 8_000,
                brush_max_resolution: 1_200,
                brush_densification: Some(BrushDensificationPreset {
                    growth_grad_threshold: 0.00004,
                    growth_select_fraction: 0.15,
                    growth_stop_iter: 6_000,
                }),
            },
            Self::Balanced => QualityPreset {
                frame_retention_ratio: 0.50,
                initial_fps: Some(8.0),
                rescue_max_fps: Some(12.0),
                sfm_max_image_size: 1_920,
                sfm_max_features: 8_192,
                sfm_allow_two_view_tracks: false,
                brush_iterations: 15_000,
                brush_max_resolution: 1_600,
                brush_densification: Some(BrushDensificationPreset {
                    growth_grad_threshold: 0.00003,
                    growth_select_fraction: 0.2,
                    growth_stop_iter: 12_000,
                }),
            },
            Self::High => QualityPreset {
                frame_retention_ratio: 1.00,
                initial_fps: Some(12.0),
                rescue_max_fps: Some(15.0),
                sfm_max_image_size: 3_200,
                sfm_max_features: 16_384,
                sfm_allow_two_view_tracks: true,
                brush_iterations: 30_000,
                // Planner-enabled High resolves this from VRAM. Legacy keeps 2K.
                brush_max_resolution: 2_000,
                brush_densification: None,
            },
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
        return ResolvedBrushTrainingPreset {
            profile: BrushTrainingProfile::Legacy,
            detected_total_memory_mb,
            configured_max_splats: None,
            preset: BrushTrainingPreset {
                total_steps: base.brush_iterations,
                max_resolution: base.brush_max_resolution,
                refine_every: 200,
                max_splats: None,
                densification: None,
            },
        };
    }

    match quality {
        Quality::Fast => {
            resolved_quality_profile(BrushTrainingProfile::Fast, detected_total_memory_mb, base)
        }
        Quality::Balanced => resolved_quality_profile(
            BrushTrainingProfile::Balanced,
            detected_total_memory_mb,
            base,
        ),
        Quality::High => {
            let profile = match detected_total_memory_mb {
                Some(memory) if memory >= 12_288 => BrushTrainingProfile::HighLarge,
                Some(memory) if memory >= 8_192 => BrushTrainingProfile::HighStandard,
                _ => BrushTrainingProfile::HighLow,
            };
            resolve_high_profile(
                profile,
                detected_total_memory_mb,
                source_long_edge,
                initial_sfm_points,
            )
        }
    }
}

fn resolved_quality_profile(
    profile: BrushTrainingProfile,
    detected_total_memory_mb: Option<u64>,
    base: QualityPreset,
) -> ResolvedBrushTrainingPreset {
    ResolvedBrushTrainingPreset {
        profile,
        detected_total_memory_mb,
        configured_max_splats: None,
        preset: BrushTrainingPreset {
            total_steps: base.brush_iterations,
            max_resolution: base.brush_max_resolution,
            refine_every: 200,
            max_splats: None,
            densification: base.brush_densification,
        },
    }
}

fn resolve_high_profile(
    profile: BrushTrainingProfile,
    detected_total_memory_mb: Option<u64>,
    source_long_edge: u32,
    initial_sfm_points: u64,
) -> ResolvedBrushTrainingPreset {
    let (max_resolution, growth_grad_threshold, growth_select_fraction, growth_stop_iter, cap) =
        match profile {
            BrushTrainingProfile::HighLow => (3_200, 0.00002, 0.25, 23_000, 200_000),
            BrushTrainingProfile::HighStandard => (3_840, 0.00002, 0.3, 25_000, 1_500_000),
            BrushTrainingProfile::HighLarge => {
                (source_long_edge.max(1), 0.00002, 0.3, 25_000, 4_000_000)
            }
            BrushTrainingProfile::HighEmergency => (2_000, 0.00004, 0.2, 20_000, 200_000),
            _ => unreachable!("only High profiles are resolved here"),
        };
    let initial_sfm_points = initial_sfm_points.min(u32::MAX as u64) as u32;
    ResolvedBrushTrainingPreset {
        profile,
        detected_total_memory_mb,
        configured_max_splats: Some(cap),
        preset: BrushTrainingPreset {
            total_steps: 30_000,
            max_resolution,
            refine_every: 200,
            max_splats: Some(cap.max(initial_sfm_points)),
            densification: Some(BrushDensificationPreset {
                growth_grad_threshold,
                growth_select_fraction,
                growth_stop_iter,
            }),
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
    fn high_splat_cap_never_falls_below_initial_geometry() {
        let resolved =
            resolve_brush_training_preset(Quality::High, true, Some(4_096), 3_840, 350_000);
        assert_eq!(resolved.configured_max_splats, Some(200_000));
        assert_eq!(resolved.preset.max_splats, Some(350_000));
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
    fn planner_disabled_keeps_legacy_brush_behavior() {
        let resolved =
            resolve_brush_training_preset(Quality::High, false, Some(24_576), 7_680, 50_000);
        assert_eq!(resolved.profile, BrushTrainingProfile::Legacy);
        assert_eq!(resolved.preset.max_resolution, 2_000);
        assert!(resolved.preset.densification.is_none());
        assert!(resolved.preset.max_splats.is_none());
    }

    #[test]
    fn balanced_is_default() {
        assert_eq!(Quality::default(), Quality::Balanced);
    }
}
