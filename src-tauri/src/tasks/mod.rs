//! Application-owned task registry. Transports never own a generation future.
pub mod cli_guard;
pub mod logs;
mod notifications;
#[cfg(test)]
mod tests;

pub use notifications::TaskView;

use crate::{
    engines::EnginePaths,
    pipeline::{
        effectiveness::PlannerEffectivenessTracker,
        retry_preparation::{
            PreparationContext, PreparationDiagnosticContext, RetryPreparationOperation,
        },
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

    pub fn can_continue(self) -> bool {
        matches!(self, Self::Failed | Self::Cancelled | Self::Interrupted)
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classification_version: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub termination: Option<crate::process::ProcessTermination>,
    pub failure_id: Option<Uuid>,
    pub log_sources: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRun {
    pub run_id: Uuid,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continued_from_status: Option<TaskStatus>,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub logs: BTreeMap<String, logs::RegisteredLog>,
    #[serde(default)]
    pub error: Option<TaskError>,
    #[serde(default)]
    pub last_engine: Option<crate::pipeline::PipelineEngine>,
    #[serde(default)]
    pub last_exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub brush_executions: Vec<crate::pipeline::brush_execution::BrushExecutionSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_brush_execution: Option<usize>,
}

impl TaskRun {
    fn observe_brush(
        &mut self,
        event: &PipelineEvent,
        configuration: Option<crate::presets::ResolvedBrushTrainingPreset>,
    ) {
        if event.kind == crate::pipeline::EventKind::Stage
            && !matches!(
                event.stage,
                PipelineStage::TrainingSplats | PipelineStage::Exporting
            )
        {
            self.current_brush_execution = None;
        }
        if event.engine != Some(crate::pipeline::PipelineEngine::Brush) {
            return;
        }
        let Some(runtime) = event.runtime.as_ref().filter(|v| v.process_id != 0) else {
            return;
        };
        if let Some(last) = self.brush_executions.last_mut() {
            if last.process_id == runtime.process_id
                && last.operation.as_ref().map(|v| v.id) == event.operation.as_ref().map(|v| v.id)
            {
                last.update(runtime);
                return;
            }
        }
        // Only the initial snapshot can create an execution. Delayed samples may not.
        if runtime.phase != "starting" {
            return;
        }
        self.brush_executions.push(
            crate::pipeline::brush_execution::BrushExecutionSnapshot::new(
                runtime,
                event.operation.clone(),
            ),
        );
        self.brush_executions
            .last_mut()
            .expect("new execution")
            .configuration = configuration;
        self.current_brush_execution = Some(self.brush_executions.len() - 1);
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ba_policy_version: Option<u32>,
    /// None on tasks created before common Alpha execution semantics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brush_execution_options: Option<crate::project::metadata::BrushExecutionOptions>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creation_intent: Option<crate::project::creation::ProjectCreationIntent>,
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
    #[serde(default)]
    pub generation_policy: Option<crate::pipeline::generation_policy::GenerationPolicy>,
    #[serde(default)]
    pub automatically_retried: bool,
    pub projects_root: PathBuf,
    pub configuration: PipelineOptimizationConfig,
    pub actual_configuration: Option<Value>,
    pub status: TaskStatus,
    pub stage: Option<PipelineStage>,
    #[serde(default)]
    pub substage: Option<String>,
    #[serde(default)]
    pub operation: Option<crate::pipeline::event::PipelineOperation>,
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
    pub task: TaskView,
    pub events: Vec<PipelineEvent>,
    pub dropped_event_count: u64,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<crate::error::InputAdmissionReason>,
    pub message: String,
    pub task_id: Option<Uuid>,
    pub run_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}
impl ServiceError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            reason: None,
            message: message.into(),
            task_id: None,
            run_id: None,
            details: None,
        }
    }

    fn task_busy(active: &TaskRecord) -> Self {
        Self {
            code: "TASK_BUSY".into(),
            reason: None,
            message: "已有任务正在运行".into(),
            task_id: Some(active.task_id),
            run_id: active.run_id,
            details: None,
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
        if let crate::error::SplatError::InputAdmission(reason) = &e {
            let mut error = Self::new("UNSUPPORTED_INPUT", e.to_string());
            error.reason = Some(*reason);
            return error;
        }
        if let crate::error::SplatError::Process(message) = &e {
            for code in ["INPUT_CHANGED", "CONFIRMATION_REQUIRED"] {
                if message.starts_with(code) {
                    return Self::new(code, message.clone());
                }
            }
        }
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

pub(crate) fn generation_parameters(parameters: Option<Value>) -> Result<Value> {
    let mut parameters = parameters.unwrap_or_else(|| serde_json::json!({}));
    if !parameters.is_object() {
        return Err(ServiceError::new(
            "INVALID_ARGUMENT",
            "Invalid generation parameters",
        ));
    }
    let policy: crate::pipeline::generation_policy::GenerationPolicy = parameters
        .get("generationPolicy")
        .filter(|v| !v.is_null())
        .cloned()
        .map(serde_json::from_value)
        .transpose()?
        .unwrap_or_else(|| crate::pipeline::generation_policy::GenerationPolicy {
            auto_retry_enabled: parameters
                .pointer("/reshootOptions/allowFallback")
                .and_then(Value::as_bool)
                .unwrap_or(true),
            ..Default::default()
        });
    policy.validate()?;
    // This function admits new tasks. Every continuation retains its saved
    // policy and must not be silently migrated to today's defaults.
    let policy = crate::pipeline::generation_policy::GenerationPolicy {
        version: crate::pipeline::generation_policy::VERSION,
        ..policy
    };
    parameters["generationPolicy"] = serde_json::to_value(policy)?;
    if parameters.get("reshootOptions").is_some_and(Value::is_null) {
        parameters.as_object_mut().unwrap().remove("reshootOptions");
    }
    if let Some(options) = parameters
        .get_mut("reshootOptions")
        .and_then(Value::as_object_mut)
    {
        options.insert(
            "allowFallback".into(),
            Value::Bool(policy.auto_retry_enabled),
        );
    }
    Ok(parameters)
}

fn sync_saved_generation_policy(
    parameters: &mut Option<Value>,
    policy: Option<crate::pipeline::generation_policy::GenerationPolicy>,
) -> Result<()> {
    if parameters.is_none() && policy.is_none() {
        return Ok(());
    }
    let value = parameters.get_or_insert_with(|| serde_json::json!({}));
    let object = value
        .as_object_mut()
        .ok_or_else(|| ServiceError::new("INVALID_DATA", "Invalid saved generation parameters"))?;
    if let Some(policy) = policy {
        object.insert("generationPolicy".into(), serde_json::to_value(policy)?);
    } else {
        object.remove("generationPolicy");
    }
    Ok(())
}

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
            Arc<PipelineRunner>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = crate::error::Result<PipelineResult>> + Send>,
        > + Send
        + Sync,
>;
type Emit = Arc<dyn Fn(TaskUpdate) + Send + Sync>;
#[derive(Clone)]
pub struct TaskService {
    invocation_access: Option<crate::pipeline::access::InvocationAccess>,
    registry: Arc<SyncMutex<Registry>>,
    gate: Arc<Mutex<()>>,
    root: PathBuf,
    run_lock_path: PathBuf,
    run_lock: Arc<SyncMutex<Option<(Uuid, std::fs::File)>>>,
    pub(crate) active: Arc<Mutex<Option<Arc<PipelineRunner>>>>,
    pub(crate) diagnostics: Arc<crate::diagnostics::DiagnosticService>,
    emitter: Arc<SyncMutex<Option<Emit>>>,
    notifications: Arc<SyncMutex<notifications::PendingNotifications>>,
    #[cfg(test)]
    pub(crate) executor: Arc<SyncMutex<Option<TestExecutor>>>,
    #[cfg(test)]
    continuation_index_path: Option<PathBuf>,
}
impl Default for TaskService {
    fn default() -> Self {
        Self::at(catalog::app_data_root().unwrap_or_else(|_| PathBuf::from(".")))
    }
}
impl TaskService {
    pub fn desktop_execution_lock() -> crate::error::Result<PathBuf> {
        Ok(catalog::app_data_root()?.join("generation.lock"))
    }
    pub fn at(root: PathBuf) -> Self {
        Self {
            invocation_access: None,
            run_lock_path: root.join("generation.lock"),
            run_lock: Arc::new(SyncMutex::new(None)),
            registry: Arc::new(SyncMutex::new(Registry::default())),
            gate: Arc::new(Mutex::new(())),
            root,
            active: Arc::new(Mutex::new(None)),
            diagnostics: Arc::new(Default::default()),
            emitter: Arc::new(SyncMutex::new(None)),
            notifications: Arc::new(SyncMutex::new(Default::default())),
            #[cfg(test)]
            executor: Arc::new(SyncMutex::new(None)),
            #[cfg(test)]
            continuation_index_path: None,
        }
    }
    /// CLI uses its own registry, but shares the desktop's OS-owned execution lock.
    pub fn with_run_lock(mut self, path: PathBuf) -> Self {
        self.run_lock_path = path;
        self
    }
    /// A clone shares task ownership, but receives the current invocation's
    /// permissions. Never persist or infer this grant from `TaskRecord.source`.
    pub fn with_invocation_access(
        mut self,
        access: crate::pipeline::access::InvocationAccess,
    ) -> Self {
        self.invocation_access = Some(access);
        self
    }
    pub fn execution_settled(&self, run: Option<Uuid>) -> bool {
        !self
            .run_lock
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .is_some_and(|(active, _)| Some(*active) == run)
    }
    pub fn attach(&self, app: tauri::AppHandle) {
        use tauri::Emitter;
        *self.emitter.lock().unwrap_or_else(|p| p.into_inner()) = Some(Arc::new(move |update| {
            let _ = app.emit("task-update", &update);
        }));
    }
    fn records(&self) -> std::sync::MutexGuard<'_, Registry> {
        self.registry.lock().unwrap_or_else(|p| p.into_inner())
    }
    fn emitter(&self) -> Option<Emit> {
        self.emitter
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// Lifecycle callers hold `gate` through persistence and notification.
    /// The notification mutex serializes emission with synchronous runner events.
    fn notify(&self, id: Uuid) {
        let Some(emit) = self.emitter() else {
            return;
        };
        let mut pending = self.notifications.lock().unwrap_or_else(|p| p.into_inner());
        self.emit_pending(&mut pending, id, &emit, true);
    }

    fn emit_pending(
        &self,
        pending: &mut notifications::PendingNotifications,
        id: Uuid,
        emit: &Emit,
        immediate: bool,
    ) {
        let queued = pending.tasks.remove(&id);
        if queued.is_none() && !immediate {
            return;
        }
        let task = {
            let registry = self.records();
            let Some(task) = registry.tasks.get(&id) else {
                return;
            };
            // Only lifecycle code may announce a non-active status, after persistence.
            if !immediate && !task.status.active() {
                return;
            }
            TaskView::from(task)
        };
        let queued = queued.filter(|queued| queued.run_id == task.run_id);
        let dropped_event_count = queued
            .as_ref()
            .map_or(0, |queued| queued.dropped_event_count);
        let events = queued
            .map(|queued| queued.into_events())
            .unwrap_or_default()
            .into_iter()
            .filter(|event| {
                event.task_id == Some(id)
                    && event.run_id == task.run_id
                    && event.revision <= task.revision
            })
            .collect();
        emit(TaskUpdate {
            task,
            events,
            dropped_event_count,
        });
    }

    fn flush_notifications(&self) {
        // Do not wait for a start/cancel/finish checkpoint, or announce its state early.
        let Ok(_gate) = self.gate.try_lock() else {
            return;
        };
        let Some(emit) = self.emitter() else {
            return;
        };
        let mut pending = self.notifications.lock().unwrap_or_else(|p| p.into_inner());
        let ids: Vec<_> = pending.tasks.keys().copied().collect();
        for id in ids {
            self.emit_pending(&mut pending, id, &emit, false);
        }
    }

    pub fn spawn_notification_pump(&self) -> tauri::async_runtime::JoinHandle<()> {
        let service = self.clone();
        tauri::async_runtime::spawn(async move {
            // Construct the timer inside Tauri's runtime, never in the setup hook.
            let mut timer = tokio::time::interval_at(
                tokio::time::Instant::now() + notifications::NOTIFICATION_INTERVAL,
                notifications::NOTIFICATION_INTERVAL,
            );
            timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                timer.tick().await;
                service.flush_notifications();
            }
        })
    }
    async fn persist(&self) -> Result<()> {
        let snapshot = serde_json::to_value(&*self.records())?;
        tokio::fs::create_dir_all(&self.root).await?;
        atomic_write_json(&self.root.join("tasks.json"), &snapshot).await?;
        Ok(())
    }

    async fn reconcile_task_publication(task: &mut TaskRecord) -> Result<Option<(PathBuf, Uuid)>> {
        let Some(project_root) = task.project_path.clone() else {
            return Ok(None);
        };
        let transaction_path = project_root
            .join("work")
            .join(crate::pipeline::publication::TRANSACTION_FILE_NAME);
        let Some(publication) =
            crate::pipeline::publication::committed_publication(&project_root, &transaction_path)
                .await?
        else {
            return Ok(None);
        };
        if publication.transaction.phase
            == crate::pipeline::publication::PublicationPhase::Reconciled
            && task.status == TaskStatus::Completed
            && task
                .result
                .as_ref()
                .and_then(|value| {
                    serde_json::from_value::<crate::pipeline::runner::PipelineResult>(value.clone())
                        .ok()
                })
                .is_some_and(|result| {
                    result.final_ply == publication.output.final_ply
                        && result.splat_count == publication.output.splat_count
                        && result.file_size == publication.output.file_size
                })
        {
            // The publication coordinator has already proved all three local
            // views durable and the task contains the full public result.
            // Avoid rewriting it and incrementing the revision on every app
            // startup.
            return Ok(None);
        }
        let metadata_path = project_root.join("project.json");
        let mut metadata: crate::project::ProjectMetadata =
            serde_json::from_slice(&tokio::fs::read(&metadata_path).await?)?;
        let state_path = project_root.join("state.json");
        let mut state: crate::project::PipelineStateFile = match tokio::fs::read(&state_path).await
        {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                crate::project::PipelineStateFile::created_for(
                    metadata.quality,
                    metadata.input_type,
                )
            }
            Err(error) => return Err(error.into()),
        };
        let completed_at = publication
            .transaction
            .committed_at
            .unwrap_or_else(Utc::now);
        metadata.status = ProjectStatus::Completed;
        metadata.project_path = project_root.clone();
        metadata.output_path = Some(publication.output.final_ply.clone());
        metadata.output = Some(publication.output.clone());
        metadata.completed_at = Some(completed_at);
        metadata.failure_message = None;
        metadata.duration_ms = Some(metadata.duration_ms.unwrap_or(0).max(task.elapsed_ms));
        state.brush_complete = true;
        state.stage = PipelineStage::Completed;
        atomic_write_json(&metadata_path, &metadata).await?;
        atomic_write_json(&state_path, &state).await?;
        crate::pipeline::publication::mark_project_views_reconciled(
            &project_root,
            &transaction_path,
            publication.transaction.transaction_id,
        )
        .await?;

        let result = crate::pipeline::runner::pipeline_result_from_publication(
            &project_root,
            &metadata,
            &state,
            &publication,
        );
        task.status = TaskStatus::Completed;
        task.stage = Some(PipelineStage::Completed);
        task.project_id = Some(metadata.id);
        task.project_path = Some(project_root.clone());
        task.result = Some(serde_json::to_value(result)?);
        task.error = None;
        task.progress = Some(100.0);
        task.estimated_progress = Some(100.0);
        task.eta_seconds = None;
        task.updated_at = completed_at;
        if let Some(run) = task.runs.last_mut() {
            run.ended_at.get_or_insert(completed_at);
            run.error = None;
        }
        task.revision += 1;
        Ok(Some((project_root, publication.transaction.transaction_id)))
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
        let mut reconciled_publications = Vec::new();
        for task in registry.tasks.values_mut() {
            if let Some(publication) = Self::reconcile_task_publication(task).await? {
                reconciled_publications.push(publication);
                continue;
            }
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
        self.persist().await?;
        for (project_root, transaction_id) in reconciled_publications {
            let transaction_path = project_root
                .join("work")
                .join(crate::pipeline::publication::TRANSACTION_FILE_NAME);
            if let Err(error) = crate::pipeline::publication::mark_reconciled(
                &project_root,
                &transaction_path,
                transaction_id,
            )
            .await
            {
                // Project and task views are already durable. Leaving the
                // transaction at ProjectViewsReconciled makes the final mark
                // retryable on the next startup without reverting completion.
                tracing::warn!(%error, "Could not finalize publication task view");
            }
        }
        Ok(())
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
        let mut parameters = generation_parameters(parameters)?;
        if let Some(value) = parameters.get_mut("inputs") {
            let inputs =
                serde_json::from_value::<crate::pipeline::inputs::GenerationInputs>(value.clone())?
                    .canonicalized()?;
            *value = serde_json::to_value(inputs)?;
        }
        let parameters = Some(parameters);
        let _legacy_planner_enabled = planner_enabled;
        let planner_enabled = true;
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
        if let Some(value) = parameters.as_ref().and_then(|p| p.get("inputs")) {
            let inputs =
                serde_json::from_value::<crate::pipeline::inputs::GenerationInputs>(value.clone())?;
            crate::pipeline::input_admission::ensure_new_inputs_supported(&inputs)?;
        } else if crate::pipeline::input_admission::path_contains_colmap_input(&input)? {
            crate::pipeline::input_admission::ensure_colmap_import_enabled()?;
        }
        let input = std::fs::canonicalize(input)
            .map_err(|_| ServiceError::new("INVALID_PATH", "本地输入不存在或不可访问"))?;
        let inputs = parameters
            .as_ref()
            .and_then(|p| p.get("inputs"))
            .cloned()
            .map(serde_json::from_value::<crate::pipeline::inputs::GenerationInputs>)
            .transpose()?;
        if let Some(inputs) = &inputs {
            inputs.check_identities()?;
        }
        if inputs.is_none()
            && !input.is_dir()
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
        // Admit a supplemental task atomically with its frozen source identity.
        // A concurrent repeated MCP request must never observe it as generation.
        let source_project_id = parameters
            .as_ref()
            .filter(|p| p.get("reshootOptions").is_some())
            .and_then(|p| p.get("source_project_id"))
            .cloned()
            .map(serde_json::from_value::<Uuid>)
            .transpose()?;
        let now = Utc::now();
        let task = TaskRecord {
            ba_policy_version: Some(crate::pipeline::ba_policy::VERSION),
            brush_execution_options: Some(Default::default()),
            creation_intent: None,
            schema_version: 1,
            task_id: id.unwrap_or_else(Uuid::new_v4),
            run_id: None,
            project_id: None,
            project_path: None,
            input_type: if inputs.as_ref().is_some_and(|i|!matches!(i,crate::pipeline::inputs::GenerationInputs::Materials{items} if items.len()==1&&matches!(items[0].source,crate::pipeline::inputs::InputSource::Video{..}))) || input.is_dir() {
                crate::project::ProjectInputType::Images
            } else {
                crate::project::ProjectInputType::Video
            },
            project_deleted: false,
            input_path: input,
            quality,
            source: Some(source.into()),
            configuration_inferred: false,
            task_kind: if source_project_id.is_some() {
                "reshoot"
            } else {
                "generation"
            }
            .into(),
            source_project_id,
            planner_enabled,
            projects_root,
            configuration: (*crate::presets::pipeline_optimization_config()).clone(),
            generation_policy: parameters
                .as_ref()
                .and_then(|p| p.get("generationPolicy"))
                .cloned()
                .map(serde_json::from_value)
                .transpose()?,
            automatically_retried: false,
            actual_configuration: None,
            status: TaskStatus::Created,
            stage: None,
            substage: None,
            operation: None,
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
        self.notify(task.task_id);
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
            let mut result = metadata
                .output
                .as_ref()
                .map(serde_json::to_value)
                .transpose()?;
            if let (Some(inputs), Some(result)) = (checkpoint.inputs.as_ref(), result.as_mut()) {
                let mut unused = inputs.unused.clone();
                for source in &inputs.source_adoption {
                    if !source.adopted && !unused.contains(&source.source_id) {
                        unused.push(source.source_id.clone());
                    }
                }
                if !unused.is_empty() {
                    let labels = unused
                        .iter()
                        .map(|source_id| match &inputs.request {
                            crate::pipeline::inputs::GenerationInputs::Materials { items } => items
                                .iter()
                                .find(|item| &item.id == source_id)
                                .and_then(|item| item.source.path().file_name())
                                .map(|name| name.to_string_lossy().into_owned())
                                .unwrap_or_else(|| source_id.clone()),
                            _ => source_id.clone(),
                        })
                        .collect::<Vec<_>>();
                    if let Some(object) = result.as_object_mut() {
                        object.insert(
                            "warning".into(),
                            serde_json::Value::String(format!(
                                "部分素材未能参与生成：{}。本次仅使用已连接到同一场景的素材。",
                                labels.join("、")
                            )),
                        );
                    }
                }
            }
            if let (Some(inputs), Some(result)) = (checkpoint.inputs.as_ref(), result.as_mut()) {
                if let Some(warning) = crate::pipeline::inputs::source_adoption_warning(inputs) {
                    result["warning"] = Value::String(warning);
                }
            }
            let task = TaskRecord {
                ba_policy_version: checkpoint.ba_policy_version,
                brush_execution_options: checkpoint.brush_training.execution_options,
                creation_intent: checkpoint.creation.as_ref().map(|c| c.intent.clone()),
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
                generation_policy: checkpoint.generation_policy,
                automatically_retried: checkpoint.automatically_retried(),
                projects_root: path.parent().unwrap_or(&path).into(),
                configuration: checkpoint
                    .configuration
                    .clone()
                    .unwrap_or_else(|| (*crate::presets::pipeline_optimization_config()).clone()),
                actual_configuration: Some(actual_snapshot(serde_json::to_value(&checkpoint)?)),
                status,
                stage: Some(checkpoint.stage),
                substage: None,
                operation: None,
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
                    classification_version: None,
                    termination: None,
                    failure_id: None,
                    log_sources: registered.keys().cloned().collect(),
                }),
                result,
                runs: vec![TaskRun {
                    run_id,
                    kind: "legacy".into(),
                    continued_from_status: None,
                    started_at: metadata.started_at.unwrap_or(metadata.created_at),
                    ended_at: Some(metadata.completed_at.unwrap_or(Utc::now())),
                    logs: registered,
                    last_engine: None,
                    last_exit_code: None,
                    error: None,
                    brush_executions: Vec::new(),
                    current_brush_execution: None,
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
                    existing.generation_policy = task.generation_policy;
                    // A continuation retains controlled retries already spent
                    // on this work order, even across process/run boundaries.
                    existing.automatically_retried |= task.automatically_retried;
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
            self.notify(task.task_id);
        }
        Ok(())
    }

    pub(crate) async fn capture_failure(
        &self,
        error: crate::commands::PipelineCommandError,
        paths: &[PathBuf],
    ) -> crate::commands::PipelineCommandError {
        self.capture_failure_with_executions(error, paths, &[], None)
            .await
    }

    async fn capture_failure_with_executions(
        &self,
        mut error: crate::commands::PipelineCommandError,
        paths: &[PathBuf],
        executions: &[crate::pipeline::brush_execution::BrushExecutionSnapshot],
        current: Option<usize>,
    ) -> crate::commands::PipelineCommandError {
        if error.code != "cancelled" {
            let project = error
                .project_id
                .as_ref()
                .and_then(|id| Uuid::parse_str(id).ok());
            error.failure_id = Some(Box::new(
                self.diagnostics
                    .capture_with_executions(
                        error.failed_stage,
                        error.engine,
                        error.failure_kind,
                        &error.message,
                        project,
                        paths,
                        executions,
                        current,
                    )
                    .await,
            ));
        }
        error
    }

    pub(crate) async fn capture_preparation_failure(
        &self,
        mut error: crate::commands::PipelineCommandError,
        context: &PreparationDiagnosticContext,
        paths: &[PathBuf],
    ) -> crate::commands::PipelineCommandError {
        if error.code != "cancelled" {
            error.failed_stage = None;
            error.engine = Some(crate::pipeline::PipelineEngine::System);
            error.project_id = context.project_id.map(|id| id.to_string().into_boxed_str());
            error.failure_id = Some(Box::new(
                self.diagnostics
                    .capture_preparation(&error.message, context, paths)
                    .await,
            ));
            let mut private_paths = paths.to_vec();
            private_paths.extend(context.project_root.iter().cloned());
            error.message = context.redacted_message(
                &error.message,
                &crate::diagnostics::redact::private_values(&private_paths),
            );
        }
        error
    }

    async fn preparation_service_error(
        &self,
        error: crate::error::SplatError,
        context: &PreparationDiagnosticContext,
        task_id: Option<Uuid>,
        run_id: Option<Uuid>,
        paths: &[PathBuf],
    ) -> ServiceError {
        // Preserve the typed primary error until classification and diagnostic
        // capture finish. JSON is not turned into a synthetic I/O failure.
        let code = if error.io_error().is_some() {
            "IO_ERROR"
        } else if matches!(&error, crate::error::SplatError::Json(_)) {
            "INVALID_DATA"
        } else {
            "TASK_ERROR"
        };
        let command = self
            .capture_preparation_failure(
                crate::commands::PipelineCommandError::from(error),
                context,
                paths,
            )
            .await;
        let mut safe_command = serde_json::to_value(&command).unwrap_or_else(|_| {
            serde_json::json!({"code":command.code,"message":&command.message,
                "failureId":&command.failure_id})
        });
        if let Some(object) = safe_command.as_object_mut() {
            object.remove("projectPath");
            object.remove("logsDirectory");
        }
        ServiceError {
            code: code.into(),
            reason: None,
            message: command.message,
            task_id,
            run_id,
            details: Some(serde_json::json!({
                "pipelineError":safe_command,"preparation":context.safe_details(),
            })),
        }
    }

    async fn preflight_catalog_continuation(
        &self,
        project: Uuid,
        expected_run: Option<Uuid>,
    ) -> Result<()> {
        let context = PreparationContext::new(Some(project), None);
        #[cfg(test)]
        let lookup = async {
            if let Some(index) = &self.continuation_index_path {
                catalog::registered_project_path_from(index, project).await
            } else {
                catalog::registered_project_path(project).await
            }
        };
        #[cfg(not(test))]
        let lookup = catalog::registered_project_path(project);
        let root = context
            .step(
                RetryPreparationOperation::ReadRetryProject,
                "project_index",
                None,
                lookup,
            )
            .await;
        let root = match root {
            Ok(root) => root,
            Err(crate::error::SplatError::Process(message))
                if message == "项目索引中不存在该项目" =>
            {
                return Err(ServiceError::new("TASK_NOT_FOUND", "项目任务不存在"));
            }
            Err(error) => {
                return Err(self
                    .preparation_service_error(error, &context.snapshot(), None, expected_run, &[])
                    .await);
            }
        };
        let context = context.with_project_root(root.clone());
        let metadata = context
            .step(
                RetryPreparationOperation::ReadRetryProject,
                "project_metadata",
                None,
                catalog::read_project_metadata_at(&root, project),
            )
            .await;
        let metadata = match metadata {
            Ok(metadata) => metadata,
            Err(error) => {
                return Err(self
                    .preparation_service_error(
                        error,
                        &context.snapshot(),
                        None,
                        expected_run,
                        std::slice::from_ref(&root),
                    )
                    .await);
            }
        };
        if let Err(error) = context
            .step(
                RetryPreparationOperation::ReadRetryState,
                "project_state",
                None,
                async {
                    let bytes = tokio::fs::read(root.join("state.json")).await?;
                    serde_json::from_slice::<Box<crate::project::PipelineStateFile>>(&bytes)
                        .map_err(crate::error::SplatError::from)
                },
            )
            .await
        {
            return Err(self
                .preparation_service_error(
                    error,
                    &context.snapshot(),
                    metadata.workspace_task_id,
                    expected_run,
                    std::slice::from_ref(&root),
                )
                .await);
        }
        Ok(())
    }

    /// Admission runs on the application's executor even if its caller disappears.
    pub async fn start(
        &self,
        id: Uuid,
        engines: EnginePaths,
        telemetry: Option<TelemetryService>,
    ) -> Result<StartReceipt> {
        let service = self.clone();
        tokio::spawn(async move {
            service
                .start_inner(id, engines, telemetry, false, None)
                .await
        })
        .await
        .map_err(|e| ServiceError::new("START_FAILED", e.to_string()))?
    }
    pub async fn retry_failed(
        &self,
        id: Uuid,
        failed_run: Uuid,
        engines: EnginePaths,
        telemetry: Option<TelemetryService>,
    ) -> Result<StartReceipt> {
        // Compatibility name only: failure, cancellation and interruption all
        // continue the saved work order without resetting recipes or allowances.
        self.continue_identified(id, failed_run, engines, telemetry)
            .await
    }

    async fn start_inner(
        &self,
        id: Uuid,
        engines: EnginePaths,
        telemetry: Option<TelemetryService>,
        continuing: bool,
        expected_run: Option<Uuid>,
    ) -> Result<StartReceipt> {
        let _gate = self.gate.lock().await;
        self.load_locked().await?;
        let mut task = self
            .records()
            .tasks
            .get(&id)
            .cloned()
            .ok_or_else(|| ServiceError::new("TASK_NOT_FOUND", "任务不存在"))?;
        let original_task = task.clone();
        if let Some(expected) = expected_run {
            if task.run_id != Some(expected) {
                return Err(ServiceError::new(
                    "STALE_RUN_ID",
                    "The task execution has changed",
                ));
            }
        }
        if continuing && !task.status.can_continue() {
            return Err(ServiceError::new(
                "TASK_NOT_RESUMABLE",
                "Only failed, cancelled, or interrupted tasks may be continued",
            ));
        }
        if task.status.active() || (!continuing && task.status != TaskStatus::Created) {
            return Ok(StartReceipt::existing(&task));
        }
        // An execution conflict is not evidence that this stopped task's files
        // are unreadable. Reject it before touching project preparation data.
        if let Some(active) = self.records().tasks.values().find(|t| t.status.active()) {
            return Err(ServiceError::task_busy(active));
        }
        let mut continuation_state: Option<Box<crate::project::PipelineStateFile>> = None;
        if let Some(project_id) = task.project_id.filter(|_| continuing) {
            // The project state is authoritative for all continuations. Read
            // failures must not silently fall back to a stale task/default recipe.
            let root = task.project_path.as_ref().ok_or_else(|| {
                ServiceError::new("INVALID_DATA", "The saved project location is missing")
            })?;
            let preparation = PreparationContext::new(Some(project_id), Some(root.clone()));
            let saved = preparation
                .step(
                    RetryPreparationOperation::ReadRetryState,
                    "project_state",
                    None,
                    async {
                        let bytes = tokio::fs::read(root.join("state.json")).await?;
                        Ok(serde_json::from_slice::<
                            Box<crate::project::PipelineStateFile>,
                        >(&bytes)?)
                    },
                )
                .await;
            let saved = match saved {
                Ok(saved) => saved,
                Err(error) => {
                    return Err(self
                        .preparation_service_error(
                            error,
                            &preparation.snapshot(),
                            Some(task.task_id),
                            task.run_id,
                            std::slice::from_ref(&task.input_path),
                        )
                        .await);
                }
            };
            if let Err(error) = preparation
                .step(
                    RetryPreparationOperation::ReadRetryProject,
                    "project_metadata",
                    None,
                    catalog::read_project_metadata_at(root, project_id),
                )
                .await
            {
                return Err(self
                    .preparation_service_error(
                        error,
                        &preparation.snapshot(),
                        Some(task.task_id),
                        task.run_id,
                        std::slice::from_ref(&task.input_path),
                    )
                    .await);
            }
            task.quality = saved.preset;
            task.ba_policy_version = saved.ba_policy_version;
            task.planner_enabled = saved.planner_enabled;
            if let Some(configuration) = saved.configuration.as_ref() {
                task.configuration = configuration.clone();
            }
            task.generation_policy = saved.generation_policy;
            sync_saved_generation_policy(&mut task.request_parameters, saved.generation_policy)?;
            task.automatically_retried |= saved.automatically_retried();
            task.actual_configuration = Some(actual_snapshot(serde_json::to_value(&saved)?));
            continuation_state = Some(saved);
        }
        if !continuing || task.project_id.is_none() {
            if task
                .request_parameters
                .as_ref()
                .and_then(|p| p.get("inputs"))
                .is_none()
            {
                if let Some(access) = &self.invocation_access {
                    if task.input_path.is_dir() {
                        access.authorize_image_directory(&task.input_path)?;
                    } else {
                        access.authorize(&task.input_path)?;
                    }
                }
            }
            if let Some(inputs) = task
                .request_parameters
                .as_ref()
                .and_then(|p| p.get("inputs"))
            {
                let inputs: crate::pipeline::inputs::GenerationInputs =
                    serde_json::from_value(inputs.clone())?;
                if let Some(access) = &self.invocation_access {
                    access.authorize_inputs(&inputs)?;
                }
                let policy = task.generation_policy.unwrap_or_default();
                crate::pipeline::inputs::validate(&engines, &inputs, task.quality, policy).await?;
            }
            if task.source.as_deref() == Some("mcp") {
                if let (Some(source), Some(expected)) = (
                    task.source_project_id,
                    task.request_parameters
                        .as_ref()
                        .and_then(|p| p.pointer("/reshootOptions/sourceIdentity"))
                        .and_then(Value::as_str),
                ) {
                    let current = PipelineRunner::new(engines.clone(), |_| {})
                        .with_invocation_access(self.invocation_access.clone())
                        .inspect_reshoot_source_v2(source)
                        .await
                        .map_err(|e| ServiceError::new("INPUT_CHANGED", e.to_string()))?;
                    if current.source_identity != expected {
                        return Err(ServiceError::new(
                            "INPUT_CHANGED",
                            "Reshoot source changed; inspect and confirm again",
                        ));
                    }
                }
            }
            if let Some(expected) = task
                .request_parameters
                .as_ref()
                .and_then(|value| value.get("videoSourceIdentity"))
                .and_then(Value::as_str)
            {
                if let Some(access) = &self.invocation_access {
                    access.authorize(&task.input_path)?;
                }
                let actual = crate::video::panorama::source_identity(&task.input_path)
                    .map_err(|e| ServiceError::new("INPUT_CHANGED", e.to_string()))?;
                if actual != expected {
                    return Err(ServiceError::new(
                        "INPUT_CHANGED",
                        "素材文件已发生变化，请重新分析后再生成",
                    ));
                }
            }
        }
        if !continuing && task.generation_policy.is_none() {
            task.planner_enabled = true;
            task.configuration = (*crate::presets::pipeline_optimization_config()).clone();
            task.request_parameters = Some(generation_parameters(task.request_parameters.take())?);
        }
        let run_id = Uuid::new_v4();
        if let Some(parent) = self.run_lock_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let run_lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&self.run_lock_path)?;
        run_lock.try_lock().map_err(|error| {
            ServiceError::new(
                "TASK_BUSY",
                format!("Generation execution is locked: {error}"),
            )
        })?;
        if task.project_id.is_none() {
            let reserved = crate::project::creation::ProjectCreationIntent::reserve(&task, run_id)?;
            if let Some(previous) = &task.creation_intent {
                if previous.request_identity != reserved.request_identity
                    || previous.task_id != task.task_id
                {
                    return Err(ServiceError::new(
                        "INVALID_DATA",
                        "Project creation intent does not match the saved request",
                    ));
                }
            } else {
                task.creation_intent = Some(reserved);
            }
        }
        let service = self.clone();
        let lifecycle = self.clone();
        let ready_service = self.clone();
        let effectiveness = PlannerEffectivenessTracker::default();
        let summary_supported = if let Some(value) = task
            .request_parameters
            .as_ref()
            .and_then(|p| p.get("inputs"))
        {
            match serde_json::from_value::<crate::pipeline::inputs::GenerationInputs>(value.clone())
            {
                Ok(crate::pipeline::inputs::GenerationInputs::Materials { items })
                    if items.len() == 1 =>
                {
                    matches!(
                        items[0].source,
                        crate::pipeline::inputs::InputSource::Video { .. }
                    ) || if continuing && task.project_path.is_some() {
                        // Statistics must not reopen originals before the work
                        // order has decided whether any external read is needed.
                        continuation_state
                            .as_ref()
                            .and_then(|saved| saved.inputs.as_ref())
                            .is_some_and(|c| {
                                c.sources.len() == 1 && c.sources[0].dimensions.len() == 1
                            })
                    } else {
                        crate::pipeline::inputs::inspect_material(
                            &engines,
                            &items[0],
                            task.quality,
                            task.generation_policy.unwrap_or_default(),
                        )
                        .await
                        .is_ok_and(|a| a.dimensions.len() == 1)
                    }
                }
                _ => false,
            }
        } else {
            true
        };
        let session = telemetry.filter(|_| summary_supported).map(|telemetry| {
            Arc::new(PipelineTelemetrySession::new_with_planner_evaluation(
                telemetry,
                task.quality,
                if task.input_type == crate::project::ProjectInputType::Images {
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
                effectiveness.clone(),
            ))
        });
        let observe = session.clone();
        let reshoot_options = task
            .request_parameters
            .as_ref()
            .and_then(|v| v.get("reshootOptions"))
            .map(|v| serde_json::from_value::<crate::pipeline::reshoot::ReshootOptions>(v.clone()))
            .transpose()
            .map_err(|_| ServiceError::new("INVALID_ARGUMENT", "Invalid saved reshoot options"))?;
        let policy = task.generation_policy.or(task
            .request_parameters
            .as_ref()
            .and_then(|p| p.get("generationPolicy"))
            .cloned()
            .map(serde_json::from_value::<crate::pipeline::generation_policy::GenerationPolicy>)
            .transpose()?);
        let runner = Arc::new(
            PipelineRunner::new_with_planner(engines, task.planner_enabled, move |event| {
                if let Some(s) = &observe {
                    s.observe(&event);
                }
                service.on_event(id, run_id, event);
            })
            .with_effectiveness_tracker(effectiveness)
            .with_invocation_access(self.invocation_access.clone())
            .with_generation_policy(if !continuing || task.project_id.is_none() {
                policy
            } else {
                None
            })
            .with_video_projection(
                task.request_parameters
                    .as_ref()
                    .and_then(|v| v.get("videoProjection"))
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default(),
            )
            .with_workspace_task_id(Some(id))
            .with_creation_intent(task.creation_intent.clone())
            .with_project_ready_observer(move |metadata| {
                let service = ready_service.clone();
                Box::pin(async move {
                    service
                        .bind_created_project(id, run_id, metadata)
                        .await
                        .map_err(|e| crate::error::SplatError::Process(e.to_string()))
                })
            })
            .with_execution_run_id(Some(run_id))
            .with_inputs(
                task.request_parameters
                    .as_ref()
                    .and_then(|v| v.get("inputs"))
                    .cloned()
                    .map(serde_json::from_value)
                    .transpose()?,
            )
            .with_reshoot_options(reshoot_options)
            .with_image_matching_strategy(
                task.request_parameters
                    .as_ref()
                    .and_then(|v| v.get("imageMatchingStrategy"))
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default(),
            )
            .with_input_boundary(
                (task.source.as_deref() == Some("mcp")).then(|| task.input_path.clone()),
            )
            .with_lifecycle_observer(move |update| lifecycle.on_runner_update(id, run_id, update)),
        );
        let starting = {
            let mut registry = self.records();
            let t = registry.tasks.get_mut(&id).expect("task");
            t.quality = task.quality;
            t.ba_policy_version = task.ba_policy_version;
            t.creation_intent = task.creation_intent.clone();
            t.planner_enabled = task.planner_enabled;
            t.configuration = task.configuration.clone();
            t.request_parameters = task.request_parameters.clone();
            t.generation_policy = policy;
            t.actual_configuration = task.actual_configuration.clone();
            if !continuing {
                t.automatically_retried = false;
            } else {
                t.automatically_retried = task.automatically_retried;
            }
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
            t.substage = None;
            t.operation = None;
            t.estimated_progress = None;
            t.sequence = 0;
            t.recent_events.clear();
            t.revision += 1;
            t.updated_at = Utc::now();
            t.runs.push(TaskRun {
                run_id,
                kind: if continuing {
                    "continue"
                } else {
                    &task.task_kind
                }
                .into(),
                continued_from_status: continuing.then_some(task.status),
                started_at: Utc::now(),
                ended_at: None,
                logs: Default::default(),
                last_engine: None,
                last_exit_code: None,
                error: None,
                brush_executions: Vec::new(),
                current_brush_execution: None,
            });
            t.clone()
        };
        if let Err(e) = self.persist().await {
            self.records().tasks.insert(id, original_task);
            *self.active.lock().await = None;
            return Err(e);
        }
        *self.active.lock().await = Some(runner.clone());
        *self.run_lock.lock().unwrap_or_else(|p| p.into_inner()) = Some((run_id, run_lock));
        self.notify(id);
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
            let worker = tokio::spawn(Box::pin(crate::presets::with_pipeline_config(
                task.configuration.clone(),
                async move {
                    #[cfg(test)]
                    if let Some(executor) = test_executor {
                        return executor(execution, execute_runner).await;
                    }
                    if let Some(project) = execution.project_id {
                        Box::pin(execute_runner.resume(project)).await
                    } else if let Some(source) = execution.source_project_id {
                        Box::pin(execute_runner.generate_incremental_reshoot(
                            source,
                            &execution.input_path,
                            if execution.input_path.is_dir() {
                                ReshootInputType::Images
                            } else {
                                ReshootInputType::Video
                            },
                            &execution.projects_root,
                            execution.quality,
                        ))
                        .await
                    } else {
                        Box::pin(execute_runner.generate(
                            &execution.input_path,
                            execution.quality,
                            &execution.projects_root,
                        ))
                        .await
                    }
                },
            )));
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
    async fn bind_created_project(
        &self,
        id: Uuid,
        run: Uuid,
        metadata: crate::project::ProjectMetadata,
    ) -> Result<()> {
        let _gate = self.gate.lock().await;
        {
            let mut records = self.records();
            let task = records
                .tasks
                .get_mut(&id)
                .ok_or_else(|| ServiceError::new("TASK_NOT_FOUND", "Task not found"))?;
            if task.run_id != Some(run) || !task.status.active() {
                return Err(ServiceError::new(
                    "STALE_RUN_ID",
                    "The task execution has changed",
                ));
            }
            if metadata.workspace_task_id != Some(id)
                || task.creation_intent.as_ref().is_some_and(|intent| {
                    intent.project_id != metadata.id || intent.project_path != metadata.project_path
                })
            {
                return Err(ServiceError::new(
                    "INVALID_DATA",
                    "Project creation binding mismatch",
                ));
            }
            task.project_id = Some(metadata.id);
            task.project_path = Some(metadata.project_path);
            task.revision += 1;
        }
        self.persist().await?;
        self.notify(id);
        Ok(())
    }

    fn on_event(&self, id: Uuid, run: Uuid, mut event: PipelineEvent) {
        // A single lock orders record updates, queued events, and emitted snapshots.
        // In particular, an old run cannot enqueue after a new run's notification.
        let mut pending = self.notifications.lock().unwrap_or_else(|p| p.into_inner());
        let immediate = {
            let mut registry = self.records();
            let Some(t) = registry.tasks.get_mut(&id) else {
                return;
            };
            if t.run_id != Some(run) || !t.status.active() {
                return;
            }
            if event.sequence > 0 && event.sequence <= t.sequence {
                return;
            }
            // The supervisor, after durable result/error capture, owns terminal transitions.
            if matches!(
                event.stage,
                PipelineStage::Completed | PipelineStage::Failed | PipelineStage::Cancelled
            ) {
                return;
            }
            let starting = t.status == TaskStatus::Starting;
            let lifecycle = event.kind == crate::pipeline::EventKind::Stage;
            if event
                .operation
                .as_ref()
                .zip(t.operation.as_ref())
                .is_some_and(|(incoming, current)| incoming.id < current.id)
                || (!lifecycle
                    && event
                        .operation
                        .as_ref()
                        .zip(t.operation.as_ref())
                        .is_some_and(|(incoming, current)| incoming.id != current.id))
            {
                return;
            }
            let stage_changed = lifecycle
                && (t.stage != Some(event.stage)
                    || t.substage != event.substage
                    || event.operation.as_ref().map(|op| op.id)
                        != t.operation.as_ref().map(|op| op.id));
            if starting {
                t.status = TaskStatus::Running;
            }
            if stage_changed {
                t.progress = None;
                t.current = None;
                t.total = None;
                t.unit = None;
                t.eta_seconds = None;
                t.runtime = None;
            }
            let configuration = t
                .actual_configuration
                .as_ref()
                .and_then(|v| v.pointer("/brush_training/resolved"))
                .and_then(|v| serde_json::from_value(v.clone()).ok());
            if let Some(execution) = t.runs.last_mut() {
                execution.observe_brush(&event, configuration);
            }
            t.revision += 1;
            t.sequence = t.sequence.max(event.sequence);
            if lifecycle {
                t.stage = Some(event.stage);
                t.substage = event.substage.clone();
                t.operation = event.operation.clone();
            }
            t.updated_at = event.timestamp;
            t.elapsed_ms = t.elapsed_offset_ms.saturating_add(event.elapsed_ms);
            t.estimated_progress = Some(
                t.estimated_progress
                    .unwrap_or(0.0)
                    .max(event.progress.min(99.9)),
            );
            let progress_event = matches!(
                event.kind,
                crate::pipeline::EventKind::Stage | crate::pipeline::EventKind::Progress
            ) && t.stage == Some(event.stage);
            if progress_event {
                t.current = event.current;
                t.total = event.total;
                t.unit = event.unit.clone();
                t.progress = if event.indeterminate {
                    None
                } else {
                    event.stage_progress
                };
            }
            if let Some(runtime) = event
                .runtime
                .as_ref()
                .filter(|_| t.stage == Some(event.stage))
            {
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
            starting || stage_changed || event.kind == crate::pipeline::EventKind::Capability
        };
        pending.task(id, Some(run)).push(event);
        if immediate {
            // Runner callbacks are synchronous. A lifecycle operation in progress
            // will flush this batch itself after its checkpoint becomes durable.
            if let Ok(_gate) = self.gate.try_lock() {
                if let Some(emit) = self.emitter() {
                    self.emit_pending(&mut pending, id, &emit, false);
                }
            }
        }
    }
    fn on_runner_update(&self, id: Uuid, run: Uuid, update: RunnerUpdate) {
        let mut pending = self.notifications.lock().unwrap_or_else(|p| p.into_inner());
        let immediate = matches!(&update, RunnerUpdate::Project(_));
        {
            let mut registry = self.records();
            let Some(t) = registry.tasks.get_mut(&id) else {
                return;
            };
            if t.run_id != Some(run) || !t.status.active() {
                return;
            }
            match update {
                RunnerUpdate::BrushPreparation => {
                    if let Some(execution) = t.runs.last_mut() {
                        execution.current_brush_execution = None;
                    }
                }
                RunnerUpdate::AutomaticRetry => {
                    t.automatically_retried = true;
                }
                RunnerUpdate::Configuration(configuration) => {
                    t.generation_policy = configuration
                        .get("generation_policy")
                        .filter(|p| !p.is_null())
                        .cloned()
                        .and_then(|v| serde_json::from_value(v).ok());
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
                        if engine == crate::pipeline::PipelineEngine::Brush {
                            if let Some(execution) = r
                                .brush_executions
                                .iter_mut()
                                .rev()
                                .find(|v| v.ended_at.is_none())
                            {
                                execution.finish(exit_code);
                            }
                        }
                    }
                }
            }
            t.revision += 1;
        }
        pending.task(id, Some(run));
        if immediate {
            if let Ok(_gate) = self.gate.try_lock() {
                if let Some(emit) = self.emitter() {
                    self.emit_pending(&mut pending, id, &emit, false);
                }
            }
        }
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
        let (executions, current_execution) = {
            let mut registry = self.records();
            let Some(task) = registry
                .tasks
                .get_mut(&id)
                .filter(|t| t.run_id == Some(run))
            else {
                return;
            };
            // Cancellation and launch failures do not necessarily emit ProcessExited.
            // Freeze the local evidence before preparing a diagnostic or terminal record.
            task.runs
                .last_mut()
                .map(|v| {
                    for execution in &mut v.brush_executions {
                        execution.finish(None);
                    }
                    (v.brush_executions.clone(), v.current_brush_execution)
                })
                .unwrap_or_default()
        };
        let input = self
            .records()
            .tasks
            .get(&id)
            .map(|t| t.input_path.clone())
            .unwrap_or_default();
        let error = match &result {
            Ok(_) => None,
            Err(e) => {
                let error = crate::commands::PipelineCommandError::from_runner(e, &runner);
                let preparation = runner.preparation_diagnostic_context().filter(|context| {
                    error.failed_stage.is_none()
                        && context.safe_details().get("operation").is_some()
                });
                Some(if let Some(preparation) = preparation {
                    self.capture_preparation_failure(error, &preparation, &[input])
                        .await
                } else if executions.is_empty() {
                    self.capture_failure(error, &[input]).await
                } else {
                    self.capture_failure_with_executions(
                        error,
                        &[input],
                        &executions,
                        current_execution,
                    )
                    .await
                })
            }
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
                if let Some(actual) = &t.actual_configuration {
                    t.generation_policy = actual
                        .get("generation_policy")
                        .filter(|p| !p.is_null())
                        .cloned()
                        .and_then(|v| serde_json::from_value(v).ok());
                    t.automatically_retried = actual
                        .get("automatically_retried")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                }
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
                    exit_code: e.termination.as_ref().and_then(|status| status.exit_code),
                    classification: e.failure_kind.map(str::to_owned),
                    classification_is_heuristic: e.classification_is_heuristic,
                    classification_version: e.classification_version,
                    termination: e.termination.map(|status| *status),
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
            if t.status == TaskStatus::Completed {
                t.estimated_progress = Some(100.0);
                t.progress = Some(100.0);
            }
            t.revision += 1;
            t.clone()
        };
        let terminal_persisted = match self.persist().await {
            Ok(()) => true,
            Err(e) => {
                tracing::error!(code=%e.code,"Could not persist task terminal state");
                false
            }
        };
        if terminal_persisted && task.status == TaskStatus::Completed {
            if let Some(project_root) = task.project_path.as_ref() {
                let transaction_path = project_root
                    .join("work")
                    .join(crate::pipeline::publication::TRANSACTION_FILE_NAME);
                match crate::pipeline::publication::committed_publication(
                    project_root,
                    &transaction_path,
                )
                .await
                {
                    Ok(Some(publication)) => {
                        if let Err(error) = crate::pipeline::publication::mark_reconciled(
                            project_root,
                            &transaction_path,
                            publication.transaction.transaction_id,
                        )
                        .await
                        {
                            tracing::warn!(%error, "Could not finalize publication task view");
                        }
                    }
                    Ok(None) => {}
                    Err(error) => {
                        tracing::warn!(%error, "Could not verify committed publication after task persistence");
                    }
                }
            }
        }
        *self.active.lock().await = None;
        let mut lock = self.run_lock.lock().unwrap_or_else(|p| p.into_inner());
        if lock.as_ref().is_some_and(|(id, _)| *id == run) {
            lock.take();
        }
        drop(lock);
        self.notify(task.task_id);
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
            service.notify(id);
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
        let tasks = self.all().await?;
        let mut task = tasks
            .iter()
            .find(|t| t.project_id == Some(project))
            .cloned();
        if task.is_none() {
            if let Some(active) = tasks.iter().find(|task| task.status.active()) {
                return Err(ServiceError::task_busy(active));
            }
            self.preflight_catalog_continuation(project, None).await?;
            self.import_projects().await?;
            task = self
                .all()
                .await?
                .into_iter()
                .find(|t| t.project_id == Some(project));
        }
        let task = task.ok_or_else(|| ServiceError::new("TASK_NOT_FOUND", "项目任务不存在"))?;
        let expected_run = task.run_id.ok_or_else(|| {
            ServiceError::new(
                "INVALID_DATA",
                "The saved task execution identity is missing",
            )
        })?;
        self.continue_identified(task.task_id, expected_run, engines, telemetry)
            .await
    }

    /// Continue the saved work order associated with a project.
    ///
    /// GUI project actions use the same run identity guard as MCP actions. The
    /// legacy project-only entry point remains available for old callers that
    /// do not have a task/run identity.
    pub async fn resume_project_identified(
        &self,
        project: Uuid,
        expected_run: Uuid,
        engines: EnginePaths,
        telemetry: Option<TelemetryService>,
    ) -> Result<StartReceipt> {
        // A GUI action already carries the workspace task/run identity. Prefer
        // that durable local record and consult the project catalog only when
        // this registry does not know the project yet. This also preserves the
        // stale-run response when an unrelated catalog entry is unreadable.
        let tasks = self.all().await?;
        let mut task = tasks
            .iter()
            .find(|task| task.project_id == Some(project))
            .cloned();
        if task.is_none() {
            if let Some(active) = tasks.iter().find(|task| task.status.active()) {
                return Err(ServiceError::task_busy(active));
            }
            self.preflight_catalog_continuation(project, Some(expected_run))
                .await?;
            self.import_projects().await?;
            task = self
                .all()
                .await?
                .into_iter()
                .find(|task| task.project_id == Some(project));
        }
        let task = task.ok_or_else(|| ServiceError::new("TASK_NOT_FOUND", "项目任务不存在"))?;
        self.continue_identified(task.task_id, expected_run, engines, telemetry)
            .await
    }

    pub async fn resume_identified(
        &self,
        id: Uuid,
        expected_run: Uuid,
        engines: EnginePaths,
        telemetry: Option<TelemetryService>,
    ) -> Result<StartReceipt> {
        self.continue_identified(id, expected_run, engines, telemetry)
            .await
    }

    /// Every stopped task continues from the same persisted work order. A new
    /// run isolates process evidence, not recipe versions or retry allowances.
    pub async fn continue_identified(
        &self,
        id: Uuid,
        expected_run: Uuid,
        engines: EnginePaths,
        telemetry: Option<TelemetryService>,
    ) -> Result<StartReceipt> {
        let service = self.clone();
        tokio::spawn(async move {
            service
                .start_inner(id, engines, telemetry, true, Some(expected_run))
                .await
        })
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
                        classification_version: None,
                        termination: None,
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
                self.notify(task.task_id);
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
        self.notify(task.task_id);
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
    let retried = state.get("generationPolicy").is_some_and(Value::is_object)
        && (state
            .pointer("/brushTraining/oomRetryStarted")
            .and_then(Value::as_bool)
            == Some(true)
            || [
                "/reshoot/recipe/reconstructionFallbacks",
                "/reshoot/recipe/trainingFallbacks",
            ]
            .iter()
            .any(|p| {
                state
                    .pointer(p)
                    .and_then(Value::as_u64)
                    .is_some_and(|n| n > 0)
            }));
    let mut result = serde_json::json!({"quality":state.get("preset"),"planner_enabled":state.get("plannerEnabled"),"resolution_policy_version":state.get("resolutionPolicyVersion"),"resolution_plan":state.get("resolutionPlan"),"brush_training":state.get("brushTraining"),"frame_plan":{"sampling_fps":frames.and_then(|f|f.get("samplingFps")),"estimated_frames":frames.and_then(|f|f.get("estimatedFrames")),"extracted_frames":frames.and_then(|f|f.get("extractedFrames")),"mask_count":frames.and_then(|f|f.get("maskCount"))}});
    result["generation_policy"] = state
        .get("generationPolicy")
        .cloned()
        .unwrap_or(Value::Null);
    result["automatically_retried"] = Value::Bool(retried);
    result["reshoot_recipe"] = state
        .pointer("/reshoot/recipe")
        .cloned()
        .unwrap_or(Value::Null);
    extend_actual_summary(&mut result, &state);
    result
}

fn extend_actual_summary(result: &mut Value, state: &Value) {
    result["inputs"] = state.get("inputs").cloned().unwrap_or(Value::Null);
    if let Some(inputs) = result.get_mut("inputs").and_then(Value::as_object_mut) {
        // Recovery instructions and temporary patch audit records are local
        // implementation details, not bounded GUI/MCP status summaries.
        inputs.remove("recoverySources");
        inputs.remove("temporaryPngFocalRecords");
        inputs.remove("temporaryPngFocalPendingRecords");
    }
    if let Some(sources) = result
        .pointer_mut("/inputs/sources")
        .and_then(Value::as_array_mut)
    {
        for source in sources {
            source["bridgeFrameCount"] =
                serde_json::json!(source["bridgeFrames"].as_array().map(Vec::len));
            if let Some(source) = source.as_object_mut() {
                source.remove("originals");
                source.remove("panoramaRecords");
                source.remove("bridgeFrames");
            }
            if let Some(frames) = source.get_mut("frames").and_then(Value::as_object_mut) {
                frames.remove("selectedFrames");
                frames.remove("candidateFrames");
            }
        }
    }
    let panorama = state.get("panorama").is_some_and(Value::is_object);
    result["input_projection"] =
        if let Some(sources) = state.pointer("/inputs/sources").and_then(Value::as_array) {
            let adopted = sources
                .iter()
                .filter(|s| s["adopted"].as_u64().unwrap_or(0) > 0)
                .collect::<Vec<_>>();
            let projections = adopted
                .iter()
                .filter_map(|s| s["projection"].as_str())
                .collect::<std::collections::BTreeSet<_>>();
            if !adopted.is_empty()
                && projections.len() == 1
                && projections
                    .iter()
                    .all(|p| matches!(*p, "perspective" | "equirectangular"))
            {
                serde_json::json!(projections.first())
            } else {
                Value::Null
            }
        } else if panorama {
            Value::String("equirectangular".into())
        } else {
            state
                .get("videoProjection")
                .filter(|v| matches!(v.as_str(), Some("perspective" | "equirectangular")))
                .cloned()
                .unwrap_or(Value::Null)
        };
    result["image_matching_strategy"] =
        if state.get("inputType").and_then(Value::as_str) == Some("video") {
            Value::String("sequential".into())
        } else {
            state
                .get("imageMatchingStrategy")
                .cloned()
                .unwrap_or_else(|| Value::String("exhaustive".into()))
        };
    result["gps"] = serde_json::json!({"status":state.pointer("/gps/status"),"reference_valid":state.pointer("/gps/referenceValid")});
    result["backfill"] = serde_json::json!({"status":state.pointer("/bridgeBackfill/status"),"initial_frames":state.pointer("/frames/initialExtractedFrames"),"actual_frames":state.pointer("/frames/extractedFrames"),"total_frame_budget":state.pointer("/frames/rescueMaxFrames")});
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
    let mut result = serde_json::json!({"quality":state.preset,"planner_enabled":state.planner_enabled,"resolution_policy_version":state.resolution_policy_version,"resolution_plan":state.resolution_plan,"brush_training":state.brush_training,"reshoot_recipe":state.reshoot.as_ref().and_then(|r|r.recipe.as_ref()),"frame_plan":{"sampling_fps":state.frames.as_ref().map(|f|f.sampling_fps),"estimated_frames":state.frames.as_ref().map(|f|f.estimated_frames),"extracted_frames":state.frames.as_ref().and_then(|f|f.extracted_frames),"mask_count":state.frames.as_ref().and_then(|f|f.mask_count)}});
    if let Ok(value) = serde_json::to_value(state) {
        extend_actual_summary(&mut result, &value);
    }
    result
}
