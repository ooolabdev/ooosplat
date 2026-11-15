//! One-off, opt-in recovery of the disk-full record loss on 2026-11-14.
//! Not an application recovery API: no task registry writes or engine calls.
//! `validate` works only on the frozen D: copy; `install` requires its report
//! and byte-identical original evidence. Remove after the incident is closed.
#![cfg(windows)]

use super::*;
use crate::pipeline::{artifacts, reshoot_connection, reshoot_snapshot, work_order};
use crate::project::creation::{ProjectCreationIntent, ProjectCreationState};
use crate::project::manager::atomic_write_json;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use tokio_util::sync::CancellationToken;

const RECOVERY_ROOT: &str = r"D:\OOOSplat-Recovery\20261114-8016f801-save-repair";
const ORIGINAL: &str = r"E:\GaussianSplatting\test\20261114-133109_reshoot_8016f801";
const TASK: &str = "c596ec9d-0f41-4ef1-a73d-90ddc69226f7";
const PROJECT: &str = "8016f801-de64-4b0c-9d18-890e90548964";
const RUN: &str = "adac9924-6852-453a-bc92-543f44a74953";
const TASKS_SHA: &str = "a50aceea2ed15d203779353ea2fd02bb0dda2c66e613b74cb4a19298de2d5368";

fn require(condition: bool, detail: &str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(SplatError::Process(format!("Recovery stopped: {detail}")))
    }
}

fn read_json(path: &Path) -> Result<Value> {
    Ok(serde_json::from_slice(&std::fs::read(path)?)?)
}

fn fingerprint(path: &Path) -> Result<String> {
    crate::video::panorama::fingerprint(path)
}

/// Full frozen copy evidence; no mtime/size-only assertion is used to install.
fn inventory(root: &Path) -> Result<BTreeMap<String, String>> {
    fn visit(root: &Path, dir: &Path, out: &mut BTreeMap<String, String>) -> Result<()> {
        use std::os::windows::fs::MetadataExt;
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            let meta = std::fs::symlink_metadata(&path)?;
            require(
                !meta.file_type().is_symlink() && meta.file_attributes() & 0x400 == 0,
                "redirected evidence entry",
            )?;
            if meta.is_dir() {
                visit(root, &path, out)?;
            } else {
                require(meta.is_file(), "non-file evidence entry")?;
                out.insert(
                    path.strip_prefix(root)
                        .map_err(|e| SplatError::Process(e.to_string()))?
                        .to_string_lossy()
                        .replace('\\', "/"),
                    fingerprint(&path)?,
                );
            }
        }
        Ok(())
    }
    let mut out = BTreeMap::new();
    visit(root, root, &mut out)?;
    Ok(out)
}

fn verify_no_pending_barrier(paths: &ProjectPaths) -> Result<()> {
    // The task summary omits barriers. Absence is established from complete
    // preparation records plus the closed execution and full directory copy.
    for absent in ["cache-archive", "joint-bridge"] {
        require(
            !paths.work.join(absent).try_exists()?,
            "unexpected recovery directory",
        )?;
    }
    let log = std::fs::read_to_string(paths.logs.join("retry-preparation.log"))?;
    require(
        !log.contains("operation=archive_retry_cache")
            && !log.contains("operation=create_retry_archive")
            && !log.contains("operation=write_retry_archive_progress"),
        "historical archive requires separate reconciliation",
    )?;
    let writes = log
        .lines()
        .filter(|line| line.contains("operation=write_retry_reuse_barrier"))
        .collect::<Vec<_>>();
    let expected = [
        (
            "2026-11-14T05:31:12.439089100+00:00",
            "8e783a00-558c-45a8-a197-39bf4e78ee8e",
            "started",
        ),
        (
            "2026-11-14T05:31:12.442731+00:00",
            "8e783a00-558c-45a8-a197-39bf4e78ee8e",
            "completed",
        ),
        (
            "2026-11-14T05:36:43.507555500+00:00",
            "9e174f70-642b-48bb-94dd-417514b1daf4",
            "started",
        ),
        (
            "2026-11-14T05:36:43.509915100+00:00",
            "9e174f70-642b-48bb-94dd-417514b1daf4",
            "completed",
        ),
    ];
    require(
        writes.len() == expected.len(),
        "unexpected historical barrier count",
    )?;
    for (line, (time, id, status)) in writes.into_iter().zip(expected) {
        let known = format!("{time} preparation_id={id} status={status} operation=write_retry_reuse_barrier path_role=project_state cache=none error_kind=none os_error=none io_substep=none");
        require(line == known, "unresolved or unexpected historical barrier")?;
    }
    for name in [
        "database.db-wal",
        "database.db-journal",
        "base-database.db-wal",
        "base-database.db-journal",
    ] {
        let sidecar = paths.colmap.join(name);
        require(
            !sidecar.try_exists()?,
            "database sidecar requires separate recovery",
        )?;
    }
    require(
        !paths
            .work
            .join(crate::pipeline::publication::TRANSACTION_FILE_NAME)
            .try_exists()?,
        "publication requires normal reconciliation",
    )?;
    require(
        !paths.project.join("final.ply").try_exists()?,
        "unexpected published output",
    )?;
    Ok(())
}

