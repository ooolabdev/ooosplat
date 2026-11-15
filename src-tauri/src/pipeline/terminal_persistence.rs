//! Best-effort failure finalization must not replace the error that ended the
//! execution. Keep secondary save failures in bounded, run-local memory so the
//! shared task service can include them even when project logs cannot be written.
use std::sync::Mutex;

use super::{mark_state_terminal, PipelineRunner};
use crate::{
    error::SplatError,
    project::{
        manager::atomic_write_json_for_preparation, ProjectManager, ProjectMetadata, ProjectPaths,
    },
};

#[derive(Default)]
pub(super) struct Diagnostics(Mutex<Vec<String>>);

/// Never reconstructed from an IPC message: only the run-local collector may
/// create this context, after reducing secondary errors to path-free fields.
#[derive(Debug, Clone)]
pub(crate) struct TerminalSaveContext {
    original_message: String,
    safe_details: String,
}

impl TerminalSaveContext {
    pub(crate) fn original_message(&self) -> &str {
        &self.original_message
    }

    pub(crate) fn safe_details(&self) -> &str {
        &self.safe_details
    }
}

#[derive(Clone, Copy)]
enum Operation {
    WriteMetadata,
    ReadState,
    WriteState,
}

impl Operation {
    fn fields(self) -> (&'static str, &'static str) {
        match self {
            Self::WriteMetadata => ("write_terminal_metadata", "project_metadata"),
            Self::ReadState => ("read_terminal_state", "project_state"),
            Self::WriteState => ("write_terminal_state", "project_state"),
        }
    }
}

impl Diagnostics {
    pub(super) fn clear(&self) {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).clear();
    }

    fn record(&self, operation: Operation, error: &SplatError) {
        let detail = safe_detail(operation, error);
        // The original error is deliberately not formatted here: it may carry
        // an absolute path. This warning is supplementary, not a stage event.
        tracing::warn!(detail = %detail, "Terminal record persistence failed");
        let mut details = self.0.lock().unwrap_or_else(|p| p.into_inner());
        if details.len() < 3 && !details.contains(&detail) {
            details.push(detail);
        }
    }
}

fn safe_detail(operation: Operation, error: &SplatError) -> String {
    let (operation, role) = operation.fields();
    let mut detail = format!("terminal_save_operation={operation}\npath_role={role}");
    let mut source = error;
    while let SplatError::Diagnostic {
        source: inner,
        detail: context,
    } = source
    {
        // Only known atomic-save substeps cross this boundary; neither arbitrary
        // context nor a secondary error's human-readable/path-bearing text does.
        for line in context.lines() {
            if matches!(
                line,
                "io_substep=create_state_temp"
                    | "io_substep=write_state_temp"
                    | "io_substep=flush_state_temp"
                    | "io_substep=sync_state_temp"
                    | "io_substep=verify_state_temp"
                    | "io_substep=replace_state"
            ) && !detail.lines().any(|existing| existing == line)
            {
                detail.push('\n');
                detail.push_str(line);
            }
        }
        source = inner;
    }
    match source {
        SplatError::Io(error) => {
            detail.push_str(&format!("\nsecondary_error_kind={:?}", error.kind()));
            if let Some(code) = error.raw_os_error() {
                detail.push_str(&format!("\nsecondary_os_error={code}"));
            }
        }
        SplatError::Json(error) => detail.push_str(&format!(
            "\nsecondary_error_kind=json_{:?}\njson_line={}\njson_column={}",
            error.classify(),
            error.line(),
            error.column()
        )),
        _ => detail.push_str("\nsecondary_error_kind=unexpected_persistence_error"),
    }
    detail
}

impl PipelineRunner {
    pub(crate) fn terminal_save_context(&self, error: &SplatError) -> Option<TerminalSaveContext> {
        let details = self
            .terminal_save_diagnostics
            .0
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        (!details.is_empty()).then(|| TerminalSaveContext {
            original_message: error.to_string(),
            safe_details: details.join("\n"),
        })
    }

    pub(crate) fn failure_message(&self, error: &SplatError) -> String {
        let mut message = error.to_string();
        let details = self
            .terminal_save_diagnostics
            .0
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        for detail in details.iter() {
            message.push('\n');
            message.push_str(detail);
        }
        message
    }

