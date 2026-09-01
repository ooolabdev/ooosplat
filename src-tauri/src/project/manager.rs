use std::path::{Path, PathBuf};

use chrono::{Local, Utc};
use uuid::Uuid;

use crate::{
    error::{Result, SplatError},
    presets::Quality,
    project::{catalog, PipelineStateFile, ProjectMetadata, ProjectStatus, PROJECT_APP_ID},
};

#[derive(Debug, Clone)]
pub struct ProjectPaths {
    pub id: Uuid,
    pub project: PathBuf,
    pub metadata: PathBuf,
    pub source: PathBuf,
    pub output: PathBuf,
    pub work: PathBuf,
    pub frames: PathBuf,
    pub colmap: PathBuf,
    pub brush: PathBuf,
    pub logs: PathBuf,
    pub state: PathBuf,
}

#[derive(Debug, Clone)]
pub struct ProjectManager {
    projects_root: PathBuf,
    register_in_catalog: bool,
}

impl ProjectManager {
    pub fn system_default() -> Result<Self> {
        Ok(Self {
            projects_root: catalog::default_projects_root()?,
            register_in_catalog: true,
        })
    }
    pub fn with_root(projects_root: PathBuf) -> Self {
        Self {
            projects_root,
            register_in_catalog: true,
        }
    }
    pub fn for_diagnostics(projects_root: PathBuf) -> Self {
        Self {
            projects_root,
            register_in_catalog: false,
        }
    }

    pub async fn validate_root(root: &Path) -> Result<()> {
        tokio::fs::create_dir_all(root).await?;
        if !root.is_dir() {
            return Err(SplatError::InvalidPath(root.to_path_buf()));
        }
        let probe = root.join(format!(".ooosplat-write-{}.tmp", Uuid::new_v4()));
        tokio::fs::write(&probe, b"OOOSplat")
            .await
            .map_err(|error| {
                SplatError::Process(format!("项目根目录不可写：{}（{error}）", root.display()))
            })?;
        tokio::fs::remove_file(probe).await?;
        Ok(())
    }

    pub async fn create(
        &self,
        source_video: &Path,
        quality: Quality,
    ) -> Result<(ProjectPaths, ProjectMetadata)> {
        validate_video_path(source_video)?;
        Self::validate_root(&self.projects_root).await?;
        let id = Uuid::new_v4();
        let stem = source_video
            .file_stem()
            .and_then(|v| v.to_str())
            .unwrap_or("project");
        let base = format!(
            "{}_{}",
            Local::now().format("%Y%m%d-%H%M%S"),
            sanitize_project_name(stem)
        );
        let project = unique_project_path(&self.projects_root, &base);
        let source = project.join("source");
        let work = project.join("work");
        let frames = work.join("frames");
        let colmap = work.join("colmap");
        let brush = work.join("brush");
        let logs = project.join("logs");
        for directory in [&source, &frames, &colmap, &brush, &logs] {
            tokio::fs::create_dir_all(directory).await?;
        }
        let extension = source_video
            .extension()
            .and_then(|v| v.to_str())
            .unwrap_or("mp4")
            .to_ascii_lowercase();
        let stored_source = source.join(format!("input.{extension}"));
        tokio::fs::copy(source_video, &stored_source).await?;
        let now = Utc::now();
        let metadata = ProjectMetadata {
            schema_version: crate::project::metadata::schema_version(),
            app_id: PROJECT_APP_ID.into(),
            id,
            name: base,
            created_at: now,
            started_at: Some(now),
            completed_at: None,
            duration_ms: None,
            status: ProjectStatus::Running,
            source_path: stored_source,
            quality,
            project_path: project.clone(),
            output_path: None,
            output: None,
            failure_message: None,
            model: "final.ply".into(),
            transform: Default::default(),
        };
        let metadata_path = project.join("project.json");
        atomic_write_json(&metadata_path, &metadata).await?;
        let state = project.join("state.json");
        atomic_write_json(&state, &PipelineStateFile::created(quality)).await?;
        if self.register_in_catalog {
            catalog::register_project(id, &project).await?;
        }
        Ok((
            ProjectPaths {
                id,
                project: project.clone(),
                metadata: metadata_path,
                source,
                output: project,
                work,
                frames,
                colmap,
                brush,
                logs,
                state,
            },
            metadata,
        ))
    }

    pub async fn write_state(&self, path: &Path, state: &PipelineStateFile) -> Result<()> {
        atomic_write_json(path, state).await
    }
    pub async fn write_metadata(&self, path: &Path, metadata: &ProjectMetadata) -> Result<()> {
        atomic_write_json(path, metadata).await
    }
}

pub fn sanitize_project_name(value: &str) -> String {
    let mut result = value
        .chars()
        .map(|ch| {
            if matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || ch.is_control()
            {
                '_'
            } else {
                ch
            }
        })
        .collect::<String>();
    result = result.trim().trim_end_matches(['.', ' ']).to_string();
    if result.is_empty() {
        result = "project".into();
    }
    result.chars().take(64).collect()
}

