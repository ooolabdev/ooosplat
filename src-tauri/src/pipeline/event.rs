use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::engines::ColmapAccelerationStatus;

use super::{progress::stage_progress_range, PipelineStage};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EventKind {
    Stage,
    Progress,
    Log,
    Heartbeat,
    Capability,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EventLevel {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PipelineEngine {
    System,
    Ffmpeg,
    Colmap,
    Brush,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineEvent {
    pub sequence: u64,
    pub timestamp: DateTime<Utc>,
    pub kind: EventKind,
    pub level: EventLevel,
    pub stage: PipelineStage,
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
}

impl PipelineEvent {
    pub fn mapped(stage: PipelineStage, stage_progress: f32, message: impl Into<String>) -> Self {
        let bounded = stage_progress.clamp(0.0, 1.0);
        let (start, end) = stage_progress_range(stage);
        Self {
            sequence: 0,
            timestamp: Utc::now(),
            kind: EventKind::Stage,
            level: EventLevel::Info,
            stage,
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
        }
    }
}
