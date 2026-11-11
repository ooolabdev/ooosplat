//! Versioned defaults for new runs. Absence in a saved state means legacy behavior.
use serde::{Deserialize, Serialize};

use crate::{
    error::{Result, SplatError},
    presets::{self, PlannerResolutionPlan, Quality, ResolvedBrushTrainingPreset},
    video::{
        minimum_selected_frames, FramePlan, FrameSelectionStrategy, QualityV2FrameSelection,
        VideoInfo,
    },
};

pub const VERSION: u32 = 7;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct GenerationPolicy {
    pub version: u32,
    pub auto_retry_enabled: bool,
    /// None retains the exact tier FPS default, without UI rounding.
    pub frame_sampling_ratio: Option<f64>,
    /// Version 4 shared target. None retains the exact per-tier FPS default.
    pub frame_sampling_fps: Option<f64>,
}

impl Default for GenerationPolicy {
    fn default() -> Self {
        Self {
            version: VERSION,
            auto_retry_enabled: true,
            frame_sampling_ratio: None,
            frame_sampling_fps: None,
        }
    }
}

impl GenerationPolicy {
    fn default_tier_fps(quality: Quality) -> f64 {
        match quality {
            Quality::Fast => 6.0,
            Quality::Balanced => 8.0,
            Quality::High => 12.0,
        }
    }

    fn removal_frame_plan(
        self,
        video: &VideoInfo,
        quality: Quality,
        removal_fps: Option<f64>,
    ) -> FramePlan {
        let mut preset = quality.preset();
        let minimum_target = if self.version >= 6 {
            self.frame_sampling_bounds(video)
                .map(|(minimum, _)| minimum)
                .unwrap_or(video.fps)
        } else {
            0.1
        };
        let maximum_removal = (video.fps - minimum_target).max(0.0);
        let removal = removal_fps
            .unwrap_or_else(|| {
                if self.version >= 6 {
                    video.fps - Self::default_tier_fps(quality).min(video.fps)
                } else {
                    Self::default_tier_fps(quality)
                }
            })
            .clamp(0.0, maximum_removal);
        if self.version >= 6 && removal <= f64::EPSILON * video.fps.max(1.0) {
            return QualityV2FrameSelection.create_all_frames_plan(video, removal_fps.is_some());
        }
        preset.initial_fps = Some((video.fps - removal).max(minimum_target).min(video.fps));
        if removal_fps.is_some() {
            QualityV2FrameSelection.create_manual_bridge_plan(video, &preset)
        } else {
            QualityV2FrameSelection.create_plan(video, &preset)
        }
    }

    pub fn oom_retry(
        self,
        resolved: ResolvedBrushTrainingPreset,
        consumed: bool,
        last_count: Option<u64>,
        initial: u64,
    ) -> Option<ResolvedBrushTrainingPreset> {
        if !self.auto_retry_enabled || consumed {
            return None;
        }
        oom_retry_preset(resolved, last_count, initial)
    }
    pub fn validate(self) -> Result<Self> {
        if !(1..=VERSION).contains(&self.version)
            || (self.frame_sampling_fps.is_some()
                && (self.version < 4 || self.frame_sampling_ratio.is_some()))
            || self
                .frame_sampling_fps
                .is_some_and(|fps| !fps.is_finite() || fps < 0.1)
            || self
                .frame_sampling_ratio
                .is_some_and(|r| !r.is_finite() || !(0.001..=1.0).contains(&r))
        {
            return Err(SplatError::Process("Invalid generation policy".into()));
        }
        Ok(self)
    }

    pub fn frame_plan(self, video: &VideoInfo, quality: Quality) -> FramePlan {
        if self.version >= 5
            && self.frame_sampling_ratio.is_none()
            && self.frame_sampling_fps.is_none()
        {
            return self.removal_frame_plan(video, quality, None);
        }
        let mut preset = quality.preset();
        if let Some(ratio) = self.frame_sampling_ratio {
            preset.initial_fps = Some(video.fps * ratio);
        }
        if let Some(fps) = self.frame_sampling_fps {
            preset.initial_fps = Some(fps.min(video.fps));
        }
        if self.version >= 3
            && (self.frame_sampling_ratio.is_some() || self.frame_sampling_fps.is_some())
        {
            QualityV2FrameSelection.create_manual_bridge_plan(video, &preset)
        } else {
            QualityV2FrameSelection.create_plan(video, &preset)
        }
    }

