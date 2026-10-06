//! Application-owned task registry. Transports never own a generation future.
pub mod logs;
#[cfg(test)]
mod tests;

use crate::{
    engines::EnginePaths,
    pipeline::{
        runner::{PipelineResult, PipelineRunner, ReshootInputType, RunnerUpdate},
        PipelineEvent, PipelineStage,
    },
    presets::{PipelineOptimizationConfig, Quality},
    project::{catalog, manager::atomic_write_json, ProjectStatus},
    telemetry::{PipelineTelemetrySession, TelemetryInputType, TelemetryRunKind, TelemetryService},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex as SyncMutex},
};
use tokio::sync::Mutex;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Created,
    Starting,
    Running,
    Cancelling,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}
impl TaskStatus {
    pub fn project_status(self) -> ProjectStatus {
        match self {
            Self::Completed => ProjectStatus::Completed,
            Self::Failed => ProjectStatus::Failed,
            Self::Cancelled => ProjectStatus::Cancelled,
            Self::Interrupted | Self::Created => ProjectStatus::Interrupted,
            _ => ProjectStatus::Running,
        }
    }
    pub fn active(self) -> bool {
        matches!(self, Self::Starting | Self::Running | Self::Cancelling)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskError {
    pub code: String,
    pub message: String,
    pub failed_stage: Option<PipelineStage>,
    pub engine: Option<crate::pipeline::PipelineEngine>,
    pub exit_code: Option<i32>,
    pub classification: Option<String>,
    pub classification_is_heuristic: bool,
    pub failure_id: Option<Uuid>,
    pub log_sources: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRun {
    pub run_id: Uuid,
    pub kind: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub logs: BTreeMap<String, logs::RegisteredLog>,
    #[serde(default)]
    pub error: Option<TaskError>,
    #[serde(default)]
    pub last_engine: Option<crate::pipeline::PipelineEngine>,
    #[serde(default)]
    pub last_exit_code: Option<i32>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRecord {
    pub schema_version: u32,
    pub task_id: Uuid,
    pub run_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub project_path: Option<PathBuf>,
    pub input_path: PathBuf,
    #[serde(default)]
    pub input_type: crate::project::ProjectInputType,
    #[serde(default)]
    pub project_deleted: bool,
    pub quality: Quality,
    pub source: Option<String>,
    #[serde(default)]
    pub configuration_inferred: bool,
    pub task_kind: String,
    pub source_project_id: Option<Uuid>,
    pub planner_enabled: bool,
    pub projects_root: PathBuf,
    pub configuration: PipelineOptimizationConfig,
    pub actual_configuration: Option<Value>,
    pub status: TaskStatus,
    pub stage: Option<PipelineStage>,
    pub revision: u64,
    pub sequence: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub elapsed_ms: u64,
    #[serde(default)]
    pub elapsed_offset_ms: u64,
    #[serde(default)]
    pub runtime: Option<crate::pipeline::runtime::RuntimeSnapshot>,
    pub progress: Option<f32>,
    pub estimated_progress: Option<f32>,
    pub current: Option<u64>,
    pub total: Option<u64>,
    pub unit: Option<String>,
    pub eta_seconds: Option<f64>,
    pub error: Option<TaskError>,
    pub result: Option<Value>,
    pub runs: Vec<TaskRun>,
    pub client_request_id: Option<String>,
    pub request_parameters: Option<Value>,
    #[serde(default)]
    pub recent_events: Vec<PipelineEvent>,
}
#[derive(Debug, Clone, Serialize)]
pub struct TaskUpdate {
    pub task: TaskRecord,
    pub event: Option<PipelineEvent>,
}
#[derive(Debug, Clone, Serialize)]
pub struct StartReceipt {
    pub accepted: bool,
    pub task_id: Uuid,
    pub run_id: Option<Uuid>,
    pub status: TaskStatus,
    pub revision: u64,
}
impl StartReceipt {
    fn existing(t: &TaskRecord) -> Self {
        Self {
            accepted: false,
            task_id: t.task_id,
            run_id: t.run_id,
            status: t.status,
            revision: t.revision,
        }
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct ServiceError {
    pub code: String,
    pub message: String,
    pub task_id: Option<Uuid>,
    pub run_id: Option<Uuid>,
}
impl ServiceError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            task_id: None,
            run_id: None,
        }
    }
}
impl std::fmt::Display for ServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for ServiceError {}
impl From<crate::error::SplatError> for ServiceError {
    fn from(e: crate::error::SplatError) -> Self {
        Self::new("TASK_ERROR", e.to_string())
    }
}
impl From<std::io::Error> for ServiceError {
    fn from(e: std::io::Error) -> Self {
        Self::new("IO_ERROR", e.to_string())
    }
}
impl From<serde_json::Error> for ServiceError {
    fn from(e: serde_json::Error) -> Self {
        Self::new("INVALID_DATA", e.to_string())
    }
}
pub type Result<T> = std::result::Result<T, ServiceError>;

#[derive(Default, Serialize, Deserialize)]
struct Registry {
    #[serde(default)]
    tasks: BTreeMap<Uuid, TaskRecord>,
    #[serde(skip)]
    loaded: bool,
}
#[cfg(test)]
pub(crate) type TestExecutor = Arc<
    dyn Fn(
            TaskRecord,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = crate::error::Result<PipelineResult>> + Send>,
        > + Send
        + Sync,
>;
type Emit = Arc<dyn Fn(TaskUpdate) + Send + Sync>;
#[derive(Clone)]
pub struct TaskService {
    registry: Arc<SyncMutex<Registry>>,
    gate: Arc<Mutex<()>>,
    root: PathBuf,
    pub(crate) active: Arc<Mutex<Option<Arc<PipelineRunner>>>>,
    pub(crate) diagnostics: Arc<crate::diagnostics::DiagnosticService>,
    emitter: Arc<SyncMutex<Option<Emit>>>,
    #[cfg(test)]
    pub(crate) executor: Arc<SyncMutex<Option<TestExecutor>>>,
}
impl Default for TaskService {
    fn default() -> Self {
        Self::at(catalog::app_data_root().unwrap_or_else(|_| PathBuf::from(".")))
    }
}
impl TaskService {
    pub fn at(root: PathBuf) -> Self {
        Self {
            registry: Arc::new(SyncMutex::new(Registry::default())),
            gate: Arc::new(Mutex::new(())),
            root,
            active: Arc::new(Mutex::new(None)),
            diagnostics: Arc::new(Default::default()),
            emitter: Arc::new(SyncMutex::new(None)),
            #[cfg(test)]
            executor: Arc::new(SyncMutex::new(None)),
        }
    }
    pub fn attach(&self, app: tauri::AppHandle) {
        use tauri::Emitter;
        *self.emitter.lock().unwrap_or_else(|p| p.into_inner()) = Some(Arc::new(move |update| {
            let _ = app.emit("task-update", &update);
            if let Some(event) = update.event {
                let _ = app.emit("pipeline-event", event);
            }
        }));
    }
    fn records(&self) -> std::sync::MutexGuard<'_, Registry> {
        self.registry.lock().unwrap_or_else(|p| p.into_inner())
    }
    fn notify(&self, mut t: TaskRecord, event: Option<PipelineEvent>) {
        t.recent_events.clear();
        if let Some(emit) = self
            .emitter
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
        {
            emit(TaskUpdate { task: t, event });
        }
    }
    async fn persist(&self) -> Result<()> {
        let snapshot = serde_json::to_value(&*self.records())?;
        tokio::fs::create_dir_all(&self.root).await?;
        atomic_write_json(&self.root.join("tasks.json"), &snapshot).await?;
        Ok(())
    }
    async fn load_locked(&self) -> Result<()> {
        if self.records().loaded {
            return Ok(());
        }
        let path = self.root.join("tasks.json");
        let mut registry = match tokio::fs::read(&path).await {
            Ok(bytes) => serde_json::from_slice::<Registry>(&bytes)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Registry::default(),
            Err(e) => return Err(e.into()),
        };
        for task in registry.tasks.values_mut() {
            if task.status.active() {
                task.status = TaskStatus::Interrupted;
                task.revision += 1;
                task.updated_at = Utc::now();
                task.eta_seconds = None;
                if let Some(run) = task.runs.last_mut() {
                    run.ended_at = None;
                }
                if let Some(project) = &task.project_path {
                    if let Ok(bytes) = tokio::fs::read(project.join("project.json")).await {
                        if let Ok(mut metadata) =
                            serde_json::from_slice::<crate::project::ProjectMetadata>(&bytes)
                        {
                            if catalog::project_is_durably_completed(project, &metadata).await {
                                task.status = TaskStatus::Completed;
                                if let Some(output) = &metadata.output {
                                    task.result = Some(serde_json::to_value(output)?);
                                }
                            } else {
                                metadata.status = ProjectStatus::Interrupted;
                                metadata.duration_ms =
                                    Some(task.elapsed_ms.max(metadata.duration_ms.unwrap_or(0)));
                                atomic_write_json(&project.join("project.json"), &metadata).await?;
                            }
                        }
                    }
                }
            }
        }
        registry.loaded = true;
        *self.records() = registry;
        self.persist().await
    }
    pub async fn initialize(&self) -> Result<()> {
        let _gate = self.gate.lock().await;
        self.load_locked().await
    }
    pub async fn get(&self, id: Uuid) -> Result<TaskRecord> {
        self.initialize().await?;
        self.records()
            .tasks
            .get(&id)
            .cloned()
            .ok_or_else(|| ServiceError::new("TASK_NOT_FOUND", "任务不存在"))
    }
    pub async fn all(&self) -> Result<Vec<TaskRecord>> {
        self.initialize().await?;
        let mut tasks: Vec<_> = self.records().tasks.values().cloned().collect();
        tasks.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| b.task_id.cmp(&a.task_id))
        });
        Ok(tasks)
    }
    pub async fn running(&self) -> Result<Option<TaskRecord>> {
        Ok(self.all().await?.into_iter().find(|t| t.status.active()))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create(
        &self,
        id: Option<Uuid>,
        input: PathBuf,
        quality: Quality,
        projects_root: PathBuf,
        planner_enabled: bool,
        source: &str,
        key: Option<String>,
        parameters: Option<Value>,
    ) -> Result<TaskRecord> {
        let service = self.clone();
        let source = source.to_owned();
        tokio::spawn(async move {
            service
                .create_inner(
                    id,
                    input,
                    quality,
                    projects_root,
                    planner_enabled,
                    &source,
                    key,
                    parameters,
                )
                .await
        })
        .await
        .map_err(|error| ServiceError::new("CREATE_FAILED", error.to_string()))?
    }

    #[allow(clippy::too_many_arguments)]
    async fn create_inner(
        &self,
        id: Option<Uuid>,
        input: PathBuf,
        quality: Quality,
        projects_root: PathBuf,
        planner_enabled: bool,
        source: &str,
        key: Option<String>,
        parameters: Option<Value>,
    ) -> Result<TaskRecord> {
        let _gate = self.gate.lock().await;
        self.load_locked().await?;
        if let Some(key) = &key {
            if key.is_empty() || key.len() > 256 {
                return Err(ServiceError::new(
                    "INVALID_ARGUMENT",
                    "client_request_id 必须为 1–256 字节",
                ));
            }
            if let Some(old) = self
                .records()
                .tasks
                .values()
                .find(|t| t.client_request_id.as_ref() == Some(key))
                .cloned()
            {
                return if old.request_parameters == parameters {
                    Ok(old)
                } else {
                    Err(ServiceError::new(
                        "IDEMPOTENCY_CONFLICT",
                        "相同 client_request_id 已用于不同参数",
                    ))
                };
            }
        }
        if let Some(id) = id {
            if let Some(t) = self.records().tasks.get(&id).cloned() {
                return Ok(t);
            }
        }
        let input = std::fs::canonicalize(input)
            .map_err(|_| ServiceError::new("INVALID_PATH", "本地输入不存在或不可访问"))?;
        if !input.is_dir()
            && !matches!(
                input
                    .extension()
                    .and_then(|x| x.to_str())
                    .map(str::to_ascii_lowercase)
                    .as_deref(),
                Some("mp4" | "mov")
            )
        {
            return Err(ServiceError::new(
                "INVALID_INPUT",
                "仅支持 MP4/MOV 或图片目录",
            ));
        }
        if !projects_root.is_absolute() {
            return Err(ServiceError::new(
                "INVALID_PATH",
                "项目根目录必须为绝对路径",
            ));
        }
        let now = Utc::now();
        let task = TaskRecord {
            schema_version: 1,
            task_id: id.unwrap_or_else(Uuid::new_v4),
            run_id: None,
            project_id: None,
            project_path: None,
            input_type: if input.is_dir() {
                crate::project::ProjectInputType::Images
            } else {
                crate::project::ProjectInputType::Video
            },
            project_deleted: false,
            input_path: input,
            quality,
            source: Some(source.into()),
            configuration_inferred: false,
            task_kind: "generation".into(),
            source_project_id: None,
            planner_enabled,
            projects_root,
            configuration: (*crate::presets::pipeline_optimization_config()).clone(),
            actual_configuration: None,
            status: TaskStatus::Created,
            stage: None,
            revision: 1,
            sequence: 0,
            created_at: now,
            updated_at: now,
            elapsed_ms: 0,
            elapsed_offset_ms: 0,
            runtime: None,
            progress: None,
            estimated_progress: None,
            current: None,
            total: None,
            unit: None,
            eta_seconds: None,
            error: None,
            result: None,
            runs: Vec::new(),
            client_request_id: key,
            request_parameters: parameters,
            recent_events: Vec::new(),
        };
        self.records().tasks.insert(task.task_id, task.clone());
        if let Err(e) = self.persist().await {
            self.records().tasks.remove(&task.task_id);
            return Err(e);
        }
        self.notify(task.clone(), None);
        Ok(task)
    }

    pub async fn import_projects(&self) -> Result<()> {
        self.initialize().await?;
        let overview = catalog::get_overview().await?;
        for summary in overview.projects {
            let _gate = self.gate.lock().await;
            if self
                .records()
                .tasks
                .values()
                .any(|t| t.project_id == Some(summary.id))
            {
                continue;
            }
            let (path, mut metadata) = match catalog::load_registered_project(summary.id).await {
                Ok(v) => v,
                Err(_) => continue,
            };
            let checkpoint: crate::project::PipelineStateFile =
                match tokio::fs::read(path.join("state.json"))
                    .await
                    .ok()
                    .and_then(|b| serde_json::from_slice(&b).ok())
                {
                    Some(s) => s,
                    None => continue,
                };
            let status = match summary.status {
                ProjectStatus::Completed => TaskStatus::Completed,
                ProjectStatus::Failed => TaskStatus::Failed,
                ProjectStatus::Cancelled => TaskStatus::Cancelled,
                _ => TaskStatus::Interrupted,
            };
            if status == TaskStatus::Interrupted && metadata.status == ProjectStatus::Running {
                metadata.status = ProjectStatus::Interrupted;
                atomic_write_json(&path.join("project.json"), &metadata).await?;
            }
            let mut registered = BTreeMap::new();
            if let Ok(mut entries) = tokio::fs::read_dir(path.join("logs")).await {
                while let Some(entry) = entries.next_entry().await? {
                    let file = entry.path();
                    if registered.len() >= 32 {
                        break;
                    }
                    if file.extension().is_some_and(|s| s == "log") {
                        if let Ok(mut log) = logs::register(&path.join("logs"), &file, 0) {
                            log.end = std::fs::metadata(&file).ok().map(|m| m.len());
                            registered.insert(log.source.clone(), log);
                        }
                    }
                }
            }
            let id = summary.workspace_task_id.unwrap_or_else(Uuid::new_v4);
            let run_id = Uuid::new_v4();
            let task = TaskRecord {
                schema_version: 1,
                task_id: id,
                run_id: Some(run_id),
                project_id: Some(metadata.id),
                project_path: Some(path.clone()),
                input_type: metadata.input_type,
                project_deleted: false,
                input_path: metadata.source_path.clone(),
                quality: metadata.quality,
                source: summary.workspace_task_id.map(|_| "gui".into()),
                configuration_inferred: checkpoint.configuration.is_none(),
                task_kind: if metadata.reshoot.is_some() {
                    "reshoot"
                } else {
                    "generation"
                }
                .into(),
                source_project_id: metadata.reshoot.as_ref().map(|r| r.source_project_id),
                planner_enabled: checkpoint.planner_enabled,
                projects_root: path.parent().unwrap_or(&path).into(),
                configuration: checkpoint
                    .configuration
                    .clone()
                    .unwrap_or_else(|| (*crate::presets::pipeline_optimization_config()).clone()),
                actual_configuration: Some(actual_snapshot(serde_json::to_value(&checkpoint)?)),
                status,
                stage: Some(checkpoint.stage),
                revision: 1,
                sequence: 0,
                created_at: metadata.created_at,
                updated_at: metadata.completed_at.unwrap_or(metadata.created_at),
                elapsed_ms: metadata.duration_ms.unwrap_or(0),
                elapsed_offset_ms: 0,
                runtime: None,
                progress: None,
                estimated_progress: None,
                current: None,
                total: None,
                unit: None,
                eta_seconds: None,
                error: metadata.failure_message.as_ref().map(|message| TaskError {
                    code: "pipeline_failed".into(),
                    message: message.clone(),
                    failed_stage: None,
                    engine: None,
                    exit_code: None,
                    classification: None,
                    classification_is_heuristic: false,
                    failure_id: None,
                    log_sources: registered.keys().cloned().collect(),
                }),
                result: metadata
                    .output
                    .as_ref()
                    .map(serde_json::to_value)
                    .transpose()?,
                runs: vec![TaskRun {
                    run_id,
                    kind: "legacy".into(),
                    started_at: metadata.started_at.unwrap_or(metadata.created_at),
                    ended_at: Some(metadata.completed_at.unwrap_or(Utc::now())),
                    logs: registered,
                    last_engine: None,
                    last_exit_code: None,
                    error: None,
                }],
                client_request_id: None,
                request_parameters: None,
                recent_events: vec![],
            };
            let task = {
                let mut registry = self.records();
                if let Some(existing) = registry.tasks.get_mut(&id) {
                    if existing.status.active() {
                        continue;
                    }
                    existing.project_id = task.project_id;
                    existing.project_path = task.project_path;
                    existing.status = task.status;
                    existing.stage = task.stage;
                    existing.actual_configuration = task.actual_configuration;
                    existing.result = task.result;
                    existing.revision += 1;
                    if let Some(run) = existing.runs.last_mut() {
                        for (source, log) in task.runs[0].logs.clone() {
                            run.logs.entry(source).or_insert(log);
                        }
                    }
                    existing.clone()
                } else {
                    registry.tasks.insert(id, task.clone());
                    task
                }
            };
            self.persist().await?;
            self.notify(task, None);
        }
        Ok(())
    }

    pub(crate) async fn capture_failure(
        &self,
        mut error: crate::commands::PipelineCommandError,
        paths: &[PathBuf],
    ) -> crate::commands::PipelineCommandError {
        if error.code != "cancelled" {
            let project = error
                .project_id
                .as_ref()
                .and_then(|id| Uuid::parse_str(id).ok());
            error.failure_id = Some(Box::new(
                self.diagnostics
                    .capture(
                        error.failed_stage,
                        error.engine,
                        error.failure_kind,
                        &error.message,
                        project,
                        paths,
                    )
                    .await,
            ));
        }
        error
    }

    /// Admission runs on the application's executor even if its caller disappears.
    pub async fn start(
        &self,
        id: Uuid,
        engines: EnginePaths,
        telemetry: Option<TelemetryService>,
    ) -> Result<StartReceipt> {
        let service = self.clone();
        tokio::spawn(async move { service.start_inner(id, engines, telemetry, false).await })
            .await
            .map_err(|e| ServiceError::new("START_FAILED", e.to_string()))?
    }
    async fn start_inner(
        &self,
        id: Uuid,
        engines: EnginePaths,
        telemetry: Option<TelemetryService>,
        resume: bool,
    ) -> Result<StartReceipt> {
        let _gate = self.gate.lock().await;
        self.load_locked().await?;
        let task = self
            .records()
            .tasks
            .get(&id)
            .cloned()
            .ok_or_else(|| ServiceError::new("TASK_NOT_FOUND", "任务不存在"))?;
        if task.status.active() || (!resume && task.status != TaskStatus::Created) {
            return Ok(StartReceipt::existing(&task));
        }
        if resume && task.status == TaskStatus::Completed {
            return Err(ServiceError::new(
                "TASK_NOT_STARTABLE",
                "已完成项目无需恢复",
            ));
        }
        if let Some(active) = self.records().tasks.values().find(|t| t.status.active()) {
            return Err(ServiceError {
                code: "TASK_BUSY".into(),
                message: "已有任务正在运行".into(),
                task_id: Some(active.task_id),
                run_id: active.run_id,
            });
        }
        let run_id = Uuid::new_v4();
        let service = self.clone();
        let lifecycle = self.clone();
        let session = telemetry.map(|telemetry| {
            Arc::new(PipelineTelemetrySession::new_with_planner_evaluation(
                telemetry,
                task.quality,
                if task.input_path.is_dir() {
                    TelemetryInputType::Images
                } else {
                    TelemetryInputType::Video
                },
                if task.project_id.is_some() {
                    TelemetryRunKind::Resume
                } else {
                    TelemetryRunKind::New
                },
                task.planner_enabled,
                Default::default(),
            ))
        });
        let observe = session.clone();
        let runner = Arc::new(
            PipelineRunner::new_with_planner(engines, task.planner_enabled, move |event| {
                if let Some(s) = &observe {
                    s.observe(&event);
                }
                service.on_event(id, run_id, event);
            })
            .with_workspace_task_id(Some(id))
            .with_input_boundary(
                (task.source.as_deref() == Some("mcp")).then(|| task.input_path.clone()),
            )
            .with_lifecycle_observer(move |update| lifecycle.on_runner_update(id, run_id, update)),
        );
        let starting = {
            let mut registry = self.records();
            let t = registry.tasks.get_mut(&id).expect("task");
            t.run_id = Some(run_id);
            t.status = TaskStatus::Starting;
            t.elapsed_offset_ms = t.elapsed_ms;
            t.progress = None;
            t.current = None;
            t.total = None;
            t.unit = None;
            t.eta_seconds = None;
            t.runtime = None;
            let previous_error = t.error.clone();
            if let Some(run) = t.runs.last_mut() {
                if run.error.is_none() {
                    run.error = previous_error;
                }
            }
            t.error = None;
            t.stage = None;
            t.estimated_progress = None;
            t.sequence = 0;
            t.recent_events.clear();
            t.revision += 1;
            t.updated_at = Utc::now();
            t.runs.push(TaskRun {
                run_id,
                kind: if task.project_id.is_some() {
                    "resume"
                } else {
                    &task.task_kind
                }
                .into(),
                started_at: Utc::now(),
                ended_at: None,
                logs: Default::default(),
                last_engine: None,
                last_exit_code: None,
                error: None,
            });
            t.clone()
        };
        if let Err(e) = self.persist().await {
            self.records().tasks.insert(id, task);
            *self.active.lock().await = None;
            return Err(e);
        }
        *self.active.lock().await = Some(runner.clone());
        self.notify(starting.clone(), None);
        let service = self.clone();
        tokio::spawn(async move {
            let _cleanup = ExecutionCleanup {
                service: service.clone(),
                task: id,
                run: run_id,
                runner: runner.clone(),
            };
            if let Some(s) = &session {
                s.generation_started();
            }
            let execute_runner = runner.clone();
            let execution = task.clone();
            #[cfg(test)]
            let test_executor = service.executor.lock().unwrap().clone();
            let worker = tokio::spawn(crate::presets::with_pipeline_config(
                task.configuration.clone(),
                async move {
                    #[cfg(test)]
                    if let Some(executor) = test_executor {
                        return executor(execution).await;
                    }
                    if let Some(project) = execution.project_id {
                        execute_runner.resume(project).await
                    } else if let Some(source) = execution.source_project_id {
                        execute_runner
                            .generate_incremental_reshoot(
                                source,
                                &execution.input_path,
                                if execution.input_path.is_dir() {
                                    ReshootInputType::Images
                                } else {
                                    ReshootInputType::Video
                                },
                                &execution.projects_root,
                            )
                            .await
                    } else {
                        execute_runner
                            .generate(
                                &execution.input_path,
                                execution.quality,
                                &execution.projects_root,
                            )
                            .await
                    }
                },
            ));
            let result = worker.await.unwrap_or_else(|e| {
                runner.cancel();
                Err(crate::error::SplatError::Process(format!(
                    "任务执行异常：{e}"
                )))
            });
            if let Some(s) = session {
                match &result {
                    Ok(r) => s.generation_completed(
                        r.duration_ms,
                        r.input_images,
                        r.source_duration_seconds,
                    ),
                    Err(e) => s.generation_failed(e),
                }
            }
            service.finish(id, run_id, runner, result).await;
        });
        let mut receipt = StartReceipt::existing(&starting);
        receipt.accepted = true;
        Ok(receipt)
    }
    fn on_event(&self, id: Uuid, run: Uuid, mut event: PipelineEvent) {
        let task = {
            let mut registry = self.records();
            let Some(t) = registry.tasks.get_mut(&id) else {
                return;
            };
            if t.run_id != Some(run) || !t.status.active() {
                return;
            }
            // The supervisor, after durable result/error capture, owns terminal transitions.
            if matches!(
                event.stage,
                PipelineStage::Completed | PipelineStage::Failed | PipelineStage::Cancelled
            ) {
                return;
            }
            if t.status == TaskStatus::Starting {
                t.status = TaskStatus::Running;
            }
            if t.stage != Some(event.stage) {
                t.progress = None;
                t.current = None;
                t.total = None;
                t.unit = None;
                t.eta_seconds = None;
                t.runtime = None;
            }
            t.revision += 1;
            t.sequence = event.sequence;
            t.stage = Some(event.stage);
            t.updated_at = event.timestamp;
            t.elapsed_ms = t.elapsed_offset_ms.saturating_add(event.elapsed_ms);
            t.estimated_progress = Some(t.estimated_progress.unwrap_or(0.0).max(event.progress));
            if event.current.is_some() {
                t.current = event.current;
                t.total = event.total;
                t.unit = event.unit.clone();
                t.progress = if !event.indeterminate {
                    event
                        .current
                        .zip(event.total)
                        .filter(|(_, total)| *total > 0)
                        .map(|(current, total)| (current as f32 / total as f32 * 100.0).min(100.0))
                } else {
                    None
                };
            }
            if let Some(runtime) = &event.runtime {
                t.runtime = Some(runtime.clone());
                t.eta_seconds = runtime
                    .training
                    .as_ref()
                    .and_then(|training| training.remaining_seconds);
            }
            event.task_id = Some(id);
            event.run_id = Some(run);
            event.revision = t.revision;
            if event.kind != crate::pipeline::EventKind::Runtime
                && event.kind != crate::pipeline::EventKind::Heartbeat
            {
                t.recent_events.push(event.clone());
                if t.recent_events.len() > 500 {
                    t.recent_events.remove(0);
                }
            }
            t.clone()
        };
        self.notify(task, Some(event));
    }
    fn on_runner_update(&self, id: Uuid, run: Uuid, update: RunnerUpdate) {
        let task = {
            let mut registry = self.records();
            let Some(t) = registry.tasks.get_mut(&id) else {
                return;
            };
            if t.run_id != Some(run) || !t.status.active() {
                return;
            }
            match update {
                RunnerUpdate::Configuration(configuration) => {
                    t.actual_configuration = Some(configuration);
                }
                RunnerUpdate::Project(metadata) => {
                    t.project_id = Some(metadata.id);
                    t.project_path = Some(metadata.project_path);
                }
                RunnerUpdate::LogOpened { path, offset } => {
                    if let Some(root) = &t.project_path {
                        if let Ok(log) = logs::register(&root.join("logs"), &path, offset) {
                            let logs = &mut t.runs.last_mut().expect("run").logs;
                            match logs.get(&log.source) {
                                Some(existing) if existing.file_id == log.file_id => {}
                                _ => {
                                    logs.insert(log.source.clone(), log);
                                }
                            }
                        }
                    }
                }
                RunnerUpdate::ProcessExited { engine, exit_code } => {
                    if let Some(r) = t.runs.last_mut() {
                        r.last_engine = Some(engine);
                        r.last_exit_code = exit_code;
                    }
                }
            }
            t.revision += 1;
            t.clone()
        };
        self.notify(task, None);
    }
    async fn finish(
        &self,
        id: Uuid,
        run: Uuid,
        runner: Arc<PipelineRunner>,
        result: crate::error::Result<PipelineResult>,
    ) {
        let _gate = self.gate.lock().await;
        let context = runner.failure_context();
        let input = self
            .records()
            .tasks
            .get(&id)
            .map(|t| t.input_path.clone())
            .unwrap_or_default();
        let error = match &result {
            Ok(_) => None,
            Err(e) => Some(
                self.capture_failure(
                    crate::commands::PipelineCommandError::from_runner(e, &runner),
                    &[input],
                )
                .await,
            ),
        };
        let failure_id = error
            .as_ref()
            .and_then(|e| e.failure_id.as_deref())
            .copied();
        let actual = None::<Value>;
        let task = {
            let mut registry = self.records();
            let Some(t) = registry.tasks.get_mut(&id) else {
                return;
            };
            if t.run_id != Some(run) {
                return;
            }
            t.status = match &result {
                Ok(_) => TaskStatus::Completed,
                Err(crate::error::SplatError::Cancelled) => TaskStatus::Cancelled,
                Err(_) => TaskStatus::Failed,
            };
            t.stage = Some(match t.status {
                TaskStatus::Completed => PipelineStage::Completed,
                TaskStatus::Cancelled => PipelineStage::Cancelled,
                _ => PipelineStage::Failed,
            });
            if let Some(r) = t.runs.last_mut() {
                r.ended_at = Some(Utc::now());
                for log in r.logs.values_mut() {
                    log.end = std::fs::metadata(&log.path).ok().map(|m| m.len());
                }
            }
            t.project_id = context.project_id.or(t.project_id);
            t.project_path = context.project_path.or(t.project_path.clone());
            if let Some(actual) = actual {
                t.actual_configuration = Some(actual_snapshot(actual));
            }
            t.result = result
                .as_ref()
                .ok()
                .and_then(|r| serde_json::to_value(r).ok());
            if let Some(e) = error {
                t.error = Some(TaskError {
                    code: e.code.into(),
                    message: e.message,
                    failed_stage: e.failed_stage,
                    engine: e.engine,
                    exit_code: t
                        .runs
                        .last()
                        .and_then(|r| r.last_exit_code)
                        .filter(|code| *code != 0),
                    classification: e.failure_kind.map(str::to_owned),
                    classification_is_heuristic: e.failure_kind.is_some(),
                    failure_id,
                    log_sources: t
                        .runs
                        .last()
                        .map(|r| r.logs.keys().cloned().collect())
                        .unwrap_or_default(),
                });
            }
            if let Some(run) = t.runs.last_mut() {
                run.error = t.error.clone();
            }
            t.updated_at = Utc::now();
            t.elapsed_ms = runner
                .elapsed_ms()
                .saturating_add(runner.elapsed_offset_ms().max(t.elapsed_offset_ms));
            t.eta_seconds = None;
            t.revision += 1;
            t.clone()
        };
        if let Err(e) = self.persist().await {
            tracing::error!(code=%e.code,"Could not persist task terminal state");
        }
        *self.active.lock().await = None;
        self.notify(task, None);
    }
    pub async fn cancel(&self, id: Uuid, run: Uuid) -> Result<TaskRecord> {
        let service = self.clone();
        tokio::spawn(async move {
            let _gate = service.gate.lock().await;
            service.load_locked().await?;
            let old = service
                .records()
                .tasks
                .get(&id)
                .cloned()
                .ok_or_else(|| ServiceError::new("TASK_NOT_FOUND", "任务不存在"))?;
            if old.run_id != Some(run) {
                return Err(ServiceError::new("STALE_RUN_ID", "取消请求不属于当前执行"));
            }
            if !old.status.active() {
                return Ok(old);
            }
            let task = {
                let mut registry = service.records();
                let t = registry.tasks.get_mut(&id).expect("task");
                t.status = TaskStatus::Cancelling;
                t.revision += 1;
                t.updated_at = Utc::now();
                t.clone()
            };
            if let Err(e) = service.persist().await {
                service.records().tasks.insert(id, old);
                return Err(e);
            }
            service.notify(task.clone(), None);
            if let Some(runner) = service.active.lock().await.as_ref() {
                runner.cancel();
            }
            Ok(task)
        })
        .await
        .map_err(|e| ServiceError::new("CANCEL_FAILED", e.to_string()))?
    }
    pub async fn resume(
        &self,
        project: Uuid,
        engines: EnginePaths,
        telemetry: Option<TelemetryService>,
    ) -> Result<StartReceipt> {
        self.import_projects().await?;
        let id = self
            .all()
            .await?
            .into_iter()
            .find(|t| t.project_id == Some(project))
            .ok_or_else(|| ServiceError::new("TASK_NOT_FOUND", "项目任务不存在"))?
            .task_id;
        let service = self.clone();
        tokio::spawn(async move { service.start_inner(id, engines, telemetry, true).await })
            .await
            .map_err(|e| ServiceError::new("START_FAILED", e.to_string()))?
    }

    pub async fn mark_reshoot(&self, id: Uuid, source: Uuid) -> Result<()> {
        let _gate = self.gate.lock().await;
        {
            let mut r = self.records();
            let t = r
                .tasks
                .get_mut(&id)
                .ok_or_else(|| ServiceError::new("TASK_NOT_FOUND", "任务不存在"))?;
            t.task_kind = "reshoot".into();
            t.source_project_id = Some(source);
        }
        self.persist().await
    }
    pub async fn wait(&self, id: Uuid) -> Result<TaskRecord> {
        loop {
            let task = self.get(id).await?;
            if !task.status.active() {
                return Ok(task);
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }
    pub async fn reconcile_projects(
        &self,
        projects: &[crate::project::catalog::ProjectSummary],
    ) -> Result<()> {
        let _gate = self.gate.lock().await;
        self.load_locked().await?;
        let mut changes = vec![];
        {
            let mut registry = self.records();
            for project in projects {
                if project.status != ProjectStatus::Interrupted {
                    continue;
                }
                if let Some(task) = registry.tasks.values_mut().find(|task| {
                    task.project_id == Some(project.id)
                        && task.status == TaskStatus::Completed
                        && !task.project_deleted
                }) {
                    task.status = TaskStatus::Interrupted;
                    task.result = None;
                    task.revision += 1;
                    task.updated_at = Utc::now();
                    task.error = Some(TaskError {
                        code: "ARTIFACT_UNAVAILABLE".into(),
                        message: project.failure_message.clone().unwrap_or_else(|| {
                            "项目产物当前不可访问，可以使用现有恢复入口检查并修复".into()
                        }),
                        failed_stage: None,
                        engine: None,
                        exit_code: None,
                        classification: None,
                        classification_is_heuristic: false,
                        failure_id: None,
                        log_sources: task
                            .runs
                            .last()
                            .map(|run| run.logs.keys().cloned().collect())
                            .unwrap_or_default(),
                    });
                    changes.push(task.clone());
                }
            }
        }
        if !changes.is_empty() {
            self.persist().await?;
            for task in changes {
                self.notify(task, None);
            }
        }
        Ok(())
    }
    pub async fn mark_project_deleted(&self, project: Uuid) -> Result<()> {
        let _gate = self.gate.lock().await;
        let task = {
            let mut registry = self.records();
            let Some(task) = registry
                .tasks
                .values_mut()
                .find(|task| task.project_id == Some(project))
            else {
                return Ok(());
            };
            task.project_deleted = true;
            task.project_path = None;
            task.result = None;
            task.revision += 1;
            task.updated_at = Utc::now();
            task.clone()
        };
        self.persist().await?;
        self.notify(task, None);
        Ok(())
    }
    pub fn spawn_checkpoint_writer(&self) -> tauri::async_runtime::JoinHandle<()> {
        let service = self.clone();
        // Tauri's synchronous setup hook runs on the UI thread, outside a
        // current Tokio runtime. Spawn on the application-owned runtime.
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                let _gate = service.gate.lock().await;
                if service.records().tasks.values().any(|t| t.status.active()) {
                    let _ = service.persist().await;
                }
            }
        })
    }
}

