use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};

use serde::{Deserialize, Serialize};
use tauri::{ipc::InvokeBody, Emitter, Manager, State};
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::{
    engines::{
        ffprobe::probe_video, health::check_colmap_acceleration as detect_colmap_acceleration,
        ColmapAccelerationStatus, EnginePaths, EngineStatus,
    },
    error::{Result, SplatError},
    pipeline::{
        estimate::{estimate_runtime, estimate_runtime_for_images, RuntimeEstimate},
        runner::{PipelineResult, PipelineRunner},
    },
    presets::Quality,
    project::{
        catalog::{self, AppSettings, ProjectOverview},
        manager::atomic_write_json,
        GaussianCrop, GaussianEditing, GaussianTransform, PipelineStateFile, ProjectInputType,
        ProjectStatus,
    },
    reconstruction::{
        edit_mask::{
            cleanup_old_masks, count_deleted, mask_path, packed_mask_bytes, read_mask,
            remove_edit_files, write_mask_atomic,
        },
        ply::inspect_gaussian_ply,
        splat_transform::{export_transformed_ply_with_edits, GaussianExportEdits},
    },
    telemetry::{
        PipelineTelemetrySession, TelemetryInputType, TelemetryPreferences, TelemetryService,
    },
    video::{
        analyze_image_sequence, create_image_plan, FramePlan, FrameSelectionStrategy,
        ImageSequenceInfo, UniformRatioFrameSelection, VideoInfo,
    },
};

#[derive(Default)]
pub struct PipelineController {
    active: Mutex<Option<Arc<PipelineRunner>>>,
}

#[derive(Default)]
pub struct PreviewController {
    active: Mutex<Option<GaussianPreviewSession>>,
    metadata_write: Mutex<()>,
    export: Mutex<()>,
    video_export: Mutex<Option<GaussianVideoExportSession>>,
    edit_save: Mutex<Option<GaussianEditSaveSession>>,
}

