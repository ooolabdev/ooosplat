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
#[path = "safe_activity.rs"]
mod safe_activity;

pub const MAX_BYTES: usize = 128 * 1024;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisteredLog {
    pub source: String,
    pub path: PathBuf,
    pub root: PathBuf,
    pub file_id: String,
    pub start: u64,
    pub end: Option<u64>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub activity: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub activity_complete: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

pub(crate) struct ProjectActivityTail {
    pub events: Vec<crate::pipeline::PipelineEvent>,
    pub persisted: bool,
}

/// A project fallback has no live TaskRecord cursor. Only a validated run
/// header, never a filename's modification time, can select its activity.
pub(crate) fn project_activity_tail(
    project: &Path,
    project_id: Uuid,
    task_id: Option<Uuid>,
    expected_run: Option<Uuid>,
    private_paths: &[PathBuf],
) -> Result<Option<ProjectActivityTail>> {
    use std::io::BufRead;
    let project = std::fs::canonicalize(project)?;
    let root = project.join("logs");
    let metadata = match std::fs::symlink_metadata(&root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink()
        || std::fs::canonicalize(&root)?.parent() != Some(project.as_path())
    {
        return Err(ServiceError::new(
            "LOG_ACCESS_DENIED",
            "日志文件不属于该任务",
        ));
    }
    let root = std::fs::canonicalize(root)?;
    let mut candidates = Vec::new();
    let mut found = false;
    for (index, entry) in std::fs::read_dir(&root)?.enumerate() {
        if index >= 4096 {
            return Err(ServiceError::new("INVALID_LOG_SOURCE", "日志来源名称过长"));
        }
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("activity-") || !name.ends_with(".log") {
            continue;
        }
        found = true;
        if entry.file_type()?.is_symlink() {
            return Err(ServiceError::new(
                "LOG_ACCESS_DENIED",
                "日志文件不属于该任务",
            ));
        }
        if !entry.file_type()?.is_file() {
            continue;
        }
        let file = secure_open(&root, &entry.path())?;
        let mut first = String::new();
        std::io::BufReader::new(file.take(4097)).read_line(&mut first)?;
        if first.len() > 4096 {
            continue;
        }
        let Ok(header) = serde_json::from_str::<super::activity::RunHeader>(&first) else {
            continue;
        };
        if header.activity_version != 1
            || header.project_id != Some(project_id)
            || task_id.is_some_and(|id| header.task_id != id)
            || expected_run.is_some_and(|id| header.run_id != id)
            || name != format!("activity-{}.log", header.run_id)
        {
            continue;
        }
        candidates.push((header, entry.path()));
    }
    if candidates.is_empty() {
        return Ok(found.then_some(ProjectActivityTail {
            events: Vec::new(),
            persisted: false,
        }));
    }
    candidates.sort_by_key(|(header, _)| header.started_at);
    if candidates.len() > 1
        && candidates[candidates.len() - 2].0.started_at == candidates.last().unwrap().0.started_at
    {
        return Ok(Some(ProjectActivityTail {
            events: Vec::new(),
            persisted: false,
        }));
    }
    let (header, path) = candidates.pop().unwrap();
    let mut file = secure_open(&root, &path)?;
    let length = file.metadata()?.len();
    let offset = length.saturating_sub(256 * 1024);
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = vec![0; (length - offset) as usize];
    file.read_exact(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes);
    let text = if offset > 0 {
        text.split_once('\n').map_or("", |(_, rest)| rest)
    } else {
        &text
    };
    let private = crate::diagnostics::redact::private_values(private_paths);
    let mut events = std::collections::VecDeque::new();
    let mut complete = false;
    let mut valid = true;
    let mut previous = 0;
    for line in text.lines().filter(|line| !line.is_empty()) {
        if serde_json::from_str::<super::activity::RunHeader>(line).is_ok() {
            continue;
        }
        if let Ok(footer) = serde_json::from_str::<super::activity::RunFooter>(line) {
            complete = footer.activity_closed && footer.complete && footer.run_id == header.run_id;
            continue;
        }
        let Ok(mut event) = serde_json::from_str::<crate::pipeline::PipelineEvent>(line) else {
            valid = false;
            continue;
        };
        if event.task_id != Some(header.task_id)
            || event.run_id != Some(header.run_id)
            || event.sequence <= previous
            || matches!(
                event.kind,
                crate::pipeline::EventKind::Runtime | crate::pipeline::EventKind::Heartbeat
            )
        {
            valid = false;
            continue;
        }
        previous = event.sequence;
        sanitize_activity_event(&mut event, &private);
        events.push_back(event);
        if events.len() > 500 {
            events.pop_front();
        }
    }
    Ok(Some(ProjectActivityTail {
        events: events.into_iter().collect(),
        persisted: complete && valid,
    }))
}

fn sanitize_activity_event(event: &mut crate::pipeline::PipelineEvent, private: &[String]) {
    event.message = safe_activity::message(event)
        .unwrap_or_else(|| crate::diagnostics::redact::sanitize(&event.message, private));
    if let Some(source_id) = event
        .operation
        .as_mut()
        .and_then(|op| op.source_id.as_mut())
    {
        *source_id = crate::diagnostics::redact::sanitize(source_id, private);
    }
    event.runtime = None;
    event.acceleration = None;
}

/// Keep old single-stem source names, but never replace a different path merely
/// because another attempt chose the same filename.
pub(super) fn insert_registered(
    logs: &mut BTreeMap<String, RegisteredLog>,
    mut incoming: RegisteredLog,
) {
    if let Some((key, old)) = logs.iter_mut().find(|(_, old)| old.path == incoming.path) {
        incoming.source = key.clone();
        if old.file_id == incoming.file_id {
            incoming.start = old.start.min(incoming.start);
            incoming.end = incoming.end.or(old.end);
        }
        *old = incoming;
        return;
    }
    if logs.contains_key(&incoming.source) {
        use sha2::{Digest, Sha256};
        let relative = incoming
            .path
            .strip_prefix(&incoming.root)
            .unwrap_or(&incoming.path);
        let digest = format!(
            "{:x}",
            Sha256::digest(relative.to_string_lossy().as_bytes())
        );
        incoming.source = format!("{}-{}", incoming.source, &digest[..12]);
        let base = incoming.source.clone();
        let mut suffix = 1;
        while logs.contains_key(&incoming.source) {
            incoming.source = format!("{base}-{suffix}");
            suffix += 1;
        }
    }
    logs.insert(incoming.source.clone(), incoming);
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub activity_events: Option<Vec<crate::pipeline::PipelineEvent>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub activity_journal_persisted: Option<bool>,
}
#[derive(Serialize, Deserialize)]
pub(super) struct Position {
    identity: String,
    offset: u64,
    pub(super) anchor: String,
    #[serde(default)]
    rebased: bool,
    #[serde(default)]
    inside_line: bool,
}
#[derive(Serialize, Deserialize)]
pub(super) struct Cursor {
    version: u32,
    task_id: Uuid,
    run_id: Uuid,
    sources: Vec<String>,
    #[serde(default)]
    all_sources: bool,
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
    if path
        .file_stem()
        .is_some_and(|name| name.to_string_lossy().len() > 64)
    {
        return Err(ServiceError::new("INVALID_LOG_SOURCE", "日志来源名称过长"));
    }
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
        activity: false,
        activity_complete: false,
    })
}

