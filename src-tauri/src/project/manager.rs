use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use chrono::{Local, Utc};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    error::{Result, SplatError},
    presets::Quality,
    project::{
        catalog, PipelineStateFile, ProjectInputType, ProjectMetadata, ProjectStatus,
        PROJECT_APP_ID,
    },
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
    pub colmap_frames: PathBuf,
    pub masks: PathBuf,
    pub colmap: PathBuf,
    pub brush: PathBuf,
    pub logs: PathBuf,
    pub state: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectImportProgress {
    pub current: u64,
    pub total: u64,
}

pub type ProjectImportObserver = Arc<dyn Fn(ProjectImportProgress) + Send + Sync + 'static>;

impl ProjectPaths {
    pub fn existing(id: Uuid, project: PathBuf) -> Self {
        let source = project.join("source");
        let work = project.join("work");
        Self {
            id,
            metadata: project.join("project.json"),
            output: project.clone(),
            frames: work.join("frames"),
            colmap_frames: work.join("colmap_frames"),
            masks: work.join("masks"),
            colmap: work.join("colmap"),
            brush: work.join("brush"),
            logs: project.join("logs"),
            state: project.join("state.json"),
            project,
            source,
            work,
        }
    }
}

pub type ProjectCheckpointObserver = Arc<dyn Fn(&PipelineStateFile) + Send + Sync>;

#[derive(Clone)]
pub struct ProjectManager {
    creation_intent: Option<crate::project::creation::ProjectCreationIntent>,
    checkpoint_observer: Option<ProjectCheckpointObserver>,
    projects_root: PathBuf,
    input_boundary: Option<PathBuf>,
    register_in_catalog: bool,
}

impl ProjectManager {
    /// Creates an owned workspace without copying or reinterpreting its heterogeneous inputs.
    pub async fn create_collection(
        &self,
        quality: Quality,
        inputs: crate::pipeline::inputs::GenerationInputs,
    ) -> Result<(ProjectPaths, ProjectMetadata)> {
        Self::validate_root(&self.projects_root).await?;
        let id = self
            .creation_intent
            .as_ref()
            .map_or_else(Uuid::new_v4, |i| i.project_id);
        let stem = match &inputs {
            crate::pipeline::inputs::GenerationInputs::Colmap { .. } => "colmap".into(),
            crate::pipeline::inputs::GenerationInputs::Materials { items } if items.len() == 1 => {
                match &items[0].source {
                    crate::pipeline::inputs::InputSource::Images { .. } => "images".into(),
                    source => source
                        .path()
                        .file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                }
            }
            _ => "mixed".into(),
        };
        let base = format!(
            "{}_{}",
            Local::now().format("%Y%m%d-%H%M%S"),
            sanitize_project_name(&stem)
        );
        let paths = ProjectPaths::existing(
            id,
            self.creation_intent
                .as_ref()
                .map(|i| i.project_path.clone())
                .unwrap_or_else(|| unique_project_path(&self.projects_root, &base)),
        );
        let input_type = if matches!(&inputs,crate::pipeline::inputs::GenerationInputs::Materials{items} if items.len()==1&&matches!(items[0].source,crate::pipeline::inputs::InputSource::Video{..}))
        {
            ProjectInputType::Video
        } else {
            ProjectInputType::Images
        };
        if self
            .begin_creation(&paths, quality, input_type)
            .await?
            .is_some_and(|s| s.creation.as_ref().is_some_and(|c| c.ready))
        {
            let metadata = serde_json::from_slice(&tokio::fs::read(&paths.metadata).await?)?;
            return Ok((paths, metadata));
        }
        for dir in [
            &paths.source,
            &paths.frames,
            &paths.colmap_frames,
            &paths.masks,
            &paths.colmap,
            &paths.brush,
            &paths.logs,
        ] {
            tokio::fs::create_dir_all(dir).await?;
        }
        let input_type = if matches!(&inputs,crate::pipeline::inputs::GenerationInputs::Materials{items} if items.len()==1&&matches!(items[0].source,crate::pipeline::inputs::InputSource::Video{..}))
        {
            ProjectInputType::Video
        } else {
            ProjectInputType::Images
        };
        let metadata = ProjectMetadata {
            id,
            workspace_task_id: self.creation_intent.as_ref().map(|i| i.task_id),
            app_id: PROJECT_APP_ID.into(),
            schema_version: crate::project::metadata::schema_version(),
            name: base,
            created_at: Utc::now(),
            source_path: paths.source.clone(),
            input_type,
            quality,
            project_path: paths.project.clone(),
            model: "final.ply".into(),
            ..Default::default()
        };
        self.write_metadata(&paths.metadata, &metadata).await?;
        let mut state = PipelineStateFile::created_for(quality, input_type);
        state.ba_policy_version = self
            .creation_intent
            .as_ref()
            .map_or(Some(crate::pipeline::ba_policy::VERSION), |i| {
                i.ba_policy_version
            });
        state.brush_training.execution_options = self
            .creation_intent
            .as_ref()
            .map_or_else(|| Some(Default::default()), |i| i.brush_execution_options);
        state.creation = self.creation_intent.clone().map(|intent| {
            crate::project::creation::ProjectCreationState {
                intent,
                ready: false,
            }
        });
        state.inputs = Some(crate::pipeline::inputs::InputCheckpoint::new(inputs));
        self.write_state(&paths.state, &state).await?;
        if self.register_in_catalog && self.creation_intent.is_none() {
            catalog::register_project(id, &paths.project).await?;
        }
        Ok((paths, metadata))
    }
    pub fn system_default() -> Result<Self> {
        Ok(Self {
            creation_intent: None,
            input_boundary: None,
            checkpoint_observer: None,
            projects_root: catalog::default_projects_root()?,
            register_in_catalog: true,
        })
    }
    pub fn with_root(projects_root: PathBuf) -> Self {
        Self {
            projects_root,
            creation_intent: None,
            input_boundary: None,
            checkpoint_observer: None,
            register_in_catalog: true,
        }
    }
    pub fn for_diagnostics(projects_root: PathBuf) -> Self {
        Self {
            projects_root,
            creation_intent: None,
            input_boundary: None,
            checkpoint_observer: None,
            register_in_catalog: false,
        }
    }

