use std::{
    collections::HashMap,
    ffi::OsString,
    path::{Path, PathBuf},
    process::Stdio,
    sync::OnceLock,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use crate::error::{Result, SplatError};

use super::{
    health::{
        check_colmap_acceleration, diagnostic_gpu_devices, diagnostic_gpu_identities,
        LocalGpuIdentity,
    },
    AccelerationReasonCode, ColmapAccelerationStatus, ColmapBackend, EnginePaths,
    GpuDetectionState, GpuDeviceInfo,
};

const INVENTORY_TTL: Duration = Duration::from_secs(10 * 60);
const PROBE_TIMEOUT: Duration = Duration::from_secs(8);
const PROBE_DEVICE: &str = "DiscreteGpu(4294967295)";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BrushDeviceKind {
    DiscreteGpu,
    IntegratedGpu,
}

impl BrushDeviceKind {
    pub fn cubecl_name(self, index: usize) -> String {
        match self {
            Self::DiscreteGpu => format!("DiscreteGpu({index})"),
            Self::IntegratedGpu => format!("IntegratedGpu({index})"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SelectionConfidence {
    PciIdentity,
    UniqueModel,
    Ordered,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuSelection {
    pub local_device_id: String,
    pub device_kind: BrushDeviceKind,
    pub selection_confidence: SelectionConfidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuDeviceOption {
    pub local_device_id: String,
    pub name: String,
    pub vendor: String,
    pub device_kind: BrushDeviceKind,
    pub adapter_index: usize,
    pub total_memory_mb: Option<u64>,
    pub driver: Option<String>,
    #[serde(skip_serializing)]
    pub compute_capability: Option<String>,
    pub colmap_compatible: bool,
    pub colmap_gpu_index: Option<u32>,
    pub recommended: bool,
    pub selection_confidence: SelectionConfidence,
    pub duplicate_model: bool,
}

impl GpuDeviceOption {
    pub fn selection(&self) -> GpuSelection {
        GpuSelection {
            local_device_id: self.local_device_id.clone(),
            device_kind: self.device_kind,
            selection_confidence: self.selection_confidence,
        }
    }

    pub fn cubecl_device(&self) -> String {
        self.device_kind.cubecl_name(self.adapter_index)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuInventory {
    pub devices: Vec<GpuDeviceOption>,
    pub recommended_device_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BaBackendPlan {
    Ceres,
    CasparEligible,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedHardwarePlan {
    pub selection: GpuSelection,
    pub device_name: String,
    pub device_vendor: String,
    pub brush_device: String,
    pub brush_total_memory_mb: Option<u64>,
    pub colmap_backend: ColmapBackend,
    pub colmap_gpu_index: Option<u32>,
    pub ba_backend: BaBackendPlan,
    pub device_summary: String,
    pub selection_confidence: SelectionConfidence,
}

impl ResolvedHardwarePlan {
    pub fn brush_environment(&self) -> (OsString, OsString) {
        (
            OsString::from("CUBECL_WGPU_DEFAULT_DEVICE"),
            OsString::from(&self.brush_device),
        )
    }
}

#[derive(Debug, Clone)]
struct AdapterRecord {
    name: String,
    vendor_id: u32,
    device_id: u32,
    kind: BrushDeviceKind,
    index: usize,
    pci_bus_id: Option<String>,
    driver: Option<String>,
}

static INVENTORY_CACHE: OnceLock<Mutex<Option<(Instant, PathBuf, GpuInventory)>>> = OnceLock::new();

pub async fn inventory(paths: &EnginePaths, refresh: bool) -> Result<GpuInventory> {
    // Pipeline/MCP unit tests intentionally construct engine roots without
    // bundled native binaries. Keep their admission tests independent from
    // host GPU hardware; production builds always probe the pinned Brush.
    #[cfg(test)]
    if !paths.brush.is_file() {
        return Ok(test_inventory());
    }
    let cache = INVENTORY_CACHE.get_or_init(|| Mutex::new(None));
    let mut guard = cache.lock().await;
    if !refresh {
        if let Some((recorded, executable, value)) = guard.as_ref() {
            if executable == &paths.brush && recorded.elapsed() <= INVENTORY_TTL {
                return Ok(value.clone());
            }
        }
    }
    let adapters = probe_brush_adapters(&paths.brush).await?;
    let (nvidia, nvidia_identities) =
        tokio::join!(diagnostic_gpu_devices(), diagnostic_gpu_identities());
    let automatic = check_colmap_acceleration(paths).await;
    let value = build_inventory(adapters, nvidia, nvidia_identities, &automatic);
    *guard = Some((Instant::now(), paths.brush.clone(), value.clone()));
    Ok(value)
}

#[cfg(test)]
fn test_inventory() -> GpuInventory {
    let device = GpuDeviceOption {
        local_device_id: "test-gpu".into(),
        name: "Test GPU".into(),
        vendor: "NVIDIA".into(),
        device_kind: BrushDeviceKind::DiscreteGpu,
        adapter_index: 0,
        total_memory_mb: Some(8 * 1024),
        driver: Some("test".into()),
        compute_capability: Some("9.0".into()),
        colmap_compatible: true,
        colmap_gpu_index: Some(0),
        recommended: true,
        selection_confidence: SelectionConfidence::UniqueModel,
        duplicate_model: false,
    };
    GpuInventory {
        recommended_device_id: Some(device.local_device_id.clone()),
        devices: vec![device],
    }
}

pub async fn resolve(
    paths: &EnginePaths,
    selection: &GpuSelection,
) -> Result<(ResolvedHardwarePlan, ColmapAccelerationStatus)> {
    let inventory = inventory(paths, true).await?;
    let selected = inventory
        .devices
        .iter()
        .find(|device| device.local_device_id == selection.local_device_id)
        .ok_or_else(|| SplatError::Process("GPU_SELECTION_UNAVAILABLE".into()))?;
    if selected.device_kind != selection.device_kind {
        return Err(SplatError::Process("GPU_SELECTION_CHANGED".into()));
    }
    let automatic = check_colmap_acceleration(paths).await;
    let colmap_backend = if selected.colmap_compatible {
        ColmapBackend::Gpu
    } else {
        ColmapBackend::Cpu
    };
    let acceleration = ColmapAccelerationStatus {
        backend: colmap_backend,
        detection_state: GpuDetectionState::Ready,
        reason_code: if selected.colmap_compatible {
            AccelerationReasonCode::GpuReady
        } else {
            AccelerationReasonCode::NoNvidiaGpu
        },
        reason: if selected.colmap_compatible {
            format!(
                "已启用 {}（驱动 {}，Compute Capability {}）",
                selected.name,
                selected.driver.as_deref().unwrap_or_default(),
                selected.compute_capability.as_deref().unwrap_or_default(),
            )
        } else {
            format!("{} is used for training; COLMAP uses CPU", selected.name)
        },
        device: selected.colmap_gpu_index.map(|index| GpuDeviceInfo {
            index,
            name: selected.name.clone(),
            driver_version: selected.driver.clone().unwrap_or_default(),
            compute_capability: selected.compute_capability.clone().unwrap_or_default(),
            total_memory_mb: selected.total_memory_mb,
        }),
        requirements: automatic.requirements,
        detected_nvidia_device_count: automatic.detected_nvidia_device_count,
    };
    Ok((
        ResolvedHardwarePlan {
            selection: selected.selection(),
            device_name: selected.name.clone(),
            device_vendor: selected.vendor.clone(),
            brush_device: selected.cubecl_device(),
            brush_total_memory_mb: selected.total_memory_mb,
            colmap_backend,
            colmap_gpu_index: selected
                .colmap_gpu_index
                .filter(|_| selected.colmap_compatible),
            ba_backend: if selected.colmap_compatible {
                BaBackendPlan::CasparEligible
            } else {
                BaBackendPlan::Ceres
            },
            device_summary: format!(
                "{} · {} · {}",
                selected.name,
                selected.vendor,
                selected.cubecl_device()
            ),
            selection_confidence: selected.selection_confidence,
        },
        acceleration,
    ))
}

async fn probe_brush_adapters(executable: &Path) -> Result<Vec<AdapterRecord>> {
    if !executable.is_file() {
        return Err(SplatError::EngineMissing(executable.display().to_string()));
    }
    let missing = std::env::temp_dir().join(format!(
        "ooosplat-gpu-probe-missing-{}",
        uuid::Uuid::new_v4()
    ));
    let mut command = tokio::process::Command::new(executable);
    command.kill_on_drop(true);
    command
        .arg(&missing)
        .env("CUBECL_WGPU_DEFAULT_DEVICE", PROBE_DEVICE)
        .env(
            "RUST_LOG",
            "brush_cli=info,brush_process=info,cubecl_wgpu=info,burn_wgpu=info",
        )
        .env("RUST_BACKTRACE", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.as_std_mut().creation_flags(0x0800_0000);
    }
    let output = tokio::time::timeout(PROBE_TIMEOUT, command.output())
        .await
        .map_err(|_| SplatError::Process("GPU adapter detection timed out".into()))??;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    let adapters = parse_adapter_records(&text);
    if adapters.is_empty() {
        return Err(SplatError::Process(
            "No Brush-compatible hardware graphics device was detected".into(),
        ));
    }
    Ok(adapters)
}

fn parse_adapter_records(text: &str) -> Vec<AdapterRecord> {
    let mut records = Vec::new();
    // The deliberate invalid-device probe prints the complete adapter list in
    // one diagnostic. Restrict parsing to that list so a logger repeating the
    // same diagnostic cannot manufacture duplicate devices. Devices inside
    // the list are intentionally not deduplicated: two identical boards are
    // distinct selectable adapters even when a platform omits PCI identity.
    let Some((_, adapter_list)) = text.split_once("adapters [") else {
        return records;
    };
    let mut rest = adapter_list;
    while let Some(start) = rest.find("AdapterInfo {") {
        rest = &rest[start + "AdapterInfo {".len()..];
        let Some(end) = rest.find('}') else { break };
        let body = &rest[..end];
        rest = &rest[end + 1..];
        let Some(name) = quoted(body, "name: \"") else {
            continue;
        };
        let Some(device_type) = scalar(body, "device_type: ") else {
            continue;
        };
        let kind = match device_type {
            "DiscreteGpu" => BrushDeviceKind::DiscreteGpu,
            "IntegratedGpu" => BrushDeviceKind::IntegratedGpu,
            _ => continue,
        };
        let vendor_id = scalar(body, "vendor: ")
            .and_then(|value| value.parse().ok())
            .unwrap_or_default();
        let device_id = scalar(body, "device: ")
            .and_then(|value| value.parse().ok())
            .unwrap_or_default();
        let pci_bus_id = quoted(body, "device_pci_bus_id: \"")
            .filter(|value| !value.is_empty() && *value != "0000:00:00.0")
            .map(str::to_owned);
        let driver = quoted(body, "driver: \"").map(str::to_owned);
        let index = records.iter().filter(|record| record.kind == kind).count();
        records.push(AdapterRecord {
            name: name.to_owned(),
            vendor_id,
            device_id,
            kind,
            index,
            pci_bus_id,
            driver,
        });
    }
    records
}

fn quoted<'a>(body: &'a str, marker: &str) -> Option<&'a str> {
    let value = body.split_once(marker)?.1;
    value.split_once('"').map(|(value, _)| value)
}

fn scalar<'a>(body: &'a str, marker: &str) -> Option<&'a str> {
    let value = body.split_once(marker)?.1;
    Some(value.split(',').next()?.trim())
}

fn build_inventory(
    adapters: Vec<AdapterRecord>,
    nvidia: Vec<GpuDeviceInfo>,
    nvidia_identities: Vec<LocalGpuIdentity>,
    automatic: &ColmapAccelerationStatus,
) -> GpuInventory {
    let mut model_counts = HashMap::<String, usize>::new();
    for adapter in &adapters {
        *model_counts
            .entry(normalize_name(&adapter.name))
            .or_default() += 1;
    }
    let mut nvidia_ordinals = HashMap::<String, usize>::new();
    let mut devices = Vec::with_capacity(adapters.len());
    for adapter in adapters {
        let normalized = normalize_name(&adapter.name);
        let duplicate_model = model_counts.get(&normalized).copied().unwrap_or(0) > 1;
        let pci_identity = adapter.pci_bus_id.as_deref().and_then(|pci| {
            nvidia_identities.iter().find(|identity| {
                identity
                    .pci_bus_id
                    .as_deref()
                    .is_some_and(|candidate| same_pci(candidate, pci))
            })
        });
        let ordinal = nvidia_ordinals.entry(normalized.clone()).or_default();
        let model_ordinal = *ordinal;
        let mapped_identity = pci_identity.or_else(|| {
            nvidia_identities
                .iter()
                .filter(|identity| normalize_name(&identity.name) == normalized)
                .nth(model_ordinal)
        });
        let mapped_nvidia = mapped_identity
            .and_then(|identity| nvidia.iter().find(|device| device.index == identity.index))
            .cloned()
            .or_else(|| {
                nvidia
                    .iter()
                    .filter(|device| normalize_name(&device.name) == normalized)
                    .nth(model_ordinal)
                    .cloned()
            });
        if pci_identity.is_none() && mapped_nvidia.is_some() {
            *ordinal += 1;
        }
        let compatible = mapped_nvidia.as_ref().is_some_and(|device| {
            version_at_least(
                &device.driver_version,
                &automatic.requirements.minimum_driver_version,
            ) && version_at_least(
                &device.compute_capability,
                &automatic.requirements.minimum_compute_capability,
            )
        });
        // Prefer a shared PCI identity. If either runtime omits it, identical
        // boards intentionally fall back to their respective adapter order.
        let confidence = if pci_identity.is_some() && mapped_nvidia.is_some() {
            SelectionConfidence::PciIdentity
        } else if duplicate_model {
            SelectionConfidence::Ordered
        } else {
            SelectionConfidence::UniqueModel
        };
        let stable_uuid = if duplicate_model {
            None
        } else {
            mapped_identity.and_then(|identity| identity.uuid.as_deref())
        };
        let local_device_id = local_device_id(&adapter, stable_uuid);
        devices.push(GpuDeviceOption {
            local_device_id,
            name: adapter.name,
            vendor: vendor_name(adapter.vendor_id).to_owned(),
            device_kind: adapter.kind,
            adapter_index: adapter.index,
            total_memory_mb: mapped_nvidia
                .as_ref()
                .and_then(|device| device.total_memory_mb),
            driver: mapped_nvidia
                .as_ref()
                .map(|device| device.driver_version.clone())
                .or(adapter.driver),
            compute_capability: mapped_nvidia
                .as_ref()
                .map(|device| device.compute_capability.clone()),
            colmap_compatible: compatible,
            colmap_gpu_index: mapped_nvidia.map(|device| device.index),
            recommended: false,
            selection_confidence: confidence,
            duplicate_model,
        });
    }
    let recommended = automatic
        .device
        .as_ref()
        .and_then(|selected| {
            devices.iter().find(|device| {
                device.colmap_gpu_index == Some(selected.index) && device.colmap_compatible
            })
        })
        .or_else(|| {
            devices
                .iter()
                .find(|device| device.device_kind == BrushDeviceKind::DiscreteGpu)
        })
        .or_else(|| devices.first())
        .map(|device| device.local_device_id.clone());
    for device in &mut devices {
        device.recommended = recommended.as_deref() == Some(device.local_device_id.as_str());
    }
    GpuInventory {
        devices,
        recommended_device_id: recommended,
    }
}

fn local_device_id(adapter: &AdapterRecord, stable_uuid: Option<&str>) -> String {
    let mut hash = Sha256::new();
    hash.update(adapter.vendor_id.to_le_bytes());
    hash.update(adapter.device_id.to_le_bytes());
    hash.update(adapter.name.as_bytes());
    hash.update(adapter.pci_bus_id.as_deref().unwrap_or_default().as_bytes());
    hash.update(stable_uuid.unwrap_or_default().as_bytes());
    hash.update([adapter.kind as u8]);
    // A hardware PCI identity survives WGPU adapter-order changes. Platforms
    // that do not expose one deliberately fall back to the system order.
    if adapter.pci_bus_id.is_none() && stable_uuid.is_none() {
        hash.update(adapter.index.to_le_bytes());
    }
    format!("gpu-{}", &format!("{:x}", hash.finalize())[..16])
}

fn same_pci(left: &str, right: &str) -> bool {
    let normalize = |value: &str| {
        value
            .trim()
            .trim_start_matches("00000000:")
            .trim_start_matches("0000:")
            .to_ascii_lowercase()
    };
    normalize(left) == normalize(right)
}

fn normalize_name(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(|character| character.to_lowercase())
        .collect()
}

fn version_at_least(actual: &str, minimum: &str) -> bool {
    let parse = |value: &str| {
        value
            .split('.')
            .map(|part| part.parse::<u64>().unwrap_or_default())
            .collect::<Vec<_>>()
    };
    let mut actual = parse(actual);
    let mut minimum = parse(minimum);
    let length = actual.len().max(minimum.len());
    actual.resize(length, 0);
    minimum.resize(length, 0);
    actual >= minimum
}

fn vendor_name(vendor: u32) -> &'static str {
    match vendor {
        0x10de => "NVIDIA",
        0x1002 | 0x1022 => "AMD",
        0x8086 => "Intel",
        0x106b => "Apple",
        _ => "Other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pinned_brush_adapter_output_and_filters_software_devices() {
        let text = r#"No Discrete GPU device found, adapters [AdapterInfo { name: "NVIDIA RTX", vendor: 4318, device: 123, device_type: DiscreteGpu, device_pci_bus_id: "0000:01:00.0", driver: "NVIDIA", driver_info: "1", backend: Vulkan }, AdapterInfo { name: "Intel UHD", vendor: 32902, device: 456, device_type: IntegratedGpu, device_pci_bus_id: "0000:00:02.0", driver: "Intel", driver_info: "2", backend: Vulkan }, AdapterInfo { name: "llvmpipe", vendor: 0, device: 0, device_type: Cpu, device_pci_bus_id: "", driver: "", driver_info: "", backend: Vulkan }]"#;
        let parsed = parse_adapter_records(text);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].index, 0);
        assert_eq!(parsed[1].kind, BrushDeviceKind::IntegratedGpu);
    }

    #[test]
    fn duplicate_models_keep_distinct_local_ids_and_ordered_confidence() {
        let adapter = |bus: &str, index| AdapterRecord {
            name: "NVIDIA RTX".into(),
            vendor_id: 0x10de,
            device_id: 1,
            kind: BrushDeviceKind::DiscreteGpu,
            index,
            pci_bus_id: Some(bus.into()),
            driver: None,
        };
        let automatic = ColmapAccelerationStatus {
            backend: ColmapBackend::Cpu,
            detection_state: GpuDetectionState::Ready,
            reason_code: AccelerationReasonCode::NoNvidiaGpu,
            reason: String::new(),
            device: None,
            requirements: super::super::AccelerationRequirements {
                minimum_driver_version: "1".into(),
                minimum_compute_capability: "1".into(),
            },
            detected_nvidia_device_count: 0,
        };
        let inventory = build_inventory(
            vec![adapter("1", 0), adapter("2", 1)],
            vec![],
            vec![],
            &automatic,
        );
        assert_ne!(
            inventory.devices[0].local_device_id,
            inventory.devices[1].local_device_id
        );
        assert!(inventory
            .devices
            .iter()
            .all(|device| device.duplicate_model));
        assert!(inventory
            .devices
            .iter()
            .all(|device| device.selection_confidence == SelectionConfidence::Ordered));
    }

    #[test]
    fn pci_identity_maps_cuda_and_survives_adapter_reordering() {
        let adapter = |index| AdapterRecord {
            name: "NVIDIA RTX".into(),
            vendor_id: 0x10de,
            device_id: 1,
            kind: BrushDeviceKind::DiscreteGpu,
            index,
            pci_bus_id: Some("0000:01:00.0".into()),
            driver: None,
        };
        assert_eq!(
            local_device_id(&adapter(0), None),
            local_device_id(&adapter(1), None)
        );
        assert!(same_pci("00000000:01:00.0", "0000:01:00.0"));
    }

    #[test]
    fn recommendation_follows_existing_colmap_choice_through_pci_mapping() {
        let adapters = ["0000:01:00.0", "0000:02:00.0"]
            .into_iter()
            .enumerate()
            .map(|(index, bus)| AdapterRecord {
                name: "NVIDIA RTX".into(),
                vendor_id: 0x10de,
                device_id: 1,
                kind: BrushDeviceKind::DiscreteGpu,
                index,
                pci_bus_id: Some(bus.into()),
                driver: None,
            })
            .collect();
        let device = |index| GpuDeviceInfo {
            index,
            name: "NVIDIA RTX".into(),
            driver_version: "600".into(),
            compute_capability: "9.0".into(),
            total_memory_mb: Some(8192),
        };
        let identities = vec![
            LocalGpuIdentity {
                index: 0,
                name: "NVIDIA RTX".into(),
                pci_bus_id: Some("00000000:02:00.0".into()),
                uuid: Some("second".into()),
            },
            LocalGpuIdentity {
                index: 1,
                name: "NVIDIA RTX".into(),
                pci_bus_id: Some("00000000:01:00.0".into()),
                uuid: Some("first".into()),
            },
        ];
        let automatic = ColmapAccelerationStatus {
            backend: ColmapBackend::Gpu,
            detection_state: GpuDetectionState::Ready,
            reason_code: AccelerationReasonCode::GpuReady,
            reason: String::new(),
            device: Some(device(1)),
            requirements: super::super::AccelerationRequirements {
                minimum_driver_version: "1".into(),
                minimum_compute_capability: "1".into(),
            },
            detected_nvidia_device_count: 2,
        };
        let inventory =
            build_inventory(adapters, vec![device(0), device(1)], identities, &automatic);
        assert_eq!(inventory.devices[0].colmap_gpu_index, Some(1));
        assert_eq!(inventory.devices[1].colmap_gpu_index, Some(0));
        assert!(inventory.devices[0].recommended);
        assert!(inventory
            .devices
            .iter()
            .all(|device| device.selection_confidence == SelectionConfidence::PciIdentity));
    }
}
