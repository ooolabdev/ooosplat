use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use crate::{
    error::{Result, SplatError},
    presets::BrushTrainingPreset,
    process::{ProcessManager, ProcessObserver, ProcessSpec},
};

fn train_args(
    dataset: &Path,
    output_directory: &Path,
    preset: BrushTrainingPreset,
) -> Vec<OsString> {
    let mut args = vec![
        OsString::from("--total-steps"),
        preset.total_steps.to_string().into(),
        OsString::from("--max-resolution"),
        preset.max_resolution.to_string().into(),
        OsString::from("--refine-every"),
        preset.refine_every.to_string().into(),
    ];
    if let Some(max_splats) = preset.max_splats {
        args.extend([
            OsString::from("--max-splats"),
            max_splats.to_string().into(),
        ]);
    }
    if let Some(densification) = preset.densification {
        args.extend([
            OsString::from("--growth-grad-threshold"),
            densification.growth_grad_threshold.to_string().into(),
            OsString::from("--growth-select-fraction"),
            densification.growth_select_fraction.to_string().into(),
            OsString::from("--growth-stop-iter"),
            densification.growth_stop_iter.to_string().into(),
        ]);
    }
    args.extend([
        OsString::from("--export-every"),
        preset.total_steps.to_string().into(),
        OsString::from("--export-path"),
        output_directory.into(),
        OsString::from("--export-name"),
        OsString::from("final.ply.tmp"),
        dataset.into(),
    ]);
    args
}

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
    preset: BrushTrainingPreset,
    log_path: PathBuf,
    manager: &ProcessManager,
    observer: Option<ProcessObserver>,
) -> Result<PathBuf> {
    tokio::fs::create_dir_all(output_directory).await?;
    let candidate = output_directory.join("final.ply.tmp");
    let alternate = output_directory.join("final.ply.tmp.ply");
    for partial in [&candidate, &alternate] {
        if partial.exists() {
            tokio::fs::remove_file(partial).await?;
        }
    }
    let output = manager
        .run(ProcessSpec {
            executable: executable.to_path_buf(),
            args: train_args(dataset, output_directory, preset),
            working_directory: Some(output_directory.to_path_buf()),
            log_path: Some(log_path),
            observer,
        })
        .await?;
    if !output.success {
        let detail = output.failure_detail();
        if is_out_of_memory_detail(&detail) {
            return Err(SplatError::BrushOutOfMemory(detail));
        }
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

fn is_out_of_memory_detail(detail: &str) -> bool {
    let normalized = detail.to_ascii_lowercase();
    [
        "out of memory",
        "outofmemory",
        "buffertoobig",
        "buffer too big",
        "failed to allocate",
        "allocation failed",
    ]
    .iter()
    .any(|token| normalized.contains(token))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presets::{resolve_brush_training_preset, Quality};

    fn args_as_strings(args: Vec<OsString>) -> Vec<String> {
        args.into_iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn default_training_does_not_override_densification() {
        let args = args_as_strings(train_args(
            Path::new("dataset"),
            Path::new("output"),
            resolve_brush_training_preset(Quality::Balanced, false, None, 1_600, 0).preset,
        ));

        assert!(!args.iter().any(|arg| arg == "--growth-select-fraction"));
        assert!(!args.iter().any(|arg| arg == "--growth-stop-iter"));
        assert!(!args.iter().any(|arg| arg == "--max-splats"));
    }

    #[test]
    fn high_profile_passes_all_bounded_training_overrides() {
        let preset =
            resolve_brush_training_preset(Quality::High, true, Some(8_192), 3_840, 60_000).preset;
        let args = args_as_strings(train_args(
            Path::new("dataset"),
            Path::new("output"),
            preset,
        ));

        assert!(args
            .windows(2)
            .any(|pair| pair == ["--growth-grad-threshold", "0.00002"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--growth-select-fraction", "0.3"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--growth-stop-iter", "25000"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--refine-every", "200"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--max-splats", "1500000"]));
    }

    #[test]
    fn fast_and_balanced_pass_efficiency_densification_profiles() {
        let fast = args_as_strings(train_args(
            Path::new("dataset"),
            Path::new("output"),
            resolve_brush_training_preset(Quality::Fast, true, Some(8_192), 3_840, 0).preset,
        ));
        assert!(fast
            .windows(2)
            .any(|pair| pair == ["--growth-select-fraction", "0.15"]));
        assert!(fast
            .windows(2)
            .any(|pair| pair == ["--growth-stop-iter", "6000"]));

        let balanced = args_as_strings(train_args(
            Path::new("dataset"),
            Path::new("output"),
            resolve_brush_training_preset(Quality::Balanced, true, Some(8_192), 3_840, 0).preset,
        ));
        assert!(balanced
            .windows(2)
            .any(|pair| pair == ["--growth-grad-threshold", "0.00003"]));
        assert!(balanced
            .windows(2)
            .any(|pair| pair == ["--growth-stop-iter", "12000"]));
        assert!(!balanced.iter().any(|arg| arg == "--max-splats"));
    }

    #[test]
    fn oom_classifier_is_specific_and_does_not_retry_generic_device_loss() {
        assert!(is_out_of_memory_detail("OutOfMemory while allocating"));
        assert!(is_out_of_memory_detail("BufferTooBig(2290420416)"));
        assert!(is_out_of_memory_detail("GPU allocation failed"));
        assert!(!is_out_of_memory_detail("DeviceLost: driver reset"));
        assert!(!is_out_of_memory_detail("process exited with code 1"));
    }
}
