//! Durable creation identity in existing task/state files, not a new ticket.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectCreationIntent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ba_policy_version: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brush_execution_options: Option<crate::project::metadata::BrushExecutionOptions>,
    pub intent_id: Uuid,
    pub project_id: Uuid,
    pub task_id: Uuid,
    pub creating_run_id: Uuid,
    pub project_path: PathBuf,
    pub request_identity: String,
}

impl ProjectCreationIntent {
    pub fn reserve(task: &crate::tasks::TaskRecord, run_id: Uuid) -> crate::tasks::Result<Self> {
        use sha2::{Digest, Sha256};
        let mut intent = serde_json::json!({
            "input": task.input_path, "request": task.request_parameters,
            "quality": task.quality, "configuration": task.configuration,
            "source": task.source_project_id, "root": task.projects_root,
            "planner": task.planner_enabled,
            "brushExecutionOptions": task.brush_execution_options,
        });
        // Omission must preserve the byte-for-byte identity of reservations
        // created before this policy existed, including interrupted creation.
        if let Some(version) = task.ba_policy_version {
            intent["baPolicyVersion"] = serde_json::json!(version);
        }
        let identity = serde_json::to_vec(&intent)?;
        let project_id = Uuid::new_v4();
        let name = if task.source_project_id.is_some() {
            "reshoot"
        } else if task.input_path.is_dir() {
            "images"
        } else {
            task.input_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("project")
        };
        let name: String = name
            .chars()
            .take(48)
            .map(|c| {
                if c.is_alphanumeric() || matches!(c, '-' | '_') {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        Ok(Self {
            ba_policy_version: task.ba_policy_version,
            brush_execution_options: task.brush_execution_options,
            intent_id: Uuid::new_v4(),
            project_id,
            task_id: task.task_id,
            creating_run_id: run_id,
            project_path: task.projects_root.join(format!(
                "{}_{}_{}",
                chrono::Local::now().format("%Y%m%d-%H%M%S"),
                name,
                &project_id.simple().to_string()[..8]
            )),
            request_identity: format!("{:x}", Sha256::digest(identity)),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectCreationState {
    pub intent: ProjectCreationIntent,
    pub ready: bool,
}
