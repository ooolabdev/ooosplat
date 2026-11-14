use std::{ffi::OsString, path::Path};

use crate::{
    error::{Result, SplatError},
    process::{ProcessManager, ProcessOutput, ProcessSpec},
    video::{parse_ffprobe_json, VideoInfo},
};

// Structured metadata must be captured whole, never as a diagnostic tail.
// Keep a separate, bounded allowance for large stream/container tags (including
// projection metadata); stderr remains a small diagnostic tail.
pub(crate) const MAX_PROBE_JSON_BYTES: usize = 8 * 1024 * 1024;

/// Attach best-effort diagnostics to a pre-project probe without hiding its cause.
pub async fn with_probe_diagnostics<T>(
    result: Result<T>,
    executable: &Path,
    input: &Path,
    log_path: std::result::Result<std::path::PathBuf, String>,
) -> Result<T> {
    let error = match result {
        Ok(value) => return Ok(value),
        Err(error) => error,
    };
    let detail = match log_path {
        Ok(log_path) => {
            let diagnostic = format!(
                "executable: {}\ninput: {}\n{error}\n",
                executable.display(),
                input.display(),
            );
            let saved = async {
                if let Some(parent) = log_path.parent() {
                    tokio::fs::create_dir_all(parent).await?;
                }
                tokio::fs::write(&log_path, diagnostic).await
            }
            .await;
            match saved {
                Ok(()) => format!("Diagnostic log: {}", log_path.display()),
                Err(log_error) => format!("Could not save diagnostic log: {log_error}"),
            }
        }
        Err(log_error) => format!("Could not save diagnostic log: {log_error}"),
    };
    Err(SplatError::Diagnostic {
        source: Box::new(error),
        detail,
    })
}

pub async fn probe_video(
    executable: &Path,
    input: &Path,
    log_path: Option<std::path::PathBuf>,
    process_manager: &ProcessManager,
) -> Result<VideoInfo> {
    Ok(
        probe_video_metadata(executable, input, log_path, process_manager)
            .await?
            .0,
    )
}

pub async fn probe_video_metadata(
    executable: &Path,
    input: &Path,
    log_path: Option<std::path::PathBuf>,
    process_manager: &ProcessManager,
) -> Result<(VideoInfo, serde_json::Value)> {
    if !input.is_file() {
        return Err(SplatError::InvalidPath(input.to_path_buf()));
    }
    let output = process_manager
        .clone()
        .with_output_capture_limit(64 * 1024)
        .with_complete_stdout_limit(MAX_PROBE_JSON_BYTES)
        .run(ProcessSpec {
            executable: executable.to_path_buf(),
            args: vec![
                OsString::from("-v"),
                OsString::from("error"),
                OsString::from("-select_streams"),
                OsString::from("v:0"),
                OsString::from("-show_streams"),
                OsString::from("-show_format"),
                OsString::from("-of"),
                OsString::from("json"),
                input.as_os_str().to_owned(),
            ],
            working_directory: input.parent().map(Path::to_path_buf),
            log_path,
            observer: None,
        })
        .await?;
    Ok((
        parse_probe_output(executable, &output)?,
        serde_json::from_str(&output.stdout)
            .map_err(|error| probe_parse_error(&output, error.into()))?,
    ))
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
    parse_ffprobe_json(&output.stdout).map_err(|error| probe_parse_error(output, error))
}

