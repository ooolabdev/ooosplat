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
    pub target_sampling_fps: f64,
    pub minimum_frames: u64,
    pub maximum_frames: u64,
    pub brush_iterations: usize,
    pub brush_max_resolution: u32,
    pub brush_max_splats: u32,
    pub brush_sh_degree: u8,
}

impl Quality {
    pub const fn preset(self) -> QualityPreset {
        match self {
            Self::Fast => QualityPreset {
                target_sampling_fps: 3.0,
                minimum_frames: 48,
                maximum_frames: 120,
                brush_iterations: 6_000,
                brush_max_resolution: 960,
                brush_max_splats: 600_000,
                brush_sh_degree: 2,
            },
            Self::Balanced => QualityPreset {
                target_sampling_fps: 5.0,
                minimum_frames: 72,
                maximum_frames: 240,
                brush_iterations: 10_000,
                brush_max_resolution: 1_200,
                brush_max_splats: 1_000_000,
                brush_sh_degree: 3,
            },
            Self::High => QualityPreset {
                target_sampling_fps: 8.0,
                minimum_frames: 120,
                maximum_frames: 400,
                brush_iterations: 15_000,
                brush_max_resolution: 1_600,
                brush_max_splats: 1_500_000,
                brush_sh_degree: 3,
            },
        }
    }
}

impl QualityPreset {
    /// Keeps enough headroom for the desktop compositor and WebGPU allocations on
    /// small laptop GPUs. Unknown/non-NVIDIA devices use the conservative 8 GB tier.
    pub const fn for_vram_mb(mut self, total_vram_mb: Option<u64>) -> Self {
        let vram = match total_vram_mb {
            Some(value) => value,
            None => 8_192,
        };
        if vram <= 6_500 {
            self.brush_max_resolution = if self.brush_max_resolution > 960 {
                960
            } else {
                self.brush_max_resolution
            };
            self.brush_max_splats = if self.brush_max_splats > 600_000 {
                600_000
            } else {
                self.brush_max_splats
            };
        } else if vram <= 9_000 {
            self.brush_max_resolution = if self.brush_max_resolution > 1_200 {
                1_200
            } else {
                self.brush_max_resolution
            };
            self.brush_max_splats = if self.brush_max_splats > 1_000_000 {
                1_000_000
            } else {
                self.brush_max_splats
            };
        }
        self
    }

    pub const fn degraded_for_oom(mut self) -> Self {
        self.brush_max_resolution = self.brush_max_resolution * 3 / 4;
        if self.brush_max_resolution < 720 {
            self.brush_max_resolution = 720;
        }
        self.brush_max_splats /= 2;
        if self.brush_max_splats < 300_000 {
            self.brush_max_splats = 300_000;
        }
        self
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
        assert_eq!(Quality::Fast.preset().target_sampling_fps, 3.0);
        assert_eq!(Quality::Balanced.preset().maximum_frames, 240);
        assert_eq!(Quality::High.preset().minimum_frames, 120);
        assert_eq!(Quality::Fast.preset().brush_iterations, 6_000);
        assert_eq!(Quality::Balanced.preset().brush_max_resolution, 1_200);
        assert_eq!(Quality::High.preset().brush_max_splats, 1_500_000);
    }

    #[test]
    fn small_vram_profile_caps_expensive_brush_settings() {
        let preset = Quality::High.preset().for_vram_mb(Some(8_151));
        assert_eq!(preset.brush_max_resolution, 1_200);
        assert_eq!(preset.brush_max_splats, 1_000_000);
        let retry = preset.degraded_for_oom();
        assert_eq!(retry.brush_max_resolution, 900);
        assert_eq!(retry.brush_max_splats, 500_000);
    }

    #[test]
    fn balanced_is_default() {
        assert_eq!(Quality::default(), Quality::Balanced);
    }
}
