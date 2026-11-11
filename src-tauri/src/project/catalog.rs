use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::{
    error::{Result, SplatError},
    pipeline::{
        estimate::{RuntimeInputKind, RuntimeSample},
        PipelineStage,
    },
    project::{
        manager::atomic_write_json, PipelineStateFile, ProjectMetadata, ProjectStatus,
        PROJECT_APP_ID,
    },
    reconstruction::ply::inspect_gaussian_ply,
};

static PROJECT_INDEX_LOCK: Mutex<()> = Mutex::const_new(());

pub async fn runtime_samples() -> Vec<RuntimeSample> {
    let Ok(index) = load_index().await else {
        return Vec::new();
    };
    let mut samples = Vec::new();
    for item in index.projects.into_iter().rev() {
        let Ok(metadata_bytes) = tokio::fs::read(item.path.join("project.json")).await else {
            continue;
        };
        let Ok(metadata) = serde_json::from_slice::<ProjectMetadata>(&metadata_bytes) else {
            continue;
        };
        let Some(duration_ms) = metadata
            .duration_ms
            .filter(|_| metadata.status == ProjectStatus::Completed)
        else {
            continue;
        };
        let state_bytes = tokio::fs::read(item.path.join("state.json")).await.ok();
        let state = state_bytes
            .as_deref()
            .and_then(|bytes| serde_json::from_slice::<PipelineStateFile>(bytes).ok());
        let brush = state
            .as_ref()
            .and_then(|state| state.brush_training.resolved);
        let Some(extracted_frames) = runtime_sample_frame_count(
            state_bytes.as_deref(),
            metadata.output.as_ref().map(|output| output.input_images),
        ) else {
            continue;
        };
        samples.push(RuntimeSample {
            quality: metadata.quality,
            input_kind: match metadata.input_type {
                crate::project::ProjectInputType::Video => RuntimeInputKind::Video,
                crate::project::ProjectInputType::Images => RuntimeInputKind::Images,
            },
            source_long_edge: state
                .as_ref()
                .and_then(|state| {
                    state
                        .video
                        .as_ref()
                        .map(|video| video.width.max(video.height))
                        .or_else(|| {
                            state
                                .image_sequence
                                .as_ref()
                                .map(|images| images.width.max(images.height))
                        })
                })
                .unwrap_or(1),
            working_long_edge: state
                .as_ref()
                .and_then(|state| state.resolution_plan)
                .map(|plan| plan.working_long_edge()),
            resolution_policy_version: state
                .as_ref()
                .and_then(|state| state.resolution_policy_version),
            extracted_frames,
            duration_ms,
            brush,
        });
        if samples.len() == 20 {
            break;
        }
    }
    samples
}

fn runtime_sample_frame_count(
    state_bytes: Option<&[u8]>,
    output_frames: Option<u64>,
) -> Option<u64> {
    state_bytes
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(bytes).ok())
        .and_then(|state| {
            state
                .pointer("/frames/extractedFrames")
                .and_then(|value| value.as_u64())
        })
        .or(output_frames)
        .filter(|count| *count > 0)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    #[serde(default)]
    pub mcp: crate::mcp::McpSettings,
    pub schema_version: u32,
    pub projects_root: PathBuf,
    #[serde(default = "default_planner_enabled")]
    pub planner_enabled: bool,
}

