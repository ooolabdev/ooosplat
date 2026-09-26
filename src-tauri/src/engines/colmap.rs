use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use crate::{
    error::{Result, SplatError},
    presets::{MatchingBudget, Quality, SfmBudget},
    process::{ProcessManager, ProcessObserver, ProcessSpec},
};

use rusqlite::{Connection, OpenFlags};

pub const HIGH_QUALITY_EXPERIMENT_ENV: &str = "OOOSPLAT_COLMAP_HIGH_QUALITY_EXPERIMENT";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ColmapQualityTuning {
    pub enabled: bool,
    pub estimate_affine_shape: bool,
    pub domain_size_pooling: bool,
    pub guided_matching: bool,
    pub ba_local_max_num_iterations: Option<u32>,
    pub ba_local_max_refinements: Option<u32>,
    pub ba_global_max_num_iterations: Option<u32>,
}

impl ColmapQualityTuning {
    pub const fn for_run(quality: Quality, experiment_requested: bool) -> Self {
        if experiment_requested && matches!(quality, Quality::High) {
            Self {
                enabled: true,
                estimate_affine_shape: true,
                domain_size_pooling: false,
                guided_matching: true,
                ba_local_max_num_iterations: Some(30),
                ba_local_max_refinements: Some(3),
                ba_global_max_num_iterations: Some(75),
            }
        } else {
            Self {
                enabled: false,
                estimate_affine_shape: false,
                domain_size_pooling: false,
                guided_matching: false,
                ba_local_max_num_iterations: None,
                ba_local_max_refinements: None,
                ba_global_max_num_iterations: None,
            }
        }
    }

    pub fn requested_from_environment() -> bool {
        std::env::var(HIGH_QUALITY_EXPERIMENT_ENV).is_ok_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
    }

