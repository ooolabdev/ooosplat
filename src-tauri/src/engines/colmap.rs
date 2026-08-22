use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use crate::{
    error::{Result, SplatError},
    process::{ProcessManager, ProcessObserver, ProcessSpec},
};

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

pub async fn extract_features(
    executable: &Path,
    database: &Path,
    images: &Path,
    log: PathBuf,
    manager: &ProcessManager,
    observer: Option<ProcessObserver>,
    gpu_index: Option<u32>,
) -> Result<()> {
    run_colmap(
        executable,
        feature_extraction_args(database, images, gpu_index),
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
    run_colmap(
        executable,
        sequential_matching_args(database, gpu_index),
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
    gpu_index: Option<u32>,
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
        "--FeatureExtraction.use_gpu".into(),
        (if gpu_index.is_some() { "1" } else { "0" }).into(),
    ];
    if let Some(index) = gpu_index {
        args.push("--FeatureExtraction.gpu_index".into());
        args.push(index.to_string().into());
    }
    args
}

fn sequential_matching_args(database: &Path, gpu_index: Option<u32>) -> Vec<OsString> {
    let mut args = vec![
        "sequential_matcher".into(),
        "--database_path".into(),
        database.into(),
        "--FeatureMatching.use_gpu".into(),
        (if gpu_index.is_some() { "1" } else { "0" }).into(),
    ];
    if let Some(index) = gpu_index {
        args.push("--FeatureMatching.gpu_index".into());
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
            Some(2),
        ));
        assert!(extraction
            .windows(2)
            .any(|pair| pair == ["--FeatureExtraction.use_gpu", "1"]));
        assert!(extraction
            .windows(2)
            .any(|pair| pair == ["--FeatureExtraction.gpu_index", "2"]));

        let matching = strings(sequential_matching_args(Path::new("database.db"), Some(2)));
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
        ));
        assert!(extraction
            .windows(2)
            .any(|pair| pair == ["--FeatureExtraction.use_gpu", "0"]));
        assert!(!extraction
            .iter()
            .any(|arg| arg == "--FeatureExtraction.gpu_index"));

        let matching = strings(sequential_matching_args(Path::new("database.db"), None));
        assert!(matching
            .windows(2)
            .any(|pair| pair == ["--FeatureMatching.use_gpu", "0"]));
        assert!(!matching
            .iter()
            .any(|arg| arg == "--FeatureMatching.gpu_index"));
    }
}
