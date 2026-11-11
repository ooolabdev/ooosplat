//! Ordinary-image Splat ceilings, adapted from TangoGS's static allowance.
//! This does not implement TangoGS's growth/pruning controller or promise a
//! final point count. ERP faces must not be paired with spherical tracks.
use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Component, Path},
};

use image::{DynamicImage, ImageDecoder, ImageReader};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::{
    error::{Result, SplatError},
    presets::Quality,
    reconstruction::colmap_model::read_cameras,
};

/// Preserve the native-resolution Detailed snapshot format for saved tasks.
pub const VERSION: u32 = 1;
pub const COEFFICIENT: f64 = 0.5;
pub const TIER_VERSION: u32 = 2;
const TANGO_SCALE: f64 = 2_462_476.0 * 0.063_362_106;
const DECODE_LIMIT: u64 = 512 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FineSplatBudget {
    pub version: u32,
    pub coefficient: f64,
    pub registered_images: u64,
    pub effective_pixels: u64,
    pub sparse_points: u64,
    pub tracked_points: u64,
    pub observations: u64,
    pub mean_track_length: f64,
    pub requested_max_splats: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality: Option<Quality>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub training_max_resolution: Option<u32>,
}

impl FineSplatBudget {
    pub fn validated_ceiling(self, sparse_points: u64) -> Result<u32> {
        if self.version != VERSION
            || self.coefficient != COEFFICIENT
            || self.quality.is_some()
            || self.training_max_resolution.is_some()
        {
            return Err(invalid());
        }
        self.validate_values(sparse_points)
    }

    /// The new tier budgets are bound to the actual Brush load resolution.
    /// Legacy Detailed records remain byte-for-byte compatible and are not
    /// reinterpreted at another quality or resolution.
    pub fn validated_ceiling_for_quality(
        self,
        sparse_points: u64,
        quality: Quality,
        max_resolution: u32,
    ) -> Result<u32> {
        if quality == Quality::High {
            return self.validated_ceiling(sparse_points);
        }
        if self.version != TIER_VERSION
            || self.coefficient != coefficient(quality)
            || self.quality != Some(quality)
            || self.training_max_resolution != Some(max_resolution)
            || max_resolution == 0
        {
            return Err(invalid());
        }
        self.validate_values(sparse_points)
    }

    fn validate_values(self, sparse_points: u64) -> Result<u32> {
        if self.sparse_points != sparse_points
            || self.tracked_points > sparse_points
            || self.registered_images == 0
            || !self.mean_track_length.is_finite()
            || (self.mean_track_length - self.observations as f64 / self.tracked_points as f64)
                .abs()
                > 1e-12 * self.mean_track_length.abs().max(1.)
            || self.requested_max_splats
                != ceiling_with_coefficient(
                    self.effective_pixels,
                    self.tracked_points,
                    self.observations,
                    self.coefficient,
                )?
        {
            return Err(invalid());
        }
        Ok(self.requested_max_splats)
    }
}

pub const fn coefficient(quality: Quality) -> f64 {
    match quality {
        Quality::Fast => 0.2,
        Quality::Balanced => 0.4,
        Quality::High => COEFFICIENT,
    }
}

fn invalid() -> SplatError {
    SplatError::Process("Cannot calculate Splat budget from invalid training data".into())
}

fn check(cancel: &CancellationToken) -> Result<()> {
    if cancel.is_cancelled() {
        Err(SplatError::Cancelled)
    } else {
        Ok(())
    }
}

fn bytes<const N: usize>(reader: &mut impl Read) -> Result<[u8; N]> {
    let mut value = [0; N];
    reader.read_exact(&mut value)?;
    Ok(value)
}

struct Image {
    id: u32,
    camera: u32,
    name: String,
    observations: u64,
}

