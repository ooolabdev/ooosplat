use std::{ffi::OsString, path::Path};

use crate::{
    error::{Result, SplatError},
    process::{ProcessManager, ProcessSpec},
    video::{parse_ffprobe_json, VideoInfo},
};

pub async fn probe_video(
    executable: &Path,
    input: &Path,
    log_path: Option<std::path::PathBuf>,
) -> Result<VideoInfo> {
    if !input.is_file() {
        return Err(SplatError::InvalidPath(input.to_path_buf()));
    }
    let output = ProcessManager::new().run(ProcessSpec {
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
    if !output.success {
        return Err(SplatError::InvalidVideo("FFprobe 无法解码这个文件".into()));
    }
    parse_ffprobe_json(&output.stdout)
}
