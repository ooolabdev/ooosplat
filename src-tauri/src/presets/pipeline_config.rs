use std::{env, fs, sync::OnceLock};

use serde::Deserialize;

use super::{BrushDensificationPreset, BrushTrainingProfile, Quality};

pub const PIPELINE_CONFIG_ENV: &str = "OOOSPLAT_PIPELINE_CONFIG";
const EMBEDDED_CONFIG: &str = include_str!("../../../config/pipeline-optimization.json");

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PipelineOptimizationConfig {
    #[serde(rename = "_comment", default)]
    pub comment: Option<BilingualComment>,
    pub schema_version: u32,
    pub shared: SharedConfig,
    pub caspar: CasparConfig,
    pub high_vram: HighVramConfig,
    pub qualities: QualityConfigs,
    pub brush_profiles: BrushProfileConfigs,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BilingualComment {
    #[serde(rename = "zh-CN")]
    pub zh_cn: String,
    pub en: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedConfig {
    #[serde(rename = "_comment", default)]
    pub comment: Option<BilingualComment>,
    pub minimum_selected_frames: u64,
    pub bridge_trigger_ratio: f64,
    pub good_registration_ratio: f64,
    pub brush_refine_every: u32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CasparConfig {
    #[serde(rename = "_comment", default)]
    pub comment: Option<BilingualComment>,
    pub minimum_keypoints: u64,
    pub minimum_verified_matches: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HighVramConfig {
    #[serde(rename = "_comment", default)]
    pub comment: Option<BilingualComment>,
    pub standard_minimum_mi_b: u64,
    pub large_minimum_mi_b: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QualityConfigs {
    #[serde(rename = "_comment", default)]
    pub comment: Option<BilingualComment>,
    pub fast: QualityConfig,
    pub balanced: QualityConfig,
    pub high: QualityConfig,
}

impl QualityConfigs {
    pub fn get(&self, quality: Quality) -> &QualityConfig {
        match quality {
            Quality::Fast => &self.fast,
            Quality::Balanced => &self.balanced,
            Quality::High => &self.high,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QualityConfig {
    #[serde(rename = "_comment", default)]
    pub comment: Option<BilingualComment>,
    pub automatic_optimization_off: LegacyQualityConfig,
    pub automatic_optimization_on: AutomaticQualityConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LegacyQualityConfig {
    #[serde(rename = "_comment", default)]
    pub comment: Option<BilingualComment>,
    pub frame_retention_ratio: f64,
    pub incremental_sfm_max_image_size: u32,
    pub incremental_sfm_max_features: u32,
    pub brush: LegacyBrushConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LegacyBrushConfig {
    pub total_steps: usize,
    pub max_resolution: u32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomaticQualityConfig {
    #[serde(rename = "_comment", default)]
    pub comment: Option<BilingualComment>,
    pub initial_fps: f64,
    pub rescue_max_fps: f64,
    pub sfm_max_features: u32,
    pub allow_two_view_tracks: bool,
    pub initial_brush_profile: BrushTrainingProfile,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrushProfileConfigs {
    #[serde(rename = "_comment", default)]
    pub comment: Option<BilingualComment>,
    pub fast: BrushProfileConfig,
    pub balanced: BrushProfileConfig,
    pub high_low: BrushProfileConfig,
    pub high_standard: BrushProfileConfig,
    pub high_large: BrushProfileConfig,
    pub high_emergency: BrushProfileConfig,
}

impl BrushProfileConfigs {
    pub fn get(&self, profile: BrushTrainingProfile) -> &BrushProfileConfig {
        match profile {
            BrushTrainingProfile::Fast => &self.fast,
            BrushTrainingProfile::Balanced => &self.balanced,
            BrushTrainingProfile::HighLow => &self.high_low,
            BrushTrainingProfile::HighStandard => &self.high_standard,
            BrushTrainingProfile::HighLarge => &self.high_large,
            BrushTrainingProfile::HighEmergency => &self.high_emergency,
            BrushTrainingProfile::Legacy => {
                unreachable!("legacy Brush settings are stored per quality")
            }
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrushProfileConfig {
    #[serde(rename = "_comment", default)]
    pub comment: Option<BilingualComment>,
    pub working_max_long_edge: ResolutionValue,
    pub sfm_max_image_size: u32,
    pub total_steps: usize,
    pub max_resolution: ResolutionValue,
    pub max_splats: Option<u32>,
    pub densification: Option<BrushDensificationPreset>,
    pub oom_fallback: Option<BrushTrainingProfile>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(untagged)]
pub enum ResolutionValue {
    Fixed(u32),
    Named(SourceResolution),
}

impl ResolutionValue {
    pub fn resolve(self, source_long_edge: u32) -> u32 {
        match self {
            Self::Fixed(value) => value,
            Self::Named(SourceResolution::Source) => source_long_edge.max(1),
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceResolution {
    Source,
}

static CONFIG: OnceLock<PipelineOptimizationConfig> = OnceLock::new();

pub fn pipeline_optimization_config() -> &'static PipelineOptimizationConfig {
    CONFIG.get_or_init(|| match env::var_os(PIPELINE_CONFIG_ENV) {
        Some(path) => match fs::read_to_string(&path)
            .map_err(|error| error.to_string())
            .and_then(|json| parse_and_validate(&json))
        {
            Ok(config) => config,
            Err(error) => {
                eprintln!(
                    "OOOSplat ignored invalid {PIPELINE_CONFIG_ENV} override '{}': {error}; using embedded defaults",
                    std::path::Path::new(&path).display()
                );
                embedded_config()
            }
        },
        None => embedded_config(),
    })
}

fn embedded_config() -> PipelineOptimizationConfig {
    parse_and_validate(EMBEDDED_CONFIG).expect("embedded pipeline-optimization.json must be valid")
}

fn parse_and_validate(json: &str) -> Result<PipelineOptimizationConfig, String> {
    let config: PipelineOptimizationConfig =
        serde_json::from_str(json).map_err(|error| error.to_string())?;
    config.validate()?;
    Ok(config)
}

impl PipelineOptimizationConfig {
    fn validate(&self) -> Result<(), String> {
        if self.schema_version != 1 {
            return Err(format!(
                "unsupported schemaVersion {}; expected 1",
                self.schema_version
            ));
        }
        if self.shared.minimum_selected_frames == 0 {
            return Err("shared.minimumSelectedFrames must be greater than zero".into());
        }
        if !(0.0..=1.0).contains(&self.shared.bridge_trigger_ratio)
            || !self.shared.bridge_trigger_ratio.is_finite()
        {
            return Err("shared.bridgeTriggerRatio must be finite and between 0 and 1".into());
        }
        if !(0.0..=1.0).contains(&self.shared.good_registration_ratio)
            || !self.shared.good_registration_ratio.is_finite()
        {
            return Err("shared.goodRegistrationRatio must be finite and between 0 and 1".into());
        }
        if self.shared.brush_refine_every == 0 {
            return Err("shared.brushRefineEvery must be greater than zero".into());
        }
        if self.caspar.minimum_keypoints == 0 || self.caspar.minimum_verified_matches == 0 {
            return Err("Caspar thresholds must be greater than zero".into());
        }
        if self.high_vram.standard_minimum_mi_b == 0
            || self.high_vram.large_minimum_mi_b <= self.high_vram.standard_minimum_mi_b
        {
            return Err(
                "highVram.largeMinimumMiB must be greater than standardMinimumMiB > 0".into(),
            );
        }
        for (name, quality) in [
            ("fast", &self.qualities.fast),
            ("balanced", &self.qualities.balanced),
            ("high", &self.qualities.high),
        ] {
            quality.validate(name)?;
        }
        for profile in [
            BrushTrainingProfile::Fast,
            BrushTrainingProfile::Balanced,
            BrushTrainingProfile::HighLow,
            BrushTrainingProfile::HighStandard,
            BrushTrainingProfile::HighLarge,
            BrushTrainingProfile::HighEmergency,
        ] {
            let value = self.brush_profiles.get(profile);
            value.validate(profile.label())?;
            if matches!(
                profile,
                BrushTrainingProfile::Fast | BrushTrainingProfile::Balanced
            ) && value.oom_fallback.is_some()
            {
                return Err(format!(
                    "brushProfiles.{}.oomFallback must be null because OOM retry is High-only",
                    profile.label()
                ));
            }
        }
        for start in [
            BrushTrainingProfile::HighLow,
            BrushTrainingProfile::HighStandard,
            BrushTrainingProfile::HighLarge,
            BrushTrainingProfile::HighEmergency,
        ] {
            let mut visited = Vec::new();
            let mut current = Some(start);
            while let Some(profile) = current {
                if !matches!(
                    profile,
                    BrushTrainingProfile::HighLow
                        | BrushTrainingProfile::HighStandard
                        | BrushTrainingProfile::HighLarge
                        | BrushTrainingProfile::HighEmergency
                ) {
                    return Err(format!(
                        "brushProfiles.{}.oomFallback must target a High profile",
                        visited.last().copied().unwrap_or(start).label()
                    ));
                }
                if visited.contains(&profile) {
                    return Err(format!(
                        "brushProfiles OOM fallback chain contains a cycle at {}",
                        profile.label()
                    ));
                }
                visited.push(profile);
                let value = self.brush_profiles.get(profile);
                if value.max_splats.is_none() || value.densification.is_none() {
                    return Err(format!(
                        "brushProfiles.{} must define maxSplats and densification",
                        profile.label()
                    ));
                }
                current = value.oom_fallback;
            }
        }
        Ok(())
    }
}

impl QualityConfig {
    fn validate(&self, name: &str) -> Result<(), String> {
        let legacy = &self.automatic_optimization_off;
        if !(0.0..=1.0).contains(&legacy.frame_retention_ratio)
            || !legacy.frame_retention_ratio.is_finite()
        {
            return Err(format!(
                "qualities.{name}.automaticOptimizationOff.frameRetentionRatio must be finite and between 0 and 1"
            ));
        }
        if legacy.incremental_sfm_max_image_size == 0
            || legacy.incremental_sfm_max_features == 0
            || legacy.brush.total_steps == 0
            || legacy.brush.max_resolution == 0
        {
            return Err(format!(
                "qualities.{name}.automaticOptimizationOff numeric limits must be greater than zero"
            ));
        }
        let automatic = &self.automatic_optimization_on;
        if !automatic.initial_fps.is_finite()
            || automatic.initial_fps <= 0.0
            || !automatic.rescue_max_fps.is_finite()
            || automatic.rescue_max_fps < automatic.initial_fps
            || automatic.sfm_max_features == 0
        {
            return Err(format!(
                "qualities.{name}.automaticOptimizationOn requires 0 < initialFps <= rescueMaxFps and sfmMaxFeatures > 0"
            ));
        }
        if matches!(
            automatic.initial_brush_profile,
            BrushTrainingProfile::Legacy
        ) {
            return Err(format!(
                "qualities.{name}.automaticOptimizationOn.initialBrushProfile cannot be legacy"
            ));
        }
        Ok(())
    }
}

impl BrushProfileConfig {
    fn validate(&self, name: &str) -> Result<(), String> {
        if self.sfm_max_image_size == 0 || self.total_steps == 0 {
            return Err(format!(
                "brushProfiles.{name} image and iteration limits must be greater than zero"
            ));
        }
        for (field, value) in [
            ("workingMaxLongEdge", self.working_max_long_edge),
            ("maxResolution", self.max_resolution),
        ] {
            if matches!(value, ResolutionValue::Fixed(0)) {
                return Err(format!(
                    "brushProfiles.{name}.{field} must be greater than zero"
                ));
            }
        }
        if let Some(densification) = self.densification {
            if !densification.growth_grad_threshold.is_finite()
                || densification.growth_grad_threshold <= 0.0
                || !densification.growth_select_fraction.is_finite()
                || !(0.0..=1.0).contains(&densification.growth_select_fraction)
                || densification.growth_stop_iter == 0
                || densification.growth_stop_iter as usize > self.total_steps
            {
                return Err(format!(
                    "brushProfiles.{name}.densification contains an invalid threshold, fraction, or stop iteration"
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_configuration_is_valid() {
        let config = parse_and_validate(EMBEDDED_CONFIG).unwrap();
        assert_eq!(config.schema_version, 1);
        assert_eq!(config.caspar.minimum_keypoints, 2_000_000);
        assert_eq!(
            config.qualities.fast.automatic_optimization_on.initial_fps,
            6.0
        );
    }

    #[test]
    fn unknown_fields_and_invalid_ranges_are_rejected() {
        let unknown = EMBEDDED_CONFIG.replacen(
            "\"schemaVersion\": 1",
            "\"schemaVersion\": 1, \"typo\": true",
            1,
        );
        assert!(parse_and_validate(&unknown).is_err());

        let invalid = EMBEDDED_CONFIG.replacen(
            "\"bridgeTriggerRatio\": 0.8",
            "\"bridgeTriggerRatio\": 1.5",
            1,
        );
        assert!(parse_and_validate(&invalid).is_err());
    }
}
