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
impl PipelineRunner {
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
        tokio::fs::copy(
            paths.colmap.join("database.db"),
            p.colmap.join("database.db"),
        )
        .await?;
        let mut updated = c.clone();
        let mut additions = vec![];
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
                let preset = quality.preset();
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
                        max_size: preset.planned_sfm_max_image_size,
                        max_features: preset.planned_sfm_max_features,
                    },
                    paths.logs.join("joint-bridge-features.log"),
                    &self.process_manager,
                    Some(self.process_observer(
                        PipelineStage::ExtractingFeatures,
                        PipelineEngine::Colmap,
                        Some(new_names.len() as u64),
                        ObserverMode::BracketProgress,
                    )),
                    accel.gpu_index(),
                )
                .await?;
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
            self.collection_mapper(&p, &updated, state, Some(&authority), accel)
                .await?;
            let count = updated.sources.iter().map(|s| s.names.len() as u64).sum();
            let (candidate, report) = best_model(&sparse_attempt(&p, state), count)?;
            if report.registered_images < initial.images.len() as u64 {
                return Err(SplatError::Process("Bridge registration regressed".into()));
            }
            Ok(candidate)
        }
        .await;
        match result {
            Ok(candidate) => {
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
            Err(error) if !matches!(&error,SplatError::Process(message) if message=="Bridge registration regressed"||message=="稀疏重建没有可用的注册图像或三维点") => {
                return Err(error)
            }
            Err(error) => {
                state.bridge_backfill.status = BridgeBackfillStatus::FailedRolledBack;
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
            }
        }
        state.inputs = Some(c.clone());
        if let Some(f) = state.frames.as_mut() {
            f.extracted_frames = Some(c.sources.iter().map(|s| s.names.len() as u64).sum());
        }
        self.record_collection_artifacts(paths, state).await?;
        manager.write_state(&paths.state, state).await?;
        Ok(())
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
    super::super::reshoot_runner::copy_reshoot_file(
        &attempt.join("colmap/database.db"),
        &paths.colmap.join("database.db"),
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
        std::fs::write(attempt.join("colmap/database.db"), b"owned database").unwrap();
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
        assert_eq!(
            std::fs::read(moved.colmap.join("database.db")).unwrap(),
            b"owned database"
        );
        assert_eq!(
            crate::video::panorama::fingerprint(&fixture.images.join("normal.png")).unwrap(),
            original
        );
    }
}
