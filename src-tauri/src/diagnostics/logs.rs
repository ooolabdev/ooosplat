//! Session-local log provenance. These records never cross IPC or enter reports.
//! A filename/mtime is not evidence that a log belongs to a failed operation.
use crate::{
    pipeline::{event::PipelineOperation, PipelineEngine},
    tasks::logs::{diagnostic_tail, register, RegisteredLog},
};
use std::{collections::VecDeque, path::Path};
use uuid::Uuid;

const MAX_EXECUTION_LOGS: usize = 32;
const MAX_LOG_BYTES: usize = 64 * 1024;
const MAX_LOG_LINES: usize = 200;

#[derive(Debug, Clone)]
pub(crate) struct FrozenLog {
    pub text: String,
    pub truncated: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct ExecutionLog {
    run_id: Uuid,
    operation: PipelineOperation,
    engine: PipelineEngine,
    process_id: u32,
    opened_path: std::path::PathBuf,
    log: RegisteredLog,
    end: Option<u64>,
    frozen: Option<FrozenLog>,
    termination: Option<crate::process::ProcessTermination>,
}

impl ExecutionLog {
    fn matches(&self, run_id: Uuid, operation: &PipelineOperation, engine: PipelineEngine) -> bool {
        self.run_id == run_id
            && self.operation.id == operation.id
            && self.operation.attempt == operation.attempt
            && self.operation.source_id == operation.source_id
            && self.engine == engine
    }

    pub fn freeze(self) -> Option<FrozenLog> {
        self.frozen
    }

