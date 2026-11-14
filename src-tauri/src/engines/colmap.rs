use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use crate::{
    error::{Result, SplatError},
    process::{ProcessManager, ProcessObserver, ProcessSpec},
};

mod initialization;
#[cfg(test)]
mod mapper_native_smoke;
mod mapper_outcome;
pub(crate) use mapper_outcome::{
    is_recoverable_mapper_geometry_failure, select_reconstruction_model,
};
#[cfg(test)]
mod initialization_recovery_tests;
#[cfg(test)]
mod initialization_runtime_tests;
mod vocabulary;

pub const LOCKED_VERSION: &str = "4.2.1";
pub const LOCKED_COMMIT: &str = "bd1fcf654d2dd8fefa1466999c190a246f83f4b9";

/// Scope is frozen with the task; capability checks apply to every version.
#[derive(Debug, Clone, Copy, Default)]
pub struct MapperBaPolicy {
    pub policy_version: Option<u32>,
    pub automatic_optimization: bool,
    pub gpu_index: Option<u32>,
    pub allow_legacy: bool,
    pub panorama: bool,
    pub pose_priors: bool,
}

/// Pixel/camera conversion only: no features, matching, BA, or pose estimation.
pub async fn undistort_training_images(
    executable: &Path,
    images: &Path,
    model: &Path,
    output: &Path,
    log: PathBuf,
    manager: &ProcessManager,
    observer: Option<ProcessObserver>,
) -> Result<()> {
    tokio::fs::create_dir_all(output).await?;
    run_colmap(
        executable,
        vec![
            "image_undistorter".into(),
            "--image_path".into(),
            images.into(),
            "--input_path".into(),
            model.into(),
            "--output_path".into(),
            output.into(),
            "--output_type".into(),
            "COLMAP".into(),
            "--copy_policy".into(),
            "copy".into(),
            "--max_image_size".into(),
            "-1".into(),
            "--max_scale".into(),
            "1".into(),
        ],
        model.parent().unwrap_or(model),
        log,
        manager,
        observer,
    )
    .await
}

pub fn is_locked_build(help: &str) -> bool {
    let short_commit = &LOCKED_COMMIT[..7];
    [LOCKED_COMMIT, short_commit].into_iter().any(|commit| {
        let marker = format!("COLMAP {LOCKED_VERSION} (Commit {commit}");
        help.match_indices(&marker).any(|(index, _)| {
            help[index + marker.len()..]
                .chars()
                .next()
                .is_some_and(|character| character.is_whitespace() || character == ')')
        })
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ColmapCliFamily {
    Legacy39,
    Modern4,
}

impl ColmapCliFamily {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Legacy39 => "COLMAP 3.9 CLI",
            Self::Modern4 => "COLMAP 4.x CLI",
        }
    }
}

pub fn detect_cli_family(feature_help: &str, matching_help: &str) -> Option<ColmapCliFamily> {
    if feature_help.contains("--FeatureExtraction.use_gpu")
        && matching_help.contains("--FeatureMatching.use_gpu")
    {
        Some(ColmapCliFamily::Modern4)
    } else if feature_help.contains("--SiftExtraction.use_gpu")
        && matching_help.contains("--SiftMatching.use_gpu")
    {
        Some(ColmapCliFamily::Legacy39)
    } else {
        None
    }
}

async fn command_help(
    executable: &Path,
    command_name: &str,
    manager: &ProcessManager,
) -> Result<String> {
    let output = manager
        .run(ProcessSpec {
            executable: executable.to_path_buf(),
            args: vec![command_name.into(), "-h".into()],
            working_directory: executable.parent().map(Path::to_path_buf),
            log_path: None,
            observer: None,
        })
        .await?;
    if !output.success {
        return Err(SplatError::UnsupportedEngine(format!(
            "COLMAP {command_name} -h 退出码 {:?}",
            output.exit_code
        )));
    }
    Ok(format!("{}\n{}", output.stdout, output.stderr))
}

async fn feature_gpu_options(
    executable: &Path,
    manager: &ProcessManager,
) -> Result<(&'static str, &'static str)> {
    let help = command_help(executable, "feature_extractor", manager).await?;
    if help.contains("--FeatureExtraction.use_gpu") {
        Ok((
            "--FeatureExtraction.use_gpu",
            "--FeatureExtraction.gpu_index",
        ))
    } else if help.contains("--SiftExtraction.use_gpu") {
        Ok(("--SiftExtraction.use_gpu", "--SiftExtraction.gpu_index"))
    } else {
        Err(SplatError::UnsupportedEngine(
            "COLMAP feature_extractor 不支持已知的 SIFT GPU 参数".into(),
        ))
    }
}

async fn matching_gpu_options(
    executable: &Path,
    matcher: &str,
    manager: &ProcessManager,
) -> Result<(&'static str, &'static str)> {
    let help = command_help(executable, matcher, manager).await?;
    if help.contains("--FeatureMatching.use_gpu") {
        Ok(("--FeatureMatching.use_gpu", "--FeatureMatching.gpu_index"))
    } else if help.contains("--SiftMatching.use_gpu") {
        Ok(("--SiftMatching.use_gpu", "--SiftMatching.gpu_index"))
    } else {
        Err(SplatError::UnsupportedEngine(format!(
            "COLMAP {matcher} 不支持已知的 SIFT GPU 参数"
        )))
    }
}

pub fn require_verified_cli(executable: &Path) -> Result<()> {
    if executable.is_file() {
        Ok(())
    } else {
        Err(SplatError::EngineMissing(executable.display().to_string()))
    }
}

pub(crate) async fn run_colmap(
    executable: &Path,
    mut args: Vec<OsString>,
    working_directory: &Path,
    log_path: PathBuf,
    manager: &ProcessManager,
    observer: Option<ProcessObserver>,
) -> Result<()> {
    let camera_config = working_directory.join("panorama-camera.json");
    if args.first().is_some_and(|a| a == "feature_extractor") && camera_config.is_file() {
        let config: crate::video::panorama::PanoramaCheckpoint =
            serde_json::from_slice(&tokio::fs::read(camera_config).await?)?;
        apply_panorama_camera(&mut args, &config);
    }
    let operation = args
        .first()
        .map(|arg| arg.to_string_lossy().into_owned())
        .unwrap_or_default();
    // Collect evidence before the process capture/display limits. This wrapper
    // is invocation-local, so an appended log or a previous seed cannot decide
    // whether the current failure is safe to treat as a geometric rejection.
    let evidence = std::sync::Arc::new(std::sync::Mutex::new(
        crate::failure::EngineFailureEvidence::default(),
    ));
    let observed_evidence = evidence.clone();
    let combined_observer: ProcessObserver = std::sync::Arc::new(move |update| {
        if let crate::process::ProcessUpdate::Line { line, .. } = &update {
            observed_evidence
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .observe(line);
        }
        if let Some(observer) = &observer {
            observer(update);
        }
    });
    let output = manager
        .run(ProcessSpec {
            executable: executable.to_path_buf(),
            args,
            working_directory: Some(working_directory.to_path_buf()),
            log_path: Some(log_path),
            observer: Some(combined_observer),
        })
        .await?;
    if output.success {
        Ok(())
    } else {
        Err(SplatError::EngineProcess(Box::new(
            crate::failure::EngineProcessFailure::from_output_with_evidence(
                crate::pipeline::PipelineEngine::Colmap,
                &operation,
                &output,
                &evidence
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()),
            ),
        )))
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_mapper_with_ceres_fallback(
    executable: &Path,
    caspar_args: Vec<OsString>,
    ceres_args: Vec<OsString>,
    output: &Path,
    working_directory: &Path,
    log_path: PathBuf,
    manager: &ProcessManager,
    observer: Option<ProcessObserver>,
    selection: &super::colmap_ba::BaSelection,
) -> Result<()> {
    let attempt = Box::pin(run_mapper_with_ceres_fallback_once(
        executable,
        caspar_args.clone(),
        ceres_args.clone(),
        output,
        working_directory,
        log_path.clone(),
        manager,
        observer.clone(),
        selection,
    ))
    .await;
    attempt.result?;
    let used_caspar = attempt.used_caspar;
    let (actual_args, actual_selection) = if selection.gpu_index.is_some() && !used_caspar {
        let mut args = ceres_args.clone();
        super::colmap_ba::append_explicit_ceres_options(&mut args);
        (
            args,
            super::colmap_ba::ceres_selection("Ceres retained after Caspar compatibility fallback"),
        )
    } else {
        (caspar_args, selection.clone())
    };
    Box::pin(recover_mapper_initialization(
        executable,
        actual_args,
        ceres_args,
        output,
        working_directory,
        log_path,
        manager,
        observer,
        actual_selection,
    ))
    .await
}

/// BA compatibility fallback for exactly one geometric initialization attempt.
/// The return value freezes the backend actually used, so a later seed search
/// cannot accidentally re-enable a Caspar backend already disabled in session.
struct MapperAttemptOutcome {
    used_caspar: bool,
    result: Result<()>,
}

#[allow(clippy::too_many_arguments)]
async fn run_mapper_with_ceres_fallback_once(
    executable: &Path,
    caspar_args: Vec<OsString>,
    mut ceres_args: Vec<OsString>,
    output: &Path,
    working_directory: &Path,
    log_path: PathBuf,
    manager: &ProcessManager,
    observer: Option<ProcessObserver>,
    selection: &super::colmap_ba::BaSelection,
) -> MapperAttemptOutcome {
    if let Err(error) = guard_mapper_output(output, working_directory, &ceres_args) {
        return MapperAttemptOutcome {
            used_caspar: selection.gpu_index.is_some(),
            result: Err(error),
        };
    }
    let result = run_colmap(
        executable,
        caspar_args,
        working_directory,
        log_path.clone(),
        manager,
        observer.clone(),
    )
    .await;
    let Some(index) = selection.gpu_index else {
        return MapperAttemptOutcome {
            used_caspar: false,
            result,
        };
    };
    let caspar_error = match result {
        Ok(()) => {
            return MapperAttemptOutcome {
                used_caspar: true,
                result: Ok(()),
            }
        }
        Err(error) if should_retry_mapper_with_ceres(selection, &error) => error,
        Err(error) => {
            return MapperAttemptOutcome {
                used_caspar: true,
                result: Err(error),
            }
        }
    };

    super::colmap_ba::disable_after_runtime_failure(
        executable,
        index,
        "Caspar mapper process failed; Ceres fallback activated",
    )
    .await;
    append_explanation(
        &log_path,
        &format!(
            "[OOOSplat] Global Caspar mapper failed; resetting mapper output and retrying once with local/global Ceres: {caspar_error}\n"
        ),
    )
    .await;
    tracing::warn!(
        "Global Caspar mapper failed on GPU {}; retrying once with Ceres: {}",
        index,
        caspar_error
    );
    if let Err(error) = guard_mapper_output(output, working_directory, &ceres_args) {
        return MapperAttemptOutcome {
            used_caspar: false,
            result: Err(mapper_outcome::with_previous_mapper_attempt(
                error,
                caspar_error,
            )),
        };
    }
    if let Err(error) = reset_mapper_output(output).await {
        return MapperAttemptOutcome {
            used_caspar: false,
            result: Err(mapper_outcome::with_previous_mapper_attempt(
                error,
                caspar_error,
            )),
        };
    }
    super::colmap_ba::append_explicit_ceres_options(&mut ceres_args);
    let result = match run_colmap(
        executable,
        ceres_args,
        working_directory,
        log_path.clone(),
        manager,
        observer,
    )
    .await
    {
        Ok(()) => {
            append_explanation(
                &log_path,
                "[OOOSplat] Ceres fallback completed successfully; Caspar is disabled for this engine/GPU for the remainder of the application process.\n",
            )
            .await;
            Ok(())
        }
        // Cancellation/start/IO failures retain their own type; an earlier
        // Caspar failure must not override the final attempt's outcome.
        Err(error) => Err(mapper_outcome::with_previous_mapper_attempt(
            error,
            caspar_error,
        )),
    };
    MapperAttemptOutcome {
        used_caspar: false,
        result,
    }
}

/// Recover only a demonstrably invalid cold initialization. An existing
/// reconstruction, fixed calibration, pose prior, or explicit seed is never
/// replaced by this compatibility path. All original numeric tolerances and
/// per-source camera assignments remain untouched.
#[allow(clippy::too_many_arguments)]
async fn recover_mapper_initialization(
    executable: &Path,
    mut actual_args: Vec<OsString>,
    ceres_args: Vec<OsString>,
    output: &Path,
    working_directory: &Path,
    log_path: PathBuf,
    manager: &ProcessManager,
    observer: Option<ProcessObserver>,
    mut selection: super::colmap_ba::BaSelection,
) -> Result<()> {
    if !actual_args.first().is_some_and(|arg| arg == "mapper")
        || actual_args.iter().any(|arg| {
            arg == "--input_path"
                || arg
                    .to_str()
                    .is_some_and(|arg| arg.starts_with("--input_path="))
        })
    {
        return Ok(());
    }
    let output = resolved_path(output, working_directory)?;
    let checked_output = output.clone();
    let cancel = manager.child_token();
    let check_cancel = cancel.clone();
    let Some(mut best) = crate::presets::spawn_pipeline_blocking(move || {
        initialization::assess_output(&checked_output, &check_cancel)
    })
    .await
    .map_err(|error| SplatError::Process(format!("Mapper output inspection failed: {error}")))??
    else {
        // The ordinary output validator retains its existing missing-model
        // error. Do not turn a malformed/no-output result into new work here.
        return Ok(());
    };
    if best.valid_intrinsics {
        return Ok(());
    }
    let args = actual_args.clone();
    let working = working_directory.to_path_buf();
    let check_cancel = cancel.clone();
    let Some(seeds) = crate::presets::spawn_pipeline_blocking(move || {
        let Some(request) = initialization::eligible_request(&args, &working)? else {
            return Ok(None);
        };
        initialization::select_seed_images(&request, &check_cancel).map(Some)
    })
    .await
    .map_err(|error| {
        SplatError::Process(format!("Mapper initialization inspection failed: {error}"))
    })??
    else {
        return Ok(());
    };
    if !initialization::should_recover(&best, seeds.input_images) {
        return Ok(());
    }
    append_explanation(
        &log_path,
        &format!(
            "[OOOSplat] Invalid cold Mapper intrinsics with {}/{} registered images; checking at most {} deterministic first-image initializations. Original images, masks, matches and geometric thresholds are unchanged.\n[OOOSplat] initialization_rejection=invalid_intrinsics\n{}\n",
            best.registered_images, seeds.input_images, seeds.seeds.len(), best.invalid_camera_parameters.join("\n")
        ),
    )
    .await;
    let mut rejected_attempts = Vec::new();
    let mut last_attempt_was_geometry_failure = false;
    for (index, seed) in seeds.seeds.into_iter().enumerate() {
        if cancel.is_cancelled() {
            return Err(SplatError::Cancelled);
        }
        let preparation = (|| {
            let attempt = initialization::create_attempt_output(&output)?;
            let args = initialization::recovery_args(&actual_args, &attempt, seed)?;
            let ceres = initialization::recovery_args(&ceres_args, &attempt, seed)?;
            Ok((attempt, args, ceres))
        })();
        let (attempt, args, ceres) = preparation.map_err(|error| {
            mapper_outcome::with_previous_geometry_attempts(error, rejected_attempts.clone())
        })?;
        append_explanation(
            &log_path,
            &format!(
                "[OOOSplat] Initialization check {}/{}: first image ID {}. BA remains parallel; no fixed second image or relaxed geometric threshold.\n",
                index + 1, initialization::MAX_RECOVERY_SEEDS, seed
            ),
        )
        .await;
        let execution = Box::pin(run_mapper_with_ceres_fallback_once(
            executable,
            args,
            ceres,
            &attempt,
            working_directory,
            log_path.clone(),
            manager,
            observer.clone(),
            &selection,
        ))
        .await;
        // A failed Ceres fallback still disables Caspar for later seeds.
        // Backend state is not conditional on a usable reconstruction.
        if selection.gpu_index.is_some() && !execution.used_caspar {
            actual_args = ceres_args.clone();
            super::colmap_ba::append_explicit_ceres_options(&mut actual_args);
            selection = super::colmap_ba::ceres_selection(
                "Ceres retained after Caspar compatibility fallback",
            );
        }
        if let Err(error) = execution.result {
            if is_recoverable_mapper_geometry_failure(&error) {
                last_attempt_was_geometry_failure = true;
                append_explanation(
                    &log_path,
                    &format!(
                        "[OOOSplat] Initialization check {}/{} rejected geometrically; retaining the previous verified candidate and checking the remaining frozen seeds.\n{error}\n",
                        index + 1,
                        initialization::MAX_RECOVERY_SEEDS,
                    ),
                )
                .await;
                if let SplatError::EngineProcess(failure) = error {
                    rejected_attempts.push(*failure);
                }
                continue;
            }
            return Err(mapper_outcome::with_previous_geometry_attempts(
                error,
                rejected_attempts,
            ));
        }
        last_attempt_was_geometry_failure = false;
        let checked_attempt = attempt.clone();
        let check_cancel = cancel.clone();
        let candidate = crate::presets::spawn_pipeline_blocking(move || {
            initialization::assess_recovery_output(&checked_attempt, &check_cancel)
        })
        .await
        .map_err(|error| SplatError::Process(format!("Mapper output inspection failed: {error}")))
        .and_then(|result| result)
        .map_err(|error| {
            mapper_outcome::with_previous_geometry_attempts(error, rejected_attempts.clone())
        })?;
        if let Some(candidate) = candidate {
            if !candidate.valid_intrinsics {
                append_explanation(&log_path, &format!(
                    "[OOOSplat] Initialization check {}: initialization_rejection=invalid_intrinsics\n{}\n",
                    index + 1, candidate.invalid_camera_parameters.join("\n"),
                )).await;
            }
            let checked_output = output.clone();
            let previous = best.clone();
            let check_cancel = cancel.clone();
            let promoted = crate::presets::spawn_pipeline_blocking(move || {
                initialization::promote_if_better(
                    &checked_output,
                    &previous,
                    &candidate,
                    &check_cancel,
                )
            })
            .await
            .map_err(|error| SplatError::Process(format!("Mapper model admission failed: {error}")))
            .and_then(|result| result)
            .map_err(|error| {
                mapper_outcome::with_previous_geometry_attempts(error, rejected_attempts.clone())
            })?;
            if let Some(promoted) = promoted {
                best = promoted;
                append_explanation(
                    &log_path,
                    &format!(
                        "[OOOSplat] Valid initialization adopted: {}/{} registered images. Previous model outputs are preserved.\n",
                        best.registered_images, seeds.input_images
                    ),
                )
                .await;
            }
        }
        if best.valid_intrinsics && best.registered_images >= seeds.input_images {
            return Ok(());
        }
    }
    if cancel.is_cancelled() {
        return Err(SplatError::Cancelled);
    }
    if !best.valid_intrinsics {
        if let Some(last) = last_attempt_was_geometry_failure
            .then(|| rejected_attempts.pop())
            .flatten()
        {
            return Err(mapper_outcome::with_previous_geometry_attempts(
                SplatError::EngineProcess(Box::new(last)),
                rejected_attempts,
            ));
        }
        return Err(mapper_outcome::with_previous_geometry_attempts(
            SplatError::Process(
                "COLMAP initialization produced invalid camera parameters; bounded alternative initializations did not produce a better valid reconstruction".into(),
            ),
            rejected_attempts,
        ));
    }
    Ok(())
}

fn resolved_path(path: &Path, working_directory: &Path) -> Result<PathBuf> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        working_directory.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !normalized.pop() {
                    return Err(SplatError::InvalidPath(path));
                }
            }
            component => normalized.push(component.as_os_str()),
        }
    }
    let mut ancestor = normalized.as_path();
    let mut missing = Vec::new();
    while !ancestor.exists() {
        missing.push(
            ancestor
                .file_name()
                .ok_or_else(|| SplatError::InvalidPath(normalized.clone()))?
                .to_os_string(),
        );
        ancestor = ancestor
            .parent()
            .ok_or_else(|| SplatError::InvalidPath(normalized.clone()))?;
    }
    let mut canonical = std::fs::canonicalize(ancestor)?;
    for component in missing.iter().rev() {
        canonical.push(component);
    }
    Ok(canonical)
}

