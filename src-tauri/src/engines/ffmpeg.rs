use std::{
    collections::BTreeSet,
    ffi::OsString,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{
    error::{Result, SplatError},
    process::{ProcessManager, ProcessObserver, ProcessSpec},
    video::{FramePlan, PlannedFrame},
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
    pub width: u32,
    pub height: u32,
}

#[allow(clippy::too_many_arguments)]
pub async fn extract_uniform_frames(
    executable: &Path,
    input: &Path,
    output_directory: &Path,
    mask_directory: &Path,
    plan: &FramePlan,
    has_alpha: bool,
    target_dimensions: Option<(u32, u32)>,
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
        target_dimensions,
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

/// Extracts the exact source-frame indices selected by Quality v2 and names
/// each output after its source index so COLMAP registrations can be mapped
/// back to the video timeline.
#[allow(clippy::too_many_arguments)]
pub async fn extract_selected_frames(
    executable: &Path,
    input: &Path,
    output_directory: &Path,
    mask_directory: &Path,
    selected_frames: &[PlannedFrame],
    has_alpha: bool,
    target_dimensions: Option<(u32, u32)>,
    log_path: Option<PathBuf>,
    process_manager: &ProcessManager,
    observer: Option<ProcessObserver>,
) -> Result<FrameExtractionResult> {
    if !input.is_file() {
        return Err(SplatError::InvalidPath(input.to_path_buf()));
    }
    if selected_frames.is_empty() {
        return Err(SplatError::Process(
            "Quality v2 did not select any source frames".into(),
        ));
    }
    ensure_clean_output(output_directory, mask_directory).await?;
    tokio::fs::create_dir_all(output_directory).await?;
    if has_alpha {
        tokio::fs::create_dir_all(mask_directory).await?;
    }
    let script_path = output_directory
        .parent()
        .unwrap_or(output_directory)
        .join(format!("frame-selection-{}.ffscript", uuid::Uuid::new_v4()));
    let expression = selection_expression(selected_frames);
    let script = selected_filter_script(&expression, has_alpha, target_dimensions);
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
            "FFmpeg failed to extract the Quality v2 frame list, exit code {:?}{}",
            output.exit_code,
            if detail.is_empty() {
                String::new()
            } else {
                format!("\n{detail}")
            }
        )));
    }
    rename_selected_outputs(output_directory, mask_directory, selected_frames, has_alpha).await?;
    validate_extraction(output_directory, mask_directory, has_alpha).await
}

