//! Caspar is an optional BA accelerator, independent of SIFT GPU support.
use crate::{
    presets::pipeline_optimization_config,
    process::{ProcessManager, ProcessOutput, ProcessSpec},
};
use rusqlite::{Connection, OpenFlags};
use std::{
    collections::{HashMap, HashSet},
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

pub(crate) fn ceres_selection(reason: &str) -> BaSelection {
    ceres(reason)
}

fn argument<'a>(args: &'a [OsString], key: &str) -> Result<Option<&'a std::ffi::OsStr>, String> {
    let mut found = None;
    for (index, item) in args.iter().enumerate() {
        if item == key {
            if found.is_some() {
                return Err(format!("duplicate mapper option {key}"));
            }
            found = Some(
                args.get(index + 1)
                    .ok_or_else(|| format!("missing mapper option value {key}"))?
                    .as_os_str(),
            );
        }
    }
    Ok(found)
}

fn boolean_argument(args: &[OsString], key: &str, default: bool) -> Result<bool, String> {
    match argument(args, key)?
        .map(|v| {
            v.to_str()
                .ok_or_else(|| format!("non-text mapper boolean {key}"))
        })
        .transpose()?
    {
        None => Ok(default),
        Some("1" | "true") => Ok(true),
        Some("0" | "false") => Ok(false),
        _ => Err(format!("unknown mapper boolean {key}")),
    }
}

fn argument_path(
    args: &[OsString],
    key: &str,
    working_directory: &Path,
) -> Result<Option<PathBuf>, String> {
    Ok(argument(args, key)?.map(PathBuf::from).map(|path| {
        if path.is_absolute() {
            path
        } else {
            working_directory.join(path)
        }
    }))
}

fn file_names(path: &Path) -> Result<HashSet<String>, String> {
    std::fs::read_to_string(path)
        .map_err(|e| e.to_string())
        .map(|text| {
            text.lines()
                .filter(|v| !v.is_empty())
                .map(String::from)
                .collect()
        })
}

