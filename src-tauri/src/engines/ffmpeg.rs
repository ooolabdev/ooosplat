use std::{
    collections::BTreeSet,
    ffi::OsString,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
};

use serde::{Deserialize, Serialize};

use crate::{
    error::{Result, SplatError},
    planner::FrameCandidate,
    process::{ProcessManager, ProcessObserver, ProcessSpec},
    video::{FramePlan, VideoInfo},
};

use tokio::{
    io::{AsyncReadExt, BufReader},
    process::Command,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FrameImageFormat {
    Jpeg,
    Png,
}

impl FrameImageFormat {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Jpeg => "jpeg",
            Self::Png => "png",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameExtractionResult {
    pub frame_count: u64,
    pub image_format: FrameImageFormat,
    pub mask_count: u64,
    pub has_alpha: bool,
}

#[allow(clippy::too_many_arguments)]
pub async fn extract_uniform_frames(
    executable: &Path,
    input: &Path,
    output_directory: &Path,
    mask_directory: &Path,
    plan: &FramePlan,
    has_alpha: bool,
    log_path: Option<PathBuf>,
    process_manager: &ProcessManager,
    observer: Option<ProcessObserver>,
) -> Result<FrameExtractionResult> {
    if !input.is_file() {
        return Err(SplatError::InvalidPath(input.to_path_buf()));
    }
    ensure_clean_output(output_directory, mask_directory).await?;
    tokio::fs::create_dir_all(output_directory).await?;
    if has_alpha {
        tokio::fs::create_dir_all(mask_directory).await?;
    }

    let args = frame_extraction_args(
        input,
        output_directory,
        mask_directory,
        plan.sampling_fps,
        has_alpha,
    );
    let output = process_manager
        .run(ProcessSpec {
            executable: executable.to_path_buf(),
            args,
            working_directory: output_directory.parent().map(Path::to_path_buf),
            log_path,
            observer,
        })
        .await?;
    if !output.success {
        let operation = if has_alpha {
            "透明画面或 Alpha Mask 提取"
        } else {
            "画面提取"
        };
        return Err(SplatError::Process(format!(
            "FFmpeg {operation}失败，退出码 {:?}",
            output.exit_code
        )));
    }

    validate_extraction(output_directory, mask_directory, has_alpha).await
}

/// Decodes a low-resolution in-memory stream for Planner. It never writes
/// analysis frames to disk and therefore does not multiply full-resolution IO.
pub async fn scan_frame_candidates(
    executable: &Path,
    input: &Path,
    video: &VideoInfo,
    analysis_fps: f64,
    process_manager: &ProcessManager,
    observer: Option<Arc<dyn Fn(u64, u64) + Send + Sync>>,
) -> Result<Vec<FrameCandidate>> {
    const WIDTH: usize = 320;
    const HEIGHT: usize = 180;
    const FRAME_BYTES: usize = WIDTH * HEIGHT;
    if !input.is_file() {
        return Err(SplatError::InvalidPath(input.to_path_buf()));
    }
    let fps = analysis_fps.min(video.fps).max(0.25);
    let mut command = Command::new(executable);
    command
        .args(["-hide_banner", "-nostdin", "-loglevel", "error", "-i"])
        .arg(input)
        .args([
            "-vf",
            &format!("fps={fps:.8},scale={WIDTH}:{HEIGHT},format=gray"),
            "-pix_fmt",
            "gray",
            "-f",
            "rawvideo",
            "pipe:1",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command.spawn().map_err(|error| {
        SplatError::Process(format!("Unable to start FFmpeg analysis scan: {error}"))
    })?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| SplatError::Process("FFmpeg analysis stdout unavailable".into()))?;
    let mut reader = BufReader::new(stdout);
    let mut stderr = child.stderr.take().map(BufReader::new);
    let stderr_task = tokio::spawn(async move {
        let mut bytes = Vec::new();
        if let Some(reader) = stderr.as_mut() {
            let _ = reader.read_to_end(&mut bytes).await;
        }
        String::from_utf8_lossy(&bytes).into_owned()
    });
    let cancellation = process_manager.child_token();
    let mut current = vec![0_u8; FRAME_BYTES];
    let mut previous: Option<Vec<u8>> = None;
    let mut candidates = Vec::new();
    let expected = (video.duration * fps).round().max(1.0) as u64;
    let mut last_percent = u64::MAX;
    loop {
        let read = tokio::select! {
            _ = cancellation.cancelled() => {
                let _ = child.kill().await;
                return Err(SplatError::Cancelled);
            }
            read = reader.read_exact(&mut current) => read,
        };
        match read {
            Ok(_) => {
                let index = candidates.len();
                let mean = current
                    .iter()
                    .step_by(4)
                    .map(|value| *value as f32)
                    .sum::<f32>()
                    / (FRAME_BYTES / 4) as f32;
                let exposure = (1.0 - (mean - 128.0).abs() / 128.0).clamp(0.0, 1.0);
                let mut gradients = 0_u64;
                let mut gradient_samples = 0_u64;
                for y in (1..HEIGHT).step_by(3) {
                    for x in (1..WIDTH).step_by(3) {
                        let at = y * WIDTH + x;
                        gradients += current[at].abs_diff(current[at - 1]) as u64;
                        gradients += current[at].abs_diff(current[at - WIDTH]) as u64;
                        gradient_samples += 2;
                    }
                }
                let sharpness =
                    (gradients as f32 / gradient_samples.max(1) as f32 / 32.0).clamp(0.0, 1.0);
                let motion = previous
                    .as_ref()
                    .map(|last| {
                        current
                            .iter()
                            .zip(last)
                            .step_by(4)
                            .map(|(a, b)| a.abs_diff(*b) as u64)
                            .sum::<u64>() as f32
                            / (FRAME_BYTES / 4) as f32
                            / 48.0
                    })
                    .unwrap_or(0.0)
                    .clamp(0.0, 1.0);
                let timestamp = index as f64 / fps;
                candidates.push(FrameCandidate {
                    frame_index: (timestamp * video.fps)
                        .round()
                        .min(video.total_frames.saturating_sub(1) as f64)
                        as i64,
                    timestamp,
                    sharpness_score: sharpness,
                    motion_score: motion,
                    view_change_score: motion,
                    exposure_score: exposure,
                });
                let current_count = candidates.len() as u64;
                let percent = current_count.saturating_mul(100) / expected.max(1);
                if percent != last_percent {
                    last_percent = percent;
                    if let Some(observer) = &observer {
                        observer(current_count, expected);
                    }
                }
                previous = Some(current.clone());
            }
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(error) => {
                return Err(SplatError::Process(format!(
                    "FFmpeg analysis scan failed: {error}"
                )))
            }
        }
    }
    drop(reader);
    let status = child
        .wait()
        .await
        .map_err(|error| SplatError::Process(format!("FFmpeg analysis wait failed: {error}")))?;
    let stderr = stderr_task.await.unwrap_or_default();
    if !status.success() {
        return Err(SplatError::Process(format!(
            "FFmpeg analysis scan exited with {status}: {}",
            stderr.trim()
        )));
    }
    if candidates.is_empty() {
        return Err(SplatError::Process(
            "FFmpeg analysis scan produced no frames".into(),
        ));
    }
    Ok(candidates)
}

/// Extracts exactly the full-resolution frames selected by Planner. The
/// Quality preferred FPS is never passed to FFmpeg on this path.
#[allow(clippy::too_many_arguments)]
pub async fn extract_selected_frames(
    executable: &Path,
    input: &Path,
    output_directory: &Path,
    mask_directory: &Path,
    plan: &FramePlan,
    has_alpha: bool,
    log_path: Option<PathBuf>,
    process_manager: &ProcessManager,
    observer: Option<ProcessObserver>,
) -> Result<FrameExtractionResult> {
    if !input.is_file() {
        return Err(SplatError::InvalidPath(input.to_path_buf()));
    }
    if plan.selected_frames.is_empty() {
        return Err(SplatError::Process("Planner 未选择任何画面".into()));
    }
    ensure_clean_output(output_directory, mask_directory).await?;
    tokio::fs::create_dir_all(output_directory).await?;
    if has_alpha {
        tokio::fs::create_dir_all(mask_directory).await?;
    }

    let script_path = output_directory
        .parent()
        .unwrap_or(output_directory)
        .join("frame-selection.ffscript");
    let expression = selection_expression(plan);
    let script = if has_alpha {
        format!("[0:v]select='{expression}',format=rgba,split=2[rgba][masksrc];[masksrc]alphaextract[mask]")
    } else {
        format!("select='{expression}'")
    };
    tokio::fs::write(&script_path, script).await?;
    let args = selected_frame_extraction_args(
        input,
        output_directory,
        mask_directory,
        &script_path,
        has_alpha,
    );
    let output = process_manager
        .run(ProcessSpec {
            executable: executable.to_path_buf(),
            args,
            working_directory: output_directory.parent().map(Path::to_path_buf),
            log_path,
            observer,
        })
        .await;
    let _ = tokio::fs::remove_file(&script_path).await;
    let output = output?;
    if !output.success {
        let detail = output.failure_detail();
        return Err(SplatError::Process(format!(
            "FFmpeg 按 Planner 画面清单提取失败，退出码 {:?}{}",
            output.exit_code,
            if detail.is_empty() {
                String::new()
            } else {
                format!("\n{detail}")
            }
        )));
    }
    rename_selected_outputs(output_directory, mask_directory, plan, has_alpha).await?;
    validate_extraction(output_directory, mask_directory, has_alpha).await
}

#[allow(clippy::too_many_arguments)]
pub async fn extract_additional_frames(
    executable: &Path,
    input: &Path,
    output_directory: &Path,
    mask_directory: &Path,
    plan: &FramePlan,
    has_alpha: bool,
    log_path: Option<PathBuf>,
    process_manager: &ProcessManager,
    observer: Option<ProcessObserver>,
) -> Result<FrameExtractionResult> {
    let parent = output_directory.parent().unwrap_or(output_directory);
    let temporary = parent.join(format!(".planner-backfill-{}", uuid::Uuid::new_v4()));
    let temporary_frames = temporary.join("frames");
    let temporary_masks = temporary.join("masks");
    let result = extract_selected_frames(
        executable,
        input,
        &temporary_frames,
        &temporary_masks,
        plan,
        has_alpha,
        log_path,
        process_manager,
        observer,
    )
    .await;
    if let Err(error) = result {
        let _ = tokio::fs::remove_dir_all(&temporary).await;
        return Err(error);
    }
    tokio::fs::create_dir_all(output_directory).await?;
    move_images(&temporary_frames, output_directory).await?;
    if has_alpha {
        tokio::fs::create_dir_all(mask_directory).await?;
        move_images(&temporary_masks, mask_directory).await?;
    }
    let _ = tokio::fs::remove_dir_all(&temporary).await;
    validate_extraction(output_directory, mask_directory, has_alpha).await
}

async fn rename_selected_outputs(
    frames: &Path,
    masks: &Path,
    plan: &FramePlan,
    has_alpha: bool,
) -> Result<()> {
    let extension = if has_alpha { "png" } else { "jpg" };
    let mut frame_paths = image_paths(frames, extension).await?;
    let mut selected = plan.selected_frames.clone();
    selected.sort_by_key(|frame| frame.source_frame_index);
    if frame_paths.len() != selected.len() {
        return Err(SplatError::Process(format!(
            "Planner selected {} frames but FFmpeg wrote {}",
            selected.len(),
            frame_paths.len()
        )));
    }
    frame_paths.sort();
    for (source, selected) in frame_paths.into_iter().zip(&selected) {
        let destination = frames.join(format!(
            "frame_{:010}.{extension}",
            selected.source_frame_index
        ));
        tokio::fs::rename(source, destination).await?;
    }
    if has_alpha {
        let mut mask_paths = image_paths(masks, "png").await?;
        mask_paths.sort();
        if mask_paths.len() != selected.len() {
            return Err(SplatError::Process(
                "Planner alpha mask count mismatch".into(),
            ));
        }
        for (source, selected) in mask_paths.into_iter().zip(&selected) {
            let destination =
                masks.join(format!("frame_{:010}.png.png", selected.source_frame_index));
            tokio::fs::rename(source, destination).await?;
        }
    }
    Ok(())
}

async fn move_images(source: &Path, destination: &Path) -> Result<()> {
    let mut entries = tokio::fs::read_dir(source).await?;
    while let Some(entry) = entries.next_entry().await? {
        if entry.path().is_file() && is_pipeline_image(&entry.path()) {
            let target = destination.join(entry.file_name());
            if target.exists() {
                return Err(SplatError::Process(format!(
                    "Planner backfill would overwrite {}",
                    target.display()
                )));
            }
            tokio::fs::rename(entry.path(), target).await?;
        }
    }
    Ok(())
}

async fn image_paths(directory: &Path, extension: &str) -> Result<Vec<PathBuf>> {
    let mut output = Vec::new();
    let mut entries = tokio::fs::read_dir(directory).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.is_file()
            && path
                .extension()
                .is_some_and(|value| value.eq_ignore_ascii_case(extension))
        {
            output.push(path);
        }
    }
    Ok(output)
}

fn selection_expression(plan: &FramePlan) -> String {
    let mut indices = plan
        .selected_frames
        .iter()
        .map(|frame| frame.source_frame_index)
        .collect::<Vec<_>>();
    indices.sort_unstable();
    indices.dedup();
    balanced_selection_expression(&indices)
}

/// FFmpeg represents arithmetic expressions as a recursive tree. A linear
/// `eq(n,a)+eq(n,b)+...` list therefore becomes deep enough to fail filter
/// initialization for long videos. This binary decision tree keeps exact
/// source-frame selection while limiting expression depth to O(log N).
fn balanced_selection_expression(indices: &[u64]) -> String {
    let Some((&pivot, rest)) = indices.split_first() else {
        return "0".into();
    };
    if rest.is_empty() {
        return format!("eq(n\\,{pivot})");
    }
    let middle = indices.len() / 2;
    let pivot = indices[middle];
    let left = balanced_selection_expression(&indices[..middle]);
    let right = balanced_selection_expression(&indices[middle + 1..]);
    format!("if(eq(n\\,{pivot})\\,1\\,if(lt(n\\,{pivot})\\,{left}\\,{right}))")
}

fn selected_frame_extraction_args(
    input: &Path,
    output_directory: &Path,
    mask_directory: &Path,
    script_path: &Path,
    has_alpha: bool,
) -> Vec<OsString> {
    let mut args = vec![
        "-hide_banner".into(),
        "-nostdin".into(),
        "-nostats".into(),
        "-y".into(),
        "-i".into(),
        input.as_os_str().to_owned(),
    ];
    if has_alpha {
        args.extend([
            "-filter_complex_script".into(),
            script_path.as_os_str().to_owned(),
            "-map".into(),
            "[rgba]".into(),
            "-c:v".into(),
            "png".into(),
            "-pix_fmt".into(),
            "rgba".into(),
            "-vsync".into(),
            "vfr".into(),
            "-start_number".into(),
            "1".into(),
            output_directory.join("frame_%06d.png").into_os_string(),
            "-map".into(),
            "[mask]".into(),
            "-c:v".into(),
            "png".into(),
            "-pix_fmt".into(),
            "gray".into(),
            "-vsync".into(),
            "vfr".into(),
            "-start_number".into(),
            "1".into(),
            mask_directory.join("frame_%06d.png.png").into_os_string(),
        ]);
    } else {
        args.extend([
            "-filter_script:v".into(),
            script_path.as_os_str().to_owned(),
            "-vsync".into(),
            "vfr".into(),
            "-q:v".into(),
            "2".into(),
            "-start_number".into(),
            "1".into(),
            output_directory.join("frame_%06d.jpg").into_os_string(),
        ]);
    }
    args.extend(["-progress".into(), "pipe:1".into()]);
    args
}

fn frame_extraction_args(
    input: &Path,
    output_directory: &Path,
    mask_directory: &Path,
    sampling_fps: f64,
    has_alpha: bool,
) -> Vec<OsString> {
    let mut args = vec![
        "-hide_banner".into(),
        "-nostdin".into(),
        "-nostats".into(),
        "-y".into(),
        "-i".into(),
        input.as_os_str().to_owned(),
    ];
    let legacy_scale = "scale='min(1920,iw)':'min(1920,ih)':force_original_aspect_ratio=decrease";
    if has_alpha {
        let filter = format!(
            "[0:v]fps={sampling_fps:.8},{legacy_scale},format=rgba,split=2[rgba][masksrc];[masksrc]alphaextract[mask]"
        );
        args.extend([
            "-filter_complex".into(),
            filter.into(),
            "-map".into(),
            "[rgba]".into(),
            "-c:v".into(),
            "png".into(),
            "-pix_fmt".into(),
            "rgba".into(),
            "-start_number".into(),
            "1".into(),
            output_directory.join("frame_%06d.png").into_os_string(),
            "-map".into(),
            "[mask]".into(),
            "-c:v".into(),
            "png".into(),
            "-pix_fmt".into(),
            "gray".into(),
            "-start_number".into(),
            "1".into(),
            mask_directory.join("frame_%06d.png.png").into_os_string(),
        ]);
    } else {
        let filter = format!("fps={sampling_fps:.8},{legacy_scale}");
        args.extend([
            "-vf".into(),
            filter.into(),
            "-q:v".into(),
            "2".into(),
            "-start_number".into(),
            "1".into(),
            output_directory.join("frame_%06d.jpg").into_os_string(),
        ]);
    }
    args.extend(["-progress".into(), "pipe:1".into()]);
    args
}

async fn ensure_clean_output(frames: &Path, masks: &Path) -> Result<()> {
    for directory in [frames, masks] {
        if !directory.exists() {
            continue;
        }
        let mut entries = tokio::fs::read_dir(directory).await?;
        while let Some(entry) = entries.next_entry().await? {
            if entry.path().is_file() && is_pipeline_image(&entry.path()) {
                return Err(SplatError::Process(format!(
                    "输出目录 {} 中已有图像；为避免混用残缺结果，任务已停止",
                    directory.display()
                )));
            }
        }
    }
    Ok(())
}

pub(crate) async fn validate_extraction(
    frames: &Path,
    masks: &Path,
    has_alpha: bool,
) -> Result<FrameExtractionResult> {
    let expected_extension = if has_alpha { "png" } else { "jpg" };
    let frame_names = image_names(frames, expected_extension).await?;
    if frame_names.is_empty() {
        return Err(SplatError::Process("FFmpeg 未输出任何画面".into()));
    }

    let mask_count = if has_alpha {
        let mask_names = image_names(masks, "png").await?;
        if frame_names.len() != mask_names.len()
            || frame_names
                .iter()
                .any(|name| !mask_names.contains(&format!("{name}.png")))
        {
            return Err(SplatError::Process(format!(
                "透明画面与 Alpha Mask 不完整：画面 {} 张，Mask {} 张",
                frame_names.len(),
                mask_names.len()
            )));
        }
        mask_names.len() as u64
    } else {
        0
    };

    Ok(FrameExtractionResult {
        frame_count: frame_names.len() as u64,
        image_format: if has_alpha {
            FrameImageFormat::Png
        } else {
            FrameImageFormat::Jpeg
        },
        mask_count,
        has_alpha,
    })
}

async fn image_names(directory: &Path, extension: &str) -> Result<BTreeSet<String>> {
    let mut names = BTreeSet::new();
    let mut entries = tokio::fs::read_dir(directory).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.is_file()
            && path
                .extension()
                .is_some_and(|value| value.eq_ignore_ascii_case(extension))
        {
            names.insert(entry.file_name().to_string_lossy().into_owned());
        }
    }
    Ok(names)
}

fn is_pipeline_image(path: &Path) -> bool {
    path.extension().is_some_and(|extension| {
        extension.eq_ignore_ascii_case("jpg")
            || extension.eq_ignore_ascii_case("jpeg")
            || extension.eq_ignore_ascii_case("png")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args_as_strings(args: Vec<OsString>) -> Vec<String> {
        args.into_iter()
            .map(|value| value.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn planner_selection_uses_a_balanced_exact_lookup() {
        assert_eq!(balanced_selection_expression(&[]), "0");
        assert_eq!(balanced_selection_expression(&[7]), "eq(n\\,7)");
        assert_eq!(
            balanced_selection_expression(&[1, 4, 9]),
            "if(eq(n\\,4)\\,1\\,if(lt(n\\,4)\\,eq(n\\,1)\\,eq(n\\,9)))"
        );

        let large = balanced_selection_expression(&(0..20_000).collect::<Vec<_>>());
        let mut depth = 0_u32;
        let mut maximum_depth = 0_u32;
        for character in large.chars() {
            match character {
                '(' => {
                    depth += 1;
                    maximum_depth = maximum_depth.max(depth);
                }
                ')' => depth -= 1,
                _ => {}
            }
        }
        assert_eq!(depth, 0);
        assert!(maximum_depth < 64, "expression depth was {maximum_depth}");
        assert!(!large.contains('+'));
    }

    #[test]
    fn installed_ffmpeg_accepts_a_large_balanced_selection_when_configured() {
        let Some(executable) = std::env::var_os("OOOSPLAT_TEST_FFMPEG") else {
            return;
        };
        let temporary = tempfile::tempdir().unwrap();
        let script = temporary.path().join("large-selection.ffscript");
        let indices = (0..50_000).map(|value| value * 2).collect::<Vec<_>>();
        std::fs::write(
            &script,
            format!(
                "select='{}'",
                balanced_selection_expression(indices.as_slice())
            ),
        )
        .unwrap();

        let output = std::process::Command::new(executable)
            .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
            .arg("color=black:s=16x16:r=1:d=1")
            .arg("-filter_script:v")
            .arg(&script)
            .args(["-frames:v", "1", "-f", "null", "-"])
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "FFmpeg rejected the large balanced selection: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn opaque_extraction_keeps_the_jpeg_pipeline() {
        let args = args_as_strings(frame_extraction_args(
            Path::new("input.mov"),
            Path::new("frames"),
            Path::new("masks"),
            15.0,
            false,
        ));
        assert!(args.iter().any(|value| value.ends_with("frame_%06d.jpg")));
        assert!(!args.iter().any(|value| value == "-filter_complex"));
        assert!(!args.iter().any(|value| value.contains("alphaextract")));
        assert!(args.iter().any(|value| value.contains("scale=")));
    }

    #[test]
    fn alpha_extraction_emits_rgba_frames_and_colmap_masks() {
        let args = args_as_strings(frame_extraction_args(
            Path::new("input.mov"),
            Path::new("frames"),
            Path::new("masks"),
            15.0,
            true,
        ));
        assert!(args.iter().any(|value| value == "-filter_complex"));
        assert!(args.iter().any(|value| value.contains("alphaextract")));
        assert!(args.iter().any(|value| value.ends_with("frame_%06d.png")));
        assert!(args
            .iter()
            .any(|value| value.ends_with("frame_%06d.png.png")));
        assert!(args.iter().any(|value| value.contains("scale=")));
    }

    #[test]
    fn planner_extraction_uses_an_explicit_list_not_fps() {
        let args = args_as_strings(selected_frame_extraction_args(
            Path::new("input.mov"),
            Path::new("frames"),
            Path::new("masks"),
            Path::new("selection.ffscript"),
            false,
        ));
        assert!(args.iter().any(|value| value == "-filter_script:v"));
        assert!(!args.iter().any(|value| value.contains("fps=")));
    }

    #[tokio::test]
    async fn validates_matching_alpha_frame_and_mask_names() {
        let temporary = tempfile::tempdir().unwrap();
        let frames = temporary.path().join("frames");
        let masks = temporary.path().join("masks");
        tokio::fs::create_dir_all(&frames).await.unwrap();
        tokio::fs::create_dir_all(&masks).await.unwrap();
        tokio::fs::write(frames.join("frame_000001.png"), b"png")
            .await
            .unwrap();
        tokio::fs::write(masks.join("frame_000001.png.png"), b"mask")
            .await
            .unwrap();
        let result = validate_extraction(&frames, &masks, true).await.unwrap();
        assert_eq!(result.frame_count, 1);
        assert_eq!(result.mask_count, 1);
        assert_eq!(result.image_format, FrameImageFormat::Png);
    }

    #[tokio::test]
    async fn rejects_partial_alpha_output() {
        let temporary = tempfile::tempdir().unwrap();
        let frames = temporary.path().join("frames");
        let masks = temporary.path().join("masks");
        tokio::fs::create_dir_all(&frames).await.unwrap();
        tokio::fs::create_dir_all(&masks).await.unwrap();
        tokio::fs::write(frames.join("frame_000001.png"), b"png")
            .await
            .unwrap();
        let error = validate_extraction(&frames, &masks, true)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("Alpha Mask 不完整"));
    }
}
