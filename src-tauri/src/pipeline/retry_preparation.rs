//! Internal diagnostics for preparation, before an engine or pipeline stage runs.
//! The original error remains typed; recording diagnostics is always best effort.
use crate::error::{Result, SplatError, RETRY_PREPARATION_MARKER};
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    future::Future,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

const LOG_BYTES: usize = 64 * 1024;
const LOG_LINES: usize = 200;

#[derive(Debug, Clone, Copy)]
pub(crate) enum RetryPreparationOperation {
    ReadRetryProject,
    ReadRetryState,
    WriteRetryReuseBarrier,
    CreateRetryArchive,
    ArchiveRetryCache,
    WriteRetryArchiveProgress,
    RecoverWorkingDatabase,
}

impl RetryPreparationOperation {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::ReadRetryProject => "read_retry_project",
            Self::ReadRetryState => "read_retry_state",
            Self::WriteRetryReuseBarrier => "write_retry_reuse_barrier",
            Self::CreateRetryArchive => "create_retry_archive",
            Self::ArchiveRetryCache => "archive_retry_cache",
            Self::WriteRetryArchiveProgress => "write_retry_archive_progress",
            Self::RecoverWorkingDatabase => "recover_working_database",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct PreparationContext {
    preparation_id: Uuid,
    project_id: Option<Uuid>,
    project_root: Option<PathBuf>,
    record: Arc<Mutex<PreparationRecord>>,
}

#[derive(Debug, Default)]
struct PreparationRecord {
    lines: VecDeque<String>,
    bytes: usize,
    details: Value,
    classification_kind: Option<&'static str>,
    registered_log: Option<(PathBuf, u64)>,
}

/// Only constructed from an internally resolved project or task record. Paths
/// are for local log access/redaction, never part of the public details object.
#[derive(Debug, Clone)]
pub(crate) struct PreparationDiagnosticContext {
    pub(crate) preparation_id: Uuid,
    pub(crate) project_id: Option<Uuid>,
    pub(crate) project_root: Option<PathBuf>,
    pub(crate) operation_log: String,
    pub(crate) classification_kind: Option<&'static str>,
    details: Value,
}

impl PreparationDiagnosticContext {
    pub(crate) fn safe_details(&self) -> Value {
        self.details.clone()
    }

    /// Private project/source names may themselves be words such as "state"
    /// or "brush". Redact the untrusted primary text, not the known operation
    /// labels, then rebuild the technical suffix from typed, path-free facts.
    pub(crate) fn redacted_message(&self, message: &str, private: &[String]) -> String {
        let primary = message
            .lines()
            .filter(|line| {
                !line.starts_with("io_substep=")
                    && !(line.starts_with("重试准备上下文：")
                        && line.contains(RETRY_PREPARATION_MARKER))
            })
            .collect::<Vec<_>>()
            .join("\n");
        let mut message = crate::diagnostics::redact::sanitize(&primary, private);
        if let (Some(operation), Some(role)) = (
            self.details.get("operation").and_then(Value::as_str),
            self.details.get("pathRole").and_then(Value::as_str),
        ) {
            message.push_str(&format!(
                "\n重试准备上下文：{RETRY_PREPARATION_MARKER}{operation}; path_role={role}",
            ));
            for (field, tag) in [
                ("cache", "cache"),
                ("ioSubstep", "io_substep"),
                ("errorKind", "error_kind"),
            ] {
                if let Some(value) = self.details.get(field).and_then(Value::as_str) {
                    message.push_str(&format!("; {tag}={value}"));
                }
            }
            if let Some(code) = self.details.get("osError").and_then(Value::as_i64) {
                message.push_str(&format!("; os_error={code}"));
            }
        }
        message
    }
}

// All callers use role/category identifiers, not paths. Guard this boundary so
// a future caller cannot accidentally put filenames or GPS into the logs.
fn label(value: &str) -> &str {
    if !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
    {
        value
    } else {
        "unknown"
    }
}

fn io_substep(error: &SplatError) -> Option<&str> {
    if let SplatError::Diagnostic { source, detail } = error {
        // The preparation-only atomic writer provides a fixed, path-free tag.
        // Take the inner primary error first (rollback is only secondary).
        io_substep(source).or_else(|| {
            detail
                .strip_prefix("io_substep=")
                .and_then(|s| s.split([';', ' ', '\n']).next())
                .filter(|s| label(s) == *s)
        })
    } else {
        None
    }
}

impl PreparationContext {
    pub(crate) fn new(project_id: Option<Uuid>, project_root: Option<PathBuf>) -> Self {
        Self {
            preparation_id: Uuid::new_v4(),
            project_id,
            project_root,
            record: Arc::new(Mutex::new(PreparationRecord::default())),
        }
    }

