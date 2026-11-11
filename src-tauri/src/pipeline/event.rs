use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::engines::ColmapAccelerationStatus;

use super::{progress::stage_progress_range, PipelineStage};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EventKind {
    Stage,
    Progress,
    Log,
    Heartbeat,
    Capability,
    Runtime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EventLevel {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PipelineEngine {
    System,
    Ffmpeg,
    Colmap,
    Brush,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineOperation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
    pub id: u64,
    /// Actual process execution number within this stage; zero for in-process preparation.
    pub attempt: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineEvent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<PipelineOperation>,
    #[serde(default)]
    pub task_id: Option<uuid::Uuid>,
    #[serde(default)]
    pub run_id: Option<uuid::Uuid>,
    #[serde(default)]
    pub revision: u64,
    pub sequence: u64,
    pub timestamp: DateTime<Utc>,
    pub kind: EventKind,
    pub level: EventLevel,
    pub stage: PipelineStage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub substage: Option<String>,
    pub engine: Option<PipelineEngine>,
    pub progress: f32,
    pub stage_progress: Option<f32>,
    pub indeterminate: bool,
    pub message: String,
    pub current: Option<u64>,
    pub total: Option<u64>,
    pub unit: Option<String>,
    pub elapsed_ms: u64,
    pub acceleration: Option<ColmapAccelerationStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<super::runtime::RuntimeSnapshot>,
}

impl PipelineEvent {
    pub fn mapped(stage: PipelineStage, stage_progress: f32, message: impl Into<String>) -> Self {
        let bounded = stage_progress.clamp(0.0, 1.0);
        let (start, end) = stage_progress_range(stage);
        Self {
            operation: None,
            task_id: None,
            run_id: None,
            revision: 0,
            sequence: 0,
            timestamp: Utc::now(),
            kind: EventKind::Stage,
            level: EventLevel::Info,
            stage,
            substage: None,
            engine: Some(PipelineEngine::System),
            progress: start + (end - start) * bounded,
            stage_progress: Some(bounded * 100.0),
            indeterminate: false,
            message: message.into(),
            current: None,
            total: None,
            unit: None,
            elapsed_ms: 0,
            acceleration: None,
            runtime: None,
        }
    }
}