pub fn authorize_input(path: &Path, roots: &[PathBuf]) -> Result<PathBuf> {
    let path = std::fs::canonicalize(path)
        .map_err(|_| ServiceError::new("INVALID_PATH", "输入不存在或不可访问"))?;
    if !roots
        .iter()
        .filter_map(|r| {
            std::fs::canonicalize(r)
                .ok()
                .filter(|resolved| resolved == r)
        })
        .any(|r| path.starts_with(r))
    {
        return Err(ServiceError::new(
            "PATH_NOT_AUTHORIZED",
            "素材路径不在设置中授权的目录内",
        ));
    }
    Ok(path)
}

fn actual_snapshot(state: Value) -> Value {
    let frames = state.get("frames");
    serde_json::json!({"quality":state.get("preset"),"planner_enabled":state.get("plannerEnabled"),"resolution_policy_version":state.get("resolutionPolicyVersion"),"resolution_plan":state.get("resolutionPlan"),"brush_training":state.get("brushTraining"),"frame_plan":{"sampling_fps":frames.and_then(|f|f.get("samplingFps")),"estimated_frames":frames.and_then(|f|f.get("estimatedFrames")),"extracted_frames":frames.and_then(|f|f.get("extractedFrames")),"mask_count":frames.and_then(|f|f.get("maskCount"))}})
}

