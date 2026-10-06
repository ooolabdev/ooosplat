//! Bounded reads of registered engine logs. A cursor is data, never a path.
use super::{Result, ServiceError, TaskRecord};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};
use uuid::Uuid;

pub const MAX_BYTES: usize = 128 * 1024;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisteredLog {
    pub source: String,
    pub path: PathBuf,
    pub root: PathBuf,
    pub file_id: String,
    pub start: u64,
    pub end: Option<u64>,
}
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogRequest {
    pub task_id: Uuid,
    pub run_id: Option<Uuid>,
    pub sources: Option<Vec<String>>,
    pub cursor: Option<String>,
    pub tail_lines: Option<usize>,
    pub max_bytes: Option<usize>,
}
#[derive(Debug, Clone, Serialize)]
pub struct LogChunk {
    pub source: String,
    pub text: String,
    pub partial_line: bool,
}
#[derive(Debug, Clone, Serialize)]
pub struct LogPage {
    pub task_id: Uuid,
    pub run_id: Option<Uuid>,
    pub entries: Vec<LogChunk>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
    pub truncated: bool,
    pub cursor_reset: bool,
    pub reset_reason: Option<String>,
    pub available_sources: Vec<String>,
    pub content_is_untrusted: bool,
}
#[derive(Serialize, Deserialize)]
pub(super) struct Position {
    identity: String,
    offset: u64,
    pub(super) anchor: String,
}
#[derive(Serialize, Deserialize)]
pub(super) struct Cursor {
    version: u32,
    task_id: Uuid,
    run_id: Uuid,
    sources: Vec<String>,
    pub(super) files: BTreeMap<String, Position>,
}
fn file_id(file: &File) -> std::io::Result<String> {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
        };
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut info) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(format!(
            "{}:{}:{}",
            info.dwVolumeSerialNumber, info.nFileIndexHigh, info.nFileIndexLow
        ))
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let m = file.metadata()?;
        Ok(format!("{}:{}", m.dev(), m.ino()))
    }
}
fn secure_open(root: &Path, path: &Path) -> Result<File> {
    let resolved_root = std::fs::canonicalize(root)?;
    if resolved_root != root {
        return Err(ServiceError::new(
            "LOG_ACCESS_DENIED",
            "登记的日志目录身份发生变化",
        ));
    }
    let root = resolved_root;
    let file = File::open(path)?;
    let resolved = std::fs::canonicalize(path)?;
    if !resolved.starts_with(&root) || !file.metadata()?.is_file() {
        return Err(ServiceError::new(
            "LOG_ACCESS_DENIED",
            "日志文件不属于该任务",
        ));
    }
    if file_id(&file)? != file_id(&File::open(&resolved)?)? {
        return Err(ServiceError::new(
            "LOG_ACCESS_DENIED",
            "日志文件身份发生变化",
        ));
    }
    Ok(file)
}
pub fn register(root: &Path, path: &Path, start: u64) -> Result<RegisteredLog> {
    let parent = std::fs::canonicalize(
        root.parent()
            .ok_or_else(|| ServiceError::new("LOG_ACCESS_DENIED", "日志目录无效"))?,
    )?;
    let root = std::fs::canonicalize(root)?;
    if !root.starts_with(parent) {
        return Err(ServiceError::new("LOG_ACCESS_DENIED", "日志目录不属于项目"));
    }
    let path = std::fs::canonicalize(path)?;
    let file = secure_open(&root, &path)?;
    Ok(RegisteredLog {
        source: path
            .file_stem()
            .and_then(|v| v.to_str())
            .unwrap_or("system")
            .into(),
        path,
        root,
        file_id: file_id(&file)?,
        start,
        end: None,
    })
}
fn encode(cursor: &Cursor) -> Result<String> {
    Ok(serde_json::to_vec(cursor)?
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}
pub(super) fn decode(value: &str) -> Result<Cursor> {
    if value.len() > 16 * 1024 || value.len() % 2 != 0 {
        return Err(ServiceError::new("INVALID_CURSOR", "日志 cursor 无效"));
    }
    let bytes = (0..value.len())
        .step_by(2)
        .map(|i| {
            value
                .get(i..i + 2)
                .and_then(|s| u8::from_str_radix(s, 16).ok())
                .ok_or_else(|| ServiceError::new("INVALID_CURSOR", "日志 cursor 无效"))
        })
        .collect::<Result<Vec<_>>>()?;
    serde_json::from_slice(&bytes)
        .map_err(|_| ServiceError::new("INVALID_CURSOR", "日志 cursor 无效"))
}
fn anchor(file: &mut File, offset: u64) -> std::io::Result<String> {
    let n = offset.min(32) as usize;
    file.seek(SeekFrom::Start(offset - n as u64))?;
    let mut bytes = vec![0; n];
    file.read_exact(&mut bytes)?;
    use sha2::Digest;
    Ok(sha2::Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}
pub fn read(task: &TaskRecord, request: &LogRequest) -> Result<LogPage> {
    if task.task_id != request.task_id {
        return Err(ServiceError::new("LOG_ACCESS_DENIED", "日志不属于该任务"));
    }
    let bytes = request.max_bytes.unwrap_or(32 * 1024);
    let lines = request.tail_lines.unwrap_or(100);
    if bytes == 0 || bytes > MAX_BYTES || lines == 0 || lines > 500 {
        return Err(ServiceError::new(
            "INVALID_ARGUMENT",
            "max_bytes 必须为 1–131072；tail_lines 必须为 1–500",
        ));
    }
    let run_id = request.run_id.or(task.run_id);
    let run = run_id.and_then(|id| task.runs.iter().find(|r| r.run_id == id));
    if run_id.is_some() && run.is_none() {
        return Err(ServiceError::new("RUN_NOT_FOUND", "任务不存在该执行实例"));
    }
    let mut page = LogPage {
        task_id: task.task_id,
        run_id,
        entries: vec![],
        next_cursor: None,
        has_more: false,
        truncated: false,
        cursor_reset: false,
        reset_reason: None,
        available_sources: run
            .map(|r| r.logs.keys().cloned().collect())
            .unwrap_or_default(),
        content_is_untrusted: true,
    };
    let Some(run) = run else { return Ok(page) };
    let mut sources = request
        .sources
        .clone()
        .unwrap_or_else(|| page.available_sources.clone());
    sources.sort();
    sources.dedup();
    if sources.len() > 32 || sources.iter().any(|s| !run.logs.contains_key(s)) {
        return Err(ServiceError::new(
            "INVALID_LOG_SOURCE",
            "只能读取该执行已登记的日志来源",
        ));
    }
    let mut cursor = if let Some(value) = &request.cursor {
        let cursor = decode(value)?;
        if cursor.version != 1
            || cursor.task_id != task.task_id
            || cursor.run_id != run.run_id
            || cursor.sources != sources
        {
            return Err(ServiceError::new(
                "CURSOR_MISMATCH",
                "cursor 不属于该任务、执行或来源筛选",
            ));
        }
        cursor
    } else {
        Cursor {
            version: 1,
            task_id: task.task_id,
            run_id: run.run_id,
            sources: sources.clone(),
            files: Default::default(),
        }
    };
    let mut remaining = bytes;
    for source in &sources {
        let log = &run.logs[source];
        let mut file = match secure_open(&log.root, &log.path) {
            Ok(f) => f,
            Err(_e) if !log.path.exists() => {
                page.cursor_reset = true;
                page.reset_reason = Some("log_missing".into());
                continue;
            }
            Err(e) => return Err(e),
        };
        let identity = file_id(&file)?;
        let length = file.metadata()?.len();
        let end = log.end.unwrap_or(length).min(length);
        let old = cursor.files.get(source);
        let mut start = log.start.min(end);
        let mut reset = false;
        if let Some(old) = old {
            if old.identity != identity
                || old.offset > end
                || old.offset < start
                || anchor(&mut file, old.offset)? != old.anchor
            {
                reset = true;
                page.cursor_reset = true;
                page.reset_reason = Some("log_rotated_or_truncated".into());
                start = 0;
            } else {
                start = old.offset;
            }
        } else if identity != log.file_id || end < log.start {
            reset = true;
            start = 0;
            page.cursor_reset = true;
            page.reset_reason = Some("log_rotated_or_truncated".into());
        }
        let tail = request.cursor.is_none() || reset || old.is_none();
        if remaining == 0 {
            page.has_more |= start < end;
            page.truncated |= start < end;
            continue;
        }
        let window = remaining.min(end.saturating_sub(start) as usize);
        let mut offset = if tail {
            end.saturating_sub(window as u64).max(start)
        } else {
            start
        };
        file.seek(SeekFrom::Start(offset))?;
        let mut data = vec![0; window.min(end.saturating_sub(offset) as usize)];
        file.read_exact(&mut data)?;
        if tail && offset > start {
            if let Some(n) = data.iter().position(|b| *b == b'\n') {
                offset += n as u64 + 1;
                data.drain(..=n);
            }
            page.truncated = true;
        }
        if tail {
            let leading = data.iter().take_while(|b| (**b & 0xc0) == 0x80).count();
            if leading > 0 {
                offset += leading as u64;
                data.drain(..leading);
            }
        }
        if tail {
            let boundaries: Vec<_> = data
                .iter()
                .enumerate()
                .filter_map(|(i, b)| (*b == b'\n').then_some(i + 1))
                .collect();
            if boundaries.len() > lines {
                let n = boundaries[boundaries.len() - lines - 1];
                offset += n as u64;
                data.drain(..n);
                page.truncated = true;
            }
        }
        let live = run.ended_at.is_none();
        let mut partial = false;
        let count = if live && data.last() != Some(&b'\n') {
            if let Some(i) = data.iter().rposition(|b| *b == b'\n') {
                i + 1
            } else if window == remaining {
                partial = true;
                data.len()
            } else {
                0
            }
        } else {
            data.len()
        };
        let mut count = count;
        if let Err(e) = std::str::from_utf8(&data[..count]) {
            if e.error_len().is_none() {
                count = e.valid_up_to();
            }
        }
        let text = String::from_utf8_lossy(&data[..count]);
        if count > 0 {
            let mut cleaned = crate::diagnostics::redact::sanitize(
                &text,
                &crate::diagnostics::redact::private_values(&[
                    task.input_path.clone(),
                    task.projects_root.clone(),
                ]),
            );
            if text.ends_with('\n') {
                cleaned.push('\n');
            }
            page.entries.push(LogChunk {
                source: source.clone(),
                text: cleaned,
                partial_line: partial,
            });
        }
        remaining = remaining.saturating_sub(count);
        let next = offset + count as u64;
        cursor.files.insert(
            source.clone(),
            Position {
                identity,
                offset: next,
                anchor: anchor(&mut file, next)?,
            },
        );
        // A pending partial line is retried on the next poll, rather than busy-polled.
        page.has_more |= next < end && count > 0;
        page.truncated |= next < end && count > 0;
    }
    page.next_cursor = Some(encode(&cursor)?);
    Ok(page)
}