    pub(crate) fn snapshot(&self) -> PreparationDiagnosticContext {
        let record = self.record.lock().unwrap_or_else(|p| p.into_inner());
        PreparationDiagnosticContext {
            preparation_id: self.preparation_id,
            project_id: self.project_id,
            project_root: self.project_root.clone(),
            operation_log: record.lines.iter().map(String::as_str).collect(),
            classification_kind: record.classification_kind,
            details: record.details.clone(),
        }
    }

    pub(crate) fn with_project_root(mut self, root: PathBuf) -> Self {
        self.project_root = Some(root);
        self
    }

    pub(crate) fn registered_log(&self) -> Option<(PathBuf, u64)> {
        self.record
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .registered_log
            .clone()
    }

    pub(crate) async fn step<T, F>(
        &self,
        operation: RetryPreparationOperation,
        path_role: &str,
        cache: Option<&str>,
        future: F,
    ) -> Result<T>
    where
        F: Future<Output = Result<T>>,
    {
        self.record("started", operation, path_role, cache, None)
            .await;
        match future.await {
            Ok(value) => {
                self.record("completed", operation, path_role, cache, None)
                    .await;
                Ok(value)
            }
            Err(error) => {
                self.record("failed", operation, path_role, cache, Some(&error))
                    .await;
                let cache = cache
                    .map(|v| format!("; cache={}", label(v)))
                    .unwrap_or_default();
                Err(error.with_io_diagnostic(format!(
                    "重试准备上下文：{RETRY_PREPARATION_MARKER}{}; path_role={}{}",
                    operation.as_str(),
                    label(path_role),
                    cache
                )))
            }
        }
    }

    async fn record(
        &self,
        status: &str,
        operation: RetryPreparationOperation,
        path_role: &str,
        cache: Option<&str>,
        error: Option<&SplatError>,
    ) {
        let io = error.and_then(SplatError::io_error);
        let kind = io.map(|v| format!("{:?}", v.kind()));
        let os_error = io.and_then(std::io::Error::raw_os_error);
        let substep = error.and_then(io_substep);
        let line = format!(
            "{} preparation_id={} status={} operation={} path_role={} cache={} error_kind={} os_error={} io_substep={}\n",
            chrono::Utc::now().to_rfc3339(), self.preparation_id, status,
            operation.as_str(), label(path_role), cache.map(label).unwrap_or("none"),
            kind.as_deref().unwrap_or("none"), os_error.map(|v| v.to_string()).as_deref().unwrap_or("none"),
            substep.unwrap_or("none"));
        {
            let mut record = self.record.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(error) = error {
                record.classification_kind = crate::failure::classify(None, error).kind;
                record.details = json!({"preparationId":self.preparation_id,
                    "operation":operation.as_str(), "pathRole":label(path_role),
                    "cache":cache.map(label), "ioSubstep":substep, "errorKind":kind,
                    "osError":os_error});
            }
        }
        self.append_line(line).await;
    }

    /// The archive observer supplies metadata-only observations (operation,
    /// attributes, timings, OS codes), never filenames or ACL/GPS contents.
    pub(crate) async fn record_archive_observation(
        &self,
        cache: &str,
        observation: &impl serde::Serialize,
    ) {
        if let Ok(value) = serde_json::to_string(observation) {
            self.append_line(format!(
                "{} preparation_id={} cache={} archiveIo={}\n",
                chrono::Utc::now().to_rfc3339(),
                self.preparation_id,
                label(cache),
                value,
            ))
            .await;
        }
    }

