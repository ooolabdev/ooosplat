use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use crate::{
    engines::ColmapAccelerationStatus,
    error::{Result, SplatError},
    presets::BrushTrainingPreset,
    process::{ProcessManager, ProcessObserver, ProcessOutput, ProcessSpec},
};

const BRUSH_GPU_LOG_FILTER: &str =
    "brush_cli=info,brush_process=info,cubecl_wgpu=info,burn_wgpu=info";

/// Capabilities verified against OOOBrush 866a7d65b4d6a591ba0d52a88f8b20ea3465c1e5.
/// Its COLMAP reader and renderer both apply these distortion models. A matching
/// name/version string is not sufficient: custom runtimes retain conversion.
pub(crate) fn supports_native_colmap(executable: &Path, model_id: u32) -> bool {
    if !matches!(model_id, 0 | 1 | 2 | 3 | 4 | 6) {
        return false;
    }
    type Identity = (PathBuf, u64, std::time::SystemTime);
    static CACHE: std::sync::OnceLock<std::sync::Mutex<Option<(Identity, bool)>>> =
        std::sync::OnceLock::new();
    let Ok(path) = std::fs::canonicalize(executable) else {
        return false;
    };
    let Ok(metadata) = std::fs::metadata(&path) else {
        return false;
    };
    let Ok(modified) = metadata.modified() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    let identity = (path.clone(), metadata.len(), modified);
    let mut cache = CACHE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    if let Some((previous, verified)) = cache.as_ref() {
        if *previous == identity {
            return *verified;
        }
    }
    let verified = verified_native_binary(&path);
    *cache = Some((identity, verified));
    verified
}

fn verified_native_binary(path: &Path) -> bool {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    // Do not automatically widen capability claims when runtime manifests are
    // updated. Review the exact reader/renderer before adding another digest.
    let expected = if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        "f65bba1c7fcbdbb13e9299897ecb885f90f30555bee9452a526e372eacf338df"
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "ca819f94716c04add531e69e5f6812a2d1e9a8cefbc2803da000d905d4fccb0a"
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "da613b4ed1c551f41e448d8ab0dc9caf0f80bfd9f6943542e1b02c93d19ca74a"
    } else {
        return false;
    };
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => hash.update(&buffer[..read]),
            Err(_) => return false,
        }
    }
    format!("{:x}", hash.finalize()) == expected
}
pub(super) const REQUIRED_CLI_FLAGS: &[&str] = &[
    "--total-train-iters",
    "--max-resolution",
    "--refine-every",
    "--max-splats",
    "--growth-grad-threshold",
    "--growth-select-fraction",
    "--growth-stop-iter",
    "--growth-start-iter",
    "--sh-degree",
    "--export-every",
    "--export-path",
    "--export-name",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrushGpuLaunchPolicy {
    expected_device_name: Option<String>,
    force_first_discrete_gpu: bool,
    disable_amd_switchable_graphics: bool,
    selection_reason: &'static str,
}

pub struct BrushTrainingOptions<'a> {
    pub init_sh_degree: Option<u32>,
    pub alpha_mode: Option<&'static str>,
    pub warm: Option<crate::pipeline::reshoot::WarmStartParameters>,
    pub preset: BrushTrainingPreset,
    pub panorama: bool,
    pub log_path: PathBuf,
    pub gpu_launch_policy: &'a BrushGpuLaunchPolicy,
}

impl BrushGpuLaunchPolicy {
    pub fn from_acceleration(acceleration: &ColmapAccelerationStatus) -> Self {
        Self::resolve(cfg!(windows), acceleration)
    }

    /// COLMAP's selected GPU is not necessarily Brush's default adapter in a
    /// multi-GPU topology. Do not give panoramas a large VRAM budget when the
    /// launch policy cannot identify the training device.
    pub fn known_training_memory_mb(&self, acceleration: &ColmapAccelerationStatus) -> Option<u64> {
        self.expected_device_name.as_ref().and_then(|expected| {
            acceleration
                .device
                .as_ref()
                .filter(|device| &device.name == expected)
                .and_then(|_| acceleration.usable_gpu_total_memory_mb())
        })
    }

