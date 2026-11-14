//! Bounded recovery of a cold Mapper initialization with invalid intrinsics.
//!
//! The locked engine's parallel initial-pair search can truncate a better
//! candidate when another worker finishes first. A single supplied first-image
//! ID makes that search one task, without forcing bundle adjustment to run
//! single-threaded. This module never changes geometric acceptance thresholds,
//! camera grouping, pixels, features, matches, existing poses, or prior data.

use std::{
    collections::{HashMap, HashSet},
    ffi::OsString,
    fs::{File, OpenOptions},
    io::Read,
    path::{Path, PathBuf},
};

use rusqlite::{Connection, OpenFlags};
use tokio_util::sync::CancellationToken;

use crate::{
    error::{Result, SplatError},
    reconstruction::{
        colmap_model::{read_cameras, read_registered_images, ColmapCamera},
        direct_input::database_read_error,
        good_registered_ratio,
    },
};

pub(super) const MAX_RECOVERY_SEEDS: usize = 3;
const PAIR_ID_BASE: u64 = 2_147_483_647;
const ATTEMPT_PREFIX: &str = "initialization-attempt-";

#[derive(Debug, Clone)]
pub(super) struct InitializationRequest {
    pub database: PathBuf,
    image_names: Option<HashSet<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ModelAssessment {
    pub model: PathBuf,
    pub registered_images: u64,
    pub points_3d: u64,
    pub valid_intrinsics: bool,
    /// Local bounded facts from this model, not inferred from native bad-pair
    /// messages. No image names, source paths or coordinates are recorded.
    pub invalid_camera_parameters: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SeedSelection {
    pub input_images: u64,
    pub seeds: Vec<u32>,
}

#[derive(Debug, Default)]
struct SeedImage {
    prior: bool,
    correspondences: u64,
    neighbors: Vec<u32>,
}

fn check_cancel(cancel: &CancellationToken) -> Result<()> {
    if cancel.is_cancelled() {
        Err(SplatError::Cancelled)
    } else {
        Ok(())
    }
}

fn option_present(args: &[OsString], name: &str) -> bool {
    args.iter().any(|value| {
        value == name
            || value
                .to_str()
                .is_some_and(|value| value.starts_with(&format!("{name}=")))
    })
}

fn option_explicitly_disabled(args: &[OsString], name: &str) -> bool {
    let values = args
        .iter()
        .enumerate()
        .filter_map(|(index, arg)| {
            if arg == name {
                Some(args.get(index + 1).and_then(|value| value.to_str()))
            } else {
                arg.to_str()
                    .and_then(|value| value.strip_prefix(&format!("{name}=")))
                    .map(Some)
            }
        })
        .collect::<Vec<_>>();
    values.len() == 1 && matches!(values[0], Some("0" | "false"))
}

fn option_path(args: &[OsString], name: &str, working_directory: &Path) -> Result<Option<PathBuf>> {
    let positions = args
        .iter()
        .enumerate()
        .filter(|(_, value)| *value == name)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if positions.len() > 1 {
        return Err(SplatError::Process(format!(
            "Duplicate protected mapper option {name}"
        )));
    }
    let Some(index) = positions.first().copied() else {
        return Ok(None);
    };
    let value = args
        .get(index + 1)
        .ok_or_else(|| SplatError::Process(format!("Mapper option {name} has no value")))?;
    super::resolved_path(&PathBuf::from(value), working_directory).map(Some)
}

fn open_database(path: &Path) -> Result<Connection> {
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(database_read_error)
}

/// Only a new, unconstrained ordinary Mapper may choose a new initialization.
/// Existing-model/Bridge/GPS executions retain their complete original route.
pub(super) fn eligible_request(
    args: &[OsString],
    working_directory: &Path,
) -> Result<Option<InitializationRequest>> {
    if !args.first().is_some_and(|arg| arg == "mapper")
        || [
            "--input_path",
            "--Mapper.init_image_id1",
            "--Mapper.init_image_id2",
            "--Mapper.constant_camera_list_path",
            // This narrow compatibility path mirrors the locked defaults. It
            // must not interpret an explicitly customized safety bound as bad.
            "--Mapper.min_focal_length_ratio",
            "--Mapper.max_focal_length_ratio",
            "--Mapper.max_extra_param",
            "--Mapper.min_num_matches",
            "--Mapper.ignore_watermarks",
        ]
        .iter()
        .any(|key| option_present(args, key))
    {
        return Ok(None);
    }
    if option_present(args, "--Mapper.fix_existing_frames")
        && !option_explicitly_disabled(args, "--Mapper.fix_existing_frames")
    {
        return Ok(None);
    }
    // The public builders use split arguments. Do not reinterpret an unknown
    // CLI representation or accidentally inspect a different database.
    if args.iter().any(|arg| {
        arg.to_str().is_some_and(|arg| {
            arg.starts_with("--database_path=") || arg.starts_with("--Mapper.image_list_path=")
        })
    }) {
        return Ok(None);
    }
    let Some(database) = option_path(args, "--database_path", working_directory)? else {
        return Ok(None);
    };
    let image_names = option_path(args, "--Mapper.image_list_path", working_directory)?
        .map(|path| -> Result<HashSet<String>> {
            let text = std::fs::read_to_string(path)?;
            let mut names = HashSet::new();
            for line in text.lines().filter(|line| !line.is_empty()) {
                if !names.insert(line.to_owned()) {
                    return Err(SplatError::Process(
                        "Mapper image list contains duplicate names".into(),
                    ));
                }
            }
            Ok(names)
        })
        .transpose()?;
    let request = InitializationRequest {
        database,
        image_names,
    };
    let database = open_database(&request.database)?;
    let mut statement = database
        .prepare("SELECT images.name,cameras.model FROM images JOIN cameras USING(camera_id)")
        .map_err(database_read_error)?;
    let mut rows = statement.query([]).map_err(database_read_error)?;
    while let Some(row) = rows.next().map_err(database_read_error)? {
        let name: String = row.get(0).map_err(database_read_error)?;
        if request
            .image_names
            .as_ref()
            .is_some_and(|names| !names.contains(&name))
        {
            continue;
        }
        let model: i32 = row.get(1).map_err(database_read_error)?;
        if !matches!(model, 0 | 1 | 2 | 3 | 4 | 6) {
            return Ok(None);
        }
    }
    Ok(Some(request))
}

/// Rank the same verified graph used by Mapper, deterministically. Reading
/// statements/connection are dropped before another engine can open the DB.
pub(super) fn select_seed_images(
    request: &InitializationRequest,
    cancel: &CancellationToken,
) -> Result<SeedSelection> {
    check_cancel(cancel)?;
    let database = open_database(&request.database)?;
    let mut images = HashMap::<u32, SeedImage>::new();
    let mut statement = database
        .prepare("SELECT images.image_id,images.name,cameras.prior_focal_length FROM images JOIN cameras USING(camera_id) ORDER BY images.image_id")
        .map_err(database_read_error)?;
    let mut rows = statement.query([]).map_err(database_read_error)?;
    while let Some(row) = rows.next().map_err(database_read_error)? {
        check_cancel(cancel)?;
        let id: u32 = row.get(0).map_err(database_read_error)?;
        let name: String = row.get(1).map_err(database_read_error)?;
        if request
            .image_names
            .as_ref()
            .is_some_and(|names| !names.contains(&name))
        {
            continue;
        }
        if id == 0 || u64::from(id) >= PAIR_ID_BASE {
            return Err(SplatError::Process(
                "Invalid COLMAP image identifier".into(),
            ));
        }
        let prior: i64 = row.get(2).map_err(database_read_error)?;
        images.insert(
            id,
            SeedImage {
                prior: prior != 0,
                ..Default::default()
            },
        );
    }
    drop(rows);
    drop(statement);
    if request
        .image_names
        .as_ref()
        .is_some_and(|names| names.len() != images.len())
    {
        return Err(SplatError::Process(
            "Mapper image list does not match the feature database".into(),
        ));
    }
    let input_images = images.len() as u64;
    let mut statement = database
        .prepare("SELECT pair_id,rows FROM two_view_geometries WHERE rows>=15 AND config NOT IN (0,1) ORDER BY pair_id")
        .map_err(database_read_error)?;
    let mut rows = statement.query([]).map_err(database_read_error)?;
    while let Some(row) = rows.next().map_err(database_read_error)? {
        check_cancel(cancel)?;
        let pair: u64 = row.get(0).map_err(database_read_error)?;
        let count: u64 = row.get(1).map_err(database_read_error)?;
        let first = pair / PAIR_ID_BASE;
        let second = pair % PAIR_ID_BASE;
        if first == 0 || first >= second || second >= PAIR_ID_BASE {
            return Err(SplatError::Process(
                "Invalid COLMAP image pair identifier".into(),
            ));
        }
        let first = first as u32;
        let second = second as u32;
        if !images.contains_key(&first) || !images.contains_key(&second) {
            continue;
        }
        for (image, neighbor) in [(first, second), (second, first)] {
            let entry = images
                .get_mut(&image)
                .expect("both image IDs checked above");
            entry.correspondences = entry.correspondences.checked_add(count).ok_or_else(|| {
                SplatError::Process("COLMAP correspondence count overflow".into())
            })?;
            // The database primary key identifies each unordered pair once.
            // Keep one compact adjacency list, not a dense similarity matrix.
            entry.neighbors.push(neighbor);
        }
    }
    let mut ranked = images
        .into_iter()
        .filter(|(_, image)| image.correspondences > 0)
        .collect::<Vec<_>>();
    ranked.sort_by(|(left_id, left), (right_id, right)| {
        right
            .prior
            .cmp(&left.prior)
            .then_with(|| right.correspondences.cmp(&left.correspondences))
            .then_with(|| left_id.cmp(right_id))
    });
    let mut covered = HashSet::new();
    let mut seeds = Vec::with_capacity(MAX_RECOVERY_SEEDS);
    while seeds.len() < MAX_RECOVERY_SEEDS && !ranked.is_empty() {
        check_cancel(cancel)?;
        // The first seed follows native priority. Later seeds maximize new
        // verified-neighborhood coverage, so three near-identical views do
        // not consume all three attempts. Priority order breaks equal gains.
        let mut selected = 0;
        if !seeds.is_empty() {
            let mut best_gain = 0;
            for (index, (id, image)) in ranked.iter().enumerate() {
                check_cancel(cancel)?;
                let gain = usize::from(!covered.contains(id))
                    + image
                        .neighbors
                        .iter()
                        .filter(|id| !covered.contains(*id))
                        .count();
                if gain > best_gain {
                    best_gain = gain;
                    selected = index;
                }
            }
        }
        let (id, image) = ranked.remove(selected);
        covered.insert(id);
        covered.extend(image.neighbors);
        seeds.push(id);
    }
    Ok(SeedSelection {
        input_images,
        seeds,
    })
}

fn valid_camera(camera: &ColmapCamera) -> bool {
    let (focal_indices, principal_indices, extra_start): (&[usize], [usize; 2], usize) =
        match camera.model_id {
            0 => (&[0], [1, 2], 3),
            1 => (&[0, 1], [2, 3], 4),
            2 | 3 => (&[0], [1, 2], 3),
            4 | 6 => (&[0, 1], [2, 3], 4),
            // No new validity policy for spherical/fisheye/unknown models.
            _ => return true,
        };
    if camera.width == 0
        || camera.height == 0
        || camera.params.len() < extra_start
        || camera.params.iter().any(|v| !v.is_finite())
    {
        return false;
    }
    let max_dimension = camera.width.max(camera.height) as f64;
    focal_indices.iter().all(|index| {
        camera.params.get(*index).is_some_and(|focal| {
            let ratio = focal / max_dimension;
            (0.1..=10.).contains(&ratio)
        })
    }) && camera
        .params
        .get(principal_indices[0])
        .is_some_and(|cx| (0. ..=camera.width as f64).contains(cx))
        && camera
            .params
            .get(principal_indices[1])
            .is_some_and(|cy| (0. ..=camera.height as f64).contains(cy))
        && camera.params[extra_start..]
            .iter()
            .all(|extra| extra.abs() <= 1.)
}

fn invalid_camera_parameters(camera: &ColmapCamera) -> Vec<String> {
    if valid_camera(camera) {
        return Vec::new();
    }
    let (focal_indices, principal_indices, extra_start): (&[usize], [usize; 2], usize) =
        match camera.model_id {
            0 => (&[0], [1, 2], 3),
            1 => (&[0, 1], [2, 3], 4),
            2 | 3 => (&[0], [1, 2], 3),
            4 | 6 => (&[0, 1], [2, 3], 4),
            _ => return Vec::new(),
        };
    let prefix = format!("camera_id={} model_id={}", camera.id, camera.model_id);
    let mut details = Vec::new();
    if camera.width == 0 || camera.height == 0 {
        details.push(format!(
            "{prefix} field=dimensions value={}x{} required=positive",
            camera.width, camera.height
        ));
    }
    if camera.params.len() < extra_start {
        details.push(format!(
            "{prefix} field=parameter_count value={} required_at_least={extra_start}",
            camera.params.len()
        ));
    }
    let max_dimension = camera.width.max(camera.height) as f64;
    for (index, value) in camera.params.iter().enumerate() {
        let constraint = if !value.is_finite() {
            Some("required=finite".to_owned())
        } else if focal_indices.contains(&index) && !(0.1..=10.).contains(&(value / max_dimension))
        {
            Some(format!(
                "normalized_value={} allowed_normalized_range=[0.1,10]",
                value / max_dimension
            ))
        } else if index == principal_indices[0] && !(0. ..=camera.width as f64).contains(value) {
            Some(format!("allowed_range=[0,{}]", camera.width))
        } else if index == principal_indices[1] && !(0. ..=camera.height as f64).contains(value) {
            Some(format!("allowed_range=[0,{}]", camera.height))
        } else if index >= extra_start && value.abs() > 1. {
            Some("allowed_range=[-1,1]".to_owned())
        } else {
            None
        };
        if let Some(constraint) = constraint {
            details.push(format!(
                "{prefix} parameter_index={index} value={value} {constraint}"
            ));
        }
        if details.len() >= 8 {
            break;
        }
    }
    details
}

fn read_count(path: &Path) -> Result<u64> {
    let mut value = [0; 8];
    File::open(path)?.read_exact(&mut value)?;
    Ok(u64::from_le_bytes(value))
}

fn assess_model(model: &Path, cancel: &CancellationToken) -> Result<Option<ModelAssessment>> {
    check_cancel(cancel)?;
    let camera_path = model.join("cameras.bin");
    match std::fs::symlink_metadata(&camera_path) {
        Ok(metadata) if !metadata.file_type().is_file() => {
            return Err(SplatError::InvalidPath(camera_path));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    }
    let cameras = read_cameras(model)?;
    check_cancel(cancel)?;
    let image_path = model.join("images.bin");
    let image_count = read_count(&image_path)?;
    // The existing image reader reserves from the header. Bound that reserve
    // by the file's minimal per-image record before calling it.
    if image_count > image_path.metadata()?.len().saturating_sub(8) / 73 {
        return Err(SplatError::Process(
            "Invalid COLMAP registered-image count".into(),
        ));
    }
    let points_3d = read_count(&model.join("points3D.bin"))?;
    if image_count == 0 || points_3d == 0 {
        return Ok(None);
    }
    // Normal successful models need only camera parameters and count headers.
    // Do not add another scan of every 2D observation before the shared full
    // model/cache validation. Only an invalid camera requires checking whether
    // registered images actually use it.
    if cameras.iter().all(valid_camera) {
        return Ok(Some(ModelAssessment {
            model: model.to_path_buf(),
            registered_images: image_count,
            points_3d,
            valid_intrinsics: true,
            invalid_camera_parameters: Vec::new(),
        }));
    }
    let registered = read_registered_images(model)?;
    let used_cameras = registered
        .iter()
        .map(|image| image.camera_id)
        .collect::<HashSet<_>>();
    let camera_ids = cameras
        .iter()
        .map(|camera| camera.id)
        .collect::<HashSet<_>>();
    if !used_cameras.is_subset(&camera_ids) {
        return Err(SplatError::Process(
            "COLMAP image references a missing camera".into(),
        ));
    }
    Ok(Some(ModelAssessment {
        model: model.to_path_buf(),
        registered_images: registered.len() as u64,
        points_3d,
        valid_intrinsics: cameras
            .iter()
            .filter(|camera| used_cameras.contains(&camera.id))
            .all(valid_camera),
        invalid_camera_parameters: cameras
            .iter()
            .filter(|camera| used_cameras.contains(&camera.id))
            .flat_map(invalid_camera_parameters)
            .take(8)
            .collect(),
    }))
}

/// Mirror the public model selector's stable maximum-registration choice.
pub(super) fn assess_output(
    output: &Path,
    cancel: &CancellationToken,
) -> Result<Option<ModelAssessment>> {
    assess_output_inner(output, cancel, false)
}

/// An alternative attempt may contain a larger invalid component and a
/// smaller healthy one. Only healthy models are candidates for admission.
pub(super) fn assess_recovery_output(
    output: &Path,
    cancel: &CancellationToken,
) -> Result<Option<ModelAssessment>> {
    assess_output_inner(output, cancel, true)
}

fn assess_output_inner(
    output: &Path,
    cancel: &CancellationToken,
    prefer_valid: bool,
) -> Result<Option<ModelAssessment>> {
    check_cancel(cancel)?;
    let mut candidates = vec![output.to_path_buf()];
    let mut children = Vec::new();
    for entry in std::fs::read_dir(output)? {
        check_cancel(cancel)?;
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            return Err(SplatError::InvalidPath(entry.path()));
        }
        if kind.is_dir() {
            children.push(entry.path());
        }
    }
    children.sort();
    candidates.extend(children);
    let mut best: Option<ModelAssessment> = None;
    for model in candidates {
        if let Some(candidate) = assess_model(&model, cancel)? {
            if best.as_ref().is_none_or(|best| {
                if prefer_valid && candidate.valid_intrinsics != best.valid_intrinsics {
                    candidate.valid_intrinsics
                } else {
                    candidate.registered_images > best.registered_images
                }
            }) {
                best = Some(candidate);
            }
        }
    }
    Ok(best)
}

pub(super) fn should_recover(best: &ModelAssessment, input_images: u64) -> bool {
    input_images > 0
        && best.registered_images as f64 / (input_images as f64) < good_registered_ratio()
        && !best.valid_intrinsics
}

/// Supply one first image, not a fixed pair or a globally single-threaded BA.
pub(super) fn recovery_args(
    original: &[OsString],
    attempt_output: &Path,
    seed: u32,
) -> Result<Vec<OsString>> {
    if !original.first().is_some_and(|arg| arg == "mapper")
        || [
            "--input_path",
            "--Mapper.init_image_id1",
            "--Mapper.init_image_id2",
            "--Mapper.constant_camera_list_path",
        ]
        .iter()
        .any(|key| option_present(original, key))
    {
        return Err(SplatError::Process(
            "Constrained Mapper initialization cannot be replaced".into(),
        ));
    }
    if seed == 0 || u64::from(seed) >= PAIR_ID_BASE {
        return Err(SplatError::Process(
            "Invalid COLMAP initialization image".into(),
        ));
    }
    let mut args = Vec::with_capacity(original.len() + 4);
    let mut source = original.iter();
    let mut output_seen = false;
    while let Some(arg) = source.next() {
        if arg == "--output_path" {
            if output_seen || source.next().is_none() {
                return Err(SplatError::Process("Invalid mapper output option".into()));
            }
            output_seen = true;
            args.extend([arg.clone(), attempt_output.as_os_str().to_owned()]);
        } else if arg == "--Mapper.random_seed" {
            if source.next().is_none() {
                return Err(SplatError::Process("Mapper random seed is missing".into()));
            }
        } else {
            args.push(arg.clone());
        }
    }
    if !output_seen {
        return Err(SplatError::Process("Mapper output path is missing".into()));
    }
    args.extend([
        "--Mapper.init_image_id1".into(),
        seed.to_string().into(),
        "--Mapper.random_seed".into(),
        "0".into(),
    ]);
    Ok(args)
}

pub(super) fn create_attempt_output(output: &Path) -> Result<PathBuf> {
    let parent = output
        .parent()
        .ok_or_else(|| SplatError::InvalidPath(output.to_path_buf()))?;
    let path = parent.join(format!("{ATTEMPT_PREFIX}{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&path)?;
    Ok(path)
}

/// Admit an independently validated better model under a fresh model name.
/// Native/current models are never reset, overwritten, or deleted here.
pub(super) fn promote_if_better(
    output: &Path,
    previous: &ModelAssessment,
    candidate: &ModelAssessment,
    cancel: &CancellationToken,
) -> Result<Option<ModelAssessment>> {
    check_cancel(cancel)?;
    if !candidate.valid_intrinsics || candidate.registered_images <= previous.registered_images {
        return Ok(None);
    }
    let parent = output
        .parent()
        .ok_or_else(|| SplatError::InvalidPath(output.to_path_buf()))?;
    let attempt = if candidate.model.file_name().is_some_and(|name| {
        name.to_str()
            .is_some_and(|name| name.starts_with(ATTEMPT_PREFIX))
    }) {
        candidate.model.as_path()
    } else {
        candidate
            .model
            .parent()
            .ok_or_else(|| SplatError::InvalidPath(candidate.model.clone()))?
    };
    if attempt.parent() != Some(parent)
        || !attempt.file_name().is_some_and(|name| {
            name.to_str()
                .is_some_and(|name| name.starts_with(ATTEMPT_PREFIX))
        })
        || std::fs::symlink_metadata(attempt)?.file_type().is_symlink()
        || std::fs::symlink_metadata(&candidate.model)?
            .file_type()
            .is_symlink()
    {
        return Err(SplatError::InvalidPath(candidate.model.clone()));
    }
    let mut numeric = 0_u64;
    let destination = loop {
        let destination = output.join(numeric.to_string());
        match std::fs::create_dir(&destination) {
            Ok(()) => break destination,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        numeric = numeric
            .checked_add(1)
            .ok_or_else(|| SplatError::Process("Too many Mapper output models".into()))?;
    };
    // Reserve a fresh directory atomically. Install cameras.bin last, so all
    // existing selectors ignore a promotion interrupted before it is complete.
    // Links/copies never replace a file, and the candidate remains immutable.
    let mut files = std::fs::read_dir(&candidate.model)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    files.sort_by_key(|path| {
        (
            path.file_name().is_some_and(|name| name == "cameras.bin"),
            path.clone(),
        )
    });
    for source in files {
        check_cancel(cancel)?;
        if !std::fs::symlink_metadata(&source)?.file_type().is_file() {
            return Err(SplatError::InvalidPath(source));
        }
        let target = destination.join(
            source
                .file_name()
                .ok_or_else(|| SplatError::InvalidPath(source.clone()))?,
        );
        match std::fs::hard_link(&source, &target) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(error.into())
            }
            Err(_) => {
                let mut reader = File::open(&source)?;
                let mut writer = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&target)?;
                let mut buffer = vec![0; 128 * 1024];
                loop {
                    check_cancel(cancel)?;
                    let length = reader.read(&mut buffer)?;
                    if length == 0 {
                        break;
                    }
                    std::io::Write::write_all(&mut writer, &buffer[..length])?;
                }
                writer.sync_all()?;
            }
        }
    }
    Ok(Some(ModelAssessment {
        model: destination,
        ..candidate.clone()
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(root: &Path) -> Vec<OsString> {
        vec![
            "mapper".into(),
            "--database_path".into(),
            root.join("database.db").into(),
            "--image_path".into(),
            root.join("images").into(),
            "--output_path".into(),
            root.join("sparse").into(),
        ]
    }

    fn database(root: &Path) {
        let db = Connection::open(root.join("database.db")).unwrap();
        db.execute_batch("CREATE TABLE cameras(camera_id INTEGER PRIMARY KEY,model INTEGER,prior_focal_length INTEGER);
            CREATE TABLE images(image_id INTEGER PRIMARY KEY,name TEXT,camera_id INTEGER);
            CREATE TABLE two_view_geometries(pair_id INTEGER PRIMARY KEY,rows INTEGER,config INTEGER);
            INSERT INTO cameras VALUES(1,2,0),(2,1,1);
            INSERT INTO images VALUES(1,'a.png',1),(2,'b.png',1),(3,'c.png',2),(4,'d.png',1);").unwrap();
        for (a, b, count, config) in [
            (1, 2, 100, 3),
            (1, 3, 20, 3),
            (2, 3, 20, 3),
            (3, 4, 20, 3),
            (1, 4, 9999, 0),
            (2, 4, 9999, 1),
            (1, 4, 14, 3),
        ] {
            db.execute("INSERT INTO two_view_geometries VALUES(?1,?2,?3) ON CONFLICT(pair_id) DO UPDATE SET rows=excluded.rows,config=excluded.config",
                rusqlite::params![a as u64 * PAIR_ID_BASE + b as u64,count,config]).unwrap();
        }
    }

    fn model(path: &Path, image_count: u32, k: f64) {
        use std::io::Write;
        std::fs::create_dir_all(path).unwrap();
        let mut cameras = File::create(path.join("cameras.bin")).unwrap();
        cameras.write_all(&1_u64.to_le_bytes()).unwrap();
        cameras.write_all(&1_u32.to_le_bytes()).unwrap();
        cameras.write_all(&2_i32.to_le_bytes()).unwrap();
        for size in [2560_u64, 3840] {
            cameras.write_all(&size.to_le_bytes()).unwrap();
        }
        for value in [27380_f64, 1280., 1920., k] {
            cameras.write_all(&value.to_le_bytes()).unwrap();
        }
        let mut images = File::create(path.join("images.bin")).unwrap();
        images
            .write_all(&u64::from(image_count).to_le_bytes())
            .unwrap();
        for image in 1..=image_count {
            images.write_all(&image.to_le_bytes()).unwrap();
            images.write_all(&[0_u8; 56]).unwrap();
            images.write_all(&1_u32.to_le_bytes()).unwrap();
            images
                .write_all(format!("{image}.png\0").as_bytes())
                .unwrap();
            images.write_all(&0_u64.to_le_bytes()).unwrap();
        }
        std::fs::write(path.join("points3D.bin"), 100_u64.to_le_bytes()).unwrap();
    }

    #[test]
    fn candidate_order_prefers_prior_then_graph_weight_then_stable_id() {
        let root = tempfile::tempdir().unwrap();
        database(root.path());
        let before = std::fs::read(root.path().join("database.db")).unwrap();
        let request = eligible_request(&args(root.path()), root.path())
            .unwrap()
            .unwrap();
        let selected = select_seed_images(&request, &CancellationToken::new()).unwrap();
        assert_eq!(selected.input_images, 4);
        assert_eq!(selected.seeds, [3, 1, 2]);
        assert_eq!(
            std::fs::read(root.path().join("database.db")).unwrap(),
            before
        );
    }

    #[test]
    fn candidate_graph_obeys_exact_image_list_and_ignores_other_images() {
        let root = tempfile::tempdir().unwrap();
        database(root.path());
        std::fs::write(root.path().join("images.txt"), "a.png\nb.png\nd.png\n").unwrap();
        let mut command = args(root.path());
        command.extend(["--Mapper.image_list_path".into(), "images.txt".into()]);
        let request = eligible_request(&command, root.path()).unwrap().unwrap();
        let selected = select_seed_images(&request, &CancellationToken::new()).unwrap();
        assert_eq!(selected.input_images, 3);
        assert_eq!(selected.seeds, [1, 2]);
        std::fs::write(root.path().join("images.txt"), "missing.png\n").unwrap();
        let request = eligible_request(&command, root.path()).unwrap().unwrap();
        assert!(select_seed_images(&request, &CancellationToken::new()).is_err());
    }

    #[test]
    fn later_seeds_cover_distinct_graph_neighborhoods_instead_of_repeating_top_cluster() {
        let root = tempfile::tempdir().unwrap();
        database(root.path());
        let db = Connection::open(root.path().join("database.db")).unwrap();
        db.execute_batch("DELETE FROM two_view_geometries; UPDATE cameras SET prior_focal_length=0;
            INSERT INTO images VALUES(5,'e.png',1),(6,'f.png',1),(7,'g.png',1),(8,'h.png',1),(9,'i.png',1);").unwrap();
        for (first, second, count) in [
            (1, 2, 1000),
            (1, 3, 1000),
            (2, 3, 1000),
            (4, 5, 20),
            (4, 6, 20),
            (4, 7, 20),
            (4, 8, 20),
            (5, 9, 15),
            (6, 9, 15),
            (7, 9, 15),
            (8, 9, 15),
        ] {
            db.execute(
                "INSERT INTO two_view_geometries VALUES(?1,?2,3)",
                rusqlite::params![first as u64 * PAIR_ID_BASE + second as u64, count],
            )
            .unwrap();
        }
        drop(db);
        let request = eligible_request(&args(root.path()), root.path())
            .unwrap()
            .unwrap();
        let selected = select_seed_images(&request, &CancellationToken::new()).unwrap();
        assert_eq!(selected.input_images, 9);
        assert_eq!(selected.seeds, [1, 4, 9]);
        assert_eq!(
            selected,
            select_seed_images(&request, &CancellationToken::new()).unwrap()
        );
    }

    #[test]
    fn constrained_initialization_and_prior_mapper_are_never_reinterpreted() {
        let root = tempfile::tempdir().unwrap();
        for (option, value) in [
            ("--input_path", "old"),
            ("--Mapper.init_image_id1", "1"),
            ("--Mapper.init_image_id2", "2"),
            ("--Mapper.constant_camera_list_path", "fixed"),
            ("--Mapper.max_extra_param", "2"),
        ] {
            let mut command = args(root.path());
            command.extend([option.into(), value.into()]);
            assert!(eligible_request(&command, root.path()).unwrap().is_none());
        }
        let mut command = args(root.path());
        command[0] = "pose_prior_mapper".into();
        assert!(eligible_request(&command, root.path()).unwrap().is_none());
        let mut command = args(root.path());
        command.push("--input_path=old".into());
        assert!(eligible_request(&command, root.path()).unwrap().is_none());
    }

    #[test]
    fn panorama_and_unverified_camera_models_do_not_enter_recovery() {
        let root = tempfile::tempdir().unwrap();
        database(root.path());
        let db = Connection::open(root.path().join("database.db")).unwrap();
        for model in [17, 5, 999] {
            db.execute("UPDATE cameras SET model=?1 WHERE camera_id=1", [model])
                .unwrap();
            assert!(eligible_request(&args(root.path()), root.path())
                .unwrap()
                .is_none());
        }
    }

    #[test]
    fn full_reshoot_fix_existing_frames_zero_is_not_an_incremental_constraint() {
        let root = tempfile::tempdir().unwrap();
        database(root.path());
        for value in ["0", "false"] {
            let mut command = args(root.path());
            command.extend(["--Mapper.fix_existing_frames".into(), value.into()]);
            assert!(eligible_request(&command, root.path()).unwrap().is_some());
        }
        for value in ["1", "true"] {
            let mut command = args(root.path());
            command.extend(["--Mapper.fix_existing_frames".into(), value.into()]);
            assert!(eligible_request(&command, root.path()).unwrap().is_none());
        }
        let mut command = args(root.path());
        command.push("--Mapper.fix_existing_frames=0".into());
        assert!(eligible_request(&command, root.path()).unwrap().is_some());
    }

    #[test]
    fn recovery_changes_only_output_first_seed_and_random_sampling() {
        let root = tempfile::tempdir().unwrap();
        let mut command = args(root.path());
        command.extend([
            "--Mapper.ba_global_backend".into(),
            "CASPAR".into(),
            "--Mapper.tri_ignore_two_view_tracks".into(),
            "0".into(),
            "--Mapper.num_threads".into(),
            "8".into(),
            "--Mapper.random_seed".into(),
            "-1".into(),
        ]);
        let recovered = recovery_args(&command, Path::new("isolated"), 9).unwrap();
        assert_eq!(
            recovered
                .iter()
                .filter(|arg| *arg == "--Mapper.random_seed")
                .count(),
            1
        );
        for (key, value) in [
            ("--Mapper.ba_global_backend", "CASPAR"),
            ("--Mapper.tri_ignore_two_view_tracks", "0"),
            ("--Mapper.num_threads", "8"),
            ("--Mapper.init_image_id1", "9"),
            ("--Mapper.random_seed", "0"),
            ("--output_path", "isolated"),
        ] {
            assert!(recovered
                .windows(2)
                .any(|pair| pair[0] == key && pair[1] == value));
        }
        assert!(!recovered.iter().any(|arg| arg == "--Mapper.init_image_id2"
            || arg == "--Mapper.init_min_tri_angle"
            || arg == "--Mapper.multiple_models"));
    }

    #[test]
    fn good_or_unconnected_valid_models_do_not_trigger_additional_mapping() {
        let root = tempfile::tempdir().unwrap();
        model(&root.path().join("0"), 3, 1.93);
        let cancel = CancellationToken::new();
        let mut best = assess_output(root.path(), &cancel).unwrap().unwrap();
        assert!(should_recover(&best, 90));
        best.valid_intrinsics = true;
        assert!(!should_recover(&best, 90));
        best.valid_intrinsics = false;
        best.registered_images = 72;
        assert!(!should_recover(&best, 90));
        assert!(!should_recover(&best, 0));
    }

    #[test]
    fn only_used_camera_validity_matters_and_largest_native_model_is_assessed() {
        let root = tempfile::tempdir().unwrap();
        model(&root.path().join("0"), 3, 1.93);
        model(&root.path().join("1"), 90, 0.55);
        let best = assess_output(root.path(), &CancellationToken::new())
            .unwrap()
            .unwrap();
        assert_eq!(best.registered_images, 90);
        assert!(best.valid_intrinsics);
    }

    #[test]
    fn recovery_assessment_does_not_hide_a_healthy_model_behind_a_larger_invalid_one() {
        let root = tempfile::tempdir().unwrap();
        model(&root.path().join("0"), 90, 1.93);
        model(&root.path().join("1"), 60, 0.55);
        model(&root.path().join("2"), 61, 0.5);
        let cancel = CancellationToken::new();
        let original = assess_output(root.path(), &cancel).unwrap().unwrap();
        assert_eq!(original.registered_images, 90);
        assert!(!original.valid_intrinsics);
        let candidate = assess_recovery_output(root.path(), &cancel)
            .unwrap()
            .unwrap();
        assert_eq!(candidate.registered_images, 61);
        assert!(candidate.valid_intrinsics);
    }

    #[test]
    fn recovered_model_is_added_without_overwriting_existing_models() {
        let root = tempfile::tempdir().unwrap();
        let output = root.path().join("sparse");
        model(&output.join("0"), 3, 1.93);
        let original = std::fs::read(output.join("0/cameras.bin")).unwrap();
        let cancel = CancellationToken::new();
        let before = assess_output(&output, &cancel).unwrap().unwrap();
        let attempt = create_attempt_output(&output).unwrap();
        model(&attempt.join("0"), 90, 0.55);
        let candidate = assess_output(&attempt, &cancel).unwrap().unwrap();
        let adopted = promote_if_better(&output, &before, &candidate, &cancel)
            .unwrap()
            .unwrap();
        assert_eq!(adopted.model, output.join("1"));
        assert_eq!(
            std::fs::read(output.join("0/cameras.bin")).unwrap(),
            original
        );
        assert_eq!(
            assess_output(&output, &cancel)
                .unwrap()
                .unwrap()
                .registered_images,
            90
        );
    }

    #[test]
    fn invalid_or_smaller_candidates_never_replace_the_original() {
        let root = tempfile::tempdir().unwrap();
        let output = root.path().join("sparse");
        model(&output.join("0"), 3, 1.93);
        let cancel = CancellationToken::new();
        let before = assess_output(&output, &cancel).unwrap().unwrap();
        let attempt = create_attempt_output(&output).unwrap();
        model(&attempt.join("0"), 2, 0.55);
        let candidate = assess_output(&attempt, &cancel).unwrap().unwrap();
        assert!(promote_if_better(&output, &before, &candidate, &cancel)
            .unwrap()
            .is_none());
        model(&attempt.join("1"), 90, 1.93);
        let candidate = assess_output(&attempt, &cancel).unwrap().unwrap();
        assert!(promote_if_better(&output, &before, &candidate, &cancel)
            .unwrap()
            .is_none());
        assert!(output.join("0/cameras.bin").is_file());
    }

    #[test]
    fn cancellation_prevents_database_work_and_model_promotion() {
        let root = tempfile::tempdir().unwrap();
        database(root.path());
        let request = eligible_request(&args(root.path()), root.path())
            .unwrap()
            .unwrap();
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert!(matches!(
            select_seed_images(&request, &cancel),
            Err(SplatError::Cancelled)
        ));
        assert!(matches!(
            assess_output(root.path(), &cancel),
            Err(SplatError::Cancelled)
        ));
    }

    #[test]
    fn camera_validity_matches_native_focal_and_radial_bounds() {
        let mut camera = ColmapCamera {
            id: 1,
            model_id: 2,
            model: "SIMPLE_RADIAL".into(),
            width: 2560,
            height: 3840,
            params: vec![27380., 1280., 1920., 1.],
        };
        assert!(valid_camera(&camera));
        camera.params[3] = 1.000001;
        assert!(!valid_camera(&camera));
        camera.params[3] = -1.;
        assert!(valid_camera(&camera));
        camera.params[3] = 0.;
        camera.params[0] = 384.;
        assert!(valid_camera(&camera));
        camera.params[0] = 383.;
        assert!(!valid_camera(&camera));
        camera.params[0] = 38400.;
        assert!(valid_camera(&camera));
        camera.params[0] = 38401.;
        assert!(!valid_camera(&camera));
        camera.params[0] = f64::NAN;
        assert!(!valid_camera(&camera));
        camera.params.clear();
        assert!(!valid_camera(&camera));
    }

    #[test]
    fn invalid_intrinsics_diagnostic_reports_measured_parameter_and_original_bound() {
        let mut camera = ColmapCamera {
            id: 42,
            model_id: 2,
            model: "SIMPLE_RADIAL".into(),
            width: 2560,
            height: 3840,
            params: vec![27380., 1280., 1920., 1.93],
        };
        let details = invalid_camera_parameters(&camera);
        assert_eq!(
            details,
            ["camera_id=42 model_id=2 parameter_index=3 value=1.93 allowed_range=[-1,1]"]
        );
        camera.params[3] = 0.;
        camera.params[0] = 38401.;
        let details = invalid_camera_parameters(&camera).join("\n");
        assert!(details.contains("parameter_index=0 value=38401"));
        assert!(details.contains("allowed_normalized_range=[0.1,10]"));
        camera.params[0] = 38400.;
        assert!(invalid_camera_parameters(&camera).is_empty());
        let root = tempfile::tempdir().unwrap();
        model(&root.path().join("0"), 3, 1.93);
        let assessment = assess_output(root.path(), &CancellationToken::new())
            .unwrap()
            .unwrap();
        assert_eq!(assessment.invalid_camera_parameters.len(), 1);
        assert!(assessment.invalid_camera_parameters[0].contains("parameter_index=3 value=1.93"));
        assert!(!assessment.invalid_camera_parameters[0]
            .contains(root.path().to_string_lossy().as_ref()));
    }

    #[test]
    fn malformed_count_cannot_allocate_from_an_untrusted_image_header() {
        let root = tempfile::tempdir().unwrap();
        model(root.path(), 3, 0.5);
        std::fs::write(root.path().join("images.bin"), u64::MAX.to_le_bytes()).unwrap();
        assert!(assess_output(root.path(), &CancellationToken::new()).is_err());
    }

    #[test]
    fn promotion_refuses_models_outside_the_owned_attempt_directory() {
        let root = tempfile::tempdir().unwrap();
        let output = root.path().join("sparse");
        model(&output.join("0"), 3, 1.93);
        let cancel = CancellationToken::new();
        let previous = assess_output(&output, &cancel).unwrap().unwrap();
        let unowned = root.path().join("source-project");
        model(&unowned, 90, 0.5);
        let candidate = assess_output(&unowned, &cancel).unwrap().unwrap();
        assert!(promote_if_better(&output, &previous, &candidate, &cancel).is_err());
        assert!(unowned.join("images.bin").is_file());
        assert!(output.join("0/images.bin").is_file());
    }
}
