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

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QualityPreset {
    pub frame_retention_ratio: f64,
    /// Quality v2 initial video sampling target. `None` keeps every source frame.
    pub initial_fps: Option<f64>,
    /// Maximum video sampling budget available to one Bridge Backfill.
    pub rescue_max_fps: Option<f64>,
    pub sfm_max_image_size: u32,
    pub sfm_max_features: u32,
    pub brush_iterations: usize,
    pub brush_max_resolution: u32,
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
                brush_iterations: 8_000,
                brush_max_resolution: 1_200,
            },
            Self::Balanced => QualityPreset {
                frame_retention_ratio: 0.50,
                initial_fps: Some(8.0),
                rescue_max_fps: Some(12.0),
                sfm_max_image_size: 1_920,
                sfm_max_features: 8_192,
                brush_iterations: 15_000,
                brush_max_resolution: 1_600,
            },
            Self::High => QualityPreset {
                frame_retention_ratio: 1.00,
                initial_fps: None,
                rescue_max_fps: None,
                sfm_max_image_size: 1_920,
                sfm_max_features: 8_192,
                brush_iterations: 30_000,
                brush_max_resolution: 2_000,
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
        assert_eq!(Quality::Fast.preset().initial_fps, Some(6.0));
        assert_eq!(Quality::Fast.preset().rescue_max_fps, Some(9.0));
        assert_eq!(Quality::Balanced.preset().initial_fps, Some(8.0));
        assert_eq!(Quality::Balanced.preset().rescue_max_fps, Some(12.0));
        assert_eq!(Quality::High.preset().initial_fps, None);
        assert_eq!(Quality::Fast.preset().sfm_max_features, 4_096);
    }

    #[test]
    fn balanced_is_default() {
        assert_eq!(Quality::default(), Quality::Balanced);
    }
}