    pub fn frame_plan_for_material(
        self,
        video: &VideoInfo,
        quality: Quality,
        removal_fps: Option<f64>,
    ) -> Result<FramePlan> {
        if self.version < 5 {
            return Ok(self.frame_plan(video, quality));
        }
        if self.frame_sampling_ratio.is_some() || self.frame_sampling_fps.is_some() {
            if removal_fps.is_some() {
                return Err(SplatError::Process(
                    "Per-material removal FPS conflicts with the legacy sampling setting".into(),
                ));
            }
            return Ok(self.frame_plan(video, quality));
        }
        let invalid_removal = if self.version >= 6 {
            let maximum = self
                .frame_sampling_bounds(video)
                .map(|(minimum, _)| (video.fps - minimum).max(0.0))
                .unwrap_or(0.0);
            removal_fps.is_some_and(|fps| !fps.is_finite() || fps < 0.0 || fps > maximum)
        } else {
            removal_fps.is_some_and(|fps| {
                !fps.is_finite() || fps < 0.1 || fps > (video.fps - 0.1).max(0.1)
            })
        };
        if invalid_removal {
            return Err(SplatError::Process("Invalid frame removal FPS".into()));
        }
        Ok(self.removal_frame_plan(video, quality, removal_fps))
    }

    /// Returns the requested sampling-FPS interval for the version-6 removal
    /// control. Actual displayed endpoints still come from `FramePlan`, since
    /// frame counts, VFR timing and rounding can make the effective FPS differ.
    pub fn frame_sampling_bounds(self, video: &VideoInfo) -> Option<(f64, f64)> {
        if self.version < 6
            || !video.fps.is_finite()
            || video.fps <= 0.0
            || !video.duration.is_finite()
            || video.duration <= 0.0
        {
            return None;
        }
        let minimum = (minimum_selected_frames() as f64 / video.duration)
            .max(1.0)
            .min(video.fps);
        Some((minimum, video.fps))
    }

    pub fn resolution(
        self,
        quality: Quality,
        memory: Option<u64>,
        width: u32,
        height: u32,
        video: bool,
    ) -> PlannerResolutionPlan {
        // HighLarge is the existing native-resolution recipe. Hardware no longer
        // selects a lower High profile; legacy states keep their saved profile.
        presets::resolve_planner_resolution_plan(
            quality,
            if quality == Quality::High {
                Some(u64::MAX)
            } else {
                memory
            },
            width,
            height,
            video,
        )
    }

    pub fn training(
        self,
        quality: Quality,
        memory: Option<u64>,
        resolution: &PlannerResolutionPlan,
        sparse_points: u64,
        initial_points: u64,
        fine_budget: Option<&super::splat_budget::FineSplatBudget>,
    ) -> Result<ResolvedBrushTrainingPreset> {
        let max_resolution = if quality == Quality::High {
            resolution.working_long_edge()
        } else {
            resolution.brush_initial_max_resolution.max(1)
        };
        self.training_with_max_resolution(
            quality,
            memory,
            resolution,
            sparse_points,
            initial_points,
            fine_budget,
            max_resolution,
        )
    }

    /// The shared dataset runner may retain larger prepared views than the
    /// legacy recipe. Budget the pixels Brush will actually load, without
    /// changing either path's existing resolution policy.
    #[allow(clippy::too_many_arguments)]
    pub fn training_with_max_resolution(
        self,
        quality: Quality,
        memory: Option<u64>,
        resolution: &PlannerResolutionPlan,
        sparse_points: u64,
        initial_points: u64,
        fine_budget: Option<&super::splat_budget::FineSplatBudget>,
        max_resolution: u32,
    ) -> Result<ResolvedBrushTrainingPreset> {
        u32::try_from(initial_points).map_err(|_| {
            SplatError::Process("Initialization exceeds the supported Splat count range".into())
        })?;
        let mut resolved = presets::resolve_brush_training_preset_for_plan(
            quality,
            memory,
            resolution.working_long_edge(),
            initial_points,
            resolution,
        );
        resolved.preset.max_resolution = max_resolution.max(1);
        if quality == Quality::High {
            if self.version >= 2 {
                let budget = fine_budget.ok_or_else(|| {
                    SplatError::Process("Detailed Splat budget is missing or stale".into())
                })?;
                apply_budget(
                    &mut resolved,
                    budget.validated_ceiling(sparse_points)?,
                    initial_points,
                )?;
            } else {
                apply_sparse_budget(&mut resolved, sparse_points, initial_points)?;
            }
        } else if self.requires_pixel_budget(quality) {
            let budget = fine_budget
                .ok_or_else(|| SplatError::Process("Splat budget is missing or stale".into()))?;
            let requested = budget.validated_ceiling_for_quality(
                sparse_points,
                quality,
                resolved.preset.max_resolution,
            )?;
            apply_budget(&mut resolved, requested, initial_points)?;
        }
        Ok(resolved)
    }

