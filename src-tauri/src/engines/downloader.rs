use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::Emitter;

use crate::{
    engines::{EngineKind, EnginePaths, EngineStatus},
    error::{Result, SplatError},
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineDownloadProgress {
    pub engine: EngineKind,
    pub phase: String, // "downloading" | "extracting" | "ready" | "failed"
    pub percent: f32,
    pub message: String,
}

struct EngineDownloadSpec {
    url: String,
    archive_name: String,
    binary_name: String,
    destination_subdir: String,
}

fn get_spec_for_engine(kind: EngineKind) -> Option<EngineDownloadSpec> {
    #[cfg(target_os = "windows")]
    {
        match kind {
            EngineKind::Brush => Some(EngineDownloadSpec {
                url: "https://github.com/ArthurBrussee/brush/releases/download/v0.3.0/brush-app-x86_64-pc-windows-msvc.zip".into(),
                archive_name: "brush-app-x86_64-pc-windows-msvc.zip".into(),
                binary_name: "brush_app.exe".into(),
                destination_subdir: "brush".into(),
            }),
            EngineKind::Ffmpeg | EngineKind::Ffprobe => Some(EngineDownloadSpec {
                url: "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-n8.1-latest-win64-lgpl-shared-8.1.zip".into(),
                archive_name: "ffmpeg-n8.1-latest-win64-lgpl-shared-8.1.zip".into(),
                binary_name: if kind == EngineKind::Ffmpeg { "ffmpeg.exe".into() } else { "ffprobe.exe".into() },
                destination_subdir: "ffmpeg".into(),
            }),
            EngineKind::Colmap => Some(EngineDownloadSpec {
                url: "https://github.com/colmap/colmap/releases/download/4.0.4/colmap-x64-windows-cuda.zip".into(),
                archive_name: "colmap-x64-windows-cuda.zip".into(),
                binary_name: "colmap.exe".into(),
                destination_subdir: "colmap/bin".into(),
            }),
        }
    }

    #[cfg(target_os = "macos")]
    {
        match kind {
            EngineKind::Brush => {
                #[cfg(target_arch = "aarch64")]
                let (url, archive) = (
                    "https://github.com/ArthurBrussee/brush/releases/download/v0.3.0/brush-app-aarch64-apple-darwin.tar.xz",
                    "brush-app-aarch64-apple-darwin.tar.xz",
                );
                #[cfg(not(target_arch = "aarch64"))]
                let (url, archive) = (
                    "https://github.com/ArthurBrussee/brush/releases/download/v0.3.0/brush-app-x86_64-apple-darwin.tar.xz",
                    "brush-app-x86_64-apple-darwin.tar.xz",
                );
                Some(EngineDownloadSpec {
                    url: url.into(),
                    archive_name: archive.into(),
                    binary_name: "brush_app".into(),
                    destination_subdir: "brush".into(),
                })
            }
            EngineKind::Ffmpeg | EngineKind::Ffprobe => {
                #[cfg(target_arch = "aarch64")]
                let (url, archive) = (
                    "https://github.com/eugeneware/ffmpeg-static/releases/download/b7.1/ffmpeg-darwin-arm64",
                    "ffmpeg-darwin-arm64",
                );
                #[cfg(not(target_arch = "aarch64"))]
                let (url, archive) = (
                    "https://github.com/eugeneware/ffmpeg-static/releases/download/b7.1/ffmpeg-darwin-x64",
                    "ffmpeg-darwin-x64",
                );
                Some(EngineDownloadSpec {
                    url: url.into(),
                    archive_name: archive.into(),
                    binary_name: if kind == EngineKind::Ffmpeg { "ffmpeg".into() } else { "ffprobe".into() },
                    destination_subdir: "ffmpeg".into(),
                })
            }
            EngineKind::Colmap => Some(EngineDownloadSpec {
                url: "https://github.com/colmap/colmap/releases/download/3.11.1/colmap-arm64.zip".into(),
                archive_name: "colmap-arm64.zip".into(),
                binary_name: "colmap".into(),
                destination_subdir: "colmap/bin".into(),
            }),
        }
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        None
    }
}

pub async fn download_and_install_missing(
    app: &tauri::AppHandle,
    paths: &EnginePaths,
) -> Result<Vec<EngineStatus>> {
    let statuses = paths.check_all().await;
    let missing = statuses
        .iter()
        .filter(|status| !status.can_start)
        .map(|status| status.kind)
        .collect::<Vec<_>>();

    if missing.is_empty() {
        return Ok(statuses);
    }

    let cache_dir = paths.root.join(".cache").join("engines");
    tokio::fs::create_dir_all(&cache_dir).await?;

    for kind in missing {
        let Some(spec) = get_spec_for_engine(kind) else {
            continue;
        };

        // 1. Emit progress: Starting
        let _ = app.emit(
            "engine-download-progress",
            EngineDownloadProgress {
                engine: kind,
                phase: "downloading".into(),
                percent: 0.0,
                message: format!("正在下载 {:?} 引擎...", kind),
            },
        );

        let archive_path = cache_dir.join(&spec.archive_name);
        download_file(app, &spec.url, &archive_path, kind).await?;

        // 2. Emit progress: Extracting
        let _ = app.emit(
            "engine-download-progress",
            EngineDownloadProgress {
                engine: kind,
                phase: "extracting".into(),
                percent: 85.0,
                message: format!("正在解压并配置 {:?} 引擎...", kind),
            },
        );

        let dest_dir = paths.root.join(&spec.destination_subdir);
        tokio::fs::create_dir_all(&dest_dir).await?;
        extract_archive(&archive_path, &dest_dir, &spec.binary_name).await?;

        // 3. Emit progress: Ready
        let _ = app.emit(
            "engine-download-progress",
            EngineDownloadProgress {
                engine: kind,
                phase: "ready".into(),
                percent: 100.0,
                message: format!("{:?} 引擎已就绪", kind),
            },
        );
    }

    // Return updated engine statuses
    Ok(paths.check_all().await)
}

async fn download_file(
    app: &tauri::AppHandle,
    url: &str,
    destination: &Path,
    kind: EngineKind,
) -> Result<()> {
    if destination.exists() && destination.metadata()?.len() > 1024 {
        let _ = app.emit(
            "engine-download-progress",
            EngineDownloadProgress {
                engine: kind,
                phase: "downloading".into(),
                percent: 75.0,
                message: format!("已使用缓存文件: {}", destination.display()),
            },
        );
        return Ok(());
    }

    let temp_dest = destination.with_extension("tmp");
    if temp_dest.exists() {
        let _ = tokio::fs::remove_file(&temp_dest).await;
    }

    #[cfg(windows)]
    let curl_cmd = "curl.exe";
    #[cfg(not(windows))]
    let curl_cmd = "curl";

    let mut command = tokio::process::Command::new(curl_cmd);
    command
        .arg("-L")
        .arg("--fail")
        .arg("--silent")
        .arg("--show-error")
        .arg("-H")
        .arg("User-Agent: Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)")
        .arg("-o")
        .arg(&temp_dest)
        .arg(url);

    let status = command
        .status()
        .await
        .map_err(|e| SplatError::Process(format!("下载引擎失败 (无法启动 curl)：{e}")))?;

    if !status.success() {
        return Err(SplatError::Process(format!(
            "下载引擎失败：curl 退出码 {:?}",
            status.code()
        )));
    }

    tokio::fs::rename(temp_dest, destination).await?;
    Ok(())
}

async fn extract_archive(
    archive: &Path,
    destination: &Path,
    binary_name: &str,
) -> Result<()> {
    let archive_str = archive.to_string_lossy();
    let is_tar = archive_str.ends_with(".tar.xz")
        || archive_str.ends_with(".txz")
        || archive_str.ends_with(".tar.gz")
        || archive_str.ends_with(".tgz")
        || archive_str.ends_with(".tar");
    let is_zip = archive_str.ends_with(".zip");

    if is_tar {
        #[cfg(windows)]
        let tar_cmd = "tar.exe";
        #[cfg(not(windows))]
        let tar_cmd = "tar";

        let mut command = tokio::process::Command::new(tar_cmd);
        command
            .arg("-xf")
            .arg(archive)
            .arg("-C")
            .arg(destination);
        let status = command.status().await.map_err(|e| {
            SplatError::Process(format!("解压失败 (无法启动 tar)：{e}"))
        })?;
        if !status.success() {
            return Err(SplatError::Process(format!(
                "解压失败：tar 退出码 {:?}",
                status.code()
            )));
        }
    } else if is_zip {
        #[cfg(windows)]
        {
            let mut command = tokio::process::Command::new("tar.exe");
            command
                .arg("-xf")
                .arg(archive)
                .arg("-C")
                .arg(destination);
            let status = command.status().await?;
            if !status.success() {
                // Fallback to powershell Expand-Archive
                let mut ps = tokio::process::Command::new("powershell");
                ps.arg("-NoProfile")
                    .arg("-Command")
                    .arg(format!(
                        "Expand-Archive -LiteralPath '{}' -DestinationPath '{}' -Force",
                        archive.display(),
                        destination.display()
                    ));
                ps.status().await?;
            }
        }

        #[cfg(not(windows))]
        {
            let mut command = tokio::process::Command::new("unzip");
            command
                .arg("-o")
                .arg(archive)
                .arg("-d")
                .arg(destination);
            let status = command.status().await.map_err(|e| {
                SplatError::Process(format!("解压失败 (无法启动 unzip)：{e}"))
            })?;
            if !status.success() {
                return Err(SplatError::Process(format!(
                    "解压失败：unzip 退出码 {:?}",
                    status.code()
                )));
            }
        }
    } else {
        // Direct single binary download
        let target = destination.join(binary_name);
        tokio::fs::copy(archive, &target).await?;
    }

    // Locate the target executable in destination or its subdirectories
    let target_bin = find_binary_recursive(destination, binary_name)
        .unwrap_or_else(|| destination.join(binary_name));

    // Ensure permissions on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if target_bin.is_file() {
            let _ = std::fs::set_permissions(&target_bin, std::fs::Permissions::from_mode(0o755));
        }
        // If binary is in a subdirectory, copy or link it to top destination if needed
        let direct_target = destination.join(binary_name);
        if target_bin.is_file() && target_bin != direct_target {
            let _ = std::fs::copy(&target_bin, &direct_target);
            let _ = std::fs::set_permissions(&direct_target, std::fs::Permissions::from_mode(0o755));
        }
    }

    Ok(())
}

fn find_binary_recursive(dir: &Path, name: &str) -> Option<PathBuf> {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                if let Some(file_name) = path.file_name() {
                    if file_name.to_string_lossy().eq_ignore_ascii_case(name) {
                        return Some(path);
                    }
                }
            } else if path.is_dir() {
                if let Some(found) = find_binary_recursive(&path, name) {
                    return Some(found);
                }
            }
        }
    }
    None
}
