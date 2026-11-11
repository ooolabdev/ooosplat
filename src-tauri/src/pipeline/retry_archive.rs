//! Narrow diagnostics and bounded sharing-conflict retries for cache renames.
//! This does not repair permissions, change attributes, copy/delete caches, or
//! infer that an AccessDenied error is a transient file lock.

use std::{future::Future, io, path::Path, time::Instant};

use serde::Serialize;
use tokio_util::sync::CancellationToken;

use crate::error::{Result, SplatError};

const RETRY_DELAYS_MS: [u64; 3] = [50, 150, 350];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct EntryMetadata {
    pub file_type: &'static str,
    pub length: u64,
    pub readonly: bool,
    pub windows_attributes: Option<u32>,
}

impl EntryMetadata {
    fn from_metadata(metadata: std::fs::Metadata) -> Self {
        #[cfg(windows)]
        let windows_attributes = {
            use std::os::windows::fs::MetadataExt;
            Some(metadata.file_attributes())
        };
        #[cfg(not(windows))]
        let windows_attributes = None;
        Self {
            file_type: if metadata.file_type().is_symlink() {
                "symlink"
            } else if metadata.is_dir() {
                "directory"
            } else if metadata.is_file() {
                "file"
            } else {
                "other"
            },
            length: metadata.len(),
            readonly: metadata.permissions().readonly(),
            windows_attributes,
        }
    }
}

/// Contains roles and metadata only, never source/destination path strings.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ArchiveIoObservation {
    pub operation: &'static str,
    pub path_role: &'static str,
    pub outcome: &'static str,
    pub attempt: u8,
    pub elapsed_ms: u64,
    pub wait_ms: Option<u64>,
    pub error_kind: Option<String>,
    pub os_error: Option<i32>,
    pub source: Option<EntryMetadata>,
    pub destination: Option<EntryMetadata>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ArchiveRenameSummary {
    pub attempts: u8,
    pub elapsed_ms: u64,
}

fn cancelled(cancel: &CancellationToken) -> Result<()> {
    if cancel.is_cancelled() {
        Err(SplatError::Cancelled)
    } else {
        Ok(())
    }
}

