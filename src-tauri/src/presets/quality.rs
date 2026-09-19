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

/// Brush training knobs that a preset may override.
///
/// Every field is optional and unset by default, so a preset only deviates from
/// Brush's own defaults where a measurement justified it. See `docs/brush_help.txt`
/// (produced by `scripts/probe_brush_help.sh`) for the flags the shipped binary
/// really exposes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BrushTuning {
    /// `Some(order)` overrides Brush's `--sh-degree` (default 3). Lowering the
    /// spherical-harmonics order speeds training up but drops view-dependent
    /// effects such as reflections and highlights.
    pub sh_degree: Option<u32>,
    /// `Some(step)` stops densification at that step. Brush stops at 15000 by
    /// default, so this may only ever shorten growth, never extend it.
    pub growth_stop_iter: Option<usize>,
    /// `Some(steps)` overrides Brush's `--refine-every` (default 200). Brush ties
    /// this value to how many images cover the scene, so leave it unset unless a
    /// measurement justifies changing it.
    pub refine_every: Option<usize>,
    /// `Some(count)` caps the gaussian count. Set too low it loses detail, so it
    /// stays unset unless a measurement justifies a cap.
    pub max_splats: Option<usize>,
    /// Export once at the end instead of every few thousand steps. Intermediate
    /// exports copy the whole splat set from device to host and, with a fixed
    /// export name, only overwrite the same file.
    pub single_export: bool,
}

impl BrushTuning {
    /// Brush's own defaults: every knob stays unset so nothing is overridden.
    pub const fn brush_defaults() -> Self {
        Self {
            sh_degree: None,
            growth_stop_iter: None,
            refine_every: None,
            max_splats: None,
            single_export: false,
        }
    }

    /// Tuning for the High preset, where the longest training run makes the
    /// savings worth the quality trade-off.
    pub const fn high_detail() -> Self {
        Self {
            // Keeps some view-dependent shading rather than dropping to degree 1.
            sh_degree: Some(2),
            // Shortens densification (Brush's default is 15000) while leaving
            // room for the late iterations that only refine existing splats.
            growth_stop_iter: Some(12_000),
            refine_every: None,
            max_splats: None,
            single_export: true,
        }
    }

    /// Whether any knob overrides Brush's defaults.
    pub const fn is_brush_defaults(self) -> bool {
        self.sh_degree.is_none()
            && self.growth_stop_iter.is_none()
            && self.refine_every.is_none()
            && self.max_splats.is_none()
            && !self.single_export
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QualityPreset {
    pub frame_retention_ratio: f64,
    pub brush_iterations: usize,
    pub brush_max_resolution: u32,
    pub brush_tuning: BrushTuning,
}

impl Quality {
    pub const fn preset(self) -> QualityPreset {
        match self {
            Self::Fast => QualityPreset {
                frame_retention_ratio: 0.30,
                brush_iterations: 8_000,
                brush_max_resolution: 1_200,
                brush_tuning: BrushTuning::brush_defaults(),
            },
            Self::Balanced => QualityPreset {
                frame_retention_ratio: 0.50,
                brush_iterations: 15_000,
                brush_max_resolution: 1_600,
                brush_tuning: BrushTuning::brush_defaults(),
            },
            Self::High => QualityPreset {
                frame_retention_ratio: 1.00,
                brush_iterations: 30_000,
                brush_max_resolution: 2_000,
                // 只有 High 档的调优偏离 Brush 默认值：它的训练最久，省下的时间最划算。
                brush_tuning: BrushTuning::high_detail(),
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

    /// 训练调优只作用于 High 档：其余档位必须与 Brush 默认值逐项一致。
    ///
    /// 这条是刻意的闸门——调优会牺牲质量换时间，只有训练最久的 High 档值得，
    /// 将来若扩大范围必须先改这里。
    #[test]
    fn brush_tuning_is_gated_to_the_high_preset() {
        for quality in [Quality::Fast, Quality::Balanced] {
            let tuning = quality.preset().brush_tuning;
            assert!(
                tuning.is_brush_defaults(),
                "{quality:?} 不应偏离 Brush 默认训练参数：{tuning:?}"
            );
        }
        let high = Quality::High.preset().brush_tuning;
        assert!(!high.is_brush_defaults(), "High 档应启用调优");
        assert_eq!(high.sh_degree, Some(2));
        assert_eq!(high.growth_stop_iter, Some(12_000));
        assert!(high.single_export);
    }
}
