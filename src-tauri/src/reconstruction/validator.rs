use std::{io::Read, path::Path};

use serde::{Deserialize, Serialize};

use crate::error::{Result, SplatError};

pub const GOOD_REGISTERED_RATIO: f64 = 0.80;
pub const WARNING_REGISTERED_RATIO: f64 = 0.50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReconstructionQuality {
    Good,
    Warning,
    Failed,
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
        } else if registered_ratio >= WARNING_REGISTERED_RATIO {
            ReconstructionQuality::Warning
        } else {
            ReconstructionQuality::Failed
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thresholds_match_product_rules() {
        assert_eq!(classify(0.8), ReconstructionQuality::Good);
        assert_eq!(classify(0.5), ReconstructionQuality::Warning);
        assert_eq!(classify(0.499), ReconstructionQuality::Failed);
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
        } else if ratio >= WARNING_REGISTERED_RATIO {
            ReconstructionQuality::Warning
        } else {
            ReconstructionQuality::Failed
        }
    }
}
