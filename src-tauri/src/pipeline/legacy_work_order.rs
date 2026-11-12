//! Narrow adapters from already-reconstructed single-input work into the shared
//! generation executor. This is not a second training implementation.
//!
//! An old recipe is adopted only when its policy, working pixels, feature rows,
//! camera IDs and final poses can be proved. A complete saved actual recipe
//! does not require a newer GenerationPolicy; missing evidence never permits
//! substituting today's defaults.

use super::*;
use crate::pipeline::{
    execution_plan::{ExecutionPlan, SnapshotModel},
    inputs::{
        GenerationInputs, InputCheckpoint, InputSource, MaterialInput, SourceAdoption,
        SourceCheckpoint,
    },
    reshoot::{PanoramaFrameRecord, TrainingMode},
};
use crate::reconstruction::direct_input::{self, Model};
use crate::video::panorama::VideoProjection;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub(super) const UNRESTORABLE_RECIPE: &str =
    "旧任务的历史参数或初始化数据不完整，无法安全继续。原项目和已有结果保持不变。";

fn unrestorable() -> SplatError {
    SplatError::Process(UNRESTORABLE_RECIPE.into())
}

struct InitializationCheck(PathBuf);
impl Drop for InitializationCheck {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Validate retained training pixels and poses without deriving a new dataset.
/// In particular, old undistorted inputs must not be replaced by today's native
/// distortion path merely to make their task fit a modern checkpoint shape.
fn saved_dataset_identity(
    dataset: &Path,
    authority: &Path,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<String> {
    let model_root = dataset.join("sparse/0");
    let model = direct_input::read_model(&model_root)?;
    direct_input::inspect(&crate::pipeline::inputs::ColmapInput {
        model: model_root.clone(),
        images: dataset.join("images"),
        database: None,
        identity: None,
    })?;
    let original = direct_input::read_model(authority)?;
    // ERP derivations need their explicit historical face/source mapping. The
    // ordinary compatibility path never guesses the names of panorama faces.
    let manifest = if original.cameras.iter().any(|camera| camera.model_id == 17) {
        Some(serde_json::from_slice::<serde_json::Value>(
            &std::fs::read(dataset.join("manifest.json"))?,
        )?)
    } else {
        None
    };
    let mut hash = Sha256::new();
    hash.update(b"saved-legacy-training-pixels-v1");
    for file in reusable_model_files(authority)? {
        hash.update(hash_file(&file, cancel)?);
    }
    for file in reusable_model_files(&model_root)? {
        hash.update(hash_file(&file, cancel)?);
    }
    let mut seen = BTreeSet::new();
    for image in &model.images {
        if !seen.insert(image.pose.name.clone()) {
            return Err(unrestorable());
        }
        if let Some(source) = original
            .images
            .iter()
            .find(|source| source.pose.name == image.pose.name)
        {
            if !same_pose(&source.pose, &image.pose) {
                return Err(unrestorable());
            }
        } else {
            let row = manifest
                .as_ref()
                .and_then(|v| v.get("faces"))
                .and_then(|v| v.as_array())
                .and_then(|rows| {
                    rows.iter().find(|row| {
                        row.get("image_name").and_then(|v| v.as_str())
                            == Some(image.pose.name.as_str())
                    })
                })
                .ok_or_else(unrestorable)?;
            let name = row
                .get("erp_name")
                .and_then(|v| v.as_str())
                .ok_or_else(unrestorable)?;
            let source = original
                .images
                .iter()
                .find(|source| source.pose.name == name)
                .ok_or_else(unrestorable)?;
            let face = row
                .get("face")
                .and_then(|v| v.as_str())
                .ok_or_else(unrestorable)?;
            let rotation = crate::video::panorama::FACES
                .iter()
                .find(|(name, _)| *name == face)
                .map(|(_, q)| *q)
                .ok_or_else(unrestorable)?;
            let expected_r = crate::reconstruction::gps::rotation(source.pose.qvec);
            let actual_r = crate::reconstruction::gps::rotation(image.pose.qvec);
            let expected_t = crate::reconstruction::gps::mul(rotation, source.pose.tvec);
            if (0..3).any(|i| {
                (0..3).any(|j| {
                    let expected = (0..3)
                        .map(|k| rotation[i][k] * expected_r[k][j])
                        .sum::<f64>();
                    (expected - actual_r[i][j]).abs() > 1e-7
                })
            }) || expected_t
                .iter()
                .zip(image.pose.tvec)
                .any(|(a, b)| (a - b).abs() > 1e-7 * (1. + a.abs()))
            {
                return Err(unrestorable());
            }
        }
        let path = direct_input::image_path(&dataset.join("images"), &image.pose.name)?;
        hash.update(image.pose.name.as_bytes());
        hash.update(hash_file(&path, cancel)?);
    }
    if model.images.is_empty()
        || original.images.iter().any(|source| {
            !model
                .images
                .iter()
                .any(|image| image.pose.name == source.pose.name)
                && !manifest
                    .as_ref()
                    .and_then(|v| v.get("faces"))
                    .and_then(|v| v.as_array())
                    .is_some_and(|rows| {
                        rows.iter().any(|r| {
                            r.get("erp_name").and_then(|v| v.as_str())
                                == Some(source.pose.name.as_str())
                        })
                    })
        })
    {
        return Err(unrestorable());
    }
    for point in &model.points {
        let source = original
            .points
            .iter()
            .find(|source| source.id == point.id)
            .ok_or_else(unrestorable)?;
        if source
            .xyz
            .iter()
            .zip(point.xyz)
            .any(|(a, b)| (a - b).abs() > 1e-8 * (1. + a.abs()))
        {
            return Err(unrestorable());
        }
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn same_pose(
    a: &crate::video::panorama::PosedImage,
    b: &crate::video::panorama::PosedImage,
) -> bool {
    let dot = a.qvec.iter().zip(b.qvec).map(|(a, b)| a * b).sum::<f64>();
    (1. - dot.abs()).abs() < 1e-8
        && a.tvec
            .iter()
            .zip(b.tvec)
            .all(|(a, b)| (a - b).abs() < 1e-7 * (1. + a.abs()))
}

fn unsupported_recipe(state: &PipelineStateFile, is_reshoot: bool) -> bool {
    if state.inputs.is_some() || state.brush_complete || state.generation_policy.is_none() {
        return true;
    }
    if !is_reshoot {
        return state.reshoot.is_some();
    }
    let Some(checkpoint) = state.reshoot.as_ref() else {
        return true;
    };
    let Some(recipe) = checkpoint.recipe.as_ref() else {
        return true;
    };
    !recipe.uses_normal_parameters()
        || recipe.warm_parameters.is_some()
        // V2 used a GPS-transform JSON value here; V3 uses a frozen coordinate
        // identity. Do not relabel one as the other or silently replace it.
        || recipe.init_georeference.is_some()
        || recipe.init_sha256.is_some()
        || !checkpoint.source_snapshot_complete
        || !checkpoint.supplemental_frames_complete
        || !checkpoint.supplemental_features_complete
        || !checkpoint.incremental_matching_complete
        || !checkpoint.incremental_reconstruction_complete
}

fn invalid_working_set() -> SplatError {
    SplatError::Process(
        "Saved legacy reconstruction cannot be safely adopted by the shared executor".into(),
    )
}

fn hash_file(path: &Path, cancel: &tokio_util::sync::CancellationToken) -> Result<String> {
    let mut input = std::fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut bytes = vec![0u8; 256 * 1024];
    loop {
        if cancel.is_cancelled() {
            return Err(SplatError::Cancelled);
        }
        let count = std::io::Read::read(&mut input, &mut bytes)?;
        if count == 0 {
            return Ok(format!("{:x}", hash.finalize()));
        }
        hash.update(&bytes[..count]);
    }
}

fn verify_frozen_source_pixels(
    paths: &ProjectPaths,
    names: &[String],
    snapshot: &crate::pipeline::reshoot_snapshot::ReshootSnapshotV3,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<()> {
    let frozen_names = snapshot
        .assets
        .iter()
        .filter_map(|asset| asset.relative_path.strip_prefix("frames/"))
        .collect::<BTreeSet<_>>();
    if frozen_names != names.iter().map(String::as_str).collect() {
        return Err(invalid_working_set());
    }
    for name in names {
        for role in ["frames", "colmap_frames"] {
            let relative = format!("{role}/{name}");
            let asset = snapshot
                .assets
                .iter()
                .find(|asset| asset.relative_path == relative)
                .ok_or_else(invalid_working_set)?;
            if hash_file(&paths.work.join(&relative), cancel)? != asset.sha256 {
                return Err(invalid_working_set());
            }
        }
        let relative = format!("masks/{name}.png");
        if let Some(asset) = snapshot
            .assets
            .iter()
            .find(|asset| asset.relative_path == relative)
        {
            if hash_file(&paths.work.join(relative), cancel)? != asset.sha256 {
                return Err(invalid_working_set());
            }
        }
    }
    Ok(())
}

/// Prove actual input pixels and database coordinates, rather than trusting
/// flags from the old runner. Image and camera IDs are never renumbered here.
fn validate_working_set(
    paths: &ProjectPaths,
    model: &Model,
    names: &[String],
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<BTreeMap<String, (u32, [u32; 2])>> {
    let expected = names.iter().cloned().collect::<BTreeSet<_>>();
    if names.is_empty() || expected.len() != names.len() || model.images.is_empty() {
        return Err(invalid_working_set());
    }
    let database = direct_input::open_immutable_database(&paths.colmap.join("database.db"))?;
    let integrity: String = database
        .query_row("PRAGMA integrity_check(1)", [], |row| row.get(0))
        .map_err(direct_input::database_read_error)?;
    if integrity != "ok" {
        return Err(invalid_working_set());
    }
    let mut rows = database
        .prepare("SELECT image_id,name,camera_id FROM images ORDER BY image_id")
        .map_err(direct_input::database_read_error)?;
    let images = rows
        .query_map([], |row| {
            Ok((
                row.get::<_, u32>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, u32>(2)?,
            ))
        })
        .map_err(direct_input::database_read_error)?;
    let mut records = BTreeMap::new();
    let mut ids = BTreeSet::new();
    for image in images {
        if cancel.is_cancelled() {
            return Err(SplatError::Cancelled);
        }
        let (id, name, camera_id) = image.map_err(direct_input::database_read_error)?;
        if id == 0 || !ids.insert(id) || !expected.contains(&name) {
            return Err(invalid_working_set());
        }
        let frame = direct_input::image_path(&paths.frames, &name)?;
        let rgb = direct_input::image_path(&paths.colmap_frames, &name)?;
        let header = crate::video::image_projection::inspect(&frame)?;
        let rgb_header = crate::video::image_projection::inspect(&rgb)?;
        if (header.width, header.height) != (rgb_header.width, rgb_header.height)
            || header.orientation != 1
            || rgb_header.orientation != 1
        {
            return Err(invalid_working_set());
        }
        let geometry: (i32, u64, u64) = database
            .query_row(
                "SELECT model,width,height FROM cameras WHERE camera_id=?1",
                [camera_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(direct_input::database_read_error)?;
        if (geometry.1, geometry.2) != (u64::from(header.width), u64::from(header.height)) {
            return Err(invalid_working_set());
        }
        if let Some(camera) = model.cameras.iter().find(|camera| camera.id == camera_id) {
            if geometry != (camera.model_id, camera.width, camera.height) {
                return Err(invalid_working_set());
            }
        }
        let mask = paths.masks.join(format!("{name}.png"));
        if mask.is_file()
            && image::image_dimensions(&mask)
                .map_err(|error| SplatError::Process(error.to_string()))?
                != (header.width, header.height)
        {
            return Err(invalid_working_set());
        }
        if records
            .insert(name, (id, camera_id, [header.width, header.height]))
            .is_some()
        {
            return Err(invalid_working_set());
        }
    }
    if records.len() != expected.len() {
        return Err(invalid_working_set());
    }
    for image in &model.images {
        if cancel.is_cancelled() {
            return Err(SplatError::Cancelled);
        }
        let Some((id, camera, _)) = records.get(&image.pose.name) else {
            return Err(invalid_working_set());
        };
        if (*id, *camera) != (image.pose.id, image.pose.camera_id) {
            return Err(invalid_working_set());
        }
        let (count, columns, data): (usize, usize, Option<Vec<u8>>) = database
            .query_row(
                "SELECT rows,cols,data FROM keypoints WHERE image_id=?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(direct_input::database_read_error)?;
        let data = data.unwrap_or_default();
        let expected_bytes = count.checked_mul(columns).and_then(|n| n.checked_mul(4));
        if count != image.observations.len()
            || !matches!(columns, 2 | 4 | 6)
            || expected_bytes != Some(data.len())
        {
            return Err(invalid_working_set());
        }
        for (observation, bytes) in image
            .observations
            .iter()
            .zip(data.chunks_exact(columns * 4))
        {
            let x = f64::from(f32::from_le_bytes(bytes[..4].try_into().unwrap()));
            let y = f64::from(f32::from_le_bytes(bytes[4..8].try_into().unwrap()));
            if !x.is_finite()
                || !y.is_finite()
                || (x - observation.x).abs() > 0.001
                || (y - observation.y).abs() > 0.001
            {
                return Err(invalid_working_set());
            }
        }
    }
    Ok(records
        .into_iter()
        .map(|(name, (_, camera, dimensions))| (name, (camera, dimensions)))
        .collect())
}

fn source_checkpoint(
    id: &str,
    identity: String,
    names: Vec<String>,
    records: &BTreeMap<String, (u32, [u32; 2])>,
    model: &Model,
    panorama_records: &[PanoramaFrameRecord],
) -> SourceCheckpoint {
    let camera_ids = names
        .iter()
        .filter_map(|name| records.get(name).map(|entry| entry.0))
        .collect::<BTreeSet<_>>();
    let erp = camera_ids
        .iter()
        .filter_map(|id| model.cameras.iter().find(|camera| camera.id == *id))
        .filter(|camera| camera.model_id == 17)
        .count();
    let projection = if erp == 0 {
        VideoProjection::Perspective
    } else if erp == camera_ids.len() {
        VideoProjection::Equirectangular
    } else {
        VideoProjection::Auto
    };
    SourceCheckpoint {
        id: id.into(),
        identity,
        ordinary_image_preparation_version: None,
        camera_ids: camera_ids.into_iter().collect(),
        camera_assignment: None,
        projection,
        originals: vec![],
        video: None,
        image_sequence: None,
        dimensions: names
            .iter()
            .filter_map(|name| records.get(name).map(|entry| entry.1))
            .collect(),
        working_dimensions: None,
        frames: None,
        panorama_records: panorama_records
            .iter()
            .filter(|record| names.contains(&record.prepared_name))
            .cloned()
            .collect(),
        names,
        adopted: 0,
        bridge_frames: vec![],
    }
}

impl PipelineRunner {
    /// A retained dataset plus a saved actual recipe is sufficient evidence
    /// even when the task predates GenerationPolicy. This adapter owns no
    /// training algorithm or publication implementation.
    #[allow(clippy::too_many_arguments)]
    async fn saved_legacy_training_tail(
        &self,
        manager: &ProjectManager,
        paths: &ProjectPaths,
        metadata: &mut ProjectMetadata,
        mut state: PipelineStateFile,
        model: &Path,
        acceleration: &crate::engines::ColmapAccelerationStatus,
    ) -> Result<Option<PipelineResult>> {
        if state.inputs.is_some() || state.brush_complete {
            return Ok(None);
        }
        let Some(resolved) = state.brush_training.resolved else {
            return Ok(None);
        };
        let recipe = state
            .reshoot
            .as_ref()
            .and_then(|r| r.recipe.as_ref())
            .cloned();
        if metadata.reshoot.is_some() && recipe.is_none() {
            return Err(unrestorable());
        }
        if recipe.as_ref().is_some_and(|r| {
            !r.uses_normal_parameters()
                && r.actual.training == TrainingMode::Warm
                && r.warm_parameters.is_none()
        }) {
            return Err(unrestorable());
        }
        let authority = direct_input::read_model(model)?;
        self.record_reshoot_connection_model(manager, paths, &mut state, model)
            .await?;
        let is_erp = authority.cameras.iter().any(|camera| camera.model_id == 17);
        let dataset = if is_erp {
            paths.work.join("panorama-training")
        } else {
            paths.brush.join("dataset")
        };
        if !dataset.is_dir() {
            return Err(unrestorable());
        }
        let owned_dataset = dataset.clone();
        let owned_model = model.to_path_buf();
        let cancel = self.process_manager.child_token();
        let identity = crate::presets::spawn_pipeline_blocking(move || {
            saved_dataset_identity(&owned_dataset, &owned_model, &cancel)
        })
        .await
        .map_err(|e| SplatError::Process(e.to_string()))??;
        if state
            .brush_training
            .legacy_dataset_identity
            .as_ref()
            .is_some_and(|saved| saved != &identity)
        {
            return Err(unrestorable());
        }
        let warm = recipe
            .as_ref()
            .is_some_and(|r| r.actual.training == TrainingMode::Warm);
        let initial = if warm {
            let recipe = recipe.as_ref().unwrap();
            let init = dataset.join("init.ply");
            let frozen = paths.work.join("legacy-training-init.ply");
            if let Some(expected) = state.brush_training.legacy_initialization_sha256.as_ref() {
                let (_, _, hash) = super::reshoot_runner::validate_ply(
                    &frozen,
                    &self.process_manager.child_token(),
                )
                .await?;
                if hash != *expected {
                    return Err(unrestorable());
                }
                if !init.exists() {
                    super::reshoot_runner::copy_reshoot_file(
                        &frozen,
                        &init,
                        &self.process_manager.child_token(),
                    )
                    .await?;
                }
            }
            let (info, sh, hash) =
                super::reshoot_runner::validate_ply(&init, &self.process_manager.child_token())
                    .await?;
            if recipe
                .warm_parameters
                .is_some_and(|warm| warm.sh_degree != sh)
            {
                return Err(unrestorable());
            }
            if let Some(expected) = state.brush_training.legacy_initialization_sha256.as_ref() {
                if &hash != expected {
                    return Err(unrestorable());
                }
            } else {
                // Old initSha256 is the ORIGINAL Gaussian hash, not the
                // transformed init's hash. Treat the old GPS JSON as JSON.
                let transform = state
                    .gps
                    .as_ref()
                    .filter(|g| g.status == "complete")
                    .and_then(|g| g.transform);
                let key = transform.map(serde_json::to_value).transpose()?;
                let saved_key = recipe
                    .init_georeference
                    .as_deref()
                    .map(serde_json::from_str::<serde_json::Value>)
                    .transpose()
                    .map_err(|_| unrestorable())?;
                if key != saved_key {
                    return Err(unrestorable());
                }
                if let Some(transform) = transform {
                    let source = metadata
                        .reshoot
                        .as_ref()
                        .ok_or_else(unrestorable)?
                        .source_final_ply
                        .clone();
                    self.authorize_external_path(&source)?;
                    let (_, _, original_hash) = super::reshoot_runner::validate_ply(
                        &source,
                        &self.process_manager.child_token(),
                    )
                    .await?;
                    if recipe.init_sha256.as_deref() != Some(original_hash.as_str()) {
                        return Err(unrestorable());
                    }
                    let check = InitializationCheck(
                        paths
                            .work
                            .join(format!(".legacy-init-check-{}.ply", uuid::Uuid::new_v4())),
                    );
                    std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&check.0)?;
                    let output = check.0.clone();
                    let token = self.process_manager.child_token();
                    crate::presets::spawn_pipeline_blocking(move || {
                        crate::reconstruction::splat_transform::export_transformed_ply_to(
                            &source,
                            &output,
                            transform.engine_transform(),
                            Default::default(),
                            |_, _| {},
                            || {
                                if token.is_cancelled() {
                                    Err(SplatError::Cancelled)
                                } else {
                                    Ok(())
                                }
                            },
                        )
                    })
                    .await
                    .map_err(|e| SplatError::Process(e.to_string()))??;
                    let (_, _, verified) = super::reshoot_runner::validate_ply(
                        &check.0,
                        &self.process_manager.child_token(),
                    )
                    .await?;
                    if verified != hash {
                        return Err(unrestorable());
                    }
                } else if recipe.init_sha256.as_deref() != Some(hash.as_str()) {
                    return Err(unrestorable());
                }
                if frozen.exists() {
                    if hash_file(&frozen, &self.process_manager.child_token())? != hash {
                        return Err(unrestorable());
                    }
                } else {
                    super::reshoot_runner::copy_reshoot_file(
                        &init,
                        &frozen,
                        &self.process_manager.child_token(),
                    )
                    .await?;
                    if hash_file(&frozen, &self.process_manager.child_token())? != hash {
                        return Err(unrestorable());
                    }
                }
                state.brush_training.legacy_initialization_sha256 = Some(hash);
            }
            info.splat_count
        } else {
            if dataset.join("init.ply").exists() {
                return Err(unrestorable());
            }
            authority.points.len() as u64
        };
        if resolved
            .preset
            .max_splats
            .is_some_and(|limit| u64::from(limit) < initial)
        {
            return Err(unrestorable());
        }
        state.brush_training.legacy_dataset_identity = Some(identity);
        // Freeze the exact replay evidence before spawning Brush. No profile,
        // warm multiplier, coordinate transform or retry count is recalculated.
        manager.write_state(&paths.state, &state).await?;
        let mut attempt = paths.clone();
        if let Some(recipe) = &recipe {
            attempt.brush = paths.brush.join(format!(
                "attempt-{}-{}",
                recipe.reconstruction_fallbacks, recipe.training_fallbacks
            ));
        }
        self.events.stage(
            PipelineStage::TrainingSplats,
            0.,
            "训练图片准备完成，即将启动训练",
        );
        self.events.training_substage("training");
        let candidate = self
            .train_brush_with_oom_retry(
                manager,
                &attempt,
                &mut state,
                metadata.quality,
                acceleration,
                &dataset,
                initial,
                resolved,
                0,
            )
            .await?;
        super::reshoot_runner::validate_ply(&candidate, &self.process_manager.child_token())
            .await?;
        let report = ReconstructionValidator::validate_with_input_images(
            state
                .frames
                .as_ref()
                .and_then(|f| f.extracted_frames)
                .ok_or_else(unrestorable)?,
            model,
        )?;
        let ply = inspect_gaussian_ply(&candidate)?;
        let output = ProjectOutput {
            final_ply: paths.project.join("final.ply"),
            file_size: ply.file_size,
            splat_count: ply.splat_count,
            input_images: report.input_images,
            registered_images: report.registered_images,
            registered_ratio: report.registered_ratio,
            points_3d: report.points_3d,
        };
        state.brush_complete = true;
        state.validated_output = Some(output.clone());
        crate::pipeline::artifacts::record_saved_legacy_candidate(
            paths,
            &mut state,
            &candidate,
            self.execution_run_id,
            &self.process_manager.child_token(),
        )
        .await?;
        manager.write_state(&paths.state, &state).await?;
        self.publish_work_order(
            manager,
            paths,
            metadata,
            &mut state,
            &candidate,
            output,
            "legacy-saved-training".into(),
            None,
        )
        .await
        .map(Some)
    }

    /// `None` is a deliberate compatibility refusal. The caller must stop or
    /// retain an explicitly separate compatibility policy, never inject v6.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn handoff_legacy_training(
        &self,
        manager: &ProjectManager,
        paths: &ProjectPaths,
        metadata: &mut ProjectMetadata,
        state: PipelineStateFile,
        model_path: &Path,
        acceleration: &crate::engines::ColmapAccelerationStatus,
    ) -> Result<Option<PipelineResult>> {
        if state.brush_training.resolved.is_some()
            && state.inputs.is_none()
            && !state.brush_complete
        {
            return self
                .saved_legacy_training_tail(
                    manager,
                    paths,
                    metadata,
                    state,
                    model_path,
                    acceleration,
                )
                .await;
        }
        let reshoot = metadata.reshoot.is_some();
        if unsupported_recipe(&state, reshoot) {
            return Ok(None);
        }
        state.generation_policy.unwrap().validate()?;
        if !state.retry_cache_pending.is_empty() {
            return Err(invalid_working_set());
        }
        let recipe = state
            .reshoot
            .as_ref()
            .and_then(|checkpoint| checkpoint.recipe.clone());
        let names = if let Some(recipe) = &recipe {
            recipe
                .source_names
                .iter()
                .chain(&recipe.new_names)
                .cloned()
                .collect::<Vec<_>>()
        } else {
            crate::video::list_images(&paths.frames)?
                .into_iter()
                .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
                .collect()
        };
        let input_count = state
            .frames
            .as_ref()
            .and_then(|frames| frames.extracted_frames);
        if input_count != Some(names.len() as u64) {
            return Err(invalid_working_set());
        }
        let owned_paths = paths.clone();
        let owned_model = model_path.to_path_buf();
        let owned_names = names.clone();
        let cancel = self.process_manager.child_token();
        let (model, records, report) = crate::presets::spawn_pipeline_blocking(move || {
            let model = direct_input::read_model(&owned_model)?;
            let records = validate_working_set(&owned_paths, &model, &owned_names, &cancel)?;
            let report = ReconstructionValidator::validate_with_input_images(
                owned_names.len() as u64,
                &owned_model,
            )?;
            Ok::<_, SplatError>((model, records, report))
        })
        .await
        .map_err(|error| SplatError::Process(error.to_string()))??;
        let input_source = match metadata.input_type {
            ProjectInputType::Video => InputSource::Video {
                path: metadata.source_path.clone(),
            },
            ProjectInputType::Images => InputSource::ImageFolder {
                path: metadata.source_path.clone(),
            },
        };
        let input_identity = input_source.identity()?;
        let mut working = state.clone();
        let mut working_metadata = metadata.clone();
        let mut plan = ExecutionPlan::normal();
        let mut sources = Vec::new();
        if let Some(recipe) = recipe.as_ref() {
            if recipe.source_names.is_empty()
                || recipe.new_names.is_empty()
                || !model
                    .images
                    .iter()
                    .any(|image| recipe.new_names.contains(&image.pose.name))
            {
                return Err(invalid_working_set());
            }
            self.ensure_reshoot_snapshot_v3(paths, &mut working_metadata, &mut working)
                .await?;
            let root = paths.work.join("reshoot-snapshot-v3");
            let manifest = crate::pipeline::reshoot_snapshot::verify_snapshot(&root)?;
            let owned_paths = paths.clone();
            let source_names = recipe.source_names.clone();
            let owned_manifest = manifest.clone();
            let cancel = self.process_manager.child_token();
            crate::presets::spawn_pipeline_blocking(move || {
                verify_frozen_source_pixels(&owned_paths, &source_names, &owned_manifest, &cancel)
            })
            .await
            .map_err(|error| SplatError::Process(error.to_string()))??;
            let frozen_model = direct_input::read_model(&root.join("colmap/sparse"))?;
            if recipe.actual.training == TrainingMode::Warm && manifest.source_ply.is_none() {
                return Err(invalid_working_set());
            }
            let saved = working
                .reshoot
                .as_ref()
                .and_then(|checkpoint| checkpoint.recipe.as_ref())
                .ok_or_else(invalid_working_set)?;
            plan = ExecutionPlan::reshoot(
                saved,
                SnapshotModel {
                    relative_path: "reshoot-snapshot-v3/colmap/sparse".into(),
                    sha256: manifest.content_identity.clone(),
                    coordinate_frame_id: manifest.coordinate_frame_id.clone(),
                    has_erp: manifest.has_erp,
                    fixed_images: frozen_model
                        .images
                        .iter()
                        .map(|image| image.pose.id)
                        .collect(),
                    fixed_cameras: frozen_model
                        .cameras
                        .iter()
                        .map(|camera| camera.id)
                        .collect(),
                },
            );
            let base_database = paths.colmap.join("base-database.db");
            if !base_database.exists() {
                tokio::fs::copy(root.join("colmap/database.db"), &base_database).await?;
            }
            sources.push(source_checkpoint(
                "__source",
                recipe.options.source_identity.clone(),
                recipe.source_names.clone(),
                &records,
                &model,
                &recipe.panorama_frames,
            ));
        }
        // These operations were not part of the saved single-input recipe.
        // A migration must not grant another bridge or candidate expansion.
        plan.bridge.enabled = false;
        plan.bridge.max_joint_rounds = 0;
        let item_id = "legacy-input";
        let new_names = recipe
            .as_ref()
            .map_or_else(|| names.clone(), |recipe| recipe.new_names.clone());
        let mut source = source_checkpoint(
            item_id,
            input_identity.clone(),
            new_names,
            &records,
            &model,
            recipe
                .as_ref()
                .map_or(&[][..], |recipe| recipe.panorama_frames.as_slice()),
        );
        source.video = state.video.clone();
        source.image_sequence = state.image_sequence.clone();
        if metadata.input_type == ProjectInputType::Images
            && source.projection == VideoProjection::Perspective
            && state.panorama.is_none()
            && state.video_projection == VideoProjection::Perspective
        {
            // Only the newly prepared ordinary-photo batch uses this task's
            // policy. Frozen historical source batches retain their own one.
            source.ordinary_image_preparation_version = state.ordinary_image_preparation_version;
        }
        if !reshoot {
            source.frames = state.frames.clone();
        }
        let projection = source.projection;
        sources.push(source);
        let item = MaterialInput {
            id: item_id.into(),
            source: input_source,
            identity: Some(input_identity),
            projection,
            image_matching_strategy: recipe
                .as_ref()
                .map_or(state.image_matching_strategy, |recipe| {
                    recipe.options.image_matching_strategy
                }),
            camera_relation: recipe
                .as_ref()
                .and_then(|recipe| recipe.options.camera_relation),
            confirmed_small_images: true,
            confirmed_image_order: true,
            frame_removal_fps: None,
        };
        let mut checkpoint =
            InputCheckpoint::new(GenerationInputs::Materials { items: vec![item] });
        checkpoint.camera_grouping_policy_version = working.camera_grouping_policy_version;
        checkpoint.ordinary_image_preparation_version = working.ordinary_image_preparation_version;
        checkpoint.sources = sources;
        let registered = model
            .images
            .iter()
            .map(|image| image.pose.name.as_str())
            .collect::<BTreeSet<_>>();
        for source in &mut checkpoint.sources {
            source.adopted = source
                .names
                .iter()
                .filter(|name| registered.contains(name.as_str()))
                .count() as u64;
        }
        checkpoint.source_adoption = checkpoint
            .sources
            .iter()
            .filter(|source| source.id != "__source")
            .map(|source| SourceAdoption {
                source_id: source.id.clone(),
                planned_frames: source.names.len() as u64,
                prepared_frames: source.names.len() as u64,
                registered_frames: source.adopted,
                adopted: source.adopted > 0,
                omitted_reason: (source.adopted == 0)
                    .then(|| "not_connected_to_selected_model".into()),
            })
            .collect();
        checkpoint.execution_plan = Some(plan);
        checkpoint.prepared = true;
        checkpoint.expanded_matching = true;
        checkpoint.reshoot_eligible = true;
        working.inputs = Some(checkpoint);
        working.features_complete = true;
        working.matching_complete = true;
        working.reconstruction_complete = true;
        working.stage = PipelineStage::ValidatingReconstruction;
        if let Some(checkpoint) = working.reshoot.as_mut() {
            checkpoint.registered_reshoot_count = working
                .inputs
                .as_ref()
                .unwrap()
                .sources
                .iter()
                .filter(|source| source.id != "__source")
                .map(|source| source.adopted)
                .sum();
            if let Some(provenance) = working_metadata.reshoot.as_mut() {
                provenance.registered_reshoot_count = checkpoint.registered_reshoot_count;
            }
        }
        // Preserve the exact model bytes as well as its IDs. Re-serializing a
        // text/rig model here would change the dataset identity on the next
        // continuation despite identical poses, needlessly rejecting a recipe.
        let authority = paths.colmap.join("authoritative");
        let staging = paths
            .colmap
            .join(format!(".legacy-authoritative-{}", uuid::Uuid::new_v4()));
        let expected_model = super::gps_runner::model_identity(model_path)?;
        tokio::fs::create_dir_all(&staging).await?;
        for source in reusable_model_files(model_path)? {
            super::reshoot_runner::copy_reshoot_file(
                &source,
                &staging.join(source.file_name().unwrap()),
                &self.process_manager.child_token(),
            )
            .await?;
        }
        if super::gps_runner::model_identity(&staging)? != expected_model
            || super::gps_runner::model_identity(model_path)? != expected_model
        {
            return Err(invalid_working_set());
        }
        if authority.exists() {
            tokio::fs::rename(
                &authority,
                paths
                    .colmap
                    .join(format!("legacy-authoritative-{}", uuid::Uuid::new_v4())),
            )
            .await?;
        }
        tokio::fs::rename(&staging, &authority).await?;
        // A saved resolved recipe is immutable. When it was not yet resolved,
        // use the saved policy and resolution plan exactly once, not the new
        // task defaults. Dataset preparation itself is the shared operation.
        let panorama_records = recipe
            .as_ref()
            .map_or(&[][..], |recipe| recipe.panorama_frames.as_slice());
        let dataset = self
            .input_dataset(
                paths,
                &authority,
                metadata.quality,
                panorama_records,
                &names,
            )
            .await?;
        let identity = read_training_dataset_identity(&dataset.path).await?;
        if working
            .brush_training
            .dataset_identity
            .as_ref()
            .is_some_and(|saved| saved != &identity)
        {
            return Err(invalid_working_set());
        }
        if working.brush_training.resolved.is_none() {
            let policy = working.generation_policy.unwrap();
            let initial = working
                .inputs
                .as_ref()
                .and_then(|inputs| inputs.execution_plan.as_ref())
                .and_then(|plan| plan.gaussian_seed())
                .map(|(path, _, _)| {
                    inspect_gaussian_ply(&paths.work.join(path)).map(|info| info.splat_count)
                })
                .transpose()?
                .unwrap_or(report.points_3d);
            let launch = brush::BrushGpuLaunchPolicy::from_acceleration(acceleration);
            let memory = if dataset.has_erp {
                launch.known_training_memory_mb(acceleration)
            } else {
                acceleration.usable_gpu_total_memory_mb()
            };
            let resolution = working.resolution_plan.unwrap_or_else(|| {
                policy.resolution(
                    metadata.quality,
                    memory,
                    dataset.max_edge,
                    dataset.max_edge,
                    false,
                )
            });
            self.prepare_fine_splat_budget(
                &mut working,
                metadata.quality,
                &dataset.path.join("sparse/0"),
                &dataset.path.join("images"),
                dataset.has_erp,
                if metadata.quality == Quality::High {
                    resolution.working_long_edge()
                } else {
                    resolution.brush_initial_max_resolution.max(1)
                },
            )
            .await?;
            let resolved = if dataset.has_erp {
                policy.panorama_training(
                    metadata.quality,
                    memory,
                    dataset.max_edge,
                    report.points_3d,
                    initial,
                )?
            } else {
                policy.training(
                    metadata.quality,
                    memory,
                    &resolution,
                    report.points_3d,
                    initial,
                    working.brush_training.fine_splat_budget.as_ref(),
                )?
            };
            working
                .brush_training
                .initial_resolved
                .get_or_insert(resolved);
            working.brush_training.resolved = Some(resolved);
            working.resolution_plan.get_or_insert(resolution);
        }
        working.brush_training.dataset_identity = Some(identity);
        working.inputs.as_mut().unwrap().reference_image = Some(dataset.reference);
        self.record_collection_artifacts(paths, &mut working)
            .await?;
        manager
            .write_metadata(&paths.metadata, &working_metadata)
            .await?;
        manager.write_state(&paths.state, &working).await?;
        *metadata = working_metadata;
        let warning = working
            .inputs
            .as_ref()
            .and_then(crate::pipeline::inputs::source_adoption_warning);
        Box::pin(self.run_inputs_after_reconstruction(
            manager,
            paths,
            metadata,
            working,
            acceleration,
            authority,
            report,
            warning,
        ))
        .await
        .map(Some)
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    pub(crate) fn retained_dataset(root: &Path) -> (ProjectPaths, PathBuf) {
        let input =
            crate::reconstruction::direct_input::tests::fixture(&root.join("source"), false);
        let paths = ProjectPaths::existing(uuid::Uuid::new_v4(), root.join("project"));
        let dataset = paths.brush.join("dataset");
        std::fs::create_dir_all(dataset.join("images")).unwrap();
        direct_input::read_model(&input.model)
            .unwrap()
            .write(&dataset.join("sparse/0"))
            .unwrap();
        for name in ["normal.png", "second.png"] {
            std::fs::copy(input.images.join(name), dataset.join("images").join(name)).unwrap();
        }
        (paths, input.model)
    }

    #[test]
    fn retained_training_pixels_are_content_bound_and_camera_poses_cannot_change() {
        let root = tempfile::tempdir().unwrap();
        let (paths, authority) = retained_dataset(root.path());
        let dataset = paths.brush.join("dataset");
        let token = tokio_util::sync::CancellationToken::new();
        let before = saved_dataset_identity(&dataset, &authority, &token).unwrap();
        assert_eq!(
            before,
            saved_dataset_identity(&dataset, &authority, &token).unwrap()
        );
        image::RgbaImage::new(64, 48)
            .save(dataset.join("images/normal.png"))
            .unwrap();
        assert_ne!(
            before,
            saved_dataset_identity(&dataset, &authority, &token).unwrap()
        );
        let mut model = direct_input::read_model(&dataset.join("sparse/0")).unwrap();
        model.images[0].pose.tvec[0] += 1.;
        model.write(&dataset.join("sparse/0")).unwrap();
        assert!(saved_dataset_identity(&dataset, &authority, &token).is_err());
    }

    #[tokio::test]
    async fn complete_actual_legacy_recipe_reaches_common_training_without_policy_or_new_defaults()
    {
        for warm in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let (paths, authority) = retained_dataset(root.path());
            let manager = ProjectManager::for_diagnostics(root.path().join("projects"));
            let mut metadata = ProjectMetadata {
                quality: Quality::Fast,
                ..Default::default()
            };
            let mut state = if warm {
                legacy_reshoot_state()
            } else {
                PipelineStateFile::created(Quality::Fast)
            };
            state.generation_policy = None;
            state.preset = Quality::Fast;
            state.brush_training.oom_retry_used = true;
            let mut resolved =
                crate::presets::resolve_brush_training_preset(Quality::Fast, false, None, 64, 1);
            if warm {
                let mut ply = b"ply\nformat binary_little_endian 1.0\nelement vertex 1\n".to_vec();
                for property in [
                    "x", "y", "z", "f_dc_0", "f_dc_1", "f_dc_2", "opacity", "scale_0", "scale_1",
                    "scale_2", "rot_0", "rot_1", "rot_2", "rot_3",
                ] {
                    ply.extend_from_slice(format!("property float {property}\n").as_bytes());
                }
                ply.extend_from_slice(b"end_header\n");
                for value in [
                    0f32, 0., 1., 0.1, 0.1, 0.1, 0., -3., -3., -3., 1., 0., 0., 0.,
                ] {
                    ply.extend_from_slice(&value.to_le_bytes());
                }
                let init = paths.brush.join("dataset/init.ply");
                std::fs::write(&init, &ply).unwrap();
                let (preset, parameters) = crate::pipeline::reshoot::warm_training(
                    resolved.preset,
                    Quality::Fast,
                    2,
                    1,
                    0,
                )
                .unwrap();
                resolved.preset = preset;
                let recipe = state.reshoot.as_mut().unwrap().recipe.as_mut().unwrap();
                // The retained fixture's actual registered views must also
                // prove that both historical and new images participated.
                recipe.source_names = vec!["normal.png".into()];
                recipe.new_names = vec!["second.png".into()];
                recipe.actual.training = TrainingMode::Warm;
                recipe.normal_parameters_version = None;
                recipe.warm_parameters = Some(parameters);
                recipe.init_sha256 =
                    Some(hash_file(&init, &tokio_util::sync::CancellationToken::new()).unwrap());
                recipe.init_georeference = None;
                metadata.reshoot = Some(ReshootProvenance {
                    source_project_id: uuid::Uuid::new_v4(),
                    source_project_path: root.path().join("missing-original"),
                    source_final_ply: root.path().join("missing-original/final.ply"),
                    reshoot_source_path: root.path().join("input"),
                    camera_id: 7,
                    camera_model: "SIMPLE_RADIAL".into(),
                    width: 64,
                    height: 48,
                    has_alpha: false,
                    mask_count: 0,
                    source_image_count: 1,
                    reshoot_frame_count: 1,
                    registered_reshoot_count: 1,
                });
            }
            state.brush_training.resolved = Some(resolved);
            let acceleration:crate::engines::ColmapAccelerationStatus=serde_json::from_value(serde_json::json!({
                "backend":"cpu","detectionState":"unavailable","reasonCode":"noNvidiaGpu","reason":"fixture","device":null,
                "requirements":{"minimumDriverVersion":"580.00","minimumComputeCapability":"7.5"}
            })).unwrap();
            let runner = PipelineRunner::new(
                EnginePaths::from_root(root.path().join("missing-engines")),
                |_| {},
            );
            let error = runner
                .saved_legacy_training_tail(
                    &manager,
                    &paths,
                    &mut metadata,
                    state,
                    &authority,
                    &acceleration,
                )
                .await
                .unwrap_err();
            assert!(!error.to_string().contains(UNRESTORABLE_RECIPE), "{error}");
            let retained = manager.read_state(&paths.state).await.unwrap();
            assert_eq!(retained.generation_policy, None);
            assert_eq!(retained.brush_training.resolved, Some(resolved));
            assert!(retained.brush_training.oom_retry_used);
            assert!(retained.brush_training.legacy_dataset_identity.is_some());
            if warm {
                assert!(paths.work.join("legacy-training-init.ply").is_file());
                assert!(retained
                    .brush_training
                    .legacy_initialization_sha256
                    .is_some());
                assert_eq!(
                    retained
                        .reshoot
                        .as_ref()
                        .unwrap()
                        .recipe
                        .as_ref()
                        .unwrap()
                        .reconstruction_fallbacks,
                    1
                );
                assert_eq!(
                    retained
                        .reshoot
                        .as_ref()
                        .unwrap()
                        .recipe
                        .as_ref()
                        .unwrap()
                        .training_fallbacks,
                    1
                );
            }
        }
    }

    fn legacy_reshoot_state() -> PipelineStateFile {
        use crate::pipeline::reshoot::{
            ModelCondition, ReshootOptions, ReshootPlan, ReshootRecipe, RECIPE_VERSION,
        };
        let mut state = PipelineStateFile::created(Quality::Balanced);
        state.generation_policy = Some(Default::default());
        let plan = ReshootPlan::resolve(ModelCondition::Appearance, true, true);
        state.reshoot = Some(ReshootState {
            source_snapshot_complete: true,
            supplemental_frames_complete: true,
            supplemental_features_complete: true,
            incremental_matching_complete: true,
            incremental_reconstruction_complete: true,
            recipe: Some(ReshootRecipe {
                version: RECIPE_VERSION,
                normal_parameters_version: Some(1),
                camera_assignment: None,
                options: ReshootOptions {
                    condition: ModelCondition::Appearance,
                    camera_relation: None,
                    allow_fallback: false,
                    source_identity: "source".into(),
                    input_identity: "input".into(),
                    projection: VideoProjection::Perspective,
                    image_matching_strategy: Default::default(),
                },
                initial: plan,
                actual: plan,
                batch_id: "batch".into(),
                reconstruction_fallbacks: 1,
                training_fallbacks: 1,
                expanded_matching: true,
                source_names: vec!["old.png".into()],
                new_names: vec!["new.png".into()],
                warm_parameters: None,
                init_sha256: None,
                init_georeference: None,
                stored_input_identity: Some("input".into()),
                panorama_frames: vec![],
            }),
            ..Default::default()
        });
        state
    }

    #[test]
    fn missing_policy_and_completed_candidates_are_never_reinterpreted() {
        let mut state = PipelineStateFile::created(Quality::Balanced);
        assert!(unsupported_recipe(&state, false));
        state.generation_policy = Some(Default::default());
        assert!(!unsupported_recipe(&state, false));
        state.brush_complete = true;
        assert!(unsupported_recipe(&state, false));
        assert!(unsupported_recipe(&state, true));
    }

    #[test]
    fn completed_normal_recipe_is_eligible_without_resetting_consumed_allowances() {
        let mut state = legacy_reshoot_state();
        state.brush_training.oom_retry_used = true;
        state.brush_training.oom_retry_source_count = Some(1_000_000);
        let before = serde_json::to_value(&state).unwrap();
        assert!(!unsupported_recipe(&state, true));
        assert_eq!(serde_json::to_value(&state).unwrap(), before);
        state
            .reshoot
            .as_mut()
            .unwrap()
            .recipe
            .as_mut()
            .unwrap()
            .normal_parameters_version = None;
        assert!(unsupported_recipe(&state, true));
    }

    #[test]
    fn existing_v2_initialization_identity_is_not_relabelled_as_v3() {
        let mut state = legacy_reshoot_state();
        let recipe = state.reshoot.as_mut().unwrap().recipe.as_mut().unwrap();
        recipe.init_sha256 = Some("old-ply-digest".into());
        recipe.init_georeference = Some("old-gps-transform".into());
        assert!(unsupported_recipe(&state, true));
        let recipe = state.reshoot.as_ref().unwrap().recipe.as_ref().unwrap();
        assert_eq!(recipe.init_sha256.as_deref(), Some("old-ply-digest"));
        assert_eq!(
            recipe.init_georeference.as_deref(),
            Some("old-gps-transform")
        );
    }

    #[test]
    fn camera_ids_and_pixel_coordinates_must_match_the_actual_database() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::existing(uuid::Uuid::new_v4(), temp.path().to_path_buf());
        for dir in [&paths.frames, &paths.colmap_frames, &paths.colmap] {
            std::fs::create_dir_all(dir).unwrap();
        }
        for dir in [&paths.frames, &paths.colmap_frames] {
            image::RgbImage::new(2, 2)
                .save(dir.join("one.png"))
                .unwrap();
        }
        let database = rusqlite::Connection::open(paths.colmap.join("database.db")).unwrap();
        database.execute_batch("CREATE TABLE cameras(camera_id INTEGER,model INTEGER,width INTEGER,height INTEGER); CREATE TABLE images(image_id INTEGER,name TEXT,camera_id INTEGER); CREATE TABLE keypoints(image_id INTEGER,rows INTEGER,cols INTEGER,data BLOB); INSERT INTO cameras VALUES(7,2,2,2); INSERT INTO images VALUES(13,'one.png',7);").unwrap();
        let data = [0.5f32.to_le_bytes(), 1.5f32.to_le_bytes()].concat();
        database
            .execute("INSERT INTO keypoints VALUES(13,1,2,?1)", [data])
            .unwrap();
        drop(database);
        let mut model = Model {
            cameras: vec![ColmapCamera {
                id: 7,
                model_id: 2,
                model: "SIMPLE_RADIAL".into(),
                width: 2,
                height: 2,
                params: vec![2., 1., 1., 0.],
            }],
            images: vec![direct_input::ModelImage {
                pose: crate::video::panorama::PosedImage {
                    id: 13,
                    qvec: [1., 0., 0., 0.],
                    tvec: [0.; 3],
                    camera_id: 7,
                    name: "one.png".into(),
                },
                observations: vec![direct_input::Observation {
                    x: 0.5,
                    y: 1.5,
                    point: None,
                }],
            }],
            points: vec![],
        };
        let names = vec!["one.png".to_owned()];
        let cancel = tokio_util::sync::CancellationToken::new();
        let adopted = validate_working_set(&paths, &model, &names, &cancel).unwrap();
        assert_eq!(adopted["one.png"], (7, [2, 2]));
        model.images[0].pose.id = 14;
        assert!(validate_working_set(&paths, &model, &names, &cancel).is_err());
        model.images[0].pose.id = 13;
        model.images[0].observations[0].x = 1.5;
        assert!(validate_working_set(&paths, &model, &names, &cancel).is_err());
        assert!(validate_working_set(
            &paths,
            &model,
            &["one.png".into(), "one.png".into()],
            &cancel
        )
        .is_err());
    }
}
