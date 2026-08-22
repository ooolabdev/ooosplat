use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    time::Duration,
};

use serde::{Deserialize, Serialize};

use crate::process::{ProcessManager, ProcessSpec};

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
    pub acceleration: Option<ColmapAccelerationStatus>,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColmapBackend {
    Cpu,
    Gpu,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AccelerationReasonCode {
    GpuReady,
    ColmapUnavailable,
    ColmapCudaUnavailable,
    RequirementsUnavailable,
    NvidiaSmiNotFound,
    ProbeFailed,
    ProbeTimeout,
    NoNvidiaGpu,
    DriverVersionUnknown,
    DriverTooOld,
    ComputeCapabilityUnknown,
    ComputeCapabilityTooLow,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuDeviceInfo {
    pub index: u32,
    pub name: String,
    pub driver_version: String,
    pub compute_capability: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccelerationRequirements {
    pub minimum_driver_version: String,
    pub minimum_compute_capability: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ColmapAccelerationStatus {
    pub backend: ColmapBackend,
    pub reason_code: AccelerationReasonCode,
    pub reason: String,
    pub device: Option<GpuDeviceInfo>,
    pub requirements: AccelerationRequirements,
}

impl ColmapAccelerationStatus {
    pub const fn use_gpu(&self) -> bool {
        matches!(self.backend, ColmapBackend::Gpu)
    }

    pub fn gpu_index(&self) -> Option<u32> {
        self.use_gpu()
            .then(|| self.device.as_ref().map(|device| device.index))
            .flatten()
    }
}

const DEFAULT_MINIMUM_DRIVER: &str = "528.33";
const DEFAULT_MINIMUM_COMPUTE_CAPABILITY: &str = "5.0";
const NVIDIA_SMI_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct EnginePaths {
    pub root: PathBuf,
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
    pub colmap: PathBuf,
    pub brush: PathBuf,
}

fn find_in_path(cmd: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        #[cfg(windows)]
        {
            let candidate_exe = dir.join(format!("{cmd}.exe"));
            if candidate_exe.is_file() {
                return Some(candidate_exe);
            }
        }
        let candidate = dir.join(cmd);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn resolve_engine_path(
    root: &Path,
    subdir: &str,
    candidates: &[&str],
    system_cmd: &str,
) -> PathBuf {
    let engine_dir = root.join(subdir);
    for rel in candidates {
        let path = engine_dir.join(rel);
        if path.is_file() {
            return path;
        }
    }
    if let Some(sys_path) = find_in_path(system_cmd) {
        return sys_path;
    }
    engine_dir.join(candidates[0])
}

impl EnginePaths {
    pub fn from_root(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        #[cfg(windows)]
        let (ffmpeg_candidates, ffprobe_candidates, colmap_candidates, brush_candidates) = (
            &["ffmpeg.exe", "bin/ffmpeg.exe"][..],
            &["ffprobe.exe", "bin/ffprobe.exe"][..],
            &["bin/colmap.exe", "colmap.exe"][..],
            &["brush_app.exe", "brush.exe"][..],
        );
        #[cfg(not(windows))]
        let (ffmpeg_candidates, ffprobe_candidates, colmap_candidates, brush_candidates) = (
            &["ffmpeg", "bin/ffmpeg"][..],
            &["ffprobe", "bin/ffprobe"][..],
            &[
                "bin/colmap",
                "colmap",
                "COLMAP.app/Contents/MacOS/colmap",
            ][..],
            &[
                "brush_app",
                "brush",
                "brush-app-aarch64-apple-darwin/brush_app",
                "brush-app-x86_64-apple-darwin/brush_app",
            ][..],
        );

        Self {
            ffmpeg: resolve_engine_path(&root, "ffmpeg", ffmpeg_candidates, "ffmpeg"),
            ffprobe: resolve_engine_path(&root, "ffmpeg", ffprobe_candidates, "ffprobe"),
            colmap: resolve_engine_path(&root, "colmap", colmap_candidates, "colmap"),
            brush: resolve_engine_path(&root, "brush", brush_candidates, "brush_app"),
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
            check_colmap(&self.colmap, &self.root),
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
        acceleration: None,
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
                acceleration: None,
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
            acceleration: None,
            detail: error.to_string(),
        },
    }
}

async fn check_colmap(path: &Path, engines_root: &Path) -> EngineStatus {
    if !path.is_file() {
        let mut status = missing(EngineKind::Colmap, path);
        status.acceleration = Some(cpu_status(
            AccelerationReasonCode::ColmapUnavailable,
            format!("未找到内置 COLMAP：{}", path.display()),
            None,
            requirements_or_default(engines_root),
        ));
        return status;
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
                    acceleration: Some(cpu_status(
                        AccelerationReasonCode::ColmapUnavailable,
                        format!("COLMAP 无法启动：{error}"),
                        None,
                        requirements_or_default(engines_root),
                    )),
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
    let acceleration = if !successful {
        cpu_status(
            AccelerationReasonCode::ColmapUnavailable,
            "COLMAP 必需命令无法正常启动，不能启用 GPU 加速".into(),
            None,
            requirements_or_default(engines_root),
        )
    } else if cpu_only != Some(false) {
        #[cfg(target_os = "macos")]
        let cpu_reason = "macOS 系统使用 CPU 模式进行特征提取与匹配".to_string();
        #[cfg(not(target_os = "macos"))]
        let cpu_reason = "内置 COLMAP 未检测到完整 CUDA 运行时，已使用 CPU".to_string();

        cpu_status(
            AccelerationReasonCode::ColmapCudaUnavailable,
            cpu_reason,
            None,
            requirements_or_default(engines_root),
        )
    } else {
        detect_acceleration(engines_root).await
    };
    let detail = match cpu_only {
        Some(true) => "三个必需命令可启动，帮助输出明确报告无 CUDA".into(),
        Some(false) => format!("三个必需命令可启动；{}", acceleration.reason),
        None => "命令可启动，但帮助输出未明确证明是否包含 CUDA".into(),
    };
    EngineStatus {
        kind: EngineKind::Colmap,
        path: path.to_path_buf(),
        exists: true,
        can_start: successful,
        version: first_line,
        cpu_only,
        acceleration: Some(acceleration),
        detail,
    }
}

fn runtime_contains_cuda(directory: &Path) -> bool {
    let mut found = [false; 3];
    scan_cuda_runtime(directory, &mut found);
    found.into_iter().all(|present| present)
}

fn scan_cuda_runtime(directory: &Path, found: &mut [bool; 3]) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan_cuda_runtime(&path, found);
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        found[0] |= name.contains("cudart64_");
        found[1] |= name.contains("curand64_");
        found[2] |= name == "onnxruntime_providers_cuda.dll";
    }
}

pub async fn check_colmap_acceleration(paths: &EnginePaths) -> ColmapAccelerationStatus {
    check_colmap(&paths.colmap, &paths.root)
        .await
        .acceleration
        .unwrap_or_else(|| {
            cpu_status(
                AccelerationReasonCode::ColmapUnavailable,
                "无法读取 COLMAP 加速状态，已使用 CPU".into(),
                None,
                requirements_or_default(&paths.root),
            )
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct NumericVersion(u32, u32);

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EngineManifest {
    engines: Vec<ManifestEngine>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManifestEngine {
    name: String,
    cuda_compatibility: Option<ManifestCudaCompatibility>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManifestCudaCompatibility {
    minimum_windows_driver: String,
    minimum_compute_capability: String,
}

#[derive(Debug)]
enum ProbeError {
    NotFound,
    Failed,
    Timeout,
    NoGpu,
    InvalidOutput,
}

fn default_requirements() -> AccelerationRequirements {
    AccelerationRequirements {
        minimum_driver_version: DEFAULT_MINIMUM_DRIVER.into(),
        minimum_compute_capability: DEFAULT_MINIMUM_COMPUTE_CAPABILITY.into(),
    }
}

fn requirements_or_default(engines_root: &Path) -> AccelerationRequirements {
    load_requirements(engines_root).unwrap_or_else(|_| default_requirements())
}

fn load_requirements(engines_root: &Path) -> std::result::Result<AccelerationRequirements, String> {
    let path = engines_root.join("manifest.json");
    let bytes =
        std::fs::read(&path).map_err(|error| format!("无法读取 {}：{error}", path.display()))?;
    let manifest: EngineManifest = serde_json::from_slice(&bytes)
        .map_err(|error| format!("无法解析 {}：{error}", path.display()))?;
    let compatibility = manifest
        .engines
        .into_iter()
        .find(|engine| engine.name.eq_ignore_ascii_case("COLMAP"))
        .and_then(|engine| engine.cuda_compatibility)
        .ok_or_else(|| "引擎清单缺少 COLMAP cudaCompatibility".to_string())?;
    parse_version(&compatibility.minimum_windows_driver)
        .ok_or_else(|| "引擎清单中的最低驱动版本无效".to_string())?;
    parse_version(&compatibility.minimum_compute_capability)
        .ok_or_else(|| "引擎清单中的最低 Compute Capability 无效".to_string())?;
    Ok(AccelerationRequirements {
        minimum_driver_version: compatibility.minimum_windows_driver,
        minimum_compute_capability: compatibility.minimum_compute_capability,
    })
}

fn cpu_status(
    reason_code: AccelerationReasonCode,
    reason: String,
    device: Option<GpuDeviceInfo>,
    requirements: AccelerationRequirements,
) -> ColmapAccelerationStatus {
    ColmapAccelerationStatus {
        backend: ColmapBackend::Cpu,
        reason_code,
        reason,
        device,
        requirements,
    }
}

async fn detect_acceleration(engines_root: &Path) -> ColmapAccelerationStatus {
    let requirements = match load_requirements(engines_root) {
        Ok(requirements) => requirements,
        Err(error) => {
            return cpu_status(
                AccelerationReasonCode::RequirementsUnavailable,
                format!("{error}，已为兼容性使用 CPU"),
                None,
                default_requirements(),
            )
        }
    };
    let devices = match probe_gpu_devices().await {
        Ok(devices) => devices,
        Err(error) => return probe_error_status(error, requirements),
    };
    choose_acceleration(devices, requirements)
}

fn probe_error_status(
    error: ProbeError,
    requirements: AccelerationRequirements,
) -> ColmapAccelerationStatus {
    let (reason_code, reason) = match error {
        ProbeError::NotFound => (
            AccelerationReasonCode::NvidiaSmiNotFound,
            "未检测到 NVIDIA 驱动，已使用 CPU",
        ),
        ProbeError::Timeout => (
            AccelerationReasonCode::ProbeTimeout,
            "NVIDIA 显卡检测超时，已使用 CPU",
        ),
        ProbeError::NoGpu => (
            AccelerationReasonCode::NoNvidiaGpu,
            "未检测到 NVIDIA 显卡，已使用 CPU",
        ),
        ProbeError::InvalidOutput => (
            AccelerationReasonCode::ComputeCapabilityUnknown,
            "无法读取显卡驱动或 Compute Capability，已使用 CPU",
        ),
        ProbeError::Failed => (
            AccelerationReasonCode::ProbeFailed,
            "NVIDIA 显卡检测失败，已使用 CPU",
        ),
    };
    cpu_status(reason_code, reason.into(), None, requirements)
}

fn choose_acceleration(
    devices: Vec<GpuDeviceInfo>,
    requirements: AccelerationRequirements,
) -> ColmapAccelerationStatus {
    let minimum_driver = parse_version(&requirements.minimum_driver_version)
        .expect("validated acceleration requirement");
    let minimum_compute = parse_version(&requirements.minimum_compute_capability)
        .expect("validated acceleration requirement");

    let mut compatible = devices
        .iter()
        .filter_map(|device| {
            let driver = parse_version(&device.driver_version)?;
            let compute = parse_version(&device.compute_capability)?;
            (driver >= minimum_driver && compute >= minimum_compute).then_some((
                compute,
                device.index,
                device.clone(),
            ))
        })
        .collect::<Vec<_>>();
    compatible.sort_by(|left, right| right.0.cmp(&left.0).then(left.1.cmp(&right.1)));
    if let Some((_, _, device)) = compatible.into_iter().next() {
        return ColmapAccelerationStatus {
            backend: ColmapBackend::Gpu,
            reason_code: AccelerationReasonCode::GpuReady,
            reason: format!(
                "已启用 {}（驱动 {}，Compute Capability {}）",
                device.name, device.driver_version, device.compute_capability
            ),
            device: Some(device),
            requirements,
        };
    }

    let driver_eligible = devices.iter().filter(|device| {
        parse_version(&device.driver_version).is_some_and(|version| version >= minimum_driver)
    });
    let mut low_compute = driver_eligible
        .clone()
        .filter_map(|device| {
            parse_version(&device.compute_capability)
                .filter(|version| *version < minimum_compute)
                .map(|version| (version, device.clone()))
        })
        .collect::<Vec<_>>();
    low_compute.sort_by_key(|item| std::cmp::Reverse(item.0));
    if let Some((_, device)) = low_compute.into_iter().next() {
        return cpu_status(
            AccelerationReasonCode::ComputeCapabilityTooLow,
            format!(
                "{} 的 Compute Capability {} 低于最低要求 {}，已使用 CPU",
                device.name, device.compute_capability, requirements.minimum_compute_capability
            ),
            Some(device),
            requirements,
        );
    }
    if let Some(device) = devices.iter().find(|device| {
        parse_version(&device.driver_version).is_some_and(|version| version >= minimum_driver)
            && parse_version(&device.compute_capability).is_none()
    }) {
        return cpu_status(
            AccelerationReasonCode::ComputeCapabilityUnknown,
            format!("无法读取 {} 的 Compute Capability，已使用 CPU", device.name),
            Some(device.clone()),
            requirements,
        );
    }

    let mut old_driver = devices
        .iter()
        .filter_map(|device| parse_version(&device.driver_version).map(|v| (v, device.clone())))
        .collect::<Vec<_>>();
    old_driver.sort_by_key(|item| std::cmp::Reverse(item.0));
    if let Some((_, device)) = old_driver.into_iter().next() {
        return cpu_status(
            AccelerationReasonCode::DriverTooOld,
            format!(
                "NVIDIA 驱动 {} 低于最低要求 {}，已使用 CPU",
                device.driver_version, requirements.minimum_driver_version
            ),
            Some(device),
            requirements,
        );
    }
    let device = devices.into_iter().next();
    cpu_status(
        AccelerationReasonCode::DriverVersionUnknown,
        "无法读取 NVIDIA 驱动版本，已使用 CPU".into(),
        device,
        requirements,
    )
}

async fn probe_gpu_devices() -> std::result::Result<Vec<GpuDeviceInfo>, ProbeError> {
    let candidate = nvidia_smi_candidates()
        .into_iter()
        .find(|candidate| candidate.is_file())
        .ok_or(ProbeError::NotFound)?;
    let manager = ProcessManager::new();
    let result = tokio::time::timeout(
        NVIDIA_SMI_TIMEOUT,
        manager.run(ProcessSpec {
            executable: candidate,
            args: vec![
                OsString::from("--query-gpu=index,name,driver_version,compute_cap"),
                OsString::from("--format=csv,noheader,nounits"),
            ],
            working_directory: None,
            log_path: None,
            observer: None,
        }),
    )
    .await
    .map_err(|_| ProbeError::Timeout)?
    .map_err(|_| ProbeError::Failed)?;
    if !result.success {
        return Err(ProbeError::Failed);
    }
    parse_nvidia_smi_csv(&result.stdout)
}

fn parse_nvidia_smi_csv(output: &str) -> std::result::Result<Vec<GpuDeviceInfo>, ProbeError> {
    let mut devices = Vec::new();
    for line in output.lines().filter(|line| !line.trim().is_empty()) {
        let fields = line.split(',').map(str::trim).collect::<Vec<_>>();
        if fields.len() < 4 {
            return Err(ProbeError::InvalidOutput);
        }
        let index = fields[0]
            .parse::<u32>()
            .map_err(|_| ProbeError::InvalidOutput)?;
        devices.push(GpuDeviceInfo {
            index,
            name: fields[1..fields.len() - 2].join(", "),
            driver_version: fields[fields.len() - 2].to_string(),
            compute_capability: fields[fields.len() - 1].to_string(),
        });
    }
    if devices.is_empty() {
        Err(ProbeError::NoGpu)
    } else {
        Ok(devices)
    }
}

fn parse_version(value: &str) -> Option<NumericVersion> {
    let mut parts = value.trim().split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    Some(NumericVersion(major, minor))
}

fn nvidia_smi_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    #[cfg(windows)]
    {
        match std::env::var_os("SystemRoot") {
            Some(root) => candidates.push(PathBuf::from(root).join("System32").join("nvidia-smi.exe")),
            None => candidates.push(PathBuf::from(r"C:\Windows\System32\nvidia-smi.exe")),
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            #[cfg(windows)]
            candidates.push(dir.join("nvidia-smi.exe"));
            #[cfg(not(windows))]
            candidates.push(dir.join("nvidia-smi"));
        }
    }
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;

    fn requirements() -> AccelerationRequirements {
        default_requirements()
    }

    fn device(index: u32, driver: &str, compute: &str) -> GpuDeviceInfo {
        GpuDeviceInfo {
            index,
            name: format!("GPU {index}"),
            driver_version: driver.into(),
            compute_capability: compute.into(),
        }
    }

    #[test]
    fn parses_nvidia_smi_csv() {
        let devices = parse_nvidia_smi_csv(
            "0, NVIDIA GeForce RTX 3060 Ti, 560.81, 8.6\n1, NVIDIA RTX 4090, 560.81, 8.9\n",
        )
        .unwrap();
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].name, "NVIDIA GeForce RTX 3060 Ti");
        assert_eq!(devices[1].compute_capability, "8.9");
    }

    #[test]
    fn rejects_malformed_nvidia_smi_output() {
        assert!(matches!(
            parse_nvidia_smi_csv("not a gpu row"),
            Err(ProbeError::InvalidOutput)
        ));
        assert!(matches!(parse_nvidia_smi_csv("\n"), Err(ProbeError::NoGpu)));
    }

    #[test]
    fn accepts_exact_compatibility_boundaries() {
        let status = choose_acceleration(vec![device(0, "528.33", "5.0")], requirements());
        assert_eq!(status.backend, ColmapBackend::Gpu);
        assert_eq!(status.reason_code, AccelerationReasonCode::GpuReady);
    }

    #[test]
    fn rejects_old_driver_and_low_compute_capability() {
        let old_driver = choose_acceleration(vec![device(0, "528.32", "8.6")], requirements());
        assert_eq!(old_driver.backend, ColmapBackend::Cpu);
        assert_eq!(old_driver.reason_code, AccelerationReasonCode::DriverTooOld);

        let old_gpu = choose_acceleration(vec![device(0, "560.81", "4.9")], requirements());
        assert_eq!(old_gpu.backend, ColmapBackend::Cpu);
        assert_eq!(
            old_gpu.reason_code,
            AccelerationReasonCode::ComputeCapabilityTooLow
        );
    }

    #[test]
    fn selects_highest_compute_capability_then_lowest_index() {
        let status = choose_acceleration(
            vec![
                device(2, "560.81", "8.6"),
                device(1, "560.81", "8.9"),
                device(0, "560.81", "8.9"),
            ],
            requirements(),
        );
        assert_eq!(status.device.unwrap().index, 0);
    }

    #[test]
    fn maps_probe_failures_to_conservative_cpu_reasons() {
        for (error, reason) in [
            (
                ProbeError::NotFound,
                AccelerationReasonCode::NvidiaSmiNotFound,
            ),
            (ProbeError::Failed, AccelerationReasonCode::ProbeFailed),
            (ProbeError::Timeout, AccelerationReasonCode::ProbeTimeout),
            (ProbeError::NoGpu, AccelerationReasonCode::NoNvidiaGpu),
            (
                ProbeError::InvalidOutput,
                AccelerationReasonCode::ComputeCapabilityUnknown,
            ),
        ] {
            let status = probe_error_status(error, requirements());
            assert_eq!(status.backend, ColmapBackend::Cpu);
            assert_eq!(status.reason_code, reason);
        }
    }

    #[test]
    fn requires_the_complete_locked_cuda_runtime_set() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("cudart64_12.dll"), []).unwrap();
        std::fs::write(directory.path().join("curand64_10.dll"), []).unwrap();
        assert!(!runtime_contains_cuda(directory.path()));
        std::fs::write(directory.path().join("onnxruntime_providers_cuda.dll"), []).unwrap();
        assert!(runtime_contains_cuda(directory.path()));
    }

    #[test]
    fn resolves_engine_paths_across_platforms() {
        let root = tempfile::tempdir().unwrap();
        let ffmpeg_dir = root.path().join("ffmpeg");
        let colmap_dir = root.path().join("colmap").join("bin");
        let brush_dir = root.path().join("brush");
        std::fs::create_dir_all(&ffmpeg_dir).unwrap();
        std::fs::create_dir_all(&colmap_dir).unwrap();
        std::fs::create_dir_all(&brush_dir).unwrap();

        #[cfg(windows)]
        {
            std::fs::write(ffmpeg_dir.join("ffmpeg.exe"), []).unwrap();
            std::fs::write(ffmpeg_dir.join("ffprobe.exe"), []).unwrap();
            std::fs::write(colmap_dir.join("colmap.exe"), []).unwrap();
            std::fs::write(brush_dir.join("brush_app.exe"), []).unwrap();
        }
        #[cfg(not(windows))]
        {
            std::fs::write(ffmpeg_dir.join("ffmpeg"), []).unwrap();
            std::fs::write(ffmpeg_dir.join("ffprobe"), []).unwrap();
            std::fs::write(colmap_dir.join("colmap"), []).unwrap();
            std::fs::write(brush_dir.join("brush_app"), []).unwrap();
        }

        let paths = EnginePaths::from_root(root.path());
        assert!(paths.ffmpeg.is_file());
        assert!(paths.ffprobe.is_file());
        assert!(paths.colmap.is_file());
        assert!(paths.brush.is_file());
    }
}
