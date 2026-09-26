// Standalone std-only mirror of OOOSplat ZLUDA GPU-mode logic (for machines without MSVC).
// Compile: rustc --test -o zluda_logic_test.exe zluda_logic_test.rs && .\zluda_logic_test.exe
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ColmapGpuMode {
    Auto,
    Zluda,
    Directml,
    Force,
    Off,
}

fn parse_colmap_gpu_mode(raw: &str) -> ColmapGpuMode {
    match raw.trim().to_ascii_lowercase().as_str() {
        "zluda" => ColmapGpuMode::Zluda,
        "directml" => ColmapGpuMode::Directml,
        "force" => ColmapGpuMode::Force,
        "off" => ColmapGpuMode::Off,
        _ => ColmapGpuMode::Auto,
    }
}

fn zluda_staged_near_colmap(colmap_path: &Path) -> bool {
    let Some(dir) = colmap_path.parent() else {
        return false;
    };
    let runtime_dlls = [
        dir.join("nvcuda.dll"),
        dir.join("nvcuda64.dll"),
        dir.parent()
            .map(|parent| parent.join("nvcuda.dll"))
            .unwrap_or_default(),
    ];
    let stage_markers = [
        dir.join("zluda-stage.txt"),
        dir.join(".zluda-stage"),
        dir.join("zluda_staged.txt"),
        dir.join("gpu-report.json"),
        dir.join("runtime-test.json"),
    ];
    runtime_dlls.iter().any(|path| path.is_file())
        || stage_markers.iter().any(|path| path.is_file())
}

fn directml_staged_near_colmap(colmap_path: &Path) -> bool {
    let Some(dir) = colmap_path.parent() else {
        return false;
    };
    let provider_dlls = [
        dir.join("DirectML.dll"),
        dir.join("onnxruntime_providers_directml.dll"),
        dir.join("onnxruntime_providers_dml.dll"),
        dir.parent()
            .map(|parent| parent.join("DirectML.dll"))
            .unwrap_or_default(),
    ];
    let stage_markers = [dir.join("directml-stage.txt"), dir.join(".directml-stage")];
    provider_dlls.iter().any(|path| path.is_file())
        || stage_markers.iter().any(|path| path.is_file())
}

fn resolve_mode_and_stage(
    mode: ColmapGpuMode,
    zluda_staged: bool,
    directml_staged: bool,
) -> (&'static str, bool) {
    // returns (reason_code, use_gpu)
    match mode {
        ColmapGpuMode::Off => ("colmapGpuDisabled", false),
        ColmapGpuMode::Force => ("zludaReady", true),
        ColmapGpuMode::Zluda => {
            if zluda_staged {
                ("zludaReady", true)
            } else {
                ("zludaNotStaged", false)
            }
        }
        ColmapGpuMode::Directml => {
            if directml_staged {
                ("directmlReady", true)
            } else {
                ("directmlNotStaged", false)
            }
        }
        ColmapGpuMode::Auto => {
            if directml_staged {
                ("directmlReady", true)
            } else if zluda_staged {
                ("zludaReady", true)
            } else {
                ("nvidiaOrCpu", false) // placeholder; NVIDIA path is separate
            }
        }
    }
}

fn gpu_index_for(reason_code: &str, use_gpu: bool, device_index: Option<u32>) -> Option<u32> {
    if reason_code == "zludaReady" || reason_code == "directmlReady" {
        return None;
    }
    if use_gpu {
        device_index
    } else {
        None
    }
}

fn colmap_use_gpu_arg(use_gpu: bool, gpu_index: Option<u32>) -> (String, Option<(String, String)>) {
    let use_flag = if use_gpu { "1" } else { "0" }.to_string();
    match gpu_index {
        Some(index) => (use_flag, Some(("gpu_index".into(), index.to_string()))),
        None => (use_flag, None),
    }
}