const fn default_planner_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IndexedProject {
    id: Uuid,
    path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectIndex {
    schema_version: u32,
    projects: Vec<IndexedProject>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSummary {
    pub id: Uuid,
    pub workspace_task_id: Option<Uuid>,
    pub task_kind: &'static str,
    pub name: String,
    pub status: ProjectStatus,
    pub project_path: PathBuf,
    pub final_ply: Option<PathBuf>,
    pub file_size: Option<u64>,
    pub splat_count: Option<u64>,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub duration_ms: Option<u64>,
    pub quality: Option<crate::presets::Quality>,
    pub source_name: String,
    pub model_format: Option<crate::project::ImportedGaussianFormat>,
    pub source_available: Option<bool>,
    pub reshoot_eligible: Option<bool>,
    pub reshoot_reason: Option<String>,
    pub registered_ratio: Option<f64>,
    pub points_3d: Option<u64>,
    pub failure_message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectOverview {
    pub projects_root: PathBuf,
    pub planner_enabled: bool,
    pub projects: Vec<ProjectSummary>,
}

struct CompletionSnapshot {
    creation_ready: bool,
    inputs_reshoot_eligible: Option<bool>,
    path: Option<PathBuf>,
    info: Option<crate::reconstruction::ply::PlyInfo>,
    marked_complete: bool,
}

fn imported_source_matches(metadata: &ProjectMetadata) -> bool {
    let Some(imported) = &metadata.imported else {
        return true;
    };
    let Ok(file) = imported.source_model_path.metadata() else {
        return false;
    };
    if imported.canonical_model_path.is_some() {
        return true;
    }
    let modified_ms = file
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|value| value.as_millis() as u64);
    file.len() == imported.source_fingerprint.file_size
        && modified_ms == imported.source_fingerprint.modified_ms
}

async fn completion_snapshot(root: &Path, metadata: &ProjectMetadata) -> CompletionSnapshot {
    let path = project_model_path(root, metadata)
        .filter(|path| path.is_file())
        .filter(|_| match metadata.imported.as_ref() {
            Some(imported) if imported.canonical_model_path.is_none() => {
                imported_source_matches(metadata)
            }
            _ => true,
        });
    let state = tokio::fs::read(root.join("state.json"))
        .await
        .ok()
        .and_then(|bytes| serde_json::from_slice::<crate::project::PipelineStateFile>(&bytes).ok());
    let state_completed = state
        .as_ref()
        .is_some_and(|state| state.stage == PipelineStage::Completed);
    let inputs_reshoot_eligible = state
        .as_ref()
        .and_then(|s| s.inputs.as_ref())
        .map(|i| i.reshoot_eligible);
    let creation_ready = state
        .as_ref()
        .and_then(|s| s.creation.as_ref())
        .is_none_or(|c| c.ready);
    let marked_complete = metadata.status == ProjectStatus::Completed || state_completed;
    let info = if let Some(path) = path.clone().filter(|_| creation_ready) {
        tokio::task::spawn_blocking(move || inspect_gaussian_ply(&path).ok())
            .await
            .ok()
            .flatten()
    } else {
        None
    };
    CompletionSnapshot {
        creation_ready,
        inputs_reshoot_eligible,
        path,
        info,
        marked_complete,
    }
}

pub fn project_model_path(root: &Path, metadata: &ProjectMetadata) -> Option<PathBuf> {
    if let Some(imported) = &metadata.imported {
        return imported
            .canonical_model_path
            .as_ref()
            .map(|path| root.join(path))
            .or_else(|| Some(imported.source_model_path.clone()));
    }
    let direct = root.join("final.ply");
    let legacy = root.join("output").join("final.ply");
    direct
        .is_file()
        .then_some(direct)
        .or_else(|| legacy.is_file().then_some(legacy))
}

pub(crate) async fn project_is_durably_completed(root: &Path, metadata: &ProjectMetadata) -> bool {
    let transaction = root
        .join("work")
        .join(crate::pipeline::publication::TRANSACTION_FILE_NAME);
    if transaction.is_file() {
        return crate::pipeline::publication::committed_publication(root, &transaction)
            .await
            .is_ok_and(|publication| publication.is_some());
    }
    let snapshot = completion_snapshot(root, metadata).await;
    snapshot.marked_complete && snapshot.info.is_some()
}

pub(crate) fn app_data_root() -> Result<PathBuf> {
    dirs::data_local_dir()
        .map(|v| v.join("SplatStudio"))
        .ok_or_else(|| SplatError::Process("无法定位本机应用数据目录".into()))
}
fn settings_path() -> Result<PathBuf> {
    Ok(app_data_root()?.join("settings.json"))
}
fn index_path() -> Result<PathBuf> {
    Ok(app_data_root()?.join("project-index.json"))
}
pub fn default_projects_root() -> Result<PathBuf> {
    dirs::document_dir()
        .or_else(|| dirs::home_dir().map(|home| home.join("Documents")))
        .or_else(|| dirs::data_local_dir().map(|data| data.join("Documents")))
        .map(|v| v.join("SplatStudio").join("Projects"))
        .ok_or_else(|| SplatError::Process("无法定位 Documents 目录".into()))
}

pub async fn load_settings() -> Result<AppSettings> {
    let path = settings_path()?;
    if path.is_file() {
        if let Ok(bytes) = tokio::fs::read(&path).await {
            if let Ok(value) = serde_json::from_slice(&bytes) {
                return Ok(value);
            }
        }
    }
    Ok(AppSettings {
        mcp: Default::default(),
        schema_version: 2,
        projects_root: default_projects_root()?,
        planner_enabled: true,
    })
}

pub(crate) async fn save_settings(settings: &AppSettings) -> Result<()> {
    let path = settings_path()?;
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    atomic_write_json(&path, settings).await
}

/// 记住项目根目录。
pub async fn save_projects_root(root: PathBuf) -> Result<AppSettings> {
    crate::project::ProjectManager::validate_root(&root).await?;
    let mut settings = load_settings().await?;
    settings.projects_root = root;
    save_settings(&settings).await?;
    Ok(settings)
}

pub async fn save_planner_enabled(enabled: bool) -> Result<AppSettings> {
    let mut settings = load_settings().await?;
    settings.schema_version = settings.schema_version.max(2);
    settings.planner_enabled = enabled;
    save_settings(&settings).await?;
    Ok(settings)
}

async fn load_index_from(path: &Path) -> Result<ProjectIndex> {
    if path.is_file() {
        if let Ok(bytes) = tokio::fs::read(path).await {
            if let Ok(value) = serde_json::from_slice(&bytes) {
                return Ok(value);
            }
        }
    }
    Ok(ProjectIndex {
        schema_version: 1,
        projects: Vec::new(),
    })
}

async fn load_index() -> Result<ProjectIndex> {
    let _guard = PROJECT_INDEX_LOCK.lock().await;
    load_index_from(&index_path()?).await
}

async fn save_index_to(path: &Path, index: &ProjectIndex) -> Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    atomic_write_json(path, index).await
}

async fn register_project_at(index_path: &Path, id: Uuid, path: &Path) -> Result<()> {
    let _guard = PROJECT_INDEX_LOCK.lock().await;
    let mut index = load_index_from(index_path).await?;
    index
        .projects
        .retain(|item| item.id != id && item.path != path);
    index.projects.push(IndexedProject {
        id,
        path: path.to_path_buf(),
    });
    save_index_to(index_path, &index).await
}

pub async fn register_project(id: Uuid, path: &Path) -> Result<()> {
    register_project_at(&index_path()?, id, path).await
}

pub async fn validate_registered_final_ply(source: &Path) -> Result<PathBuf> {
    let source = std::fs::canonicalize(source)?;
    if source.file_name().and_then(|value| value.to_str()) != Some("final.ply") {
        return Err(SplatError::InvalidPath(source));
    }
    let index = load_index().await?;
    for item in index.projects {
        let root = match std::fs::canonicalize(&item.path) {
            Ok(path) => path,
            Err(_) => continue,
        };
        let direct = root.join("final.ply");
        let legacy = root.join("output").join("final.ply");
        if [direct, legacy]
            .into_iter()
            .filter_map(|path| std::fs::canonicalize(path).ok())
            .any(|path| path == source)
        {
            return Ok(source);
        }
    }
    Err(SplatError::InvalidPath(source))
}

async fn load_registered_project_from(
    index_path: &Path,
    id: Uuid,
) -> Result<(PathBuf, ProjectMetadata)> {
    let _guard = PROJECT_INDEX_LOCK.lock().await;
    let index = load_index_from(index_path).await?;
    let item = index
        .projects
        .into_iter()
        .find(|item| item.id == id)
        .ok_or_else(|| SplatError::Process("项目索引中不存在该项目".into()))?;
    let metadata = read_project_metadata_at(&item.path, id).await?;
    Ok((item.path, metadata))
}

/// Resolve a requested project before reading metadata, so continuation errors
/// can retain their project identity even when project.json cannot be read.
pub(crate) async fn registered_project_path(id: Uuid) -> Result<PathBuf> {
    registered_project_path_from(&index_path()?, id).await
}

pub(crate) async fn registered_project_path_from(index_path: &Path, id: Uuid) -> Result<PathBuf> {
    let _guard = PROJECT_INDEX_LOCK.lock().await;
    // Unlike background catalog scans, an explicit continuation must surface a
    // broken/unreadable index instead of treating it as an empty registry.
    let bytes = match tokio::fs::read(index_path).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(SplatError::Process("项目索引中不存在该项目".into()));
        }
        Err(error) => return Err(error.into()),
    };
    let index: ProjectIndex = serde_json::from_slice(&bytes)?;
    index
        .projects
        .into_iter()
        .find(|item| item.id == id)
        .map(|item| item.path)
        .ok_or_else(|| SplatError::Process("项目索引中不存在该项目".into()))
}