    pub fn termination_matches(&self, expected: &crate::process::ProcessTermination) -> bool {
        self.termination.as_ref() == Some(expected)
    }
}

#[derive(Debug, Default)]
pub(crate) struct ExecutionLogs {
    logs: VecDeque<ExecutionLog>,
}

impl ExecutionLogs {
    #[allow(clippy::too_many_arguments)]
    pub fn opened(
        &mut self,
        run_id: Uuid,
        operation: &PipelineOperation,
        engine: PipelineEngine,
        process_id: u32,
        root: &Path,
        path: &Path,
        start: u64,
    ) {
        let Ok(log) = register(root, path, start) else {
            return;
        };
        self.logs.push_back(ExecutionLog {
            run_id,
            operation: operation.clone(),
            engine,
            process_id,
            opened_path: path.to_owned(),
            log,
            end: None,
            frozen: None,
            termination: None,
        });
        while self.logs.len() > MAX_EXECUTION_LOGS {
            self.logs.pop_front();
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn closed(
        &mut self,
        run_id: Uuid,
        operation: &PipelineOperation,
        engine: PipelineEngine,
        process_id: u32,
        path: &Path,
        end: u64,
        success: bool,
        termination: crate::process::ProcessTermination,
    ) {
        if let Some(log) = self.logs.iter_mut().rev().find(|log| {
            log.matches(run_id, operation, engine)
                && log.process_id == process_id
                && log.opened_path == path
                && log.end.is_none()
        }) {
            if end >= log.log.start {
                log.end = Some(end);
                log.termination = Some(termination);
                // Freeze immediately after the termination row is written.
                // Later appends, rotations or truncate/regrow cannot change
                // evidence belonging to this already finished execution.
                if !success {
                    log.frozen = diagnostic_tail(&log.log, end, MAX_LOG_BYTES, MAX_LOG_LINES)
                        .map(|(text, truncated)| FrozenLog { text, truncated });
                }
            }
        }
    }

    pub fn for_failure(
        &self,
        run_id: Uuid,
        operation: &PipelineOperation,
        engine: PipelineEngine,
    ) -> Option<ExecutionLog> {
        self.logs
            .iter()
            .rev()
            .find(|log| log.matches(run_id, operation, engine))
            .filter(|log| log.frozen.is_some())
            .cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn operation(id: u64) -> PipelineOperation {
        PipelineOperation {
            source_id: Some("source-2".into()),
            id,
            attempt: id as u32,
        }
    }

    fn append(path: &Path, text: &str) -> u64 {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        file.write_all(text.as_bytes()).unwrap();
        file.metadata().unwrap().len()
    }

    fn record(
        logs: &mut ExecutionLogs,
        run: Uuid,
        op: &PipelineOperation,
        engine: PipelineEngine,
        root: &Path,
        path: &Path,
        text: &str,
    ) {
        let start = append(path, "");
        logs.opened(run, op, engine, 17, root, path, start);
        let end = append(path, text);
        logs.closed(run, op, engine, 17, path, end, false, Default::default());
    }

    #[test]
    fn gps_and_each_joint_bridge_operation_use_registered_ranges_not_filenames_or_mtime() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let run = Uuid::new_v4();
        let mut logs = ExecutionLogs::default();
        for (index, (name, engine)) in [
            ("gps.log", PipelineEngine::Colmap),
            ("joint-bridge-frames.log", PipelineEngine::Ffmpeg),
            ("joint-bridge-features.log", PipelineEngine::Colmap),
            ("joint-bridge-matching.log", PipelineEngine::Colmap),
        ]
        .into_iter()
        .enumerate()
        {
            let op = operation(index as u64 + 1);
            record(&mut logs, run, &op, engine, root, &root.join(name), name);
            append(&root.join("colmap.log"), "newer unrelated visual log\n");
            append(&root.join("ffmpeg.log"), "newer unrelated extraction log\n");
            let tail = logs
                .for_failure(run, &op, engine)
                .unwrap()
                .freeze()
                .unwrap();
            assert_eq!(tail.text, name);
            assert!(!tail.truncated);
            assert!(logs.for_failure(Uuid::new_v4(), &op, engine).is_none());
            assert!(logs
                .for_failure(run, &operation(op.id + 100), engine)
                .is_none());
        }
    }

    #[test]
    fn repeated_attempts_in_one_file_keep_exact_intervals_and_frozen_content() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let path = root.join("colmap.log");
        let run = Uuid::new_v4();
        let mut logs = ExecutionLogs::default();
        record(
            &mut logs,
            run,
            &operation(1),
            PipelineEngine::Colmap,
            root,
            &path,
            "Caspar failed\ntermination: 1\n",
        );
        record(
            &mut logs,
            run,
            &operation(2),
            PipelineEngine::Colmap,
            root,
            &path,
            "Ceres failed\ntermination: 2\n",
        );
        let first = logs
            .for_failure(run, &operation(1), PipelineEngine::Colmap)
            .unwrap()
            .freeze()
            .unwrap();
        let last = logs
            .for_failure(run, &operation(2), PipelineEngine::Colmap)
            .unwrap()
            .freeze()
            .unwrap();
        assert_eq!(first.text, "Caspar failed\ntermination: 1");
        assert_eq!(last.text, "Ceres failed\ntermination: 2");
        append(&path, "later unrelated output\n");
        assert_eq!(last.text, "Ceres failed\ntermination: 2");
        assert_eq!(
            logs.for_failure(run, &operation(2), PipelineEngine::Colmap)
                .unwrap()
                .freeze()
                .unwrap()
                .text,
            last.text
        );
    }

    #[test]
    fn unsealed_missing_or_unproven_logs_have_no_fallback() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let path = root.join("brush.log");
        let run = Uuid::new_v4();
        let mut logs = ExecutionLogs::default();
        append(&path, "historical output\n");
        let start = append(&path, "");
        logs.opened(
            run,
            &operation(1),
            PipelineEngine::Brush,
            17,
            root,
            &path,
            start,
        );
        append(&path, "unfinished\n");
        assert!(logs
            .for_failure(run, &operation(1), PipelineEngine::Brush)
            .is_none());
        logs.closed(
            run,
            &operation(1),
            PipelineEngine::Brush,
            99,
            &path,
            100,
            false,
            Default::default(),
        );
        assert!(logs
            .for_failure(run, &operation(1), PipelineEngine::Brush)
            .is_none());
        logs.closed(
            run,
            &operation(1),
            PipelineEngine::Brush,
            17,
            &path,
            start,
            false,
            Default::default(),
        );
        assert!(logs
            .for_failure(run, &operation(1), PipelineEngine::Colmap)
            .is_none());
        assert_eq!(
            logs.for_failure(run, &operation(1), PipelineEngine::Brush)
                .unwrap()
                .freeze()
                .unwrap()
                .text,
            ""
        );
    }

