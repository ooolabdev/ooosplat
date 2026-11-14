//! Durable rollback ownership for the legacy, in-place Bridge path. Never
//! manufacture a pre-Bridge database from an already running old attempt.
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::Path,
    time::{Duration, Instant},
};

use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use super::PreparedFrames;
use crate::{
    error::{Result, SplatError},
    planner::{BridgeBackfillPlan, BridgeBackfillStatus},
    project::{manager::atomic_write_json, FrameState, PipelineStateFile, ProjectPaths},
    reconstruction::{direct_input::database_read_error, validator::ReconstructionReport},
    video::FramePlan,
};

const DIRECTORY: &str = "legacy-bridge-snapshot";

#[derive(Serialize, Deserialize)]
struct Snapshot {
    version: u32,
    database_sha256: String,
    database_semantics: String,
    frames: FrameState,
    plan: FramePlan,
    bridge_plan: BridgeBackfillPlan,
    initial_model: String,
    model_files: BTreeMap<String, String>,
    initial_registered: u64,
    initial_points: u64,
    #[serde(default)]
    actual_transparency: Option<bool>,
    #[serde(default)]
    rollback_pending: bool,
    #[serde(default)]
    duration_ms: Option<u64>,
}

fn stopped() -> SplatError {
    SplatError::Process(super::legacy_work_order::UNRESTORABLE_RECIPE.into())
}

fn check_cancel(cancel: &CancellationToken) -> Result<()> {
    if cancel.is_cancelled() {
        Err(SplatError::Cancelled)
    } else {
        Ok(())
    }
}

fn plain(path: &Path, directory: bool) -> Result<()> {
    let meta = std::fs::symlink_metadata(path)?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return Err(stopped());
        }
    }
    if meta.file_type().is_symlink()
        || (directory && !meta.is_dir())
        || (!directory && !meta.is_file())
    {
        return Err(stopped());
    }
    Ok(())
}

fn hash_file(path: &Path, cancel: &CancellationToken) -> Result<String> {
    plain(path, false)?;
    let mut file = std::fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        check_cancel(cancel)?;
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn read_snapshot(paths: &ProjectPaths) -> Result<Option<Snapshot>> {
    let folder = paths.colmap.join(DIRECTORY);
    match std::fs::symlink_metadata(&folder) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
        Ok(_) => plain(&folder, true)?,
    }
    plain(&paths.work, true)?;
    plain(&paths.colmap, true)?;
    let path = folder.join("snapshot.json");
    plain(&path, false)?;
    let snapshot: Snapshot = serde_json::from_reader(std::fs::File::open(path)?)?;
    let initial: BTreeSet<_> = snapshot
        .frames
        .selected_frames
        .iter()
        .map(|frame| frame.source_frame_index)
        .collect();
    let additional: BTreeSet<_> = snapshot
        .bridge_plan
        .selected_frame_indices
        .iter()
        .copied()
        .collect();
    if snapshot.version != 1
        || snapshot.frames.extracted_frames != Some(snapshot.bridge_plan.initial_input_images)
        || initial.len() as u64 != snapshot.bridge_plan.initial_input_images
        || additional.len() != snapshot.bridge_plan.selected_frame_indices.len()
        || additional.iter().any(|index| initial.contains(index))
    {
        return Err(stopped());
    }
    Ok(Some(snapshot))
}

