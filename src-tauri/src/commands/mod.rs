use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::{ipc::InvokeBody, Emitter, Manager, State};
use tauri_plugin_opener::OpenerExt;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::sync::Mutex;
use uuid::Uuid;

pub mod html_export;

use crate::{
    engines::{
        ffprobe::probe_video,
        health::{
            check_colmap_acceleration as refresh_colmap_acceleration,
            check_colmap_acceleration_cached as detect_colmap_acceleration,
        },
        ColmapAccelerationStatus, EnginePaths, EngineStatus,
    },
    error::{Result, SplatError},
    pipeline::{
        estimate::{
            estimate_runtime_for_images_with_brush_and_resolution,
            estimate_runtime_with_brush_and_resolution, RuntimeEstimate,
        },
        runner::{
            PipelineFailureContext, PipelineResult, PipelineRunner, ReshootInputInfo,
            ReshootInputType, ReshootSourceInfo,
        },
        PipelineEngine, PipelineStage,
    },
    presets::{
        resolve_brush_training_preset, resolve_brush_training_preset_for_plan,
        resolve_planner_resolution_plan, Quality,
    },
    process::ProcessManager,
    project::{
        catalog::{self, AppSettings, ProjectOverview, ProjectSummary},
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
    telemetry::{TelemetryPreferences, TelemetryService},
    video::{
        analyze_image_sequence, create_image_plan, FramePlan, FrameSelectionStrategy,
        ImageSequenceInfo, QualityV2FrameSelection, UniformRatioFrameSelection, VideoInfo,
    },
};

pub type PipelineController = crate::tasks::TaskService;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineCommandError {
    pub(crate) code: &'static str,
    pub(crate) message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) failed_stage: Option<PipelineStage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) engine: Option<PipelineEngine>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) failure_kind: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) project_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) project_path: Option<Box<PathBuf>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    logs_directory: Option<Box<PathBuf>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) failure_id: Option<Box<Uuid>>,
}

impl From<SplatError> for PipelineCommandError {
    fn from(error: SplatError) -> Self {
        Self {
            code: if matches!(error, SplatError::Cancelled) {
                "cancelled"
            } else {
                "pipeline_failed"
            },
            message: error.to_string(),
            failed_stage: None,
            engine: None,
            failure_kind: None,
            project_id: None,
            project_path: None,
            logs_directory: None,
            failure_id: None,
        }
    }
}

impl PipelineCommandError {
    pub(crate) fn from_runner(error: &SplatError, runner: &PipelineRunner) -> Self {
        let message = error.to_string();
        let cancelled = matches!(error, SplatError::Cancelled);
        let PipelineFailureContext {
            failed_stage,
            project_id,
            project_path,
            logs_directory,
        } = runner.failure_context();
        let (engine, failure_kind) = classify_pipeline_failure(failed_stage, &message);
        Self {
            code: if cancelled {
                "cancelled"
            } else {
                "pipeline_failed"
            },
            message,
            failed_stage,
            engine,
            failure_kind: if cancelled { None } else { failure_kind },
            project_id: project_id.map(|value| value.to_string()),
            project_path: project_path.map(Box::new),
            logs_directory: logs_directory.map(Box::new),
            failure_id: None,
        }
    }
}

