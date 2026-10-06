use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use crate::{
    error::{Result, SplatError},
    process::{ProcessManager, ProcessObserver, ProcessSpec},
};

pub const LOCKED_VERSION: &str = "4.2.1";
pub const LOCKED_COMMIT: &str = "bd1fcf654d2dd8fefa1466999c190a246f83f4b9";

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

async fn run_colmap(
    executable: &Path,
    args: Vec<OsString>,
    working_directory: &Path,
    log_path: PathBuf,
    manager: &ProcessManager,
    observer: Option<ProcessObserver>,
) -> Result<()> {
    let output = manager
        .run(ProcessSpec {
            executable: executable.to_path_buf(),
            args,
            working_directory: Some(working_directory.to_path_buf()),
            log_path: Some(log_path),
            observer,
        })
        .await?;
    if output.success {
        Ok(())
    } else {
        let detail = output.failure_detail();
        Err(SplatError::Process(format!(
            "COLMAP 退出码 {:?}{}",
            output.exit_code,
            if detail.is_empty() {
                String::new()
            } else {
                format!("\n{detail}")
            }
        )))
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_mapper_with_ceres_fallback(
    executable: &Path,
    caspar_args: Vec<OsString>,
    mut ceres_args: Vec<OsString>,
    output: &Path,
    working_directory: &Path,
    log_path: PathBuf,
    manager: &ProcessManager,
    observer: Option<ProcessObserver>,
    selection: &super::colmap_ba::BaSelection,
) -> Result<()> {
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
        return result;
    };
    let caspar_error = match result {
        Ok(()) => return Ok(()),
        Err(error) if should_retry_mapper_with_ceres(selection, &error) => error,
        Err(error) => return Err(error),
    };

    super::colmap_ba::disable_after_runtime_failure(
        executable,
        index,
        "Caspar mapper process failed; Ceres fallback activated",
    )
    .await;
    append_log(
        &log_path,
        &format!(
            "[OOOSplat] Global Caspar mapper failed; resetting mapper output and retrying once with local/global Ceres: {caspar_error}\n"
        ),
    )
    .await?;
    tracing::warn!(
        "Global Caspar mapper failed on GPU {}; retrying once with Ceres: {}",
        index,
        caspar_error
    );
    reset_mapper_output(output).await?;
    super::colmap_ba::append_explicit_ceres_options(&mut ceres_args);
    match run_colmap(
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
            append_log(
                &log_path,
                "[OOOSplat] Ceres fallback completed successfully; Caspar is disabled for this engine/GPU for the remainder of the application process.\n",
            )
            .await?;
            Ok(())
        }
        Err(ceres_error) => Err(SplatError::Process(format!(
            "Caspar mapper failed and the automatic Ceres retry also failed.\nCaspar: {caspar_error}\nCeres: {ceres_error}"
        ))),
    }
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
    selection.gpu_index.is_some() && matches!(error, SplatError::Process(_))
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

pub async fn match_sequential(
    executable: &Path,
    database: &Path,
    log: PathBuf,
    manager: &ProcessManager,
    observer: Option<ProcessObserver>,
    gpu_index: Option<u32>,
) -> Result<()> {
    let (use_gpu_option, gpu_index_option) =
        matching_gpu_options(executable, "sequential_matcher", manager).await?;
    run_colmap(
        executable,
        sequential_matching_args(database, gpu_index, use_gpu_option, gpu_index_option),
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
    ]);
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
    log: PathBuf,
    manager: &ProcessManager,
    observer: Option<ProcessObserver>,
    gpu_index: Option<u32>,
) -> Result<()> {
    tokio::fs::create_dir_all(output).await?;
    let ceres_args = mapper_args(database, images, Some(input_model), output, false);
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
    mut args: Vec<OsString>,
    gpu_index: Option<u32>,
    automatic_optimization: bool,
    log: &Path,
) -> Result<(Vec<OsString>, super::colmap_ba::BaSelection)> {
    let selection = super::colmap_ba::select_for_mapper(
        executable,
        database,
        gpu_index,
        automatic_optimization,
    )
    .await;
    super::colmap_ba::append_mapper_options(&mut args, &selection);
    append_log(log, &format!("[OOOSplat] {}\n", selection.detail)).await?;
    tracing::info!("{}", selection.detail);
    Ok((args, selection))
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

#[cfg(test)]
mod tests {
    use super::*;

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
        ));
        assert!(extraction
            .windows(2)
            .any(|pair| pair == ["--ImageReader.mask_path", "../masks"]));
        assert!(extraction
            .windows(2)
            .any(|pair| pair == ["--image_path", "../colmap_frames"]));
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
        assert!(should_retry_mapper_with_ceres(
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