fn images(model: &Path, cancel: &CancellationToken) -> Result<Vec<Image>> {
    let path = model.join("images.bin");
    let mut result = Vec::new();
    if path.is_file() {
        let file = File::open(path)?;
        let length = file.metadata()?.len();
        let mut reader = BufReader::new(file);
        let count = u64::from_le_bytes(bytes(&mut reader)?);
        if count == 0 || count > 1_000_000 || count > length / 73 {
            return Err(invalid());
        }
        for _ in 0..count {
            check(cancel)?;
            let head: [u8; 64] = bytes(&mut reader)?;
            let mut name = Vec::new();
            loop {
                let [byte] = bytes(&mut reader)?;
                if byte == 0 {
                    break;
                }
                name.push(byte);
                if name.len() > 32 * 1024 {
                    return Err(invalid());
                }
            }
            let observations = u64::from_le_bytes(bytes(&mut reader)?);
            let end = observations
                .checked_mul(24)
                .and_then(|n| reader.stream_position().ok()?.checked_add(n))
                .filter(|end| *end <= length)
                .ok_or_else(invalid)?;
            // Do not read or allocate all 2D features just to count tracks.
            reader.seek(SeekFrom::Start(end))?;
            result.push(Image {
                id: u32::from_le_bytes(head[..4].try_into().unwrap()),
                camera: u32::from_le_bytes(head[60..].try_into().unwrap()),
                name: String::from_utf8(name).map_err(|_| invalid())?,
                observations,
            });
        }
        if reader.stream_position()? != length {
            return Err(invalid());
        }
    } else {
        let mut lines = BufReader::new(File::open(model.join("images.txt"))?).lines();
        while let Some(line) = lines.next() {
            check(cancel)?;
            let line = line?;
            if line.trim().is_empty() || line.trim_start().starts_with('#') {
                continue;
            }
            let fields = line.split_whitespace().collect::<Vec<_>>();
            if fields.len() < 10 {
                return Err(invalid());
            }
            let observations = lines.next().ok_or_else(invalid)??;
            let count = observations.split_whitespace().count();
            if count % 3 != 0 {
                return Err(invalid());
            }
            result.push(Image {
                id: fields[0].parse().map_err(|_| invalid())?,
                camera: fields[8].parse().map_err(|_| invalid())?,
                name: fields[9..].join(" "),
                observations: (count / 3) as u64,
            });
        }
    }
    let mut ids = HashSet::new();
    let mut names = HashSet::new();
    if result.is_empty()
        || result
            .iter()
            .any(|i| i.id == 0 || !ids.insert(i.id) || i.name.is_empty() || !names.insert(&i.name))
    {
        return Err(invalid());
    }
    Ok(result)
}

fn tracks(model: &Path, images: &[Image], cancel: &CancellationToken) -> Result<(u64, u64, u64)> {
    let registered: HashMap<_, _> = images.iter().map(|i| (i.id, i.observations)).collect();
    let mut points = 0_u64;
    let mut tracked = 0_u64;
    let mut observations = 0_u64;
    let mut seen = HashSet::new();
    let mut add = |entries: &[(u32, u32)]| -> Result<()> {
        seen.clear();
        let valid = entries
            .iter()
            .filter(|(id, index)| {
                registered.get(id).is_some_and(|n| u64::from(*index) < *n) && seen.insert(*id)
            })
            .count() as u64;
        points = points.checked_add(1).ok_or_else(invalid)?;
        if valid > 0 {
            tracked = tracked.checked_add(1).ok_or_else(invalid)?;
            observations = observations.checked_add(valid).ok_or_else(invalid)?;
        }
        Ok(())
    };
    let path = model.join("points3D.bin");
    if path.is_file() {
        let file = File::open(path)?;
        let length = file.metadata()?.len();
        let mut reader = BufReader::new(file);
        let count = u64::from_le_bytes(bytes(&mut reader)?);
        if count == 0 || count > length.saturating_sub(8) / 51 {
            return Err(invalid());
        }
        let mut entries = Vec::new();
        for _ in 0..count {
            check(cancel)?;
            let head: [u8; 51] = bytes(&mut reader)?;
            let count = u64::from_le_bytes(head[43..].try_into().unwrap());
            if count > 1_000_000 || count > length / 8 {
                return Err(invalid());
            }
            entries.clear();
            for _ in 0..count {
                entries.push((
                    u32::from_le_bytes(bytes(&mut reader)?),
                    u32::from_le_bytes(bytes(&mut reader)?),
                ));
            }
            add(&entries)?;
        }
        if reader.stream_position()? != length {
            return Err(invalid());
        }
    } else {
        for line in BufReader::new(File::open(model.join("points3D.txt"))?).lines() {
            check(cancel)?;
            let line = line?;
            if line.trim().is_empty() || line.trim_start().starts_with('#') {
                continue;
            }
            let fields = line.split_whitespace().collect::<Vec<_>>();
            if fields.len() < 8 || (fields.len() - 8) % 2 != 0 {
                return Err(invalid());
            }
            let entries = fields[8..]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|v| {
                    Ok((
                        v[0].parse().map_err(|_| invalid())?,
                        v[1].parse().map_err(|_| invalid())?,
                    ))
                })
                .collect::<Result<Vec<_>>>()?;
            add(&entries)?;
        }
    }
    if points == 0 || tracked == 0 || observations == 0 {
        return Err(invalid());
    }
    Ok((points, tracked, observations))
}