fn model_hashes(
    paths: &ProjectPaths,
    relative: &str,
    cancel: &CancellationToken,
) -> Result<BTreeMap<String, String>> {
    let relative = Path::new(relative);
    if relative.is_absolute()
        || relative
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err(stopped());
    }
    let mut model = paths.colmap.clone();
    for component in relative.components() {
        model.push(component);
        plain(&model, true)?;
    }
    let mut files = BTreeMap::new();
    for name in [
        "cameras.bin",
        "images.bin",
        "points3D.bin",
        "cameras.txt",
        "images.txt",
        "points3D.txt",
        "rigs.bin",
        "frames.bin",
        "rigs.txt",
        "frames.txt",
    ] {
        match std::fs::symlink_metadata(model.join(name)) {
            Ok(_) => {
                files.insert(name.into(), hash_file(&model.join(name), cancel)?);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    if files.is_empty() {
        return Err(stopped());
    }
    Ok(files)
}

fn semantic_identity(connection: &Connection, cancel: &CancellationToken) -> Result<String> {
    use rusqlite::types::ValueRef;
    let mut hash = Sha256::new();
    let mut schema = connection.prepare("SELECT name,sql FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name").map_err(database_read_error)?;
    let tables = schema
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(database_read_error)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(database_read_error)?;
    for (name, sql) in tables {
        check_cancel(cancel)?;
        hash.update((name.len() as u64).to_le_bytes());
        hash.update(name.as_bytes());
        hash.update((sql.len() as u64).to_le_bytes());
        hash.update(sql.as_bytes());
        // COLMAP's supported schema consists of ordinary rowid tables. Do not
        // guess a row order for an unknown WITHOUT ROWID/virtual schema.
        if sql.to_ascii_lowercase().contains("without rowid")
            || sql.to_ascii_lowercase().contains("virtual table")
        {
            return Err(stopped());
        }
        let mut statement = connection
            .prepare(&format!(
                "SELECT * FROM \"{}\" ORDER BY rowid",
                name.replace('"', "\"\"")
            ))
            .map_err(database_read_error)?;
        let columns = statement.column_count();
        let mut rows = statement.query([]).map_err(database_read_error)?;
        while let Some(row) = rows.next().map_err(database_read_error)? {
            check_cancel(cancel)?;
            hash.update([0xff]);
            for column in 0..columns {
                match row.get_ref(column).map_err(database_read_error)? {
                    ValueRef::Null => hash.update([0]),
                    ValueRef::Integer(value) => {
                        hash.update([1]);
                        hash.update(value.to_le_bytes());
                    }
                    ValueRef::Real(value) => {
                        hash.update([2]);
                        hash.update(value.to_bits().to_le_bytes());
                    }
                    ValueRef::Text(value) => {
                        hash.update([3]);
                        hash.update((value.len() as u64).to_le_bytes());
                        hash.update(value);
                    }
                    ValueRef::Blob(value) => {
                        hash.update([4]);
                        hash.update((value.len() as u64).to_le_bytes());
                        hash.update(value);
                    }
                }
            }
        }
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn backup(
    source: &Connection,
    destination: &mut Connection,
    cancel: &CancellationToken,
) -> Result<()> {
    check_cancel(cancel)?;
    let backup = rusqlite::backup::Backup::new(source, destination).map_err(database_read_error)?;
    let mut busy_since = None;
    loop {
        check_cancel(cancel)?;
        match backup.step(128).map_err(database_read_error)? {
            rusqlite::backup::StepResult::Done => return Ok(()),
            rusqlite::backup::StepResult::More => busy_since = None,
            _ => {
                if busy_since.get_or_insert_with(Instant::now).elapsed() >= Duration::from_secs(2) {
                    return Err(SplatError::Io(std::io::Error::new(
                        std::io::ErrorKind::WouldBlock,
                        "Legacy Bridge database backup is in use",
                    )));
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    }
}

pub(super) async fn prepare(
    paths: &ProjectPaths,
    state: &PipelineStateFile,
    prepared: &PreparedFrames,
    plan: &BridgeBackfillPlan,
    model: &Path,
    report: &ReconstructionReport,
    cancel: &CancellationToken,
) -> Result<()> {
    check_cancel(cancel)?;
    let paths = paths.clone();
    let frames = state.frames.clone().ok_or_else(stopped)?;
    let original_plan = prepared.plan.clone();
    let bridge_plan = plan.clone();
    let initial_model = model
        .strip_prefix(&paths.colmap)
        .map_err(|_| stopped())?
        .to_string_lossy()
        .replace('\\', "/");
    let initial_registered = report.registered_images;
    let initial_points = report.points_3d;
    let actual_transparency = state.actual_transparency;
    let running = state.bridge_backfill.status == BridgeBackfillStatus::Running;
    let cancel = cancel.clone();
    crate::presets::spawn_pipeline_blocking(move || -> Result<()> {
        if let Some(snapshot) = read_snapshot(&paths)? {
            if snapshot.bridge_plan != bridge_plan
                || snapshot.initial_model != initial_model
                || snapshot.rollback_pending
                || hash_file(&paths.colmap.join(DIRECTORY).join("database.db"), &cancel)?
                    != snapshot.database_sha256
                || model_hashes(&paths, &initial_model, &cancel)? != snapshot.model_files
            {
                return Err(stopped());
            }
            if !running {
                let connection = crate::pipeline::sqlite_recovery::settle(
                    &paths.colmap.join("database.db"),
                    &cancel,
                )?;
                if semantic_identity(&connection, &cancel)? != snapshot.database_semantics {
                    return Err(stopped());
                }
            }
            return Ok(());
        }
        // A legacy Running attempt may already have inserted keypoints or
        // matches. It cannot be relabelled as the initial database.
        if running {
            return Err(stopped());
        }
        plain(&paths.work, true)?;
        plain(&paths.colmap, true)?;
        let extension = if frames.has_alpha { "png" } else { "jpg" };
        let expected: BTreeSet<_> = original_plan
            .selected_frames
            .iter()
            .map(|f| format!("frame_{:010}.{extension}", f.source_frame_index))
            .collect();
        if expected.len() as u64 != bridge_plan.initial_input_images {
            return Err(stopped());
        }
        for index in &bridge_plan.selected_frame_indices {
            let name = format!("frame_{index:010}.{extension}");
            if expected.contains(&name) {
                return Err(stopped());
            }
            for path in [
                paths.frames.join(&name),
                paths.colmap_frames.join(&name),
                paths.masks.join(format!("{name}.png")),
            ] {
                match std::fs::symlink_metadata(path) {
                    Ok(_) => return Err(stopped()),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
            }
        }
        let database = paths.colmap.join("database.db");
        plain(&database, false)?;
        let source = crate::pipeline::sqlite_recovery::settle(&database, &cancel)?;
        let actual = source
            .prepare("SELECT name FROM images ORDER BY name")
            .map_err(database_read_error)?
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(database_read_error)?
            .collect::<std::result::Result<BTreeSet<_>, _>>()
            .map_err(database_read_error)?;
        if actual != expected {
            return Err(stopped());
        }
        let identity = semantic_identity(&source, &cancel)?;
        let temporary = paths.colmap.join(format!(
            ".legacy-bridge-snapshot-{}.tmp",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&temporary)?;
        let destination_path = temporary.join("database.db");
        let mut destination = Connection::open(&destination_path).map_err(database_read_error)?;
        backup(&source, &mut destination, &cancel)?;
        drop(destination);
        drop(source);
        // FlushFileBuffers on Windows requires a writable handle. This is our
        // newly created staging database, never a source project file.
        std::fs::OpenOptions::new()
            .write(true)
            .open(&destination_path)?
            .sync_all()?;
        let snapshot = Snapshot {
            version: 1,
            database_sha256: hash_file(&destination_path, &cancel)?,
            database_semantics: identity,
            frames,
            plan: original_plan,
            bridge_plan,
            initial_model: initial_model.clone(),
            model_files: model_hashes(&paths, &initial_model, &cancel)?,
            initial_registered,
            initial_points,
            actual_transparency,
            rollback_pending: false,
            duration_ms: None,
        };
        let mut file = std::fs::File::create(temporary.join("snapshot.json"))?;
        serde_json::to_writer(&mut file, &snapshot)?;
        file.sync_all()?;
        drop(file);
        check_cancel(&cancel)?;
        std::fs::rename(temporary, paths.colmap.join(DIRECTORY))?;
        Ok(())
    })
    .await
    .map_err(|error| SplatError::Process(error.to_string()))?
}

pub(super) async fn mark_rollback(paths: &ProjectPaths, duration_ms: u64) -> Result<()> {
    let mut snapshot = read_snapshot(paths)?.ok_or_else(stopped)?;
    snapshot.rollback_pending = true;
    snapshot.duration_ms = Some(duration_ms);
    atomic_write_json(
        &paths.colmap.join(DIRECTORY).join("snapshot.json"),
        &snapshot,
    )
    .await
}

/// Caller must save state and then acknowledge. The pending barrier survives
/// failures in database restore, file removal and state replacement.
pub(super) async fn recover_pending(
    paths: &ProjectPaths,
    state: &mut PipelineStateFile,
    cancel: &CancellationToken,
) -> Result<bool> {
    check_cancel(cancel)?;
    if let Some(snapshot) = read_snapshot(paths)? {
        if snapshot.rollback_pending
            && (state.bridge_backfill.plan.as_ref() != Some(&snapshot.bridge_plan)
                || state.bridge_backfill.initial_model.as_deref() != Some(&snapshot.initial_model))
        {
            return Err(stopped());
        }
    }
    let paths = paths.clone();
    let cancel = cancel.clone();
    let snapshot = crate::presets::spawn_pipeline_blocking(move || -> Result<Option<Snapshot>> {
        let Some(snapshot) = read_snapshot(&paths)? else {
            return Ok(None);
        };
        if !snapshot.rollback_pending {
            return Ok(None);
        }
        let database = paths.colmap.join(DIRECTORY).join("database.db");
        if hash_file(&database, &cancel)? != snapshot.database_sha256
            || model_hashes(&paths, &snapshot.initial_model, &cancel)? != snapshot.model_files
        {
            return Err(stopped());
        }
        let source = Connection::open_with_flags(&database, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(database_read_error)?;
        let target = paths.colmap.join("database.db");
        plain(&target, false)?;
        let mut destination = crate::pipeline::sqlite_recovery::settle(&target, &cancel)?;
        backup(&source, &mut destination, &cancel)?;
        let integrity: String = destination
            .query_row("PRAGMA integrity_check(1)", [], |row| row.get(0))
            .map_err(database_read_error)?;
        if integrity != "ok"
            || semantic_identity(&destination, &cancel)? != snapshot.database_semantics
        {
            return Err(stopped());
        }
        drop(destination);
        drop(source);
        for directory in [&paths.frames, &paths.colmap_frames] {
            plain(directory, true)?;
        }
        if snapshot.frames.has_alpha {
            plain(&paths.masks, true)?;
        }
        let extension = if snapshot.frames.has_alpha {
            "png"
        } else {
            "jpg"
        };
        for index in &snapshot.bridge_plan.selected_frame_indices {
            check_cancel(&cancel)?;
            let name = format!("frame_{index:010}.{extension}");
            let mut files = vec![paths.frames.join(&name), paths.colmap_frames.join(&name)];
            if snapshot.frames.has_alpha {
                files.push(paths.masks.join(format!("{name}.png")));
            }
            for path in files {
                match std::fs::symlink_metadata(&path) {
                    Ok(_) => {
                        plain(&path, false)?;
                        std::fs::remove_file(path)?;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
            }
        }
        Ok(Some(snapshot))
    })
    .await
    .map_err(|error| SplatError::Process(error.to_string()))??;
    let Some(snapshot) = snapshot else {
        return Ok(false);
    };
    state.frames = Some(snapshot.frames);
    state.actual_transparency = snapshot.actual_transparency;
    state.bridge_backfill.status = BridgeBackfillStatus::FailedRolledBack;
    state.bridge_backfill.selected_model = Some(snapshot.initial_model);
    state.bridge_backfill.final_registered_images = Some(snapshot.initial_registered);
    state.bridge_backfill.final_points_3d = Some(snapshot.initial_points);
    state.bridge_backfill.duration_ms = snapshot.duration_ms;
    Ok(true)
}

pub(super) async fn acknowledge_rollback(paths: &ProjectPaths) -> Result<()> {
    let mut snapshot = read_snapshot(paths)?.ok_or_else(stopped)?;
    snapshot.rollback_pending = false;
    atomic_write_json(
        &paths.colmap.join(DIRECTORY).join("snapshot.json"),
        &snapshot,
    )
    .await
}

pub(super) fn restore_prepared(paths: &ProjectPaths, prepared: &mut PreparedFrames) -> Result<()> {
    let snapshot = read_snapshot(paths)?.ok_or_else(stopped)?;
    prepared.plan = snapshot.plan;
    // FramePlan intentionally omits its large lists from serialization. The
    // durable FrameState is the authority for the frozen frame selection.
    prepared.plan.selected_frames = snapshot.frames.selected_frames.clone();
    prepared.plan.candidate_frames = snapshot.frames.candidate_frames.clone();
    prepared.extracted_frames = snapshot.frames.extracted_frames.ok_or_else(stopped)?;
    prepared.mask_count = snapshot.frames.mask_count.unwrap_or(0);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        presets::Quality, project::ProjectInputType,
        reconstruction::validator::ReconstructionQuality, video::PlannedFrame,
    };

    fn fixture(
        root: &Path,
    ) -> (
        ProjectPaths,
        PipelineStateFile,
        PreparedFrames,
        BridgeBackfillPlan,
        std::path::PathBuf,
        ReconstructionReport,
    ) {
        let paths = ProjectPaths::existing(uuid::Uuid::new_v4(), root.to_path_buf());
        for dir in [
            &paths.frames,
            &paths.colmap_frames,
            &paths.masks,
            &paths.colmap,
        ] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let selected: Vec<_> = [0, 10]
            .into_iter()
            .map(|index| PlannedFrame {
                source_frame_index: index,
                timestamp_seconds: index as f64 / 30.,
            })
            .collect();
        for frame in &selected {
            let name = format!("frame_{:010}.jpg", frame.source_frame_index);
            std::fs::write(paths.frames.join(&name), b"original pixels").unwrap();
            std::fs::write(paths.colmap_frames.join(&name), b"original pixels").unwrap();
        }
        let database = Connection::open(paths.colmap.join("database.db")).unwrap();
        database.execute_batch("CREATE TABLE images(image_id INTEGER PRIMARY KEY, name TEXT, camera_id INTEGER); CREATE TABLE keypoints(image_id INTEGER PRIMARY KEY, rows INTEGER, cols INTEGER, data BLOB); CREATE TABLE matches(pair_id INTEGER PRIMARY KEY, data BLOB); INSERT INTO images VALUES(1,'frame_0000000000.jpg',1),(2,'frame_0000000010.jpg',1); INSERT INTO keypoints VALUES(1,1,2,x'0000000000000000'),(2,0,2,x''); INSERT INTO matches VALUES(2147483649,x'0102');").unwrap();
        drop(database);
        let model = paths.colmap.join("sparse/0");
        std::fs::create_dir_all(&model).unwrap();
        // This module tests snapshot ownership, not COLMAP model validation.
        // Its caller separately validates a full model before prepare/rollback.
        for name in ["cameras.bin", "images.bin", "points3D.bin"] {
            std::fs::write(model.join(name), b"trusted model fixture").unwrap();
        }
        let plan = FramePlan {
            selected_frames: selected.clone(),
            candidate_frames: selected.clone(),
            retention_ratio: 0.2,
            sampling_fps: 6.,
            estimated_frames: 2,
            rescue_max_frames: 3,
            ..Default::default()
        };
        let mut state = PipelineStateFile::created_for(Quality::Fast, ProjectInputType::Video);
        state.frames = Some(FrameState {
            extracted_frames: Some(2),
            estimated_frames: 2,
            selected_frames: selected.clone(),
            candidate_frames: selected,
            mask_count: Some(0),
            sampling_fps: 6.,
            retention_ratio: 0.2,
            ..Default::default()
        });
        let bridge = BridgeBackfillPlan {
            initial_input_images: 2,
            initial_registered_images: 1,
            selected_frame_indices: vec![5],
            ..Default::default()
        };
        state.bridge_backfill.plan = Some(bridge.clone());
        state.bridge_backfill.initial_model = Some("sparse/0".into());
        let prepared = PreparedFrames {
            input_type: ProjectInputType::Video,
            video: None,
            image_sequence: None,
            plan,
            extracted_frames: 2,
            image_format: "jpg".into(),
            mask_count: 0,
            has_alpha: false,
            working_width: 128,
            working_height: 64,
        };
        let report = ReconstructionReport {
            input_images: 2,
            registered_images: 1,
            registered_ratio: 0.5,
            points_3d: 30,
            quality: ReconstructionQuality::Warning,
        };
        (paths, state, prepared, bridge, model, report)
    }

    #[tokio::test]
    async fn rollback_restores_database_and_owned_frames_and_survives_state_save_interruption() {
        let root = tempfile::tempdir().unwrap();
        let (paths, mut state, mut prepared, plan, model, report) = fixture(root.path());
        let cancel = CancellationToken::new();
        std::fs::write(paths.project.join("final.ply"), b"published model").unwrap();
        prepare(&paths, &state, &prepared, &plan, &model, &report, &cancel)
            .await
            .unwrap();
        state.bridge_backfill.status = BridgeBackfillStatus::Running;
        state.actual_transparency = Some(true);
        let database = Connection::open(paths.colmap.join("database.db")).unwrap();
        database.execute_batch("INSERT INTO images VALUES(3,'frame_0000000005.jpg',1); INSERT INTO matches VALUES(2147483650,x'0304'); DELETE FROM keypoints WHERE image_id=1;").unwrap();
        drop(database);
        for folder in [&paths.frames, &paths.colmap_frames] {
            std::fs::write(folder.join("frame_0000000005.jpg"), b"bridge").unwrap();
        }
        state.frames.as_mut().unwrap().extracted_frames = Some(3);
        mark_rollback(&paths, 123).await.unwrap();
        assert!(recover_pending(&paths, &mut state, &cancel).await.unwrap());
        assert_eq!(state.frames.as_ref().unwrap().extracted_frames, Some(2));
        assert_eq!(
            state.bridge_backfill.status,
            BridgeBackfillStatus::FailedRolledBack
        );
        assert_eq!(state.bridge_backfill.duration_ms, Some(123));
        assert_eq!(state.actual_transparency, None);
        assert!(!paths.frames.join("frame_0000000005.jpg").exists());
        let database = Connection::open(paths.colmap.join("database.db")).unwrap();
        assert_eq!(
            database
                .query_row("SELECT count(*) FROM images", [], |row| row
                    .get::<_, u64>(0))
                .unwrap(),
            2
        );
        assert_eq!(
            database
                .query_row("SELECT count(*) FROM keypoints", [], |row| row
                    .get::<_, u64>(0))
                .unwrap(),
            2
        );
        drop(database);
        // No state acknowledgement yet: a restart repeats the same owned
        // rollback instead of re-running Mapper or forgetting the barrier.
        assert!(read_snapshot(&paths).unwrap().unwrap().rollback_pending);
        assert!(recover_pending(&paths, &mut state, &cancel).await.unwrap());
        atomic_write_json(&paths.state, &state).await.unwrap();
        acknowledge_rollback(&paths).await.unwrap();
        assert!(!recover_pending(&paths, &mut state, &cancel).await.unwrap());
        restore_prepared(&paths, &mut prepared).unwrap();
        assert_eq!(prepared.plan.selected_frames.len(), 2);
        assert_eq!(prepared.plan.candidate_frames.len(), 2);
        assert_eq!(
            std::fs::read(paths.project.join("final.ply")).unwrap(),
            b"published model"
        );
        assert_eq!(
            std::fs::read(paths.frames.join("frame_0000000000.jpg")).unwrap(),
            b"original pixels"
        );
    }

    #[tokio::test]
    async fn old_running_without_snapshot_cannot_mislabel_mutated_database_as_initial() {
        let root = tempfile::tempdir().unwrap();
        let (paths, mut state, prepared, plan, model, report) = fixture(root.path());
        state.bridge_backfill.status = BridgeBackfillStatus::Running;
        assert!(prepare(
            &paths,
            &state,
            &prepared,
            &plan,
            &model,
            &report,
            &CancellationToken::new()
        )
        .await
        .is_err());
        assert!(!paths.colmap.join(DIRECTORY).exists());
    }

    #[tokio::test]
    async fn modified_snapshot_or_seed_model_cannot_restore_or_delete_current_frames() {
        for change_model in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let (paths, mut state, prepared, plan, model, report) = fixture(root.path());
            let cancel = CancellationToken::new();
            prepare(&paths, &state, &prepared, &plan, &model, &report, &cancel)
                .await
                .unwrap();
            state.bridge_backfill.status = BridgeBackfillStatus::Running;
            mark_rollback(&paths, 1).await.unwrap();
            let before = hash_file(&paths.colmap.join("database.db"), &cancel).unwrap();
            std::fs::write(paths.frames.join("frame_0000000005.jpg"), b"bridge").unwrap();
            if change_model {
                std::fs::write(model.join("images.bin"), b"changed").unwrap();
            } else {
                std::fs::write(paths.colmap.join(DIRECTORY).join("database.db"), b"changed")
                    .unwrap();
            }
            assert!(recover_pending(&paths, &mut state, &cancel).await.is_err());
            assert_eq!(state.bridge_backfill.status, BridgeBackfillStatus::Running);
            assert_eq!(
                before,
                hash_file(&paths.colmap.join("database.db"), &cancel).unwrap()
            );
            assert!(paths.frames.join("frame_0000000005.jpg").is_file());
            assert!(read_snapshot(&paths).unwrap().unwrap().rollback_pending);
        }
    }

    #[tokio::test]
    async fn failed_owned_file_cleanup_keeps_pending_barrier_until_it_can_complete() {
        let root = tempfile::tempdir().unwrap();
        let (paths, mut state, prepared, plan, model, report) = fixture(root.path());
        let cancel = CancellationToken::new();
        prepare(&paths, &state, &prepared, &plan, &model, &report, &cancel)
            .await
            .unwrap();
        state.bridge_backfill.status = BridgeBackfillStatus::Running;
        mark_rollback(&paths, 1).await.unwrap();
        let blocked = paths.colmap_frames.join("frame_0000000005.jpg");
        std::fs::create_dir(&blocked).unwrap();
        assert!(recover_pending(&paths, &mut state, &cancel).await.is_err());
        assert_eq!(state.bridge_backfill.status, BridgeBackfillStatus::Running);
        assert!(read_snapshot(&paths).unwrap().unwrap().rollback_pending);
        std::fs::remove_dir(blocked).unwrap();
        assert!(recover_pending(&paths, &mut state, &cancel).await.unwrap());
    }

    #[tokio::test]
    async fn cancellation_keeps_rollback_barrier_and_original_failure_state() {
        let root = tempfile::tempdir().unwrap();
        let (paths, mut state, prepared, plan, model, report) = fixture(root.path());
        prepare(
            &paths,
            &state,
            &prepared,
            &plan,
            &model,
            &report,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        state.bridge_backfill.status = BridgeBackfillStatus::Running;
        mark_rollback(&paths, 1).await.unwrap();
        let cancelled = CancellationToken::new();
        cancelled.cancel();
        assert!(matches!(
            recover_pending(&paths, &mut state, &cancelled).await,
            Err(SplatError::Cancelled)
        ));
        assert_eq!(state.bridge_backfill.status, BridgeBackfillStatus::Running);
        assert!(read_snapshot(&paths).unwrap().unwrap().rollback_pending);
    }
}