    pub fn with_checkpoint_observer(mut self, observer: ProjectCheckpointObserver) -> Self {
        self.checkpoint_observer = Some(observer);
        self
    }

    pub fn with_creation_intent(
        mut self,
        intent: Option<crate::project::creation::ProjectCreationIntent>,
    ) -> Self {
        self.creation_intent = intent;
        self
    }

    async fn begin_creation(
        &self,
        paths: &ProjectPaths,
        quality: Quality,
        input_type: ProjectInputType,
    ) -> Result<Option<PipelineStateFile>> {
        let Some(intent) = &self.creation_intent else {
            return Ok(None);
        };
        if intent.project_id != paths.id
            || intent.project_path != paths.project
            || paths.project.parent() != Some(self.projects_root.as_path())
        {
            return Err(SplatError::Process(
                "Project creation intent does not match its destination".into(),
            ));
        }
        if paths.project.exists() {
            let root = std::fs::canonicalize(&self.projects_root)?;
            let canonical = std::fs::canonicalize(&paths.project)?;
            if canonical.parent() != Some(root.as_path()) {
                return Err(SplatError::InvalidPath(paths.project.clone()));
            }
            if paths.state.exists() {
                let saved = self.read_state(&paths.state).await?;
                if saved.creation.as_ref().map(|s| &s.intent) != Some(intent) {
                    return Err(SplatError::Process(
                        "Project creation identity mismatch".into(),
                    ));
                }
                if paths.metadata.exists() {
                    let metadata: ProjectMetadata =
                        serde_json::from_slice(&tokio::fs::read(&paths.metadata).await?)?;
                    if metadata.id != intent.project_id
                        || metadata.workspace_task_id != Some(intent.task_id)
                    {
                        return Err(SplatError::Process(
                            "Project creation binding mismatch".into(),
                        ));
                    }
                } else if saved.creation.as_ref().is_some_and(|c| c.ready) {
                    return Err(SplatError::Process(
                        "Completed creation is missing project metadata".into(),
                    ));
                }
                return Ok(Some(saved));
            }
            if std::fs::read_dir(&paths.project)?.next().is_some() {
                return Err(SplatError::Process(
                    "Unidentified project directory cannot be adopted".into(),
                ));
            }
        }
        let mut state = PipelineStateFile::created_for(quality, input_type);
        state.creation = Some(crate::project::creation::ProjectCreationState {
            intent: intent.clone(),
            ready: false,
        });
        // Publish the directory only after its first owner state is durable.
        // A hard exit while writing that first JSON may leave a truncated temp
        // file, but cannot strand an unidentified non-empty project directory.
        let staging = self
            .projects_root
            .join(format!(".creation-{}", intent.intent_id));
        tokio::fs::create_dir_all(&staging).await?;
        let canonical_root = std::fs::canonicalize(&self.projects_root)?;
        let canonical_stage = std::fs::canonicalize(&staging)?;
        if canonical_stage.parent() != Some(canonical_root.as_path())
            || canonical_stage.file_name() != staging.file_name()
        {
            return Err(SplatError::InvalidPath(staging));
        }
        for entry in std::fs::read_dir(&staging)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let temporary = name
                .strip_prefix(".state.json.")
                .and_then(|v| v.strip_suffix(".tmp"))
                .is_some_and(|v| Uuid::parse_str(v).is_ok());
            if !entry.file_type()?.is_file() || (name != "state.json" && !temporary) {
                return Err(SplatError::Process(
                    "Unidentified creation staging cannot be adopted".into(),
                ));
            }
        }
        if staging.join("state.json").exists() {
            let saved = self.read_state(&staging.join("state.json")).await?;
            if saved.creation.as_ref().map(|c| &c.intent) != Some(intent) {
                return Err(SplatError::Process(
                    "Project creation identity mismatch".into(),
                ));
            }
        }
        self.write_state(&staging.join("state.json"), &state)
            .await?;
        if paths.project.exists() {
            // Only an empty, reserved directory reaches this branch. Keep it
            // rather than deleting it; install the complete owner file first.
            tokio::fs::rename(staging.join("state.json"), &paths.state).await?;
        } else {
            tokio::fs::rename(&staging, &paths.project).await?;
        }
        Ok(Some(state))
    }

    /// Ready is the durable barrier before catalog registration or engine work.
    pub async fn finish_creation(
        &self,
        paths: &ProjectPaths,
        metadata: &ProjectMetadata,
        state: &mut PipelineStateFile,
    ) -> Result<()> {
        let Some(creation) = state.creation.as_mut() else {
            return Ok(());
        };
        if creation.intent.project_id != paths.id
            || metadata.workspace_task_id != Some(creation.intent.task_id)
        {
            return Err(SplatError::Process(
                "Project creation binding mismatch".into(),
            ));
        }
        self.write_metadata(&paths.metadata, metadata).await?;
        creation.ready = true;
        self.write_state(&paths.state, state).await?;
        if self.register_in_catalog {
            catalog::register_project(paths.id, &paths.project).await?;
        }
        Ok(())
    }

    pub fn with_input_boundary(mut self, boundary: Option<PathBuf>) -> Self {
        self.input_boundary = boundary;
        self
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
        input: &Path,
        quality: Quality,
    ) -> Result<(ProjectPaths, ProjectMetadata)> {
        self.create_with_progress(input, quality, None, None).await
    }

    pub async fn create_with_progress(
        &self,
        input: &Path,
        quality: Quality,
        observer: Option<ProjectImportObserver>,
        cancellation: Option<CancellationToken>,
    ) -> Result<(ProjectPaths, ProjectMetadata)> {
        self.create_inner(input, quality, observer, cancellation, false)
            .await
    }

    pub async fn create_reshoot_v2(
        &self,
        input: &Path,
        quality: Quality,
        cancellation: CancellationToken,
    ) -> Result<(ProjectPaths, ProjectMetadata)> {
        self.create_inner(input, quality, None, Some(cancellation), true)
            .await
    }

    async fn create_inner(
        &self,
        input: &Path,
        quality: Quality,
        observer: Option<ProjectImportObserver>,
        cancellation: Option<CancellationToken>,
        mixed_sizes: bool,
    ) -> Result<(ProjectPaths, ProjectMetadata)> {
        let import_identity = if input.is_dir() {
            Some(crate::video::image_projection::source_identity(input)?)
        } else {
            None
        };
        let image_scan = if input.is_dir() {
            let source = input.to_path_buf();
            let token = cancellation.clone();
            let boundary = self.input_boundary.clone();
            Some(
                crate::presets::spawn_pipeline_blocking(move || {
                    for path in crate::video::list_images(&source)? {
                        if token.as_ref().is_some_and(CancellationToken::is_cancelled) {
                            return Err(SplatError::Cancelled);
                        }
                        if let Some(root) = &boundary {
                            crate::tasks::authorize_input(&path, std::slice::from_ref(root))
                                .map_err(|e| SplatError::Process(e.to_string()))?;
                        }
                    }
                    let scan = if mixed_sizes {
                        crate::video::image_projection::analyze_reshoot(&source)?
                    } else {
                        crate::video::image_projection::analyze(&source)?
                    };
                    Ok(crate::video::image_sequence::ImageSequenceScan {
                        info: scan.info,
                        images: scan
                            .images
                            .into_iter()
                            .map(|image| crate::video::image_sequence::ScannedImage {
                                path: image.path,
                                has_alpha_channel: image.alpha,
                            })
                            .collect(),
                    })
                })
                .await
                .map_err(|error| SplatError::Process(format!("图片序列分析任务失败：{error}")))??,
            )
        } else {
            None
        };
        let input_type = if image_scan.is_some() {
            ProjectInputType::Images
        } else {
            validate_video_path(input)?;
            ProjectInputType::Video
        };
        Self::validate_root(&self.projects_root).await?;
        let id = self
            .creation_intent
            .as_ref()
            .map_or_else(Uuid::new_v4, |i| i.project_id);
        let stem = if input_type == ProjectInputType::Images {
            input
                .file_name()
                .and_then(|v| v.to_str())
                .unwrap_or("images")
        } else {
            input
                .file_stem()
                .and_then(|v| v.to_str())
                .unwrap_or("project")
        };
        let base = format!(
            "{}_{}",
            Local::now().format("%Y%m%d-%H%M%S"),
            sanitize_project_name(stem)
        );
        let project = self
            .creation_intent
            .as_ref()
            .map(|i| i.project_path.clone())
            .unwrap_or_else(|| unique_project_path(&self.projects_root, &base));
        let reserved_paths = ProjectPaths::existing(id, project.clone());
        if self
            .begin_creation(&reserved_paths, quality, input_type)
            .await?
            .is_some_and(|s| s.creation.as_ref().is_some_and(|c| c.ready))
        {
            let metadata =
                serde_json::from_slice(&tokio::fs::read(&reserved_paths.metadata).await?)?;
            return Ok((reserved_paths, metadata));
        }
        let source = project.join("source");
        let work = project.join("work");
        let frames = work.join("frames");
        let colmap_frames = work.join("colmap_frames");
        let masks = work.join("masks");
        let colmap = work.join("colmap");
        let brush = work.join("brush");
        let logs = project.join("logs");
        for directory in [&source, &frames, &colmap_frames, &colmap, &brush, &logs] {
            tokio::fs::create_dir_all(directory).await?;
        }
        let stored_source = if input_type == ProjectInputType::Images {
            let images_dir = source.join("images");
            tokio::fs::create_dir_all(&images_dir).await?;
            let scan = image_scan
                .as_ref()
                .expect("image projects have a completed header scan");
            let total = scan.images.len() as u64;
            let mut last_percent = -1_i32;
            let source_identity = import_identity.as_ref().expect("image import identity");
            let mut origins = std::collections::BTreeMap::new();
            if let Some(observer) = &observer {
                observer(ProjectImportProgress { current: 0, total });
            }
            for (index, image) in scan.images.iter().enumerate() {
                if cancellation
                    .as_ref()
                    .is_some_and(CancellationToken::is_cancelled)
                {
                    if self.creation_intent.is_none() {
                        let _ = tokio::fs::remove_dir_all(&project).await;
                    }
                    return Err(SplatError::Cancelled);
                }
                if let Some(boundary) = &self.input_boundary {
                    crate::tasks::authorize_input(&image.path, std::slice::from_ref(boundary))
                        .map_err(|e| SplatError::Process(e.to_string()))?;
                }
                let dest =
                    images_dir.join(crate::video::normalized_image_name(index, &image.path)?);
                let stat = tokio::fs::metadata(&image.path).await?;
                let original = crate::video::image_projection::OriginalImageIdentity {
                    path: std::fs::canonicalize(&image.path)?,
                    size: stat.len(),
                    modified_nanos: stat
                        .modified()?
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_err(|e| SplatError::Process(e.to_string()))?
                        .as_nanos()
                        .to_string(),
                };
                if let Err(error) = tokio::fs::copy(&image.path, &dest).await {
                    if self.creation_intent.is_none() {
                        let _ = tokio::fs::remove_dir_all(&project).await;
                    }
                    return Err(error.into());
                }
                origins.insert(
                    dest.file_name().unwrap().to_string_lossy().into_owned(),
                    original,
                );
                let current = index as u64 + 1;
                let percent = (current.saturating_mul(100) / total.max(1)) as i32;
                if percent != last_percent || current == total {
                    last_percent = percent;
                    if let Some(observer) = &observer {
                        observer(ProjectImportProgress { current, total });
                    }
                }
            }
            if &crate::video::image_projection::source_identity(input)? != source_identity {
                return Err(SplatError::Process(
                    "素材文件已发生变化，请重新分析后再生成".into(),
                ));
            }
            atomic_write_json(&source.join("image-origins.json"), &origins).await?;
            images_dir
        } else {
            let extension = input
                .extension()
                .and_then(|v| v.to_str())
                .unwrap_or("mp4")
                .to_ascii_lowercase();
            let stored = source.join(format!("input.{extension}"));
            if let Some(boundary) = &self.input_boundary {
                if std::fs::canonicalize(input)? != *boundary {
                    return Err(SplatError::InvalidPath(input.to_path_buf()));
                }
            }
            tokio::fs::copy(input, &stored).await?;
            stored
        };
        let now = Utc::now();
        let metadata = ProjectMetadata {
            schema_version: crate::project::metadata::schema_version(),
            app_id: PROJECT_APP_ID.into(),
            id,
            workspace_task_id: self.creation_intent.as_ref().map(|i| i.task_id),
            name: base,
            created_at: now,
            started_at: Some(now),
            completed_at: None,
            duration_ms: None,
            status: ProjectStatus::Running,
            source_path: stored_source,
            input_type,
            quality,
            project_path: project.clone(),
            output_path: None,
            output: None,
            failure_message: None,
            model: "final.ply".into(),
            transform: Default::default(),
            editing: Default::default(),
            reshoot: None,
            imported: None,
        };
        let metadata_path = project.join("project.json");
        atomic_write_json(&metadata_path, &metadata).await?;
        let state = project.join("state.json");
        let mut pipeline_state = PipelineStateFile::created_for(quality, input_type);
        pipeline_state.ba_policy_version = self
            .creation_intent
            .as_ref()
            .map_or(Some(crate::pipeline::ba_policy::VERSION), |i| {
                i.ba_policy_version
            });
        pipeline_state.brush_training.execution_options = self
            .creation_intent
            .as_ref()
            .map_or_else(|| Some(Default::default()), |i| i.brush_execution_options);
        pipeline_state.creation = self.creation_intent.clone().map(|intent| {
            crate::project::creation::ProjectCreationState {
                intent,
                ready: false,
            }
        });
        pipeline_state.image_sequence = image_scan.map(|scan| scan.info);
        atomic_write_json(&state, &pipeline_state).await?;
        if self.register_in_catalog && self.creation_intent.is_none() {
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
                colmap_frames,
                masks,
                colmap,
                brush,
                logs,
                state,
            },
            metadata,
        ))
    }

    pub async fn write_state(&self, path: &Path, state: &PipelineStateFile) -> Result<()> {
        atomic_write_json(path, state).await?;
        if let Some(observer) = &self.checkpoint_observer {
            observer(state);
        }
        Ok(())
    }
    /// Preparation saves retain their original I/O error while identifying the
    /// failed atomic-write step. Normal checkpoint saves intentionally opt out.
    pub(crate) async fn write_state_for_preparation(
        &self,
        path: &Path,
        state: &PipelineStateFile,
    ) -> Result<()> {
        atomic_write_json_for_preparation(path, state).await?;
        if let Some(observer) = &self.checkpoint_observer {
            observer(state);
        }
        Ok(())
    }
    pub async fn read_state(&self, path: &Path) -> Result<PipelineStateFile> {
        Ok(serde_json::from_slice(&tokio::fs::read(path).await?)?)
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

pub(crate) fn unique_project_path(root: &Path, base: &str) -> PathBuf {
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
    atomic_write_json_with_context(path, value, &AtomicWriteContext::default()).await
}

pub(crate) async fn atomic_write_json_for_preparation<T: serde::Serialize>(
    path: &Path,
    value: &T,
) -> Result<()> {
    atomic_write_json_with_context(path, value, &AtomicWriteContext::preparation()).await
}

#[derive(Default)]
struct AtomicWriteContext {
    preparation: bool,
    // Per-call injection avoids thread-local/global hooks leaking across async
    // tests or unrelated checkpoint writes.
    #[cfg(test)]
    failures: Vec<(&'static str, i32)>,
}

impl AtomicWriteContext {
    fn preparation() -> Self {
        Self {
            preparation: true,
            #[cfg(test)]
            failures: Vec::new(),
        }
    }

    fn error(&self, step: &'static str, error: std::io::Error) -> SplatError {
        let error = SplatError::Io(error);
        if self.preparation {
            error.with_io_diagnostic(format!("io_substep={step}"))
        } else {
            error
        }
    }

    fn before(&self, step: &'static str) -> Result<()> {
        #[cfg(test)]
        if let Some((_, code)) = self.failures.iter().find(|(name, _)| *name == step) {
            return Err(self.error(step, std::io::Error::from_raw_os_error(*code)));
        }
        let _ = step;
        Ok(())
    }

    async fn io<T>(
        &self,
        step: &'static str,
        operation: impl std::future::Future<Output = std::io::Result<T>>,
    ) -> Result<T> {
        self.before(step)?;
        operation.await.map_err(|error| self.error(step, error))
    }

    #[cfg(windows)]
    fn sync_io<T>(
        &self,
        step: &'static str,
        operation: impl FnOnce() -> std::io::Result<T>,
    ) -> Result<T> {
        self.before(step)?;
        operation().map_err(|error| self.error(step, error))
    }
}

async fn atomic_write_json_with_context<T: serde::Serialize>(
    path: &Path,
    value: &T,
    context: &AtomicWriteContext,
) -> Result<()> {
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("data.json");
    let temporary = path.with_file_name(format!(".{file_name}.{}.tmp", Uuid::new_v4()));
    let bytes = serde_json::to_vec_pretty(value)?;
    let result = async {
        let mut file = context
            .io("create_state_temp", tokio::fs::File::create(&temporary))
            .await?;
        use tokio::io::AsyncWriteExt;
        context
            .io("write_state_temp", file.write_all(&bytes))
            .await?;
        context.io("sync_state_temp", file.sync_all()).await?;
        drop(file);
        atomic_replace_with_context(&temporary, path, context)
    }
    .await;
    if result.is_err() {
        let _ = context
            .io("cleanup_state_temp", tokio::fs::remove_file(&temporary))
            .await;
    }
    result
}

pub(crate) async fn atomic_replace_file(source: &Path, destination: &Path) -> Result<()> {
    let source = source.to_path_buf();
    let destination = destination.to_path_buf();
    crate::presets::spawn_pipeline_blocking(move || atomic_replace(&source, &destination))
        .await
        .map_err(|error| SplatError::Process(format!("原子发布任务失败：{error}")))?
}

fn atomic_replace(source: &Path, destination: &Path) -> Result<()> {
    atomic_replace_with_context(source, destination, &AtomicWriteContext::default())
}

#[cfg(windows)]
fn atomic_replace_with_context(
    source: &Path,
    destination: &Path,
    context: &AtomicWriteContext,
) -> Result<()> {
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
    context.before("replace_state")?;
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
        copy_replace_for_encrypted_directory_with_context(source, destination, context)
    } else {
        Err(context.error("replace_state", error))
    }
}

#[cfg(all(windows, test))]
fn copy_replace_for_encrypted_directory(source: &Path, destination: &Path) -> Result<()> {
    copy_replace_for_encrypted_directory_with_context(
        source,
        destination,
        &AtomicWriteContext::default(),
    )
}

#[cfg(windows)]
fn copy_replace_for_encrypted_directory_with_context(
    source: &Path,
    destination: &Path,
    context: &AtomicWriteContext,
) -> Result<()> {
    let backup_extension = destination
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| format!("{extension}.bak"))
        .unwrap_or_else(|| "bak".into());
    let backup = destination.with_extension(backup_extension);

    if backup.exists() {
        context.sync_io("remove_state_backup", || std::fs::remove_file(&backup))?;
    }
    if destination.exists() {
        context.sync_io("backup_state", || std::fs::copy(destination, &backup))?;
        let file = context.sync_io("open_state_backup", || {
            std::fs::OpenOptions::new().write(true).open(&backup)
        })?;
        context.sync_io("sync_state_backup", || file.sync_all())?;
    }

    let replacement = (|| -> Result<()> {
        context.sync_io("copy_state_replacement", || {
            std::fs::copy(source, destination)
        })?;
        let file = context.sync_io("open_state_replacement", || {
            std::fs::OpenOptions::new().write(true).open(destination)
        })?;
        context.sync_io("sync_state_replacement", || file.sync_all())?;
        drop(file);
        context.sync_io("remove_state_temp", || std::fs::remove_file(source))?;
        Ok(())
    })();

    if let Err(mut error) = replacement {
        if backup.is_file() {
            if let Err(restore) = context.sync_io("restore_state_backup", || {
                std::fs::copy(&backup, destination)
            }) {
                if context.preparation {
                    let io = restore.io_error();
                    error = error.with_io_diagnostic(format!(
                        "io_substep=restore_state_backup; secondary_error_kind={:?}; secondary_os_error={}",
                        io.map(std::io::Error::kind),
                        io.and_then(std::io::Error::raw_os_error)
                            .map_or_else(|| "none".into(), |value| value.to_string()),
                    ));
                }
            }
        }
        return Err(error);
    }
    if backup.is_file() {
        context.sync_io("cleanup_state_backup", || std::fs::remove_file(backup))?;
    }
    Ok(())
}

