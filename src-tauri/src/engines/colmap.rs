use std::{
    collections::HashMap,
    ffi::OsString,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

use tokio_util::sync::CancellationToken;

use crate::{
    error::{Result, SplatError},
    process::{ProcessManager, ProcessObserver, ProcessSpec},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MapperBackend {
    Global,
    Incremental,
}

impl MapperBackend {
    pub const fn command(self) -> &'static str {
        match self {
            Self::Global => "global_mapper",
            Self::Incremental => "mapper",
        }
    }
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

/// Parameter-name prefixes for the SIFT families plus the tuning knobs the
/// pipeline configures from a quality preset.
///
/// A COLMAP build exposes one SIFT family for feature extraction and for every
/// matcher at once, so a single probe per executable answers both questions and
/// no caller needs to hardcode a `--SiftExtraction`/`--FeatureExtraction` prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColmapCliCapabilities {
    family: ColmapCliFamily,
    /// `Some(true)` = the subcommand answered, `Some(false)` = it is absent in
    /// this build, `None` = the probe failed and the answer is still unknown.
    /// An unknown answer must never be cached as "unsupported".
    view_graph_calibrator: Option<bool>,
}

impl ColmapCliCapabilities {
    pub const fn from_family(family: ColmapCliFamily) -> Self {
        Self {
            family,
            view_graph_calibrator: None,
        }
    }

    const fn with_view_graph_calibrator(self, supported: Option<bool>) -> Self {
        Self {
            family: self.family,
            view_graph_calibrator: supported,
        }
    }

    pub const fn family(self) -> ColmapCliFamily {
        self.family
    }

    /// Whether `view_graph_calibrator` is known to exist in this build.
    ///
    /// The global mapper warns that it depends on focal-length priors and
    /// recommends running this step first. Frames extracted by FFmpeg carry no
    /// EXIF, so the cameras COLMAP creates start without priors, and this step is
    /// worth running wherever the subcommand actually exists.
    pub const fn has_view_graph_calibrator(self) -> bool {
        matches!(self.view_graph_calibrator, Some(true))
    }

    /// Whether every probe produced a definite answer, so the result is safe to
    /// cache for the rest of the process.
    const fn is_conclusive(self) -> bool {
        self.view_graph_calibrator.is_some()
    }

    fn max_image_size_option(self) -> &'static str {
        match self.family {
            ColmapCliFamily::Legacy39 => "--SiftExtraction.max_image_size",
            // COLMAP 4.x split this one option into the FeatureExtraction
            // namespace while the remaining SIFT knobs stayed under SiftExtraction.
            ColmapCliFamily::Modern4 => "--FeatureExtraction.max_image_size",
        }
    }

    fn max_num_features_option(self) -> &'static str {
        "--SiftExtraction.max_num_features"
    }

    fn estimate_affine_shape_option(self) -> &'static str {
        "--SiftExtraction.estimate_affine_shape"
    }

    fn domain_size_pooling_option(self) -> &'static str {
        "--SiftExtraction.domain_size_pooling"
    }

    fn feature_gpu_options(self) -> (&'static str, &'static str) {
        match self.family {
            ColmapCliFamily::Legacy39 => ("--SiftExtraction.use_gpu", "--SiftExtraction.gpu_index"),
            ColmapCliFamily::Modern4 => (
                "--FeatureExtraction.use_gpu",
                "--FeatureExtraction.gpu_index",
            ),
        }
    }

    fn matching_gpu_options(self) -> (&'static str, &'static str) {
        match self.family {
            ColmapCliFamily::Legacy39 => ("--SiftMatching.use_gpu", "--SiftMatching.gpu_index"),
            ColmapCliFamily::Modern4 => {
                ("--FeatureMatching.use_gpu", "--FeatureMatching.gpu_index")
            }
        }
    }
}

/// Feature-extraction limits taken from the selected quality preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeatureExtractionTuning {
    /// Upper bound on the longest image edge handed to SIFT. Caps memory and time.
    pub max_image_size: u32,
    /// Upper bound on features per image. Lower values degrade pose accuracy.
    pub max_num_features: u32,
}

/// Sequential-matcher tuning taken from the selected quality preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SequentialMatchingTuning {
    /// Number of neighbouring frames each frame is matched against.
    pub overlap: u32,
}

struct CapabilityCache {
    entries: Mutex<HashMap<PathBuf, Option<ColmapCliCapabilities>>>,
    probe: tokio::sync::Mutex<()>,
}

