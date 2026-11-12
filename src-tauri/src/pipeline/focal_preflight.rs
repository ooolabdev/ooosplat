//! Read-only collection preflight for the temporary PNG/XMP focal workaround.
//!
//! Keep the group selection shared with feature extraction: a cached database
//! cannot skip policy checks, and preflight must inspect the actual first work
//! image of each newly initialized shared camera, never an arbitrary original.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{
    error::{Result, SplatError},
    process::ProcessManager,
    project::{PipelineStateFile, ProjectPaths},
    reconstruction::{colmap_model::read_cameras, direct_input::open_immutable_database},
    video::{
        panorama::VideoProjection,
        temporary_png_focal_workaround::{self as workaround, Checkpoint, FocalOverride, Request},
    },
};

use super::{
    execution_plan::MatchPlan,
    inputs::{GenerationInputs, InputCheckpoint, InputSource, SourceCheckpoint},
    reshoot::CameraRelation,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FocalRecord {
    pub source_id: String,
    pub group_index: usize,
    pub checkpoint: Checkpoint,
    pub raw_focal_length: String,
    pub raw_x_resolution: String,
    pub unit: u32,
}

impl FocalRecord {
    fn detected(source_id: &str, group_index: usize, focal: FocalOverride) -> Self {
        Self {
            source_id: source_id.into(),
            group_index,
            checkpoint: focal.checkpoint,
            raw_focal_length: focal.raw_focal_length,
            raw_x_resolution: focal.raw_x_resolution,
            unit: focal.unit,
        }
    }

    pub fn camera_params(&self) -> String {
        self.checkpoint
            .camera_params
            .map(|value| value.to_string())
            .join(",")
    }

    pub fn log_summary(&self) -> String {
        format!(
            "[TemporaryPngFocal] enabled=true source={} group={} first={} focal={} xres={} unit={} camera_params={}",
            self.source_id,
            self.group_index,
            self.checkpoint.image_name,
            self.raw_focal_length,
            self.raw_x_resolution,
            self.unit,
            self.camera_params(),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FeatureGroups {
    pub existing_camera_id: Option<u32>,
    pub groups: Vec<Vec<String>>,
}

/// Record the replacement policy before moving either cache. Keep the saved
/// training recipe and consumed rescue allowances: an incompatible recipe is
/// an executor error, not permission to silently adopt a new one.
pub(crate) fn begin_collection_focal_update(
    state: &mut PipelineStateFile,
    records: &[FocalRecord],
) -> bool {
    let Some(checkpoint) = state.inputs.as_mut() else {
        return false;
    };
    if checkpoint.temporary_png_focal_pending_records.is_some()
        || checkpoint.temporary_png_focal_records.as_deref() == Some(records)
    {
        return false;
    }
    checkpoint.temporary_png_focal_pending_records = Some(records.to_vec());
    checkpoint.reference_image = None;
    state.features_complete = false;
    state.matching_complete = false;
    state.reconstruction_complete = false;
    state.brush_complete = false;
    state.validated_output = None;
    state.stage = super::PipelineStage::ExtractingFeatures;
    state
        .artifact_manifests
        .entries
        .retain(|stage, _| stage == "prepared");
    for cache in ["brush", "colmap"] {
        if !state.retry_cache_pending.iter().any(|name| name == cache) {
            state.retry_cache_pending.push(cache.into());
        }
    }
    true
}

/// Shared by policy preflight and the actual COLMAP extraction call. The saved
/// policy keeps old photo tasks independent. New photo tasks share a camera
/// within each source only after checking the actual prepared pixel dimensions;
/// videos, ERP and explicit compatible old-camera reuse keep their own rules.
pub(crate) fn collection_feature_groups(
    paths: &ProjectPaths,
    checkpoint: &InputCheckpoint,
    source: &SourceCheckpoint,
) -> Result<FeatureGroups> {
    if source.id == "__source" {
        return Ok(FeatureGroups {
            existing_camera_id: None,
            groups: vec![],
        });
    }
    let reshoot = checkpoint
        .execution_plan
        .as_ref()
        .is_some_and(|plan| plan.matching == MatchPlan::ReshootCrossSource);
    let relation = match &checkpoint.request {
        GenerationInputs::Materials { items } => items
            .iter()
            .find(|item| item.id == source.id)
            .and_then(|item| item.camera_relation),
        GenerationInputs::Colmap { .. } => None,
    };
    let mut existing_camera_id = None;
    if reshoot
        && source.projection == VideoProjection::Perspective
        && relation == Some(CameraRelation::Same)
    {
        let cameras = read_cameras(&paths.colmap.join("base-model"))?;
        let connection = open_immutable_database(&paths.colmap.join("base-database.db"))?;
        let count: u64 = connection
            .query_row("SELECT COUNT(*) FROM cameras", [], |row| row.get(0))
            .map_err(crate::reconstruction::direct_input::database_read_error)?;
        if count == 1
            && cameras.len() == 1
            && cameras[0].model_id != 17
            && source
                .video
                .as_ref()
                .is_none_or(|video| video.rotation.rem_euclid(360) == 0)
        {
            let mut compatible = true;
            if source.video.is_none() {
                for original in &source.originals {
                    compatible &=
                        crate::video::image_projection::inspect(original)?.orientation == 1;
                }
            }
            if compatible {
                for name in &source.names {
                    let path = crate::reconstruction::direct_input::image_path(
                        &paths.colmap_frames,
                        name,
                    )?;
                    let header = crate::video::image_projection::inspect(&path)?;
                    compatible &= (
                        u64::from(header.width),
                        u64::from(header.height),
                        header.orientation,
                    ) == (cameras[0].width, cameras[0].height, 1);
                }
            }
            if compatible {
                existing_camera_id = Some(cameras[0].id);
            }
        }
    }
    let groups = if source.video.is_some()
        || source.projection == VideoProjection::Equirectangular
        || existing_camera_id.is_some()
    {
        vec![source.names.clone()]
    } else if checkpoint.camera_grouping_policy_version
        == Some(super::inputs::CAMERA_GROUPING_POLICY_VERSION)
    {
        validate_shared_source_dimensions(paths, source)?;
        vec![source.names.clone()]
    } else {
        source.names.iter().map(|name| vec![name.clone()]).collect()
    };
    Ok(FeatureGroups {
        existing_camera_id,
        groups,
    })
}

fn validate_shared_source_dimensions(
    paths: &ProjectPaths,
    source: &SourceCheckpoint,
) -> Result<()> {
    let mut expected = None;
    for name in &source.names {
        let path = crate::reconstruction::direct_input::image_path(&paths.colmap_frames, name)?;
        let header = crate::video::image_projection::inspect(&path)?;
        // The shared reader exposes EXIF-oriented dimensions. Camera dimensions
        // describe the actual encoded work pixels, not an untrusted source or
        // analysis summary. Normally preparation has already removed rotation.
        let current = if (5..=8).contains(&header.orientation) {
            (header.height, header.width)
        } else {
            (header.width, header.height)
        };
        if let Some((width, height)) = expected {
            if current != (width, height) {
                return Err(SplatError::InvalidVideo(format!(
                    "图片序列分辨率不一致：{} 为 {}×{}，预期 {}×{}",
                    path.display(),
                    current.0,
                    current.1,
                    width,
                    height,
                )));
            }
        } else {
            expected = Some(current);
        }
    }
    if expected.is_none() {
        return Err(SplatError::InvalidVideo(
            "图片序列文件夹中至少需要 2 张 JPG、JPEG 或 PNG 图片".into(),
        ));
    }
    Ok(())
}

fn ordinary_image_source(checkpoint: &InputCheckpoint, source: &SourceCheckpoint) -> bool {
    source.id != "__source"
        && source.video.is_none()
        && source.projection == VideoProjection::Perspective
        && matches!(&checkpoint.request, GenerationInputs::Materials { items }
            if items.iter().any(|item| item.id == source.id
                && matches!(&item.source, InputSource::ImageFolder { .. } | InputSource::Images { .. })))
}

/// Run before deciding whether completed feature/database checkpoints may be
/// reused. `Some` contains only groups demonstrably affected by the original
/// workaround; `None` never asks the caller to invalidate a normal project.
/// This function does not mutate state, emit image-list files or archive data.
pub(crate) async fn inspect_collection_focal(
    executable: &Path,
    manager: &ProcessManager,
    paths: &ProjectPaths,
    state: &PipelineStateFile,
) -> Result<Option<Vec<FocalRecord>>> {
    let Some(checkpoint) = state.inputs.as_ref() else {
        return Ok(None);
    };
    if !checkpoint.prepared || checkpoint.imported_reconstruction {
        return Ok(None);
    }
    let mut records = Vec::new();
    for source in &checkpoint.sources {
        if manager.child_token().is_cancelled() {
            return Err(SplatError::Cancelled);
        }
        if !ordinary_image_source(checkpoint, source) {
            continue;
        }
        if source
            .camera_assignment
            .as_ref()
            .is_some_and(|assignment| assignment.reused_camera_id.is_some())
        {
            // That extraction used existing_camera_id, not camera_params. Do
            // not reinterpret a saved calibration as a new PNG initialization.
            continue;
        }
        let groups = collection_feature_groups(paths, checkpoint, source)?;
        if groups.existing_camera_id.is_some() {
            continue;
        }
        for (index, names) in groups.groups.iter().enumerate() {
            if manager.child_token().is_cancelled() {
                return Err(SplatError::Cancelled);
            }
            let request = Request {
                images: &paths.colmap_frames,
                image_list: None,
                ordinary_images: true,
                camera_model: "SIMPLE_RADIAL",
                single_camera: true,
                explicit_params: None,
                existing_camera_id: None,
                calibrated: false,
            };
            let focal = match source.ordinary_image_preparation_version {
                Some(version) => {
                    workaround::detect_derived_image_names(
                        executable,
                        manager,
                        request,
                        names,
                        &paths.frames,
                        &paths.masks,
                        version,
                    )
                    .await
                }
                None => workaround::detect_image_names(executable, manager, request, names).await,
            };
            if manager.child_token().is_cancelled() {
                return Err(SplatError::Cancelled);
            }
            if let Some(focal) = focal {
                records.push(FocalRecord::detected(&source.id, index, focal));
            }
        }
    }
    Ok((!records.is_empty()).then_some(records))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        pipeline::inputs::MaterialInput, presets::Quality, project::ImageMatchingStrategy,
    };
    use std::path::PathBuf;
    use uuid::Uuid;

    fn state(source: InputSource, projection: VideoProjection) -> PipelineStateFile {
        let mut checkpoint = InputCheckpoint::new(GenerationInputs::Materials {
            items: vec![MaterialInput {
                id: "images".into(),
                source,
                projection,
                identity: None,
                image_matching_strategy: ImageMatchingStrategy::Exhaustive,
                camera_relation: None,
                confirmed_small_images: true,
                confirmed_image_order: false,
                frame_removal_fps: None,
            }],
        });
        checkpoint.prepared = true;
        checkpoint.sources.push(SourceCheckpoint {
            id: "images".into(),
            identity: "source-identity".into(),
            ordinary_image_preparation_version: None,
            camera_ids: vec![],
            camera_assignment: None,
            projection,
            names: vec!["source_2.png".into(), "source_10.png".into()],
            originals: vec![],
            video: None,
            image_sequence: None,
            dimensions: vec![],
            working_dimensions: None,
            frames: None,
            panorama_records: vec![],
            adopted: 0,
            bridge_frames: vec![],
        });
        let mut state = PipelineStateFile::created(Quality::Balanced);
        state.inputs = Some(checkpoint);
        state.features_complete = true;
        state
    }

    #[test]
    fn image_camera_groups_are_identical_before_and_after_feature_completion() {
        let root = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::existing(Uuid::new_v4(), root.path().into());
        let mut state = state(
            InputSource::ImageFolder {
                path: PathBuf::from("images"),
            },
            VideoProjection::Perspective,
        );
        let checkpoint = state.inputs.as_ref().unwrap();
        let groups = collection_feature_groups(&paths, checkpoint, &checkpoint.sources[0]).unwrap();
        assert_eq!(groups.existing_camera_id, None);
        assert_eq!(
            groups.groups,
            vec![vec!["source_2.png"], vec!["source_10.png"]]
        );
        assert!(ordinary_image_source(checkpoint, &checkpoint.sources[0]));
        state.features_complete = false;
        let checkpoint = state.inputs.as_ref().unwrap();
        assert_eq!(
            groups,
            collection_feature_groups(&paths, checkpoint, &checkpoint.sources[0]).unwrap()
        );
    }

    fn write_work_images(paths: &ProjectPaths, names: &[String], width: u32, height: u32) {
        std::fs::create_dir_all(&paths.colmap_frames).unwrap();
        for name in names {
            image::RgbImage::new(width, height)
                .save(paths.colmap_frames.join(name))
                .unwrap();
        }
    }

    #[test]
    fn saved_shared_policy_initializes_one_camera_for_the_whole_photo_source() {
        let root = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::existing(Uuid::new_v4(), root.path().into());
        for source in [
            InputSource::ImageFolder {
                path: "images".into(),
            },
            InputSource::Images {
                paths: vec!["a.png".into(), "b.png".into()],
            },
        ] {
            let mut state = state(source, VideoProjection::Perspective);
            let checkpoint = state.inputs.as_mut().unwrap();
            checkpoint.camera_grouping_policy_version = Some(1);
            write_work_images(&paths, &checkpoint.sources[0].names, 4, 6);
            let grouped =
                collection_feature_groups(&paths, checkpoint, &checkpoint.sources[0]).unwrap();
            assert_eq!(grouped.existing_camera_id, None);
            assert_eq!(grouped.groups, vec![checkpoint.sources[0].names.clone()]);
            state.features_complete = false;
            let checkpoint = state.inputs.as_ref().unwrap();
            assert_eq!(
                grouped,
                collection_feature_groups(&paths, checkpoint, &checkpoint.sources[0]).unwrap()
            );
        }
    }

    #[test]
    fn shared_photo_cameras_never_merge_different_sources_or_require_equal_source_sizes() {
        let root = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::existing(Uuid::new_v4(), root.path().into());
        let mut state = state(
            InputSource::ImageFolder {
                path: "first".into(),
            },
            VideoProjection::Perspective,
        );
        let checkpoint = state.inputs.as_mut().unwrap();
        checkpoint.camera_grouping_policy_version = Some(1);
        let mut second = checkpoint.sources[0].clone();
        second.id = "second".into();
        second.identity = "another-source".into();
        second.names = vec!["second_2.png".into(), "second_10.png".into()];
        write_work_images(&paths, &checkpoint.sources[0].names, 4, 6);
        write_work_images(&paths, &second.names, 8, 6);
        checkpoint.sources.push(second);
        let groups = checkpoint
            .sources
            .iter()
            .map(|source| collection_feature_groups(&paths, checkpoint, source).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(groups[0].groups, vec![checkpoint.sources[0].names.clone()]);
        assert_eq!(groups[1].groups, vec![checkpoint.sources[1].names.clone()]);
        assert!(groups
            .iter()
            .all(|group| group.existing_camera_id.is_none()));
        assert!(!groups[0].groups[0]
            .iter()
            .any(|name| groups[1].groups[0].contains(name)));
    }

    #[test]
    fn shared_photo_policy_rejects_mixed_real_work_dimensions_not_just_stale_metadata() {
        let root = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::existing(Uuid::new_v4(), root.path().into());
        let mut state = state(
            InputSource::ImageFolder {
                path: "images".into(),
            },
            VideoProjection::Perspective,
        );
        let checkpoint = state.inputs.as_mut().unwrap();
        checkpoint.camera_grouping_policy_version = Some(1);
        checkpoint.sources[0].working_dimensions = Some([4, 6]);
        checkpoint.sources[0].dimensions = vec![[4, 6], [4, 6]];
        write_work_images(&paths, &checkpoint.sources[0].names, 4, 6);
        image::RgbImage::new(5, 6)
            .save(paths.colmap_frames.join(&checkpoint.sources[0].names[1]))
            .unwrap();
        let error =
            collection_feature_groups(&paths, checkpoint, &checkpoint.sources[0]).unwrap_err();
        assert!(matches!(error, SplatError::InvalidVideo(_)));
        assert!(error.to_string().contains("图片序列分辨率不一致"));
        assert!(error.to_string().contains("5×6，预期 4×6"));
        assert!(!paths.colmap.join("database.db").exists());
        checkpoint.camera_grouping_policy_version = None;
        assert_eq!(
            collection_feature_groups(&paths, checkpoint, &checkpoint.sources[0])
                .unwrap()
                .groups
                .len(),
            2
        );
    }

    #[test]
    fn different_or_unsure_reshoot_settings_keep_new_sources_shared_but_not_the_old_camera() {
        let root = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::existing(Uuid::new_v4(), root.path().into());
        for relation in [CameraRelation::Different, CameraRelation::Unsure] {
            let mut state = state(
                InputSource::ImageFolder {
                    path: "new-images".into(),
                },
                VideoProjection::Perspective,
            );
            let checkpoint = state.inputs.as_mut().unwrap();
            checkpoint.camera_grouping_policy_version = Some(1);
            let mut plan = super::super::execution_plan::ExecutionPlan::normal();
            plan.matching = MatchPlan::ReshootCrossSource;
            checkpoint.execution_plan = Some(plan);
            if let GenerationInputs::Materials { items } = &mut checkpoint.request {
                items[0].camera_relation = Some(relation);
            }
            write_work_images(&paths, &checkpoint.sources[0].names, 4, 6);
            let grouped =
                collection_feature_groups(&paths, checkpoint, &checkpoint.sources[0]).unwrap();
            assert_eq!(grouped.existing_camera_id, None);
            assert_eq!(grouped.groups, vec![checkpoint.sources[0].names.clone()]);
        }
    }

    #[test]
    fn saved_shared_policy_does_not_change_video_erp_or_old_source_grouping() {
        let root = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::existing(Uuid::new_v4(), root.path().into());
        let mut state = state(
            InputSource::ImageFolder {
                path: "images".into(),
            },
            VideoProjection::Perspective,
        );
        let checkpoint = state.inputs.as_mut().unwrap();
        checkpoint.camera_grouping_policy_version = Some(1);
        checkpoint.sources[0].projection = VideoProjection::Equirectangular;
        let erp = collection_feature_groups(&paths, checkpoint, &checkpoint.sources[0]).unwrap();
        assert_eq!(erp.groups, vec![checkpoint.sources[0].names.clone()]);
        checkpoint.sources[0].projection = VideoProjection::Perspective;
        checkpoint.sources[0].video = Some(crate::video::VideoInfo {
            width: 4,
            height: 6,
            total_frames: 60,
            fps: 30.0,
            duration: 2.0,
            codec: "h264".into(),
            rotation: 0,
            pixel_format: "yuv420p".into(),
            has_alpha: false,
        });
        let video = collection_feature_groups(&paths, checkpoint, &checkpoint.sources[0]).unwrap();
        assert_eq!(video.groups, vec![checkpoint.sources[0].names.clone()]);
        checkpoint.sources[0].id = "__source".into();
        assert!(
            collection_feature_groups(&paths, checkpoint, &checkpoint.sources[0])
                .unwrap()
                .groups
                .is_empty()
        );
    }

    #[tokio::test]
    async fn videos_erp_and_imported_sources_never_enter_the_workaround() {
        let root = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::existing(Uuid::new_v4(), root.path().into());
        let manager = ProcessManager::new();
        for (source, projection) in [
            (
                InputSource::Video {
                    path: "clip.mov".into(),
                },
                VideoProjection::Perspective,
            ),
            (
                InputSource::ImageFolder { path: "erp".into() },
                VideoProjection::Equirectangular,
            ),
        ] {
            let state = state(source, projection);
            assert!(!ordinary_image_source(
                state.inputs.as_ref().unwrap(),
                &state.inputs.as_ref().unwrap().sources[0]
            ));
            assert!(inspect_collection_focal(
                Path::new("missing-engine"),
                &manager,
                &paths,
                &state
            )
            .await
            .unwrap()
            .is_none());
        }
        let mut state = state(
            InputSource::Images {
                paths: vec!["a.png".into(), "b.png".into()],
            },
            VideoProjection::Perspective,
        );
        state.inputs.as_mut().unwrap().imported_reconstruction = true;
        assert!(
            inspect_collection_focal(Path::new("missing-engine"), &manager, &paths, &state)
                .await
                .unwrap()
                .is_none()
        );
        state.inputs.as_mut().unwrap().imported_reconstruction = false;
        state.inputs.as_mut().unwrap().sources[0].id = "__source".into();
        assert!(
            inspect_collection_focal(Path::new("missing-engine"), &manager, &paths, &state)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn explicitly_reused_camera_skips_png_override_and_checks_real_work_pixels() {
        let root = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::existing(Uuid::new_v4(), root.path().into());
        std::fs::create_dir_all(paths.colmap.join("base-model")).unwrap();
        std::fs::create_dir_all(&paths.colmap_frames).unwrap();
        std::fs::write(
            paths.colmap.join("base-model/cameras.txt"),
            "1 SIMPLE_RADIAL 4 4 4 2 2 0\n",
        )
        .unwrap();
        let db = rusqlite::Connection::open(paths.colmap.join("base-database.db")).unwrap();
        db.execute_batch("CREATE TABLE cameras(camera_id INTEGER); INSERT INTO cameras VALUES(1);")
            .unwrap();
        drop(db);
        let mut state = state(
            InputSource::ImageFolder {
                path: "images".into(),
            },
            VideoProjection::Perspective,
        );
        let checkpoint = state.inputs.as_mut().unwrap();
        let mut plan = super::super::execution_plan::ExecutionPlan::normal();
        plan.matching = MatchPlan::ReshootCrossSource;
        checkpoint.execution_plan = Some(plan);
        if let GenerationInputs::Materials { items } = &mut checkpoint.request {
            items[0].camera_relation = Some(CameraRelation::Same);
        }
        for name in &checkpoint.sources[0].names {
            image::RgbaImage::from_pixel(4, 4, image::Rgba([1, 2, 3, 255]))
                .save(paths.colmap_frames.join(name))
                .unwrap();
        }
        let groups = collection_feature_groups(&paths, checkpoint, &checkpoint.sources[0]).unwrap();
        assert_eq!(groups.existing_camera_id, Some(1));
        assert_eq!(groups.groups, vec![checkpoint.sources[0].names.clone()]);
        let mut shared_checkpoint = checkpoint.clone();
        shared_checkpoint.camera_grouping_policy_version = Some(1);
        assert_eq!(
            groups,
            collection_feature_groups(&paths, &shared_checkpoint, &shared_checkpoint.sources[0])
                .unwrap(),
        );
        assert!(inspect_collection_focal(
            Path::new("missing-engine"),
            &ProcessManager::new(),
            &paths,
            &state,
        )
        .await
        .unwrap()
        .is_none());
        image::RgbaImage::from_pixel(5, 4, image::Rgba([1, 2, 3, 255]))
            .save(paths.colmap_frames.join("source_2.png"))
            .unwrap();
        let checkpoint = state.inputs.as_ref().unwrap();
        let groups = collection_feature_groups(&paths, checkpoint, &checkpoint.sources[0]).unwrap();
        assert_eq!(groups.existing_camera_id, None);
        assert_eq!(groups.groups.len(), 2);
        let mut shared_checkpoint = checkpoint.clone();
        shared_checkpoint.camera_grouping_policy_version = Some(1);
        assert!(collection_feature_groups(
            &paths,
            &shared_checkpoint,
            &shared_checkpoint.sources[0]
        )
        .is_err());
    }

    #[test]
    fn records_bind_the_actual_group_image_and_round_trip_optionally() {
        let mut state = state(
            InputSource::ImageFolder {
                path: "images".into(),
            },
            VideoProjection::Perspective,
        );
        let record = FocalRecord::detected(
            "images",
            1,
            FocalOverride {
                checkpoint: Checkpoint {
                    version: 1,
                    image_name: "source_10.png".into(),
                    sha256: "a".repeat(64),
                    width: 8192,
                    height: 5464,
                    camera_params: [22728.65980253445, 4096., 2732., 0.],
                },
                raw_focal_length: "100/1".into(),
                raw_x_resolution: "8192000/1419".into(),
                unit: 2,
            },
        );
        assert_eq!(record.camera_params(), "22728.65980253445,4096,2732,0");
        let old = serde_json::to_value(&state).unwrap();
        assert!(old["inputs"].get("temporaryPngFocalRecords").is_none());
        state.inputs.as_mut().unwrap().temporary_png_focal_records = Some(vec![record.clone()]);
        let restored: PipelineStateFile =
            serde_json::from_value(serde_json::to_value(&state).unwrap()).unwrap();
        assert_eq!(
            restored.inputs.unwrap().temporary_png_focal_records,
            Some(vec![record])
        );
    }
}