    fn resolve(is_windows: bool, acceleration: &ColmapAccelerationStatus) -> Self {
        let selected = acceleration.device.as_ref();
        let unambiguous_single_nvidia = is_windows
            && acceleration.use_gpu()
            && acceleration.detected_nvidia_device_count == 1
            && selected.is_some_and(|device| device.index == 0);
        let selection_reason = if unambiguous_single_nvidia {
            "single_nvidia_discrete_gpu"
        } else if !is_windows {
            "non_windows_default"
        } else if !acceleration.use_gpu() {
            "no_compatible_nvidia_gpu"
        } else {
            "ambiguous_multi_gpu_topology"
        };

        Self {
            expected_device_name: if unambiguous_single_nvidia {
                selected.map(|device| device.name.clone())
            } else {
                None
            },
            force_first_discrete_gpu: unambiguous_single_nvidia,
            disable_amd_switchable_graphics: unambiguous_single_nvidia,
            selection_reason,
        }
    }

    fn environment(&self) -> Vec<(OsString, OsString)> {
        self.environment_with_rust_log(std::env::var_os("RUST_LOG"))
    }

    fn environment_with_rust_log(
        &self,
        existing_rust_log: Option<OsString>,
    ) -> Vec<(OsString, OsString)> {
        let mut environment = Vec::new();
        if self.force_first_discrete_gpu {
            environment.push((
                OsString::from("CUBECL_WGPU_DEFAULT_DEVICE"),
                OsString::from("DiscreteGpu(0)"),
            ));
        }
        if self.disable_amd_switchable_graphics {
            environment.push((
                OsString::from("DISABLE_LAYER_AMD_SWITCHABLE_GRAPHICS_1"),
                OsString::from("1"),
            ));
        }
        let rust_log = match existing_rust_log {
            Some(value) if !value.is_empty() => {
                let mut combined = value;
                combined.push(",");
                combined.push(BRUSH_GPU_LOG_FILTER);
                combined
            }
            _ => OsString::from(BRUSH_GPU_LOG_FILTER),
        };
        environment.push((OsString::from("RUST_LOG"), rust_log));
        if std::env::var_os("RUST_BACKTRACE").is_none() {
            environment.push((OsString::from("RUST_BACKTRACE"), OsString::from("1")));
        }
        environment
    }

    pub fn log_summary(&self) -> String {
        format!(
            "backend=Vulkan selection={} expectedDevice={} cubeclDevice={} amdSwitchableLayerDisabled={}",
            self.selection_reason,
            self.expected_device_name.as_deref().unwrap_or("unverified"),
            if self.force_first_discrete_gpu {
                "DiscreteGpu(0)"
            } else {
                "automatic"
            },
            self.disable_amd_switchable_graphics
        )
    }
}

