use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{process::Stdio, time::Duration};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SystemInfo {
    pub name: String,
    pub version: Option<String>,
    pub arch: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GpuInfo {
    pub name: String,
    pub driver_version: Option<String>,
    pub total_memory_mb: Option<u64>,
    pub compute_capability: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Environment {
    pub system: SystemInfo,
    pub gpus: Vec<GpuInfo>,
    pub actual_device: Option<String>,
}

async fn command(program: &str, args: &[&str]) -> Option<String> {
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    let result = tokio::time::timeout(Duration::from_secs(3), command.output())
        .await
        .ok()?
        .ok()?;
    (result.status.success() && result.stdout.len() <= 256 * 1024)
        .then(|| String::from_utf8_lossy(&result.stdout).into_owned())
}

fn string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)?
        .as_str()
        .filter(|v| !v.is_empty())
        .map(|v| v.chars().take(256).collect())
}

pub(super) fn windows_info(value: &Value, env: &mut Environment) {
    if let Some(name) = string(value, "osName") {
        env.system.name = name;
    }
    env.system.version = string(value, "osVersion");
    let gpu = value.get("gpus").unwrap_or(&Value::Null);
    let entries = gpu.as_array().cloned().unwrap_or_else(|| vec![gpu.clone()]);
    for entry in entries {
        if let Some(name) = string(&entry, "Name") {
            env.gpus.push(GpuInfo {
                name,
                driver_version: string(&entry, "DriverVersion"),
                total_memory_mb: None,
                compute_capability: None,
            });
        }
    }
    // Win32_VideoController.AdapterRAM is 32-bit and unreliable above 4 GiB; omit it.
}

pub(super) fn macos_info(value: &Value, env: &mut Environment) {
    if let Some(entries) = value.get("SPDisplaysDataType").and_then(Value::as_array) {
        for entry in entries {
            if let Some(name) = string(entry, "sppci_model") {
                let memory = string(entry, "spdisplays_vram")
                    .or_else(|| string(entry, "spdisplays_vram_shared"));
                let total_memory_mb = memory.and_then(|value| {
                    let number = value.split_whitespace().next()?.parse::<u64>().ok()?;
                    if value.contains("GB") {
                        number.checked_mul(1024)
                    } else if value.contains("MB") {
                        Some(number)
                    } else {
                        None
                    }
                });
                env.gpus.push(GpuInfo {
                    name,
                    driver_version: string(entry, "spdisplays_driver-version"),
                    total_memory_mb,
                    compute_capability: None,
                });
            }
        }
    }
}

pub(super) fn linux_gpus(text: &str, env: &mut Environment) {
    for line in text.lines().filter(|line| {
        [
            "VGA compatible controller:",
            "3D controller:",
            "Display controller:",
        ]
        .iter()
        .any(|kind| line.contains(kind))
    }) {
        if let Some((_, name)) = line.split_once(": ") {
            env.gpus.push(GpuInfo {
                name: name.chars().take(256).collect(),
                driver_version: None,
                total_memory_mb: None,
                compute_capability: None,
            });
        }
    }
}

pub async fn collect() -> Environment {
    let mut env = Environment {
        system: SystemInfo {
            name: std::env::consts::OS.into(),
            version: None,
            arch: std::env::consts::ARCH.into(),
        },
        gpus: vec![],
        actual_device: None,
    };
    let nvidia = crate::engines::health::diagnostic_gpu_devices();
    let native = async {
        match std::env::consts::OS {
            "windows" => command("powershell.exe", &["-NoProfile", "-NonInteractive", "-Command", "[Console]::OutputEncoding=[System.Text.UTF8Encoding]::new($false); $o=Get-CimInstance Win32_OperatingSystem; @{osName=$o.Caption;osVersion=$o.Version;gpus=@(Get-CimInstance Win32_VideoController | Select-Object Name,DriverVersion)} | ConvertTo-Json -Depth 3 -Compress"]).await,
            "macos" => command("system_profiler", &["SPDisplaysDataType", "-json", "-detailLevel", "mini"]).await,
            "linux" => command("lspci", &["-k"]).await,
            _ => None,
        }
    };
    let (nvidia, native) = tokio::join!(nvidia, native);
    if let Some(native) = native {
        match std::env::consts::OS {
            "windows" => {
                if let Ok(json) = serde_json::from_str(&native) {
                    windows_info(&json, &mut env);
                }
            }
            "macos" => {
                if let Ok(json) = serde_json::from_str(&native) {
                    macos_info(&json, &mut env);
                }
            }
            "linux" => linux_gpus(&native, &mut env),
            _ => {}
        }
    }
    if cfg!(target_os = "macos") {
        env.system.version = command("sw_vers", &["-productVersion"])
            .await
            .map(|v| v.trim().into());
    }
    if cfg!(target_os = "linux") {
        if let Ok(release) = tokio::fs::read_to_string("/etc/os-release").await {
            for line in release.lines() {
                if let Some(name) = line.strip_prefix("NAME=") {
                    env.system.name = name.trim_matches('"').chars().take(256).collect();
                }
                if let Some(version) = line.strip_prefix("VERSION_ID=") {
                    env.system.version =
                        Some(version.trim_matches('"').chars().take(256).collect());
                }
            }
        }
    }
    for gpu in nvidia {
        let entry = GpuInfo {
            name: gpu.name,
            driver_version: Some(gpu.driver_version),
            total_memory_mb: gpu.total_memory_mb,
            compute_capability: Some(gpu.compute_capability),
        };
        if let Some(existing) = env
            .gpus
            .iter_mut()
            .find(|value| value.name.eq_ignore_ascii_case(&entry.name))
        {
            *existing = entry;
        } else {
            env.gpus.push(entry);
        }
    }
    env.gpus.truncate(16);
    env
}

#[cfg(test)]
mod tests {
    use super::*;
    fn empty() -> Environment {
        Environment {
            system: SystemInfo {
                name: "unknown".into(),
                version: None,
                arch: "x86_64".into(),
            },
            gpus: vec![],
            actual_device: None,
        }
    }
    #[test]
    fn parses_only_approved_windows_fields_and_multiple_gpus() {
        let mut env = empty();
        windows_info(
            &serde_json::json!({"osName":"Windows 11","osVersion":"10.0.26100","User":"private","gpus":[{"Name":"Intel UHD","DriverVersion":"1","Serial":"private"},{"Name":"NVIDIA RTX","AdapterRAM":4294967295_u64}]}),
            &mut env,
        );
        assert_eq!(env.gpus.len(), 2);
        assert_eq!(env.gpus[1].total_memory_mb, None);
        assert!(!serde_json::to_string(&env).unwrap().contains("private"));
    }
    #[test]
    fn parses_apple_and_linux_without_serials_or_unknown_memory_guesses() {
        let mut env = empty();
        macos_info(
            &serde_json::json!({"SPDisplaysDataType":[{"sppci_model":"Apple M4","serial":"private","spdisplays_vram_shared":"Dynamic"}]}),
            &mut env,
        );
        linux_gpus("01:00.0 VGA compatible controller: AMD Radeon\n02:00.0 3D controller: Intel GPU\nNetwork controller: private", &mut env);
        assert_eq!(env.gpus.len(), 3);
        assert!(env.gpus.iter().all(|gpu| gpu.total_memory_mb.is_none()));
        assert!(!serde_json::to_string(&env).unwrap().contains("private"));
    }
}