/// Identical metadata/ownership validation for already-bound tasks and catalog
/// continuations. Callers attach preparation diagnostics around this read.
pub(crate) async fn read_project_metadata_at(path: &Path, id: Uuid) -> Result<ProjectMetadata> {
    let bytes = tokio::fs::read(path.join("project.json")).await?;
    let metadata: ProjectMetadata = serde_json::from_slice(&bytes)?;
    if !has_project_ownership(&metadata, path, id) {
        return Err(SplatError::Process(
            "项目身份校验失败，拒绝访问预览文件".into(),
        ));
    }
    Ok(metadata)
}

pub async fn load_registered_project(id: Uuid) -> Result<(PathBuf, ProjectMetadata)> {
    load_registered_project_from(&index_path()?, id).await
}

async fn load_registered_project_summary_from(
    index_path: &Path,
    id: Uuid,
) -> Result<(PathBuf, ProjectMetadata, ProjectSummary)> {
    let (project_root, metadata) = load_registered_project_from(index_path, id).await?;
    let summary = summarize_project_metadata(&project_root, metadata.clone()).await?;
    Ok((project_root, metadata, summary))
}

pub async fn load_registered_project_summary(
    id: Uuid,
) -> Result<(PathBuf, ProjectMetadata, ProjectSummary)> {
    load_registered_project_summary_from(&index_path()?, id).await
}

