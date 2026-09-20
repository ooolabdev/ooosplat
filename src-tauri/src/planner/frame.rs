use thiserror::Error;

use crate::{
    presets::{FrameBudget, Quality},
    video::{FramePlan, FramePlanningMode, MinimumFrameProtection, PlannedFrame, VideoInfo},
};

use super::{CapturePrior, FrameCandidate};

/// Lightweight facts produced by a future capture analyzer. Keeping this
/// contract independent from decoding lets P0 ship without selecting a final
/// optical-flow or ML implementation.
#[derive(Debug, Clone, PartialEq)]
pub struct CaptureAnalysis {
    /// 0 = easy/static capture, 1 = difficult/high-motion capture.
    pub activity: f64,
    pub prior: CapturePrior,
    pub candidates: Vec<FrameCandidate>,
}

impl Default for CaptureAnalysis {
    fn default() -> Self {
        Self {
            activity: 0.5,
            prior: CapturePrior::default(),
            candidates: Vec::new(),
        }
    }
}

#[derive(Debug, Error, PartialEq)]
pub enum PlannerError {
    #[error("frame budget cannot select a frame from this video")]
    EmptyPlan,
    #[error("requested backfill exceeds the quality frame ceiling")]
    FrameCeilingExhausted,
}

pub trait FramePlanner {
    fn plan(
        &self,
        video: &VideoInfo,
        quality: Quality,
        analysis: CaptureAnalysis,
    ) -> Result<FramePlan, PlannerError>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct BudgetFramePlanner;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MinimumCapturePolicy {
    pub min_selected_frames: u64,
    pub preferred_min_frames: u64,
}

impl Default for MinimumCapturePolicy {
    fn default() -> Self {
        Self {
            min_selected_frames: 30,
            preferred_min_frames: 40,
        }
    }
}

impl MinimumCapturePolicy {
    pub fn minimum_required_fps(self, video: &VideoInfo) -> f64 {
        if video.duration > 0.0 {
            self.min_selected_frames as f64 / video.duration
        } else {
            0.0
        }
    }

    pub fn analysis_fps(self, video: &VideoInfo, quality_analysis_fps: f64) -> f64 {
        quality_analysis_fps
            .max(self.minimum_required_fps(video))
            .min(video.fps)
            .max(0.0)
    }
}

impl BudgetFramePlanner {
    fn uniform_frames(video: &VideoInfo, fps: f64) -> Vec<PlannedFrame> {
        if video.duration <= 0.0 || video.fps <= 0.0 || fps <= 0.0 {
            return Vec::new();
        }
        let fps = fps.min(video.fps);
        let count = ((video.duration * fps).round().max(1.0) as u64).min(video.total_frames);
        (0..count)
            .map(|position| {
                let source_frame_index = ((position as f64 * video.total_frames as f64)
                    / count.max(1) as f64)
                    .floor()
                    .min(video.total_frames.saturating_sub(1) as f64)
                    as u64;
                let timestamp = ((source_frame_index as f64 + 0.5) / video.fps).min(video.duration);
                PlannedFrame {
                    source_frame_index,
                    timestamp_seconds: timestamp,
                }
            })
            .collect::<Vec<_>>()
            .into_iter()
            .fold(Vec::new(), |mut frames, frame| {
                if frames.last().is_none_or(|last: &PlannedFrame| {
                    last.source_frame_index != frame.source_frame_index
                }) {
                    frames.push(frame);
                }
                frames
            })
    }

