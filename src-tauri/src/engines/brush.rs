use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use crate::{
    error::{Result, SplatError},
    presets::ResolvedBrushBudget,
    process::{ProcessManager, ProcessObserver, ProcessSpec},
};

pub fn require_verified_cli(executable: &Path) -> Result<()> {
    if executable.is_file() {
        Ok(())
    } else {
        Err(SplatError::EngineMissing(executable.display().to_string()))
    }
}

pub async fn train(
    executable: &Path,
    dataset: &Path,
    output_directory: &Path,
    budget: ResolvedBrushBudget,
    log_path: PathBuf,
    manager: &ProcessManager,
    observer: Option<ProcessObserver>,
) -> Result<PathBuf> {
    tokio::fs::create_dir_all(output_directory).await?;
    let candidate = output_directory.join("final.ply.tmp");
    if candidate.exists() {
        tokio::fs::remove_file(&candidate).await?;
    }
    let output = manager
        .run(ProcessSpec {
            executable: executable.to_path_buf(),
            args: vec![
                OsString::from("--total-steps"),
                budget.iterations.to_string().into(),
                OsString::from("--max-resolution"),
                budget.max_resolution.to_string().into(),
                OsString::from("--export-every"),
                budget.iterations.to_string().into(),
                OsString::from("--export-path"),
                output_directory.into(),
                OsString::from("--export-name"),
                OsString::from("final.ply.tmp"),
                dataset.into(),
            ],
            working_directory: Some(output_directory.to_path_buf()),
            log_path: Some(log_path),
            observer,
        })
        .await?;
    if !output.success {
        let detail = output.failure_detail();
        return Err(SplatError::Process(format!(
            "Brush 退出码 {:?}{}",
            output.exit_code,
            if detail.is_empty() {
                String::new()
            } else {
                format!("\n{detail}")
            }
        )));
    }
    let candidate = if candidate.is_file() {
        candidate
    } else {
        let alternate = output_directory.join("final.ply.tmp.ply");
        if alternate.is_file() {
            alternate
        } else {
            candidate
        }
    };
    if !candidate.is_file() {
        return Err(SplatError::Process(format!(
            "Brush 未生成预期文件：{}",
            candidate.display()
        )));
    }
    Ok(candidate)
}

/// Camera parameters and RGB dimensions must always use the same scale. Brush
/// applies this transform internally when `--max-resolution` downsizes the
/// dataset; this helper documents and verifies the contract at our boundary.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraIntrinsics {
    pub width: u32,
    pub height: u32,
    pub fx: f64,
    pub fy: f64,
    pub cx: f64,
    pub cy: f64,
}

impl CameraIntrinsics {
    pub fn scaled_to(self, width: u32, height: u32) -> Self {
        let scale_x = width as f64 / self.width.max(1) as f64;
        let scale_y = height as f64 / self.height.max(1) as f64;
        Self {
            width,
            height,
            fx: self.fx * scale_x,
            fy: self.fy * scale_y,
            cx: self.cx * scale_x,
            cy: self.cy * scale_y,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_intrinsics_follow_training_image_scale() {
        let camera = CameraIntrinsics {
            width: 3_840,
            height: 2_160,
            fx: 3_000.0,
            fy: 3_020.0,
            cx: 1_920.0,
            cy: 1_080.0,
        };
        let scaled = camera.scaled_to(1_920, 1_080);
        assert_eq!(scaled.fx, 1_500.0);
        assert_eq!(scaled.fy, 1_510.0);
        assert_eq!(scaled.cx, 960.0);
        assert_eq!(scaled.cy, 540.0);
    }
}