    async fn append_line(&self, line: String) {
        {
            let mut record = self.record.lock().unwrap_or_else(|p| p.into_inner());
            record.bytes += line.len();
            record.lines.push_back(line.clone());
            while record.lines.len() > LOG_LINES || record.bytes > LOG_BYTES {
                if let Some(line) = record.lines.pop_front() {
                    record.bytes -= line.len();
                } else {
                    break;
                }
            }
        }
        if let Some((path, offset)) = self.append_disk(&line).await {
            let mut record = self.record.lock().unwrap_or_else(|p| p.into_inner());
            record.registered_log.get_or_insert((path, offset));
        }
    }

    pub(crate) async fn record_database_recovery(&self, outcome: &str, reason: &str) {
        self.append_line(format!(
            "{} preparation_id={} operation=recover_working_database path_role=colmap_database outcome={} reason={}\n",
            chrono::Utc::now().to_rfc3339(), self.preparation_id,
            label(outcome), label(reason),
        )).await;
    }

    async fn append_disk(&self, line: &str) -> Option<(PathBuf, u64)> {
        let root = tokio::fs::canonicalize(self.project_root.as_ref()?)
            .await
            .ok()?;
        let directory = root.join("logs");
        match tokio::fs::symlink_metadata(&directory).await {
            Ok(meta) if redirected(&meta) || !meta.is_dir() => return None,
            Ok(_) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tokio::fs::create_dir(&directory).await.ok()?;
            }
            Err(_) => return None,
        }
        if tokio::fs::canonicalize(&directory).await.ok()? != directory {
            return None;
        }
        let path = directory.join("retry-preparation.log");
        let offset = match tokio::fs::symlink_metadata(&path).await {
            Ok(meta) if redirected(&meta) || !meta.is_file() => return None,
            Ok(meta) => meta.len(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
            Err(_) => return None,
        };
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await
            .ok()?;
        file.write_all(line.as_bytes()).await.ok()?;
        file.flush().await.ok()?;
        Some((path, offset))
    }
}

fn redirected(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_type().is_symlink()
            || windows_file_attributes_redirected(metadata.file_attributes())
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

#[cfg(windows)]
fn windows_file_attributes_redirected(attributes: u32) -> bool {
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_files_and_directories_are_not_redirected() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("plain-file");
        std::fs::write(&file, b"plain").unwrap();

        assert!(!redirected(&std::fs::symlink_metadata(&file).unwrap()));
        assert!(!redirected(
            &std::fs::symlink_metadata(root.path()).unwrap()
        ));
    }

    #[cfg(unix)]
    #[test]
    fn unix_symlinks_are_redirected() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("target");
        let link = root.path().join("link");
        std::fs::write(&target, b"target").unwrap();
        symlink(&target, &link).unwrap();

        assert!(redirected(&std::fs::symlink_metadata(link).unwrap()));
    }

    #[cfg(windows)]
    #[test]
    fn windows_reparse_attributes_are_redirected() {
        assert!(!windows_file_attributes_redirected(0));
        assert!(windows_file_attributes_redirected(0x400));
        assert!(windows_file_attributes_redirected(0x400 | 0x10));
    }

