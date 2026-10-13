use std::{ffi::OsString, path::Path};

use crate::{
    error::{Result, SplatError},
    process::{ProcessManager, ProcessOutput, ProcessSpec},
    video::{parse_ffprobe_json, VideoInfo},
};

/// Preserve pre-project failures, including errors that occur before process startup.
pub async fn probe_video_with_diagnostics(
    executable: &Path,
    input: &Path,
    log_path: &Path,
    process_manager: &ProcessManager,
) -> Result<VideoInfo> {
    match probe_video(executable, input, None, process_manager).await {
        Ok(video) => Ok(video),
        Err(error) => {
            let diagnostic = format!(
                "executable: {}\ninput: {}\n{error}\n",
                executable.display(),
                input.display(),
            );
            let saved = async {
                if let Some(parent) = log_path.parent() {
                    tokio::fs::create_dir_all(parent).await?;
                }
                tokio::fs::write(log_path, diagnostic).await
            }
            .await;
            let detail = match saved {
                Ok(()) => format!("Diagnostic log: {}", log_path.display()),
                Err(log_error) => format!("Could not save diagnostic log: {log_error}"),
            };
            Err(SplatError::Diagnostic {
                source: Box::new(error),
                detail,
            })
        }
    }
}

pub async fn probe_video(
    executable: &Path,
    input: &Path,
    log_path: Option<std::path::PathBuf>,
    process_manager: &ProcessManager,
) -> Result<VideoInfo> {
    if !input.is_file() {
        return Err(SplatError::InvalidPath(input.to_path_buf()));
    }
    let output = process_manager.run(ProcessSpec {
        executable: executable.to_path_buf(),
        args: vec![
            OsString::from("-v"), OsString::from("error"),
            OsString::from("-select_streams"), OsString::from("v:0"),
            OsString::from("-show_entries"),
            OsString::from("stream=width,height,avg_frame_rate,r_frame_rate,nb_frames,codec_name,pix_fmt:stream_tags=rotate:stream_side_data=rotation:format=duration"),
            OsString::from("-of"), OsString::from("json"),
            input.as_os_str().to_owned(),
        ],
        working_directory: input.parent().map(Path::to_path_buf),
        log_path,
        observer: None,
    }).await?;
    parse_probe_output(executable, &output)
}

fn parse_probe_output(executable: &Path, output: &ProcessOutput) -> Result<VideoInfo> {
    if !output.success {
        #[cfg(windows)]
        if output.exit_code == Some(0xC000_0135_u32 as i32) {
            // STATUS_DLL_NOT_FOUND is a Windows loader failure, not a media error.
            return Err(SplatError::EngineStart {
                engine: executable.display().to_string(),
                detail: format!(
                    "缺少运行所需的 DLL（退出码 0xC0000135）\n{}",
                    output.failure_detail()
                ),
            });
        }
        // A nonzero exit can also mean a broken runtime or inaccessible input.
        // Keep the diagnostics without claiming that the video is corrupt.
        return Err(SplatError::Process(format!(
            "FFprobe 视频分析失败，退出码 {}，引擎 {}\n{}",
            output
                .exit_code
                .map_or_else(|| "N/A".into(), |code| code.to_string()),
            executable.display(),
            output.failure_detail(),
        )));
    }
    parse_ffprobe_json(&output.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn preflight_logs_startup_failure_before_a_project_exists() {
        let directory = tempfile::tempdir().unwrap();
        let input = directory.path().join("input.mp4");
        tokio::fs::write(&input, b"input").await.unwrap();
        let executable = directory.path().join("missing-ffprobe.exe");
        let log = directory.path().join("logs/probe.log");
        let error = probe_video_with_diagnostics(&executable, &input, &log, &ProcessManager::new())
            .await
            .unwrap_err();
        let diagnostic = tokio::fs::read_to_string(&log).await.unwrap();
        assert!(diagnostic.contains(&executable.display().to_string()));
        assert!(diagnostic.contains("找不到本地处理引擎"));
        assert!(error.to_string().contains(&log.display().to_string()));
        let second_executable = directory.path().join("another-missing-ffprobe.exe");
        probe_video_with_diagnostics(&second_executable, &input, &log, &ProcessManager::new())
            .await
            .unwrap_err();
        let latest = tokio::fs::read_to_string(&log).await.unwrap();
        assert!(latest.contains(&second_executable.display().to_string()));
        assert!(!latest.contains(&executable.display().to_string()));
    }

    #[tokio::test]
    async fn log_write_failure_preserves_the_original_error() {
        let directory = tempfile::tempdir().unwrap();
        let input = directory.path().join("missing.mp4");
        let error = probe_video_with_diagnostics(
            Path::new("ffprobe"),
            &input,
            directory.path(),
            &ProcessManager::new(),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("Could not save diagnostic log"));
        assert!(matches!(error, SplatError::Diagnostic { source, .. }
            if matches!(*source, SplatError::InvalidPath(_))));
    }

    #[test]
    fn failed_probe_retains_diagnostics_without_blaming_the_video() {
        let executable = Path::new("engines/ffmpeg/ffprobe.exe");
        for (exit_code, stderr) in [
            (Some(1), "moov atom not found"),
            (None, "process terminated"),
        ] {
            let output = ProcessOutput {
                success: false,
                exit_code,
                stdout: String::new(),
                stderr: stderr.into(),
            };
            let error = parse_probe_output(executable, &output).unwrap_err();
            assert!(matches!(error, SplatError::Process(_)), "{error}");
            let message = error.to_string();
            assert!(message.contains(&executable.display().to_string()));
            assert!(
                message.contains(&exit_code.map_or_else(|| "N/A".into(), |code| code.to_string()))
            );
            assert!(message.contains(stderr));
            assert!(!message.contains("视频无效"));
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_missing_dll_is_classified_as_an_engine_failure() {
        let output = ProcessOutput {
            success: false,
            exit_code: Some(0xC000_0135_u32 as i32),
            stdout: String::new(),
            stderr: String::new(),
        };
        let error = parse_probe_output(Path::new("ffprobe.exe"), &output).unwrap_err();
        assert!(matches!(error, SplatError::EngineStart { .. }));
        assert!(error.to_string().contains("0xC0000135"));
    }

    #[test]
    fn successful_probe_still_validates_video_streams() {
        let output = ProcessOutput {
            success: true,
            exit_code: Some(0),
            stdout: r#"{"streams": [], "format": {"duration": "10"}}"#.into(),
            stderr: String::new(),
        };
        let error = parse_probe_output(Path::new("ffprobe"), &output).unwrap_err();
        assert!(matches!(error, SplatError::InvalidVideo(_)));
        assert!(error.to_string().contains("文件中没有视频轨道"));
    }
}
