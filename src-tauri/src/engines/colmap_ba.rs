//! Caspar is an optional BA accelerator, independent of SIFT GPU support.
use crate::{
    presets::pipeline_optimization_config,
    process::{ProcessManager, ProcessOutput, ProcessSpec},
};
use rusqlite::{Connection, OpenFlags};
use std::{
    collections::HashMap,
    ffi::OsString,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
    time::Duration,
};
use tokio::sync::{Mutex, OnceCell};

#[derive(Clone, Debug)]
pub struct BaSelection {
    pub gpu_index: Option<u32>,
    pub detail: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DatabaseStats {
    pub keypoints: u64,
    pub verified_matches: u64,
}

type ProbeCache = Mutex<HashMap<(PathBuf, u32), Arc<OnceCell<BaSelection>>>>;
static CACHE: OnceLock<ProbeCache> = OnceLock::new();
type FailureCache = Mutex<HashMap<(PathBuf, u32), String>>;
static RUNTIME_FAILURES: OnceLock<FailureCache> = OnceLock::new();

fn cache_key(executable: &Path, index: u32) -> (PathBuf, u32) {
    (
        std::fs::canonicalize(executable).unwrap_or_else(|_| executable.into()),
        index,
    )
}

pub async fn select(executable: &Path, compatible_gpu: Option<u32>) -> BaSelection {
    let Some(index) = compatible_gpu.filter(|_| !cfg!(target_os = "macos")) else {
        return ceres("no compatible Caspar GPU (or macOS CPU build)");
    };
    let key = cache_key(executable, index);
    if let Some(reason) = RUNTIME_FAILURES
        .get_or_init(Default::default)
        .lock()
        .await
        .get(&key)
        .cloned()
    {
        return ceres(&format!("Caspar disabled after mapper failure: {reason}"));
    }
    let cell = CACHE
        .get_or_init(Default::default)
        .lock()
        .await
        .entry(key)
        .or_default()
        .clone();
    cell.get_or_init(|| probe(executable, index)).await.clone()
}

pub async fn select_for_mapper(
    executable: &Path,
    database: &Path,
    compatible_gpu: Option<u32>,
    automatic_optimization: bool,
) -> BaSelection {
    if !automatic_optimization {
        return ceres("automatic optimization is disabled");
    }
    if compatible_gpu.is_none() || cfg!(target_os = "macos") {
        return select(executable, compatible_gpu).await;
    }
    let database = database.to_path_buf();
    let stats =
        match crate::presets::spawn_pipeline_blocking(move || read_database_stats(&database)).await
        {
            Ok(Ok(stats)) => stats,
            Ok(Err(reason)) => return ceres(&format!("database statistics unavailable: {reason}")),
            Err(reason) => return ceres(&format!("database statistics task failed: {reason}")),
        };
    if !meets_caspar_threshold(stats) {
        let thresholds = &pipeline_optimization_config().caspar;
        return ceres(&format!(
            "dataset below Caspar threshold (keypoints={}, verified_matches={}, required keypoints>={}, verified_matches>={})",
            stats.keypoints,
            stats.verified_matches,
            thresholds.minimum_keypoints,
            thresholds.minimum_verified_matches
        ));
    }
    let capability = select(executable, compatible_gpu).await;
    match capability.gpu_index {
        Some(index) => BaSelection {
            gpu_index: Some(index),
            detail: format!(
                "BA: local Ceres, global Caspar GPU {index}; execution probe passed; keypoints={}, verified_matches={}",
                stats.keypoints, stats.verified_matches
            ),
        },
        None => capability,
    }
}

pub fn meets_caspar_threshold(stats: DatabaseStats) -> bool {
    let thresholds = &pipeline_optimization_config().caspar;
    stats.keypoints >= thresholds.minimum_keypoints
        && stats.verified_matches >= thresholds.minimum_verified_matches
}

pub async fn disable_after_runtime_failure(executable: &Path, index: u32, reason: &str) {
    RUNTIME_FAILURES
        .get_or_init(Default::default)
        .lock()
        .await
        .insert(cache_key(executable, index), reason.to_owned());
}

pub fn read_database_stats(database: &Path) -> Result<DatabaseStats, String> {
    let connection = Connection::open_with_flags(
        database,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| error.to_string())?;
    let sum = |table: &str| -> Result<u64, String> {
        let sql = format!("SELECT COALESCE(SUM(rows), 0) FROM {table}");
        let value: i64 = connection
            .query_row(&sql, [], |row| row.get(0))
            .map_err(|error| error.to_string())?;
        u64::try_from(value).map_err(|_| format!("negative row total in {table}"))
    };
    Ok(DatabaseStats {
        keypoints: sum("keypoints")?,
        verified_matches: sum("two_view_geometries")?,
    })
}

fn ceres(reason: &str) -> BaSelection {
    BaSelection {
        gpu_index: None,
        detail: format!("BA: Ceres CPU; {reason}"),
    }
}

async fn bounded_command(executable: &Path, args: Vec<OsString>) -> Result<ProcessOutput, String> {
    let manager = ProcessManager::new();
    let environment = [
        ("GLOG_v".into(), "2".into()),
        ("GLOG_logtostderr".into(), "1".into()),
    ];
    let run = manager.run_with_environment(
        ProcessSpec {
            executable: executable.into(),
            args,
            working_directory: executable.parent().map(Path::to_path_buf),
            log_path: None,
            observer: None,
        },
        &environment,
    );
    tokio::pin!(run);
    tokio::select! {
        output = &mut run => output.map_err(|error| error.to_string()),
        _ = tokio::time::sleep(Duration::from_secs(15)) => {
            // Let ProcessManager reap the child/process tree before deleting its fixture.
            manager.cancel();
            let _ = run.await;
            Err("Caspar probe timed out".into())
        }
    }
}

async fn probe(executable: &Path, index: u32) -> BaSelection {
    let result = async {
        let help = bounded_command(executable, vec!["bundle_adjuster".into(), "-h".into()]).await?;
        if !help.success
            || !format!("{}{}", help.stdout, help.stderr)
                .contains("--BundleAdjustmentCaspar.gpu_index")
        {
            return Err("Caspar was not compiled into COLMAP".into());
        }
        let mapper_help = bounded_command(executable, vec!["mapper".into(), "-h".into()]).await?;
        let mapper_text = format!("{}{}", mapper_help.stdout, mapper_help.stderr);
        if !mapper_help.success
            || ![
                "--Mapper.ba_local_backend",
                "--Mapper.ba_global_backend",
                "--Mapper.ba_gpu_index",
            ]
            .iter()
            .all(|option| mapper_text.contains(option))
        {
            return Err("COLMAP mapper is missing Caspar backend options".into());
        }
        let directory = ProbeDirectory(
            std::env::temp_dir().join(format!("ooosplat-caspar-{}", uuid::Uuid::new_v4())),
        );
        let input = directory.0.join("input");
        let output = directory.0.join("output");
        tokio::fs::create_dir_all(&input)
            .await
            .map_err(|e| e.to_string())?;
        tokio::fs::create_dir_all(&output)
            .await
            .map_err(|e| e.to_string())?;
        for (name, contents) in fixture() {
            tokio::fs::write(input.join(name), contents)
                .await
                .map_err(|e| e.to_string())?;
        }
        let run = bounded_command(
            executable,
            vec![
                "bundle_adjuster".into(),
                "--input_path".into(),
                input.into_os_string(),
                "--output_path".into(),
                output.clone().into_os_string(),
                "--BundleAdjustment.backend".into(),
                "CASPAR".into(),
                "--log_level".into(),
                "2".into(),
                "--BundleAdjustmentCaspar.gpu_index".into(),
                index.to_string().into(),
            ],
        )
        .await?;
        if !run.success
            || !output.join("points3D.bin").is_file()
            || !format!("{}{}", run.stdout, run.stderr).contains("Creating Caspar bundle adjuster")
        {
            return Err(format!("Caspar execution failed: {}", run.failure_detail()));
        }
        Ok(())
    }
    .await;
    match result {
        Ok(()) => BaSelection {
            gpu_index: Some(index),
            detail: format!("BA: Caspar GPU {index}; execution probe passed"),
        },
        Err(reason) => ceres(&reason),
    }
}

struct ProbeDirectory(PathBuf);
impl Drop for ProbeDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// Non-planar, shared SIMPLE_RADIAL model; no image files or database are needed
// by bundle_adjuster. Text-model legacy loading creates trivial rigs/frames.
fn fixture() -> [(&'static str, String); 3] {
    let points = [
        (-0.5, -0.3, 3.0),
        (0.4, -0.2, 3.5),
        (-0.2, 0.4, 4.0),
        (0.5, 0.3, 4.5),
        (0.0, -0.5, 5.0),
        (-0.4, 0.1, 5.5),
        (0.3, 0.0, 3.8),
        (0.1, 0.5, 4.8),
    ];
    let mut images = String::new();
    for id in 1..=3 {
        let tx = (id - 1) as f64 * 0.25;
        images.push_str(&format!("{id} 1 0 0 0 {tx} 0 0 1 probe{id}.png\n"));
        for (i, (x, y, z)) in points.iter().enumerate() {
            images.push_str(&format!(
                "{} {} {} ",
                500.0 * (x + tx) / z + 320.0,
                500.0 * y / z + 240.0,
                i + 1
            ));
        }
        images.push('\n');
    }
    let mut model_points = String::new();
    for (i, (x, y, z)) in points.iter().enumerate() {
        model_points.push_str(&format!(
            "{} {} {y} {z} 128 128 128 0 1 {i} 2 {i} 3 {i}\n",
            i + 1,
            x + 0.002
        ));
    }
    [
        (
            "cameras.txt",
            "1 SIMPLE_RADIAL 640 480 500 320 240 0\n".into(),
        ),
        ("images.txt", images),
        ("points3D.txt", model_points),
    ]
}

pub fn append_mapper_options(args: &mut Vec<OsString>, selection: &BaSelection) {
    // Ceres CPU is the upstream default, also preserving older developer CLIs.
    if let Some(index) = selection.gpu_index {
        args.extend([
            "--Mapper.ba_local_backend".into(),
            "CERES".into(),
            "--Mapper.ba_global_backend".into(),
            "CASPAR".into(),
            "--Mapper.ba_gpu_index".into(),
            index.to_string().into(),
            "--log_level".into(),
            "2".into(),
        ]);
    }
}

pub fn append_explicit_ceres_options(args: &mut Vec<OsString>) {
    args.extend([
        "--Mapper.ba_local_backend".into(),
        "CERES".into(),
        "--Mapper.ba_global_backend".into(),
        "CERES".into(),
    ]);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn caspar_sets_local_ceres_and_global_caspar_without_changing_iterations() {
        let mut args = vec![];
        append_mapper_options(
            &mut args,
            &BaSelection {
                gpu_index: Some(2),
                detail: String::new(),
            },
        );
        assert_eq!(
            args,
            [
                "--Mapper.ba_local_backend",
                "CERES",
                "--Mapper.ba_global_backend",
                "CASPAR",
                "--Mapper.ba_gpu_index",
                "2",
                "--log_level",
                "2"
            ]
            .map(OsString::from)
        );
        append_mapper_options(&mut args, &ceres("probe failed"));
        assert_eq!(args.len(), 8);
    }
    #[test]
    fn retry_explicitly_sets_both_backends_to_ceres() {
        let mut args = vec![OsString::from("mapper")];
        append_explicit_ceres_options(&mut args);
        assert_eq!(
            args,
            [
                "mapper",
                "--Mapper.ba_local_backend",
                "CERES",
                "--Mapper.ba_global_backend",
                "CERES",
            ]
            .map(OsString::from)
        );
    }
    #[test]
    fn database_statistics_sum_keypoints_and_verified_inlier_matches() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("database.db");
        let connection = Connection::open(&database).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE keypoints(rows INTEGER NOT NULL);\
                 CREATE TABLE two_view_geometries(rows INTEGER NOT NULL);\
                 INSERT INTO keypoints VALUES (1200000), (800000);\
                 INSERT INTO two_view_geometries VALUES (1500000), (500000);",
            )
            .unwrap();
        drop(connection);
        let stats = read_database_stats(&database).unwrap();
        assert_eq!(
            stats,
            DatabaseStats {
                keypoints: 2_000_000,
                verified_matches: 2_000_000
            }
        );
        assert!(meets_caspar_threshold(stats));
    }
    #[test]
    fn both_caspar_thresholds_are_inclusive_and_required() {
        assert!(meets_caspar_threshold(DatabaseStats {
            keypoints: 2_000_000,
            verified_matches: 2_000_000
        }));
        assert!(!meets_caspar_threshold(DatabaseStats {
            keypoints: 1_999_999,
            verified_matches: 2_000_000
        }));
        assert!(!meets_caspar_threshold(DatabaseStats {
            keypoints: 2_000_000,
            verified_matches: 1_999_999
        }));
    }
    #[test]
    fn invalid_database_statistics_fail_closed() {
        let directory = tempfile::tempdir().unwrap();
        let missing_table = directory.path().join("missing.db");
        Connection::open(&missing_table).unwrap();
        assert!(read_database_stats(&missing_table).is_err());

        let negative = directory.path().join("negative.db");
        let connection = Connection::open(&negative).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE keypoints(rows INTEGER NOT NULL);\
                 CREATE TABLE two_view_geometries(rows INTEGER NOT NULL);\
                 INSERT INTO keypoints VALUES (-1);",
            )
            .unwrap();
        drop(connection);
        assert!(read_database_stats(&negative).is_err());
    }
    #[tokio::test]
    async fn disabled_automatic_optimization_does_not_read_the_database_or_probe() {
        let selection = select_for_mapper(
            Path::new("missing-colmap"),
            Path::new("missing-database"),
            Some(0),
            false,
        )
        .await;
        assert_eq!(selection.gpu_index, None);
        assert!(selection
            .detail
            .contains("automatic optimization is disabled"));
    }
    #[tokio::test]
    async fn missing_compatible_gpu_does_not_read_the_database() {
        let selection = select_for_mapper(
            Path::new("missing-colmap"),
            Path::new("missing-database"),
            None,
            true,
        )
        .await;
        assert_eq!(selection.gpu_index, None);
        assert!(selection.detail.contains("no compatible Caspar GPU"));
    }
    #[cfg(not(target_os = "macos"))]
    #[tokio::test]
    async fn undersized_database_does_not_start_the_caspar_probe() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("database.db");
        let connection = Connection::open(&database).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE keypoints(rows INTEGER NOT NULL);\
                 CREATE TABLE two_view_geometries(rows INTEGER NOT NULL);\
                 INSERT INTO keypoints VALUES (1999999);\
                 INSERT INTO two_view_geometries VALUES (2000000);",
            )
            .unwrap();
        drop(connection);
        let selection =
            select_for_mapper(Path::new("missing-colmap"), &database, Some(0), true).await;
        assert_eq!(selection.gpu_index, None);
        assert!(selection.detail.contains("below Caspar threshold"));
    }
    #[cfg(not(target_os = "macos"))]
    #[tokio::test]
    async fn mapper_failure_disables_cached_caspar_capability_for_the_process() {
        let executable =
            std::env::temp_dir().join(format!("runtime-failed-colmap-{}", uuid::Uuid::new_v4()));
        disable_after_runtime_failure(&executable, 4, "Caspar solver failed").await;
        let selection = select(&executable, Some(4)).await;
        assert_eq!(selection.gpu_index, None);
        assert!(selection.detail.contains("disabled after mapper failure"));
    }
    #[tokio::test]
    async fn cpu_selection_does_not_start_colmap() {
        let result = select(Path::new("missing-colmap"), None).await;
        assert_eq!(result.gpu_index, None);
        assert!(result.detail.contains("Ceres CPU"));
    }
    #[cfg(not(target_os = "macos"))]
    #[tokio::test]
    async fn failed_probe_is_cached_and_does_not_change_the_sift_device() {
        let executable =
            std::env::temp_dir().join(format!("missing-colmap-{}", uuid::Uuid::new_v4()));
        let sift_device = Some(2);
        let (first, second) = tokio::join!(
            select(&executable, sift_device),
            select(&executable, sift_device)
        );
        assert_eq!(first.gpu_index, None);
        assert_eq!(first.detail, second.detail);
        assert!(first.detail.contains("Ceres CPU"));
        assert_eq!(sift_device, Some(2));
        let cache = CACHE.get().unwrap().lock().await;
        assert!(cache.get(&(executable, 2)).unwrap().get().is_some());
    }
    #[test]
    fn fixture_has_observations_and_bidirectional_tracks() {
        let files = fixture();
        assert!(files[0].1.contains("SIMPLE_RADIAL"));
        assert_eq!(files[1].1.lines().count(), 6);
        assert_eq!(files[2].1.lines().count(), 8);
    }
}