fn probe_parse_error(output: &ProcessOutput, error: SplatError) -> SplatError {
    if !matches!(error, SplatError::Json(_)) {
        return error;
    }
    SplatError::Diagnostic {
        source: Box::new(error),
        // Do not include JSON contents: tags may contain paths or location data.
        detail: format!(
            "ffprobe_output=invalid_json\nstdout_bytes={}\nstdout_complete=true\n{}",
            output.stdout.len(),
            output.termination,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn successful_output(stdout: String) -> ProcessOutput {
        ProcessOutput {
            success: true,
            exit_code: Some(0),
            termination: crate::process::ProcessTermination {
                exit_code: Some(0),
                ..Default::default()
            },
            stdout,
            stderr: String::new(),
        }
    }

    #[test]
    fn large_metadata_reproduces_old_integer_error_but_complete_json_parses() {
        // The old 64 KiB tail starts with `1 ` inside an otherwise valid tag.
        // This reproduces the exact class of root-structure parse error without
        // claiming to reproduce the unavailable user's original video.
        let document = format!(
            r#"{{"streams":[{{"width":1920,"height":1080,"avg_frame_rate":"30/1","nb_frames":"300","pix_fmt":"yuv420p"}}],"format":{{"duration":"10"}},"padding":"{}1 {}"}}"#,
            "x".repeat(1024),
            "a".repeat(65532),
        );
        let tail = &document[document.len() - 64 * 1024..];
        assert!(tail.starts_with("1 "));
        let old_error = parse_ffprobe_json(tail).unwrap_err().to_string();
        assert!(old_error.contains("integer `1`"), "{old_error}");
        assert!(old_error.contains("ProbeDocument"), "{old_error}");
        let output = successful_output(document);
        let video = parse_probe_output(Path::new("ffprobe"), &output).unwrap();
        assert_eq!((video.width, video.height), (1920, 1080));
        assert_eq!(video.fps, 30.0);
        assert_eq!(video.total_frames, 300);
        let json: serde_json::Value = serde_json::from_str(&output.stdout).unwrap();
        assert_eq!(json["padding"].as_str().unwrap().len(), 1024 + 2 + 65532);
    }

    #[test]
    fn malformed_complete_output_keeps_safe_parse_context() {
        for document in [
            "1",
            "",
            "{\"streams\": [",
            "{\"private_tag\":\"not-for-logs\",]",
        ] {
            let error =
                parse_probe_output(Path::new("ffprobe"), &successful_output(document.into()))
                    .unwrap_err();
            let message = error.to_string();
            assert!(message.contains("ffprobe_output=invalid_json"));
            assert!(message.contains(&format!("stdout_bytes={}", document.len())));
            assert!(message.contains("stdout_complete=true"));
            assert!(message.contains("exit_code: Some(0)"));
            assert!(!message.contains("not-for-logs"));
            assert!(matches!(error, SplatError::Diagnostic { source, .. }
                if matches!(*source, SplatError::Json(_))));
        }
    }

    #[cfg(windows)]
    #[tokio::test]
    #[ignore = "requires local bundled FFmpeg/FFprobe; creates only isolated tiny synthetic videos"]
    async fn native_probe_large_metadata_and_limit() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let ffmpeg = repo.join("engines/ffmpeg/ffmpeg.exe");
        let ffprobe = repo.join("engines/ffmpeg/ffprobe.exe");
        assert!(ffmpeg.is_file() && ffprobe.is_file());
        let scratch = repo.join(".tmp");
        std::fs::create_dir_all(&scratch).unwrap();
        let directory = tempfile::Builder::new()
            .prefix("probe-json-regression-")
            .tempdir_in(scratch)
            .unwrap();
        // Exercise the real production process/capture/parser chain without
        // accessing user media or running SfM/training. Matroska retains both
        // stream and container tags, exercising both JSON-consuming probes.
        for (index, metadata_size) in [16, 128 * 1024, MAX_PROBE_JSON_BYTES]
            .into_iter()
            .enumerate()
        {
            let metadata = directory.path().join(format!("metadata-{index}.txt"));
            std::fs::write(
                &metadata,
                format!(";FFMETADATA1\ncomment={}\n", "x".repeat(metadata_size)),
            )
            .unwrap();
            let input = directory.path().join(format!("clip-{index}.mkv"));
            let output = ProcessManager::new()
                .run(ProcessSpec {
                    executable: ffmpeg.clone(),
                    args: vec![
                        "-hide_banner".into(),
                        "-v".into(),
                        "error".into(),
                        "-f".into(),
                        "lavfi".into(),
                        "-i".into(),
                        "color=c=black:s=64x32:r=1:d=1".into(),
                        "-f".into(),
                        "ffmetadata".into(),
                        "-i".into(),
                        metadata.into_os_string(),
                        "-map".into(),
                        "0:v:0".into(),
                        "-map_metadata".into(),
                        "1".into(),
                        "-map_metadata:s:v:0".into(),
                        "1:g".into(),
                        "-frames:v".into(),
                        "1".into(),
                        "-c:v".into(),
                        "ffv1".into(),
                        input.as_os_str().into(),
                    ],
                    working_directory: Some(directory.path().into()),
                    log_path: None,
                    observer: None,
                })
                .await
                .unwrap();
            assert!(output.success, "{}", output.failure_detail());
            let original = std::fs::read(&input).unwrap();
            let manager = ProcessManager::new();
            let result = probe_video_metadata(&ffprobe, &input, None, &manager).await;
            if metadata_size == MAX_PROBE_JSON_BYTES {
                let error = result.unwrap_err().to_string();
                assert!(error.contains("stdout_truncated=true"), "{error}");
                assert!(error.contains("limit_bytes=8388608"), "{error}");
                assert!(!error.contains("ProbeDocument"));
                assert!(!error.contains("xxxxxxxx"));
            } else {
                let (video, json) = result.unwrap();
                assert_eq!((video.width, video.height, video.fps), (64, 32, 1.0));
                let tags = json["format"]["tags"].as_object().unwrap();
                let comment = tags
                    .iter()
                    .find(|(key, _)| key.eq_ignore_ascii_case("comment"))
                    .unwrap()
                    .1;
                assert_eq!(comment.as_str().unwrap().len(), metadata_size);
                assert_eq!(video.total_frames, 1);
                let expected = crate::video::panorama::detect_metadata(&json, &video, false);
                let detection = crate::video::panorama::detect(
                    &ffprobe, &ffmpeg, &input, &video, &manager, false,
                )
                .await
                .unwrap();
                assert_eq!(detection.unsupported, expected.unsupported);
                assert_eq!(detection.supported_erp, expected.supported_erp);
                assert_eq!(detection.requires_choice, expected.requires_choice);
            }
            assert!(!manager.has_active_processes());
            assert_eq!(std::fs::read(&input).unwrap(), original);
        }
    }

    async fn probe_video_with_diagnostics(
        executable: &Path,
        input: &Path,
        log_path: &Path,
        manager: &ProcessManager,
    ) -> Result<VideoInfo> {
        let result = probe_video(executable, input, None, manager).await;
        with_probe_diagnostics(result, executable, input, Ok(log_path.to_path_buf())).await
    }

    #[tokio::test]
    async fn unavailable_log_directory_preserves_the_original_probe_error() {
        let error = with_probe_diagnostics::<VideoInfo>(
            Err(SplatError::EngineMissing("ffprobe.exe".into())),
            Path::new("ffprobe.exe"),
            Path::new("input.mp4"),
            Err("Cannot resolve diagnostic log directory".into()),
        )
        .await
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("Cannot resolve diagnostic log directory"));
        assert!(matches!(error, SplatError::Diagnostic { source, .. }
            if matches!(*source, SplatError::EngineMissing(_))));
    }

    #[tokio::test]
    async fn unavailable_log_directory_does_not_fail_a_successful_probe() {
        let value = with_probe_diagnostics(
            Ok(42),
            Path::new("ffprobe.exe"),
            Path::new("input.mp4"),
            Err("Cannot resolve diagnostic log directory".into()),
        )
        .await
        .unwrap();
        assert_eq!(value, 42);
    }

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
                termination: crate::process::ProcessTermination {
                    exit_code,
                    ..Default::default()
                },
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
            termination: crate::process::ProcessTermination {
                exit_code: Some(0xC000_0135_u32 as i32),
                windows_status: Some("0xC0000135".into()),
                ..Default::default()
            },
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
            termination: crate::process::ProcessTermination {
                exit_code: Some(0),
                ..Default::default()
            },
            stdout: r#"{"streams": [], "format": {"duration": "10"}}"#.into(),
            stderr: String::new(),
        };
        let error = parse_probe_output(Path::new("ffprobe"), &output).unwrap_err();
        assert!(matches!(error, SplatError::InvalidVideo(_)));
        assert!(error.to_string().contains("文件中没有视频轨道"));
    }
}
