//! Classification of new failures. Native status and explicit engine evidence
//! survive IPC; presentation never reclassifies a truncated/historical log.
use crate::{
    error::SplatError,
    pipeline::{PipelineEngine, PipelineStage},
    process::{ProcessOutput, ProcessTermination},
};

pub const CLASSIFICATION_VERSION: u32 = 1;

pub fn known_kind(value: Option<&str>) -> Option<&'static str> {
    [
        "brush_out_of_memory",
        "brush_resource_limit",
        "colmap_crash",
        "mapper_source",
        "mapper_storage",
        "brush_dataset",
        "brush_device_lost",
        "brush_gpu",
    ]
    .into_iter()
    .find(|kind| Some(*kind) == value)
}

#[derive(Debug, Clone, Copy)]
pub struct FailureClassification {
    pub engine: Option<PipelineEngine>,
    pub kind: Option<&'static str>,
    pub heuristic: bool,
}

/// Small, process-local evidence collected before output tail truncation. This
/// is not serialized: only the bounded detail and existing classification cross
/// IPC. A new accumulator is required for every engine invocation.
#[derive(Debug, Clone, Default)]
pub struct EngineFailureEvidence {
    oom: Vec<String>,
    resource_limit: Vec<String>,
    device_lost: Vec<String>,
    unsupported_feature: Vec<String>,
    storage: Vec<String>,
    dataset: Vec<String>,
    mapper: Vec<String>,
    mapper_terminal: bool,
    mapper_operational_error: bool,
    gpu_initialization_started: bool,
    gpu_initialization_completed: bool,
    brush_final_iteration: Option<u64>,
    brush_export_failure: Option<(u64, Option<i32>, String)>,
    brush_any_export_failure: bool,
    brush_numeric_divergence: Vec<String>,
}

impl EngineFailureEvidence {
    /// The export contract belongs to this invocation, never to a previous
    /// log file or a guessed last progress line.
    pub fn brush_training(final_iteration: u64) -> Self {
        Self {
            brush_final_iteration: Some(final_iteration),
            ..Self::default()
        }
    }

    pub fn brush_final_export_failed(&self) -> bool {
        self.brush_export_failure.is_some()
    }

    pub fn observe(&mut self, line: &str) {
        fn keep(lines: &mut Vec<String>, line: &str, matched: bool) {
            if matched && lines.len() < 4 {
                let line: String = line.chars().take(512).collect();
                if !lines.contains(&line) {
                    lines.push(line);
                }
            }
        }
        keep(&mut self.oom, line, gpu_oom(line));
        keep(&mut self.resource_limit, line, gpu_resource_limit(line));
        keep(&mut self.device_lost, line, device_lost(line));
        keep(
            &mut self.unsupported_feature,
            line,
            gpu_feature_unsupported(line),
        );
        keep(&mut self.storage, line, storage(line));
        keep(&mut self.dataset, line, dataset(line));
        keep(&mut self.mapper, line, mapper_source(line));
        let lower = line.to_ascii_lowercase();
        self.mapper_terminal |= lower.contains("failed to create any sparse model");
        self.mapper_operational_error |= mapper_operational_error(line);
        self.gpu_initialization_started |= lower.contains("gpu initialization started");
        self.gpu_initialization_completed |= lower.contains("gpu initialization completed");
        keep(
            &mut self.brush_numeric_divergence,
            line,
            brush_numeric_divergence(line),
        );
        self.brush_any_export_failure |= brush_export_failure_iteration(line).is_some();
        if let Some(iteration) = brush_export_failure_iteration(line)
            .filter(|iteration| Some(*iteration) == self.brush_final_iteration)
            .filter(|_| self.brush_export_failure.is_none())
        {
            self.brush_export_failure = Some((
                iteration,
                reported_os_error(line),
                line.chars().take(1_024).collect(),
            ));
        }
    }

    fn from_output(output: &ProcessOutput) -> Self {
        let mut evidence = Self::default();
        for line in output.stdout.lines().chain(output.stderr.lines()) {
            evidence.observe(line);
        }
        evidence
    }
}

/// Internal execution evidence; not serialized into the project or wire schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrushFailureReason {
    FinalExport {
        iteration: u64,
        /// An error reported by Brush, not a fabricated application I/O error.
        reported_os_error: Option<i32>,
    },
    NumericDivergence,
}

#[derive(Debug, Clone)]
pub struct EngineProcessFailure {
    pub last_splat_count: Option<u64>,
    pub engine: PipelineEngine,
    pub operation: String,
    pub termination: ProcessTermination,
    pub detail: String,
    pub classification: FailureClassification,
    pub previous_attempts: Vec<EngineProcessFailure>,
    /// A narrow execution decision, not the heuristic mapper_source UI kind.
    pub mapper_geometry_failure: bool,
    pub brush_failure_reason: Option<BrushFailureReason>,
}