    pub(super) async fn persist_terminal_records(
        &self,
        manager: &ProjectManager,
        paths: &ProjectPaths,
        metadata: &ProjectMetadata,
        cancelled: bool,
    ) {
        // The context-aware writer preserves the precise I/O substep. This is
        // still failure finalization, never a synthetic pipeline/preparation stage.
        if let Err(error) = atomic_write_json_for_preparation(&paths.metadata, metadata).await {
            self.terminal_save_diagnostics
                .record(Operation::WriteMetadata, &error);
        }
        match manager.read_state(&paths.state).await {
            Ok(state) => {
                let state = mark_state_terminal(state, cancelled);
                if let Err(error) = manager
                    .write_state_for_preparation(&paths.state, &state)
                    .await
                {
                    self.terminal_save_diagnostics
                        .record(Operation::WriteState, &error);
                }
            }
            Err(error) => {
                // A failed read is not an empty work order. Preserve damaged or
                // inaccessible state verbatim instead of manufacturing defaults.
                self.terminal_save_diagnostics
                    .record(Operation::ReadState, &error);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        commands::PipelineCommandError,
        engines::EnginePaths,
        pipeline::{PipelineEngine, PipelineStage},
        presets::Quality,
        project::PipelineStateFile,
    };

    fn runner() -> PipelineRunner {
        PipelineRunner::new(EnginePaths::from_root("missing-engines"), |_| {})
            .with_execution_run_id(Some(uuid::Uuid::new_v4()))
    }

    #[tokio::test]
    async fn terminal_save_failures_reach_shared_command_error_without_replacing_primary() {
        let root = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::existing(uuid::Uuid::new_v4(), root.path().into());
        std::fs::create_dir(&paths.metadata).unwrap(); // Atomic replacement cannot overwrite a directory.
        std::fs::write(&paths.state, b"{damaged checkpoint").unwrap();
        let manager = ProjectManager::for_diagnostics(root.path().into());
        let runner = runner();
        let original = SplatError::Io(std::io::Error::from_raw_os_error(112));
        runner
            .persist_terminal_records(&manager, &paths, &ProjectMetadata::default(), false)
            .await;

        let command = PipelineCommandError::from_runner(&original, &runner);
        assert_eq!(original.io_error().unwrap().raw_os_error(), Some(112));
        assert!(command.message.starts_with(&original.to_string()));
        assert!(command
            .message
            .contains("terminal_save_operation=write_terminal_metadata"));
        assert!(command.message.contains("io_substep=replace_state"));
        assert!(command
            .message
            .contains("terminal_save_operation=read_terminal_state"));
        assert!(command.message.contains("secondary_error_kind=json_Syntax"));
        assert!(!command
            .message
            .contains(root.path().to_string_lossy().as_ref()));
        assert_eq!(std::fs::read(&paths.state).unwrap(), b"{damaged checkpoint");
        assert!(paths.metadata.is_dir());
        assert_eq!(command.failed_stage, None);
    }

    #[tokio::test]
    async fn cancellation_and_completed_checkpoints_survive_terminal_metadata_failure() {
        let root = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::existing(uuid::Uuid::new_v4(), root.path().into());
        std::fs::create_dir(&paths.metadata).unwrap();
        let manager = ProjectManager::for_diagnostics(root.path().into());
        let mut state = PipelineStateFile::created(Quality::Balanced);
        state.features_complete = true;
        state.matching_complete = true;
        state.reconstruction_complete = true;
        state.brush_training.oom_retry_used = true;
        manager.write_state(&paths.state, &state).await.unwrap();
        let runner = runner();
        runner
            .persist_terminal_records(&manager, &paths, &ProjectMetadata::default(), true)
            .await;
        let original = SplatError::Cancelled;
        let command = PipelineCommandError::from_runner(&original, &runner);
        assert!(matches!(original, SplatError::Cancelled));
        assert_eq!(command.code, "cancelled");
        assert_eq!(command.failure_kind, None);
        assert!(command.message.contains("write_terminal_metadata"));
        let saved = manager.read_state(&paths.state).await.unwrap();
        assert_eq!(saved.stage, PipelineStage::Cancelled);
        assert!(
            saved.features_complete && saved.matching_complete && saved.reconstruction_complete
        );
        assert!(saved.brush_training.oom_retry_used);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn terminal_state_replacement_failure_keeps_checkpoint_and_is_reported() {
        use std::os::windows::fs::OpenOptionsExt;

        let root = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::existing(uuid::Uuid::new_v4(), root.path().into());
        let manager = ProjectManager::for_diagnostics(root.path().into());
        let mut state = PipelineStateFile::created(Quality::Balanced);
        state.reconstruction_complete = true;
        manager.write_state(&paths.state, &state).await.unwrap();
        let before = std::fs::read(&paths.state).unwrap();
        // Allow the normal state read, but hold a real Windows handle which
        // disallows replacement. No ACL changes or disk exhaustion are needed.
        let guard = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&paths.state)
            .unwrap();
        let runner = runner();
        runner
            .persist_terminal_records(&manager, &paths, &ProjectMetadata::default(), false)
            .await;
        let command = PipelineCommandError::from_runner(
            &SplatError::BrushOutOfMemory("original GPU error".into()),
            &runner,
        );
        assert!(command
            .message
            .contains("terminal_save_operation=write_terminal_state"));
        assert!(command.message.contains("io_substep=replace_state"));
        assert!(command.message.contains("secondary_os_error="));
        assert_eq!(command.failure_kind, Some("brush_out_of_memory"));
        assert_eq!(std::fs::read(&paths.state).unwrap(), before);
        drop(guard);
    }

    #[test]
    fn secondary_context_is_bounded_safe_and_reset_only_for_another_run() {
        let runner = runner();
        let run = runner.execution_run_id;
        let secondary = SplatError::Io(std::io::Error::from_raw_os_error(112))
            .with_io_diagnostic("io_substep=flush_state_temp\npath=C:\\private\\project");
        for _ in 0..100 {
            runner
                .terminal_save_diagnostics
                .record(Operation::WriteState, &secondary);
        }
        let original = SplatError::Process("original failure".into());
        let message = runner.failure_message(&original);
        assert_eq!(message.matches("terminal_save_operation=").count(), 1);
        assert!(message.contains("secondary_os_error=112"));
        assert!(message.contains("io_substep=flush_state_temp"));
        assert!(!message.contains("private"));
        let runner = runner.with_execution_run_id(run);
        assert_eq!(runner.failure_message(&original), message);
        let runner = runner.with_execution_run_id(Some(uuid::Uuid::new_v4()));
        assert_eq!(runner.failure_message(&original), original.to_string());
    }

    #[test]
    fn engine_termination_and_classification_are_not_wrapped_by_save_failures() {
        let runner = runner();
        runner.terminal_save_diagnostics.record(
            Operation::WriteState,
            &SplatError::Io(std::io::Error::from_raw_os_error(5)),
        );
        let termination = crate::process::ProcessTermination {
            exit_code: Some(101),
            ..Default::default()
        };
        let output = crate::process::ProcessOutput {
            success: false,
            exit_code: termination.exit_code,
            termination: termination.clone(),
            stdout: String::new(),
            stderr: "GPU out of memory".into(),
        };
        let original =
            SplatError::EngineProcess(Box::new(crate::failure::EngineProcessFailure::from_output(
                PipelineEngine::Brush,
                "train",
                &output,
            )));
        let command = PipelineCommandError::from_runner(&original, &runner);
        assert!(matches!(original, SplatError::EngineProcess(_)));
        assert_eq!(command.termination.as_deref(), Some(&termination));
        assert_eq!(command.failure_kind, Some("brush_out_of_memory"));
        assert!(command.message.contains("secondary_os_error=5"));
    }

    #[tokio::test]
    async fn terminal_report_context_survives_long_causes_and_private_basenames() {
        use crate::pipeline::{brush_execution::BrushExecutionSnapshot, runtime::RuntimeTracker};

        for basename in ["state", "project", "brush"] {
            for with_execution in [false, true] {
                let runner = runner();
                runner.terminal_save_diagnostics.record(
                    Operation::WriteState,
                    &SplatError::Io(std::io::Error::from_raw_os_error(112))
                        .with_io_diagnostic("io_substep=flush_state_temp\npath=C:\\private"),
                );
                let primary = SplatError::Process(format!(
                    "original failure remains first\nC:\\AliceSensitive\\{basename}\\photo.png\n{}",
                    "earlier attempt context\n".repeat(2000)
                ));
                let command = PipelineCommandError::from_runner(&primary, &runner);
                let mut executions = Vec::new();
                if with_execution {
                    executions.push(BrushExecutionSnapshot::new(
                        &RuntimeTracker::new().snapshot,
                        None,
                    ));
                }
                let private_paths = [std::path::PathBuf::from(basename), "AliceSensitive".into()];
                let service = crate::diagnostics::DiagnosticService::default();
                let id = service
                    .capture_with_execution_log(
                        Some(PipelineStage::TrainingSplats),
                        Some(PipelineEngine::Brush),
                        Some("brush_out_of_memory"),
                        &command.message,
                        None,
                        &private_paths,
                        &executions,
                        None,
                        None,
                        command.terminal_save_context.as_deref(),
                    )
                    .await;
                let report = service.captured_report_for_test(id).unwrap();
                assert!(report
                    .detail
                    .starts_with("外部进程执行失败：original failure remains first"));
                assert!(report
                    .detail
                    .contains("terminal_save_operation=write_terminal_state"));
                assert!(report.detail.contains("path_role=project_state"));
                assert!(report.detail.contains("io_substep=flush_state_temp"));
                assert!(report.detail.contains("secondary_os_error=112"));
                assert_eq!(report.detail.matches("terminal_save_operation=").count(), 1);
                assert!(report.detail.len() <= 16 * 1024);
                assert!(!report.detail.contains("AliceSensitive"));
                assert!(!report.detail.contains("photo.png"));
                assert!(!report.detail.contains("C:\\"));
                assert_eq!(report.error_code, "brush_out_of_memory");
                assert_eq!(report.schema_version, 1);
                assert_eq!(
                    serde_json::to_value(report)
                        .unwrap()
                        .as_object()
                        .unwrap()
                        .len(),
                    12
                );
                let encoded = serde_json::to_value(&command).unwrap();
                assert!(encoded.get("terminalSaveContext").is_none());
                assert!(encoded.get("terminal_save_context").is_none());
            }
        }
    }

    #[tokio::test]
    async fn terminal_context_is_kept_when_preparation_capture_owns_the_report() {
        let runner = runner();
        runner.terminal_save_diagnostics.record(
            Operation::WriteMetadata,
            &SplatError::Io(std::io::Error::from_raw_os_error(5))
                .with_io_diagnostic("io_substep=replace_state"),
        );
        let primary = SplatError::Process(format!(
            "original failure\nC:\\AliceSensitive\\project\\photo.png\n{}",
            "earlier attempt context\n".repeat(2000)
        ));
        let command = PipelineCommandError::from_runner(&primary, &runner);
        let context = crate::pipeline::retry_preparation::PreparationContext::new(None, None);
        let service = crate::diagnostics::DiagnosticService::default();
        let id = service
            .capture_preparation(
                &command.message,
                &context.snapshot(),
                &["project".into(), "AliceSensitive".into()],
                command.terminal_save_context.as_deref(),
            )
            .await;
        let report = service.captured_report_for_test(id).unwrap();
        assert!(report.detail.contains("original failure"));
        assert!(report
            .detail
            .contains("terminal_save_operation=write_terminal_metadata"));
        assert!(report.detail.contains("path_role=project_metadata"));
        assert!(report.detail.contains("io_substep=replace_state"));
        assert!(report.detail.contains("secondary_os_error=5"));
        assert!(!report.detail.contains("AliceSensitive"));
        assert!(!report.detail.contains("photo.png"));
        assert!(report.detail.len() <= 16 * 1024);
        assert_eq!(report.stage, None);
        assert_eq!(report.engine, Some(PipelineEngine::System));
    }
}