#[cfg(not(windows))]
fn atomic_replace_with_context(
    source: &Path,
    destination: &Path,
    context: &AtomicWriteContext,
) -> Result<()> {
    context.before("replace_state")?;
    std::fs::rename(source, destination).map_err(|error| context.error("replace_state", error))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn reserved(root: &Path) -> crate::project::creation::ProjectCreationIntent {
        crate::project::creation::ProjectCreationIntent {
            ba_policy_version: Some(crate::pipeline::ba_policy::VERSION),
            brush_execution_options: Some(Default::default()),
            intent_id: Uuid::new_v4(),
            project_id: Uuid::new_v4(),
            task_id: Uuid::new_v4(),
            creating_run_id: Uuid::new_v4(),
            project_path: root.join("reserved"),
            request_identity: "request-v1".into(),
        }
    }

    #[tokio::test]
    async fn first_owner_state_hard_exit_does_not_strand_an_unidentified_project() {
        let root = tempfile::tempdir().unwrap();
        let intent = reserved(root.path());
        let staging = root.path().join(format!(".creation-{}", intent.intent_id));
        std::fs::create_dir(&staging).unwrap();
        let interrupted = staging.join(format!(".state.json.{}.tmp", Uuid::new_v4()));
        std::fs::write(interrupted, b"{ truncated first owner state").unwrap();
        let paths = ProjectPaths::existing(intent.project_id, intent.project_path.clone());
        let manager = ProjectManager::for_diagnostics(root.path().into())
            .with_creation_intent(Some(intent.clone()));
        manager
            .begin_creation(&paths, Quality::Fast, ProjectInputType::Video)
            .await
            .unwrap();
        assert!(!staging.exists());
        assert_eq!(
            manager
                .read_state(&paths.state)
                .await
                .unwrap()
                .creation
                .unwrap()
                .intent,
            intent
        );
        assert!(!paths.metadata.exists());
    }

    #[tokio::test]
    async fn interrupted_creation_reuses_identity_and_waits_for_complete_configuration() {
        use crate::pipeline::inputs::{GenerationInputs, InputSource, MaterialInput};
        let root = tempfile::tempdir().unwrap();
        let intent = reserved(root.path());
        let manager = ProjectManager::for_diagnostics(root.path().into())
            .with_creation_intent(Some(intent.clone()));
        let inputs = GenerationInputs::Materials {
            items: vec![MaterialInput {
                id: "video".into(),
                source: InputSource::Video {
                    path: root.path().join("source.mp4"),
                },
                identity: None,
                projection: crate::video::panorama::VideoProjection::Perspective,
                image_matching_strategy: Default::default(),
                camera_relation: None,
                confirmed_small_images: false,
                confirmed_image_order: false,
                frame_removal_fps: None,
            }],
        };
        let paths = ProjectPaths::existing(intent.project_id, intent.project_path.clone());
        manager
            .begin_creation(&paths, Quality::Fast, ProjectInputType::Video)
            .await
            .unwrap();
        // Fault after the durable owner state, before metadata/configuration.
        let (same, metadata) = manager
            .create_collection(Quality::Fast, inputs.clone())
            .await
            .unwrap();
        assert_eq!(same.id, intent.project_id);
        assert_eq!(metadata.workspace_task_id, Some(intent.task_id));
        let mut state = manager.read_state(&same.state).await.unwrap();
        assert!(!state.creation.as_ref().unwrap().ready);
        assert!(
            catalog::summarize_project_metadata(&same.project, metadata.clone())
                .await
                .is_err()
        );
        state.configuration = Some((*crate::presets::pipeline_optimization_config()).clone());
        state.generation_policy = Some(Default::default());
        manager
            .finish_creation(&same, &metadata, &mut state)
            .await
            .unwrap();
        assert!(
            manager
                .read_state(&same.state)
                .await
                .unwrap()
                .creation
                .unwrap()
                .ready
        );
        let (again, _) = manager
            .create_collection(Quality::Fast, inputs)
            .await
            .unwrap();
        assert_eq!(again.project, same.project);
        assert_eq!(
            serde_json::to_value(manager.read_state(&same.state).await.unwrap().configuration)
                .unwrap(),
            serde_json::to_value(&state.configuration).unwrap()
        );
        assert!(state.brush_training.execution_options.is_some());
        assert_eq!(
            state.ba_policy_version,
            Some(crate::pipeline::ba_policy::VERSION)
        );
    }

    #[tokio::test]
    async fn creation_failure_preserves_barrier_and_rejects_unknown_directory() {
        let root = tempfile::tempdir().unwrap();
        let intent = reserved(root.path());
        let manager = ProjectManager::for_diagnostics(root.path().into())
            .with_creation_intent(Some(intent.clone()));
        let paths = ProjectPaths::existing(intent.project_id, intent.project_path.clone());
        manager
            .begin_creation(&paths, Quality::Fast, ProjectInputType::Video)
            .await
            .unwrap();
        std::fs::create_dir(&paths.metadata).unwrap();
        let mut state = manager.read_state(&paths.state).await.unwrap();
        let metadata = ProjectMetadata {
            id: intent.project_id,
            workspace_task_id: Some(intent.task_id),
            ..Default::default()
        };
        assert!(manager
            .finish_creation(&paths, &metadata, &mut state)
            .await
            .is_err());
        assert!(
            !manager
                .read_state(&paths.state)
                .await
                .unwrap()
                .creation
                .unwrap()
                .ready
        );
        let other = reserved(root.path());
        let other_manager = manager.clone().with_creation_intent(Some(other.clone()));
        assert!(other_manager
            .begin_creation(
                &ProjectPaths::existing(other.project_id, other.project_path),
                Quality::Fast,
                ProjectInputType::Video
            )
            .await
            .is_err());
        assert!(paths.state.is_file());
    }
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

    #[tokio::test]
    async fn preparation_atomic_write_preserves_os_error_for_each_substep() {
        for step in [
            "create_state_temp",
            "write_state_temp",
            "sync_state_temp",
            "replace_state",
        ] {
            for code in [5, 32, 33, 112] {
                let temporary = tempfile::tempdir().unwrap();
                let path = temporary.path().join("state.json");
                let original = br#"{"preserved":true}"#;
                std::fs::write(&path, original).unwrap();
                let context = AtomicWriteContext {
                    preparation: true,
                    failures: vec![(step, code)],
                };
                let error = atomic_write_json_with_context(
                    &path,
                    &serde_json::json!({"replacement": true}),
                    &context,
                )
                .await
                .unwrap_err();
                assert_eq!(
                    error.io_error().and_then(std::io::Error::raw_os_error),
                    Some(code),
                    "{step}"
                );
                assert_eq!(
                    error.io_error().unwrap().kind(),
                    std::io::Error::from_raw_os_error(code).kind()
                );
                assert!(error.to_string().contains(&format!("io_substep={step}")));
                assert!(!error
                    .to_string()
                    .contains(&path.to_string_lossy().to_string()));
                assert_eq!(std::fs::read(&path).unwrap(), original);
                assert_eq!(std::fs::read_dir(temporary.path()).unwrap().count(), 1);
            }
        }
    }

    #[tokio::test]
    async fn preparation_atomic_write_is_opt_in_for_real_io_failures() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("missing").join("state.json");
        let value = serde_json::json!({"value": 1});
        let ordinary = atomic_write_json(&path, &value).await.unwrap_err();
        let preparation = atomic_write_json_for_preparation(&path, &value)
            .await
            .unwrap_err();
        assert!(matches!(&ordinary, SplatError::Io(_)));
        assert!(!ordinary.to_string().contains("io_substep="));
        assert!(preparation
            .to_string()
            .contains("io_substep=create_state_temp"));
        assert_eq!(
            ordinary.io_error().unwrap().raw_os_error(),
            preparation.io_error().unwrap().raw_os_error()
        );
        assert_eq!(
            ordinary.io_error().unwrap().kind(),
            preparation.io_error().unwrap().kind()
        );
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn preparation_atomic_write_cleanup_keeps_the_primary_os_error() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("state.json");
        let original = br#"{"preserved":true}"#;
        std::fs::write(&path, original).unwrap();
        let context = AtomicWriteContext {
            preparation: true,
            failures: vec![("write_state_temp", 112), ("cleanup_state_temp", 5)],
        };
        let error = atomic_write_json_with_context(
            &path,
            &serde_json::json!({"replacement": true}),
            &context,
        )
        .await
        .unwrap_err();
        assert_eq!(
            error.io_error().and_then(std::io::Error::raw_os_error),
            Some(112)
        );
        assert!(error.to_string().contains("io_substep=write_state_temp"));
        assert!(!error.to_string().contains("io_substep=cleanup_state_temp"));
        assert_eq!(std::fs::read(&path).unwrap(), original);
        // The failed best-effort cleanup is confined to this temporary fixture.
        assert_eq!(std::fs::read_dir(temporary.path()).unwrap().count(), 2);
    }

    #[tokio::test]
    async fn preparation_state_write_observer_runs_only_after_success() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let temporary = tempfile::tempdir().unwrap();
        let notifications = Arc::new(AtomicUsize::new(0));
        let captured = notifications.clone();
        let manager = ProjectManager::for_diagnostics(temporary.path().into())
            .with_checkpoint_observer(Arc::new(move |_| {
                captured.fetch_add(1, Ordering::SeqCst);
            }));
        let state = PipelineStateFile::created(Quality::Balanced);
        let missing = temporary.path().join("missing").join("state.json");
        let error = manager
            .write_state_for_preparation(&missing, &state)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("io_substep=create_state_temp"));
        assert_eq!(notifications.load(Ordering::SeqCst), 0);
        let path = temporary.path().join("state.json");
        manager
            .write_state_for_preparation(&path, &state)
            .await
            .unwrap();
        assert_eq!(notifications.load(Ordering::SeqCst), 1);
        assert_eq!(
            std::fs::read(&path).unwrap(),
            serde_json::to_vec_pretty(&state).unwrap()
        );
    }

    #[cfg(windows)]
    #[test]
    fn preparation_encrypted_replacement_identifies_backup_copy_and_sync_failures() {
        for step in [
            "remove_state_backup",
            "backup_state",
            "open_state_backup",
            "sync_state_backup",
            "copy_state_replacement",
            "open_state_replacement",
            "sync_state_replacement",
            "remove_state_temp",
            "cleanup_state_backup",
        ] {
            let temporary = tempfile::tempdir().unwrap();
            let source = temporary.path().join("state.json.tmp");
            let destination = temporary.path().join("state.json");
            let backup = temporary.path().join("state.json.bak");
            std::fs::write(&source, br#"{"value":2}"#).unwrap();
            std::fs::write(&destination, br#"{"value":1}"#).unwrap();
            std::fs::write(&backup, br#"{"older":true}"#).unwrap();
            let context = AtomicWriteContext {
                preparation: true,
                failures: vec![(step, 32)],
            };
            let error =
                copy_replace_for_encrypted_directory_with_context(&source, &destination, &context)
                    .unwrap_err();
            assert_eq!(
                error.io_error().and_then(std::io::Error::raw_os_error),
                Some(32),
                "{step}"
            );
            assert!(error.to_string().contains(&format!("io_substep={step}")));
            assert!(!error
                .to_string()
                .contains(&temporary.path().to_string_lossy().to_string()));
        }
    }

    #[cfg(windows)]
    #[test]
    fn preparation_encrypted_restore_failure_does_not_replace_primary_failure() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("state.json.tmp");
        let destination = temporary.path().join("state.json");
        std::fs::write(&source, br#"{"value":2}"#).unwrap();
        std::fs::write(&destination, br#"{"value":1}"#).unwrap();
        let context = AtomicWriteContext {
            preparation: true,
            failures: vec![("sync_state_replacement", 112), ("restore_state_backup", 5)],
        };
        let error =
            copy_replace_for_encrypted_directory_with_context(&source, &destination, &context)
                .unwrap_err();
        assert_eq!(
            error.io_error().and_then(std::io::Error::raw_os_error),
            Some(112)
        );
        let detail = error.to_string();
        assert!(detail.contains("io_substep=sync_state_replacement"));
        assert!(detail.contains("io_substep=restore_state_backup"));
        assert!(detail.contains("secondary_os_error=5"));
        assert!(destination.with_extension("json.bak").is_file());
    }

    #[tokio::test]
    async fn concurrent_json_writes_use_independent_temporary_files() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("project-index.json");
        let writes = (0..16).map(|value| {
            let path = path.clone();
            tokio::spawn(async move {
                atomic_write_json(&path, &serde_json::json!({"value": value})).await
            })
        });
        for write in writes {
            write.await.unwrap().unwrap();
        }

        let value: serde_json::Value =
            serde_json::from_slice(&tokio::fs::read(&path).await.unwrap()).unwrap();
        assert!(value["value"].as_u64().is_some_and(|value| value < 16));
        let leftovers = std::fs::read_dir(temporary.path())
            .unwrap()
            .filter_map(std::result::Result::ok)
            .filter(|entry| entry.path() != path)
            .collect::<Vec<_>>();
        assert!(
            leftovers.is_empty(),
            "leftover temporary files: {leftovers:?}"
        );
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
        assert!(paths.colmap_frames.is_dir());
    }

    #[tokio::test]
    async fn copies_image_sequences_with_stable_names() {
        let temporary = tempfile::tempdir().unwrap();
        let input = temporary.path().join("图片序列");
        std::fs::create_dir(&input).unwrap();
        image::RgbImage::new(2, 2)
            .save(input.join("image10.jpg"))
            .unwrap();
        image::RgbImage::new(2, 2)
            .save(input.join("image2.png"))
            .unwrap();
        let (paths, metadata) = ProjectManager::for_diagnostics(temporary.path().join("projects"))
            .create(&input, Quality::Balanced)
            .await
            .unwrap();
        assert_eq!(metadata.input_type, ProjectInputType::Images);
        assert!(metadata.source_path.is_dir());
        assert!(metadata.source_path.join("frame_000001.png").is_file());
        assert!(metadata.source_path.join("frame_000002.jpg").is_file());
        let origins: std::collections::BTreeMap<
            String,
            crate::video::image_projection::OriginalImageIdentity,
        > = serde_json::from_slice(
            &std::fs::read(paths.source.join("image-origins.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            origins["frame_000001.png"].path,
            std::fs::canonicalize(input.join("image2.png")).unwrap()
        );
        assert_eq!(
            origins["frame_000002.jpg"].size,
            std::fs::metadata(input.join("image10.jpg")).unwrap().len()
        );
        let state: PipelineStateFile =
            serde_json::from_slice(&tokio::fs::read(paths.state).await.unwrap()).unwrap();
        assert_eq!(state.input_type, ProjectInputType::Images);
        assert_eq!(state.image_sequence.unwrap().image_count, 2);
    }

    #[tokio::test]
    async fn reports_image_import_progress() {
        let temporary = tempfile::tempdir().unwrap();
        let input = temporary.path().join("images");
        std::fs::create_dir(&input).unwrap();
        image::RgbImage::new(2, 2)
            .save(input.join("image1.png"))
            .unwrap();
        image::RgbImage::new(2, 2)
            .save(input.join("image2.jpg"))
            .unwrap();
        let updates = Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured = updates.clone();
        let observer: ProjectImportObserver = Arc::new(move |progress| {
            captured.lock().unwrap().push(progress);
        });

        ProjectManager::for_diagnostics(temporary.path().join("projects"))
            .create_with_progress(&input, Quality::Balanced, Some(observer), None)
            .await
            .unwrap();

        let updates = updates.lock().unwrap();
        assert_eq!(
            updates.first().copied(),
            Some(ProjectImportProgress {
                current: 0,
                total: 2
            })
        );
        assert_eq!(
            updates.last().copied(),
            Some(ProjectImportProgress {
                current: 2,
                total: 2
            })
        );
    }

    #[tokio::test]
    async fn image_project_creation_honors_cancellation_without_registering_a_project() {
        let temporary = tempfile::tempdir().unwrap();
        let input = temporary.path().join("images");
        std::fs::create_dir(&input).unwrap();
        image::RgbImage::new(2, 2)
            .save(input.join("image1.png"))
            .unwrap();
        image::RgbImage::new(2, 2)
            .save(input.join("image2.png"))
            .unwrap();
        let projects = temporary.path().join("projects");
        let cancellation = CancellationToken::new();
        let cancellation_from_observer = cancellation.clone();
        let observer: ProjectImportObserver = Arc::new(move |progress| {
            if progress.current == 1 {
                cancellation_from_observer.cancel();
            }
        });

        let result = ProjectManager::for_diagnostics(projects.clone())
            .create_with_progress(
                &input,
                Quality::Balanced,
                Some(observer),
                Some(cancellation),
            )
            .await;

        assert!(matches!(result, Err(SplatError::Cancelled)));
        assert!(!projects.exists() || std::fs::read_dir(projects).unwrap().next().is_none());
    }
}
