//! Reuse supported native training views; derive ERP/compatibility views only.
use super::*;
use crate::reconstruction::direct_input::{read_model, Model};
use crate::video::panorama;

#[derive(Serialize, Deserialize)]
pub(super) struct Dataset {
    pub path: PathBuf,
    pub has_erp: bool,
    pub max_edge: u32,
    pub reference: String,
    #[serde(default)]
    pub derivation_version: u32,
}

const DATASET_DERIVATION_VERSION: u32 = 2;

#[cfg(test)]
fn registered_pixel_identity(
    pixels: Vec<(String, PathBuf)>,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<String> {
    registered_pixel_identity_observed(pixels, cancel, None)
}

fn registered_pixel_identity_observed(
    mut pixels: Vec<(String, PathBuf)>,
    cancel: &tokio_util::sync::CancellationToken,
    observer: Option<&(dyn Fn(u64, u64) + Send + Sync)>,
) -> Result<String> {
    use sha2::{Digest, Sha256};
    pixels.sort_by(|a, b| a.0.cmp(&b.0));
    let mut digest = Sha256::new();
    let total = pixels.len() as u64;
    for (index, (name, path)) in pixels.into_iter().enumerate() {
        digest.update((name.len() as u64).to_le_bytes());
        digest.update(name.as_bytes());
        digest.update(crate::video::panorama::fingerprint_cancellable(
            &path, cancel,
        )?);
        if let Some(observer) = observer {
            observer(index as u64 + 1, total);
        }
    }
    Ok(format!("{:x}", digest.finalize()))
}

struct PreparationProgress<'a> {
    events: &'a EventSink,
    activity: super::application_activity::ApplicationActivity,
    total: u64,
    last: Option<Instant>,
}
impl<'a> PreparationProgress<'a> {
    fn new(
        events: &'a EventSink,
        total: usize,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Self {
        let mut progress = Self {
            events,
            activity: super::application_activity::ApplicationActivity::new(
                events,
                PipelineStage::TrainingSplats,
                cancel,
                "正在准备训练图片",
                None,
            ),
            total: total as u64,
            last: None,
        };
        progress.update(0);
        progress
    }
    fn update(&mut self, current: u64) {
        self.activity.touch();
        if current != self.total && self.last.is_some_and(|last| last.elapsed().as_secs() < 1) {
            return;
        }
        self.last = Some(Instant::now());
        self.events.send(
            PipelineStage::TrainingSplats,
            Some(PipelineEngine::System),
            EventKind::Progress,
            EventLevel::Info,
            Some(current as f32 / self.total.max(1) as f32 * 0.95),
            true,
            format!("正在准备训练图片 {current}/{}", self.total),
            Some(current),
            Some(self.total),
            Some("images"),
        );
    }
}

/// Remove an old destination before writing: it may be a hard link to immutable
/// prepared input from an interrupted previous derivation.
async fn unlink_destination(path: &Path) -> Result<()> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

async fn reuse_prepared_image(source: &Path, destination: &Path) -> Result<()> {
    tokio::fs::create_dir_all(destination.parent().unwrap()).await?;
    unlink_destination(destination).await?;
    if tokio::fs::hard_link(source, destination).await.is_err() {
        tokio::fs::copy(source, destination).await?;
    }
    Ok(())
}
fn prune(model: &mut Model, keep: impl Fn(&ColmapCamera) -> bool) {
    model.cameras.retain(keep);
    model
        .images
        .retain(|i| model.cameras.iter().any(|c| c.id == i.pose.camera_id));
    let ids = model
        .images
        .iter()
        .map(|i| i.pose.id)
        .collect::<HashSet<_>>();
    for p in &mut model.points {
        p.track.retain(|(id, _)| ids.contains(id));
    }
}

/// Partition the already parsed and validated authority. Both projections
/// retain the exact poses, observations and sparse point properties; only
/// camera/image membership and the corresponding tracks are filtered.
fn training_model_subset(authority: &Model, erp: bool) -> Model {
    let mut model = Model {
        cameras: authority.cameras.clone(),
        images: authority.images.clone(),
        points: authority.points.clone(),
    };
    prune(&mut model, |camera| (camera.model_id == 17) == erp);
    model
}
impl PipelineRunner {
    pub(super) async fn input_dataset(
        &self,
        paths: &ProjectPaths,
        model: &Path,
        quality: Quality,
        records: &[crate::pipeline::reshoot::PanoramaFrameRecord],
        order: &[String],
    ) -> Result<Dataset> {
        let executable = self.engines.brush.clone();
        let native_runtime = crate::presets::spawn_pipeline_blocking(move || {
            brush::supports_native_colmap(&executable, 0)
        })
        .await
        .map_err(|error| SplatError::Process(error.to_string()))?;
        Box::pin(self.input_dataset_with_native_runtime(
            paths,
            model,
            quality,
            records,
            order,
            native_runtime,
        ))
        .await
        .map_err(|error| {
            error.with_io_diagnostic(
            "training_preparation_operation=prepare_training_dataset\npath_role=training_dataset"
        )
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn input_dataset_with_native_runtime(
        &self,
        paths: &ProjectPaths,
        model: &Path,
        quality: Quality,
        records: &[crate::pipeline::reshoot::PanoramaFrameRecord],
        order: &[String],
        native_runtime: bool,
    ) -> Result<Dataset> {
        if self.process_manager.child_token().is_cancelled() {
            return Err(SplatError::Cancelled);
        }
        let authority = read_model(model)?;
        let has_erp = authority.images.iter().any(|i| {
            authority
                .cameras
                .iter()
                .any(|c| c.id == i.pose.camera_id && c.model_id == 17)
        });
        self.events.training_substage("trainingPreparation");
        self.events.send(
            PipelineStage::TrainingSplats,
            Some(PipelineEngine::System),
            EventKind::Stage,
            EventLevel::Info,
            None,
            true,
            "正在准备训练图片",
            None,
            None,
            None,
        );
        let root = paths.brush.join("dataset");
        self.events.training_substage("trainingValidation");
        let verification = Arc::new(self.application_activity(
            PipelineStage::TrainingSplats,
            "正在校验训练图片和相机数据",
            Some(authority.images.len() as u64),
        ));
        let model_identity = super::gps_runner::model_identity(model)?;
        let legacy_key = serde_json::to_string(&(
            &model_identity,
            crate::pipeline::inputs::InputSource::Images {
                paths: authority
                    .images
                    .iter()
                    .map(|i| paths.frames.join(&i.pose.name))
                    .collect(),
            }
            .identity()?,
            order,
            quality,
            crate::video::panorama::CONVERSION_VERSION,
        ))?;
        let old_key =
            serde_json::to_string(&(DATASET_DERIVATION_VERSION, &legacy_key, native_runtime))?;
        // A recipe binds pixels and camera coordinates, not copy timestamps.
        // Rebuilding an identical prepared frame must not alter the recipe's
        // identity solely because its mtime changed.
        let pixels = authority
            .images
            .iter()
            .map(|image| (image.pose.name.clone(), paths.frames.join(&image.pose.name)))
            .collect::<Vec<_>>();
        let cancel = self.process_manager.child_token();
        let progress = verification.clone();
        let content_identity = crate::presets::spawn_pipeline_blocking(move || {
            registered_pixel_identity_observed(
                pixels,
                &cancel,
                Some(&|current, total| progress.update(current, total)),
            )
        })
        .await
        .map_err(|error| SplatError::Process(error.to_string()))??;
        let key = serde_json::to_string(&(
            "content-bound-dataset-v1",
            DATASET_DERIVATION_VERSION,
            &model_identity,
            content_identity,
            order,
            quality,
            crate::video::panorama::CONVERSION_VERSION,
            native_runtime,
        ))?;
        let signature = root.join("inputs-dataset.json");
        if let Ok(bytes) = tokio::fs::read(&signature).await {
            if let Ok((previous, dataset)) = serde_json::from_slice::<(String, Dataset)>(&bytes) {
                if (previous == key
                    || previous == old_key
                    || (dataset.derivation_version == 0 && previous == legacy_key))
                    && dataset.path == root
                {
                    if let Ok(model) = read_model(&root.join("sparse/0")) {
                        let ordinary = authority
                            .images
                            .iter()
                            .filter(|image| {
                                authority.cameras.iter().any(|camera| {
                                    camera.id == image.pose.camera_id && camera.model_id != 17
                                })
                            })
                            .collect::<Vec<_>>();
                        let expected_images =
                            ordinary.len() + (authority.images.len() - ordinary.len()) * 6;
                        let complete = model.images.len() == expected_images
                            && dataset.has_erp == has_erp
                            && model
                                .images
                                .iter()
                                .any(|image| image.pose.name == dataset.reference)
                            && ordinary.iter().all(|expected| {
                                model
                                    .images
                                    .iter()
                                    .any(|image| image.pose.name == expected.pose.name)
                            })
                            && model
                                .cameras
                                .iter()
                                .map(|camera| camera.width.max(camera.height))
                                .max()
                                == Some(u64::from(dataset.max_edge));
                        if complete
                            && model.images.iter().all(|i| {
                                model
                                    .cameras
                                    .iter()
                                    .find(|c| c.id == i.pose.camera_id)
                                    .is_some_and(|c| {
                                        image::image_dimensions(
                                            root.join("images").join(&i.pose.name),
                                        )
                                        .is_ok_and(
                                            |(w, h)| {
                                                (u64::from(w), u64::from(h)) == (c.width, c.height)
                                            },
                                        )
                                    })
                            })
                        {
                            verification.finish();
                            self.events.training_substage("trainingValidation");
                            self.events.stage(
                                PipelineStage::TrainingSplats,
                                1.,
                                "已校验并复用训练缓存",
                            );
                            return Ok(dataset);
                        }
                    }
                }
            }
        }
        verification.finish();
        drop(verification);
        self.events.training_substage("trainingPreparation");
        self.events
            .stage(PipelineStage::TrainingSplats, 0., "正在准备训练图片");
        // Invalidate before writing any derived pixel/model file. A cancelled or
        // partially failed rebuild must never regain an old completed signature.
        if signature.exists() {
            tokio::fs::rename(
                &signature,
                root.join(format!(
                    "inputs-dataset.previous-{}.json",
                    uuid::Uuid::new_v4()
                )),
            )
            .await?;
        }
        tokio::fs::create_dir_all(root.join("images")).await?;
        let normal = training_model_subset(&authority, false);
        let mut derived = Model {
            cameras: vec![],
            images: vec![],
            points: authority.points.clone(),
        };
        let mut references = HashMap::new();
        if !normal.images.is_empty() {
            let native = native_runtime
                && normal
                    .cameras
                    .iter()
                    .all(|camera| matches!(camera.model_id, 0 | 1 | 2 | 3 | 4 | 6));
            let mut progress = PreparationProgress::new(
                &self.events,
                normal.images.len(),
                &self.process_manager.child_token(),
            );
            if native {
                for (index, image) in normal.images.iter().enumerate() {
                    if self.process_manager.child_token().is_cancelled() {
                        return Err(SplatError::Cancelled);
                    }
                    let camera = normal
                        .cameras
                        .iter()
                        .find(|camera| camera.id == image.pose.camera_id)
                        .ok_or_else(|| {
                            SplatError::Process("Missing native training camera".into())
                        })?;
                    let source = crate::reconstruction::direct_input::image_path(
                        &paths.frames,
                        &image.pose.name,
                    )?;
                    let (width, height) = image::image_dimensions(&source)
                        .map_err(|error| SplatError::Process(error.to_string()))?;
                    if (u64::from(width), u64::from(height)) != (camera.width, camera.height) {
                        return Err(SplatError::Process(
                            "Native training image/camera dimensions mismatch".into(),
                        ));
                    }
                    reuse_prepared_image(&source, &root.join("images").join(&image.pose.name))
                        .await?;
                    references.insert(image.pose.name.clone(), image.pose.name.clone());
                    progress.update((index + 1) as u64);
                }
                derived = normal;
            } else {
                let input = paths.work.join("ordinary-training-model");
                normal.write(&input)?;
                let output = paths.work.join("undistorted-training");
                colmap::undistort_training_images(
                    &self.engines.colmap,
                    &paths.frames,
                    &input,
                    &output,
                    paths.logs.join("undistort.log"),
                    &self.process_manager,
                    Some(self.process_observer(
                        PipelineStage::TrainingSplats,
                        PipelineEngine::Colmap,
                        None,
                        ObserverMode::TrainingPreparation,
                    )),
                )
                .await?;
                let model = if output.join("sparse/cameras.bin").is_file() {
                    output.join("sparse")
                } else {
                    output.join("sparse/0")
                };
                let converted = read_model(&model)?;
                for (index, image) in converted.images.iter().enumerate() {
                    if self.process_manager.child_token().is_cancelled() {
                        return Err(SplatError::Cancelled);
                    }
                    let source = paths.frames.join(&image.pose.name);
                    let dest = root.join("images").join(&image.pose.name);
                    tokio::fs::create_dir_all(dest.parent().unwrap()).await?;
                    unlink_destination(&dest).await?;
                    tokio::fs::copy(output.join("images").join(&image.pose.name), &dest).await?;
                    let original = authority
                        .cameras
                        .iter()
                        .find(|c| c.id == image.pose.camera_id)
                        .ok_or_else(|| SplatError::Process("Missing source camera".into()))?;
                    let target = converted
                        .cameras
                        .iter()
                        .find(|c| c.id == image.pose.camera_id)
                        .ok_or_else(|| SplatError::Process("Missing converted camera".into()))?;
                    let (source, dest, original, target, cancel) = (
                        source,
                        dest,
                        original.clone(),
                        target.clone(),
                        self.process_manager.child_token(),
                    );
                    crate::presets::spawn_pipeline_blocking(move || {
                        restore_undistorted_alpha(&source, &dest, &original, &target, &cancel)
                    })
                    .await
                    .map_err(|error| SplatError::Process(error.to_string()))??;
                    references.insert(image.pose.name.clone(), image.pose.name.clone());
                    progress.update((index + 1) as u64);
                }
                derived.cameras = converted.cameras;
                derived.images = converted.images;
                let tracks = converted
                    .points
                    .into_iter()
                    .map(|p| (p.id, p.track))
                    .collect::<HashMap<_, _>>();
                for p in &mut derived.points {
                    p.track = tracks.get(&p.id).cloned().unwrap_or_default();
                }
            }
        } else {
            for p in &mut derived.points {
                p.track.clear();
            }
        }
        if has_erp {
            self.events.training_substage("trainingPreparation");
            self.events
                .stage(PipelineStage::TrainingSplats, 0., "正在准备训练图片");
            let mut sphere = training_model_subset(&authority, true);
            let aliases = paths.work.join("sphere-training-images");
            tokio::fs::create_dir_all(&aliases).await?;
            let mut source_names = HashMap::new();
            let aliases_progress = self.application_activity(
                PipelineStage::TrainingSplats,
                "正在准备训练图片",
                Some(sphere.images.len() as u64),
            );
            let aliases_total = sphere.images.len() as u64;
            // Old sphere observations do not refer to the six derived pinhole views.
            for (index, i) in sphere.images.iter_mut().enumerate() {
                i.observations.clear();
                let name = format!("input_{index:010}.png");
                tokio::fs::copy(paths.frames.join(&i.pose.name), aliases.join(&name)).await?;
                source_names.insert(name.clone(), i.pose.name.clone());
                i.pose.name = name;
                aliases_progress.update(index as u64 + 1, aliases_total);
            }
            aliases_progress.finish();
            for p in &mut sphere.points {
                p.track.clear();
            }
            let subset = paths.work.join("sphere-training-model");
            sphere.write(&subset)?;
            let cache = paths.work.join("panorama-training");
            let frames = aliases;
            let model = subset.clone();
            let cache_path = cache.clone();
            let cancel = self.process_manager.child_token();
            let aliases_by_source = source_names
                .iter()
                .map(|(alias, name)| (name, alias))
                .collect::<HashMap<_, _>>();
            let records = records
                .iter()
                .filter_map(|r| {
                    aliases_by_source.get(&r.prepared_name).map(|alias| {
                        let mut r = r.clone();
                        r.prepared_name = (*alias).clone();
                        r
                    })
                })
                .collect::<Vec<_>>();
            self.events.training_substage("panoramaTrainingPreparation");
            self.events
                .stage(PipelineStage::TrainingSplats, 0., "正在准备训练图片");
            let events = self.events.clone();
            let face_activity = self.observed_activity("正在准备训练图片");
            let panorama_images = records
                .iter()
                .all(|record| record.timestamp_seconds.is_none());
            crate::presets::spawn_pipeline_blocking(move || {
                let last_progress = std::cell::Cell::new(None::<Instant>);
                panorama::prepare_reshoot_training_cache_for_policy(
                    &frames,
                    &model,
                    &cache_path,
                    &records,
                    quality,
                    true,
                    &cancel,
                    |update| {
                        face_activity.touch();
                        if let panorama::TrainingCacheUpdate::Faces {
                            completed, total, ..
                        } = update
                        {
                            if completed != total && last_progress.get().is_some_and(|last| last.elapsed().as_secs() < 1) {
                                return;
                            }
                            last_progress.set(Some(Instant::now()));
                            events.send(
                                PipelineStage::TrainingSplats,
                                Some(PipelineEngine::System),
                                EventKind::Progress,
                                EventLevel::Info,
                                Some(completed as f32 / total.max(1) as f32 * 0.95),
                                false,
                                if panorama_images {
                                    format!("正在准备训练图片：全景图片 {}/{}，六面图片 {completed}/{total}", completed / 6, total / 6)
                                } else {
                                    format!("正在准备训练图片：全景帧 {}/{}，六面图片 {completed}/{total}", completed / 6, total / 6)
                                },
                                Some(completed),
                                Some(total),
                                Some(if panorama_images { "panoramaImageFaces" } else { "panoramaFaces" }),
                            );
                        }
                    },
                )
            })
            .await
            .map_err(|e| SplatError::Process(e.to_string()))??;
            let mut faces = read_model(&cache.join("sparse/0"))?;
            let mut next_camera = derived.cameras.iter().map(|c| c.id).max().unwrap_or(0);
            let mut next_image = derived.images.iter().map(|i| i.pose.id).max().unwrap_or(0);
            let mut cameras = HashMap::new();
            for c in &mut faces.cameras {
                next_camera = next_camera
                    .checked_add(1)
                    .ok_or_else(|| SplatError::Process("Camera ID overflow".into()))?;
                cameras.insert(c.id, next_camera);
                c.id = next_camera;
            }
            let manifest: serde_json::Value =
                serde_json::from_slice(&std::fs::read(cache.join("manifest.json"))?)?;
            let fronts = manifest["faces"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|row| row["face"] == "front")
                .filter_map(|row| Some((row["image_name"].as_str()?, row["erp_name"].as_str()?)))
                .collect::<HashMap<_, _>>();
            self.events.training_substage("trainingPreparation");
            let copy_progress = self.application_activity(
                PipelineStage::TrainingSplats,
                "正在准备训练图片",
                Some(faces.images.len() as u64),
            );
            let copy_total = faces.images.len() as u64;
            for (index, i) in faces.images.iter_mut().enumerate() {
                next_image = next_image
                    .checked_add(1)
                    .ok_or_else(|| SplatError::Process("Image ID overflow".into()))?;
                i.pose.id = next_image;
                i.pose.camera_id = cameras[&i.pose.camera_id];
                let name = format!("erp/{}", i.pose.name);
                let dest = root.join("images").join(&name);
                tokio::fs::create_dir_all(dest.parent().unwrap()).await?;
                unlink_destination(&dest).await?;
                tokio::fs::copy(cache.join("images").join(&i.pose.name), dest).await?;
                if let Some(erp) = fronts.get(i.pose.name.as_str()) {
                    references.insert(
                        source_names.get(*erp).cloned().ok_or_else(|| {
                            SplatError::Process("Missing ERP source identity".into())
                        })?,
                        name.clone(),
                    );
                }
                i.pose.name = name;
                copy_progress.update(index as u64 + 1, copy_total);
            }
            copy_progress.finish();
            derived.cameras.extend(faces.cameras);
            derived.images.extend(faces.images);
        }
        self.events.training_substage("trainingValidation");
        self.events.stage(
            PipelineStage::TrainingSplats,
            1.,
            "正在校验训练图片和相机数据",
        );
        derived.validate()?;
        derived.write(&root.join("sparse/0"))?;
        let reference = order
            .iter()
            .find_map(|name| references.get(name))
            .cloned()
            .ok_or_else(|| SplatError::Process("Missing training reference view".into()))?;
        let max_edge = derived
            .cameras
            .iter()
            .map(|c| c.width.max(c.height))
            .max()
            .and_then(|v| u32::try_from(v).ok())
            .ok_or_else(|| SplatError::Process("Training dimensions overflow".into()))?;
        let dataset = Dataset {
            path: root,
            has_erp,
            max_edge,
            reference,
            derivation_version: DATASET_DERIVATION_VERSION,
        };
        crate::project::manager::atomic_write_json(&signature, &(&key, &dataset)).await?;
        Ok(dataset)
    }
}

/// COLMAP writes RGB; restore alpha with the same output-pinhole -> input-camera mapping.
fn restore_undistorted_alpha(
    source: &Path,
    dest: &Path,
    input: &ColmapCamera,
    output: &ColmapCamera,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<()> {
    use image::ImageDecoder;
    let mut reader = image::ImageReader::open(source)?.with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(panorama::MAX_DECODE_BYTES);
    reader.limits(limits);
    let decoder = reader
        .into_decoder()
        .map_err(|e| SplatError::Process(e.to_string()))?;
    if !decoder.color_type().has_alpha() {
        return Ok(());
    }
    let rgba = image::DynamicImage::from_decoder(decoder)
        .map_err(|e| SplatError::Process(e.to_string()))?
        .to_rgba8();
    let mut reader = image::ImageReader::open(dest)?.with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(panorama::MAX_DECODE_BYTES);
    reader.limits(limits);
    let mut result = reader
        .decode()
        .map_err(|e| SplatError::Process(e.to_string()))?
        .to_rgba8();
    if output.model_id != 1
        || (u64::from(result.width()), u64::from(result.height())) != (output.width, output.height)
    {
        return Err(SplatError::Process(
            "Undistorted alpha camera mismatch".into(),
        ));
    }
    for y in 0..result.height() {
        if cancel.is_cancelled() {
            return Err(SplatError::Cancelled);
        }
        for x in 0..result.width() {
            let nx = (x as f64 + 0.5 - output.params[2]) / output.params[0];
            let ny = (y as f64 + 0.5 - output.params[3]) / output.params[1];
            let (px, py) = project(input, nx, ny)?;
            let (px, py) = (px - 0.5, py - 0.5);
            let alpha = if px < 0.
                || py < 0.
                || px > rgba.width() as f64 - 1.
                || py > rgba.height() as f64 - 1.
            {
                0.
            } else {
                let x0 = px.floor() as u32;
                let y0 = py.floor() as u32;
                let dx = px - x0 as f64;
                let dy = py - y0 as f64;
                let a = |x, y| rgba.get_pixel(x, y)[3] as f64;
                (1. - dy) * ((1. - dx) * a(x0, y0) + dx * a((x0 + 1).min(rgba.width() - 1), y0))
                    + dy * ((1. - dx) * a(x0, (y0 + 1).min(rgba.height() - 1))
                        + dx * a(
                            (x0 + 1).min(rgba.width() - 1),
                            (y0 + 1).min(rgba.height() - 1),
                        ))
            };
            result.get_pixel_mut(x, y)[3] = alpha.round().clamp(0., 255.) as u8;
        }
    }
    result
        .save_with_format(dest, image::ImageFormat::Png)
        .map_err(|e| SplatError::Process(e.to_string()))?;
    Ok(())
}
fn project(c: &ColmapCamera, x: f64, y: f64) -> Result<(f64, f64)> {
    let p = &c.params;
    let r = x * x + y * y;
    let (fx, fy, cx, cy, k1, k2, p1, p2, k3, k4, k5, k6) = match c.model_id {
        0 => (p[0], p[0], p[1], p[2], 0., 0., 0., 0., 0., 0., 0., 0.),
        1 => (p[0], p[1], p[2], p[3], 0., 0., 0., 0., 0., 0., 0., 0.),
        2 => (p[0], p[0], p[1], p[2], p[3], 0., 0., 0., 0., 0., 0., 0.),
        3 => (p[0], p[0], p[1], p[2], p[3], p[4], 0., 0., 0., 0., 0., 0.),
        4 => (
            p[0], p[1], p[2], p[3], p[4], p[5], p[6], p[7], 0., 0., 0., 0.,
        ),
        6 => (
            p[0], p[1], p[2], p[3], p[4], p[5], p[6], p[7], p[8], p[9], p[10], p[11],
        ),
        _ => return Err(SplatError::Process("Unsupported distortion model".into())),
    };
    let radial =
        (1. + k1 * r + k2 * r * r + k3 * r * r * r) / (1. + k4 * r + k5 * r * r + k6 * r * r * r);
    let px = fx * (x * radial + 2. * p1 * x * y + p2 * (r + 2. * x * x)) + cx;
    let py = fy * (y * radial + p1 * (r + 2. * y * y) + 2. * p2 * x * y) + cy;
    if !px.is_finite() || !py.is_finite() {
        return Err(SplatError::Process("Invalid distortion mapping".into()));
    }
    Ok((px, py))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_subsets_reuse_authority_without_changing_model_bytes_or_source_identity() {
        for mixed in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let input = crate::reconstruction::direct_input::tests::fixture(root.path(), mixed);
            let source_identity = super::super::gps_runner::model_identity(&input.model).unwrap();
            let authority = read_model(&input.model).unwrap();
            let source_tracks = authority
                .points
                .iter()
                .map(|point| point.track.clone())
                .collect::<Vec<_>>();
            let source_image_count = authority.images.len();
            for erp in [false, true] {
                let mut previous = read_model(&input.model).unwrap();
                prune(&mut previous, |camera| (camera.model_id == 17) == erp);
                let reused = training_model_subset(&authority, erp);
                assert_eq!(reused.images.len(), previous.images.len());
                assert_eq!(reused.cameras.len(), previous.cameras.len());
                assert_eq!(reused.points.len(), previous.points.len());
                if reused.images.is_empty() {
                    continue;
                }
                let expected_root = root.path().join(format!("reparsed-{erp}"));
                let reused_root = root.path().join(format!("reused-{erp}"));
                previous.write(&expected_root).unwrap();
                reused.write(&reused_root).unwrap();
                assert_eq!(
                    super::super::gps_runner::model_identity(&expected_root).unwrap(),
                    super::super::gps_runner::model_identity(&reused_root).unwrap(),
                );
            }
            assert_eq!(authority.images.len(), source_image_count);
            assert_eq!(
                authority
                    .points
                    .iter()
                    .map(|point| point.track.clone())
                    .collect::<Vec<_>>(),
                source_tracks
            );
            assert_eq!(
                super::super::gps_runner::model_identity(&input.model).unwrap(),
                source_identity
            );
        }
    }

    #[test]
    fn rebuilt_identical_pixels_keep_the_recipe_identity_but_changed_pixels_do_not() {
        let root = tempfile::tempdir().unwrap();
        let a = root.path().join("original.png");
        let b = root.path().join("rebuilt.png");
        std::fs::write(&a, b"same saved image bytes").unwrap();
        std::fs::copy(&a, &b).unwrap();
        let token = tokio_util::sync::CancellationToken::new();
        let original = registered_pixel_identity(vec![("frame.png".into(), a)], &token).unwrap();
        assert_eq!(
            original,
            registered_pixel_identity(vec![("frame.png".into(), b.clone())], &token).unwrap()
        );
        std::fs::write(&b, b"changed pixels").unwrap();
        assert_ne!(
            original,
            registered_pixel_identity(vec![("frame.png".into(), b)], &token).unwrap()
        );
        token.cancel();
        assert!(matches!(
            registered_pixel_identity(
                vec![("frame.png".into(), root.path().join("original.png"))],
                &token
            ),
            Err(SplatError::Cancelled)
        ));
    }

    fn fixture_paths(root: &Path) -> (ProjectPaths, crate::pipeline::inputs::ColmapInput) {
        let input = crate::reconstruction::direct_input::tests::fixture(root, false);
        let paths = ProjectPaths::existing(uuid::Uuid::new_v4(), root.join("project"));
        for path in [&paths.frames, &paths.logs, &paths.brush] {
            std::fs::create_dir_all(path).unwrap();
        }
        for name in ["normal.png", "second.png"] {
            std::fs::copy(input.images.join(name), paths.frames.join(name)).unwrap();
        }
        (paths, input)
    }

    #[tokio::test]
    async fn native_colmap_models_reuse_rgba_without_undistortion() {
        for (id, name, params) in [
            (0, "SIMPLE_PINHOLE", vec![50., 32., 24.]),
            (1, "PINHOLE", vec![50., 51., 32., 24.]),
            (2, "SIMPLE_RADIAL", vec![50., 32., 24., 0.1]),
            (3, "RADIAL", vec![50., 32., 24., 0.1, -0.02]),
            (
                4,
                "OPENCV",
                vec![50., 51., 32., 24., 0.1, -0.02, 0.01, -0.01],
            ),
            (
                6,
                "FULL_OPENCV",
                vec![
                    50., 51., 32., 24., 0.1, -0.02, 0.01, -0.01, 0.003, 0.004, 0.005, 0.006,
                ],
            ),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let (paths, input) = fixture_paths(temp.path());
            let mut model = read_model(&input.model).unwrap();
            model.cameras[0].model_id = id;
            model.cameras[0].model = name.into();
            model.cameras[0].params = params.clone();
            model.write(&input.model).unwrap();
            let source = std::fs::read(paths.frames.join("normal.png")).unwrap();
            let events = Arc::new(std::sync::Mutex::new(Vec::new()));
            let captured = events.clone();
            let runner = PipelineRunner::new(
                default_engine_paths(Some(temp.path().join("no-engines"))),
                move |event| {
                    captured.lock().unwrap().push(event);
                },
            );
            let dataset = runner
                .input_dataset_with_native_runtime(
                    &paths,
                    &input.model,
                    Quality::Fast,
                    &[],
                    &["normal.png".into(), "second.png".into()],
                    true,
                )
                .await
                .unwrap();
            assert_eq!(dataset.derivation_version, DATASET_DERIVATION_VERSION);
            assert!(!dataset.has_erp);
            assert!(!paths.logs.join("undistort.log").exists());
            assert_eq!(
                std::fs::read(dataset.path.join("images/normal.png")).unwrap(),
                source
            );
            let result = read_model(&dataset.path.join("sparse/0")).unwrap();
            assert_eq!(result.cameras[0].model_id, id);
            assert_eq!(result.cameras[0].params, params);
            assert_eq!(result.points[0].xyz, model.points[0].xyz);
            assert_eq!(result.points[0].track, model.points[0].track);
            assert_eq!(
                result.images[0].observations[0].x,
                model.images[0].observations[0].x
            );
            assert_eq!(result.images[0].pose.qvec, model.images[0].pose.qvec);
            assert_eq!(result.images[0].pose.tvec, model.images[0].pose.tvec);
            let events = events.lock().unwrap();
            assert!(events.iter().any(|e| e.message == "正在准备训练图片 0/2"));
            assert!(events.iter().any(|e| e.message == "正在准备训练图片 2/2"));
            assert!(events.iter().all(|e| !e.message.contains("全景")));
        }
    }

    #[tokio::test]
    async fn legacy_complete_dataset_is_reused_but_partial_dataset_is_rebuilt() {
        let temp = tempfile::tempdir().unwrap();
        let (paths, input) = fixture_paths(temp.path());
        let runner = PipelineRunner::new(
            default_engine_paths(Some(temp.path().join("no-engines"))),
            |_| {},
        );
        let order = ["normal.png".into(), "second.png".into()];
        let dataset = runner
            .input_dataset_with_native_runtime(
                &paths,
                &input.model,
                Quality::Fast,
                &[],
                &order,
                true,
            )
            .await
            .unwrap();
        let signature = dataset.path.join("inputs-dataset.json");
        let (_, mut saved): (String, Dataset) =
            serde_json::from_slice(&std::fs::read(&signature).unwrap()).unwrap();
        let legacy_key = serde_json::to_string(&(
            super::super::gps_runner::model_identity(&input.model).unwrap(),
            crate::pipeline::inputs::InputSource::Images {
                paths: read_model(&input.model)
                    .unwrap()
                    .images
                    .iter()
                    .map(|image| paths.frames.join(&image.pose.name))
                    .collect(),
            }
            .identity()
            .unwrap(),
            &order,
            Quality::Fast,
            crate::video::panorama::CONVERSION_VERSION,
        ))
        .unwrap();
        saved.derivation_version = 0;
        std::fs::write(
            &signature,
            serde_json::to_vec(&(legacy_key, &saved)).unwrap(),
        )
        .unwrap();
        let reused = runner
            .input_dataset_with_native_runtime(
                &paths,
                &input.model,
                Quality::Fast,
                &[],
                &order,
                false,
            )
            .await
            .unwrap();
        assert_eq!(reused.derivation_version, 0);
        let before = std::fs::read(paths.frames.join("normal.png")).unwrap();
        std::fs::remove_file(dataset.path.join("images/second.png")).unwrap();
        let rebuilt = runner
            .input_dataset_with_native_runtime(
                &paths,
                &input.model,
                Quality::Fast,
                &[],
                &order,
                true,
            )
            .await
            .unwrap();
        assert_eq!(rebuilt.derivation_version, DATASET_DERIVATION_VERSION);
        assert_eq!(
            std::fs::read(paths.frames.join("normal.png")).unwrap(),
            before
        );
        assert!(dataset.path.join("images/second.png").is_file());
    }

    #[tokio::test]
    async fn native_dataset_rejects_mismatched_dimensions_and_cancellation() {
        let temp = tempfile::tempdir().unwrap();
        let (paths, input) = fixture_paths(temp.path());
        let runner = PipelineRunner::new(
            default_engine_paths(Some(temp.path().join("no-engines"))),
            |_| {},
        );
        image::RgbaImage::new(32, 48)
            .save(paths.frames.join("normal.png"))
            .unwrap();
        assert!(runner
            .input_dataset_with_native_runtime(
                &paths,
                &input.model,
                Quality::Fast,
                &[],
                &["normal.png".into()],
                true
            )
            .await
            .is_err());
        assert!(!paths.brush.join("dataset/inputs-dataset.json").exists());
        runner.process_manager.cancel();
        assert!(matches!(
            runner
                .input_dataset_with_native_runtime(
                    &paths,
                    &input.model,
                    Quality::Fast,
                    &[],
                    &["normal.png".into()],
                    true
                )
                .await,
            Err(SplatError::Cancelled)
        ));
    }

    #[tokio::test]
    async fn dataset_cache_rejects_missing_model_image_and_reference() {
        let temp = tempfile::tempdir().unwrap();
        let (paths, input) = fixture_paths(temp.path());
        let runner = PipelineRunner::new(
            default_engine_paths(Some(temp.path().join("no-engines"))),
            |_| {},
        );
        let order = ["normal.png".into(), "second.png".into()];
        let dataset = runner
            .input_dataset_with_native_runtime(
                &paths,
                &input.model,
                Quality::Fast,
                &[],
                &order,
                true,
            )
            .await
            .unwrap();
        let sparse = dataset.path.join("sparse/0");
        let mut damaged = read_model(&sparse).unwrap();
        let removed = damaged.images.pop().unwrap().pose.id;
        for point in &mut damaged.points {
            point.track.retain(|(image, _)| *image != removed);
        }
        damaged.write(&sparse).unwrap();
        runner
            .input_dataset_with_native_runtime(
                &paths,
                &input.model,
                Quality::Fast,
                &[],
                &order,
                true,
            )
            .await
            .unwrap();
        assert_eq!(read_model(&sparse).unwrap().images.len(), 2);
        let signature = dataset.path.join("inputs-dataset.json");
        let (key, mut saved): (String, Dataset) =
            serde_json::from_slice(&std::fs::read(&signature).unwrap()).unwrap();
        saved.reference = "missing.png".into();
        std::fs::write(&signature, serde_json::to_vec(&(key, saved)).unwrap()).unwrap();
        let repaired = runner
            .input_dataset_with_native_runtime(
                &paths,
                &input.model,
                Quality::Fast,
                &[],
                &order,
                true,
            )
            .await
            .unwrap();
        assert_eq!(repaired.reference, "normal.png");
    }

    #[tokio::test]
    #[cfg(target_os = "windows")]
    #[ignore = "Read-only 251-frame toyWithMask preparation benchmark; isolated derived files, no SfM or Brush training"]
    async fn native_toy_mask_training_preparation_benchmark() {
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let source = PathBuf::from(r"E:\GaussianSplatting\test\20261104-230239_toyWithMask");
        let authority = source.join("work/colmap/authoritative");
        let model = read_model(&authority).unwrap();
        assert_eq!(
            model.images.len(),
            251,
            "The benchmark must use the inspected source project"
        );
        let engines = default_engine_paths(Some(repo.join("engines")));
        assert!(brush::supports_native_colmap(&engines.brush, 2));
        let temporary_parent = repo.join(".tmp/training-preparation-validation");
        std::fs::create_dir_all(&temporary_parent).unwrap();
        let temporary = tempfile::Builder::new()
            .prefix("native-toy-")
            .tempdir_in(&temporary_parent)
            .unwrap();
        let paths = ProjectPaths::existing(uuid::Uuid::new_v4(), temporary.path().join("project"));
        for path in [&paths.frames, &paths.logs, &paths.brush] {
            std::fs::create_dir_all(path).unwrap();
        }
        let mut before = vec![];
        let mut order = vec![];
        for image in &model.images {
            let name = &image.pose.name;
            let original = source.join("work/frames").join(name);
            before.push((original.clone(), panorama::fingerprint(&original).unwrap()));
            order.push(name.clone());
            // Source projects outside the workspace are read-only. Copy the
            // fixture snapshot without creating another link to their inode.
            std::fs::copy(original, paths.frames.join(name)).unwrap();
        }
        for path in crate::reconstruction::colmap_model::reusable_model_files(&authority).unwrap() {
            before.push((path.clone(), panorama::fingerprint(&path).unwrap()));
        }
        let database = source.join("work/colmap/database.db");
        before.push((database.clone(), panorama::fingerprint(&database).unwrap()));
        let runner = PipelineRunner::new(engines, |_| {});
        let started = Instant::now();
        let dataset = runner
            .input_dataset(&paths, &authority, Quality::Fast, &[], &order)
            .await
            .unwrap();
        let prepare_ms = started.elapsed().as_millis();
        assert!(!dataset.has_erp);
        assert!(!paths.logs.join("undistort.log").exists());
        for image in &model.images {
            let original = source.join("work/frames").join(&image.pose.name);
            let derived = dataset.path.join("images").join(&image.pose.name);
            assert_eq!(
                panorama::fingerprint(&original).unwrap(),
                panorama::fingerprint(&derived).unwrap()
            );
        }
        let started = Instant::now();
        let cached = runner
            .input_dataset(&paths, &authority, Quality::Fast, &[], &order)
            .await
            .unwrap();
        let cache_ms = started.elapsed().as_millis();
        assert_eq!(cached.reference, dataset.reference);
        for (path, hash) in before {
            assert_eq!(
                panorama::fingerprint(&path).unwrap(),
                hash,
                "Source input changed"
            );
        }
        let proof = serde_json::json!({
            "registeredImages": model.images.len(), "preparationMs": prepare_ms,
            "cachedPreparationMs": cache_ms, "undistorterExecuted": false,
            "alphaReencoded": false, "sourceHashesUnchanged": true,
            "sfmExecuted": false, "brushExecuted": false,
            "historicalSerialAlphaPreparationMs": 189294,
            "historicalTimingIncludesOnlyImageGeneration": true,
        });
        println!("{proof}");
        std::fs::write(
            temporary_parent.join("native-toy-251.json"),
            serde_json::to_vec_pretty(&proof).unwrap(),
        )
        .unwrap();
    }
    #[tokio::test]
    async fn mixed_native_dataset_preserves_world_points_alpha_and_reference() {
        let root = tempfile::tempdir().unwrap();
        let input = crate::reconstruction::direct_input::tests::fixture(root.path(), true);
        let paths = ProjectPaths::existing(uuid::Uuid::new_v4(), root.path().join("project"));
        for p in [&paths.frames, &paths.logs, &paths.brush] {
            std::fs::create_dir_all(p).unwrap();
        }
        let mut hashes = vec![];
        for name in ["normal.png", "second.png"] {
            let source = input.images.join(name);
            hashes.push((
                source.clone(),
                crate::video::panorama::fingerprint(&source).unwrap(),
            ));
            std::fs::copy(source, paths.frames.join(name)).unwrap();
        }
        let events = Arc::new(std::sync::Mutex::new(vec![]));
        let capture = events.clone();
        let runner = PipelineRunner::new(
            default_engine_paths(Some(root.path().join("no-engines"))),
            move |e| capture.lock().unwrap().push(e),
        );
        let records = vec![crate::pipeline::reshoot::PanoramaFrameRecord {
            prepared_name: "second.png".into(),
            batch_id: "erp".into(),
            source_name: "second.png".into(),
            source_sha256: String::new(),
            orientation: Some(1),
            source_index: 0,
            timestamp_seconds: None,
            pts: None,
        }];
        let dataset = runner
            .input_dataset_with_native_runtime(
                &paths,
                &input.model,
                Quality::Fast,
                &records,
                &["second.png".into(), "normal.png".into()],
                true,
            )
            .await
            .unwrap();
        assert!(dataset.has_erp);
        assert!(dataset.reference.ends_with("front.png"));
        let model = read_model(&dataset.path.join("sparse/0")).unwrap();
        assert_eq!(model.images.len(), 7);
        assert_eq!(model.points[0].xyz, [0., 0., 5.]);
        assert_eq!(model.points[0].track.len(), 1);
        let result = image::open(dataset.path.join("images/normal.png"))
            .unwrap()
            .to_rgba8();
        assert_eq!(result.get_pixel(0, 24)[3], 0);
        assert_eq!(result.get_pixel(32, 24)[3], 255);
        assert!(events
            .lock()
            .unwrap()
            .iter()
            .all(|e| e.stage == PipelineStage::TrainingSplats));
        assert!(!paths.logs.join("colmap.log").exists());
        assert!(!paths.logs.join("undistort.log").exists());
        let signature = dataset.path.join("inputs-dataset.json");
        let before = std::fs::metadata(&signature).unwrap().modified().unwrap();
        let cached = runner
            .input_dataset_with_native_runtime(
                &paths,
                &input.model,
                Quality::Fast,
                &records,
                &["second.png".into(), "normal.png".into()],
                true,
            )
            .await
            .unwrap();
        assert_eq!(cached.reference, dataset.reference);
        assert_eq!(
            std::fs::metadata(&signature).unwrap().modified().unwrap(),
            before
        );
        for (path, hash) in hashes {
            assert_eq!(crate::video::panorama::fingerprint(&path).unwrap(), hash);
        }
    }

    #[tokio::test]
    #[ignore = "Bundled COLMAP compatibility conversion of two synthetic images; no features, matching, Mapper or Brush training"]
    async fn native_compatibility_undistortion_preserves_alpha_and_source_files() {
        let temporary = tempfile::tempdir().unwrap();
        let (paths, input) = fixture_paths(temporary.path());
        let mut authority = read_model(&input.model).unwrap();
        authority.cameras[0].model_id = 2;
        authority.cameras[0].model = "SIMPLE_RADIAL".into();
        authority.cameras[0].params = vec![50., 32., 24., 0.08];
        authority.write(&input.model).unwrap();
        let mut hashes = vec![];
        for name in ["normal.png", "second.png"] {
            for root in [&paths.frames, &input.images] {
                let path = root.join(name);
                hashes.push((path.clone(), panorama::fingerprint(&path).unwrap()));
            }
        }
        for path in crate::reconstruction::colmap_model::reusable_model_files(&input.model).unwrap()
        {
            hashes.push((path.clone(), panorama::fingerprint(&path).unwrap()));
        }
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let events = Arc::new(std::sync::Mutex::new(vec![]));
        let capture = events.clone();
        let runner = PipelineRunner::new(
            default_engine_paths(Some(repo.join("engines"))),
            move |event| {
                capture.lock().unwrap().push(event);
            },
        );
        let dataset = runner
            .input_dataset_with_native_runtime(
                &paths,
                &input.model,
                Quality::Fast,
                &[],
                &["normal.png".into(), "second.png".into()],
                false,
            )
            .await
            .unwrap();
        assert!(!dataset.has_erp);
        assert_eq!(dataset.reference, "normal.png");
        assert!(paths.logs.join("undistort.log").is_file());
        assert!(!paths.logs.join("colmap.log").exists());
        assert!(!paths.logs.join("brush.log").exists());
        let derived = read_model(&dataset.path.join("sparse/0")).unwrap();
        assert_eq!(derived.images.len(), 2);
        assert_eq!(derived.points[0].xyz, authority.points[0].xyz);
        assert!(derived.cameras.iter().all(|camera| camera.model_id == 1));
        for image in &derived.images {
            let camera = derived
                .cameras
                .iter()
                .find(|camera| camera.id == image.pose.camera_id)
                .unwrap();
            let pixels = image::open(dataset.path.join("images").join(&image.pose.name)).unwrap();
            assert_eq!(
                (u64::from(pixels.width()), u64::from(pixels.height())),
                (camera.width, camera.height)
            );
            if image.pose.name == "normal.png" {
                assert!(pixels.color().has_alpha());
                let rgba = pixels.to_rgba8();
                assert!(rgba.pixels().any(|pixel| pixel[3] == 0));
                assert!(rgba.pixels().any(|pixel| pixel[3] == 255));
                assert_eq!(rgba.get_pixel(rgba.width() / 2, rgba.height() / 2)[3], 255);
            }
            let original = authority
                .images
                .iter()
                .find(|original| original.pose.id == image.pose.id)
                .unwrap();
            assert_eq!(image.pose.qvec, original.pose.qvec);
            assert_eq!(image.pose.tvec, original.pose.tvec);
        }
        assert!(events
            .lock()
            .unwrap()
            .iter()
            .all(|event| event.stage == PipelineStage::TrainingSplats));
        for (path, hash) in hashes {
            assert_eq!(
                panorama::fingerprint(&path).unwrap(),
                hash,
                "Compatibility conversion modified source input"
            );
        }
        let signature = dataset.path.join("inputs-dataset.json");
        let before = std::fs::metadata(&signature).unwrap().modified().unwrap();
        let cached = runner
            .input_dataset_with_native_runtime(
                &paths,
                &input.model,
                Quality::Fast,
                &[],
                &["normal.png".into(), "second.png".into()],
                false,
            )
            .await
            .unwrap();
        assert_eq!(cached.reference, dataset.reference);
        assert_eq!(
            std::fs::metadata(&signature).unwrap().modified().unwrap(),
            before
        );
    }

    #[test]
    fn distortion_models_use_their_actual_parameters() {
        let c = ColmapCamera {
            id: 1,
            model_id: 2,
            model: "SIMPLE_RADIAL".into(),
            width: 100,
            height: 80,
            params: vec![50., 50., 40., 0.1],
        };
        let (x, y) = project(&c, 0.2, 0.3).unwrap();
        assert!((x - (50. * 0.2 * (1. + 0.1 * 0.13) + 50.)).abs() < 1e-10);
        assert!((y - (50. * 0.3 * (1. + 0.1 * 0.13) + 40.)).abs() < 1e-10);
    }

    #[tokio::test]
    #[ignore = "Read-only public-photo import + bundled undistorter/Brush, 8 smoke iterations in an isolated directory; not quality acceptance"]
    async fn native_imported_public_photos_train_without_sfm() {
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let source = repo.join(".cache/gps-v1/public-projects/20261026-203054_images");
        let input_model = source.join("work/colmap/sparse/0");
        let mut model = read_model(&input_model).unwrap();
        model.images.truncate(2);
        let ids = model
            .images
            .iter()
            .map(|i| i.pose.id)
            .collect::<HashSet<_>>();
        for p in &mut model.points {
            p.track.retain(|(id, _)| ids.contains(id));
        }
        model
            .cameras
            .retain(|c| model.images.iter().any(|i| i.pose.camera_id == c.id));
        let temp = tempfile::tempdir().unwrap();
        let owned_model = temp.path().join("selected-model");
        model.write(&owned_model).unwrap();
        let mut hashes = vec![];
        for i in &model.images {
            let source = source.join("work/frames").join(&i.pose.name);
            hashes.push((source.clone(), panorama::fingerprint(&source).unwrap()));
        }
        let manager = ProjectManager::for_diagnostics(temp.path().join("derived"));
        let (paths, mut metadata) = manager
            .create_collection(
                Quality::Fast,
                crate::pipeline::inputs::GenerationInputs::Colmap {
                    input: crate::pipeline::inputs::ColmapInput {
                        model: owned_model,
                        images: source.join("work/frames"),
                        database: None,
                        identity: None,
                    },
                },
            )
            .await
            .unwrap();
        metadata.started_at = Some(Utc::now());
        let mut state = manager.read_state(&paths.state).await.unwrap();
        state.generation_policy = Some(crate::pipeline::generation_policy::GenerationPolicy {
            auto_retry_enabled: false,
            ..Default::default()
        });
        let mut resolved = crate::presets::resolve_brush_training_preset(
            Quality::Fast,
            true,
            None,
            1024,
            model.points.len() as u64,
        );
        resolved.preset.total_steps = 8;
        state.brush_training.resolved = Some(resolved);
        state.brush_training.initial_resolved = Some(resolved);
        let events = Arc::new(std::sync::Mutex::new(vec![]));
        let captured = events.clone();
        let runner =
            PipelineRunner::new(default_engine_paths(Some(repo.join("engines"))), move |e| {
                captured.lock().unwrap().push(e)
            });
        let acceleration = runner.verify_pipeline_engines().await.unwrap();
        let started = Instant::now();
        let result = runner
            .run_inputs(&manager, &paths, &mut metadata, state, &acceleration)
            .await
            .unwrap();
        let info = inspect_gaussian_ply(&result.final_ply).unwrap();
        let saved = manager.read_state(&paths.state).await.unwrap();
        assert!(saved.reconstruction_complete && saved.brush_complete);
        assert!(!saved.features_complete && !saved.matching_complete);
        assert!(events
            .lock()
            .unwrap()
            .iter()
            .any(|e| e.message == format!("已发布 {} 个 Splat", info.splat_count)));
        assert!(info.splat_count > 0);
        assert!(!paths.logs.join("colmap.log").exists());
        assert!(events.lock().unwrap().iter().all(|e| !matches!(
            e.stage,
            PipelineStage::ExtractingFeatures
                | PipelineStage::Matching
                | PipelineStage::Reconstructing
        )));
        for (path, hash) in hashes {
            assert_eq!(panorama::fingerprint(&path).unwrap(), hash);
        }
        let proof = serde_json::json!({"registeredImages":model.images.len(),"sparsePoints":model.points.len(),"smokeIterations":8,"splatCount":info.splat_count,"elapsedMs":started.elapsed().as_millis(),"sfmExecuted":false,"sourceHashesUnchanged":true,"qualityAcceptance":false});
        println!("{proof}");
        std::fs::create_dir_all(repo.join(".tmp/multi-input-validation")).unwrap();
        std::fs::write(
            repo.join(".tmp/multi-input-validation/native-public-import.json"),
            serde_json::to_vec_pretty(&proof).unwrap(),
        )
        .unwrap();
    }
}