struct ExecutionCleanup {
    service: TaskService,
    task: Uuid,
    run: Uuid,
    runner: Arc<PipelineRunner>,
}
impl Drop for ExecutionCleanup {
    fn drop(&mut self) {
        self.runner.cancel();
        let service = self.service.clone();
        let runner = self.runner.clone();
        let task = self.task;
        let run = self.run;
        // Destructors can also run outside an entered Tokio runtime.
        tauri::async_runtime::spawn(async move {
            if service
                .get(task)
                .await
                .is_ok_and(|t| t.run_id == Some(run) && t.status.active())
            {
                service
                    .finish(
                        task,
                        run,
                        runner,
                        Err(crate::error::SplatError::Process(
                            "执行监督器异常结束".into(),
                        )),
                    )
                    .await;
            }
        });
    }
}

pub(crate) fn checkpoint_snapshot(state: &crate::project::PipelineStateFile) -> Value {
    serde_json::json!({"quality":state.preset,"planner_enabled":state.planner_enabled,"resolution_policy_version":state.resolution_policy_version,"resolution_plan":state.resolution_plan,"brush_training":state.brush_training,"frame_plan":{"sampling_fps":state.frames.as_ref().map(|f|f.sampling_fps),"estimated_frames":state.frames.as_ref().map(|f|f.estimated_frames),"extracted_frames":state.frames.as_ref().and_then(|f|f.extracted_frames),"mask_count":state.frames.as_ref().and_then(|f|f.mask_count)}})
}
