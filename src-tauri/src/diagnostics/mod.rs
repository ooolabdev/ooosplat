//! Explicit, one-shot diagnostic consent. Never uses the anonymous telemetry service.
mod hardware;
mod redact;

use crate::{
    commands::PipelineController,
    pipeline::{PipelineEngine, PipelineStage},
    project::catalog,
};
use chrono::{DateTime, Utc};
use hardware::Environment;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    future::Future,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::Mutex,
    time::Duration,
};
use tauri::State;
use uuid::Uuid;

const LOG_BYTES: usize = 64 * 1024;
const REPORT_BYTES: usize = 128 * 1024;
const ENDPOINT: &str = include_str!("../../../config/diagnostics-endpoint.txt");

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticError {
    pub code: &'static str,
    pub message: &'static str,
}
fn error(code: &'static str) -> DiagnosticError {
    DiagnosticError {
        code,
        message: match code {
            "report_expired" => "This report is not available in the current app session.",
            "report_busy" => "This report is already being sent.",
            "report_too_large" => "The report exceeds the permitted size.",
            "report_unavailable" => "The reporting service is unavailable. Try again later.",
            _ => "The report could not be sent. Please try again.",
        },
    }
}
type Result<T> = std::result::Result<T, DiagnosticError>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiagnosticReport {
    pub schema_version: u32,
    pub report_id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub app_version: String,
    pub stage: Option<PipelineStage>,
    pub engine: Option<PipelineEngine>,
    pub error_code: String,
    pub reason: String,
    pub detail: String,
    pub log_tail: String,
    pub logs_truncated: bool,
    pub environment: Environment,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportDraft {
    pub draft_id: Uuid,
    pub report: DiagnosticReport,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReportReceipt {
    pub report_id: Uuid,
    pub accepted: bool,
}

#[derive(Clone)]
struct FailureSnapshot {
    report: DiagnosticReport,
    private: Vec<String>,
}
struct Draft {
    report: DiagnosticReport,
    in_flight: bool,
    sent: bool,
}
#[derive(Default)]
struct Reports {
    failures: HashMap<Uuid, FailureSnapshot>,
    drafts: HashMap<Uuid, Draft>,
}
// Reports are bounded individually and live only in this application's memory.
// Do not expire or evict previews during the session: consent must keep referring
// to the exact content the user reviewed, including after a failed send.
#[derive(Default)]
pub struct DiagnosticService {
    reports: Mutex<Reports>,
}

fn blank_environment() -> Environment {
    Environment {
        system: hardware::SystemInfo {
            name: std::env::consts::OS.into(),
            version: None,
            arch: std::env::consts::ARCH.into(),
        },
        gpus: vec![],
        actual_device: None,
    }
}
fn reason(kind: Option<&str>) -> &'static str {
    match kind {
        Some("mapper_source") => "The images could not be connected into a reconstruction.",
        Some("mapper_storage") => "The reconstruction files could not be read.",
        Some("brush_dataset") => {
            "The training images or reconstruction data could not be read completely."
        }
        Some("brush_device_lost") => "The graphics device disconnected during training.",
        Some("brush_gpu") => "The graphics device could not complete training.",
        _ => "The generation step could not be completed. See the diagnostic details.",
    }
}

fn read_tail(path: &Path) -> std::io::Result<(String, bool)> {
    let mut file = std::fs::File::open(path)?;
    let size = file.metadata()?.len();
    let offset = size.saturating_sub(LOG_BYTES as u64);
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = Vec::new();
    file.take(LOG_BYTES as u64).read_to_end(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes);
    let text = if offset > 0 {
        text.split_once('\n').map(|(_, tail)| tail).unwrap_or("")
    } else {
        &text
    };
    let lines = text.lines().collect::<Vec<_>>();
    let truncated = offset > 0 || lines.len() > 200;
    Ok((
        redact::bounded(
            &lines[lines.len().saturating_sub(200)..].join("\n"),
            LOG_BYTES,
        ),
        truncated,
    ))
}

impl DiagnosticService {
    pub async fn capture(
        &self,
        stage: Option<PipelineStage>,
        engine: Option<PipelineEngine>,
        kind: Option<&str>,
        message: &str,
        project_id: Option<Uuid>,
        paths: &[PathBuf],
    ) -> Uuid {
        let mut private_paths = paths.to_vec();
        let mut private_names = Vec::new();
        let mut log = String::new();
        let mut logs_truncated = false;
        if let Some(project_id) = project_id {
            if let Ok((root, metadata)) = catalog::load_registered_project(project_id).await {
                private_names.push(metadata.name);
                private_paths.extend([root.clone(), metadata.source_path]);
                let names: &[&str] = match engine {
                    Some(PipelineEngine::Brush) => &["brush.log", "brush-retry.log"],
                    Some(PipelineEngine::Colmap) => &[
                        "colmap.log",
                        "colmap-bridge-features.log",
                        "colmap-bridge-matching.log",
                        "colmap-bridge-mapper.log",
                    ],
                    Some(PipelineEngine::Ffmpeg) => {
                        &["ffmpeg.log", "ffprobe.log", "ffmpeg-bridge.log"]
                    }
                    _ => &[],
                };
                let names = names.iter().map(|v| v.to_string()).collect::<Vec<_>>();
                if let Ok(Some((tail, truncated))) = tokio::task::spawn_blocking(move || {
                    let root = std::fs::canonicalize(root).ok()?;
                    let directory = std::fs::canonicalize(root.join("logs")).ok()?;
                    if !directory.starts_with(&root) {
                        return None;
                    }
                    let mut candidates = names
                        .into_iter()
                        .filter_map(|name| {
                            let path = std::fs::canonicalize(directory.join(name)).ok()?;
                            if !path.starts_with(&directory) {
                                return None;
                            }
                            Some((std::fs::metadata(&path).ok()?.modified().ok()?, path))
                        })
                        .collect::<Vec<_>>();
                    candidates.sort_by_key(|(modified, _)| *modified);
                    read_tail(&candidates.last()?.1).ok()
                })
                .await
                {
                    log = tail;
                    logs_truncated = truncated;
                }
            }
        }
        let mut private = redact::private_values(&private_paths);
        private.extend(private_names.into_iter().filter(|v| !v.is_empty()));
        let id = Uuid::new_v4();
        let report = DiagnosticReport {
            schema_version: 1,
            report_id: id,
            timestamp: Utc::now(),
            app_version: env!("CARGO_PKG_VERSION").into(),
            stage,
            engine,
            error_code: kind.unwrap_or("pipeline_failed").into(),
            reason: reason(kind).into(),
            detail: redact::bounded(&redact::sanitize(message, &private), 16 * 1024),
            log_tail: redact::bounded(&redact::sanitize(&log, &private), LOG_BYTES),
            logs_truncated,
            environment: blank_environment(),
        };
        let mut reports = self.reports.lock().unwrap_or_else(|v| v.into_inner());
        reports
            .failures
            .insert(id, FailureSnapshot { report, private });
        id
    }

    pub async fn prepare(&self, failure_id: Uuid) -> Result<ReportDraft> {
        self.prepare_with(failure_id, hardware::collect).await
    }

    async fn prepare_with<F, Fut>(&self, failure_id: Uuid, collect: F) -> Result<ReportDraft>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Environment>,
    {
        let snapshot = {
            let reports = self.reports.lock().unwrap_or_else(|v| v.into_inner());
            if let Some((id, draft)) = reports
                .drafts
                .iter()
                .find(|(_, v)| v.report.report_id == failure_id)
            {
                return Ok(ReportDraft {
                    draft_id: *id,
                    report: draft.report.clone(),
                });
            }
            reports
                .failures
                .get(&failure_id)
                .cloned()
                .ok_or_else(|| error("report_expired"))?
        };
        let mut report = snapshot.report;
        report.environment = tokio::time::timeout(Duration::from_secs(5), collect())
            .await
            .unwrap_or_else(|_| blank_environment());
        report.environment.system.name =
            redact::sanitize(&report.environment.system.name, &snapshot.private);
        report.environment.system.version = report
            .environment
            .system
            .version
            .map(|v| redact::sanitize(&v, &snapshot.private));
        for gpu in &mut report.environment.gpus {
            gpu.name = redact::sanitize(&gpu.name, &snapshot.private);
            gpu.driver_version = gpu
                .driver_version
                .take()
                .map(|v| redact::sanitize(&v, &snapshot.private));
        }
        // Never infer the actual training GPU from a preferred adapter. Only explicit log records qualify.
        report.environment.actual_device = report.log_tail.lines().find_map(|line| {
            let lower = line.to_ascii_lowercase();
            if !["using adapter", "selected adapter", "using device"]
                .iter()
                .any(|v| lower.contains(v))
            {
                return None;
            }
            report
                .environment
                .gpus
                .iter()
                .find(|gpu| !gpu.name.is_empty() && line.contains(&gpu.name))
                .map(|gpu| gpu.name.clone())
        });
        if serde_json::to_vec(&report)
            .map_err(|_| error("report_failed"))?
            .len()
            > REPORT_BYTES
        {
            report.log_tail = redact::bounded(&report.log_tail, LOG_BYTES / 2);
            report.logs_truncated = true;
        }
        if serde_json::to_vec(&report)
            .map_err(|_| error("report_failed"))?
            .len()
            > REPORT_BYTES
        {
            return Err(error("report_too_large"));
        }
        let mut reports = self.reports.lock().unwrap_or_else(|v| v.into_inner());
        // A concurrent preview must not mint a second draft for the same failure.
        if let Some((id, draft)) = reports
            .drafts
            .iter()
            .find(|(_, v)| v.report.report_id == failure_id)
        {
            return Ok(ReportDraft {
                draft_id: *id,
                report: draft.report.clone(),
            });
        }
        if !reports.failures.contains_key(&failure_id) {
            return Err(error("report_expired"));
        }
        let draft_id = Uuid::new_v4();
        reports.drafts.insert(
            draft_id,
            Draft {
                report: report.clone(),
                in_flight: false,
                sent: false,
            },
        );
        Ok(ReportDraft { draft_id, report })
    }

    async fn send_with<F, Fut>(&self, draft_id: Uuid, deliver: F) -> Result<ReportReceipt>
    where
        F: FnOnce(DiagnosticReport) -> Fut,
        Fut: Future<Output = Result<ReportReceipt>>,
    {
        let report = {
            let mut reports = self.reports.lock().unwrap_or_else(|v| v.into_inner());
            let draft = reports
                .drafts
                .get_mut(&draft_id)
                .ok_or_else(|| error("report_expired"))?;
            if draft.sent {
                return Ok(ReportReceipt {
                    report_id: draft.report.report_id,
                    accepted: true,
                });
            }
            if draft.in_flight {
                return Err(error("report_busy"));
            }
            draft.in_flight = true;
            draft.report.clone()
        };
        let id = report.report_id;
        let result = tokio::time::timeout(Duration::from_secs(10), deliver(report))
            .await
            .unwrap_or_else(|_| Err(error("report_unavailable")))
            .and_then(|receipt| {
                if receipt.accepted && receipt.report_id == id {
                    Ok(receipt)
                } else {
                    Err(error("report_unavailable"))
                }
            });
        let mut reports = self.reports.lock().unwrap_or_else(|v| v.into_inner());
        if let Some(draft) = reports.drafts.get_mut(&draft_id) {
            draft.in_flight = false;
            draft.sent = result.is_ok();
        }
        result
    }

    async fn send(&self, draft_id: Uuid) -> Result<ReportReceipt> {
        self.send_with(draft_id, |report| async move {
            let endpoint =
                reqwest::Url::parse(ENDPOINT.trim()).map_err(|_| error("report_unavailable"))?;
            if endpoint.scheme() != "https" {
                return Err(error("report_unavailable"));
            }
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .connect_timeout(Duration::from_secs(5))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|_| error("report_unavailable"))?;
            let mut response = client
                .post(endpoint)
                .header("X-OOOSplat-Report", "diagnostic-v1")
                .json(&report)
                .send()
                .await
                .map_err(|_| error("report_unavailable"))?;
            if !response.status().is_success() {
                return Err(error("report_unavailable"));
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| error("report_unavailable"))?
            {
                if bytes.len() + chunk.len() > 4096 {
                    return Err(error("report_unavailable"));
                }
                bytes.extend_from_slice(&chunk);
            }
            serde_json::from_slice(&bytes).map_err(|_| error("report_unavailable"))
        })
        .await
    }
}