fn train_args(
    dataset: &Path,
    output_directory: &Path,
    preset: BrushTrainingPreset,
) -> Vec<OsString> {
    let mut args = vec![
        OsString::from("--total-train-iters"),
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
    if dataset.join("manifest.json").is_file() && dataset.join("signature.json").is_file() {
        args.extend(["--max-scene-batch-cache-size".into(), "2GiB".into()]);
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

fn train_args_for_mode(
    dataset: &Path,
    output_directory: &Path,
    preset: BrushTrainingPreset,
    panorama: bool,
) -> Vec<OsString> {
    let mut args = train_args(dataset, output_directory, preset);
    if panorama {
        // Pin these defaults for the versioned panorama recipe only.
        args.splice(
            0..0,
            ["--growth-start-iter", "0", "--sh-degree", "3"].map(OsString::from),
        );
    }
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
    options: BrushTrainingOptions<'_>,
    manager: &ProcessManager,
    observer: Option<ProcessObserver>,
) -> Result<PathBuf> {
    if manager.child_token().is_cancelled() {
        return Err(SplatError::Cancelled);
    }
    tokio::fs::create_dir_all(output_directory).await.map_err(SplatError::from)
        .map_err(|error| error.with_io_diagnostic(
            "training_preparation_operation=prepare_brush_output\npath_role=brush_output\nio_substep=create_directory"
        ))?;
    let candidate = output_directory.join("final.ply.tmp");
    let alternate = output_directory.join("final.ply.tmp.ply");
    for partial in [&candidate, &alternate] {
        match tokio::fs::symlink_metadata(partial).await {
            Ok(_) => tokio::fs::remove_file(partial).await.map_err(SplatError::from)
                .map_err(|error| error.with_io_diagnostic(
                    "training_preparation_operation=prepare_brush_output\npath_role=brush_candidate\nio_substep=remove_incomplete_candidate"
                ))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
            Err(error) => return Err(SplatError::Io(error).with_io_diagnostic(
                "training_preparation_operation=prepare_brush_output\npath_role=brush_candidate\nio_substep=inspect_incomplete_candidate"
            )),
        }
    }
    let environment = options.gpu_launch_policy.environment();
    let mut args = train_args_for_mode(
        dataset,
        output_directory,
        options.preset,
        options.panorama && options.warm.is_none(),
    );
    if let Some(degree) = options.init_sh_degree.filter(|_| options.warm.is_none()) {
        if degree > 3 {
            return Err(SplatError::Process(
                "Unsupported initialization SH degree".into(),
            ));
        }
        // Preserve the trusted initialization's SH without applying the legacy warm recipe.
        if let Some(index) = args.iter().position(|a| a == "--sh-degree") {
            args[index + 1] = degree.to_string().into();
        } else {
            args.extend(["--sh-degree".into(), degree.to_string().into()]);
        }
    }
    if let Some(mode) = options.alpha_mode {
        args.extend([OsString::from("--alpha-mode"), OsString::from(mode)]);
    }
    if let Some(warm) = options.warm {
        args.splice(
            0..0,
            [
                ("--growth-start-iter", warm.growth_start.to_string()),
                ("--sh-degree", warm.sh_degree.to_string()),
                ("--lr-mean", warm.lr_mean.to_string()),
                ("--lr-mean-end", warm.lr_mean_end.to_string()),
                ("--lr-coeffs-dc", warm.lr_coeffs_dc.to_string()),
                ("--lr-opac", warm.lr_opac.to_string()),
                ("--lr-scale", warm.lr_scale.to_string()),
                ("--lr-rotation", warm.lr_rotation.to_string()),
                ("--mean-noise-weight", warm.mean_noise_weight.to_string()),
            ]
            .into_iter()
            .flat_map(|(key, value)| [OsString::from(key), OsString::from(value)]),
        );
    }
    let last_splat_count = std::sync::Arc::new(std::sync::Mutex::new(None));
    let evidence = std::sync::Arc::new(std::sync::Mutex::new(
        crate::failure::EngineFailureEvidence::default(),
    ));
    let live_evidence = evidence.clone();
    let counts = last_splat_count.clone();
    let observer: ProcessObserver = std::sync::Arc::new(move |update| {
        if let crate::process::ProcessUpdate::Line { line, .. } = &update {
            live_evidence
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .observe(line);
            if let Some(count) = crate::pipeline::runtime::brush_splat_count(line) {
                *counts.lock().unwrap_or_else(|p| p.into_inner()) = Some(count);
            }
        }
        if let Some(observer) = &observer {
            observer(update);
        }
    });
    let output = manager
        .run_with_environment(
            ProcessSpec {
                executable: executable.to_path_buf(),
                args,
                working_directory: Some(output_directory.to_path_buf()),
                log_path: Some(options.log_path),
                observer: Some(observer),
            },
            &environment,
        )
        .await.map_err(|error| error.with_io_diagnostic(
            "training_preparation_operation=run_brush\npath_role=brush_process_io\nio_substep=execute_and_capture"
        ))?;
    resolve_train_output(&output, candidate, alternate).map_err(|mut error| {
        if let SplatError::EngineProcess(failure) = &mut error {
            **failure = crate::failure::EngineProcessFailure::from_output_with_evidence(
                crate::pipeline::PipelineEngine::Brush,
                "train",
                &output,
                &evidence.lock().unwrap_or_else(|p| p.into_inner()),
            );
            failure.last_splat_count = *last_splat_count.lock().unwrap_or_else(|p| p.into_inner());
        }
        error
    })
}

fn resolve_train_output(
    output: &ProcessOutput,
    candidate: PathBuf,
    alternate: PathBuf,
) -> Result<PathBuf> {
    if !output.success {
        return Err(SplatError::EngineProcess(Box::new(
            crate::failure::EngineProcessFailure::from_output(
                crate::pipeline::PipelineEngine::Brush,
                "train",
                output,
            ),
        )));
    }
    let is_file = |path: &Path| -> Result<bool> {
        match std::fs::metadata(path) {
            Ok(metadata) => Ok(metadata.is_file()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(SplatError::Io(error).with_io_diagnostic(
                "training_preparation_operation=validate_brush_output\npath_role=brush_candidate\nio_substep=read_metadata"
            )),
        }
    };
    let candidate = if is_file(&candidate)? {
        candidate
    } else if is_file(&alternate)? {
        alternate
    } else {
        return Err(SplatError::Process(format!(
            "Brush 未生成预期文件：{}",
            candidate.display()
        )));
    };
    Ok(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        engines::{
            AccelerationReasonCode, AccelerationRequirements, ColmapBackend, GpuDetectionState,
            GpuDeviceInfo,
        },
        presets::{resolve_brush_training_preset, Quality},
    };

    #[tokio::test]
    async fn training_output_preparation_failure_retains_io_context_without_starting_engine() {
        let root = tempfile::tempdir().unwrap();
        let output = root.path().join("not-a-directory");
        std::fs::write(&output, b"existing file").unwrap();
        let policy =
            BrushGpuLaunchPolicy::resolve(false, &acceleration(ColmapBackend::Cpu, 0, None));
        let result = train(
            &root.path().join("missing-brush.exe"),
            root.path(),
            &output,
            BrushTrainingOptions {
                init_sh_degree: None,
                alpha_mode: None,
                warm: None,
                preset: resolve_brush_training_preset(Quality::Fast, false, None, 128, 0).preset,
                panorama: false,
                log_path: root.path().join("brush.log"),
                gpu_launch_policy: &policy,
            },
            &ProcessManager::new(),
            None,
        )
        .await
        .unwrap_err();
        assert!(result.io_error().is_some());
        assert!(result.io_error().unwrap().raw_os_error().is_some());
        assert!(result
            .to_string()
            .contains("training_preparation_operation=prepare_brush_output"));
        assert!(result.to_string().contains("io_substep=create_directory"));
        assert!(!result.is_brush_oom());
        assert!(!root.path().join("brush.log").exists());
        assert_eq!(std::fs::read(output).unwrap(), b"existing file");
    }

    #[tokio::test]
    async fn cancelled_training_does_not_delete_candidate_or_replace_cancel_with_preparation_error()
    {
        let root = tempfile::tempdir().unwrap();
        let candidate = root.path().join("final.ply.tmp");
        std::fs::write(&candidate, b"existing candidate").unwrap();
        let policy =
            BrushGpuLaunchPolicy::resolve(false, &acceleration(ColmapBackend::Cpu, 0, None));
        let manager = ProcessManager::new();
        manager.cancel();
        let result = train(
            &root.path().join("missing-brush.exe"),
            root.path(),
            root.path(),
            BrushTrainingOptions {
                init_sh_degree: None,
                alpha_mode: None,
                warm: None,
                preset: resolve_brush_training_preset(Quality::Fast, false, None, 128, 0).preset,
                panorama: false,
                log_path: root.path().join("brush.log"),
                gpu_launch_policy: &policy,
            },
            &manager,
            None,
        )
        .await;
        assert!(matches!(result, Err(SplatError::Cancelled)));
        assert_eq!(std::fs::read(candidate).unwrap(), b"existing candidate");
        assert!(!root.path().join("brush.log").exists());
    }

    #[test]
    fn native_camera_capability_rejects_unknown_and_changed_runtime() {
        let temp = tempfile::tempdir().unwrap();
        let binary = temp.path().join("brush_app.exe");
        std::fs::write(&binary, b"brush-cli 1.0.0").unwrap();
        assert!(!supports_native_colmap(&binary, 2));
        std::fs::write(&binary, b"a replaced runtime with another identity").unwrap();
        assert!(!supports_native_colmap(&binary, 1));
        assert!(!supports_native_colmap(&binary, 17));
        assert!(!supports_native_colmap(&temp.path().join("missing"), 1));
    }

    fn args_as_strings(args: Vec<OsString>) -> Vec<String> {
        args.into_iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    fn acceleration(
        backend: ColmapBackend,
        count: usize,
        index: Option<u32>,
    ) -> ColmapAccelerationStatus {
        ColmapAccelerationStatus {
            backend,
            detection_state: if backend == ColmapBackend::Gpu {
                GpuDetectionState::Ready
            } else {
                GpuDetectionState::Unavailable
            },
            reason_code: if backend == ColmapBackend::Gpu {
                AccelerationReasonCode::GpuReady
            } else {
                AccelerationReasonCode::NoNvidiaGpu
            },
            reason: String::new(),
            device: index.map(|index| GpuDeviceInfo {
                index,
                name: "NVIDIA GeForce RTX 3060 Laptop GPU".into(),
                driver_version: "560.81".into(),
                compute_capability: "8.6".into(),
                total_memory_mb: Some(6_144),
            }),
            requirements: AccelerationRequirements {
                minimum_driver_version: "528.33".into(),
                minimum_compute_capability: "5.0".into(),
            },
            detected_nvidia_device_count: count,
        }
    }

    fn environment_value<'a>(
        environment: &'a [(OsString, OsString)],
        key: &str,
    ) -> Option<&'a str> {
        environment
            .iter()
            .find(|(candidate, _)| candidate == key)
            .and_then(|(_, value)| value.to_str())
    }

    #[test]
    fn windows_single_nvidia_policy_forces_first_discrete_gpu_for_brush_only() {
        let policy =
            BrushGpuLaunchPolicy::resolve(true, &acceleration(ColmapBackend::Gpu, 1, Some(0)));
        assert_eq!(
            policy.known_training_memory_mb(&acceleration(ColmapBackend::Gpu, 1, Some(0))),
            Some(6144)
        );
        let environment = policy.environment_with_rust_log(Some("app=debug".into()));

        assert_eq!(
            environment_value(&environment, "CUBECL_WGPU_DEFAULT_DEVICE"),
            Some("DiscreteGpu(0)")
        );
        assert_eq!(
            environment_value(&environment, "DISABLE_LAYER_AMD_SWITCHABLE_GRAPHICS_1"),
            Some("1")
        );
        assert_eq!(
            environment_value(&environment, "RUST_LOG"),
            Some("app=debug,brush_cli=info,brush_process=info,cubecl_wgpu=info,burn_wgpu=info")
        );
        assert!(!environment.iter().any(|(key, _)| key == "WGPU_BACKEND"));
        assert!(policy.log_summary().contains("backend=Vulkan"));
        assert!(policy
            .log_summary()
            .contains("expectedDevice=NVIDIA GeForce RTX 3060 Laptop GPU"));
    }

    #[test]
    fn ambiguous_or_cpu_topologies_keep_automatic_device_selection() {
        for status in [
            acceleration(ColmapBackend::Gpu, 2, Some(0)),
            acceleration(ColmapBackend::Gpu, 1, Some(1)),
            acceleration(ColmapBackend::Cpu, 0, None),
        ] {
            let policy = BrushGpuLaunchPolicy::resolve(true, &status);
            assert_eq!(policy.known_training_memory_mb(&status), None);
            let environment = policy.environment_with_rust_log(None);
            assert_eq!(
                environment_value(&environment, "CUBECL_WGPU_DEFAULT_DEVICE"),
                None
            );
            assert_eq!(
                environment_value(&environment, "DISABLE_LAYER_AMD_SWITCHABLE_GRAPHICS_1"),
                None
            );
            assert_eq!(
                environment_value(&environment, "RUST_LOG"),
                Some(BRUSH_GPU_LOG_FILTER)
            );
        }
    }

    #[test]
    fn non_windows_policy_never_forces_windows_gpu_workarounds() {
        let policy =
            BrushGpuLaunchPolicy::resolve(false, &acceleration(ColmapBackend::Gpu, 1, Some(0)));
        let environment = policy.environment_with_rust_log(None);
        assert_eq!(
            environment_value(&environment, "CUBECL_WGPU_DEFAULT_DEVICE"),
            None
        );
        assert_eq!(
            environment_value(&environment, "DISABLE_LAYER_AMD_SWITCHABLE_GRAPHICS_1"),
            None
        );
    }

    #[test]
    fn new_cli_preserves_iterations_exports_and_unicode_paths() {
        let dataset = Path::new("中文 数据集");
        let output = Path::new("中文 导出目录");
        let preset = resolve_brush_training_preset(Quality::Balanced, false, None, 1_600, 0).preset;
        let args = train_args(dataset, output, preset);
        assert_eq!(args.last(), Some(&dataset.as_os_str().to_owned()));
        assert!(args.windows(2).any(|pair| pair[0] == "--total-train-iters"
            && pair[1] == OsString::from(preset.total_steps.to_string())));
        assert!(args
            .windows(2)
            .any(|pair| pair[0] == "--export-path" && pair[1] == output.as_os_str()));
        assert!(args
            .windows(2)
            .any(|pair| pair[0] == "--export-name" && pair[1] == "final.ply.tmp"));
        assert!(!args
            .iter()
            .any(|arg| arg == "--total-steps" || arg == "--lod-levels"));
    }

    #[test]
    fn panorama_cli_pins_growth_start_and_sh_without_changing_ordinary_jobs() {
        for quality in [Quality::Fast, Quality::Balanced, Quality::High] {
            let r =
                crate::presets::resolve_panorama_training_preset(quality, false, None, 512, 10_192)
                    .unwrap();
            let dataset = Path::new("existing panorama");
            let output = Path::new("isolated output");
            let args = args_as_strings(train_args_for_mode(dataset, output, r.preset, true));
            assert_eq!(args.last().unwrap(), "existing panorama");
            for (flag, value) in [
                ("--growth-start-iter", "0".into()),
                ("--sh-degree", "3".into()),
                ("--total-train-iters", r.preset.total_steps.to_string()),
                ("--max-splats", r.preset.max_splats.unwrap().to_string()),
            ] {
                assert_eq!(args.iter().filter(|v| *v == flag).count(), 1);
                assert!(args.windows(2).any(|w| w[0] == flag && w[1] == value));
            }
            assert_eq!(
                train_args_for_mode(dataset, output, r.preset, false),
                train_args(dataset, output, r.preset)
            );
        }
    }

    #[test]
    fn completed_iterations_or_zero_exit_do_not_establish_successful_export() {
        let directory = tempfile::tempdir().unwrap();
        let candidate = directory.path().join("final.ply.tmp");
        let alternate = directory.path().join("final.ply.tmp.ply");
        let output = ProcessOutput {
            success: true,
            exit_code: Some(0),
            termination: crate::process::ProcessTermination::default(),
            stdout: "Training progress: iteration=4 total=4\nDone training!".into(),
            stderr: "Export at iteration 4 failed".into(),
        };
        assert!(resolve_train_output(&output, candidate.clone(), alternate.clone()).is_err());
        std::fs::write(&candidate, "candidate is validated as PLY by the pipeline").unwrap();
        assert_eq!(
            resolve_train_output(&output, candidate.clone(), alternate.clone()).unwrap(),
            candidate
        );
        let failed = ProcessOutput {
            success: false,
            exit_code: Some(1),
            stderr: "VK_ERROR_DEVICE_LOST".into(),
            ..output
        };
        assert!(matches!(
            resolve_train_output(&failed, candidate, alternate),
            Err(SplatError::EngineProcess(ref failure)) if failure.classification.kind == Some("brush_device_lost")
        ));
    }

    #[test]
    fn optimization_off_passes_compatible_densification_for_all_qualities() {
        for (quality, steps, resolution) in [
            (Quality::Fast, "8000", "1200"),
            (Quality::Balanced, "15000", "1600"),
            (Quality::High, "30000", "2000"),
        ] {
            let args = args_as_strings(train_args(
                Path::new("dataset"),
                Path::new("output"),
                resolve_brush_training_preset(quality, false, None, 3_840, 0).preset,
            ));
            for pair in [
                ["--total-train-iters", steps],
                ["--max-resolution", resolution],
                ["--refine-every", "200"],
                ["--growth-grad-threshold", "0.0025"],
                ["--growth-select-fraction", "0.1"],
                ["--growth-stop-iter", "15000"],
            ] {
                assert!(
                    args.windows(2).any(|actual| actual == pair),
                    "{quality}: {pair:?}"
                );
            }
            assert!(!args.iter().any(|arg| arg == "--max-splats"));
            assert!(!args.iter().any(|arg| arg == "--split-at-screen-size"));
            assert!(!args.iter().any(|arg| arg == "--min-scale-factor"));
        }
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
            .any(|pair| pair == ["--growth-grad-threshold", "0.0016"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--growth-select-fraction", "0.14"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--growth-stop-iter", "23000"]));
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
            .any(|pair| pair == ["--growth-grad-threshold", "0.0022"]));
        assert!(fast
            .windows(2)
            .any(|pair| pair == ["--growth-select-fraction", "0.11"]));
        assert!(fast
            .windows(2)
            .any(|pair| pair == ["--growth-stop-iter", "7000"]));
        assert!(fast
            .windows(2)
            .any(|pair| pair == ["--max-splats", "500000"]));

        let balanced = args_as_strings(train_args(
            Path::new("dataset"),
            Path::new("output"),
            resolve_brush_training_preset(Quality::Balanced, true, Some(8_192), 3_840, 0).preset,
        ));
        assert!(balanced
            .windows(2)
            .any(|pair| pair == ["--growth-grad-threshold", "0.002"]));
        assert!(balanced
            .windows(2)
            .any(|pair| pair == ["--growth-select-fraction", "0.12"]));
        assert!(balanced
            .windows(2)
            .any(|pair| pair == ["--growth-stop-iter", "13000"]));
        assert!(balanced
            .windows(2)
            .any(|pair| pair == ["--max-splats", "1000000"]));
    }

    #[test]
    fn gpu_failure_classifiers_keep_oom_and_device_loss_separate() {
        use crate::failure::{
            device_lost as is_device_lost_detail, gpu_oom as is_out_of_memory_detail,
        };
        assert!(is_out_of_memory_detail("wgpu OutOfMemory while allocating"));
        assert!(!is_out_of_memory_detail("BufferTooBig(2290420416)"));
        assert!(!is_out_of_memory_detail("GPU allocation failed"));
        assert!(!is_out_of_memory_detail("DeviceLost: driver reset"));
        assert!(is_device_lost_detail("DeviceLost: driver reset"));
        assert!(is_device_lost_detail("Parent device is lost"));
        assert!(is_device_lost_detail("VK_ERROR_DEVICE_LOST"));
        assert!(!is_device_lost_detail("process exited with code 101"));
        assert!(!is_device_lost_detail(
            "process exited with code -1073740791"
        ));
        assert!(!is_out_of_memory_detail("process exited with code 1"));
    }

    #[test]
    fn real_panorama_wgpu_oom_remains_oom_despite_secondary_kernel_failures() {
        let output = ProcessOutput {
            success: false,
            exit_code: Some(101),
            termination: crate::process::ProcessTermination::default(),
            stdout: "Refine iter 41001, 3476124 splats.".into(),
            stderr: "wgpu error: Out of Memory\nFailed to read counts: An error happened during execution\nThe bytes were never written\nskipped ProjectForwardKernel\nMemory location was never initialized".into(),
        };
        assert!(matches!(
            resolve_train_output(
                &output,
                PathBuf::from("absent.ply"),
                PathBuf::from("absent.tmp.ply")
            ),
            Err(ref error) if error.is_brush_oom()
        ));
    }
}