    fn adaptive_frames(
        video: &VideoInfo,
        candidates: &[FrameCandidate],
        selected_fps: f64,
        min_fps: f64,
    ) -> Vec<PlannedFrame> {
        if candidates.is_empty() {
            return Self::uniform_frames(video, selected_fps);
        }
        let target_gap = 1.0 / selected_fps.max(0.001);
        let max_gap = 1.0 / min_fps.max(0.001);
        let mut output = Vec::new();
        let mut window_start = candidates[0].timestamp;
        let mut index = 0;
        while index < candidates.len() {
            let mut end = index + 1;
            while end < candidates.len() && candidates[end].timestamp - window_start < target_gap {
                end += 1;
            }
            let best = candidates[index..end]
                .iter()
                .max_by(|left, right| {
                    candidate_score(left)
                        .partial_cmp(&candidate_score(right))
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .unwrap_or(&candidates[index]);
            let must_fill_gap = output.last().is_none_or(|last: &PlannedFrame| {
                best.timestamp - last.timestamp_seconds <= max_gap * 1.05
            });
            let chosen = if must_fill_gap {
                best
            } else {
                &candidates[index]
            };
            output.push(PlannedFrame {
                source_frame_index: chosen.frame_index.max(0) as u64,
                timestamp_seconds: chosen.timestamp,
            });
            window_start = chosen.timestamp + target_gap;
            while end < candidates.len() && candidates[end].timestamp < window_start {
                end += 1;
            }
            index = end.max(index + 1);
        }
        output.sort_by_key(|frame| frame.source_frame_index);
        output.dedup_by_key(|frame| frame.source_frame_index);
        output
    }

    pub fn backfill(&self, plan: &mut FramePlan, additional: usize) -> Result<(), PlannerError> {
        let selected: std::collections::HashSet<_> = plan
            .selected_frames
            .iter()
            .map(|frame| frame.source_frame_index)
            .collect();
        let additions: Vec<_> = plan
            .candidate_frames
            .iter()
            .filter(|frame| !selected.contains(&frame.source_frame_index))
            .take(additional)
            .cloned()
            .collect();
        if additions.len() != additional {
            return Err(PlannerError::FrameCeilingExhausted);
        }
        plan.selected_frames.extend(additions);
        plan.selected_frames
            .sort_by_key(|frame| frame.source_frame_index);
        plan.estimated_frames = plan.selected_frames.len() as u64;
        Ok(())
    }

    fn backfill_minimum(
        selected_frames: &mut Vec<PlannedFrame>,
        candidates: &[FrameCandidate],
        candidate_frames: &[PlannedFrame],
        target: usize,
    ) -> usize {
        let initial_count = selected_frames.len();
        while selected_frames.len() < target {
            let selected: std::collections::HashSet<_> = selected_frames
                .iter()
                .map(|frame| frame.source_frame_index)
                .collect();
            let next = candidate_frames
                .iter()
                .filter(|frame| !selected.contains(&frame.source_frame_index))
                .max_by(|left, right| {
                    let score = |frame: &PlannedFrame| {
                        let temporal_coverage = selected_frames
                            .iter()
                            .map(|selected| {
                                (selected.timestamp_seconds - frame.timestamp_seconds).abs()
                            })
                            .fold(f64::INFINITY, f64::min);
                        let quality = candidates
                            .iter()
                            .find(|candidate| {
                                candidate.frame_index.max(0) as u64 == frame.source_frame_index
                            })
                            .map(candidate_score)
                            .unwrap_or(0.5) as f64;
                        // Filling the largest temporal hole preserves coverage and
                        // continuity; quality breaks close ties in favor of sharper,
                        // better-exposed frames with useful view change.
                        temporal_coverage * 0.65 + quality * 0.35
                    };
                    score(left)
                        .partial_cmp(&score(right))
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .cloned();
            let Some(next) = next else {
                break;
            };
            selected_frames.push(next);
        }
        selected_frames.sort_by_key(|frame| frame.source_frame_index);
        selected_frames.dedup_by_key(|frame| frame.source_frame_index);
        selected_frames.len().saturating_sub(initial_count)
    }
}

impl FramePlanner for BudgetFramePlanner {
    fn plan(
        &self,
        video: &VideoInfo,
        quality: Quality,
        analysis: CaptureAnalysis,
    ) -> Result<FramePlan, PlannerError> {
        let budget: FrameBudget = quality.preset().frame;
        let minimum_policy = MinimumCapturePolicy::default();
        let activity = analysis.activity.clamp(0.0, 1.0);
        let normal_target_fps = if activity <= 0.5 {
            budget.min_fps + (budget.preferred_fps - budget.min_fps) * activity * 2.0
        } else {
            budget.preferred_fps + (budget.max_fps - budget.preferred_fps) * (activity - 0.5) * 2.0
        }
        .min(video.fps)
        .max(0.0);
        let minimum_required_fps = minimum_policy.minimum_required_fps(video);
        let selected_fps = normal_target_fps.max(minimum_required_fps).min(video.fps);
        let candidate_fps = minimum_policy
            .analysis_fps(video, budget.analysis_fps)
            .max(selected_fps);
        let mut selected_frames =
            Self::adaptive_frames(video, &analysis.candidates, selected_fps, budget.min_fps);
        if selected_frames.is_empty() {
            return Err(PlannerError::EmptyPlan);
        }
        let candidate_frames = if analysis.candidates.is_empty() {
            Self::uniform_frames(video, candidate_fps)
        } else {
            analysis
                .candidates
                .iter()
                .map(|candidate| PlannedFrame {
                    source_frame_index: candidate.frame_index.max(0) as u64,
                    timestamp_seconds: candidate.timestamp,
                })
                .collect()
        };
        let target_before_filter = ((video.duration * selected_fps).round() as u64)
            .min(video.total_frames)
            .max(1);
        let selected_after_filter = selected_frames.len() as u64;
        let minimum_available_target = minimum_policy
            .min_selected_frames
            .min(video.total_frames)
            .min(candidate_frames.len() as u64);
        let backfilled_for_minimum_count = Self::backfill_minimum(
            &mut selected_frames,
            &analysis.candidates,
            &candidate_frames,
            minimum_available_target as usize,
        ) as u64;
        let actual_average_fps = selected_frames.len() as f64 / video.duration.max(0.001);
        let retention_ratio = if video.fps > 0.0 {
            actual_average_fps / video.fps
        } else {
            0.0
        };
        Ok(FramePlan {
            quality: Some(quality),
            retention_ratio,
            sampling_fps: actual_average_fps,
            actual_average_fps,
            target_fps: selected_fps,
            candidate_fps,
            estimated_frames: selected_frames.len() as u64,
            planning_mode: FramePlanningMode::Budgeted,
            preferred_fps: budget.preferred_fps,
            selected_frames,
            candidate_frames,
            minimum_frame_protection: MinimumFrameProtection {
                short_capture: (video.duration * normal_target_fps).round()
                    < minimum_policy.preferred_min_frames as f64,
                minimum_required_fps,
                minimum_frame_target: minimum_policy.min_selected_frames,
                minimum_frame_override_applied: minimum_required_fps
                    > normal_target_fps + f64::EPSILON,
                minimum_frame_target_unreachable: video.total_frames
                    < minimum_policy.min_selected_frames
                    || minimum_available_target < minimum_policy.min_selected_frames,
                selected_frames_before_filter: target_before_filter,
                selected_frames_after_filter: selected_after_filter,
                backfilled_for_minimum_count,
            },
        })
    }
}

fn candidate_score(candidate: &FrameCandidate) -> f32 {
    candidate.sharpness_score * 0.42
        + candidate.exposure_score * 0.28
        + candidate.view_change_score * 0.20
        + candidate.motion_score * 0.10
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presets::Quality;

    fn video() -> VideoInfo {
        VideoInfo {
            duration: 10.0,
            width: 1920,
            height: 1080,
            fps: 30.0,
            total_frames: 300,
            codec: "h264".into(),
            rotation: 0,
            pixel_format: "yuv420p".into(),
            has_alpha: false,
        }
    }

    fn video_with(duration: f64, fps: f64, total_frames: u64) -> VideoInfo {
        VideoInfo {
            duration,
            fps,
            total_frames,
            ..video()
        }
    }

    #[test]
    fn difficulty_moves_selection_inside_quality_boundaries() {
        let easy = BudgetFramePlanner
            .plan(
                &video(),
                Quality::Fast,
                CaptureAnalysis {
                    activity: 0.0,
                    ..CaptureAnalysis::default()
                },
            )
            .unwrap();
        let normal = BudgetFramePlanner
            .plan(
                &video(),
                Quality::Fast,
                CaptureAnalysis {
                    activity: 0.5,
                    ..CaptureAnalysis::default()
                },
            )
            .unwrap();
        let hard = BudgetFramePlanner
            .plan(
                &video(),
                Quality::Fast,
                CaptureAnalysis {
                    activity: 1.0,
                    ..CaptureAnalysis::default()
                },
            )
            .unwrap();
        assert_eq!(easy.sampling_fps, 4.0);
        assert_eq!(normal.sampling_fps, 6.0);
        assert_eq!(hard.sampling_fps, 9.0);
        assert!(!normal.selected_frames.is_empty());
        assert!(normal.candidate_frames.len() > normal.selected_frames.len());
    }

    #[test]
    fn backfill_cannot_exceed_candidates() {
        let mut plan = BudgetFramePlanner
            .plan(&video(), Quality::Fast, CaptureAnalysis::default())
            .unwrap();
        assert_eq!(
            BudgetFramePlanner.backfill(&mut plan, usize::MAX),
            Err(PlannerError::FrameCeilingExhausted)
        );
    }

    #[test]
    fn fast_three_second_capture_overrides_quality_max_to_reach_thirty_frames() {
        let plan = BudgetFramePlanner
            .plan(
                &video_with(3.0, 30.0, 90),
                Quality::Fast,
                CaptureAnalysis {
                    activity: 0.5,
                    ..CaptureAnalysis::default()
                },
            )
            .unwrap();

        assert_eq!(plan.estimated_frames, 30);
        assert!(plan.target_fps > Quality::Fast.preset().frame.max_fps);
        assert_eq!(plan.target_fps, 10.0);
        assert!(plan.minimum_frame_protection.short_capture);
        assert!(plan.minimum_frame_protection.minimum_frame_override_applied);
        assert!(
            !plan
                .minimum_frame_protection
                .minimum_frame_target_unreachable
        );
    }

    #[test]
    fn fast_ten_second_capture_keeps_normal_quality_fps() {
        let plan = BudgetFramePlanner
            .plan(
                &video(),
                Quality::Fast,
                CaptureAnalysis {
                    activity: 0.5,
                    ..CaptureAnalysis::default()
                },
            )
            .unwrap();

        assert_eq!(plan.minimum_frame_protection.minimum_required_fps, 3.0);
        assert_eq!(plan.target_fps, 6.0);
        assert!(!plan.minimum_frame_protection.minimum_frame_override_applied);
    }

    #[test]
    fn source_with_fewer_than_thirty_frames_keeps_every_available_frame() {
        let plan = BudgetFramePlanner
            .plan(
                &video_with(1.0, 20.0, 20),
                Quality::Fast,
                CaptureAnalysis::default(),
            )
            .unwrap();

        assert_eq!(plan.estimated_frames, 20);
        assert_eq!(plan.selected_frames.len(), 20);
        assert!(
            plan.minimum_frame_protection
                .minimum_frame_target_unreachable
        );
    }

    #[test]
    fn filtered_selection_backfills_only_enough_ranked_candidates_for_minimum() {
        let candidates: Vec<_> = (0..35)
            .map(|index| FrameCandidate {
                frame_index: index,
                timestamp: index as f64 / 10.0,
                sharpness_score: 0.6 + (index % 3) as f32 * 0.1,
                motion_score: 0.4,
                view_change_score: 0.5,
                exposure_score: 0.8,
            })
            .collect();
        let candidate_frames: Vec<_> = candidates
            .iter()
            .map(|candidate| PlannedFrame {
                source_frame_index: candidate.frame_index as u64,
                timestamp_seconds: candidate.timestamp,
            })
            .collect();
        let mut selected = candidate_frames.iter().take(22).cloned().collect();

        let added =
            BudgetFramePlanner::backfill_minimum(&mut selected, &candidates, &candidate_frames, 30);

        assert_eq!(added, 8);
        assert_eq!(selected.len(), 30);
        assert!(selected.len() < candidate_frames.len());
    }
}