fn guard_mapper_output(output: &Path, working_directory: &Path, args: &[OsString]) -> Result<()> {
    for key in [
        "--database_path",
        "--output_path",
        "--input_path",
        "--image_path",
        "--Mapper.image_list_path",
        "--Mapper.constant_camera_list_path",
    ] {
        if args.iter().filter(|arg| *arg == key).count() > 1 {
            return Err(SplatError::Process(format!(
                "Duplicate protected mapper option {key}"
            )));
        }
    }
    let output = resolved_path(output, working_directory)?;
    let root = resolved_path(Path::new("."), working_directory)?;
    if output.parent().is_none() || output == root {
        return Err(SplatError::Process(
            "Mapper output must be a separate attempt directory".into(),
        ));
    }
    let argument = |key: &str| {
        args.windows(2)
            .find(|pair| pair[0] == key)
            .map(|pair| PathBuf::from(&pair[1]))
    };
    if let Some(configured) = argument("--output_path") {
        if resolved_path(&configured, working_directory)? != output {
            return Err(SplatError::Process(
                "Mapper output path does not match its attempt".into(),
            ));
        }
    } else {
        return Err(SplatError::Process("Mapper output path is missing".into()));
    }
    for key in [
        "--database_path",
        "--input_path",
        "--image_path",
        "--Mapper.image_list_path",
        "--Mapper.constant_camera_list_path",
    ] {
        if let Some(path) = argument(key) {
            let path = resolved_path(&path, working_directory)?;
            if path.starts_with(&output)
                || (matches!(key, "--input_path" | "--image_path") && output.starts_with(&path))
            {
                return Err(SplatError::Process(
                    "Mapper output overlaps its database, source images or input model".into(),
                ));
            }
        }
    }
    Ok(())
}

/// Execute existing complete mapper arguments with one shared selection and
/// compatibility fallback. Only the supplied attempt output may be reset.
#[allow(clippy::too_many_arguments)]
pub async fn run_mapper_with_ba(
    executable: &Path,
    database: &Path,
    args: Vec<OsString>,
    output: &Path,
    working_directory: &Path,
    log: PathBuf,
    manager: &ProcessManager,
    observer: Option<ProcessObserver>,
    policy: MapperBaPolicy,
) -> Result<()> {
    if !args
        .first()
        .is_some_and(|arg| arg == "mapper" || arg == "pose_prior_mapper")
    {
        return Err(SplatError::Process(
            "BA execution requires a mapper command".into(),
        ));
    }
    let configured_database = args
        .windows(2)
        .find(|pair| pair[0] == "--database_path")
        .map(|pair| PathBuf::from(&pair[1]))
        .ok_or_else(|| SplatError::Process("Mapper database path is missing".into()))?;
    if resolved_path(database, working_directory)?
        != resolved_path(&configured_database, working_directory)?
    {
        return Err(SplatError::Process(
            "Mapper execution database differs from its BA capability database".into(),
        ));
    }
    guard_mapper_output(output, working_directory, &args)?;
    tokio::fs::create_dir_all(output).await?;
    let ceres_args = without_ba_backends(args);
    let (args, selection) = with_ba_policy(
        executable,
        database,
        ceres_args.clone(),
        working_directory,
        policy,
        &log,
    )
    .await?;
    run_mapper_with_ceres_fallback(
        executable,
        args,
        ceres_args,
        output,
        working_directory,
        log,
        manager,
        observer,
        &selection,
    )
    .await
}

fn without_ba_backends(args: Vec<OsString>) -> Vec<OsString> {
    let mut cleaned = Vec::with_capacity(args.len());
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if [
            "--Mapper.ba_local_backend",
            "--Mapper.ba_global_backend",
            "--Mapper.ba_gpu_index",
        ]
        .iter()
        .any(|key| arg == *key)
        {
            args.next();
        } else {
            cleaned.push(arg);
        }
    }
    cleaned
}

async fn reset_mapper_output(output: &Path) -> Result<()> {
    if output.exists() {
        tokio::fs::remove_dir_all(output).await?;
    }
    tokio::fs::create_dir_all(output).await?;
    Ok(())
}

fn should_retry_mapper_with_ceres(
    selection: &super::colmap_ba::BaSelection,
    error: &SplatError,
) -> bool {
    selection.gpu_index.is_some()
        && matches!(error, SplatError::EngineProcess(failure)
        if failure.engine == crate::pipeline::PipelineEngine::Colmap
            && !matches!(failure.classification.kind,Some("mapper_storage"|"mapper_io"|"io_failed"|"disk_space_low")))
}

async fn append_log(path: &Path, message: &str) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await?
        .write_all(message.as_bytes())
        .await?;
    Ok(())
}

