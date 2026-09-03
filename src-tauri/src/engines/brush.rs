use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use crate::{
    error::{Result, SplatError},
    presets::QualityPreset,
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
    preset: QualityPreset,
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
                preset.brush_iterations.to_string().into(),
                OsString::from("--max-resolution"),
                preset.brush_max_resolution.to_string().into(),
                OsString::from("--max-splats"),
                preset.brush_max_splats.to_string().into(),
                OsString::from("--sh-degree"),
                preset.brush_sh_degree.to_string().into(),
                OsString::from("--export-every"),
                preset.brush_iterations.to_string().into(),
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
        let detail = process_error_detail(&output.stdout, &output.stderr);
        return Err(SplatError::Process(format!(
            "Brush 退出码 {:?}：{detail}",
            output.exit_code,
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

fn process_error_detail(stdout: &str, stderr: &str) -> String {
    stderr
        .lines()
        .chain(stdout.lines())
        .rev()
        .find(|line| !line.trim().is_empty())
        .map(str::trim)
        .unwrap_or("未提供错误详情")
        .chars()
        .take(500)
        .collect()
}

pub fn is_out_of_memory(error: &SplatError) -> bool {
    let normalized = error.to_string().to_ascii_lowercase();
    [
        "out of memory",
        "outofmemory",
        "buffer too big",
        "buffertoobig",
        "allocation failed",
        "device lost",
    ]
    .iter()
    .any(|needle| normalized.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_common_webgpu_oom_messages() {
        assert!(is_out_of_memory(&SplatError::Process(
            "Brush: BufferTooBig while allocating".into()
        )));
        assert!(!is_out_of_memory(&SplatError::Process(
            "invalid dataset".into()
        )));
    }
}
