//! Bounded, application-owned UI notification buffers. Durable logs stay on disk.
use super::{TaskRecord, TaskStatus};
use crate::pipeline::{EventKind, PipelineEvent, PipelineStage};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use uuid::Uuid;

pub(super) const NOTIFICATION_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);
pub(super) const MAX_PENDING_LOGS: usize = 200;
pub(super) const MAX_PENDING_LOG_BYTES: usize = 64 * 1024;

/// Only fields consumed by the UI. Full records remain available to query tools.
#[derive(Debug, Clone, Serialize)]
pub struct TaskView {
    pub task_id: Uuid,
    pub run_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub project_path: Option<PathBuf>,
    pub project_deleted: bool,
    pub input_path: PathBuf,
    pub input_type: crate::project::ProjectInputType,
    pub quality: crate::presets::Quality,
    pub source: Option<String>,
    pub task_kind: String,
    pub source_project_id: Option<Uuid>,
    pub planner_enabled: bool,
    pub projects_root: PathBuf,
    pub status: TaskStatus,
    pub stage: Option<PipelineStage>,
    pub revision: u64,
    pub sequence: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub elapsed_ms: u64,
    pub runtime: Option<crate::pipeline::runtime::RuntimeSnapshot>,
    pub progress: Option<f32>,
    pub estimated_progress: Option<f32>,
    pub current: Option<u64>,
    pub total: Option<u64>,
    pub unit: Option<String>,
    pub eta_seconds: Option<f64>,
    pub error: Option<super::TaskError>,
    pub result: Option<Value>,
    pub runs: Vec<RunView>,
    pub recent_events: Vec<PipelineEvent>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RunView {
    pub run_id: Uuid,
    pub kind: String,
}

impl From<&TaskRecord> for TaskView {
    fn from(task: &TaskRecord) -> Self {
        Self {
            task_id: task.task_id,
            run_id: task.run_id,
            project_id: task.project_id,
            project_path: task.project_path.clone(),
            project_deleted: task.project_deleted,
            input_path: task.input_path.clone(),
            input_type: task.input_type,
            quality: task.quality,
            source: task.source.clone(),
            task_kind: task.task_kind.clone(),
            source_project_id: task.source_project_id,
            planner_enabled: task.planner_enabled,
            projects_root: task.projects_root.clone(),
            status: task.status,
            stage: task.stage,
            revision: task.revision,
            sequence: task.sequence,
            created_at: task.created_at,
            updated_at: task.updated_at,
            elapsed_ms: task.elapsed_ms,
            runtime: task.runtime.clone(),
            progress: task.progress,
            estimated_progress: task.estimated_progress,
            current: task.current,
            total: task.total,
            unit: task.unit.clone(),
            eta_seconds: task.eta_seconds,
            error: task.error.clone(),
            result: task.result.clone(),
            runs: task
                .runs
                .iter()
                .rev()
                .find(|run| Some(run.run_id) == task.run_id)
                .map(|run| RunView {
                    run_id: run.run_id,
                    kind: run.kind.clone(),
                })
                .into_iter()
                .collect(),
            recent_events: Vec::new(),
        }
    }
}

#[derive(Default)]
pub(super) struct PendingNotifications {
    pub tasks: BTreeMap<Uuid, PendingTask>,
}

impl PendingNotifications {
    pub fn task(&mut self, task: Uuid, run: Option<Uuid>) -> &mut PendingTask {
        let pending = self.tasks.entry(task).or_default();
        if pending.run_id != run {
            *pending = PendingTask {
                run_id: run,
                ..Default::default()
            };
        }
        pending
    }
}

#[derive(Default)]
pub(super) struct PendingTask {
    pub run_id: Option<Uuid>,
    logs: VecDeque<(PipelineEvent, usize)>,
    pub log_bytes: usize,
    pub dropped_event_count: u64,
    progress: Option<PipelineEvent>,
    runtime: Option<PipelineEvent>,
    heartbeat: Option<PipelineEvent>,
    stage: Option<PipelineEvent>,
    capability: Option<PipelineEvent>,
}

impl PendingTask {
    pub fn push(&mut self, event: PipelineEvent) {
        match event.kind {
            EventKind::Log => {
                // Count serialized bytes without allocating a JSON copy per log line.
                let mut count = ByteCount::default();
                if serde_json::to_writer(&mut count, &event).is_err()
                    || count.0 > MAX_PENDING_LOG_BYTES
                {
                    self.dropped_event_count += 1;
                    return;
                }
                self.log_bytes += count.0;
                self.logs.push_back((event, count.0));
                while self.logs.len() > MAX_PENDING_LOGS || self.log_bytes > MAX_PENDING_LOG_BYTES {
                    if let Some((_, bytes)) = self.logs.pop_front() {
                        self.log_bytes -= bytes;
                        self.dropped_event_count += 1;
                    }
                }
            }
            EventKind::Progress => replace_latest(&mut self.progress, event),
            EventKind::Runtime => replace_latest(&mut self.runtime, event),
            EventKind::Heartbeat => replace_latest(&mut self.heartbeat, event),
            EventKind::Stage => replace_latest(&mut self.stage, event),
            EventKind::Capability => replace_latest(&mut self.capability, event),
        }
    }

    pub fn into_events(self) -> Vec<PipelineEvent> {
        let mut events: Vec<_> = self.logs.into_iter().map(|(event, _)| event).collect();
        events.extend(self.progress);
        events.extend(self.runtime);
        events.extend(self.heartbeat);
        events.extend(self.stage);
        events.extend(self.capability);
        events.sort_unstable_by_key(|event| event.sequence);
        events
    }

    #[cfg(test)]
    pub fn log_count(&self) -> usize {
        self.logs.len()
    }
}

fn replace_latest(slot: &mut Option<PipelineEvent>, event: PipelineEvent) {
    if slot
        .as_ref()
        .is_none_or(|previous| event.sequence == 0 || event.sequence > previous.sequence)
    {
        *slot = Some(event);
    }
}

#[derive(Default)]
struct ByteCount(usize);
impl std::io::Write for ByteCount {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 += bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
