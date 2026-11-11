//! Rebuild missing video pixels from the frozen *adopted* frame list.
//! This adapter never plans frames, spends Bridge budget or changes quality.

use super::*;
use crate::pipeline::inputs::{InputSource, MaterialInput, SourceCheckpoint};
use crate::project::manager::atomic_replace_file;
use crate::video::panorama::VideoProjection;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

struct FrozenVideoPlan {
    selected: Vec<PlannedFrame>,
    /// Exact old project names, indexed by the source-frame index.
    names: BTreeMap<u64, String>,
    has_alpha: bool,
}

fn invalid_plan() -> SplatError {
    SplatError::Process("Saved video preparation plan cannot be safely restored".into())
}

fn frozen_plan(item: &MaterialInput, saved: &SourceCheckpoint) -> Result<FrozenVideoPlan> {
    if !matches!(item.source, InputSource::Video { .. })
        || saved.id != item.id
        || item.identity.as_deref() != Some(saved.identity.as_str())
        || saved.names.is_empty()
    {
        return Err(invalid_plan());
    }
    let video = saved.video.as_ref().ok_or_else(invalid_plan)?;
    let names = saved.names.iter().cloned().collect::<BTreeSet<_>>();
    if names.len() != saved.names.len() || saved.panorama_records.len() != names.len() {
        return Err(invalid_plan());
    }
    if saved
        .frames
        .as_ref()
        .and_then(|frames| frames.extracted_frames)
        .is_some_and(|count| count != names.len() as u64)
    {
        return Err(invalid_plan());
    }
    let mut selected = Vec::with_capacity(names.len());
    let mut mapping = BTreeMap::new();
    let mut seen_names = BTreeSet::new();
    let extension = if video.has_alpha { "png" } else { "jpg" };
    for record in &saved.panorama_records {
        let name = &record.prepared_name;
        let suffix = format!("frame_{:010}.{extension}", record.source_index);
        // Generated video names are flat and carry the decoder index. Never
        // invent a different prefix or map records to filenames by zip order.
        if !names.contains(name)
            || !seen_names.insert(name.clone())
            || name.contains(['/', '\\', ':', '\0', '\n', '\r'])
            || !name.ends_with(&suffix)
            || record.batch_id != saved.id
            || record
                .orientation
                .is_some_and(|orientation| orientation != 1)
            || saved
                .frames
                .as_ref()
                .and_then(|frames| frames.bridge_source_frames)
                .is_some_and(|actual_count| record.source_index >= actual_count)
            || (saved.projection == VideoProjection::Equirectangular && record.pts.is_none())
        {
            return Err(invalid_plan());
        }
        let timestamp = record
            .timestamp_seconds
            .filter(|value| value.is_finite())
            .ok_or_else(invalid_plan)?;
        if mapping.insert(record.source_index, name.clone()).is_some() {
            return Err(invalid_plan());
        }
        selected.push(PlannedFrame {
            source_frame_index: record.source_index,
            timestamp_seconds: timestamp,
        });
    }
    selected.sort_by_key(|frame| frame.source_frame_index);
    Ok(FrozenVideoPlan {
        selected,
        names: mapping,
        has_alpha: video.has_alpha,
    })
}

