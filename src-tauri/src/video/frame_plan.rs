use serde::{Deserialize, Serialize};

use crate::{
    presets::QualityPreset,
    video::{FrameFilterConfig, VideoInfo},
};

/// 候选超采样比例：`candidate_fps` 未显式配置时，按 target 的 1.5 倍取候选密度。
///
/// 智能过滤会剔除模糊、曝光异常与时序冗余帧，超采样保证过滤后仍有足够的有效帧。
const OVERSAMPLE_FACTOR: f64 = 1.5;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FramePlan {
    pub retention_ratio: f64,
    pub sampling_fps: f64,
    pub estimated_frames: u64,
}

pub trait FrameSelectionStrategy {
    fn create_plan(&self, video: &VideoInfo, preset: &QualityPreset) -> FramePlan;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct UniformRatioFrameSelection;

#[derive(Debug, Default, Clone, Copy)]
pub struct SmartFrameSelection;

/// 解析 L0 的三个帧率，返回 `(target_fps, candidate_fps)`。
///
/// · `source_fps`：源视频的真实帧率。probe 层已保证它有限且位于 `(0, 1000]`，
///   这里仍对退化输入做兜底，避免除零与 NaN 传播到 ffmpeg 参数。
/// · `target_fps`：期望的**最终**保留密度；未显式配置时退化为 `retention_ratio × source_fps`
///   （即 v2 的语义：`retention_ratio` 描述的是抽帧密度比例）。
/// · `candidate_fps`：送进智能筛选前的**候选**密度；未显式配置时退化为 `target_fps × 1.5`。
///
/// 约束按「可调意图服从源帧率硬上限」的顺序收敛，不报错：
/// `0 <= target_fps <= candidate_fps <= source_fps`。
/// 之所以不报错，是因为这两个值来自档位配置而不是用户逐次输入，收敛比中断生成更合理；
/// 收敛后的实际值会由 `FramePlan::sampling_fps` 如实反映，便于审计。
pub fn resolve_frame_rates(source_fps: f64, preset: &QualityPreset) -> (f64, f64) {
    let source = if source_fps.is_finite() && source_fps > 0.0 {
        source_fps
    } else {
        f64::EPSILON
    };
    let target = preset
        .target_fps
        .filter(|value| value.is_finite() && *value > 0.0)
        .unwrap_or(source * preset.frame_retention_ratio)
        .clamp(0.0, source);
    let candidate = preset
        .candidate_fps
        .filter(|value| value.is_finite() && *value > 0.0)
        .unwrap_or(target * OVERSAMPLE_FACTOR)
        .max(target)
        .min(source);
    (target, candidate)
}

/// 由目标密度反推窗口配额：`keep_per_window ≈ target_fps / candidate_fps × window_size`。
///
/// 这是让 `target_fps` 真正决定**最终**保留密度的一步：候选密度由 `candidate_fps` 决定，
/// 过滤阶段按 `keep_per_window / window_size` 的比例保留，两者相乘才是最终密度。
/// 配额夹在 `1..=window_size`：至少留一帧，且不会超过窗口本身（否则配额永远不生效）。
pub fn resolved_keep_per_window(
    target_fps: f64,
    candidate_fps: f64,
    config: &FrameFilterConfig,
) -> usize {
    if !target_fps.is_finite()
        || !candidate_fps.is_finite()
        || target_fps <= 0.0
        || candidate_fps <= 0.0
    {
        return config.keep_per_window;
    }
    let window = config.window_size.max(1);
    let quota = (target_fps / candidate_fps * window as f64).round();
    (quota.max(1.0) as usize).min(window)
}

/// 本次运行**实际使用**的过滤配置。
///
/// 档位显式配置了 `target_fps` 时，窗口配额由目标密度与候选密度共同反推，
/// 这样"最终保留密度"就由 `target_fps` 决定，而不是由 `keep_per_window` 的
/// 历史取值间接决定。未配置 `target_fps` 时原样返回档位里的配置（行为不变）。
///
/// 与 [`resolve_frame_rates`] 不同，这里不需要源帧率：两个输入都是绝对帧率。
pub fn resolved_filter_config(preset: &QualityPreset, candidate_fps: f64) -> FrameFilterConfig {
    let mut config = preset.smart_filter_config;
    let Some(target) = preset.target_fps else {
        return config;
    };
    config.keep_per_window = resolved_keep_per_window(target, candidate_fps, &config);
    config
}
impl FrameSelectionStrategy for SmartFrameSelection {
    fn create_plan(&self, video: &VideoInfo, preset: &QualityPreset) -> FramePlan {
        // 抽帧率就是候选帧密度：过滤发生在 ffmpeg 抽帧之后，先按候选密度多抽一些。
        let (_, candidate_fps) = resolve_frame_rates(video.fps, preset);
        FramePlan {
            retention_ratio: preset.frame_retention_ratio,
            sampling_fps: candidate_fps,
            estimated_frames: (video.total_frames as f64
                * (candidate_fps / video.fps.max(f64::EPSILON)))
            .round() as u64,
        }
    }
}

impl FrameSelectionStrategy for UniformRatioFrameSelection {
    fn create_plan(&self, video: &VideoInfo, preset: &QualityPreset) -> FramePlan {
        FramePlan {
            retention_ratio: preset.frame_retention_ratio,
            sampling_fps: video.fps * preset.frame_retention_ratio,
            estimated_frames: ((video.total_frames as f64) * preset.frame_retention_ratio).round()
                as u64,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presets::{Quality, QualityPreset};

    #[test]
    fn smart_selection_oversamples_for_post_filtering() {
        let plan =
            SmartFrameSelection.create_plan(&thirty_fps_video(), &Quality::Balanced.preset());
        assert_eq!(plan.sampling_fps, 20.0);
        assert_eq!(plan.estimated_frames, 1_200);
    }

    fn thirty_fps_video() -> VideoInfo {
        VideoInfo {
            duration: 60.0,
            width: 1920,
            height: 1080,
            fps: 30.0,
            total_frames: 1800,
            codec: "h264".into(),
            rotation: 0,
            pixel_format: "yuv420p".into(),
            has_alpha: false,
        }
    }

    /// 未配置绝对帧率时，必须与 v2 的 `retention_ratio × 1.5 × source` 逐位一致。
    ///
    /// 三档出厂预设现在都配了绝对帧率，所以这里用显式 `None` 的档位来锁这条回退路径。
    #[test]
    fn legacy_ratio_formula_is_unchanged_when_rates_are_unset() {
        for base in [
            Quality::Fast.preset(),
            Quality::Balanced.preset(),
            Quality::High.preset(),
        ] {
            let preset = QualityPreset {
                target_fps: None,
                candidate_fps: None,
                ..base
            };
            for fps in [12.0_f64, 24.0, 29.97, 30.0, 59.94, 120.0] {
                let video = VideoInfo {
                    fps,
                    ..thirty_fps_video()
                };
                let plan = SmartFrameSelection.create_plan(&video, &preset);
                assert_eq!(
                    plan.sampling_fps,
                    (fps * preset.frame_retention_ratio * OVERSAMPLE_FACTOR).min(fps),
                    "fps={fps} retention={}",
                    preset.frame_retention_ratio
                );
            }
        }
    }

    /// 三档必须配置绝对帧率，且目标密度随档位递增。
    #[test]
    fn absolute_frame_rates_follow_the_ladder() {
        let ladder = [
            (Quality::Fast, 6.0, 12.0),
            (Quality::Balanced, 8.0, 20.0),
            (Quality::High, 10.0, 30.0),
        ];
        for (quality, target, candidate) in ladder {
            let preset = quality.preset();
            assert_eq!(preset.target_fps, Some(target), "target_fps 档位值");
            assert_eq!(
                preset.candidate_fps,
                Some(candidate),
                "candidate_fps 档位值"
            );
            assert!(target <= candidate, "候选密度不得低于目标密度");
        }
    }

    /// 这是本次改动的核心不变量：**最终保留密度由 `target_fps` 决定**，
    /// 不再由 `keep_per_window` 的历史取值间接决定，也不随源帧率漂移。
    #[test]
    fn target_fps_governs_the_final_kept_density() {
        for (quality, target) in [
            (Quality::Fast, 6.0_f64),
            (Quality::Balanced, 8.0),
            (Quality::High, 10.0),
        ] {
            for fps in [24.0_f64, 30.0, 60.0] {
                let duration = 60.0;
                let video = VideoInfo {
                    duration,
                    fps,
                    total_frames: (duration * fps).round() as u64,
                    ..thirty_fps_video()
                };
                let preset = quality.preset();
                let plan = SmartFrameSelection.create_plan(&video, &preset);
                let config = resolved_filter_config(&preset, plan.sampling_fps);
                let kept = crate::video::expected_kept_frames(plan.estimated_frames, &config);
                let final_fps = kept as f64 / duration;
                assert!(
                    (final_fps - target).abs() <= 1.0,
                    "{quality:?} @ {fps}fps：最终密度 {final_fps:.2} fps 偏离目标 {target} fps"
                );
            }
        }
    }

    /// 配额反推本身的边界：夹在 `1..=window_size`，非法输入退回档位原值。
    #[test]
    fn resolved_quota_is_clamped_and_safe() {
        let config = FrameFilterConfig::balanced();
        let window = config.window_size;
        assert_eq!(resolved_keep_per_window(8.0, 20.0, &config), 4);
        assert_eq!(resolved_keep_per_window(30.0, 20.0, &config), window);
        assert_eq!(resolved_keep_per_window(0.01, 20.0, &config), 1);
        for bad in [0.0_f64, -1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(
                resolved_keep_per_window(bad, 20.0, &config),
                config.keep_per_window,
                "非法 target={bad} 必须退回档位原值"
            );
            assert_eq!(
                resolved_keep_per_window(8.0, bad, &config),
                config.keep_per_window,
                "非法 candidate={bad} 必须退回档位原值"
            );
        }
        // 未配置 target_fps 时，档位里的配额原样保留（行为不变）。
        let unset = QualityPreset {
            target_fps: None,
            candidate_fps: None,
            ..Quality::Balanced.preset()
        };
        assert_eq!(
            resolved_filter_config(&unset, 20.0).keep_per_window,
            unset.smart_filter_config.keep_per_window
        );
    }

    /// 显式候选密度按绝对值生效，且绝不突破源帧率。
    #[test]
    fn explicit_candidate_fps_is_honoured_and_capped_by_source() {
        let preset = QualityPreset {
            candidate_fps: Some(20.0),
            ..Quality::Balanced.preset()
        };
        let plan = SmartFrameSelection.create_plan(&thirty_fps_video(), &preset);
        assert_eq!(plan.sampling_fps, 20.0);

        // 源帧率低于候选密度时收敛到源帧率：否则同一时间戳会被抽成重复帧。
        let slow = VideoInfo {
            fps: 12.0,
            total_frames: 720,
            ..thirty_fps_video()
        };
        let plan = SmartFrameSelection.create_plan(&slow, &preset);
        assert_eq!(plan.sampling_fps, 12.0);
    }

    /// 候选密度不得低于目标密度，否则"最终保留密度"这个意图不可能达成。
    #[test]
    fn candidate_fps_never_drops_below_target() {
        let preset = QualityPreset {
            target_fps: Some(8.0),
            candidate_fps: Some(5.0),
            ..Quality::Balanced.preset()
        };
        let (target, candidate) = resolve_frame_rates(30.0, &preset);
        assert_eq!(target, 8.0);
        assert_eq!(candidate, 8.0, "候选密度被抬到目标密度");
    }

    #[test]
    fn target_fps_is_capped_by_source() {
        let preset = QualityPreset {
            target_fps: Some(120.0),
            ..Quality::Balanced.preset()
        };
        let (target, candidate) = resolve_frame_rates(30.0, &preset);
        assert_eq!(target, 30.0);
        assert_eq!(candidate, 30.0);
    }

    /// 源帧率缺失或退化时不得产生 NaN / 0，也不得超过源帧率。
    #[test]
    fn degenerate_source_rates_stay_usable() {
        for source in [0.0_f64, -30.0, f64::NAN, f64::INFINITY] {
            let (target, candidate) = resolve_frame_rates(source, &Quality::Balanced.preset());
            assert!(candidate.is_finite() && candidate > 0.0, "source={source}");
            assert!(target.is_finite() && target >= 0.0, "source={source}");
            assert!(
                candidate
                    <= if source.is_finite() && source > 0.0 {
                        source
                    } else {
                        f64::EPSILON
                    },
                "source={source}"
            );
        }
    }

    /// 对任意源帧率，解析结果都必须满足 `target <= candidate <= source`。
    #[test]
    fn resolved_rates_always_respect_the_three_way_order() {
        let presets = [
            Quality::Fast.preset(),
            Quality::Balanced.preset(),
            Quality::High.preset(),
            QualityPreset {
                target_fps: Some(10.0),
                candidate_fps: Some(30.0),
                ..Quality::Balanced.preset()
            },
            QualityPreset {
                target_fps: Some(60.0),
                candidate_fps: Some(120.0),
                ..Quality::High.preset()
            },
        ];
        for preset in presets {
            for source in [1.0_f64, 12.0, 24.0, 30.0, 59.94, 240.0] {
                let (target, candidate) = resolve_frame_rates(source, &preset);
                assert!(target <= candidate, "source={source}");
                assert!(candidate <= source, "source={source}");
                assert!(target >= 0.0, "source={source}");
            }
        }
    }

    #[test]
    fn calculates_required_sampling_rates() {
        let strategy = UniformRatioFrameSelection;
        let video = thirty_fps_video();
        assert_eq!(
            strategy
                .create_plan(&video, &Quality::Fast.preset())
                .sampling_fps,
            9.0
        );
        assert_eq!(
            strategy
                .create_plan(&video, &Quality::Balanced.preset())
                .sampling_fps,
            15.0
        );
        assert_eq!(
            strategy
                .create_plan(&video, &Quality::High.preset())
                .sampling_fps,
            30.0
        );
    }

    #[test]
    fn estimates_frames_without_a_cap() {
        let strategy = UniformRatioFrameSelection;
        let video = thirty_fps_video();
        assert_eq!(
            strategy
                .create_plan(&video, &Quality::Fast.preset())
                .estimated_frames,
            540
        );
        assert_eq!(
            strategy
                .create_plan(&video, &Quality::Balanced.preset())
                .estimated_frames,
            900
        );
        assert_eq!(
            strategy
                .create_plan(&video, &Quality::High.preset())
                .estimated_frames,
            1800
        );

        let long_video = VideoInfo {
            total_frames: 180_000,
            ..video
        };
        assert_eq!(
            strategy
                .create_plan(&long_video, &Quality::High.preset())
                .estimated_frames,
            180_000
        );
    }
}