fn capability_cache() -> &'static CapabilityCache {
    static CACHE: OnceLock<CapabilityCache> = OnceLock::new();
    CACHE.get_or_init(|| CapabilityCache {
        entries: Mutex::new(HashMap::new()),
        probe: tokio::sync::Mutex::new(()),
    })
}

fn normalized_executable_path(executable: &Path) -> PathBuf {
    executable
        .canonicalize()
        .unwrap_or_else(|_| executable.to_path_buf())
}

fn cached_capabilities(key: &Path) -> Option<Option<ColmapCliCapabilities>> {
    capability_cache()
        .entries
        .lock()
        .ok()
        .and_then(|entries| entries.get(key).copied())
}

fn unsupported_cli_error() -> SplatError {
    SplatError::UnsupportedEngine(
        "COLMAP 既不支持 FeatureExtraction.* 也不支持 SiftExtraction.* 参数族".into(),
    )
}

/// Resolves the CLI capabilities of one COLMAP executable.
///
/// The probe runs inside a single-flight lock and its result — including a
/// negative one — is cached per normalized executable path, so repeated calls
/// from the UI or from concurrent pipeline stages never spawn `-h` twice.
pub async fn cli_capabilities(
    executable: &Path,
    manager: &ProcessManager,
) -> Result<ColmapCliCapabilities> {
    let cache = capability_cache();
    let key = normalized_executable_path(executable);
    if let Some(cached) = cached_capabilities(&key) {
        return cached.ok_or_else(unsupported_cli_error);
    }
    let _probe = cache.probe.lock().await;
    if let Some(cached) = cached_capabilities(&key) {
        return cached.ok_or_else(unsupported_cli_error);
    }
    let feature_help = command_help(executable, "feature_extractor", manager).await?;
    let matching_help = command_help(executable, "sequential_matcher", manager).await?;
    // A missing subcommand is not a capability failure, so it is probed
    // separately: `Ok(false)` is a definite absence and stays cacheable, while a
    // probe that could not run stays `None` so it is never cached as "unsupported".
    let view_graph_calibrator = command_supported(executable, "view_graph_calibrator", manager)
        .await
        .ok();
    let detected = detect_cli_family(&feature_help, &matching_help).map(|family| {
        ColmapCliCapabilities::from_family(family).with_view_graph_calibrator(view_graph_calibrator)
    });
    // Cache the answer when it is definite: an unsupported family is a real
    // negative, and so is a calibrator flag that was actually observed.
    let cacheable = match detected {
        None => true,
        Some(capabilities) => capabilities.is_conclusive(),
    };
    if cacheable {
        if let Ok(mut entries) = cache.entries.lock() {
            entries.insert(key, detected);
        }
    }
    detected.ok_or_else(unsupported_cli_error)
}

