//! Debug-only native E2E host. Production planners, validators and task service
//! are unchanged. Only external facts and process execution are substituted.
mod artifacts;
mod engine;
pub mod lifecycle;

use crate::{
    engines::gpu_selection::{BrushDeviceKind, GpuDeviceOption, GpuInventory, SelectionConfidence},
    error::{Result, SplatError},
};
pub use engine::execute;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};
use tokio::sync::Notify;

pub fn enabled() -> bool {
    // Native E2E binaries always fail closed, even before initialize(). Rust
    // unit tests retain their existing temp-directory/process fixtures.
    !cfg!(test) || HARNESS.get().is_some()
}
pub(super) fn invalid(message: impl std::fmt::Display) -> SplatError {
    SplatError::Process(format!("E2E harness: {message}"))
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InputFixture {
    pub name: String,
    pub kind: String,
    #[serde(default = "default_count")]
    pub count: u32,
    #[serde(default = "default_width")]
    pub width: u32,
    #[serde(default = "default_height")]
    pub height: u32,
    #[serde(default)]
    pub portrait: bool,
    #[serde(default)]
    pub alpha: bool,
    #[serde(default)]
    pub transparent: bool,
    #[serde(default)]
    pub erp: bool,
    #[serde(default)]
    pub jpeg: bool,
    #[serde(default)]
    pub mismatched_dimensions: bool,
    #[serde(default = "default_fps")]
    pub fps: f64,
    #[serde(default = "default_duration")]
    pub duration: f64,
    #[serde(default)]
    pub timestamps: Vec<f64>,
}
fn default_count() -> u32 {
    30
}
fn default_width() -> u32 {
    96
}
fn default_height() -> u32 {
    64
}
fn default_fps() -> f64 {
    30.
}
fn default_duration() -> f64 {
    2.
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Fault {
    pub operation: String,
    pub occurrence: usize,
    pub outcome: String,
    #[serde(default)]
    pub gate: Option<String>,
    #[serde(default)]
    pub registered: Option<usize>,
    #[serde(default)]
    pub task: Option<usize>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub registered_by_source: std::collections::BTreeMap<String, usize>,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Scenario {
    pub name: String,
    pub inputs: Vec<InputFixture>,
    pub allowed_operations: Vec<String>,
    #[serde(default)]
    pub faults: Vec<Fault>,
    #[serde(default)]
    pub multiple_gpus: bool,
    #[serde(default)]
    pub cpu_colmap: bool,
    #[serde(default)]
    pub shared_json_readers: bool,
    #[serde(default)]
    pub barriers: Vec<lifecycle::Barrier>,
    #[serde(default)]
    pub late_event: Option<lifecycle::LateRule>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Invocation {
    pub sequence: usize,
    pub engine: String,
    pub operation: String,
    pub args: Vec<String>,
    pub environment: Vec<(String, String)>,
    pub working_directory: Option<PathBuf>,
    pub log_path: Option<PathBuf>,
    pub outcome: String,
    pub dataset_init_files: Vec<String>,
    #[serde(default)]
    pub dataset_init_sha256: Vec<(String, String)>,
    #[serde(default)]
    pub run_id: Option<uuid::Uuid>,
    #[serde(default)]
    pub pipeline_operation: Option<crate::pipeline::event::PipelineOperation>,
    #[serde(default)]
    pub scope: lifecycle::Scope,
    #[serde(default)]
    pub input_lists: std::collections::BTreeMap<String, Vec<String>>,
}
pub(super) struct Harness {
    pub root: PathBuf,
    pub scenario: Scenario,
    pub calls: Mutex<Vec<Invocation>>,
    pub released: Mutex<BTreeSet<String>>,
    pub pending: Mutex<BTreeSet<String>>,
    pub changed: Notify,
    pub lifecycle: lifecycle::Lifecycle,
    pub deferred: Mutex<std::collections::BTreeMap<String, lifecycle::Deferred>>,
}
static HARNESS: OnceLock<Harness> = OnceLock::new();

/// Durable test evidence, using same-directory staging and Windows-compatible rename.
pub(super) fn save_evidence(path: &Path, value: &impl Serialize) -> Result<()> {
    use std::io::Write;
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let bytes = serde_json::to_vec_pretty(value)?;
    file.write_all(&bytes)?;
    file.flush()?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&temporary, path)?;
    Ok(())
}

/// A deterministic read-only observer, not a replacement writer or an I/O
/// retry. Holding this ordinary handle reproduces the Windows overlap that
/// previously made creation/publication saves fail with OS5.
pub fn json_save_reader(path: &Path) -> Result<Option<std::fs::File>> {
    let Some(h) = HARNESS.get().filter(|h| h.scenario.shared_json_readers) else {
        return Ok(None);
    };
    safe_path(&h.root, path)?;
    match std::fs::File::open(path) {
        Ok(file) => Ok(Some(file)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}
// Called by the real operation observer, not inferred from timestamps or the
// most recent task. Probe calls outside a pipeline intentionally have no run.
pub fn bind_process(
    process_id: u32,
    run_id: Option<uuid::Uuid>,
    operation: Option<&crate::pipeline::event::PipelineOperation>,
) {
    if let (Some(h), Some(index)) = (HARNESS.get(), process_id.checked_sub(1_000_001)) {
        if let Some(call) = h.calls.lock().unwrap().get_mut(index as usize) {
            call.run_id = run_id;
            call.pipeline_operation = operation.cloned();
            if let Some(op) = operation {
                call.scope.operation_id = Some(op.id);
                if op.source_id.is_some() {
                    call.scope.source_id = op.source_id.clone();
                    if let Some(bound) = call
                        .working_directory
                        .as_ref()
                        .and_then(|path| lifecycle::scope(path, op.source_id.as_deref()))
                    {
                        call.scope.source_name = bound.source_name;
                    }
                }
            }
        }
    }
}

/// Fault injection after a *real*, durable publication transition. Never
/// manufactures a candidate or marks any task complete.
pub async fn publication_checkpoint(path: &Path, phase: &str) -> Result<()> {
    if !enabled() {
        return Ok(());
    }
    let h = host()?;
    safe_path(&h.root, path)?;
    lifecycle::at(path, None, &format!("publication:{phase}"), "checkpoint", json!({"transaction":std::fs::read(path).ok().and_then(|b|serde_json::from_slice::<Value>(&b).ok())})).await?;
    let key = format!("publication:{phase}");
    if !h.scenario.faults.iter().any(|f| f.operation == key) {
        return Ok(());
    }
    let (sequence, fault) = {
        let mut calls = h.calls.lock().unwrap();
        let occurrence = calls
            .iter()
            .filter(|c| c.engine == "publication" && c.operation == phase)
            .count()
            + 1;
        let sequence = calls.len() + 1;
        let fault = h
            .scenario
            .faults
            .iter()
            .find(|f| f.operation == key && f.occurrence == occurrence)
            .cloned();
        calls.push(Invocation {
            sequence,
            engine: "publication".into(),
            operation: phase.into(),
            args: vec![],
            environment: vec![],
            working_directory: path.parent().map(Path::to_path_buf),
            log_path: None,
            outcome: "running".into(),
            dataset_init_files: vec![],
            dataset_init_sha256: vec![],
            run_id: None,
            pipeline_operation: None,
            scope: lifecycle::scope(path, None).unwrap_or_default(),
            input_lists: Default::default(),
        });
        save_evidence(&h.root.join("invocations.json"), &*calls)?;
        (sequence, fault)
    };
    if let Some(gate) = fault.as_ref().and_then(|f| f.gate.as_ref()) {
        h.pending.lock().unwrap().insert(gate.clone());
        loop {
            let changed = h.changed.notified();
            if h.released.lock().unwrap().contains(gate) {
                break;
            }
            changed.await;
        }
        h.pending.lock().unwrap().remove(gate);
    }
    let fail = fault.is_some_and(|f| f.outcome == "io");
    {
        let mut calls = h.calls.lock().unwrap();
        calls[sequence - 1].outcome = if fail { "error" } else { "success" }.into();
        save_evidence(&h.root.join("invocations.json"), &*calls)?;
    }
    if fail {
        Err(SplatError::Io(std::io::Error::from_raw_os_error(5)))
    } else {
        Ok(())
    }
}
pub(super) fn host() -> Result<&'static Harness> {
    HARNESS
        .get()
        .ok_or_else(|| invalid("no scenario was initialized; external processes are forbidden"))
}
pub fn data_root() -> Result<PathBuf> {
    Ok(host()?.root.join("app-data"))
}
pub fn engines_root() -> Result<PathBuf> {
    Ok(host()?.root.join("engines"))
}
pub fn validate_path(root: &Path, path: &Path) -> Result<()> {
    safe_path(root, path).map(|_| ())
}
pub fn configure_context(context: &mut tauri::Context<tauri::Wry>) -> Result<()> {
    let port = std::env::var("OOOSPLAT_E2E_CDP_PORT")
        .map_err(invalid)?
        .parse::<u16>()
        .map_err(invalid)?;
    if port == 0 {
        return Err(invalid("invalid debug port"));
    }
    let config = context.config_mut();
    config.identifier = "studio.ooo.splat.e2e".into();
    let window = config
        .app
        .windows
        .first_mut()
        .ok_or_else(|| invalid("missing test window"))?;
    window.additional_browser_args=Some(format!("--remote-debugging-port={port} --remote-debugging-address=127.0.0.1 --disable-background-networking"));
    Ok(())
}

/// Check every existing ancestor, not just the final component (junctions on
/// Windows are redirections too). Never follow a test path outside its owner.
pub(super) fn safe_path(root: &Path, path: &Path) -> Result<PathBuf> {
    let portable = |p: &Path| {
        PathBuf::from(
            p.to_string_lossy()
                .strip_prefix(r"\\?\")
                .unwrap_or(&p.to_string_lossy()),
        )
    };
    let root = portable(root);
    let path = std::path::absolute(portable(path))?;
    if !path.starts_with(&root)
        || path
            .components()
            .any(|p| matches!(p, std::path::Component::ParentDir))
    {
        return Err(invalid(format!(
            "path escapes the test root: {}",
            path.display()
        )));
    }
    let mut current = Some(path.as_path());
    while let Some(p) = current {
        if !p.starts_with(&root) {
            break;
        }
        if let Ok(meta) = std::fs::symlink_metadata(p) {
            let symlink = meta.file_type().is_symlink();
            #[cfg(windows)]
            let redirected = {
                use std::os::windows::fs::MetadataExt;
                symlink || meta.file_attributes() & 0x400 != 0
            };
            #[cfg(not(windows))]
            let redirected = symlink;
            if redirected {
                return Err(invalid("redirected test path"));
            }
        }
        current = p.parent();
    }
    Ok(path)
}

pub fn initialize() -> Result<()> {
    if HARNESS.get().is_some() {
        return Ok(());
    }
    let root = std::fs::canonicalize(
        std::env::var_os("OOOSPLAT_E2E_ROOT")
            .ok_or_else(|| invalid("OOOSPLAT_E2E_ROOT is required"))?,
    )?;
    let root = PathBuf::from(
        root.to_string_lossy()
            .strip_prefix(r"\\?\")
            .unwrap_or(&root.to_string_lossy()),
    );
    let workspace = std::fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap())?;
    let allowed = workspace.join(".cache/e2e/runs");
    safe_path(&allowed, &root)?;
    let token = std::env::var("OOOSPLAT_E2E_TOKEN").map_err(invalid)?;
    if token.len() < 32 || std::fs::read_to_string(root.join(".owner"))? != token {
        return Err(invalid("test-root ownership mismatch"));
    }
    let scenario_bytes = std::fs::read(root.join("scenario.json"))?;
    if scenario_bytes.len() > 65536 {
        return Err(invalid("scenario exceeds 64 KiB"));
    }
    let scenario: Scenario = serde_json::from_slice(&scenario_bytes)?;
    if scenario.inputs.is_empty()
        || scenario.inputs.len() > 8
        || scenario.allowed_operations.is_empty()
    {
        return Err(invalid("empty or oversized scenario"));
    }
    let mut calls: Vec<Invocation> = match std::fs::read(root.join("invocations.json")) {
        Ok(bytes) => serde_json::from_slice(&bytes)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e.into()),
    };
    if calls.iter().any(|call| call.outcome == "running") {
        for call in &mut calls {
            if call.outcome == "running" {
                call.outcome = "interrupted".into();
            }
        }
        save_evidence(&root.join("invocations.json"), &calls)?;
    }
    let harness = Harness {
        root,
        scenario,
        calls: Mutex::new(calls),
        released: Mutex::default(),
        pending: Mutex::default(),
        changed: Notify::new(),
        lifecycle: lifecycle::Lifecycle::default(),
        deferred: Mutex::default(),
    };
    lifecycle::initialize(&harness)?;
    artifacts::prepare_inputs(&harness)?;
    HARNESS
        .set(harness)
        .map_err(|_| invalid("duplicate initialization"))?;
    Ok(())
}

pub fn inventory() -> Result<GpuInventory> {
    let h = host()?;
    let devices = (0..if h.scenario.multiple_gpus { 2 } else { 1 })
        .map(|index| GpuDeviceOption {
            local_device_id: format!("e2e-gpu-{index}"),
            name: format!("E2E GPU {index}"),
            vendor: if h.scenario.cpu_colmap {
                "Intel"
            } else {
                "NVIDIA"
            }
            .into(),
            device_kind: BrushDeviceKind::DiscreteGpu,
            adapter_index: index,
            total_memory_mb: Some(8192),
            driver: Some("600.00".into()),
            compute_capability: Some("8.6".into()),
            colmap_compatible: !h.scenario.cpu_colmap,
            colmap_gpu_index: (!h.scenario.cpu_colmap).then_some(index as u32),
            recommended: index == 0,
            selection_confidence: SelectionConfidence::Ordered,
            duplicate_model: false,
        })
        .collect();
    Ok(GpuInventory {
        devices,
        recommended_device_id: Some("e2e-gpu-0".into()),
    })
}
pub fn acceleration() -> Result<crate::engines::ColmapAccelerationStatus> {
    use crate::engines::health::*;
    let d = inventory()?.devices.remove(0);
    Ok(ColmapAccelerationStatus {
        backend: if d.colmap_compatible {
            ColmapBackend::Gpu
        } else {
            ColmapBackend::Cpu
        },
        detection_state: GpuDetectionState::Ready,
        reason_code: if d.colmap_compatible {
            AccelerationReasonCode::GpuReady
        } else {
            AccelerationReasonCode::NoNvidiaGpu
        },
        reason: "E2E simulated hardware; not a hardware validation".into(),
        device: d.colmap_compatible.then(|| GpuDeviceInfo {
            index: 0,
            name: d.name,
            driver_version: "600.00".into(),
            compute_capability: "8.6".into(),
            total_memory_mb: Some(8192),
        }),
        requirements: AccelerationRequirements {
            minimum_driver_version: "580.00".into(),
            minimum_compute_capability: "7.5".into(),
        },
        detected_nvidia_device_count: usize::from(d.colmap_compatible),
    })
}
pub fn health(paths: &crate::engines::EnginePaths) -> Result<Vec<crate::engines::EngineStatus>> {
    use crate::engines::health::*;
    [
        (EngineKind::Ffmpeg, &paths.ffmpeg),
        (EngineKind::Ffprobe, &paths.ffprobe),
        (EngineKind::Colmap, &paths.colmap),
        (EngineKind::Brush, &paths.brush),
    ]
    .into_iter()
    .map(|(kind, path)| {
        Ok(EngineStatus {
            kind,
            path: path.clone(),
            exists: true,
            can_start: true,
            version: Some("E2E fixture".into()),
            cpu_only: None,
            acceleration: if kind == EngineKind::Colmap {
                Some(acceleration()?)
            } else {
                None
            },
            colmap_cli_family: (kind == EngineKind::Colmap)
                .then_some(crate::engines::colmap::ColmapCliFamily::Modern4),
            detail: "Simulated engine; real execution forbidden".into(),
        })
    })
    .collect()
}

#[tauri::command]
pub async fn e2e_snapshot(
    tasks: tauri::State<'_, crate::tasks::TaskService>,
) -> std::result::Result<Value, String> {
    let h = host().map_err(|e| e.to_string())?;
    let tasks = tasks.e2e_records();
    let mut projects = Vec::new();
    for task in &tasks {
        if let Some(path) = task
            .project_path
            .as_ref()
            .or_else(|| task.creation_intent.as_ref().map(|i| &i.project_path))
            .filter(|p| p.join("state.json").exists())
        {
            safe_path(&h.root, path).map_err(|e| e.to_string())?;
            let read = |name: &str| {
                let file = path.join(name);
                if safe_path(&h.root, &file).is_err() {
                    return Some(json!({"e2eReadError":"redirected state path"}));
                }
                if file.metadata().is_ok_and(|m| m.len() > 8 * 1024 * 1024) {
                    return Some(json!({"e2eReadError":"state exceeds test budget"}));
                }
                std::fs::read(file)
                    .ok()
                    .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            };
            projects.push(json!({"taskId":task.task_id,"path":path,"state":read("state.json"),"metadata":read("project.json")}));
        }
    }
    Ok(
        json!({"scenario":h.scenario.name,"root":h.root,"inputs":h.scenario.inputs.iter().map(|i|h.root.join("fixtures").join(&i.name)).collect::<Vec<_>>(),"calls":*h.calls.lock().unwrap(),"pending":*h.pending.lock().unwrap(),"tasks":tasks,"projects":projects,"lifecycle":lifecycle::snapshot(h)}),
    )
}
#[tauri::command]
pub fn e2e_release(
    gate: String,
    task_id: uuid::Uuid,
    run_id: uuid::Uuid,
) -> std::result::Result<(), String> {
    let h = host().map_err(|e| e.to_string())?;
    if !h
        .scenario
        .faults
        .iter()
        .any(|f| f.gate.as_ref() == Some(&gate))
    {
        return Err("Unknown execution gate".into());
    }
    if !h.pending.lock().unwrap().contains(&gate)
        || !h.calls.lock().unwrap().iter().any(|call| {
            call.outcome == "running"
                && call.scope.task_id == Some(task_id)
                && call.scope.run_id == Some(run_id)
                && h.scenario.faults.iter().any(|fault| {
                    fault.gate.as_ref() == Some(&gate)
                        && fault.operation == format!("{}:{}", call.engine, call.operation)
                })
        })
    {
        return Err("Stale or foreign execution gate".into());
    }
    h.released.lock().unwrap().insert(gate);
    h.changed.notify_waiters();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unknown_scenario_fields_are_rejected() {
        assert!(serde_json::from_value::<Scenario>(
            json!({"name":"x","inputs":[],"allowedOperations":[],"recipe":{}})
        )
        .is_err());
    }
    #[test]
    fn test_paths_cannot_escape() {
        let root = std::env::temp_dir().join("e2e-path-test");
        assert!(safe_path(&root, &root.join("../outside")).is_err());
        assert!(safe_path(&root, &root.join("frames/a.png")).is_ok());
    }
}
