//! Ordered multi-input execution inside the existing Runner/lifecycle/locks.
use super::*;
use crate::pipeline::{inputs::*, reshoot::PanoramaFrameRecord};
use crate::reconstruction::direct_input::read_model;
use crate::video::panorama::VideoProjection;
#[path = "inputs_bridge.rs"]
mod bridge;
#[cfg(test)]
#[path = "gps_resume_validation.rs"]
mod gps_resume_validation;
#[cfg(test)]
#[path = "inputs_runner_tests.rs"]
mod tests;

fn bind_collection_training_recipe(
    state: &mut PipelineStateFile,
    dataset_identity: String,
    rebuilt_work: bool,
) -> Result<()> {
    if state.brush_training.resolved.is_none() {
        return Ok(());
    }
    if state.brush_training.dataset_identity.as_ref() != Some(&dataset_identity)
        && (!state.retry_cache_pending.is_empty()
            || (!rebuilt_work
                && !crate::pipeline::artifacts::validated_training_dataset_rebuild(state)))
    {
        return Err(SplatError::Process(
            "Saved training recipe is incompatible with the regenerated training dataset".into(),
        ));
    }
    // Rebinding affects provenance only: actual parameters, initialization
    // validation and consumed rescue allowances remain exactly as saved.
    state.brush_training.dataset_identity = Some(dataset_identity);
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FrozenReshootSourceBatch {
    id: String,
    identity: String,
    names: Vec<String>,
    projection: VideoProjection,
    #[serde(default)]
    camera_ids: Vec<u32>,
    #[serde(default)]
    panorama_records: Vec<PanoramaFrameRecord>,
    #[serde(default)]
    video: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FrozenReshootSourceContext {
    source_identity: String,
    names: Vec<String>,
    panorama_records: Vec<PanoramaFrameRecord>,
    /// Original prepared-source boundaries are diagnostic/provenance data.
    /// Execution currently exposes the immutable source model as one fixed
    /// seed, but freezing these batches prevents continuous reshoots from
    /// losing camera/projection/source ownership information.
    #[serde(default)]
    source_batches: Vec<FrozenReshootSourceBatch>,
    #[serde(default = "default_true")]
    reshoot_eligible: bool,
    generation_policy: Option<crate::pipeline::generation_policy::GenerationPolicy>,
    configuration: Option<crate::presets::PipelineOptimizationConfig>,
    gps: Option<crate::reconstruction::gps::Checkpoint>,
    transform: crate::project::GaussianTransform,
}

fn default_true() -> bool {
    true
}

pub(super) struct FrozenReshootSource {
    manifest: crate::pipeline::reshoot_snapshot::ReshootSnapshotV3,
    context: FrozenReshootSourceContext,
    frames: PathBuf,
    colmap_frames: PathBuf,
    masks: PathBuf,
    database: PathBuf,
    model: PathBuf,
    source_ply: Option<PathBuf>,
}

impl PipelineRunner {
    pub(super) async fn ensure_reshoot_snapshot_v3(
        &self,
        paths: &ProjectPaths,
        metadata: &mut ProjectMetadata,
        state: &mut PipelineStateFile,
    ) -> Result<FrozenReshootSource> {
        use crate::pipeline::reshoot_snapshot::{
            verify_snapshot, ReshootSnapshotBuilder, SnapshotAssetInput, SnapshotAssetRole,
            RESHOOT_SNAPSHOT_MANIFEST,
        };

        let provenance = metadata
            .reshoot
            .as_ref()
            .ok_or_else(|| SplatError::Process("Missing reshoot provenance".into()))?;
        let snapshot_root = paths.work.join("reshoot-snapshot-v3");
        let manifest_path = snapshot_root.join(RESHOOT_SNAPSHOT_MANIFEST);
        let manifest = if manifest_path.is_file() {
            let snapshot = verify_snapshot(&snapshot_root)?;
            if snapshot.source_project_id != Some(provenance.source_project_id)
                || state
                    .reshoot
                    .as_ref()
                    .and_then(|value| value.snapshot_v3_identity.as_ref())
                    .is_some_and(|identity| identity != &snapshot.content_identity)
            {
                return Err(SplatError::Process(
                    "Reshoot source snapshot identity changed".into(),
                ));
            }
            snapshot
        } else {
            self.authorize_reshoot_source(provenance.source_project_id)
                .await?;
            let source = super::reshoot_runner::source_assets(provenance.source_project_id).await?;
            let source_identity = super::reshoot_runner::source_identity(&source)?;
            let expected = state
                .reshoot
                .as_ref()
                .and_then(|value| value.recipe.as_ref())
                .map(|recipe| recipe.options.source_identity.as_str())
                .ok_or_else(|| SplatError::Process("Missing reshoot recipe".into()))?;
            if source_identity != expected {
                return Err(SplatError::Process("INPUT_CHANGED".into()));
            }

            let mut names = source.names.clone();
            let mut seen = HashSet::new();
            names.retain(|name| seen.insert(name.clone()));
            if names.is_empty() {
                return Err(SplatError::Process(
                    "Reshoot source contains no prepared images".into(),
                ));
            }
            let panorama_records = source
                .state
                .as_ref()
                .and_then(|state| state.inputs.as_ref())
                .map(|inputs| {
                    inputs
                        .sources
                        .iter()
                        .flat_map(|source| source.panorama_records.clone())
                        .collect::<Vec<_>>()
                })
                .filter(|records| !records.is_empty())
                .or_else(|| {
                    source
                        .state
                        .as_ref()
                        .and_then(|state| state.reshoot.as_ref())
                        .and_then(|value| value.recipe.as_ref())
                        .map(|recipe| recipe.panorama_frames.clone())
                })
                .unwrap_or_default();
            let source_model_has_erp = read_model(&source.model)?
                .cameras
                .iter()
                .any(|camera| camera.model_id == 17);
            let source_reshoot_eligible = source
                .state
                .as_ref()
                .and_then(|state| state.inputs.as_ref())
                .is_none_or(|inputs| inputs.reshoot_eligible);
            let source_batches = source
                .state
                .as_ref()
                .and_then(|state| state.inputs.as_ref())
                .map(|inputs| {
                    inputs
                        .sources
                        .iter()
                        .map(|batch| FrozenReshootSourceBatch {
                            id: batch.id.clone(),
                            identity: batch.identity.clone(),
                            names: batch.names.clone(),
                            projection: batch.projection,
                            camera_ids: batch.camera_ids.clone(),
                            panorama_records: batch.panorama_records.clone(),
                            video: batch.video.is_some(),
                        })
                        .collect::<Vec<_>>()
                })
                .filter(|batches| !batches.is_empty())
                .unwrap_or_else(|| {
                    vec![FrozenReshootSourceBatch {
                        id: "__source".into(),
                        identity: source_identity.clone(),
                        names: names.clone(),
                        projection: if source_model_has_erp {
                            VideoProjection::Equirectangular
                        } else {
                            VideoProjection::Perspective
                        },
                        camera_ids: Vec::new(),
                        panorama_records: panorama_records.clone(),
                        video: source.metadata.input_type == ProjectInputType::Video,
                    }]
                });
            let context = FrozenReshootSourceContext {
                source_identity: source_identity.clone(),
                names: names.clone(),
                panorama_records,
                source_batches,
                reshoot_eligible: source_reshoot_eligible,
                generation_policy: source
                    .state
                    .as_ref()
                    .and_then(|state| state.generation_policy),
                configuration: source
                    .state
                    .as_ref()
                    .and_then(|state| state.configuration.clone()),
                gps: source.state.as_ref().and_then(|state| state.gps.clone()),
                transform: source.metadata.transform,
            };
            let context_source = paths.work.join(format!(
                ".reshoot-snapshot-context-{}.json",
                uuid::Uuid::new_v4()
            ));
            crate::project::manager::atomic_write_json(&context_source, &context).await?;

            let coordinate_frame_id = source
                .state
                .as_ref()
                .and_then(|state| state.gps.as_ref())
                .and_then(|gps| gps.output_identity.clone())
                .unwrap_or_else(|| source_identity.clone());
            let mut builder = ReshootSnapshotBuilder::new(&snapshot_root)
                .source_project_id(provenance.source_project_id)
                .coordinate_frame_id(coordinate_frame_id)
                .model(&source.model)
                .database(&source.database)
                .asset(SnapshotAssetInput {
                    role: SnapshotAssetRole::Other,
                    source: context_source.clone(),
                    relative_path: PathBuf::from("source/context.json"),
                });
            if let Some(ply) = catalog::project_model_path(&source.root, &source.metadata)
                .filter(|_| source.metadata.imported.is_none())
                .filter(|path| inspect_gaussian_ply(path).is_ok())
            {
                builder = builder.source_ply(ply);
            }
            for name in &names {
                let frame = crate::reconstruction::direct_input::image_path(&source.images, name)?;
                let colmap_frame =
                    crate::reconstruction::direct_input::image_path(&source.colmap_images, name)?;
                builder = builder
                    .asset(SnapshotAssetInput {
                        role: SnapshotAssetRole::Image,
                        source: frame,
                        relative_path: PathBuf::from("frames").join(name),
                    })
                    .asset(SnapshotAssetInput {
                        role: SnapshotAssetRole::Other,
                        source: colmap_frame,
                        relative_path: PathBuf::from("colmap_frames").join(name),
                    });
                let mask = source.masks.join(format!("{name}.png"));
                if mask.is_file() {
                    builder = builder.mask(mask, format!("{name}.png"));
                }
            }
            let gps_manifest = source.root.join("work/gps/manifest.json");
            if gps_manifest.is_file() {
                builder = builder.asset(SnapshotAssetInput {
                    role: SnapshotAssetRole::Gps,
                    source: gps_manifest,
                    relative_path: PathBuf::from("gps/manifest.json"),
                });
            }
            let result = builder.build(&self.process_manager.child_token()).await;
            let _ = tokio::fs::remove_file(context_source).await;
            result?
        };

        let context: FrozenReshootSourceContext = serde_json::from_slice(
            &tokio::fs::read(snapshot_root.join("source/context.json")).await?,
        )?;
        if !context.reshoot_eligible {
            return Err(SplatError::Process(
                "Reshoot snapshot is not eligible for reusable reconstruction".into(),
            ));
        }
        if !context.source_batches.is_empty() {
            let mapped = context
                .source_batches
                .iter()
                .flat_map(|batch| batch.names.iter().cloned())
                .collect::<HashSet<_>>();
            let expected = context.names.iter().cloned().collect::<HashSet<_>>();
            if mapped != expected {
                return Err(SplatError::Process(
                    "Reshoot snapshot source mapping is inconsistent".into(),
                ));
            }
        }
        if state
            .reshoot
            .as_ref()
            .and_then(|value| value.recipe.as_ref())
            .is_some_and(|recipe| recipe.options.source_identity != context.source_identity)
        {
            return Err(SplatError::Process(
                "Reshoot snapshot does not match the saved source identity".into(),
            ));
        }
        if let Some(reshoot) = state.reshoot.as_mut() {
            reshoot.snapshot_v3_identity = Some(manifest.content_identity.clone());
            reshoot.snapshot_v3_ready = true;
            if let Some(recipe) = reshoot.recipe.as_mut() {
                recipe.init_georeference = Some(manifest.coordinate_frame_id.clone());
                recipe.init_sha256 = manifest.source_ply.as_ref().map(|ply| ply.sha256.clone());
                if manifest.source_ply.is_none()
                    && recipe.actual.training == crate::pipeline::reshoot::TrainingMode::Warm
                {
                    recipe.initial.training = crate::pipeline::reshoot::TrainingMode::Cold;
                    recipe.actual.training = crate::pipeline::reshoot::TrainingMode::Cold;
                }
            }
        }
        let source_ply = manifest
            .source_ply
            .as_ref()
            .map(|ply| snapshot_root.join(&ply.relative_path));
        if let (Some(provenance), Some(source_ply)) = (metadata.reshoot.as_mut(), &source_ply) {
            provenance.source_final_ply = source_ply.clone();
        }
        Ok(FrozenReshootSource {
            frames: snapshot_root.join("frames"),
            colmap_frames: snapshot_root.join("colmap_frames"),
            masks: snapshot_root.join("masks"),
            database: snapshot_root.join("colmap/database.db"),
            model: snapshot_root.join("colmap/sparse"),
            source_ply,
            manifest,
            context,
        })
    }

    #[cfg(test)]
    pub(super) async fn prepare_inputs_retry(
        &self,
        manager: &ProjectManager,
        paths: &ProjectPaths,
        state: &mut PipelineStateFile,
        _policy: Option<crate::pipeline::generation_policy::GenerationPolicy>,
    ) -> Result<()> {
        self.prepare_continuation(manager, paths, state).await
    }
    pub(super) async fn generate_inputs(
        &self,
        manager: ProjectManager,
        mut inputs: GenerationInputs,
        quality: Quality,
        source_id: Option<uuid::Uuid>,
    ) -> Result<PipelineResult> {
        self.authorize_external_inputs(&inputs)?;
        inputs = inputs.canonicalized()?;
        if source_id.is_some() && matches!(&inputs, GenerationInputs::Colmap { .. }) {
            return Err(SplatError::Process(
                "补拍暂不支持导入 COLMAP 重建结果，请添加视频或图片。".into(),
            ));
        }
        let manager = manager.with_creation_intent(self.creation_intent.clone());
        crate::pipeline::inputs::validate(
            &self.engines,
            &inputs,
            quality,
            self.policy().unwrap_or_default(),
        )
        .await?;
        inputs.authorized_paths()?;
        match &mut inputs {
            GenerationInputs::Materials { items } => {
                for item in items {
                    let a = inspect_material(
                        &self.engines,
                        item,
                        quality,
                        self.policy().unwrap_or_default(),
                    )
                    .await?;
                    if item.identity.as_ref().is_some_and(|i| i != &a.identity) {
                        return Err(SplatError::Process("INPUT_CHANGED".into()));
                    }
                    item.projection = projection(item, &a)?;
                    if source_id.is_some()
                        && item.projection == VideoProjection::Perspective
                        && item.camera_relation.is_none()
                    {
                        return Err(SplatError::Process("CONFIRMATION_REQUIRED".into()));
                    }
                    item.identity = Some(a.identity);
                }
            }
            GenerationInputs::Colmap { input } => {
                let a = crate::reconstruction::direct_input::inspect(input)?;
                input.identity = Some(a.identity);
            }
        }
        let acceleration = self.verify_pipeline_engines().await?;
        self.events.acceleration(acceleration.clone());
        let (paths, mut metadata) = manager.create_collection(quality, inputs).await?;
        metadata.workspace_task_id = self.workspace_task_id;
        let mut state = manager.read_state(&paths.state).await?;
        state.generation_policy = Some(self.policy().unwrap_or_default());
        state.planner_enabled = true;
        if let Some(InputCheckpoint {
            request: GenerationInputs::Materials { items },
            ..
        }) = &state.inputs
        {
            if items.len() == 1 {
                let a = inspect_material(
                    &self.engines,
                    &items[0],
                    quality,
                    state.generation_policy.unwrap(),
                )
                .await?;
                state.video = a.video;
                state.image_sequence = a.image_sequence;
            }
        }
        if let Some(id) = source_id {
            use crate::pipeline::reshoot::*;
            self.authorize_reshoot_source(id).await?;
            let source = super::reshoot_runner::source_assets(id).await?;
            let mut options = self
                .reshoot_options
                .clone()
                .ok_or_else(|| SplatError::Process("CONFIRMATION_REQUIRED".into()))?;
            if super::reshoot_runner::source_identity(&source)? != options.source_identity {
                return Err(SplatError::Process("INPUT_CHANGED".into()));
            }
            options.allow_fallback = state.generation_policy.unwrap().auto_retry_enabled;
            let plan =
                ReshootPlan::resolve(options.condition, true, source.metadata.imported.is_none());
            metadata.name = reshoot_project_name(&source.metadata.name);
            metadata.transform = source.metadata.transform;
            metadata.reshoot = Some(ReshootProvenance {
                source_project_id: id,
                source_project_path: source.root.clone(),
                source_final_ply: catalog::project_model_path(&source.root, &source.metadata)
                    .unwrap_or_else(|| source.root.join("final.ply")),
                reshoot_source_path: metadata.source_path.clone(),
                camera_id: 0,
                camera_model: String::new(),
                width: 0,
                height: 0,
                has_alpha: false,
                mask_count: 0,
                source_image_count: source.names.len() as u64,
                reshoot_frame_count: 0,
                registered_reshoot_count: 0,
            });
            state.reshoot = Some(ReshootState {
                recipe: Some(ReshootRecipe {
                    version: RECIPE_VERSION,
                    normal_parameters_version: Some(1),
                    camera_assignment: None,
                    options,
                    initial: plan,
                    actual: plan,
                    batch_id: paths.id.simple().to_string(),
                    reconstruction_fallbacks: 0,
                    training_fallbacks: 0,
                    expanded_matching: false,
                    source_names: vec![],
                    new_names: vec![],
                    warm_parameters: None,
                    init_sha256: None,
                    init_georeference: None,
                    stored_input_identity: None,
                    panorama_frames: vec![],
                }),
                ..Default::default()
            });
        }
        manager.write_metadata(&paths.metadata, &metadata).await?;
        state.configuration = Some((*crate::presets::pipeline_optimization_config()).clone());
        manager.write_state(&paths.state, &state).await?;
        Box::pin(self.execute_project(manager, paths, &mut metadata, state, &acceleration)).await
    }

    pub(super) async fn run_inputs(
        &self,
        manager: &ProjectManager,
        paths: &ProjectPaths,
        metadata: &mut ProjectMetadata,
        state: PipelineStateFile,
        acceleration: &crate::engines::ColmapAccelerationStatus,
    ) -> Result<PipelineResult> {
        Box::pin(self.run_inputs_with_reconstruction(
            manager,
            paths,
            metadata,
            state,
            acceleration,
            None,
        ))
        .await
    }

    /// Compatibility adapters may supply a reconstruction that has already
    /// been reconciled in this invocation. Everything after it still uses the
    /// same scheduler, training and publication path.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn run_inputs_after_reconstruction(
        &self,
        manager: &ProjectManager,
        paths: &ProjectPaths,
        metadata: &mut ProjectMetadata,
        state: PipelineStateFile,
        acceleration: &crate::engines::ColmapAccelerationStatus,
        model: PathBuf,
        report: ReconstructionReport,
        warning: Option<String>,
    ) -> Result<PipelineResult> {
        Box::pin(self.run_inputs_with_reconstruction(
            manager,
            paths,
            metadata,
            state,
            acceleration,
            Some((model, report, warning)),
        ))
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_inputs_with_reconstruction(
        &self,
        manager: &ProjectManager,
        paths: &ProjectPaths,
        metadata: &mut ProjectMetadata,
        mut state: PipelineStateFile,
        acceleration: &crate::engines::ColmapAccelerationStatus,
        mut reconstruction: Option<(PathBuf, ReconstructionReport, Option<String>)>,
    ) -> Result<PipelineResult> {
        let mut checkpoint = state
            .inputs
            .clone()
            .ok_or_else(|| SplatError::Process("Missing input checkpoint".into()))?;
        let policy = state.generation_policy.ok_or_else(|| {
            SplatError::Process("Missing saved generation policy for the shared work order".into())
        })?;
        let authority = paths.colmap.join("authoritative");
        let mut frozen_source = if metadata.reshoot.is_some() {
            Some(
                self.ensure_reshoot_snapshot_v3(paths, metadata, &mut state)
                    .await?,
            )
        } else {
            None
        };
        if checkpoint.execution_plan.is_none() {
            if let Some(frozen) = frozen_source.as_ref() {
                let recipe = state
                    .reshoot
                    .as_mut()
                    .and_then(|value| value.recipe.as_mut())
                    .ok_or_else(|| SplatError::Process("Missing reshoot recipe".into()))?;
                if frozen.source_ply.is_none() {
                    recipe.initial.training = crate::pipeline::reshoot::TrainingMode::Cold;
                    recipe.actual.training = crate::pipeline::reshoot::TrainingMode::Cold;
                }
                checkpoint.execution_plan =
                    Some(crate::pipeline::execution_plan::ExecutionPlan::reshoot(
                        recipe,
                        crate::pipeline::execution_plan::SnapshotModel {
                            relative_path: PathBuf::from("reshoot-snapshot-v3/colmap/sparse"),
                            sha256: frozen.manifest.content_identity.clone(),
                            coordinate_frame_id: frozen.manifest.coordinate_frame_id.clone(),
                            has_erp: frozen.manifest.has_erp,
                            fixed_images: frozen
                                .manifest
                                .registered_images
                                .iter()
                                .map(|image| image.id)
                                .collect(),
                            fixed_cameras: frozen
                                .manifest
                                .cameras
                                .iter()
                                .map(|camera| camera.id)
                                .collect(),
                        },
                    ));
                state.inputs = Some(checkpoint.clone());
                manager.write_state(&paths.state, &state).await?;
                manager.write_metadata(&paths.metadata, metadata).await?;
            } else {
                checkpoint.execution_plan =
                    Some(crate::pipeline::execution_plan::ExecutionPlan::normal());
            }
        }
        use crate::pipeline::work_order::{next_action, GenerationAction, Readiness};
        let mut training_dataset: Option<super::input_dataset::Dataset> = None;
        let mut rebuilt_work = false;
        // The final model is immutable across prepare/train in this call.
        // New invocations and new reconstructions always validate afresh.
        let mut connection_validated = false;
        loop {
            if self.process_manager.child_token().is_cancelled() {
                return Err(SplatError::Cancelled);
            }
            let mut names = checkpoint
                .sources
                .iter()
                .flat_map(|s| s.names.clone())
                .collect::<Vec<_>>();
            let completed_candidate = state
                .brush_complete
                .then(|| collection_candidate(paths, &state))
                .flatten();
            let ready = Readiness {
                reconstruction: reconstruction.is_some(),
                training_dataset: training_dataset.is_some(),
                candidate: completed_candidate.is_some(),
            };
            self.events.source(None);
            let erp = checkpoint
                .sources
                .iter()
                .any(|source| source.projection == VideoProjection::Equirectangular)
                || checkpoint
                    .execution_plan
                    .as_ref()
                    .is_some_and(|plan| plan.source_has_erp);
            self.events
                .panorama_phase
                .store(u64::from(erp), Ordering::Relaxed);
            self.events.panorama_images.store(
                checkpoint
                    .sources
                    .iter()
                    .all(|source| source.video.is_none()),
                Ordering::Relaxed,
            );
            let action = next_action(&state, ready);
            match action {
                GenerationAction::PrepareInputs => {
                    connection_validated = false;
                    rebuilt_work = true;
                    self.authorize_external_inputs(&checkpoint.request)?;
                    checkpoint.request.check_identities()?;
                    self.events
                        .stage(PipelineStage::ExtractingFrames, 0., "正在准备训练图片");
                    match &checkpoint.request {
                        GenerationInputs::Colmap { input } => {
                            let info = crate::reconstruction::direct_input::inspect(input)?;
                            let model = read_model(&input.model)?;
                            for image in &model.images {
                                let source = crate::reconstruction::direct_input::image_path(
                                    &input.images,
                                    &image.pose.name,
                                )?;
                                let target = paths.frames.join(&image.pose.name);
                                tokio::fs::create_dir_all(target.parent().unwrap()).await?;
                                tokio::fs::copy(&source, &target).await?;
                                let rgb = paths.colmap_frames.join(&image.pose.name);
                                tokio::fs::create_dir_all(rgb.parent().unwrap()).await?;
                                tokio::fs::copy(source, rgb).await?;
                            }
                            model.write(&authority)?;
                            checkpoint.imported_reconstruction = true;
                            checkpoint.reshoot_eligible = info.reshoot_eligible;
                            if info.reshoot_eligible {
                                if let Some(db) = &input.database {
                                    tokio::fs::copy(db, paths.colmap.join("database.db")).await?;
                                }
                            }
                            let mut names = model
                                .images
                                .iter()
                                .map(|i| i.pose.name.clone())
                                .collect::<Vec<_>>();
                            names.sort();
                            let records = names
                                .iter()
                                .enumerate()
                                .map(|(i, name)| PanoramaFrameRecord {
                                    prepared_name: name.clone(),
                                    batch_id: "imported".into(),
                                    source_name: name.clone(),
                                    source_sha256: String::new(),
                                    orientation: Some(1),
                                    source_index: i as u64,
                                    timestamp_seconds: None,
                                    pts: None,
                                })
                                .collect();
                            checkpoint.sources = vec![SourceCheckpoint {
                                id: "imported".into(),
                                identity: info.identity.clone(),
                                camera_ids: model
                                    .images
                                    .iter()
                                    .map(|i| i.pose.camera_id)
                                    .collect::<std::collections::BTreeSet<_>>()
                                    .into_iter()
                                    .collect(),
                                camera_assignment: None,
                                names,
                                originals: vec![],
                                projection: model_projection(&model),
                                video: None,
                                image_sequence: None,
                                dimensions: vec![],
                                working_dimensions: None,
                                frames: None,
                                panorama_records: records,
                                adopted: info.image_count,
                                bridge_frames: vec![],
                            }];
                            checkpoint.execution_plan =
                                Some(crate::pipeline::execution_plan::ExecutionPlan::imported(
                                    crate::pipeline::execution_plan::SnapshotModel {
                                        relative_path: PathBuf::from("colmap/authoritative"),
                                        sha256: info.identity.clone(),
                                        coordinate_frame_id: info.identity.clone(),
                                        has_erp: model
                                            .cameras
                                            .iter()
                                            .any(|camera| camera.model_id == 17),
                                        fixed_images: model
                                            .images
                                            .iter()
                                            .map(|image| image.pose.id)
                                            .collect(),
                                        fixed_cameras: model
                                            .cameras
                                            .iter()
                                            .map(|camera| camera.id)
                                            .collect(),
                                    },
                                ));
                            state.reconstruction_complete = true;
                            // Intentionally not marked as executed: UI receives an explicit skip descriptor.
                            self.events.stage(
                                PipelineStage::TrainingSplats,
                                0.,
                                "使用导入的重建结果，跳过特征提取、图像匹配和相机重建。",
                            );
                        }
                        GenerationInputs::Materials { items } => {
                            if metadata.reshoot.is_some()
                                && !checkpoint
                                    .sources
                                    .iter()
                                    .any(|source| source.id == "__source")
                            {
                                let frozen = match frozen_source.take() {
                                    Some(frozen) => frozen,
                                    None => {
                                        self.ensure_reshoot_snapshot_v3(paths, metadata, &mut state)
                                            .await?
                                    }
                                };
                                let source_identity = frozen.context.source_identity.clone();
                                let original = read_model(&frozen.model)?;
                                let base = paths.colmap.join("base-model");
                                original.write(&base)?;
                                super::reshoot_runner::copy_reshoot_file(
                                    &frozen.database,
                                    &paths.colmap.join("base-database.db"),
                                    &self.process_manager.child_token(),
                                )
                                .await?;
                                let old_names = frozen.context.names.clone();
                                let progress = super::reshoot_runner::ReshootProgress::new(
                                    &self.events,
                                    PipelineStage::ExtractingFrames,
                                    &self.process_manager.child_token(),
                                    "正在准备原项目文件",
                                    Some(old_names.len() as u64),
                                );
                                for (i, name) in old_names.iter().enumerate() {
                                    for (from, to) in [
                                        (&frozen.frames, &paths.frames),
                                        (&frozen.colmap_frames, &paths.colmap_frames),
                                    ] {
                                        let target = to.join(name);
                                        tokio::fs::create_dir_all(target.parent().unwrap()).await?;
                                        super::reshoot_runner::copy_reshoot_file(
                                            &from.join(name),
                                            &target,
                                            &self.process_manager.child_token(),
                                        )
                                        .await?;
                                    }
                                    let mask = frozen.masks.join(format!("{name}.png"));
                                    if mask.is_file() {
                                        let target = paths.masks.join(format!("{name}.png"));
                                        tokio::fs::create_dir_all(target.parent().unwrap()).await?;
                                        tokio::fs::copy(mask, target).await?;
                                    }
                                    progress.update((i + 1) as u64, old_names.len() as u64);
                                }
                                drop(progress);
                                let mut records = frozen.context.panorama_records.clone();
                                if records.is_empty() {
                                    for (index, name) in old_names.iter().enumerate() {
                                        let source_index = Path::new(name)
                                            .file_stem()
                                            .and_then(|s| s.to_str())
                                            .and_then(|s| s.strip_prefix("frame_"))
                                            .and_then(|s| s.parse::<u64>().ok())
                                            .unwrap_or(index as u64);
                                        records.push(PanoramaFrameRecord {
                                            prepared_name: name.clone(),
                                            batch_id: "__source".into(),
                                            source_name: name.clone(),
                                            source_sha256: String::new(),
                                            orientation: Some(1),
                                            source_index,
                                            timestamp_seconds: None,
                                            pts: None,
                                        });
                                    }
                                }
                                checkpoint.sources.push(SourceCheckpoint {
                                    id: "__source".into(),
                                    identity: source_identity.clone(),
                                    camera_ids: original.cameras.iter().map(|c| c.id).collect(),
                                    camera_assignment: None,
                                    projection: model_projection(&original),
                                    names: old_names.clone(),
                                    originals: vec![],
                                    video: None,
                                    image_sequence: None,
                                    dimensions: vec![],
                                    working_dimensions: None,
                                    frames: None,
                                    panorama_records: records,
                                    adopted: 0,
                                    bridge_frames: vec![],
                                });
                                if let Some(r) =
                                    state.reshoot.as_mut().and_then(|r| r.recipe.as_mut())
                                {
                                    r.source_names = old_names;
                                    let seed_model =
                                        crate::pipeline::execution_plan::SnapshotModel {
                                            relative_path: PathBuf::from(
                                                "reshoot-snapshot-v3/colmap/sparse",
                                            ),
                                            sha256: frozen.manifest.content_identity.clone(),
                                            coordinate_frame_id: frozen
                                                .manifest
                                                .coordinate_frame_id
                                                .clone(),
                                            has_erp: frozen.manifest.has_erp,
                                            fixed_images: frozen
                                                .manifest
                                                .registered_images
                                                .iter()
                                                .map(|image| image.id)
                                                .collect(),
                                            fixed_cameras: frozen
                                                .manifest
                                                .cameras
                                                .iter()
                                                .map(|camera| camera.id)
                                                .collect(),
                                        };
                                    if frozen.source_ply.is_none() {
                                        r.actual.training =
                                            crate::pipeline::reshoot::TrainingMode::Cold;
                                    }
                                    checkpoint.execution_plan = Some(
                                        crate::pipeline::execution_plan::ExecutionPlan::reshoot(
                                            r, seed_model,
                                        ),
                                    );
                                }
                            }
                            for (index, item) in items.iter().enumerate() {
                                self.events.source(Some(&item.id));
                                if checkpoint.sources.iter().any(|s| {
                                    s.id == item.id
                                        && item.identity.as_deref() == Some(&s.identity)
                                        && s.names.iter().all(|n| {
                                            paths.frames.join(n).is_file()
                                                && paths.colmap_frames.join(n).is_file()
                                        })
                                }) {
                                    continue;
                                }
                                if self.process_manager.child_token().is_cancelled() {
                                    return Err(SplatError::Cancelled);
                                }
                                if let Some(saved) = checkpoint
                                    .recovery_sources
                                    .iter()
                                    .find(|source| source.id == item.id && source.video.is_some())
                                    .cloned()
                                {
                                    let source = self
                                        .restore_frozen_video_source(paths, item, &saved)
                                        .await?;
                                    checkpoint.sources.retain(|source| source.id != item.id);
                                    checkpoint.sources.push(source);
                                    state.inputs = Some(checkpoint.clone());
                                    manager.write_state(&paths.state, &state).await?;
                                    continue;
                                }
                                let a =
                                    inspect_material(&self.engines, item, metadata.quality, policy)
                                        .await?;
                                if item.identity.as_ref() != Some(&a.identity) {
                                    return Err(SplatError::Process("INPUT_CHANGED".into()));
                                }
                                let resolved = projection(item, &a)?;
                                let prefix = format!("b{}_s{index:04}_", paths.id.simple());
                                let batch = paths.work.join(format!("inputs/{index:04}"));
                                if batch.exists() {
                                    tokio::fs::rename(
                                        &batch,
                                        batch.with_extension(format!(
                                            "partial-{}",
                                            uuid::Uuid::new_v4()
                                        )),
                                    )
                                    .await?;
                                }
                                tokio::fs::create_dir_all(&batch).await?;
                                let mut source = SourceCheckpoint {
                                    id: item.id.clone(),
                                    identity: a.identity.clone(),
                                    camera_ids: vec![],
                                    camera_assignment: None,
                                    projection: resolved,
                                    names: vec![],
                                    originals: item.source.members()?,
                                    video: a.video.clone(),
                                    image_sequence: a.image_sequence.clone(),
                                    dimensions: a.dimensions.clone(),
                                    working_dimensions: None,
                                    frames: None,
                                    panorama_records: vec![],
                                    adopted: 0,
                                    bridge_frames: vec![],
                                };
                                if let Some(video) = a.video {
                                    let erp = resolved == VideoProjection::Equirectangular;
                                    let (w, h) = oriented_video_dimensions(&video);
                                    let target = if erp {
                                        let cap = match metadata.quality {
                                            Quality::Fast => 2048,
                                            Quality::Balanced => 3072,
                                            Quality::High => w,
                                        };
                                        let width = w.min(cap);
                                        (width, width / 2)
                                    } else {
                                        let r = policy.resolution(
                                            metadata.quality,
                                            acceleration.usable_gpu_total_memory_mb(),
                                            w,
                                            h,
                                            true,
                                        );
                                        (r.working_width, r.working_height)
                                    };
                                    let prepared = self
                                        .prepare_frames(
                                            item.source.path(),
                                            metadata.quality,
                                            &batch.join("frames"),
                                            &batch.join("colmap_frames"),
                                            &batch.join("masks"),
                                            Some(&paths.logs),
                                            true,
                                            Some(video),
                                            Some(target),
                                            erp,
                                            item.frame_removal_fps,
                                        )
                                        .await?;
                                    source.video = prepared.video.clone();
                                    source.working_dimensions =
                                        Some([prepared.working_width, prepared.working_height]);
                                    let mut frames = FrameState::from(&prepared.plan);
                                    frames.extracted_frames = Some(prepared.extracted_frames);
                                    frames.initial_extracted_frames = prepared.extracted_frames;
                                    frames.has_alpha = prepared.has_alpha;
                                    if item.frame_removal_fps.is_some()
                                        || policy.frame_sampling_fps.is_some()
                                        || policy.frame_sampling_ratio.is_some()
                                    {
                                        let total = prepared.video.as_ref().unwrap().total_frames;
                                        frames.rescue_max_frames = prepared
                                            .extracted_frames
                                            .saturating_add((prepared.extracted_frames / 5).min(
                                                total.saturating_sub(prepared.extracted_frames),
                                            ));
                                    }
                                    let times: Vec<crate::video::panorama::FrameTime> = if erp {
                                        serde_json::from_slice(
                                            &tokio::fs::read(
                                                batch.join("panorama-frame-times.json"),
                                            )
                                            .await?,
                                        )?
                                    } else {
                                        vec![]
                                    };
                                    let files = crate::video::list_images(&batch.join("frames"))?;
                                    for (i, path) in files.iter().enumerate() {
                                        let local = path.file_name().unwrap().to_string_lossy();
                                        let name = format!("{prefix}{local}");
                                        tokio::fs::copy(path, paths.frames.join(&name)).await?;
                                        tokio::fs::copy(
                                            batch.join("colmap_frames").join(local.as_ref()),
                                            paths.colmap_frames.join(&name),
                                        )
                                        .await?;
                                        let mask = batch.join("masks").join(format!("{local}.png"));
                                        if mask.is_file() {
                                            tokio::fs::copy(
                                                mask,
                                                paths.masks.join(format!("{name}.png")),
                                            )
                                            .await?;
                                        }
                                        let planned =
                                            prepared.plan.selected_frames.get(i).ok_or_else(
                                                || {
                                                    SplatError::Process(
                                                        "Missing source frame mapping".into(),
                                                    )
                                                },
                                            )?;
                                        let time = times.iter().find(|t| {
                                            t.source_frame_index == planned.source_frame_index
                                        });
                                        source.panorama_records.push(PanoramaFrameRecord {
                                            prepared_name: name.clone(),
                                            batch_id: item.id.clone(),
                                            source_name: item
                                                .source
                                                .path()
                                                .to_string_lossy()
                                                .into(),
                                            source_sha256: String::new(),
                                            orientation: Some(1),
                                            source_index: planned.source_frame_index,
                                            timestamp_seconds: Some(planned.timestamp_seconds),
                                            pts: time.map(|t| t.pts),
                                        });
                                        source.names.push(name);
                                    }
                                    source.frames = Some(frames);
                                } else {
                                    let files = item.source.members()?;
                                    let out = paths.frames.clone();
                                    let rgb = paths.colmap_frames.clone();
                                    let masks = paths.masks.clone();
                                    let cancel = self.process_manager.child_token();
                                    let erp = resolved == VideoProjection::Equirectangular;
                                    let quality = metadata.quality;
                                    let progress = metadata.reshoot.as_ref().map(|_| {
                                        super::reshoot_runner::ReshootProgress::new(
                                            &self.events,
                                            PipelineStage::ExtractingFrames,
                                            &cancel,
                                            "正在准备补拍画面",
                                            Some(files.len() as u64),
                                        )
                                    });
                                    let names =
                                        crate::presets::spawn_pipeline_blocking(move || {
                                            prepare_images(
                                                &files,
                                                &prefix,
                                                &out,
                                                &rgb,
                                                &masks,
                                                erp,
                                                quality,
                                                &cancel,
                                                progress.as_ref(),
                                            )
                                        })
                                        .await
                                        .map_err(|e| SplatError::Process(e.to_string()))??;
                                    for (i, name) in names.iter().enumerate() {
                                        source.panorama_records.push(PanoramaFrameRecord {
                                            prepared_name: name.clone(),
                                            batch_id: item.id.clone(),
                                            source_name: source.originals[i]
                                                .to_string_lossy()
                                                .into(),
                                            source_sha256: String::new(),
                                            orientation: Some(
                                                crate::video::image_projection::inspect(
                                                    &source.originals[i],
                                                )?
                                                .orientation,
                                            ),
                                            source_index: i as u64,
                                            timestamp_seconds: None,
                                            pts: None,
                                        });
                                    }
                                    source.names = names;
                                }
                                if item.source.identity()? != a.identity {
                                    return Err(SplatError::Process("INPUT_CHANGED".into()));
                                }
                                checkpoint.sources.retain(|saved| saved.id != source.id);
                                checkpoint.sources.push(source);
                                state.inputs = Some(checkpoint.clone());
                                let n = checkpoint
                                    .sources
                                    .iter()
                                    .map(|s| s.names.len() as u64)
                                    .sum();
                                state.frames = Some(FrameState {
                                    extracted_frames: Some(n),
                                    colmap_input_version: Some(COLMAP_INPUT_VERSION),
                                    ..Default::default()
                                });
                                manager.write_state(&paths.state, &state).await?;
                            }
                            checkpoint.reshoot_eligible = true;
                        }
                    }
                    checkpoint.request.check_identities()?;
                    checkpoint.prepared = true;
                    if let Some(r) = state.reshoot.as_mut().and_then(|r| r.recipe.as_mut()) {
                        r.new_names = checkpoint
                            .sources
                            .iter()
                            .filter(|s| s.id != "__source")
                            .flat_map(|s| s.names.clone())
                            .collect();
                        r.panorama_frames = checkpoint
                            .sources
                            .iter()
                            .flat_map(|s| s.panorama_records.clone())
                            .collect();
                    }
                    let n = checkpoint
                        .sources
                        .iter()
                        .map(|s| s.names.len() as u64)
                        .sum();
                    if let Some(reshoot) = state.reshoot.as_mut() {
                        reshoot.source_snapshot_complete = true;
                        reshoot.supplemental_frames_complete = true;
                        reshoot.source_image_count = checkpoint
                            .sources
                            .iter()
                            .filter(|s| s.id == "__source")
                            .map(|s| s.names.len() as u64)
                            .sum();
                        reshoot.reshoot_frame_count = n - reshoot.source_image_count;
                        reshoot.colmap_input_version = Some(COLMAP_INPUT_VERSION);
                        if let Some(provenance) = metadata.reshoot.as_mut() {
                            provenance.source_image_count = reshoot.source_image_count;
                            provenance.reshoot_frame_count = reshoot.reshoot_frame_count;
                        }
                    }
                    state.frames = Some(FrameState {
                        extracted_frames: Some(n),
                        initial_extracted_frames: n,
                        colmap_input_version: Some(COLMAP_INPUT_VERSION),
                        ..Default::default()
                    });
                    checkpoint.recovery_sources.clear();
                    state.inputs = Some(checkpoint.clone());
                    self.record_collection_artifacts(paths, &mut state).await?;
                    manager.write_state(&paths.state, &state).await?;
                }
                GenerationAction::ExtractFeatures => {
                    // New work has no pixels at the executor entry preflight.
                    // Re-run it now that the exact COLMAP groups exist, before
                    // either extracting features or trusting a saved DB.
                    state.inputs = Some(checkpoint.clone());
                    self.apply_collection_focal_preflight(manager, paths, &mut state)
                        .await?;
                    checkpoint = state.inputs.clone().ok_or_else(|| {
                        SplatError::Process("Missing input checkpoint after focal preflight".into())
                    })?;
                    self.collection_features(
                        manager,
                        paths,
                        &mut state,
                        &mut checkpoint,
                        acceleration,
                    )
                    .await?;
                }
                GenerationAction::MatchImages => {
                    self.collection_matches(
                        paths,
                        &checkpoint,
                        metadata.quality,
                        acceleration,
                        checkpoint.expanded_matching,
                    )
                    .await?;
                    state.matching_complete = true;
                    self.record_collection_artifacts(paths, &mut state).await?;
                    manager.write_state(&paths.state, &state).await?;
                }
                GenerationAction::Reconstruct => {
                    connection_validated = false;
                    rebuilt_work = true;
                    if !state.reconstruction_complete {
                        loop {
                            let base = paths.colmap.join("base-model");
                            let incremental = execution_plan_is_incremental(&checkpoint);
                            self.collection_mapper(
                                paths,
                                &checkpoint,
                                &state,
                                incremental.then_some(base.as_path()),
                                acceleration,
                            )
                            .await?;
                            let (selected, _) =
                                best_model(&sparse_attempt(paths, &state), names.len() as u64)?;
                            let mismatch =
                                incremental && !collection_coordinates_match(&base, &selected)?;
                            if mismatch {
                                if apply_execution_plan_fallback(
                                    &mut state,
                                    crate::pipeline::reshoot::AlgorithmFailure::CoordinateMismatch,
                                ) {
                                    checkpoint.execution_plan = state
                                        .inputs
                                        .as_ref()
                                        .and_then(|inputs| inputs.execution_plan.clone());
                                    prepare_collection_reconstruction_retry(
                                        &mut state,
                                        &mut checkpoint,
                                    );
                                    self.record_collection_artifacts(paths, &mut state).await?;
                                    manager.write_state(&paths.state, &state).await?;
                                    self.mark_automatic_retry();
                                    self.events.send(
                                        PipelineStage::Reconstructing,
                                        Some(PipelineEngine::System),
                                        EventKind::Log,
                                        EventLevel::Warning,
                                        None,
                                        true,
                                        "当前方案未能完成，正在自动改用更耗时的方案。",
                                        None,
                                        None,
                                        None,
                                    );
                                    continue;
                                }
                                return Err(SplatError::Process(
                                    "补拍画面未能注册到同一场景".into(),
                                ));
                            }
                            // The first model is a bridge seed. Cross-source expansion
                            // and reshoot fallback are evaluated only after the single
                            // bounded Bridge round.
                            read_model(&selected)?.write(&authority)?;
                            state.reconstruction_complete = true;
                            self.record_collection_artifacts(paths, &mut state).await?;
                            manager.write_state(&paths.state, &state).await?;
                            break;
                        }
                    }
                }
                GenerationAction::ReconcileReconstruction => {
                    let bridge_enabled = checkpoint
                        .execution_plan
                        .as_ref()
                        .is_none_or(|plan| plan.bridge.enabled && plan.bridge.max_joint_rounds > 0);
                    if bridge_enabled && !checkpoint.imported_reconstruction {
                        self.collection_bridge(
                            manager,
                            paths,
                            &mut state,
                            &mut checkpoint,
                            metadata.quality,
                            acceleration,
                        )
                        .await?;
                        names = checkpoint
                            .sources
                            .iter()
                            .flat_map(|s| s.names.clone())
                            .collect();
                    }
                    if !checkpoint.imported_reconstruction {
                        self.collection_reconcile_after_bridge(
                            manager,
                            paths,
                            &mut state,
                            &mut checkpoint,
                            metadata.quality,
                            acceleration,
                        )
                        .await?;
                    }
                    let effective_model = self
                        .collection_gps(
                            manager,
                            paths,
                            metadata,
                            &mut state,
                            &mut checkpoint,
                            &authority,
                        )
                        .await?;
                    let model = read_model(&effective_model)?;
                    let registered = model
                        .images
                        .iter()
                        .map(|i| i.pose.name.as_str())
                        .collect::<HashSet<_>>();
                    for source in &mut checkpoint.sources {
                        source.adopted = source
                            .names
                            .iter()
                            .filter(|n| registered.contains(n.as_str()))
                            .count() as u64;
                    }
                    checkpoint.source_adoption = checkpoint
                        .sources
                        .iter()
                        .filter(|source| source.id != "__source")
                        .map(|source| {
                            let planned_frames = source
                                .frames
                                .as_ref()
                                .map(|frames| {
                                    frames.initial_extracted_frames.max(frames.estimated_frames)
                                })
                                .or_else(|| {
                                    source
                                        .image_sequence
                                        .as_ref()
                                        .map(|images| images.image_count)
                                })
                                .unwrap_or(source.names.len() as u64);
                            crate::pipeline::inputs::SourceAdoption {
                                source_id: source.id.clone(),
                                planned_frames,
                                prepared_frames: source.names.len() as u64,
                                registered_frames: source.adopted,
                                adopted: source.adopted > 0,
                                omitted_reason: (source.adopted == 0)
                                    .then(|| "not_connected_to_selected_model".into()),
                            }
                        })
                        .collect();
                    if let Some(reshoot) = state.reshoot.as_mut() {
                        reshoot.registered_reshoot_count = checkpoint
                            .sources
                            .iter()
                            .filter(|s| s.id != "__source")
                            .map(|s| s.adopted)
                            .sum();
                        reshoot.incremental_matching_complete = true;
                        reshoot.incremental_reconstruction_complete = true;
                        if let Some(provenance) = metadata.reshoot.as_mut() {
                            provenance.registered_reshoot_count = reshoot.registered_reshoot_count;
                        }
                    }
                    checkpoint.unused = checkpoint
                        .source_adoption
                        .iter()
                        .filter(|source| !source.adopted)
                        .map(|source| source.source_id.clone())
                        .collect();
                    let warning = crate::pipeline::inputs::source_adoption_warning(&checkpoint);
                    if let Some(warning) = &warning {
                        self.events.send(
                            PipelineStage::ValidatingReconstruction,
                            Some(PipelineEngine::System),
                            EventKind::Log,
                            EventLevel::Warning,
                            None,
                            true,
                            warning,
                            None,
                            None,
                            None,
                        );
                    }
                    let report = ReconstructionValidator::validate_with_input_images(
                        names.len() as u64,
                        &effective_model,
                    )?;
                    self.record_reconstruction_metrics(
                        &report,
                        acceleration,
                        has_transparency(
                            &paths.frames,
                            &names,
                            &self.process_manager.child_token(),
                        )?,
                        metadata.reshoot.is_some(),
                    );
                    // Adoption is already known even if undistortion, face preparation or
                    // training fails/cancels later. Do not defer it until a successful dataset.
                    state.inputs = Some(checkpoint.clone());
                    state.stage = PipelineStage::ValidatingReconstruction;
                    manager.write_state(&paths.state, &state).await?;
                    manager.write_metadata(&paths.metadata, metadata).await?;
                    // This is the final effective model after the normal
                    // Bridge/candidate/GPS steps, not an intermediate mapper
                    // submodel. Save it before rejecting disconnected work so
                    // continuation repeats this guard without rerunning SfM.
                    self.record_reshoot_connection_model(
                        manager,
                        paths,
                        &mut state,
                        &effective_model,
                    )
                    .await?;
                    connection_validated = true;
                    reconstruction = Some((effective_model, report, warning));
                }
                GenerationAction::PrepareTraining => {
                    let (effective_model, _, _) =
                        reconstruction.as_ref().expect("validated reconstruction");
                    if !connection_validated {
                        self.record_reshoot_connection_model(
                            manager,
                            paths,
                            &mut state,
                            effective_model,
                        )
                        .await?;
                        connection_validated = true;
                    }
                    self.events.training_substage("trainingPreparation");
                    self.events
                        .stage(PipelineStage::TrainingSplats, 0., "正在准备训练图片");
                    let records = checkpoint
                        .sources
                        .iter()
                        .flat_map(|s| s.panorama_records.clone())
                        .collect::<Vec<_>>();
                    let dataset = self
                        .input_dataset(paths, effective_model, metadata.quality, &records, &names)
                        .await?;
                    let dataset_identity = read_training_dataset_identity(&dataset.path).await?;
                    bind_collection_training_recipe(&mut state, dataset_identity, rebuilt_work)?;
                    checkpoint.reference_image = Some(dataset.reference.clone());
                    state.inputs = Some(checkpoint.clone());
                    state.stage = PipelineStage::TrainingSplats;
                    self.events.training_substage("trainingCheckpoint");
                    self.events.send(
                        PipelineStage::TrainingSplats,
                        Some(PipelineEngine::System),
                        EventKind::Stage,
                        EventLevel::Info,
                        None,
                        true,
                        "正在保存生成检查点",
                        None,
                        None,
                        None,
                    );
                    self.record_collection_artifacts(paths, &mut state).await?;
                    manager.write_state(&paths.state, &state).await?;
                    training_dataset = Some(dataset);
                }
                GenerationAction::Train => {
                    let (model, report, _) =
                        reconstruction.as_ref().expect("validated reconstruction");
                    if !connection_validated {
                        self.record_reshoot_connection_model(manager, paths, &mut state, model)
                            .await?;
                        connection_validated = true;
                    }
                    let dataset = training_dataset
                        .as_ref()
                        .expect("validated training dataset");
                    let mut previous_training_failure: Option<SplatError> = None;
                    let candidate = loop {
                        let mut initial = report.points_3d;
                        if !state.brush_complete {
                            if let Some(inputs) = state.inputs.as_mut() {
                                inputs.init_sh_degree = None;
                            }
                        }
                        let warm_seed = (!state.brush_complete)
                            .then(|| {
                                state
                                    .inputs
                                    .as_ref()
                                    .and_then(|inputs| inputs.execution_plan.as_ref())
                                    .and_then(|plan| plan.gaussian_seed())
                                    .map(|(path, hash, frame)| {
                                        (path.clone(), hash.to_owned(), frame.to_owned())
                                    })
                            })
                            .flatten();
                        if let Some((relative_model, expected_sha256, expected_coordinate_frame)) =
                            &warm_seed
                        {
                            let recipe = state
                                .reshoot
                                .as_ref()
                                .and_then(|reshoot| reshoot.recipe.as_ref())
                                .ok_or_else(|| {
                                    SplatError::Process("Missing reshoot recipe".into())
                                })?;
                            self.events.training_substage("trainingPreparation");
                            self.events.stage(
                                PipelineStage::TrainingSplats,
                                0.,
                                "正在准备原模型训练副本",
                            );
                            let _progress = super::reshoot_runner::ReshootProgress::new(
                                &self.events,
                                PipelineStage::TrainingSplats,
                                &self.process_manager.child_token(),
                                "正在准备原模型训练副本",
                                None,
                            );
                            let model = paths.work.join(relative_model);
                            let (info, sh, hash) = super::reshoot_runner::validate_ply(
                                &model,
                                &self.process_manager.child_token(),
                            )
                            .await?;
                            if hash != *expected_sha256
                                || recipe.init_sha256.as_deref() != Some(expected_sha256.as_str())
                                || recipe.init_georeference.as_deref()
                                    != Some(expected_coordinate_frame.as_str())
                            {
                                return Err(SplatError::Process(
                        "Warm reshoot initialization no longer matches its immutable snapshot"
                            .into(),
                    ));
                            }
                            let init = dataset.path.join("init.ply");
                            let transform = state
                                .gps
                                .as_ref()
                                .filter(|g| g.status == "complete")
                                .and_then(|g| g.transform);
                            if let Some(transform) = transform {
                                let model = model.clone();
                                let output = init.clone();
                                let cancel = self.process_manager.child_token();
                                crate::presets::spawn_pipeline_blocking(move || {
                        crate::reconstruction::splat_transform::export_transformed_ply_to(
                            &model,
                            &output,
                            transform.engine_transform(),
                            Default::default(),
                            |_, _| {},
                            || {
                                if cancel.is_cancelled() {
                                    Err(SplatError::Cancelled)
                                } else {
                                    Ok(())
                                }
                            },
                        )
                    })
                    .await
                    .map_err(|e| SplatError::Process(e.to_string()))??;
                            } else {
                                super::reshoot_runner::copy_reshoot_file(
                                    &model,
                                    &init,
                                    &self.process_manager.child_token(),
                                )
                                .await?;
                            }
                            let (copy, copy_sh, copy_hash) = super::reshoot_runner::validate_ply(
                                &init,
                                &self.process_manager.child_token(),
                            )
                            .await?;
                            if copy.splat_count != info.splat_count
                                || sh != copy_sh
                                || (transform.is_none() && hash != copy_hash)
                            {
                                return Err(SplatError::Process(
                                    "Initialization snapshot changed".into(),
                                ));
                            }
                            initial = info.splat_count;
                            if let Some(inputs) = state.inputs.as_mut() {
                                inputs.init_sh_degree = Some(sh);
                            }
                        }
                        if warm_seed.is_none() {
                            let init = dataset.path.join("init.ply");
                            if init.exists() {
                                tokio::fs::rename(
                                    &init,
                                    dataset
                                        .path
                                        .join(format!("init.warm-{}.ply", uuid::Uuid::new_v4())),
                                )
                                .await?;
                            }
                        }
                        self.events.training_substage("trainingValidation");
                        self.events.send(
                            PipelineStage::TrainingSplats,
                            Some(PipelineEngine::System),
                            EventKind::Stage,
                            EventLevel::Info,
                            None,
                            true,
                            "正在校验训练图片和相机数据",
                            None,
                            None,
                            None,
                        );
                        let memory = acceleration.usable_gpu_total_memory_mb();
                        let resolution = state.resolution_plan.unwrap_or_else(|| {
                            policy.resolution(
                                metadata.quality,
                                memory,
                                dataset.max_edge,
                                dataset.max_edge,
                                false,
                            )
                        });
                        self.prepare_fine_splat_budget(
                            &mut state,
                            metadata.quality,
                            &dataset.path.join("sparse/0"),
                            &dataset.path.join("images"),
                            dataset.has_erp,
                            resolution.working_long_edge(),
                        )
                        .await?;
                        let saved_resolved = state.brush_training.resolved;
                        let mut resolved = if let Some(saved) = saved_resolved {
                            saved
                        } else if dataset.has_erp {
                            policy.panorama_training(
                                metadata.quality,
                                memory,
                                dataset.max_edge,
                                report.points_3d,
                                initial,
                            )?
                        } else {
                            policy.training_with_max_resolution(
                                metadata.quality,
                                memory,
                                &resolution,
                                report.points_3d,
                                initial,
                                state.brush_training.fine_splat_budget.as_ref(),
                                resolution.working_long_edge(),
                            )?
                        };
                        // Face resolution must never clamp ordinary views in a mixed dataset.
                        if saved_resolved.is_none() {
                            resolved.preset.max_resolution = resolution.working_long_edge();
                        }
                        if resolved
                            .preset
                            .max_splats
                            .is_some_and(|limit| u64::from(limit) < initial)
                        {
                            return Err(SplatError::Process("Saved Splat budget cannot contain the validated initialization without discarding points".into()));
                        }
                        state
                            .brush_training
                            .initial_resolved
                            .get_or_insert(resolved);
                        state.brush_training.resolved = Some(resolved);
                        state.brush_training.dataset_identity =
                            Some(read_training_dataset_identity(&dataset.path).await?);
                        if saved_resolved.is_none() {
                            state.resolution_plan = Some(resolution);
                        }
                        self.prepare_brush_observation(&state);
                        self.events.training_substage("trainingCheckpoint");
                        self.events.send(
                            PipelineStage::TrainingSplats,
                            Some(PipelineEngine::System),
                            EventKind::Stage,
                            EventLevel::Info,
                            None,
                            true,
                            "正在保存生成检查点",
                            None,
                            None,
                            None,
                        );
                        manager.write_state(&paths.state, &state).await?;
                        self.events.stage(
                            PipelineStage::TrainingSplats,
                            1.,
                            "训练图片准备完成，即将启动训练",
                        );
                        self.events.training_substage("training");
                        let mut attempt = paths.clone();
                        let training_attempt = state
                            .reshoot
                            .as_ref()
                            .and_then(|r| r.recipe.as_ref())
                            .map_or(0, |r| r.training_fallbacks);
                        attempt.brush = paths
                            .brush
                            .join(format!("collection-training-{training_attempt}"));
                        let runtime_samples = catalog::runtime_samples().await;
                        let estimate_plan = FramePlan {
                            estimated_frames: report.input_images,
                            ..Default::default()
                        };
                        let estimate = if let [source] = checkpoint.sources.as_slice() {
                            if let Some(video) = &source.video {
                                crate::pipeline::estimate::estimate_calibrated_brush_stage_ms(
                                    video,
                                    &estimate_plan,
                                    metadata.quality,
                                    &runtime_samples,
                                    Some(&resolved),
                                    state.resolution_policy_version,
                                    dataset.max_edge,
                                )
                            } else {
                                crate::pipeline::estimate::estimate_calibrated_brush_stage_ms_for_images(
                        report.input_images, dataset.max_edge, &estimate_plan, metadata.quality,
                        &runtime_samples, Some(&resolved), state.resolution_policy_version, dataset.max_edge,
                    )
                            }
                        } else {
                            crate::pipeline::estimate::estimate_calibrated_brush_stage_ms_for_images(
                                report.input_images,
                                dataset.max_edge,
                                &estimate_plan,
                                metadata.quality,
                                &runtime_samples,
                                Some(&resolved),
                                state.resolution_policy_version,
                                dataset.max_edge,
                            )
                        };
                        let outcome = if state.brush_complete {
                            Ok(reusable_brush_candidate(&attempt.brush).ok_or_else(|| {
                                SplatError::Process("Missing completed Brush output".into())
                            })?)
                        } else {
                            self.train_brush_with_oom_retry(
                                manager,
                                &attempt,
                                &mut state,
                                metadata.quality,
                                acceleration,
                                &dataset.path,
                                initial,
                                resolved,
                                estimate,
                            )
                            .await
                        };
                        let (validation, invalid) = match outcome {
                            Ok(path) => {
                                let validation = super::reshoot_runner::validate_ply(
                                    &path,
                                    &self.process_manager.child_token(),
                                )
                                .await
                                .map(|_| path);
                                let invalid = validation.as_ref().err().is_some_and(|e| {
                                    !matches!(e, SplatError::Io(_) | SplatError::Cancelled)
                                });
                                (validation, invalid)
                            }
                            Err(error) => {
                                let invalid =
                                    crate::pipeline::reshoot::explicit_numeric_divergence(&error);
                                (Err(error), invalid)
                            }
                        };
                        match validation {
                            Ok(path) => {
                                state.brush_complete = true;
                                self.record_collection_artifacts(paths, &mut state).await?;
                                manager.write_state(&paths.state, &state).await?;
                                break path;
                            }
                            Err(error) => {
                                let fallback = invalid
                        && apply_execution_plan_fallback(
                            &mut state,
                            crate::pipeline::reshoot::AlgorithmFailure::InvalidWarmResult,
                        );
                                if !fallback {
                                    return Err(
                                        if let Some(previous) = previous_training_failure {
                                            error.with_previous_attempt(previous)
                                        } else {
                                            error
                                        },
                                    );
                                }
                                previous_training_failure = Some(error);
                                state.brush_complete = false;
                                state.brush_training.resolved = None;
                                state.brush_training.dataset_identity = None;
                                state.validated_output = None;
                                manager.write_state(&paths.state, &state).await?;
                                self.mark_automatic_retry();
                                self.events.stage(
                                    PipelineStage::TrainingSplats,
                                    0.,
                                    "当前方案未能完成，正在自动改用更耗时的方案。",
                                );
                            }
                        }
                    };
                    let ply = inspect_gaussian_ply(&candidate)?;
                    state.validated_output = Some(ProjectOutput {
                        final_ply: paths.project.join("final.ply"),
                        file_size: ply.file_size,
                        splat_count: ply.splat_count,
                        input_images: report.input_images,
                        registered_images: report.registered_images,
                        registered_ratio: report.registered_ratio,
                        points_3d: report.points_3d,
                    });
                    manager.write_state(&paths.state, &state).await?;
                }
                GenerationAction::Publish => {
                    let candidate = completed_candidate.expect("validated completed candidate");
                    let output = match state.validated_output.clone() {
                        Some(output) => output,
                        None => {
                            let model =
                                if state.gps.as_ref().is_some_and(|g| g.status == "complete") {
                                    paths.colmap.join("gps-v1/model")
                                } else {
                                    authority.clone()
                                };
                            let report = ReconstructionValidator::validate_with_input_images(
                                names.len() as u64,
                                &model,
                            )?;
                            let ply = inspect_gaussian_ply(&candidate)?;
                            ProjectOutput {
                                final_ply: paths.project.join("final.ply"),
                                file_size: ply.file_size,
                                splat_count: ply.splat_count,
                                input_images: report.input_images,
                                registered_images: report.registered_images,
                                registered_ratio: report.registered_ratio,
                                points_3d: report.points_3d,
                            }
                        }
                    };
                    let warning = crate::pipeline::inputs::source_adoption_warning(&checkpoint);
                    let training_attempt = state
                        .reshoot
                        .as_ref()
                        .and_then(|r| r.recipe.as_ref())
                        .map_or(0, |r| r.training_fallbacks);
                    return self
                        .publish_work_order(
                            manager,
                            paths,
                            metadata,
                            &mut state,
                            &candidate,
                            output,
                            format!("collection-training-{training_attempt}"),
                            warning,
                        )
                        .await;
                }
            }
        }
    }

    async fn collection_gps(
        &self,
        manager: &ProjectManager,
        paths: &ProjectPaths,
        metadata: &ProjectMetadata,
        state: &mut PipelineStateFile,
        c: &mut InputCheckpoint,
        visual: &Path,
    ) -> Result<PathBuf> {
        use crate::reconstruction::gps;
        if c.imported_reconstruction {
            return Ok(visual.into());
        }
        let local = paths.work.join("gps/manifest.json");
        let mut origin = None;
        let records = if state.gps.as_ref().is_some_and(|g| g.status != "pending")
            && local.is_file()
        {
            let manifest: gps::Manifest = serde_json::from_slice(&tokio::fs::read(local).await?)?;
            if state.gps.as_ref().is_some_and(|g| g.reference_valid) && metadata.reshoot.is_some() {
                origin = manifest.origin;
            }
            manifest.records
        } else {
            let mut candidates = vec![];
            let mut records = vec![];
            if metadata.reshoot.is_some() {
                let snapshot = paths.work.join("reshoot-snapshot-v3");
                let old = snapshot.join("gps/manifest.json");
                let saved = tokio::fs::read(snapshot.join("source/context.json"))
                    .await
                    .ok()
                    .and_then(|bytes| {
                        serde_json::from_slice::<FrozenReshootSourceContext>(&bytes).ok()
                    });
                if let (Some(saved), Ok(bytes)) = (saved, tokio::fs::read(old).await) {
                    if let Ok(manifest) = serde_json::from_slice::<gps::Manifest>(&bytes) {
                        if saved.gps.as_ref().is_some_and(|g| {
                            g.fingerprint == gps::fingerprint(&manifest.records).unwrap_or_default()
                                && g.manifest_identity.as_ref()
                                    == gps::manifest_identity(&manifest).ok().as_ref()
                        }) {
                            if saved.gps.as_ref().is_some_and(|g| g.reference_valid) {
                                origin = manifest.origin;
                            }
                            records = manifest.records;
                        }
                    }
                }
                if records.is_empty() {
                    if let Some(source) = c.sources.iter().find(|s| s.id == "__source") {
                        candidates.extend(
                            source
                                .names
                                .iter()
                                .map(|n| (n.clone(), paths.frames.join(n))),
                        );
                    }
                }
            }
            for source in &c.sources {
                if source.video.is_none() && source.id != "__source" {
                    for original in &source.originals {
                        self.authorize_external_path(original)?;
                    }
                    candidates.extend(
                        source
                            .names
                            .iter()
                            .cloned()
                            .zip(source.originals.iter().cloned()),
                    );
                }
            }
            let cancel = self.process_manager.child_token();
            records.extend(
                crate::presets::spawn_pipeline_blocking(move || gps::collect(&candidates, &cancel))
                    .await
                    .map_err(|e| SplatError::Process(e.to_string()))??,
            );
            records
        };
        let record_names = records
            .iter()
            .map(|record| record.name.clone())
            .collect::<HashSet<_>>();
        state.gps.get_or_insert_with(gps::Checkpoint::pending);
        // Only an Incremental execution plan is allowed to anchor GPS BA to
        // the frozen source. A Full fallback deliberately releases old poses
        // and intrinsics, even though the task still has reshoot provenance.
        let base = collection_gps_base_model(paths, c);
        let fixed = paths.colmap.join("gps-fixed-cameras.txt");
        if let Some(base) = &base {
            tokio::fs::write(
                &fixed,
                crate::reconstruction::colmap_model::read_cameras(base)?
                    .iter()
                    .map(|c| c.id.to_string())
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
            .await?;
        }
        let result = self
            .apply_gps(
                manager,
                paths,
                state,
                records,
                origin,
                visual,
                base.as_deref(),
                base.as_ref().map(|_| fixed.as_path()),
            )
            .await?
            .unwrap_or_else(|| visual.into());
        let registered = crate::reconstruction::colmap_model::read_registered_images(&result)?
            .into_iter()
            .map(|image| image.name)
            .collect::<HashSet<_>>();
        c.gps_assistance = crate::pipeline::inputs::GpsAssistance::participating(
            c,
            &record_names,
            &registered,
            state.gps.as_ref().map(|g| g.status.as_str()),
        );
        state.inputs = Some(c.clone());
        manager.write_state(&paths.state, state).await?;
        Ok(result)
    }
    /// Policy checks precede feature-cache reuse, not only feature execution.
    /// The pending policy and shared archive barrier survive every interruption.
    pub(super) async fn apply_collection_focal_preflight(
        &self,
        manager: &ProjectManager,
        paths: &ProjectPaths,
        state: &mut PipelineStateFile,
    ) -> Result<()> {
        let pending = state
            .inputs
            .as_ref()
            .and_then(|checkpoint| checkpoint.temporary_png_focal_pending_records.clone());
        let recovering = pending.is_some();
        if let Some(checkpoint) = state.inputs.as_mut() {
            checkpoint.temporary_png_focal_active_records = None;
        }
        let records = match pending {
            Some(records) => records,
            None => match crate::pipeline::focal_preflight::inspect_collection_focal(
                &self.engines.colmap,
                &self.process_manager,
                paths,
                state,
            )
            .await?
            {
                Some(records) => records,
                None => return Ok(()),
            },
        };
        if state.inputs.as_ref().is_some_and(|checkpoint| {
            checkpoint.temporary_png_focal_pending_records.is_none()
                && checkpoint.temporary_png_focal_records.as_ref() == Some(&records)
        }) {
            state
                .inputs
                .as_mut()
                .unwrap()
                .temporary_png_focal_active_records = Some(records);
            return Ok(());
        }
        self.persist_collection_focal_update(manager, paths, state, records)
            .await?;
        if recovering {
            // Reconfirm runtime applicability after an interrupted archive
            // rebuilt its local base; saved audit records are not new approval.
            let active = crate::pipeline::focal_preflight::inspect_collection_focal(
                &self.engines.colmap,
                &self.process_manager,
                paths,
                state,
            )
            .await?;
            state
                .inputs
                .as_mut()
                .unwrap()
                .temporary_png_focal_active_records = active;
        }
        Ok(())
    }

    async fn persist_collection_focal_update(
        &self,
        manager: &ProjectManager,
        paths: &ProjectPaths,
        state: &mut PipelineStateFile,
        records: Vec<crate::pipeline::focal_preflight::FocalRecord>,
    ) -> Result<()> {
        if crate::pipeline::focal_preflight::begin_collection_focal_update(state, &records) {
            // Commit both the requested policy and prohibition on reuse before
            // a single cached byte is moved. Normal image/mask assets stay put.
            self.retry_preparation_step(
                paths,
                RetryPreparationOperation::WriteRetryReuseBarrier,
                "project_state",
                None,
                manager.write_state_for_preparation(&paths.state, state),
            )
            .await?;
        }
        self.prepare_continuation(manager, paths, state).await?;
        if self.process_manager.child_token().is_cancelled() {
            return Err(SplatError::Cancelled);
        }
        tokio::fs::create_dir_all(&paths.colmap).await?;
        tokio::fs::create_dir_all(&paths.brush).await?;
        if state.inputs.as_ref().is_some_and(execution_plan_is_reshoot) {
            // `colmap` also held derived base copies. Recreate them from the
            // verified local snapshot, never from the now-optional old project.
            let snapshot_root = paths.work.join("reshoot-snapshot-v3");
            let expected = state
                .reshoot
                .as_ref()
                .and_then(|reshoot| reshoot.snapshot_v3_identity.clone());
            let snapshot = snapshot_root.clone();
            let model = paths.colmap.join("base-model");
            crate::presets::spawn_pipeline_blocking(move || -> Result<()> {
                let manifest = crate::pipeline::reshoot_snapshot::verify_snapshot(&snapshot)?;
                if expected
                    .as_ref()
                    .is_some_and(|identity| identity != &manifest.content_identity)
                {
                    return Err(SplatError::Process(
                        "Reshoot source snapshot identity changed".into(),
                    ));
                }
                read_model(&snapshot.join("colmap/sparse"))?.write(&model)
            })
            .await
            .map_err(|error| SplatError::Process(error.to_string()))??;
            super::reshoot_runner::copy_reshoot_file(
                &snapshot_root.join("colmap/database.db"),
                &paths.colmap.join("base-database.db"),
                &self.process_manager.child_token(),
            )
            .await?;
            let paths = paths.clone();
            let mut restored = state.clone();
            let cancel = self.process_manager.child_token();
            restored =
                crate::presets::spawn_pipeline_blocking(move || -> Result<PipelineStateFile> {
                    crate::pipeline::artifacts::rebind_restored_focal_base(
                        &paths,
                        &mut restored,
                        &cancel,
                    )?;
                    Ok(restored)
                })
                .await
                .map_err(|error| SplatError::Process(error.to_string()))??;
            *state = restored;
        }
        let checkpoint = state.inputs.as_mut().ok_or_else(|| {
            SplatError::Process("Missing prepared inputs for focal policy".into())
        })?;
        checkpoint.temporary_png_focal_records = Some(records.clone());
        checkpoint.temporary_png_focal_pending_records = None;
        checkpoint.temporary_png_focal_active_records = Some(records.clone());
        self.retry_preparation_step(
            paths,
            RetryPreparationOperation::WriteRetryArchiveProgress,
            "project_state",
            None,
            manager.write_state_for_preparation(&paths.state, state),
        )
        .await?;
        for record in records {
            self.events.send(
                PipelineStage::Created,
                Some(PipelineEngine::System),
                EventKind::Log,
                EventLevel::Info,
                None,
                true,
                format!("{} cacheInvalidated=true", record.log_summary()),
                None,
                None,
                None,
            );
        }
        Ok(())
    }

    async fn collection_features(
        &self,
        manager: &ProjectManager,
        paths: &ProjectPaths,
        state: &mut PipelineStateFile,
        c: &mut InputCheckpoint,
        accel: &crate::engines::ColmapAccelerationStatus,
    ) -> Result<()> {
        if state.features_complete {
            return Ok(());
        }
        self.events
            .stage(PipelineStage::ExtractingFeatures, 0., "正在提取画面信息");
        let db = paths.colmap.join("database.db");
        if db.exists() {
            tokio::fs::rename(
                &db,
                paths
                    .colmap
                    .join(format!("incomplete-{}.db", uuid::Uuid::new_v4())),
            )
            .await?;
        }
        if execution_plan_is_reshoot(c) {
            tokio::fs::copy(paths.colmap.join("base-database.db"), &db).await?;
            super::reshoot_runner::sync_refined_cameras(&db, &paths.colmap.join("base-model"))?;
        }
        let preset = state.preset.preset();
        let reshoot_plan = execution_plan_is_reshoot(c);
        let groupings = c
            .sources
            .iter()
            .map(|source| {
                crate::pipeline::focal_preflight::collection_feature_groups(paths, c, source)
            })
            .collect::<Result<Vec<_>>>()?;
        let focal_records = c
            .temporary_png_focal_active_records
            .clone()
            .unwrap_or_default();
        let ordinary_image_sources = match &c.request {
            GenerationInputs::Materials { items } => items
                .iter()
                .filter(|item| {
                    matches!(
                        item.source,
                        InputSource::ImageFolder { .. } | InputSource::Images { .. }
                    )
                })
                .map(|item| item.id.clone())
                .collect::<HashSet<_>>(),
            GenerationInputs::Colmap { .. } => HashSet::new(),
        };
        for (si, (s, grouping)) in c.sources.iter_mut().zip(groupings).enumerate() {
            if s.id == "__source" {
                continue;
            }
            self.events.source(Some(&s.id));
            let reused = grouping.existing_camera_id;
            let groups = grouping.groups;
            s.camera_assignment = Some(crate::pipeline::reshoot::CameraAssignment {
                version: 1,
                reused_camera_id: reused,
            });
            if reshoot_plan {
                self.events.send(PipelineStage::ExtractingFeatures,Some(PipelineEngine::System),EventKind::Log,EventLevel::Info,None,true,if reused.is_some(){"补拍相机设置：沿用原设置"}else if matches!(&c.request,GenerationInputs::Materials{items} if items.iter().any(|i|i.id==s.id&&i.camera_relation==Some(crate::pipeline::reshoot::CameraRelation::Same))){"补拍相机设置：条件不兼容，独立处理"}else{"补拍相机设置：独立处理"},None,None,None);
            }
            for (gi, group) in groups.iter().enumerate() {
                let list = paths.colmap.join(format!("source-{si}-group-{gi}.txt"));
                write_image_list(&list, group).await?;
                let dims = if s.projection == VideoProjection::Equirectangular {
                    let h = crate::video::image_projection::inspect(
                        &paths.colmap_frames.join(&group[0]),
                    )?;
                    Some((h.width, h.height))
                } else {
                    None
                };
                let ordinary_images =
                    ordinary_image_sources.contains(&s.id) && s.video.is_none() && dims.is_none();
                let camera_params = if ordinary_images && reused.is_none() {
                    focal_records
                        .iter()
                        .find(|record| record.source_id == s.id && record.group_index == gi)
                        .map(crate::pipeline::focal_preflight::FocalRecord::camera_params)
                } else {
                    None
                };
                let masks = feature_masks_for_group(
                    paths,
                    group,
                    source_requires_feature_masks(s),
                    self.process_manager.child_token(),
                )
                .await?;
                colmap::extract_preflighted_group(
                    &self.engines.colmap,
                    &db,
                    colmap::ReshootFeatureOptions {
                        images: &paths.colmap_frames,
                        list: &list,
                        masks: masks.as_deref(),
                        panorama_dimensions: dims,
                        existing_camera_id: reused,
                        camera_params: camera_params.as_deref(),
                        ordinary_images,
                        max_size: preset.planned_sfm_max_image_size,
                        max_features: preset.planned_sfm_max_features,
                    },
                    paths.logs.join("colmap.log"),
                    &self.process_manager,
                    Some(self.process_observer(
                        PipelineStage::ExtractingFeatures,
                        PipelineEngine::Colmap,
                        Some(group.len() as u64),
                        ObserverMode::BracketProgress,
                    )),
                    accel.gpu_index(),
                )
                .await?;
            }
        }
        let connection =
            rusqlite::Connection::open_with_flags(&db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|e| SplatError::Process(e.to_string()))?;
        for source in &mut c.sources {
            let mut cameras = std::collections::BTreeSet::new();
            for name in &source.names {
                let camera: u32 = connection
                    .query_row(
                        "SELECT camera_id FROM images WHERE name=?1",
                        [name],
                        |row| row.get(0),
                    )
                    .map_err(|e| SplatError::Process(e.to_string()))?;
                cameras.insert(camera);
            }
            source.camera_ids = cameras.into_iter().collect();
        }
        state.inputs = Some(c.clone());
        state.features_complete = true;
        self.record_collection_artifacts(paths, state).await?;
        manager.write_state(&paths.state, state).await?;
        Ok(())
    }
    async fn collection_matches(
        &self,
        paths: &ProjectPaths,
        c: &InputCheckpoint,
        quality: Quality,
        accel: &crate::engines::ColmapAccelerationStatus,
        expanded: bool,
    ) -> Result<()> {
        self.events.source(None);
        use std::io::Write;
        self.events
            .stage(PipelineStage::Matching, 0., "正在匹配图像");
        let db = paths.colmap.join("database.db");
        let path = paths.colmap.join("input-pairs.txt");
        let mut pairs = std::io::BufWriter::new(std::fs::File::create(&path)?);
        let items = match &c.request {
            GenerationInputs::Materials { items } => items,
            _ => return Ok(()),
        };
        for (i, a) in c.sources.iter().enumerate() {
            if a.id == "__source" {
                continue;
            }
            let sequential = a.video.is_some()
                || items
                    .iter()
                    .find(|item| item.id == a.id)
                    .is_some_and(|item| {
                        item.image_matching_strategy
                            == crate::project::ImageMatchingStrategy::Sequential
                    });
            for (index, name) in a.names.iter().enumerate() {
                for other in
                    a.names
                        .iter()
                        .skip(index + 1)
                        .take(if sequential { 10 } else { usize::MAX })
                {
                    writeln!(pairs, "{name} {other}")?;
                }
            }
            for b in c.sources.iter().take(i) {
                if a.names.len().saturating_mul(b.names.len()) <= 20_000 {
                    for x in &a.names {
                        for y in &b.names {
                            writeln!(pairs, "{x} {y}")?;
                        }
                    }
                } else {
                    let matches = colmap::retrieve_reshoot_pairs(
                        &self.engines.colmap,
                        &db,
                        &b.names,
                        &a.names,
                        if expanded { 60 } else { 30 },
                        paths.logs.join("colmap.log"),
                        &self.process_manager,
                        Some(self.process_observer(
                            PipelineStage::Matching,
                            PipelineEngine::Colmap,
                            None,
                            ObserverMode::Matching,
                        )),
                    )
                    .await?;
                    for (x, y) in matches {
                        writeln!(pairs, "{x} {y}")?;
                    }
                }
            }
            if sequential && quality.loop_detection_enabled(true) {
                let list = paths.colmap.join(format!("loop-{i}.txt"));
                write_image_list(&list, &a.names).await?;
                colmap::match_reshoot_candidates(
                    &self.engines.colmap,
                    &db,
                    &list,
                    50,
                    paths.logs.join("colmap.log"),
                    &self.process_manager,
                    Some(self.process_observer(
                        PipelineStage::Matching,
                        PipelineEngine::Colmap,
                        None,
                        ObserverMode::Matching,
                    )),
                    accel.gpu_index(),
                )
                .await?;
            }
        }
        pairs.flush()?;
        drop(pairs);
        colmap::match_pairs(
            &self.engines.colmap,
            &db,
            &path,
            paths.logs.join("colmap.log"),
            &self.process_manager,
            Some(self.process_observer(
                PipelineStage::Matching,
                PipelineEngine::Colmap,
                None,
                ObserverMode::Matching,
            )),
            accel.gpu_index(),
        )
        .await
    }
    async fn collection_reconcile_after_bridge(
        &self,
        manager: &ProjectManager,
        paths: &ProjectPaths,
        state: &mut PipelineStateFile,
        checkpoint: &mut InputCheckpoint,
        quality: Quality,
        acceleration: &crate::engines::ColmapAccelerationStatus,
    ) -> Result<()> {
        let authority = paths.colmap.join("authoritative");
        let base = paths.colmap.join("base-model");
        let input_count = checkpoint
            .sources
            .iter()
            .map(|source| source.names.len() as u64)
            .sum();
        let mut rerun_mapper = false;
        loop {
            if rerun_mapper {
                let incremental = execution_plan_is_incremental(checkpoint);
                self.collection_mapper(
                    paths,
                    checkpoint,
                    state,
                    incremental.then_some(base.as_path()),
                    acceleration,
                )
                .await?;
                let (selected, _) = best_model(&sparse_attempt(paths, state), input_count)?;
                if incremental && !collection_coordinates_match(&base, &selected)? {
                    let fallback = apply_execution_plan_fallback(
                        state,
                        crate::pipeline::reshoot::AlgorithmFailure::CoordinateMismatch,
                    );
                    if fallback {
                        checkpoint.execution_plan = state
                            .inputs
                            .as_ref()
                            .and_then(|inputs| inputs.execution_plan.clone());
                        prepare_collection_reconstruction_retry(state, checkpoint);
                        self.record_collection_artifacts(paths, state).await?;
                        manager.write_state(&paths.state, state).await?;
                        self.mark_automatic_retry();
                        rerun_mapper = true;
                        continue;
                    }
                    return Err(SplatError::Process(
                        "Reshoot reconstruction changed the frozen source coordinate frame".into(),
                    ));
                }
                read_model(&selected)?.write(&authority)?;
                state.reconstruction_complete = true;
                state.inputs = Some(checkpoint.clone());
                self.record_collection_artifacts(paths, state).await?;
                manager.write_state(&paths.state, state).await?;
            }

            let registered = read_registered_images(&authority)?;
            let registered = registered
                .iter()
                .map(|image| image.name.as_str())
                .collect::<HashSet<_>>();
            let missing_sources = checkpoint
                .sources
                .iter()
                .filter(|source| source.id != "__source" && !source.names.is_empty())
                .filter(|source| {
                    !source
                        .names
                        .iter()
                        .any(|name| registered.contains(name.as_str()))
                })
                .count();
            let new_source_count = checkpoint
                .sources
                .iter()
                .filter(|source| source.id != "__source" && !source.names.is_empty())
                .count();

            // The normal 30-candidate pass and video Bridge have both run. One
            // bounded 60-candidate expansion is now allowed for disconnected
            // sources without inventing another coordinate system.
            if missing_sources > 0 && !checkpoint.expanded_matching {
                // Make the mapper result non-reusable before mutating the
                // match database. If matching is interrupted, resume may run
                // the mapper once against the partial (still valid) database,
                // then this idempotent expansion is attempted again. The
                // completion flag is persisted only after COLMAP succeeds.
                state.matching_complete = false;
                state.reconstruction_complete = false;
                state.brush_complete = false;
                state.validated_output = None;
                state.gps = None;
                state.stage = PipelineStage::Matching;
                state.inputs = Some(checkpoint.clone());
                self.record_collection_artifacts(paths, state).await?;
                manager.write_state(&paths.state, state).await?;
                self.collection_matches(paths, checkpoint, quality, acceleration, true)
                    .await?;
                checkpoint.expanded_matching = true;
                state.matching_complete = true;
                state.stage = PipelineStage::Reconstructing;
                state.inputs = Some(checkpoint.clone());
                self.record_collection_artifacts(paths, state).await?;
                manager.write_state(&paths.state, state).await?;
                rerun_mapper = true;
                continue;
            }

            let require_new = checkpoint
                .execution_plan
                .as_ref()
                .map_or(state.reshoot.is_some(), |plan| {
                    plan.success.require_any_new_source
                });
            let allow_partial = checkpoint
                .execution_plan
                .as_ref()
                .is_none_or(|plan| plan.success.allow_partial_new_sources);
            if require_new && new_source_count > 0 && missing_sources == new_source_count {
                let fallback = apply_execution_plan_fallback(
                    state,
                    crate::pipeline::reshoot::AlgorithmFailure::NoNewRegistrations,
                );
                if fallback {
                    checkpoint.execution_plan = state
                        .inputs
                        .as_ref()
                        .and_then(|inputs| inputs.execution_plan.clone());
                    prepare_collection_reconstruction_retry(state, checkpoint);
                    self.record_collection_artifacts(paths, state).await?;
                    manager.write_state(&paths.state, state).await?;
                    self.mark_automatic_retry();
                    rerun_mapper = true;
                    continue;
                }
                return Err(SplatError::Process(
                    "补拍画面没有成功加入原项目。请确保新旧素材有足够重叠。".into(),
                ));
            }
            if missing_sources > 0 && !allow_partial {
                return Err(SplatError::Process(
                    "补拍画面没有成功加入原项目。请确保新旧素材有足够重叠。".into(),
                ));
            }
            return Ok(());
        }
    }

    async fn collection_mapper(
        &self,
        paths: &ProjectPaths,
        c: &InputCheckpoint,
        state: &PipelineStateFile,
        input: Option<&Path>,
        accel: &crate::engines::ColmapAccelerationStatus,
    ) -> Result<()> {
        self.events.source(None);
        let names = c
            .sources
            .iter()
            .flat_map(|s| s.names.clone())
            .collect::<Vec<_>>();
        let list = paths.colmap.join("mapper-images.txt");
        write_image_list(&list, &names).await?;
        let transparent =
            has_transparency(&paths.frames, &names, &self.process_manager.child_token())?;
        let output = sparse_attempt(paths, state);
        if output.exists() {
            tokio::fs::rename(
                &output,
                output.with_extension(format!("previous-{}", uuid::Uuid::new_v4())),
            )
            .await?;
        }
        self.events
            .stage(PipelineStage::Reconstructing, 0., "正在还原拍摄场景");
        let erp = c
            .sources
            .iter()
            .any(|s| s.projection == VideoProjection::Equirectangular)
            || c.execution_plan
                .as_ref()
                .is_some_and(|plan| plan.source_has_erp);
        if !erp && !execution_plan_is_reshoot(c) {
            return if let Some(base) = input {
                colmap::map_from_existing(
                    &self.engines.colmap,
                    &paths.colmap.join("database.db"),
                    &paths.colmap_frames,
                    base,
                    &sparse_attempt(paths, state),
                    true,
                    transparent,
                    paths.logs.join("colmap.log"),
                    &self.process_manager,
                    Some(self.process_observer(
                        PipelineStage::Reconstructing,
                        PipelineEngine::Colmap,
                        Some(names.len() as u64),
                        ObserverMode::Mapper,
                    )),
                    accel.gpu_index(),
                )
                .await
            } else {
                colmap::map(
                    &self.engines.colmap,
                    &paths.colmap.join("database.db"),
                    &paths.colmap_frames,
                    &sparse_attempt(paths, state),
                    transparent,
                    true,
                    paths.logs.join("colmap.log"),
                    &self.process_manager,
                    Some(self.process_observer(
                        PipelineStage::Reconstructing,
                        PipelineEngine::Colmap,
                        Some(names.len() as u64),
                        ObserverMode::Mapper,
                    )),
                    accel.gpu_index(),
                )
                .await
            };
        }
        let fixed = paths.colmap.join("fixed-cameras.txt");
        if let Some(base) = input {
            tokio::fs::write(
                &fixed,
                crate::reconstruction::colmap_model::read_cameras(base)?
                    .iter()
                    .map(|c| c.id.to_string())
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
            .await?;
        }
        colmap::map_reshoot(
            &self.engines.colmap,
            &paths.colmap.join("database.db"),
            colmap::ReshootMapperOptions {
                images: &paths.colmap_frames,
                input,
                output: &sparse_attempt(paths, state),
                image_list: &list,
                fixed_cameras: input.map(|_| fixed.as_path()),
                panorama: erp,
                allow_two_view_tracks: transparent,
            },
            colmap::MapperBaPolicy {
                policy_version: state.ba_policy_version,
                automatic_optimization: true,
                gpu_index: accel.gpu_index(),
                panorama: erp,
                ..Default::default()
            },
            paths.logs.join("colmap.log"),
            &self.process_manager,
            Some(self.process_observer(
                PipelineStage::Reconstructing,
                PipelineEngine::Colmap,
                Some(names.len() as u64),
                ObserverMode::Mapper,
            )),
        )
        .await
    }
}

fn sparse_attempt(paths: &ProjectPaths, state: &PipelineStateFile) -> PathBuf {
    paths.colmap.join(format!(
        "sparse-attempt-{}",
        state
            .inputs
            .as_ref()
            .and_then(|inputs| inputs.execution_plan.as_ref())
            .map(|plan| plan.fallback.reconstruction_fallbacks_used)
            .or_else(|| {
                state
                    .reshoot
                    .as_ref()
                    .and_then(|reshoot| reshoot.recipe.as_ref())
                    .map(|recipe| recipe.reconstruction_fallbacks)
            })
            .unwrap_or(0)
    ))
}
fn model_projection(model: &crate::reconstruction::direct_input::Model) -> VideoProjection {
    let erp = model
        .images
        .iter()
        .filter(|i| {
            model
                .cameras
                .iter()
                .any(|c| c.id == i.pose.camera_id && c.model_id == 17)
        })
        .count();
    if erp == model.images.len() {
        VideoProjection::Equirectangular
    } else if erp == 0 {
        VideoProjection::Perspective
    } else {
        VideoProjection::Auto
    }
}

fn collection_candidate(paths: &ProjectPaths, state: &PipelineStateFile) -> Option<PathBuf> {
    let attempt = state
        .reshoot
        .as_ref()
        .and_then(|value| value.recipe.as_ref())
        .map_or(0, |recipe| recipe.training_fallbacks);
    reusable_brush_candidate(&paths.brush.join(format!("collection-training-{attempt}")))
}

fn execution_plan_is_incremental(checkpoint: &InputCheckpoint) -> bool {
    checkpoint.execution_plan.as_ref().is_some_and(|plan| {
        matches!(
            plan.reconstruction,
            crate::pipeline::execution_plan::ReconstructionPlan::Incremental { .. }
        )
    })
}

fn collection_gps_base_model(
    paths: &ProjectPaths,
    checkpoint: &InputCheckpoint,
) -> Option<PathBuf> {
    execution_plan_is_incremental(checkpoint).then(|| paths.colmap.join("base-model"))
}

fn execution_plan_is_reshoot(checkpoint: &InputCheckpoint) -> bool {
    checkpoint.execution_plan.as_ref().is_some_and(|plan| {
        plan.matching == crate::pipeline::execution_plan::MatchPlan::ReshootCrossSource
    })
}

/// Apply a user-authorized algorithm fallback and immediately rebuild the
/// execution plan. Runtime code reads the rebuilt plan; the recipe remains the
/// durable answer/counter record used to derive it.
fn apply_execution_plan_fallback(
    state: &mut PipelineStateFile,
    failure: crate::pipeline::reshoot::AlgorithmFailure,
) -> bool {
    let allowed = state
        .inputs
        .as_ref()
        .and_then(|inputs| inputs.execution_plan.as_ref())
        .is_some_and(|plan| plan.fallback.allow_algorithm_fallback);
    if !allowed {
        return false;
    }
    let Some(mut next_recipe) = state
        .reshoot
        .as_ref()
        .and_then(|reshoot| reshoot.recipe.clone())
    else {
        return false;
    };
    if !next_recipe.fallback(failure) {
        return false;
    }
    let Some(seed) = state
        .inputs
        .as_ref()
        .and_then(|inputs| inputs.execution_plan.as_ref())
        .and_then(|plan| plan.source_model.clone())
    else {
        return false;
    };
    let next_plan = crate::pipeline::execution_plan::ExecutionPlan::reshoot(&next_recipe, seed);
    if let Some(recipe) = state
        .reshoot
        .as_mut()
        .and_then(|reshoot| reshoot.recipe.as_mut())
    {
        *recipe = next_recipe;
    }
    if let Some(inputs) = state.inputs.as_mut() {
        inputs.execution_plan = Some(next_plan);
    }
    true
}

/// Persists an automatic reconstruction fallback as a pending operation, not
/// as a completed reconstruction. This is deliberately shared by coordinate
/// mismatch and zero-new-registration fallbacks so a crash after saving the
/// changed plan cannot skip the mapper on resume.
fn prepare_collection_reconstruction_retry(
    state: &mut PipelineStateFile,
    checkpoint: &mut InputCheckpoint,
) {
    state.reconstruction_complete = false;
    state.brush_complete = false;
    state.validated_output = None;
    state.gps = None;
    state.stage = PipelineStage::Reconstructing;
    checkpoint.reference_image = None;
    checkpoint.source_adoption.clear();
    checkpoint.unused.clear();
    state.inputs = Some(checkpoint.clone());
}

fn collection_coordinates_match(base: &Path, updated: &Path) -> Result<bool> {
    let old = read_model(base)?;
    let new = read_model(updated)?;
    for camera in old.cameras {
        if !new.cameras.contains(&camera) {
            return Ok(false);
        }
    }
    for image in old.images {
        let pose = image.pose;
        let Some(other) = new
            .images
            .iter()
            .map(|i| &i.pose)
            .find(|p| p.id == pose.id && p.camera_id == pose.camera_id && p.name == pose.name)
        else {
            return Ok(false);
        };
        let dot = pose
            .qvec
            .iter()
            .zip(other.qvec)
            .map(|(a, b)| a * b)
            .sum::<f64>();
        if (1. - dot.abs()).abs() > 1e-10
            || pose
                .tvec
                .iter()
                .zip(other.tvec)
                .any(|(a, b)| (a - b).abs() > 1e-8 * (1. + a.abs()))
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn best_model(root: &Path, count: u64) -> Result<(PathBuf, ReconstructionReport)> {
    let mut candidates = vec![root.to_path_buf()];
    let mut children = std::fs::read_dir(root)?
        .map(|e| e.map(|e| e.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    children.sort();
    candidates.extend(children);
    let mut best = None;
    for path in candidates {
        if path.join("cameras.bin").is_file() {
            let report = ReconstructionValidator::validate_with_input_images(count, &path)?;
            if best
                .as_ref()
                .is_none_or(|(_, r): &(PathBuf, ReconstructionReport)| {
                    report.registered_images > r.registered_images
                })
            {
                best = Some((path, report));
            }
        }
    }
    best.ok_or_else(|| SplatError::Process("稀疏重建没有可用的注册图像或三维点".into()))
}
fn source_requires_feature_masks(source: &SourceCheckpoint) -> bool {
    // Old frame checkpoints can default an absent Alpha field to false. A
    // saved true source channel must not silently lose its prepared masks.
    source
        .frames
        .as_ref()
        .is_some_and(|frames| frames.has_alpha)
        || source.video.as_ref().is_some_and(|video| video.has_alpha)
        || source
            .image_sequence
            .as_ref()
            .is_some_and(|images| images.has_alpha)
}

async fn feature_masks_for_group(
    paths: &ProjectPaths,
    names: &[String],
    required: bool,
    cancel: tokio_util::sync::CancellationToken,
) -> Result<Option<PathBuf>> {
    let images = paths.colmap_frames.clone();
    let masks = paths.masks.clone();
    let selected = masks.clone();
    let names = names.to_vec();
    let enabled = crate::presets::spawn_pipeline_blocking(move || {
        crate::pipeline::feature_input::use_masks(&images, &masks, &names, required, &cancel)
    })
    .await
    .map_err(|error| SplatError::Process(error.to_string()))??;
    Ok(enabled.then_some(selected))
}

#[allow(clippy::too_many_arguments)]
fn prepare_images(
    files: &[PathBuf],
    prefix: &str,
    frames: &Path,
    rgb: &Path,
    masks: &Path,
    erp: bool,
    quality: Quality,
    cancel: &tokio_util::sync::CancellationToken,
    progress: Option<&super::reshoot_runner::ReshootProgress>,
) -> Result<Vec<String>> {
    use image::ImageDecoder;
    let mut names = vec![];
    for (i, path) in files.iter().enumerate() {
        if cancel.is_cancelled() {
            return Err(SplatError::Cancelled);
        }
        if !erp {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("png");
            let name = super::reshoot_runner::normalize_image(
                path,
                frames,
                rgb,
                masks,
                &format!("{prefix}{i:010}.{ext}"),
                cancel,
            )?;
            let h = crate::video::image_projection::inspect(&frames.join(&name))?;
            let mask = masks.join(format!("{name}.png"));
            if !mask.is_file() {
                image::GrayImage::from_pixel(h.width, h.height, image::Luma([255]))
                    .save(mask)
                    .map_err(|e| SplatError::Process(e.to_string()))?;
            }
            names.push(name);
            if let Some(progress) = progress {
                progress.update((i + 1) as u64, files.len() as u64);
            }
            continue;
        }
        let header = crate::video::image_projection::inspect(path)?;
        if u64::from(header.width) * u64::from(header.height) * 4
            > crate::video::panorama::MAX_DECODE_BYTES
        {
            return Err(SplatError::Process(
                "Image exceeds the 512 MiB decode limit".into(),
            ));
        }
        let mut reader = image::ImageReader::open(path)?.with_guessed_format()?;
        let mut limits = image::Limits::default();
        limits.max_alloc = Some(crate::video::panorama::MAX_DECODE_BYTES);
        reader.limits(limits);
        let mut decoder = reader
            .into_decoder()
            .map_err(|e| SplatError::Process(e.to_string()))?;
        let orientation = decoder
            .orientation()
            .map_err(|e| SplatError::Process(e.to_string()))?;
        let mut decoded = image::DynamicImage::from_decoder(decoder)
            .map_err(|e| SplatError::Process(e.to_string()))?;
        decoded.apply_orientation(orientation);
        if erp {
            let cap = match quality {
                Quality::Fast => 2048,
                Quality::Balanced => 3072,
                Quality::High => decoded.width(),
            };
            if decoded.width() != decoded.height() * 2 {
                return Err(SplatError::Process("不支持此图片集的全景格式。请使用同尺寸、已拼接、单目、完整 360°×180° 的 2:1 ERP 图片。".into()));
            }
            if decoded.width() > cap {
                decoded = decoded.resize_exact(cap, cap / 2, image::imageops::FilterType::Lanczos3);
            }
        }
        let name = format!("{prefix}{i:010}.png");
        decoded
            .save(frames.join(&name))
            .map_err(|e| SplatError::Process(e.to_string()))?;
        decoded
            .to_rgb8()
            .save(rgb.join(&name))
            .map_err(|e| SplatError::Process(e.to_string()))?;
        let alpha = decoded.to_rgba8();
        let mask = image::GrayImage::from_fn(alpha.width(), alpha.height(), |x, y| {
            image::Luma([alpha.get_pixel(x, y)[3]])
        });
        mask.save(masks.join(format!("{name}.png")))
            .map_err(|e| SplatError::Process(e.to_string()))?;
        names.push(name);
        if let Some(progress) = progress {
            progress.update((i + 1) as u64, files.len() as u64);
        }
    }
    Ok(names)
}
fn has_transparency(
    frames: &Path,
    names: &[String],
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<bool> {
    for name in names {
        if cancel.is_cancelled() {
            return Err(SplatError::Cancelled);
        }
        let path = frames.join(name);
        if crate::video::image_projection::inspect(&path)?.alpha {
            let mut r = image::ImageReader::open(path)?.with_guessed_format()?;
            let mut l = image::Limits::default();
            l.max_alloc = Some(crate::video::panorama::MAX_DECODE_BYTES);
            r.limits(l);
            let image = r.decode().map_err(|e| SplatError::Process(e.to_string()))?;
            if crate::video::image_sequence::has_native_transparency(&image) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}
