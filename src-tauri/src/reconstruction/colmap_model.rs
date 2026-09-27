use std::{
    fs::File,
    io::{BufReader, Read},
    path::{Path, PathBuf},
};

use crate::error::{Result, SplatError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColmapCamera {
    pub id: u32,
    pub model_id: i32,
    pub model: String,
    pub width: u64,
    pub height: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredImage {
    pub id: u32,
    pub camera_id: u32,
    pub name: String,
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

fn read_u64(reader: &mut impl Read) -> Result<u64> {
    let mut bytes = [0_u8; 8];
    reader.read_exact(&mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}

fn skip(reader: &mut impl Read, bytes: u64) -> Result<()> {
    let copied = std::io::copy(&mut reader.take(bytes), &mut std::io::sink())?;
    if copied != bytes {
        return Err(SplatError::Process(
            "COLMAP 模型文件提前结束，内容可能已损坏".into(),
        ));
    }
    Ok(())
}

fn camera_model(model_id: i32) -> Option<(&'static str, u64)> {
    Some(match model_id {
        0 => ("SIMPLE_PINHOLE", 3),
        1 => ("PINHOLE", 4),
        2 => ("SIMPLE_RADIAL", 4),
        3 => ("RADIAL", 5),
        4 => ("OPENCV", 8),
        5 => ("OPENCV_FISHEYE", 8),
        6 => ("FULL_OPENCV", 12),
        7 => ("FOV", 5),
        8 => ("SIMPLE_RADIAL_FISHEYE", 4),
        9 => ("RADIAL_FISHEYE", 5),
        10 => ("THIN_PRISM_FISHEYE", 12),
        11 => ("RAD_TAN_THIN_PRISM_FISHEYE", 16),
        _ => return None,
    })
}

pub fn read_single_camera(model: &Path) -> Result<ColmapCamera> {
    let path = model.join("cameras.bin");
    let mut reader = BufReader::new(File::open(&path)?);
    let count = read_u64(&mut reader)?;
    if count != 1 {
        return Err(SplatError::Process(format!(
            "高清补拍要求原项目只使用一台共享相机，当前模型包含 {count} 台相机"
        )));
    }
    let id = read_u32(&mut reader)?;
    let model_id = read_i32(&mut reader)?;
    let width = read_u64(&mut reader)?;
    let height = read_u64(&mut reader)?;
    let (model_name, parameter_count) = camera_model(model_id).ok_or_else(|| {
        SplatError::Process(format!("原项目使用了暂不支持的相机模型编号 {model_id}"))
    })?;
    skip(&mut reader, parameter_count * 8)?;
    Ok(ColmapCamera {
        id,
        model_id,
        model: model_name.into(),
        width,
        height,
    })
}

pub fn read_registered_images(model: &Path) -> Result<Vec<RegisteredImage>> {
    let path = model.join("images.bin");
    let mut reader = BufReader::new(File::open(&path)?);
    let count = read_u64(&mut reader)?;
    let capacity = usize::try_from(count)
        .map_err(|_| SplatError::Process("COLMAP 图像数量超出可读取范围".into()))?;
    let mut images = Vec::with_capacity(capacity);
    for _ in 0..count {
        let id = read_u32(&mut reader)?;
        skip(&mut reader, 7 * 8)?;
        let camera_id = read_u32(&mut reader)?;
        let mut name = Vec::new();
        loop {
            let mut byte = [0_u8; 1];
            reader.read_exact(&mut byte)?;
            if byte[0] == 0 {
                break;
            }
            name.push(byte[0]);
            if name.len() > 32 * 1024 {
                return Err(SplatError::Process("COLMAP 图像名称异常过长".into()));
            }
        }
        let point_count = read_u64(&mut reader)?;
        skip(&mut reader, point_count.saturating_mul(24))?;
        images.push(RegisteredImage {
            id,
            camera_id,
            name: String::from_utf8(name)
                .map_err(|_| SplatError::Process("COLMAP 图像名称不是有效 UTF-8".into()))?,
        });
    }
    Ok(images)
}

pub fn required_model_files(model: &Path) -> [PathBuf; 3] {
    [
        model.join("cameras.bin"),
        model.join("images.bin"),
        model.join("points3D.bin"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn reads_one_camera_and_registered_image_names() {
        let root = tempfile::tempdir().unwrap();
        let mut cameras = File::create(root.path().join("cameras.bin")).unwrap();
        cameras.write_all(&1_u64.to_le_bytes()).unwrap();
        cameras.write_all(&7_u32.to_le_bytes()).unwrap();
        cameras.write_all(&2_i32.to_le_bytes()).unwrap();
        cameras.write_all(&1920_u64.to_le_bytes()).unwrap();
        cameras.write_all(&1080_u64.to_le_bytes()).unwrap();
        cameras.write_all(&[0_u8; 32]).unwrap();

        let mut images = File::create(root.path().join("images.bin")).unwrap();
        images.write_all(&1_u64.to_le_bytes()).unwrap();
        images.write_all(&3_u32.to_le_bytes()).unwrap();
        images.write_all(&[0_u8; 56]).unwrap();
        images.write_all(&7_u32.to_le_bytes()).unwrap();
        images.write_all(b"frame_000001.jpg\0").unwrap();
        images.write_all(&0_u64.to_le_bytes()).unwrap();

        let camera = read_single_camera(root.path()).unwrap();
        assert_eq!((camera.id, camera.width, camera.height), (7, 1920, 1080));
        assert_eq!(camera.model, "SIMPLE_RADIAL");
        assert_eq!(
            read_registered_images(root.path()).unwrap()[0].name,
            "frame_000001.jpg"
        );
    }
}