#[derive(Debug, Clone)]
struct GaussianPreviewSession {
    project_id: Uuid,
    asset_paths: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
struct GaussianEditSaveSession {
    edit_id: Uuid,
    project_id: Uuid,
    base_revision: u64,
    next_revision: u64,
    splat_count: u64,
    crop: Option<GaussianCrop>,
}

#[derive(Debug, Clone)]
struct GaussianVideoExportSession {
    export_id: Uuid,
    project_id: Uuid,
    destination: PathBuf,
    temporary: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GaussianPreviewDescriptor {
    project_id: Uuid,
    model_path: PathBuf,
    asset_path: PathBuf,
    format: &'static str,
    file_size: u64,
    splat_count: u64,
    transform: GaussianTransform,
    editing: GaussianEditing,
    edit_mask_asset_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GaussianEditDraft {
    crop: Option<GaussianCrop>,
    base_revision: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GaussianEditSaveReservation {
    edit_id: Uuid,
    expected_mask_bytes: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GaussianExportProgress {
    project_id: Uuid,
    processed_splats: u64,
    total_splats: u64,
    progress: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GaussianExportResult {
    path: PathBuf,
    file_size: u64,
    splat_count: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GaussianVideoExportReservation {
    export_id: Uuid,
    destination_path: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GaussianVideoExportResult {
    path: PathBuf,
    file_size: u64,
    width: u32,
    height: u32,
    fps: u32,
    duration_ms: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeAndPlan {
    input_type: ProjectInputType,
    video: Option<VideoInfo>,
    image_sequence: Option<ImageSequenceInfo>,
    plan: FramePlan,
    estimate: RuntimeEstimate,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReshootRequest {
    source_project_id: String,
    reshoot_path: String,
    quality: Quality,
    projects_root: String,
    regions: Vec<GaussianCrop>,
    guidance: Vec<String>,
    /// PNG data URLs with the circled region and its shooting directions.
    #[serde(default)]
    guidance_images: Vec<String>,
}

fn paths_for_app(app: &tauri::AppHandle) -> EnginePaths {
    EnginePaths::discover(app.path().resource_dir().ok().as_deref())
}

#[tauri::command]
pub async fn check_engines(app: tauri::AppHandle) -> Vec<EngineStatus> {
    paths_for_app(&app).check_all().await
}

#[tauri::command]
pub async fn check_colmap_acceleration(app: tauri::AppHandle) -> ColmapAccelerationStatus {
    detect_colmap_acceleration(&paths_for_app(&app)).await
}

#[tauri::command]
pub async fn probe_and_plan(
    app: tauri::AppHandle,
    path: String,
    quality: Quality,
) -> std::result::Result<ProbeAndPlan, SplatError> {
    let engine_paths = paths_for_app(&app);
    let samples = catalog::runtime_samples().await;
    let input = PathBuf::from(path);
    if input.is_dir() {
        let image_sequence = tokio::task::spawn_blocking({
            let input = input.clone();
            move || analyze_image_sequence(&input)
        })
        .await
        .map_err(|error| SplatError::Process(format!("图片序列分析任务失败：{error}")))??;
        let plan = create_image_plan(&image_sequence, &quality.preset());
        let estimate =
            estimate_runtime_for_images(image_sequence.image_count, &plan, quality, &samples);
        Ok(ProbeAndPlan {
            input_type: ProjectInputType::Images,
            video: None,
            image_sequence: Some(image_sequence),
            plan,
            estimate,
        })
    } else {
        let video = probe_video(&engine_paths.ffprobe, &input, None).await?;
        let plan = UniformRatioFrameSelection.create_plan(&video, &quality.preset());
        let estimate = estimate_runtime(&video, &plan, quality, &samples);
        Ok(ProbeAndPlan {
            input_type: ProjectInputType::Video,
            video: Some(video),
            image_sequence: None,
            plan,
            estimate,
        })
    }
}

fn resume_checkpoint_fraction(state: &PipelineStateFile) -> (f64, &'static str) {
    if state.brush_complete {
        (0.98, "结果发布")
    } else if state.reconstruction_complete {
        (0.60, "Brush 训练")
    } else if state.matching_complete {
        (0.45, "相机重建")
    } else if state.features_complete {
        (0.32, "顺序匹配")
    } else if state
        .frames
        .as_ref()
        .and_then(|frames| frames.extracted_frames)
        .is_some_and(|count| count > 0)
    {
        (0.20, "特征提取")
    } else {
        (0.0, "画面提取")
    }
}

#[tauri::command]
pub async fn estimate_project_runtime(
    app: tauri::AppHandle,
    project_id: Uuid,
) -> Result<RuntimeEstimate> {
    let (project, metadata) = catalog::load_registered_project(project_id).await?;
    let state_bytes = tokio::fs::read(project.join("state.json")).await?;
    let state: PipelineStateFile = serde_json::from_slice(&state_bytes)?;
    let saved_plan = state.frames.as_ref().map(|frames| FramePlan {
        retention_ratio: frames.retention_ratio,
        sampling_fps: frames.sampling_fps,
        estimated_frames: frames
            .extracted_frames
            .unwrap_or(frames.estimated_frames)
            .max(1),
    });
    let samples = catalog::runtime_samples().await;
    let mut estimate = match metadata.input_type {
        ProjectInputType::Video => {
            let video = match state.video.clone() {
                Some(video) => video,
                None => {
                    probe_video(&paths_for_app(&app).ffprobe, &metadata.source_path, None).await?
                }
            };
            let plan = saved_plan.unwrap_or_else(|| {
                UniformRatioFrameSelection.create_plan(&video, &metadata.quality.preset())
            });
            estimate_runtime(&video, &plan, metadata.quality, &samples)
        }
        ProjectInputType::Images => {
            let image_sequence = match state.image_sequence.clone() {
                Some(info) => info,
                None => tokio::task::spawn_blocking({
                    let source = metadata.source_path.clone();
                    move || analyze_image_sequence(&source)
                })
                .await
                .map_err(|error| SplatError::Process(format!("图片序列分析任务失败：{error}")))??,
            };
            let plan = saved_plan
                .unwrap_or_else(|| create_image_plan(&image_sequence, &metadata.quality.preset()));
            estimate_runtime_for_images(
                image_sequence.image_count,
                &plan,
                metadata.quality,
                &samples,
            )
        }
    };
    let previous_duration = metadata.duration_ms.unwrap_or(0);
    let (completed_fraction, next_stage) = resume_checkpoint_fraction(&state);
    let remaining_fraction = 1.0 - completed_fraction;
    let remaining = |total: u64| ((total as f64 * remaining_fraction).round() as u64).max(1_000);
    estimate.estimated_ms = previous_duration.saturating_add(remaining(estimate.estimated_ms));
    estimate.lower_bound_ms = previous_duration.saturating_add(remaining(estimate.lower_bound_ms));
    estimate.upper_bound_ms = previous_duration.saturating_add(remaining(estimate.upper_bound_ms));
    estimate.basis = format!(
        "{}；已计入此前耗时，预计从{next_stage}阶段继续",
        estimate.basis
    );
    Ok(estimate)
}

#[tauri::command]
pub async fn get_project_overview(
    state: State<'_, PipelineController>,
) -> std::result::Result<ProjectOverview, SplatError> {
    let mut overview = catalog::get_overview().await?;
    if state.active.lock().await.is_none() {
        for project in &mut overview.projects {
            if project.status == ProjectStatus::Running {
                project.status = ProjectStatus::Interrupted;
            }
        }
    }
    Ok(overview)
}

#[tauri::command]
pub async fn set_projects_root(
    projects_root: String,
) -> std::result::Result<AppSettings, SplatError> {
    catalog::save_projects_root(PathBuf::from(projects_root)).await
}

#[tauri::command]
pub async fn initialize_telemetry(
    telemetry: State<'_, TelemetryService>,
) -> std::result::Result<TelemetryPreferences, SplatError> {
    telemetry.initialize().await
}

#[tauri::command]
pub async fn set_telemetry_consent(
    telemetry: State<'_, TelemetryService>,
    enabled: bool,
) -> std::result::Result<TelemetryPreferences, SplatError> {
    telemetry.set_consent(enabled).await
}

#[tauri::command]
pub async fn start_pipeline(
    app: tauri::AppHandle,
    state: State<'_, PipelineController>,
    telemetry: State<'_, TelemetryService>,
    path: String,
    quality: Quality,
    projects_root: String,
) -> std::result::Result<PipelineResult, SplatError> {
    let emitter = app.clone();
    let started = Instant::now();
    let telemetry_session = Arc::new(PipelineTelemetrySession::new(
        telemetry.inner().clone(),
        quality,
        if Path::new(&path).is_dir() {
            TelemetryInputType::Images
        } else {
            TelemetryInputType::Video
        },
    ));
    let event_telemetry = telemetry_session.clone();
    let runner = Arc::new(PipelineRunner::new(paths_for_app(&app), move |event| {
        event_telemetry.observe(&event);
        let _ = emitter.emit("pipeline-event", event);
    }));
    {
        let mut active = state.active.lock().await;
        if active.is_some() {
            return Err(SplatError::Process("已有任务正在运行".into()));
        }
        *active = Some(runner.clone());
    }
    telemetry_session.generation_started();
    let result = runner
        .generate(Path::new(&path), quality, Path::new(&projects_root))
        .await;
    match &result {
        Ok(output) => telemetry_session.generation_completed(
            output.duration_ms,
            output.input_images,
            output.source_duration_seconds,
        ),
        Err(error) => telemetry_session.generation_failed(error),
    }
    if let Err(error) = &result {
        let stage = if matches!(error, SplatError::Cancelled) {
            crate::pipeline::PipelineStage::Cancelled
        } else {
            crate::pipeline::PipelineStage::Failed
        };
        let mut event = crate::pipeline::PipelineEvent::mapped(stage, 1.0, error.to_string());
        event.elapsed_ms = started.elapsed().as_millis() as u64;
        let _ = app.emit("pipeline-event", event);
    }
    *state.active.lock().await = None;
    result
}

#[tauri::command]
pub async fn resume_pipeline(
    app: tauri::AppHandle,
    state: State<'_, PipelineController>,
    telemetry: State<'_, TelemetryService>,
    project_id: String,
) -> std::result::Result<PipelineResult, SplatError> {
    let project_id = parse_project_id(&project_id)?;
    let (_, metadata) = catalog::load_registered_project(project_id).await?;
    let emitter = app.clone();
    let started = Instant::now();
    let telemetry_session = Arc::new(PipelineTelemetrySession::new(
        telemetry.inner().clone(),
        metadata.quality,
        match metadata.input_type {
            ProjectInputType::Video => TelemetryInputType::Video,
            ProjectInputType::Images => TelemetryInputType::Images,
        },
    ));
    let event_telemetry = telemetry_session.clone();
    let runner = Arc::new(PipelineRunner::new(paths_for_app(&app), move |event| {
        event_telemetry.observe(&event);
        let _ = emitter.emit("pipeline-event", event);
    }));
    {
        let mut active = state.active.lock().await;
        if active.is_some() {
            return Err(SplatError::Process("已有任务正在运行".into()));
        }
        *active = Some(runner.clone());
    }
    telemetry_session.generation_started();
    let result = runner.resume(project_id).await;
    match &result {
        Ok(output) => telemetry_session.generation_completed(
            output.duration_ms,
            output.input_images,
            output.source_duration_seconds,
        ),
        Err(error) => telemetry_session.generation_failed(error),
    }
    if let Err(error) = &result {
        let stage = if matches!(error, SplatError::Cancelled) {
            crate::pipeline::PipelineStage::Cancelled
        } else {
            crate::pipeline::PipelineStage::Failed
        };
        let mut event = crate::pipeline::PipelineEvent::mapped(stage, 1.0, error.to_string());
        event.elapsed_ms = started.elapsed().as_millis() as u64;
        let _ = app.emit("pipeline-event", event);
    }
    *state.active.lock().await = None;
    result
}

#[tauri::command]
pub async fn start_reshoot_pipeline(
    app: tauri::AppHandle,
    state: State<'_, PipelineController>,
    request: ReshootRequest,
) -> std::result::Result<PipelineResult, SplatError> {
    let source_project_id = Uuid::parse_str(&request.source_project_id)
        .map_err(|_| SplatError::Process("原项目 ID 无效".into()))?;
    if request.regions.is_empty() {
        return Err(SplatError::Process(
            "请先在预览中圈选至少一个模糊区域".into(),
        ));
    }
    for region in &request.regions {
        region.validate()?;
    }
    if request.guidance.len() != request.regions.len()
        || request.guidance_images.len() != request.regions.len()
    {
        return Err(SplatError::Process(
            "补拍区域与补拍指引数量不一致，请重新圈选区域".into(),
        ));
    }
    let emitter = app.clone();
    let started = Instant::now();
    let runner = Arc::new(PipelineRunner::new(paths_for_app(&app), move |event| {
        let _ = emitter.emit("pipeline-event", event);
    }));
    {
        let mut active = state.active.lock().await;
        if active.is_some() {
            return Err(SplatError::Process("已有任务正在运行".into()));
        }
        *active = Some(runner.clone());
    }
    let result = runner
        .generate_reshoot(
            source_project_id,
            Path::new(&request.reshoot_path),
            request.quality,
            Path::new(&request.projects_root),
            crate::pipeline::runner::ReshootPlan {
                regions: request.regions,
                guidance: request.guidance,
                guidance_images: request.guidance_images,
            },
        )
        .await;
    if let Err(error) = &result {
        let stage = if matches!(error, SplatError::Cancelled) {
            crate::pipeline::PipelineStage::Cancelled
        } else {
            crate::pipeline::PipelineStage::Failed
        };
        let mut event = crate::pipeline::PipelineEvent::mapped(stage, 1.0, error.to_string());
        event.elapsed_ms = started.elapsed().as_millis() as u64;
        let _ = app.emit("pipeline-event", event);
    }
    *state.active.lock().await = None;
    result
}

#[tauri::command]
pub async fn cancel_pipeline(state: State<'_, PipelineController>) -> Result<()> {
    if let Some(runner) = state.active.lock().await.as_ref() {
        runner.cancel();
    }
    Ok(())
}

#[tauri::command]
pub async fn delete_project(
    app: tauri::AppHandle,
    state: State<'_, PipelineController>,
    preview: State<'_, PreviewController>,
    project_id: String,
) -> Result<()> {
    if state.active.lock().await.is_some() {
        return Err(SplatError::Process("任务运行期间不能删除项目".into()));
    }
    let id =
        Uuid::parse_str(&project_id).map_err(|_| SplatError::Process("项目 ID 无效".into()))?;
    let mut active = preview.active.lock().await;
    if active
        .as_ref()
        .is_some_and(|session| session.project_id == id)
    {
        if let Some(session) = active.take() {
            discard_preview_asset(&app, session).await;
        }
    }
    drop(active);
    let mut edit_save = preview.edit_save.lock().await;
    if edit_save
        .as_ref()
        .is_some_and(|session| session.project_id == id)
    {
        *edit_save = None;
    }
    drop(edit_save);
    catalog::delete_project(id).await
}

fn parse_project_id(project_id: &str) -> Result<Uuid> {
    Uuid::parse_str(project_id).map_err(|_| SplatError::Process("项目 ID 无效".into()))
}

fn preview_client_path(path: &Path) -> PathBuf {
    let value = path.to_string_lossy();
    if let Some(unc) = value.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{unc}"))
    } else if let Some(local) = value.strip_prefix(r"\\?\") {
        PathBuf::from(local)
    } else {
        path.to_path_buf()
    }
}

async fn create_preview_asset(project_root: &Path, source: &Path) -> Result<PathBuf> {
    let directory = project_root.join("work").join("preview");
    tokio::fs::create_dir_all(&directory).await?;
    let asset_path = directory.join(format!("preview-{}.ply", Uuid::new_v4().simple()));

    // A per-session hard link gives the asset protocol a fresh URL without copying a large PLY.
    // Some network filesystems do not support hard links, so retain a copy fallback for UNC roots.
    if let Err(link_error) = tokio::fs::hard_link(source, &asset_path).await {
        tokio::fs::copy(source, &asset_path)
            .await
            .map_err(|copy_error| {
                SplatError::Process(format!(
                    "无法准备高斯泼溅预览文件（硬链接：{link_error}；复制：{copy_error}）"
                ))
            })?;
    }
    Ok(asset_path)
}

async fn discard_preview_asset(app: &tauri::AppHandle, session: GaussianPreviewSession) {
    // `forbid_file` is permanent for the lifetime of this Tauri scope. It is therefore only safe
    // for the unique session alias, never for final.ply, which must be previewable again later.
    let parent = session
        .asset_paths
        .first()
        .and_then(|path| path.parent())
        .map(Path::to_path_buf);
    for asset_path in session.asset_paths {
        let _ = app.asset_protocol_scope().forbid_file(&asset_path);
        let _ = tokio::fs::remove_file(asset_path).await;
    }
    if let Some(parent) = parent {
        let _ = tokio::fs::remove_dir(parent).await;
    }
}

#[tauri::command]
pub async fn prepare_gaussian_preview(
    app: tauri::AppHandle,
    state: State<'_, PreviewController>,
    project_id: String,
) -> Result<GaussianPreviewDescriptor> {
    let id = parse_project_id(&project_id)?;
    let (project_root, path, metadata) = catalog::registered_final_ply_for_project(id).await?;
    let info = inspect_gaussian_ply(&path)?;
    let transform = metadata.transform.validate()?;
    let editing = metadata.editing.validate(info.splat_count)?;
    let edit_mask = read_mask(&project_root, editing).map_err(|error| {
        SplatError::Process(format!(
            "Gaussian 编辑数据已损坏或与源文件不兼容：{error}。可在预览中重置编辑状态后恢复。"
        ))
    })?;

    let mut active = state.active.lock().await;
    if let Some(previous) = active.take() {
        discard_preview_asset(&app, previous).await;
    }
    let asset_path = create_preview_asset(&project_root, &path).await?;
    app.asset_protocol_scope()
        .allow_file(&asset_path)
        .map_err(|error| {
            let _ = std::fs::remove_file(&asset_path);
            SplatError::Process(format!("无法开放本地 PLY 预览资源：{error}"))
        })?;
    let mut asset_paths = vec![asset_path.clone()];
    let edit_mask_asset_path = if editing.revision > 0 || editing.deleted_count > 0 {
        let path = asset_path.with_extension("mask.bin");
        tokio::fs::write(&path, &edit_mask).await?;
        app.asset_protocol_scope()
            .allow_file(&path)
            .map_err(|error| {
                let _ = std::fs::remove_file(&path);
                SplatError::Process(format!("无法开放 Gaussian 编辑位图资源：{error}"))
            })?;
        asset_paths.push(path.clone());
        Some(preview_client_path(&path))
    } else {
        None
    };
    *active = Some(GaussianPreviewSession {
        project_id: id,
        asset_paths,
    });

    Ok(GaussianPreviewDescriptor {
        project_id: id,
        model_path: preview_client_path(&path),
        asset_path: preview_client_path(&asset_path),
        format: "ply",
        file_size: info.file_size,
        splat_count: info.splat_count,
        transform,
        editing,
        edit_mask_asset_path,
    })
}

#[tauri::command]
pub async fn release_gaussian_preview(
    app: tauri::AppHandle,
    state: State<'_, PreviewController>,
    project_id: String,
) -> Result<()> {
    let id = parse_project_id(&project_id)?;
    let mut active = state.active.lock().await;
    if active
        .as_ref()
        .is_some_and(|session| session.project_id == id)
    {
        if let Some(session) = active.take() {
            discard_preview_asset(&app, session).await;
        }
    }
    drop(active);

    let mut edit_save = state.edit_save.lock().await;
    if edit_save
        .as_ref()
        .is_some_and(|session| session.project_id == id)
    {
        *edit_save = None;
    }
    drop(edit_save);

    let mut video_export = state.video_export.lock().await;
    if video_export
        .as_ref()
        .is_some_and(|session| session.project_id == id)
    {
        if let Some(session) = video_export.take() {
            let _ = tokio::fs::remove_file(session.temporary).await;
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn begin_gaussian_edit_save(
    state: State<'_, PreviewController>,
    project_id: String,
    edit_state: GaussianEditDraft,
) -> Result<GaussianEditSaveReservation> {
    let project_id = parse_project_id(&project_id)?;
    let (_, source, metadata) = catalog::registered_final_ply_for_project(project_id).await?;
    let info = inspect_gaussian_ply(&source)?;
    let current = metadata.editing.validate(info.splat_count)?;
    if current.revision != edit_state.base_revision {
        return Err(SplatError::Process(format!(
            "Gaussian 编辑状态已更新（当前修订 {}，提交基于 {}），请重新加载后再试",
            current.revision, edit_state.base_revision
        )));
    }
    if let Some(crop) = edit_state.crop {
        crop.validate()?;
    }
    if !state
        .active
        .lock()
        .await
        .as_ref()
        .is_some_and(|session| session.project_id == project_id)
    {
        return Err(SplatError::Process(
            "项目当前未在预览中打开，无法保存编辑".into(),
        ));
    }
    let mut active = state.edit_save.lock().await;
    if active.is_some() {
        return Err(SplatError::Process("已有 Gaussian 编辑正在保存".into()));
    }
    let edit_id = Uuid::new_v4();
    let next_revision = current.revision.saturating_add(1);
    *active = Some(GaussianEditSaveSession {
        edit_id,
        project_id,
        base_revision: current.revision,
        next_revision,
        splat_count: info.splat_count,
        crop: edit_state.crop,
    });
    Ok(GaussianEditSaveReservation {
        edit_id,
        expected_mask_bytes: packed_mask_bytes(info.splat_count)?,
    })
}

#[tauri::command]
pub async fn commit_gaussian_edit_save(
    state: State<'_, PreviewController>,
    request: tauri::ipc::Request<'_>,
) -> Result<GaussianEditing> {
    let edit_id = request
        .headers()
        .get("x-ooosplat-edit-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or_else(|| SplatError::Process("Gaussian 编辑保存令牌无效".into()))?;
    let mask = match request.body() {
        InvokeBody::Raw(bytes) => bytes.as_slice(),
        _ => {
            return Err(SplatError::Process(
                "Gaussian 编辑位图必须通过原始二进制 IPC 提交".into(),
            ))
        }
    };
    let mut active = state.edit_save.lock().await;
    if !active
        .as_ref()
        .is_some_and(|session| session.edit_id == edit_id)
    {
        return Err(SplatError::Process(
            "Gaussian 编辑保存会话不存在或已结束".into(),
        ));
    }
    // Taking the matching session while retaining the mutex guard guarantees every
    // success or failure closes this token and no second save can overtake it.
    let session = active.take().expect("matching session checked above");
    if mask.len() != packed_mask_bytes(session.splat_count)? {
        return Err(SplatError::Process(
            "Gaussian 编辑位图长度与源文件不一致".into(),
        ));
    }
    let deleted_count = count_deleted(mask, session.splat_count)?;
    let _write_guard = state.metadata_write.lock().await;
    let (root, _, mut metadata) =
        catalog::registered_final_ply_for_project(session.project_id).await?;
    if metadata.editing.revision != session.base_revision {
        return Err(SplatError::Process(
            "保存期间编辑状态已发生变化，请重新加载".into(),
        ));
    }
    let editing = GaussianEditing {
        crop: session.crop,
        revision: session.next_revision,
        source_splat_count: session.splat_count,
        deleted_count,
    };
    // A process crash can leave a future revision file behind before project.json
    // was published. It is not referenced by current metadata and is safe to replace.
    let destination = mask_path(&root, editing.revision);
    if destination.exists() {
        std::fs::remove_file(&destination)?;
    }
    write_mask_atomic(&root, editing.revision, editing.source_splat_count, mask)?;
    metadata.schema_version = crate::project::metadata::schema_version();
    metadata.editing = editing;
    if let Err(error) = atomic_write_json(&root.join("project.json"), &metadata).await {
        let _ = std::fs::remove_file(mask_path(&root, editing.revision));
        return Err(error);
    }
    cleanup_old_masks(&root, editing.revision);
    drop(active);
    Ok(editing)
}

#[tauri::command]
pub async fn reset_gaussian_edits(
    state: State<'_, PreviewController>,
    project_id: String,
) -> Result<GaussianEditing> {
    let id = parse_project_id(&project_id)?;
    let mut active = state.edit_save.lock().await;
    if active
        .as_ref()
        .is_some_and(|session| session.project_id == id)
    {
        *active = None;
    }
    drop(active);
    let _write_guard = state.metadata_write.lock().await;
    let (root, source, mut metadata) = catalog::registered_final_ply_for_project(id).await?;
    let splat_count = inspect_gaussian_ply(&source)?.splat_count;
    metadata.schema_version = crate::project::metadata::schema_version();
    metadata.editing = GaussianEditing {
        source_splat_count: splat_count,
        ..GaussianEditing::default()
    };
    atomic_write_json(&root.join("project.json"), &metadata).await?;
    // The metadata switch is the authoritative reset. Old mask files are now
    // unreachable, so a cleanup failure must not prevent reopening final.ply.
    let _ = remove_edit_files(&root);
    Ok(metadata.editing)
}

#[tauri::command]
pub async fn save_gaussian_transform(
    state: State<'_, PreviewController>,
    project_id: String,
    transform: GaussianTransform,
) -> Result<GaussianTransform> {
    let id = parse_project_id(&project_id)?;
    let transform = transform.validate()?;
    let _write_guard = state.metadata_write.lock().await;
    let (root, _, mut metadata) = catalog::registered_final_ply_for_project(id).await?;
    metadata.schema_version = crate::project::metadata::schema_version();
    metadata.model = "final.ply".into();
    metadata.transform = transform;
    atomic_write_json(&root.join("project.json"), &metadata).await?;
    Ok(transform)
}

#[tauri::command]
pub async fn export_transformed_gaussian(
    app: tauri::AppHandle,
    state: State<'_, PreviewController>,
    project_id: String,
    transform: GaussianTransform,
    edit_revision: Option<u64>,
) -> Result<GaussianExportResult> {
    let id = parse_project_id(&project_id)?;
    let transform = transform.validate()?;
    let _export_guard = state.export.lock().await;
    let (root, source, metadata) = catalog::registered_final_ply_for_project(id).await?;
    let source_info = inspect_gaussian_ply(&source)?;
    let editing = metadata.editing.validate(source_info.splat_count)?;
    if edit_revision.is_some_and(|revision| revision != editing.revision) {
        return Err(SplatError::Process(format!(
            "编辑状态尚未保存完成（导出请求修订 {:?}，当前修订 {}）",
            edit_revision, editing.revision
        )));
    }
    let mask = read_mask(&root, editing)?;
    let emitter = app.clone();
    let (path, info) = tokio::task::spawn_blocking(move || {
        export_transformed_ply_with_edits(
            &source,
            &root,
            transform,
            GaussianExportEdits {
                crop: editing.crop,
                deleted_mask: Some(&mask),
            },
            |processed, total| {
                let _ = emitter.emit(
                    "gaussian-export-progress",
                    GaussianExportProgress {
                        project_id: id,
                        processed_splats: processed,
                        total_splats: total,
                        progress: if total == 0 {
                            0.0
                        } else {
                            processed as f64 / total as f64 * 100.0
                        },
                    },
                );
            },
        )
    })
    .await
    .map_err(|error| SplatError::Process(format!("Gaussian 导出线程失败：{error}")))??;
    Ok(GaussianExportResult {
        path,
        file_size: info.file_size,
        splat_count: info.splat_count,
    })
}

const GAUSSIAN_VIDEO_WIDTH: u32 = 1080;
const GAUSSIAN_VIDEO_HEIGHT: u32 = 1920;
const GAUSSIAN_VIDEO_FPS: u32 = 30;
const GAUSSIAN_VIDEO_DURATION_MS: u64 = 23_000;
const MAX_GAUSSIAN_VIDEO_BYTES: usize = 1024 * 1024 * 1024;

fn next_gaussian_video_path(root: &Path) -> PathBuf {
    for number in 1_u32.. {
        let name = if number == 1 {
            "preview.mp4".to_owned()
        } else {
            format!("preview-{number}.mp4")
        };
        let candidate = root.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!("video export numbering is unbounded")
}

fn contains_mp4_ftyp(bytes: &[u8]) -> bool {
    bytes
        .get(..bytes.len().min(64))
        .is_some_and(|header| header.windows(4).any(|window| window == b"ftyp"))
}

#[tauri::command]
pub async fn begin_gaussian_video_export(
    state: State<'_, PreviewController>,
    project_id: String,
) -> Result<GaussianVideoExportReservation> {
    let project_id = parse_project_id(&project_id)?;
    let (root, _, _) = catalog::registered_final_ply_for_project(project_id).await?;

    let active = state.active.lock().await;
    if active
        .as_ref()
        .is_none_or(|session| session.project_id != project_id)
    {
        return Err(SplatError::Process(
            "该项目当前未在 OOOSplat 预览中打开，无法导出视频。".into(),
        ));
    }
    drop(active);

    let mut video_export = state.video_export.lock().await;
    if video_export.is_some() {
        return Err(SplatError::Process(
            "已有一个高斯预览视频正在导出，请等待其完成或先取消。".into(),
        ));
    }

    let export_id = Uuid::new_v4();
    let destination = next_gaussian_video_path(&root);
    let temporary = root.join(format!(".ooosplat-preview-{export_id}.mp4.tmp"));
    *video_export = Some(GaussianVideoExportSession {
        export_id,
        project_id,
        destination: destination.clone(),
        temporary,
    });

    Ok(GaussianVideoExportReservation {
        export_id,
        destination_path: preview_client_path(&destination),
    })
}

async fn write_gaussian_video(
    session: &GaussianVideoExportSession,
    bytes: &[u8],
) -> Result<GaussianVideoExportResult> {
    if bytes.is_empty() {
        return Err(SplatError::Process("视频编码器返回了空文件。".into()));
    }
    if bytes.len() > MAX_GAUSSIAN_VIDEO_BYTES {
        return Err(SplatError::Process("视频文件超过 1 GB 安全限制。".into()));
    }
    if !contains_mp4_ftyp(bytes) {
        return Err(SplatError::Process(
            "视频编码结果不是有效的 MP4 文件（缺少 ftyp 标识）。".into(),
        ));
    }
    if session.destination.exists() {
        return Err(SplatError::Process(format!(
            "视频目标文件已存在，请重新开始导出：{}",
            session.destination.display()
        )));
    }

    let write_result = async {
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&session.temporary)
            .await?;
        file.write_all(bytes).await?;
        file.flush().await?;
        file.sync_all().await?;
        drop(file);
        tokio::fs::rename(&session.temporary, &session.destination).await?;
        Result::<()>::Ok(())
    }
    .await;

    if let Err(error) = write_result {
        let _ = tokio::fs::remove_file(&session.temporary).await;
        return Err(error);
    }

    Ok(GaussianVideoExportResult {
        path: preview_client_path(&session.destination),
        file_size: bytes.len() as u64,
        width: GAUSSIAN_VIDEO_WIDTH,
        height: GAUSSIAN_VIDEO_HEIGHT,
        fps: GAUSSIAN_VIDEO_FPS,
        duration_ms: GAUSSIAN_VIDEO_DURATION_MS,
    })
}

#[tauri::command]
pub async fn commit_gaussian_video_export(
    state: State<'_, PreviewController>,
    request: tauri::ipc::Request<'_>,
) -> Result<GaussianVideoExportResult> {
    let export_id = request
        .headers()
        .get("x-ooosplat-export-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or_else(|| SplatError::Process("视频导出令牌无效。".into()))?;
    let bytes = match request.body() {
        InvokeBody::Raw(bytes) => bytes.as_slice(),
        _ => {
            return Err(SplatError::Process(
                "视频必须通过原始二进制 IPC 提交。".into(),
            ))
        }
    };

    let mut video_export = state.video_export.lock().await;
    let session = video_export
        .as_ref()
        .filter(|session| session.export_id == export_id)
        .cloned()
        .ok_or_else(|| SplatError::Process("视频导出会话不存在或已经结束。".into()))?;
    let result = write_gaussian_video(&session, bytes).await;
    *video_export = None;
    result
}

#[tauri::command]
pub async fn cancel_gaussian_video_export(
    state: State<'_, PreviewController>,
    export_id: String,
) -> Result<()> {
    let export_id = Uuid::parse_str(&export_id)
        .map_err(|_| SplatError::Process("视频导出令牌无效。".into()))?;
    let mut video_export = state.video_export.lock().await;
    if video_export
        .as_ref()
        .is_some_and(|session| session.export_id == export_id)
    {
        if let Some(session) = video_export.take() {
            let _ = tokio::fs::remove_file(session.temporary).await;
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn export_ply(source_path: String, destination_path: String) -> Result<u64> {
    let source = catalog::validate_registered_final_ply(Path::new(&source_path)).await?;
    let destination = PathBuf::from(destination_path);
    if destination
        .extension()
        .is_none_or(|extension| !extension.eq_ignore_ascii_case("ply"))
    {
        return Err(SplatError::InvalidPath(destination));
    }
    Ok(tokio::fs::copy(source, destination).await?)
}

#[cfg(test)]
mod tests {
    use super::{
        contains_mp4_ftyp, create_preview_asset, next_gaussian_video_path, preview_client_path,
        write_gaussian_video, GaussianVideoExportSession,
    };
    use std::{fs, path::Path};
    use tempfile::tempdir;
    use uuid::Uuid;

    #[test]
    fn preview_path_keeps_regular_paths() {
        assert_eq!(
            preview_client_path(Path::new(r"C:\Projects\场景\final.ply")),
            Path::new(r"C:\Projects\场景\final.ply")
        );
    }

    #[test]
    fn preview_path_removes_windows_verbatim_prefix() {
        assert_eq!(
            preview_client_path(Path::new(r"\\?\C:\Projects\场景\final.ply")),
            Path::new(r"C:\Projects\场景\final.ply")
        );
    }

    #[test]
    fn preview_path_restores_unc_prefix() {
        assert_eq!(
            preview_client_path(Path::new(r"\\?\UNC\server\share\final.ply")),
            Path::new(r"\\server\share\final.ply")
        );
    }

    #[tokio::test]
    async fn preview_assets_use_a_fresh_session_path_without_reusing_final_ply() {
        let root = tempdir().expect("temporary project");
        let source = root.path().join("final.ply");
        fs::write(&source, b"ply preview fixture").expect("source ply");

        let first = create_preview_asset(root.path(), &source)
            .await
            .expect("first preview asset");
        let second = create_preview_asset(root.path(), &source)
            .await
            .expect("second preview asset");

        assert_ne!(first, second);
        assert_ne!(first, source);
        assert!(first.starts_with(root.path().join("work").join("preview")));
        assert_eq!(
            fs::read(first).expect("first preview contents"),
            b"ply preview fixture"
        );
        assert_eq!(
            fs::read(second).expect("second preview contents"),
            b"ply preview fixture"
        );
    }

    #[test]
    fn video_path_uses_the_first_available_preview_number() {
        let root = tempdir().expect("temporary project");
        assert_eq!(
            next_gaussian_video_path(root.path()),
            root.path().join("preview.mp4")
        );
        fs::write(root.path().join("preview.mp4"), b"existing").expect("first video");
        fs::write(root.path().join("preview-2.mp4"), b"existing").expect("second video");
        assert_eq!(
            next_gaussian_video_path(root.path()),
            root.path().join("preview-3.mp4")
        );
    }

    #[test]
    fn mp4_validation_requires_ftyp_near_the_start() {
        expect_ftyp(true, b"\0\0\0\x18ftypisom\0\0\0\0");
        expect_ftyp(false, b"not-an-mp4");
        let mut late = vec![0_u8; 80];
        late[70..74].copy_from_slice(b"ftyp");
        expect_ftyp(false, &late);
    }

    fn expect_ftyp(expected: bool, bytes: &[u8]) {
        assert_eq!(contains_mp4_ftyp(bytes), expected);
    }

    #[tokio::test]
    async fn video_publish_is_atomic_and_never_overwrites_an_existing_export() {
        let root = tempdir().expect("temporary project");
        let destination = root.path().join("preview.mp4");
        let temporary = root.path().join(".preview.tmp");
        let session = GaussianVideoExportSession {
            export_id: Uuid::new_v4(),
            project_id: Uuid::new_v4(),
            destination: destination.clone(),
            temporary: temporary.clone(),
        };
        let bytes = b"\0\0\0\x18ftypisom\0\0\0\0payload";

        let result = write_gaussian_video(&session, bytes)
            .await
            .expect("valid MP4 is published");
        assert_eq!(result.path, destination);
        assert_eq!(fs::read(&destination).expect("published bytes"), bytes);
        assert!(!temporary.exists());

        let error = write_gaussian_video(&session, bytes)
            .await
            .expect_err("existing video must not be overwritten");
        assert!(error.to_string().contains("已存在"));
        assert_eq!(fs::read(&destination).expect("original export"), bytes);
    }

    #[tokio::test]
    async fn invalid_mp4_is_rejected_without_leaving_a_temporary_file() {
        let root = tempdir().expect("temporary project");
        let session = GaussianVideoExportSession {
            export_id: Uuid::new_v4(),
            project_id: Uuid::new_v4(),
            destination: root.path().join("preview.mp4"),
            temporary: root.path().join(".preview.tmp"),
        };

        write_gaussian_video(&session, b"not-an-mp4")
            .await
            .expect_err("invalid MP4 must be rejected");
        assert!(!session.destination.exists());
        assert!(!session.temporary.exists());
    }
}