/// Capability of the actual images/models, independent of the ordinary tiny
/// GPU probe. The pinned Caspar adapters only cover PINHOLE/SIMPLE_RADIAL;
/// unsupported cameras are otherwise silently skipped by upstream Caspar.
pub fn mapper_compatibility(
    database: &Path,
    args: &[OsString],
    working_directory: &Path,
) -> Result<(), String> {
    let db = Connection::open_with_flags(
        database,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| e.to_string())?;
    let selected = argument_path(args, "--Mapper.image_list_path", working_directory)?
        .map(|p| file_names(&p))
        .transpose()?;
    if selected.as_ref().is_some_and(HashSet::is_empty) {
        return Err("empty mapper image list".into());
    }
    let mut statement = db.prepare("SELECT images.name, images.camera_id, cameras.model FROM images LEFT JOIN cameras USING(camera_id)")
        .map_err(|e| e.to_string())?;
    let rows = statement
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, u32>(1)?,
                r.get::<_, Option<i32>>(2)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    let mut names = HashSet::new();
    let mut camera_ids = HashSet::new();
    for row in rows {
        let (name, id, model) = row.map_err(|e| e.to_string())?;
        if selected.as_ref().is_some_and(|set| !set.contains(&name)) {
            continue;
        }
        if !names.insert(name) {
            return Err("duplicate mapper image names".into());
        }
        if !matches!(model, Some(1 | 2)) {
            return Err(format!(
                "Caspar does not support camera model {model:?}; using all observations with Ceres"
            ));
        }
        camera_ids.insert(id);
    }
    if names.is_empty() || selected.as_ref().is_some_and(|set| set != &names) {
        return Err("mapper image/camera membership cannot be verified".into());
    }
    if let Some(model) = argument_path(args, "--input_path", working_directory)? {
        for camera in
            crate::reconstruction::colmap_model::read_cameras(&model).map_err(|e| e.to_string())?
        {
            if !matches!(camera.model_id, 1 | 2) {
                return Err(format!(
                    "Caspar does not support input-model camera {}",
                    camera.model
                ));
            }
            camera_ids.insert(camera.id);
        }
        check_model_rigs(&model)?;
    }
    let fixed = argument_path(
        args,
        "--Mapper.constant_camera_list_path",
        working_directory,
    )?
    .map(|p| file_names(&p))
    .transpose()?
    .unwrap_or_default();
    let focal = boolean_argument(args, "--Mapper.ba_refine_focal_length", true)?;
    let extra = boolean_argument(args, "--Mapper.ba_refine_extra_params", true)?;
    if focal != extra && camera_ids.iter().any(|id| !fixed.contains(&id.to_string())) {
        return Err(
            "Caspar requires variable focal length and extra parameters to be refined together"
                .into(),
        );
    }
    if boolean_argument(args, "--Mapper.ba_constant_rig_from_world_rotation", false)? {
        return Err("Caspar does not support constant rig rotation with variable rig poses".into());
    }
    // Current generated factors are verified for trivial reference sensors.
    // Do not assume a successful three-image probe also validates camera rigs.
    let rig_count: i64 = db.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('rigs','rig_sensors','frames','frame_data')", [], |r| r.get(0)).map_err(|e| e.to_string())?;
    if rig_count != 4 {
        return Err("camera rig metadata is unavailable for Caspar verification".into());
    }
    let mut rigs = db
        .prepare("SELECT sensor_id FROM rig_sensors WHERE sensor_type=0")
        .map_err(|e| e.to_string())?;
    for id in rigs
        .query_map([], |r| r.get::<_, u32>(0))
        .map_err(|e| e.to_string())?
    {
        if camera_ids.contains(&id.map_err(|e| e.to_string())?) {
            return Err("non-reference rig sensor capability is not verified for Caspar".into());
        }
    }
    let reference_cameras = db
        .prepare("SELECT ref_sensor_id FROM rigs WHERE ref_sensor_type=0")
        .map_err(|e| e.to_string())?
        .query_map([], |r| r.get::<_, u32>(0))
        .map_err(|e| e.to_string())?
        .collect::<rusqlite::Result<HashSet<_>>>()
        .map_err(|e| e.to_string())?;
    if !camera_ids.is_subset(&reference_cameras) {
        return Err("camera reference-sensor mapping cannot be verified for Caspar".into());
    }
    let mut frames = db.prepare("SELECT images.name,images.camera_id,frame_data.sensor_id,rigs.ref_sensor_id,rigs.ref_sensor_type FROM images LEFT JOIN frame_data ON frame_data.data_id=images.image_id AND frame_data.sensor_type=0 LEFT JOIN frames USING(frame_id) LEFT JOIN rigs USING(rig_id)")
        .map_err(|e| e.to_string())?;
    let mut verified = HashSet::new();
    for row in frames
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, u32>(1)?,
                r.get::<_, Option<u32>>(2)?,
                r.get::<_, Option<u32>>(3)?,
                r.get::<_, Option<u32>>(4)?,
            ))
        })
        .map_err(|e| e.to_string())?
    {
        let (name, camera, sensor, reference, kind) = row.map_err(|e| e.to_string())?;
        if !names.contains(&name) {
            continue;
        }
        if sensor != Some(camera)
            || reference != Some(camera)
            || kind != Some(0)
            || !verified.insert(name)
        {
            return Err("image reference-frame mapping cannot be verified for Caspar".into());
        }
    }
    if verified != names {
        return Err("incomplete image reference-frame mapping for Caspar".into());
    }
    Ok(())
}

fn check_model_rigs(model: &Path) -> Result<(), String> {
    use std::io::Read;
    let binary = model.join("rigs.bin");
    if binary.is_file() {
        let mut file = std::fs::File::open(binary).map_err(|e| e.to_string())?;
        let mut count = [0; 8];
        file.read_exact(&mut count).map_err(|e| e.to_string())?;
        let count = u64::from_le_bytes(count);
        if count > 100_000 {
            return Err("invalid input-model rig count".into());
        }
        for _ in 0..count {
            let mut record = [0; 16];
            file.read_exact(&mut record).map_err(|e| e.to_string())?;
            let sensors = u32::from_le_bytes(record[4..8].try_into().unwrap());
            let sensor_type = u32::from_le_bytes(record[8..12].try_into().unwrap());
            if sensors != 1 || sensor_type != 0 {
                return Err(
                    "input-model non-reference rig sensors are not verified for Caspar".into(),
                );
            }
        }
        if file.read(&mut [0; 1]).map_err(|e| e.to_string())? != 0 {
            return Err("invalid input-model rig data".into());
        }
    } else if model.join("rigs.txt").is_file() {
        let text = std::fs::read_to_string(model.join("rigs.txt")).map_err(|e| e.to_string())?;
        for row in text
            .lines()
            .filter(|row| !row.trim().is_empty() && !row.trim_start().starts_with('#'))
        {
            let fields = row.split_whitespace().collect::<Vec<_>>();
            if fields.len() != 4 || fields[1] != "1" || fields[2] != "CAMERA" {
                return Err("input-model rig capability is not verified for Caspar".into());
            }
        }
    } else if model.join("frames.bin").is_file() || model.join("frames.txt").is_file() {
        return Err("input-model frames have no verifiable rig definition".into());
    }
    Ok(())
}

