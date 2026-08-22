use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{
    error::{Result, SplatError},
    process::{ProcessManager, ProcessSpec},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EngineKind {
    Ffmpeg,
    Ffprobe,
    Colmap,
    Brush,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineStatus {
    pub kind: EngineKind,
    pub path: PathBuf,
    pub exists: bool,
    pub can_start: bool,
    pub version: Option<String>,
    pub cpu_only: Option<bool>,
    pub gpu_available: Option<bool>,
    pub detail: String,
}

#[derive(Debug, Clone)]
pub struct EnginePaths {
    pub root: PathBuf,
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
    pub colmap: PathBuf,
    pub brush: PathBuf,
}

impl EnginePaths {
    pub fn from_root(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            ffmpeg: root.join("ffmpeg").join("ffmpeg.exe"),
            ffprobe: root.join("ffmpeg").join("ffprobe.exe"),
            colmap: root.join("colmap").join("bin").join("colmap.exe"),
            brush: root.join("brush").join("brush_app.exe"),
            root,
        }
    }

    pub fn discover(resource_dir: Option<&Path>) -> Self {
        if let Some(value) = std::env::var_os("OOOSPLAT_ENGINE_DIR") {
            return Self::from_root(value);
        }

        let current = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let candidates = [
            resource_dir.map(|path| path.join("engines")),
            Some(current.join("engines")),
            Some(current.join("..").join("engines")),
        ];
        let root = candidates
            .into_iter()
            .flatten()
            .find(|path| path.is_dir())
            .unwrap_or_else(|| current.join("engines"));
        Self::from_root(root)
    }

    pub async fn check_all(&self) -> Vec<EngineStatus> {
        let (ffmpeg, ffprobe, colmap, brush) = tokio::join!(
            check_basic(EngineKind::Ffmpeg, &self.ffmpeg, &["-version"]),
            check_basic(EngineKind::Ffprobe, &self.ffprobe, &["-version"]),
            check_colmap(&self.colmap),
            check_basic(EngineKind::Brush, &self.brush, &["--help"]),
        );
        vec![ffmpeg, ffprobe, colmap, brush]
    }
}

fn missing(kind: EngineKind, path: &Path) -> EngineStatus {
    EngineStatus {
        kind,
        path: path.to_path_buf(),
        exists: false,
        can_start: false,
        version: None,
        cpu_only: None,
        gpu_available: None,
        detail: format!("未找到 {}", path.display()),
    }
}

async fn check_basic(kind: EngineKind, path: &Path, args: &[&str]) -> EngineStatus {
    if !path.is_file() {
        return missing(kind, path);
    }
    let manager = ProcessManager::new();
    let result = manager
        .run(ProcessSpec {
            executable: path.to_path_buf(),
            args: args.iter().map(OsString::from).collect(),
            working_directory: path.parent().map(Path::to_path_buf),
            log_path: None,
            observer: None,
        })
        .await;

    match result {
        Ok(output) => {
            let combined = format!("{}\n{}", output.stdout, output.stderr);
            let first_line = combined
                .lines()
                .find(|line| !line.trim().is_empty())
                .map(|line| line.trim().to_owned());
            EngineStatus {
                kind,
                path: path.to_path_buf(),
                exists: true,
                can_start: output.success,
                version: first_line,
                cpu_only: None,
                gpu_available: None,
                detail: if output.success {
                    "引擎可启动".into()
                } else {
                    format!("帮助命令退出码：{:?}", output.exit_code)
                },
            }
        }
        Err(error) => EngineStatus {
            kind,
            path: path.to_path_buf(),
            exists: true,
            can_start: false,
            version: None,
            cpu_only: None,
            gpu_available: None,
            detail: error.to_string(),
        },
    }
}

async fn check_colmap(path: &Path) -> EngineStatus {
    if !path.is_file() {
        return missing(EngineKind::Colmap, path);
    }
    let manager = ProcessManager::new();
    let mut help = String::new();
    let mut successful = true;
    for args in [
        vec!["feature_extractor", "-h"],
        vec!["sequential_matcher", "-h"],
        vec!["mapper", "-h"],
    ] {
        match manager
            .run(ProcessSpec {
                executable: path.to_path_buf(),
                args: args.into_iter().map(OsString::from).collect(),
                working_directory: path.parent().map(Path::to_path_buf),
                log_path: None,
                observer: None,
            })
            .await
        {
            Ok(output) => {
                successful &= output.success;
                help.push_str(&output.stdout);
                help.push_str(&output.stderr);
            }
            Err(error) => {
                return EngineStatus {
                    kind: EngineKind::Colmap,
                    path: path.to_path_buf(),
                    exists: true,
                    can_start: false,
                    version: None,
                    cpu_only: None,
                    gpu_available: None,
                    detail: error.to_string(),
                }
            }
        }
    }

    let lower = help.to_ascii_lowercase();
    let explicit_cpu = [
        "cuda: no",
        "cuda support: no",
        "without cuda",
        "no cuda support",
    ]
    .iter()
    .any(|marker| lower.contains(marker));
    let bundled_cuda = path.parent().is_some_and(runtime_contains_cuda);
    let cpu_only = if bundled_cuda {
        Some(false)
    } else if explicit_cpu {
        Some(true)
    } else {
        None
    };
    let first_line = help
        .lines()
        .find(|line| !line.trim().is_empty())
        .map(|line| line.trim().to_owned());
    let gpu_available = probe_gpu().await;
    let detail = match cpu_only {
        Some(true) => "三个必需命令可启动，帮助输出明确报告无 CUDA".into(),
        Some(false) => "运行目录中发现 CUDA 运行时，支持 GPU 加速".into(),
        None => "命令可启动，但帮助输出未明确证明是否包含 CUDA".into(),
    };
    EngineStatus {
        kind: EngineKind::Colmap,
        path: path.to_path_buf(),
        exists: true,
        can_start: successful,
        version: first_line,
        cpu_only,
        gpu_available,
        detail,
    }
}

fn runtime_contains_cuda(directory: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let path = entry.path();
        if path.is_dir() {
            return runtime_contains_cuda(&path);
        }
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        ["cudart", "cublas", "cudnn", "cuda.dll"]
            .iter()
            .any(|needle| name.contains(needle))
    })
}

