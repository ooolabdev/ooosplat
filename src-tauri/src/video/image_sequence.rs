use std::{
    cmp::Ordering,
    collections::BTreeSet,
    fs::File,
    io::{BufReader, Read},
    path::{Path, PathBuf},
    sync::Arc,
};

use image::{ColorType, DynamicImage, GenericImageView, GrayImage, ImageReader, Luma};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::{
    error::{Result, SplatError},
    presets::QualityPreset,
    video::FramePlan,
};

pub const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png"];
pub const LARGE_SEQUENCE_WARNING_COUNT: u64 = 500;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageSequenceInfo {
    pub image_count: u64,
    pub width: u32,
    pub height: u32,
    pub has_alpha: bool,
    pub requires_large_sequence_confirmation: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedImageSequence {
    pub image_count: u64,
    pub mask_count: u64,
    pub has_alpha: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannedImage {
    pub path: PathBuf,
    pub has_alpha_channel: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImageSequenceScan {
    pub info: ImageSequenceInfo,
    pub images: Vec<ScannedImage>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageSequenceNaming {
    Primary,
    Reshoot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImagePreparationPhase {
    LinkingFrames,
    InspectingAlpha,
    WritingOpaqueMasks,
    Validating,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImagePreparationProgress {
    pub phase: ImagePreparationPhase,
    pub current: u64,
    pub total: u64,
    pub stage_progress: f32,
}

pub type ImagePreparationObserver = Arc<dyn Fn(ImagePreparationProgress) + Send + Sync + 'static>;

pub fn is_image_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .is_some_and(|ext| IMAGE_EXTENSIONS.contains(&ext.as_str()))
}

pub fn list_images(dir: &Path) -> Result<Vec<PathBuf>> {
    if !dir.is_dir() {
        return Err(SplatError::InvalidPath(dir.to_path_buf()));
    }
    let mut files = std::fs::read_dir(dir)?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.is_file() && is_image_file(path))
        .collect::<Vec<_>>();
    files.sort_by(|left, right| natural_cmp(&file_name(left), &file_name(right)));
    Ok(files)
}

pub fn analyze_image_sequence(dir: &Path) -> Result<ImageSequenceInfo> {
    Ok(scan_image_sequence(dir, None)?.info)
}

pub fn scan_image_sequence(
    dir: &Path,
    cancellation: Option<&CancellationToken>,
) -> Result<ImageSequenceScan> {
    scan_image_sequence_with_boundary(dir, cancellation, None)
}

pub fn scan_image_sequence_with_boundary(
    dir: &Path,
    cancellation: Option<&CancellationToken>,
    boundary: Option<&Path>,
) -> Result<ImageSequenceScan> {
    if let Some(boundary) = boundary {
        crate::tasks::authorize_input(dir, &[boundary.to_path_buf()])
            .map_err(|error| SplatError::Process(error.to_string()))?;
    }
    let files = list_images(dir)?;
    if files.len() < 2 {
        return Err(SplatError::InvalidVideo(
            "图片序列文件夹中至少需要 2 张 JPG、JPEG 或 PNG 图片".into(),
        ));
    }

    let mut dimensions: Option<(u32, u32)> = None;
    let mut has_alpha = false;
    let mut images = Vec::with_capacity(files.len());
    for path in files {
        if let Some(boundary) = boundary {
            crate::tasks::authorize_input(&path, &[boundary.to_path_buf()])
                .map_err(|e| SplatError::Process(e.to_string()))?;
        }
        ensure_not_cancelled(cancellation)?;
        let (current, has_alpha_channel) = read_image_header(&path)?;
        if let Some(expected) = dimensions {
            if expected != current {
                return Err(SplatError::InvalidVideo(format!(
                    "图片序列分辨率不一致：{} 为 {}×{}，预期 {}×{}",
                    path.display(),
                    current.0,
                    current.1,
                    expected.0,
                    expected.1
                )));
            }
        } else {
            dimensions = Some(current);
        }
        has_alpha |= has_alpha_channel;
        images.push(ScannedImage {
            path,
            has_alpha_channel,
        });
    }

    let (width, height) = dimensions.expect("two image headers provide dimensions");
    let image_count = images.len() as u64;
    Ok(ImageSequenceScan {
        info: ImageSequenceInfo {
            image_count,
            width,
            height,
            has_alpha,
            requires_large_sequence_confirmation: image_count > LARGE_SEQUENCE_WARNING_COUNT,
        },
        images,
    })
}

pub fn create_plan(info: &ImageSequenceInfo, _preset: &QualityPreset) -> FramePlan {
    FramePlan {
        retention_ratio: 1.0,
        sampling_fps: 0.0,
        estimated_frames: info.image_count,
        rescue_max_frames: info.image_count,
        ..FramePlan::default()
    }
}

pub fn normalized_image_name(index: usize, source: &Path) -> Result<String> {
    normalized_sequence_name(index, source, "frame_")
}

fn normalized_sequence_name(index: usize, source: &Path, prefix: &str) -> Result<String> {
    let extension = source
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .filter(|value| IMAGE_EXTENSIONS.contains(&value.as_str()))
        .ok_or_else(|| SplatError::InvalidPath(source.to_path_buf()))?;
    Ok(format!("{prefix}{:06}.{extension}", index + 1))
}

pub fn prepare_image_sequence(
    source_dir: &Path,
    frames_dir: &Path,
    colmap_frames_dir: &Path,
    masks_dir: &Path,
) -> Result<PreparedImageSequence> {
    let scan = scan_image_sequence(source_dir, None)?;
    prepare_scanned_image_sequence(
        scan,
        frames_dir,
        colmap_frames_dir,
        masks_dir,
        ImageSequenceNaming::Primary,
        None,
        None,
    )
}

pub fn prepare_reshoot_image_sequence(
    source_dir: &Path,
    frames_dir: &Path,
    colmap_frames_dir: &Path,
    masks_dir: &Path,
) -> Result<PreparedImageSequence> {
    let scan = scan_image_sequence(source_dir, None)?;
    prepare_scanned_image_sequence(
        scan,
        frames_dir,
        colmap_frames_dir,
        masks_dir,
        ImageSequenceNaming::Reshoot,
        None,
        None,
    )
}

pub fn prepare_scanned_image_sequence(
    scan: ImageSequenceScan,
    frames_dir: &Path,
    colmap_frames_dir: &Path,
    masks_dir: &Path,
    naming: ImageSequenceNaming,
    observer: Option<ImagePreparationObserver>,
    cancellation: Option<&CancellationToken>,
) -> Result<PreparedImageSequence> {
    prepare_scanned_image_sequence_with_preparation(
        scan,
        frames_dir,
        colmap_frames_dir,
        masks_dir,
        naming,
        observer,
        cancellation,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn prepare_scanned_image_sequence_with_preparation(
    scan: ImageSequenceScan,
    frames_dir: &Path,
    colmap_frames_dir: &Path,
    masks_dir: &Path,
    naming: ImageSequenceNaming,
    observer: Option<ImagePreparationObserver>,
    cancellation: Option<&CancellationToken>,
    ordinary_preparation: Option<u32>,
) -> Result<PreparedImageSequence> {
    let derived_sfm = match ordinary_preparation {
        None => false,
        Some(crate::pipeline::inputs::ORDINARY_IMAGE_PREPARATION_VERSION) => true,
        Some(_) => {
            return Err(SplatError::Process(
                "Unsupported source image preparation policy".into(),
            ))
        }
    };
    let derivative_cancellation = cancellation.cloned().unwrap_or_default();
    let info = &scan.info;
    std::fs::create_dir_all(frames_dir)?;
    std::fs::create_dir_all(colmap_frames_dir)?;
    if info.has_alpha {
        std::fs::create_dir_all(masks_dir)?;
    }

    let mut reporter = ProgressReporter::new(observer);
    let total_images = scan.images.len() as u64;
    let alpha_images = scan
        .images
        .iter()
        .filter(|image| image.has_alpha_channel)
        .count() as u64;
    let link_end = if alpha_images == 0 { 0.9 } else { 0.35 };
    let prefix = match naming {
        ImageSequenceNaming::Primary => "frame_",
        ImageSequenceNaming::Reshoot => "reshoot_",
    };

    let mut names = Vec::with_capacity(scan.images.len());
    for (index, image) in scan.images.iter().enumerate() {
        ensure_not_cancelled(cancellation)?;
        let name = normalized_sequence_name(index, &image.path, prefix)?;
        link_or_copy(&image.path, &frames_dir.join(&name))?;
        if !derived_sfm || !image.has_alpha_channel {
            link_or_copy(&image.path, &colmap_frames_dir.join(&name))?;
        }
        names.push(name);
        reporter.report(ImagePreparationProgress {
            phase: ImagePreparationPhase::LinkingFrames,
            current: index as u64 + 1,
            total: total_images,
            stage_progress: link_end * (index as f32 + 1.0) / total_images as f32,
        });
    }

    let mut has_transparency = false;
    if alpha_images > 0 {
        let mut completed = 0_u64;
        for (index, image) in scan.images.iter().enumerate() {
            if !image.has_alpha_channel {
                continue;
            }
            ensure_not_cancelled(cancellation)?;
            let decoded = decode_image(&image.path)?;
            let transparent = has_native_transparency(&decoded);
            let mask = masks_dir.join(format!("{}.png", names[index]));
            if derived_sfm {
                write_ordinary_sfm_derivatives_from_source(
                    Some(&image.path),
                    &decoded,
                    &colmap_frames_dir.join(&names[index]),
                    &mask,
                    &derivative_cancellation,
                )?;
            } else {
                write_alpha_mask(&decoded, &mask)?;
            }
            has_transparency |= transparent;
            completed += 1;
            reporter.report(ImagePreparationProgress {
                phase: ImagePreparationPhase::InspectingAlpha,
                current: completed,
                total: alpha_images,
                stage_progress: 0.35 + 0.5 * completed as f32 / alpha_images as f32,
            });
        }

        if has_transparency {
            let opaque_images = total_images - alpha_images;
            if opaque_images > 0 {
                let opaque_mask = GrayImage::from_pixel(info.width, info.height, Luma([255]));
                let mut completed = 0_u64;
                for (index, image) in scan.images.iter().enumerate() {
                    if image.has_alpha_channel {
                        continue;
                    }
                    ensure_not_cancelled(cancellation)?;
                    save_mask(
                        &opaque_mask,
                        &masks_dir.join(format!("{}.png", names[index])),
                    )?;
                    completed += 1;
                    reporter.report(ImagePreparationProgress {
                        phase: ImagePreparationPhase::WritingOpaqueMasks,
                        current: completed,
                        total: opaque_images,
                        stage_progress: (0.85 + 0.1 * completed as f32 / opaque_images as f32)
                            .min(0.95),
                    });
                }
            }
        } else if !derived_sfm {
            for (index, image) in scan.images.iter().enumerate() {
                if image.has_alpha_channel {
                    ensure_not_cancelled(cancellation)?;
                    let mask = masks_dir.join(format!("{}.png", names[index]));
                    if mask.exists() {
                        std::fs::remove_file(mask)?;
                    }
                }
            }
        }
    }

    ensure_not_cancelled(cancellation)?;
    reporter.report(ImagePreparationProgress {
        phase: ImagePreparationPhase::Validating,
        current: 0,
        total: total_images,
        stage_progress: 0.95,
    });
    let prepared = validate_image_sequence_with_preparation(
        frames_dir,
        colmap_frames_dir,
        masks_dir,
        info.image_count,
        has_transparency,
        naming,
        cancellation,
        ordinary_preparation,
    )?;
    reporter.report(ImagePreparationProgress {
        phase: ImagePreparationPhase::Validating,
        current: total_images,
        total: total_images,
        stage_progress: 1.0,
    });
    Ok(prepared)
}

pub fn validate_reshoot_image_sequence(
    frames_dir: &Path,
    colmap_frames_dir: &Path,
    masks_dir: &Path,
    expected_count: u64,
    has_alpha: bool,
) -> Result<PreparedImageSequence> {
    validate_named_sequence_with_cancellation(
        frames_dir,
        colmap_frames_dir,
        masks_dir,
        expected_count,
        has_alpha,
        "reshoot_",
        None,
    )
}

pub fn validate_prepared_image_sequence(
    frames_dir: &Path,
    colmap_frames_dir: &Path,
    masks_dir: &Path,
    expected_count: u64,
    has_alpha: bool,
) -> Result<PreparedImageSequence> {
    validate_named_sequence_with_cancellation(
        frames_dir,
        colmap_frames_dir,
        masks_dir,
        expected_count,
        has_alpha,
        "frame_",
        None,
    )
}

/// Validate the complete frozen input set, including all historical reshoot
/// batches. Unlike the prefix-based preparation helpers, no image is ignored.
pub fn validate_prepared_image_names(
    frames_dir: &Path,
    colmap_frames_dir: &Path,
    masks_dir: &Path,
    expected_names: &[String],
    has_alpha: bool,
) -> Result<PreparedImageSequence> {
    let mut expected = BTreeSet::new();
    let mut portable_names = BTreeSet::new();
    for name in expected_names {
        let path = Path::new(name);
        if name.is_empty()
            || name.contains(['/', '\\', ':', '\0', '\n', '\r'])
            || name.chars().any(char::is_control)
            || path.file_name().and_then(|value| value.to_str()) != Some(name.as_str())
            || !is_image_file(path)
            || !expected.insert(name.as_str())
            || !portable_names.insert(name.to_ascii_lowercase())
        {
            return Err(SplatError::Process(
                "Prepared image names are unsafe or duplicated".into(),
            ));
        }
    }
    if expected.is_empty() {
        return Err(SplatError::Process(
            "Prepared image names must not be empty".into(),
        ));
    }

    let read_exact_images = |directory: &Path| -> Result<Vec<PathBuf>> {
        let mut images = Vec::new();
        let mut actual = BTreeSet::new();
        for entry in std::fs::read_dir(directory)? {
            let path = entry?.path();
            if path.is_file() && is_image_file(&path) {
                let name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or_else(|| {
                        SplatError::Process("Prepared image name cannot be read".into())
                    })?;
                actual.insert(name.to_owned());
                images.push(path);
            }
        }
        if actual.iter().map(String::as_str).collect::<BTreeSet<_>>() != expected {
            return Err(SplatError::Process(
                "Prepared image set does not match the saved image names".into(),
            ));
        }
        images.sort_by(|left, right| natural_cmp(&file_name(left), &file_name(right)));
        Ok(images)
    };
    let frames = read_exact_images(frames_dir)?;
    let colmap_frames = read_exact_images(colmap_frames_dir)?;
    validate_selected_sequence(
        &frames,
        &colmap_frames,
        masks_dir,
        expected_names.len() as u64,
        has_alpha,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn validate_image_sequence_with_preparation(
    frames_dir: &Path,
    colmap_frames_dir: &Path,
    masks_dir: &Path,
    expected_count: u64,
    has_alpha: bool,
    naming: ImageSequenceNaming,
    cancellation: Option<&CancellationToken>,
    ordinary_preparation: Option<u32>,
) -> Result<PreparedImageSequence> {
    let prefix = match naming {
        ImageSequenceNaming::Primary => "frame_",
        ImageSequenceNaming::Reshoot => "reshoot_",
    };
    if ordinary_preparation.is_none() {
        return validate_named_sequence_with_cancellation(
            frames_dir,
            colmap_frames_dir,
            masks_dir,
            expected_count,
            has_alpha,
            prefix,
            cancellation,
        );
    }
    if ordinary_preparation != Some(crate::pipeline::inputs::ORDINARY_IMAGE_PREPARATION_VERSION) {
        return Err(SplatError::Process(
            "Unsupported source image preparation policy".into(),
        ));
    }
    let named_images = |directory: &Path| -> Result<Vec<PathBuf>> {
        Ok(list_images(directory)?
            .into_iter()
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(prefix))
            })
            .collect())
    };
    let frames = named_images(frames_dir)?;
    let colmap_frames = named_images(colmap_frames_dir)?;
    if expected_count == 0
        || frames.len() as u64 != expected_count
        || colmap_frames.len() != frames.len()
    {
        return Err(SplatError::Process(
            "Prepared image set does not match the saved image names".into(),
        ));
    }
    let cancel = cancellation.cloned().unwrap_or_default();
    let mut mask_count = 0;
    for (frame, colmap_frame) in frames.iter().zip(&colmap_frames) {
        ensure_not_cancelled(Some(&cancel))?;
        if frame.file_name() != colmap_frame.file_name()
            || std::fs::metadata(frame)?.len() == 0
            || std::fs::metadata(colmap_frame)?.len() == 0
        {
            return Err(SplatError::Process(
                "Prepared image set does not match the saved image names".into(),
            ));
        }
        let name = frame
            .file_name()
            .ok_or_else(|| SplatError::InvalidPath(frame.clone()))?;
        let mask = masks_dir.join(format!("{}.png", name.to_string_lossy()));
        validate_ordinary_sfm_derivatives(frame, colmap_frame, &mask, &cancel)?;
        match std::fs::metadata(&mask) {
            Ok(metadata) if metadata.is_file() && metadata.len() > 0 => mask_count += 1,
            Ok(_) => return Err(SplatError::InvalidPath(mask)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && !has_alpha => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(PreparedImageSequence {
        image_count: frames.len() as u64,
        mask_count,
        has_alpha,
    })
}

fn validate_named_sequence_with_cancellation(
    frames_dir: &Path,
    colmap_frames_dir: &Path,
    masks_dir: &Path,
    expected_count: u64,
    has_alpha: bool,
    prefix: &str,
    cancellation: Option<&CancellationToken>,
) -> Result<PreparedImageSequence> {
    let frames = list_images(frames_dir)?
        .into_iter()
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(prefix))
        })
        .collect::<Vec<_>>();
    let colmap_frames = list_images(colmap_frames_dir)?
        .into_iter()
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(prefix))
        })
        .collect::<Vec<_>>();
    validate_selected_sequence(
        &frames,
        &colmap_frames,
        masks_dir,
        expected_count,
        has_alpha,
        cancellation,
    )
}

fn validate_selected_sequence(
    frames: &[PathBuf],
    colmap_frames: &[PathBuf],
    masks_dir: &Path,
    expected_count: u64,
    has_alpha: bool,
    cancellation: Option<&CancellationToken>,
) -> Result<PreparedImageSequence> {
    let mut invalid_frame = false;
    for path in frames {
        ensure_not_cancelled(cancellation)?;
        invalid_frame |= std::fs::metadata(path).map_or(true, |metadata| metadata.len() == 0);
    }
    if frames.len() as u64 != expected_count || invalid_frame {
        return Err(SplatError::Process(format!(
            "图片序列检查点不完整：预期 {expected_count} 张，实际 {} 张",
            frames.len()
        )));
    }

    if colmap_frames.len() != frames.len() {
        return Err(SplatError::Process(format!(
            "COLMAP 图片检查点不完整：预期 {} 张，实际 {} 张",
            frames.len(),
            colmap_frames.len()
        )));
    }
    for (frame, colmap_frame) in frames.iter().zip(colmap_frames) {
        ensure_not_cancelled(cancellation)?;
        if frame.file_name() != colmap_frame.file_name()
            || std::fs::metadata(colmap_frame).map_or(true, |metadata| metadata.len() == 0)
        {
            return Err(SplatError::Process(format!(
                "COLMAP 图片与原始准备帧不一致：{}",
                colmap_frame.display()
            )));
        }
        validate_colmap_frame_copy(frame, colmap_frame)?;
    }

    let mask_count = if has_alpha {
        for frame in frames {
            ensure_not_cancelled(cancellation)?;
            let name = frame
                .file_name()
                .and_then(|value| value.to_str())
                .ok_or_else(|| SplatError::Process("图片序列包含无法读取的文件名".into()))?;
            let mask = masks_dir.join(format!("{name}.png"));
            if !mask.is_file() || std::fs::metadata(&mask)?.len() == 0 {
                return Err(SplatError::Process(format!(
                    "COLMAP Mask 缺失：{}",
                    mask.display()
                )));
            }
            validate_binary_mask(&mask, read_image_header(frame)?.0)?;
        }
        frames.len() as u64
    } else {
        0
    };

    Ok(PreparedImageSequence {
        image_count: frames.len() as u64,
        mask_count,
        has_alpha,
    })
}

fn decode_image(path: &Path) -> Result<DynamicImage> {
    ImageReader::open(path)
        .map_err(|error| SplatError::Process(format!("无法读取图片 {}：{error}", path.display())))?
        .with_guessed_format()
        .map_err(|error| {
            SplatError::Process(format!("无法识别图片格式 {}：{error}", path.display()))
        })?
        .decode()
        .map_err(|error| SplatError::Process(format!("图片解码失败 {}：{error}", path.display())))
}

fn read_image_header(path: &Path) -> Result<((u32, u32), bool)> {
    // Constructing image's JPEG decoder reads the whole file even when only
    // dimensions are requested. Share the bounded preflight reader instead.
    let header = super::image_projection::inspect(path).map_err(|error| {
        SplatError::Process(format!("无法读取图片头 {}：{error}", path.display()))
    })?;
    // Projection inspection reports EXIF-oriented dimensions. These callers
    // compare the encoded pixels and masks; do not rotate their coordinate map.
    let dimensions = if (5..=8).contains(&header.orientation) {
        (header.height, header.width)
    } else {
        (header.width, header.height)
    };
    Ok((dimensions, header.alpha))
}

fn write_alpha_mask(image: &DynamicImage, destination: &Path) -> Result<()> {
    let mask = match image {
        DynamicImage::ImageLumaA8(value) => {
            GrayImage::from_fn(value.width(), value.height(), |x, y| {
                Luma([if value.get_pixel(x, y)[1] >= 128 {
                    255
                } else {
                    0
                }])
            })
        }
        DynamicImage::ImageRgba8(value) => {
            GrayImage::from_fn(value.width(), value.height(), |x, y| {
                Luma([if value.get_pixel(x, y)[3] >= 128 {
                    255
                } else {
                    0
                }])
            })
        }
        DynamicImage::ImageLumaA16(value) => {
            GrayImage::from_fn(value.width(), value.height(), |x, y| {
                Luma([if value.get_pixel(x, y)[1] >= 32_768 {
                    255
                } else {
                    0
                }])
            })
        }
        DynamicImage::ImageRgba16(value) => {
            GrayImage::from_fn(value.width(), value.height(), |x, y| {
                Luma([if value.get_pixel(x, y)[3] >= 32_768 {
                    255
                } else {
                    0
                }])
            })
        }
        _ => {
            let rgba = image.to_rgba8();
            GrayImage::from_fn(rgba.width(), rgba.height(), |x, y| {
                Luma([if rgba.get_pixel(x, y)[3] >= 128 {
                    255
                } else {
                    0
                }])
            })
        }
    };
    save_mask(&mask, destination)?;
    Ok(())
}

pub(crate) fn has_native_transparency(image: &DynamicImage) -> bool {
    match image {
        DynamicImage::ImageLumaA16(value) => value.pixels().any(|pixel| pixel[1] < u16::MAX),
        DynamicImage::ImageRgba16(value) => value.pixels().any(|pixel| pixel[3] < u16::MAX),
        DynamicImage::ImageLumaA8(value) => value.pixels().any(|pixel| pixel[1] < u8::MAX),
        DynamicImage::ImageRgba8(value) => value.pixels().any(|pixel| pixel[3] < u8::MAX),
        _ => false,
    }
}

/// Separate SfM pixels from training pixels. This reproduces the original
/// ordinary-photo recipe without applying Alpha twice to Brush's RGBA input.
pub(crate) fn write_ordinary_sfm_derivatives(
    decoded: &DynamicImage,
    colmap_frame: &Path,
    mask: &Path,
    cancel: &CancellationToken,
) -> Result<()> {
    write_ordinary_sfm_derivatives_with_metadata(decoded, colmap_frame, mask, cancel, None)
}

/// Retain camera metadata only when this PNG still uses the original pixel
/// coordinates. Training frames and Alpha are unchanged and remain separate.
pub(crate) fn write_ordinary_sfm_derivatives_from_source(
    source: Option<&Path>,
    decoded: &DynamicImage,
    colmap_frame: &Path,
    mask: &Path,
    cancel: &CancellationToken,
) -> Result<()> {
    let metadata = source
        .map(|source| {
            super::png_camera_metadata::PngCameraMetadata::read(
                source,
                decoded.dimensions(),
                cancel,
            )
        })
        .transpose()?
        .flatten();
    if metadata.is_none() {
        return write_ordinary_sfm_derivatives(decoded, colmap_frame, mask, cancel);
    }
    write_ordinary_sfm_derivatives_with_metadata(decoded, colmap_frame, mask, cancel, metadata)
}

fn write_ordinary_sfm_derivatives_with_metadata(
    decoded: &DynamicImage,
    colmap_frame: &Path,
    mask: &Path,
    cancel: &CancellationToken,
    metadata: Option<super::png_camera_metadata::PngCameraMetadata>,
) -> Result<()> {
    let (rgb, alpha) = ordinary_sfm_pixels(decoded, cancel)?;
    for destination in [colmap_frame, mask] {
        // A previous target can be a hard link to immutable source pixels.
        // Always unlink the destination before encoding derived bytes.
        match std::fs::remove_file(destination) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    ensure_not_cancelled(Some(cancel))?;
    if let Some(metadata) = metadata {
        metadata.encode(&rgb, colmap_frame, cancel)?;
    } else {
        rgb.save(colmap_frame).map_err(derivative_image_error)?;
    }
    ensure_not_cancelled(Some(cancel))?;
    save_mask(&alpha, mask)?;
    ensure_not_cancelled(Some(cancel))
}

fn derivative_image_error(error: image::ImageError) -> SplatError {
    match error {
        image::ImageError::IoError(error) => SplatError::Io(error),
        error => SplatError::Process(error.to_string()),
    }
}

fn ordinary_sfm_pixels(
    image: &DynamicImage,
    cancel: &CancellationToken,
) -> Result<(DynamicImage, GrayImage)> {
    let rgb = match image {
        DynamicImage::ImageLumaA16(_) | DynamicImage::ImageRgba16(_) => {
            let rgba = image.to_rgba16();
            let mut rgb =
                image::ImageBuffer::<image::Rgb<u16>, Vec<u16>>::new(rgba.width(), rgba.height());
            for (y, row) in rgba.rows().enumerate() {
                ensure_not_cancelled(Some(cancel))?;
                for (x, pixel) in row.enumerate() {
                    let alpha = u64::from(pixel[3]);
                    rgb.put_pixel(
                        x as u32,
                        y as u32,
                        image::Rgb([
                            ((u64::from(pixel[0]) * alpha + 32_767) / 65_535) as u16,
                            ((u64::from(pixel[1]) * alpha + 32_767) / 65_535) as u16,
                            ((u64::from(pixel[2]) * alpha + 32_767) / 65_535) as u16,
                        ]),
                    );
                }
            }
            DynamicImage::ImageRgb16(rgb)
        }
        _ => {
            let rgba = image.to_rgba8();
            let mut rgb = image::RgbImage::new(rgba.width(), rgba.height());
            for (y, row) in rgba.rows().enumerate() {
                ensure_not_cancelled(Some(cancel))?;
                for (x, pixel) in row.enumerate() {
                    let alpha = u32::from(pixel[3]);
                    rgb.put_pixel(
                        x as u32,
                        y as u32,
                        image::Rgb([
                            ((u32::from(pixel[0]) * alpha + 127) / 255) as u8,
                            ((u32::from(pixel[1]) * alpha + 127) / 255) as u8,
                            ((u32::from(pixel[2]) * alpha + 127) / 255) as u8,
                        ]),
                    );
                }
            }
            DynamicImage::ImageRgb8(rgb)
        }
    };
    ensure_not_cancelled(Some(cancel))?;
    let rgba = image.to_rgba8();
    let alpha = GrayImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        Luma([rgba.get_pixel(x, y)[3]])
    });
    Ok((rgb, alpha))
}

/// Deep validation of a versioned ordinary-photo derivative. Legacy binary
/// masks and videos retain their existing validator, never this soft rule.
pub(crate) fn validate_ordinary_sfm_derivatives(
    frame: &Path,
    colmap_frame: &Path,
    mask: &Path,
    cancel: &CancellationToken,
) -> Result<()> {
    ensure_not_cancelled(Some(cancel))?;
    let header = super::image_projection::inspect(frame)?;
    if !header.alpha {
        validate_colmap_frame_copy(frame, colmap_frame)?;
        return match std::fs::metadata(mask) {
            Ok(_) => {
                validate_binary_mask(mask, read_image_header(frame)?.0)?;
                let alpha = ImageReader::open(mask)?
                    .decode()
                    .map_err(derivative_image_error)?
                    .into_luma8();
                if alpha.pixels().any(|pixel| pixel[0] != 255) {
                    return Err(SplatError::Process(format!(
                        "COLMAP 图片内容与原始准备帧不一致：{}",
                        mask.display()
                    )));
                }
                Ok(())
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        };
    }
    let decode = |path: &Path| -> Result<DynamicImage> {
        let mut reader = ImageReader::open(path)?.with_guessed_format()?;
        let mut limits = image::Limits::default();
        limits.max_alloc = Some(super::panorama::MAX_DECODE_BYTES);
        reader.limits(limits);
        reader.decode().map_err(derivative_image_error)
    };
    let decoded = decode(frame)?;
    let (expected_rgb, expected_alpha) = ordinary_sfm_pixels(&decoded, cancel)?;
    let rgb = decode(colmap_frame)?;
    let alpha = decode(mask)?;
    if rgb.color() != expected_rgb.color()
        || rgb.dimensions() != expected_rgb.dimensions()
        || rgb.into_bytes() != expected_rgb.into_bytes()
        || alpha.color() != ColorType::L8
        || alpha.dimensions() != expected_alpha.dimensions()
        || alpha.into_luma8() != expected_alpha
    {
        return Err(SplatError::Process(format!(
            "COLMAP 图片内容与原始准备帧不一致：{}",
            colmap_frame.display()
        )));
    }
    ensure_not_cancelled(Some(cancel))
}

pub(crate) fn validate_colmap_frame_copy(frame: &Path, colmap_frame: &Path) -> Result<()> {
    if read_image_header(frame)?.0 != read_image_header(colmap_frame)?.0 {
        return Err(SplatError::Process(format!(
            "COLMAP 图片尺寸与原始准备帧不一致：{}",
            colmap_frame.display()
        )));
    }
    if !files_are_identical(frame, colmap_frame)? {
        return Err(SplatError::Process(format!(
            "COLMAP 图片内容与原始准备帧不一致：{}",
            colmap_frame.display()
        )));
    }
    Ok(())
}

pub(crate) fn validate_binary_mask(mask: &Path, expected_dimensions: (u32, u32)) -> Result<()> {
    let decoded = ImageReader::open(mask)
        .map_err(|error| {
            SplatError::Process(format!("无法读取 COLMAP Mask {}：{error}", mask.display()))
        })?
        .with_guessed_format()
        .map_err(|error| {
            SplatError::Process(format!("无法识别 COLMAP Mask {}：{error}", mask.display()))
        })?
        .decode()
        .map_err(|error| {
            SplatError::Process(format!("无法解码 COLMAP Mask {}：{error}", mask.display()))
        })?;
    if decoded.color() != ColorType::L8 {
        return Err(SplatError::Process(format!(
            "COLMAP Mask 必须是 8 位灰度图：{}",
            mask.display()
        )));
    }
    if (decoded.width(), decoded.height()) != expected_dimensions {
        return Err(SplatError::Process(format!(
            "COLMAP Mask 尺寸与原始准备帧不一致：{}",
            mask.display()
        )));
    }
    if decoded
        .into_luma8()
        .into_raw()
        .into_iter()
        .any(|value| value != 0 && value != 255)
    {
        return Err(SplatError::Process(format!(
            "COLMAP Mask 包含 0 和 255 以外的像素：{}",
            mask.display()
        )));
    }
    Ok(())
}

fn files_are_identical(left: &Path, right: &Path) -> Result<bool> {
    if std::fs::metadata(left)?.len() != std::fs::metadata(right)?.len() {
        return Ok(false);
    }
    let mut left = BufReader::new(File::open(left)?);
    let mut right = BufReader::new(File::open(right)?);
    let mut left_buffer = [0_u8; 64 * 1024];
    let mut right_buffer = [0_u8; 64 * 1024];
    loop {
        let left_count = left.read(&mut left_buffer)?;
        let right_count = right.read(&mut right_buffer)?;
        if left_count != right_count || left_buffer[..left_count] != right_buffer[..right_count] {
            return Ok(false);
        }
        if left_count == 0 {
            return Ok(true);
        }
    }
}

fn save_mask(mask: &GrayImage, destination: &Path) -> Result<()> {
    mask.save(destination).map_err(|error| {
        SplatError::Process(format!(
            "无法写入 COLMAP Mask {}：{error}",
            destination.display()
        ))
    })
}

fn link_or_copy(source: &Path, destination: &Path) -> Result<()> {
    link_or_copy_with(source, destination, |source, destination| {
        std::fs::hard_link(source, destination)
    })
}

fn link_or_copy_with(
    source: &Path,
    destination: &Path,
    hard_link: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
) -> Result<()> {
    if destination.exists() {
        std::fs::remove_file(destination)?;
    }
    if hard_link(source, destination).is_err() {
        std::fs::copy(source, destination)?;
    }
    Ok(())
}

fn ensure_not_cancelled(cancellation: Option<&CancellationToken>) -> Result<()> {
    if cancellation.is_some_and(CancellationToken::is_cancelled) {
        return Err(SplatError::Cancelled);
    }
    Ok(())
}

struct ProgressReporter {
    observer: Option<ImagePreparationObserver>,
    last_phase: Option<ImagePreparationPhase>,
    last_percent: i32,
}

impl ProgressReporter {
    fn new(observer: Option<ImagePreparationObserver>) -> Self {
        Self {
            observer,
            last_phase: None,
            last_percent: -1,
        }
    }

    fn report(&mut self, progress: ImagePreparationProgress) {
        let percent = (progress.stage_progress.clamp(0.0, 1.0) * 100.0).floor() as i32;
        if self.last_phase == Some(progress.phase)
            && self.last_percent == percent
            && progress.current < progress.total
        {
            return;
        }
        self.last_phase = Some(progress.phase);
        self.last_percent = percent;
        if let Some(observer) = &self.observer {
            observer(progress);
        }
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|value| value.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

pub(crate) fn natural_cmp(left: &str, right: &str) -> Ordering {
    let left = left.as_bytes();
    let right = right.as_bytes();
    let (mut a, mut b) = (0, 0);
    while a < left.len() && b < right.len() {
        if left[a].is_ascii_digit() && right[b].is_ascii_digit() {
            let (a_start, b_start) = (a, b);
            while a < left.len() && left[a].is_ascii_digit() {
                a += 1;
            }
            while b < right.len() && right[b].is_ascii_digit() {
                b += 1;
            }
            let a_number = left[a_start..a].iter().fold(0_u128, |value, digit| {
                value
                    .saturating_mul(10)
                    .saturating_add((digit - b'0') as u128)
            });
            let b_number = right[b_start..b].iter().fold(0_u128, |value, digit| {
                value
                    .saturating_mul(10)
                    .saturating_add((digit - b'0') as u128)
            });
            match a_number.cmp(&b_number) {
                Ordering::Equal => continue,
                ordering => return ordering,
            }
        }
        match left[a].cmp(&right[b]) {
            Ordering::Equal => {
                a += 1;
                b += 1;
            }
            ordering => return ordering,
        }
    }
    left.len().cmp(&right.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_sfm_recipe_preserves_training_rgba_and_soft_alpha_at_both_bit_depths() {
        for depth in [8, 16] {
            let root = tempfile::tempdir().unwrap();
            let frame = root.path().join("frame.png");
            let rgb = root.path().join("sfm.png");
            let mask = root.path().join("mask.png");
            let image = if depth == 16 {
                let alpha = [0, 1, 16_384, 32_767, 32_768, 65_534, 65_535];
                DynamicImage::ImageRgba16(image::ImageBuffer::from_fn(7, 2, |x, _| {
                    image::Rgba([60_001, 32_769, 101, alpha[x as usize]])
                }))
            } else {
                let alpha = [0, 1, 64, 127, 128, 254, 255];
                DynamicImage::ImageRgba8(image::RgbaImage::from_fn(7, 2, |x, _| {
                    image::Rgba([201, 129, 17, alpha[x as usize]])
                }))
            };
            image.save(&frame).unwrap();
            let original = std::fs::read(&frame).unwrap();
            std::fs::hard_link(&frame, &rgb).unwrap();
            let cancel = CancellationToken::new();
            write_ordinary_sfm_derivatives(&image, &rgb, &mask, &cancel).unwrap();
            assert_eq!(std::fs::read(&frame).unwrap(), original);
            validate_ordinary_sfm_derivatives(&frame, &rgb, &mask, &cancel).unwrap();
            assert_eq!(
                ImageReader::open(&rgb).unwrap().decode().unwrap().color(),
                if depth == 16 {
                    ColorType::Rgb16
                } else {
                    ColorType::Rgb8
                }
            );
            let alpha = image::open(&mask).unwrap().into_luma8();
            assert_eq!(alpha.get_pixel(3, 0)[0], 127);
            assert!(validate_binary_mask(&mask, (7, 2)).is_err());
            if depth == 16 {
                let pixels = image::open(&rgb).unwrap().into_rgb16();
                assert_eq!(
                    pixels.get_pixel(4, 0)[0],
                    ((60_001_u64 * 32_768 + 32_767) / 65_535) as u16
                );
                assert_eq!(pixels.get_pixel(6, 0)[0], 60_001);
            } else {
                let pixels = image::open(&rgb).unwrap().into_rgb8();
                assert_eq!(
                    pixels.get_pixel(4, 0)[0],
                    ((201_u32 * 128 + 127) / 255) as u8
                );
                assert_eq!(pixels.get_pixel(6, 0)[0], 201);
            }
            let mut broken = alpha;
            broken.put_pixel(3, 0, Luma([255]));
            broken.save(&mask).unwrap();
            assert!(validate_ordinary_sfm_derivatives(&frame, &rgb, &mask, &cancel).is_err());
            assert_eq!(std::fs::read(&frame).unwrap(), original);
        }
    }

    #[test]
    fn cancelled_ordinary_sfm_preparation_does_not_overwrite_linked_originals() {
        let root = tempfile::tempdir().unwrap();
        let frame = root.path().join("frame.png");
        let rgb = root.path().join("sfm.png");
        let mask = root.path().join("mask.png");
        let image = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            2,
            2,
            image::Rgba([201, 32, 10, 1]),
        ));
        image.save(&frame).unwrap();
        std::fs::hard_link(&frame, &rgb).unwrap();
        let before = std::fs::read(&frame).unwrap();
        let token = CancellationToken::new();
        token.cancel();
        assert!(matches!(
            write_ordinary_sfm_derivatives(&image, &rgb, &mask, &token),
            Err(SplatError::Cancelled)
        ));
        assert_eq!(std::fs::read(&frame).unwrap(), before);
        assert_eq!(std::fs::read(&rgb).unwrap(), before);
        assert!(!mask.exists());
    }

    #[test]
    #[ignore = "requires explicit source/reference folders; reads two real photos and writes only its temporary derivatives"]
    fn real_ordinary_sfm_derivatives_match_historical_success_pixels() {
        let source = PathBuf::from(
            std::env::var_os("OOOSPLAT_ORDINARY_SFM_COMPARE_SOURCE").expect("source folder"),
        );
        let reference = PathBuf::from(
            std::env::var_os("OOOSPLAT_ORDINARY_SFM_COMPARE_REFERENCE").expect("reference project"),
        );
        let files = list_images(&source).unwrap();
        let historical = list_images(&reference.join("work/colmap_frames")).unwrap();
        assert_eq!(files.len(), historical.len());
        for index in [0, files.len() - 1] {
            let root = tempfile::tempdir().unwrap();
            let image = image::open(&files[index]).unwrap();
            let original = std::fs::read(&files[index]).unwrap();
            let rgb = root.path().join("sfm.png");
            let mask = root.path().join("mask.png");
            let cancel = CancellationToken::new();
            write_ordinary_sfm_derivatives(&image, &rgb, &mask, &cancel).unwrap();
            let output = image::open(&rgb).unwrap();
            let expected = image::open(&historical[index]).unwrap();
            assert_eq!(output.color(), expected.color());
            assert_eq!(output.dimensions(), expected.dimensions());
            assert_eq!(output.into_bytes(), expected.into_bytes());
            let historical_mask = reference.join("work/masks").join(format!(
                "{}.png",
                historical[index].file_name().unwrap().to_string_lossy()
            ));
            assert_eq!(
                image::open(&mask).unwrap().into_luma8(),
                image::open(&historical_mask).unwrap().into_luma8()
            );
            assert_eq!(std::fs::read(&files[index]).unwrap(), original);
            eprintln!("ordinary SfM real source/reference pixels match at index={index}");
        }
    }
    use image::{ColorType, ImageBuffer, ImageDecoder, Rgba, RgbaImage};

    fn write_rgba(path: &Path, alpha: u8) {
        RgbaImage::from_pixel(2, 2, Rgba([10, 20, 30, alpha]))
            .save(path)
            .unwrap();
    }

    #[test]
    fn accepts_only_first_phase_formats_and_naturally_sorts() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["frame10.jpg", "frame2.png", "frame1.jpeg", "skip.webp"] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        let names = list_images(dir.path())
            .unwrap()
            .into_iter()
            .map(|path| file_name(&path))
            .collect::<Vec<_>>();
        assert_eq!(names, ["frame1.jpeg", "frame2.png", "frame10.jpg"]);
    }

    #[test]
    fn detects_alpha_channels_from_headers_and_rejects_mismatched_sizes() {
        let dir = tempfile::tempdir().unwrap();
        write_rgba(&dir.path().join("1.png"), 255);
        write_rgba(&dir.path().join("2.png"), 64);
        let info = analyze_image_sequence(dir.path()).unwrap();
        assert_eq!(info.image_count, 2);
        assert!(info.has_alpha);

        RgbaImage::new(3, 2).save(dir.path().join("2.png")).unwrap();
        assert!(analyze_image_sequence(dir.path()).is_err());
    }

    #[test]
    fn jpeg_dimensions_use_the_bounded_metadata_reader() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("image.jpg");
        image::RgbImage::new(5, 3).save(&path).unwrap();
        let before = super::super::image_projection::header_reads_for_test(dir.path());

        assert_eq!(read_image_header(&path).unwrap(), ((5, 3), false));
        assert_eq!(
            super::super::image_projection::header_reads_for_test(dir.path()) - before,
            1,
        );
    }

    fn exif_orientation(orientation: u16) -> Vec<u8> {
        let mut data = b"II*\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0".to_vec();
        data.extend(orientation.to_le_bytes());
        data.extend([0; 6]);
        data
    }

    fn png_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut chunk = (data.len() as u32).to_be_bytes().to_vec();
        chunk.extend(kind);
        chunk.extend(data);
        let crc = crc32fast::hash(&chunk[4..]);
        chunk.extend(crc.to_be_bytes());
        chunk
    }

    #[test]
    fn lightweight_dimensions_preserve_encoded_pixels_for_exif_rotations() {
        let dir = tempfile::tempdir().unwrap();
        for orientation in 1..=8 {
            let tiff = exif_orientation(orientation);
            for extension in ["jpg", "png"] {
                let path = dir.path().join(format!("{orientation}.{extension}"));
                image::RgbImage::new(5, 3).save(&path).unwrap();
                let mut encoded = std::fs::read(&path).unwrap();
                if extension == "jpg" {
                    let mut data = b"Exif\0\0".to_vec();
                    data.extend(&tiff);
                    let mut segment = vec![0xff, 0xe1];
                    segment.extend(((data.len() + 2) as u16).to_be_bytes());
                    segment.extend(data);
                    encoded.splice(2..2, segment);
                } else {
                    let end = encoded.len() - 12;
                    encoded.splice(end..end, png_chunk(b"eXIf", &tiff));
                }
                std::fs::write(&path, encoded).unwrap();
                let inspected = super::super::image_projection::inspect(&path).unwrap();
                assert_eq!(inspected.orientation, orientation as u8);
                assert_eq!(read_image_header(&path).unwrap(), ((5, 3), false));
            }
        }
    }

    #[test]
    fn lightweight_header_keeps_palette_transparency() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("palette.png");
        let mut data = b"\x89PNG\r\n\x1a\n".to_vec();
        data.extend(png_chunk(b"IHDR", b"\0\0\0\x02\0\0\0\x01\x08\x03\0\0\0"));
        data.extend(png_chunk(b"PLTE", &[12, 34, 56, 78, 90, 12]));
        data.extend(png_chunk(b"tRNS", &[0, 255]));
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), Default::default());
        encoder.write_all(&[0, 0, 1]).unwrap();
        data.extend(png_chunk(b"IDAT", &encoder.finish().unwrap()));
        data.extend(png_chunk(b"IEND", &[]));
        std::fs::write(&path, data).unwrap();

        assert!(image::open(&path).unwrap().color().has_alpha());
        assert_eq!(read_image_header(&path).unwrap(), ((2, 1), true));
    }

    #[test]
    fn lightweight_header_errors_keep_the_input_filename() {
        let dir = tempfile::tempdir().unwrap();
        for (name, bytes) in [
            ("invalid.jpg", &b"not an image"[..]),
            ("truncated.png", &b"\x89PNG\r\n\x1a\n"[..]),
        ] {
            let path = dir.path().join(name);
            std::fs::write(&path, bytes).unwrap();
            assert!(read_image_header(&path)
                .unwrap_err()
                .to_string()
                .contains(name));
        }
    }

    #[test]
    fn lightweight_dimensions_do_not_replace_full_copy_validation() {
        let dir = tempfile::tempdir().unwrap();
        let frame = dir.path().join("frame.jpg");
        let counterpart = dir.path().join("colmap.jpg");
        image::RgbImage::new(5, 3).save(&frame).unwrap();
        std::fs::copy(&frame, &counterpart).unwrap();
        validate_colmap_frame_copy(&frame, &counterpart).unwrap();
        let mut bytes = std::fs::read(&counterpart).unwrap();
        *bytes.last_mut().unwrap() ^= 1;
        std::fs::write(&counterpart, bytes).unwrap();

        assert_eq!(
            read_image_header(&frame).unwrap(),
            read_image_header(&counterpart).unwrap()
        );
        assert!(validate_colmap_frame_copy(&frame, &counterpart).is_err());
    }

    #[test]
    fn header_scan_does_not_decode_pixel_payloads() {
        let dir = tempfile::tempdir().unwrap();
        write_rgba(&dir.path().join("1.png"), 255);
        let corrupt = dir.path().join("2.png");
        write_rgba(&corrupt, 255);
        let mut bytes = std::fs::read(&corrupt).unwrap();
        let idat = bytes
            .windows(4)
            .position(|window| window == b"IDAT")
            .expect("test PNG contains IDAT");
        bytes[idat + 4] ^= 0xff;
        std::fs::write(&corrupt, bytes).unwrap();

        let info = analyze_image_sequence(dir.path()).unwrap();

        assert_eq!(info.image_count, 2);
        let frames = tempfile::tempdir().unwrap();
        let colmap_frames = tempfile::tempdir().unwrap();
        let masks = tempfile::tempdir().unwrap();
        let error = prepare_image_sequence(
            dir.path(),
            frames.path(),
            colmap_frames.path(),
            masks.path(),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("2.png"));
    }

    #[test]
    fn opaque_images_are_linked_without_decoding_pixel_payloads() {
        let source = tempfile::tempdir().unwrap();
        let frames = tempfile::tempdir().unwrap();
        let colmap_frames = tempfile::tempdir().unwrap();
        let masks = tempfile::tempdir().unwrap();
        image::RgbImage::new(2, 2)
            .save(source.path().join("1.png"))
            .unwrap();
        let corrupt = source.path().join("2.png");
        image::RgbImage::new(2, 2).save(&corrupt).unwrap();
        let mut bytes = std::fs::read(&corrupt).unwrap();
        let idat = bytes
            .windows(4)
            .position(|window| window == b"IDAT")
            .expect("test PNG contains IDAT");
        bytes[idat + 4] ^= 0xff;
        std::fs::write(&corrupt, bytes).unwrap();

        let scan = scan_image_sequence(source.path(), None).unwrap();
        assert!(!scan.info.has_alpha);
        let prepared = prepare_scanned_image_sequence(
            scan,
            frames.path(),
            colmap_frames.path(),
            masks.path(),
            ImageSequenceNaming::Primary,
            None,
            None,
        )
        .unwrap();

        assert!(!prepared.has_alpha);
        assert_eq!(prepared.mask_count, 0);
        assert!(frames.path().join("frame_000002.png").is_file());
    }

    #[test]
    fn versioned_sequence_preparation_preserves_source_and_writes_soft_derivatives() {
        let source = tempfile::tempdir().unwrap();
        let frames = tempfile::tempdir().unwrap();
        let colmap_frames = tempfile::tempdir().unwrap();
        let masks = tempfile::tempdir().unwrap();
        for name in ["1.png", "2.png"] {
            RgbaImage::from_pixel(2, 1, Rgba([200, 100, 50, 127]))
                .save(source.path().join(name))
                .unwrap();
        }
        let original = std::fs::read(source.path().join("1.png")).unwrap();
        // A stale linked SfM file must be unlinked before derivative encoding.
        std::fs::hard_link(
            source.path().join("1.png"),
            colmap_frames.path().join("frame_000001.png"),
        )
        .unwrap();
        let policy = Some(crate::pipeline::inputs::ORDINARY_IMAGE_PREPARATION_VERSION);
        let prepared = prepare_scanned_image_sequence_with_preparation(
            scan_image_sequence(source.path(), None).unwrap(),
            frames.path(),
            colmap_frames.path(),
            masks.path(),
            ImageSequenceNaming::Primary,
            None,
            None,
            policy,
        )
        .unwrap();
        assert_eq!(prepared.image_count, 2);
        assert_eq!(prepared.mask_count, 2);
        assert!(prepared.has_alpha);
        assert_eq!(
            std::fs::read(source.path().join("1.png")).unwrap(),
            original
        );
        assert_eq!(
            std::fs::read(frames.path().join("frame_000001.png")).unwrap(),
            original
        );
        let rgb = image::open(colmap_frames.path().join("frame_000001.png")).unwrap();
        assert_eq!(rgb.color(), ColorType::Rgb8);
        assert_eq!(rgb.to_rgb8().get_pixel(0, 0).0, [100, 50, 25]);
        let mask = masks.path().join("frame_000001.png.png");
        assert_eq!(
            image::open(&mask).unwrap().to_luma8().get_pixel(0, 0)[0],
            127
        );
        assert!(validate_prepared_image_sequence(
            frames.path(),
            colmap_frames.path(),
            masks.path(),
            2,
            true,
        )
        .is_err());
        GrayImage::from_pixel(2, 1, Luma([128])).save(mask).unwrap();
        assert!(validate_image_sequence_with_preparation(
            frames.path(),
            colmap_frames.path(),
            masks.path(),
            2,
            true,
            ImageSequenceNaming::Primary,
            None,
            policy,
        )
        .is_err());
    }

    #[test]
    fn versioned_opaque_alpha_keeps_mask_without_enabling_transparency() {
        let source = tempfile::tempdir().unwrap();
        let frames = tempfile::tempdir().unwrap();
        let colmap_frames = tempfile::tempdir().unwrap();
        let masks = tempfile::tempdir().unwrap();
        for name in ["1.png", "2.png"] {
            write_rgba(&source.path().join(name), 255);
        }
        let policy = Some(crate::pipeline::inputs::ORDINARY_IMAGE_PREPARATION_VERSION);
        let prepared = prepare_scanned_image_sequence_with_preparation(
            scan_image_sequence(source.path(), None).unwrap(),
            frames.path(),
            colmap_frames.path(),
            masks.path(),
            ImageSequenceNaming::Reshoot,
            None,
            None,
            policy,
        )
        .unwrap();
        assert!(!prepared.has_alpha);
        assert_eq!(prepared.mask_count, 2);
        assert_eq!(
            validate_image_sequence_with_preparation(
                frames.path(),
                colmap_frames.path(),
                masks.path(),
                2,
                false,
                ImageSequenceNaming::Reshoot,
                None,
                policy,
            )
            .unwrap(),
            prepared
        );
    }

    #[test]
    fn versioned_sequence_without_policy_retains_legacy_rgba_and_binary_masks() {
        let source = tempfile::tempdir().unwrap();
        let frames = tempfile::tempdir().unwrap();
        let colmap_frames = tempfile::tempdir().unwrap();
        let masks = tempfile::tempdir().unwrap();
        for name in ["1.png", "2.png"] {
            write_rgba(&source.path().join(name), 127);
        }
        let original = std::fs::read(source.path().join("1.png")).unwrap();
        let prepared = prepare_scanned_image_sequence_with_preparation(
            scan_image_sequence(source.path(), None).unwrap(),
            frames.path(),
            colmap_frames.path(),
            masks.path(),
            ImageSequenceNaming::Primary,
            None,
            None,
            None,
        )
        .unwrap();
        assert!(prepared.has_alpha);
        assert_eq!(
            std::fs::read(colmap_frames.path().join("frame_000001.png")).unwrap(),
            original
        );
        assert_eq!(
            image::open(masks.path().join("frame_000001.png.png"))
                .unwrap()
                .to_luma8()
                .get_pixel(0, 0)[0],
            0
        );
    }

    #[test]
    fn transparent_sequences_generate_matching_masks() {
        let source = tempfile::tempdir().unwrap();
        let frames = tempfile::tempdir().unwrap();
        let colmap_frames = tempfile::tempdir().unwrap();
        let masks = tempfile::tempdir().unwrap();
        write_rgba(&source.path().join("1.png"), 0);
        write_rgba(&source.path().join("2.png"), 255);
        let prepared = prepare_image_sequence(
            source.path(),
            frames.path(),
            colmap_frames.path(),
            masks.path(),
        )
        .unwrap();
        assert_eq!(prepared.image_count, 2);
        assert_eq!(prepared.mask_count, 2);
        assert_eq!(
            image::open(masks.path().join("frame_000001.png.png"))
                .unwrap()
                .to_luma8()
                .get_pixel(0, 0)[0],
            0
        );
        assert_eq!(
            image::open(masks.path().join("frame_000002.png.png"))
                .unwrap()
                .to_luma8()
                .get_pixel(0, 0)[0],
            255
        );
        for name in ["frame_000001.png", "frame_000002.png"] {
            assert_eq!(
                std::fs::read(frames.path().join(name)).unwrap(),
                std::fs::read(colmap_frames.path().join(name)).unwrap()
            );
        }
    }

    #[test]
    fn rgba8_masks_use_the_128_boundary_without_rewriting_colmap_images() {
        let source = tempfile::tempdir().unwrap();
        let frames = tempfile::tempdir().unwrap();
        let colmap_frames = tempfile::tempdir().unwrap();
        let masks = tempfile::tempdir().unwrap();
        let pixels = [
            Rgba([255, 128, 1, 0]),
            Rgba([255, 128, 1, 127]),
            Rgba([255, 128, 1, 128]),
            Rgba([255, 128, 1, 255]),
        ];
        for name in ["1.png", "2.png"] {
            let image = RgbaImage::from_fn(4, 1, |x, _| pixels[x as usize]);
            image.save(source.path().join(name)).unwrap();
        }
        let original = std::fs::read(source.path().join("1.png")).unwrap();

        let prepared = prepare_image_sequence(
            source.path(),
            frames.path(),
            colmap_frames.path(),
            masks.path(),
        )
        .unwrap();

        assert!(prepared.has_alpha);
        assert_eq!(
            std::fs::read(frames.path().join("frame_000001.png")).unwrap(),
            original
        );
        assert_eq!(
            std::fs::read(colmap_frames.path().join("frame_000001.png")).unwrap(),
            original
        );
        assert_eq!(
            image::open(masks.path().join("frame_000001.png.png"))
                .unwrap()
                .to_luma8()
                .into_raw(),
            [0, 0, 255, 255]
        );
    }

    #[test]
    fn rgba16_masks_use_the_32768_boundary_and_preserve_original_pixels() {
        let source = tempfile::tempdir().unwrap();
        let frames = tempfile::tempdir().unwrap();
        let colmap_frames = tempfile::tempdir().unwrap();
        let masks = tempfile::tempdir().unwrap();
        let pixels = [
            Rgba([65_535_u16, 32_768, 1, 0]),
            Rgba([65_535, 32_768, 1, 32_767]),
            Rgba([65_535, 32_768, 1, 32_768]),
            Rgba([65_535, 32_768, 1, 65_535]),
        ];
        for name in ["1.png", "2.png"] {
            let image =
                ImageBuffer::<Rgba<u16>, Vec<u16>>::from_fn(4, 1, |x, _| pixels[x as usize]);
            image.save(source.path().join(name)).unwrap();
        }
        let original = std::fs::read(source.path().join("1.png")).unwrap();

        prepare_image_sequence(
            source.path(),
            frames.path(),
            colmap_frames.path(),
            masks.path(),
        )
        .unwrap();

        assert_eq!(
            std::fs::read(frames.path().join("frame_000001.png")).unwrap(),
            original
        );
        let output_path = colmap_frames.path().join("frame_000001.png");
        let decoder = ImageReader::open(&output_path)
            .unwrap()
            .with_guessed_format()
            .unwrap()
            .into_decoder()
            .unwrap();
        assert_eq!(decoder.color_type(), ColorType::Rgba16);
        assert_eq!(std::fs::read(output_path).unwrap(), original);
        let output = image::open(frames.path().join("frame_000001.png"))
            .unwrap()
            .to_rgba16();
        for (index, expected) in pixels.iter().enumerate() {
            assert_eq!(output.get_pixel(index as u32, 0), expected);
        }
        assert_eq!(
            image::open(masks.path().join("frame_000001.png.png"))
                .unwrap()
                .to_luma8()
                .into_raw(),
            [0, 0, 255, 255]
        );
    }

    #[test]
    fn luma_alpha_masks_use_native_8_and_16_bit_thresholds() {
        let temporary = tempfile::tempdir().unwrap();
        let mask8 = temporary.path().join("luma-alpha-8.png");
        let mask16 = temporary.path().join("luma-alpha-16.png");
        let image8 = DynamicImage::ImageLumaA8(ImageBuffer::from_fn(4, 1, |x, _| {
            image::LumaA([42, [0, 127, 128, 255][x as usize]])
        }));
        let image16 = DynamicImage::ImageLumaA16(ImageBuffer::from_fn(4, 1, |x, _| {
            image::LumaA([42_000, [0, 32_767, 32_768, 65_535][x as usize]])
        }));

        write_alpha_mask(&image8, &mask8).unwrap();
        write_alpha_mask(&image16, &mask16).unwrap();

        for path in [&mask8, &mask16] {
            let decoded = image::open(path).unwrap();
            assert_eq!(decoded.color(), ColorType::L8);
            assert_eq!(decoded.into_luma8().into_raw(), [0, 0, 255, 255]);
        }
    }

    #[test]
    fn opaque_inputs_are_byte_identical_in_frames_and_colmap_frames() {
        let source = tempfile::tempdir().unwrap();
        let frames = tempfile::tempdir().unwrap();
        let colmap_frames = tempfile::tempdir().unwrap();
        let masks = tempfile::tempdir().unwrap();
        image::RgbImage::from_pixel(2, 2, image::Rgb([11, 22, 33]))
            .save(source.path().join("1.jpg"))
            .unwrap();
        image::RgbImage::from_pixel(2, 2, image::Rgb([44, 55, 66]))
            .save(source.path().join("2.png"))
            .unwrap();

        let prepared = prepare_image_sequence(
            source.path(),
            frames.path(),
            colmap_frames.path(),
            masks.path(),
        )
        .unwrap();

        assert!(!prepared.has_alpha);
        for (source_name, prepared_name) in
            [("1.jpg", "frame_000001.jpg"), ("2.png", "frame_000002.png")]
        {
            let expected = std::fs::read(source.path().join(source_name)).unwrap();
            assert_eq!(
                std::fs::read(frames.path().join(prepared_name)).unwrap(),
                expected
            );
            assert_eq!(
                std::fs::read(colmap_frames.path().join(prepared_name)).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn checkpoint_validation_rejects_missing_misnamed_or_mismatched_colmap_images() {
        let source = tempfile::tempdir().unwrap();
        let frames = tempfile::tempdir().unwrap();
        let colmap_frames = tempfile::tempdir().unwrap();
        let masks = tempfile::tempdir().unwrap();
        write_rgba(&source.path().join("1.png"), 64);
        write_rgba(&source.path().join("2.png"), 128);
        prepare_image_sequence(
            source.path(),
            frames.path(),
            colmap_frames.path(),
            masks.path(),
        )
        .unwrap();

        let second = colmap_frames.path().join("frame_000002.png");
        std::fs::remove_file(&second).unwrap();
        assert!(validate_prepared_image_sequence(
            frames.path(),
            colmap_frames.path(),
            masks.path(),
            2,
            true,
        )
        .is_err());

        image::RgbImage::new(2, 2).save(&second).unwrap();
        std::fs::rename(&second, colmap_frames.path().join("wrong.png")).unwrap();
        assert!(validate_prepared_image_sequence(
            frames.path(),
            colmap_frames.path(),
            masks.path(),
            2,
            true,
        )
        .is_err());

        std::fs::remove_file(colmap_frames.path().join("wrong.png")).unwrap();
        RgbaImage::from_pixel(2, 2, Rgba([9, 8, 7, 128]))
            .save(&second)
            .unwrap();
        assert!(validate_prepared_image_sequence(
            frames.path(),
            colmap_frames.path(),
            masks.path(),
            2,
            true,
        )
        .is_err());

        std::fs::remove_file(&second).unwrap();
        image::RgbImage::new(3, 2).save(&second).unwrap();
        assert!(validate_prepared_image_sequence(
            frames.path(),
            colmap_frames.path(),
            masks.path(),
            2,
            true,
        )
        .is_err());

        std::fs::remove_file(&second).unwrap();
        std::fs::copy(
            frames.path().join("frame_000002.png"),
            colmap_frames.path().join("frame_000002.png"),
        )
        .unwrap();
        image::GrayImage::from_pixel(2, 2, Luma([1]))
            .save(masks.path().join("frame_000002.png.png"))
            .unwrap();
        assert!(validate_prepared_image_sequence(
            frames.path(),
            colmap_frames.path(),
            masks.path(),
            2,
            true,
        )
        .is_err());
    }

    fn prepared_name_fixture(has_alpha: bool) -> (tempfile::TempDir, Vec<String>) {
        let root = tempfile::tempdir().unwrap();
        for directory in ["frames", "colmap_frames", "masks"] {
            std::fs::create_dir(root.path().join(directory)).unwrap();
        }
        let names = vec![
            "frame_000001.png".to_owned(),
            "reshoot_000001.png".to_owned(),
            "reshoot_second_000001.png".to_owned(),
        ];
        for (name, (width, height)) in names.iter().zip([(2, 3), (5, 2), (3, 7)]) {
            let frame = root.path().join("frames").join(name);
            if has_alpha {
                RgbaImage::from_pixel(width, height, Rgba([11, 22, 33, 64]))
                    .save(&frame)
                    .unwrap();
                GrayImage::from_pixel(width, height, Luma([0]))
                    .save(root.path().join("masks").join(format!("{name}.png")))
                    .unwrap();
            } else {
                image::RgbImage::from_pixel(width, height, image::Rgb([11, 22, 33]))
                    .save(&frame)
                    .unwrap();
            }
            std::fs::copy(&frame, root.path().join("colmap_frames").join(name)).unwrap();
        }
        (root, names)
    }

    fn validate_name_fixture(
        root: &Path,
        names: &[String],
        has_alpha: bool,
    ) -> Result<PreparedImageSequence> {
        validate_prepared_image_names(
            &root.join("frames"),
            &root.join("colmap_frames"),
            &root.join("masks"),
            names,
            has_alpha,
        )
    }

    #[test]
    fn exact_prepared_names_accept_primary_and_multiple_reshoot_batches_with_mixed_dimensions() {
        let (root, mut names) = prepared_name_fixture(false);
        names.reverse();
        assert_eq!(
            validate_name_fixture(root.path(), &names, false).unwrap(),
            PreparedImageSequence {
                image_count: 3,
                mask_count: 0,
                has_alpha: false,
            }
        );
        // These APIs intentionally validate just their own preparation prefix.
        assert_eq!(
            validate_prepared_image_sequence(
                &root.path().join("frames"),
                &root.path().join("colmap_frames"),
                &root.path().join("masks"),
                1,
                false,
            )
            .unwrap()
            .image_count,
            1,
        );
        assert_eq!(
            validate_reshoot_image_sequence(
                &root.path().join("frames"),
                &root.path().join("colmap_frames"),
                &root.path().join("masks"),
                2,
                false,
            )
            .unwrap()
            .image_count,
            2,
        );
    }

    #[test]
    fn exact_prepared_names_reject_unsafe_empty_duplicate_or_unsupported_names() {
        let (root, names) = prepared_name_fixture(false);
        assert!(validate_name_fixture(root.path(), &[], false).is_err());
        for invalid in [
            "",
            "../frame_000001.png",
            "nested/frame_000001.png",
            "nested\\frame_000001.png",
            "/frame_000001.png",
            "C:\\frame_000001.png",
            "C:frame_000001.png",
            "frame\0.png",
            "frame\n.png",
            "frame\r.png",
            "frame\t.png",
            "frame.webp",
            "frame_000001.png",
            "FRAME_000001.PNG",
        ] {
            let mut invalid_names = names.clone();
            invalid_names.push(invalid.to_owned());
            assert!(
                validate_name_fixture(root.path(), &invalid_names, false).is_err(),
                "accepted unsafe or duplicate name {invalid:?}",
            );
        }
    }

    #[test]
    fn exact_prepared_names_reject_missing_extra_and_same_count_replacement_in_either_directory() {
        for directory in ["frames", "colmap_frames"] {
            for mutation in ["missing", "extra", "replacement"] {
                let (root, names) = prepared_name_fixture(false);
                let directory = root.path().join(directory);
                match mutation {
                    "missing" => std::fs::remove_file(directory.join(&names[1])).unwrap(),
                    "extra" => {
                        std::fs::copy(directory.join(&names[0]), directory.join("unexpected.png"))
                            .unwrap();
                    }
                    "replacement" => {
                        std::fs::rename(
                            directory.join(&names[1]),
                            directory.join("unexpected.png"),
                        )
                        .unwrap();
                    }
                    _ => unreachable!(),
                }
                assert!(
                    validate_name_fixture(root.path(), &names, false).is_err(),
                    "accepted {mutation} image in {}",
                    directory.display(),
                );
            }
        }
    }

    #[test]
    fn exact_prepared_names_reject_changed_colmap_pixels_dimensions_and_empty_frames() {
        for mutation in ["pixels", "dimensions", "empty", "corrupt"] {
            let (root, names) = prepared_name_fixture(false);
            let changed = root.path().join("colmap_frames").join(&names[2]);
            match mutation {
                "pixels" => image::RgbImage::from_pixel(3, 7, image::Rgb([44, 55, 66]))
                    .save(&changed)
                    .unwrap(),
                "dimensions" => image::RgbImage::new(4, 7).save(&changed).unwrap(),
                "empty" => std::fs::write(&changed, []).unwrap(),
                "corrupt" => std::fs::write(&changed, b"not a PNG").unwrap(),
                _ => unreachable!(),
            }
            assert!(
                validate_name_fixture(root.path(), &names, false).is_err(),
                "accepted {mutation} COLMAP image",
            );
        }
    }

    #[test]
    fn exact_prepared_names_validate_alpha_masks_for_every_historical_batch() {
        let (root, names) = prepared_name_fixture(true);
        assert_eq!(
            validate_name_fixture(root.path(), &names, true).unwrap(),
            PreparedImageSequence {
                image_count: 3,
                mask_count: 3,
                has_alpha: true,
            }
        );
    }

    #[test]
    fn exact_prepared_names_reject_missing_nonbinary_or_mismatched_alpha_masks() {
        for mutation in ["missing", "nonbinary", "dimensions", "color"] {
            let (root, names) = prepared_name_fixture(true);
            let mask = root.path().join("masks").join(format!("{}.png", names[2]));
            match mutation {
                "missing" => std::fs::remove_file(&mask).unwrap(),
                "nonbinary" => GrayImage::from_pixel(3, 7, Luma([1])).save(&mask).unwrap(),
                "dimensions" => GrayImage::from_pixel(4, 7, Luma([255]))
                    .save(&mask)
                    .unwrap(),
                "color" => image::RgbImage::new(3, 7).save(&mask).unwrap(),
                _ => unreachable!(),
            }
            assert!(
                validate_name_fixture(root.path(), &names, true).is_err(),
                "accepted {mutation} mask",
            );
        }
    }

    #[test]
    fn opaque_rgba_sequences_discard_candidate_masks() {
        let source = tempfile::tempdir().unwrap();
        let frames = tempfile::tempdir().unwrap();
        let colmap_frames = tempfile::tempdir().unwrap();
        let masks = tempfile::tempdir().unwrap();
        write_rgba(&source.path().join("1.png"), 255);
        write_rgba(&source.path().join("2.png"), 255);

        let scan = scan_image_sequence(source.path(), None).unwrap();
        assert!(scan.info.has_alpha);
        let prepared = prepare_scanned_image_sequence(
            scan,
            frames.path(),
            colmap_frames.path(),
            masks.path(),
            ImageSequenceNaming::Primary,
            None,
            None,
        )
        .unwrap();

        assert!(!prepared.has_alpha);
        assert_eq!(prepared.mask_count, 0);
        assert!(std::fs::read_dir(masks.path()).unwrap().next().is_none());
    }

    #[test]
    fn mixed_sequences_generate_white_masks_for_opaque_images() {
        let source = tempfile::tempdir().unwrap();
        let frames = tempfile::tempdir().unwrap();
        let colmap_frames = tempfile::tempdir().unwrap();
        let masks = tempfile::tempdir().unwrap();
        image::RgbImage::new(2, 2)
            .save(source.path().join("1.jpg"))
            .unwrap();
        write_rgba(&source.path().join("2.png"), 0);

        let prepared = prepare_image_sequence(
            source.path(),
            frames.path(),
            colmap_frames.path(),
            masks.path(),
        )
        .unwrap();

        assert!(prepared.has_alpha);
        assert_eq!(prepared.mask_count, 2);
        assert_eq!(
            image::open(masks.path().join("frame_000001.jpg.png"))
                .unwrap()
                .to_luma8()
                .get_pixel(0, 0)[0],
            255
        );
        assert_eq!(
            image::open(masks.path().join("frame_000002.png.png"))
                .unwrap()
                .to_luma8()
                .get_pixel(0, 0)[0],
            0
        );
        for name in ["frame_000001.jpg", "frame_000002.png"] {
            assert_eq!(
                std::fs::read(frames.path().join(name)).unwrap(),
                std::fs::read(colmap_frames.path().join(name)).unwrap()
            );
        }
    }

    #[test]
    fn reshoot_sequences_use_a_collision_free_prefix() {
        let source = tempfile::tempdir().unwrap();
        let frames = tempfile::tempdir().unwrap();
        let colmap_frames = tempfile::tempdir().unwrap();
        let masks = tempfile::tempdir().unwrap();
        write_rgba(&source.path().join("1.png"), 0);
        write_rgba(&source.path().join("2.png"), 255);
        let scan = scan_image_sequence(source.path(), None).unwrap();
        let prepared = prepare_scanned_image_sequence(
            scan,
            frames.path(),
            colmap_frames.path(),
            masks.path(),
            ImageSequenceNaming::Reshoot,
            None,
            None,
        )
        .unwrap();
        assert_eq!(prepared.mask_count, 2);
        assert!(frames.path().join("reshoot_000001.png").is_file());
        assert!(masks.path().join("reshoot_000001.png.png").is_file());
    }

    #[test]
    fn preparation_progress_is_monotonic_and_cancellation_is_honored() {
        let source = tempfile::tempdir().unwrap();
        let frames = tempfile::tempdir().unwrap();
        let colmap_frames = tempfile::tempdir().unwrap();
        let masks = tempfile::tempdir().unwrap();
        write_rgba(&source.path().join("1.png"), 0);
        write_rgba(&source.path().join("2.png"), 255);
        let scan = scan_image_sequence(source.path(), None).unwrap();
        let updates = Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured = updates.clone();
        let observer: ImagePreparationObserver = Arc::new(move |progress| {
            captured.lock().unwrap().push(progress.stage_progress);
        });

        prepare_scanned_image_sequence(
            scan,
            frames.path(),
            colmap_frames.path(),
            masks.path(),
            ImageSequenceNaming::Primary,
            Some(observer),
            None,
        )
        .unwrap();
        let updates = updates.lock().unwrap();
        assert!(!updates.is_empty());
        assert_eq!(updates.last().copied(), Some(1.0));
        assert!(updates.windows(2).all(|pair| pair[0] <= pair[1]));

        let cancellation = CancellationToken::new();
        cancellation.cancel();
        assert!(matches!(
            scan_image_sequence(source.path(), Some(&cancellation)),
            Err(SplatError::Cancelled)
        ));

        let cancelled_frames = tempfile::tempdir().unwrap();
        let cancelled_colmap_frames = tempfile::tempdir().unwrap();
        let cancelled_masks = tempfile::tempdir().unwrap();
        let cancellation = CancellationToken::new();
        let cancellation_from_observer = cancellation.clone();
        let observer: ImagePreparationObserver = Arc::new(move |progress| {
            if progress.phase == ImagePreparationPhase::LinkingFrames && progress.current == 1 {
                cancellation_from_observer.cancel();
            }
        });
        let scan = scan_image_sequence(source.path(), None).unwrap();
        assert!(matches!(
            prepare_scanned_image_sequence(
                scan,
                cancelled_frames.path(),
                cancelled_colmap_frames.path(),
                cancelled_masks.path(),
                ImageSequenceNaming::Primary,
                Some(observer),
                Some(&cancellation),
            ),
            Err(SplatError::Cancelled)
        ));
    }

    #[test]
    fn hard_link_failure_falls_back_to_copy() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.jpg");
        let destination = dir.path().join("destination.jpg");
        std::fs::write(&source, b"image bytes").unwrap();

        link_or_copy_with(&source, &destination, |_, _| {
            Err(std::io::Error::new(
                std::io::ErrorKind::CrossesDevices,
                "different volume",
            ))
        })
        .unwrap();

        assert_eq!(std::fs::read(destination).unwrap(), b"image bytes");
    }
}
