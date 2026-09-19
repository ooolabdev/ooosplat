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

/// Builds the Brush training command line.
///
/// Only flags the shipped Brush 0.3.0 binary really exposes are emitted; see
/// `docs/brush_help.txt` (produced by `scripts/probe_brush_help.sh`) for the
/// authoritative list.
///
/// A preset whose tuning is `BrushTuning::brush_defaults()` emits nothing but the
/// original flags, so the common tiers behave exactly as they did before the
/// tuning existed. Optional knobs are omitted entirely rather than sent empty.
fn training_args(preset: QualityPreset, output_directory: &Path, dataset: &Path) -> Vec<OsString> {
    let tuning = preset.brush_tuning;
    // Intermediate exports copy the whole splat set from device to host, so the
    // tuned preset exports once at the end. Untuned presets keep the previous
    // behaviour of exporting as training refines.
    let export_every = if tuning.single_export {
        preset.brush_iterations
    } else {
        (preset.brush_iterations / 10).clamp(500, 2_000)
    };
    let mut args = vec![
        OsString::from("--total-steps"),
        preset.brush_iterations.to_string().into(),
        OsString::from("--max-resolution"),
        preset.brush_max_resolution.to_string().into(),
        OsString::from("--export-every"),
        export_every.to_string().into(),
        OsString::from("--export-path"),
        output_directory.into(),
        OsString::from("--export-name"),
        OsString::from("final.ply.tmp"),
    ];
    if let Some(sh_degree) = tuning.sh_degree {
        args.push(OsString::from("--sh-degree"));
        args.push(sh_degree.to_string().into());
    }
    if let Some(growth_stop_iter) = tuning.growth_stop_iter {
        args.push(OsString::from("--growth-stop-iter"));
        args.push(growth_stop_iter.to_string().into());
    }
    if let Some(refine_every) = tuning.refine_every {
        args.push(OsString::from("--refine-every"));
        args.push(refine_every.to_string().into());
    }
    if let Some(max_splats) = tuning.max_splats {
        args.push(OsString::from("--max-splats"));
        args.push(max_splats.to_string().into());
    }
    // The dataset stays last: a leading positional argument would be parsed as
    // the source path instead of a flag.
    args.push(dataset.into());
    args
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
            args: training_args(preset, output_directory, dataset),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presets::Quality;

    fn args(preset: QualityPreset) -> Vec<String> {
        training_args(preset, Path::new("out"), Path::new("dataset/dense"))
            .into_iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    fn value_of(args: &[String], flag: &str) -> Option<String> {
        args.iter()
            .position(|arg| arg == flag)
            .and_then(|index| args.get(index + 1))
            .cloned()
    }

    #[test]
    fn untuned_presets_pass_no_tuning_flags() {
        // Fast and Balanced must behave exactly as they did before the tuning
        // existed, so every optional flag stays off the command line entirely.
        for quality in [Quality::Fast, Quality::Balanced] {
            let preset = quality.preset();
            assert!(
                preset.brush_tuning.is_brush_defaults(),
                "{quality:?} should be untuned"
            );
            let args = args(preset);
            for flag in [
                "--sh-degree",
                "--growth-stop-iter",
                "--refine-every",
                "--max-splats",
            ] {
                assert!(
                    !args.iter().any(|arg| arg == flag),
                    "{quality:?} must not pass {flag}"
                );
            }
            // Untuned presets keep the intermediate export interval, so the
            // export cadence stays shorter than the whole training run.
            assert_ne!(
                value_of(&args, "--total-steps"),
                value_of(&args, "--export-every"),
                "{quality:?} must keep exporting as training refines"
            );
            assert!(
                value_of(&args, "--export-every")
                    .and_then(|value| value.parse::<usize>().ok())
                    .is_some_and(|every| every < preset.brush_iterations),
                "{quality:?} export interval must stay below the total step count"
            );
        }
    }

    #[test]
    fn only_the_high_preset_uses_the_tuning() {
        let high = Quality::High.preset();
        assert!(!high.brush_tuning.is_brush_defaults());
        let args = args(high);
        assert_eq!(value_of(&args, "--sh-degree").as_deref(), Some("2"));
        assert_eq!(
            value_of(&args, "--growth-stop-iter").as_deref(),
            Some("12000")
        );
        // No measurement justifies overriding these two, so they stay unset.
        assert!(!args.iter().any(|arg| arg == "--refine-every"));
        assert!(!args.iter().any(|arg| arg == "--max-splats"));
        // The tuned preset exports once, at the end of training.
        assert_eq!(
            value_of(&args, "--export-every"),
            value_of(&args, "--total-steps")
        );
    }

    #[test]
    fn tuning_never_extends_densification_or_training() {
        for quality in [Quality::Fast, Quality::Balanced, Quality::High] {
            let preset = quality.preset();
            if let Some(stop) = preset.brush_tuning.growth_stop_iter {
                // Brush stops growth at 15000 by default; asking for a later stop
                // would extend the slowest part of training.
                assert!(stop <= 15_000, "{quality:?} would extend densification");
                assert!(
                    stop <= preset.brush_iterations,
                    "{quality:?} stops growth after training already ended"
                );
            }
            if let Some(degree) = preset.brush_tuning.sh_degree {
                // Brush defaults to degree 3; the tuning only ever lowers it.
                assert!(degree <= 3, "{quality:?} raised the SH degree");
            }
        }
    }

    #[test]
    fn export_name_and_dataset_stay_intact() {
        for quality in [Quality::Fast, Quality::Balanced, Quality::High] {
            let args = args(quality.preset());
            assert_eq!(
                value_of(&args, "--export-name").as_deref(),
                Some("final.ply.tmp")
            );
            assert_eq!(value_of(&args, "--export-path").as_deref(), Some("out"));
            assert_eq!(args.last().map(String::as_str), Some("dataset/dense"));
        }
    }
}