    #[tokio::test]
    async fn all_os_codes_and_atomic_substep_survive_without_a_writable_log() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("logs"), b"not a directory").unwrap();
        for code in [5, 32, 33, 112] {
            let context = PreparationContext::new(Some(Uuid::new_v4()), Some(root.path().into()));
            let error = context
                .step(
                    RetryPreparationOperation::WriteRetryReuseBarrier,
                    "project_state",
                    None,
                    async {
                        Err::<(), _>(
                            SplatError::from(std::io::Error::from_raw_os_error(code))
                                .with_io_diagnostic("io_substep=replace_state"),
                        )
                    },
                )
                .await
                .unwrap_err();
            assert_eq!(error.io_error().unwrap().raw_os_error(), Some(code));
            let snapshot = context.snapshot();
            assert!(snapshot.operation_log.contains("status=failed"));
            assert!(snapshot.operation_log.contains("io_substep=replace_state"));
            assert!(snapshot
                .operation_log
                .contains(&snapshot.preparation_id.to_string()));
            assert_eq!(snapshot.safe_details()["osError"], code);
            assert_eq!(snapshot.safe_details()["ioSubstep"], "replace_state");
            assert!(!snapshot
                .safe_details()
                .to_string()
                .contains(&root.path().to_string_lossy().to_string()));
        }
    }

    #[tokio::test]
    async fn corrupt_json_is_not_relabelled_as_io_and_logs_are_bounded() {
        let context = PreparationContext::new(None, None);
        for _ in 0..150 {
            let error = context
                .step(
                    RetryPreparationOperation::ReadRetryState,
                    "project_state",
                    None,
                    async { Err::<(), _>(serde_json::from_str::<Value>("{").unwrap_err().into()) },
                )
                .await
                .unwrap_err();
            assert!(matches!(error, SplatError::Json(_)));
        }
        let snapshot = context.snapshot();
        assert!(snapshot.operation_log.lines().count() <= LOG_LINES);
        assert!(snapshot.operation_log.len() <= LOG_BYTES);
        assert!(snapshot.safe_details()["osError"].is_null());
        assert!(snapshot.safe_details()["errorKind"].is_null());
    }

    #[tokio::test]
    async fn roles_cannot_leak_paths_and_sessions_do_not_share_buffers() {
        let first = PreparationContext::new(None, None);
        let second = PreparationContext::new(None, None);
        let _ = first
            .step(
                RetryPreparationOperation::ArchiveRetryCache,
                r"C:\private\state.json",
                Some("../cache"),
                async { Err::<(), _>(std::io::Error::from_raw_os_error(5).into()) },
            )
            .await;
        let snapshot = first.snapshot();
        assert!(!snapshot.operation_log.contains("private"));
        assert_eq!(snapshot.safe_details()["pathRole"], "unknown");
        assert!(second.snapshot().operation_log.is_empty());
    }

    #[tokio::test]
    async fn archive_observations_also_survive_unwritable_logs_and_bind_to_the_session() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("logs"), b"not a directory").unwrap();
        let context = PreparationContext::new(None, Some(root.path().into()));
        context
            .record_archive_observation("brush", &json!({"operation":"rename", "osError":32}))
            .await;
        let snapshot = context.snapshot();
        assert!(snapshot.operation_log.contains("archiveIo="));
        assert!(snapshot.operation_log.contains("\"osError\":32"));
        assert!(snapshot
            .operation_log
            .contains(&format!("preparation_id={}", snapshot.preparation_id)));
        assert!(context.registered_log().is_none());
    }

    #[tokio::test]
    async fn private_names_do_not_erase_fixed_operation_labels() {
        let context = PreparationContext::new(None, None);
        let error = context
            .step(
                RetryPreparationOperation::WriteRetryReuseBarrier,
                "project_state",
                Some("brush"),
                async {
                    Err::<(), _>(
                        SplatError::from(std::io::Error::from_raw_os_error(5))
                            .with_io_diagnostic("io_substep=replace_state"),
                    )
                },
            )
            .await
            .unwrap_err();
        let snapshot = context.snapshot();
        let text = snapshot.redacted_message(
            &format!("{error}\nC:/private/project/state"),
            &["state".into(), "project".into(), "brush".into()],
        );
        assert!(text.contains("retry_preparation_operation=write_retry_reuse_barrier"));
        assert!(text.contains("path_role=project_state"));
        assert!(text.contains("cache=brush"));
        assert!(text.contains("io_substep=replace_state"));
        assert!(text.contains("os_error=5"));
        assert!(!text.contains("C:/private"));
    }
}