    pub fn log_summary(self, budget: SfmBudget) -> String {
        format!(
            "COLMAP quality experiment:\n\
             enabled = {}\n\
             SIFT max image size = {}\n\
             SIFT max features = {}\n\
             estimate affine shape = {}\n\
             domain size pooling = {}\n\
             guided matching = {}\n\
             BA local iterations = {}\n\
             BA local refinements = {}\n\
             BA global iterations = {}",
            self.enabled,
            budget.max_image_size,
            budget.max_features,
            self.estimate_affine_shape,
            self.domain_size_pooling,
            self.guided_matching,
            self.ba_local_max_num_iterations.unwrap_or(25),
            self.ba_local_max_refinements.unwrap_or(2),
            self.ba_global_max_num_iterations.unwrap_or(50),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ColmapCliFamily {
    Legacy39,
    Modern4,
}

pub const VOCABULARY_TREE_FILE: &str = "vocab_tree_faiss_flickr100K_words256K.bin";

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ColmapDatabaseMetrics {
    pub image_count: u64,
    pub total_detected_features: u64,
    pub mean_features_per_image: f64,
    pub median_features_per_image: f64,
    pub raw_match_pairs: u64,
    pub raw_matches: u64,
    pub geometrically_verified_pairs: u64,
    pub verified_correspondences: u64,
}

pub fn analyze_database(database: &Path) -> Result<ColmapDatabaseMetrics> {
    let connection = Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(database_error)?;
    let image_count = connection
        .query_row("SELECT COUNT(*) FROM images", [], |row| {
            row.get::<_, u64>(0)
        })
        .map_err(database_error)?;
    let mut feature_counts = Vec::new();
    {
        let mut statement = connection
            .prepare("SELECT rows FROM keypoints ORDER BY image_id")
            .map_err(database_error)?;
        let rows = statement
            .query_map([], |row| row.get::<_, u64>(0))
            .map_err(database_error)?;
        for row in rows {
            feature_counts.push(row.map_err(database_error)?);
        }
    }
    feature_counts.sort_unstable();
    let total_detected_features = feature_counts.iter().sum();
    let mean_features_per_image = if feature_counts.is_empty() {
        0.0
    } else {
        total_detected_features as f64 / feature_counts.len() as f64
    };
    let median_features_per_image = match feature_counts.len() {
        0 => 0.0,
        length if length % 2 == 1 => feature_counts[length / 2] as f64,
        length => (feature_counts[length / 2 - 1] as f64 + feature_counts[length / 2] as f64) / 2.0,
    };
    let (raw_match_pairs, raw_matches) = connection
        .query_row(
            "SELECT COUNT(*), COALESCE(SUM(rows), 0) FROM matches WHERE rows > 0",
            [],
            |row| Ok((row.get::<_, u64>(0)?, row.get::<_, u64>(1)?)),
        )
        .map_err(database_error)?;
    let (geometrically_verified_pairs, verified_correspondences) = connection
        .query_row(
            "SELECT COUNT(*), COALESCE(SUM(rows), 0) FROM two_view_geometries WHERE rows > 0",
            [],
            |row| Ok((row.get::<_, u64>(0)?, row.get::<_, u64>(1)?)),
        )
        .map_err(database_error)?;
    Ok(ColmapDatabaseMetrics {
        image_count,
        total_detected_features,
        mean_features_per_image,
        median_features_per_image,
        raw_match_pairs,
        raw_matches,
        geometrically_verified_pairs,
        verified_correspondences,
    })
}

fn database_error(error: rusqlite::Error) -> SplatError {
    SplatError::Process(format!("Unable to query COLMAP benchmark metrics: {error}"))
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

pub fn require_vocabulary_tree(path: &Path) -> Result<()> {
    if path.is_file() {
        Ok(())
    } else {
        Err(SplatError::EngineMissing(path.display().to_string()))
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
    budget: SfmBudget,
    tuning: ColmapQualityTuning,
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
            budget,
            tuning,
        ),
        database.parent().unwrap_or(images),
        log,
        manager,
        observer,
    )
    .await
}

/// Exact current-main feature extraction path: engine defaults, without
/// Quality v2 SfM caps. This is the Planner-off A/B baseline.
#[allow(clippy::too_many_arguments)]
pub async fn extract_features_legacy(
    executable: &Path,
    database: &Path,
    images: &Path,
    masks: Option<&Path>,
    log: PathBuf,
    manager: &ProcessManager,
    observer: Option<ProcessObserver>,
    gpu_index: Option<u32>,
    tuning: ColmapQualityTuning,
) -> Result<()> {
    let (use_gpu_option, gpu_index_option) = feature_gpu_options(executable, manager).await?;
    run_colmap(
        executable,
        feature_extraction_args_base(
            database,
            images,
            masks,
            gpu_index,
            use_gpu_option,
            gpu_index_option,
            None,
            None,
            tuning,
        ),
        database.parent().unwrap_or(images),
        log,
        manager,
        observer,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn extract_features_for_list(
    executable: &Path,
    database: &Path,
    images: &Path,
    masks: Option<&Path>,
    image_list: &Path,
    log: PathBuf,
    manager: &ProcessManager,
    observer: Option<ProcessObserver>,
    gpu_index: Option<u32>,
    budget: SfmBudget,
    tuning: ColmapQualityTuning,
) -> Result<()> {
    let (use_gpu_option, gpu_index_option) = feature_gpu_options(executable, manager).await?;
    run_colmap(
        executable,
        feature_extraction_args_base(
            database,
            images,
            masks,
            gpu_index,
            use_gpu_option,
            gpu_index_option,
            Some(budget),
            Some(image_list),
            tuning,
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
    budget: MatchingBudget,
    tuning: ColmapQualityTuning,
) -> Result<()> {
    let (use_gpu_option, gpu_index_option) =
        matching_gpu_options(executable, "sequential_matcher", manager).await?;
    run_colmap(
        executable,
        sequential_matching_args(
            database,
            gpu_index,
            use_gpu_option,
            gpu_index_option,
            budget.sequential_overlap,
            tuning,
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
    tuning: ColmapQualityTuning,
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
            tuning,
        ),
        database.parent().unwrap_or(Path::new(".")),
        log,
        manager,
        observer,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn match_sequential_with_loop(
    executable: &Path,
    database: &Path,
    vocabulary_tree: &Path,
    log: PathBuf,
    manager: &ProcessManager,
    observer: Option<ProcessObserver>,
    gpu_index: Option<u32>,
    budget: MatchingBudget,
    tuning: ColmapQualityTuning,
) -> Result<()> {
    require_vocabulary_tree(vocabulary_tree)?;
    let (use_gpu_option, gpu_index_option) =
        matching_gpu_options(executable, "sequential_matcher", manager).await?;
    let args = sequential_matching_with_loop_args(
        database,
        vocabulary_tree,
        gpu_index,
        use_gpu_option,
        gpu_index_option,
        budget.sequential_overlap,
        budget.prefilter_neighbors,
        tuning,
    );
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
pub async fn match_prefilter(
    executable: &Path,
    database: &Path,
    vocabulary_tree: &Path,
    log: PathBuf,
    manager: &ProcessManager,
    observer: Option<ProcessObserver>,
    gpu_index: Option<u32>,
    neighbors: u32,
    tuning: ColmapQualityTuning,
) -> Result<()> {
    let budget = MatchingBudget {
        sequential_overlap: neighbors.max(4),
        prefilter_neighbors: neighbors.max(4),
    };
    match_sequential_with_loop(
        executable,
        database,
        vocabulary_tree,
        log,
        manager,
        observer,
        gpu_index,
        budget,
        tuning,
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
    tuning: ColmapQualityTuning,
) -> Result<()> {
    let (use_gpu_option, gpu_index_option) =
        matching_gpu_options(executable, "matches_importer", manager).await?;
    let mut args = matching_args(
        "matches_importer",
        database,
        gpu_index,
        use_gpu_option,
        gpu_index_option,
        tuning,
    );
    args.extend([
        OsString::from("--match_list_path"),
        pair_list.into(),
        OsString::from("--match_type"),
        OsString::from("pairs"),
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
    budget: SfmBudget,
    tuning: ColmapQualityTuning,
) -> Vec<OsString> {
    feature_extraction_args_base(
        database,
        images,
        masks,
        gpu_index,
        use_gpu_option,
        gpu_index_option,
        Some(budget),
        None,
        tuning,
    )
}

#[allow(clippy::too_many_arguments)]
fn feature_extraction_args_base(
    database: &Path,
    images: &Path,
    masks: Option<&Path>,
    gpu_index: Option<u32>,
    use_gpu_option: &str,
    gpu_index_option: &str,
    budget: Option<SfmBudget>,
    image_list: Option<&Path>,
    tuning: ColmapQualityTuning,
) -> Vec<OsString> {
    let max_image_size_option = if use_gpu_option.starts_with("--FeatureExtraction") {
        "--FeatureExtraction.max_image_size"
    } else {
        "--SiftExtraction.max_image_size"
    };
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
    if let Some(budget) = budget {
        args.extend([
            max_image_size_option.into(),
            budget.max_image_size.to_string().into(),
            "--SiftExtraction.max_num_features".into(),
            budget.max_features.to_string().into(),
        ]);
    }
    if tuning.enabled {
        args.extend([
            "--SiftExtraction.estimate_affine_shape".into(),
            (if tuning.estimate_affine_shape {
                "1"
            } else {
                "0"
            })
            .into(),
            "--SiftExtraction.domain_size_pooling".into(),
            (if tuning.domain_size_pooling { "1" } else { "0" }).into(),
        ]);
    }
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

fn sequential_matching_args(
    database: &Path,
    gpu_index: Option<u32>,
    use_gpu_option: &str,
    gpu_index_option: &str,
    sequential_overlap: u32,
    tuning: ColmapQualityTuning,
) -> Vec<OsString> {
    let mut args = matching_args(
        "sequential_matcher",
        database,
        gpu_index,
        use_gpu_option,
        gpu_index_option,
        tuning,
    );
    args.extend([
        OsString::from("--SequentialMatching.overlap"),
        sequential_overlap.to_string().into(),
    ]);
    args
}

#[allow(clippy::too_many_arguments)]
fn sequential_matching_with_loop_args(
    database: &Path,
    vocabulary_tree: &Path,
    gpu_index: Option<u32>,
    use_gpu_option: &str,
    gpu_index_option: &str,
    sequential_overlap: u32,
    loop_detection_num_images: u32,
    tuning: ColmapQualityTuning,
) -> Vec<OsString> {
    let mut args = sequential_matching_args(
        database,
        gpu_index,
        use_gpu_option,
        gpu_index_option,
        sequential_overlap,
        tuning,
    );
    args.extend([
        OsString::from("--SequentialMatching.loop_detection"),
        OsString::from("1"),
        OsString::from("--SequentialMatching.loop_detection_num_images"),
        loop_detection_num_images.to_string().into(),
        OsString::from("--SequentialMatching.vocab_tree_path"),
        vocabulary_tree.into(),
    ]);
    args
}

fn matching_args(
    matcher: &str,
    database: &Path,
    gpu_index: Option<u32>,
    use_gpu_option: &str,
    gpu_index_option: &str,
    tuning: ColmapQualityTuning,
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
    if tuning.guided_matching {
        let guided_matching_option = if use_gpu_option.starts_with("--FeatureMatching") {
            "--FeatureMatching.guided_matching"
        } else {
            "--SiftMatching.guided_matching"
        };
        args.push(guided_matching_option.into());
        args.push("1".into());
    }
    args
}

#[allow(clippy::too_many_arguments)]
pub async fn map(
    executable: &Path,
    database: &Path,
    images: &Path,
    output: &Path,
    log: PathBuf,
    manager: &ProcessManager,
    observer: Option<ProcessObserver>,
    tuning: ColmapQualityTuning,
) -> Result<()> {
    tokio::fs::create_dir_all(output).await?;
    let args = incremental_mapper_args(database, images, output, tuning);
    run_colmap(
        executable,
        args,
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
    output: &Path,
    tuning: ColmapQualityTuning,
) -> Vec<OsString> {
    let mut args = vec![
        "mapper".into(),
        "--database_path".into(),
        database.into(),
        "--image_path".into(),
        images.into(),
        "--output_path".into(),
        output.into(),
    ];
    if let Some(value) = tuning.ba_local_max_num_iterations {
        args.extend([
            "--Mapper.ba_local_max_num_iterations".into(),
            value.to_string().into(),
        ]);
    }
    if let Some(value) = tuning.ba_local_max_refinements {
        args.extend([
            "--Mapper.ba_local_max_refinements".into(),
            value.to_string().into(),
        ]);
    }
    if let Some(value) = tuning.ba_global_max_num_iterations {
        args.extend([
            "--Mapper.ba_global_max_num_iterations".into(),
            value.to_string().into(),
        ]);
    }
    args
}

pub async fn calibrate_view_graph(
    executable: &Path,
    database: &Path,
    log: PathBuf,
    manager: &ProcessManager,
    observer: Option<ProcessObserver>,
) -> Result<()> {
    run_colmap(
        executable,
        vec![
            "view_graph_calibrator".into(),
            "--database_path".into(),
            database.into(),
        ],
        database.parent().unwrap_or(Path::new(".")),
        log,
        manager,
        observer,
    )
    .await
}

pub async fn map_global(
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
            "global_mapper".into(),
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

pub async fn analyze_model(
    executable: &Path,
    model: &Path,
    manager: &ProcessManager,
) -> Result<String> {
    let output = manager
        .run(ProcessSpec {
            executable: executable.to_path_buf(),
            args: vec!["model_analyzer".into(), "--path".into(), model.into()],
            working_directory: model.parent().map(Path::to_path_buf),
            log_path: None,
            observer: None,
        })
        .await?;
    if !output.success {
        return Err(SplatError::Process(format!(
            "COLMAP model_analyzer failed: {}",
            output.failure_detail()
        )));
    }
    Ok(format!("{}\n{}", output.stdout, output.stderr))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sfm_budget() -> SfmBudget {
        crate::presets::Quality::Balanced.budget().baseline.sfm
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
            sfm_budget(),
            ColmapQualityTuning::default(),
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
            15,
            ColmapQualityTuning::default(),
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
            sfm_budget(),
            ColmapQualityTuning::default(),
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
            15,
            ColmapQualityTuning::default(),
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
            ColmapQualityTuning::default(),
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
    fn loop_matching_uses_the_bundled_vocabulary_tree() {
        let matching = strings(sequential_matching_with_loop_args(
            Path::new("database.db"),
            Path::new("engines/colmap/share/vocab.bin"),
            Some(0),
            "--FeatureMatching.use_gpu",
            "--FeatureMatching.gpu_index",
            20,
            32,
            ColmapQualityTuning::default(),
        ));
        assert!(matching
            .windows(2)
            .any(|pair| pair == ["--SequentialMatching.loop_detection", "1"]));
        assert!(matching.windows(2).any(|pair| {
            pair == [
                "--SequentialMatching.vocab_tree_path",
                "engines/colmap/share/vocab.bin",
            ]
        }));
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
            sfm_budget(),
            ColmapQualityTuning::default(),
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
            sfm_budget(),
            ColmapQualityTuning::default(),
        ));
        assert!(extraction
            .windows(2)
            .any(|pair| pair == ["--ImageReader.mask_path", "../masks"]));
    }

    #[test]
    fn legacy_feature_path_keeps_engine_defaults() {
        let extraction = strings(feature_extraction_args_base(
            Path::new("database.db"),
            Path::new("frames"),
            None,
            None,
            "--FeatureExtraction.use_gpu",
            "--FeatureExtraction.gpu_index",
            None,
            None,
            ColmapQualityTuning::default(),
        ));
        assert!(!extraction.iter().any(|arg| arg.contains("max_image_size")));
        assert!(!extraction
            .iter()
            .any(|arg| arg.contains("max_num_features")));
        assert!(!extraction.iter().any(|arg| arg == "--image_list_path"));
    }

    #[test]
    fn feature_and_matching_budgets_reach_colmap_arguments() {
        let extraction = strings(feature_extraction_args(
            Path::new("database.db"),
            Path::new("frames"),
            None,
            None,
            "--FeatureExtraction.use_gpu",
            "--FeatureExtraction.gpu_index",
            sfm_budget(),
            ColmapQualityTuning::default(),
        ));
        assert!(extraction
            .windows(2)
            .any(|pair| pair == ["--FeatureExtraction.max_image_size", "1920"]));
        assert!(extraction
            .windows(2)
            .any(|pair| pair == ["--SiftExtraction.max_num_features", "8192"]));
        let matching = strings(sequential_matching_args(
            Path::new("database.db"),
            None,
            "--FeatureMatching.use_gpu",
            "--FeatureMatching.gpu_index",
            20,
            ColmapQualityTuning::default(),
        ));
        assert!(matching
            .windows(2)
            .any(|pair| pair == ["--SequentialMatching.overlap", "20"]));
    }

    #[test]
    fn colmap_high_experiment_is_opt_in_and_high_only() {
        assert!(!ColmapQualityTuning::for_run(Quality::High, false).enabled);
        assert!(!ColmapQualityTuning::for_run(Quality::Balanced, true).enabled);
        assert!(ColmapQualityTuning::for_run(Quality::High, true).enabled);
    }

    #[test]
    fn colmap_high_experiment_changes_only_requested_colmap_tuning() {
        let tuning = ColmapQualityTuning::for_run(Quality::High, true);
        let extraction = strings(feature_extraction_args(
            Path::new("database.db"),
            Path::new("frames"),
            None,
            None,
            "--FeatureExtraction.use_gpu",
            "--FeatureExtraction.gpu_index",
            SfmBudget {
                max_image_size: 2_400,
                max_features: 8_192,
            },
            tuning,
        ));
        assert!(extraction
            .windows(2)
            .any(|pair| pair == ["--SiftExtraction.estimate_affine_shape", "1"]));
        assert!(extraction
            .windows(2)
            .any(|pair| pair == ["--SiftExtraction.domain_size_pooling", "0"]));

        let matching = strings(sequential_matching_args(
            Path::new("database.db"),
            None,
            "--FeatureMatching.use_gpu",
            "--FeatureMatching.gpu_index",
            20,
            tuning,
        ));
        assert!(matching
            .windows(2)
            .any(|pair| pair == ["--FeatureMatching.guided_matching", "1"]));

        let mapper = strings(incremental_mapper_args(
            Path::new("database.db"),
            Path::new("frames"),
            Path::new("sparse"),
            tuning,
        ));
        for expected in [
            ["--Mapper.ba_local_max_num_iterations", "30"],
            ["--Mapper.ba_local_max_refinements", "3"],
            ["--Mapper.ba_global_max_num_iterations", "75"],
        ] {
            assert!(mapper.windows(2).any(|pair| pair == expected));
        }
    }

    #[test]
    fn default_tuning_preserves_existing_colmap_arguments() {
        let extraction = strings(feature_extraction_args(
            Path::new("database.db"),
            Path::new("frames"),
            None,
            None,
            "--FeatureExtraction.use_gpu",
            "--FeatureExtraction.gpu_index",
            sfm_budget(),
            ColmapQualityTuning::default(),
        ));
        let matching = strings(matching_args(
            "exhaustive_matcher",
            Path::new("database.db"),
            None,
            "--FeatureMatching.use_gpu",
            "--FeatureMatching.gpu_index",
            ColmapQualityTuning::default(),
        ));
        let mapper = strings(incremental_mapper_args(
            Path::new("database.db"),
            Path::new("frames"),
            Path::new("sparse"),
            ColmapQualityTuning::default(),
        ));
        assert!(!extraction.iter().any(|arg| arg.contains("affine_shape")));
        assert!(!extraction
            .iter()
            .any(|arg| arg.contains("domain_size_pooling")));
        assert!(!matching.iter().any(|arg| arg.contains("guided_matching")));
        assert!(!mapper.iter().any(|arg| arg.contains("ba_local")));
        assert!(!mapper.iter().any(|arg| arg.contains("ba_global")));
    }

    #[test]
    fn reads_stable_benchmark_metrics_from_colmap_database() {
        let temporary = tempfile::tempdir().unwrap();
        let database = temporary.path().join("database.db");
        let connection = Connection::open(&database).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE images(image_id INTEGER PRIMARY KEY);\
                 CREATE TABLE keypoints(image_id INTEGER PRIMARY KEY, rows INTEGER);\
                 CREATE TABLE matches(pair_id INTEGER PRIMARY KEY, rows INTEGER);\
                 CREATE TABLE two_view_geometries(pair_id INTEGER PRIMARY KEY, rows INTEGER);\
                 INSERT INTO images VALUES(1),(2),(3),(4);\
                 INSERT INTO keypoints VALUES(1,10),(2,20),(3,30),(4,40);\
                 INSERT INTO matches VALUES(1,100),(2,0),(3,50);\
                 INSERT INTO two_view_geometries VALUES(1,80),(2,0),(3,45);",
            )
            .unwrap();
        drop(connection);

        let metrics = analyze_database(&database).unwrap();
        assert_eq!(metrics.image_count, 4);
        assert_eq!(metrics.total_detected_features, 100);
        assert_eq!(metrics.mean_features_per_image, 25.0);
        assert_eq!(metrics.median_features_per_image, 25.0);
        assert_eq!(metrics.raw_match_pairs, 2);
        assert_eq!(metrics.raw_matches, 150);
        assert_eq!(metrics.geometrically_verified_pairs, 2);
        assert_eq!(metrics.verified_correspondences, 125);
    }
}