/// Count Alpha > 0, not COLMAP's Alpha >= 0.5 feature mask. The latter would
/// incorrectly remove semi-transparent pixels that still participate in Brush.
fn effective_pixels(
    path: &Path,
    max_resolution: Option<u32>,
    cancel: &CancellationToken,
) -> Result<((u32, u32), u64)> {
    check(cancel)?;
    let decoder = ImageReader::open(path)?
        .with_guessed_format()?
        .into_decoder()
        .map_err(|e| SplatError::Process(e.to_string()))?;
    let dimensions = decoder.dimensions();
    let total = u64::from(dimensions.0) * u64::from(dimensions.1);
    if total == 0 || decoder.total_bytes() > DECODE_LIMIT {
        return Err(invalid());
    }
    let output_dimensions = if let Some(max) = max_resolution {
        if max == 0 {
            return Err(invalid());
        }
        let decoded_dimensions = jpeg_decode_dimensions(path, dimensions, max, cancel)?;
        brush_output_dimensions(decoded_dimensions, max)
    } else {
        dimensions
    };
    if !decoder.color_type().has_alpha() {
        return Ok((
            dimensions,
            u64::from(output_dimensions.0) * u64::from(output_dimensions.1),
        ));
    }
    let mut decoded =
        DynamicImage::from_decoder(decoder).map_err(|e| SplatError::Process(e.to_string()))?;
    check(cancel)?;
    if dimensions != output_dimensions {
        // Match the pinned Brush LoadImage::load path, including f32 size
        // truncation and Alpha support after Lanczos filtering.
        decoded = decoded.resize_exact(
            output_dimensions.0,
            output_dimensions.1,
            image::imageops::FilterType::Lanczos3,
        );
        check(cancel)?;
    }
    if max_resolution.is_some() {
        // Brush's packed training tensor is RGBA8, including for 16-bit
        // sources. Preserve the historical native-alpha rule only for v1.
        decoded = DynamicImage::ImageRgba8(decoded.into_rgba8());
    }
    let mut count = 0_u64;
    macro_rules! count {
        ($buffer:expr, $channels:expr, $alpha:expr) => {
            for chunk in $buffer.as_raw().chunks(8192 * $channels) {
                check(cancel)?;
                count += chunk
                    .chunks_exact($channels)
                    .filter(|p| p[$alpha] > 0)
                    .count() as u64;
            }
        };
    }
    match decoded {
        DynamicImage::ImageRgba8(v) => count!(v, 4, 3),
        DynamicImage::ImageLumaA8(v) => count!(v, 2, 1),
        DynamicImage::ImageRgba16(v) => count!(v, 4, 3),
        DynamicImage::ImageLumaA16(v) => count!(v, 2, 1),
        _ => return Err(invalid()),
    }
    Ok((dimensions, count))
}

/// Brush caps the long edge with an f32 scale, then truncates both dimensions.
/// This intentionally does not use image::resize's aspect-ratio rounding.
fn brush_output_dimensions((width, height): (u32, u32), max: u32) -> (u32, u32) {
    let scale = max as f32 / width.max(height).max(max) as f32;
    if scale < 1.0 {
        (
            (width as f32 * scale).max(1.0) as u32,
            (height as f32 * scale).max(1.0) as u32,
        )
    } else {
        (width, height)
    }
}