/// Read only the proven byte interval of one execution. Unlike interactive log
/// cursors, diagnostics must never rebase a replaced/truncated file to zero.
pub(crate) fn diagnostic_tail(
    log: &RegisteredLog,
    end: u64,
    max_bytes: usize,
    max_lines: usize,
) -> Option<(String, bool)> {
    if max_bytes == 0 || max_lines == 0 || end < log.start {
        return None;
    }
    let mut file = secure_open(&log.root, &log.path).ok()?;
    if file_id(&file).ok()? != log.file_id || file.metadata().ok()?.len() < end {
        return None;
    }
    let offset = end.saturating_sub(max_bytes as u64).max(log.start);
    file.seek(SeekFrom::Start(offset)).ok()?;
    let mut bytes = vec![0; (end - offset) as usize];
    file.read_exact(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let text = if offset > log.start {
        text.split_once('\n').map(|(_, tail)| tail).unwrap_or("")
    } else {
        &text
    };
    let lines = text.lines().collect::<Vec<_>>();
    let truncated = offset > log.start || lines.len() > max_lines;
    Some((
        crate::diagnostics::redact::bounded(
            &lines[lines.len().saturating_sub(max_lines)..].join("\n"),
            max_bytes,
        ),
        truncated,
    ))
}
fn cursor_key() -> Result<&'static [u8; 32]> {
    static KEY: std::sync::OnceLock<[u8; 32]> = std::sync::OnceLock::new();
    if let Some(key) = KEY.get() {
        return Ok(key);
    }
    let mut key = [0; 32];
    getrandom::fill(&mut key)
        .map_err(|_| ServiceError::new("RANDOM_SOURCE_FAILED", "无法生成日志 cursor 凭据"))?;
    let _ = KEY.set(key);
    Ok(KEY.get().expect("cursor key initialized"))
}
fn from_hex(value: &str) -> Result<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return Err(ServiceError::new("INVALID_CURSOR", "日志 cursor 无效"));
    }
    (0..value.len())
        .step_by(2)
        .map(|i| {
            value
                .get(i..i + 2)
                .and_then(|s| u8::from_str_radix(s, 16).ok())
                .ok_or_else(|| ServiceError::new("INVALID_CURSOR", "日志 cursor 无效"))
        })
        .collect()
}
fn encode(cursor: &Cursor) -> Result<String> {
    use hmac::Mac;
    let bytes = serde_json::to_vec(cursor)?;
    let mut mac =
        hmac::Hmac::<sha2::Sha256>::new_from_slice(cursor_key()?).expect("valid HMAC key");
    mac.update(&bytes);
    let body: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let tag: String = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Ok(format!("{body}.{tag}"))
}
pub(super) fn decode(value: &str) -> Result<Cursor> {
    use hmac::Mac;
    if value.len() > 32 * 1024 {
        return Err(ServiceError::new("INVALID_CURSOR", "日志 cursor 无效"));
    }
    let (body, tag) = value
        .split_once('.')
        .ok_or_else(|| ServiceError::new("INVALID_CURSOR", "日志 cursor 无效"))?;
    let bytes = from_hex(body)?;
    let tag = from_hex(tag)?;
    let mut mac =
        hmac::Hmac::<sha2::Sha256>::new_from_slice(cursor_key()?).expect("valid HMAC key");
    mac.update(&bytes);
    mac.verify_slice(&tag).map_err(|_| {
        ServiceError::new(
            "CURSOR_INVALID_OR_EXPIRED",
            "cursor 无效或应用已重启，请清空 cursor 重新读取",
        )
    })?;
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

fn activity_is_closed(file: &mut File, end: u64, run: Uuid) -> std::io::Result<bool> {
    let start = end.saturating_sub(4096);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = vec![0; (end - start) as usize];
    file.read_exact(&mut bytes)?;
    let tail = String::from_utf8_lossy(&bytes);
    Ok(tail
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .and_then(|line| serde_json::from_str::<super::activity::RunFooter>(line).ok())
        .is_some_and(|footer| footer.activity_closed && footer.complete && footer.run_id == run))
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
        activity_events: None,
        activity_journal_persisted: None,
    };
    let Some(run) = run else { return Ok(page) };
    let mut sources = request
        .sources
        .clone()
        .unwrap_or_else(|| page.available_sources.clone());
    sources.sort();
    sources.dedup();
    if sources
        .iter()
        .filter(|source| run.logs.get(*source).is_none_or(|log| !log.activity))
        .count()
        > 32
        || sources.iter().any(|s| !run.logs.contains_key(s))
    {
        return Err(ServiceError::new(
            "INVALID_LOG_SOURCE",
            "只能读取该执行已登记的日志来源",
        ));
    }
    let mut cursor = if let Some(value) = &request.cursor {
        let mut cursor = decode(value)?;
        let added_sources = request.sources.is_none()
            && cursor.all_sources
            && cursor.sources.iter().all(|source| sources.contains(source));
        if cursor.version != 1
            || cursor.task_id != task.task_id
            || cursor.run_id != run.run_id
            || (cursor.sources != sources && !added_sources)
        {
            return Err(ServiceError::new(
                "CURSOR_MISMATCH",
                "cursor 不属于该任务、执行或来源筛选",
            ));
        }
        cursor.sources = sources.clone();
        cursor
    } else {
        Cursor {
            version: 1,
            task_id: task.task_id,
            run_id: run.run_id,
            sources: sources.clone(),
            all_sources: request.sources.is_none(),
            files: Default::default(),
        }
    };
    let mut remaining = bytes;
    let mut remaining_lines = if request.cursor.is_none() { lines } else { 500 };
    for source in &sources {
        let log = &run.logs[source];
        if log.activity {
            page.activity_events.get_or_insert_with(Vec::new);
            page.activity_journal_persisted =
                Some(page.activity_journal_persisted.unwrap_or(true) && log.activity_complete);
        }
        let mut file = match secure_open(&log.root, &log.path) {
            Ok(f) => f,
            Err(_e) if !log.path.exists() => {
                page.cursor_reset = true;
                page.reset_reason = Some("log_missing".into());
                if log.activity {
                    page.activity_journal_persisted = Some(false);
                }
                continue;
            }
            Err(e) => return Err(e),
        };
        let identity = file_id(&file)?;
        let length = file.metadata()?.len();
        let end = log.end.unwrap_or(length).min(length);
        if log.activity
            && !(task.status.active() && task.run_id == Some(run.run_id))
            && !activity_is_closed(&mut file, end, run.run_id)?
        {
            page.activity_journal_persisted = Some(false);
        }
        let old = cursor.files.get(source);
        let was_rebased = old.is_some_and(|position| position.rebased);
        let was_inside = old.is_some_and(|position| position.inside_line);
        let mut start = if was_rebased { 0 } else { log.start.min(end) };
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
        if !(task.status.active() && task.run_id == Some(run.run_id))
            && (reset || identity != log.file_id)
        {
            page.cursor_reset = true;
            page.reset_reason = Some("historical_log_replaced".into());
            if log.activity {
                page.activity_journal_persisted = Some(false);
            }
            continue;
        }
        if log.activity && (reset || identity != log.file_id) {
            page.activity_journal_persisted = Some(false);
        }
        let tail = request.cursor.is_none() || reset || old.is_none();
        if remaining == 0 || remaining_lines == 0 {
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
        let mut incomplete_utf8_only = false;
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
                incomplete_utf8_only = leading == data.len();
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
            if boundaries.len() > remaining_lines {
                let n = boundaries[boundaries.len() - remaining_lines - 1];
                offset += n as u64;
                data.drain(..n);
                page.truncated = true;
            }
        }
        let live = task.status.active() && task.run_id == Some(run.run_id);
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
        let boundaries: Vec<_> = data[..count]
            .iter()
            .enumerate()
            .filter_map(|(i, b)| (*b == b'\n').then_some(i + 1))
            .collect();
        if boundaries.len() > remaining_lines {
            count = boundaries[remaining_lines - 1];
        }

        if let Err(e) = std::str::from_utf8(&data[..count]) {
            if e.error_len().is_none() {
                count = e.valid_up_to();
                incomplete_utf8_only |= count == 0 && !data.is_empty();
            }
        }
        let inside_line = count > 0 && data[count - 1] != b'\n';
        let split_fragment = partial
            || (inside_line
                && ((tail && offset > start) || (!tail && offset + (count as u64) < end)));
        // A preceding source may leave only the last newline, part of a UTF-8
        // character, or a slice of a JSON event in this page's budget. Retry
        // this source with a fresh page, without moving its cursor or losing
        // the complete rows already collected from other sources.
        if remaining < bytes
            && window > 0
            && (count == 0 || (window == remaining && split_fragment))
        {
            page.has_more |= start < end;
            page.truncated |= start < end;
            if log.activity {
                page.activity_journal_persisted = Some(false);
            }
            continue;
        }
        if count == 0 && window == remaining && window > 0 && incomplete_utf8_only {
            return Err(ServiceError::new(
                "LOG_WINDOW_TOO_SMALL",
                "max_bytes 不足以读取完整 UTF-8 字符，请增大读取窗口",
            ));
        }
        let text = String::from_utf8_lossy(&data[..count]);
        let text = if partial || (tail && offset > start && inside_line) {
            "[REDACTED_PARTIAL_LINE]".to_string()
        } else if was_inside {
            let rest = text.find('\n').map(|i| &text[i + 1..]).unwrap_or("");
            format!("[REDACTED_PARTIAL_LINE]\n{rest}")
        } else {
            text.into_owned()
        };

        if count > 0 {
            let private = crate::diagnostics::redact::private_values(&[
                task.input_path.clone(),
                task.projects_root.clone(),
            ]);
            let mut cleaned = if log.activity {
                let mut rendered = String::new();
                for line in text.lines().filter(|line| !line.is_empty()) {
                    if let Ok(header) = serde_json::from_str::<super::activity::RunHeader>(line) {
                        if header.activity_version != 1
                            || header.task_id != task.task_id
                            || header.run_id != run.run_id
                        {
                            page.activity_journal_persisted = Some(false);
                        }
                        continue;
                    }
                    if let Ok(footer) = serde_json::from_str::<super::activity::RunFooter>(line) {
                        if !footer.activity_closed
                            || footer.run_id != run.run_id
                            || !footer.complete
                        {
                            page.activity_journal_persisted = Some(false);
                        }
                        continue;
                    }
                    let Ok(mut event) =
                        serde_json::from_str::<crate::pipeline::PipelineEvent>(line)
                    else {
                        page.activity_journal_persisted = Some(false);
                        page.truncated = true;
                        continue;
                    };
                    if event.task_id != Some(task.task_id)
                        || event.run_id != Some(run.run_id)
                        || matches!(
                            event.kind,
                            crate::pipeline::EventKind::Runtime
                                | crate::pipeline::EventKind::Heartbeat
                        )
                    {
                        page.activity_journal_persisted = Some(false);
                        page.truncated = true;
                        continue;
                    }
                    // Sanitize fields before serializing. Redacting JSON text
                    // can corrupt escaping or accidentally destroy safe keys.
                    sanitize_activity_event(&mut event, &private);
                    rendered.push_str(&event.message);
                    rendered.push('\n');
                    page.activity_events
                        .as_mut()
                        .expect("activity source")
                        .push(event);
                }
                rendered.trim_end_matches('\n').to_owned()
            } else {
                crate::diagnostics::redact::sanitize(&text, &private)
            };
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
        let used_lines = data[..count].iter().filter(|b| **b == b'\n').count()
            + usize::from(count > 0 && inside_line);
        remaining_lines = remaining_lines.saturating_sub(used_lines);
        let next = offset + count as u64;
        cursor.files.insert(
            source.clone(),
            Position {
                identity,
                offset: next,
                anchor: anchor(&mut file, next)?,
                rebased: reset || was_rebased,
                inside_line,
            },
        );
        // A pending partial line is retried on the next poll, rather than busy-polled.
        page.has_more |=
            next < end && count > 0 && (!live || window == remaining.saturating_add(count));
        page.truncated |= next < end && count > 0;
    }
    page.next_cursor = Some(encode(&cursor)?);
    Ok(page)
}

#[cfg(test)]
mod pagination_regressions {
    use super::*;
    use crate::{
        pipeline::{EventKind, PipelineEvent, PipelineStage},
        presets::Quality,
        tasks::{TaskRun, TaskService, TaskStatus},
    };

    async fn fixture(contents: &[(&str, &str)]) -> (tempfile::TempDir, TaskRecord) {
        let root = tempfile::tempdir().unwrap();
        let input = root.path().join("input.mp4");
        std::fs::write(&input, b"test input").unwrap();
        let service = TaskService::at(root.path().join("registry"));
        let mut task = service
            .create(
                None,
                input,
                Quality::Fast,
                root.path().join("projects"),
                false,
                "gui",
                None,
                None,
            )
            .await
            .unwrap();
        let run_id = Uuid::new_v4();
        let directory = root.path().join("logs");
        std::fs::create_dir(&directory).unwrap();
        let mut logs = BTreeMap::new();
        for (name, content) in contents {
            let path = directory.join(format!("{name}.log"));
            std::fs::write(&path, content).unwrap();
            insert_registered(&mut logs, register(&directory, &path, 0).unwrap());
        }
        task.status = TaskStatus::Completed;
        task.run_id = Some(run_id);
        task.runs.push(TaskRun {
            run_id,
            kind: "generation".into(),
            continued_from_status: None,
            started_at: chrono::Utc::now(),
            ended_at: Some(chrono::Utc::now()),
            logs,
            last_engine: None,
            last_exit_code: None,
            error: None,
            brush_executions: Vec::new(),
            current_brush_execution: None,
        });
        (root, task)
    }

    fn request(task: &TaskRecord, budget: usize, cursor: Option<String>) -> LogRequest {
        LogRequest {
            task_id: task.task_id,
            run_id: task.run_id,
            max_bytes: Some(budget),
            cursor,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn cross_source_tiny_remainders_defer_without_advancing_or_losing_complete_lines() {
        for remainder in [1, 2, 3, 7] {
            for text in ["中文行\n", "a short ASCII line\n", "中文"] {
                if text.len() <= remainder {
                    continue;
                }
                let first = format!("{}\n", "a".repeat(32 - remainder - 1));
                let (_root, task) = fixture(&[("a-first", &first), ("z-next", text)]).await;
                let page = read(&task, &request(&task, 32, None)).unwrap();
                assert!(page.has_more, "remainder={remainder} text={text}");
                assert_eq!(page.entries.len(), 1);
                assert_eq!(page.entries[0].source, "a-first");
                let cursor = decode(page.next_cursor.as_ref().unwrap()).unwrap();
                assert!(!cursor.files.contains_key("z-next"));
                let next = read(&task, &request(&task, 32, page.next_cursor)).unwrap();
                assert!(!next.has_more);
                assert_eq!(
                    next.entries
                        .iter()
                        .map(|entry| entry.text.as_str())
                        .collect::<String>(),
                    text
                );
            }
        }
    }

    #[tokio::test]
    async fn deferred_activity_json_is_read_whole_with_a_fresh_budget_in_tail_and_forward_modes() {
        for forward in [false, true] {
            let (root, mut task) = fixture(&[]).await;
            task.status = TaskStatus::Running;
            let directory = root.path().join("logs");
            let path = directory.join("z-activity.log");
            let run_id = task.run_id.unwrap();
            let header = crate::tasks::activity::RunHeader {
                activity_version: 1,
                task_id: task.task_id,
                run_id,
                project_id: Some(Uuid::new_v4()),
                started_at: chrono::Utc::now(),
            };
            let header = format!("{}\n", serde_json::to_string(&header).unwrap());
            let mut event = PipelineEvent::mapped(
                PipelineStage::TrainingSplats,
                0.5,
                "正在准备训练图片：41/90",
            );
            event.kind = EventKind::Progress;
            event.task_id = Some(task.task_id);
            event.run_id = Some(run_id);
            event.sequence = 1;
            event.current = Some(41);
            event.total = Some(90);
            event.unit = Some("files".into());
            let event = format!("{}\n", serde_json::to_string(&event).unwrap());
            assert!(header.len() + event.len() < 2048);
            std::fs::write(
                &path,
                if forward {
                    header.clone()
                } else {
                    format!("{header}{event}")
                },
            )
            .unwrap();
            let mut log = register(&directory, &path, 0).unwrap();
            log.activity = true;
            insert_registered(&mut task.runs.last_mut().unwrap().logs, log);
            let old_cursor = if forward {
                let page = read(&task, &request(&task, 2048, None)).unwrap();
                assert!(page.activity_events.as_ref().unwrap().is_empty());
                std::fs::write(&path, format!("{header}{event}")).unwrap();
                page.next_cursor
            } else {
                None
            };
            let before_position = old_cursor
                .as_deref()
                .map(|value| decode(value).unwrap().files["z-activity"].offset);
            let first = directory.join("a-first.log");
            std::fs::write(&first, format!("{}\n", "a".repeat(2045))).unwrap();
            insert_registered(
                &mut task.runs.last_mut().unwrap().logs,
                register(&directory, &first, 0).unwrap(),
            );
            let original = std::fs::read(&path).unwrap();
            let page = read(&task, &request(&task, 2048, old_cursor)).unwrap();
            assert!(page.has_more);
            assert_eq!(page.activity_journal_persisted, Some(false));
            assert!(page.activity_events.as_ref().unwrap().is_empty());
            assert_eq!(page.entries.len(), 1);
            let cursor = decode(page.next_cursor.as_ref().unwrap()).unwrap();
            assert_eq!(
                cursor
                    .files
                    .get("z-activity")
                    .map(|position| position.offset),
                before_position
            );
            let next = read(&task, &request(&task, 2048, page.next_cursor)).unwrap();
            assert!(!next.has_more);
            let events = next.activity_events.as_ref().unwrap();
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].message, "正在准备训练图片：41/90");
            assert!(!next
                .entries
                .iter()
                .any(|entry| entry.text.contains("REDACTED_PARTIAL_LINE")));
            assert_eq!(std::fs::read(path).unwrap(), original);
        }
    }

    #[tokio::test]
    async fn a_full_tiny_page_distinguishes_utf8_characters_from_a_discarded_ascii_tail() {
        for text in ["an ASCII line longer than the whole budget\n", "x\n"] {
            let (_root, task) = fixture(&[("only", text)]).await;
            let page = read(&task, &request(&task, 1, None)).unwrap();
            assert!(!page.has_more);
            let cursor = decode(page.next_cursor.as_ref().unwrap()).unwrap();
            assert_eq!(cursor.files["only"].offset, text.len() as u64);
            assert!(page.truncated);
        }
        let (_root, task) = fixture(&[("only", "中")]).await;
        for budget in [1, 2] {
            assert_eq!(
                read(&task, &request(&task, budget, None)).unwrap_err().code,
                "LOG_WINDOW_TOO_SMALL"
            );
        }
        assert_eq!(
            read(&task, &request(&task, 3, None)).unwrap().entries[0].text,
            "中"
        );
    }

    #[tokio::test]
    async fn oversized_lines_make_bounded_progress_without_exposing_their_fragments() {
        let text = format!("private/{}", "sensitive".repeat(100));
        let (_root, task) = fixture(&[("only", &text)]).await;
        let page = read(&task, &request(&task, 32, None)).unwrap();
        assert!(!page.has_more);
        assert!(page.truncated);
        assert_eq!(page.entries[0].text, "[REDACTED_PARTIAL_LINE]");
        assert_eq!(
            decode(page.next_cursor.as_ref().unwrap()).unwrap().files["only"].offset,
            text.len() as u64
        );
    }

    #[tokio::test]
    async fn a_real_log_open_error_is_not_reclassified_as_a_budget_deferral() {
        let (root, task) = fixture(&[("only", "real log\n")]).await;
        let path = root.path().join("logs/only.log");
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(read(&task, &request(&task, 32, None)).is_err());
    }
}

#[cfg(test)]
mod real_read_regressions {
    use super::*;

    /// Explicit local diagnostic only. All paths come from the caller-named
    /// existing registry, and every operation is a bounded, read-only log read.
    #[tokio::test]
    #[ignore = "requires OOOSPLAT_ACTIVITY_VALIDATION_REGISTRY naming an existing local task registry"]
    async fn real_completed_task_log_pages_and_historical_readers_are_read_only() {
        let path = std::env::var_os("OOOSPLAT_ACTIVITY_VALIDATION_REGISTRY")
            .expect("set OOOSPLAT_ACTIVITY_VALIDATION_REGISTRY for explicit local verification");
        let registry: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let tasks = registry["tasks"].as_object().unwrap();
        let mut failures = Vec::new();
        let mut checked = 0;
        for task in tasks.values() {
            let task: TaskRecord = serde_json::from_value(task.clone()).unwrap();
            if task.status != super::super::TaskStatus::Completed
                || task.project_path.as_ref().is_none_or(|path| !path.is_dir())
            {
                continue;
            }
            checked += 1;
            let name = task
                .project_path
                .as_ref()
                .unwrap()
                .file_name()
                .unwrap()
                .to_string_lossy();
            let mut cursor = None;
            let mut finished = false;
            for page_number in 0..16 {
                let request = LogRequest {
                    task_id: task.task_id,
                    run_id: task.run_id,
                    cursor: cursor.clone(),
                    max_bytes: Some(32768),
                    ..Default::default()
                };
                match read(&task, &request) {
                    Ok(page) => {
                        if !page.has_more {
                            finished = true;
                            break;
                        }
                        if page.next_cursor.is_none() || page.next_cursor == cursor {
                            failures.push(format!(
                                "{} {name} CURSOR_NOT_ADVANCING page={page_number}",
                                task.task_id
                            ));
                            break;
                        }
                        cursor = page.next_cursor;
                    }
                    Err(error) => {
                        failures.push(format!(
                            "{} {name} {} page={page_number}",
                            task.task_id, error.code
                        ));
                        break;
                    }
                }
            }
            if !finished
                && !failures
                    .last()
                    .is_some_and(|error| error.starts_with(&task.task_id.to_string()))
            {
                failures.push(format!("{} {name} LOG_DRAIN_LIMIT", task.task_id));
            }
            if ["20261006-225143_images", "20261111-143414_images_3e06947d"]
                .contains(&name.as_ref())
            {
                match crate::commands::probe_historical_log_read_for_test(&task).await {
                    Ok((rows, persisted)) => eprintln!(
                        "historical {} {name} rows={rows} persisted={persisted:?}",
                        task.task_id
                    ),
                    Err(code) => failures.push(format!("{} {name} {code}", task.task_id)),
                }
            }
        }
        eprintln!(
            "checked completed existing projects={checked}; log-read failures={}",
            failures.len()
        );
        for failure in &failures {
            eprintln!("{failure}");
        }
        assert!(checked > 0);
        assert!(
            failures.is_empty(),
            "read-only completed-log failures listed above"
        );
    }
}