/// Recovery must prove historical feature coordinates, not merely certify the
/// current files as a new baseline. Ordinary manifest capture is intentionally
/// lighter and does not compare every sparse observation with SQLite.
fn verify_database_model(database: &Path, model_path: &Path, image_roots: &[&Path]) -> Result<()> {
    use crate::reconstruction::direct_input::{
        database_read_error, open_immutable_database, read_model,
    };
    let db = open_immutable_database(database)?;
    let integrity: String = db
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .map_err(database_read_error)?;
    require(integrity == "ok", "database integrity failure")?;
    artifacts::read_database_summary(&db, &CancellationToken::new())?;
    let model = read_model(model_path)?;
    let mut statement = db
        .prepare("SELECT image_id,name,camera_id FROM images ORDER BY image_id")
        .map_err(database_read_error)?;
    let images = statement
        .query_map([], |r| {
            Ok((
                r.get::<_, u32>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, u32>(2)?,
            ))
        })
        .map_err(database_read_error)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(database_read_error)?;
    require(
        images.len() == model.images.len(),
        "database/model image set differs",
    )?;
    for camera in &model.cameras {
        let geometry: (i32, u64, u64) = db
            .query_row(
                "SELECT model,width,height FROM cameras WHERE camera_id=?1",
                [camera.id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .map_err(database_read_error)?;
        require(
            geometry == (camera.model_id, camera.width, camera.height),
            "camera geometry differs from database",
        )?;
    }
    for (id, name, camera_id) in images {
        let image = model
            .images
            .iter()
            .find(|i| i.pose.id == id && i.pose.name == name && i.pose.camera_id == camera_id)
            .ok_or_else(|| {
                SplatError::Process(
                    "Recovery stopped: database/model image identity differs".into(),
                )
            })?;
        let camera = model
            .cameras
            .iter()
            .find(|c| c.id == camera_id)
            .ok_or_else(|| SplatError::Process("Missing model camera".into()))?;
        for root in image_roots {
            let path = crate::reconstruction::direct_input::image_path(root, &name)?;
            let header = crate::video::image_projection::inspect(&path)?;
            require(
                header.orientation == 1
                    && !header.alpha
                    && (u64::from(header.width), u64::from(header.height))
                        == (camera.width, camera.height),
                "prepared image coordinate/Alpha mapping differs",
            )?;
        }
        let (rows, columns, data): (usize, usize, Option<Vec<u8>>) = db
            .query_row(
                "SELECT rows,cols,data FROM keypoints WHERE image_id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .map_err(database_read_error)?;
        let data = data.unwrap_or_default();
        require(
            rows == image.observations.len()
                && matches!(columns, 2 | 4 | 6)
                && rows.checked_mul(columns).and_then(|n| n.checked_mul(4)) == Some(data.len()),
            "sparse observation rows differ from features",
        )?;
        for (observation, bytes) in image
            .observations
            .iter()
            .zip(data.chunks_exact(columns * 4))
        {
            let x = f64::from(f32::from_le_bytes(bytes[..4].try_into().unwrap()));
            let y = f64::from(f32::from_le_bytes(bytes[4..8].try_into().unwrap()));
            require(
                x.is_finite()
                    && y.is_finite()
                    && (x - observation.x).abs() <= 0.001
                    && (y - observation.y).abs() <= 0.001,
                "sparse observation coordinates differ from features",
            )?;
        }
    }
    Ok(())
}

fn verify_opaque_rotation_masks(paths: &ProjectPaths) -> Result<u64> {
    let model =
        crate::reconstruction::direct_input::read_model(&paths.colmap.join("authoritative"))?;
    let expected = model
        .images
        .iter()
        .map(|image| format!("{}.png", image.pose.name))
        .collect::<std::collections::BTreeSet<_>>();
    let mut actual = std::collections::BTreeSet::new();
    for entry in std::fs::read_dir(&paths.masks)? {
        let entry = entry?;
        require(entry.file_type()?.is_file(), "unexpected Mask entry")?;
        actual.insert(entry.file_name().to_string_lossy().into_owned());
    }
    require(
        actual == expected && actual.len() == 32,
        "Mask/image names differ",
    )?;
    for image in &model.images {
        let camera = model
            .cameras
            .iter()
            .find(|c| c.id == image.pose.camera_id)
            .ok_or_else(|| SplatError::Process("Missing Mask camera".into()))?;
        let path = paths.masks.join(format!("{}.png", image.pose.name));
        let mut reader = image::ImageReader::open(path)?.with_guessed_format()?;
        let mut limits = image::Limits::default();
        limits.max_alloc = Some(crate::video::panorama::MAX_DECODE_BYTES);
        reader.limits(limits);
        let decoded = reader
            .decode()
            .map_err(|e| SplatError::Process(format!("Recovery Mask decode failed: {e}")))?;
        require(
            decoded.color() == image::ColorType::L8
                && (u64::from(decoded.width()), u64::from(decoded.height()))
                    == (camera.width, camera.height)
                && decoded
                    .into_luma8()
                    .as_raw()
                    .iter()
                    .all(|pixel| *pixel == 255),
            "rotation-derived Mask is not opaque/aligned",
        )?;
    }
    Ok(actual.len() as u64)
}

fn restore_records(
    paths: &ProjectPaths,
    registry: &Value,
) -> Result<(ProjectMetadata, PipelineStateFile)> {
    let task_value = registry["tasks"][TASK].clone();
    let task: crate::tasks::TaskRecord = serde_json::from_value(task_value.clone())?;
    require(
        task_value["status"] == "failed" && task_value["run_id"] == RUN,
        "task/run changed",
    )?;
    require(
        task_value["project_id"] == PROJECT,
        "project identity changed",
    )?;
    require(
        task_value["elapsed_ms"] == 7_625_096u64,
        "elapsed evidence changed",
    )?;
    require(task_value["quality"] == "fast", "quality changed")?;
    let request = &task_value["request_parameters"];
    let actual = &task_value["actual_configuration"];
    let intent: ProjectCreationIntent =
        serde_json::from_value(task_value["creation_intent"].clone())?;
    // Reproduce only the historical reservation's pure digest. Do not call
    // reserve(), which also chooses a new directory/UUID and may stat media.
    let mut intent_fields = json!({
        "input": task.input_path, "request": task.request_parameters,
        "quality": task.quality, "configuration": task.configuration,
        "source": task.source_project_id, "root": task.projects_root,
        "planner": task.planner_enabled, "brushExecutionOptions": task.brush_execution_options,
    });
    if let Some(version) = task.ba_policy_version {
        intent_fields["baPolicyVersion"] = json!(version);
    }
    if let Some(version) = task.camera_grouping_policy_version {
        intent_fields["cameraGroupingPolicyVersion"] = json!(version);
    }
    if let Some(version) = task.ordinary_image_preparation_version {
        intent_fields["ordinaryImagePreparationVersion"] = json!(version);
    }
    let request_identity = format!("{:x}", Sha256::digest(serde_json::to_vec(&intent_fields)?));
    require(
        request_identity == intent.request_identity,
        "creation request no longer bound",
    )?;
    require(
        intent.project_id == paths.id && intent.task_id.to_string() == TASK,
        "creation ownership mismatch",
    )?;
    require(
        intent.project_path == Path::new(ORIGINAL),
        "unexpected original target",
    )?;
    require(
        request["generationPolicy"]["version"] == 7,
        "historical v7 evidence missing",
    )?;
    require(
        actual["inputs"]["request"] == request["inputs"],
        "request differs from saved inputs",
    )?;

    let snapshot = reshoot_snapshot::verify_snapshot(&paths.work.join("reshoot-snapshot-v3"))?;
    require(
        snapshot.registered_images.len() == 17 && !snapshot.has_erp,
        "snapshot not the expected ordinary source",
    )?;
    let source_ply = snapshot
        .source_ply
        .as_ref()
        .ok_or_else(|| SplatError::Process("Missing frozen source PLY".into()))?;
    require(
        source_ply.splat_count == 128_674 && source_ply.sh_degree == Some(3),
        "frozen PLY facts changed",
    )?;
    let context = read_json(&paths.work.join("reshoot-snapshot-v3/source/context.json"))?;
    require(
        context["generationPolicy"]["version"] == 7,
        "source policy evidence missing",
    )?;
    require(
        context["gps"]["status"] == "fallback" && context["gps"]["referenceValid"] == false,
        "source geography unexpectedly trusted",
    )?;

    let recipe: crate::pipeline::reshoot::ReshootRecipe =
        serde_json::from_value(actual["reshoot_recipe"].clone())?;
    require(
        recipe.training_fallbacks == 1
            && recipe.reconstruction_fallbacks == 0
            && recipe.actual.training == crate::pipeline::reshoot::TrainingMode::Cold
            && recipe.actual.reconstruction
                == crate::pipeline::reshoot::ReconstructionMode::Incremental
            && recipe.new_names.len() == 15
            && recipe.source_names.len() == 17
            && recipe.panorama_frames.len() == 32,
        "fallback / provenance evidence incomplete",
    )?;
    require(
        recipe.init_sha256.as_ref() == Some(&source_ply.sha256),
        "source PLY provenance mismatch",
    )?;
    require(
        actual["inputs"]["executionPlan"]["trainingSeed"]["kind"] == "sparsePoints",
        "actual route is not cold",
    )?;
    require(
        actual["inputs"]["executionPlan"]["fallback"]["trainingFallbacksUsed"] == 1,
        "plan allowance mismatch",
    )?;
    let records: BTreeMap<_, _> = recipe
        .panorama_frames
        .iter()
        .map(|r| (r.prepared_name.clone(), r.clone()))
        .collect();
    require(records.len() == 32, "duplicate saved origin")?;
    let mut inputs_json = actual["inputs"].clone();
    for source in inputs_json["sources"]
        .as_array_mut()
        .ok_or_else(|| SplatError::Process("Missing sources".into()))?
    {
        let names = source["names"]
            .as_array()
            .ok_or_else(|| SplatError::Process("Missing names".into()))?;
        let mapped = names
            .iter()
            .map(|n| {
                records
                    .get(n.as_str().unwrap_or_default())
                    .cloned()
                    .ok_or_else(|| SplatError::Process("Missing frozen origin".into()))
            })
            .collect::<Result<Vec<_>>>()?;
        require(
            mapped
                .iter()
                .all(|r| r.pts.is_none() && r.timestamp_seconds.is_none()),
            "not the expected image input",
        )?;
        source["originals"] =
            serde_json::to_value(mapped.iter().map(|r| &r.source_name).collect::<Vec<_>>())?;
        source["panoramaRecords"] = serde_json::to_value(mapped)?;
    }
    let mut inputs: crate::pipeline::inputs::InputCheckpoint = serde_json::from_value(inputs_json)?;
    require(
        inputs.sources.len() == 2 && inputs.prepared && inputs.bridge_used,
        "prepared/Bridge history differs",
    )?;
    require(
        inputs
            .sources
            .iter()
            .all(|s| s.video.is_none() && s.frames.is_none()),
        "unexpected video plans",
    )?;
    // This changes only disposable dataset readiness, not its saved recipe key.
    inputs.reference_image = None;

    let mut state = PipelineStateFile::created_for(Quality::Fast, ProjectInputType::Images);
    state.stage = PipelineStage::Failed;
    state.creation = Some(ProjectCreationState {
        intent: intent.clone(),
        ready: true,
    });
    state.generation_policy = Some(serde_json::from_value(request["generationPolicy"].clone())?);
    state.configuration = Some(serde_json::from_value(task_value["configuration"].clone())?);
    state.planner_enabled = actual["planner_enabled"]
        .as_bool()
        .ok_or_else(|| SplatError::Process("Missing planner setting".into()))?;
    state.ordinary_image_preparation_version = intent.ordinary_image_preparation_version;
    state.camera_grouping_policy_version = intent.camera_grouping_policy_version;
    state.ba_policy_version = intent.ba_policy_version;
    state.resolution_policy_version =
        serde_json::from_value(actual["resolution_policy_version"].clone())?;
    state.resolution_plan = serde_json::from_value(actual["resolution_plan"].clone())?;
    state.brush_training = serde_json::from_value(actual["brush_training"].clone())?;
    require(
        !state.brush_training.oom_retry_started && !state.brush_training.oom_retry_used,
        "OOM allowance evidence changed",
    )?;
    require(
        state.brush_training.execution_options == intent.brush_execution_options,
        "Alpha execution options differ",
    )?;
    let resolved = actual["brush_training"]["resolved"].clone();
    require(
        resolved == actual["brush_training"]["initialResolved"],
        "unexpected degraded recipe",
    )?;
    require(
        resolved["preset"]["totalSteps"] == 8000
            && resolved["preset"]["maxSplats"] == 1_234_708
            && resolved["preset"]["maxResolution"] == 5712,
        "executed recipe evidence differs",
    )?;
    let runs = task_value["runs"]
        .as_array()
        .ok_or_else(|| SplatError::Process("Missing run evidence".into()))?;
    require(
        runs.len() == 2 && runs[1]["run_id"] == RUN,
        "unexpected execution history",
    )?;
    let executions = runs[1]["brush_executions"]
        .as_array()
        .ok_or_else(|| SplatError::Process("Missing execution evidence".into()))?;
    require(
        executions.len() == 2
            && executions
                .iter()
                .all(|e| e["configuration"] == resolved && !e["endedAt"].is_null()),
        "Brush recipe/exits not frozen",
    )?;
    state.frames = Some(crate::project::FrameState {
        extracted_frames: Some(32),
        initial_extracted_frames: 32,
        colmap_input_version: Some(2),
        ..Default::default()
    });
    state.image_sequence = inputs
        .sources
        .iter()
        .find(|s| s.id != "__source")
        .and_then(|s| s.image_sequence.clone());
    state.actual_transparency = Some(false);
    state.inputs = Some(inputs);
    state.reshoot = Some(crate::project::metadata::ReshootState {
        recipe: Some(recipe),
        source_snapshot_complete: true,
        supplemental_frames_complete: true,
        supplemental_features_complete: true,
        incremental_matching_complete: true,
        incremental_reconstruction_complete: true,
        source_image_count: 17,
        reshoot_frame_count: 15,
        registered_reshoot_count: 15,
        colmap_input_version: Some(2),
        snapshot_v3_identity: Some(snapshot.content_identity),
        snapshot_v3_ready: true,
        ..Default::default()
    });
    state.features_complete = true;
    state.matching_complete = true;
    state.reconstruction_complete = true;

    let authority = paths.colmap.join("authoritative");
    let model = crate::reconstruction::direct_input::read_model(&authority)?;
    require(
        model.images.len() == 32 && model.points.len() == 7424,
        "authoritative reconstruction counts changed",
    )?;
    require(
        model.cameras.len() == 2 && model.cameras.iter().all(|c| c.model_id == 2),
        "unexpected camera models",
    )?;
    verify_database_model(
        &paths.colmap.join("database.db"),
        &authority,
        &[&paths.frames, &paths.colmap_frames],
    )?;
    let base_model = paths.colmap.join("base-model");
    verify_database_model(
        &paths.colmap.join("base-database.db"),
        &base_model,
        &[&paths.frames, &paths.colmap_frames],
    )?;
    require(
        gps_runner::model_identity(&base_model)?
            == gps_runner::model_identity(&paths.work.join("reshoot-snapshot-v3/colmap/sparse"))?,
        "base model differs from immutable source",
    )?;
    require(
        reshoot_runner::coordinates_match(&base_model, &authority)?,
        "old camera coordinate frame changed",
    )?;
    let proof = reshoot_connection::validate_model(&state, &authority, &CancellationToken::new())?
        .ok_or_else(|| SplatError::Process("Missing connection proof".into()))?;
    require(
        proof.old_registered == 17 && proof.new_registered == 15,
        "new/old connection not proven",
    )?;
    state.reshoot.as_mut().unwrap().connection_proof = Some(proof);

    // GPS never changed this model's coordinates. Preserve the abandoned route
    // using the saved local records, not the source project's GPS checkpoint.
    require(
        actual["gps"]["status"] == "fallback",
        "GPS route evidence missing",
    )?;
    let gps: crate::reconstruction::gps::Manifest =
        serde_json::from_value(read_json(&paths.work.join("gps/manifest.json"))?)?;
    require(
        gps.records.len() == 32 && !paths.colmap.join("gps-v1").try_exists()?,
        "unexpected GPS attempt assets",
    )?;
    let input_identity = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(
            gps_runner::model_identity(&authority)?,
            Option::<crate::video::gps::GpsFix>::None
        ))?)
    );
    state.gps = Some(crate::reconstruction::gps::Checkpoint {
        status: "fallback".into(),
        fingerprint: crate::reconstruction::gps::fingerprint(&gps.records)?,
        input_identity: Some(input_identity),
        manifest_identity: Some(crate::reconstruction::gps::manifest_identity(&gps)?),
        ..crate::reconstruction::gps::Checkpoint::pending()
    });

    // Bind the restored state to pre-incident model/pixels, not newly generated
    // manifests alone. This is the exact historical dataset key construction.
    let dataset: (String, Value) =
        serde_json::from_value(read_json(&paths.brush.join("dataset/inputs-dataset.json"))?)?;
    let mut pixels = model
        .images
        .iter()
        .map(|i| (i.pose.name.clone(), paths.frames.join(&i.pose.name)))
        .collect::<Vec<_>>();
    pixels.sort_by(|a, b| a.0.cmp(&b.0));
    let mut pixel_hash = Sha256::new();
    for (name, path) in pixels {
        pixel_hash.update((name.len() as u64).to_le_bytes());
        pixel_hash.update(name.as_bytes());
        pixel_hash.update(fingerprint(&path)?);
    }
    let order = state
        .inputs
        .as_ref()
        .unwrap()
        .sources
        .iter()
        .flat_map(|s| s.names.clone())
        .collect::<Vec<_>>();
    let key = serde_json::to_string(&(
        "content-bound-dataset-v1",
        2,
        gps_runner::model_identity(&authority)?,
        format!("{:x}", pixel_hash.finalize()),
        order,
        Quality::Fast,
        1,
        true,
    ))?;
    require(
        dataset.0 == key && state.brush_training.dataset_identity.as_ref() == Some(&key),
        "historical dataset content binding changed",
    )?;
    for attempt in 0..=1 {
        require(
            std::fs::metadata(
                paths
                    .brush
                    .join(format!("collection-training-{attempt}/final.ply.tmp")),
            )?
            .len()
                == 0,
            "candidate unexpectedly contains data",
        )?;
    }
    let warm = paths
        .brush
        .join("dataset/init.warm-9b77a548-eecb-425c-a74b-b1397515e110.ply");
    require(
        fingerprint(&warm)? == source_ply.sha256,
        "leftover warm file not the frozen source model",
    )?;
    // EXIF orientation normalization produces an all-white mask even for an
    // opaque JPEG. Do not confuse Mask existence with Alpha/transparency.
    let mask_count = verify_opaque_rotation_masks(paths)?;
    state.frames.as_mut().unwrap().mask_count = Some(mask_count);
    state.reshoot.as_mut().unwrap().mask_count = mask_count;

    let source_id = snapshot
        .source_project_id
        .ok_or_else(|| SplatError::Process("Missing source project identity".into()))?;
    let source = registry["tasks"]
        .as_object()
        .unwrap()
        .values()
        .find(|t| t["project_id"] == source_id.to_string())
        .ok_or_else(|| SplatError::Process("Missing saved source provenance".into()))?;
    let source_path = PathBuf::from(
        source["project_path"]
            .as_str()
            .ok_or_else(|| SplatError::Process("Missing source project path".into()))?,
    );
    let metadata: ProjectMetadata = serde_json::from_value(json!({
        "schemaVersion": crate::project::metadata::schema_version(), "appId": crate::project::PROJECT_APP_ID,
        "id": PROJECT, "workspaceTaskId": TASK, "name": "20261114-133109_reshoot_8016f801",
        "createdAt": runs[0]["started_at"], "startedAt": runs[1]["started_at"], "completedAt": null,
        "durationMs": task_value["elapsed_ms"], "status": "failed", "sourcePath": paths.source,
        "inputType": "images", "quality": "fast", "projectPath": paths.project,
        "outputPath": null, "output": null, "failureMessage": task_value["error"]["message"], "model": "final.ply",
        "transform": context["transform"],
        "reshoot": {"sourceProjectId": source_id, "sourceProjectPath": source_path,
          "sourceFinalPly": source_path.join("final.ply"), "reshootSourcePath": task_value["input_path"],
          "cameraId": 1, "cameraModel": "SIMPLE_RADIAL", "width": snapshot.cameras[0].width,
          "height": snapshot.cameras[0].height, "hasAlpha": false, "maskCount": mask_count,
          "sourceImageCount": 17, "reshootFrameCount": 15, "registeredReshootCount": 15}
    }))?;
    // After the strict incident verification above, collect the same manifests
    // the common executor uses. These are newly verified, never historical.
    state.artifact_manifests = artifacts::capture_collection_artifacts(
        paths,
        &state,
        Some(uuid::Uuid::parse_str(RUN).unwrap()),
    )?;
    require(
        state.artifact_manifests.entries.len() == 4,
        "unexpected recovered artifact stages",
    )?;
    Ok((metadata, state))
}

