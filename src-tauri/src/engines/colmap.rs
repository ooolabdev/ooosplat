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
    manager: &ProcessManager,
) -> Result<(&'static str, &'static str)> {
    let help = command_help(executable, "sequential_matcher", manager).await?;
    if help.contains("--FeatureMatching.use_gpu") {
        Ok(("--FeatureMatching.use_gpu", "--FeatureMatching.gpu_index"))
    } else if help.contains("--SiftMatching.use_gpu") {
        Ok(("--SiftMatching.use_gpu", "--SiftMatching.gpu_index"))
    } else {
        Err(SplatError::UnsupportedEngine(
            "COLMAP sequential_matcher 不支持已知的 SIFT GPU 参数".into(),
        ))
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
        Err(SplatError::Process(format!(
            "COLMAP 退出码 {:?}",
            output.exit_code
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

pub async fn match_sequential(
    executable: &Path,
    database: &Path,
    log: PathBuf,
    manager: &ProcessManager,
    observer: Option<ProcessObserver>,
    gpu_index: Option<u32>,
) -> Result<()> {
    let (use_gpu_option, gpu_index_option) = matching_gpu_options(executable, manager).await?;
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

fn sequential_matching_args(
    database: &Path,
    gpu_index: Option<u32>,
    use_gpu_option: &str,
    gpu_index_option: &str,
) -> Vec<OsString> {
    let mut args = vec![
        "sequential_matcher".into(),
        "--database_path".into(),
        database.into(),
        use_gpu_option.into(),
        (if gpu_index.is_some() { "1" } else { "0" }).into(),
    ];
    if let Some(index) = gpu_index {
        args.push(gpu_index_option.into());
        args.push(index.to_string().into());
    }
    args.extend([
        OsString::from("--SequentialMatching.overlap"),
        OsString::from("10"),
    ]);
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
}