pub async fn registered_final_ply_for_project(
    id: Uuid,
) -> Result<(PathBuf, PathBuf, ProjectMetadata)> {
    let (root, metadata) = load_registered_project(id).await?;
    if !project_is_durably_completed(&root, &metadata).await {
        return Err(SplatError::Process("只有已完成的项目可以预览".into()));
    }
    if metadata.imported.is_none() && metadata.model != "final.ply" {
        return Err(SplatError::Process("项目模型路径无效".into()));
    }
    let path = project_model_path(&root, &metadata)
        .filter(|path| path.is_file())
        .ok_or_else(|| {
            if metadata.imported.is_some() {
                SplatError::Process("外部 Gaussian 源文件不可用，请重新定位后再试".into())
            } else {
                SplatError::Process("项目缺少 final.ply".into())
            }
        })?;
    if let Some(imported) = &metadata.imported {
        if imported.canonical_model_path.is_none() {
            let file = path.metadata()?;
            let modified_ms = file
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|value| value.as_millis() as u64);
            if file.len() != imported.source_fingerprint.file_size
                || modified_ms != imported.source_fingerprint.modified_ms
            {
                return Err(SplatError::Process(
                    "外部 Gaussian 文件已发生变化。为避免把旧编辑应用到不同的 Gaussian 顺序，请重新定位原文件或作为新项目导入。".into(),
                ));
            }
        }
    }
    Ok((root, std::fs::canonicalize(path)?, metadata))
}

async fn scan_root(root: &Path, destinations: &mut Vec<PathBuf>) -> Result<()> {
    if !root.is_dir() {
        return Ok(());
    }
    let mut entries = tokio::fs::read_dir(root).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.is_dir() && path.join("project.json").is_file() {
            destinations.push(path);
        }
    }
    Ok(())
}

async fn rebuild_index_at(index_path: &Path, roots: &[PathBuf]) -> Result<Vec<ProjectSummary>> {
    let _guard = PROJECT_INDEX_LOCK.lock().await;
    let mut index = load_index_from(index_path).await?;
    let mut paths = index
        .projects
        .iter()
        .map(|v| v.path.clone())
        .collect::<Vec<_>>();
    for root in roots {
        scan_root(root, &mut paths).await?;
    }
    let mut seen = HashSet::new();
    paths.retain(|path| seen.insert(path.clone()));
    let mut summaries = Vec::new();
    let mut valid = Vec::new();
    for path in paths {
        if let Ok(summary) = summarize_project(&path).await {
            valid.push(IndexedProject {
                id: summary.id,
                path,
            });
            summaries.push(summary);
        }
    }
    summaries.sort_by(|a, b| {
        b.completed_at
            .unwrap_or(b.created_at)
            .cmp(&a.completed_at.unwrap_or(a.created_at))
    });
    index.projects = valid;
    save_index_to(index_path, &index).await?;
    Ok(summaries)
}

pub async fn get_overview() -> Result<ProjectOverview> {
    let settings = load_settings().await?;
    let default_root = default_projects_root()?;
    let mut roots = vec![default_root.clone()];
    if settings.projects_root != default_root {
        roots.push(settings.projects_root.clone());
    }
    let summaries = rebuild_index_at(&index_path()?, &roots).await?;
    Ok(ProjectOverview {
        projects_root: settings.projects_root,
        planner_enabled: settings.planner_enabled,
        projects: summaries,
    })
}

