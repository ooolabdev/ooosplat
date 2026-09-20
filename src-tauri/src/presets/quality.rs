use serde::{Deserialize, Deserializer, Serialize};
use std::{fmt, str::FromStr};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Quality {
    Fast,
    #[default]
    Balanced,
    High,
}

/// Planner boundaries. These values are budgets, not FFmpeg extraction rates.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameBudget {
    pub preferred_fps: f64,
    pub min_fps: f64,
    pub max_fps: f64,
    pub analysis_fps: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SfmBudget {
    pub max_image_size: u32,
    pub max_features: u32,
}

impl SfmBudget {
    pub fn capped_for_source(self, width: u32, height: u32) -> Self {
        Self {
            max_image_size: self.max_image_size.min(width.max(height).max(1)),
            ..self
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchingBudget {
    pub sequential_overlap: u32,
    pub prefilter_neighbors: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RescueBudget {
    pub max_rounds: u32,
    pub allow_frame_backfill: bool,
    pub allow_local_exhaustive: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BrushResolutionPolicy {
    Fixed(u32),
    Auto {
        preferred_max: u32,
        allow_native: bool,
    },
    PreferNative,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrushBudget {
    pub iterations: usize,
    pub resolution: BrushResolutionPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BaselineBudget {
    pub sfm: SfmBudget,
    pub matching: MatchingBudget,
    pub brush: BrushBudget,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionBudget {
    pub sfm_rescue: SfmBudget,
    pub rescue: RescueBudget,
    pub brush_levels: Vec<BrushBudget>,
}

/// Quality v2 is a resource envelope. Algorithm choice belongs to Planner.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityPreset {
    pub frame: FrameBudget,
    pub baseline: BaselineBudget,
    pub extension: ExtensionBudget,
    /// Preserved only when loading a legacy flat preset; it is intentionally
    /// not translated to preferred_fps because the concepts are different.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub legacy_frame_retention_ratio: Option<f64>,
}

pub type QualityBudget = QualityPreset;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BrushResolutionContext {
    pub source_width: u32,
    pub source_height: u32,
    pub frame_count: u64,
    pub available_memory_mb: Option<u64>,
    pub safety_margin: f32,
}

impl BrushResolutionContext {
    pub const DEFAULT_SAFETY_MARGIN: f32 = 0.20;

    pub const fn new(
        source_width: u32,
        source_height: u32,
        frame_count: u64,
        available_memory_mb: Option<u64>,
    ) -> Self {
        Self {
            source_width,
            source_height,
            frame_count,
            available_memory_mb,
            safety_margin: Self::DEFAULT_SAFETY_MARGIN,
        }
    }

    pub fn source_max_dimension(self) -> u32 {
        self.source_width.max(self.source_height).max(1)
    }

    fn safe_max_dimension(self) -> u32 {
        let usable_memory = self.available_memory_mb.map(|memory| {
            let margin = self.safety_margin.clamp(0.0, 0.75) as f64;
            (memory as f64 * (1.0 - margin)).round() as u64
        });
        let memory_limit = match usable_memory {
            Some(memory) if memory < 4_096 => 1_600,
            Some(memory) if memory < 8_192 => 2_400,
            Some(memory) if memory < 16_384 => 3_200,
            Some(_) => u32::MAX,
            None => 2_400,
        };
        let frame_limit = if self.frame_count > 1_200 {
            2_400
        } else if self.frame_count > 600 {
            3_200
        } else {
            u32::MAX
        };
        memory_limit.min(frame_limit)
    }
}

impl BrushResolutionPolicy {
    pub fn resolve(self, context: BrushResolutionContext) -> u32 {
        let native = context.source_max_dimension();
        let safe_max = context.safe_max_dimension();
        match self {
            Self::Fixed(value) => value.max(1).min(native),
            Self::Auto {
                preferred_max,
                allow_native,
            } => {
                if allow_native && native <= safe_max {
                    native
                } else {
                    preferred_max.max(1).min(safe_max).min(native)
                }
            }
            Self::PreferNative => native.min(safe_max),
        }
    }

    pub const fn estimate_max_resolution(self) -> u32 {
        match self {
            Self::Fixed(value) => value,
            Self::Auto { preferred_max, .. } => preferred_max,
            Self::PreferNative => 3_200,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedBrushBudget {
    pub iterations: usize,
    pub max_resolution: u32,
}

impl BrushBudget {
    pub fn resolve(self, context: BrushResolutionContext) -> ResolvedBrushBudget {
        ResolvedBrushBudget {
            iterations: self.iterations,
            max_resolution: self.resolution.resolve(context),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct QualityBudgetOverrides {
    pub preferred_fps: Option<f64>,
    pub min_fps: Option<f64>,
    pub max_fps: Option<f64>,
    pub analysis_fps: Option<f64>,
    pub sfm_max_image_size: Option<u32>,
    pub sfm_max_features: Option<u32>,
    pub rescue_max_image_size: Option<u32>,
    pub rescue_max_features: Option<u32>,
    pub sequential_overlap: Option<u32>,
    pub prefilter_neighbors: Option<u32>,
    pub rescue_max_rounds: Option<u32>,
    pub allow_frame_backfill: Option<bool>,
    pub allow_local_exhaustive: Option<bool>,
    pub brush_iterations: Option<usize>,
    pub brush_resolution: Option<BrushResolutionPolicy>,
}

impl QualityPreset {
    pub fn with_overrides(mut self, overrides: QualityBudgetOverrides) -> Self {
        self.frame.preferred_fps = overrides.preferred_fps.unwrap_or(self.frame.preferred_fps);
        self.frame.min_fps = overrides.min_fps.unwrap_or(self.frame.min_fps);
        self.frame.max_fps = overrides
            .max_fps
            .unwrap_or(self.frame.max_fps)
            .max(self.frame.min_fps);
        self.frame.preferred_fps = self
            .frame
            .preferred_fps
            .clamp(self.frame.min_fps, self.frame.max_fps);
        self.frame.analysis_fps = overrides
            .analysis_fps
            .unwrap_or(self.frame.analysis_fps)
            .max(self.frame.max_fps);
        self.baseline.sfm.max_image_size = overrides
            .sfm_max_image_size
            .unwrap_or(self.baseline.sfm.max_image_size);
        self.baseline.sfm.max_features = overrides
            .sfm_max_features
            .unwrap_or(self.baseline.sfm.max_features);
        self.extension.sfm_rescue.max_image_size = overrides
            .rescue_max_image_size
            .unwrap_or(self.extension.sfm_rescue.max_image_size);
        self.extension.sfm_rescue.max_features = overrides
            .rescue_max_features
            .unwrap_or(self.extension.sfm_rescue.max_features);
        self.baseline.matching.sequential_overlap = overrides
            .sequential_overlap
            .unwrap_or(self.baseline.matching.sequential_overlap);
        self.baseline.matching.prefilter_neighbors = overrides
            .prefilter_neighbors
            .unwrap_or(self.baseline.matching.prefilter_neighbors);
        self.extension.rescue.max_rounds = overrides
            .rescue_max_rounds
            .unwrap_or(self.extension.rescue.max_rounds);
        self.extension.rescue.allow_frame_backfill = overrides
            .allow_frame_backfill
            .unwrap_or(self.extension.rescue.allow_frame_backfill);
        self.extension.rescue.allow_local_exhaustive = overrides
            .allow_local_exhaustive
            .unwrap_or(self.extension.rescue.allow_local_exhaustive);
        self.baseline.brush.iterations = overrides
            .brush_iterations
            .unwrap_or(self.baseline.brush.iterations);
        self.baseline.brush.resolution = overrides
            .brush_resolution
            .unwrap_or(self.baseline.brush.resolution);
        self
    }
}

pub fn resolve_with_precedence<T>(explicit: Option<T>, preset: Option<T>, default: T) -> T {
    explicit.or(preset).unwrap_or(default)
}

impl Quality {
    pub fn budget(self) -> QualityBudget {
        match self {
            Self::Fast => QualityPreset {
                frame: FrameBudget {
                    preferred_fps: 6.0,
                    min_fps: 4.0,
                    max_fps: 9.0,
                    analysis_fps: 12.0,
                },
                baseline: BaselineBudget {
                    sfm: SfmBudget {
                        max_image_size: 1_600,
                        max_features: 4_096,
                    },
                    matching: MatchingBudget {
                        sequential_overlap: 10,
                        prefilter_neighbors: 16,
                    },
                    brush: BrushBudget {
                        iterations: 8_000,
                        resolution: BrushResolutionPolicy::Fixed(1_200),
                    },
                },
                extension: ExtensionBudget {
                    sfm_rescue: SfmBudget {
                        max_image_size: 1_920,
                        max_features: 8_192,
                    },
                    rescue: RescueBudget {
                        max_rounds: 1,
                        allow_frame_backfill: true,
                        allow_local_exhaustive: false,
                    },
                    brush_levels: vec![BrushBudget {
                        iterations: 15_000,
                        resolution: BrushResolutionPolicy::Fixed(1_600),
                    }],
                },
                legacy_frame_retention_ratio: None,
            },
            Self::Balanced => QualityPreset {
                frame: FrameBudget {
                    preferred_fps: 8.0,
                    min_fps: 6.0,
                    max_fps: 12.0,
                    analysis_fps: 20.0,
                },
                baseline: BaselineBudget {
                    sfm: SfmBudget {
                        max_image_size: 1_920,
                        max_features: 8_192,
                    },
                    matching: MatchingBudget {
                        sequential_overlap: 15,
                        prefilter_neighbors: 24,
                    },
                    brush: BrushBudget {
                        iterations: 15_000,
                        resolution: BrushResolutionPolicy::Fixed(1_600),
                    },
                },
                extension: ExtensionBudget {
                    sfm_rescue: SfmBudget {
                        max_image_size: 2_400,
                        max_features: 12_288,
                    },
                    rescue: RescueBudget {
                        max_rounds: 2,
                        allow_frame_backfill: true,
                        allow_local_exhaustive: true,
                    },
                    brush_levels: vec![BrushBudget {
                        iterations: 30_000,
                        resolution: BrushResolutionPolicy::Fixed(2_000),
                    }],
                },
                legacy_frame_retention_ratio: None,
            },
            Self::High => QualityPreset {
                frame: FrameBudget {
                    preferred_fps: 10.0,
                    min_fps: 8.0,
                    max_fps: 15.0,
                    analysis_fps: 30.0,
                },
                baseline: BaselineBudget {
                    sfm: SfmBudget {
                        max_image_size: 2_400,
                        max_features: 8_192,
                    },
                    matching: MatchingBudget {
                        sequential_overlap: 20,
                        prefilter_neighbors: 32,
                    },
                    brush: BrushBudget {
                        iterations: 30_000,
                        resolution: BrushResolutionPolicy::Fixed(2_000),
                    },
                },
                extension: ExtensionBudget {
                    sfm_rescue: SfmBudget {
                        max_image_size: 3_200,
                        max_features: 16_384,
                    },
                    rescue: RescueBudget {
                        max_rounds: 3,
                        allow_frame_backfill: true,
                        allow_local_exhaustive: true,
                    },
                    brush_levels: vec![
                        BrushBudget {
                            iterations: 30_000,
                            resolution: BrushResolutionPolicy::Fixed(2_400),
                        },
                        BrushBudget {
                            iterations: 50_000,
                            resolution: BrushResolutionPolicy::Auto {
                                preferred_max: 3_200,
                                allow_native: true,
                            },
                        },
                    ],
                },
                legacy_frame_retention_ratio: None,
            },
        }
    }

    pub fn preset(self) -> QualityPreset {
        self.budget()
    }

    pub const fn legacy_frame_retention_ratio(self) -> f64 {
        match self {
            Self::Fast => 0.30,
            Self::Balanced => 0.50,
            Self::High => 1.0,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct QualityPresetV2 {
    frame: FrameBudget,
    baseline: BaselineBudget,
    extension: ExtensionBudget,
    #[serde(default)]
    legacy_frame_retention_ratio: Option<f64>,
}

#[derive(Deserialize)]
struct LegacyQualityPreset {
    #[serde(alias = "frameRetentionRatio")]
    frame_retention_ratio: f64,
    #[serde(alias = "brushIterations")]
    brush_iterations: usize,
    #[serde(alias = "brushMaxResolution")]
    brush_max_resolution: u32,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum QualityPresetWire {
    V2(QualityPresetV2),
    Legacy(LegacyQualityPreset),
}

impl<'de> Deserialize<'de> for QualityPreset {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(match QualityPresetWire::deserialize(deserializer)? {
            QualityPresetWire::V2(value) => Self {
                frame: value.frame,
                baseline: value.baseline,
                extension: value.extension,
                legacy_frame_retention_ratio: value.legacy_frame_retention_ratio,
            },
            QualityPresetWire::Legacy(value) => {
                let quality = if value.frame_retention_ratio <= 0.35 {
                    Quality::Fast
                } else if value.frame_retention_ratio >= 0.90 {
                    Quality::High
                } else {
                    Quality::Balanced
                };
                let mut preset = quality.preset();
                preset.legacy_frame_retention_ratio = Some(value.frame_retention_ratio);
                preset.baseline.brush = BrushBudget {
                    iterations: value.brush_iterations,
                    resolution: BrushResolutionPolicy::Fixed(value.brush_max_resolution.max(1)),
                };
                preset
            }
        })
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
    fn latest_presets_are_exact() {
        let fast = Quality::Fast.preset();
        assert_eq!(
            fast.frame,
            FrameBudget {
                preferred_fps: 6.0,
                min_fps: 4.0,
                max_fps: 9.0,
                analysis_fps: 12.0
            }
        );
        assert_eq!(
            fast.baseline.sfm,
            SfmBudget {
                max_image_size: 1_600,
                max_features: 4_096
            }
        );
        assert_eq!(
            fast.extension.sfm_rescue,
            SfmBudget {
                max_image_size: 1_920,
                max_features: 8_192
            }
        );
        assert_eq!(fast.baseline.brush.iterations, 8_000);
        assert_eq!(fast.extension.brush_levels[0].iterations, 15_000);
        let balanced = Quality::Balanced.preset();
        assert_eq!(
            balanced.frame,
            FrameBudget {
                preferred_fps: 8.0,
                min_fps: 6.0,
                max_fps: 12.0,
                analysis_fps: 20.0
            }
        );
        assert_eq!(balanced.baseline.sfm.max_image_size, 1_920);
        assert_eq!(balanced.baseline.brush.iterations, 15_000);
        let high = Quality::High.preset();
        assert_eq!(
            high.frame,
            FrameBudget {
                preferred_fps: 10.0,
                min_fps: 8.0,
                max_fps: 15.0,
                analysis_fps: 30.0
            }
        );
        assert_eq!(
            high.baseline.brush,
            BrushBudget {
                iterations: 30_000,
                resolution: BrushResolutionPolicy::Fixed(2_000)
            }
        );
        assert_eq!(high.extension.brush_levels.len(), 2);
        assert_eq!(high.extension.brush_levels[1].iterations, 50_000);
    }

    #[test]
    fn budgets_contain_no_algorithm_policy() {
        let value = serde_json::to_value(Quality::High.preset()).unwrap();
        assert!(value.get("mapper").is_none());
        assert!(value.get("pairing").is_none());
    }

    #[test]
    fn legacy_ratio_is_preserved_not_translated_to_fps() {
        let legacy = r#"{"frame_retention_ratio":0.47,"brush_iterations":15000,"brush_max_resolution":1600}"#;
        let preset: QualityPreset = serde_json::from_str(legacy).unwrap();
        assert_eq!(preset.legacy_frame_retention_ratio, Some(0.47));
        assert_eq!(preset.frame.preferred_fps, 8.0);
    }

    #[test]
    fn sfm_cap_never_upscales() {
        let sfm = Quality::High.preset().baseline.sfm;
        assert_eq!(sfm.capped_for_source(1_280, 720).max_image_size, 1_280);
        assert_eq!(sfm.capped_for_source(3_840, 2_160).max_image_size, 2_400);
    }

    #[test]
    fn matching_rescue_and_brush_ceilings_are_exact_and_monotonic() {
        let fast = Quality::Fast.preset();
        let balanced = Quality::Balanced.preset();
        let high = Quality::High.preset();
        assert_eq!(
            fast.baseline.matching,
            MatchingBudget {
                sequential_overlap: 10,
                prefilter_neighbors: 16
            }
        );
        assert_eq!(
            balanced.baseline.matching,
            MatchingBudget {
                sequential_overlap: 15,
                prefilter_neighbors: 24
            }
        );
        assert_eq!(
            high.baseline.matching,
            MatchingBudget {
                sequential_overlap: 20,
                prefilter_neighbors: 32
            }
        );
        assert_eq!(
            fast.extension.rescue,
            RescueBudget {
                max_rounds: 1,
                allow_frame_backfill: true,
                allow_local_exhaustive: false
            }
        );
        assert_eq!(balanced.extension.rescue.max_rounds, 2);
        assert_eq!(high.extension.rescue.max_rounds, 3);
        assert_eq!(
            fast.extension.brush_levels.last().unwrap().iterations,
            15_000
        );
        assert_eq!(
            balanced.extension.brush_levels.last().unwrap().iterations,
            30_000
        );
        assert_eq!(
            high.extension.brush_levels.last().unwrap().iterations,
            50_000
        );
        assert!(fast.frame.preferred_fps < balanced.frame.preferred_fps);
        assert!(balanced.frame.preferred_fps < high.frame.preferred_fps);
        assert!(fast.frame.max_fps < balanced.frame.max_fps);
        assert!(balanced.frame.max_fps < high.frame.max_fps);
    }
}