pub async fn require_colmap(paths: &EnginePaths, use_gpu: bool) -> Result<()> {
    let status = check_colmap(&paths.colmap).await;
    if !status.can_start {
        return Err(SplatError::UnsupportedEngine(status.detail));
    }
    if use_gpu {
        if status.cpu_only == Some(true) {
            return Err(SplatError::UnsupportedEngine(
                "当前 COLMAP 为 CPU 构建，GPU 模式不可用".into(),
            ));
        }
        if status.gpu_available == Some(false) {
            return Err(SplatError::UnsupportedEngine(
                "未检测到 NVIDIA 显卡驱动，GPU 模式不可用".into(),
            ));
        }
    }
    Ok(())
}

/// 探测 NVIDIA 显卡驱动是否可用（决定 GPU 模式是否可行）。
/// 依次检查 System32 与 PATH 下的 nvidia-smi.exe；运行 `nvidia-smi -L`
/// 成功且输出包含 "GPU" 视为可用。未找到或运行失败视为不可用。
async fn probe_gpu() -> Option<bool> {
    for candidate in nvidia_smi_candidates() {
        if !candidate.is_file() {
            continue;
        }
        let manager = ProcessManager::new();
        let result = manager
            .run(ProcessSpec {
                executable: candidate,
                args: vec![OsString::from("-L")],
                working_directory: None,
                log_path: None,
                observer: None,
            })
            .await;
        return match result {
            Ok(output) if output.success => {
                let combined = format!("{}\n{}", output.stdout, output.stderr);
                Some(combined.to_ascii_lowercase().contains("gpu"))
            }
            _ => Some(false),
        };
    }
    Some(false)
}

fn nvidia_smi_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    match std::env::var_os("SystemRoot") {
        Some(root) => candidates.push(PathBuf::from(root).join("System32").join("nvidia-smi.exe")),
        None => candidates.push(PathBuf::from(r"C:\Windows\System32\nvidia-smi.exe")),
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            candidates.push(dir.join("nvidia-smi.exe"));
        }
    }
    candidates
}