impl EngineProcessFailure {
    pub fn from_output(engine: PipelineEngine, operation: &str, output: &ProcessOutput) -> Self {
        Self::from_output_with_evidence(
            engine,
            operation,
            output,
            &EngineFailureEvidence::from_output(output),
        )
    }

    pub fn from_output_with_evidence(
        engine: PipelineEngine,
        operation: &str,
        output: &ProcessOutput,
        evidence: &EngineFailureEvidence,
    ) -> Self {
        let mut classification = FailureClassification {
            engine: Some(engine),
            kind: None,
            heuristic: false,
        };
        let (kind, lines): (Option<&'static str>, &[String]) = match engine {
            // A known failed final export cannot be rescued by retraining.
            // Keep its native termination and reported storage cause even if
            // an earlier warning also happened to mention a GPU failure.
            PipelineEngine::Brush if evidence.brush_final_export_failed() => (None, &[]),
            PipelineEngine::Brush if !evidence.oom.is_empty() => {
                (Some("brush_out_of_memory"), &evidence.oom)
            }
            PipelineEngine::Brush if !evidence.resource_limit.is_empty() => {
                (Some("brush_resource_limit"), &evidence.resource_limit)
            }
            PipelineEngine::Brush if !evidence.device_lost.is_empty() => {
                (Some("brush_device_lost"), &evidence.device_lost)
            }
            PipelineEngine::Brush if !evidence.unsupported_feature.is_empty() => {
                (Some("brush_gpu"), &evidence.unsupported_feature)
            }
            PipelineEngine::Brush
                if output.termination.abnormal()
                    && evidence.gpu_initialization_started
                    && !evidence.gpu_initialization_completed =>
            {
                (Some("brush_gpu"), &[])
            }
            PipelineEngine::Colmap if output.termination.abnormal() => (Some("colmap_crash"), &[]),
            PipelineEngine::Colmap if !evidence.storage.is_empty() => {
                classification.heuristic = true;
                (Some("mapper_storage"), &evidence.storage)
            }
            PipelineEngine::Colmap if operation == "mapper" && !evidence.mapper.is_empty() => {
                classification.heuristic = true;
                (Some("mapper_source"), &evidence.mapper)
            }
            PipelineEngine::Brush if !evidence.dataset.is_empty() => {
                classification.heuristic = true;
                (Some("brush_dataset"), &evidence.dataset)
            }
            _ => (None, &[]),
        };
        classification.kind = kind;
        // Preserve bounded root evidence from BOTH current streams, including
        // early lines outside failure_detail(). No full output crosses IPC.
        let brush_failure_reason = if engine == PipelineEngine::Brush {
            if let Some((iteration, reported_os_error, _)) = &evidence.brush_export_failure {
                Some(BrushFailureReason::FinalExport {
                    iteration: *iteration,
                    reported_os_error: *reported_os_error,
                })
            } else if kind.is_none()
                && !output.success
                && !output.termination.abnormal()
                && output.termination.exit_code.is_some_and(|code| code != 101)
                && evidence.storage.is_empty()
                && evidence.dataset.is_empty()
                && !evidence.mapper_operational_error
                && !evidence.brush_any_export_failure
                && !evidence.brush_numeric_divergence.is_empty()
            {
                Some(BrushFailureReason::NumericDivergence)
            } else {
                None
            }
        } else {
            None
        };
        let mut root_lines = lines.join("\n");
        match brush_failure_reason {
            Some(BrushFailureReason::FinalExport { .. }) => {
                if let Some((_, _, line)) = &evidence.brush_export_failure {
                    if !root_lines.is_empty() {
                        root_lines.push('\n');
                    }
                    root_lines.push_str(line);
                }
            }
            Some(BrushFailureReason::NumericDivergence) => {
                root_lines = evidence.brush_numeric_divergence.join("\n");
            }
            None => {}
        }
        let tail = output.failure_detail();
        let mut detail = if root_lines.is_empty() {
            tail
        } else {
            format!("{root_lines}\n--- current attempt tail ---\n{tail}")
        };
        if engine == PipelineEngine::Brush {
            if let Some(BrushFailureReason::FinalExport {
                iteration,
                reported_os_error,
            }) = brush_failure_reason
            {
                detail.push_str(&format!(
                    "\nfailure_reason=brush_final_export_failed\nexport_iteration={iteration}\nreported_os_error={reported_os_error:?}"
                ));
            }
            if kind == Some("brush_gpu") && !evidence.unsupported_feature.is_empty() {
                detail.push_str("\nfailure_reason=gpu_feature_unsupported");
            } else if kind == Some("brush_gpu")
                && output.termination.abnormal()
                && evidence.gpu_initialization_started
                && !evidence.gpu_initialization_completed
            {
                detail.push_str("\nfailure_reason=gpu_initialization_terminated");
            }
            if evidence.gpu_initialization_started && !evidence.gpu_initialization_completed {
                detail.push_str("\nexecution_phase=gpu_initialization");
            }
        }
        Self {
            // Brush fills this from its per-process observer, which preserves
            // cross-stream arrival order and survives bounded output capture.
            last_splat_count: None,
            engine,
            operation: operation.into(),
            termination: output.termination.clone(),
            detail,
            classification,
            previous_attempts: vec![],
            mapper_geometry_failure: engine == PipelineEngine::Colmap
                && operation == "mapper"
                && !output.success
                && output.exit_code == Some(1)
                && output.termination.exit_code == Some(1)
                && !output.termination.abnormal()
                && evidence.mapper_terminal
                && !evidence.mapper_operational_error,
            brush_failure_reason,
        }
    }

