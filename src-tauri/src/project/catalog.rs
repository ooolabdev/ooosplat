use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    error::{Result, SplatError},
    pipeline::estimate::RuntimeSample,
    project::{manager::atomic_write_json, ProjectMetadata, ProjectStatus, PROJECT_APP_ID},
    reconstruction::ply::inspect_gaussian_ply,
};

pub async fn runtime_samples() -> Vec<RuntimeSample> {
    let Ok(index) = load_index().await else {
        return Vec::new();
    };
    let mut samples = Vec::new();
    for item in index.projects.into_iter().rev().take(20) {
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
        let Ok(state_bytes) = tokio::fs::read(item.path.join("state.json")).await else {
            continue;
        };
        let Ok(state) = serde_json::from_slice::<crate::project::PipelineStateFile>(&state_bytes)
        else {
            continue;
        };
        let (Some(video), Some(frames)) = (state.video, state.frames) else {
            continue;
        };
        let Some(extracted_frames) = frames.extracted_frames.filter(|count| *count > 0) else {
            continue;
        };
        samples.push(RuntimeSample {
            video,
            quality: metadata.quality,
            extracted_frames,
            duration_ms,
        });
    }
    samples
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub schema_version: u32,
    pub projects_root: PathBuf,
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
    pub name: String,
    pub status: ProjectStatus,
    pub project_path: PathBuf,
    pub final_ply: Option<PathBuf>,
    pub file_size: Option<u64>,
    pub splat_count: Option<u64>,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub duration_ms: Option<u64>,
    pub quality: crate::presets::Quality,
    pub source_name: String,
    pub registered_ratio: Option<f64>,
    pub points_3d: Option<u64>,
    pub failure_message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectOverview {
    pub projects_root: PathBuf,
    pub projects: Vec<ProjectSummary>,
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
        schema_version: 1,
        projects_root: default_projects_root()?,
    })
}

async fn save_settings(settings: &AppSettings) -> Result<()> {
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

async fn load_index() -> Result<ProjectIndex> {
    let path = index_path()?;
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

async fn save_index(index: &ProjectIndex) -> Result<()> {
    let path = index_path()?;
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    atomic_write_json(&path, index).await
}

pub async fn register_project(id: Uuid, path: &Path) -> Result<()> {
    let mut index = load_index().await?;
    index
        .projects
        .retain(|item| item.id != id && item.path != path);
    index.projects.push(IndexedProject {
        id,
        path: path.to_path_buf(),
    });
    save_index(&index).await
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

pub async fn load_registered_project(id: Uuid) -> Result<(PathBuf, ProjectMetadata)> {
    let index = load_index().await?;
    let item = index
        .projects
        .into_iter()
        .find(|item| item.id == id)
        .ok_or_else(|| SplatError::Process("项目索引中不存在该项目".into()))?;
    let bytes = tokio::fs::read(item.path.join("project.json")).await?;
    let metadata: ProjectMetadata = serde_json::from_slice(&bytes)?;
    if !has_project_ownership(&metadata, &item.path, id) {
        return Err(SplatError::Process(
            "项目身份校验失败，拒绝访问预览文件".into(),
        ));
    }
    Ok((item.path, metadata))
}

pub async fn registered_final_ply_for_project(
    id: Uuid,
) -> Result<(PathBuf, PathBuf, ProjectMetadata)> {
    let (root, metadata) = load_registered_project(id).await?;
    if metadata.status != ProjectStatus::Completed {
        return Err(SplatError::Process("只有已完成的项目可以预览".into()));
    }
    if metadata.model != "final.ply" {
        return Err(SplatError::Process("项目模型路径无效".into()));
    }
    let direct = root.join("final.ply");
    let legacy = root.join("output").join("final.ply");
    let path = if direct.is_file() {
        direct
    } else if legacy.is_file() {
        legacy
    } else {
        return Err(SplatError::Process("项目缺少 final.ply".into()));
    };
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

pub async fn get_overview() -> Result<ProjectOverview> {
    let settings = load_settings().await?;
    let mut index = load_index().await?;
    let mut paths = index
        .projects
        .iter()
        .map(|v| v.path.clone())
        .collect::<Vec<_>>();
    scan_root(&default_projects_root()?, &mut paths).await?;
    if settings.projects_root != default_projects_root()? {
        scan_root(&settings.projects_root, &mut paths).await?;
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
    save_index(&index).await?;
    Ok(ProjectOverview {
        projects_root: settings.projects_root,
        projects: summaries,
    })
}

async fn summarize_project(project: &Path) -> Result<ProjectSummary> {
    let bytes = tokio::fs::read(project.join("project.json")).await?;
    let mut metadata: ProjectMetadata = serde_json::from_slice(&bytes)?;
    let new_ply = project.join("final.ply");
    let legacy_ply = project.join("output").join("final.ply");
    let final_ply = if new_ply.is_file() {
        Some(new_ply)
    } else if legacy_ply.is_file() {
        Some(legacy_ply)
    } else {
        None
    };
    if final_ply.is_some() {
        metadata.status = ProjectStatus::Completed;
    }
    let info = final_ply
        .as_deref()
        .and_then(|path| inspect_gaussian_ply(path).ok());
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
    Ok(ProjectSummary {
        id: metadata.id,
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
        quality: metadata.quality,
        source_name,
        registered_ratio: output.map(|v| v.registered_ratio),
        points_3d: output.map(|v| v.points_3d),
        failure_message: metadata.failure_message,
    })
}

pub async fn delete_project(id: Uuid) -> Result<()> {
    let mut index = load_index().await?;
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
    save_index(&index).await
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
    #[test]
    fn default_summary_shape_is_serializable() {
        let value = AppSettings {
            schema_version: 1,
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
        assert_eq!(parsed.projects_root, PathBuf::from("C:/旧目录"));
        assert!(!serde_json::to_string(&parsed)
            .unwrap()
            .contains("colmapAcceleration"));
    }

    #[test]
    fn deletion_ownership_rejects_unmarked_directories() {
        let id = Uuid::new_v4();
        let metadata = ProjectMetadata {
            schema_version: 2,
            app_id: "another.application".into(),
            id,
            name: "foreign".into(),
            created_at: Utc::now(),
            started_at: None,
            completed_at: None,
            duration_ms: None,
            status: ProjectStatus::Failed,
            source_path: PathBuf::new(),
            quality: crate::presets::Quality::Balanced,
            project_path: PathBuf::from("C:/arbitrary-folder"),
            output_path: None,
            output: None,
            failure_message: None,
            model: "final.ply".into(),
            transform: Default::default(),
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