#[tokio::test]
#[ignore = "operator-only recovery; reads frozen evidence and writes only the explicitly selected target"]
async fn recover_8016f801_without_starting_engines() -> Result<()> {
    let base = PathBuf::from(RECOVERY_ROOT);
    let mode =
        std::env::var("OOOSPLAT_RECOVERY_8016F801_MODE").unwrap_or_else(|_| "validate".into());
    require(
        matches!(mode.as_str(), "validate" | "install"),
        "unknown recovery mode",
    )?;
    let install = mode == "install";
    let isolated = base.join("isolated-project");
    let project = if install {
        PathBuf::from(ORIGINAL)
    } else {
        isolated
    };
    let paths = ProjectPaths::existing(uuid::Uuid::parse_str(PROJECT).unwrap(), project);
    let registry_path = base.join("tasks-before.json");
    require(
        fingerprint(&registry_path)? == TASKS_SHA,
        "frozen registry changed",
    )?;
    let registry = read_json(&registry_path)?;
    let previous = inventory(&paths.project)?;
    if install {
        let validated = read_json(&base.join("validation-passed.json"))?;
        require(
            validated["status"] == "validated" && validated["taskId"] == TASK,
            "isolation was not validated",
        )?;
        let expected: BTreeMap<String, String> =
            serde_json::from_value(read_json(&base.join("evidence-before.json"))?)?;
        require(
            previous == expected,
            "original project changed since isolation",
        )?;
        let live = Path::new(r"C:\Users\Administrator\AppData\Local\SplatStudio\tasks.json");
        require(
            fingerprint(live)? == TASKS_SHA,
            "live task registry changed; stop before installing",
        )?;
    }
    require(
        std::fs::metadata(&paths.state)?.len() == 0
            && std::fs::metadata(&paths.metadata)?.len() == 0,
        "record is no longer the known empty file",
    )?;
    verify_no_pending_barrier(&paths)?;
    let (metadata, mut state) = restore_records(&paths, &registry)?;
    let recipe = serde_json::to_value(&state.brush_training)?;
    let initial = serde_json::to_value(&state.reshoot)?;
    let before_state = serde_json::to_value(&state)?;
    let manager = ProjectManager::for_diagnostics(paths.project.parent().unwrap().into());
    let runner = PipelineRunner::new(
        EnginePaths::from_root(base.join("no-engines-allowed")),
        |_| {},
    )
    .with_execution_run_id(Some(uuid::Uuid::parse_str(RUN).unwrap()));
    require(
        !base.join("no-engines-allowed").try_exists()?,
        "engine-free validation sentinel is occupied",
    )?;
    // Capture and all source/model validation must succeed BEFORE any write.
    if !install {
        require(
            !base.join("evidence-before.json").try_exists()?,
            "previous recovery evidence exists; do not overwrite",
        )?;
        atomic_write_json(&base.join("evidence-before.json"), &previous).await?;
        atomic_write_json(
            &base.join("recovered-state-before-archive.json"),
            &before_state,
        )
        .await?;
    }
    let backup = base.join(if install {
        "original-empty-records"
    } else {
        "isolated-empty-records"
    });
    tokio::fs::create_dir(&backup).await?;
    tokio::fs::copy(&paths.state, backup.join("state.json")).await?;
    tokio::fs::copy(&paths.metadata, backup.join("project.json")).await?;
    // Existing barrier writer + archive loop owns every rename and saves each
    // completed move. Failure leaves pending work and all files recoverable.
    state.retry_cache_pending.push("brush".into());
    manager
        .write_state_for_preparation(&paths.state, &state)
        .await?;
    manager.write_metadata(&paths.metadata, &metadata).await?;
    runner
        .prepare_continuation(&manager, &paths, &mut state)
        .await?;
    require(
        !paths.brush.try_exists()? && state.retry_cache_pending.is_empty(),
        "training cache archive incomplete",
    )?;
    artifacts::normalize_collection_cancellable(
        &paths,
        &mut state,
        Some(uuid::Uuid::parse_str(RUN).unwrap()),
        &CancellationToken::new(),
    )
    .await?;
    require(
        state.features_complete
            && state.matching_complete
            && state.reconstruction_complete
            && !state.brush_complete,
        "shared normalizer rejected recovered reconstruction",
    )?;
    require(
        state
            .inputs
            .as_ref()
            .is_some_and(|i| i.prepared && i.reference_image.is_none()),
        "unexpected training readiness",
    )?;
    require(
        state.retry_cache_pending.is_empty() && state.database_recovery_pending.is_none(),
        "normalizer requested further invalidation",
    )?;
    require(
        serde_json::to_value(&state.brush_training)? == recipe
            && serde_json::to_value(&state.reshoot)? == initial,
        "normalization changed recipe or allowances",
    )?;
    require(
        work_order::next_action(
            &state,
            work_order::Readiness {
                reconstruction: true,
                ..Default::default()
            },
        ) == work_order::GenerationAction::PrepareTraining,
        "next action is not training preparation",
    )?;
    // Normalization selects the next executable stage, but this operation did
    // not start a run. Keep the persisted stop reason aligned with metadata and
    // the untouched task record until the user explicitly continues it.
    state = mark_state_terminal(state, false);
    manager
        .write_state_for_preparation(&paths.state, &state)
        .await?;
    let readback = manager.read_state(&paths.state).await?;
    require(
        serde_json::to_value(&readback)? == serde_json::to_value(&state)?,
        "state readback mismatch",
    )?;
    let after = inventory(&paths.project)?;
    for (name, digest) in &previous {
        if let Some(inside_brush) = name.strip_prefix("work/brush/") {
            let suffix = format!("/brush/{inside_brush}");
            let archived = after
                .iter()
                .filter(|(path, _)| {
                    path.starts_with("work/cache-archive/") && path.ends_with(&suffix)
                })
                .collect::<Vec<_>>();
            require(
                archived.len() == 1 && archived[0].1 == digest,
                "archived training evidence changed or missing",
            )?;
            continue;
        }
        if name == "state.json" || name == "project.json" || name == "logs/retry-preparation.log" {
            continue;
        }
        require(
            after.get(name) == Some(digest),
            "immutable project evidence changed",
        )?;
    }
    let report = json!({"status": if install {"installed"} else {"validated"}, "taskId": TASK, "projectId": PROJECT, "runId": RUN,
        "registrySha256": TASKS_SHA, "nextAction": "prepareTraining", "engineStarts": 0,
        "oldImages": 17, "newImages": 15, "registeredImages": 32, "points3d": 7424,
        "trainingFallbacks": 1, "reconstructionFallbacks": 0, "oomRetryUsed": false,
        "elapsedMs": 7_625_096u64, "recordsReconstructedFrom": ["frozen task request and creation intent", "closed Brush execution configuration", "verified immutable snapshot", "historical content-bound dataset key", "validated SQLite/model observations"],
        "manifests": "newly captured after incident validation; not historical", "metadataCreatedAt": "first recorded run start; exact lost metadata timestamp unavailable",
        "registryWritten": false, "trainingResumed": false, "originalsAccessed": false,
        "preservedFiles": previous.len(), "note": "No optimizer/training checkpoint exists; next continue uses saved cold recipe from step zero."});
    atomic_write_json(
        &base.join(if install {
            "installation-passed.json"
        } else {
            "validation-passed.json"
        }),
        &report,
    )
    .await?;
    println!("Recovery {mode} passed: 32 registered images, cached SfM retained, next=PrepareTraining, engine starts=0");
    Ok(())
}

