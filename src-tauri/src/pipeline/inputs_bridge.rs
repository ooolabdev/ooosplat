//! One joint bridge round, with independent frozen budgets and real clip-local timestamps.
use super::*;
use crate::planner::BridgeBackfillPlan;
use std::collections::BTreeSet;
use std::io::Write;
#[derive(Clone, Serialize, Deserialize)]
struct Publication {
    checkpoint: InputCheckpoint,
    model: PathBuf,
    additions: Vec<String>,
}

enum JointBridgeAttempt {
    Accepted(PathBuf, ReconstructionReport),
    /// Only explicit mapper geometry evidence or an otherwise complete,
    /// readable candidate that regressed can enter this branch.
    Rejected(SplatError),
}

async fn bridge_candidate(
    root: PathBuf,
    count: u64,
) -> Result<Option<(PathBuf, ReconstructionReport)>> {
    crate::presets::spawn_pipeline_blocking(move || {
        colmap::select_reconstruction_model(&root, count)
    })
    .await
    .map_err(|error| SplatError::Process(error.to_string()))?
}

async fn copy_bridge_database(
    paths: &ProjectPaths,
    source: PathBuf,
    target: PathBuf,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<()> {
    let root = paths.project.clone();
    let cancel = cancel.clone();
    crate::presets::spawn_pipeline_blocking(move || {
        crate::pipeline::sqlite_recovery::copy_owned_database(&root, &source, &target, &cancel)
    })
    .await
    .map_err(|error| SplatError::Process(error.to_string()))?
    .map_err(|error| {
        error.with_io_diagnostic(
            "bridge_operation=copy_database_snapshot\npath_role=colmap_database",
        )
    })
}

impl PipelineRunner {
    async fn archive_unpublished_bridge_attempt(
        &self,
        manager: &ProjectManager,
        paths: &ProjectPaths,
        state: &mut PipelineStateFile,
    ) -> Result<()> {
        if !tokio::fs::try_exists(paths.work.join("joint-bridge")).await? {
            return Ok(());
        }
        if tokio::fs::try_exists(paths.work.join("joint-bridge/publication.json")).await? {
            return Err(SplatError::Process(
                "Pending Bridge publication must be reconciled first".into(),
            ));
        }
        // A stopped attempt may own WAL/journal files. Move its whole namespace
        // behind the existing durable barrier, never overwrite just its main DB.
        if !state
            .retry_cache_pending
            .iter()
            .any(|entry| entry == "joint-bridge")
        {
            state.retry_cache_pending.push("joint-bridge".into());
        }
        self.prepare_continuation(manager, paths, state).await
    }
    /// Finish a durable Bridge publication before artifact normalization sees
    /// half-installed RGB/Mask/database files as a changed prepared dataset.
    /// Replaying an already copied file is safe; no engine is started here.
    pub(in crate::pipeline::runner) async fn reconcile_collection_bridge(
        &self,
        manager: &ProjectManager,
        paths: &ProjectPaths,
        state: &mut PipelineStateFile,
    ) -> Result<bool> {
        let publication = paths.work.join("joint-bridge/publication.json");
        if state.bridge_backfill.status != BridgeBackfillStatus::Running
            || !tokio::fs::try_exists(&publication).await?
        {
            return Ok(false);
        }
        if !state.retry_cache_pending.is_empty() {
            // A durable invalidation barrier cannot be bypassed by an older
            // publication file that happened to survive the failed attempt.
            return Ok(false);
        }
        let pending: Box<Publication> =
            serde_json::from_slice(&tokio::fs::read(&publication).await?)?;
        let expected = Box::new(state.inputs.clone().ok_or_else(|| {
            SplatError::Process("Bridge publication has no saved input checkpoint".into())
        })?);
        let validation_paths = paths.clone();
        let validation = pending.clone();
        let cancel = self.process_manager.child_token();
        let validation_cancel = cancel.clone();
        crate::presets::spawn_pipeline_blocking(move || {
            validate_pending_publication(
                &validation_paths,
                &expected,
                &validation,
                &validation_cancel,
            )
        })
        .await
        .map_err(|error| SplatError::Process(error.to_string()))??;
        publish(paths, &pending, &cancel).await?;
        state.inputs = Some(pending.checkpoint);
        state.bridge_backfill.status = BridgeBackfillStatus::Completed;
        if let Some(checkpoint) = state.inputs.as_ref() {
            self.restore_collection_metrics(checkpoint, state.preset, state.generation_policy);
        }
        if let Some(frames) = state.frames.as_mut() {
            frames.extracted_frames = Some(
                state
                    .inputs
                    .as_ref()
                    .unwrap()
                    .sources
                    .iter()
                    .map(|source| source.names.len() as u64)
                    .sum(),
            );
        }
        self.record_collection_artifacts(paths, state).await?;
        manager.write_state(&paths.state, state).await?;
        Ok(true)
    }

    pub(super) async fn collection_bridge(
        &self,
        manager: &ProjectManager,
        paths: &ProjectPaths,
        state: &mut PipelineStateFile,
        c: &mut InputCheckpoint,
        quality: Quality,
        accel: &crate::engines::ColmapAccelerationStatus,
    ) -> Result<()> {
        let publication = paths.work.join("joint-bridge/publication.json");
        if self
            .reconcile_collection_bridge(manager, paths, state)
            .await?
        {
            *c = state.inputs.clone().expect("reconciled Bridge checkpoint");
            return Ok(());
        }
        let authority = paths.colmap.join("authoritative");
        let initial = read_model(&authority)?;
        // Older collection checkpoints could persist Running before discovering
        // that no source had an actionable bridge frame. Re-evaluate that
        // incomplete decision once instead of carrying Running forever.
        if c.bridge_used
            && matches!(
                state.bridge_backfill.status,
                BridgeBackfillStatus::NotEvaluated | BridgeBackfillStatus::Running
            )
            && !publication.is_file()
            && c.sources
                .iter()
                .all(|source| source.bridge_frames.is_empty())
        {
            c.bridge_used = false;
        }
        if !c.bridge_used {
            let observations = c.telemetry.get_or_insert_with(Default::default);
            observations.gpu_vendor = Some(if accel.detected_nvidia_device_count > 0 {
                PlannerGpuVendor::Nvidia
            } else if cfg!(target_os = "macos") {
                PlannerGpuVendor::Apple
            } else {
                PlannerGpuVendor::Unknown
            });
            let input_images = c.sources.iter().map(|s| s.names.len() as u64).sum();
            observations.bridge = Some(PlannerBridgeMetrics {
                status: PlannerBridgeStatus::NotEvaluated,
                trigger_ratio: bridge_trigger_ratio(),
                initial_input_images: input_images,
                initial_registered_images: initial.images.len() as u64,
                initial_points_3d: initial.points.len() as u64,
                final_input_images: input_images,
                final_registered_images: initial.images.len() as u64,
                final_points_3d: initial.points.len() as u64,
                ..Default::default()
            });
            if let Some(allow_two_view_tracks) = state.actual_transparency {
                observations.initial_reconstruction = Some(PlannerInitialReconstructionMetrics {
                    input_images,
                    registered_images: initial.images.len() as u64,
                    points_3d: initial.points.len() as u64,
                    backend: accel.backend,
                    allow_two_view_tracks,
                });
            }
            c.bridge_used = true;
            let new_video_sources_only = c
                .execution_plan
                .as_ref()
                .is_some_and(|plan| plan.bridge.new_video_sources_only);
            let registered = initial
                .images
                .iter()
                .map(|i| i.pose.name.as_str())
                .collect::<HashSet<_>>();
            let mut source_plans = Vec::new();
            let mut selected_bridge_frames = 0usize;
            for s in c.sources.iter_mut() {
                if new_video_sources_only && s.id == "__source" {
                    continue;
                }
                let Some(f) = &s.frames else { continue };
                let batch_index = material_batch_index(&c.request, &s.id)?;
                let selected = s
                    .panorama_records
                    .iter()
                    .filter(|r| registered.contains(r.prepared_name.as_str()))
                    .map(|r| r.source_index)
                    .collect::<BTreeSet<_>>();
                let plan = FramePlan {
                    retention_ratio: f.retention_ratio,
                    sampling_fps: f.sampling_fps,
                    estimated_frames: f.initial_extracted_frames,
                    selected_frames: f.selected_frames.clone(),
                    candidate_frames: f.candidate_frames.clone(),
                    bridge_source_frames: f.bridge_source_frames,
                    rescue_max_frames: f.rescue_max_frames,
                    minimum_frame_override_applied: f.minimum_frame_override_applied,
                };
                let selection = plan_bridge_backfill_cancellable(
                    &plan,
                    &selected,
                    selected.len() as u64,
                    &self.process_manager.child_token(),
                )?;
                // Bridge needs a registered camera from this same new video.
                // A completely disconnected source is handled by cross-source
                // matching expansion and, when authorized, reconstruction
                // fallback after this bridge evaluation.
                if selected.is_empty() {
                    s.bridge_frames.clear();
                    source_plans.push(selection);
                    continue;
                }
                if selection.initial_registration_ratio < bridge_trigger_ratio()
                    && !selection.selected_frame_indices.is_empty()
                {
                    s.bridge_frames = resolve_bridge_frames(
                        &paths.work.join(format!("inputs/{batch_index:04}")),
                        true,
                        &plan,
                        &selection.selected_frame_indices,
                        s.video.as_ref().map(|v| v.fps),
                    )?;
                    selected_bridge_frames += s.bridge_frames.len();
                } else {
                    s.bridge_frames.clear();
                }
                source_plans.push(selection);
            }
            state.bridge_backfill.status =
                joint_bridge_status(&source_plans, selected_bridge_frames);
            observe_joint_selection(
                c,
                &source_plans,
                selected_bridge_frames,
                state.bridge_backfill.status,
            );
            self.restore_collection_metrics(c, state.preset, state.generation_policy);
            state.inputs = Some(c.clone());
            manager.write_state(&paths.state, state).await?;
        }
        if state.bridge_backfill.status != BridgeBackfillStatus::Running
            || c.sources.iter().all(|s| s.bridge_frames.is_empty())
        {
            return Ok(());
        }
        // Permission failures are admission failures, not algorithm failures
        // that may be silently rolled back to the initial reconstruction.
        if let GenerationInputs::Materials { items } = &c.request {
            for source in c.sources.iter().filter(|s| !s.bridge_frames.is_empty()) {
                let input = items
                    .iter()
                    .find(|i| i.id == source.id)
                    .ok_or_else(|| SplatError::Process("Missing bridge source".into()))?;
                self.authorize_external_path(input.source.path())?;
            }
        }
        // The attempt owns its database and image namespace. An unsuccessful bridge never mutates initial caches.
        let identity_model = authority.clone();
        let initial_identity = crate::presets::spawn_pipeline_blocking(move || {
            super::super::gps_runner::model_identity(&identity_model)
        })
        .await
        .map_err(|error| SplatError::Process(error.to_string()))??;
        let initial_transparency = state.actual_transparency;
        self.archive_unpublished_bridge_attempt(manager, paths, state)
            .await?;
        let attempt = paths.work.join("joint-bridge");
        tokio::fs::create_dir_all(&attempt).await?;
        let mut p = paths.clone();
        p.colmap = attempt.join("colmap");
        p.frames = attempt.join("frames");
        p.colmap_frames = attempt.join("colmap_frames");
        p.masks = attempt.join("masks");
        for dir in [&p.colmap, &p.frames, &p.colmap_frames, &p.masks] {
            tokio::fs::create_dir_all(dir).await?;
        }
        copy_bridge_database(
            paths,
            paths.colmap.join("database.db"),
            p.colmap.join("database.db"),
            &self.process_manager.child_token(),
        )
        .await?;
        let mut updated = c.clone();
        let mut additions = vec![];
        let feature_progress = FeatureProgressScope::new(
            c.sources
                .iter()
                .filter(|source| source.id != "__source" && !source.bridge_frames.is_empty())
                .map(|source| {
                    (
                        format!("joint-bridge:{}", source.id),
                        source.bridge_frames.len() as u64,
                    )
                }),
        );
        let started = Instant::now();
        let mut extracted_count = 0u64;
        let result = async {
            for s in &c.sources {
                for name in &s.names {
                    for (from, to) in [
                        (&paths.frames, &p.frames),
                        (&paths.colmap_frames, &p.colmap_frames),
                    ] {
                        link_copy(&from.join(name), &to.join(name)).await?;
                    }
                    let mask = paths.masks.join(format!("{name}.png"));
                    if mask.is_file() {
                        link_copy(&mask, &p.masks.join(format!("{name}.png"))).await?;
                    }
                }
            }
            let list = p.colmap.join("bridge-pairs.txt");
            let mut pairs = std::io::BufWriter::new(std::fs::File::create(&list)?);
            for (index, s) in updated.sources.iter_mut().enumerate() {
                if s.bridge_frames.is_empty() {
                    continue;
                }
                self.events.source(Some(&s.id));
                let input = match &c.request {
                    GenerationInputs::Materials { items } => items
                        .iter()
                        .find(|i| i.id == s.id)
                        .ok_or_else(|| SplatError::Process("Missing bridge source".into()))?,
                    _ => unreachable!(),
                };
                let batch_index = material_batch_index(&c.request, &s.id)?;
                self.authorize_external_path(input.source.path())?;
                if input.source.identity()? != s.identity {
                    return Err(SplatError::Process("INPUT_CHANGED".into()));
                }
                let video = s
                    .video
                    .as_ref()
                    .ok_or_else(|| SplatError::Process("Missing bridge video".into()))?;
                let first = s
                    .names
                    .first()
                    .ok_or_else(|| SplatError::Process("Missing initial frames".into()))?;
                let dimensions =
                    crate::video::image_projection::inspect(&paths.frames.join(first))?;
                let source_prefix = first
                    .split_once("frame_")
                    .map(|(prefix, _)| prefix)
                    .ok_or_else(|| SplatError::Process("Invalid video frame namespace".into()))?;
                let output = attempt.join(format!("source-{index:04}"));
                let extracted = crate::engines::ffmpeg::extract_selected_frames(
                    &self.engines.ffmpeg,
                    input.source.path(),
                    &output.join("frames"),
                    &output.join("colmap_frames"),
                    &output.join("masks"),
                    &s.bridge_frames,
                    video.has_alpha,
                    Some((dimensions.width, dimensions.height)),
                    Some(paths.logs.join("joint-bridge-frames.log")),
                    &self.process_manager,
                    Some(self.process_observer(
                        PipelineStage::ExtractingFrames,
                        PipelineEngine::Ffmpeg,
                        Some(s.bridge_frames.len() as u64),
                        ObserverMode::Ffmpeg,
                    )),
                )
                .await?;
                if extracted.frame_count != s.bridge_frames.len() as u64 {
                    return Err(SplatError::Process("Incomplete bridge extraction".into()));
                }
                extracted_count = extracted_count.saturating_add(extracted.frame_count);
                let times: Vec<crate::video::panorama::FrameTime> = serde_json::from_slice(
                    &tokio::fs::read(
                        paths
                            .work
                            .join(format!("inputs/{batch_index:04}/panorama-frame-times.json")),
                    )
                    .await?,
                )?;
                let mut new_names = vec![];
                for frame in &s.bridge_frames {
                    let extension = if extracted.has_alpha { "png" } else { "jpg" };
                    let local = format!("frame_{:010}.{extension}", frame.source_frame_index);
                    let name = format!("{source_prefix}{local}");
                    for (from, to) in [
                        (&output.join("frames"), &p.frames),
                        (&output.join("colmap_frames"), &p.colmap_frames),
                    ] {
                        tokio::fs::copy(from.join(&local), to.join(&name)).await?;
                    }
                    let mask = output.join("masks").join(format!("{local}.png"));
                    if mask.is_file() {
                        tokio::fs::copy(mask, p.masks.join(format!("{name}.png"))).await?;
                    }
                    s.panorama_records.push(PanoramaFrameRecord {
                        prepared_name: name.clone(),
                        batch_id: s.id.clone(),
                        source_name: input.source.path().to_string_lossy().into(),
                        source_sha256: String::new(),
                        orientation: Some(1),
                        source_index: frame.source_frame_index,
                        timestamp_seconds: Some(frame.timestamp_seconds),
                        pts: times
                            .iter()
                            .find(|t| t.source_frame_index == frame.source_frame_index)
                            .map(|t| t.pts),
                    });
                    new_names.push(name.clone());
                    additions.push(name);
                }
                let camera = initial
                    .images
                    .iter()
                    .find(|i| s.names.contains(&i.pose.name))
                    .ok_or_else(|| {
                        SplatError::Process("Bridge source has no registered camera".into())
                    })?
                    .pose
                    .camera_id;
                let feature_list = p.colmap.join(format!("source-{index}-new.txt"));
                write_image_list(&feature_list, &new_names).await?;
                let panorama_width = if s.projection == VideoProjection::Equirectangular {
                    Some(
                        crate::video::image_projection::inspect(
                            &p.colmap_frames.join(&new_names[0]),
                        )?
                        .width,
                    )
                } else {
                    None
                };
                let (max_size, max_features) = collection_feature_limits(quality, panorama_width);
                let masks = feature_masks_for_group(
                    &p,
                    &new_names,
                    extracted.has_alpha,
                    self.process_manager.child_token(),
                )
                .await?;
                colmap::extract_reshoot_group(
                    &self.engines.colmap,
                    &p.colmap.join("database.db"),
                    colmap::ReshootFeatureOptions {
                        images: &p.colmap_frames,
                        list: &feature_list,
                        masks: masks.as_deref(),
                        panorama_dimensions: None,
                        existing_camera_id: Some(camera),
                        camera_params: None,
                        ordinary_images: false,
                        max_size,
                        max_features,
                    },
                    paths.logs.join("joint-bridge-features.log"),
                    &self.process_manager,
                    Some(self.feature_process_observer(
                        PipelineStage::ExtractingFeatures,
                        &feature_progress,
                        format!("joint-bridge:{}", s.id),
                    )),
                    accel.gpu_index(),
                )
                .await?;
                self.complete_feature_group(
                    PipelineStage::ExtractingFeatures,
                    &feature_progress,
                    format!("joint-bridge:{}", s.id),
                );
                self.events.source(None);
                s.names.extend(new_names.clone());
                let mut ordered = s
                    .panorama_records
                    .iter()
                    .map(|r| (r.source_index, &r.prepared_name))
                    .collect::<Vec<_>>();
                ordered.sort_by_key(|r| r.0);
                for name in new_names {
                    let position = ordered.iter().position(|(_, n)| **n == name).unwrap();
                    for (_, other) in
                        &ordered[position.saturating_sub(10)..(position + 11).min(ordered.len())]
                    {
                        if **other != name {
                            writeln!(pairs, "{name} {other}")?;
                        }
                    }
                }
                s.names = ordered.iter().map(|(_, name)| (*name).clone()).collect();
                if let Some(frames) = s.frames.as_mut() {
                    frames.extracted_frames = Some(s.names.len() as u64);
                }
            }
            pairs.flush()?;
            drop(pairs);
            colmap::match_pairs(
                &self.engines.colmap,
                &p.colmap.join("database.db"),
                &list,
                paths.logs.join("joint-bridge-matching.log"),
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
            let transparent = match self
                .collection_mapper(&p, &updated, state, Some(&authority), accel)
                .await
            {
                Ok(transparent) => transparent,
                Err(error) if colmap::is_recoverable_mapper_geometry_failure(&error) => {
                    return Ok(JointBridgeAttempt::Rejected(error));
                }
                Err(error) => return Err(error),
            };
            state.actual_transparency = Some(transparent);
            let count = updated.sources.iter().map(|s| s.names.len() as u64).sum();
            let Some((candidate, report)) =
                bridge_candidate(sparse_attempt(&p, state), count).await?
            else {
                return Ok(JointBridgeAttempt::Rejected(SplatError::Process(
                    "Bridge produced no usable sparse model".into(),
                )));
            };
            if report.registered_images < initial.images.len() as u64 {
                return Ok(JointBridgeAttempt::Rejected(SplatError::Process(
                    "Bridge registration regressed".into(),
                )));
            }
            Ok(JointBridgeAttempt::Accepted(candidate, report))
        }
        .await;
        if let Some(metrics) = c.telemetry.as_mut().and_then(|saved| saved.bridge.as_mut()) {
            metrics.added_frames = metrics.added_frames.max(extracted_count);
            metrics.duration_ms = Some(
                metrics
                    .duration_ms
                    .unwrap_or(0)
                    .saturating_add(started.elapsed().as_millis() as u64),
            );
        }
        match result {
            Ok(JointBridgeAttempt::Accepted(candidate, report)) => {
                updated.telemetry = c.telemetry.clone();
                if let Some(metrics) = updated
                    .telemetry
                    .as_mut()
                    .and_then(|saved| saved.bridge.as_mut())
                {
                    metrics.status = PlannerBridgeStatus::Completed;
                    metrics.final_input_images = report.input_images;
                    metrics.final_registered_images = report.registered_images;
                    metrics.final_points_3d = report.points_3d;
                    metrics.adopted = true;
                }
                let model = candidate
                    .strip_prefix(&attempt)
                    .map(Path::to_path_buf)
                    .map_err(|_| SplatError::Process("Invalid bridge candidate path".into()))?;
                let pending = Publication {
                    checkpoint: updated,
                    // Keep the recovery record portable when the whole
                    // project directory is moved between interruption and
                    // resume. `publish` still accepts legacy absolute records.
                    model,
                    additions,
                };
                crate::project::manager::atomic_write_json(&publication, &pending).await?;
                publish(paths, &pending, &self.process_manager.child_token()).await?;
                *c = pending.checkpoint;
                state.bridge_backfill.status = BridgeBackfillStatus::Completed;
            }
            Err(error) => {
                state.actual_transparency = initial_transparency;
                self.restore_collection_metrics(c, state.preset, state.generation_policy);
                state.inputs = Some(c.clone());
                if let Err(save_error) = manager.write_state(&paths.state, state).await {
                    tracing::warn!(%save_error, "Could not save failed Bridge observation");
                }
                return Err(error);
            }
            Ok(JointBridgeAttempt::Rejected(error)) => {
                state.actual_transparency = initial_transparency;
                return self
                    .finish_joint_geometry_rollback(
                        manager,
                        paths,
                        state,
                        c,
                        &initial_identity,
                        &error,
                    )
                    .await;
            }
        }
        state.inputs = Some(c.clone());
        self.restore_collection_metrics(c, state.preset, state.generation_policy);
        if let Some(f) = state.frames.as_mut() {
            f.extracted_frames = Some(c.sources.iter().map(|s| s.names.len() as u64).sum());
        }
        self.record_collection_artifacts(paths, state).await?;
        manager.write_state(&paths.state, state).await?;
        Ok(())
    }

    async fn finish_joint_geometry_rollback(
        &self,
        manager: &ProjectManager,
        paths: &ProjectPaths,
        state: &mut PipelineStateFile,
        checkpoint: &mut InputCheckpoint,
        initial_identity: &str,
        error: &SplatError,
    ) -> Result<()> {
        if self.process_manager.child_token().is_cancelled() {
            return Err(SplatError::Cancelled);
        }
        let authority = paths.colmap.join("authoritative");
        let count = checkpoint
            .sources
            .iter()
            .map(|source| source.names.len() as u64)
            .sum();
        if bridge_candidate(authority.clone(), count).await?.is_none()
            || super::super::gps_runner::model_identity(&authority)? != initial_identity
        {
            return Err(SplatError::Process(
                "Bridge initial model changed during the attempt".into(),
            ));
        }
        if self.process_manager.child_token().is_cancelled() {
            return Err(SplatError::Cancelled);
        }
        state.bridge_backfill.status = BridgeBackfillStatus::FailedRolledBack;
        if let Some(metrics) = checkpoint
            .telemetry
            .as_mut()
            .and_then(|saved| saved.bridge.as_mut())
        {
            metrics.status = PlannerBridgeStatus::FailedRolledBack;
            metrics.adopted = false;
        }
        state.inputs = Some(checkpoint.clone());
        self.restore_collection_metrics(checkpoint, state.preset, state.generation_policy);
        if let Some(frames) = state.frames.as_mut() {
            frames.extracted_frames = Some(count);
        }
        self.record_collection_artifacts(paths, state).await?;
        manager.write_state(&paths.state, state).await?;
        // The durable terminal checkpoint precedes any success-like notice.
        self.events.send(
            PipelineStage::ValidatingReconstruction,
            Some(PipelineEngine::System),
            EventKind::Log,
            EventLevel::Warning,
            None,
            true,
            format!("Bridge Backfill 未采用，继续使用初始模型：{error}"),
            None,
            None,
            None,
        );
        Ok(())
    }
}

fn observe_joint_selection(
    checkpoint: &mut InputCheckpoint,
    plans: &[BridgeBackfillPlan],
    requested_frames: usize,
    status: BridgeBackfillStatus,
) {
    if let Some(metrics) = checkpoint
        .telemetry
        .as_mut()
        .and_then(|saved| saved.bridge.as_mut())
    {
        metrics.status = if plans.is_empty() {
            PlannerBridgeStatus::NotApplicable
        } else {
            status.into()
        };
        metrics.available_budget = plans.iter().map(|plan| plan.available_budget).sum();
        metrics.requested_frames = requested_frames as u64;
        metrics.internal_bridge_count = plans.iter().map(|plan| plan.internal_bridge_count).sum();
        metrics.edge_extension_count = plans.iter().map(|plan| plan.edge_extension_count).sum();
    }
}

fn material_batch_index(request: &GenerationInputs, source_id: &str) -> Result<usize> {
    match request {
        GenerationInputs::Materials { items } => items
            .iter()
            .position(|item| item.id == source_id)
            .ok_or_else(|| SplatError::Process("Missing bridge source batch".into())),
        GenerationInputs::Colmap { .. } => Err(SplatError::Process(
            "COLMAP direct training does not have video bridge batches".into(),
        )),
    }
}

fn joint_bridge_status(
    source_plans: &[BridgeBackfillPlan],
    selected_bridge_frames: usize,
) -> BridgeBackfillStatus {
    if selected_bridge_frames > 0 {
        BridgeBackfillStatus::Running
    } else if source_plans
        .iter()
        .any(|plan| plan.initial_registration_ratio < bridge_trigger_ratio())
    {
        BridgeBackfillStatus::NoBudget
    } else {
        BridgeBackfillStatus::NotNeeded
    }
}

fn validate_pending_publication(
    paths: &ProjectPaths,
    current: &InputCheckpoint,
    pending: &Publication,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<()> {
    let invalid =
        || SplatError::Process("Bridge publication does not match the saved input plan".into());
    if cancel.is_cancelled() {
        return Err(SplatError::Cancelled);
    }
    let mut unchanged = pending.checkpoint.clone();
    unchanged.sources = current.sources.clone();
    // An observation may be completed before publication is reconciled. It
    // must never become an execution-plan or source-admission constraint.
    unchanged.telemetry = current.telemetry.clone();
    if !current.prepared
        || !current.bridge_used
        || pending.additions.is_empty()
        || serde_json::to_value(&unchanged)? != serde_json::to_value(current)?
        || pending.checkpoint.sources.len() != current.sources.len()
    {
        return Err(invalid());
    }
    let additions = pending.additions.iter().cloned().collect::<HashSet<_>>();
    if additions.len() != pending.additions.len() {
        return Err(invalid());
    }
    let attempt = paths.work.join("joint-bridge");
    let mut actual_additions = HashSet::new();
    let mut all_names = HashSet::new();
    for (before, after) in current.sources.iter().zip(&pending.checkpoint.sources) {
        if cancel.is_cancelled() {
            return Err(SplatError::Cancelled);
        }
        let mut unchanged = after.clone();
        unchanged.names = before.names.clone();
        unchanged.panorama_records = before.panorama_records.clone();
        if let (Some(expected), Some(actual)) = (&before.frames, unchanged.frames.as_mut()) {
            actual.extracted_frames = expected.extracted_frames;
        }
        if serde_json::to_value(&unchanged)? != serde_json::to_value(before)? {
            return Err(invalid());
        }
        let old = before.names.iter().collect::<HashSet<_>>();
        if old.len() != before.names.len()
            || !before.names.iter().all(|name| after.names.contains(name))
            || after.panorama_records.len()
                != before.panorama_records.len() + before.bridge_frames.len()
            || serde_json::to_value(&after.panorama_records[..before.panorama_records.len()])?
                != serde_json::to_value(&before.panorama_records)?
        {
            return Err(invalid());
        }
        let mut source_additions = HashSet::new();
        for name in &after.names {
            if !all_names.insert(name.clone()) {
                return Err(invalid());
            }
            for directory in ["frames", "colmap_frames"] {
                let image = crate::reconstruction::direct_input::image_path(
                    &attempt.join(directory),
                    name,
                )?;
                crate::video::image_projection::inspect(&image)?;
            }
            if !old.contains(name) {
                source_additions.insert(name.clone());
                actual_additions.insert(name.clone());
                let rgb =
                    crate::video::image_projection::inspect(&attempt.join("frames").join(name))?;
                let mask = attempt.join("masks").join(format!("{name}.png"));
                let needs_mask = before
                    .frames
                    .as_ref()
                    .is_some_and(|frames| frames.has_alpha);
                if needs_mask || mask.is_file() {
                    let mask = crate::reconstruction::direct_input::image_path(
                        &attempt.join("masks"),
                        &format!("{name}.png"),
                    )?;
                    let mask = crate::video::image_projection::inspect(&mask)?;
                    if (mask.width, mask.height) != (rgb.width, rgb.height) {
                        return Err(invalid());
                    }
                }
            }
        }
        if source_additions.len() != before.bridge_frames.len()
            || after
                .frames
                .as_ref()
                .is_some_and(|frames| frames.extracted_frames != Some(after.names.len() as u64))
        {
            return Err(invalid());
        }
        let new_records = &after.panorama_records[before.panorama_records.len()..];
        for (frame, record) in before.bridge_frames.iter().zip(new_records) {
            let stem = Path::new(&record.prepared_name)
                .file_stem()
                .and_then(|stem| stem.to_str());
            let prefix = before
                .names
                .first()
                .and_then(|name| name.split_once("frame_"))
                .map(|(prefix, _)| prefix)
                .ok_or_else(invalid)?;
            let expected_name = format!("{prefix}frame_{:010}", frame.source_frame_index);
            if record.batch_id != before.id
                || record.source_index != frame.source_frame_index
                || record.timestamp_seconds != Some(frame.timestamp_seconds)
                || !frame.timestamp_seconds.is_finite()
                || !source_additions.contains(&record.prepared_name)
                || stem != Some(expected_name.as_str())
            {
                return Err(invalid());
            }
        }
    }
    if additions != actual_additions {
        return Err(invalid());
    }
    let model = publication_model(paths, pending)?;
    let database = std::fs::canonicalize(attempt.join("colmap/database.db"))?;
    if !database.starts_with(std::fs::canonicalize(attempt.join("colmap"))?) {
        return Err(invalid());
    }
    let connection = rusqlite::Connection::open_with_flags(
        &database,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|error| SplatError::Process(error.to_string()))?;
    let mut query = connection
        .prepare("SELECT name FROM images")
        .map_err(|error| SplatError::Process(error.to_string()))?;
    let database_names = query
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| SplatError::Process(error.to_string()))?
        .collect::<std::result::Result<HashSet<_>, _>>()
        .map_err(|error| SplatError::Process(error.to_string()))?;
    if database_names != all_names {
        return Err(invalid());
    }
    let input = ColmapInput {
        model,
        images: attempt.join("colmap_frames"),
        database: Some(database),
        identity: None,
    };
    let info = crate::reconstruction::direct_input::inspect(&input)?;
    if !info.reshoot_eligible
        || read_model(&input.model)?
            .images
            .iter()
            .any(|image| !all_names.contains(&image.pose.name))
    {
        return Err(invalid());
    }
    if cancel.is_cancelled() {
        return Err(SplatError::Cancelled);
    }
    Ok(())
}

fn publication_model(paths: &ProjectPaths, pending: &Publication) -> Result<PathBuf> {
    let attempt = paths.work.join("joint-bridge");
    let saved_model = if pending.model.is_absolute() {
        // Compatibility for publication.json written before bridge paths were
        // portable. Resolve the suffix following `joint-bridge` under the
        // current project instead of requiring the old absolute root.
        let components = pending.model.components().collect::<Vec<_>>();
        let marker = components
            .iter()
            .position(|component| component.as_os_str() == "joint-bridge")
            .ok_or_else(|| SplatError::Process("Invalid bridge publication path".into()))?;
        let mut relative = PathBuf::new();
        for component in &components[marker + 1..] {
            match component {
                std::path::Component::Normal(value) => relative.push(value),
                _ => {
                    return Err(SplatError::Process(
                        "Invalid bridge publication path".into(),
                    ))
                }
            }
        }
        attempt.join(relative)
    } else {
        if pending
            .model
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
        {
            return Err(SplatError::Process(
                "Invalid bridge publication path".into(),
            ));
        }
        attempt.join(&pending.model)
    };
    let candidate = std::fs::canonicalize(saved_model)?;
    if !candidate.starts_with(std::fs::canonicalize(attempt.join("colmap"))?) {
        return Err(SplatError::Process(
            "Invalid bridge publication path".into(),
        ));
    }
    Ok(candidate)
}

async fn publish(
    paths: &ProjectPaths,
    pending: &Publication,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<()> {
    if cancel.is_cancelled() {
        return Err(SplatError::Cancelled);
    }
    let candidate = publication_model(paths, pending)?;
    let attempt = paths.work.join("joint-bridge");
    for name in &pending.additions {
        for relative in ["frames", "colmap_frames"] {
            let source =
                crate::reconstruction::direct_input::image_path(&attempt.join(relative), name)?;
            super::super::reshoot_runner::copy_reshoot_file(
                &source,
                &paths.work.join(relative).join(name),
                cancel,
            )
            .await?;
        }
        let mask = attempt.join("masks").join(format!("{name}.png"));
        if mask.is_file() {
            super::super::reshoot_runner::copy_reshoot_file(
                &mask,
                &paths.masks.join(format!("{name}.png")),
                cancel,
            )
            .await?;
        }
    }
    read_model(&candidate)?.write(&paths.colmap.join("authoritative"))?;
    copy_bridge_database(
        paths,
        attempt.join("colmap/database.db"),
        paths.colmap.join("database.db"),
        cancel,
    )
    .await?;
    Ok(())
}
async fn link_copy(source: &Path, dest: &Path) -> Result<()> {
    if dest.exists() {
        return Ok(());
    }
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    if tokio::fs::hard_link(source, dest).await.is_err() {
        tokio::fs::copy(source, dest).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stopped_joint_attempt_archives_its_database_family_only_after_a_saved_barrier() {
        for block_state_write in [true, false] {
            let (root, paths, mut state, _) = recovery_fixture();
            let manager = ProjectManager::for_diagnostics(root.path().to_owned());
            let attempt = paths.work.join("joint-bridge");
            for suffix in ["-wal", "-journal", "-shm"] {
                std::fs::write(attempt.join(format!("colmap/database.db{suffix}")), suffix)
                    .unwrap();
            }
            let original = std::fs::read(paths.colmap.join("database.db")).unwrap();
            let frozen = serde_json::to_value(state.inputs.as_ref().unwrap()).unwrap();
            if block_state_write {
                std::fs::create_dir(&paths.state).unwrap();
            }
            let runner = PipelineRunner::new(
                EnginePaths::from_root(root.path().join("missing-engines")),
                |_| {},
            );
            let result = runner
                .archive_unpublished_bridge_attempt(&manager, &paths, &mut state)
                .await;
            if block_state_write {
                assert!(result.unwrap_err().io_error().is_some());
                assert!(attempt.join("colmap/database.db-wal").is_file());
                assert!(!paths.work.join("cache-archive").exists());
            } else {
                result.unwrap();
                assert!(!attempt.exists());
                let archive = std::fs::read_dir(paths.work.join("cache-archive"))
                    .unwrap()
                    .next()
                    .unwrap()
                    .unwrap()
                    .path()
                    .join("joint-bridge/colmap");
                for suffix in ["", "-wal", "-journal", "-shm"] {
                    assert!(archive.join(format!("database.db{suffix}")).is_file());
                }
                let saved = manager.read_state(&paths.state).await.unwrap();
                assert!(saved.retry_cache_pending.is_empty());
                assert_eq!(
                    serde_json::to_value(saved.inputs.as_ref().unwrap()).unwrap(),
                    frozen
                );
            }
            assert_eq!(
                std::fs::read(paths.colmap.join("database.db")).unwrap(),
                original
            );
            assert_eq!(
                serde_json::to_value(state.inputs.as_ref().unwrap()).unwrap(),
                frozen
            );
        }
    }

    #[tokio::test]
    async fn geometry_rollback_is_durable_preserves_counts_and_never_regrants_budget() {
        let (root, paths, mut state, _) = recovery_fixture();
        let manager = ProjectManager::for_diagnostics(root.path().to_owned());
        let mut checkpoint = state.inputs.clone().unwrap();
        checkpoint.bridge_used = true;
        checkpoint.telemetry = Some(crate::pipeline::effectiveness::CollectionTelemetry {
            bridge: Some(PlannerBridgeMetrics {
                status: PlannerBridgeStatus::Running,
                added_frames: 1,
                requested_frames: 1,
                available_budget: 1,
                duration_ms: Some(321),
                ..Default::default()
            }),
            ..Default::default()
        });
        let original_db = std::fs::read(paths.colmap.join("database.db")).unwrap();
        let identity =
            super::super::super::gps_runner::model_identity(&paths.colmap.join("authoritative"))
                .unwrap();
        let runner = PipelineRunner::new(
            EnginePaths::from_root(root.path().join("missing-engines")),
            |_| {},
        );
        runner
            .finish_joint_geometry_rollback(
                &manager,
                &paths,
                &mut state,
                &mut checkpoint,
                &identity,
                &SplatError::Process("explicit geometry rejection".into()),
            )
            .await
            .unwrap();
        let saved = manager.read_state(&paths.state).await.unwrap();
        assert_eq!(
            saved.bridge_backfill.status,
            BridgeBackfillStatus::FailedRolledBack
        );
        let stored = saved.inputs.unwrap();
        assert!(stored.bridge_used);
        assert_eq!(stored.sources[0].bridge_frames.len(), 1);
        let metrics = stored.telemetry.unwrap().bridge.unwrap();
        assert_eq!(metrics.status, PlannerBridgeStatus::FailedRolledBack);
        assert_eq!(
            (
                metrics.added_frames,
                metrics.available_budget,
                metrics.duration_ms
            ),
            (1, 1, Some(321))
        );
        assert!(!metrics.adopted);
        assert_eq!(
            original_db,
            std::fs::read(paths.colmap.join("database.db")).unwrap()
        );
    }

    #[tokio::test]
    async fn geometry_rollback_cannot_report_success_before_state_is_saved() {
        let (root, paths, mut state, _) = recovery_fixture();
        let manager = ProjectManager::for_diagnostics(root.path().to_owned());
        let mut checkpoint = state.inputs.clone().unwrap();
        let identity =
            super::super::super::gps_runner::model_identity(&paths.colmap.join("authoritative"))
                .unwrap();
        std::fs::create_dir(&paths.state).unwrap();
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let observed = events.clone();
        let runner = PipelineRunner::new(
            EnginePaths::from_root(root.path().join("missing-engines")),
            move |event| {
                observed.lock().unwrap().push(event);
            },
        );
        let error = runner
            .finish_joint_geometry_rollback(
                &manager,
                &paths,
                &mut state,
                &mut checkpoint,
                &identity,
                &SplatError::Process("explicit geometry rejection".into()),
            )
            .await
            .unwrap_err();
        assert!(error.io_error().is_some());
        assert!(!events
            .lock()
            .unwrap()
            .iter()
            .any(|event| event.message.contains("继续使用初始模型")));
        assert!(paths.colmap.join("database.db").is_file());
    }

    fn source_plan(input: u64, registered: u64, available_budget: u64) -> BridgeBackfillPlan {
        BridgeBackfillPlan {
            initial_registered_images: registered,
            initial_input_images: input,
            initial_registration_ratio: if input == 0 {
                0.0
            } else {
                registered as f64 / input as f64
            },
            available_budget,
            ..Default::default()
        }
    }

    #[test]
    fn joint_observation_sums_source_local_frames_without_summing_shared_model_points() {
        let mut checkpoint = InputCheckpoint::new(GenerationInputs::Materials { items: vec![] });
        checkpoint.telemetry = Some(crate::pipeline::effectiveness::CollectionTelemetry {
            bridge: Some(PlannerBridgeMetrics {
                initial_points_3d: 123,
                final_points_3d: 123,
                ..Default::default()
            }),
            ..Default::default()
        });
        let first = BridgeBackfillPlan {
            available_budget: 2,
            selected_frame_indices: vec![10],
            internal_bridge_count: 1,
            ..Default::default()
        };
        let second = BridgeBackfillPlan {
            available_budget: 3,
            selected_frame_indices: vec![10],
            edge_extension_count: 1,
            ..Default::default()
        };
        observe_joint_selection(
            &mut checkpoint,
            &[first, second],
            2,
            BridgeBackfillStatus::Running,
        );
        let saved: InputCheckpoint =
            serde_json::from_slice(&serde_json::to_vec(&checkpoint).unwrap()).unwrap();
        let metrics = saved.telemetry.unwrap().bridge.unwrap();
        assert_eq!(metrics.available_budget, 5);
        assert_eq!(metrics.requested_frames, 2);
        assert_eq!(
            (metrics.internal_bridge_count, metrics.edge_extension_count),
            (1, 1)
        );
        assert_eq!(
            (metrics.initial_points_3d, metrics.final_points_3d),
            (123, 123)
        );
    }

    #[tokio::test]
    async fn shared_bridge_no_budget_records_real_initial_model_without_starting_engines() {
        let (root, paths, mut state, pending) = recovery_fixture();
        let mut checkpoint = pending.checkpoint;
        checkpoint.bridge_used = false;
        checkpoint.telemetry = None;
        let frames = checkpoint.sources[0].frames.as_mut().unwrap();
        frames.initial_extracted_frames = 2;
        frames.estimated_frames = 2;
        frames.selected_frames = vec![
            crate::video::PlannedFrame {
                source_frame_index: 0,
                timestamp_seconds: 0.0,
            },
            crate::video::PlannedFrame {
                source_frame_index: 1,
                timestamp_seconds: 1.0,
            },
        ];
        frames.rescue_max_frames = 2;
        checkpoint.sources[0].bridge_frames.clear();
        state.inputs = Some(checkpoint.clone());
        state.bridge_backfill.status = BridgeBackfillStatus::NotEvaluated;
        state.actual_transparency = Some(true);
        let tracker = PlannerEffectivenessTracker::default();
        let runner = PipelineRunner::new(
            EnginePaths::from_root(root.path().join("missing-engines")),
            |_| {},
        )
        .with_effectiveness_tracker(tracker.clone());
        let acceleration = serde_json::from_value(serde_json::json!({
            "backend":"cpu","detectionState":"ready","reasonCode":"gpuReady","reason":"fixture",
            "device":null,"requirements":{"minimumDriverVersion":"580","minimumComputeCapability":"7.5"}
        })).unwrap();
        runner
            .collection_bridge(
                &ProjectManager::for_diagnostics(root.path().to_path_buf()),
                &paths,
                &mut state,
                &mut checkpoint,
                Quality::Balanced,
                &acceleration,
            )
            .await
            .unwrap();
        let metrics = tracker.snapshot();
        assert_eq!(
            metrics.bridge.as_ref().unwrap().status,
            PlannerBridgeStatus::NoBudget
        );
        assert_eq!(metrics.bridge.as_ref().unwrap().initial_input_images, 2);
        assert_eq!(
            metrics.bridge.as_ref().unwrap().initial_registered_images,
            1
        );
        assert_eq!(
            metrics
                .initial_reconstruction
                .as_ref()
                .unwrap()
                .registered_images,
            1
        );
        assert!(
            metrics
                .initial_reconstruction
                .as_ref()
                .unwrap()
                .allow_two_view_tracks
        );
        assert!(!paths.logs.join("joint-bridge-frames.log").exists());
    }

    #[tokio::test]
    async fn bridge_publication_observations_are_not_admission_identity_and_restore_after_restart()
    {
        let (root, paths, mut state, mut pending) = recovery_fixture();
        state.inputs.as_mut().unwrap().telemetry =
            Some(crate::pipeline::effectiveness::CollectionTelemetry {
                bridge: Some(PlannerBridgeMetrics {
                    status: PlannerBridgeStatus::Running,
                    initial_input_images: 1,
                    initial_registered_images: 1,
                    initial_points_3d: 7,
                    ..Default::default()
                }),
                ..Default::default()
            });
        pending.checkpoint.telemetry = state.inputs.as_ref().unwrap().telemetry.clone();
        let observed = pending
            .checkpoint
            .telemetry
            .as_mut()
            .unwrap()
            .bridge
            .as_mut()
            .unwrap();
        observed.status = PlannerBridgeStatus::Completed;
        observed.requested_frames = 1;
        observed.added_frames = 1;
        observed.final_input_images = 2;
        observed.final_registered_images = 2;
        observed.final_points_3d = 9;
        observed.adopted = true;
        validate_pending_publication(
            &paths,
            state.inputs.as_ref().unwrap(),
            &pending,
            &tokio_util::sync::CancellationToken::new(),
        )
        .unwrap();
        crate::project::manager::atomic_write_json(
            &paths.work.join("joint-bridge/publication.json"),
            &pending,
        )
        .await
        .unwrap();
        let tracker = PlannerEffectivenessTracker::default();
        let manager = ProjectManager::for_diagnostics(root.path().to_path_buf());
        let runner = PipelineRunner::new(
            EnginePaths::from_root(root.path().join("missing-engines")),
            |_| {},
        )
        .with_effectiveness_tracker(tracker.clone());
        assert!(runner
            .reconcile_collection_bridge(&manager, &paths, &mut state)
            .await
            .unwrap());
        let saved = manager.read_state(&paths.state).await.unwrap();
        let observed = tracker.snapshot().bridge.unwrap();
        assert_eq!(observed.status, PlannerBridgeStatus::Completed);
        assert_eq!(
            (observed.initial_points_3d, observed.final_points_3d),
            (7, 9)
        );
        assert_eq!(observed.added_frames, 1);
        let restored = PlannerEffectivenessTracker::default();
        PipelineRunner::new(default_engine_paths(None), |_| {})
            .with_effectiveness_tracker(restored.clone())
            .restore_source_metrics(&saved);
        assert_eq!(restored.snapshot().bridge, Some(observed));
    }

    #[test]
    fn joint_bridge_uses_each_video_ratio_instead_of_the_weighted_total() {
        let plans = [source_plan(1_000, 1_000, 0), source_plan(100, 5, 20)];
        let weighted_ratio = plans
            .iter()
            .map(|plan| plan.initial_registered_images)
            .sum::<u64>() as f64
            / plans
                .iter()
                .map(|plan| plan.initial_input_images)
                .sum::<u64>() as f64;
        assert!(weighted_ratio >= bridge_trigger_ratio());
        assert_eq!(
            joint_bridge_status(&plans, 1),
            BridgeBackfillStatus::Running
        );
    }

    #[test]
    fn low_registration_without_an_actionable_frame_is_terminal_no_budget() {
        let plans = [source_plan(100, 20, 0)];
        assert_eq!(
            joint_bridge_status(&plans, 0),
            BridgeBackfillStatus::NoBudget
        );
    }

    #[test]
    fn fully_registered_sources_are_terminal_not_needed() {
        let plans = [source_plan(100, 100, 0), source_plan(50, 45, 10)];
        assert_eq!(
            joint_bridge_status(&plans, 0),
            BridgeBackfillStatus::NotNeeded
        );
        assert_eq!(joint_bridge_status(&[], 0), BridgeBackfillStatus::NotNeeded);
    }

    fn write_recovery_database(path: &Path, model: &crate::reconstruction::direct_input::Model) {
        let db = rusqlite::Connection::open(path).unwrap();
        db.execute_batch(
            "CREATE TABLE cameras(camera_id INTEGER,model INTEGER,width INTEGER,height INTEGER);\
             CREATE TABLE images(image_id INTEGER,name TEXT,camera_id INTEGER);\
             CREATE TABLE keypoints(image_id INTEGER,rows INTEGER,cols INTEGER,data BLOB);\
             CREATE TABLE descriptors(image_id INTEGER,rows INTEGER,cols INTEGER,data BLOB);\
             CREATE TABLE matches(pair_id INTEGER,rows INTEGER,cols INTEGER,data BLOB);\
             CREATE TABLE two_view_geometries(pair_id INTEGER,rows INTEGER,cols INTEGER,data BLOB);\
             CREATE TABLE rigs(rig_id INTEGER,ref_sensor_id INTEGER,ref_sensor_type INTEGER);\
             CREATE TABLE rig_sensors(rig_id INTEGER);\
             CREATE TABLE frames(frame_id INTEGER,rig_id INTEGER);\
             CREATE TABLE frame_data(frame_id INTEGER,data_id INTEGER,sensor_id INTEGER,sensor_type INTEGER);",
        ).unwrap();
        for camera in &model.cameras {
            db.execute(
                "INSERT INTO cameras VALUES(?1,?2,?3,?4)",
                rusqlite::params![camera.id, camera.model_id, camera.width, camera.height],
            )
            .unwrap();
            db.execute("INSERT INTO rigs VALUES(?1,?1,0)", [camera.id])
                .unwrap();
        }
        for image in &model.images {
            let pose = &image.pose;
            db.execute(
                "INSERT INTO images VALUES(?1,?2,?3)",
                rusqlite::params![pose.id, pose.name, pose.camera_id],
            )
            .unwrap();
            let xy = image
                .observations
                .iter()
                .flat_map(|point| [point.x as f32, point.y as f32])
                .flat_map(f32::to_le_bytes)
                .collect::<Vec<_>>();
            db.execute(
                "INSERT INTO keypoints VALUES(?1,?2,2,?3)",
                rusqlite::params![pose.id, image.observations.len(), xy],
            )
            .unwrap();
            db.execute(
                "INSERT INTO descriptors VALUES(?1,?2,128,?3)",
                rusqlite::params![
                    pose.id,
                    image.observations.len(),
                    vec![0_u8; image.observations.len() * 128]
                ],
            )
            .unwrap();
            db.execute(
                "INSERT INTO frames VALUES(?1,?2)",
                rusqlite::params![pose.id, pose.camera_id],
            )
            .unwrap();
            db.execute(
                "INSERT INTO frame_data VALUES(?1,?1,?2,0)",
                rusqlite::params![pose.id, pose.camera_id],
            )
            .unwrap();
        }
    }

    fn recovery_fixture() -> (
        tempfile::TempDir,
        ProjectPaths,
        PipelineStateFile,
        Publication,
    ) {
        let root = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::existing(uuid::Uuid::new_v4(), root.path().join("project"));
        for dir in [
            &paths.frames,
            &paths.colmap_frames,
            &paths.masks,
            &paths.colmap,
        ] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let fixture =
            crate::reconstruction::direct_input::tests::fixture(&root.path().join("source"), false);
        let mut model = read_model(&fixture.model).unwrap();
        let names = ["b0_frame_0000000000.png", "b0_frame_0000000001.png"];
        for (image, name) in model.images.iter_mut().zip(names) {
            image.pose.name = name.into();
        }
        let attempt = paths.work.join("joint-bridge");
        let model_path = attempt.join("colmap/sparse/0");
        model.write(&model_path).unwrap();
        write_recovery_database(&attempt.join("colmap/database.db"), &model);
        for directory in ["frames", "colmap_frames", "masks"] {
            std::fs::create_dir_all(attempt.join(directory)).unwrap();
            for name in names {
                let target = if directory == "masks" {
                    format!("{name}.png")
                } else {
                    name.into()
                };
                std::fs::copy(
                    fixture.images.join("normal.png"),
                    attempt.join(directory).join(target),
                )
                .unwrap();
            }
        }
        let mut initial = read_model(&model_path).unwrap();
        initial.images.truncate(1);
        for point in &mut initial.points {
            point
                .track
                .retain(|(id, _)| *id == initial.images[0].pose.id);
        }
        initial.write(&paths.colmap.join("authoritative")).unwrap();
        write_recovery_database(&paths.colmap.join("database.db"), &initial);
        for directory in ["frames", "colmap_frames", "masks"] {
            let name = if directory == "masks" {
                format!("{}.png", names[0])
            } else {
                names[0].into()
            };
            std::fs::copy(
                attempt.join(directory).join(&name),
                paths.work.join(directory).join(name),
            )
            .unwrap();
        }
        let video_path = root.path().join("source.mov");
        std::fs::write(&video_path, b"unchanged video identity").unwrap();
        let source = InputSource::Video {
            path: video_path.clone(),
        };
        let identity = source.identity().unwrap();
        let mut checkpoint = InputCheckpoint::new(GenerationInputs::Materials {
            items: vec![MaterialInput {
                id: "clip".into(),
                source,
                identity: Some(identity.clone()),
                projection: VideoProjection::Perspective,
                image_matching_strategy: crate::project::ImageMatchingStrategy::Exhaustive,
                camera_relation: None,
                confirmed_small_images: false,
                confirmed_image_order: false,
                frame_removal_fps: None,
            }],
        });
        checkpoint.prepared = true;
        checkpoint.bridge_used = true;
        checkpoint.execution_plan = Some(crate::pipeline::execution_plan::ExecutionPlan::normal());
        let record = |index: u64| PanoramaFrameRecord {
            prepared_name: names[index as usize].into(),
            batch_id: "clip".into(),
            source_name: video_path.to_string_lossy().into_owned(),
            source_sha256: String::new(),
            orientation: Some(1),
            source_index: index,
            timestamp_seconds: Some(index as f64),
            pts: Some(index as i64),
        };
        checkpoint.sources.push(SourceCheckpoint {
            id: "clip".into(),
            identity,
            ordinary_image_preparation_version: None,
            camera_ids: vec![7],
            camera_assignment: None,
            projection: VideoProjection::Perspective,
            names: vec![names[0].into()],
            originals: vec![video_path.clone()],
            video: Some(crate::video::VideoInfo {
                duration: 2.,
                width: 64,
                height: 48,
                fps: 1.,
                total_frames: 2,
                codec: "png".into(),
                rotation: 0,
                pixel_format: "rgba".into(),
                has_alpha: true,
            }),
            image_sequence: None,
            dimensions: vec![[64, 48]],
            working_dimensions: Some([64, 48]),
            frames: Some(FrameState {
                extracted_frames: Some(1),
                initial_extracted_frames: 1,
                selected_frames: vec![crate::video::PlannedFrame {
                    source_frame_index: 0,
                    timestamp_seconds: 0.,
                }],
                rescue_max_frames: 2,
                has_alpha: true,
                ..Default::default()
            }),
            panorama_records: vec![record(0)],
            adopted: 0,
            bridge_frames: vec![crate::video::PlannedFrame {
                source_frame_index: 1,
                timestamp_seconds: 1.,
            }],
        });
        let mut completed = checkpoint.clone();
        completed.sources[0].names.push(names[1].into());
        completed.sources[0].panorama_records.push(record(1));
        completed.sources[0]
            .frames
            .as_mut()
            .unwrap()
            .extracted_frames = Some(2);
        let pending = Publication {
            checkpoint: completed,
            model: PathBuf::from("colmap/sparse/0"),
            additions: vec![names[1].into()],
        };
        let mut state = PipelineStateFile::created(Quality::Balanced);
        state.inputs = Some(checkpoint);
        state.frames = Some(FrameState {
            extracted_frames: Some(1),
            ..Default::default()
        });
        state.features_complete = true;
        state.matching_complete = true;
        state.reconstruction_complete = true;
        state.bridge_backfill.status = BridgeBackfillStatus::Running;
        crate::pipeline::artifacts::record_collection_artifacts(&paths, &mut state, None).unwrap();
        (root, paths, state, pending)
    }

    #[tokio::test]
    async fn transparent_half_published_bridge_recovers_before_normalization_without_engines() {
        let (root, paths, mut state, pending) = recovery_fixture();
        let manager = ProjectManager::for_diagnostics(root.path().to_path_buf());
        let attempt = paths.work.join("joint-bridge");
        let added = &pending.additions[0];
        crate::project::manager::atomic_write_json(&attempt.join("publication.json"), &pending)
            .await
            .unwrap();
        manager.write_state(&paths.state, &state).await.unwrap();
        // Simulate termination after installing the new transparent frame/Mask
        // but before copying the authoritative model/database or checkpoint.
        for directory in ["frames", "masks"] {
            let name = if directory == "masks" {
                format!("{added}.png")
            } else {
                added.clone()
            };
            std::fs::copy(
                attempt.join(directory).join(&name),
                paths.work.join(directory).join(name),
            )
            .unwrap();
        }
        let original =
            crate::video::panorama::fingerprint(&root.path().join("source/images/normal.png"))
                .unwrap();
        let runner = PipelineRunner::new(default_engine_paths(None), |_| {});
        assert!(runner
            .reconcile_collection_bridge(&manager, &paths, &mut state)
            .await
            .unwrap());
        assert_eq!(
            state.bridge_backfill.status,
            BridgeBackfillStatus::Completed
        );
        assert_eq!(state.frames.as_ref().unwrap().extracted_frames, Some(2));
        assert_eq!(state.inputs.as_ref().unwrap().sources[0].names.len(), 2);
        let mut restored = manager.read_state(&paths.state).await.unwrap();
        crate::pipeline::artifacts::normalize_collection(&paths, &mut restored, None)
            .await
            .unwrap();
        assert!(restored.inputs.as_ref().unwrap().prepared);
        assert!(
            restored.features_complete
                && restored.matching_complete
                && restored.reconstruction_complete
        );
        assert_eq!(
            restored.bridge_backfill.status,
            BridgeBackfillStatus::Completed
        );
        assert!(!runner
            .reconcile_collection_bridge(&manager, &paths, &mut restored)
            .await
            .unwrap());
        assert_eq!(
            crate::video::panorama::fingerprint(&root.path().join("source/images/normal.png"))
                .unwrap(),
            original
        );
    }

    #[test]
    fn bridge_recovery_rejects_changed_plan_source_and_missing_mask_without_installation() {
        let (_root, paths, state, pending) = recovery_fixture();
        let current = state.inputs.as_ref().unwrap();
        let cancel = tokio_util::sync::CancellationToken::new();
        validate_pending_publication(&paths, current, &pending, &cancel).unwrap();
        let mut other = pending.clone();
        other.checkpoint.sources[0].identity = "another-source".into();
        assert!(validate_pending_publication(&paths, current, &other, &cancel).is_err());
        let mut other = pending.clone();
        if let GenerationInputs::Materials { items } = &mut other.checkpoint.request {
            items[0].frame_removal_fps = Some(0.5);
        }
        assert!(validate_pending_publication(&paths, current, &other, &cancel).is_err());
        let mut other = pending.clone();
        other.checkpoint.sources[0].bridge_frames[0].source_frame_index = 20;
        assert!(validate_pending_publication(&paths, current, &other, &cancel).is_err());
        std::fs::remove_file(
            paths
                .work
                .join("joint-bridge/masks")
                .join(format!("{}.png", pending.additions[0])),
        )
        .unwrap();
        assert!(validate_pending_publication(&paths, current, &pending, &cancel).is_err());
        assert!(!paths.frames.join(&pending.additions[0]).exists());
        cancel.cancel();
        assert!(matches!(
            validate_pending_publication(&paths, current, &pending, &cancel),
            Err(SplatError::Cancelled)
        ));
    }

    #[tokio::test]
    async fn pending_bridge_publication_can_be_cancelled_and_replayed_without_touching_originals() {
        let root = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::existing(uuid::Uuid::new_v4(), root.path().join("project"));
        for dir in [
            &paths.frames,
            &paths.colmap_frames,
            &paths.masks,
            &paths.colmap,
        ] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let fixture =
            crate::reconstruction::direct_input::tests::fixture(&root.path().join("source"), false);
        let attempt = paths.work.join("joint-bridge");
        let candidate = attempt.join("colmap/sparse/0");
        read_model(&fixture.model)
            .unwrap()
            .write(&candidate)
            .unwrap();
        for dir in ["frames", "colmap_frames"] {
            std::fs::create_dir_all(attempt.join(dir)).unwrap();
            for name in ["normal.png", "second.png"] {
                std::fs::copy(fixture.images.join(name), attempt.join(dir).join(name)).unwrap();
            }
        }
        write_recovery_database(
            &attempt.join("colmap/database.db"),
            &read_model(&candidate).unwrap(),
        );
        let original =
            crate::video::panorama::fingerprint(&fixture.images.join("normal.png")).unwrap();
        let pending = Publication {
            checkpoint: InputCheckpoint::new(GenerationInputs::Materials { items: vec![] }),
            model: candidate,
            additions: vec!["normal.png".into(), "second.png".into()],
        };
        crate::project::manager::atomic_write_json(&attempt.join("publication.json"), &pending)
            .await
            .unwrap();
        let cancel = tokio_util::sync::CancellationToken::new();
        cancel.cancel();
        assert!(matches!(
            publish(&paths, &pending, &cancel).await,
            Err(SplatError::Cancelled)
        ));
        assert!(!paths.colmap.join("authoritative").exists());
        let moved_root = root.path().join("moved-project");
        std::fs::rename(&paths.project, &moved_root).unwrap();
        let moved = ProjectPaths::existing(paths.id, moved_root);
        let resumed: Publication = serde_json::from_slice(
            &std::fs::read(moved.work.join("joint-bridge/publication.json")).unwrap(),
        )
        .unwrap();
        publish(&moved, &resumed, &Default::default())
            .await
            .unwrap();
        let portable = Publication {
            model: PathBuf::from("colmap/sparse/0"),
            ..resumed.clone()
        };
        publish(&moved, &portable, &Default::default())
            .await
            .unwrap();
        assert_eq!(
            read_model(&moved.colmap.join("authoritative"))
                .unwrap()
                .images
                .len(),
            2
        );
        let db = rusqlite::Connection::open(moved.colmap.join("database.db")).unwrap();
        let names = db
            .prepare("SELECT name FROM images ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(names, ["normal.png", "second.png"]);
        assert_eq!(
            crate::video::panorama::fingerprint(&fixture.images.join("normal.png")).unwrap(),
            original
        );
    }
}