    pub fn is_mapper_geometry_failure(&self) -> bool {
        self.mapper_geometry_failure
    }
}

impl std::fmt::Display for EngineProcessFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let engine = match self.engine {
            PipelineEngine::Colmap => "COLMAP",
            PipelineEngine::Brush => "Brush",
            _ => "Engine",
        };
        write!(
            f,
            "{engine} {} attempt {}: {}\n{}",
            self.operation,
            self.previous_attempts.len() + 1,
            self.termination,
            self.detail
        )?;
        for (index, previous) in self.previous_attempts.iter().enumerate() {
            write!(
                f,
                "\n--- previous attempt {} (context only) ---\n{}\n{}",
                index + 1,
                previous.termination,
                previous.detail
            )?;
        }
        Ok(())
    }
}

pub fn classify(stage: Option<PipelineStage>, error: &SplatError) -> FailureClassification {
    if let SplatError::Diagnostic { source, .. } = error {
        return classify(stage, source);
    }
    if let SplatError::EngineProcess(failure) = error {
        return failure.classification;
    }
    let engine = match stage {
        Some(
            PipelineStage::Reconstructing
            | PipelineStage::ValidatingReconstruction
            | PipelineStage::ExtractingFeatures
            | PipelineStage::Matching,
        ) => PipelineEngine::Colmap,
        Some(PipelineStage::TrainingSplats) => PipelineEngine::Brush,
        Some(PipelineStage::ProbingVideo | PipelineStage::ExtractingFrames) => {
            PipelineEngine::Ffmpeg
        }
        _ => PipelineEngine::System,
    };
    let mut result = FailureClassification {
        engine: Some(engine),
        kind: None,
        heuristic: false,
    };
    match error {
        SplatError::Cancelled => return result,
        SplatError::BrushOutOfMemory(_) => {
            result.engine = Some(PipelineEngine::Brush);
            result.kind = Some("brush_out_of_memory");
        }
        SplatError::BrushDeviceLost(_) => {
            result.engine = Some(PipelineEngine::Brush);
            result.kind = Some("brush_device_lost");
        }
        SplatError::Io(_) if engine == PipelineEngine::Colmap => {
            result.kind = Some("mapper_storage")
        }
        SplatError::Process(message) => {
            result.kind = if engine == PipelineEngine::Colmap && storage(message) {
                Some("mapper_storage")
            } else if matches!(
                stage,
                Some(PipelineStage::Reconstructing | PipelineStage::ValidatingReconstruction)
            ) && mapper_source(message)
            {
                Some("mapper_source")
            } else if engine == PipelineEngine::Brush && gpu_oom(message) {
                Some("brush_out_of_memory")
            } else if engine == PipelineEngine::Brush && gpu_resource_limit(message) {
                Some("brush_resource_limit")
            } else if engine == PipelineEngine::Brush && device_lost(message) {
                Some("brush_device_lost")
            } else if engine == PipelineEngine::Brush && dataset(message) {
                Some("brush_dataset")
            } else {
                None
            };
            result.heuristic = result.kind.is_some();
        }
        _ => {}
    }
    result
}

pub fn gpu_oom(text: &str) -> bool {
    text.lines().any(|line| {
        let lower = line.to_ascii_lowercase();
        if [
            "out_of_host_memory",
            "host memory",
            "cpu memory",
            "system memory",
        ]
        .iter()
        .any(|s| lower.contains(s))
        {
            return false;
        }
        [
            "vk_error_out_of_device_memory",
            "error_out_of_device_memory",
            "cuda_error_out_of_memory",
            "dxgi_error_out_of_memory",
        ]
        .iter()
        .any(|s| lower.contains(s))
            || ((lower.contains("out of memory") || lower.contains("outofmemory"))
                && [
                    "wgpu", "gpu", "vram", "cuda", "vulkan", "metal", "d3d", "显存",
                ]
                .iter()
                .any(|s| lower.contains(s)))
    })
}
pub fn gpu_resource_limit(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    [
        "buffertoobig",
        "buffer too big",
        "exceeds max_buffer_size",
        "exceeds the maximum buffer size",
    ]
    .iter()
    .any(|s| lower.contains(s))
}
pub fn device_lost(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    [
        "devicelost",
        "device lost",
        "device_lost",
        "parent device is lost",
        "vk_error_device_lost",
        "dxgi_error_device_removed",
        "显卡设备连接中断",
    ]
    .iter()
    .any(|s| lower.contains(s))
}