/// Only an actual NotFound means absence. Permission and other I/O errors keep
/// their original std::io::Error for the caller's semantic diagnostic wrapper.
pub(super) async fn inspect_entry<O, F>(
    path: &Path,
    path_role: &'static str,
    cancel: &CancellationToken,
    mut observe: O,
) -> Result<Option<EntryMetadata>>
where
    O: FnMut(ArchiveIoObservation) -> F,
    F: Future<Output = ()>,
{
    cancelled(cancel)?;
    let started = Instant::now();
    let result = tokio::fs::symlink_metadata(path).await;
    let error = result.as_ref().err();
    let observation = ArchiveIoObservation {
        operation: "inspect_retry_cache",
        path_role,
        outcome: match error {
            Some(error) if error.kind() == io::ErrorKind::NotFound => "not_found",
            Some(_) => "failed",
            None => "completed",
        },
        attempt: 1,
        elapsed_ms: started.elapsed().as_millis() as u64,
        wait_ms: None,
        error_kind: error.map(|error| format!("{:?}", error.kind())),
        os_error: error.and_then(io::Error::raw_os_error),
        source: result
            .as_ref()
            .ok()
            .cloned()
            .map(EntryMetadata::from_metadata),
        destination: None,
    };
    observe(observation).await;
    cancelled(cancel)?;
    match result {
        Ok(metadata) => Ok(Some(EntryMetadata::from_metadata(metadata))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub(super) async fn rename_entry<O, F>(
    source: &Path,
    destination: &Path,
    cancel: &CancellationToken,
    mut observe: O,
) -> Result<ArchiveRenameSummary>
where
    O: FnMut(ArchiveIoObservation) -> F,
    F: Future<Output = ()>,
{
    let source_metadata = inspect_entry(source, "cache_entry", cancel, &mut observe).await?;
    let destination_metadata =
        inspect_entry(destination, "cache_archive_entry", cancel, &mut observe).await?;
    rename_with(
        cancel,
        cfg!(windows),
        source_metadata,
        destination_metadata,
        || tokio::fs::rename(source, destination),
        |delay| tokio::time::sleep(std::time::Duration::from_millis(delay)),
        observe,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn rename_with<Operation, OperationFuture, Wait, WaitFuture, Observe, ObserveFuture>(
    cancel: &CancellationToken,
    windows: bool,
    source: Option<EntryMetadata>,
    destination: Option<EntryMetadata>,
    mut operation: Operation,
    mut wait: Wait,
    mut observe: Observe,
) -> Result<ArchiveRenameSummary>
where
    Operation: FnMut() -> OperationFuture,
    OperationFuture: Future<Output = io::Result<()>>,
    Wait: FnMut(u64) -> WaitFuture,
    WaitFuture: Future<Output = ()>,
    Observe: FnMut(ArchiveIoObservation) -> ObserveFuture,
    ObserveFuture: Future<Output = ()>,
{
    let started = Instant::now();
    for attempt in 1..=4_u8 {
        cancelled(cancel)?;
        let attempt_started = Instant::now();
        let result = operation().await;
        let error = result.as_ref().err();
        observe(ArchiveIoObservation {
            operation: "rename_retry_cache",
            path_role: "cache_entry",
            outcome: if result.is_ok() {
                "completed"
            } else {
                "failed"
            },
            attempt,
            elapsed_ms: attempt_started.elapsed().as_millis() as u64,
            wait_ms: None,
            error_kind: error.map(|error| format!("{:?}", error.kind())),
            os_error: error.and_then(io::Error::raw_os_error),
            source: source.clone(),
            destination: destination.clone(),
        })
        .await;
        let error = match result {
            Ok(()) => {
                // A successful rename must be committed by the caller even if
                // cancellation arrives now; do not report a moved cache as an
                // operation that never happened.
                return Ok(ArchiveRenameSummary {
                    attempts: attempt,
                    elapsed_ms: started.elapsed().as_millis() as u64,
                });
            }
            Err(error) => error,
        };
        let retryable = windows && matches!(error.raw_os_error(), Some(32 | 33));
        if !retryable || attempt == 4 {
            return Err(error.into());
        }
        let delay = RETRY_DELAYS_MS[usize::from(attempt - 1)];
        observe(ArchiveIoObservation {
            operation: "wait_retry_cache_sharing_conflict",
            path_role: "cache_entry",
            outcome: "scheduled",
            attempt,
            elapsed_ms: started.elapsed().as_millis() as u64,
            wait_ms: Some(delay),
            error_kind: Some(format!("{:?}", error.kind())),
            os_error: error.raw_os_error(),
            source: source.clone(),
            destination: destination.clone(),
        })
        .await;
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(SplatError::Cancelled),
            _ = wait(delay) => {}
        }
    }
    unreachable!("The fourth failure always returns its original error")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[tokio::test]
    async fn windows_sharing_and_lock_violations_retry_with_only_the_bounded_delays() {
        for code in [32, 33] {
            let mut calls = 0;
            let waits = Arc::new(Mutex::new(Vec::new()));
            let recorded_waits = waits.clone();
            let summary = rename_with(
                &CancellationToken::new(),
                true,
                None,
                None,
                || {
                    calls += 1;
                    std::future::ready(if calls <= 3 {
                        Err(io::Error::from_raw_os_error(code))
                    } else {
                        Ok(())
                    })
                },
                move |delay| {
                    recorded_waits.lock().unwrap().push(delay);
                    std::future::ready(())
                },
                |_| std::future::ready(()),
            )
            .await
            .unwrap();
            assert_eq!(calls, 4);
            assert_eq!(summary.attempts, 4);
            assert_eq!(*waits.lock().unwrap(), RETRY_DELAYS_MS);
        }
    }

    #[tokio::test]
    async fn access_denied_and_nonwindows_errors_are_not_retried() {
        for (windows, code) in [(true, 5), (true, 2), (false, 32), (false, 33)] {
            let mut calls = 0;
            let error = rename_with(
                &CancellationToken::new(),
                windows,
                None,
                None,
                || {
                    calls += 1;
                    std::future::ready(Err(io::Error::from_raw_os_error(code)))
                },
                |_| async { panic!("This error must not schedule a retry") },
                |_| std::future::ready(()),
            )
            .await
            .unwrap_err();
            assert_eq!(calls, 1);
            assert_eq!(
                error.io_error().and_then(io::Error::raw_os_error),
                Some(code)
            );
        }
    }

    #[tokio::test]
    async fn exhausted_sharing_retries_preserve_the_last_os_error_and_observations() {
        let mut calls = 0;
        let observations = Arc::new(Mutex::new(Vec::new()));
        let recorded = observations.clone();
        let error = rename_with(
            &CancellationToken::new(),
            true,
            None,
            None,
            || {
                calls += 1;
                std::future::ready(Err(io::Error::from_raw_os_error(if calls == 4 {
                    33
                } else {
                    32
                })))
            },
            |_| std::future::ready(()),
            move |event| {
                recorded.lock().unwrap().push(event);
                std::future::ready(())
            },
        )
        .await
        .unwrap_err();
        assert_eq!(calls, 4);
        assert_eq!(error.io_error().and_then(io::Error::raw_os_error), Some(33));
        let observations = observations.lock().unwrap();
        assert_eq!(
            observations
                .iter()
                .filter(|event| event.operation == "rename_retry_cache")
                .count(),
            4
        );
        assert_eq!(
            observations
                .iter()
                .filter(|event| event.wait_ms.is_some())
                .count(),
            3
        );
    }

    #[tokio::test]
    async fn cancellation_during_the_delay_prevents_the_next_rename() {
        let cancel = CancellationToken::new();
        let during_wait = cancel.clone();
        let mut calls = 0;
        let error = rename_with(
            &cancel,
            true,
            None,
            None,
            || {
                calls += 1;
                std::future::ready(Err(io::Error::from_raw_os_error(32)))
            },
            move |_| {
                during_wait.cancel();
                std::future::ready(())
            },
            |_| std::future::ready(()),
        )
        .await
        .unwrap_err();
        assert!(matches!(error, SplatError::Cancelled));
        assert_eq!(calls, 1);
    }

    #[tokio::test]
    async fn cancellation_before_start_performs_no_rename() {
        let cancel = CancellationToken::new();
        cancel.cancel();
        let error = rename_with(
            &cancel,
            true,
            None,
            None,
            || async { panic!("A cancelled operation must not rename") },
            |_| std::future::ready(()),
            |_| std::future::ready(()),
        )
        .await
        .unwrap_err();
        assert!(matches!(error, SplatError::Cancelled));
    }

    #[tokio::test]
    async fn inspection_distinguishes_absence_and_preserves_metadata_without_paths() {
        let root = tempfile::tempdir().unwrap();
        let cancel = CancellationToken::new();
        assert!(
            inspect_entry(&root.path().join("missing"), "cache_entry", &cancel, |_| {
                std::future::ready(())
            })
            .await
            .unwrap()
            .is_none()
        );
        let entry = inspect_entry(root.path(), "cache_entry", &cancel, |_| {
            std::future::ready(())
        })
        .await
        .unwrap()
        .unwrap();
        assert_eq!(entry.file_type, "directory");
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn a_real_windows_locked_child_is_not_bypassed_and_unlocking_allows_rename() {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};

        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("brush");
        let destination = root.path().join("archive/brush");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
        std::fs::write(source.join("sentinel"), b"unchanged").unwrap();
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .open(source.join("sentinel"))
            .unwrap();
        let error = rename_entry(&source, &destination, &CancellationToken::new(), |_| {
            std::future::ready(())
        })
        .await
        .unwrap_err();
        assert!(matches!(
            error.io_error().and_then(io::Error::raw_os_error),
            Some(5 | 32 | 33)
        ));
        assert!(source.is_dir());
        assert!(!destination.exists());
        drop(lock);
        let summary = rename_entry(&source, &destination, &CancellationToken::new(), |_| {
            std::future::ready(())
        })
        .await
        .unwrap();
        assert_eq!(summary.attempts, 1);
        assert!(!source.exists());
        assert_eq!(
            std::fs::read(destination.join("sentinel")).unwrap(),
            b"unchanged"
        );
    }
}
