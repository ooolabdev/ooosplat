use std::{
    collections::BTreeSet,
    ffi::OsString,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{
    error::{Result, SplatError},
    process::{ProcessManager, ProcessObserver, ProcessSpec},
    video::{
        image_sequence::{validate_binary_mask, validate_colmap_frame_copy},
        FramePlan, PlannedFrame,
    },
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

const FRAME_MAPPING_VERSION: u32 = 1;
const FRAME_STATS_FORMAT: &str = "{n}\t{ni}\t{ptsi}\t{tbi}";
const MAX_FRAME_STATS_LINE_BYTES: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct EncodedFrameMapping {
    output_sequence: u64,
    source_frame_index: u64,
    input_pts: Option<i64>,
    input_time_base: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FrameExtractionManifest<'a> {
    version: u32,
    planned_count: usize,
    unique_planned_count: usize,
    planned_frames: &'a [PlannedFrame],
    emitted_frames: &'a [EncodedFrameMapping],
    #[serde(skip_serializing_if = "Option::is_none")]
    emitted_masks: Option<&'a [EncodedFrameMapping]>,
    output_frame_count: usize,
    output_mask_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    drop_frames: Option<u64>,
    missing_source_indices: &'a [u64],
    unexpected_source_indices: &'a [u64],
    mapping_valid: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a str>,
}

#[allow(clippy::too_many_arguments)]
pub async fn extract_uniform_frames(
    executable: &Path,
    input: &Path,
    output_directory: &Path,
    colmap_directory: &Path,
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
    ensure_clean_output(output_directory, colmap_directory, mask_directory).await?;
    tokio::fs::create_dir_all(output_directory).await?;
    tokio::fs::create_dir_all(colmap_directory).await?;
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

    mirror_images(output_directory, colmap_directory).await?;

    validate_extraction(
        output_directory,
        colmap_directory,
        mask_directory,
        has_alpha,
    )
    .await
}

/// Extracts the exact source-frame indices selected by Quality v2 and names
/// each output after its source index so COLMAP registrations can be mapped
/// back to the video timeline.
#[allow(clippy::too_many_arguments)]
pub async fn extract_selected_frames(
    executable: &Path,
    input: &Path,
    output_directory: &Path,
    colmap_directory: &Path,
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
    let selected_frames = validated_selected_frames(selected_frames)?;
    ensure_clean_output(output_directory, colmap_directory, mask_directory).await?;
    tokio::fs::create_dir_all(output_directory).await?;
    tokio::fs::create_dir_all(colmap_directory).await?;
    if has_alpha {
        tokio::fs::create_dir_all(mask_directory).await?;
    }
    let parent = output_directory.parent().unwrap_or(output_directory);
    let extraction_id = uuid::Uuid::new_v4();
    let script_path = parent.join(format!("frame-selection-{extraction_id}.ffscript"));
    let frame_stats_path = parent.join(format!("frame-selection-{extraction_id}-frames.stats"));
    let mask_stats_path =
        has_alpha.then(|| parent.join(format!("frame-selection-{extraction_id}-masks.stats")));
    let manifest_path = parent.join("frame-extraction-map.json");
    let _ = tokio::fs::remove_file(&manifest_path).await;
    let expression = selection_expression(&selected_frames);
    let script = selected_filter_script(&expression, has_alpha, target_dimensions);
    tokio::fs::write(&script_path, script).await?;
    let args = selected_frame_extraction_args(
        input,
        output_directory,
        mask_directory,
        &script_path,
        has_alpha,
        &frame_stats_path,
        mask_stats_path.as_deref(),
    );
    let diagnostic_log = log_path.clone();
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
    let output = match output {
        Ok(output) => output,
        Err(error) => {
            cleanup_frame_stats(&frame_stats_path, mask_stats_path.as_deref()).await;
            return Err(error);
        }
    };
    if !output.success {
        let detail = output.failure_detail();
        cleanup_frame_stats(&frame_stats_path, mask_stats_path.as_deref()).await;
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

    let drop_frames = last_progress_value(&output.stdout, "drop_frames");
    let emitted_frames = match read_frame_stats(&frame_stats_path, selected_frames.len()).await {
        Ok(records) => records,
        Err(error) => {
            let detail = format!("FFmpeg exact frame statistics are unavailable: {error}");
            persist_frame_manifest(
                &manifest_path,
                &selected_frames,
                &[],
                None,
                image_paths(output_directory, if has_alpha { "png" } else { "jpg" })
                    .await
                    .map_or(0, |paths| paths.len()),
                0,
                drop_frames,
                &[],
                &[],
                false,
                Some(&detail),
            )
            .await;
            append_frame_mapping_log(diagnostic_log.as_deref(), &detail).await;
            cleanup_frame_stats(&frame_stats_path, mask_stats_path.as_deref()).await;
            return Err(SplatError::Process(detail));
        }
    };
    let emitted_masks = if let Some(path) = mask_stats_path.as_deref() {
        match read_frame_stats(path, selected_frames.len()).await {
            Ok(records) => Some(records),
            Err(error) => {
                let detail = format!("FFmpeg alpha-mask frame statistics are unavailable: {error}");
                persist_frame_manifest(
                    &manifest_path,
                    &selected_frames,
                    &emitted_frames,
                    None,
                    image_paths(output_directory, "png")
                        .await
                        .map_or(0, |paths| paths.len()),
                    image_paths(mask_directory, "png")
                        .await
                        .map_or(0, |paths| paths.len()),
                    drop_frames,
                    &[],
                    &[],
                    false,
                    Some(&detail),
                )
                .await;
                append_frame_mapping_log(diagnostic_log.as_deref(), &detail).await;
                cleanup_frame_stats(&frame_stats_path, mask_stats_path.as_deref()).await;
                return Err(SplatError::Process(detail));
            }
        }
    } else {
        None
    };
    let extension = if has_alpha { "png" } else { "jpg" };
    let output_frame_count = match image_paths(output_directory, extension).await {
        Ok(paths) => paths.len(),
        Err(error) => {
            cleanup_frame_stats(&frame_stats_path, mask_stats_path.as_deref()).await;
            return Err(error);
        }
    };
    let output_mask_count = if has_alpha {
        match image_paths(mask_directory, "png").await {
            Ok(paths) => paths.len(),
            Err(error) => {
                cleanup_frame_stats(&frame_stats_path, mask_stats_path.as_deref()).await;
                return Err(error);
            }
        }
    } else {
        0
    };
    let assessment = assess_frame_mapping(
        &selected_frames,
        &emitted_frames,
        emitted_masks.as_deref(),
        output_frame_count,
        output_mask_count,
        has_alpha,
    );
    let summary = frame_mapping_summary(
        &selected_frames,
        &emitted_frames,
        output_frame_count,
        output_mask_count,
        drop_frames,
        &assessment,
    );
    persist_frame_manifest(
        &manifest_path,
        &selected_frames,
        &emitted_frames,
        emitted_masks.as_deref(),
        output_frame_count,
        output_mask_count,
        drop_frames,
        &assessment.missing,
        &assessment.unexpected,
        assessment.valid,
        (!assessment.valid).then_some(summary.as_str()),
    )
    .await;
    append_frame_mapping_log(diagnostic_log.as_deref(), &summary).await;
    cleanup_frame_stats(&frame_stats_path, mask_stats_path.as_deref()).await;
    if !assessment.valid {
        return Err(SplatError::Process(summary));
    }

    rename_selected_outputs(output_directory, mask_directory, &emitted_frames, has_alpha).await?;
    mirror_images(output_directory, colmap_directory).await?;
    validate_extraction(
        output_directory,
        colmap_directory,
        mask_directory,
        has_alpha,
    )
    .await
}

/// Extracts only Bridge Backfill frames into a temporary directory, then
/// atomically merges those images into the existing frame set.
#[allow(clippy::too_many_arguments)]
pub async fn extract_additional_frames(
    executable: &Path,
    input: &Path,
    output_directory: &Path,
    colmap_directory: &Path,
    mask_directory: &Path,
    selected_frames: &[PlannedFrame],
    has_alpha: bool,
    target_dimensions: Option<(u32, u32)>,
    log_path: Option<PathBuf>,
    process_manager: &ProcessManager,
    observer: Option<ProcessObserver>,
) -> Result<FrameExtractionResult> {
    remove_selected_outputs(
        output_directory,
        colmap_directory,
        mask_directory,
        selected_frames,
        has_alpha,
    )
    .await?;
    let parent = output_directory.parent().unwrap_or(output_directory);
    let temporary = parent.join(format!(".bridge-backfill-{}", uuid::Uuid::new_v4()));
    let temporary_frames = temporary.join("frames");
    let temporary_colmap_frames = temporary.join("colmap_frames");
    let temporary_masks = temporary.join("masks");
    let extraction = extract_selected_frames(
        executable,
        input,
        &temporary_frames,
        &temporary_colmap_frames,
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
    tokio::fs::create_dir_all(colmap_directory).await?;
    let mut image_sets = vec![
        (temporary_frames.as_path(), output_directory),
        (temporary_colmap_frames.as_path(), colmap_directory),
    ];
    if has_alpha {
        tokio::fs::create_dir_all(mask_directory).await?;
        image_sets.push((temporary_masks.as_path(), mask_directory));
    }
    if let Err(error) = move_image_sets(&image_sets).await {
        let _ = tokio::fs::remove_dir_all(&temporary).await;
        return Err(error);
    }
    let _ = tokio::fs::remove_dir_all(&temporary).await;
    validate_extraction(
        output_directory,
        colmap_directory,
        mask_directory,
        has_alpha,
    )
    .await
}

async fn remove_selected_outputs(
    frames: &Path,
    colmap_frames: &Path,
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
        match tokio::fs::remove_file(colmap_frames.join(&name)).await {
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

fn validated_selected_frames(selected_frames: &[PlannedFrame]) -> Result<Vec<PlannedFrame>> {
    if selected_frames.is_empty() {
        return Err(SplatError::Process(
            "Quality v2 did not select any source frames".into(),
        ));
    }
    if selected_frames
        .iter()
        .any(|frame| !frame.timestamp_seconds.is_finite())
    {
        return Err(SplatError::Process(
            "Quality v2 frame plan contains a non-finite timestamp".into(),
        ));
    }
    let mut selected = selected_frames.to_vec();
    selected.sort_by_key(|frame| frame.source_frame_index);
    if selected
        .windows(2)
        .any(|frames| frames[0].source_frame_index == frames[1].source_frame_index)
    {
        return Err(SplatError::Process(
            "Quality v2 frame plan contains duplicate source indices".into(),
        ));
    }
    Ok(selected)
}

async fn read_frame_stats(path: &Path, expected_count: usize) -> Result<Vec<EncodedFrameMapping>> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || parse_frame_stats(&path, expected_count))
        .await
        .map_err(|error| {
            SplatError::Process(format!("FFmpeg frame statistics task failed: {error}"))
        })?
}

fn parse_frame_stats(path: &Path, expected_count: usize) -> Result<Vec<EncodedFrameMapping>> {
    let file = std::fs::File::open(path)
        .map_err(|error| SplatError::Process(format!("cannot open frame statistics: {error}")))?;
    let mut records = Vec::with_capacity(expected_count.min(16_384));
    for (line_number, line) in BufReader::new(file).split(b'\n').enumerate() {
        let mut line = line.map_err(|error| {
            SplatError::Process(format!("cannot read frame statistics: {error}"))
        })?;
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        if line.is_empty() {
            continue;
        }
        if line.len() > MAX_FRAME_STATS_LINE_BYTES {
            return Err(SplatError::Process(format!(
                "frame statistics line {} exceeds the size limit",
                line_number + 1
            )));
        }
        if records.len() >= expected_count.saturating_add(1) {
            return Err(SplatError::Process(
                "frame statistics contain more records than the extraction plan".into(),
            ));
        }
        let line = std::str::from_utf8(&line).map_err(|_| {
            SplatError::Process(format!(
                "frame statistics line {} is not UTF-8",
                line_number + 1
            ))
        })?;
        let fields = line.split_ascii_whitespace().collect::<Vec<_>>();
        if fields.len() != 4 {
            return Err(SplatError::Process(format!(
                "frame statistics line {} has {} fields instead of 4",
                line_number + 1,
                fields.len()
            )));
        }
        let output_sequence = fields[0].parse::<u64>().map_err(|_| {
            SplatError::Process(format!(
                "frame statistics line {} has an invalid output sequence",
                line_number + 1
            ))
        })?;
        if output_sequence != records.len() as u64 {
            return Err(SplatError::Process(format!(
                "frame statistics output sequence is not contiguous at line {}",
                line_number + 1
            )));
        }
        let source_frame_index = fields[1].parse::<i64>().map_err(|_| {
            SplatError::Process(format!(
                "frame statistics line {} has an invalid source frame index",
                line_number + 1
            ))
        })?;
        let source_frame_index = u64::try_from(source_frame_index).map_err(|_| {
            SplatError::Process(format!(
                "frame statistics line {} does not identify a source frame",
                line_number + 1
            ))
        })?;
        let input_pts = fields[2].parse::<i64>().map_err(|_| {
            SplatError::Process(format!(
                "frame statistics line {} has an invalid input PTS",
                line_number + 1
            ))
        })?;
        let (time_base_numerator, time_base_denominator) = fields[3]
            .split_once('/')
            .and_then(|(numerator, denominator)| {
                Some((
                    numerator.parse::<i64>().ok()?,
                    denominator.parse::<i64>().ok()?,
                ))
            })
            .ok_or_else(|| {
                SplatError::Process(format!(
                    "frame statistics line {} has an invalid input time base",
                    line_number + 1
                ))
            })?;
        if time_base_denominator == 0 {
            return Err(SplatError::Process(format!(
                "frame statistics line {} has a zero time-base denominator",
                line_number + 1
            )));
        }
        records.push(EncodedFrameMapping {
            output_sequence,
            source_frame_index,
            input_pts: (input_pts != i64::MAX).then_some(input_pts),
            input_time_base: format!("{time_base_numerator}/{time_base_denominator}"),
        });
    }
    Ok(records)
}

#[derive(Debug)]
struct FrameMappingAssessment {
    valid: bool,
    sequence_valid: bool,
    emitted_indices_unique: bool,
    alpha_mapping_valid: bool,
    missing: Vec<u64>,
    unexpected: Vec<u64>,
}

fn assess_frame_mapping(
    selected_frames: &[PlannedFrame],
    emitted_frames: &[EncodedFrameMapping],
    emitted_masks: Option<&[EncodedFrameMapping]>,
    output_frame_count: usize,
    output_mask_count: usize,
    has_alpha: bool,
) -> FrameMappingAssessment {
    let planned = selected_frames
        .iter()
        .map(|frame| frame.source_frame_index)
        .collect::<BTreeSet<_>>();
    let emitted = emitted_frames
        .iter()
        .map(|frame| frame.source_frame_index)
        .collect::<BTreeSet<_>>();
    let sequence_valid = emitted_frames
        .iter()
        .enumerate()
        .all(|(index, frame)| frame.output_sequence == index as u64);
    let emitted_indices_unique = emitted.len() == emitted_frames.len();
    let alpha_mapping_valid = if has_alpha {
        emitted_masks.is_some_and(|masks| masks == emitted_frames)
    } else {
        emitted_masks.is_none()
    };
    let missing = planned.difference(&emitted).copied().collect::<Vec<_>>();
    let unexpected = emitted.difference(&planned).copied().collect::<Vec<_>>();
    let expected = selected_frames.len();
    let valid = sequence_valid
        && emitted_indices_unique
        && alpha_mapping_valid
        && missing.is_empty()
        && unexpected.is_empty()
        && emitted_frames.len() == expected
        && output_frame_count == expected
        && (!has_alpha || output_mask_count == expected);
    FrameMappingAssessment {
        valid,
        sequence_valid,
        emitted_indices_unique,
        alpha_mapping_valid,
        missing,
        unexpected,
    }
}

fn last_progress_value(output: &str, key: &str) -> Option<u64> {
    output.lines().rev().find_map(|line| {
        let (name, value) = line.trim().split_once('=')?;
        (name == key).then(|| value.trim().parse::<u64>().ok())?
    })
}

fn compact_index_ranges(indices: &[u64]) -> String {
    const MAX_RANGES: usize = 64;
    if indices.is_empty() {
        return "none".into();
    }
    let mut ranges = Vec::new();
    let mut start = indices[0];
    let mut end = start;
    for index in indices.iter().copied().skip(1) {
        if index == end.saturating_add(1) {
            end = index;
        } else {
            ranges.push((start, end));
            start = index;
            end = index;
        }
    }
    ranges.push((start, end));
    let omitted = ranges.len().saturating_sub(MAX_RANGES);
    let mut text = ranges
        .iter()
        .take(MAX_RANGES)
        .map(|(start, end)| {
            if start == end {
                start.to_string()
            } else {
                format!("{start}-{end}")
            }
        })
        .collect::<Vec<_>>()
        .join(",");
    if omitted > 0 {
        text.push_str(&format!(",...(+{omitted} ranges)"));
    }
    text
}

fn frame_mapping_summary(
    selected_frames: &[PlannedFrame],
    emitted_frames: &[EncodedFrameMapping],
    output_frame_count: usize,
    output_mask_count: usize,
    drop_frames: Option<u64>,
    assessment: &FrameMappingAssessment,
) -> String {
    let unique_planned_count = selected_frames
        .iter()
        .map(|frame| frame.source_frame_index)
        .collect::<BTreeSet<_>>()
        .len();
    format!(
        "FFmpeg exact frame mapping: planned={}, unique={}, emitted={}, images={}, masks={}, drop_frames={}, sequence_valid={}, emitted_unique={}, alpha_mapping_valid={}, missing={}, unexpected={}",
        selected_frames.len(),
        unique_planned_count,
        emitted_frames.len(),
        output_frame_count,
        output_mask_count,
        drop_frames.map_or_else(|| "unknown".into(), |value| value.to_string()),
        assessment.sequence_valid,
        assessment.emitted_indices_unique,
        assessment.alpha_mapping_valid,
        compact_index_ranges(&assessment.missing),
        compact_index_ranges(&assessment.unexpected),
    )
}

#[allow(clippy::too_many_arguments)]
async fn persist_frame_manifest(
    path: &Path,
    selected_frames: &[PlannedFrame],
    emitted_frames: &[EncodedFrameMapping],
    emitted_masks: Option<&[EncodedFrameMapping]>,
    output_frame_count: usize,
    output_mask_count: usize,
    drop_frames: Option<u64>,
    missing_source_indices: &[u64],
    unexpected_source_indices: &[u64],
    mapping_valid: bool,
    error: Option<&str>,
) {
    let unique_planned_count = selected_frames
        .iter()
        .map(|frame| frame.source_frame_index)
        .collect::<BTreeSet<_>>()
        .len();
    let manifest = FrameExtractionManifest {
        version: FRAME_MAPPING_VERSION,
        planned_count: selected_frames.len(),
        unique_planned_count,
        planned_frames: selected_frames,
        emitted_frames,
        emitted_masks,
        output_frame_count,
        output_mask_count,
        drop_frames,
        missing_source_indices,
        unexpected_source_indices,
        mapping_valid,
        error,
    };
    if let Ok(bytes) = serde_json::to_vec_pretty(&manifest) {
        let _ = tokio::fs::write(path, bytes).await;
    }
}

async fn append_frame_mapping_log(path: Option<&Path>, summary: &str) {
    use tokio::io::AsyncWriteExt;

    let Some(path) = path else {
        return;
    };
    if let Ok(mut file) = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
    {
        let _ = file
            .write_all(format!("\n[OOOSplat] {summary}\n").as_bytes())
            .await;
    }
}

async fn cleanup_frame_stats(frame_stats: &Path, mask_stats: Option<&Path>) {
    let _ = tokio::fs::remove_file(frame_stats).await;
    if let Some(path) = mask_stats {
        let _ = tokio::fs::remove_file(path).await;
    }
}

async fn rename_selected_outputs(
    frames: &Path,
    masks: &Path,
    emitted_frames: &[EncodedFrameMapping],
    has_alpha: bool,
) -> Result<()> {
    let extension = if has_alpha { "png" } else { "jpg" };
    let mut frame_paths = image_paths(frames, extension).await?;
    if frame_paths.len() != emitted_frames.len() {
        return Err(SplatError::Process(format!(
            "FFmpeg frame mapping contains {} records but {} images were written",
            emitted_frames.len(),
            frame_paths.len()
        )));
    }
    frame_paths.sort();
    for (source, frame) in frame_paths.into_iter().zip(emitted_frames) {
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
        if mask_paths.len() != emitted_frames.len() {
            return Err(SplatError::Process(
                "Quality v2 alpha mask count mismatch".into(),
            ));
        }
        for (source, frame) in mask_paths.into_iter().zip(emitted_frames) {
            tokio::fs::rename(
                source,
                masks.join(format!("frame_{:010}.png.png", frame.source_frame_index)),
            )
            .await?;
        }
    }
    Ok(())
}

async fn move_image_sets(sets: &[(&Path, &Path)]) -> Result<()> {
    let mut pending = Vec::new();
    for (source, destination) in sets {
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
                pending.push((entry.path(), target));
            }
        }
    }
    let mut moved = Vec::new();
    for (source, target) in pending {
        if let Err(error) = tokio::fs::rename(&source, &target).await {
            for (original, destination) in moved.into_iter().rev() {
                let _ = tokio::fs::rename(destination, original).await;
            }
            return Err(error.into());
        }
        moved.push((source, target));
    }
    Ok(())
}

async fn mirror_images(source: &Path, destination: &Path) -> Result<()> {
    tokio::fs::create_dir_all(destination).await?;
    let mut entries = tokio::fs::read_dir(source).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if !path.is_file() || !is_pipeline_image(&path) {
            continue;
        }
        let target = destination.join(entry.file_name());
        if target.exists() {
            tokio::fs::remove_file(&target).await?;
        }
        if tokio::fs::hard_link(&path, &target).await.is_err() {
            tokio::fs::copy(&path, &target).await?;
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
        format!(
            "[0:v]select='{expression}'{scale},format=rgba,split=2[rgba][masksrc];\
             [masksrc]alphaextract,lut=y='if(gte(val\\,128)\\,255\\,0)',format=gray[mask]"
        )
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
    frame_stats_path: &Path,
    mask_stats_path: Option<&Path>,
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
            "-fps_mode".into(),
            "passthrough".into(),
            "-stats_enc_pre".into(),
            frame_stats_path.as_os_str().to_owned(),
            "-stats_enc_pre_fmt".into(),
            FRAME_STATS_FORMAT.into(),
            "-start_number".into(),
            "1".into(),
            output_directory.join("frame_%06d.png").into_os_string(),
            "-map".into(),
            "[mask]".into(),
            "-c:v".into(),
            "png".into(),
            "-pix_fmt".into(),
            "gray".into(),
            "-fps_mode".into(),
            "passthrough".into(),
            "-stats_enc_pre".into(),
            mask_stats_path
                .expect("alpha extraction requires mask stats")
                .as_os_str()
                .to_owned(),
            "-stats_enc_pre_fmt".into(),
            FRAME_STATS_FORMAT.into(),
            "-start_number".into(),
            "1".into(),
            mask_directory.join("frame_%06d.png.png").into_os_string(),
        ]);
    } else {
        args.extend([
            "-filter_script:v".into(),
            script_path.as_os_str().to_owned(),
            "-fps_mode".into(),
            "passthrough".into(),
            "-stats_enc_pre".into(),
            frame_stats_path.as_os_str().to_owned(),
            "-stats_enc_pre_fmt".into(),
            FRAME_STATS_FORMAT.into(),
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
            "[0:v]fps={sampling_fps:.8},{scale},format=rgba,split=2[rgba][masksrc];\
             [masksrc]alphaextract,lut=y='if(gte(val\\,128)\\,255\\,0)',format=gray[mask]"
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

async fn ensure_clean_output(frames: &Path, colmap_frames: &Path, masks: &Path) -> Result<()> {
    for directory in [frames, colmap_frames, masks] {
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
    colmap_frames: &Path,
    masks: &Path,
    has_alpha: bool,
) -> Result<FrameExtractionResult> {
    let expected_extension = if has_alpha { "png" } else { "jpg" };
    let frame_names = image_names(frames, expected_extension).await?;
    if frame_names.is_empty() {
        return Err(SplatError::Process("FFmpeg 未输出任何画面".into()));
    }

    let colmap_names = image_names(colmap_frames, expected_extension).await?;
    if frame_names != colmap_names {
        return Err(SplatError::Process(format!(
            "COLMAP 专用图片不完整：原始帧 {} 张，COLMAP 图片 {} 张",
            frame_names.len(),
            colmap_names.len()
        )));
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

    let validation_entries = frame_names
        .iter()
        .map(|name| {
            (
                frames.join(name),
                colmap_frames.join(name),
                has_alpha.then(|| masks.join(format!("{name}.png"))),
            )
        })
        .collect::<Vec<_>>();
    let (width, height) = tokio::task::spawn_blocking(move || -> Result<(u32, u32)> {
        use image::ImageDecoder;
        let mut expected_dimensions = None;
        for (frame_path, colmap_path, mask_path) in validation_entries {
            for path in [&frame_path, &colmap_path] {
                if std::fs::metadata(path)?.len() == 0 {
                    return Err(std::io::Error::other(format!(
                        "prepared image is empty: {}",
                        path.display()
                    ))
                    .into());
                }
            }
            let decoder = image::ImageReader::open(&frame_path)?
                .with_guessed_format()?
                .into_decoder()
                .map_err(|error| std::io::Error::other(error.to_string()))?;
            let dimensions = decoder.dimensions();
            if expected_dimensions.is_some_and(|expected| expected != dimensions) {
                return Err(
                    std::io::Error::other("extracted frame dimensions are inconsistent").into(),
                );
            }
            expected_dimensions = Some(dimensions);

            validate_colmap_frame_copy(&frame_path, &colmap_path)?;
            if let Some(mask_path) = mask_path {
                if std::fs::metadata(&mask_path)?.len() == 0 {
                    return Err(std::io::Error::other(format!(
                        "alpha mask is empty: {}",
                        mask_path.display()
                    ))
                    .into());
                }
                validate_binary_mask(&mask_path, dimensions)?;
            }
        }
        expected_dimensions
            .ok_or_else(|| SplatError::Process("FFmpeg did not output any frames".into()))
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
    fn alpha_extraction_emits_rgba_frames_and_binary_masks_once() {
        let args = args_as_strings(frame_extraction_args(
            Path::new("input.mov"),
            Path::new("frames"),
            Path::new("masks"),
            15.0,
            true,
            None,
        ));
        assert!(args.iter().any(|value| value == "-filter_complex"));
        assert_eq!(
            args.iter().filter(|value| value.as_str() == "-i").count(),
            1
        );
        assert!(args.iter().any(|value| value.contains("alphaextract")));
        assert!(args.iter().any(|value| value.contains("split=2")));
        assert!(args
            .iter()
            .any(|value| value.contains("if(gte(val\\,128)\\,255\\,0)")));
        assert!(!args.iter().any(|value| value.contains("geq=")));
        assert!(!args.iter().any(|value| value == "[colmap]"));
        assert!(args.iter().any(|value| value.ends_with("frame_%06d.png")));
        assert!(args
            .iter()
            .any(|value| value.ends_with("frame_%06d.png.png")));
        assert!(!args.iter().any(|value| value.contains("colmap_frames")));
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
    fn selected_alpha_extraction_has_one_input_and_no_colmap_output() {
        let args = args_as_strings(selected_frame_extraction_args(
            Path::new("input.mov"),
            Path::new("frames"),
            Path::new("masks"),
            Path::new("selection.ffscript"),
            true,
            Path::new("frames.stats"),
            Some(Path::new("masks.stats")),
        ));
        assert_eq!(
            args.iter().filter(|value| value.as_str() == "-i").count(),
            1
        );
        assert!(!args.iter().any(|value| value == "[colmap]"));
        assert!(!args.iter().any(|value| value.contains("colmap_frames")));
        assert_eq!(
            args.iter().filter(|value| value.as_str() == "-map").count(),
            2
        );
        assert_eq!(
            args.iter()
                .filter(|value| value.as_str() == "-fps_mode")
                .count(),
            2
        );
        assert_eq!(
            args.iter()
                .filter(|value| value.as_str() == "passthrough")
                .count(),
            2
        );
        assert_eq!(
            args.iter()
                .filter(|value| value.as_str() == "-stats_enc_pre")
                .count(),
            2
        );
        assert!(!args.iter().any(|value| value == "-vsync"));
        assert!(!args.iter().any(|value| value == "vfr"));
    }

    #[test]
    fn selected_frame_plan_rejects_duplicate_indices_and_invalid_timestamps() {
        let duplicate = [
            PlannedFrame {
                source_frame_index: 4,
                timestamp_seconds: 0.1,
            },
            PlannedFrame {
                source_frame_index: 4,
                timestamp_seconds: 0.2,
            },
        ];
        assert!(validated_selected_frames(&duplicate)
            .unwrap_err()
            .to_string()
            .contains("duplicate"));
        let invalid = [PlannedFrame {
            source_frame_index: 4,
            timestamp_seconds: f64::NAN,
        }];
        assert!(validated_selected_frames(&invalid)
            .unwrap_err()
            .to_string()
            .contains("non-finite"));
    }

    #[test]
    fn frame_statistics_keep_source_indices_and_reject_unavailable_mapping() {
        let temporary = tempfile::tempdir().unwrap();
        let stats = temporary.path().join("frames.stats");
        std::fs::write(&stats, "0\t5\t100\t1/1000\n1\t9\t200\t1/1000\n").unwrap();
        assert_eq!(
            parse_frame_stats(&stats, 2).unwrap(),
            vec![
                EncodedFrameMapping {
                    output_sequence: 0,
                    source_frame_index: 5,
                    input_pts: Some(100),
                    input_time_base: "1/1000".into(),
                },
                EncodedFrameMapping {
                    output_sequence: 1,
                    source_frame_index: 9,
                    input_pts: Some(200),
                    input_time_base: "1/1000".into(),
                },
            ]
        );

        std::fs::write(&stats, "0\t-1\t9223372036854775807\t0/1\n").unwrap();
        assert!(parse_frame_stats(&stats, 1)
            .unwrap_err()
            .to_string()
            .contains("does not identify"));
    }

    #[test]
    fn mapping_assessment_rejects_missing_unexpected_and_divergent_alpha_records() {
        let selected = [
            PlannedFrame {
                source_frame_index: 5,
                timestamp_seconds: 0.1,
            },
            PlannedFrame {
                source_frame_index: 9,
                timestamp_seconds: 0.3,
            },
        ];
        let emitted = [EncodedFrameMapping {
            output_sequence: 0,
            source_frame_index: 5,
            input_pts: Some(100),
            input_time_base: "1/1000".into(),
        }];
        let assessment = assess_frame_mapping(&selected, &emitted, None, 1, 0, false);
        assert!(!assessment.valid);
        assert_eq!(assessment.missing, vec![9]);
        assert!(assessment.unexpected.is_empty());

        let unexpected = [
            emitted[0].clone(),
            EncodedFrameMapping {
                output_sequence: 1,
                source_frame_index: 12,
                input_pts: Some(200),
                input_time_base: "1/1000".into(),
            },
        ];
        let assessment = assess_frame_mapping(&selected, &unexpected, None, 2, 0, false);
        assert_eq!(assessment.missing, vec![9]);
        assert_eq!(assessment.unexpected, vec![12]);

        let assessment = assess_frame_mapping(&selected[..1], &emitted, Some(&[]), 1, 1, true);
        assert!(!assessment.valid);
        assert!(!assessment.alpha_mapping_valid);
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
        assert!(alpha.contains("alphaextract,lut="));
        assert!(alpha.contains("if(gte(val\\,128)\\,255\\,0)"));
        assert!(!alpha.contains("geq="));
        assert!(!alpha.contains("[colmap]"));
    }

    #[tokio::test]
    async fn validates_matching_alpha_frame_and_mask_names() {
        let temporary = tempfile::tempdir().unwrap();
        let frames = temporary.path().join("frames");
        let colmap_frames = temporary.path().join("colmap_frames");
        let masks = temporary.path().join("masks");
        tokio::fs::create_dir_all(&frames).await.unwrap();
        tokio::fs::create_dir_all(&colmap_frames).await.unwrap();
        tokio::fs::create_dir_all(&masks).await.unwrap();
        image::RgbaImage::new(2, 2)
            .save(frames.join("frame_000001.png"))
            .unwrap();
        tokio::fs::copy(
            frames.join("frame_000001.png"),
            colmap_frames.join("frame_000001.png"),
        )
        .await
        .unwrap();
        image::GrayImage::new(2, 2)
            .save(masks.join("frame_000001.png.png"))
            .unwrap();
        let result = validate_extraction(&frames, &colmap_frames, &masks, true)
            .await
            .unwrap();
        assert_eq!(result.frame_count, 1);
        assert_eq!(result.mask_count, 1);
        assert_eq!(result.image_format, FrameImageFormat::Png);
    }

    #[tokio::test]
    async fn selected_outputs_are_renamed_before_identical_colmap_mirroring() {
        let temporary = tempfile::tempdir().unwrap();
        let frames = temporary.path().join("frames");
        let colmap_frames = temporary.path().join("colmap_frames");
        let masks = temporary.path().join("masks");
        for directory in [&frames, &colmap_frames, &masks] {
            tokio::fs::create_dir_all(directory).await.unwrap();
        }
        for index in 1..=2 {
            let name = format!("frame_{index:06}.png");
            image::RgbaImage::from_pixel(2, 2, image::Rgba([index as u8, 2, 3, 127]))
                .save(frames.join(&name))
                .unwrap();
            image::GrayImage::from_pixel(2, 2, image::Luma([if index == 1 { 0 } else { 255 }]))
                .save(masks.join(format!("{name}.png")))
                .unwrap();
        }
        let emitted = [
            EncodedFrameMapping {
                output_sequence: 0,
                source_frame_index: 5,
                input_pts: Some(100),
                input_time_base: "1/1000".into(),
            },
            EncodedFrameMapping {
                output_sequence: 1,
                source_frame_index: 9,
                input_pts: Some(300),
                input_time_base: "1/1000".into(),
            },
        ];

        rename_selected_outputs(&frames, &masks, &emitted, true)
            .await
            .unwrap();
        mirror_images(&frames, &colmap_frames).await.unwrap();

        for name in ["frame_0000000005.png", "frame_0000000009.png"] {
            assert!(masks.join(format!("{name}.png")).is_file());
            assert_eq!(
                tokio::fs::read(frames.join(name)).await.unwrap(),
                tokio::fs::read(colmap_frames.join(name)).await.unwrap()
            );
        }
        validate_extraction(&frames, &colmap_frames, &masks, true)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn accepts_matching_alpha_images_and_rejects_content_or_dimension_mismatches() {
        let temporary = tempfile::tempdir().unwrap();
        let frames = temporary.path().join("frames");
        let colmap_frames = temporary.path().join("colmap_frames");
        let masks = temporary.path().join("masks");
        for directory in [&frames, &colmap_frames, &masks] {
            tokio::fs::create_dir_all(directory).await.unwrap();
        }
        image::RgbaImage::new(2, 2)
            .save(frames.join("frame_000001.png"))
            .unwrap();
        tokio::fs::copy(
            frames.join("frame_000001.png"),
            colmap_frames.join("frame_000001.png"),
        )
        .await
        .unwrap();
        image::GrayImage::new(2, 2)
            .save(masks.join("frame_000001.png.png"))
            .unwrap();
        assert!(validate_extraction(&frames, &colmap_frames, &masks, true)
            .await
            .is_ok());

        image::GrayImage::from_pixel(2, 2, image::Luma([1]))
            .save(masks.join("frame_000001.png.png"))
            .unwrap();
        assert!(validate_extraction(&frames, &colmap_frames, &masks, true)
            .await
            .is_err());
        image::GrayImage::new(2, 2)
            .save(masks.join("frame_000001.png.png"))
            .unwrap();

        tokio::fs::remove_file(colmap_frames.join("frame_000001.png"))
            .await
            .unwrap();
        image::RgbaImage::from_pixel(2, 2, image::Rgba([9, 8, 7, 0]))
            .save(colmap_frames.join("frame_000001.png"))
            .unwrap();
        assert!(validate_extraction(&frames, &colmap_frames, &masks, true)
            .await
            .is_err());

        tokio::fs::remove_file(colmap_frames.join("frame_000001.png"))
            .await
            .unwrap();
        image::RgbaImage::new(3, 2)
            .save(colmap_frames.join("frame_000001.png"))
            .unwrap();
        assert!(validate_extraction(&frames, &colmap_frames, &masks, true)
            .await
            .is_err());
    }

    #[tokio::test]
    #[ignore = "requires bundled FFmpeg"]
    async fn duplicate_pts_selected_frames_are_not_dropped() {
        let executable = PathBuf::from(
            std::env::var("OOOSPLAT_VALIDATION_FFMPEG")
                .expect("OOOSPLAT_VALIDATION_FFMPEG is required"),
        );
        let temporary = tempfile::tempdir().unwrap();
        let input = temporary.path().join("duplicate-pts.mkv");
        let process_manager = ProcessManager::new();
        let generated = process_manager
            .run(ProcessSpec {
                executable: executable.clone(),
                args: vec![
                    "-hide_banner".into(),
                    "-loglevel".into(),
                    "error".into(),
                    "-f".into(),
                    "lavfi".into(),
                    "-i".into(),
                    "testsrc2=size=64x64:rate=10:duration=1".into(),
                    "-vf".into(),
                    "setpts=5/TB+floor(N/2)".into(),
                    "-fps_mode".into(),
                    "passthrough".into(),
                    "-c:v".into(),
                    "ffv1".into(),
                    "-y".into(),
                    input.clone().into_os_string(),
                ],
                working_directory: Some(temporary.path().to_path_buf()),
                log_path: None,
                observer: None,
            })
            .await
            .unwrap();
        assert!(generated.success, "{}", generated.failure_detail());

        let selected = (0..10)
            .map(|source_frame_index| PlannedFrame {
                source_frame_index,
                timestamp_seconds: 5.0 + (source_frame_index / 2) as f64 / 10.0,
            })
            .collect::<Vec<_>>();
        let frames = temporary.path().join("frames");
        let colmap_frames = temporary.path().join("colmap_frames");
        let masks = temporary.path().join("masks");
        let result = extract_selected_frames(
            &executable,
            &input,
            &frames,
            &colmap_frames,
            &masks,
            &selected,
            false,
            None,
            Some(temporary.path().join("ffmpeg.log")),
            &process_manager,
            None,
        )
        .await
        .unwrap();
        assert_eq!(result.frame_count, 10);
        for index in 0..10 {
            assert!(frames.join(format!("frame_{index:010}.jpg")).is_file());
        }
        let manifest: serde_json::Value = serde_json::from_slice(
            &tokio::fs::read(temporary.path().join("frame-extraction-map.json"))
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["mappingValid"], true);
        assert_eq!(manifest["emittedFrames"].as_array().unwrap().len(), 10);

        let missing_root = temporary.path().join("missing-selection");
        let missing = [
            PlannedFrame {
                source_frame_index: 0,
                timestamp_seconds: 0.0,
            },
            PlannedFrame {
                source_frame_index: 10,
                timestamp_seconds: 0.0,
            },
        ];
        let error = extract_selected_frames(
            &executable,
            &input,
            &missing_root.join("frames"),
            &missing_root.join("colmap_frames"),
            &missing_root.join("masks"),
            &missing,
            false,
            None,
            Some(missing_root.join("ffmpeg.log")),
            &process_manager,
            None,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("missing=10"));
        assert!(
            crate::video::list_images(&missing_root.join("colmap_frames"))
                .unwrap()
                .is_empty()
        );
        let manifest: serde_json::Value = serde_json::from_slice(
            &tokio::fs::read(missing_root.join("frame-extraction-map.json"))
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["mappingValid"], false);
        assert_eq!(manifest["missingSourceIndices"], serde_json::json!([10]));
    }

    #[tokio::test]
    #[ignore = "requires bundled FFmpeg and an external alpha video"]
    async fn selected_and_bridge_alpha_integration() {
        let executable = PathBuf::from(
            std::env::var("OOOSPLAT_VALIDATION_FFMPEG")
                .expect("OOOSPLAT_VALIDATION_FFMPEG is required"),
        );
        let input = PathBuf::from(
            std::env::var("OOOSPLAT_VALIDATION_VIDEO")
                .expect("OOOSPLAT_VALIDATION_VIDEO is required"),
        );
        let output_parent = PathBuf::from(
            std::env::var("OOOSPLAT_VALIDATION_OUTPUT")
                .expect("OOOSPLAT_VALIDATION_OUTPUT is required"),
        );
        let root = output_parent.join(format!("selected-bridge-{}", uuid::Uuid::new_v4()));
        let frames = root.join("frames");
        let colmap_frames = root.join("colmap_frames");
        let masks = root.join("masks");
        let process_manager = ProcessManager::new();
        let initial = [
            PlannedFrame {
                source_frame_index: 0,
                timestamp_seconds: 0.0,
            },
            PlannedFrame {
                source_frame_index: 224,
                timestamp_seconds: 224.0 / 30.0,
            },
        ];
        let extracted = extract_selected_frames(
            &executable,
            &input,
            &frames,
            &colmap_frames,
            &masks,
            &initial,
            true,
            None,
            None,
            &process_manager,
            None,
        )
        .await
        .unwrap();
        assert_eq!(extracted.frame_count, 2);
        let bridge = [PlannedFrame {
            source_frame_index: 30,
            timestamp_seconds: 1.0,
        }];
        let merged = extract_additional_frames(
            &executable,
            &input,
            &frames,
            &colmap_frames,
            &masks,
            &bridge,
            true,
            None,
            None,
            &process_manager,
            None,
        )
        .await
        .unwrap();
        assert_eq!(merged.frame_count, 3);
        assert_eq!(merged.mask_count, 3);
        for name in image_names(&frames, "png").await.unwrap() {
            assert_eq!(
                tokio::fs::read(frames.join(&name)).await.unwrap(),
                tokio::fs::read(colmap_frames.join(&name)).await.unwrap()
            );
            let mask = image::open(masks.join(format!("{name}.png")))
                .unwrap()
                .to_luma8();
            assert!(mask
                .into_raw()
                .into_iter()
                .all(|value| value == 0 || value == 255));
        }
        eprintln!("selected/Bridge validation output: {}", root.display());
    }

    #[tokio::test]
    async fn rejects_partial_alpha_output() {
        let temporary = tempfile::tempdir().unwrap();
        let frames = temporary.path().join("frames");
        let colmap_frames = temporary.path().join("colmap_frames");
        let masks = temporary.path().join("masks");
        tokio::fs::create_dir_all(&frames).await.unwrap();
        tokio::fs::create_dir_all(&colmap_frames).await.unwrap();
        tokio::fs::create_dir_all(&masks).await.unwrap();
        image::RgbaImage::new(2, 2)
            .save(frames.join("frame_000001.png"))
            .unwrap();
        tokio::fs::copy(
            frames.join("frame_000001.png"),
            colmap_frames.join("frame_000001.png"),
        )
        .await
        .unwrap();
        let error = validate_extraction(&frames, &colmap_frames, &masks, true)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("Alpha Mask 不完整"));
    }
}
