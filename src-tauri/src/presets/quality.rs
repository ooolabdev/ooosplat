use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

use crate::engines::colmap::MapperBackend;

/// Which mapper to run for a reconstruction.
///
/// The global mapper's cost grows superlinearly with the number of observations
/// (images × features per image), and past a point its global positioning stage
/// does not finish at all. Measured on one 1920×1080 handheld clip with the Fast
/// tier parameters (1280px / 8192 features):
///
/// | frames | incremental | global |
/// | --- | --- | --- |
/// | 299 | 349 s | 145 s, single model |
/// | 600 | 436 s, **12 fragments** (largest 23.3%) | 432 s, **single model 94.8%** |
/// | 1200 | 2697 s, 21 fragments | **> 68 min, never finished** |
/// | 2121 | 3049 s, 21 fragments | **> 58 min, cancelled by the user** |
///
/// So below the threshold the global mapper is both faster and far better; above
/// it, the incremental mapper is the only one that returns at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MapperPreference {
    /// Try the global mapper first, falling back to the incremental one when it
    /// fails, produces an invalid model, or registers fewer than 60% of images.
    PreferGlobal,
    /// Never try the global mapper.
    ForceIncremental,
}

impl MapperPreference {
    pub const fn backend(self, global_available: bool) -> MapperBackend {
        match self {
            Self::PreferGlobal if global_available => MapperBackend::Global,
            _ => MapperBackend::Incremental,
        }
    }

    /// Pick the mapper for a run of `frames` images.
    ///
    /// The threshold scales with `feature_max_num_features`: a tier that extracts
    /// twice the features per image produces twice the observations, so it can
    /// afford roughly half the frames.
    pub const fn backend_for_frames(
        self,
        global_available: bool,
        frames: u64,
        feature_max_num_features: u32,
    ) -> MapperBackend {
        match self {
            Self::PreferGlobal
                if global_available
                    && frames <= global_mapper_frame_limit(feature_max_num_features) =>
            {
                MapperBackend::Global
            }
            _ => MapperBackend::Incremental,
        }
    }
}

/// Largest frame count for which the global mapper is still tried, for a tier that
/// extracts `feature_max_num_features` features per image.
///
/// The reference point is 600 frames at 8192 features — measured to be the last
/// size where the global mapper is the better choice on both axes. Framing it as a
/// total observation budget keeps the tiers that double the feature count honest.
pub const fn global_mapper_frame_limit(feature_max_num_features: u32) -> u64 {
    /// Observations the global mapper can still finish comfortably.
    const OBSERVATION_BUDGET: u64 = 600 * 8192;
    // `const fn` cannot use `.max()` (Ord is not const-stable), so spell it out.
    let per_image = if feature_max_num_features < 1 {
        1
    } else {
        feature_max_num_features as u64
    };
    let limit = OBSERVATION_BUDGET / per_image;
    if limit < 1 {
        1
    } else {
        limit
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Quality {
    Fast,
    #[default]
    Balanced,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QualityPreset {
    pub frame_retention_ratio: f64,
    pub brush_iterations: usize,
    pub brush_max_resolution: u32,
    pub mapper_backend: MapperPreference,
    pub sequential_overlap: u32,
    pub feature_max_image_size: u32,
    pub feature_max_num_features: u32,
}

impl Quality {
    pub const fn preset(self) -> QualityPreset {
        match self {
            Self::Fast => QualityPreset {
                frame_retention_ratio: 0.30,
                brush_iterations: 8_000,
                brush_max_resolution: 1_200,
                mapper_backend: MapperPreference::PreferGlobal,
                sequential_overlap: 12,
                feature_max_image_size: 1280,
                feature_max_num_features: 8192,
            },
            Self::Balanced => QualityPreset {
                frame_retention_ratio: 0.50,
                brush_iterations: 15_000,
                brush_max_resolution: 1_600,
                mapper_backend: MapperPreference::PreferGlobal,
                sequential_overlap: 15,
                feature_max_image_size: 1600,
                feature_max_num_features: 8192,
            },
            Self::High => QualityPreset {
                frame_retention_ratio: 1.00,
                brush_iterations: 30_000,
                brush_max_resolution: 2_000,
                mapper_backend: MapperPreference::PreferGlobal,
                sequential_overlap: 20,
                feature_max_image_size: 2000,
                feature_max_num_features: 16384,
            },
        }
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
    fn presets_are_centralized_and_exact() {
        assert_eq!(Quality::Fast.preset().frame_retention_ratio, 0.30);
        assert_eq!(Quality::Balanced.preset().frame_retention_ratio, 0.50);
        assert_eq!(Quality::High.preset().frame_retention_ratio, 1.00);
        assert_eq!(Quality::Fast.preset().brush_iterations, 8_000);
        assert_eq!(Quality::Balanced.preset().brush_max_resolution, 1_600);
    }

    #[test]
    fn balanced_is_default() {
        assert_eq!(Quality::default(), Quality::Balanced);
    }

    /// 按帧数切 mapper：小规模用 global，超规模退回增量。
    ///
    /// 依据（本机实测，Fast 参数）：299 帧 global 145 s 单模型；600 帧 global 432 s 单模型
    /// 94.8%（增量同期碎成 12 块、最大 23.3%）；1200 帧 global >68 分钟未完成（增量 45 分钟
    /// 跑完）；2121 帧 global >58 分钟被取消（增量 51 分钟跑完）。
    #[test]
    fn mapper_backend_switches_by_frame_count() {
        let global_available = true;
        for frames in [1_u64, 163, 299, 600] {
            assert_eq!(
                MapperPreference::PreferGlobal.backend_for_frames(global_available, frames, 8192),
                MapperBackend::Global,
                "{frames} 帧应使用 global mapper"
            );
        }
        for frames in [601_u64, 1200, 2121] {
            assert_eq!(
                MapperPreference::PreferGlobal.backend_for_frames(global_available, frames, 8192),
                MapperBackend::Incremental,
                "{frames} 帧应退回 incremental mapper"
            );
        }

        // 特征数翻倍的档位观测数也翻倍，可承受帧数减半。
        assert_eq!(global_mapper_frame_limit(8192), 600);
        assert_eq!(global_mapper_frame_limit(16_384), 300);
        assert_eq!(
            MapperPreference::PreferGlobal.backend_for_frames(global_available, 400, 16_384),
            MapperBackend::Incremental,
            "16384 特征档位在 400 帧就应退回增量"
        );

        // 引擎没有 global mapper 时无条件退回增量；ForceIncremental 永远不回 global。
        assert_eq!(
            MapperPreference::PreferGlobal.backend_for_frames(false, 100, 8192),
            MapperBackend::Incremental
        );
        assert_eq!(
            MapperPreference::ForceIncremental.backend_for_frames(true, 100, 8192),
            MapperBackend::Incremental
        );
    }

    #[test]
    fn every_preset_prefers_the_global_mapper() {
        for quality in [Quality::Fast, Quality::Balanced, Quality::High] {
            assert_eq!(
                quality.preset().mapper_backend,
                MapperPreference::PreferGlobal,
                "{quality:?} 应与其他档位一致地优先使用 global mapper"
            );
        }
        assert_eq!(
            MapperPreference::PreferGlobal.backend(false),
            MapperBackend::Incremental,
            "引擎缺少 global mapper 时必须退回增量"
        );
    }
}