    pub fn requires_pixel_budget(self, quality: Quality) -> bool {
        if quality == Quality::High {
            self.version >= 2
        } else {
            self.version >= 7
        }
    }

    pub fn panorama_training(
        self,
        quality: Quality,
        memory: Option<u64>,
        face_size: u32,
        sparse_points: u64,
        initial_points: u64,
    ) -> Result<ResolvedBrushTrainingPreset> {
        let mut resolved = presets::resolve_panorama_training_preset(
            quality,
            true,
            memory,
            face_size,
            if quality == Quality::High {
                0
            } else {
                initial_points
            },
        )?;
        if quality == Quality::High {
            apply_sparse_budget(&mut resolved, sparse_points, initial_points)?;
            let snapshot = resolved.panorama.as_mut().expect("panorama recipe");
            snapshot.recipe_version = 2;
            snapshot.memory_max_splats = None;
            snapshot.initial_points = initial_points;
            snapshot.initial_points_override = initial_points > sparse_points * 40;
        }
        Ok(resolved)
    }
}

fn apply_sparse_budget(
    resolved: &mut ResolvedBrushTrainingPreset,
    sparse: u64,
    initial: u64,
) -> Result<()> {
    let requested = sparse
        .checked_mul(40)
        .filter(|n| *n > 0)
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| {
            SplatError::Process(
                "COLMAP sparse point budget exceeds the supported Splat count range".into(),
            )
        })?;
    apply_budget(resolved, requested, initial)
}

fn apply_budget(
    resolved: &mut ResolvedBrushTrainingPreset,
    requested: u32,
    initial: u64,
) -> Result<()> {
    let initial = u32::try_from(initial).map_err(|_| {
        SplatError::Process("Initialization exceeds the supported Splat count range".into())
    })?;
    resolved.configured_max_splats = Some(requested);
    resolved.preset.max_splats = Some(requested.max(initial));
    Ok(())
}