/// Extracts only Bridge Backfill frames into a temporary directory, then
/// atomically merges those images into the existing frame set.
#[allow(clippy::too_many_arguments)]
pub async fn extract_additional_frames(
    executable: &Path,
    input: &Path,
    output_directory: &Path,
    mask_directory: &Path,
    selected_frames: &[PlannedFrame],
    has_alpha: bool,
    target_dimensions: Option<(u32, u32)>,
    log_path: Option<PathBuf>,
    process_manager: &ProcessManager,
    observer: Option<ProcessObserver>,
) -> Result<FrameExtractionResult> {
    remove_selected_outputs(output_directory, mask_directory, selected_frames, has_alpha).await?;
    let parent = output_directory.parent().unwrap_or(output_directory);
    let temporary = parent.join(format!(".bridge-backfill-{}", uuid::Uuid::new_v4()));
    let temporary_frames = temporary.join("frames");
    let temporary_masks = temporary.join("masks");
    let extraction = extract_selected_frames(
        executable,
        input,
        &temporary_frames,
        &temporary_masks,
        selected_frames,
        has_alpha,
        target_dimensions,
        log_path,
        process_manager,
        observer,
    )
    .await;
    if let Err(error) = extraction {
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

async fn remove_selected_outputs(
    frames: &Path,
    masks: &Path,
    selected: &[PlannedFrame],
    has_alpha: bool,
) -> Result<()> {
    let extension = if has_alpha { "png" } else { "jpg" };
    for frame in selected {
        let name = format!("frame_{:010}.{extension}", frame.source_frame_index);
        match tokio::fs::remove_file(frames.join(&name)).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        if has_alpha {
            match tokio::fs::remove_file(masks.join(format!("{name}.png"))).await {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
    Ok(())
}

async fn rename_selected_outputs(
    frames: &Path,
    masks: &Path,
    selected_frames: &[PlannedFrame],
    has_alpha: bool,
) -> Result<()> {
    let extension = if has_alpha { "png" } else { "jpg" };
    let mut frame_paths = image_paths(frames, extension).await?;
    let mut selected = selected_frames.to_vec();
    selected.sort_by_key(|frame| frame.source_frame_index);
    if frame_paths.len() != selected.len() {
        return Err(SplatError::Process(format!(
            "Quality v2 selected {} frames but FFmpeg wrote {}",
            selected.len(),
            frame_paths.len()
        )));
    }
    frame_paths.sort();
    for (source, frame) in frame_paths.into_iter().zip(&selected) {
        tokio::fs::rename(
            source,
            frames.join(format!(
                "frame_{:010}.{extension}",
                frame.source_frame_index
            )),
        )
        .await?;
    }
    if has_alpha {
        let mut mask_paths = image_paths(masks, "png").await?;
        mask_paths.sort();
        if mask_paths.len() != selected.len() {
            return Err(SplatError::Process(
                "Quality v2 alpha mask count mismatch".into(),
            ));
        }
        for (source, frame) in mask_paths.into_iter().zip(&selected) {
            tokio::fs::rename(
                source,
                masks.join(format!("frame_{:010}.png.png", frame.source_frame_index)),
            )
            .await?;
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
                    "Bridge Backfill would overwrite {}",
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

fn selection_expression(selected_frames: &[PlannedFrame]) -> String {
    let mut indices = selected_frames
        .iter()
        .map(|frame| frame.source_frame_index)
        .collect::<Vec<_>>();
    indices.sort_unstable();
    indices.dedup();
    balanced_selection_expression(&indices)
}

fn selected_filter_script(
    expression: &str,
    has_alpha: bool,
    target_dimensions: Option<(u32, u32)>,
) -> String {
    let scale = target_dimensions
        .map(|(width, height)| format!(",scale={width}:{height}:flags=lanczos"))
        .unwrap_or_default();
    if has_alpha {
        format!("[0:v]select='{expression}'{scale},format=rgba,split=2[rgba][masksrc];[masksrc]alphaextract[mask]")
    } else {
        format!("select='{expression}'{scale}")
    }
}

fn balanced_selection_expression(indices: &[u64]) -> String {
    if indices.is_empty() {
        return "0".into();
    }
    if indices.len() == 1 {
        return format!("eq(n\\,{})", indices[0]);
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
    target_dimensions: Option<(u32, u32)>,
) -> Vec<OsString> {
    let mut args = vec![
        "-hide_banner".into(),
        "-nostdin".into(),
        "-nostats".into(),
        "-y".into(),
        "-i".into(),
        input.as_os_str().to_owned(),
    ];
    let scale = target_dimensions.map_or_else(
        || "scale='min(1920,iw)':'min(1920,ih)':force_original_aspect_ratio=decrease".into(),
        |(width, height)| format!("scale={width}:{height}:flags=lanczos"),
    );
    if has_alpha {
        let filter = format!(
            "[0:v]fps={sampling_fps:.8},{scale},format=rgba,split=2[rgba][masksrc];[masksrc]alphaextract[mask]"
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
        let filter = format!("fps={sampling_fps:.8},{scale}");
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

    let first_frame = frame_names
        .iter()
        .next()
        .ok_or_else(|| SplatError::Process("FFmpeg did not output any frames".into()))?;
    let first_path = frames.join(first_frame);
    let first_mask_path = has_alpha.then(|| masks.join(format!("{first_frame}.png")));
    let (width, height) = tokio::task::spawn_blocking(move || {
        use image::ImageDecoder;
        let decoder = image::ImageReader::open(first_path)?
            .with_guessed_format()?
            .into_decoder()
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        let dimensions = decoder.dimensions();
        if let Some(mask_path) = first_mask_path {
            let mask_decoder = image::ImageReader::open(mask_path)?
                .with_guessed_format()?
                .into_decoder()
                .map_err(|error| std::io::Error::other(error.to_string()))?;
            if mask_decoder.dimensions() != dimensions {
                return Err(std::io::Error::other(
                    "Alpha mask dimensions do not match the extracted frame",
                ));
            }
        }
        Ok::<_, std::io::Error>(dimensions)
    })
    .await
    .map_err(|error| SplatError::Process(format!("Frame dimension task failed: {error}")))??;

    Ok(FrameExtractionResult {
        frame_count: frame_names.len() as u64,
        image_format: if has_alpha {
            FrameImageFormat::Png
        } else {
            FrameImageFormat::Jpeg
        },
        mask_count,
        has_alpha,
        width,
        height,
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
    fn opaque_extraction_keeps_the_jpeg_pipeline() {
        let args = args_as_strings(frame_extraction_args(
            Path::new("input.mov"),
            Path::new("frames"),
            Path::new("masks"),
            15.0,
            false,
            None,
        ));
        assert!(args.iter().any(|value| value.ends_with("frame_%06d.jpg")));
        assert!(!args.iter().any(|value| value == "-filter_complex"));
        assert!(!args.iter().any(|value| value.contains("alphaextract")));
    }

    #[test]
    fn alpha_extraction_emits_rgba_frames_and_colmap_masks() {
        let args = args_as_strings(frame_extraction_args(
            Path::new("input.mov"),
            Path::new("frames"),
            Path::new("masks"),
            15.0,
            true,
            None,
        ));
        assert!(args.iter().any(|value| value == "-filter_complex"));
        assert!(args.iter().any(|value| value.contains("alphaextract")));
        assert!(args.iter().any(|value| value.ends_with("frame_%06d.png")));
        assert!(args
            .iter()
            .any(|value| value.ends_with("frame_%06d.png.png")));
    }

    #[test]
    fn exact_selection_uses_balanced_expression_tree() {
        let selected = (0..1_000)
            .map(|source_frame_index| PlannedFrame {
                source_frame_index,
                timestamp_seconds: source_frame_index as f64 / 30.0,
            })
            .collect::<Vec<_>>();
        let expression = selection_expression(&selected);
        assert!(expression.contains("if(eq(n\\,"));
        assert!(!expression.contains("+eq(n"));
    }

    #[test]
    fn explicit_uniform_target_is_used_only_when_requested() {
        let args = args_as_strings(frame_extraction_args(
            Path::new("input.mov"),
            Path::new("frames"),
            Path::new("masks"),
            8.0,
            false,
            Some((1600, 900)),
        ));
        assert!(args
            .iter()
            .any(|value| value.contains("scale=1600:900:flags=lanczos")));
        assert!(!args.iter().any(|value| value.contains("min(1920,iw)")));
    }

    #[test]
    fn exact_selection_scales_rgb_and_alpha_before_mask_split() {
        let opaque = selected_filter_script("eq(n\\,0)", false, Some((1600, 900)));
        assert!(opaque.contains("scale=1600:900:flags=lanczos"));
        let alpha = selected_filter_script("eq(n\\,0)", true, Some((1920, 1080)));
        assert!(alpha.contains("scale=1920:1080:flags=lanczos,format=rgba,split=2"));
        assert!(alpha.contains("alphaextract[mask]"));
    }

    #[tokio::test]
    async fn validates_matching_alpha_frame_and_mask_names() {
        let temporary = tempfile::tempdir().unwrap();
        let frames = temporary.path().join("frames");
        let masks = temporary.path().join("masks");
        tokio::fs::create_dir_all(&frames).await.unwrap();
        tokio::fs::create_dir_all(&masks).await.unwrap();
        image::RgbaImage::new(2, 2)
            .save(frames.join("frame_000001.png"))
            .unwrap();
        image::GrayImage::new(2, 2)
            .save(masks.join("frame_000001.png.png"))
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
        image::RgbaImage::new(2, 2)
            .save(frames.join("frame_000001.png"))
            .unwrap();
        let error = validate_extraction(&frames, &masks, true)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("Alpha Mask 不完整"));
    }
}