async fn summarize_project(project: &Path) -> Result<ProjectSummary> {
    let bytes = tokio::fs::read(project.join("project.json")).await?;
    let metadata: ProjectMetadata = serde_json::from_slice(&bytes)?;
    summarize_project_metadata(project, metadata).await
}

pub(crate) async fn summarize_project_metadata(
    project: &Path,
    mut metadata: ProjectMetadata,
) -> Result<ProjectSummary> {
    let completion = completion_snapshot(project, &metadata).await;
    if !completion.creation_ready {
        return Err(SplatError::Process("Project creation is not ready".into()));
    }
    let final_ply = completion.path;
    let imported_project = metadata.imported.is_some();
    let completion_inconsistent =
        completion.marked_complete && completion.info.is_none() && !imported_project;
    if completion.marked_complete && completion.info.is_some() {
        metadata.status = ProjectStatus::Completed;
    } else if completion_inconsistent {
        metadata.status = ProjectStatus::Interrupted;
    } else if imported_project {
        metadata.status = ProjectStatus::Completed;
    }
    let info = completion.info;
    let completed_at = metadata.completed_at.or_else(|| {
        final_ply
            .as_ref()
            .and_then(|p| p.metadata().ok()?.modified().ok())
            .map(DateTime::<Utc>::from)
    });
    let source_name = metadata
        .source_path
        .file_name()
        .and_then(|v| v.to_str())
        .unwrap_or("视频")
        .to_string();
    let output = metadata.output.as_ref();
    let source_available = metadata
        .imported
        .as_ref()
        .map(|_| imported_source_matches(&metadata));
    Ok(ProjectSummary {
        id: metadata.id,
        workspace_task_id: metadata.workspace_task_id,
        task_kind: if imported_project {
            "import"
        } else if metadata.reshoot.is_some() {
            "reshoot"
        } else {
            "generation"
        },
        name: if metadata.name.is_empty() {
            project
                .file_name()
                .and_then(|v| v.to_str())
                .unwrap_or("项目")
                .into()
        } else {
            metadata.name
        },
        status: metadata.status,
        project_path: project.to_path_buf(),
        final_ply,
        file_size: info
            .as_ref()
            .map(|v| v.file_size)
            .or_else(|| output.map(|v| v.file_size)),
        splat_count: info
            .as_ref()
            .map(|v| v.splat_count)
            .or_else(|| output.map(|v| v.splat_count)),
        created_at: metadata.created_at,
        completed_at,
        duration_ms: metadata.duration_ms,
        quality: (!imported_project).then_some(metadata.quality),
        source_name,
        model_format: metadata.imported.as_ref().map(|value| value.source_format),
        source_available,
        reshoot_eligible: completion.inputs_reshoot_eligible.or_else(|| {
            metadata.imported.as_ref().map(|value| {
                value
                    .colmap
                    .as_ref()
                    .is_some_and(|colmap| colmap.reshoot_eligible)
            })
        }),
        reshoot_reason: metadata
            .imported
            .as_ref()
            .and_then(|value| value.colmap.as_ref())
            .and_then(|colmap| colmap.reshoot_reason.clone()),
        // Imported projects did not run OOOSplat reconstruction. Their optional COLMAP data is
        // provenance for reshoot compatibility, not a registration result for this task.
        registered_ratio: if imported_project {
            None
        } else {
            output.map(|value| value.registered_ratio)
        },
        points_3d: output.map(|v| v.points_3d),
        failure_message: if completion_inconsistent {
            Some("完成记录与 final.ply 不一致，可以继续任务以修复结果".into())
        } else {
            metadata.failure_message
        },
    })
}

pub async fn delete_project(id: Uuid) -> Result<()> {
    let _guard = PROJECT_INDEX_LOCK.lock().await;
    let index_path = index_path()?;
    let mut index = load_index_from(&index_path).await?;
    let item = index
        .projects
        .iter()
        .find(|item| item.id == id)
        .cloned()
        .ok_or_else(|| SplatError::Process("项目索引中不存在该项目".into()))?;
    let bytes = tokio::fs::read(item.path.join("project.json")).await?;
    let metadata: ProjectMetadata = serde_json::from_slice(&bytes)?;
    if metadata.id != id {
        return Err(SplatError::Process("项目身份校验失败，拒绝删除".into()));
    }
    let owned = has_project_ownership(&metadata, &item.path, id);
    if !owned {
        return Err(SplatError::Process(
            "目录没有 OOOSplat 所有权标记，拒绝删除".into(),
        ));
    }
    let mut targets = vec![item.path.clone()];
    if metadata.app_id != PROJECT_APP_ID {
        let legacy_work = app_data_root()?.join("work").join(id.to_string());
        if legacy_work.exists() {
            targets.push(legacy_work);
        }
    }
    tokio::task::spawn_blocking(move || trash::delete_all(targets))
        .await
        .map_err(|e| SplatError::Process(e.to_string()))?
        .map_err(|e| SplatError::Process(format!("无法移入回收站：{e}")))?;
    index.projects.retain(|item| item.id != id);
    save_index_to(&index_path, &index).await
}