/// Application explanations must not veto a compatible engine execution or
/// turn a successful reconstruction into an I/O failure. Raw process logging
/// and model/database writes deliberately retain their existing error policy.
async fn append_explanation(path: &Path, message: &str) {
    if let Err(error) = append_log(path, message).await {
        tracing::warn!(%error, "Could not append the COLMAP explanation");
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn extract_features(
    executable: &Path,
    database: &Path,
    images: &Path,
    masks: Option<&Path>,
    max_image_size: Option<u32>,
    max_features: Option<u32>,
    log: PathBuf,
    manager: &ProcessManager,
    observer: Option<ProcessObserver>,
    gpu_index: Option<u32>,
    camera_params: Option<&str>,
) -> Result<()> {
    let (use_gpu_option, gpu_index_option) = feature_gpu_options(executable, manager).await?;
    run_colmap(
        executable,
        feature_extraction_args(
            database,
            images,
            masks,
            gpu_index,
            use_gpu_option,
            gpu_index_option,
            max_image_size,
            max_features,
            None,
            camera_params,
        ),
        database.parent().unwrap_or(images),
        log,
        manager,
        observer,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn extract_features_quality_v2(
    executable: &Path,
    database: &Path,
    images: &Path,
    masks: Option<&Path>,
    image_list: Option<&Path>,
    max_image_size: u32,
    max_features: u32,
    log: PathBuf,
    manager: &ProcessManager,
    observer: Option<ProcessObserver>,
    gpu_index: Option<u32>,
    camera_params: Option<&str>,
) -> Result<()> {
    let (use_gpu_option, gpu_index_option) = feature_gpu_options(executable, manager).await?;
    run_colmap(
        executable,
        feature_extraction_args(
            database,
            images,
            masks,
            gpu_index,
            use_gpu_option,
            gpu_index_option,
            Some(max_image_size),
            Some(max_features),
            image_list,
            camera_params,
        ),
        database.parent().unwrap_or(images),
        log,
        manager,
        observer,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn extract_incremental_features(
    executable: &Path,
    database: &Path,
    images: &Path,
    image_list: &Path,
    existing_camera_id: u32,
    masks: Option<&Path>,
    max_image_size: Option<u32>,
    max_features: Option<u32>,
    log: PathBuf,
    manager: &ProcessManager,
    observer: Option<ProcessObserver>,
    gpu_index: Option<u32>,
) -> Result<()> {
    let (use_gpu_option, gpu_index_option) = feature_gpu_options(executable, manager).await?;
    run_colmap(
        executable,
        incremental_feature_extraction_args(
            database,
            images,
            image_list,
            existing_camera_id,
            masks,
            gpu_index,
            use_gpu_option,
            gpu_index_option,
            max_image_size,
            max_features,
        ),
        database.parent().unwrap_or(images),
        log,
        manager,
        observer,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn match_sequential(
    executable: &Path,
    database: &Path,
    log: PathBuf,
    manager: &ProcessManager,
    observer: Option<ProcessObserver>,
    gpu_index: Option<u32>,
    loop_detection: bool,
) -> Result<()> {
    let vocab_tree = if loop_detection {
        Some(vocabulary::prepare(executable, manager.child_token()).await?)
    } else {
        None
    };
    let (use_gpu_option, gpu_index_option) =
        matching_gpu_options(executable, "sequential_matcher", manager).await?;
    run_colmap(
        executable,
        sequential_matching_args(
            database,
            gpu_index,
            use_gpu_option,
            gpu_index_option,
            vocab_tree.as_deref(),
        ),
        database.parent().unwrap_or(Path::new(".")),
        log,
        manager,
        observer,
    )
    .await
}

pub async fn match_exhaustive(
    executable: &Path,
    database: &Path,
    log: PathBuf,
    manager: &ProcessManager,
    observer: Option<ProcessObserver>,
    gpu_index: Option<u32>,
) -> Result<()> {
    let (use_gpu_option, gpu_index_option) =
        matching_gpu_options(executable, "exhaustive_matcher", manager).await?;
    run_colmap(
        executable,
        matching_args(
            "exhaustive_matcher",
            database,
            gpu_index,
            use_gpu_option,
            gpu_index_option,
        ),
        database.parent().unwrap_or(Path::new(".")),
        log,
        manager,
        observer,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn match_pairs(
    executable: &Path,
    database: &Path,
    pair_list: &Path,
    log: PathBuf,
    manager: &ProcessManager,
    observer: Option<ProcessObserver>,
    gpu_index: Option<u32>,
) -> Result<()> {
    let (use_gpu_option, gpu_index_option) =
        matching_gpu_options(executable, "matches_importer", manager).await?;
    let mut args = matching_args(
        "matches_importer",
        database,
        gpu_index,
        use_gpu_option,
        gpu_index_option,
    );
    args.extend([
        "--match_list_path".into(),
        pair_list.into(),
        "--match_type".into(),
        "pairs".into(),
    ]);
    run_colmap(
        executable,
        args,
        database.parent().unwrap_or(Path::new(".")),
        log,
        manager,
        observer,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
fn feature_extraction_args(
    database: &Path,
    images: &Path,
    masks: Option<&Path>,
    gpu_index: Option<u32>,
    use_gpu_option: &str,
    gpu_index_option: &str,
    max_image_size: Option<u32>,
    max_features: Option<u32>,
    image_list: Option<&Path>,
    camera_params: Option<&str>,
) -> Vec<OsString> {
    let mut args = vec![
        "feature_extractor".into(),
        "--database_path".into(),
        database.into(),
        "--image_path".into(),
        images.into(),
        "--ImageReader.camera_model".into(),
        "SIMPLE_RADIAL".into(),
        "--ImageReader.single_camera".into(),
        "1".into(),
        use_gpu_option.into(),
        (if gpu_index.is_some() { "1" } else { "0" }).into(),
    ];
    append_feature_extraction_limits(&mut args, use_gpu_option, max_image_size, max_features);
    if let Some(index) = gpu_index {
        args.push(gpu_index_option.into());
        args.push(index.to_string().into());
    }
    if let Some(masks) = masks {
        args.push("--ImageReader.mask_path".into());
        args.push(masks.into());
    }
    if let Some(image_list) = image_list {
        args.push("--image_list_path".into());
        args.push(image_list.into());
    }
    if let Some(params) = camera_params {
        args.extend(["--ImageReader.camera_params".into(), params.into()]);
    }
    args
}

fn append_feature_extraction_limits(
    args: &mut Vec<OsString>,
    use_gpu_option: &str,
    max_image_size: Option<u32>,
    max_features: Option<u32>,
) {
    if let Some(max_image_size) = max_image_size {
        let max_image_size_option = if use_gpu_option.starts_with("--FeatureExtraction") {
            "--FeatureExtraction.max_image_size"
        } else {
            "--SiftExtraction.max_image_size"
        };
        args.extend([
            max_image_size_option.into(),
            max_image_size.to_string().into(),
        ]);
    }
    if let Some(max_features) = max_features {
        args.extend([
            "--SiftExtraction.max_num_features".into(),
            max_features.to_string().into(),
        ]);
    }
}

#[allow(clippy::too_many_arguments)]
fn incremental_feature_extraction_args(
    database: &Path,
    images: &Path,
    image_list: &Path,
    existing_camera_id: u32,
    masks: Option<&Path>,
    gpu_index: Option<u32>,
    use_gpu_option: &str,
    gpu_index_option: &str,
    max_image_size: Option<u32>,
    max_features: Option<u32>,
) -> Vec<OsString> {
    let mut args = vec![
        "feature_extractor".into(),
        "--database_path".into(),
        database.into(),
        "--image_path".into(),
        images.into(),
        "--image_list_path".into(),
        image_list.into(),
        "--ImageReader.existing_camera_id".into(),
        existing_camera_id.to_string().into(),
        use_gpu_option.into(),
        (if gpu_index.is_some() { "1" } else { "0" }).into(),
    ];
    append_feature_extraction_limits(&mut args, use_gpu_option, max_image_size, max_features);
    if let Some(index) = gpu_index {
        args.push(gpu_index_option.into());
        args.push(index.to_string().into());
    }
    if let Some(masks) = masks {
        args.push("--ImageReader.mask_path".into());
        args.push(masks.into());
    }
    args
}

fn sequential_matching_args(
    database: &Path,
    gpu_index: Option<u32>,
    use_gpu_option: &str,
    gpu_index_option: &str,
    vocab_tree: Option<&Path>,
) -> Vec<OsString> {
    let mut args = matching_args(
        "sequential_matcher",
        database,
        gpu_index,
        use_gpu_option,
        gpu_index_option,
    );
    args.extend([
        OsString::from("--SequentialMatching.overlap"),
        OsString::from("10"),
        OsString::from("--SequentialMatching.loop_detection"),
        OsString::from(if vocab_tree.is_some() { "1" } else { "0" }),
    ]);
    if let Some(path) = vocab_tree {
        args.extend([
            "--SequentialMatching.loop_detection_period".into(),
            "10".into(),
            "--SequentialMatching.loop_detection_num_images".into(),
            "50".into(),
            "--SequentialMatching.vocab_tree_path".into(),
            path.into(),
        ]);
    }
    args
}

fn matching_args(
    matcher: &str,
    database: &Path,
    gpu_index: Option<u32>,
    use_gpu_option: &str,
    gpu_index_option: &str,
) -> Vec<OsString> {
    let mut args = vec![
        matcher.into(),
        "--database_path".into(),
        database.into(),
        use_gpu_option.into(),
        (if gpu_index.is_some() { "1" } else { "0" }).into(),
    ];
    if let Some(index) = gpu_index {
        args.push(gpu_index_option.into());
        args.push(index.to_string().into());
    }
    args
}

#[allow(clippy::too_many_arguments)]
pub async fn map(
    executable: &Path,
    database: &Path,
    images: &Path,
    output: &Path,
    allow_two_view_tracks: bool,
    automatic_optimization: bool,
    log: PathBuf,
    manager: &ProcessManager,
    observer: Option<ProcessObserver>,
    gpu_index: Option<u32>,
) -> Result<()> {
    tokio::fs::create_dir_all(output).await?;
    let ceres_args = mapper_args(database, images, None, output, allow_two_view_tracks);
    let (args, selection) = with_ba(
        executable,
        database,
        ceres_args.clone(),
        gpu_index,
        automatic_optimization,
        &log,
    )
    .await?;
    run_mapper_with_ceres_fallback(
        executable,
        args,
        ceres_args,
        output,
        database.parent().unwrap_or(output),
        log,
        manager,
        observer,
        &selection,
    )
    .await
}

/// Continues the Incremental Mapper from an existing sparse model. The
/// baseline model is never overwritten; COLMAP writes the continued model to
/// a separate output directory so the caller can safely roll back.
#[allow(clippy::too_many_arguments)]
pub async fn map_from_existing(
    executable: &Path,
    database: &Path,
    images: &Path,
    input_model: &Path,
    output: &Path,
    automatic_optimization: bool,
    allow_two_view_tracks: bool,
    log: PathBuf,
    manager: &ProcessManager,
    observer: Option<ProcessObserver>,
    gpu_index: Option<u32>,
) -> Result<()> {
    tokio::fs::create_dir_all(output).await?;
    let ceres_args = mapper_args(
        database,
        images,
        Some(input_model),
        output,
        allow_two_view_tracks,
    );
    let (args, selection) = with_ba(
        executable,
        database,
        ceres_args.clone(),
        gpu_index,
        automatic_optimization,
        &log,
    )
    .await?;
    run_mapper_with_ceres_fallback(
        executable,
        args,
        ceres_args,
        output,
        database.parent().unwrap_or(output),
        log,
        manager,
        observer,
        &selection,
    )
    .await
}

fn mapper_args(
    database: &Path,
    images: &Path,
    input_model: Option<&Path>,
    output: &Path,
    allow_two_view_tracks: bool,
) -> Vec<OsString> {
    let mut args = vec![
        "mapper".into(),
        "--database_path".into(),
        database.into(),
        "--image_path".into(),
        images.into(),
    ];
    if let Some(input_model) = input_model {
        args.push("--input_path".into());
        args.push(input_model.into());
        args.push("--Mapper.multiple_models".into());
        args.push("0".into());
    }
    args.push("--output_path".into());
    args.push(output.into());
    if allow_two_view_tracks {
        args.push("--Mapper.tri_ignore_two_view_tracks".into());
        args.push("0".into());
    }
    args
}

#[allow(clippy::too_many_arguments)]
pub async fn map_incremental(
    executable: &Path,
    database: &Path,
    images: &Path,
    input: &Path,
    output: &Path,
    image_list: &Path,
    automatic_optimization: bool,
    log: PathBuf,
    manager: &ProcessManager,
    observer: Option<ProcessObserver>,
    gpu_index: Option<u32>,
) -> Result<()> {
    tokio::fs::create_dir_all(output).await?;
    let ceres_args = incremental_mapper_args(database, images, input, output, image_list);
    let (args, selection) = with_ba(
        executable,
        database,
        ceres_args.clone(),
        gpu_index,
        automatic_optimization,
        &log,
    )
    .await?;
    run_mapper_with_ceres_fallback(
        executable,
        args,
        ceres_args,
        output,
        database.parent().unwrap_or(output),
        log,
        manager,
        observer,
        &selection,
    )
    .await
}

async fn with_ba(
    executable: &Path,
    database: &Path,
    args: Vec<OsString>,
    gpu_index: Option<u32>,
    automatic_optimization: bool,
    log: &Path,
) -> Result<(Vec<OsString>, super::colmap_ba::BaSelection)> {
    with_ba_policy(
        executable,
        database,
        args,
        database.parent().unwrap_or(Path::new(".")),
        MapperBaPolicy {
            automatic_optimization,
            gpu_index,
            allow_legacy: true,
            ..Default::default()
        },
        log,
    )
    .await
}

async fn with_ba_policy(
    executable: &Path,
    database: &Path,
    mut args: Vec<OsString>,
    working_directory: &Path,
    policy: MapperBaPolicy,
    log: &Path,
) -> Result<(Vec<OsString>, super::colmap_ba::BaSelection)> {
    let shared = crate::pipeline::ba_policy::shared_scope(policy.policy_version)?;
    // The marker is generated from the persisted pipeline checkpoint, never
    // from a CLI version string. All mapper paths, including Bridge, share it.
    let panorama_marker = database
        .parent()
        .is_some_and(|p| p.join("panorama-camera.json").is_file());
    if panorama_marker && !args.iter().any(|a| a == "--Mapper.init_min_tri_angle") {
        args.extend([
            "--Mapper.init_min_tri_angle".into(),
            "1".into(),
            "--Mapper.init_max_forward_motion".into(),
            "1".into(),
        ]);
    }
    let selection =
        if policy.pose_priors || args.first().is_some_and(|arg| arg == "pose_prior_mapper") {
            super::colmap_ba::ceres_selection(
                "Caspar does not support GPS pose priors; all position constraints retained",
            )
        } else if policy.panorama || panorama_marker {
            super::colmap_ba::ceres_selection(
            "Caspar does not support EQUIRECTANGULAR cameras; all panorama observations retained",
        )
        } else if !shared && !policy.allow_legacy {
            super::colmap_ba::ceres_selection("saved legacy BA scope keeps this route on Ceres")
        } else if !policy.automatic_optimization || policy.gpu_index.is_none() {
            super::colmap_ba::select_for_mapper(
                executable,
                database,
                policy.gpu_index,
                policy.automatic_optimization,
            )
            .await
        } else {
            let db = database.to_path_buf();
            let check_args = args.clone();
            let directory = working_directory.to_path_buf();
            match crate::presets::spawn_pipeline_blocking(move || {
                super::colmap_ba::mapper_compatibility(&db, &check_args, &directory)
            })
            .await
            {
                Ok(Ok(())) => {
                    super::colmap_ba::select_for_mapper(
                        executable,
                        database,
                        policy.gpu_index,
                        policy.automatic_optimization,
                    )
                    .await
                }
                Ok(Err(reason)) => super::colmap_ba::ceres_selection(&reason),
                Err(error) => super::colmap_ba::ceres_selection(&format!(
                    "Caspar capability check failed: {error}"
                )),
            }
        };
    args = without_ba_backends(args);
    if selection.gpu_index.is_some() {
        super::colmap_ba::append_mapper_options(&mut args, &selection);
    } else {
        super::colmap_ba::append_explicit_ceres_options(&mut args);
    }
    append_explanation(log, &format!("[OOOSplat] {}\n", selection.detail)).await;
    tracing::info!("{}", selection.detail);
    Ok((args, selection))
}

fn apply_panorama_camera(
    args: &mut Vec<OsString>,
    config: &crate::video::panorama::PanoramaCheckpoint,
) {
    if let Some(i) = args.iter().position(|a| a == "--ImageReader.camera_model") {
        args[i + 1] = "EQUIRECTANGULAR".into();
        args.extend([
            "--ImageReader.camera_params".into(),
            format!("{},{}", config.sfm_width, config.sfm_height).into(),
        ]);
    }
}

fn incremental_mapper_args(
    database: &Path,
    images: &Path,
    input: &Path,
    output: &Path,
    image_list: &Path,
) -> Vec<OsString> {
    vec![
        "mapper".into(),
        "--database_path".into(),
        database.into(),
        "--image_path".into(),
        images.into(),
        "--input_path".into(),
        input.into(),
        "--output_path".into(),
        output.into(),
        "--Mapper.image_list_path".into(),
        image_list.into(),
        "--Mapper.fix_existing_frames".into(),
        "1".into(),
        "--Mapper.ba_refine_focal_length".into(),
        "0".into(),
        "--Mapper.ba_refine_principal_point".into(),
        "0".into(),
        "--Mapper.ba_refine_extra_params".into(),
        "0".into(),
    ]
}

pub struct ReshootFeatureOptions<'a> {
    pub images: &'a Path,
    pub list: &'a Path,
    pub masks: Option<&'a Path>,
    pub panorama_dimensions: Option<(u32, u32)>,
    pub existing_camera_id: Option<u32>,
    /// Explicit initialization wins over the narrow legacy compatibility hook.
    pub camera_params: Option<&'a str>,
    /// Actual ordinary photo input, never inferred from a missing ERP flag.
    pub ordinary_images: bool,
    pub max_size: u32,
    pub max_features: u32,
}

pub async fn extract_reshoot_group(
    executable: &Path,
    database: &Path,
    options: ReshootFeatureOptions<'_>,
    log: PathBuf,
    manager: &ProcessManager,
    observer: Option<ProcessObserver>,
    gpu: Option<u32>,
) -> Result<()> {
    let focal = if options.ordinary_images
        && options.camera_params.is_none()
        && options.panorama_dimensions.is_none()
        && options.existing_camera_id.is_none()
    {
        crate::video::temporary_png_focal_workaround::detect(
            executable,
            manager,
            crate::video::temporary_png_focal_workaround::Request {
                images: options.images,
                image_list: Some(options.list),
                ordinary_images: options.ordinary_images,
                camera_model: "SIMPLE_RADIAL",
                single_camera: true,
                explicit_params: None,
                existing_camera_id: None,
                calibrated: false,
            },
        )
        .await
    } else {
        None
    };
    let detected_params = focal.as_ref().map(|f| f.camera_params());
    extract_preflighted_group(
        executable,
        database,
        ReshootFeatureOptions {
            camera_params: options.camera_params.or(detected_params.as_deref()),
            ..options
        },
        log,
        manager,
        observer,
        gpu,
    )
    .await
}

/// Collection work orders have already checked the temporary focal policy
/// before accepting cached features. Never repeat detection during execution.
pub async fn extract_preflighted_group(
    executable: &Path,
    database: &Path,
    options: ReshootFeatureOptions<'_>,
    log: PathBuf,
    manager: &ProcessManager,
    observer: Option<ProcessObserver>,
    gpu: Option<u32>,
) -> Result<()> {
    let cancel = manager.child_token();
    if cancel.is_cancelled() {
        return Err(SplatError::Cancelled);
    }
    // Freeze the actual extraction list before starting the engine. A zero
    // exit status does not prove that COLMAP accepted any images (e.g. every
    // image can be skipped after a mask/read error).
    let expected = expected_feature_images(&tokio::fs::read_to_string(options.list).await?)?;
    let (use_gpu, gpu_option) = feature_gpu_options(executable, manager).await?;
    let params = options
        .panorama_dimensions
        .map(|(w, h)| format!("{w},{h}"))
        .or_else(|| options.camera_params.map(str::to_owned));
    let mut args = if let Some(camera_id) = options.existing_camera_id {
        incremental_feature_extraction_args(
            database,
            options.images,
            options.list,
            camera_id,
            options.masks,
            gpu,
            use_gpu,
            gpu_option,
            Some(options.max_size),
            Some(options.max_features),
        )
    } else {
        feature_extraction_args(
            database,
            options.images,
            options.masks,
            gpu,
            use_gpu,
            gpu_option,
            Some(options.max_size),
            Some(options.max_features),
            Some(options.list),
            params.as_deref(),
        )
    };
    if options.panorama_dimensions.is_some() {
        let index = args
            .iter()
            .position(|a| a == "--ImageReader.camera_model")
            .expect("model argument");
        args[index + 1] = "EQUIRECTANGULAR".into();
    }
    run_colmap(
        executable,
        args,
        database.parent().unwrap_or(options.images),
        log.clone(),
        manager,
        observer,
    )
    .await?;
    let database = database.to_path_buf();
    let validation = crate::presets::spawn_pipeline_blocking(move || {
        validate_extracted_feature_group(&database, &expected, &cancel)
    })
    .await
    .map_err(|error| {
        SplatError::Process(format!(
            "COLMAP feature database validation task failed: {error}"
        ))
    })?;
    if let Err(error) = validation {
        // The engine log is diagnostic evidence only. A log-write failure
        // must never replace the actual extraction/validation failure.
        let _ = append_log(&log, &format!("[OOOSplat] {error}\n")).await;
        return Err(error);
    }
    Ok(())
}

fn expected_feature_images(list: &str) -> Result<Vec<String>> {
    let mut names = std::collections::HashSet::new();
    let mut expected = Vec::new();
    for name in list.lines().filter(|name| !name.is_empty()) {
        if !names.insert(name) {
            return Err(SplatError::Process(
                "COLMAP feature_extractor image list contains duplicate names".into(),
            ));
        }
        expected.push(name.to_owned());
    }
    if expected.is_empty() {
        return Err(SplatError::Process(
            "COLMAP feature_extractor image list is empty".into(),
        ));
    }
    Ok(expected)
}

struct StoredImageFeatures {
    image_id: i64,
    camera_id: Option<i64>,
    keypoint_rows: Option<i64>,
    keypoint_cols: Option<i64>,
    keypoint_bytes: Option<i64>,
    descriptor_rows: Option<i64>,
    descriptor_cols: Option<i64>,
    descriptor_bytes: Option<i64>,
}

fn valid_feature_matrix(
    rows: Option<i64>,
    cols: Option<i64>,
    bytes: Option<i64>,
    keypoints: bool,
) -> Option<i64> {
    let rows = rows?;
    let cols = cols?;
    if rows < 0
        || if keypoints {
            !matches!(cols, 2 | 4 | 6)
        } else {
            cols != 128
        }
    {
        return None;
    }
    let expected = rows
        .checked_mul(cols)?
        .checked_mul(if keypoints { 4 } else { 1 })?;
    (bytes.unwrap_or(0) == expected).then_some(rows)
}

/// Validate only the requested group, with indexed lookups and SQLite length()
/// rather than copying descriptor blobs or hashing the whole database again.
fn validate_extracted_feature_group(
    database: &Path,
    expected: &[String],
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<()> {
    use rusqlite::OptionalExtension;
    if cancel.is_cancelled() {
        return Err(SplatError::Cancelled);
    }
    let db_error = |error: rusqlite::Error| {
        SplatError::Process(format!(
            "COLMAP feature_extractor database validation failed: {error}"
        ))
    };
    let connection = rusqlite::Connection::open_with_flags(
        database,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(db_error)?;
    let mut query = connection.prepare(
        "SELECT images.image_id,cameras.camera_id,keypoints.rows,keypoints.cols,length(keypoints.data),\
         descriptors.rows,descriptors.cols,length(descriptors.data) FROM images \
         LEFT JOIN cameras ON cameras.camera_id=images.camera_id \
         LEFT JOIN keypoints ON keypoints.image_id=images.image_id \
         LEFT JOIN descriptors ON descriptors.image_id=images.image_id WHERE images.name=?1"
    ).map_err(db_error)?;
    let mut incomplete = 0usize;
    let mut samples = Vec::new();
    for name in expected {
        if cancel.is_cancelled() {
            return Err(SplatError::Cancelled);
        }
        let stored = query
            .query_row([name], |row| {
                Ok(StoredImageFeatures {
                    image_id: row.get(0)?,
                    camera_id: row.get(1)?,
                    keypoint_rows: row.get(2)?,
                    keypoint_cols: row.get(3)?,
                    keypoint_bytes: row.get(4)?,
                    descriptor_rows: row.get(5)?,
                    descriptor_cols: row.get(6)?,
                    descriptor_bytes: row.get(7)?,
                })
            })
            .optional()
            .map_err(db_error)?;
        let problem = match stored {
            None => Some("image missing"),
            Some(row) if row.image_id <= 0 || row.camera_id.is_none_or(|id| id <= 0) => {
                Some("camera missing")
            }
            Some(row) => {
                let keypoints = valid_feature_matrix(
                    row.keypoint_rows,
                    row.keypoint_cols,
                    row.keypoint_bytes,
                    true,
                );
                let descriptors = valid_feature_matrix(
                    row.descriptor_rows,
                    row.descriptor_cols,
                    row.descriptor_bytes,
                    false,
                );
                (keypoints.is_none() || keypoints != descriptors)
                    .then_some("features missing or invalid")
            }
        };
        if let Some(problem) = problem {
            incomplete += 1;
            if samples.len() < 5 {
                let short = name
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(96)
                    .collect::<String>();
                samples.push(format!("{short}: {problem}"));
            }
        }
    }
    if incomplete != 0 {
        return Err(SplatError::Process(format!(
            "COLMAP feature_extractor database validation failed: expected images={}, incomplete images={incomplete}; samples=[{}]. Inspect the feature extraction log for skipped-image errors.",
            expected.len(), samples.join("; ")
        )));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn match_reshoot_candidates(
    executable: &Path,
    database: &Path,
    query_list: &Path,
    candidates: u32,
    log: PathBuf,
    manager: &ProcessManager,
    observer: Option<ProcessObserver>,
    gpu: Option<u32>,
) -> Result<()> {
    let vocab = vocabulary::prepare(executable, manager.child_token()).await?;
    let (use_gpu, gpu_option) =
        matching_gpu_options(executable, "vocab_tree_matcher", manager).await?;
    let mut args = matching_args("vocab_tree_matcher", database, gpu, use_gpu, gpu_option);
    args.extend([
        "--VocabTreeMatching.vocab_tree_path".into(),
        vocab.into(),
        "--VocabTreeMatching.match_list_path".into(),
        query_list.into(),
        "--VocabTreeMatching.num_images".into(),
        candidates.to_string().into(),
    ]);
    run_colmap(
        executable,
        args,
        database.parent().unwrap_or(Path::new(".")),
        log,
        manager,
        observer,
    )
    .await
}

/// Query only the immutable registered source images, not other new images.
/// The retriever has no output-file option; consume bounded per-query native
/// log records before ProcessManager truncates its diagnostic capture.
#[allow(clippy::too_many_arguments)]
pub async fn retrieve_reshoot_pairs(
    executable: &Path,
    database: &Path,
    old_names: &[String],
    new_names: &[String],
    candidates: u32,
    log: PathBuf,
    manager: &ProcessManager,
    observer: Option<ProcessObserver>,
) -> Result<Vec<(String, String)>> {
    retrieve_restricted_pairs(
        executable, database, old_names, new_names, candidates, false, log, manager, observer,
    )
    .await
}

/// Native 0.7.1 sequential loop detection queries every tenth image, using
/// fifty candidates and one vocabulary neighbor. Restrict the index to one
/// collection source: other videos must not become implicit temporal peers.
pub async fn retrieve_source_loop_pairs(
    executable: &Path,
    database: &Path,
    source_names: &[String],
    log: PathBuf,
    manager: &ProcessManager,
    observer: Option<ProcessObserver>,
) -> Result<Vec<(String, String)>> {
    let (ordered, queries) = sequential_loop_query_names(source_names);
    let mut pairs = retrieve_restricted_pairs(
        executable, database, &ordered, &queries, 50, true, log, manager, observer,
    )
    .await?;
    pairs.retain(|(first, second)| first != second);
    // The shared retriever reports (indexed candidate, query); native loop
    // matching sends (query, retrieved candidate) to the verifier.
    Ok(pairs
        .into_iter()
        .map(|(candidate, query)| (query, candidate))
        .collect())
}

fn sequential_loop_query_names(source_names: &[String]) -> (Vec<String>, Vec<String>) {
    let mut ordered = source_names.to_vec();
    ordered.sort();
    let queries = ordered.iter().step_by(10).cloned().collect();
    (ordered, queries)
}

#[allow(clippy::too_many_arguments)]
async fn retrieve_restricted_pairs(
    executable: &Path,
    database: &Path,
    old_names: &[String],
    new_names: &[String],
    candidates: u32,
    sequential_loop: bool,
    log: PathBuf,
    manager: &ProcessManager,
    observer: Option<ProcessObserver>,
) -> Result<Vec<(String, String)>> {
    use crate::process::ProcessUpdate;
    use std::{
        collections::HashSet,
        sync::{Arc, Mutex},
    };
    let directory = database.parent().unwrap_or(Path::new("."));
    let old_list = directory.join("retrieval-source.txt");
    let new_list = directory.join("retrieval-query.txt");
    tokio::fs::write(&old_list, old_names.join("\n")).await?;
    tokio::fs::write(&new_list, new_names.join("\n")).await?;
    let vocab = vocabulary::prepare(executable, manager.child_token()).await?;
    let old: HashSet<_> = old_names.iter().cloned().collect();
    let new: HashSet<_> = new_names.iter().cloned().collect();
    let pairs = Arc::new(Mutex::new((None::<String>, 0u32, Vec::new())));
    let output = pairs.clone();
    let collector: ProcessObserver = Arc::new(move |update| {
        if let ProcessUpdate::Line { line, .. } = &update {
            let mut state = output.lock().expect("retrieval collector");
            if let Some((_, rest)) = line.split_once("Querying for image ") {
                state.0 = rest
                    .rsplit_once(" [")
                    .map(|(name, _)| name.to_owned())
                    .filter(|name| new.contains(name));
                state.1 = 0;
            } else if let Some((_, rest)) = line.split_once("image_name=") {
                if let Some((name, _)) = rest.rsplit_once(", score=") {
                    if old.contains(name) && state.1 < candidates {
                        if let Some(query) = state.0.clone() {
                            state.2.push((name.to_owned(), query));
                            state.1 += 1;
                        }
                    }
                }
            }
        }
        if let Some(observer) = &observer {
            observer(update);
        }
    });
    let mut args = vec![
        "vocab_tree_retriever".into(),
        "--database_path".into(),
        database.into(),
        "--vocab_tree_path".into(),
        vocab.into(),
        "--database_image_list_path".into(),
        old_list.into(),
        "--query_image_list_path".into(),
        new_list.into(),
        "--num_images".into(),
        candidates.to_string().into(),
    ];
    append_sequential_loop_retrieval_options(&mut args, sequential_loop);
    run_colmap(executable, args, directory, log, manager, Some(collector)).await?;
    let result = std::mem::take(&mut pairs.lock().expect("retrieval collector").2);
    if result.is_empty() && !new_names.is_empty() && !old_names.is_empty() {
        return Err(SplatError::Process(
            "COLMAP retrieval returned no verifiable source candidates".into(),
        ));
    }
    Ok(result)
}

fn append_sequential_loop_retrieval_options(args: &mut Vec<OsString>, sequential_loop: bool) {
    if sequential_loop {
        args.extend([
            "--num_neighbors".into(),
            "1".into(),
            "--num_checks".into(),
            "64".into(),
            "--num_images_after_verification".into(),
            "0".into(),
        ]);
    }
}

pub struct ReshootMapperOptions<'a> {
    pub allow_two_view_tracks: bool,
    pub images: &'a Path,
    pub input: Option<&'a Path>,
    pub output: &'a Path,
    pub image_list: &'a Path,
    pub fixed_cameras: Option<&'a Path>,
    pub panorama: bool,
}

pub async fn map_reshoot(
    executable: &Path,
    database: &Path,
    options: ReshootMapperOptions<'_>,
    policy: MapperBaPolicy,
    log: PathBuf,
    manager: &ProcessManager,
    observer: Option<ProcessObserver>,
) -> Result<()> {
    tokio::fs::create_dir_all(options.output).await?;
    let mut args = mapper_args(
        database,
        options.images,
        options.input,
        options.output,
        options.allow_two_view_tracks,
    );
    args.extend([
        "--Mapper.image_list_path".into(),
        options.image_list.into(),
        "--Mapper.fix_existing_frames".into(),
        if options.input.is_some() {
            "1".into()
        } else {
            "0".into()
        },
        "--Mapper.ba_refine_principal_point".into(),
        "0".into(),
    ]);
    if let Some(list) = options.fixed_cameras {
        args.extend(["--Mapper.constant_camera_list_path".into(), list.into()]);
    }
    if options.panorama {
        args.extend([
            "--Mapper.init_min_tri_angle".into(),
            "1".into(),
            "--Mapper.init_max_forward_motion".into(),
            "1".into(),
        ]);
    }
    run_mapper_with_ba(
        executable,
        database,
        args,
        options.output,
        database.parent().unwrap_or(options.images),
        log,
        manager,
        observer,
        MapperBaPolicy {
            panorama: options.panorama || policy.panorama,
            ..policy
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequential_loop_retrieval_matches_native_071_without_changing_cross_source_defaults() {
        let source = (0..53)
            .rev()
            .map(|index| format!("frame_{index:04}.png"))
            .collect::<Vec<_>>();
        let (ordered, queries) = sequential_loop_query_names(&source);
        assert_eq!(ordered.len(), 53);
        assert_eq!(
            queries,
            [
                "frame_0000.png",
                "frame_0010.png",
                "frame_0020.png",
                "frame_0030.png",
                "frame_0040.png",
                "frame_0050.png",
            ]
        );
        let mut cross_source = vec![OsString::from("vocab_tree_retriever")];
        append_sequential_loop_retrieval_options(&mut cross_source, false);
        assert_eq!(cross_source, [OsString::from("vocab_tree_retriever")]);
        let mut within_source = cross_source.clone();
        append_sequential_loop_retrieval_options(&mut within_source, true);
        let within_source = strings(within_source);
        for pair in [
            ["--num_neighbors", "1"],
            ["--num_checks", "64"],
            ["--num_images_after_verification", "0"],
        ] {
            assert!(within_source.windows(2).any(|args| args == pair));
        }
    }

    fn feature_database_fixture(root: &Path) -> (PathBuf, rusqlite::Connection) {
        let path = root.join("features.db");
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE cameras(camera_id INTEGER PRIMARY KEY);
            CREATE TABLE images(image_id INTEGER PRIMARY KEY,name TEXT UNIQUE,camera_id INTEGER);
            CREATE TABLE keypoints(image_id INTEGER PRIMARY KEY,rows INTEGER,cols INTEGER,data BLOB);
            CREATE TABLE descriptors(image_id INTEGER PRIMARY KEY,rows INTEGER,cols INTEGER,data BLOB);
            INSERT INTO cameras VALUES(1);").unwrap();
        (path, db)
    }

    fn insert_stored_features(db: &rusqlite::Connection, id: i64, name: &str, rows: i64) {
        db.execute(
            "INSERT INTO images VALUES(?1,?2,1)",
            rusqlite::params![id, name],
        )
        .unwrap();
        db.execute(
            "INSERT INTO keypoints VALUES(?1,?2,4,?3)",
            rusqlite::params![id, rows, vec![0u8; rows as usize * 16]],
        )
        .unwrap();
        db.execute(
            "INSERT INTO descriptors VALUES(?1,?2,128,?3)",
            rusqlite::params![id, rows, vec![0u8; rows as usize * 128]],
        )
        .unwrap();
    }

    #[test]
    fn extraction_list_is_frozen_without_reordering_or_losing_filename_spaces() {
        assert_eq!(
            expected_feature_images("b image.jpg\r\na.jpg\r\n").unwrap(),
            ["b image.jpg", "a.jpg"]
        );
        assert!(expected_feature_images("").is_err());
        assert!(expected_feature_images("a.jpg\na.jpg\n").is_err());
    }

    #[test]
    fn successful_process_cannot_hide_all_or_partially_skipped_feature_images() {
        let root = tempfile::tempdir().unwrap();
        let (path, db) = feature_database_fixture(root.path());
        let expected = (0..12)
            .map(|i| format!("frame_{i:04}.jpg"))
            .collect::<Vec<_>>();
        let cancel = tokio_util::sync::CancellationToken::new();
        let error = validate_extracted_feature_group(&path, &expected, &cancel)
            .unwrap_err()
            .to_string();
        assert!(error.contains("expected images=12, incomplete images=12"));
        assert!(error.contains("frame_0004.jpg: image missing"));
        assert!(!error.contains("frame_0005.jpg"));
        assert!(!error.contains("Query returned no rows"));
        for (i, name) in expected.iter().take(11).enumerate() {
            insert_stored_features(&db, i as i64 + 1, name, 1);
        }
        let error = validate_extracted_feature_group(&path, &expected, &cancel)
            .unwrap_err()
            .to_string();
        assert!(error.contains("incomplete images=1"));
        assert!(error.contains("frame_0011.jpg"));
    }

    #[test]
    fn feature_postcheck_accepts_zero_features_and_only_checks_this_group_read_only() {
        let root = tempfile::tempdir().unwrap();
        let (path, db) = feature_database_fixture(root.path());
        insert_stored_features(&db, 1, "normal.jpg", 2);
        insert_stored_features(&db, 2, "blank.jpg", 0);
        // Another group can be pending, without invalidating this completed one.
        db.execute("INSERT INTO images VALUES(3,'pending.jpg',1)", [])
            .unwrap();
        drop(db);
        let before = std::fs::read(&path).unwrap();
        validate_extracted_feature_group(
            &path,
            &["normal.jpg".into(), "blank.jpg".into()],
            &tokio_util::sync::CancellationToken::new(),
        )
        .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn feature_postcheck_rejects_missing_camera_or_invalid_feature_matrices() {
        for damage in [
            "DELETE FROM cameras",
            "DELETE FROM keypoints",
            "DELETE FROM descriptors",
            "UPDATE keypoints SET rows=-1",
            "UPDATE keypoints SET cols=3",
            "UPDATE keypoints SET data=x'00'",
            "UPDATE descriptors SET rows=0,data=x''",
            "UPDATE descriptors SET cols=64",
            "UPDATE descriptors SET data=x'00'",
            "UPDATE images SET image_id=-1",
        ] {
            let root = tempfile::tempdir().unwrap();
            let (path, db) = feature_database_fixture(root.path());
            insert_stored_features(&db, 1, "a.jpg", 1);
            db.execute_batch(damage).unwrap();
            let error = validate_extracted_feature_group(
                &path,
                &["a.jpg".into()],
                &tokio_util::sync::CancellationToken::new(),
            )
            .unwrap_err()
            .to_string();
            assert!(
                error.contains("feature_extractor database validation failed"),
                "{damage}: {error}"
            );
            assert!(error.contains("incomplete images=1"), "{damage}: {error}");
        }
    }

    #[test]
    fn feature_postcheck_preserves_cancel_and_contextualizes_database_errors() {
        let cancel = tokio_util::sync::CancellationToken::new();
        cancel.cancel();
        assert!(matches!(
            validate_extracted_feature_group(
                Path::new("never-opened.db"),
                &["a.jpg".into()],
                &cancel
            ),
            Err(SplatError::Cancelled)
        ));
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("empty.db");
        drop(rusqlite::Connection::open(&path).unwrap());
        assert!(validate_extracted_feature_group(
            &path,
            &["a.jpg".into()],
            &tokio_util::sync::CancellationToken::new()
        )
        .unwrap_err()
        .to_string()
        .contains("feature_extractor database validation failed"));
    }

    #[tokio::test]
    #[ignore = "Bundled CPU ERP extraction; synthetic or OOOSPLAT_FEATURE_REGRESSION_IMAGES read-only input, no matching, Mapper or Brush"]
    async fn native_erp_features_without_masks_and_skipped_images_postcheck() {
        use sha2::{Digest, Sha256};
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let root = repo
            .join(".cache/erp-feature-mask-regression")
            .join(uuid::Uuid::new_v4().to_string());
        let masks = root.join("empty-masks");
        std::fs::create_dir_all(&masks).unwrap();
        // This override exists only in the ignored regression test. Even when
        // supplied, source images are never copied over, linked or rewritten.
        let (images, names) =
            if let Some(directory) = std::env::var_os("OOOSPLAT_FEATURE_REGRESSION_IMAGES") {
                let images = PathBuf::from(directory);
                let names = crate::video::list_images(&images)
                    .unwrap()
                    .into_iter()
                    .take(2)
                    .map(|path| path.file_name().unwrap().to_str().unwrap().to_owned())
                    .collect::<Vec<_>>();
                assert_eq!(names.len(), 2, "provide at least two prepared ERP images");
                (images, names)
            } else {
                let images = root.join("images");
                std::fs::create_dir_all(&images).unwrap();
                let names = vec!["frame_0000.jpg".to_owned(), "frame_0001.jpg".to_owned()];
                for (index, name) in names.iter().enumerate() {
                    let mut noise = 17u32 + index as u32;
                    let texture = image::RgbImage::from_fn(320, 160, |_, _| {
                        noise = noise.wrapping_mul(1664525).wrapping_add(1013904223);
                        image::Rgb([(noise >> 24) as u8, (noise >> 16) as u8, (noise >> 8) as u8])
                    });
                    texture.save(images.join(name)).unwrap();
                }
                (images, names)
            };
        let first = crate::video::image_projection::inspect(&images.join(&names[0])).unwrap();
        let dimensions = (first.width, first.height);
        assert_eq!(
            u64::from(first.width),
            u64::from(first.height) * 2,
            "test requires complete ERP input"
        );
        for name in &names {
            let header = crate::video::image_projection::inspect(&images.join(name)).unwrap();
            assert_eq!((header.width, header.height), dimensions);
        }
        let before = names
            .iter()
            .map(|name| {
                format!(
                    "{:x}",
                    Sha256::digest(std::fs::read(images.join(name)).unwrap())
                )
            })
            .collect::<Vec<_>>();
        let list = root.join("images.txt");
        std::fs::write(&list, format!("{}\n", names.join("\n"))).unwrap();
        let executable = crate::pipeline::runner::default_engine_paths(None).colmap;
        assert!(executable.is_file(), "bundled COLMAP is required");
        for masked in [false, true] {
            let database = root.join(if masked { "skipped.db" } else { "valid.db" });
            let log = root.join(if masked { "skipped.log" } else { "valid.log" });
            let result = extract_preflighted_group(
                &executable,
                &database,
                ReshootFeatureOptions {
                    images: &images,
                    list: &list,
                    masks: masked.then_some(masks.as_path()),
                    panorama_dimensions: Some(dimensions),
                    existing_camera_id: None,
                    camera_params: None,
                    ordinary_images: false,
                    max_size: first.width.min(1200),
                    max_features: 256,
                },
                log.clone(),
                &ProcessManager::new(),
                None,
                None,
            )
            .await;
            let db = rusqlite::Connection::open_with_flags(
                &database,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .unwrap();
            let count: i64 = db
                .query_row("SELECT COUNT(*) FROM images", [], |r| r.get(0))
                .unwrap();
            if masked {
                let error = result.unwrap_err().to_string();
                assert!(
                    error.contains("expected images=2, incomplete images=2"),
                    "{error}"
                );
                assert_eq!(count, 0);
                let text = std::fs::read_to_string(log).unwrap();
                assert!(text.contains("MASK_ERROR"));
                assert!(text.contains("exit_code: Some(0)"));
            } else {
                result.unwrap();
                assert_eq!(count, 2);
                let model: i32 = db
                    .query_row("SELECT model FROM cameras LIMIT 1", [], |r| r.get(0))
                    .unwrap();
                assert_eq!(model, 17);
            }
        }
        let after = names
            .iter()
            .map(|name| {
                format!(
                    "{:x}",
                    Sha256::digest(std::fs::read(images.join(name)).unwrap())
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(before, after);
        eprintln!("Native ERP feature/mask evidence: {}", root.display());
    }

    #[tokio::test]
    async fn shared_ba_scope_checks_absolute_database_models_and_constraints() {
        let root = tempfile::tempdir().unwrap();
        let database = super::super::colmap_ba::compatible_database_for_test(root.path());
        let executable = root.path().join("not-launched");
        super::super::colmap_ba::cache_success_for_test(&executable, 3).await;
        let log = root.path().join("colmap.log");
        let policy = MapperBaPolicy {
            policy_version: Some(crate::pipeline::ba_policy::VERSION),
            automatic_optimization: true,
            gpu_index: Some(3),
            ..Default::default()
        };
        let args = vec![
            "mapper".into(),
            "--Mapper.fix_existing_frames".into(),
            "1".into(),
            "--Mapper.constant_camera_list_path".into(),
            "fixed.txt".into(),
        ];
        std::fs::write(root.path().join("fixed.txt"), "1\n").unwrap();
        super::super::colmap_ba::mapper_compatibility(&database, &args, root.path()).unwrap();
        let (selected, ba) = with_ba_policy(
            &executable,
            &database,
            args.clone(),
            root.path(),
            policy,
            &log,
        )
        .await
        .unwrap();
        // The macOS runtime is CPU-only, even when a successful GPU probe is
        // cached. Keep its safety gate while testing the same mapper constraints.
        let (expected_gpu, expected_global_backend) = if cfg!(target_os = "macos") {
            assert!(ba.detail.contains("macOS CPU build"));
            (None, "CERES")
        } else {
            (Some(3), "CASPAR")
        };
        assert_eq!(ba.gpu_index, expected_gpu);
        let selected = strings(selected);
        for pair in [
            ["--Mapper.ba_local_backend", "CERES"],
            ["--Mapper.ba_global_backend", expected_global_backend],
            ["--Mapper.fix_existing_frames", "1"],
            ["--Mapper.constant_camera_list_path", "fixed.txt"],
        ] {
            assert!(selected.windows(2).any(|v| v == pair));
        }
        assert_eq!(
            selected.iter().any(|v| v == "--Mapper.ba_gpu_index"),
            expected_gpu.is_some()
        );
        for restricted in [
            MapperBaPolicy {
                policy_version: None,
                ..policy
            },
            MapperBaPolicy {
                panorama: true,
                ..policy
            },
            MapperBaPolicy {
                pose_priors: true,
                ..policy
            },
        ] {
            let (selected, ba) = with_ba_policy(
                &executable,
                &database,
                args.clone(),
                root.path(),
                restricted,
                &log,
            )
            .await
            .unwrap();
            assert_eq!(ba.gpu_index, None);
            assert!(!selected.iter().any(|v| v == "CASPAR"));
        }
        let (_, ba) = with_ba_policy(
            &executable,
            &database,
            vec!["pose_prior_mapper".into()],
            root.path(),
            policy,
            &log,
        )
        .await
        .unwrap();
        assert!(ba.detail.contains("pose priors"));
        rusqlite::Connection::open(&database)
            .unwrap()
            .execute("UPDATE cameras SET model=17 WHERE camera_id=2", [])
            .unwrap();
        let (_, ba) = with_ba_policy(&executable, &database, args, root.path(), policy, &log)
            .await
            .unwrap();
        assert!(ba.gpu_index.is_none());
        assert!(ba.detail.contains("does not support"));
        assert!(with_ba_policy(
            &executable,
            &database,
            vec!["mapper".into()],
            root.path(),
            MapperBaPolicy {
                policy_version: Some(99),
                ..policy
            },
            &log
        )
        .await
        .is_err());
    }

    #[test]
    fn mapper_retry_cannot_reset_database_images_or_seed_models() {
        let root = tempfile::tempdir().unwrap();
        let database = root.path().join("database.db");
        std::fs::write(&database, b"db").unwrap();
        let input = root.path().join("base");
        std::fs::create_dir(&input).unwrap();
        let images = root.path().join("images");
        std::fs::create_dir(&images).unwrap();
        let out = root.path().join("attempt/native");
        let mut args = vec![
            "mapper".into(),
            "--database_path".into(),
            database.into_os_string(),
            "--input_path".into(),
            input.clone().into_os_string(),
            "--image_path".into(),
            images.clone().into_os_string(),
            "--output_path".into(),
            out.clone().into_os_string(),
        ];
        guard_mapper_output(&out, root.path(), &args).unwrap();
        for unsafe_output in [
            root.path().to_path_buf(),
            input.clone(),
            input.join("nested"),
            images,
        ] {
            *args.last_mut().unwrap() = unsafe_output.clone().into_os_string();
            assert!(guard_mapper_output(&unsafe_output, root.path(), &args).is_err());
        }
        *args.last_mut().unwrap() = out.clone().into_os_string();
        args.extend([
            "--Mapper.constant_camera_list_path".into(),
            out.join("fixed.txt").into_os_string(),
        ]);
        assert!(guard_mapper_output(&out, root.path(), &args).is_err());
        assert_eq!(
            std::fs::read(root.path().join("database.db")).unwrap(),
            b"db"
        );
    }

    fn fallback_script(root: &Path, ceres_code: u8) -> (PathBuf, Vec<OsString>) {
        #[cfg(windows)]
        {
            let script = root.join("mapper.ps1");
            let path = root.to_string_lossy().replace('\'', "''");
            std::fs::write(&script,format!("param([string]$stage)\nAdd-Content -LiteralPath '{path}/attempts.txt' -Value $stage\nif ($stage -eq 'CASPAR') {{ Set-Content -LiteralPath '{path}/native/partial' -Value partial; exit 7 }}\nif (Test-Path -LiteralPath '{path}/native/partial') {{ exit 88 }}\nexit {ceres_code}\n")).unwrap();
            (
                PathBuf::from("C:/Windows/System32/WindowsPowerShell/v1.0/powershell.exe"),
                vec![
                    "-NoProfile".into(),
                    "-NonInteractive".into(),
                    "-ExecutionPolicy".into(),
                    "Bypass".into(),
                    "-File".into(),
                    script.into_os_string(),
                ],
            )
        }
        #[cfg(unix)]
        {
            let script = root.join("mapper.sh");
            let path = root.to_string_lossy().replace('\'', "'\\''");
            std::fs::write(&script,format!("printf '%s\\n' \"$1\" >> '{path}/attempts.txt'\nif [ \"$1\" = CASPAR ]; then echo partial > '{path}/native/partial'; exit 7; fi\nif [ -e '{path}/native/partial' ]; then exit 88; fi\nexit {ceres_code}\n")).unwrap();
            (PathBuf::from("/bin/sh"), vec![script.into_os_string()])
        }
    }

    #[tokio::test]
    async fn shared_fallback_runs_ceres_once_and_keeps_final_error_and_inputs() {
        for code in [0, 3] {
            let root = tempfile::tempdir().unwrap();
            let (executable, prefix) = fallback_script(root.path(), code);
            let output = root.path().join("native");
            std::fs::create_dir(&output).unwrap();
            let database = root.path().join("database.db");
            std::fs::write(&database, b"db unchanged").unwrap();
            let base = root.path().join("base");
            std::fs::create_dir(&base).unwrap();
            std::fs::write(base.join("seed"), b"seed unchanged").unwrap();
            let flags = vec![
                "--output_path".into(),
                output.clone().into_os_string(),
                "--database_path".into(),
                database.clone().into_os_string(),
                "--input_path".into(),
                base.clone().into_os_string(),
            ];
            let mut caspar = prefix.clone();
            caspar.push("CASPAR".into());
            caspar.extend(flags.clone());
            let mut ceres = prefix;
            ceres.push("CERES".into());
            ceres.extend(flags);
            let result = run_mapper_with_ceres_fallback(
                &executable,
                caspar,
                ceres,
                &output,
                root.path(),
                root.path().join("colmap.log"),
                &ProcessManager::new(),
                None,
                &super::super::colmap_ba::BaSelection {
                    gpu_index: Some(0),
                    detail: String::new(),
                },
            )
            .await;
            if code == 0 {
                result.unwrap();
            } else {
                let SplatError::EngineProcess(error) = result.unwrap_err() else {
                    panic!("expected final process evidence")
                };
                assert_eq!(error.termination.exit_code, Some(3));
                assert_eq!(error.previous_attempts.len(), 1);
                assert_eq!(error.previous_attempts[0].termination.exit_code, Some(7));
            }
            let attempts = std::fs::read_to_string(root.path().join("attempts.txt")).unwrap();
            assert_eq!(
                attempts
                    .lines()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>(),
                ["CASPAR", "CERES"]
            );
            assert_eq!(std::fs::read(database).unwrap(), b"db unchanged");
            assert_eq!(std::fs::read(base.join("seed")).unwrap(), b"seed unchanged");
            assert!(!output.join("partial").exists());
        }
    }

    #[tokio::test]
    async fn failed_ceres_attempt_still_reports_caspar_disabled_to_next_seed() {
        let root = tempfile::tempdir().unwrap();
        let (executable, prefix) = fallback_script(root.path(), 1);
        let output = root.path().join("native");
        std::fs::create_dir(&output).unwrap();
        let mut caspar = prefix.clone();
        caspar.extend([
            "CASPAR".into(),
            "--output_path".into(),
            output.clone().into_os_string(),
        ]);
        let mut ceres = prefix;
        ceres.extend([
            "CERES".into(),
            "--output_path".into(),
            output.clone().into_os_string(),
        ]);
        let outcome = run_mapper_with_ceres_fallback_once(
            &executable,
            caspar,
            ceres,
            &output,
            root.path(),
            root.path().join("colmap.log"),
            &ProcessManager::new(),
            None,
            &super::super::colmap_ba::BaSelection {
                gpu_index: Some(0),
                detail: String::new(),
            },
        )
        .await;
        assert!(!outcome.used_caspar);
        let SplatError::EngineProcess(failure) = outcome.result.unwrap_err() else {
            panic!("the final process failure must remain structured");
        };
        assert_eq!(failure.termination.exit_code, Some(1));
        assert_eq!(failure.previous_attempts.len(), 1);
        assert_eq!(failure.previous_attempts[0].termination.exit_code, Some(7));
    }

    #[tokio::test]
    async fn explanatory_log_failure_does_not_turn_successful_ceres_into_failure() {
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        let root = tempfile::tempdir().unwrap();
        let blocked = root.path().join("blocked-explanation.log");
        std::fs::create_dir(&blocked).unwrap();
        append_explanation(&blocked, "[OOOSplat] BA selection\n").await;

        let (executable, prefix) = fallback_script(root.path(), 0);
        let output = root.path().join("native");
        std::fs::create_dir(&output).unwrap();
        let database = root.path().join("database.db");
        std::fs::write(&database, b"database unchanged").unwrap();
        let log = root.path().join("colmap.log");
        let redirected = log.clone();
        let moved = root.path().join("completed-colmap.log");
        let exits = Arc::new(AtomicUsize::new(0));
        let seen = exits.clone();
        let manager = ProcessManager::new().with_lifecycle(Arc::new(move |event| {
            if matches!(event, crate::process::ProcessUpdate::Exited { .. })
                && seen.fetch_add(1, Ordering::Relaxed) == 1
            {
                // The Ceres raw writer remains valid; only the subsequent
                // application explanation is made unwritable.
                std::fs::rename(&redirected, &moved).unwrap();
                std::fs::create_dir(&redirected).unwrap();
            }
        }));
        let flags = vec![
            "--output_path".into(),
            output.clone().into_os_string(),
            "--database_path".into(),
            database.clone().into_os_string(),
        ];
        let mut caspar = prefix.clone();
        caspar.push("CASPAR".into());
        caspar.extend(flags.clone());
        let mut ceres = prefix;
        ceres.push("CERES".into());
        ceres.extend(flags);
        run_mapper_with_ceres_fallback(
            &executable,
            caspar,
            ceres,
            &output,
            root.path(),
            log,
            &manager,
            None,
            &super::super::colmap_ba::BaSelection {
                gpu_index: Some(0),
                detail: String::new(),
            },
        )
        .await
        .unwrap();
        assert_eq!(exits.load(Ordering::Relaxed), 2);
        assert_eq!(std::fs::read(database).unwrap(), b"database unchanged");
        assert!(!output.join("partial").exists());
    }

    #[tokio::test]
    async fn shared_mapper_cannot_check_one_database_and_execute_another() {
        let root = tempfile::tempdir().unwrap();
        let expected = root.path().join("expected.db");
        std::fs::write(&expected, b"expected").unwrap();
        let actual = root.path().join("actual.db");
        std::fs::write(&actual, b"actual").unwrap();
        let output = root.path().join("native");
        let error = run_mapper_with_ba(
            Path::new("never-launched"),
            &expected,
            vec![
                "mapper".into(),
                "--database_path".into(),
                actual.into_os_string(),
                "--output_path".into(),
                output.clone().into_os_string(),
            ],
            &output,
            root.path(),
            root.path().join("log"),
            &ProcessManager::new(),
            None,
            MapperBaPolicy::default(),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("differs"));
        assert!(!output.exists());
    }

    #[tokio::test]
    #[ignore = "Requires the local real PNG and bundled COLMAP; feature extraction only in an isolated database"]
    async fn native_reshoot_features_confirm_reused_and_independent_camera_ids() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let source = repo.join(".tmp/focal-diagnosis-20261019/png/images/003A9246.png");
        let original_hash = crate::video::panorama::fingerprint(&source).unwrap();
        let dir = tempfile::tempdir_in(repo.join(".cache")).unwrap();
        let images = dir.path().join("images");
        std::fs::create_dir(&images).unwrap();
        for name in ["original.png", "same.png", "different.png"] {
            std::fs::copy(&source, images.join(name)).unwrap();
        }
        let executable = repo.join("engines/colmap/bin/colmap.exe");
        let database = dir.path().join("database.db");
        let manager = ProcessManager::default();
        let mut camera_id = None;
        for (index, name) in ["original.png", "same.png", "different.png"]
            .iter()
            .enumerate()
        {
            let list = dir.path().join(format!("group-{index}.txt"));
            std::fs::write(&list, format!("{name}\n")).unwrap();
            extract_reshoot_group(
                &executable,
                &database,
                ReshootFeatureOptions {
                    images: &images,
                    list: &list,
                    masks: None,
                    panorama_dimensions: None,
                    existing_camera_id: if index == 1 { camera_id } else { None },
                    camera_params: None,
                    ordinary_images: true,
                    max_size: 800,
                    max_features: 1024,
                },
                dir.path().join("features.log"),
                &manager,
                None,
                None,
            )
            .await
            .unwrap();
            let db = rusqlite::Connection::open_with_flags(
                &database,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .unwrap();
            let assigned: u32 = db
                .query_row(
                    "SELECT camera_id FROM images WHERE name=?1",
                    [name],
                    |row| row.get(0),
                )
                .unwrap();
            let cameras: u32 = db
                .query_row("SELECT COUNT(*) FROM cameras", [], |row| row.get(0))
                .unwrap();
            if index == 0 {
                camera_id = Some(assigned);
            }
            if index == 1 {
                assert_eq!(Some(assigned), camera_id);
                assert_eq!(cameras, 1);
            }
            if index == 2 {
                assert_ne!(Some(assigned), camera_id);
                assert_eq!(cameras, 2);
            }
        }
        assert_eq!(
            crate::video::panorama::fingerprint(&source).unwrap(),
            original_hash
        );
        eprintln!("Native camera reuse verified: original/same share one camera; different creates a second camera. Original PNG SHA-256 unchanged.");
    }

    #[tokio::test]
    #[ignore = "Requires the local Ballintoy DB and bundled vocabulary; read-only retrieval in an isolated copied database"]
    async fn native_reshoot_retrieval_queries_only_registered_old_images() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let source = repo.join(".cache/panorama-v1/projects/20261017-191833_ballintoy-erp");
        let root = tempfile::tempdir_in(repo.join(".cache")).unwrap();
        let database = root.path().join("database.db");
        std::fs::copy(source.join("work/colmap/database.db"), &database).unwrap();
        let images = crate::reconstruction::colmap_model::read_registered_images(
            &source.join("work/colmap/sparse/0"),
        )
        .unwrap();
        let old: Vec<_> = images.iter().take(5).map(|i| i.name.clone()).collect();
        let new: Vec<_> = images
            .iter()
            .skip(5)
            .take(2)
            .map(|i| i.name.clone())
            .collect();
        let pairs = retrieve_reshoot_pairs(
            &repo.join("engines/colmap/bin/colmap.exe"),
            &database,
            &old,
            &new,
            3,
            root.path().join("retriever.log"),
            &ProcessManager::default(),
            None,
        )
        .await
        .unwrap();
        assert_eq!(pairs.len(), 6);
        assert!(pairs
            .iter()
            .all(|(a, b)| old.contains(a) && new.contains(b)));
    }

    #[test]
    fn panorama_camera_uses_model_17_dimensions_without_a_fake_focal_length() {
        let config = crate::video::panorama::PanoramaCheckpoint {
            version: 1,
            source_sha256: "test".into(),
            image_sources: None,
            sfm_width: 2048,
            sfm_height: 1024,
            face_size: 512,
            field_of_view_degrees: 92.,
        };
        let mut args = feature_extraction_args(
            Path::new("database.db"),
            Path::new("frames"),
            None,
            Some(2),
            "--FeatureExtraction.use_gpu",
            "--FeatureExtraction.gpu_index",
            Some(2048),
            Some(8192),
            None,
            None,
        );
        apply_panorama_camera(&mut args, &config);
        let args = strings(args);
        assert!(args
            .windows(2)
            .any(|p| p == ["--ImageReader.camera_model", "EQUIRECTANGULAR"]));
        assert!(args
            .windows(2)
            .any(|p| p == ["--ImageReader.camera_params", "2048,1024"]));
        assert!(args
            .windows(2)
            .any(|p| p == ["--FeatureExtraction.gpu_index", "2"]));
    }
    #[tokio::test]
    async fn panorama_mapper_and_bridge_bypass_caspar_even_with_optimization_and_gpu_enabled() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("panorama-camera.json"), b"{}").unwrap();
        let (args, selection) = with_ba(
            Path::new("not-launched"),
            &temp.path().join("database.db"),
            vec!["mapper".into()],
            Some(2),
            true,
            &temp.path().join("log"),
        )
        .await
        .unwrap();
        assert!(selection.gpu_index.is_none());
        let args = strings(args);
        for option in ["--Mapper.ba_local_backend", "--Mapper.ba_global_backend"] {
            assert!(args.windows(2).any(|p| p == [option, "CERES"]));
        }
        assert!(!args.iter().any(|a| a == "CASPAR"));
    }

    #[test]
    fn locked_runtime_identity_matches_the_shared_build_lock() {
        let lock: serde_json::Value =
            serde_json::from_str(include_str!("../../../engines/colmap-build.json")).unwrap();
        assert_eq!(lock["commit"], LOCKED_COMMIT);
        assert_eq!(lock["version"], LOCKED_VERSION);
        assert!(is_locked_build(&format!(
            "COLMAP {LOCKED_VERSION} (Commit {LOCKED_COMMIT})"
        )));
        assert!(is_locked_build(
            "COLMAP 4.2.1 (Commit bd1fcf6 on 2026-09-29 with CUDA, std hash maps)"
        ));
        assert!(!is_locked_build(
            "COLMAP 4.2.1 (Commit bd1fcf60 on 2026-09-29 with CUDA, std hash maps)"
        ));
        assert!(!is_locked_build(
            "COLMAP 4.2.1 (Commit 0000000 on 2026-09-29 with CUDA, std hash maps)"
        ));
        assert!(!is_locked_build("COLMAP 4.1.0.dev0 (Commit 5b76f53)"));
    }

    fn strings(args: Vec<OsString>) -> Vec<String> {
        args.into_iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn gpu_mode_sets_use_gpu_and_selected_index() {
        let extraction = strings(feature_extraction_args(
            Path::new("database.db"),
            Path::new("frames"),
            None,
            Some(2),
            "--FeatureExtraction.use_gpu",
            "--FeatureExtraction.gpu_index",
            None,
            None,
            None,
            None,
        ));
        assert!(extraction
            .windows(2)
            .any(|pair| pair == ["--FeatureExtraction.use_gpu", "1"]));
        assert!(extraction
            .windows(2)
            .any(|pair| pair == ["--FeatureExtraction.gpu_index", "2"]));

        let matching = strings(sequential_matching_args(
            Path::new("database.db"),
            Some(2),
            "--FeatureMatching.use_gpu",
            "--FeatureMatching.gpu_index",
            None,
        ));
        assert!(matching
            .windows(2)
            .any(|pair| pair == ["--FeatureMatching.use_gpu", "1"]));
        assert!(matching
            .windows(2)
            .any(|pair| pair == ["--FeatureMatching.gpu_index", "2"]));
    }

    #[test]
    fn cpu_mode_disables_gpu_without_passing_an_index() {
        let extraction = strings(feature_extraction_args(
            Path::new("database.db"),
            Path::new("frames"),
            None,
            None,
            "--FeatureExtraction.use_gpu",
            "--FeatureExtraction.gpu_index",
            None,
            None,
            None,
            None,
        ));
        assert!(extraction
            .windows(2)
            .any(|pair| pair == ["--FeatureExtraction.use_gpu", "0"]));
        assert!(!extraction
            .iter()
            .any(|arg| arg == "--FeatureExtraction.gpu_index"));

        let matching = strings(sequential_matching_args(
            Path::new("database.db"),
            None,
            "--FeatureMatching.use_gpu",
            "--FeatureMatching.gpu_index",
            None,
        ));
        assert!(matching
            .windows(2)
            .any(|pair| pair == ["--FeatureMatching.use_gpu", "0"]));
        assert!(!matching
            .iter()
            .any(|arg| arg == "--FeatureMatching.gpu_index"));
    }

    #[test]
    fn exhaustive_matching_uses_the_selected_gpu_without_video_options() {
        let matching = strings(matching_args(
            "exhaustive_matcher",
            Path::new("database.db"),
            Some(1),
            "--FeatureMatching.use_gpu",
            "--FeatureMatching.gpu_index",
        ));
        assert_eq!(matching[0], "exhaustive_matcher");
        assert!(matching
            .windows(2)
            .any(|pair| pair == ["--FeatureMatching.use_gpu", "1"]));
        assert!(matching
            .windows(2)
            .any(|pair| pair == ["--FeatureMatching.gpu_index", "1"]));
        assert!(!matching
            .iter()
            .any(|arg| arg == "--SequentialMatching.overlap"));
        assert!(!matching
            .iter()
            .any(|arg| arg.starts_with("--SequentialMatching.")));
    }

    #[test]
    fn loop_detection_is_explicit_and_preserves_sift_gpu_selection() {
        let tree = Path::new("中文 工程 with spaces/lib/colmap/vocab.bin");
        for (use_gpu, gpu_index) in [
            ("--FeatureMatching.use_gpu", "--FeatureMatching.gpu_index"),
            ("--SiftMatching.use_gpu", "--SiftMatching.gpu_index"),
        ] {
            for selected_gpu in [None, Some(2)] {
                for vocabulary in [None, Some(tree)] {
                    let args = strings(sequential_matching_args(
                        Path::new("database.db"),
                        selected_gpu,
                        use_gpu,
                        gpu_index,
                        vocabulary,
                    ));
                    let value = |name: &str| {
                        args.windows(2)
                            .find(|pair| pair[0] == name)
                            .map(|pair| pair[1].as_str())
                    };
                    assert_eq!(
                        value(use_gpu),
                        Some(if selected_gpu.is_some() { "1" } else { "0" })
                    );
                    assert_eq!(value(gpu_index), selected_gpu.map(|_| "2"));
                    assert_eq!(value("--SequentialMatching.overlap"), Some("10"));
                    assert_eq!(
                        value("--SequentialMatching.loop_detection"),
                        Some(if vocabulary.is_some() { "1" } else { "0" })
                    );
                    if vocabulary.is_some() {
                        assert_eq!(value("--SequentialMatching.vocab_tree_path"), tree.to_str());
                        assert_eq!(
                            value("--SequentialMatching.loop_detection_period"),
                            Some("10")
                        );
                        assert_eq!(
                            value("--SequentialMatching.loop_detection_num_images"),
                            Some("50")
                        );
                    } else {
                        assert_eq!(value("--SequentialMatching.vocab_tree_path"), None);
                        assert_eq!(value("--SequentialMatching.loop_detection_period"), None);
                    }
                }
            }
        }
        let bridge = strings(matching_args(
            "matches_importer",
            Path::new("database.db"),
            None,
            "--FeatureMatching.use_gpu",
            "--FeatureMatching.gpu_index",
        ));
        assert!(!bridge
            .iter()
            .any(|arg| arg.starts_with("--SequentialMatching.")));
    }

    #[tokio::test]
    async fn cancelled_loop_matching_stops_before_probing_or_launching_colmap() {
        let manager = ProcessManager::new();
        manager.cancel();
        let error = match_sequential(
            Path::new("missing/bin/colmap"),
            Path::new("database.db"),
            PathBuf::from("colmap.log"),
            &manager,
            None,
            None,
            true,
        )
        .await
        .unwrap_err();
        assert!(matches!(error, SplatError::Cancelled));
    }

    #[tokio::test]
    #[ignore = "Requires the locked bundled COLMAP runtime; runs only CPU extraction and matching"]
    async fn bundled_offline_loop_matching_smoke() {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let engines = super::super::EnginePaths::discover(None);
        let executable = std::path::absolute(&engines.colmap).unwrap();
        assert!(
            executable.is_file(),
            "Prepare the locked bundled COLMAP runtime first"
        );
        let temporary = tempfile::tempdir().unwrap();
        let working = temporary.path().join("中文 loop check with spaces");
        let images = working.join("images");
        std::fs::create_dir_all(&images).unwrap();
        let picture = std::fs::read(workspace.join("src-tauri/icons/128x128.png")).unwrap();
        for index in 1..=12 {
            std::fs::write(images.join(format!("frame_{index:06}.png")), &picture).unwrap();
        }
        let database = working.join("database.db");
        let log = working.join("colmap.log");
        let manager = ProcessManager::new();
        // Relative image paths mirror the production Windows Unicode-path
        // workaround, while the database and vocabulary use absolute paths.
        extract_features_quality_v2(
            &executable,
            &database,
            Path::new("images"),
            None,
            None,
            128,
            4096,
            log.clone(),
            &manager,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        let database_connection = rusqlite::Connection::open(&database).unwrap();
        let features: u64 = database_connection
            .query_row("SELECT SUM(rows) FROM keypoints", [], |row| row.get(0))
            .unwrap();
        assert!(features > 0, "Smoke input must have real SIFT features");
        drop(database_connection);
        match_sequential(
            &executable,
            &database,
            log.clone(),
            &manager,
            None,
            None,
            true,
        )
        .await
        .unwrap();
        let output = std::fs::read_to_string(&log).unwrap();
        let logged_arguments: Vec<_> = output.lines().map(str::trim).collect();
        assert!(logged_arguments
            .windows(2)
            .any(|pair| pair == ["--SequentialMatching.loop_detection", "1"]));
        assert!(
            output.contains("Indexing image"),
            "Bundled vocabulary must be read, not silently skipped"
        );
        assert!(output.contains("Generating sequential image pairs"));
        println!("Verified offline CPU loop matching: 12 images, {features} SIFT features, bundled hash-locked vocabulary, no Mapper or Brush.");
    }

    #[test]
    fn detects_supported_colmap_cli_families() {
        assert_eq!(
            detect_cli_family("--SiftExtraction.use_gpu", "--SiftMatching.use_gpu"),
            Some(ColmapCliFamily::Legacy39)
        );
        assert_eq!(
            detect_cli_family("--FeatureExtraction.use_gpu", "--FeatureMatching.use_gpu"),
            Some(ColmapCliFamily::Modern4)
        );
        assert_eq!(detect_cli_family("unknown", "unknown"), None);
    }

    #[test]
    fn legacy_cli_uses_legacy_gpu_option_names() {
        let extraction = strings(feature_extraction_args(
            Path::new("database.db"),
            Path::new("frames"),
            None,
            Some(0),
            "--SiftExtraction.use_gpu",
            "--SiftExtraction.gpu_index",
            None,
            None,
            None,
            None,
        ));
        assert!(extraction
            .windows(2)
            .any(|pair| pair == ["--SiftExtraction.use_gpu", "1"]));
        assert!(extraction
            .windows(2)
            .any(|pair| pair == ["--SiftExtraction.gpu_index", "0"]));
    }

    #[test]
    fn transparent_input_passes_the_colmap_mask_root() {
        let extraction = strings(feature_extraction_args(
            Path::new("database.db"),
            Path::new("../colmap_frames"),
            Some(Path::new("../masks")),
            None,
            "--FeatureExtraction.use_gpu",
            "--FeatureExtraction.gpu_index",
            None,
            None,
            None,
            None,
        ));
        assert!(extraction
            .windows(2)
            .any(|pair| pair == ["--ImageReader.mask_path", "../masks"]));
        assert!(extraction
            .windows(2)
            .any(|pair| pair == ["--image_path", "../colmap_frames"]));
    }

    #[test]
    fn optional_camera_parameters_preserve_explicit_values_masks_gpu_and_limits() {
        let args = |params| {
            feature_extraction_args(
                Path::new("database.db"),
                Path::new("../colmap_frames"),
                Some(Path::new("../masks")),
                Some(0),
                "--FeatureExtraction.use_gpu",
                "--FeatureExtraction.gpu_index",
                Some(1600),
                Some(8192),
                Some(Path::new("chosen.txt")),
                params,
            )
        };
        let original = args(None);
        assert!(!original
            .iter()
            .any(|arg| arg == "--ImageReader.camera_params"));
        for (flag, value) in [
            ("--ImageReader.camera_model", "SIMPLE_RADIAL"),
            ("--ImageReader.single_camera", "1"),
            ("--image_list_path", "chosen.txt"),
            ("--FeatureExtraction.use_gpu", "1"),
            ("--FeatureExtraction.gpu_index", "0"),
            ("--FeatureExtraction.max_image_size", "1600"),
            ("--SiftExtraction.max_num_features", "8192"),
            ("--ImageReader.mask_path", "../masks"),
        ] {
            assert!(
                original.windows(2).any(|pair| pair == [flag, value]),
                "{flag}"
            );
        }
        // The builder must not reinterpret or replace an explicit calibration.
        let explicit = "1234.5,400,300,-0.01";
        let mut overridden = args(Some(explicit));
        assert_eq!(overridden.pop().unwrap(), explicit);
        assert_eq!(overridden.pop().unwrap(), "--ImageReader.camera_params");
        assert_eq!(overridden, original);
    }

    #[test]
    fn quality_v2_feature_limits_and_image_list_reach_colmap() {
        let extraction = strings(feature_extraction_args(
            Path::new("database.db"),
            Path::new("frames"),
            None,
            None,
            "--FeatureExtraction.use_gpu",
            "--FeatureExtraction.gpu_index",
            Some(1600),
            Some(4096),
            Some(Path::new("bridge-images.txt")),
            None,
        ));
        assert!(extraction
            .windows(2)
            .any(|pair| pair == ["--FeatureExtraction.max_image_size", "1600"]));
        assert!(extraction
            .windows(2)
            .any(|pair| pair == ["--SiftExtraction.max_num_features", "4096"]));
        assert!(extraction
            .windows(2)
            .any(|pair| pair == ["--image_list_path", "bridge-images.txt"]));
    }

    #[test]
    fn image_size_only_limit_preserves_default_feature_count_for_both_cli_families() {
        for (use_gpu_option, gpu_index_option, expected_image_option) in [
            (
                "--FeatureExtraction.use_gpu",
                "--FeatureExtraction.gpu_index",
                "--FeatureExtraction.max_image_size",
            ),
            (
                "--SiftExtraction.use_gpu",
                "--SiftExtraction.gpu_index",
                "--SiftExtraction.max_image_size",
            ),
        ] {
            let extraction = strings(feature_extraction_args(
                Path::new("database.db"),
                Path::new("frames"),
                None,
                None,
                use_gpu_option,
                gpu_index_option,
                Some(1200),
                None,
                None,
                None,
            ));
            assert!(extraction
                .windows(2)
                .any(|pair| pair == [expected_image_option, "1200"]));
            assert!(!extraction
                .iter()
                .any(|arg| arg == "--SiftExtraction.max_num_features"));
        }
    }

    #[test]
    fn incremental_mapper_arguments_include_the_baseline_model() {
        let args = strings(mapper_args(
            Path::new("database.db"),
            Path::new("../colmap_frames"),
            Some(Path::new("sparse/0")),
            Path::new("sparse-bridge"),
            false,
        ));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--input_path", "sparse/0"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--Mapper.multiple_models", "0"]));
        assert_eq!(args[0], "mapper");
        assert!(!args.iter().any(|arg| arg == "global_mapper"));
    }

    #[test]
    fn high_mapper_can_triangulate_two_view_tracks() {
        let args = strings(mapper_args(
            Path::new("database.db"),
            Path::new("../colmap_frames"),
            None,
            Path::new("sparse"),
            true,
        ));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--Mapper.tri_ignore_two_view_tracks", "0"]));

        let default_args = strings(mapper_args(
            Path::new("database.db"),
            Path::new("../colmap_frames"),
            None,
            Path::new("sparse"),
            false,
        ));
        assert!(!default_args
            .iter()
            .any(|arg| arg == "--Mapper.tri_ignore_two_view_tracks"));
    }

    #[test]
    fn incremental_features_reuse_the_existing_camera_and_only_new_images() {
        let args = strings(incremental_feature_extraction_args(
            Path::new("database.db"),
            Path::new("../colmap_frames"),
            Path::new("reshoot-images.txt"),
            7,
            Some(Path::new("../reshoot-masks")),
            Some(0),
            "--FeatureExtraction.use_gpu",
            "--FeatureExtraction.gpu_index",
            Some(1600),
            Some(8192),
        ));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--image_list_path", "reshoot-images.txt"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--ImageReader.existing_camera_id", "7"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--FeatureExtraction.max_image_size", "1600"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--SiftExtraction.max_num_features", "8192"]));
        assert!(!args.iter().any(|arg| arg == "--ImageReader.single_camera"));
    }

    #[test]
    fn incremental_feature_limits_support_the_legacy_cli_family() {
        let args = strings(incremental_feature_extraction_args(
            Path::new("database.db"),
            Path::new("../colmap_frames"),
            Path::new("reshoot-images.txt"),
            3,
            None,
            None,
            "--SiftExtraction.use_gpu",
            "--SiftExtraction.gpu_index",
            Some(2000),
            Some(16384),
        ));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--SiftExtraction.max_image_size", "2000"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--SiftExtraction.max_num_features", "16384"]));
    }

    #[test]
    fn incremental_mapper_keeps_existing_frames_and_intrinsics_fixed() {
        let args = strings(incremental_mapper_args(
            Path::new("database.db"),
            Path::new("../colmap_frames"),
            Path::new("base-model"),
            Path::new("incremental-model"),
            Path::new("mapper-images.txt"),
        ));
        for pair in [
            ["--Mapper.fix_existing_frames", "1"],
            ["--Mapper.ba_refine_focal_length", "0"],
            ["--Mapper.ba_refine_principal_point", "0"],
            ["--Mapper.ba_refine_extra_params", "0"],
        ] {
            assert!(args.windows(2).any(|window| window == pair));
        }
    }

    #[test]
    fn only_a_failed_caspar_mapper_process_triggers_the_ceres_retry() {
        let caspar = super::super::colmap_ba::BaSelection {
            gpu_index: Some(0),
            detail: String::new(),
        };
        let ceres = super::super::colmap_ba::BaSelection {
            gpu_index: None,
            detail: String::new(),
        };
        assert!(!should_retry_mapper_with_ceres(
            &caspar,
            &SplatError::Process("Caspar solver failed".into())
        ));
        assert!(!should_retry_mapper_with_ceres(
            &ceres,
            &SplatError::Process("Ceres solver failed".into())
        ));
        assert!(!should_retry_mapper_with_ceres(
            &caspar,
            &SplatError::Cancelled
        ));
        assert!(!should_retry_mapper_with_ceres(
            &caspar,
            &SplatError::EngineStart {
                engine: "colmap".into(),
                detail: "missing".into()
            }
        ));
        assert!(!should_retry_mapper_with_ceres(
            &caspar,
            &SplatError::Io(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
        ));
        let output = crate::process::ProcessOutput {
            success: false,
            exit_code: Some(-1073740940),
            termination: crate::process::ProcessTermination {
                exit_code: Some(-1073740940),
                windows_status: Some("0xC0000374".into()),
                ..Default::default()
            },
            stdout: String::new(),
            stderr: "Could not register image".into(),
        };
        let error =
            SplatError::EngineProcess(Box::new(crate::failure::EngineProcessFailure::from_output(
                crate::pipeline::PipelineEngine::Colmap,
                "mapper",
                &output,
            )));
        assert!(should_retry_mapper_with_ceres(&caspar, &error));
        assert!(!should_retry_mapper_with_ceres(&ceres, &error));
        let SplatError::EngineProcess(mut storage) = error else {
            unreachable!()
        };
        storage.classification.kind = Some("mapper_storage");
        assert!(!should_retry_mapper_with_ceres(
            &caspar,
            &SplatError::EngineProcess(storage)
        ));
    }

    #[tokio::test]
    async fn ceres_retry_discards_partial_mapper_output() {
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("sparse");
        tokio::fs::create_dir_all(output.join("0")).await.unwrap();
        tokio::fs::write(output.join("0/partial.bin"), b"partial")
            .await
            .unwrap();
        reset_mapper_output(&output).await.unwrap();
        assert!(output.is_dir());
        assert!(!output.join("0/partial.bin").exists());
    }
}
