use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};

use serde::Serialize;
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
    pipeline::runner::{PipelineResult, PipelineRunner},
    presets::Quality,
    project::{
        catalog::{self, AppSettings, ProjectOverview},
        manager::atomic_write_json,
        GaussianTransform, ProjectStatus,
    },
    reconstruction::{ply::inspect_gaussian_ply, splat_transform::export_transformed_ply},
    telemetry::{PipelineTelemetrySession, TelemetryPreferences, TelemetryService},
    video::{create_image_plan, list_images, FramePlan, FrameSelectionStrategy, UniformRatioFrameSelection, VideoInfo},
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
}

#[derive(Debug, Clone)]
struct GaussianPreviewSession {
    project_id: Uuid,
    asset_path: PathBuf,
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
    video: Option<VideoInfo>,
    plan: FramePlan,
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
    let input = PathBuf::from(&path);
    if input.is_dir() {
        let images = list_images(&input)?;
        if images.is_empty() {
            return Err(SplatError::InvalidVideo(
                "图片序列为空，未找到支持的图片文件".into(),
            ));
        }
        let plan = create_image_plan(images.len() as u64, &quality.preset());
        return Ok(ProbeAndPlan { video: None, plan });
    }
    let video = probe_video(&paths_for_app(&app).ffprobe, &input, None).await?;
    let plan = UniformRatioFrameSelection.create_plan(&video, &quality.preset());
    Ok(ProbeAndPlan {
        video: Some(video),
        plan,
    })
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
    let _ = app.asset_protocol_scope().forbid_file(&session.asset_path);
    let parent = session.asset_path.parent().map(Path::to_path_buf);
    let _ = tokio::fs::remove_file(&session.asset_path).await;
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
    *active = Some(GaussianPreviewSession {
        project_id: id,
        asset_path: asset_path.clone(),
    });

    Ok(GaussianPreviewDescriptor {
        project_id: id,
        model_path: preview_client_path(&path),
        asset_path: preview_client_path(&asset_path),
        format: "ply",
        file_size: info.file_size,
        splat_count: info.splat_count,
        transform,
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
) -> Result<GaussianExportResult> {
    let id = parse_project_id(&project_id)?;
    let transform = transform.validate()?;
    let _export_guard = state.export.lock().await;
    let (root, source, _) = catalog::registered_final_ply_for_project(id).await?;
    let emitter = app.clone();
    let (path, info) = tokio::task::spawn_blocking(move || {
        export_transformed_ply(&source, &root, transform, |processed, total| {
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
        })
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
    if !active
        .as_ref()
        .is_some_and(|session| session.project_id == project_id)
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