/// Pinned Brush uses jpeg-decoder's IDCT thumbnail before its final resize for
/// ordinary 8-bit JPEGs. Read only marker headers (at most 64 KiB), not pixels;
/// unsupported/ambiguous JPEG metadata follows Brush's native decode fallback.
fn jpeg_decode_dimensions(
    path: &Path,
    dimensions: (u32, u32),
    max: u32,
    cancel: &CancellationToken,
) -> Result<(u32, u32)> {
    if !path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("jpg") || e.eq_ignore_ascii_case("jpeg"))
    {
        return Ok(dimensions);
    }
    let mut reader = BufReader::new(File::open(path)?);
    if bytes::<2>(&mut reader)? != [0xff, 0xd8] {
        return Ok(dimensions);
    }
    let mut header_bytes = 2_u64;
    while header_bytes < 64 * 1024 {
        check(cancel)?;
        if bytes::<1>(&mut reader)? != [0xff] {
            return Ok(dimensions);
        }
        let mut marker = bytes::<1>(&mut reader)?[0];
        header_bytes += 2;
        while marker == 0xff && header_bytes < 64 * 1024 {
            marker = bytes::<1>(&mut reader)?[0];
            header_bytes += 1;
        }
        if matches!(marker, 0 | 0xd9 | 0xda) {
            return Ok(dimensions);
        }
        if matches!(marker, 0x01 | 0xd0..=0xd8) {
            continue;
        }
        let length = u16::from_be_bytes(bytes(&mut reader)?) as u64;
        header_bytes += 2;
        if length < 2 {
            return Ok(dimensions);
        }
        if matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) {
            if length < 8 {
                return Ok(dimensions);
            }
            let header = bytes::<6>(&mut reader)?;
            let size = (
                u16::from_be_bytes([header[3], header[4]]) as u32,
                u16::from_be_bytes([header[1], header[2]]) as u32,
            );
            if !matches!(marker, 0xc0..=0xc2)
                || header[0] != 8
                || !matches!(header[5], 1 | 3)
                || length != 8 + u64::from(header[5]) * 3
                || size != dimensions
            {
                return Ok(dimensions);
            }
            let target = max.min(u32::from(u16::MAX));
            for divisor in [8, 4, 2, 1] {
                let scaled = (size.0.div_ceil(divisor), size.1.div_ceil(divisor));
                if scaled.0 >= target || scaled.1 >= target || divisor == 1 {
                    return Ok(scaled);
                }
            }
        }
        reader.seek(SeekFrom::Current((length - 2) as i64))?;
    }
    Ok(dimensions)
}

pub fn ceiling(effective_pixels: u64, tracked: u64, observations: u64) -> Result<u32> {
    ceiling_with_coefficient(effective_pixels, tracked, observations, COEFFICIENT)
}

pub fn ceiling_for_quality(
    effective_pixels: u64,
    tracked: u64,
    observations: u64,
    quality: Quality,
) -> Result<u32> {
    ceiling_with_coefficient(
        effective_pixels,
        tracked,
        observations,
        coefficient(quality),
    )
}

fn ceiling_with_coefficient(
    effective_pixels: u64,
    tracked: u64,
    observations: u64,
    coefficient: f64,
) -> Result<u32> {
    if effective_pixels == 0
        || tracked == 0
        || observations < tracked
        || !coefficient.is_finite()
        || coefficient <= 0.0
    {
        return Err(invalid());
    }
    let value = (coefficient
        * (TANGO_SCALE * effective_pixels as f64 * tracked as f64 / observations as f64).sqrt())
    .round();
    if !value.is_finite() || value < 1.0 || value > f64::from(u32::MAX) {
        return Err(invalid());
    }
    Ok(value as u32)
}

/// Called only after reconstruction, off the async executor. Examine every
/// registered training view, sequentially, without modifying files or sampling.
pub fn calculate(
    model: &Path,
    image_root: &Path,
    cancel: &CancellationToken,
) -> Result<FineSplatBudget> {
    calculate_internal(model, image_root, None, cancel)
}

pub fn calculate_for_quality(
    model: &Path,
    image_root: &Path,
    quality: Quality,
    max_resolution: u32,
    cancel: &CancellationToken,
) -> Result<FineSplatBudget> {
    if max_resolution == 0 {
        return Err(invalid());
    }
    if quality == Quality::High {
        // Do not change the existing Detailed formula or snapshot identity.
        calculate(model, image_root, cancel)
    } else {
        calculate_internal(model, image_root, Some((quality, max_resolution)), cancel)
    }
}