fn temp_dir() -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("zluda_logic_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn parses_colmap_gpu_mode_env_values() {
    assert_eq!(parse_colmap_gpu_mode("zluda"), ColmapGpuMode::Zluda);
    assert_eq!(parse_colmap_gpu_mode("directml"), ColmapGpuMode::Directml);
    assert_eq!(parse_colmap_gpu_mode("FORCE"), ColmapGpuMode::Force);
    assert_eq!(parse_colmap_gpu_mode(" Off "), ColmapGpuMode::Off);
    assert_eq!(parse_colmap_gpu_mode(""), ColmapGpuMode::Auto);
    assert_eq!(parse_colmap_gpu_mode("auto"), ColmapGpuMode::Auto);
    assert_eq!(parse_colmap_gpu_mode("nvidia"), ColmapGpuMode::Auto);
}

#[test]
fn detects_zluda_stage_markers_next_to_colmap() {
    let dir = temp_dir();
    let colmap = dir.join("colmap.exe");
    std::fs::write(&colmap, b"").unwrap();
    assert!(!zluda_staged_near_colmap(&colmap));
    std::fs::write(dir.join("nvcuda.dll"), b"").unwrap();
    assert!(zluda_staged_near_colmap(&colmap));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn detects_directml_provider_next_to_colmap() {
    let dir = temp_dir();
    let colmap = dir.join("colmap.exe");
    std::fs::write(&colmap, b"").unwrap();
    assert!(!directml_staged_near_colmap(&colmap));
    std::fs::write(dir.join("DirectML.dll"), b"").unwrap();
    assert!(directml_staged_near_colmap(&colmap));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn force_and_off_and_zluda_gate() {
    assert_eq!(
        resolve_mode_and_stage(ColmapGpuMode::Off, false, false),
        ("colmapGpuDisabled", false)
    );
    assert_eq!(
        resolve_mode_and_stage(ColmapGpuMode::Force, false, false),
        ("zludaReady", true)
    );
    assert_eq!(
        resolve_mode_and_stage(ColmapGpuMode::Zluda, false, false),
        ("zludaNotStaged", false)
    );
    assert_eq!(
        resolve_mode_and_stage(ColmapGpuMode::Zluda, true, false),
        ("zludaReady", true)
    );
    assert_eq!(
        resolve_mode_and_stage(ColmapGpuMode::Directml, false, false),
        ("directmlNotStaged", false)
    );
    assert_eq!(
        resolve_mode_and_stage(ColmapGpuMode::Directml, false, true),
        ("directmlReady", true)
    );
    assert_eq!(
        resolve_mode_and_stage(ColmapGpuMode::Auto, true, false),
        ("zludaReady", true)
    );
    assert_eq!(
        resolve_mode_and_stage(ColmapGpuMode::Auto, true, true),
        ("directmlReady", true)
    );
}

#[test]
fn zluda_ready_does_not_pin_gpu_index() {
    assert_eq!(gpu_index_for("zludaReady", true, Some(0)), None);
    assert_eq!(gpu_index_for("directmlReady", true, Some(0)), None);
    assert_eq!(gpu_index_for("gpuReady", true, Some(2)), Some(2));
    assert_eq!(gpu_index_for("gpuReady", false, Some(2)), None);
}

#[test]
fn colmap_args_split_use_gpu_and_index() {
    // ZLUDA: use_gpu=1, no index
    let (flag, index) = colmap_use_gpu_arg(true, None);
    assert_eq!(flag, "1");
    assert!(index.is_none());

    // NVIDIA: use_gpu=1 + index
    let (flag, index) = colmap_use_gpu_arg(true, Some(1));
    assert_eq!(flag, "1");
    assert_eq!(index, Some(("gpu_index".into(), "1".into())));

    // CPU: use_gpu=0, no index
    let (flag, index) = colmap_use_gpu_arg(false, None);
    assert_eq!(flag, "0");
    assert!(index.is_none());
}