fn unique_project_path(root: &Path, base: &str) -> PathBuf {
    let initial = root.join(base);
    if !initial.exists() {
        return initial;
    }
    for suffix in 2..10_000 {
        let candidate = root.join(format!("{base}-{suffix}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    root.join(format!("{base}-{}", Uuid::new_v4()))
}

fn validate_video_path(path: &Path) -> Result<()> {
    if !path.is_file() {
        return Err(SplatError::InvalidPath(path.to_path_buf()));
    }
    match path
        .extension()
        .and_then(|v| v.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("mp4" | "mov") => Ok(()),
        _ => Err(SplatError::InvalidVideo("仅支持 MP4 或 MOV 文件".into())),
    }
}

pub async fn atomic_write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<()> {
    let temporary = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(value)?;
    let mut file = tokio::fs::File::create(&temporary).await?;
    use tokio::io::AsyncWriteExt;
    file.write_all(&bytes).await?;
    file.sync_all().await?;
    drop(file);
    atomic_replace(&temporary, path)?;
    Ok(())
}

#[cfg(windows)]
fn atomic_replace(source: &Path, destination: &Path) -> Result<()> {
    use std::{iter, os::windows::ffi::OsStrExt};
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };
    let source_wide = source
        .as_os_str()
        .encode_wide()
        .chain(iter::once(0))
        .collect::<Vec<_>>();
    let destination_wide = destination
        .as_os_str()
        .encode_wide()
        .chain(iter::once(0))
        .collect::<Vec<_>>();
    // SAFETY: Both paths are owned, NUL-terminated UTF-16 buffers that remain live for the call.
    let result = unsafe {
        MoveFileExW(
            source_wide.as_ptr(),
            destination_wide.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result != 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if error.kind() == std::io::ErrorKind::CrossesDevices {
        copy_replace_for_encrypted_directory(source, destination)
    } else {
        Err(error.into())
    }
}

#[cfg(windows)]
fn copy_replace_for_encrypted_directory(source: &Path, destination: &Path) -> Result<()> {
    let backup = destination.with_extension("json.bak");

    if backup.exists() {
        std::fs::remove_file(&backup)?;
    }
    if destination.exists() {
        std::fs::copy(destination, &backup)?;
        std::fs::OpenOptions::new()
            .write(true)
            .open(&backup)?
            .sync_all()?;
    }

    let replacement = (|| -> std::io::Result<()> {
        std::fs::copy(source, destination)?;
        std::fs::OpenOptions::new()
            .write(true)
            .open(destination)?
            .sync_all()?;
        std::fs::remove_file(source)?;
        Ok(())
    })();

    if let Err(error) = replacement {
        if backup.is_file() {
            let _ = std::fs::copy(&backup, destination);
        }
        return Err(error.into());
    }
    if backup.is_file() {
        std::fs::remove_file(backup)?;
    }
    Ok(())
}

#[cfg(not(windows))]
fn atomic_replace(source: &Path, destination: &Path) -> Result<()> {
    std::fs::rename(source, destination)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sanitizes_windows_names() {
        assert_eq!(sanitize_project_name("房子:轨迹?.mp4"), "房子_轨迹_.mp4");
    }

    #[test]
    fn limits_names_and_avoids_collisions() {
        let temporary = tempfile::tempdir().unwrap();
        let long = "a".repeat(100);
        assert_eq!(sanitize_project_name(&long).chars().count(), 64);
        let first = temporary.path().join("20260101-120000_demo");
        std::fs::create_dir(&first).unwrap();
        assert_eq!(
            unique_project_path(temporary.path(), "20260101-120000_demo"),
            temporary.path().join("20260101-120000_demo-2")
        );
    }

    #[tokio::test]
    async fn atomically_replaces_existing_json() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("settings.json");
        atomic_write_json(&path, &serde_json::json!({"value": 1}))
            .await
            .unwrap();
        atomic_write_json(&path, &serde_json::json!({"value": 2}))
            .await
            .unwrap();
        let value: serde_json::Value =
            serde_json::from_slice(&tokio::fs::read(path).await.unwrap()).unwrap();
        assert_eq!(value["value"], 2);
    }

    #[cfg(windows)]
    #[test]
    fn copy_replacement_keeps_latest_data_and_cleans_temporary_files() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("settings.json.tmp");
        let destination = temporary.path().join("settings.json");
        let backup = temporary.path().join("settings.json.bak");
        std::fs::write(&source, br#"{"value":2}"#).unwrap();
        std::fs::write(&destination, br#"{"value":1}"#).unwrap();

        copy_replace_for_encrypted_directory(&source, &destination).unwrap();

        assert_eq!(std::fs::read(&destination).unwrap(), br#"{"value":2}"#);
        assert!(!source.exists());
        assert!(!backup.exists());
    }

    #[tokio::test]
    async fn creates_self_contained_unicode_project() {
        let temporary = tempfile::tempdir().unwrap();
        let input = temporary.path().join("鞋子 scan.mp4");
        std::fs::write(&input, b"test").unwrap();
        let (paths, metadata) = ProjectManager::for_diagnostics(temporary.path().join("项目 Root"))
            .create(&input, Quality::Balanced)
            .await
            .unwrap();
        assert!(metadata.source_path.is_file());
        assert_eq!(paths.output, paths.project);
        assert!(paths.state.is_file());
    }
}