fn dimensions_from_models(
    paths: &ProjectPaths,
    saved: &SourceCheckpoint,
) -> Result<Option<[u32; 2]>> {
    if let Some([width, height]) = saved.working_dimensions {
        return if width > 0 && height > 0 {
            Ok(Some([width, height]))
        } else {
            Err(invalid_plan())
        };
    }
    let mut dimensions = BTreeSet::new();
    let database = paths.colmap.join("database.db");
    if database.is_file() {
        let db = crate::reconstruction::direct_input::open_immutable_database(&database)?;
        let mut cameras = BTreeSet::new();
        for name in &saved.names {
            let row: (u32, u64, u64) = db.query_row(
                "SELECT images.camera_id,cameras.width,cameras.height FROM images JOIN cameras USING(camera_id) WHERE images.name=?1",
                [name], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            ).map_err(crate::reconstruction::direct_input::database_read_error)?;
            cameras.insert(row.0);
            dimensions.insert([
                u32::try_from(row.1).map_err(|_| invalid_plan())?,
                u32::try_from(row.2).map_err(|_| invalid_plan())?,
            ]);
        }
        if !saved.camera_ids.is_empty() && cameras != saved.camera_ids.iter().copied().collect() {
            return Err(invalid_plan());
        }
    }
    for root in [
        paths.colmap.join("authoritative"),
        paths.colmap.join("base-model"),
    ] {
        if !root.join("cameras.bin").is_file() && !root.join("cameras.txt").is_file() {
            continue;
        }
        let model = crate::reconstruction::direct_input::read_model(&root)?;
        for image in model
            .images
            .iter()
            .filter(|image| saved.names.contains(&image.pose.name))
        {
            if !saved.camera_ids.is_empty() && !saved.camera_ids.contains(&image.pose.camera_id) {
                return Err(invalid_plan());
            }
            let camera = model
                .cameras
                .iter()
                .find(|camera| camera.id == image.pose.camera_id)
                .ok_or_else(invalid_plan)?;
            dimensions.insert([
                u32::try_from(camera.width).map_err(|_| invalid_plan())?,
                u32::try_from(camera.height).map_err(|_| invalid_plan())?,
            ]);
        }
    }
    if dimensions.len() > 1 || dimensions.iter().any(|dimensions| dimensions.contains(&0)) {
        return Err(invalid_plan());
    }
    Ok(dimensions.into_iter().next())
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

async fn install_owned_copy(
    source: &Path,
    target: &Path,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<()> {
    let parent = target.parent().ok_or_else(invalid_plan)?;
    tokio::fs::create_dir_all(parent).await?;
    // Never truncate a destination: it may still be linked to a source project.
    let pending = parent.join(format!(".frame-restore-{}", uuid::Uuid::new_v4()));
    super::reshoot_runner::copy_reshoot_file(source, &pending, cancel).await?;
    let source = source.to_path_buf();
    let copied = pending.clone();
    let token = cancel.clone();
    crate::presets::spawn_pipeline_blocking(move || {
        if hash_file(&source, &token)? != hash_file(&copied, &token)? {
            return Err(invalid_plan());
        }
        Ok::<_, SplatError>(())
    })
    .await
    .map_err(|error| SplatError::Process(error.to_string()))??;
    if cancel.is_cancelled() {
        return Err(SplatError::Cancelled);
    }
    atomic_replace_file(&pending, target).await
}

async fn validate_owned_destinations(paths: &ProjectPaths) -> Result<()> {
    let project = tokio::fs::canonicalize(&paths.project).await?;
    for root in [
        &paths.work,
        &paths.frames,
        &paths.colmap_frames,
        &paths.masks,
    ] {
        tokio::fs::create_dir_all(root).await?;
        let info = tokio::fs::symlink_metadata(root).await?;
        if !info.is_dir()
            || info.file_type().is_symlink()
            || !tokio::fs::canonicalize(root).await?.starts_with(&project)
        {
            return Err(invalid_plan());
        }
    }
    Ok(())
}

impl PipelineRunner {
    /// Restores base and already-adopted Bridge frames together. The returned
    /// checkpoint retains the old plan, names and consumed budget verbatim.
    pub(super) async fn restore_frozen_video_source(
        &self,
        paths: &ProjectPaths,
        item: &MaterialInput,
        saved: &SourceCheckpoint,
    ) -> Result<SourceCheckpoint> {
        let plan = frozen_plan(item, saved)?;
        self.authorize_external_path(item.source.path())?;
        if item.source.identity()? != saved.identity {
            return Err(SplatError::Process("INPUT_CHANGED".into()));
        }
        validate_owned_destinations(paths).await?;
        let owned_paths = paths.clone();
        let owned_saved = saved.clone();
        let dimensions = crate::presets::spawn_pipeline_blocking(move || {
            dimensions_from_models(&owned_paths, &owned_saved)
        })
        .await
        .map_err(|error| SplatError::Process(error.to_string()))??
        .ok_or_else(invalid_plan)?;
        let staging_parent = paths.work.join("prepared-recovery");
        tokio::fs::create_dir_all(&staging_parent).await?;
        let parent_info = tokio::fs::symlink_metadata(&staging_parent).await?;
        if !parent_info.is_dir()
            || parent_info.file_type().is_symlink()
            || !tokio::fs::canonicalize(&staging_parent)
                .await?
                .starts_with(tokio::fs::canonicalize(&paths.work).await?)
        {
            return Err(invalid_plan());
        }
        let staging = staging_parent.join(uuid::Uuid::new_v4().to_string());
        tokio::fs::create_dir_all(&staging).await?;
        self.events.source(Some(&item.id));
        self.events.panorama_phase.store(
            u64::from(saved.projection == VideoProjection::Equirectangular),
            Ordering::Relaxed,
        );
        self.events
            .stage(PipelineStage::ExtractingFrames, 0., "FFmpeg 开始提取画面");
        let extracted = crate::engines::ffmpeg::extract_selected_frames(
            &self.engines.ffmpeg,
            item.source.path(),
            &staging.join("frames"),
            &staging.join("colmap_frames"),
            &staging.join("masks"),
            &plan.selected,
            plan.has_alpha,
            Some((dimensions[0], dimensions[1])),
            Some(paths.logs.join("ffmpeg-prepared-recovery.log")),
            &self.process_manager,
            Some(self.process_observer(
                PipelineStage::ExtractingFrames,
                PipelineEngine::Ffmpeg,
                Some(plan.selected.len() as u64),
                ObserverMode::Ffmpeg,
            )),
        )
        .await?;
        if extracted.frame_count != saved.names.len() as u64
            || [extracted.width, extracted.height] != dimensions
            || extracted.has_alpha != plan.has_alpha
        {
            return Err(invalid_plan());
        }
        // Identity is checked before publishing a single recovered image too.
        if item.source.identity()? != saved.identity {
            return Err(SplatError::Process("INPUT_CHANGED".into()));
        }
        let extension = if plan.has_alpha { "png" } else { "jpg" };
        let cancel = self.process_manager.child_token();
        for frame in &plan.selected {
            let name = plan
                .names
                .get(&frame.source_frame_index)
                .ok_or_else(invalid_plan)?;
            let local = format!("frame_{:010}.{extension}", frame.source_frame_index);
            for (source, target) in [
                (staging.join("frames").join(&local), paths.frames.join(name)),
                (
                    staging.join("colmap_frames").join(&local),
                    paths.colmap_frames.join(name),
                ),
            ] {
                install_owned_copy(&source, &target, &cancel).await?;
            }
            if plan.has_alpha {
                install_owned_copy(
                    &staging.join("masks").join(format!("{local}.png")),
                    &paths.masks.join(format!("{name}.png")),
                    &cancel,
                )
                .await?;
            }
        }
        if item.source.identity()? != saved.identity {
            return Err(SplatError::Process("INPUT_CHANGED".into()));
        }
        let mut restored = saved.clone();
        restored.working_dimensions = Some(dimensions);
        restored.adopted = 0;
        // Registration belongs to the invalidated DB; it will be established
        // again by the shared feature/reconstruction operation, not this copy.
        restored.camera_ids.clear();
        restored.camera_assignment = None;
        // Only this freshly allocated UUID directory is owned by this call.
        // Failed attempts are retained for diagnosis; successful copies need
        // not keep a second full set of video images on disk.
        let _ = tokio::fs::remove_dir_all(&staging).await;
        Ok(restored)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::{inputs::InputSource, reshoot::PanoramaFrameRecord};

    fn fixture() -> (MaterialInput, SourceCheckpoint) {
        let item = MaterialInput {
            id: "video".into(),
            source: InputSource::Video {
                path: PathBuf::from("video.mov"),
            },
            identity: Some("identity".into()),
            projection: VideoProjection::Perspective,
            image_matching_strategy: Default::default(),
            camera_relation: None,
            confirmed_small_images: true,
            confirmed_image_order: true,
            frame_removal_fps: None,
        };
        let names = [
            "prefix_frame_0000000002.jpg",
            "prefix_frame_0000000007.jpg",
            "prefix_frame_0000000004.jpg",
        ]
        .map(str::to_owned)
        .to_vec();
        let records = [2u64, 7, 4]
            .into_iter()
            .zip(&names)
            .map(|(index, name)| PanoramaFrameRecord {
                prepared_name: name.clone(),
                batch_id: "video".into(),
                source_name: "video.mov".into(),
                source_sha256: String::new(),
                orientation: Some(1),
                source_index: index,
                timestamp_seconds: Some(index as f64 / 30.),
                pts: Some(index as i64),
            })
            .collect();
        let saved = SourceCheckpoint {
            id: "video".into(),
            identity: "identity".into(),
            camera_ids: vec![7],
            camera_assignment: None,
            projection: VideoProjection::Perspective,
            names,
            originals: vec![],
            video: Some(VideoInfo {
                duration: 1.,
                width: 1920,
                height: 1080,
                fps: 30.,
                total_frames: 30,
                codec: "h264".into(),
                rotation: 0,
                pixel_format: "yuv420p".into(),
                has_alpha: false,
            }),
            image_sequence: None,
            dimensions: vec![[1920, 1080]],
            working_dimensions: Some([960, 540]),
            frames: Some(FrameState {
                extracted_frames: Some(3),
                initial_extracted_frames: 2,
                rescue_max_frames: 3,
                ..Default::default()
            }),
            panorama_records: records,
            adopted: 1,
            bridge_frames: vec![PlannedFrame {
                source_frame_index: 4,
                timestamp_seconds: 4. / 30.,
            }],
        };
        (item, saved)
    }

    #[test]
    fn restores_adopted_bridge_indices_and_exact_names_without_new_selection() {
        let (item, saved) = fixture();
        let before = serde_json::to_value(&saved).unwrap();
        let plan = frozen_plan(&item, &saved).unwrap();
        assert_eq!(
            plan.selected
                .iter()
                .map(|frame| frame.source_frame_index)
                .collect::<Vec<_>>(),
            [2, 4, 7]
        );
        assert_eq!(plan.names[&4], "prefix_frame_0000000004.jpg");
        assert_eq!(serde_json::to_value(&saved).unwrap(), before);
    }

    #[test]
    fn incomplete_conflicting_or_unsafe_frozen_mappings_are_rejected() {
        let (item, saved) = fixture();
        let mut damaged = saved.clone();
        damaged.panorama_records.pop();
        assert!(frozen_plan(&item, &damaged).is_err());
        let mut damaged = saved.clone();
        damaged.panorama_records[0].source_index = 7;
        assert!(frozen_plan(&item, &damaged).is_err());
        let mut damaged = saved.clone();
        damaged.panorama_records[0].timestamp_seconds = Some(f64::NAN);
        assert!(frozen_plan(&item, &damaged).is_err());
        let mut damaged = saved.clone();
        damaged.names[0] = "../prefix_frame_0000000002.jpg".into();
        damaged.panorama_records[0].prepared_name = damaged.names[0].clone();
        assert!(frozen_plan(&item, &damaged).is_err());
    }

    #[test]
    fn erp_requires_saved_real_pts_and_original_dimensions_are_not_work_dimensions() {
        let (mut item, mut saved) = fixture();
        saved.projection = VideoProjection::Equirectangular;
        item.projection = VideoProjection::Equirectangular;
        saved.panorama_records[0].pts = None;
        assert!(frozen_plan(&item, &saved).is_err());
        let root = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::existing(uuid::Uuid::new_v4(), root.path().to_path_buf());
        assert_eq!(
            dimensions_from_models(&paths, &saved).unwrap(),
            Some([960, 540])
        );
        saved.working_dimensions = None;
        assert_eq!(dimensions_from_models(&paths, &saved).unwrap(), None);
    }

    #[tokio::test]
    async fn replacing_a_working_hard_link_does_not_rewrite_its_source() {
        let root = tempfile::tempdir().unwrap();
        let original = root.path().join("original.png");
        let target = root.path().join("working.png");
        let staged = root.path().join("recovered.png");
        tokio::fs::write(&original, b"original pixels")
            .await
            .unwrap();
        tokio::fs::hard_link(&original, &target).await.unwrap();
        tokio::fs::write(&staged, b"recovered pixels")
            .await
            .unwrap();
        install_owned_copy(
            &staged,
            &target,
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(
            tokio::fs::read(&original).await.unwrap(),
            b"original pixels"
        );
        assert_eq!(tokio::fs::read(&target).await.unwrap(), b"recovered pixels");
        assert_eq!(tokio::fs::read(&staged).await.unwrap(), b"recovered pixels");
    }
}