fn has_project_ownership(metadata: &ProjectMetadata, path: &Path, id: Uuid) -> bool {
    let id_string = id.to_string();
    metadata.id == id
        && (metadata.app_id == PROJECT_APP_ID
            || path.file_name().and_then(|value| value.to_str()) == Some(id_string.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn targeted_continuation_index_reads_are_strict_but_background_scans_stay_tolerant() {
        let root = tempfile::tempdir().unwrap();
        let index = root.path().join("index.json");
        let id = Uuid::new_v4();
        assert!(matches!(registered_project_path_from(&index, id).await,
            Err(SplatError::Process(message)) if message == "项目索引中不存在该项目"));
        std::fs::write(&index, b"{").unwrap();
        assert!(load_index_from(&index).await.unwrap().projects.is_empty());
        assert!(matches!(
            registered_project_path_from(&index, id).await,
            Err(SplatError::Json(_))
        ));
        std::fs::remove_file(&index).unwrap();
        std::fs::create_dir(&index).unwrap();
        let error = registered_project_path_from(&index, id).await.unwrap_err();
        assert!(error.io_error().is_some());
        std::fs::remove_dir(&index).unwrap();
        let project = root.path().join("project");
        atomic_write_json(
            &index,
            &ProjectIndex {
                schema_version: 1,
                projects: vec![IndexedProject {
                    id,
                    path: project.clone(),
                }],
            },
        )
        .await
        .unwrap();
        assert_eq!(
            registered_project_path_from(&index, id).await.unwrap(),
            project
        );
    }

    #[tokio::test]
    async fn continuation_metadata_reader_preserves_json_io_and_ownership_checks() {
        let root = tempfile::tempdir().unwrap();
        let id = Uuid::new_v4();
        assert!(read_project_metadata_at(root.path(), id)
            .await
            .unwrap_err()
            .io_error()
            .is_some());
        let metadata_path = root.path().join("project.json");
        std::fs::write(&metadata_path, b"{").unwrap();
        assert!(matches!(
            read_project_metadata_at(root.path(), id).await,
            Err(SplatError::Json(_))
        ));
        let mut metadata = test_metadata(root.path(), ProjectStatus::Interrupted);
        atomic_write_json(&metadata_path, &metadata).await.unwrap();
        assert!(matches!(
            read_project_metadata_at(root.path(), id).await,
            Err(SplatError::Process(_))
        ));
        metadata.id = id;
        atomic_write_json(&metadata_path, &metadata).await.unwrap();
        assert_eq!(
            read_project_metadata_at(root.path(), id).await.unwrap().id,
            id
        );
    }

    fn test_metadata(root: &Path, status: ProjectStatus) -> ProjectMetadata {
        ProjectMetadata {
            schema_version: crate::project::metadata::schema_version(),
            app_id: PROJECT_APP_ID.into(),
            id: Uuid::new_v4(),
            workspace_task_id: None,
            name: "test".into(),
            created_at: Utc::now(),
            started_at: None,
            completed_at: None,
            duration_ms: None,
            status,
            source_path: root.join("source.mp4"),
            input_type: crate::project::ProjectInputType::Video,
            quality: crate::presets::Quality::Balanced,
            project_path: root.to_path_buf(),
            output_path: None,
            output: None,
            failure_message: None,
            model: "final.ply".into(),
            transform: Default::default(),
            editing: Default::default(),
            reshoot: None,
            imported: None,
        }
    }

    fn write_valid_ply(path: &Path) {
        std::fs::write(path, b"ply\nformat binary_little_endian 1.0\nelement vertex 1\nproperty float x\nproperty float y\nproperty float z\nproperty float f_dc_0\nproperty float opacity\nproperty float scale_0\nproperty float rot_0\nend_header\n").unwrap();
    }
    #[test]
    fn default_summary_shape_is_serializable() {
        let value = AppSettings {
            mcp: Default::default(),
            schema_version: 1,
            planner_enabled: true,
            projects_root: PathBuf::from("C:/项目 Root"),
        };
        assert!(serde_json::to_string(&value)
            .unwrap()
            .contains("projectsRoot"));
        assert!(!serde_json::to_string(&value)
            .unwrap()
            .contains("colmapAcceleration"));
    }

    #[test]
    fn legacy_acceleration_setting_is_ignored() {
        let json = r#"{"schemaVersion":1,"projectsRoot":"C:/旧目录","colmapAcceleration":"gpu"}"#;
        let parsed: AppSettings = serde_json::from_str(json).unwrap();
        assert!(parsed.planner_enabled);
        assert_eq!(parsed.projects_root, PathBuf::from("C:/旧目录"));
        assert!(!serde_json::to_string(&parsed)
            .unwrap()
            .contains("colmapAcceleration"));
    }

    #[test]
    fn runtime_samples_fall_back_to_project_output_for_legacy_state() {
        let legacy_state = br#"{"stage":"completed"}"#;
        assert_eq!(
            runtime_sample_frame_count(Some(legacy_state), Some(533)),
            Some(533)
        );
        let current_state = br#"{"frames":{"extractedFrames":320}}"#;
        assert_eq!(
            runtime_sample_frame_count(Some(current_state), Some(533)),
            Some(320)
        );
        assert_eq!(runtime_sample_frame_count(None, Some(0)), None);
    }

    #[tokio::test]
    async fn completion_requires_a_marker_and_a_valid_final_ply() {
        let directory = tempfile::tempdir().unwrap();
        write_valid_ply(&directory.path().join("final.ply"));
        let mut metadata = test_metadata(directory.path(), ProjectStatus::Failed);
        assert!(!project_is_durably_completed(directory.path(), &metadata).await);

        let mut state =
            crate::project::PipelineStateFile::created(crate::presets::Quality::Balanced);
        state.stage = PipelineStage::Completed;
        std::fs::write(
            directory.path().join("state.json"),
            serde_json::to_vec(&state).unwrap(),
        )
        .unwrap();
        assert!(project_is_durably_completed(directory.path(), &metadata).await);
        std::fs::remove_file(directory.path().join("state.json")).unwrap();

        metadata.status = ProjectStatus::Completed;
        assert!(project_is_durably_completed(directory.path(), &metadata).await);

        std::fs::write(directory.path().join("final.ply"), b"broken").unwrap();
        assert!(!project_is_durably_completed(directory.path(), &metadata).await);
    }

    #[tokio::test]
    async fn inconsistent_completion_is_listed_as_interrupted() {
        let directory = tempfile::tempdir().unwrap();
        let metadata = test_metadata(directory.path(), ProjectStatus::Completed);
        std::fs::write(
            directory.path().join("project.json"),
            serde_json::to_vec(&metadata).unwrap(),
        )
        .unwrap();
        std::fs::write(directory.path().join("final.ply"), b"broken").unwrap();

        let summary = summarize_project(directory.path()).await.unwrap();
        assert_eq!(summary.status, ProjectStatus::Interrupted);
        assert!(summary.failure_message.unwrap().contains("final.ply"));
    }

    #[tokio::test]
    async fn imported_summary_does_not_report_a_synthetic_registration_ratio() {
        let directory = tempfile::tempdir().unwrap();
        let model_path = directory.path().join("external.ply");
        write_valid_ply(&model_path);
        let file = model_path.metadata().unwrap();
        let modified_ms = file
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|value| value.as_millis() as u64);
        let mut metadata = test_metadata(directory.path(), ProjectStatus::Completed);
        metadata.output = Some(crate::project::ProjectOutput {
            final_ply: model_path.clone(),
            file_size: file.len(),
            splat_count: 1,
            input_images: 0,
            registered_images: 0,
            registered_ratio: 0.0,
            points_3d: 0,
        });
        metadata.imported = Some(crate::project::ImportedProjectProvenance {
            source_model_path: model_path,
            source_format: crate::project::ImportedGaussianFormat::Ply,
            source_fingerprint: crate::project::ExternalFileFingerprint {
                sha256: "test".into(),
                file_size: file.len(),
                modified_ms,
            },
            canonical_model_path: None,
            colmap: None,
        });

        let summary = summarize_project_metadata(directory.path(), metadata)
            .await
            .unwrap();

        assert_eq!(summary.task_kind, "import");
        assert_eq!(summary.registered_ratio, None);
    }

    #[tokio::test]
    async fn concurrent_registration_rebuild_and_detail_read_preserve_the_index() {
        let directory = tempfile::tempdir().unwrap();
        let projects_root = directory.path().join("projects");
        let index_path = directory.path().join("project-index.json");
        std::fs::create_dir_all(&projects_root).unwrap();

        let mut projects = Vec::new();
        for number in 0..12 {
            let project = projects_root.join(format!("project-{number}"));
            std::fs::create_dir_all(&project).unwrap();
            let mut metadata = test_metadata(&project, ProjectStatus::Failed);
            metadata.id = Uuid::new_v4();
            metadata.name = format!("project-{number}");
            metadata.project_path = project.clone();
            std::fs::write(
                project.join("project.json"),
                serde_json::to_vec(&metadata).unwrap(),
            )
            .unwrap();
            projects.push((metadata.id, project));
        }

        register_project_at(&index_path, projects[0].0, &projects[0].1)
            .await
            .unwrap();
        let detail_index = index_path.clone();
        let detail_id = projects[0].0;
        let detail = tokio::spawn(async move {
            load_registered_project_summary_from(&detail_index, detail_id).await
        });
        let rebuild_index = index_path.clone();
        let rebuild_root = projects_root.clone();
        let rebuild =
            tokio::spawn(async move { rebuild_index_at(&rebuild_index, &[rebuild_root]).await });
        let registrations = projects
            .iter()
            .skip(1)
            .cloned()
            .map(|(id, project)| {
                let index_path = index_path.clone();
                tokio::spawn(async move { register_project_at(&index_path, id, &project).await })
            })
            .collect::<Vec<_>>();

        let (_, _, summary) = detail.await.unwrap().unwrap();
        assert_eq!(summary.id, detail_id);
        rebuild.await.unwrap().unwrap();
        for registration in registrations {
            registration.await.unwrap().unwrap();
        }

        let index = load_index_from(&index_path).await.unwrap();
        assert_eq!(index.projects.len(), projects.len());
        assert!(projects
            .iter()
            .all(|(id, _)| index.projects.iter().any(|project| project.id == *id)));
        let leftovers = std::fs::read_dir(directory.path())
            .unwrap()
            .filter_map(std::result::Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .collect::<Vec<_>>();
        assert!(leftovers.is_empty(), "leftover index files: {leftovers:?}");
    }

    #[tokio::test]
    async fn registered_project_summary_does_not_rebuild_the_global_index() {
        let directory = tempfile::tempdir().unwrap();
        let index_path = directory.path().join("project-index.json");
        let project = directory.path().join("target-project");
        std::fs::create_dir_all(&project).unwrap();
        let mut metadata = test_metadata(&project, ProjectStatus::Failed);
        metadata.project_path = project.clone();
        std::fs::write(
            project.join("project.json"),
            serde_json::to_vec(&metadata).unwrap(),
        )
        .unwrap();
        register_project_at(&index_path, metadata.id, &project)
            .await
            .unwrap();
        let missing_id = Uuid::new_v4();
        register_project_at(
            &index_path,
            missing_id,
            &directory.path().join("missing-project"),
        )
        .await
        .unwrap();

        let (_, _, summary) = load_registered_project_summary_from(&index_path, metadata.id)
            .await
            .unwrap();

        assert_eq!(summary.id, metadata.id);
        let index = load_index_from(&index_path).await.unwrap();
        assert_eq!(index.projects.len(), 2);
        assert!(index
            .projects
            .iter()
            .any(|project| project.id == missing_id));
    }

    #[test]
    fn deletion_ownership_rejects_unmarked_directories() {
        let id = Uuid::new_v4();
        let metadata = ProjectMetadata {
            schema_version: 2,
            app_id: "another.application".into(),
            id,
            workspace_task_id: None,
            name: "foreign".into(),
            created_at: Utc::now(),
            started_at: None,
            completed_at: None,
            duration_ms: None,
            status: ProjectStatus::Failed,
            source_path: PathBuf::new(),
            input_type: crate::project::ProjectInputType::Video,
            quality: crate::presets::Quality::Balanced,
            project_path: PathBuf::from("C:/arbitrary-folder"),
            output_path: None,
            output: None,
            failure_message: None,
            model: "final.ply".into(),
            transform: Default::default(),
            editing: Default::default(),
            reshoot: None,
            imported: None,
        };
        assert!(!has_project_ownership(
            &metadata,
            Path::new("C:/arbitrary-folder"),
            id
        ));
        assert!(has_project_ownership(
            &metadata,
            &PathBuf::from(id.to_string()),
            id
        ));
    }
}