#[tauri::command]
pub async fn prepare_error_report(
    state: State<'_, PipelineController>,
    failure_id: Uuid,
) -> Result<ReportDraft> {
    state.diagnostics.prepare(failure_id).await
}
#[tauri::command]
pub async fn send_error_report(
    state: State<'_, PipelineController>,
    draft_id: Uuid,
) -> Result<ReportReceipt> {
    state.diagnostics.send(draft_id).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_default_uses_the_existing_telemetry_address() {
        assert_eq!(
            ENDPOINT.trim(),
            include_str!("../../../config/telemetry-endpoint.txt").trim()
        );
        assert!(ENDPOINT.trim().starts_with("https://"));
    }
    #[tokio::test]
    async fn captures_redacted_details_without_a_project_and_rejects_forged_ids() {
        let service = DiagnosticService::default();
        let id = service
            .capture(
                Some(PipelineStage::Matching),
                Some(PipelineEngine::Colmap),
                None,
                "failed /private/file.jpg: early eof",
                None,
                &[],
            )
            .await;
        assert!(!service.reports.lock().unwrap().failures[&id]
            .report
            .detail
            .contains("private"));
        assert_eq!(
            service.prepare(Uuid::new_v4()).await.unwrap_err().code,
            "report_expired"
        );
    }
    fn draft(service: &DiagnosticService) -> Uuid {
        let id = Uuid::new_v4();
        service.reports.lock().unwrap().drafts.insert(
            id,
            Draft {
                report: DiagnosticReport {
                    schema_version: 1,
                    report_id: Uuid::new_v4(),
                    timestamp: Utc::now(),
                    app_version: "test".into(),
                    stage: None,
                    engine: None,
                    error_code: "pipeline_failed".into(),
                    reason: "test".into(),
                    detail: "early eof".into(),
                    log_tail: "tail".into(),
                    logs_truncated: false,
                    environment: blank_environment(),
                },
                in_flight: false,
                sent: false,
            },
        );
        id
    }
    #[tokio::test]
    async fn consent_sends_only_the_frozen_preview_and_deduplicates_success() {
        let service = DiagnosticService::default();
        let id = draft(&service);
        let expected =
            serde_json::to_value(&service.reports.lock().unwrap().drafts[&id].report).unwrap();
        service
            .send_with(id, |report| async move {
                assert_eq!(serde_json::to_value(&report).unwrap(), expected);
                Ok(ReportReceipt {
                    report_id: report.report_id,
                    accepted: true,
                })
            })
            .await
            .unwrap();
        service
            .send_with(id, |_| async {
                panic!("already sent");
            })
            .await
            .unwrap();
    }
    #[tokio::test]
    async fn failed_sends_can_retry_and_wrong_acknowledgements_are_not_success() {
        let service = DiagnosticService::default();
        let id = draft(&service);
        assert!(service
            .send_with(id, |_| async { Err(error("report_unavailable")) })
            .await
            .is_err());
        assert!(service
            .send_with(id, |_| async {
                Ok(ReportReceipt {
                    report_id: Uuid::new_v4(),
                    accepted: true,
                })
            })
            .await
            .is_err());
        service
            .send_with(id, |report| async move {
                Ok(ReportReceipt {
                    report_id: report.report_id,
                    accepted: true,
                })
            })
            .await
            .unwrap();
    }
    #[test]
    fn bounds_log_reads_by_bytes_and_lines() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("stage.log");
        std::fs::write(&path, "line\n".repeat(100_000)).unwrap();
        let (text, truncated) = read_tail(&path).unwrap();
        assert_eq!(text.lines().count(), 200);
        assert!(truncated);
        assert!(text.len() <= LOG_BYTES);
    }

    #[tokio::test]
    async fn prepares_a_fixed_redacted_preview_and_rejects_other_session_tokens() {
        let service = DiagnosticService::default();
        let id = service
            .capture(
                None,
                None,
                None,
                "input /private/name.png failed",
                None,
                &[],
            )
            .await;
        let first = service
            .prepare_with(id, || async { blank_environment() })
            .await
            .unwrap();
        let next = service
            .prepare_with(id, || async {
                panic!("cached preview must not re-probe hardware")
            })
            .await
            .unwrap();
        assert_eq!(first.draft_id, next.draft_id);
        assert_eq!(
            serde_json::to_value(&first.report).unwrap(),
            serde_json::to_value(&next.report).unwrap()
        );
        assert!(!first.report.detail.contains("name.png"));
        assert!(first.report.log_tail.is_empty());
        assert!(!service.reports.lock().unwrap().drafts[&first.draft_id].sent);
        assert!(serde_json::to_vec(&first.report).unwrap().len() <= REPORT_BYTES);
        let restarted = DiagnosticService::default();
        assert_eq!(
            restarted
                .prepare_with(id, || async { panic!("previous session failure") })
                .await
                .unwrap_err()
                .code,
            "report_expired"
        );
        assert_eq!(
            restarted
                .send_with(first.draft_id, |_| async {
                    panic!("previous session draft")
                })
                .await
                .unwrap_err()
                .code,
            "report_expired"
        );
    }

    #[tokio::test]
    async fn reports_remain_available_for_the_entire_app_session() {
        let service = DiagnosticService::default();
        let first = service
            .capture(None, None, None, "first failure", None, &[])
            .await;
        let unprepared = service
            .capture(None, None, None, "unprepared failure", None, &[])
            .await;
        let old_timestamp = Utc::now() - chrono::Duration::days(1);
        service
            .reports
            .lock()
            .unwrap()
            .failures
            .get_mut(&first)
            .unwrap()
            .report
            .timestamp = old_timestamp;
        service
            .reports
            .lock()
            .unwrap()
            .failures
            .get_mut(&unprepared)
            .unwrap()
            .report
            .timestamp = old_timestamp;
        let original = service
            .prepare_with(first, || async { blank_environment() })
            .await
            .unwrap();
        // Neither elapsed report age nor creating more than ten reports may
        // invalidate a reviewed draft or a failure waiting to be reviewed.
        for _ in 0..12 {
            let id = service
                .capture(None, None, None, "another failure", None, &[])
                .await;
            service
                .prepare_with(id, || async { blank_environment() })
                .await
                .unwrap();
        }
        let cached = service
            .prepare_with(first, || async { panic!("must keep the reviewed report") })
            .await
            .unwrap();
        assert_eq!(cached.draft_id, original.draft_id);
        assert_eq!(cached.report.timestamp, old_timestamp);
        let later = service
            .prepare_with(unprepared, || async { blank_environment() })
            .await
            .unwrap();
        assert_eq!(later.report.timestamp, old_timestamp);
        assert!(service
            .send_with(original.draft_id, |_| async {
                Err(error("report_unavailable"))
            })
            .await
            .is_err());
        service
            .send_with(original.draft_id, |report| async move {
                assert_eq!(report.timestamp, old_timestamp);
                Ok(ReportReceipt {
                    report_id: report.report_id,
                    accepted: true,
                })
            })
            .await
            .unwrap();
        service
            .send_with(original.draft_id, |_| async {
                panic!("successful reports cannot be resent")
            })
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn only_explicit_engine_logs_identify_the_actual_gpu() {
        let service = DiagnosticService::default();
        let id = service
            .capture(
                None,
                Some(PipelineEngine::Brush),
                None,
                "Device Lost",
                None,
                &[],
            )
            .await;
        service
            .reports
            .lock()
            .unwrap()
            .failures
            .get_mut(&id)
            .unwrap()
            .report
            .log_tail = "preferred adapter: NVIDIA RTX\nusing adapter: Intel GPU".into();
        let preview = service
            .prepare_with(id, || async {
                let mut env = blank_environment();
                env.gpus = ["Intel GPU", "NVIDIA RTX"]
                    .into_iter()
                    .map(|name| hardware::GpuInfo {
                        name: name.into(),
                        driver_version: None,
                        total_memory_mb: None,
                        compute_capability: None,
                    })
                    .collect();
                env
            })
            .await
            .unwrap();
        assert_eq!(
            preview.report.environment.actual_device.as_deref(),
            Some("Intel GPU")
        );
        assert_eq!(preview.report.environment.gpus.len(), 2);
        // Later retries cannot alter the previously displayed payload.
        service
            .reports
            .lock()
            .unwrap()
            .failures
            .get_mut(&id)
            .unwrap()
            .report
            .log_tail = "using adapter: NVIDIA RTX".into();
        assert_eq!(
            service
                .prepare_with(id, || async { blank_environment() })
                .await
                .unwrap()
                .report
                .log_tail,
            preview.report.log_tail
        );
    }

    #[tokio::test]
    async fn concurrent_send_is_rejected_without_duplicate_delivery() {
        let service = DiagnosticService::default();
        let id = draft(&service);
        let (started, started_rx) = tokio::sync::oneshot::channel();
        let (release, release_rx) = tokio::sync::oneshot::channel();
        let first = service.send_with(id, |report| async move {
            started.send(()).unwrap();
            release_rx.await.unwrap();
            Ok(ReportReceipt {
                report_id: report.report_id,
                accepted: true,
            })
        });
        let second = async {
            started_rx.await.unwrap();
            assert_eq!(
                service
                    .send_with(id, |_| async { panic!("duplicate delivery") })
                    .await
                    .unwrap_err()
                    .code,
                "report_busy"
            );
            release.send(()).unwrap();
        };
        let (receipt, _) = tokio::join!(first, second);
        assert!(receipt.unwrap().accepted);
    }

    #[tokio::test]
    async fn timed_out_send_unlocks_draft_for_manual_retry() {
        let service = DiagnosticService::default();
        let id = draft(&service);
        assert_eq!(
            service
                .send_with(id, |_| std::future::pending())
                .await
                .unwrap_err()
                .code,
            "report_unavailable"
        );
        assert!(!service.reports.lock().unwrap().drafts[&id].in_flight);
        service
            .send_with(id, |report| async move {
                Ok(ReportReceipt {
                    report_id: report.report_id,
                    accepted: true,
                })
            })
            .await
            .unwrap();
    }
}