/// Finish only the terminal-state alignment of the two already validated
/// incident targets. Never repeats snapshot capture, archive, or normalization.
#[tokio::test]
#[ignore = "operator-only finalization of the validated and installed incident recovery"]
async fn finalize_8016f801_failed_state_without_starting_engines() -> Result<()> {
    let base = PathBuf::from(RECOVERY_ROOT);
    let registry = Path::new(r"C:\Users\Administrator\AppData\Local\SplatStudio\tasks.json");
    require(fingerprint(registry)? == TASKS_SHA, "live registry changed")?;
    let frozen = read_json(&base.join("tasks-before.json"))?;
    require(
        fingerprint(&base.join("tasks-before.json"))? == TASKS_SHA,
        "frozen registry changed",
    )?;
    let actual = &frozen["tasks"][TASK]["actual_configuration"];
    let historical_brush: crate::project::metadata::BrushTrainingCheckpoint =
        serde_json::from_value(actual["brush_training"].clone())?;
    let historical_recipe: crate::pipeline::reshoot::ReshootRecipe =
        serde_json::from_value(actual["reshoot_recipe"].clone())?;
    let mut targets = Vec::new();
    for (label, directory, report_file, report_status) in [
        (
            "isolated",
            base.join("isolated-project"),
            "validation-passed.json",
            "validated",
        ),
        (
            "original",
            PathBuf::from(ORIGINAL),
            "installation-passed.json",
            "installed",
        ),
    ] {
        let report = read_json(&base.join(report_file))?;
        require(
            report["status"] == report_status
                && report["taskId"] == TASK
                && report["projectId"] == PROJECT
                && report["runId"] == RUN
                && report["registrySha256"] == TASKS_SHA
                && report["engineStarts"] == 0,
            "completed recovery report is missing or mismatched",
        )?;
        let paths = ProjectPaths::existing(uuid::Uuid::parse_str(PROJECT).unwrap(), directory);
        let metadata = read_json(&paths.metadata)?;
        require(
            metadata["id"] == PROJECT
                && metadata["workspaceTaskId"] == TASK
                && metadata["status"] == "failed"
                && metadata["durationMs"] == 7_625_096u64,
            "metadata is no longer the recovered failed project",
        )?;
        let saved = read_json(&paths.state)?;
        let state: PipelineStateFile = serde_json::from_value(saved.clone())?;
        require(
            serde_json::to_vec_pretty(&state)? == std::fs::read(&paths.state)?,
            "state differs from the recovery writer's canonical bytes",
        )?;
        require(
            matches!(
                state.stage,
                PipelineStage::TrainingSplats | PipelineStage::Failed
            ) && state.features_complete
                && state.matching_complete
                && state.reconstruction_complete
                && !state.brush_complete
                && state.validated_output.is_none()
                && state.retry_cache_pending.is_empty()
                && state.database_recovery_pending.is_none(),
            "recovered readiness or stop state changed",
        )?;
        require(
            state.artifact_manifests.entries.len() == 4
                && !paths.brush.try_exists()?
                && !paths.project.join("final.ply").try_exists()?,
            "new training/publication or incomplete archive detected",
        )?;
        let reshoot = state
            .reshoot
            .as_ref()
            .ok_or_else(|| SplatError::Process("Missing recovered reshoot".into()))?;
        require(
            serde_json::to_value(&state.brush_training)?
                == serde_json::to_value(&historical_brush)?
                && serde_json::to_value(&reshoot.recipe)?
                    == serde_json::to_value(&historical_recipe)?
                && reshoot.source_image_count == 17
                && reshoot.reshoot_frame_count == 15
                && reshoot.registered_reshoot_count == 15
                && reshoot.mask_count == 32
                && reshoot
                    .connection_proof
                    .as_ref()
                    .is_some_and(|p| p.old_registered == 17 && p.new_registered == 15),
            "recipe, allowances, or recovered counts changed",
        )?;
        let mut expected = serde_json::to_value(&state)?;
        let terminal = mark_state_terminal(state, false);
        expected["stage"] = json!("failed");
        require(
            serde_json::to_value(&terminal)? == expected,
            "terminal update changed more than stage",
        )?;
        targets.push((
            label,
            paths,
            saved,
            terminal,
            fingerprint(&base.join(report_file))?,
        ));
    }
    // Preflight both targets before touching either. The only project write
    // below is state.json; keep its previous bytes as a local recovery record.
    for (label, paths, saved, terminal, _) in &targets {
        require(
            fingerprint(registry)? == TASKS_SHA,
            "registry changed before finalization",
        )?;
        require(
            read_json(&paths.state)? == *saved,
            "state changed before finalization",
        )?;
        let metadata_sha = fingerprint(&paths.metadata)?;
        let backup = base.join(format!("finalization-{label}-state-before.json"));
        if backup.try_exists()? {
            let old: PipelineStateFile = serde_json::from_value(read_json(&backup)?)?;
            require(
                serde_json::to_value(mark_state_terminal(old, false))?
                    == serde_json::to_value(terminal)?,
                "existing finalization evidence does not belong to this state",
            )?;
        } else {
            atomic_write_json(&backup, saved).await?;
        }
        let manager = ProjectManager::for_diagnostics(paths.project.parent().unwrap().into());
        manager
            .write_state_for_preparation(&paths.state, terminal)
            .await?;
        require(
            serde_json::to_value(manager.read_state(&paths.state).await?)?
                == serde_json::to_value(terminal)?
                && fingerprint(&paths.metadata)? == metadata_sha,
            "terminal readback/metadata mismatch",
        )?;
    }
    require(
        fingerprint(registry)? == TASKS_SHA,
        "registry changed during finalization",
    )?;
    atomic_write_json(
        &base.join("terminal-state-aligned.json"),
        &json!({
            "taskId": TASK, "projectId": PROJECT, "runId": RUN, "registrySha256": TASKS_SHA,
            "state": "failed", "metadata": "failed", "task": "failed", "engineStarts": 0,
            "onlyChangedField": "state.stage", "targets": ["isolated", "original"],
            "validationReportSha256": targets[0].4, "installationReportSha256": targets[1].4,
        }),
    )
    .await?;
    println!("Recovery terminal states aligned: both state.stage=failed; recipes, counters and task registry unchanged; engine starts=0");
    Ok(())
}