/// No guessing from a configured ceiling, other process, or historical count.
pub fn oom_retry_preset(
    resolved: ResolvedBrushTrainingPreset,
    last_count: Option<u64>,
    initial: u64,
) -> Option<ResolvedBrushTrainingPreset> {
    let last = last_count?;
    let cap = u32::try_from(last.checked_mul(4)? / 5).ok()?;
    if cap == 0
        || u64::from(cap) < initial
        || resolved.preset.max_splats.is_some_and(|old| cap >= old)
    {
        return None;
    }
    let mut retry = resolved;
    retry.preset.max_splats = Some(cap);
    Some(retry)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn budget(points: u64) -> super::super::splat_budget::FineSplatBudget {
        super::super::splat_budget::FineSplatBudget {
            version: super::super::splat_budget::VERSION,
            coefficient: 0.5,
            registered_images: 10,
            effective_pixels: 100_000_000,
            sparse_points: points,
            tracked_points: points,
            observations: points * 5,
            mean_track_length: 5.,
            requested_max_splats: super::super::splat_budget::ceiling(
                100_000_000,
                points,
                points * 5,
            )
            .unwrap(),
            quality: None,
            training_max_resolution: None,
        }
    }
    fn budget_for(
        quality: Quality,
        points: u64,
        max_resolution: u32,
    ) -> super::super::splat_budget::FineSplatBudget {
        let mut value = budget(points);
        if quality != Quality::High {
            value.version = super::super::splat_budget::TIER_VERSION;
            value.coefficient = super::super::splat_budget::coefficient(quality);
            value.quality = Some(quality);
            value.training_max_resolution = Some(max_resolution);
            value.requested_max_splats = super::super::splat_budget::ceiling_for_quality(
                value.effective_pixels,
                value.tracked_points,
                value.observations,
                quality,
            )
            .unwrap();
        }
        value
    }
    fn video(frames: u64, fps: f64) -> VideoInfo {
        VideoInfo {
            duration: frames as f64 / fps,
            fps,
            total_frames: frames,
            width: 1920,
            height: 1080,
            codec: "h264".into(),
            rotation: 0,
            pixel_format: "rgb24".into(),
            has_alpha: false,
        }
    }
    #[test]
    fn version_five_uses_tier_removal_defaults_and_keeps_legacy_targets() {
        let policy = GenerationPolicy {
            version: 5,
            ..GenerationPolicy::default()
        };
        for (quality, selected) in [
            (Quality::Fast, 1440),
            (Quality::Balanced, 1320),
            (Quality::High, 1080),
        ] {
            assert_eq!(
                policy
                    .frame_plan(&video(1800, 30.), quality)
                    .estimated_frames,
                selected
            );
            let previous = GenerationPolicy {
                version: 4,
                ..policy
            };
            assert_eq!(
                previous
                    .frame_plan(&video(1800, 30.), quality)
                    .estimated_frames,
                match quality {
                    Quality::Fast => 360,
                    Quality::Balanced => 480,
                    Quality::High => 720,
                }
            );
        }
        let manual = GenerationPolicy {
            frame_sampling_fps: Some(6.),
            ..policy
        };
        let a = manual.frame_plan(&video(1800, 30.), Quality::High);
        assert_eq!(a.selected_frames.len(), 360);
        assert_eq!(a.rescue_max_frames, 432);
        let b = manual.frame_plan(&video(40, 2.), Quality::High);
        assert_eq!(b.selected_frames.len(), 40);
        assert_eq!(b.rescue_max_frames, 40);
        let short = manual.frame_plan(&video(20, 30.), Quality::Fast);
        assert_eq!(short.selected_frames.len(), 20);
        assert!(GenerationPolicy {
            frame_sampling_fps: Some(8.),
            frame_sampling_ratio: Some(0.5),
            ..policy
        }
        .validate()
        .is_err());
        for fps in [0., 0.01, f64::NAN, f64::INFINITY] {
            assert!(GenerationPolicy {
                frame_sampling_fps: Some(fps),
                ..policy
            }
            .validate()
            .is_err());
        }
    }
    #[test]
    fn per_material_removal_uses_manual_bridge_budget_and_validates_range() {
        let policy = GenerationPolicy::default();
        let clip = video(1800, 30.);
        let plan = policy
            .frame_plan_for_material(&clip, Quality::Balanced, Some(8.))
            .unwrap();
        assert_eq!(plan.estimated_frames, 1320);
        assert_eq!(plan.rescue_max_frames, 1584);
        assert_eq!(plan.bridge_source_frames, Some(1800));
        assert!(policy
            .frame_plan_for_material(&clip, Quality::Balanced, Some(30.))
            .is_err());
        let legacy = GenerationPolicy {
            version: 4,
            ..policy
        };
        assert_eq!(
            legacy
                .frame_plan_for_material(&clip, Quality::Balanced, Some(8.))
                .unwrap(),
            legacy.frame_plan(&clip, Quality::Balanced)
        );
    }

    #[test]
    fn current_policy_keeps_version_six_sampling_bounds_and_can_keep_every_frame() {
        let policy = GenerationPolicy::default();
        assert_eq!(policy.version, VERSION);
        assert!(policy.validate().is_ok());
        assert!(GenerationPolicy {
            version: VERSION + 1,
            ..policy
        }
        .validate()
        .is_err());

        let long = video(1800, 30.);
        for (quality, expected_fps, expected_frames) in [
            (Quality::Fast, 6., 360),
            (Quality::Balanced, 8., 480),
            (Quality::High, 12., 720),
        ] {
            let plan = policy.frame_plan(&long, quality);
            assert_eq!(plan.sampling_fps, expected_fps);
            assert_eq!(plan.estimated_frames, expected_frames);
            assert_eq!(
                plan,
                GenerationPolicy {
                    version: 6,
                    ..policy
                }
                .frame_plan(&long, quality)
            );
        }
        let (minimum, maximum) = policy.frame_sampling_bounds(&long).unwrap();
        assert_eq!((minimum, maximum), (1., 30.));
        let low = policy
            .frame_plan_for_material(&long, Quality::Balanced, Some(long.fps - minimum))
            .unwrap();
        assert_eq!(low.estimated_frames, 60);
        assert_eq!(low.sampling_fps, 1.);
        let all = policy
            .frame_plan_for_material(&long, Quality::Balanced, Some(0.))
            .unwrap();
        assert_eq!(all.estimated_frames, 1800);
        assert_eq!(all.sampling_fps, 30.);
        assert_eq!(all.retention_ratio, 1.);
        assert_eq!(all.rescue_max_frames, 1800);
        assert_eq!(all.bridge_source_frames, Some(1800));

        let inconsistent_container_timing = VideoInfo {
            duration: 60.2,
            ..long.clone()
        };
        let all = policy
            .frame_plan_for_material(&inconsistent_container_timing, Quality::Balanced, Some(0.))
            .unwrap();
        assert_eq!(all.estimated_frames, 1800);
        assert_eq!(all.sampling_fps, 30.);
        assert_eq!(all.retention_ratio, 1.);
        assert_eq!(all.rescue_max_frames, 1800);

        let ten_seconds = video(300, 30.);
        let (minimum, maximum) = policy.frame_sampling_bounds(&ten_seconds).unwrap();
        assert_eq!((minimum, maximum), (3., 30.));
        assert_eq!(
            policy
                .frame_plan_for_material(
                    &ten_seconds,
                    Quality::Fast,
                    Some(ten_seconds.fps - minimum),
                )
                .unwrap()
                .estimated_frames,
            30
        );

        let fewer_than_minimum = video(20, 30.);
        let (minimum, maximum) = policy.frame_sampling_bounds(&fewer_than_minimum).unwrap();
        assert_eq!((minimum, maximum), (30., 30.));
        let only = policy
            .frame_plan_for_material(&fewer_than_minimum, Quality::High, Some(0.))
            .unwrap();
        assert_eq!(only.estimated_frames, 20);
        assert_eq!(only.retention_ratio, 1.);
    }

    #[test]
    fn version_five_keeps_the_legacy_removal_range() {
        let current = GenerationPolicy::default();
        let legacy = GenerationPolicy {
            version: 5,
            ..current
        };
        let clip = video(1800, 30.);
        assert!(legacy.frame_sampling_bounds(&clip).is_none());
        assert!(legacy
            .frame_plan_for_material(&clip, Quality::Balanced, Some(0.))
            .is_err());
        let plan = legacy
            .frame_plan_for_material(&clip, Quality::Balanced, Some(0.1))
            .unwrap();
        assert_eq!(plan.estimated_frames, 1794);
        assert_eq!(plan.sampling_fps, 29.9);
    }
    #[test]
    fn base_sampling_is_separate_from_backfill_and_manual_selection_is_never_capped() {
        for (quality, count, candidates) in [
            (Quality::Fast, 360, 540),
            (Quality::Balanced, 480, 720),
            (Quality::High, 720, 900),
        ] {
            let p = GenerationPolicy {
                version: 4,
                ..Default::default()
            }
            .frame_plan(&video(1800, 30.), quality);
            assert_eq!(p.estimated_frames, count);
            assert_eq!(p.rescue_max_frames, candidates);
            for ratio in [0.001, 0.1, 0.333, 0.9, 1.0] {
                let p = GenerationPolicy {
                    frame_sampling_ratio: Some(ratio),
                    ..Default::default()
                }
                .frame_plan(&video(1800, 30.), quality);
                assert!(p.estimated_frames >= 30);
                assert!(p
                    .selected_frames
                    .iter()
                    .all(|f| p.candidate_frames.contains(f)));
                assert!(p.rescue_max_frames >= p.estimated_frames);
                assert_eq!(p.estimated_frames, ((1800. * ratio).round() as u64).max(30));
            }
        }
        assert_eq!(
            GenerationPolicy::default()
                .frame_plan(&video(12, 30.), Quality::Fast)
                .estimated_frames,
            12
        );
    }
    #[test]
    fn manual_bridge_v3_preserves_initial_frames_and_caps_only_extra_frames() {
        for quality in [Quality::Fast, Quality::Balanced, Quality::High] {
            for (ratio, initial, maximum) in [
                (0.001, 30, 36),
                (0.2, 360, 432),
                (0.5, 900, 1080),
                (0.9, 1620, 1800),
                (1., 1800, 1800),
                (0.333, 599, 718),
            ] {
                let policy = GenerationPolicy {
                    frame_sampling_ratio: Some(ratio),
                    ..Default::default()
                };
                let plan = policy.frame_plan(&video(1800, 30.), quality);
                assert_eq!(plan.estimated_frames, initial);
                assert_eq!(plan.rescue_max_frames, maximum);
                assert_eq!(plan.bridge_source_frames, Some(1800));
                assert_eq!(plan.candidate_count(), 1800);
                for version in [1, 2] {
                    let old = GenerationPolicy { version, ..policy }
                        .frame_plan(&video(1800, 30.), quality);
                    assert_eq!(old.selected_frames, plan.selected_frames);
                    assert_eq!(plan.candidate_frames, plan.selected_frames);
                    assert_eq!(old.bridge_source_frames, None);
                    assert_eq!(old.rescue_max_frames, old.candidate_frames.len() as u64);
                }
                assert_eq!(
                    plan,
                    GenerationPolicy {
                        auto_retry_enabled: false,
                        ..policy
                    }
                    .frame_plan(&video(1800, 30.), quality)
                );
            }
            let default_plan = GenerationPolicy {
                version: 2,
                ..Default::default()
            }
            .frame_plan(&video(1800, 30.), quality);
            for version in [1, 2] {
                assert_eq!(
                    default_plan,
                    GenerationPolicy {
                        version,
                        ..Default::default()
                    }
                    .frame_plan(&video(1800, 30.), quality)
                );
            }
        }
        let policy = GenerationPolicy {
            frame_sampling_ratio: Some(0.001),
            ..Default::default()
        };
        let short = policy.frame_plan(&video(12, 30.), Quality::High);
        assert_eq!((short.estimated_frames, short.rescue_max_frames), (12, 12));
    }

    #[test]
    fn manual_bridge_candidates_stay_compact_when_low_fps_legacy_pool_covers_source() {
        let policy = GenerationPolicy {
            frame_sampling_ratio: Some(0.001),
            ..Default::default()
        };
        let plan = policy.frame_plan(&video(12_000_000, 6.), Quality::High);
        assert_eq!(plan.selected_frames.len(), 12_000);
        assert_eq!(plan.candidate_frames.len(), 12_000);
        assert_eq!(plan.candidate_count(), 12_000_000);
        assert_eq!(plan.rescue_max_frames, 14_400);
        for quality in [Quality::Fast, Quality::Balanced, Quality::High] {
            let low_fps = video(600, 6.);
            let current = policy.frame_plan(&low_fps, quality);
            let old = GenerationPolicy {
                version: 2,
                ..policy
            }
            .frame_plan(&low_fps, quality);
            assert_eq!(current.selected_frames, old.selected_frames);
            assert_eq!(current.candidate_frames.len(), 30);
        }
    }

    #[test]
    fn v3_keeps_v2_resolution_and_training_recipes() {
        let old = GenerationPolicy {
            version: 2,
            ..Default::default()
        };
        let new = GenerationPolicy {
            version: 3,
            ..Default::default()
        };
        for quality in [Quality::Fast, Quality::Balanced, Quality::High] {
            for memory in [None, Some(8192), Some(24576)] {
                let resolution = new.resolution(quality, memory, 4096, 2048, true);
                assert_eq!(
                    resolution,
                    old.resolution(quality, memory, 4096, 2048, true)
                );
                assert_eq!(
                    new.training(
                        quality,
                        memory,
                        &resolution,
                        10000,
                        10000,
                        Some(&budget(10000))
                    )
                    .unwrap(),
                    old.training(
                        quality,
                        memory,
                        &resolution,
                        10000,
                        10000,
                        Some(&budget(10000))
                    )
                    .unwrap()
                );
                assert_eq!(
                    new.panorama_training(quality, memory, 1024, 10000, 10000)
                        .unwrap(),
                    old.panorama_training(quality, memory, 1024, 10000, 10000)
                        .unwrap()
                );
            }
        }
    }

    #[test]
    fn retry_switch_does_not_change_initial_plan_or_preset_and_input_is_validated() {
        let off = GenerationPolicy {
            auto_retry_enabled: false,
            ..Default::default()
        };
        let on = GenerationPolicy::default();
        assert_eq!(
            off.frame_plan(&video(1800, 30.), Quality::Fast),
            on.frame_plan(&video(1800, 30.), Quality::Fast)
        );
        for quality in [Quality::Fast, Quality::Balanced, Quality::High] {
            let r = on.resolution(quality, None, 3840, 2160, true);
            let budget = budget_for(quality, 10_000, r.brush_initial_max_resolution);
            assert_eq!(
                on.training(quality, None, &r, 10_000, 10_000, Some(&budget))
                    .unwrap(),
                off.training(quality, None, &r, 10_000, 10_000, Some(&budget))
                    .unwrap()
            );
        }
        for ratio in [0., -1., 1.1, f64::NAN, f64::INFINITY] {
            assert!(GenerationPolicy {
                frame_sampling_ratio: Some(ratio),
                ..on
            }
            .validate()
            .is_err());
        }
        assert_eq!(serde_json::from_str::<GenerationPolicy>("{}").unwrap(), on);
    }
    #[test]
    fn high_native_budget_is_not_vram_tiered_and_preserves_initialization() {
        for memory in [None, Some(4096), Some(8192), Some(24576)] {
            let p = GenerationPolicy::default();
            let r = p.resolution(Quality::High, memory, 8192, 5464, true);
            assert_eq!(r.working_long_edge(), 8192);
            let t = p
                .training(
                    Quality::High,
                    memory,
                    &r,
                    200_000,
                    9_000_000,
                    Some(&budget(200_000)),
                )
                .unwrap();
            assert_eq!(
                t.configured_max_splats,
                Some(budget(200_000).requested_max_splats)
            );
            assert_eq!(t.preset.max_splats, Some(9_000_000));
            assert_eq!(t.preset.max_resolution, 8192);
            let pano = p
                .panorama_training(Quality::High, memory, 2048, 400_000, 12_000_000)
                .unwrap();
            assert_eq!(pano.preset.max_splats, Some(16_000_000));
            assert!(p
                .training(Quality::High, memory, &r, u64::MAX, 1, None)
                .is_err());
            assert!(p.training(Quality::High, memory, &r, 0, 1, None).is_err());
        }
    }
    #[test]
    fn old_policy_keeps_sparse_budget_and_new_policy_changes_only_ordinary_high_ceiling() {
        let current = GenerationPolicy {
            version: 2,
            ..Default::default()
        };
        let old = GenerationPolicy {
            version: 1,
            ..current
        };
        assert!(old.validate().is_ok());
        let r = current.resolution(Quality::High, None, 3840, 2560, false);
        let budget = budget(66_218);
        let legacy = old
            .training(Quality::High, None, &r, 66_218, 66_218, None)
            .unwrap();
        assert_eq!(legacy.preset.max_splats, Some(2_648_720));
        let modern = current
            .training(Quality::High, None, &r, 66_218, 66_218, Some(&budget))
            .unwrap();
        let mut same_recipe = modern;
        same_recipe.configured_max_splats = legacy.configured_max_splats;
        same_recipe.preset.max_splats = legacy.preset.max_splats;
        assert_eq!(same_recipe, legacy);
        assert!(current
            .training(Quality::High, None, &r, 66_218, 66_218, None)
            .is_err());
        for quality in [Quality::Fast, Quality::Balanced] {
            assert_eq!(
                old.training(quality, None, &r, 66_218, 66_218, None)
                    .unwrap(),
                current
                    .training(quality, None, &r, 66_218, 66_218, None)
                    .unwrap()
            );
        }
        for quality in [Quality::Fast, Quality::Balanced, Quality::High] {
            assert_eq!(
                old.panorama_training(quality, None, 1024, 66_218, 66_218)
                    .unwrap(),
                current
                    .panorama_training(quality, None, 1024, 66_218, 66_218)
                    .unwrap()
            );
        }
        let json = serde_json::to_string(&old).unwrap();
        let resumed: GenerationPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(resumed, old);
        assert_eq!(
            resumed
                .training(Quality::High, None, &r, 66_218, 66_218, None)
                .unwrap(),
            legacy
        );
    }

    #[test]
    fn v7_changes_only_fast_balanced_ceiling_and_keeps_saved_recipes() {
        let current = GenerationPolicy::default();
        for quality in [Quality::Fast, Quality::Balanced] {
            for memory in [None, Some(8192), Some(24576)] {
                let r = current.resolution(quality, memory, 4096, 2160, false);
                let snapshot = budget_for(quality, 10_000, r.brush_initial_max_resolution);
                let modern = current
                    .training(quality, memory, &r, 10_000, 10_000, Some(&snapshot))
                    .unwrap();
                assert_eq!(
                    modern.configured_max_splats,
                    Some(snapshot.requested_max_splats)
                );
                assert_eq!(
                    modern.preset.max_splats,
                    Some(snapshot.requested_max_splats)
                );
                assert!(current
                    .training(quality, memory, &r, 10_000, 10_000, None)
                    .is_err());
                for version in 1..=6 {
                    let old = GenerationPolicy { version, ..current };
                    assert!(!old.requires_pixel_budget(quality));
                    let legacy = old
                        .training(quality, memory, &r, 10_000, 10_000, None)
                        .unwrap();
                    let mut same_recipe = modern;
                    same_recipe.configured_max_splats = legacy.configured_max_splats;
                    same_recipe.preset.max_splats = legacy.preset.max_splats;
                    assert_eq!(same_recipe, legacy);
                    assert_eq!(
                        current.resolution(quality, memory, 4096, 2160, false),
                        old.resolution(quality, memory, 4096, 2160, false)
                    );
                    assert_eq!(
                        current.frame_plan(&video(1800, 30.), quality),
                        GenerationPolicy { version: 6, ..old }
                            .frame_plan(&video(1800, 30.), quality)
                    );
                    assert_eq!(
                        current
                            .panorama_training(quality, memory, 512, 10_000, 10_000)
                            .unwrap(),
                        old.panorama_training(quality, memory, 512, 10_000, 10_000)
                            .unwrap()
                    );
                }
                let restored: super::super::splat_budget::FineSplatBudget =
                    serde_json::from_str(&serde_json::to_string(&snapshot).unwrap()).unwrap();
                let initialized = current
                    .training(quality, memory, &r, 10_000, 8_000_000, Some(&restored))
                    .unwrap();
                assert_eq!(
                    initialized.configured_max_splats,
                    Some(snapshot.requested_max_splats)
                );
                assert_eq!(initialized.preset.max_splats, Some(8_000_000));

                // The unified path can keep native prepared views. It must use
                // a budget bound to that actual cap, not the legacy cap.
                assert!(current
                    .training_with_max_resolution(
                        quality,
                        memory,
                        &r,
                        10_000,
                        10_000,
                        Some(&snapshot),
                        r.working_long_edge()
                    )
                    .is_err());
                let native = budget_for(quality, 10_000, r.working_long_edge());
                let native_recipe = current
                    .training_with_max_resolution(
                        quality,
                        memory,
                        &r,
                        10_000,
                        10_000,
                        Some(&native),
                        r.working_long_edge(),
                    )
                    .unwrap();
                assert_eq!(native_recipe.preset.max_resolution, r.working_long_edge());
                assert_eq!(
                    native_recipe.preset.densification,
                    modern.preset.densification
                );
            }
        }
        assert_eq!(super::super::splat_budget::coefficient(Quality::Fast), 0.2);
        assert_eq!(
            super::super::splat_budget::coefficient(Quality::Balanced),
            0.4
        );
        assert_eq!(super::super::splat_budget::coefficient(Quality::High), 0.5);
    }
    #[test]
    fn oom_changes_only_ceiling_and_never_discards_initial_points() {
        let p = GenerationPolicy::default();
        let r = p.resolution(Quality::High, None, 4096, 2048, false);
        let t = p
            .training(
                Quality::High,
                None,
                &r,
                100_000,
                100_000,
                Some(&budget(100_000)),
            )
            .unwrap();
        assert!(oom_retry_preset(t, None, 1).is_none());
        assert!(oom_retry_preset(t, Some(100_000), 100_000).is_none());
        let retry = oom_retry_preset(t, Some(1_000_001), 100_000).unwrap();
        assert_eq!(retry.preset.max_splats, Some(800_000));
        let mut restored = retry;
        restored.preset.max_splats = t.preset.max_splats;
        assert_eq!(restored, t);
    }

    #[test]
    fn all_tiers_share_retry_consent_count_and_one_attempt_rules() {
        for quality in [Quality::Fast, Quality::Balanced, Quality::High] {
            let on = GenerationPolicy::default();
            let off = GenerationPolicy {
                auto_retry_enabled: false,
                ..on
            };
            let resolution = on.resolution(quality, None, 8192, 4096, false);
            let budget = budget_for(quality, 10_000, resolution.brush_initial_max_resolution);
            let ordinary = on
                .training(quality, None, &resolution, 10_000, 10_000, Some(&budget))
                .unwrap();
            let panorama = on
                .panorama_training(quality, None, 512, 10_000, 10_000)
                .unwrap();
            for initial in [ordinary, panorama] {
                let retry = on.oom_retry(initial, false, Some(200_000), 10_000).unwrap();
                assert_eq!(retry.preset.max_splats, Some(160_000));
                assert!(off
                    .oom_retry(initial, false, Some(200_000), 10_000)
                    .is_none());
                assert!(on.oom_retry(initial, true, Some(200_000), 10_000).is_none());
                assert!(on.oom_retry(initial, false, None, 10_000).is_none());
            }
        }
    }
}
