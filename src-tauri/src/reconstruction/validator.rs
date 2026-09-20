use std::{
    io::{BufReader, Read, Seek, SeekFrom},
    path::Path,
};

use serde::{Deserialize, Serialize};

use crate::error::{Result, SplatError};

pub const GOOD_REGISTERED_RATIO: f64 = 0.80;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReconstructionQuality {
    Good,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconstructionReport {
    pub input_images: u64,
    pub registered_images: u64,
    pub registered_ratio: f64,
    pub points_3d: u64,
    pub quality: ReconstructionQuality,
}

pub struct ReconstructionValidator;

impl ReconstructionValidator {
    pub fn validate(frames: &Path, sparse_model: &Path) -> Result<ReconstructionReport> {
        let cameras = sparse_model.join("cameras.bin");
        let images = sparse_model.join("images.bin");
        let points = sparse_model.join("points3D.bin");
        for path in [&cameras, &images, &points] {
            if !path.is_file() || path.metadata()?.len() <= 8 {
                return Err(SplatError::Process(format!(
                    "稀疏重建输出不完整：{}",
                    path.display()
                )));
            }
        }
        let input_images = count_input_images(frames)?;
        let registered_images = read_colmap_count(&images)?;
        let points_3d = read_colmap_count(&points)?;
        if input_images == 0 || registered_images == 0 || points_3d == 0 {
            return Err(SplatError::Process(
                "稀疏重建没有可用的注册图像或三维点".into(),
            ));
        }
        let registered_ratio = registered_images as f64 / input_images as f64;
        let quality = if registered_ratio >= GOOD_REGISTERED_RATIO {
            ReconstructionQuality::Good
        } else {
            ReconstructionQuality::Warning
        };
        Ok(ReconstructionReport {
            input_images,
            registered_images,
            registered_ratio,
            points_3d,
            quality,
        })
    }
}

fn count_input_images(directory: &Path) -> Result<u64> {
    let mut count = 0;
    for entry in std::fs::read_dir(directory)? {
        let path = entry?.path();
        if path.extension().is_some_and(|ext| {
            ext.eq_ignore_ascii_case("jpg")
                || ext.eq_ignore_ascii_case("jpeg")
                || ext.eq_ignore_ascii_case("png")
        }) {
            count += 1;
        }
    }
    Ok(count)
}

fn read_colmap_count(path: &Path) -> Result<u64> {
    let mut file = std::fs::File::open(path)?;
    let mut bytes = [0_u8; 8];
    file.read_exact(&mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}

/// Rejects non-finite poses, points, reprojection errors, and obviously invalid
/// camera intrinsics before a sparse model is allowed to reach Brush.
pub fn validate_sparse_geometry(model: &Path) -> Result<bool> {
    validate_cameras(&model.join("cameras.bin"))
        .and_then(|valid| {
            if valid {
                validate_images(&model.join("images.bin"))
            } else {
                Ok(false)
            }
        })
        .and_then(|valid| {
            if valid {
                validate_points(&model.join("points3D.bin"))
            } else {
                Ok(false)
            }
        })
}

fn validate_cameras(path: &Path) -> Result<bool> {
    let mut reader = BufReader::new(std::fs::File::open(path)?);
    let count = read_u64(&mut reader)?;
    for _ in 0..count {
        let _camera_id = read_u32(&mut reader)?;
        let model_id = read_i32(&mut reader)?;
        let width = read_u64(&mut reader)?;
        let height = read_u64(&mut reader)?;
        if width == 0 || height == 0 {
            return Ok(false);
        }
        let Some(params) = camera_parameter_count(model_id) else {
            return Ok(false);
        };
        let mut values = Vec::with_capacity(params);
        for _ in 0..params {
            values.push(read_f64(&mut reader)?);
        }
        if values.iter().any(|value| !value.is_finite())
            || values.first().is_none_or(|focal| *focal <= 0.0)
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn validate_images(path: &Path) -> Result<bool> {
    let mut reader = BufReader::new(std::fs::File::open(path)?);
    let count = read_u64(&mut reader)?;
    for _ in 0..count {
        let _image_id = read_u32(&mut reader)?;
        for _ in 0..7 {
            if !read_f64(&mut reader)?.is_finite() {
                return Ok(false);
            }
        }
        let _camera_id = read_u32(&mut reader)?;
        loop {
            let mut byte = [0_u8; 1];
            reader.read_exact(&mut byte)?;
            if byte[0] == 0 {
                break;
            }
        }
        let points = read_u64(&mut reader)?;
        let bytes = points
            .checked_mul(24)
            .ok_or_else(|| SplatError::Process("Invalid COLMAP image observations".into()))?;
        reader.seek(SeekFrom::Current(bytes as i64))?;
    }
    Ok(true)
}

fn validate_points(path: &Path) -> Result<bool> {
    let mut reader = BufReader::new(std::fs::File::open(path)?);
    let count = read_u64(&mut reader)?;
    for _ in 0..count {
        let _point_id = read_u64(&mut reader)?;
        for _ in 0..3 {
            if !read_f64(&mut reader)?.is_finite() {
                return Ok(false);
            }
        }
        let mut rgb = [0_u8; 3];
        reader.read_exact(&mut rgb)?;
        let error = read_f64(&mut reader)?;
        if !error.is_finite() || error < 0.0 {
            return Ok(false);
        }
        let track = read_u64(&mut reader)?;
        let bytes = track
            .checked_mul(8)
            .ok_or_else(|| SplatError::Process("Invalid COLMAP point track".into()))?;
        reader.seek(SeekFrom::Current(bytes as i64))?;
    }
    Ok(true)
}

fn camera_parameter_count(model_id: i32) -> Option<usize> {
    Some(match model_id {
        0 => 3,
        1 => 4,
        2 => 4,
        3 => 5,
        4 => 8,
        5 => 8,
        6 => 12,
        7 => 5,
        8 => 4,
        9 => 5,
        10 => 12,
        _ => return None,
    })
}

fn read_u64(reader: &mut impl Read) -> Result<u64> {
    let mut bytes = [0_u8; 8];
    reader.read_exact(&mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}
fn read_u32(reader: &mut impl Read) -> Result<u32> {
    let mut bytes = [0_u8; 4];
    reader.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}
fn read_i32(reader: &mut impl Read) -> Result<i32> {
    let mut bytes = [0_u8; 4];
    reader.read_exact(&mut bytes)?;
    Ok(i32::from_le_bytes(bytes))
}
fn read_f64(reader: &mut impl Read) -> Result<f64> {
    let mut bytes = [0_u8; 8];
    reader.read_exact(&mut bytes)?;
    Ok(f64::from_le_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_nonzero_registration_ratios_can_continue() {
        assert_eq!(classify(0.8), ReconstructionQuality::Good);
        assert_eq!(classify(0.5), ReconstructionQuality::Warning);
        assert_eq!(classify(0.499), ReconstructionQuality::Warning);
        assert_eq!(classify(0.001), ReconstructionQuality::Warning);
    }

    #[test]
    fn counts_jpeg_and_png_input_images() {
        let temporary = tempfile::tempdir().unwrap();
        std::fs::write(temporary.path().join("frame_1.jpg"), b"jpg").unwrap();
        std::fs::write(temporary.path().join("frame_2.jpeg"), b"jpeg").unwrap();
        std::fs::write(temporary.path().join("frame_3.png"), b"png").unwrap();
        std::fs::write(temporary.path().join("notes.txt"), b"text").unwrap();
        assert_eq!(count_input_images(temporary.path()).unwrap(), 3);
    }

    fn classify(ratio: f64) -> ReconstructionQuality {
        if ratio >= GOOD_REGISTERED_RATIO {
            ReconstructionQuality::Good
        } else {
            ReconstructionQuality::Warning
        }
    }
}