fn classify_pipeline_failure(
    stage: Option<PipelineStage>,
    message: &str,
) -> (Option<PipelineEngine>, Option<&'static str>) {
    let lower = message.to_ascii_lowercase();
    match stage {
        Some(PipelineStage::Reconstructing | PipelineStage::ValidatingReconstruction) => {
            let source_error = [
                "no initial image pair",
                "no good initial image pair",
                "could not find a good initial image pair",
                "failed to find an initial image pair",
                "failed to register",
                "could not register",
                "no images registered",
                "registered 0 images",
                "discarding reconstruction",
                "failed to create any sparse model",
                "did not produce a usable sparse model",
                "未生成完整的稀疏模型",
                "稀疏重建没有可用的注册图像或三维点",
                "稀疏重建输出不完整",
            ]
            .iter()
            .any(|needle| lower.contains(needle));
            let database_io_error = ["io error", "i/o error"]
                .iter()
                .any(|needle| lower.contains(needle))
                && ["database", "sqlite", "filesystem", "project file"]
                    .iter()
                    .any(|needle| lower.contains(needle));
            let storage_error = database_io_error
                || [
                    "database is locked",
                    "database locked",
                    "unable to open database",
                    "failed to open database",
                    "cannot open database",
                    "unable to open database file",
                    "read-only database",
                    "readonly database",
                    "no space left",
                    "disk full",
                    "input/output error",
                    "being used by another process",
                    "used by another process",
                    "sharing violation",
                    "access denied",
                    "permission denied",
                    "数据库被锁定",
                    "无法打开数据库",
                    "磁盘空间不足",
                    "磁盘已满",
                    "另一个进程正在使用",
                    "拒绝访问",
                    "权限不足",
                ]
                .iter()
                .any(|needle| lower.contains(needle));
            (
                Some(PipelineEngine::Colmap),
                Some(if source_error {
                    "mapper_source"
                } else if storage_error {
                    "mapper_storage"
                } else {
                    "mapper_source"
                }),
            )
        }
        Some(PipelineStage::TrainingSplats) => {
            let device_lost = [
                "devicelost",
                "device lost",
                "device_lost",
                "parent device is lost",
                "vk_error_device_lost",
                "dxgi_error_device_removed",
                "显卡设备连接中断",
            ]
            .iter()
            .any(|needle| lower.contains(needle));
            let dataset_error = [
                "early eof",
                "failed to load dataset",
                "io error",
                "i/o error",
                "no such file",
                "access denied",
                "permission denied",
            ]
            .iter()
            .any(|needle| lower.contains(needle));
            (
                Some(PipelineEngine::Brush),
                Some(if device_lost {
                    "brush_device_lost"
                } else if dataset_error {
                    "brush_dataset"
                } else {
                    "brush_gpu"
                }),
            )
        }
        Some(PipelineStage::ProbingVideo | PipelineStage::ExtractingFrames) => {
            (Some(PipelineEngine::Ffmpeg), None)
        }
        Some(PipelineStage::ExtractingFeatures | PipelineStage::Matching) => {
            (Some(PipelineEngine::Colmap), None)
        }
        _ => (Some(PipelineEngine::System), None),
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DroppedInputInfo {
    input_type: ProjectInputType,
}

async fn classify_dropped_input_path(path: &Path) -> Result<DroppedInputInfo> {
    let metadata = tokio::fs::metadata(path)
        .await
        .map_err(|error| SplatError::Process(format!("拖入的素材不存在或无法访问：{error}")))?;
    if metadata.is_dir() {
        return Ok(DroppedInputInfo {
            input_type: ProjectInputType::Images,
        });
    }
    if !metadata.is_file() {
        return Err(SplatError::Process(
            "仅支持 MP4/MOV 视频文件或图片文件夹".into(),
        ));
    }
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "mp4" | "mov" => Ok(DroppedInputInfo {
            input_type: ProjectInputType::Video,
        }),
        "jpg" | "jpeg" | "png" => Err(SplatError::Process(
            "请拖入包含 JPG、JPEG 或 PNG 的文件夹，不支持单张图片".into(),
        )),
        _ => Err(SplatError::Process(
            "仅支持 MP4/MOV 视频文件或图片文件夹".into(),
        )),
    }
}

#[tauri::command]
pub async fn classify_dropped_input(path: String) -> Result<DroppedInputInfo> {
    classify_dropped_input_path(Path::new(&path)).await
}

#[derive(Default)]
pub struct PreviewController {
    active: Mutex<Option<GaussianPreviewSession>>,
    lifecycle: Mutex<()>,
    metadata_write: Mutex<()>,
    export: Mutex<()>,
    video_export: Mutex<Option<GaussianVideoExportSession>>,
    html_export: Mutex<Option<html_export::HtmlExportSession>>,
    edit_save: Mutex<Option<GaussianEditSaveSession>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppRuntimeStatus {
    pipeline_running: bool,
    pipeline_project_id: Option<String>,
    pipeline_workspace_task_id: Option<String>,
    pipeline_run_elapsed_ms: u64,
    pipeline_elapsed_offset_ms: u64,
    preview_project_id: Option<String>,
    task_acceleration: Option<ColmapAccelerationStatus>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectTaskLogLine {
    source: String,
    message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectTaskDetail {
    project: ProjectSummary,
    input_type: ProjectInputType,
    source_path: PathBuf,
    projects_root: PathBuf,
    planner_enabled: bool,
    estimated_frames: Option<u64>,
    stage: PipelineStage,
    progress: f64,
    input_images: Option<u64>,
    registered_images: Option<u64>,
    video: Option<VideoInfo>,
    image_sequence: Option<ImageSequenceInfo>,
    source_project_id: Option<String>,
    logs: Vec<ProjectTaskLogLine>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProjectLocation {
    Project,
    Logs,
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
    orientation: VideoOrientation,
    edit_revision: u64,
    transform: GaussianTransform,
    cancel: tokio_util::sync::CancellationToken,
    running: bool,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VideoOrientation {
    #[default]
    Portrait,
    Landscape,
}

impl VideoOrientation {
    fn dimensions(self) -> (u32, u32) {
        match self {
            Self::Portrait => (1080, 1920),
            Self::Landscape => (1920, 1080),
        }
    }
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

pub(crate) fn paths_for_app(app: &tauri::AppHandle) -> EnginePaths {
    EnginePaths::discover(app.path().resource_dir().ok().as_deref())
}

#[tauri::command]
pub async fn check_engines(app: tauri::AppHandle) -> Vec<EngineStatus> {
    paths_for_app(&app).check_all().await
}

#[tauri::command]
pub async fn check_colmap_acceleration(app: tauri::AppHandle) -> ColmapAccelerationStatus {
    refresh_colmap_acceleration(&paths_for_app(&app)).await
}

#[tauri::command]
pub async fn probe_and_plan(
    app: tauri::AppHandle,
    path: String,
    quality: Quality,
    planner_enabled: Option<bool>,
) -> std::result::Result<ProbeAndPlan, SplatError> {
    let engine_paths = paths_for_app(&app);
    let samples = catalog::runtime_samples().await;
    let input = PathBuf::from(path);
    let planner_enabled = planner_enabled.unwrap_or(true);
    if input.is_dir() {
        let image_sequence = tokio::task::spawn_blocking({
            let input = input.clone();
            move || analyze_image_sequence(&input)
        })
        .await
        .map_err(|error| SplatError::Process(format!("图片序列分析任务失败：{error}")))??;
        let plan = create_image_plan(&image_sequence, &quality.preset());
        let acceleration = detect_colmap_acceleration(&engine_paths).await;
        let resolution = planner_enabled.then(|| {
            resolve_planner_resolution_plan(
                quality,
                acceleration.planning_gpu_total_memory_mb(),
                image_sequence.width,
                image_sequence.height,
                false,
            )
        });
        let brush = resolution.map_or_else(
            || {
                resolve_brush_training_preset(
                    quality,
                    false,
                    acceleration.planning_gpu_total_memory_mb(),
                    image_sequence.width.max(image_sequence.height),
                    0,
                )
            },
            |resolution| {
                resolve_brush_training_preset_for_plan(
                    quality,
                    acceleration.planning_gpu_total_memory_mb(),
                    image_sequence.width.max(image_sequence.height),
                    0,
                    &resolution,
                )
            },
        );
        let estimate = estimate_runtime_for_images_with_brush_and_resolution(
            image_sequence.image_count,
            image_sequence.width.max(image_sequence.height),
            &plan,
            quality,
            &samples,
            Some(&brush),
            resolution.map(|plan| plan.policy_version),
            resolution
                .map(|plan| plan.working_long_edge())
                .unwrap_or_else(|| image_sequence.width.max(image_sequence.height)),
        );
        Ok(ProbeAndPlan {
            input_type: ProjectInputType::Images,
            video: None,
            image_sequence: Some(image_sequence),
            plan,
            estimate,
        })
    } else {
        let video =
            probe_video(&engine_paths.ffprobe, &input, None, &ProcessManager::new()).await?;
        let plan = if planner_enabled {
            QualityV2FrameSelection.create_plan(&video, &quality.preset())
        } else {
            UniformRatioFrameSelection.create_plan(&video, &quality.preset())
        };
        let acceleration = detect_colmap_acceleration(&engine_paths).await;
        let (source_width, source_height) = if video.rotation.rem_euclid(180) == 90 {
            (video.height, video.width)
        } else {
            (video.width, video.height)
        };
        let resolution = planner_enabled.then(|| {
            resolve_planner_resolution_plan(
                quality,
                acceleration.planning_gpu_total_memory_mb(),
                source_width,
                source_height,
                true,
            )
        });
        let brush = resolution.map_or_else(
            || {
                resolve_brush_training_preset(
                    quality,
                    false,
                    acceleration.planning_gpu_total_memory_mb(),
                    source_width.max(source_height),
                    0,
                )
            },
            |resolution| {
                resolve_brush_training_preset_for_plan(
                    quality,
                    acceleration.planning_gpu_total_memory_mb(),
                    source_width.max(source_height),
                    0,
                    &resolution,
                )
            },
        );
        let estimate = estimate_runtime_with_brush_and_resolution(
            &video,
            &plan,
            quality,
            &samples,
            Some(&brush),
            resolution.map(|plan| plan.policy_version),
            resolution
                .map(|plan| plan.working_long_edge())
                .unwrap_or_else(|| source_width.max(source_height)),
        );
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

fn resolve_checkpoint_brush(
    state: &PipelineStateFile,
    quality: Quality,
    detected_total_memory_mb: Option<u64>,
    source_long_edge: u32,
    initial_sfm_points: u64,
) -> crate::presets::ResolvedBrushTrainingPreset {
    state.resolution_plan.map_or_else(
        || {
            resolve_brush_training_preset(
                quality,
                state.planner_enabled,
                detected_total_memory_mb,
                source_long_edge,
                initial_sfm_points,
            )
        },
        |resolution| {
            resolve_brush_training_preset_for_plan(
                quality,
                detected_total_memory_mb,
                source_long_edge,
                initial_sfm_points,
                &resolution,
            )
        },
    )
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
        selected_frames: frames.selected_frames.clone(),
        candidate_frames: frames.candidate_frames.clone(),
        rescue_max_frames: frames.rescue_max_frames,
        ..FramePlan::default()
    });
    let samples = catalog::runtime_samples().await;
    let acceleration = detect_colmap_acceleration(&paths_for_app(&app)).await;
    let detected_total_memory_mb = acceleration.planning_gpu_total_memory_mb();
    let mut estimate = match metadata.input_type {
        ProjectInputType::Video => {
            let video = match state.video.clone() {
                Some(video) => video,
                None => {
                    probe_video(
                        &paths_for_app(&app).ffprobe,
                        &metadata.source_path,
                        None,
                        &ProcessManager::new(),
                    )
                    .await?
                }
            };
            let plan = saved_plan.unwrap_or_else(|| {
                UniformRatioFrameSelection.create_plan(&video, &metadata.quality.preset())
            });
            let brush = state.brush_training.resolved.unwrap_or_else(|| {
                resolve_checkpoint_brush(
                    &state,
                    metadata.quality,
                    detected_total_memory_mb,
                    video.width.max(video.height),
                    metadata
                        .output
                        .as_ref()
                        .map(|output| output.points_3d)
                        .unwrap_or(0),
                )
            });
            estimate_runtime_with_brush_and_resolution(
                &video,
                &plan,
                metadata.quality,
                &samples,
                Some(&brush),
                state.resolution_policy_version,
                state
                    .resolution_plan
                    .map(|resolution| resolution.working_long_edge())
                    .unwrap_or_else(|| video.width.max(video.height)),
            )
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
            let brush = state.brush_training.resolved.unwrap_or_else(|| {
                resolve_checkpoint_brush(
                    &state,
                    metadata.quality,
                    detected_total_memory_mb,
                    image_sequence.width.max(image_sequence.height),
                    metadata
                        .output
                        .as_ref()
                        .map(|output| output.points_3d)
                        .unwrap_or(0),
                )
            });
            estimate_runtime_for_images_with_brush_and_resolution(
                image_sequence.image_count,
                image_sequence.width.max(image_sequence.height),
                &plan,
                metadata.quality,
                &samples,
                Some(&brush),
                state.resolution_policy_version,
                state
                    .resolution_plan
                    .map(|resolution| resolution.working_long_edge())
                    .unwrap_or_else(|| image_sequence.width.max(image_sequence.height)),
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
    let tasks = state
        .all()
        .await
        .map_err(|e| SplatError::Process(e.to_string()))?;
    for project in &mut overview.projects {
        if let Some(task) = tasks.iter().find(|t| t.project_id == Some(project.id)) {
            project.status = task.status.project_status();
        }
    }
    if state.active.lock().await.is_none() {
        for project in &mut overview.projects {
            if project.status == ProjectStatus::Running {
                project.status = ProjectStatus::Interrupted;
            }
        }
    }
    Ok(overview)
}

fn persisted_task_stage(state: &PipelineStateFile, status: ProjectStatus) -> PipelineStage {
    match status {
        ProjectStatus::Completed => PipelineStage::Completed,
        _ if state.brush_complete => PipelineStage::Exporting,
        _ if state.reconstruction_complete => PipelineStage::TrainingSplats,
        _ if state.matching_complete => PipelineStage::Reconstructing,
        _ if state.features_complete => PipelineStage::Matching,
        _ if state
            .frames
            .as_ref()
            .and_then(|frames| frames.extracted_frames)
            .is_some_and(|count| count > 0) =>
        {
            PipelineStage::ExtractingFeatures
        }
        _ => PipelineStage::ExtractingFrames,
    }
}

async fn read_project_log_tail(logs_directory: &Path) -> Result<Vec<ProjectTaskLogLine>> {
    const MAX_FILE_BYTES: u64 = 64 * 1024;
    const MAX_TOTAL_BYTES: usize = 256 * 1024;
    const MAX_LINES: usize = 500;
    let mut files = Vec::new();
    if !logs_directory.is_dir() {
        return Ok(Vec::new());
    }
    let mut entries = tokio::fs::read_dir(logs_directory).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if !entry.file_type().await?.is_file()
            || path.extension().and_then(|value| value.to_str()) != Some("log")
        {
            continue;
        }
        let metadata = entry.metadata().await?;
        let modified = metadata.modified().ok();
        files.push((modified, path, metadata.len()));
    }
    files.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    let mut lines = Vec::new();
    let mut total_bytes = 0_usize;
    for (_, path, length) in files {
        let mut file = tokio::fs::File::open(&path).await?;
        let start = length.saturating_sub(MAX_FILE_BYTES);
        file.seek(std::io::SeekFrom::Start(start)).await?;
        let mut bytes = Vec::with_capacity((length - start) as usize);
        file.read_to_end(&mut bytes).await?;
        let text = String::from_utf8_lossy(&bytes);
        let source = path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("system")
            .to_string();
        let mut file_lines = text.lines();
        if start > 0 {
            let _ = file_lines.next();
        }
        for message in file_lines {
            if message.is_empty() {
                continue;
            }
            total_bytes = total_bytes.saturating_add(message.len());
            lines.push(ProjectTaskLogLine {
                source: source.clone(),
                message: message.to_string(),
            });
            while lines.len() > MAX_LINES || total_bytes > MAX_TOTAL_BYTES {
                if let Some(removed) = lines.first() {
                    total_bytes = total_bytes.saturating_sub(removed.message.len());
                }
                lines.remove(0);
            }
        }
    }
    Ok(lines)
}

#[tauri::command]
pub async fn get_project_task_detail(
    controller: State<'_, PipelineController>,
    project_id: String,
) -> Result<ProjectTaskDetail> {
    let id = parse_project_id(&project_id)?;
    let (project_root, metadata, mut project) =
        catalog::load_registered_project_summary(id).await?;
    if let Some(task) = controller
        .all()
        .await
        .map_err(|e| SplatError::Process(e.to_string()))?
        .into_iter()
        .find(|t| t.project_id == Some(id))
    {
        project.status = task.status.project_status();
    }
    let state_bytes = tokio::fs::read(project_root.join("state.json")).await?;
    let state: PipelineStateFile = serde_json::from_slice(&state_bytes)?;
    let progress = if project.status == ProjectStatus::Completed {
        100.0
    } else {
        resume_checkpoint_fraction(&state).0 * 100.0
    };
    let stage = persisted_task_stage(&state, project.status);
    let logs = read_project_log_tail(&project_root.join("logs")).await?;
    let input_images = metadata.output.as_ref().map(|output| output.input_images);
    let registered_images = metadata
        .output
        .as_ref()
        .map(|output| output.registered_images);
    Ok(ProjectTaskDetail {
        project,
        input_type: metadata.input_type,
        source_path: metadata.source_path,
        projects_root: project_root
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default(),
        planner_enabled: state.planner_enabled,
        estimated_frames: state.frames.as_ref().map(|frames| frames.estimated_frames),
        stage,
        progress,
        input_images,
        registered_images,
        video: state.video,
        image_sequence: state.image_sequence,
        source_project_id: metadata
            .reshoot
            .as_ref()
            .map(|reshoot| reshoot.source_project_id.to_string()),
        logs,
    })
}

#[tauri::command]
pub async fn set_projects_root(
    projects_root: String,
) -> std::result::Result<AppSettings, SplatError> {
    catalog::save_projects_root(PathBuf::from(projects_root)).await
}

#[tauri::command]
pub async fn set_planner_enabled(enabled: bool) -> std::result::Result<AppSettings, SplatError> {
    catalog::save_planner_enabled(enabled).await
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

#[cfg(test)]
async fn record_command_failure(
    state: &PipelineController,
    error: PipelineCommandError,
    paths: &[PathBuf],
) -> PipelineCommandError {
    state.capture_failure(error, paths).await
}

fn service_command_error(error: crate::tasks::ServiceError) -> PipelineCommandError {
    let mut command = PipelineCommandError::from(SplatError::Process(error.to_string()));
    if error.code == "TASK_BUSY" {
        command.code = "TASK_BUSY";
    }
    command
}

async fn wait_task_result(
    state: &PipelineController,
    id: Uuid,
) -> std::result::Result<PipelineResult, PipelineCommandError> {
    let task = state.wait(id).await.map_err(service_command_error)?;
    if let Some(result) = task.result {
        return serde_json::from_value(result)
            .map_err(|e| PipelineCommandError::from(SplatError::Json(e)));
    }
    if let Some(error) = task.error {
        return Err(PipelineCommandError {
            code: if task.status == crate::tasks::TaskStatus::Cancelled {
                "cancelled"
            } else {
                "pipeline_failed"
            },
            message: error.message,
            failed_stage: error.failed_stage,
            engine: error.engine,
            failure_kind: None,
            project_id: task.project_id.map(|v| v.to_string()),
            project_path: task.project_path.map(Box::new),
            logs_directory: None,
            failure_id: error.failure_id.map(Box::new),
        });
    }
    Err(PipelineCommandError::from(SplatError::Process(
        "任务未产生结果".into(),
    )))
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn create_gui_task(
    state: State<'_, PipelineController>,
    path: String,
    quality: Quality,
    projects_root: String,
    planner_enabled: Option<bool>,
    workspace_task_id: Option<String>,
) -> std::result::Result<crate::tasks::TaskRecord, crate::tasks::ServiceError> {
    let id = workspace_task_id
        .map(|v| Uuid::parse_str(&v))
        .transpose()
        .map_err(|_| crate::tasks::ServiceError::new("INVALID_ARGUMENT", "工作区任务 ID 无效"))?;
    state
        .create(
            id,
            path.into(),
            quality,
            projects_root.into(),
            planner_enabled.unwrap_or(true),
            "gui",
            None,
            None,
        )
        .await
}
#[tauri::command]
pub async fn start_gui_task(
    app: tauri::AppHandle,
    state: State<'_, PipelineController>,
    telemetry: State<'_, TelemetryService>,
    task_id: Uuid,
) -> std::result::Result<crate::tasks::StartReceipt, crate::tasks::ServiceError> {
    state
        .start(
            task_id,
            paths_for_app(&app),
            Some(telemetry.inner().clone()),
        )
        .await
}
#[tauri::command]
pub async fn get_shared_tasks(
    state: State<'_, PipelineController>,
) -> std::result::Result<Vec<crate::tasks::TaskRecord>, crate::tasks::ServiceError> {
    state.all().await
}
#[tauri::command]
pub async fn read_shared_task_logs(
    state: State<'_, PipelineController>,
    request: crate::tasks::logs::LogRequest,
) -> std::result::Result<crate::tasks::logs::LogPage, crate::tasks::ServiceError> {
    let task = state.get(request.task_id).await?;
    tokio::task::spawn_blocking(move || crate::tasks::logs::read(&task, &request))
        .await
        .map_err(|_| crate::tasks::ServiceError::new("LOG_READ_FAILED", "日志读取异常"))?
}
#[tauri::command]
pub async fn get_shared_task(
    state: State<'_, PipelineController>,
    task_id: Uuid,
) -> std::result::Result<crate::tasks::TaskRecord, crate::tasks::ServiceError> {
    state.get(task_id).await
}
#[tauri::command]
pub async fn cancel_shared_task(
    state: State<'_, PipelineController>,
    task_id: Uuid,
    run_id: Uuid,
) -> std::result::Result<crate::tasks::TaskRecord, crate::tasks::ServiceError> {
    state.cancel(task_id, run_id).await
}
#[tauri::command]
pub async fn resume_gui_task(
    app: tauri::AppHandle,
    state: State<'_, PipelineController>,
    telemetry: State<'_, TelemetryService>,
    project_id: Uuid,
) -> std::result::Result<crate::tasks::StartReceipt, crate::tasks::ServiceError> {
    state
        .resume(
            project_id,
            paths_for_app(&app),
            Some(telemetry.inner().clone()),
        )
        .await
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn start_pipeline(
    app: tauri::AppHandle,
    state: State<'_, PipelineController>,
    telemetry: State<'_, TelemetryService>,
    path: String,
    quality: Quality,
    projects_root: String,
    planner_enabled: Option<bool>,
    workspace_task_id: Option<String>,
) -> std::result::Result<PipelineResult, PipelineCommandError> {
    let id = workspace_task_id
        .map(|v| Uuid::parse_str(&v))
        .transpose()
        .map_err(|_| {
            PipelineCommandError::from(SplatError::Process("工作区任务 ID 无效".into()))
        })?;
    let task = state
        .create(
            id,
            path.into(),
            quality,
            projects_root.into(),
            planner_enabled.unwrap_or(true),
            "gui",
            None,
            None,
        )
        .await
        .map_err(service_command_error)?;
    state
        .start(
            task.task_id,
            paths_for_app(&app),
            Some(telemetry.inner().clone()),
        )
        .await
        .map_err(service_command_error)?;
    wait_task_result(state.inner(), task.task_id).await
}
#[tauri::command]
pub async fn resume_pipeline(
    app: tauri::AppHandle,
    state: State<'_, PipelineController>,
    telemetry: State<'_, TelemetryService>,
    project_id: String,
) -> std::result::Result<PipelineResult, PipelineCommandError> {
    let id = parse_project_id(&project_id)?;
    let receipt = state
        .resume(id, paths_for_app(&app), Some(telemetry.inner().clone()))
        .await
        .map_err(service_command_error)?;
    wait_task_result(state.inner(), receipt.task_id).await
}
#[tauri::command]
pub async fn cancel_pipeline(state: State<'_, PipelineController>) -> Result<()> {
    if let Some(task) = state
        .running()
        .await
        .map_err(|e| SplatError::Process(e.to_string()))?
    {
        if let Some(run) = task.run_id {
            state
                .cancel(task.task_id, run)
                .await
                .map_err(|e| SplatError::Process(e.to_string()))?;
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn get_app_runtime_status(
    pipeline: State<'_, PipelineController>,
    preview: State<'_, PreviewController>,
) -> Result<AppRuntimeStatus> {
    let active_pipeline = pipeline.active.lock().await;
    let pipeline_running = active_pipeline.is_some();
    let pipeline_identity = active_pipeline
        .as_ref()
        .and_then(|runner| runner.current_project_identity());
    let pipeline_run_elapsed_ms = active_pipeline
        .as_ref()
        .map(|runner| runner.elapsed_ms())
        .unwrap_or(0);
    let pipeline_elapsed_offset_ms = active_pipeline
        .as_ref()
        .map(|runner| runner.elapsed_offset_ms())
        .unwrap_or(0);
    let task_acceleration = active_pipeline
        .as_ref()
        .and_then(|runner| runner.current_acceleration());
    drop(active_pipeline);
    let active_task = pipeline
        .running()
        .await
        .map_err(|e| SplatError::Process(e.to_string()))?;
    let preview_project_id = preview
        .active
        .lock()
        .await
        .as_ref()
        .map(|session| session.project_id.to_string());
    Ok(AppRuntimeStatus {
        pipeline_running,
        pipeline_project_id: pipeline_identity.map(|value| value.0.to_string()),
        pipeline_workspace_task_id: active_task.map(|task| task.task_id.to_string()),
        pipeline_run_elapsed_ms,
        pipeline_elapsed_offset_ms,
        preview_project_id,
        task_acceleration,
    })
}

#[tauri::command]
pub async fn open_project_location(
    app: tauri::AppHandle,
    project_id: String,
    location: ProjectLocation,
) -> Result<()> {
    let id = parse_project_id(&project_id)?;
    let (project, _) = catalog::load_registered_project(id).await?;
    let target = match location {
        ProjectLocation::Project => project,
        ProjectLocation::Logs => project.join("logs"),
    };
    if !target.is_dir() {
        return Err(SplatError::InvalidPath(target));
    }
    app.opener()
        .open_path(target.to_string_lossy().into_owned(), None::<String>)
        .map_err(|error| SplatError::Process(format!("无法打开文件夹：{error}")))
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
    let _lifecycle = preview.lifecycle.lock().await;
    if preview
        .html_export
        .lock()
        .await
        .as_ref()
        .is_some_and(|session| session.project_id == id)
        || preview
            .video_export
            .lock()
            .await
            .as_ref()
            .is_some_and(|session| session.project_id == id)
    {
        return Err(SplatError::Process("请先完成或取消导出再删除项目".into()));
    }
    let session = {
        let mut active = preview.active.lock().await;
        if active
            .as_ref()
            .is_some_and(|session| session.project_id == id)
        {
            active.take()
        } else {
            None
        }
    };
    if let Some(session) = session {
        discard_preview_asset(&app, session).await;
    }

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
    let _lifecycle = state.lifecycle.lock().await;
    if state.html_export.lock().await.is_some() || state.video_export.lock().await.is_some() {
        return Err(SplatError::Process(
            "导出期间不能重新加载或切换预览项目".into(),
        ));
    }
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

    let previous = state.active.lock().await.take();
    if let Some(previous) = previous {
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
    *state.active.lock().await = Some(GaussianPreviewSession {
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
    let _lifecycle = state.lifecycle.lock().await;
    let id = parse_project_id(&project_id)?;
    let mut html_export = state.html_export.lock().await;
    if let Some(session) = html_export
        .as_ref()
        .filter(|session| session.project_id == id)
    {
        session.cancel.cancel();
        if !session.running {
            *html_export = None;
        }
    }
    drop(html_export);
    let session = {
        let mut active = state.active.lock().await;
        if active
            .as_ref()
            .is_some_and(|session| session.project_id == id)
        {
            active.take()
        } else {
            None
        }
    };
    if let Some(session) = session {
        discard_preview_asset(&app, session).await;
    }

    let mut edit_save = state.edit_save.lock().await;
    if edit_save
        .as_ref()
        .is_some_and(|session| session.project_id == id)
    {
        *edit_save = None;
    }
    drop(edit_save);

    let mut video_export = state.video_export.lock().await;
    if let Some(session) = video_export
        .as_ref()
        .filter(|session| session.project_id == id)
    {
        session.cancel.cancel();
        if !session.running {
            *video_export = None;
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

const GAUSSIAN_VIDEO_FPS: u32 = 30;
const GAUSSIAN_VIDEO_DURATION_MS: u64 = 23_000;
const MAX_GAUSSIAN_VIDEO_BYTES: usize = 1024 * 1024 * 1024;

#[cfg(test)]
fn next_gaussian_video_path(root: &Path) -> PathBuf {
    next_gaussian_video_path_for_orientation(root, VideoOrientation::Portrait)
}

fn next_gaussian_video_path_for_orientation(root: &Path, orientation: VideoOrientation) -> PathBuf {
    let stem = match orientation {
        VideoOrientation::Portrait => "preview",
        VideoOrientation::Landscape => "preview-landscape",
    };
    for number in 1_u32.. {
        let name = if number == 1 {
            format!("{stem}.mp4")
        } else {
            format!("{stem}-{number}.mp4")
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
    orientation: Option<VideoOrientation>,
    edit_revision: Option<u64>,
) -> Result<GaussianVideoExportReservation> {
    let _lifecycle = state.lifecycle.lock().await;
    let project_id = parse_project_id(&project_id)?;
    let (root, _, metadata) = catalog::registered_final_ply_for_project(project_id).await?;
    if edit_revision.is_some_and(|revision| revision != metadata.editing.revision) {
        return Err(SplatError::Process(
            "编辑状态尚未保存完成，请重试导出".into(),
        ));
    }

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

    if state.html_export.lock().await.is_some() {
        return Err(SplatError::Process(
            "已有 HTML 正在导出，请等待或取消。".into(),
        ));
    }

    let mut video_export = state.video_export.lock().await;
    if video_export.is_some() {
        return Err(SplatError::Process(
            "已有一个高斯预览视频正在导出，请等待其完成或先取消。".into(),
        ));
    }

    let export_id = Uuid::new_v4();
    let orientation = orientation.unwrap_or_default();
    let destination = next_gaussian_video_path_for_orientation(&root, orientation);
    let temporary = root.join(format!(".ooosplat-preview-{export_id}.mp4.tmp"));
    *video_export = Some(GaussianVideoExportSession {
        export_id,
        project_id,
        destination: destination.clone(),
        temporary,
        orientation,
        edit_revision: metadata.editing.revision,
        transform: metadata.transform,
        cancel: tokio_util::sync::CancellationToken::new(),
        running: false,
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
    if session.cancel.is_cancelled() {
        return Err(SplatError::Cancelled);
    }
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
        for chunk in bytes.chunks(1024 * 1024) {
            if session.cancel.is_cancelled() {
                return Err(SplatError::Cancelled);
            }
            file.write_all(chunk).await?;
        }
        file.flush().await?;
        file.sync_all().await?;
        drop(file);
        if session.cancel.is_cancelled() {
            return Err(SplatError::Cancelled);
        }
        let temporary = session.temporary.clone();
        let destination = session.destination.clone();
        tokio::task::spawn_blocking(move || {
            html_export::publish_html_file(&temporary, &destination)
        })
        .await
        .map_err(|error| SplatError::Process(format!("视频发布失败：{error}")))??;
        let _ = tokio::fs::remove_file(&session.temporary).await;
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
        width: session.orientation.dimensions().0,
        height: session.orientation.dimensions().1,
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
        .as_mut()
        .filter(|session| session.export_id == export_id && !session.running)
        .ok_or_else(|| SplatError::Process("视频导出会话不存在或已经结束。".into()))?;
    session.running = true;
    let session = session.clone();
    drop(video_export);
    let _metadata_guard = state.metadata_write.lock().await;
    let result = async {
        let (_, _, metadata) =
            catalog::registered_final_ply_for_project(session.project_id).await?;
        if metadata.editing.revision != session.edit_revision
            || metadata.transform != session.transform
        {
            return Err(SplatError::Process(
                "导出期间编辑状态发生变化，请重试".into(),
            ));
        }
        write_gaussian_video(&session, bytes).await
    }
    .await;
    let mut video_export = state.video_export.lock().await;
    if video_export
        .as_ref()
        .is_some_and(|session| session.export_id == export_id)
    {
        *video_export = None;
    }
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
    if let Some(session) = video_export
        .as_ref()
        .filter(|session| session.export_id == export_id)
    {
        session.cancel.cancel();
        if !session.running {
            *video_export = None;
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
        classify_dropped_input_path, classify_pipeline_failure, contains_mp4_ftyp,
        create_preview_asset, next_gaussian_video_path, persisted_task_stage, preview_client_path,
        read_project_log_tail, write_gaussian_video, GaussianVideoExportSession,
        PipelineCommandError, VideoOrientation,
    };
    use crate::error::SplatError;
    use crate::pipeline::{PipelineEngine, PipelineStage};
    use crate::presets::Quality;
    use crate::project::{PipelineStateFile, ProjectStatus};
    use std::{fs, path::Path};
    use tempfile::tempdir;
    use uuid::Uuid;

    #[tokio::test]
    async fn dropped_input_classification_accepts_video_and_directories() {
        let root = tempdir().unwrap();
        let images = root.path().join("中文 图片");
        fs::create_dir(&images).unwrap();
        let video = root.path().join("clip.MOV");
        fs::write(&video, b"video").unwrap();
        assert_eq!(
            classify_dropped_input_path(&images)
                .await
                .unwrap()
                .input_type,
            crate::project::ProjectInputType::Images
        );
        assert_eq!(
            classify_dropped_input_path(&video)
                .await
                .unwrap()
                .input_type,
            crate::project::ProjectInputType::Video
        );
    }

    #[tokio::test]
    async fn dropped_input_classification_rejects_single_images_unknown_and_missing_paths() {
        let root = tempdir().unwrap();
        for name in ["single.jpeg", "notes.txt"] {
            let path = root.path().join(name);
            fs::write(&path, b"x").unwrap();
            assert!(classify_dropped_input_path(&path).await.is_err());
        }
        assert!(
            classify_dropped_input_path(&root.path().join("missing.mp4"))
                .await
                .is_err()
        );
    }

    #[test]
    fn pipeline_errors_have_stable_machine_readable_codes() {
        let cancelled =
            serde_json::to_value(PipelineCommandError::from(SplatError::Cancelled)).unwrap();
        assert_eq!(cancelled["code"], "cancelled");
        let failed = serde_json::to_value(PipelineCommandError::from(SplatError::Process(
            "boom".into(),
        )))
        .unwrap();
        assert_eq!(failed["code"], "pipeline_failed");
        assert!(failed["message"].as_str().unwrap().contains("boom"));
    }

    #[test]
    fn pipeline_command_error_stays_below_clippy_large_error_threshold() {
        assert!(std::mem::size_of::<PipelineCommandError>() <= 128);
    }

    #[test]
    fn persisted_task_stage_preserves_terminal_status_and_checkpoint_position() {
        let mut state = PipelineStateFile::created(Quality::Balanced);
        state.features_complete = true;
        state.matching_complete = true;
        assert_eq!(
            persisted_task_stage(&state, ProjectStatus::Interrupted),
            PipelineStage::Reconstructing
        );
        assert_eq!(
            persisted_task_stage(&state, ProjectStatus::Failed),
            PipelineStage::Reconstructing
        );
        assert_eq!(
            persisted_task_stage(&state, ProjectStatus::Cancelled),
            PipelineStage::Reconstructing
        );
    }

    #[tokio::test]
    async fn historical_log_tail_is_bounded_to_the_latest_five_hundred_lines() {
        let root = tempdir().expect("temporary log directory");
        let logs = root.path().join("logs");
        fs::create_dir_all(&logs).unwrap();
        let content = (0..620)
            .map(|index| format!("line-{index:04}"))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(logs.join("brush.log"), content).unwrap();

        let lines = read_project_log_tail(&logs).await.unwrap();
        assert_eq!(lines.len(), 500);
        assert_eq!(lines.first().unwrap().message, "line-0120");
        assert_eq!(lines.last().unwrap().message, "line-0619");
        assert!(lines.iter().all(|line| line.source == "brush"));
    }

    #[tokio::test]
    async fn failure_snapshots_cover_generic_errors_but_never_cancelled_tasks() {
        let controller = super::PipelineController::default();
        let cancelled =
            super::record_command_failure(&controller, SplatError::Cancelled.into(), &[]).await;
        assert!(cancelled.failure_id.is_none());
        let failed = super::record_command_failure(
            &controller,
            SplatError::Process("image import failed".into()).into(),
            &[],
        )
        .await;
        assert!(failed.failure_id.is_some());
        assert_eq!(failed.code, "pipeline_failed");
        for (stage, engine) in [
            (PipelineStage::ProbingVideo, PipelineEngine::Ffmpeg),
            (PipelineStage::ExtractingFrames, PipelineEngine::Ffmpeg),
            (PipelineStage::ExtractingFeatures, PipelineEngine::Colmap),
            (PipelineStage::Matching, PipelineEngine::Colmap),
            (
                PipelineStage::ValidatingReconstruction,
                PipelineEngine::Colmap,
            ),
            (PipelineStage::Exporting, PipelineEngine::System),
        ] {
            assert_eq!(
                classify_pipeline_failure(Some(stage), "failure").0,
                Some(engine)
            );
        }
    }

    #[test]
    fn classifies_mapper_and_brush_failures_for_plain_language_guidance() {
        assert_eq!(
            classify_pipeline_failure(
                Some(PipelineStage::Reconstructing),
                "Could not find a good initial image pair",
            ),
            (Some(PipelineEngine::Colmap), Some("mapper_source")),
        );
        assert_eq!(
            classify_pipeline_failure(
                Some(PipelineStage::TrainingSplats),
                "IO error while loading dataset: early eof",
            ),
            (Some(PipelineEngine::Brush), Some("brush_dataset")),
        );
        assert_eq!(
            classify_pipeline_failure(
                Some(PipelineStage::TrainingSplats),
                "Device lost while allocating a buffer",
            ),
            (Some(PipelineEngine::Brush), Some("brush_device_lost")),
        );

        let real_mapper_log = r#"
            Loading database
            Loading cameras...
            Loading matches...
            Finding good initial image pair
            No good initial image pair found.
            Discarding reconstruction because it is too small
            Failed to create any sparse model
        "#;
        assert_eq!(
            classify_pipeline_failure(Some(PipelineStage::Reconstructing), real_mapper_log),
            (Some(PipelineEngine::Colmap), Some("mapper_source")),
        );
        assert_eq!(
            classify_pipeline_failure(
                Some(PipelineStage::Reconstructing),
                "SQLite error: database is locked",
            ),
            (Some(PipelineEngine::Colmap), Some("mapper_storage")),
        );
        assert_eq!(
            classify_pipeline_failure(
                Some(PipelineStage::Reconstructing),
                "COLMAP did not produce a usable sparse model: file not found",
            ),
            (Some(PipelineEngine::Colmap), Some("mapper_source")),
        );
        assert_eq!(
            classify_pipeline_failure(
                Some(PipelineStage::ValidatingReconstruction),
                "稀疏重建没有可用的注册图像或三维点",
            ),
            (Some(PipelineEngine::Colmap), Some("mapper_source")),
        );
        assert_eq!(
            classify_pipeline_failure(
                Some(PipelineStage::Reconstructing),
                "I/O error while reading feature descriptors",
            ),
            (Some(PipelineEngine::Colmap), Some("mapper_source")),
        );
        assert_eq!(
            classify_pipeline_failure(
                Some(PipelineStage::Reconstructing),
                "I/O error while opening SQLite database",
            ),
            (Some(PipelineEngine::Colmap), Some("mapper_storage")),
        );
    }

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
            orientation: VideoOrientation::Portrait,
            edit_revision: 0,
            transform: crate::project::GaussianTransform::default(),
            cancel: tokio_util::sync::CancellationToken::new(),
            running: false,
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
    async fn landscape_video_uses_its_own_numbering_and_real_dimensions() {
        let root = tempdir().unwrap();
        let first = super::next_gaussian_video_path_for_orientation(
            root.path(),
            VideoOrientation::Landscape,
        );
        assert_eq!(first.file_name().unwrap(), "preview-landscape.mp4");
        let session = GaussianVideoExportSession {
            export_id: Uuid::new_v4(),
            project_id: Uuid::new_v4(),
            destination: first.clone(),
            temporary: root.path().join(".landscape.tmp"),
            orientation: VideoOrientation::Landscape,
            edit_revision: 0,
            transform: crate::project::GaussianTransform::default(),
            cancel: tokio_util::sync::CancellationToken::new(),
            running: false,
        };
        let result = write_gaussian_video(&session, b"\0\0\0\x18ftypisom\0\0\0\0payload")
            .await
            .unwrap();
        assert_eq!(
            (result.width, result.height, result.fps, result.duration_ms),
            (1920, 1080, 30, 23000)
        );
        assert_eq!(
            super::next_gaussian_video_path_for_orientation(
                root.path(),
                VideoOrientation::Landscape
            )
            .file_name()
            .unwrap(),
            "preview-landscape-2.mp4"
        );
        assert_eq!(
            next_gaussian_video_path(root.path()).file_name().unwrap(),
            "preview.mp4"
        );
    }

    #[tokio::test]
    async fn invalid_mp4_is_rejected_without_leaving_a_temporary_file() {
        let root = tempdir().expect("temporary project");
        let session = GaussianVideoExportSession {
            export_id: Uuid::new_v4(),
            project_id: Uuid::new_v4(),
            destination: root.path().join("preview.mp4"),
            temporary: root.path().join(".preview.tmp"),
            orientation: VideoOrientation::Portrait,
            edit_revision: 0,
            transform: crate::project::GaussianTransform::default(),
            cancel: tokio_util::sync::CancellationToken::new(),
            running: false,
        };

        write_gaussian_video(&session, b"not-an-mp4")
            .await
            .expect_err("invalid MP4 must be rejected");
        assert!(!session.destination.exists());
        assert!(!session.temporary.exists());
        session.cancel.cancel();
        assert!(matches!(
            write_gaussian_video(&session, b"\0\0\0\x18ftypisom\0\0\0\0payload").await,
            Err(SplatError::Cancelled)
        ));
        assert!(!session.destination.exists());
        assert!(!session.temporary.exists());
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IncrementalReshootRequest {
    source_project_id: String,
    reshoot_path: String,
    input_type: ReshootInputType,
    projects_root: String,
    workspace_task_id: Option<String>,
}

#[tauri::command]
pub async fn inspect_reshoot_source(
    app: tauri::AppHandle,
    project_id: String,
) -> std::result::Result<ReshootSourceInfo, SplatError> {
    let project_id =
        Uuid::parse_str(&project_id).map_err(|_| SplatError::Process("原项目 ID 无效".into()))?;
    PipelineRunner::new(paths_for_app(&app), |_| {})
        .inspect_reshoot_source(project_id)
        .await
}

#[tauri::command]
pub async fn probe_reshoot_input(
    app: tauri::AppHandle,
    project_id: String,
    path: String,
    input_type: ReshootInputType,
) -> std::result::Result<ReshootInputInfo, SplatError> {
    let project_id =
        Uuid::parse_str(&project_id).map_err(|_| SplatError::Process("原项目 ID 无效".into()))?;
    PipelineRunner::new(paths_for_app(&app), |_| {})
        .probe_reshoot_input(project_id, Path::new(&path), input_type)
        .await
}

#[tauri::command]
pub async fn start_gui_reshoot(
    app: tauri::AppHandle,
    state: State<'_, PipelineController>,
    telemetry: State<'_, TelemetryService>,
    request: IncrementalReshootRequest,
) -> std::result::Result<crate::tasks::StartReceipt, crate::tasks::ServiceError> {
    let source = Uuid::parse_str(&request.source_project_id)
        .map_err(|_| crate::tasks::ServiceError::new("INVALID_ARGUMENT", "原项目 ID 无效"))?;
    let (project, metadata) = catalog::load_registered_project(source).await?;
    if (request.input_type == ReshootInputType::Images) != Path::new(&request.reshoot_path).is_dir()
    {
        return Err(crate::tasks::ServiceError::new(
            "INVALID_INPUT",
            "补拍输入类型与路径不一致",
        ));
    }
    let checkpoint: PipelineStateFile =
        serde_json::from_slice(&tokio::fs::read(project.join("state.json")).await?)?;
    let id = request
        .workspace_task_id
        .map(|v| Uuid::parse_str(&v))
        .transpose()
        .map_err(|_| crate::tasks::ServiceError::new("INVALID_ARGUMENT", "工作区任务 ID 无效"))?;
    let task = state
        .create(
            id,
            request.reshoot_path.into(),
            metadata.quality,
            request.projects_root.into(),
            checkpoint.planner_enabled,
            "gui",
            None,
            None,
        )
        .await?;
    state.mark_reshoot(task.task_id, source).await?;
    state
        .start(
            task.task_id,
            paths_for_app(&app),
            Some(telemetry.inner().clone()),
        )
        .await
}
#[tauri::command]
pub async fn start_incremental_reshoot_pipeline(
    app: tauri::AppHandle,
    state: State<'_, PipelineController>,
    telemetry: State<'_, TelemetryService>,
    request: IncrementalReshootRequest,
) -> std::result::Result<PipelineResult, PipelineCommandError> {
    let receipt = start_gui_reshoot(app, state.clone(), telemetry, request)
        .await
        .map_err(service_command_error)?;
    wait_task_result(state.inner(), receipt.task_id).await
}