    #[test]
    fn replaced_or_truncated_unsealed_log_is_rejected_without_rebasing() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let path = root.join("gps.log");
        let run = Uuid::new_v4();
        let mut logs = ExecutionLogs::default();
        logs.opened(
            run,
            &operation(1),
            PipelineEngine::Colmap,
            17,
            root,
            &path,
            append(&path, ""),
        );
        let end = append(&path, "original execution output\n");
        std::fs::write(&path, "short").unwrap();
        logs.closed(
            run,
            &operation(1),
            PipelineEngine::Colmap,
            17,
            &path,
            end,
            false,
            Default::default(),
        );
        assert!(logs
            .for_failure(run, &operation(1), PipelineEngine::Colmap)
            .is_none());
        let start = append(&path, "");
        logs.opened(
            run,
            &operation(2),
            PipelineEngine::Colmap,
            17,
            root,
            &path,
            start,
        );
        let end = append(&path, "another execution\n");
        std::fs::rename(&path, root.join("old.log")).unwrap();
        append(&path, &"unrelated replacement\n".repeat(10));
        logs.closed(
            run,
            &operation(2),
            PipelineEngine::Colmap,
            17,
            &path,
            end,
            false,
            Default::default(),
        );
        assert!(logs
            .for_failure(run, &operation(2), PipelineEngine::Colmap)
            .is_none());
    }

    #[test]
    fn sealing_freezes_evidence_before_same_inode_truncate_and_regrow() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let path = root.join("gps.log");
        let run = Uuid::new_v4();
        let mut logs = ExecutionLogs::default();
        record(
            &mut logs,
            run,
            &operation(1),
            PipelineEngine::Colmap,
            root,
            &path,
            "original failure\ntermination: 7\n",
        );
        std::fs::write(&path, "unrelated replacement output\n".repeat(100)).unwrap();
        assert_eq!(
            logs.for_failure(run, &operation(1), PipelineEngine::Colmap)
                .unwrap()
                .freeze()
                .unwrap()
                .text,
            "original failure\ntermination: 7"
        );
    }

    #[test]
    fn range_reads_are_bounded_and_registration_memory_is_bounded() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let path = root.join("brush.log");
        let run = Uuid::new_v4();
        let mut logs = ExecutionLogs::default();
        record(
            &mut logs,
            run,
            &operation(1),
            PipelineEngine::Brush,
            root,
            &path,
            &"中文 line\n".repeat(100_000),
        );
        let tail = logs
            .for_failure(run, &operation(1), PipelineEngine::Brush)
            .unwrap()
            .freeze()
            .unwrap();
        assert_eq!(tail.text.lines().count(), MAX_LOG_LINES);
        assert!(tail.text.len() <= MAX_LOG_BYTES && tail.truncated);
        for id in 2..=MAX_EXECUTION_LOGS as u64 + 2 {
            record(
                &mut logs,
                run,
                &operation(id),
                PipelineEngine::Brush,
                root,
                &path,
                "next\n",
            );
        }
        assert_eq!(logs.logs.len(), MAX_EXECUTION_LOGS);
        assert!(logs
            .for_failure(run, &operation(1), PipelineEngine::Brush)
            .is_none());
    }
}