fn calculate_internal(
    model: &Path,
    image_root: &Path,
    tier: Option<(Quality, u32)>,
    cancel: &CancellationToken,
) -> Result<FineSplatBudget> {
    check(cancel)?;
    let cameras = read_cameras(model)?;
    if cameras.iter().any(|c| c.model_id == 17) {
        return Err(invalid());
    }
    let images = images(model, cancel)?;
    let (points, tracked, observations) = tracks(model, &images, cancel)?;
    let root = image_root.canonicalize()?;
    let mut pixels = 0_u64;
    for image in &images {
        check(cancel)?;
        let relative = Path::new(&image.name);
        if relative
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err(invalid());
        }
        let path = root.join(relative).canonicalize()?;
        if !path.starts_with(&root) || !path.is_file() {
            return Err(invalid());
        }
        let (dimensions, valid) = effective_pixels(&path, tier.map(|(_, max)| max), cancel)?;
        let camera = cameras
            .iter()
            .find(|c| c.id == image.camera)
            .ok_or_else(invalid)?;
        if (u64::from(dimensions.0), u64::from(dimensions.1)) != (camera.width, camera.height) {
            return Err(invalid());
        }
        pixels = pixels.checked_add(valid).ok_or_else(invalid)?;
    }
    Ok(FineSplatBudget {
        version: if tier.is_some() {
            TIER_VERSION
        } else {
            VERSION
        },
        coefficient: tier.map_or(COEFFICIENT, |(quality, _)| coefficient(quality)),
        registered_images: images.len() as u64,
        effective_pixels: pixels,
        sparse_points: points,
        tracked_points: tracked,
        observations,
        mean_track_length: observations as f64 / tracked as f64,
        requested_max_splats: if let Some((quality, _)) = tier {
            ceiling_for_quality(pixels, tracked, observations, quality)?
        } else {
            ceiling(pixels, tracked, observations)?
        },
        quality: tier.map(|(quality, _)| quality),
        training_max_resolution: tier.map(|(_, max)| max),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn historical_formula_matches_the_confirmed_half_coefficient() {
        assert_eq!(ceiling(364_993_345, 66_218, 191_175).unwrap(), 2_220_679);
        assert_eq!(ceiling(56_578_232, 34_842, 202_872).unwrap(), 615_653);
        for inputs in [(0, 1, 1), (1, 0, 1), (1, 2, 1), (u64::MAX, 1, 1)] {
            assert!(ceiling(inputs.0, inputs.1, inputs.2).is_err());
        }
    }

    #[test]
    fn fast_and_balanced_use_the_confirmed_coefficients() {
        let inputs = (364_993_345, 66_218, 191_175);
        assert_eq!(
            ceiling_for_quality(inputs.0, inputs.1, inputs.2, Quality::Fast).unwrap(),
            888_272
        );
        assert_eq!(
            ceiling_for_quality(inputs.0, inputs.1, inputs.2, Quality::Balanced).unwrap(),
            1_776_543
        );
        assert_eq!(
            ceiling_for_quality(inputs.0, inputs.1, inputs.2, Quality::High).unwrap(),
            ceiling(inputs.0, inputs.1, inputs.2).unwrap()
        );
        for coefficient in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(ceiling_with_coefficient(1, 1, 1, coefficient).is_err());
        }
    }

    #[test]
    fn tier_snapshot_is_bound_to_quality_and_actual_training_resolution() {
        let (_temp, model, root) = fixture();
        let cancel = CancellationToken::new();
        for quality in [Quality::Fast, Quality::Balanced] {
            let budget = calculate_for_quality(&model, &root, quality, 2, &cancel).unwrap();
            assert_eq!(budget.version, TIER_VERSION);
            assert_eq!(budget.coefficient, coefficient(quality));
            assert_eq!(budget.quality, Some(quality));
            assert_eq!(budget.training_max_resolution, Some(2));
            assert_eq!(
                budget.validated_ceiling_for_quality(3, quality, 2).unwrap(),
                budget.requested_max_splats
            );
            assert!(budget.validated_ceiling(3).is_err());
            assert!(budget.validated_ceiling_for_quality(3, quality, 3).is_err());
            assert!(budget.validated_ceiling_for_quality(4, quality, 2).is_err());
            assert!(budget
                .validated_ceiling_for_quality(3, Quality::High, 2)
                .is_err());
            let mut changed = budget;
            changed.coefficient = 0.5;
            assert!(changed
                .validated_ceiling_for_quality(3, quality, 2)
                .is_err());
            changed = budget;
            changed.version = VERSION;
            assert!(changed
                .validated_ceiling_for_quality(3, quality, 2)
                .is_err());
            changed = budget;
            changed.requested_max_splats += 1;
            assert!(changed
                .validated_ceiling_for_quality(3, quality, 2)
                .is_err());
        }
        assert!(calculate_for_quality(&model, &root, Quality::Fast, 0, &cancel).is_err());
        let fast = calculate_for_quality(&model, &root, Quality::Fast, 2, &cancel).unwrap();
        assert!(fast
            .validated_ceiling_for_quality(3, Quality::Balanced, 2)
            .is_err());
    }

    #[test]
    fn detailed_snapshot_keeps_its_original_serialized_identity() {
        let (_temp, model, root) = fixture();
        let cancel = CancellationToken::new();
        let legacy = calculate(&model, &root, &cancel).unwrap();
        let new_entry = calculate_for_quality(&model, &root, Quality::High, 1, &cancel).unwrap();
        assert_eq!(legacy, new_entry);
        let encoded = serde_json::to_string(&legacy).unwrap();
        assert!(!encoded.contains("quality"));
        assert!(!encoded.contains("trainingMaxResolution"));
        let restored: FineSplatBudget = serde_json::from_str(&encoded).unwrap();
        assert_eq!(serde_json::to_string(&restored).unwrap(), encoded);
        assert_eq!(
            restored.validated_ceiling(3).unwrap(),
            legacy.requested_max_splats
        );
    }

    #[test]
    fn effective_pixels_follow_brush_resize_and_rgba8_alpha_without_writing_files() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("image.png");
        let cancel = CancellationToken::new();
        image::RgbImage::new(11, 7).save(&path).unwrap();
        assert_eq!(
            effective_pixels(&path, Some(5), &cancel).unwrap(),
            ((11, 7), 15)
        );
        assert_eq!(effective_pixels(&path, Some(20), &cancel).unwrap().1, 77);
        assert_eq!(brush_output_dimensions((2, 100), 1), (1, 1));
        assert_eq!(brush_output_dimensions((100, 2), 1), (1, 1));
        image::ImageBuffer::<image::Rgba<u16>, Vec<u16>>::from_fn(8, 4, |x, y| {
            image::Rgba([10, 20, 30, if (x + y) % 4 == 0 { 65535 } else { 1 }])
        })
        .save(&path)
        .unwrap();
        let before = std::fs::read(&path).unwrap();
        let loaded = image::open(&path)
            .unwrap()
            .resize_exact(4, 2, image::imageops::FilterType::Lanczos3)
            .into_rgba8();
        let expected = loaded.pixels().filter(|pixel| pixel[3] > 0).count() as u64;
        assert_eq!(
            effective_pixels(&path, Some(4), &cancel).unwrap().1,
            expected
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
        image::ImageBuffer::<image::Rgba<u16>, Vec<u16>>::from_pixel(
            2,
            2,
            image::Rgba([1, 2, 3, 1]),
        )
        .save(&path)
        .unwrap();
        assert_eq!(effective_pixels(&path, None, &cancel).unwrap().1, 4);
        assert_eq!(effective_pixels(&path, Some(2), &cancel).unwrap().1, 0);
    }

    #[test]
    fn jpeg_dimensions_match_idct_headers_and_unsupported_formats_fall_back() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("image.JPG");
        let cancel = CancellationToken::new();
        let write_header = |marker, precision, channels: u8, long_metadata: bool| {
            let mut bytes = vec![0xff, 0xd8];
            if long_metadata {
                // Metadata payload can exceed the read budget without being
                // read or decoded. Marker headers remain bounded.
                bytes.extend([0xff, 0xe1, 0xff, 0xff]);
                bytes.resize(bytes.len() + 65_533, 0);
            }
            bytes.extend([0xff, marker]);
            bytes.extend((8_u16 + u16::from(channels) * 3).to_be_bytes());
            bytes.push(precision);
            bytes.extend(1001_u16.to_be_bytes());
            bytes.extend(2001_u16.to_be_bytes());
            bytes.push(channels);
            for channel in 0..channels {
                bytes.extend([channel + 1, 0x11, 0]);
            }
            std::fs::write(&path, bytes).unwrap();
        };
        for marker in [0xc0, 0xc1, 0xc2] {
            write_header(marker, 8, 3, marker == 0xc2);
            let decoded = jpeg_decode_dimensions(&path, (2001, 1001), 600, &cancel).unwrap();
            assert_eq!(decoded, (1001, 501));
            assert_eq!(brush_output_dimensions(decoded, 600), (600, 300));
        }
        for (marker, precision, channels) in [(0xc0, 8, 4), (0xc1, 12, 3), (0xc3, 16, 1)] {
            write_header(marker, precision, channels, false);
            assert_eq!(
                jpeg_decode_dimensions(&path, (2001, 1001), 600, &cancel).unwrap(),
                (2001, 1001)
            );
        }
    }

    fn fixture() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let model = temp.path().join("model");
        let root = temp.path().join("images");
        std::fs::create_dir_all(&model).unwrap();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            model.join("cameras.txt"),
            "1 SIMPLE_RADIAL 4 2 2 2 1 0\n2 SIMPLE_RADIAL 2 2 2 1 1 0\n",
        )
        .unwrap();
        std::fs::write(
            model.join("images.txt"),
            "1 1 0 0 0 0 0 0 1 a.png\n0 0 1 1 0 2\n2 1 0 0 0 0 0 0 2 b.png\n0 0 1 1 0 2\n",
        )
        .unwrap();
        std::fs::write(model.join("points3D.txt"), "1 0 0 1 255 255 255 0 1 0 2 0 999 0 1 1\n2 0 1 1 255 255 255 0 1 1 2 1 2 99\n3 1 1 1 255 255 255 0 999 0\n").unwrap();
        image::RgbaImage::from_fn(4, 2, |x, y| {
            image::Rgba([
                10,
                20,
                30,
                [0, 1, 127, 128, 255, 255, 0, 0][(y * 4 + x) as usize],
            ])
        })
        .save(root.join("a.png"))
        .unwrap();
        image::ImageBuffer::<image::Rgba<u16>, Vec<u16>>::from_fn(2, 2, |x, y| {
            image::Rgba([10, 20, 30, [0, 1, 32767, 65535][(y * 2 + x) as usize]])
        })
        .save(root.join("b.png"))
        .unwrap();
        // Unregistered files do not contribute and need not even decode.
        std::fs::write(root.join("unused.png"), b"unused").unwrap();
        (temp, model, root)
    }

    #[test]
    fn all_registered_pixels_are_counted_with_native_alpha_and_valid_tracks_only() {
        let (_temp, model, root) = fixture();
        let before = std::fs::read(root.join("a.png")).unwrap();
        let budget = calculate(&model, &root, &CancellationToken::new()).unwrap();
        assert_eq!(budget.effective_pixels, 8);
        assert_eq!(budget.registered_images, 2);
        assert_eq!(
            (
                budget.sparse_points,
                budget.tracked_points,
                budget.observations
            ),
            (3, 2, 4)
        );
        assert_eq!(budget.mean_track_length, 2.);
        assert_eq!(std::fs::read(root.join("a.png")).unwrap(), before);
        let restored: FineSplatBudget =
            serde_json::from_str(&serde_json::to_string(&budget).unwrap()).unwrap();
        assert_eq!(
            restored.validated_ceiling(3).unwrap(),
            budget.requested_max_splats
        );
        assert!(restored.validated_ceiling(4).is_err());
        let mut changed = restored;
        changed.coefficient = 0.6;
        assert!(changed.validated_ceiling(3).is_err());
    }

    #[test]
    fn binary_and_text_models_use_the_same_track_definition() {
        let (_temp, model, root) = fixture();
        let expected = calculate(&model, &root, &CancellationToken::new()).unwrap();
        let mut images = File::create(model.join("images.bin")).unwrap();
        images.write_all(&2_u64.to_le_bytes()).unwrap();
        for (id, name) in [(1_u32, "a.png"), (2, "b.png")] {
            images.write_all(&id.to_le_bytes()).unwrap();
            images.write_all(&[0; 56]).unwrap();
            images.write_all(&id.to_le_bytes()).unwrap();
            images.write_all(name.as_bytes()).unwrap();
            images.write_all(&[0]).unwrap();
            images.write_all(&2_u64.to_le_bytes()).unwrap();
            images.write_all(&[0; 48]).unwrap();
        }
        drop(images);
        let mut points = File::create(model.join("points3D.bin")).unwrap();
        points.write_all(&3_u64.to_le_bytes()).unwrap();
        for entries in [
            vec![(1_u32, 0_u32), (2, 0), (999, 0), (1, 1)],
            vec![(1, 1), (2, 1), (2, 99)],
            vec![(999, 0)],
        ] {
            points.write_all(&[0; 43]).unwrap();
            points
                .write_all(&(entries.len() as u64).to_le_bytes())
                .unwrap();
            for (image, index) in entries {
                points.write_all(&image.to_le_bytes()).unwrap();
                points.write_all(&index.to_le_bytes()).unwrap();
            }
        }
        drop(points);
        assert_eq!(
            calculate(&model, &root, &CancellationToken::new()).unwrap(),
            expected
        );
        File::options()
            .write(true)
            .open(model.join("points3D.bin"))
            .unwrap()
            .set_len(60)
            .unwrap();
        assert!(calculate(&model, &root, &CancellationToken::new()).is_err());
    }

    #[test]
    fn opaque_pixels_are_not_inferred_from_rgb_and_luma_alpha_is_supported() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("image.png");
        let cancel = CancellationToken::new();
        image::RgbImage::new(3, 2).save(&path).unwrap();
        assert_eq!(effective_pixels(&path, None, &cancel).unwrap().1, 6);
        image::ImageBuffer::<image::LumaA<u8>, Vec<u8>>::from_raw(3, 1, vec![0, 0, 0, 1, 0, 255])
            .unwrap()
            .save(&path)
            .unwrap();
        assert_eq!(effective_pixels(&path, None, &cancel).unwrap().1, 2);
        image::ImageBuffer::<image::LumaA<u16>, Vec<u16>>::from_raw(
            3,
            1,
            vec![0, 0, 0, 1, 0, 65535],
        )
        .unwrap()
        .save(&path)
        .unwrap();
        assert_eq!(effective_pixels(&path, None, &cancel).unwrap().1, 2);
        cancel.cancel();
        assert!(matches!(
            effective_pixels(&path, None, &cancel),
            Err(SplatError::Cancelled)
        ));
    }

    #[test]
    fn projection_missing_images_dimensions_paths_and_cancellation_are_guarded() {
        let (_temp, model, root) = fixture();
        let cancel = CancellationToken::new();
        std::fs::write(model.join("cameras.txt"), "1 EQUIRECTANGULAR 4 2 4 2\n").unwrap();
        assert!(calculate(&model, &root, &cancel).is_err());
        std::fs::write(model.join("cameras.txt"), "1 SIMPLE_RADIAL 2 2 2 1 1 0\n").unwrap();
        assert!(calculate(&model, &root, &cancel).is_err());
        std::fs::write(
            model.join("images.txt"),
            "1 1 0 0 0 0 0 0 1 ../a.png\n0 0 1\n",
        )
        .unwrap();
        assert!(calculate(&model, &root, &cancel).is_err());
        std::fs::write(
            model.join("images.txt"),
            "1 1 0 0 0 0 0 0 1 missing.png\n0 0 1\n",
        )
        .unwrap();
        assert!(calculate(&model, &root, &cancel).is_err());
        cancel.cancel();
        assert!(matches!(
            calculate(&model, &root, &cancel),
            Err(SplatError::Cancelled)
        ));
    }

    #[test]
    #[ignore = "read-only check against the user's existing 90-view native-resolution project"]
    fn real_history_budget_matches_the_previously_calculated_ceiling() {
        let project = Path::new("E:/GaussianSplatting/test/20261027-085019_images");
        let state_before = std::fs::read(project.join("state.json")).unwrap();
        let model = project.join("work/brush/dataset/sparse/0");
        let images = project.join("work/brush/dataset/images");
        let budget = calculate(&model, &images, &CancellationToken::new()).unwrap();
        println!("{}", serde_json::to_string(&budget).unwrap());
        assert_eq!(budget.effective_pixels, 364_993_345);
        assert_eq!(budget.requested_max_splats, 2_220_679);
        assert_eq!(budget.registered_images, 90);
        assert_eq!(
            std::fs::read(project.join("state.json")).unwrap(),
            state_before
        );
    }
}
