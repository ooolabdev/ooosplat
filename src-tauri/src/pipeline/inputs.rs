//! Ordered, transport-independent input admission. Metadata inspection never stages pixels.
use super::generation_policy::GenerationPolicy;
use crate::{
    engines::EnginePaths,
    error::{Result, SplatError},
    presets::Quality,
    project::ImageMatchingStrategy,
    video::{
        panorama::{ProjectionDetection, VideoProjection},
        FramePlan, ImageSequenceInfo, VideoInfo,
    },
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::OnceLock,
};

pub const DUPLICATE: &str = "素材存在重复输入，请移除重复项。";
pub const INVALID_COLMAP: &str = "COLMAP 数据不完整或与图片不匹配，无法直接训练。";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InputCheckpoint {
    pub version: u32,
    pub request: GenerationInputs,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_plan: Option<super::execution_plan::ExecutionPlan>,
    #[serde(default)]
    pub sources: Vec<SourceCheckpoint>,
    /// Frozen execution plans retained after prepared artifacts fail
    /// validation. These are recovery instructions, never trusted assets.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recovery_sources: Vec<SourceCheckpoint>,
    #[serde(default)]
    pub prepared: bool,
    #[serde(default)]
    pub bridge_used: bool,
    #[serde(default)]
    pub expanded_matching: bool,
    #[serde(default)]
    pub imported_reconstruction: bool,
    #[serde(default)]
    pub reshoot_eligible: bool,
    #[serde(default)]
    pub unused: Vec<String>,
    /// Durable per-source reconstruction outcome. Unlike the legacy `unused`
    /// list this records the real prepared/registered counts and survives a
    /// successful partial reshoot across restarts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_adoption: Vec<SourceAdoption>,
    /// Local summary only: no coordinates or geographic origin leave the backend.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gps_assistance: Option<GpsAssistance>,
    #[serde(default)]
    pub reference_image: Option<String>,
    #[serde(default)]
    pub init_sh_degree: Option<u32>,
    /// Temporary per-camera-group PNG/XMP initialization policy. Missing on
    /// old checkpoints; normal projects remain unchanged when no group needs
    /// the narrowly-scoped workaround.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) temporary_png_focal_records: Option<Vec<super::focal_preflight::FocalRecord>>,
    /// Durable intent: archive and base-model restoration may be interrupted
    /// before the replacement initialization policy can be committed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) temporary_png_focal_pending_records:
        Option<Vec<super::focal_preflight::FocalRecord>>,
    /// Only this preflight's verified applicability may affect a new camera.
    /// Saved audit records alone must not override an inapplicable runtime.
    #[serde(skip)]
    pub(crate) temporary_png_focal_active_records: Option<Vec<super::focal_preflight::FocalRecord>>,
}
impl InputCheckpoint {
    pub fn new(request: GenerationInputs) -> Self {
        Self {
            version: 1,
            request,
            execution_plan: None,
            sources: vec![],
            recovery_sources: vec![],
            prepared: false,
            bridge_used: false,
            expanded_matching: false,
            imported_reconstruction: false,
            reshoot_eligible: false,
            unused: vec![],
            source_adoption: vec![],
            gps_assistance: None,
            reference_image: None,
            init_sh_degree: None,
            temporary_png_focal_records: None,
            temporary_png_focal_pending_records: None,
            temporary_png_focal_active_records: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceAdoption {
    pub source_id: String,
    pub planned_frames: u64,
    pub prepared_frames: u64,
    pub registered_frames: u64,
    pub adopted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub omitted_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GpsAssistance {
    pub detected_source_ids: Vec<String>,
    pub status: String,
}

impl GpsAssistance {
    pub(super) fn participating(
        checkpoint: &InputCheckpoint,
        records: &HashSet<String>,
        registered: &HashSet<String>,
        status: Option<&str>,
    ) -> Option<Self> {
        let detected_source_ids = checkpoint
            .sources
            .iter()
            .filter(|s| {
                s.id != "__source"
                    && s.names
                        .iter()
                        .any(|n| registered.contains(n) && records.contains(n))
            })
            .map(|s| s.id.clone())
            .collect::<Vec<_>>();
        (!detected_source_ids.is_empty()).then(|| Self {
            detected_source_ids,
            status: match status {
                Some("complete") => "enabled",
                Some("fallback") => "fallback",
                _ => "unavailable",
            }
            .into(),
        })
    }
}

fn source_label(checkpoint: &InputCheckpoint, id: &str) -> String {
    match &checkpoint.request {
        GenerationInputs::Materials { items } => items
            .iter()
            .find(|i| i.id == id)
            .map(|i| {
                i.source
                    .path()
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            })
            .unwrap_or_else(|| id.into()),
        GenerationInputs::Colmap { .. } => id.into(),
    }
}

fn new_registration_warnings(checkpoint: &InputCheckpoint) -> Vec<String> {
    let reshoot = checkpoint.sources.iter().any(|s| s.id == "__source")
        || checkpoint
            .execution_plan
            .as_ref()
            .is_some_and(|p| p.success.require_any_new_source);
    if !reshoot {
        return vec![];
    }
    let sources = checkpoint
        .source_adoption
        .iter()
        .filter(|s| s.source_id != "__source" && s.prepared_frames > 0)
        .collect::<Vec<_>>();
    let prepared = sources
        .iter()
        .map(|s| u128::from(s.prepared_frames))
        .sum::<u128>();
    let registered = sources
        .iter()
        .map(|s| u128::from(s.registered_frames))
        .sum::<u128>();
    if prepared == 0 || registered == 0 {
        return vec![];
    }
    let message = |registered, prepared| {
        format!("补拍画面已加入 {registered}/{prepared} 张，部分新增画面未能找到与原场景的联系，效果可能受影响。")
    };
    if registered as f64 / (prepared as f64) < crate::reconstruction::good_registered_ratio() {
        return vec![message(registered, prepared)];
    }
    sources
        .into_iter()
        .filter(|s| {
            s.registered_frames > 0
                && (s.registered_frames as f64 / s.prepared_frames as f64)
                    < crate::reconstruction::good_registered_ratio()
        })
        .map(|s| {
            format!(
                "{}：{}",
                source_label(checkpoint, &s.source_id),
                message(
                    u128::from(s.registered_frames),
                    u128::from(s.prepared_frames)
                )
            )
        })
        .collect()
}

/// Rebuild the existing partial-adoption warning from durable per-source
/// results. This is shared by live completion and crash recovery so the task
/// detail does not lose the warning after restart.
pub fn source_adoption_warning(checkpoint: &InputCheckpoint) -> Option<String> {
    let omitted = checkpoint
        .source_adoption
        .iter()
        .filter(|source| !source.adopted)
        .collect::<Vec<_>>();
    let mut warnings = new_registration_warnings(checkpoint);
    let labels = omitted
        .iter()
        .map(|source| match &checkpoint.request {
            GenerationInputs::Materials { items } => items
                .iter()
                .find(|item| item.id == source.source_id)
                .map(|item| {
                    item.source
                        .path()
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                })
                .unwrap_or_else(|| source.source_id.clone()),
            GenerationInputs::Colmap { .. } => source.source_id.clone(),
        })
        .collect::<Vec<_>>();
    if !omitted.is_empty() {
        warnings.insert(
            0,
            format!(
                "部分素材未能参与生成：{}。本次仅使用已连接到同一场景的素材。",
                labels.join("、")
            ),
        );
    }
    (!warnings.is_empty()).then(|| warnings.join("\n"))
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceCheckpoint {
    pub id: String,
    pub identity: String,
    #[serde(default)]
    pub camera_ids: Vec<u32>,
    #[serde(default)]
    pub camera_assignment: Option<super::reshoot::CameraAssignment>,
    pub projection: VideoProjection,
    pub names: Vec<String>,
    pub originals: Vec<PathBuf>,
    pub video: Option<VideoInfo>,
    #[serde(default)]
    pub image_sequence: Option<ImageSequenceInfo>,
    #[serde(default)]
    pub dimensions: Vec<[u32; 2]>,
    /// Actual prepared COLMAP pixel dimensions, distinct from source analysis.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_dimensions: Option<[u32; 2]>,
    pub frames: Option<crate::project::FrameState>,
    #[serde(default)]
    pub panorama_records: Vec<super::reshoot::PanoramaFrameRecord>,
    #[serde(default)]
    pub adopted: u64,
    #[serde(default)]
    pub bridge_frames: Vec<crate::video::PlannedFrame>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum InputSource {
    Video { path: PathBuf },
    ImageFolder { path: PathBuf },
    Images { paths: Vec<PathBuf> },
}
impl InputSource {
    pub fn path(&self) -> &Path {
        match self {
            Self::Video { path } | Self::ImageFolder { path } => path,
            Self::Images { paths } => paths.first().map(PathBuf::as_path).unwrap_or(Path::new("")),
        }
    }
    pub fn members(&self) -> Result<Vec<PathBuf>> {
        match self {
            Self::Video { path } => Ok(vec![path.clone()]),
            Self::ImageFolder { path } => {
                let root = std::fs::canonicalize(path)?;
                let files = crate::video::list_images(path)?;
                for file in &files {
                    if !std::fs::canonicalize(file)?.starts_with(&root) {
                        return Err(SplatError::InvalidPath(file.clone()));
                    }
                }
                Ok(files)
            }
            Self::Images { paths } => {
                let mut paths = paths.clone();
                paths.sort_by(|a, b| {
                    crate::video::image_sequence::natural_cmp(
                        &a.file_name().unwrap_or_default().to_string_lossy(),
                        &b.file_name().unwrap_or_default().to_string_lossy(),
                    )
                    .then_with(|| a.cmp(b))
                });
                Ok(paths)
            }
        }
    }
    pub fn identity(&self) -> Result<String> {
        let mut hash = Sha256::new();
        hash.update(match self {
            Self::Video { .. } => "video",
            Self::ImageFolder { .. } => "folder",
            Self::Images { .. } => "images",
        });
        for path in self.members()? {
            let canonical = std::fs::canonicalize(&path)?;
            let metadata = std::fs::metadata(&canonical)?;
            if !metadata.is_file() {
                return Err(SplatError::InvalidPath(path));
            }
            let bytes = canonical.as_os_str().as_encoded_bytes();
            hash.update((bytes.len() as u64).to_le_bytes());
            hash.update(bytes);
            hash.update(metadata.len().to_le_bytes());
            hash.update(
                metadata
                    .modified()?
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(|e| SplatError::Process(e.to_string()))?
                    .as_nanos()
                    .to_le_bytes(),
            );
        }
        Ok(format!("{:x}", hash.finalize()))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MaterialInput {
    pub id: String,
    pub source: InputSource,
    #[serde(default)]
    pub identity: Option<String>,
    #[serde(default)]
    pub projection: VideoProjection,
    #[serde(default)]
    pub image_matching_strategy: ImageMatchingStrategy,
    #[serde(default)]
    pub camera_relation: Option<super::reshoot::CameraRelation>,
    #[serde(default)]
    pub confirmed_small_images: bool,
    #[serde(default)]
    pub confirmed_image_order: bool,
    /// Version 5+ video sampling: frames removed per second. Version 6 permits
    /// zero to retain every frame; older policies keep their saved validation.
    /// None uses the tier default.
    #[serde(default)]
    pub frame_removal_fps: Option<f64>,
}

/// Admission is shared by GUI, MCP and CLI; transports cannot silently choose missing answers.
pub async fn validate(
    engines: &EnginePaths,
    inputs: &GenerationInputs,
    quality: Quality,
    policy: GenerationPolicy,
) -> Result<()> {
    if policy.frame_sampling_ratio.is_some()
        && !matches!(inputs,GenerationInputs::Materials{items} if items.len()==1&&matches!(items[0].source,InputSource::Video{..}))
    {
        return Err(SplatError::Process(
            "Legacy frame ratio is supported only for a single video".into(),
        ));
    }
    inputs.check_identities()?;
    if let GenerationInputs::Materials { items } = inputs {
        for item in items {
            if item.frame_removal_fps.is_some()
                && !matches!(&item.source, InputSource::Video { .. })
            {
                return Err(SplatError::Process(
                    "Frame removal FPS is supported only for video inputs".into(),
                ));
            }
            let a = inspect_material(engines, item, quality, policy).await?;
            projection(item, &a)?;
            if let Some(images) = a.image_sequence {
                if (images.image_count < 30 && !item.confirmed_small_images)
                    || (images.image_count > 100 && !item.confirmed_image_order)
                {
                    return Err(SplatError::Process("CONFIRMATION_REQUIRED".into()));
                }
            }
        }
    }
    Ok(())
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ColmapInput {
    pub model: PathBuf,
    pub images: PathBuf,
    #[serde(default)]
    pub database: Option<PathBuf>,
    #[serde(default)]
    pub identity: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "camelCase", deny_unknown_fields)]
pub enum GenerationInputs {
    Materials { items: Vec<MaterialInput> },
    Colmap { input: ColmapInput },
}
impl GenerationInputs {
    pub fn canonicalized(&self) -> Result<Self> {
        let mut value = self.clone();
        match &mut value {
            Self::Materials { items } => {
                for item in items {
                    match &mut item.source {
                        InputSource::Video { path } | InputSource::ImageFolder { path } => {
                            *path = std::fs::canonicalize(&*path)?
                        }
                        InputSource::Images { paths } => {
                            for path in paths {
                                *path = std::fs::canonicalize(&*path)?;
                            }
                        }
                    }
                }
            }
            Self::Colmap { input } => {
                input.model = std::fs::canonicalize(&input.model)?;
                input.images = std::fs::canonicalize(&input.images)?;
                if let Some(db) = &mut input.database {
                    *db = if db.exists() {
                        std::fs::canonicalize(&*db)?
                    } else {
                        std::path::absolute(&*db)?
                    };
                }
            }
        }
        Ok(value)
    }
    pub fn primary_path(&self) -> &Path {
        match self {
            Self::Materials { items } => items
                .first()
                .map(|i| i.source.path())
                .unwrap_or(Path::new("")),
            Self::Colmap { input } => &input.model,
        }
    }
    pub fn authorized_paths(&self) -> Result<Vec<PathBuf>> {
        let mut paths = vec![];
        match self {
            Self::Materials { items } => {
                if items.is_empty() {
                    return Err(SplatError::Process("请输入视频文件或图片文件夹".into()));
                }
                let mut ids = HashSet::new();
                let mut members = HashSet::new();
                for item in items {
                    if item.id.is_empty()
                        || item.id.len() > 128
                        || item.id == "__source"
                        || !ids.insert(&item.id)
                    {
                        return Err(SplatError::Process(DUPLICATE.into()));
                    }
                    if let InputSource::Video { path } = &item.source {
                        if !path.extension().is_some_and(|e| {
                            e.eq_ignore_ascii_case("mp4") || e.eq_ignore_ascii_case("mov")
                        }) {
                            return Err(SplatError::InvalidPath(path.clone()));
                        }
                    }
                    paths.push(item.source.path().to_path_buf());
                    let files = item.source.members()?;
                    for file in files {
                        let canonical = std::fs::canonicalize(&file)?;
                        if !members.insert(canonical) {
                            return Err(SplatError::Process(DUPLICATE.into()));
                        }
                        paths.push(file);
                    }
                }
            }
            Self::Colmap { input } => {
                paths.extend([input.model.clone(), input.images.clone()]);
                if let Some(db) = &input.database {
                    paths.push(if db.exists() {
                        db.clone()
                    } else {
                        db.parent().unwrap_or(db).to_path_buf()
                    });
                }
                // Referenced images (including symlinks) require independent authorization.
                for image in
                    crate::reconstruction::colmap_model::read_registered_images(&input.model)?
                {
                    paths.push(crate::reconstruction::direct_input::image_path(
                        &input.images,
                        &image.name,
                    )?);
                }
            }
        }
        Ok(paths)
    }
    pub fn check_identities(&self) -> Result<()> {
        self.authorized_paths()?;
        match self {
            Self::Materials { items } => {
                for item in items {
                    if item
                        .identity
                        .as_ref()
                        .is_some_and(|i| item.source.identity().ok().as_ref() != Some(i))
                    {
                        return Err(SplatError::Process(
                            "INPUT_CHANGED: 素材文件已发生变化，请重新分析后再生成".into(),
                        ));
                    }
                }
            }
            Self::Colmap { input } => {
                crate::reconstruction::direct_input::inspect(input)?;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameSamplingRange {
    pub minimum_target_fps: f64,
    pub maximum_target_fps: f64,
    pub minimum: FramePlan,
    pub maximum: FramePlan,
}

fn frame_sampling_range(
    policy: GenerationPolicy,
    video: &VideoInfo,
    quality: Quality,
) -> Result<Option<FrameSamplingRange>> {
    if policy.frame_sampling_ratio.is_some() || policy.frame_sampling_fps.is_some() {
        return Ok(None);
    }
    let Some((minimum_target_fps, maximum_target_fps, minimum_removal, maximum_removal)) =
        (if let Some((minimum, maximum)) = policy.frame_sampling_bounds(video) {
            Some((minimum, maximum, (video.fps - minimum).max(0.0), 0.0))
        } else if policy.version == 5
            && video.fps.is_finite()
            && video.fps > 0.0
            && video.duration.is_finite()
            && video.duration > 0.0
        {
            let effective_maximum_removal = (video.fps - 0.1).max(0.0);
            let minimum_removal = (video.fps - 0.1).max(0.1);
            Some((
                (video.fps - minimum_removal.min(effective_maximum_removal))
                    .max(0.1)
                    .min(video.fps),
                (video.fps - 0.1_f64.min(effective_maximum_removal))
                    .max(0.1)
                    .min(video.fps),
                minimum_removal,
                0.1,
            ))
        } else {
            None
        })
    else {
        return Ok(None);
    };
    Ok(Some(FrameSamplingRange {
        minimum_target_fps,
        maximum_target_fps,
        minimum: policy.frame_plan_for_material(video, quality, Some(minimum_removal))?,
        maximum: policy.frame_plan_for_material(video, quality, Some(maximum_removal))?,
    }))
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MaterialAnalysis {
    pub id: String,
    pub identity: String,
    #[serde(default)]
    pub legacy_identity: Option<String>,
    pub video: Option<VideoInfo>,
    pub image_sequence: Option<ImageSequenceInfo>,
    #[serde(default)]
    pub dimensions: Vec<[u32; 2]>,
    pub projection_detection: ProjectionDetection,
    pub plan: Option<FramePlan>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_sampling_range: Option<FrameSamplingRange>,
}

pub async fn inspect_material(
    engines: &EnginePaths,
    item: &MaterialInput,
    quality: Quality,
    policy: GenerationPolicy,
) -> Result<MaterialAnalysis> {
    static CACHE: OnceLock<
        tokio::sync::Mutex<std::collections::VecDeque<(String, MaterialAnalysis)>>,
    > = OnceLock::new();
    let identity = item.source.identity()?;
    let mut cache = CACHE.get_or_init(Default::default).lock().await;
    let mut value = if let Some((_, v)) = cache.iter().find(|(key, _)| key == &identity) {
        v.clone()
    } else {
        let value = match &item.source {
            InputSource::Video { path } => {
                let analysis =
                    crate::commands::analyze_media(engines, &std::fs::canonicalize(path)?).await?;
                MaterialAnalysis {
                    id: item.id.clone(),
                    identity: identity.clone(),
                    legacy_identity: Some(analysis.projection_detection.source_identity.clone()),
                    video: analysis.video,
                    image_sequence: None,
                    dimensions: vec![],
                    projection_detection: analysis.projection_detection,
                    plan: None,
                    frame_sampling_range: None,
                }
            }
            _ => {
                let files = item.source.members()?;
                let key = identity.clone();
                if files
                    .iter()
                    .any(|p| !crate::video::image_sequence::is_image_file(p))
                {
                    return Err(SplatError::Process("每个图片集至少需要2张图片。".into()));
                }
                let a = crate::presets::spawn_pipeline_blocking(move || {
                    if files.len() < 2 {
                        return Err(SplatError::Process("每个图片集至少需要2张图片。".into()));
                    }
                    crate::video::image_projection::analyze_files(&files, key, true)
                })
                .await
                .map_err(|e| SplatError::Process(e.to_string()))??;
                if a.images.iter().any(|i| i.complete_erp)
                    && !a.images.iter().all(|i| i.complete_erp)
                {
                    return Err(SplatError::Process(
                        "同一图片集请使用相同的图片类型，普通图片与全景图片请分开添加。".into(),
                    ));
                }
                if a.images.iter().all(|i| i.complete_erp) && !a.detection.supported_erp {
                    return Err(SplatError::Process("不支持此图片集的全景格式。请使用同尺寸、已拼接、单目、完整 360°×180° 的 2:1 ERP 图片。".into()));
                }
                let mut dimensions = a
                    .images
                    .iter()
                    .map(|i| [i.width, i.height])
                    .collect::<Vec<_>>();
                dimensions.sort();
                dimensions.dedup();
                MaterialAnalysis {
                    id: item.id.clone(),
                    identity: identity.clone(),
                    legacy_identity: if let InputSource::ImageFolder { path } = &item.source {
                        Some(crate::video::image_projection::source_identity(path)?)
                    } else {
                        None
                    },
                    video: None,
                    image_sequence: Some(a.info),
                    dimensions,
                    projection_detection: a.detection,
                    plan: None,
                    frame_sampling_range: None,
                }
            }
        };
        if item.source.identity()? != identity {
            return Err(SplatError::Process("INPUT_CHANGED".into()));
        }
        if cache.len() >= 64 {
            cache.pop_front();
        }
        cache.push_back((identity.clone(), value.clone()));
        value
    };
    value.id = item.id.clone();
    value.projection_detection.source_identity = identity;
    if let Some(video) = value.video.as_ref() {
        value.plan =
            Some(policy.frame_plan_for_material(video, quality, item.frame_removal_fps)?);
        value.frame_sampling_range = frame_sampling_range(policy, video, quality)?;
    } else {
        value.plan = None;
        value.frame_sampling_range = None;
    }
    Ok(value)
}

pub fn projection(item: &MaterialInput, analysis: &MaterialAnalysis) -> Result<VideoProjection> {
    use VideoProjection::*;
    let d = &analysis.projection_detection;
    if d.confirmed_erp && item.projection == Perspective {
        return Err(SplatError::Process("INPUT_CHANGED".into()));
    }
    if item.projection == Perspective {
        return Ok(Perspective);
    }
    if item.projection == Equirectangular && !d.supported_erp {
        return Err(SplatError::Process("不支持此图片集的全景格式。请使用同尺寸、已拼接、单目、完整 360°×180° 的 2:1 ERP 图片。".into()));
    }
    if d.requires_choice && item.projection == Auto {
        return Err(SplatError::Process("CONFIRMATION_REQUIRED".into()));
    }
    Ok(if d.confirmed_erp || item.projection == Equirectangular {
        Equirectangular
    } else {
        Perspective
    })
}

/// Coarse aggregate estimate, never borrowing the first source's dimensions or
/// claiming a reconstruction-dependent Splat budget. No hardware probe occurs.
pub fn estimate(
    analyses: &[MaterialAnalysis],
    quality: Quality,
    samples: &[crate::pipeline::estimate::RuntimeSample],
) -> Option<crate::pipeline::estimate::RuntimeEstimate> {
    let count = analyses
        .iter()
        .map(|a| {
            a.plan.as_ref().map_or_else(
                || a.image_sequence.as_ref().map_or(0, |i| i.image_count),
                |p| p.estimated_frames,
            )
        })
        .sum::<u64>();
    if count == 0 {
        return None;
    }
    let edge = analyses
        .iter()
        .flat_map(|a| {
            a.dimensions
                .iter()
                .map(|d| d[0].max(d[1]))
                .chain(a.video.iter().map(|v| v.width.max(v.height)))
        })
        .max()
        .unwrap_or(1);
    let plan = FramePlan {
        estimated_frames: count,
        ..Default::default()
    };
    Some(
        super::estimate::estimate_runtime_for_images_with_brush_and_resolution(
            count, edge, &plan, quality, samples, None, None, edge,
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gps_summary_uses_final_adopted_model_and_never_claims_unrelated_sources() {
        let mut checkpoint = InputCheckpoint::new(GenerationInputs::Materials { items: vec![] });
        checkpoint.sources = serde_json::from_value(serde_json::json!([
            {"id":"__source","identity":"old","projection":"perspective","names":["old.png"],"originals":[]},
            {"id":"one","identity":"one","projection":"perspective","names":["a.png"],"originals":[]},
            {"id":"two","identity":"two","projection":"perspective","names":["b.png"],"originals":[]}
        ])).unwrap();
        let records = ["old.png", "a.png", "b.png"]
            .map(String::from)
            .into_iter()
            .collect();
        let final_names = ["old.png", "b.png"].map(String::from).into_iter().collect();
        let summary =
            GpsAssistance::participating(&checkpoint, &records, &final_names, Some("complete"))
                .unwrap();
        assert_eq!(summary.detected_source_ids, vec!["two"]);
        assert_eq!(summary.status, "enabled");
        assert_eq!(
            GpsAssistance::participating(&checkpoint, &records, &final_names, Some("fallback"))
                .unwrap()
                .status,
            "fallback"
        );
        assert!(GpsAssistance::participating(
            &checkpoint,
            &records,
            &HashSet::new(),
            Some("complete")
        )
        .is_none());
    }
    #[test]
    fn reshoot_low_registration_cannot_be_hidden_by_old_frames() {
        let mut c = InputCheckpoint::new(GenerationInputs::Materials {
            items: vec![item(Path::new("new"))],
        });
        let mut plan = super::super::execution_plan::ExecutionPlan::normal();
        plan.success.require_any_new_source = true;
        c.execution_plan = Some(plan);
        let source = |id: &str, n, r| SourceAdoption {
            source_id: id.into(),
            planned_frames: n,
            prepared_frames: n,
            registered_frames: r,
            adopted: r > 0,
            omitted_reason: None,
        };
        c.source_adoption = vec![source("__source", 10000, 10000), source("set-1", 1000, 1)];
        let warning = source_adoption_warning(&c).unwrap();
        assert!(warning.contains("1/1000"));
        assert!(!warning.contains("10001"));
        c.source_adoption = vec![source("set-1", 10, 1), source("second", 1000, 1000)];
        assert!(source_adoption_warning(&c)
            .unwrap()
            .starts_with("new：补拍画面已加入 1/10"));
        c.source_adoption = vec![source("set-1", 100, 80)];
        assert!(source_adoption_warning(&c).is_none());
        c.source_adoption = vec![source("set-1", 100, 0)];
        assert!(source_adoption_warning(&c)
            .unwrap()
            .contains("部分素材未能参与生成"));
        c.source_adoption.clear();
        assert!(source_adoption_warning(&c).is_none());
    }
    fn video(frames: u64, fps: f64) -> VideoInfo {
        VideoInfo {
            duration: frames as f64 / fps,
            fps,
            total_frames: frames,
            width: 1920,
            height: 1080,
            codec: "h264".into(),
            rotation: 0,
            pixel_format: "yuv420p".into(),
            has_alpha: false,
        }
    }
    fn item(path: &Path) -> MaterialInput {
        MaterialInput {
            id: "set-1".into(),
            source: InputSource::ImageFolder { path: path.into() },
            identity: None,
            projection: VideoProjection::Auto,
            image_matching_strategy: Default::default(),
            camera_relation: None,
            confirmed_small_images: false,
            confirmed_image_order: false,
            frame_removal_fps: None,
        }
    }
    fn images(path: &Path, count: usize) {
        std::fs::create_dir_all(path).unwrap();
        for i in 0..count {
            image::RgbImage::new(32, 24)
                .save(path.join(format!("image{i}.png")))
                .unwrap();
        }
    }
    #[test]
    fn version_five_inspection_range_keeps_legacy_limits() {
        let policy = GenerationPolicy {
            version: 5,
            ..GenerationPolicy::default()
        };
        let range = frame_sampling_range(policy, &video(1800, 30.), Quality::Balanced)
            .unwrap()
            .unwrap();
        assert!((range.minimum_target_fps - 0.1).abs() < 1e-9);
        assert!((range.maximum_target_fps - 29.9).abs() < 1e-9);
        assert_eq!(range.minimum.estimated_frames, 30);
        assert!((range.minimum.sampling_fps - 0.5).abs() < 1e-9);
        assert_eq!(range.maximum.estimated_frames, 1794);
        assert!((range.maximum.sampling_fps - 29.9).abs() < 1e-9);
    }
    #[test]
    fn version_six_inspection_range_keeps_the_exact_source_endpoint() {
        let mut clip = video(1800, 30.);
        clip.duration = 60.2;
        let range = frame_sampling_range(GenerationPolicy::default(), &clip, Quality::Balanced)
            .unwrap()
            .unwrap();
        assert_eq!(range.maximum_target_fps, 30.);
        assert_eq!(range.maximum.estimated_frames, 1800);
        assert_eq!(range.maximum.sampling_fps, 30.);
        assert_eq!(range.maximum.retention_ratio, 1.);
    }
    #[tokio::test]
    async fn shared_image_preflight_accepts_mixed_sizes_and_replans_without_reading_headers() {
        let root = tempfile::tempdir().unwrap();
        images(root.path(), 3);
        image::RgbImage::new(48, 64)
            .save(root.path().join("image1.png"))
            .unwrap();
        let engines = crate::pipeline::runner::default_engine_paths(Some(
            root.path().join("missing-engines"),
        ));
        let input = item(root.path());
        let policy = GenerationPolicy::default();
        let first = inspect_material(&engines, &input, Quality::Balanced, policy)
            .await
            .unwrap();
        assert_eq!(first.image_sequence.unwrap().image_count, 3);
        let reads = crate::video::image_projection::header_reads_for_test(root.path());
        let second = inspect_material(
            &engines,
            &input,
            Quality::High,
            GenerationPolicy {
                frame_sampling_fps: Some(7.5),
                ..policy
            },
        )
        .await
        .unwrap();
        assert_eq!(second.identity, first.identity);
        assert_eq!(
            crate::video::image_projection::header_reads_for_test(root.path()),
            reads
        );
    }
    #[test]
    fn image_members_are_natural_sorted_identity_bound_and_overlap_checked() {
        let root = tempfile::tempdir().unwrap();
        images(root.path(), 12);
        let mut input = item(root.path());
        let members = input.source.members().unwrap();
        assert_eq!(members[2].file_name().unwrap(), "image2.png");
        assert_eq!(members[10].file_name().unwrap(), "image10.png");
        input.identity = Some(input.source.identity().unwrap());
        let request = GenerationInputs::Materials {
            items: vec![input.clone()],
        };
        request.check_identities().unwrap();
        image::RgbImage::new(40, 30)
            .save(root.path().join("image0.png"))
            .unwrap();
        assert!(request
            .check_identities()
            .unwrap_err()
            .to_string()
            .contains("INPUT_CHANGED"));
        let mut overlap = input.clone();
        overlap.id = "loose".into();
        overlap.identity = None;
        overlap.source = InputSource::Images {
            paths: members[..2].to_vec(),
        };
        assert!(GenerationInputs::Materials {
            items: vec![input, overlap]
        }
        .authorized_paths()
        .unwrap_err()
        .to_string()
        .contains(DUPLICATE));
    }
    #[tokio::test]
    async fn image_order_is_required_per_set_above_one_hundred_and_small_sets_need_consent() {
        let root = tempfile::tempdir().unwrap();
        images(root.path(), 101);
        let engines =
            crate::pipeline::runner::default_engine_paths(Some(root.path().join("missing")));
        let input = item(root.path());
        let request = GenerationInputs::Materials {
            items: vec![input.clone()],
        };
        assert!(validate(
            &engines,
            &request,
            Quality::Fast,
            GenerationPolicy::default()
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("CONFIRMATION_REQUIRED"));
        let request = GenerationInputs::Materials {
            items: vec![MaterialInput {
                confirmed_image_order: true,
                image_matching_strategy: ImageMatchingStrategy::Sequential,
                ..input
            }],
        };
        validate(
            &engines,
            &request,
            Quality::Fast,
            GenerationPolicy::default(),
        )
        .await
        .unwrap();
        let small = root.path().join("small");
        images(&small, 2);
        let mut input = item(&small);
        assert!(validate(
            &engines,
            &GenerationInputs::Materials {
                items: vec![input.clone()]
            },
            Quality::Fast,
            GenerationPolicy::default()
        )
        .await
        .is_err());
        input.confirmed_small_images = true;
        let invalid_removal = MaterialInput {
            frame_removal_fps: Some(1.0),
            ..input.clone()
        };
        assert!(validate(
            &engines,
            &GenerationInputs::Materials {
                items: vec![invalid_removal]
            },
            Quality::Fast,
            GenerationPolicy::default(),
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("only for video"));
        validate(
            &engines,
            &GenerationInputs::Materials { items: vec![input] },
            Quality::Fast,
            GenerationPolicy::default(),
        )
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn two_to_one_images_require_choice_without_claiming_metadata_and_cannot_bypass_dimensions(
    ) {
        let root = tempfile::tempdir().unwrap();
        images(root.path(), 2);
        for name in ["image0.png", "image1.png"] {
            image::RgbImage::new(128, 64)
                .save(root.path().join(name))
                .unwrap();
        }
        let engines =
            crate::pipeline::runner::default_engine_paths(Some(root.path().join("missing")));
        let mut input = item(root.path());
        let a = inspect_material(&engines, &input, Quality::Fast, GenerationPolicy::default())
            .await
            .unwrap();
        assert!(a.projection_detection.requires_choice);
        assert!(!a.projection_detection.confirmed_erp);
        assert!(projection(&input, &a).is_err());
        input.projection = VideoProjection::Perspective;
        assert_eq!(
            projection(&input, &a).unwrap(),
            VideoProjection::Perspective
        );
        image::RgbImage::new(256, 128)
            .save(root.path().join("image1.png"))
            .unwrap();
        let a = inspect_material(&engines, &input, Quality::Fast, GenerationPolicy::default())
            .await
            .unwrap();
        assert!(!a.projection_detection.supported_erp);
        input.projection = VideoProjection::Equirectangular;
        assert!(projection(&input, &a).is_err());
    }
}
