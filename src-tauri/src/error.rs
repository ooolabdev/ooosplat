use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum SplatError {
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
    #[error("当前引擎版本不支持安全接入：{0}")]
    UnsupportedEngine(String),
    #[error("文件读写失败：{0}")]
    Io(#[from] std::io::Error),
    #[error("无法解析引擎输出：{0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, SplatError>;

impl serde::Serialize for SplatError {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}