/// Whether a subcommand exists in this build.
///
/// A non-zero exit is a definite "no" — COLMAP reports an unknown command that
/// way — while a spawn, job-object or cancellation failure returns `Err`, because
/// nothing about the build was learned. Only definite answers may be cached.
async fn command_supported(
    executable: &Path,
    command_name: &str,
    manager: &ProcessManager,
) -> Result<bool> {
    let output = manager
        .run(ProcessSpec {
            executable: executable.to_path_buf(),
            args: vec![command_name.into(), "-h".into()],
            working_directory: executable.parent().map(Path::to_path_buf),
            log_path: None,
            observer: None,
        })
        .await?;
    Ok(output.success)
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
    cancellation: Option<CancellationToken>,
) -> Result<()> {
    let output = manager
        .run_with_cancellation(
            ProcessSpec {
                executable: executable.to_path_buf(),
                args,
                working_directory: Some(working_directory.to_path_buf()),
                log_path: Some(log_path),
                observer,
            },
            cancellation,
        )
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
    tuning: FeatureExtractionTuning,
) -> Result<()> {
    let capabilities = cli_capabilities(executable, manager).await?;
    run_colmap(
        executable,
        feature_extraction_args(capabilities, database, images, masks, gpu_index, tuning),
        database.parent().unwrap_or(images),
        log,
        manager,
        observer,
        None,
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
    tuning: SequentialMatchingTuning,
) -> Result<()> {
    let capabilities = cli_capabilities(executable, manager).await?;
    run_colmap(
        executable,
        sequential_matching_args(capabilities, database, gpu_index, tuning),
        database.parent().unwrap_or(Path::new(".")),
        log,
        manager,
        observer,
        None,
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
    let capabilities = cli_capabilities(executable, manager).await?;
    run_colmap(
        executable,
        matching_args(capabilities, "exhaustive_matcher", database, gpu_index),
        database.parent().unwrap_or(Path::new(".")),
        log,
        manager,
        observer,
        None,
    )
    .await
}

fn feature_extraction_args(
    capabilities: ColmapCliCapabilities,
    database: &Path,
    images: &Path,
    masks: Option<&Path>,
    gpu_index: Option<u32>,
    tuning: FeatureExtractionTuning,
) -> Vec<OsString> {
    let (use_gpu_option, gpu_index_option) = capabilities.feature_gpu_options();
    let max_image_size_option = capabilities.max_image_size_option();
    let max_num_features_option = capabilities.max_num_features_option();
    let estimate_affine_shape_option = capabilities.estimate_affine_shape_option();
    let domain_size_pooling_option = capabilities.domain_size_pooling_option();
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
    // SIFT limits. The parameter prefix depends on the detected CLI family, so it
    // is never hardcoded here:
    // - max_image_size bounds the longest edge SIFT runs on, capping memory and
    //   extraction time for high-resolution phone footage.
    // - max_num_features bounds features per image. Values below 8192 measurably
    //   degrade pose accuracy, so the presets stay at or above that floor.
    // - estimate_affine_shape and domain_size_pooling are expensive and only pay
    //   off for strongly viewpoint-dependent texture, so both stay off.
    args.extend([
        max_image_size_option.into(),
        tuning.max_image_size.to_string().into(),
        max_num_features_option.into(),
        tuning.max_num_features.to_string().into(),
        estimate_affine_shape_option.into(),
        "0".into(),
        domain_size_pooling_option.into(),
        "0".into(),
    ]);
    args
}

fn sequential_matching_args(
    capabilities: ColmapCliCapabilities,
    database: &Path,
    gpu_index: Option<u32>,
    tuning: SequentialMatchingTuning,
) -> Vec<OsString> {
    let mut args = matching_args(capabilities, "sequential_matcher", database, gpu_index);
    // A larger overlap improves matching completeness and registration ratio but
    // costs matching time linearly. Smart frame selection already removed 30-50%
    // of the frames, so the freed budget is spent here.
    // quadratic_overlap weights nearby frames higher, which follows real camera
    // motion far better than a flat window and is essentially free.
    args.extend([
        OsString::from("--SequentialMatching.overlap"),
        OsString::from(tuning.overlap.to_string()),
        OsString::from("--SequentialMatching.quadratic_overlap"),
        OsString::from("1"),
    ]);
    args
}

fn matching_args(
    capabilities: ColmapCliCapabilities,
    matcher: &str,
    database: &Path,
    gpu_index: Option<u32>,
) -> Vec<OsString> {
    let (use_gpu_option, gpu_index_option) = capabilities.matching_gpu_options();
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

fn view_graph_calibration_args(database: &Path) -> Vec<OsString> {
    // The calibrator works on the database in place: it estimates intrinsics from
    // the match graph and records them as focal-length priors, which is the
    // precondition global_mapper warns about.
    vec![
        "view_graph_calibrator".into(),
        "--database_path".into(),
        database.into(),
    ]
}

/// Estimates camera intrinsics from the match graph and stores them as focal
/// length priors in the database, for the global mapper that runs next.
///
/// COLMAP's own guidance for `global_mapper` is to run this first. It is a
/// best-effort preparation step: callers treat a failure as non-fatal, because
/// the global mapper still runs without priors and the registered-ratio check
/// decides whether the result is usable.
pub async fn calibrate_view_graph(
    executable: &Path,
    database: &Path,
    log: PathBuf,
    manager: &ProcessManager,
    observer: Option<ProcessObserver>,
) -> Result<()> {
    run_colmap(
        executable,
        view_graph_calibration_args(database),
        database.parent().unwrap_or(Path::new(".")),
        log,
        manager,
        observer,
        None,
    )
    .await
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
    map_with_backend(
        MapperBackend::Incremental,
        executable,
        database,
        images,
        output,
        log,
        manager,
        observer,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn map_with_backend(
    backend: MapperBackend,
    executable: &Path,
    database: &Path,
    images: &Path,
    output: &Path,
    log: PathBuf,
    manager: &ProcessManager,
    observer: Option<ProcessObserver>,
    // Optional per-call token so a caller can give up on **this** mapper run alone
    // (e.g. a time budget) and still fall back to another backend afterwards.
    cancellation: Option<CancellationToken>,
) -> Result<()> {
    tokio::fs::create_dir_all(output).await?;
    run_colmap(
        executable,
        vec![
            backend.command().into(),
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
        cancellation,
    )
    .await
}

pub async fn supports_global_mapper(executable: &Path, manager: &ProcessManager) -> bool {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, bool>>> = OnceLock::new();
    let key = normalized_executable_path(executable);
    if let Some(value) = CACHE
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .ok()
        .and_then(|cache| cache.get(&key).copied())
    {
        return value;
    }
    // Serialize concurrent misses so a single `global_mapper -h` probe is shared
    // instead of one process per caller.
    let _probe = capability_cache().probe.lock().await;
    if let Some(value) = CACHE
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .ok()
        .and_then(|cache| cache.get(&key).copied())
    {
        return value;
    }
    let supported = match command_help(executable, "global_mapper", manager).await {
        Ok(_) => true,
        Err(_) => return false,
    };
    if let Ok(mut cache) = CACHE.get_or_init(|| Mutex::new(HashMap::new())).lock() {
        cache.insert(key, supported);
    }
    supported
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: Vec<OsString>) -> Vec<String> {
        args.into_iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    fn modern4() -> ColmapCliCapabilities {
        ColmapCliCapabilities::from_family(ColmapCliFamily::Modern4)
    }

    fn legacy39() -> ColmapCliCapabilities {
        ColmapCliCapabilities::from_family(ColmapCliFamily::Legacy39)
    }

    fn tuning() -> FeatureExtractionTuning {
        FeatureExtractionTuning {
            max_image_size: 1600,
            max_num_features: 8192,
        }
    }

    fn overlap(overlap: u32) -> SequentialMatchingTuning {
        SequentialMatchingTuning { overlap }
    }

    #[test]
    fn view_graph_calibration_targets_the_database_in_place() {
        let calibration = strings(view_graph_calibration_args(Path::new("database.db")));
        assert_eq!(calibration[0], "view_graph_calibrator");
        assert!(calibration
            .windows(2)
            .any(|pair| pair == ["--database_path", "database.db"]));
        // The calibrator has no output path: it writes the priors back into the
        // database the mapper reads next.
        assert!(!calibration.iter().any(|arg| arg == "--output_path"));
    }

    #[test]
    fn capabilities_default_to_no_view_graph_calibrator() {
        assert!(!modern4().has_view_graph_calibrator());
        assert_eq!(modern4().family(), ColmapCliFamily::Modern4);
    }

    #[test]
    fn mapper_backend_selects_only_the_subcommand() {
        assert_eq!(MapperBackend::Global.command(), "global_mapper");
        assert_eq!(MapperBackend::Incremental.command(), "mapper");
    }

    #[test]
    fn mapper_backend_is_serializable() {
        assert_eq!(
            serde_json::to_string(&MapperBackend::Global).unwrap(),
            "\"global\""
        );
        assert_eq!(
            serde_json::to_string(&MapperBackend::Incremental).unwrap(),
            "\"incremental\""
        );
    }

    #[test]
    fn gpu_mode_sets_use_gpu_and_selected_index() {
        let extraction = strings(feature_extraction_args(
            modern4(),
            Path::new("database.db"),
            Path::new("frames"),
            None,
            Some(2),
            tuning(),
        ));
        assert!(extraction
            .windows(2)
            .any(|pair| pair == ["--FeatureExtraction.use_gpu", "1"]));
        assert!(extraction
            .windows(2)
            .any(|pair| pair == ["--FeatureExtraction.gpu_index", "2"]));

        let matching = strings(sequential_matching_args(
            modern4(),
            Path::new("database.db"),
            Some(2),
            overlap(15),
        ));
        assert!(matching
            .windows(2)
            .any(|pair| pair == ["--FeatureMatching.use_gpu", "1"]));
        assert!(matching
            .windows(2)
            .any(|pair| pair == ["--FeatureMatching.gpu_index", "2"]));
    }

    #[test]
    fn modern_family_uses_the_feature_extraction_prefix() {
        let extraction = strings(feature_extraction_args(
            modern4(),
            Path::new("database.db"),
            Path::new("frames"),
            None,
            Some(0),
            tuning(),
        ));
        assert!(extraction
            .windows(2)
            .any(|pair| pair == ["--FeatureExtraction.max_image_size", "1600"]));
        assert!(extraction
            .windows(2)
            .any(|pair| pair == ["--SiftExtraction.max_num_features", "8192"]));
        assert!(extraction
            .windows(2)
            .any(|pair| pair == ["--SiftExtraction.estimate_affine_shape", "0"]));
        assert!(extraction
            .windows(2)
            .any(|pair| pair == ["--SiftExtraction.domain_size_pooling", "0"]));
    }

    #[test]
    fn sequential_matching_passes_overlap_and_quadratic_overlap() {
        let matching = strings(sequential_matching_args(
            modern4(),
            Path::new("database.db"),
            Some(0),
            overlap(20),
        ));
        assert!(matching
            .windows(2)
            .any(|pair| pair == ["--SequentialMatching.overlap", "20"]));
        assert!(matching
            .windows(2)
            .any(|pair| pair == ["--SequentialMatching.quadratic_overlap", "1"]));
    }

    #[test]
    fn cpu_mode_disables_gpu_without_passing_an_index() {
        let extraction = strings(feature_extraction_args(
            modern4(),
            Path::new("database.db"),
            Path::new("frames"),
            None,
            None,
            tuning(),
        ));
        assert!(extraction
            .windows(2)
            .any(|pair| pair == ["--FeatureExtraction.use_gpu", "0"]));
        assert!(!extraction
            .iter()
            .any(|arg| arg == "--FeatureExtraction.gpu_index"));
        // Non-GPU tuning parameters must survive a CPU-only run.
        assert!(extraction
            .windows(2)
            .any(|pair| pair == ["--FeatureExtraction.max_image_size", "1600"]));
        assert!(extraction
            .windows(2)
            .any(|pair| pair == ["--SiftExtraction.max_num_features", "8192"]));
        assert!(extraction
            .iter()
            .any(|arg| arg == "--ImageReader.camera_model"));

        let matching = strings(sequential_matching_args(
            modern4(),
            Path::new("database.db"),
            None,
            overlap(15),
        ));
        assert!(matching
            .windows(2)
            .any(|pair| pair == ["--FeatureMatching.use_gpu", "0"]));
        assert!(matching
            .windows(2)
            .any(|pair| pair == ["--SequentialMatching.overlap", "15"]));
        assert!(matching
            .windows(2)
            .any(|pair| pair == ["--SequentialMatching.quadratic_overlap", "1"]));
        assert!(!matching
            .iter()
            .any(|arg| arg == "--FeatureMatching.gpu_index"));
    }

    #[test]
    fn exhaustive_matching_uses_the_selected_gpu_without_video_options() {
        let matching = strings(matching_args(
            modern4(),
            "exhaustive_matcher",
            Path::new("database.db"),
            Some(1),
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
            legacy39(),
            Path::new("database.db"),
            Path::new("frames"),
            None,
            Some(0),
            tuning(),
        ));
        assert!(extraction
            .windows(2)
            .any(|pair| pair == ["--SiftExtraction.use_gpu", "1"]));
        assert!(extraction
            .windows(2)
            .any(|pair| pair == ["--SiftExtraction.gpu_index", "0"]));
        // The legacy family must use its own SIFT limits, never the modern prefix.
        assert!(extraction
            .windows(2)
            .any(|pair| pair == ["--SiftExtraction.max_image_size", "1600"]));
        assert!(extraction
            .windows(2)
            .any(|pair| pair == ["--SiftExtraction.max_num_features", "8192"]));
        assert!(extraction
            .windows(2)
            .any(|pair| pair == ["--SiftExtraction.estimate_affine_shape", "0"]));
        assert!(extraction
            .windows(2)
            .any(|pair| pair == ["--SiftExtraction.domain_size_pooling", "0"]));
        assert!(!extraction
            .iter()
            .any(|arg| arg.starts_with("--FeatureExtraction.")));

        let matching = strings(sequential_matching_args(
            legacy39(),
            Path::new("database.db"),
            Some(0),
            overlap(12),
        ));
        assert!(matching
            .windows(2)
            .any(|pair| pair == ["--SiftMatching.use_gpu", "1"]));
        assert!(matching
            .windows(2)
            .any(|pair| pair == ["--SiftMatching.gpu_index", "0"]));
    }

    #[test]
    fn transparent_input_passes_the_colmap_mask_root() {
        let extraction = strings(feature_extraction_args(
            modern4(),
            Path::new("database.db"),
            Path::new("../frames"),
            Some(Path::new("../masks")),
            None,
            tuning(),
        ));
        assert!(extraction
            .windows(2)
            .any(|pair| pair == ["--ImageReader.mask_path", "../masks"]));
    }
}
