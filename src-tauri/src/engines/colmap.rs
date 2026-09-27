use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use crate::{
    error::{Result, SplatError},
    process::{ProcessManager, ProcessObserver, ProcessSpec},
};

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
pub async fn extract_features(
    executable: &Path,
    database: &Path,
    images: &Path,
    masks: Option<&Path>,
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

fn feature_extraction_args(
    database: &Path,
    images: &Path,
    masks: Option<&Path>,
    gpu_index: Option<u32>,
    use_gpu_option: &str,
    gpu_index_option: &str,
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

pub async fn map(
    executable: &Path,
    database: &Path,
    images: &Path,
    output: &Path,
    log: PathBuf,
    manager: &ProcessManager,
    observer: Option<ProcessObserver>,
) -> Result<()> {
    tokio::fs::create_dir_all(output).await?;
    run_colmap(
        executable,
        vec![
            "mapper".into(),
            "--database_path".into(),
            database.into(),
            "--image_path".into(),
            images.into(),
            "--output_path".into(),
            output.into(),
        ],
        database.parent().unwrap_or(output),
        log,
        manager,
        observer,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn map_incremental(
    executable: &Path,
    database: &Path,
    images: &Path,
    input: &Path,
    output: &Path,
    image_list: &Path,
    log: PathBuf,
    manager: &ProcessManager,
    observer: Option<ProcessObserver>,
) -> Result<()> {
    tokio::fs::create_dir_all(output).await?;
    run_colmap(
        executable,
        incremental_mapper_args(database, images, input, output, image_list),
        database.parent().unwrap_or(output),
        log,
        manager,
        observer,
    )
    .await
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
            Path::new("../frames"),
            Some(Path::new("../masks")),
            None,
            "--FeatureExtraction.use_gpu",
            "--FeatureExtraction.gpu_index",
        ));
        assert!(extraction
            .windows(2)
            .any(|pair| pair == ["--ImageReader.mask_path", "../masks"]));
    }

    #[test]
    fn incremental_features_reuse_the_existing_camera_and_only_new_images() {
        let args = strings(incremental_feature_extraction_args(
            Path::new("database.db"),
            Path::new("../frames"),
            Path::new("reshoot-images.txt"),
            7,
            Some(Path::new("../reshoot-masks")),
            Some(0),
            "--FeatureExtraction.use_gpu",
            "--FeatureExtraction.gpu_index",
        ));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--image_list_path", "reshoot-images.txt"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--ImageReader.existing_camera_id", "7"]));
        assert!(!args.iter().any(|arg| arg == "--ImageReader.single_camera"));
    }

    #[test]
    fn incremental_mapper_keeps_existing_frames_and_intrinsics_fixed() {
        let args = strings(incremental_mapper_args(
            Path::new("database.db"),
            Path::new("../frames"),
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
}
