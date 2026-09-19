use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

use crate::video::FrameFilterConfig;

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
    pub enable_smart_filter: bool,
    pub smart_filter_config: FrameFilterConfig,
    /// 期望的**最终**保留密度（帧/秒）。`None` 表示沿用 `frame_retention_ratio × 源帧率`。
    /// 它参与 L0 的三段式约束 `target_fps <= candidate_fps <= source_fps`，
    /// 但**不直接**决定最终保留数——那仍由 `smart_filter_config` 的窗口配额决定。
    pub target_fps: Option<f64>,
    /// 送进智能筛选前的**候选**密度（帧/秒）。`None` 表示沿用
    /// `target_fps × 1.5` 的超采样比例。
    ///
    /// 绝对帧率与比例的区别很重要：比例会随源帧率线性放大（60fps 源会抽到 27 帧/秒），
    /// 绝对帧率则在任何源帧率下都给出同一个候选密度。
    pub candidate_fps: Option<f64>,
}

impl Quality {
    pub const fn preset(self) -> QualityPreset {
        match self {
            Self::Fast => QualityPreset {
                frame_retention_ratio: 0.30,
                brush_iterations: 8_000,
                brush_max_resolution: 1_200,
                enable_smart_filter: true,
                smart_filter_config: FrameFilterConfig::fast(),
                // 绝对帧率：任何源帧率下都给出同一密度（比例公式在 60fps 源上会放大到 27 帧/秒）。
                target_fps: Some(6.0),
                candidate_fps: Some(12.0),
            },
            Self::Balanced => QualityPreset {
                frame_retention_ratio: 0.50,
                brush_iterations: 15_000,
                brush_max_resolution: 1_600,
                enable_smart_filter: true,
                smart_filter_config: FrameFilterConfig::balanced(),
                target_fps: Some(8.0),
                candidate_fps: Some(20.0),
            },
            Self::High => QualityPreset {
                frame_retention_ratio: 1.00,
                brush_iterations: 30_000,
                brush_max_resolution: 2_000,
                enable_smart_filter: true,
                smart_filter_config: FrameFilterConfig::high(),
                target_fps: Some(10.0),
                candidate_fps: Some(30.0),
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
}