#[cfg(test)]
pub(super) async fn cache_success_for_test(executable: &Path, gpu: u32) {
    let cell = Arc::new(OnceCell::new());
    cell.set(BaSelection {
        gpu_index: Some(gpu),
        detail: "unit-test cached capability".into(),
    })
    .unwrap();
    CACHE
        .get_or_init(Default::default)
        .lock()
        .await
        .insert(cache_key(executable, gpu), cell);
}

#[cfg(test)]
pub(super) fn compatible_database_for_test(root: &Path) -> PathBuf {
    let database = root.join("database.db");
    let db = Connection::open(&database).unwrap();
    db.execute_batch("CREATE TABLE cameras(camera_id INTEGER,model INTEGER);
        CREATE TABLE images(image_id INTEGER,name TEXT,camera_id INTEGER);
        CREATE TABLE keypoints(rows INTEGER); CREATE TABLE two_view_geometries(rows INTEGER);
        CREATE TABLE rigs(rig_id INTEGER,ref_sensor_id INTEGER,ref_sensor_type INTEGER);
        CREATE TABLE rig_sensors(rig_id INTEGER,sensor_id INTEGER,sensor_type INTEGER);
        CREATE TABLE frames(frame_id INTEGER,rig_id INTEGER);
        CREATE TABLE frame_data(frame_id INTEGER,data_id INTEGER,sensor_id INTEGER,sensor_type INTEGER);
        INSERT INTO cameras VALUES(1,2),(2,1); INSERT INTO images VALUES(1,'a.png',1),(2,'b.png',2);
        INSERT INTO keypoints VALUES(1000000),(1000000); INSERT INTO two_view_geometries VALUES(2000000);
        INSERT INTO rigs VALUES(1,1,0),(2,2,0); INSERT INTO frames VALUES(1,1),(2,2);
        INSERT INTO frame_data VALUES(1,1,1,0),(2,2,2,0);").unwrap();
    database
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

fn validate_mapper_help(text: &str) -> Result<(), String> {
    // Camera, rig and fixed-parameter compatibility is verified only for this
    // bundled build. Matching CLI names alone cannot prove another runtime
    // preserves the same observations or calibration constraints.
    if !super::colmap::is_locked_build(text) {
        return Err("Caspar mapper constraint support is unverified for this COLMAP build".into());
    }
    if ![
        "--Mapper.ba_local_backend",
        "--Mapper.ba_global_backend",
        "--Mapper.ba_gpu_index",
    ]
    .iter()
    .all(|option| text.contains(option))
    {
        return Err("COLMAP mapper is missing Caspar backend options".into());
    }
    Ok(())
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
        if !mapper_help.success {
            return Err("COLMAP mapper capability query failed".into());
        }
        validate_mapper_help(&mapper_text)?;
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
    #[tokio::test]
    #[ignore = "Native Caspar mixed-camera/fixed-pose probe; writes a new isolated .cache fixture only"]
    async fn native_mixed_camera_fixed_constraints_probe() {
        use sha2::{Digest, Sha256};
        let executable = crate::pipeline::runner::default_engine_paths(None).colmap;
        assert!(executable.is_file(), "bundled COLMAP is required");
        let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join(".cache/ba-policy-native")
            .join(uuid::Uuid::new_v4().to_string());
        let input = directory.join("input");
        std::fs::create_dir_all(&input).unwrap();
        let mut files = fixture();
        files[0].1 =
            "1 SIMPLE_RADIAL 640 480 500 320 240 0\n2 PINHOLE 640 480 500 500 320 240\n".into();
        files[1].1 = files[1]
            .1
            .lines()
            .enumerate()
            .map(|(index, line)| {
                if index == 2 {
                    let mut fields = line
                        .split_whitespace()
                        .map(String::from)
                        .collect::<Vec<_>>();
                    fields[8] = "2".into();
                    fields.join(" ")
                } else {
                    line.into()
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        for (name, text) in &files {
            std::fs::write(input.join(name), text).unwrap();
        }
        let before = files
            .iter()
            .map(|(name, _)| {
                format!(
                    "{:x}",
                    Sha256::digest(std::fs::read(input.join(name)).unwrap())
                )
            })
            .collect::<Vec<_>>();
        let original = crate::reconstruction::direct_input::read_model(&input).unwrap();
        for fixed in [false, true] {
            let output = directory.join(if fixed { "fixed" } else { "variable" });
            std::fs::create_dir(&output).unwrap();
            let mut args: Vec<OsString> = vec![
                "bundle_adjuster".into(),
                "--input_path".into(),
                input.clone().into_os_string(),
                "--output_path".into(),
                output.clone().into_os_string(),
                "--BundleAdjustment.backend".into(),
                "CASPAR".into(),
                "--BundleAdjustmentCaspar.gpu_index".into(),
                "0".into(),
                "--BundleAdjustment.refine_principal_point".into(),
                "0".into(),
                "--log_level".into(),
                "2".into(),
            ];
            if fixed {
                args.extend([
                    "--BundleAdjustment.refine_rig_from_world".into(),
                    "0".into(),
                    "--BundleAdjustment.refine_focal_length".into(),
                    "0".into(),
                    "--BundleAdjustment.refine_extra_params".into(),
                    "0".into(),
                ]);
            }
            let process = ProcessManager::new()
                .run_with_environment(
                    ProcessSpec {
                        executable: executable.clone(),
                        args,
                        working_directory: Some(directory.clone()),
                        log_path: Some(output.join("native.log")),
                        observer: None,
                    },
                    &[
                        ("GLOG_v".into(), "2".into()),
                        ("GLOG_logtostderr".into(), "1".into()),
                    ],
                )
                .await
                .unwrap();
            assert!(process.success, "{}", process.failure_detail());
            let text = format!("{}{}", process.stdout, process.stderr);
            assert!(text.contains("Creating Caspar bundle adjuster"));
            assert!(
                !text.contains("Skipping image"),
                "no mixed-camera observations may be skipped"
            );
            let result = crate::reconstruction::direct_input::read_model(&output).unwrap();
            assert_eq!(result.images.len(), original.images.len());
            assert_eq!(result.points.len(), original.points.len());
            for image in &original.images {
                let actual = result
                    .images
                    .iter()
                    .find(|i| i.pose.name == image.pose.name)
                    .unwrap();
                assert_eq!(actual.observations.len(), image.observations.len());
                if fixed {
                    for (a, b) in actual
                        .pose
                        .qvec
                        .iter()
                        .chain(actual.pose.tvec.iter())
                        .zip(image.pose.qvec.iter().chain(image.pose.tvec.iter()))
                    {
                        assert!((a - b).abs() < 1e-10);
                    }
                }
            }
            assert_eq!(
                result
                    .cameras
                    .iter()
                    .map(|c| c.model_id)
                    .collect::<HashSet<_>>(),
                [1, 2].into_iter().collect()
            );
            if fixed {
                for camera in &original.cameras {
                    let actual = result.cameras.iter().find(|c| c.id == camera.id).unwrap();
                    assert_eq!(actual.params, camera.params);
                }
            }
        }
        let after = files
            .iter()
            .map(|(name, _)| {
                format!(
                    "{:x}",
                    Sha256::digest(std::fs::read(input.join(name)).unwrap())
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(before, after);
        eprintln!(
            "Native mixed PINHOLE/SIMPLE_RADIAL evidence: {}",
            directory.display()
        );
    }
    #[test]
    fn mapper_capability_checks_real_candidates_and_seed_models() {
        let root = tempfile::tempdir().unwrap();
        let db = compatible_database_for_test(root.path());
        let mut args = vec!["mapper".into()];
        mapper_compatibility(&db, &args, root.path()).unwrap();
        let connection = Connection::open(&db).unwrap();
        connection
            .execute_batch(
                "INSERT INTO cameras VALUES(3,17); INSERT INTO images VALUES(3,'erp.png',3);",
            )
            .unwrap();
        assert!(mapper_compatibility(&db, &args, root.path())
            .unwrap_err()
            .contains("does not support camera"));
        std::fs::write(root.path().join("selected.txt"), "a.png\nb.png\n").unwrap();
        args.extend(["--Mapper.image_list_path".into(), "selected.txt".into()]);
        mapper_compatibility(&db, &args, root.path()).unwrap();
        let model = root.path().join("base");
        std::fs::create_dir(&model).unwrap();
        std::fs::write(
            model.join("cameras.txt"),
            "1 EQUIRECTANGULAR 200 100 200 100\n",
        )
        .unwrap();
        args.extend(["--input_path".into(), "base".into()]);
        assert!(mapper_compatibility(&db, &args, root.path())
            .unwrap_err()
            .contains("input-model camera"));
    }

    #[test]
    fn mapper_capability_preserves_fixed_calibration_and_rejects_unknown_rigs() {
        let root = tempfile::tempdir().unwrap();
        let db = compatible_database_for_test(root.path());
        let mut args: Vec<OsString> = vec![
            "mapper".into(),
            "--Mapper.ba_refine_focal_length".into(),
            "0".into(),
        ];
        assert!(mapper_compatibility(&db, &args, root.path())
            .unwrap_err()
            .contains("refined together"));
        std::fs::write(root.path().join("fixed.txt"), "1\n2\n").unwrap();
        args.extend([
            "--Mapper.constant_camera_list_path".into(),
            "fixed.txt".into(),
        ]);
        mapper_compatibility(&db, &args, root.path()).unwrap();
        Connection::open(&db)
            .unwrap()
            .execute_batch("INSERT INTO rig_sensors VALUES(99,2,0);")
            .unwrap();
        assert!(mapper_compatibility(&db, &args, root.path())
            .unwrap_err()
            .contains("non-reference"));
        Connection::open(&db)
            .unwrap()
            .execute_batch("DELETE FROM rig_sensors; DELETE FROM frame_data WHERE data_id=1;")
            .unwrap();
        assert!(mapper_compatibility(&db, &args, root.path())
            .unwrap_err()
            .contains("reference-frame"));
    }

    #[test]
    fn input_model_rigs_are_checked_without_assuming_probe_coverage() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("rigs.txt"), "1 1 CAMERA 1\n").unwrap();
        check_model_rigs(root.path()).unwrap();
        std::fs::write(
            root.path().join("rigs.txt"),
            "1 2 CAMERA 1 CAMERA 2 1 1 0 0 0 0 0 0\n",
        )
        .unwrap();
        assert!(check_model_rigs(root.path()).is_err());
        std::fs::write(
            root.path().join("rigs.bin"),
            [
                &1u64.to_le_bytes()[..],
                &1u32.to_le_bytes(),
                &1u32.to_le_bytes(),
                &0u32.to_le_bytes(),
                &1u32.to_le_bytes(),
            ]
            .concat(),
        )
        .unwrap();
        check_model_rigs(root.path()).unwrap();
    }
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
    fn mapper_constraint_capability_requires_the_verified_runtime_and_options() {
        let options = "--Mapper.ba_local_backend --Mapper.ba_global_backend --Mapper.ba_gpu_index";
        for commit in [super::super::colmap::LOCKED_COMMIT, "bd1fcf6"] {
            let help = format!("COLMAP 4.2.1 (Commit {commit})\n{options}");
            validate_mapper_help(&help).unwrap();
        }
        assert!(
            validate_mapper_help(&format!("COLMAP 4.2.1 (Commit 1234567)\n{options}"))
                .unwrap_err()
                .contains("unverified")
        );
        assert!(validate_mapper_help(&format!(
            "COLMAP 4.3.0 (Commit {})\n{options}",
            super::super::colmap::LOCKED_COMMIT
        ))
        .is_err());
        assert!(validate_mapper_help(options).is_err());
        assert!(validate_mapper_help(&format!(
            "COLMAP 4.2.1 (Commit {})\n--Mapper.ba_global_backend",
            super::super::colmap::LOCKED_COMMIT
        ))
        .unwrap_err()
        .contains("missing Caspar backend options"));
    }

    #[test]
    fn fixture_has_observations_and_bidirectional_tracks() {
        let files = fixture();
        assert!(files[0].1.contains("SIMPLE_RADIAL"));
        assert_eq!(files[1].1.lines().count(), 6);
        assert_eq!(files[2].1.lines().count(), 8);
    }
}