fn gpu_feature_unsupported(text: &str) -> bool {
    text.to_ascii_lowercase()
        .contains("requested feature is not available on this device")
}

fn brush_numeric_divergence(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    [
        "non-finite training loss",
        "training loss became nan",
        "numerical divergence",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

/// Match the bundled CLI's export event, not a generic `failed` warning.
/// The caller additionally binds the parsed iteration to the planned final
/// export. Earlier periodic exports do not establish a final-export failure.
fn brush_export_failure_iteration(text: &str) -> Option<u64> {
    let text = text.trim();
    let message = if text.starts_with('[') {
        text.split_once("] ")?.1
    } else {
        text
    };
    let rest = message.strip_prefix("Export at iteration ")?;
    let (iteration, rest) = rest.split_once(' ')?;
    if iteration.is_empty() || !iteration.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let suffix = rest.strip_prefix("failed")?;
    if !(suffix.is_empty() || suffix.starts_with(':')) {
        return None;
    }
    let iteration = iteration.parse().ok()?;
    // The nested train_stream error includes the destination. If present, it
    // must be this invocation's configured candidate, not an evaluation file.
    if suffix
        .split_once(&format!(": iteration {iteration}, "))
        .is_some_and(|(_, destination)| {
            !destination.starts_with("final.ply.tmp:")
                && !destination.starts_with("final.ply.tmp.ply:")
        })
    {
        return None;
    }
    Some(iteration)
}

fn reported_os_error(text: &str) -> Option<i32> {
    let (_, suffix) = text.split_once("(os error ")?;
    let (code, _) = suffix.split_once(')')?;
    code.parse().ok()
}

/// Parse the native glog header, not an arbitrary English word beginning with
/// E/F. The bundled engine emits YYYYMMDD; older glog builds use MMDD.
fn glog_severity_and_message(line: &str) -> Option<(u8, &str)> {
    let (header, message) = line.trim_start().split_once(']')?;
    let fields: Vec<_> = header.split_whitespace().collect();
    let [date, time, thread, location] = fields.as_slice() else {
        return None;
    };
    let severity = *date.as_bytes().first()?;
    if !matches!(severity, b'I' | b'W' | b'E' | b'F')
        || !matches!(date.len(), 5 | 9)
        || !date.as_bytes()[1..].iter().all(u8::is_ascii_digit)
        || !thread.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let mut clock = time.split(':');
    let hour = clock.next()?;
    let minute = clock.next()?;
    let second = clock.next()?;
    if clock.next().is_some() {
        return None;
    }
    let (second, fraction) = second.split_once('.')?;
    for (value, maximum) in [(hour, 23), (minute, 59), (second, 60)] {
        if value.len() != 2
            || !value.bytes().all(|b| b.is_ascii_digit())
            || value.parse::<u32>().ok()? > maximum
        {
            return None;
        }
    }
    if fraction.is_empty() || !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let (file, line_number) = location.rsplit_once(':')?;
    if file.is_empty() || line_number.is_empty() || !line_number.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    Some((severity, message.trim()))
}

fn exact_no_sparse_model(message: &str) -> bool {
    message
        .trim()
        .trim_end_matches('.')
        .eq_ignore_ascii_case("Failed to create any sparse model")
}

fn mapper_operational_error(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    storage(text)
        || dataset(text)
        || gpu_oom(text)
        || gpu_resource_limit(text)
        || device_lost(text)
        || [
            "check failed",
            "assertion",
            "fatal error",
            "segmentation fault",
            "access violation",
            "terminate called",
            "panic",
            "std::bad_alloc",
            "out of memory",
            "out_of_memory",
            "failed to read",
            "failed to write",
            "failed to open",
            "failed to load",
            "failed to allocate",
            "cannot open",
            "could not open",
            "cannot read",
            "could not read",
            "unexpected eof",
            "unexpected end of file",
            "resource exhausted",
            "out_of_host_memory",
            "file does not exist",
            "no such file",
            "malformed",
            "database disk image",
        ]
        .iter()
        .any(|marker| lower.contains(marker))
        || lower
            .trim_start()
            .strip_prefix("error:")
            .is_some_and(|message| !exact_no_sparse_model(message))
        || glog_severity_and_message(text).is_some_and(|(severity, message)| {
            // A fatal is never the normal COLMAP exit-1 geometric rejection.
            severity == b'F' || (severity == b'E' && !exact_no_sparse_model(message))
        })
        || (lower.contains("sqlite")
            && ["error", "busy", "locked", "corrupt", "not a database"]
                .iter()
                .any(|marker| lower.contains(marker)))
}

fn dataset(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    [
        "early eof",
        "failed to load dataset",
        "no such file",
        "file not found",
        "access denied",
        "permission denied",
    ]
    .iter()
    .any(|s| lower.contains(s))
}
fn storage(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    [
        "database is locked",
        "database locked",
        "unable to open database",
        "failed to open database",
        "cannot open database",
        "read-only database",
        "readonly database",
        "no space left",
        "disk full",
        "input/output error",
        "being used by another process",
        "used by another process",
        "sharing violation",
        "access denied",
        "permission denied",
        "数据库被锁定",
        "无法打开数据库",
        "磁盘空间不足",
        "磁盘已满",
        "另一个进程正在使用",
        "拒绝访问",
        "权限不足",
    ]
    .iter()
    .any(|s| lower.contains(s))
        || ((lower.contains("io error") || lower.contains("i/o error"))
            && ["database", "sqlite", "filesystem", "project file"]
                .iter()
                .any(|s| lower.contains(s)))
}
fn mapper_source(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    // A single rejected registration or discarded submodel is normal mapper
    // activity, not proof that reconstruction as a whole failed.
    [
        "no initial image pair",
        "no good initial image pair",
        "could not find a good initial image pair",
        "failed to find an initial image pair",
        "no images registered",
        "registered 0 images",
        "failed to create any sparse model",
        "did not produce a usable sparse model",
        "未生成完整的稀疏模型",
        "稀疏重建没有可用的注册图像或三维点",
        "稀疏重建输出不完整",
    ]
    .iter()
    .any(|s| lower.contains(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failure(
        engine: PipelineEngine,
        code: i32,
        stdout: &str,
        stderr: &str,
    ) -> EngineProcessFailure {
        EngineProcessFailure::from_output(
            engine,
            "mapper",
            &ProcessOutput {
                success: false,
                exit_code: Some(code),
                termination: ProcessTermination {
                    exit_code: Some(code),
                    ..Default::default()
                },
                stdout: stdout.into(),
                stderr: stderr.into(),
            },
        )
    }

    #[test]
    fn issue_71_early_gpu_oom_wins_over_later_io_in_either_stream() {
        // Public redacted issue excerpts, not the unavailable full report.
        let tail = format!("{}\nFailed to read counts: An error happened during execution\nAn IO error happened\nMemory location was never initialized", "kernel skipped\n".repeat(600));
        for (stdout, stderr) in [
            ("wgpu error: Out of Memory".into(), tail.clone()),
            (String::new(), format!("wgpu error: Out of Memory\n{tail}")),
        ] {
            let result = failure(PipelineEngine::Brush, 101, &stdout, &stderr);
            assert_eq!(result.classification.kind, Some("brush_out_of_memory"));
            assert!(!result.classification.heuristic);
            assert!(result.detail.contains("wgpu error: Out of Memory"));
            assert!(result.detail.chars().count() < 7_000);
            let error = SplatError::EngineProcess(Box::new(result));
            assert!(error.is_brush_oom());
            assert_eq!(
                classify(Some(PipelineStage::TrainingSplats), &error).kind,
                Some("brush_out_of_memory")
            );
        }
    }

    #[test]
    fn final_export_failure_survives_success_exit_and_truncated_tail() {
        for os_error in [5, 112] {
            let message = format!(
                "[2026-11-14T06:47:13Z WARN  brush_cli] Export at iteration 8000 failed: Export failed after 1.296s: iteration 8000, final.ply.tmp: Failed to export ply \"[REDACTED]\": native storage error (os error {os_error})"
            );
            let mut evidence = EngineFailureEvidence::brush_training(8_000);
            evidence.observe("Training progress: iteration=7995 total=8000");
            evidence.observe(&message);
            evidence.observe("Done training! Took FormattedDuration(4220s).");
            let output = ProcessOutput {
                success: true,
                exit_code: Some(0),
                termination: ProcessTermination {
                    exit_code: Some(0),
                    windows_status: Some("0x00000000".into()),
                    ..Default::default()
                },
                stdout: "Done training!".into(),
                stderr: "tail no longer contains export event".into(),
            };
            let failure = EngineProcessFailure::from_output_with_evidence(
                PipelineEngine::Brush,
                "train",
                &output,
                &evidence,
            );
            assert_eq!(failure.termination.exit_code, Some(0));
            assert_eq!(
                failure.brush_failure_reason,
                Some(BrushFailureReason::FinalExport {
                    iteration: 8_000,
                    reported_os_error: Some(os_error),
                })
            );
            assert!(failure.detail.contains(&format!("os error {os_error}")));
            assert!(!crate::pipeline::reshoot::explicit_numeric_divergence(
                &SplatError::EngineProcess(Box::new(failure))
            ));
            let fresh = EngineProcessFailure::from_output_with_evidence(
                PipelineEngine::Brush,
                "train",
                &output,
                &EngineFailureEvidence::brush_training(8_000),
            );
            assert_eq!(fresh.brush_failure_reason, None);
        }
    }

    #[test]
    fn export_evidence_is_bound_to_final_iteration_and_exact_event() {
        let mut evidence = EngineFailureEvidence::brush_training(8_000);
        for line in [
            "Export at iteration 4000 failed: optional earlier output",
            "warning: Export at iteration 8000 failed is a documented message",
            "Export at iteration 8000 failedness is not an export event",
            "Export at iteration 8000 completed",
            "Export at iteration 8000 failed: Export failed after 1s: iteration 8000, evaluation.ply: optional output failed",
            "Optional evaluation failed",
            "Export at iteration -1 failed: invalid iteration",
            "Export at iteration 8000x failed: invalid iteration",
        ] {
            evidence.observe(line);
            assert!(!evidence.brush_final_export_failed(), "{line}");
        }
        evidence.observe("Export at iteration 8000 failed: native write error");
        assert!(evidence.brush_final_export_failed());
    }

    #[test]
    fn explicit_numeric_failure_is_collected_before_tail_truncation() {
        let output = ProcessOutput {
            success: false,
            exit_code: Some(1),
            termination: ProcessTermination {
                exit_code: Some(1),
                ..Default::default()
            },
            stdout: String::new(),
            stderr: "late output only".into(),
        };
        for blocker in [
            None,
            Some("permission denied"),
            Some("Export at iteration 8000 failed: (os error 112)"),
        ] {
            let mut evidence = EngineFailureEvidence::brush_training(8_000);
            evidence.observe("Training loss became NaN");
            if let Some(blocker) = blocker {
                evidence.observe(blocker);
            }
            let failure = EngineProcessFailure::from_output_with_evidence(
                PipelineEngine::Brush,
                "train",
                &output,
                &evidence,
            );
            assert_eq!(
                crate::pipeline::reshoot::explicit_numeric_divergence(&SplatError::EngineProcess(
                    Box::new(failure)
                )),
                blocker.is_none(),
            );
        }
        let fresh = EngineProcessFailure::from_output_with_evidence(
            PipelineEngine::Brush,
            "train",
            &output,
            &EngineFailureEvidence::brush_training(8_000),
        );
        assert_eq!(fresh.brush_failure_reason, None);
    }

    #[test]
    fn mapper_geometry_decision_uses_only_current_explicit_final_failure() {
        let marker = "Failed to create any sparse model";
        assert!(failure(PipelineEngine::Colmap, 1, "", marker).is_mapper_geometry_failure());
        for (code, text) in [
            (1, "No good initial image pair found"),
            (1, "Discarding reconstruction due to bad initial pair"),
            (2, marker),
            (101, marker),
            (
                1,
                "SQLite error: database is locked\nFailed to create any sparse model",
            ),
            (1, "Permission denied\nFailed to create any sparse model"),
            (
                1,
                "No space left on device\nFailed to create any sparse model",
            ),
            (1, "Check failed: camera\nFailed to create any sparse model"),
            (
                1,
                "VK_ERROR_OUT_OF_DEVICE_MEMORY\nFailed to create any sparse model",
            ),
            (1, "std::bad_alloc\nFailed to create any sparse model"),
            (1, "Failed to read model\nFailed to create any sparse model"),
            (
                1,
                "database disk image is malformed\nFailed to create any sparse model",
            ),
        ] {
            assert!(
                !failure(PipelineEngine::Colmap, code, "", text).is_mapper_geometry_failure(),
                "{code}: {text}"
            );
        }
        let output = ProcessOutput {
            success: false,
            exit_code: Some(1),
            termination: ProcessTermination {
                exit_code: Some(1),
                ..Default::default()
            },
            stdout: marker.into(),
            stderr: String::new(),
        };
        for operation in [
            "feature_extractor",
            "sequential_matcher",
            "pose_prior_mapper",
        ] {
            assert!(
                !EngineProcessFailure::from_output(PipelineEngine::Colmap, operation, &output)
                    .is_mapper_geometry_failure()
            );
        }
        let mut signalled = output.clone();
        signalled.termination.signal = Some(11);
        assert!(
            !EngineProcessFailure::from_output(PipelineEngine::Colmap, "mapper", &signalled)
                .is_mapper_geometry_failure()
        );
        let mut succeeded = output;
        succeeded.success = true;
        assert!(
            !EngineProcessFailure::from_output(PipelineEngine::Colmap, "mapper", &succeeded)
                .is_mapper_geometry_failure()
        );
    }

    #[test]
    fn unknown_glog_errors_and_fatals_block_geometry_fallback() {
        let terminal =
            "E20261113 18:49:36.509829 30368 sfm.cc:310] Failed to create any sparse model";
        for line in [
            "E20261113 18:49:36.475251 30368 mapper.cc:510] Unexpected backend status 42",
            "F20261113 18:49:36.475251 30368 mapper.cc:510] Unknown native problem",
            "E1113 18:49:36.475251     30368 mapper.cc:510] Unexpected backend status 42",
            "F20261113 18:49:36.475251 30368 sfm.cc:310] Failed to create any sparse model",
            "E20261113 18:49:36.475251 30368 mapper.cc:510] Unknown failure; Failed to create any sparse model",
            "Error: unknown issue; Failed to create any sparse model",
        ] {
            assert!(!failure(PipelineEngine::Colmap, 1, line, terminal).is_mapper_geometry_failure(), "{line}");
        }
        for line in [
            "I20261113 18:49:36.475251 30368 incremental_pipeline.cc:791] Discarding reconstruction due to bad initial pair",
            "W20261113 18:49:36.475251 30368 incremental_pipeline.cc:472] No good initial image pair found.",
            "Finding good initial image pair",
            "Extracting features for image 10",
            "Feature extraction completed",
        ] {
            assert!(failure(PipelineEngine::Colmap, 1, line, terminal).is_mapper_geometry_failure(), "{line}");
        }
        assert!(failure(PipelineEngine::Colmap, 1, "", terminal).is_mapper_geometry_failure());
    }

    #[test]
    fn glog_header_requires_native_date_clock_thread_and_location_format() {
        for line in [
            "Error: arbitrary English text",
            "Features: 1234",
            "E12345 18:49:36.123456 1234 sfm.cc:310] not native",
            "E20261113 28:49:36.123456 1234 sfm.cc:310] invalid time",
            "E20261113 18:49:36 1234 sfm.cc:310] missing fraction",
            "E20261113 18:49:36.123456 worker sfm.cc:310] invalid thread",
            "E20261113 18:49:36.123456 1234 sfm.cc:abc] invalid line",
        ] {
            assert!(glog_severity_and_message(line).is_none(), "{line}");
        }
        assert_eq!(
            glog_severity_and_message("E1113 18:49:36.123456 1234 sfm.cc:310] reason"),
            Some((b'E', "reason"))
        );
    }

    #[test]
    fn streamed_evidence_survives_tail_limits_and_does_not_borrow_prior_attempts() {
        let mut evidence = EngineFailureEvidence::default();
        evidence.observe("SQLite error: database is locked");
        evidence.observe("Failed to create any sparse model");
        let output = ProcessOutput {
            success: false,
            exit_code: Some(1),
            termination: ProcessTermination {
                exit_code: Some(1),
                ..Default::default()
            },
            stdout: String::new(),
            stderr: "tail after output capture limit".into(),
        };
        let failure = EngineProcessFailure::from_output_with_evidence(
            PipelineEngine::Colmap,
            "mapper",
            &output,
            &evidence,
        );
        assert!(!failure.is_mapper_geometry_failure());
        assert_eq!(failure.classification.kind, Some("mapper_storage"));
        assert!(failure.detail.contains("database is locked"));

        let mut current =
            EngineProcessFailure::from_output(PipelineEngine::Colmap, "mapper", &output);
        current.previous_attempts.push(failure);
        assert!(!current.is_mapper_geometry_failure());
        assert_eq!(current.classification.kind, None);

        let mut clean = EngineFailureEvidence::default();
        clean.observe("Failed to create any sparse model");
        assert!(EngineProcessFailure::from_output_with_evidence(
            PipelineEngine::Colmap,
            "mapper",
            &output,
            &clean,
        )
        .is_mapper_geometry_failure());
    }

    #[test]
    fn reported_gpu_initialization_failures_are_not_oom_or_dataset_errors() {
        // Redacted report evidence: these reports did not identify the actual
        // device, and this diagnostic must not guess it from installed GPUs.
        let text = "GPU initialization started\nwgpu-hal invariant was violated (usage error): Requested feature is not available on this device";
        let unsupported = failure(PipelineEngine::Brush, 101, text, "An IO error happened");
        assert_eq!(unsupported.classification.kind, Some("brush_gpu"));
        assert!(!unsupported.classification.heuristic);
        assert!(unsupported
            .detail
            .contains("failure_reason=gpu_feature_unsupported"));
        assert!(unsupported
            .detail
            .contains("execution_phase=gpu_initialization"));
        assert!(!unsupported.detail.contains("actualDevice"));
        assert!(!SplatError::EngineProcess(Box::new(unsupported)).is_brush_oom());

        let mut output = ProcessOutput {
            success: false,
            exit_code: Some(-1073741819),
            termination: ProcessTermination {
                exit_code: Some(-1073741819),
                windows_status: Some("0xC0000005".into()),
                ..Default::default()
            },
            stdout: "GPU initialization started".into(),
            stderr: String::new(),
        };
        let crashed = EngineProcessFailure::from_output(PipelineEngine::Brush, "train", &output);
        assert_eq!(crashed.classification.kind, Some("brush_gpu"));
        assert!(crashed
            .detail
            .contains("failure_reason=gpu_initialization_terminated"));
        assert!(crashed.to_string().contains("0xC0000005"));
        assert!(!SplatError::EngineProcess(Box::new(crashed)).is_brush_oom());

        output.stdout.push_str("\nGPU initialization completed");
        let later_crash =
            EngineProcessFailure::from_output(PipelineEngine::Brush, "train", &output);
        assert_eq!(later_crash.classification.kind, None);
        assert!(!later_crash
            .detail
            .contains("execution_phase=gpu_initialization"));
    }

    #[test]
    fn gpu_observer_evidence_preserves_early_oom_over_truncated_io_tail() {
        let mut evidence = EngineFailureEvidence::default();
        evidence.observe("GPU initialization started");
        evidence.observe("wgpu error: Out of Memory");
        let output = ProcessOutput {
            success: false,
            exit_code: Some(101),
            termination: ProcessTermination {
                exit_code: Some(101),
                ..Default::default()
            },
            stdout: String::new(),
            stderr: "failed to load dataset: early eof".into(),
        };
        let error = EngineProcessFailure::from_output_with_evidence(
            PipelineEngine::Brush,
            "train",
            &output,
            &evidence,
        );
        assert_eq!(error.classification.kind, Some("brush_out_of_memory"));
        assert!(error.detail.contains("wgpu error: Out of Memory"));
    }

    #[test]
    fn gpu_limit_device_loss_host_allocation_and_exit_codes_are_not_gpu_oom() {
        for (text, kind) in [
            ("BufferTooBig(2290420416)", Some("brush_resource_limit")),
            (
                "VK_ERROR_DEVICE_LOST\nAn IO error happened",
                Some("brush_device_lost"),
            ),
            ("GPU allocation failed", None),
            ("wgpu initialized\nCPU out of memory", None),
            ("VK_ERROR_OUT_OF_HOST_MEMORY", None),
            (
                "thread panicked\nFailed to read counts\nAn IO error happened",
                None,
            ),
            ("process exited with code 101", None),
            (
                "IO error while loading dataset: early eof",
                Some("brush_dataset"),
            ),
            (
                "No such file or directory: images/a.png",
                Some("brush_dataset"),
            ),
        ] {
            let result = failure(PipelineEngine::Brush, 101, "", text);
            assert_eq!(result.classification.kind, kind, "{text}");
            assert!(!SplatError::EngineProcess(Box::new(result)).is_brush_oom());
        }
    }

    #[test]
    fn issue_72_native_termination_wins_over_rejected_registration_logs() {
        let mut result = failure(
            PipelineEngine::Colmap,
            -1073740940,
            "435 (connected 435, loaded 435)\nRegistering initial image pair #206 and #285",
            "Could not register image\nDiscarding reconstruction because it is too small",
        );
        result.termination.windows_status = Some("0xC0000374".into());
        let output = ProcessOutput {
            success: false,
            exit_code: Some(-1073740940),
            termination: result.termination.clone(),
            stdout: result.detail,
            stderr: "Failed to create any sparse model".into(),
        };
        let result = EngineProcessFailure::from_output(PipelineEngine::Colmap, "mapper", &output);
        assert_eq!(result.classification.kind, Some("colmap_crash"));
        assert!(!result.classification.heuristic);
        let message = result.to_string();
        assert!(message.contains("-1073740940"));
        assert!(message.contains("0xC0000374"));
    }

    #[test]
    fn last_attempt_owns_classification_not_previous_crash_or_material_logs() {
        let final_attempt = failure(
            PipelineEngine::Colmap,
            1,
            "",
            "SQLite error: database is locked",
        );
        let mut previous = failure(
            PipelineEngine::Colmap,
            -1073740940,
            "",
            "No good initial image pair",
        );
        previous.termination.windows_status = Some("0xC0000374".into());
        let error = SplatError::EngineProcess(Box::new(final_attempt))
            .with_previous_attempt(SplatError::EngineProcess(Box::new(previous.clone())));
        assert_eq!(
            classify(Some(PipelineStage::Reconstructing), &error).kind,
            Some("mapper_storage")
        );
        assert!(error.to_string().contains("attempt 2"));
        assert!(error
            .to_string()
            .contains("previous attempt 1 (context only)"));
        assert!(matches!(
            SplatError::Cancelled
                .with_previous_attempt(SplatError::EngineProcess(Box::new(previous))),
            SplatError::Cancelled
        ));
    }

    #[test]
    fn unknown_exit_is_not_a_material_or_gpu_diagnosis_and_cancel_is_not_classified() {
        for engine in [PipelineEngine::Colmap, PipelineEngine::Brush] {
            assert_eq!(
                failure(engine, 1, "", "Could not register image #235")
                    .classification
                    .kind,
                None
            );
            assert_eq!(
                failure(engine, -1, "", "unknown exception")
                    .classification
                    .kind,
                None
            );
        }
        assert_eq!(
            classify(Some(PipelineStage::Reconstructing), &SplatError::Cancelled).kind,
            None
        );
        assert_eq!(
            classify(
                Some(PipelineStage::Reconstructing),
                &SplatError::Process(
                    "SQLite database is locked\nFailed to create any sparse model".into()
                )
            )
            .kind,
            Some("mapper_storage")
        );
    }
}
