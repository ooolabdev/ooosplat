use std::path::PathBuf;

pub(crate) const RETRY_PREPARATION_MARKER: &str = "retry_preparation_operation=";

/// Typed rejection of a new input. Existing execution failures retain their
/// historical string IPC representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, thiserror::Error)]
#[serde(rename_all = "snake_case")]
pub enum InputAdmissionReason {
    #[error("暂不支持导入 COLMAP 文件或文件夹。")]
    ColmapDisabled,
    #[error("不支持所选文件或文件夹，请选择当前入口支持的文件类型。")]
    UnsupportedInput,
}

#[derive(Debug, thiserror::Error)]
pub enum SplatError {
    #[error("{0}")]
    InputAdmission(InputAdmissionReason),
    #[error("{source}\n{detail}")]
    Diagnostic {
        source: Box<SplatError>,
        detail: String,
    },
    #[error("找不到本地处理引擎：{0}")]
    EngineMissing(String),
    #[error("本地处理引擎无法启动：{engine}（{detail}）")]
    EngineStart { engine: String, detail: String },
    #[error("视频无效：{0}")]
    InvalidVideo(String),
    #[error("不支持的文件路径：{0}")]
    InvalidPath(PathBuf),
    #[error("任务已取消")]
    Cancelled,
    #[error("外部进程执行失败：{0}")]
    Process(String),
    #[error("{0}")]
    EngineProcess(Box<crate::failure::EngineProcessFailure>),
    #[error("Brush 显存不足：{0}")]
    BrushOutOfMemory(String),
    #[error("Splat 训练期间显卡设备连接中断。请接通电源、关闭占用显卡的软件，并在 Windows 图形设置中将 brush_app.exe 设为高性能后重试。\n{0}")]
    BrushDeviceLost(String),
    #[error("当前引擎版本不支持安全接入：{0}")]
    UnsupportedEngine(String),
    #[error("文件读写失败：{0}")]
    Io(#[from] std::io::Error),
    #[error("无法解析引擎输出：{0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, SplatError>;

impl SplatError {
    /// Returns the original I/O error through diagnostic wrappers. Callers use
    /// this to add semantic context without losing `raw_os_error()`.
    pub(crate) fn io_error(&self) -> Option<&std::io::Error> {
        match self {
            Self::Diagnostic { source, .. } => source.io_error(),
            Self::Io(error) => Some(error),
            _ => None,
        }
    }

    /// Adds detail only to I/O failures. Non-I/O failures retain their exact
    /// type and presentation.
    pub(crate) fn with_io_diagnostic(self, detail: impl Into<String>) -> Self {
        if self.io_error().is_some() {
            Self::Diagnostic {
                source: Box::new(self),
                detail: detail.into(),
            }
        } else {
            self
        }
    }

    pub fn with_previous_attempt(self, previous: SplatError) -> Self {
        match (self, previous) {
            (Self::EngineProcess(mut current), Self::EngineProcess(previous)) => {
                current.previous_attempts.push(*previous);
                Self::EngineProcess(current)
            }
            (current, Self::EngineProcess(previous)) if current.io_error().is_some() => {
                // Keep the real I/O error as the source. Earlier engine output
                // is context only and must not reclassify a disk/permission
                // failure as a GPU or geometric failure.
                let summary = crate::diagnostics::redact::sanitize(
                    &previous.to_string().chars().take(2_048).collect::<String>(),
                    &[],
                );
                current.with_io_diagnostic(format!("previous_attempt_context_only=true\n{summary}"))
            }
            (current, _) => current,
        }
    }

    pub fn is_brush_oom(&self) -> bool {
        if let Self::Diagnostic { source, .. } = self {
            return source.is_brush_oom();
        }
        matches!(self, Self::BrushOutOfMemory(_))
            || matches!(self, Self::EngineProcess(failure)
                if failure.classification.kind == Some("brush_out_of_memory"))
    }
}

impl serde::Serialize for SplatError {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        if let Self::InputAdmission(reason) = self {
            use serde::ser::SerializeStruct;
            let mut value = serializer.serialize_struct("InputAdmissionError", 3)?;
            value.serialize_field("code", "UNSUPPORTED_INPUT")?;
            value.serialize_field("reason", reason)?;
            value.serialize_field("message", &reason.to_string())?;
            value.end()
        } else {
            serializer.serialize_str(&self.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_admission_has_typed_ipc_without_changing_other_errors() {
        let error = SplatError::InputAdmission(InputAdmissionReason::ColmapDisabled);
        assert_eq!(
            serde_json::to_value(error).unwrap(),
            serde_json::json!({"code":"UNSUPPORTED_INPUT","reason":"colmap_disabled",
                "message":"暂不支持导入 COLMAP 文件或文件夹。"})
        );
        let existing = SplatError::Process("unchanged".into());
        assert_eq!(
            serde_json::to_value(&existing).unwrap(),
            serde_json::json!(existing.to_string())
        );
        let ordinary = SplatError::InputAdmission(InputAdmissionReason::UnsupportedInput);
        assert_eq!(
            serde_json::to_value(ordinary).unwrap()["reason"],
            "unsupported_input"
        );
    }

    #[test]
    fn io_diagnostic_retains_original_os_error() {
        let error = SplatError::Io(std::io::Error::from_raw_os_error(5))
            .with_io_diagnostic("retry_preparation_operation=write_retry_reuse_barrier");
        assert_eq!(
            error.io_error().and_then(std::io::Error::raw_os_error),
            Some(5)
        );
        assert!(error
            .to_string()
            .contains("retry_preparation_operation=write_retry_reuse_barrier"));
    }

    #[test]
    fn io_diagnostic_does_not_reclassify_non_io_errors() {
        let error = SplatError::Process("original".into()).with_io_diagnostic("ignored");
        assert!(matches!(error, SplatError::Process(message) if message == "original"));
    }

    #[test]
    fn previous_engine_failure_does_not_replace_last_io_reason() {
        use crate::pipeline::{PipelineEngine, PipelineStage};
        use crate::process::{ProcessOutput, ProcessTermination};
        let previous = crate::failure::EngineProcessFailure::from_output(
            PipelineEngine::Colmap,
            "mapper",
            &ProcessOutput {
                success: false,
                exit_code: Some(1),
                termination: ProcessTermination {
                    exit_code: Some(1),
                    ..Default::default()
                },
                stdout: String::new(),
                stderr: "Failed to create any sparse model".into(),
            },
        );
        let error = SplatError::Io(std::io::Error::from_raw_os_error(5))
            .with_io_diagnostic("training_preparation_operation=write_state")
            .with_previous_attempt(SplatError::EngineProcess(Box::new(previous)));
        assert_eq!(
            error.io_error().and_then(std::io::Error::raw_os_error),
            Some(5)
        );
        assert!(error
            .to_string()
            .contains("previous_attempt_context_only=true"));
        assert!(error
            .to_string()
            .contains("training_preparation_operation=write_state"));
        assert_eq!(
            crate::failure::classify(Some(PipelineStage::Reconstructing), &error).kind,
            Some("mapper_storage")
        );
        assert!(!error.is_brush_oom());
    }
}
