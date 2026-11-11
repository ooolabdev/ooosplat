use std::{
    ffi::OsString,
    path::PathBuf,
    process::Stdio,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use tokio::{
    fs,
    fs::OpenOptions,
    io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader},
    process::Command,
    sync::Mutex,
};
use tokio_util::sync::CancellationToken;

use crate::error::{Result, SplatError};

pub mod resources;

#[cfg(unix)]
fn signal_process_group(process_id: u32, signal: libc::c_int) -> std::io::Result<()> {
    // The child is made the leader of a new process group before spawn, so a
    // negative PID targets it and every descendant that stays in that group.
    let result = unsafe { libc::kill(-(process_id as libc::pid_t), signal) };
    if result == 0 {
        Ok(())
    } else {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(())
        } else {
            Err(error)
        }
    }
}

#[cfg(unix)]
struct ProcessGroupGuard(u32);
#[cfg(unix)]
impl Drop for ProcessGroupGuard {
    fn drop(&mut self) {
        let _ = signal_process_group(self.0, libc::SIGKILL);
    }
}

struct StreamTasksGuard {
    finished: Arc<AtomicBool>,
    handles: Vec<tokio::task::AbortHandle>,
}
impl Drop for StreamTasksGuard {
    fn drop(&mut self) {
        self.finished.store(true, Ordering::Relaxed);
        for handle in &self.handles {
            handle.abort();
        }
    }
}

#[cfg(windows)]
mod windows_job {
    use std::{io, mem::size_of, ptr};

    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE},
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD,
                THREADENTRY32,
            },
            JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
                SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
                JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            },
            Threading::{
                OpenProcess, OpenThread, ResumeThread, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
                THREAD_SUSPEND_RESUME,
            },
        },
    };

    pub struct WindowsJob(HANDLE);

    // Windows kernel handles are process-wide values and may be moved between
    // executor threads. Ownership still remains unique through this wrapper.
    unsafe impl Send for WindowsJob {}

    impl WindowsJob {
        pub fn create() -> io::Result<Self> {
            // SAFETY: Both optional pointers are null and the returned owned handle is
            // closed in Drop. SetInformation receives a correctly sized initialized struct.
            unsafe {
                let handle = CreateJobObjectW(ptr::null(), ptr::null());
                if handle.is_null() {
                    return Err(io::Error::last_os_error());
                }
                let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
                limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                if SetInformationJobObject(
                    handle,
                    JobObjectExtendedLimitInformation,
                    (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                ) == 0
                {
                    let error = io::Error::last_os_error();
                    CloseHandle(handle);
                    return Err(error);
                }
                Ok(Self(handle))
            }
        }

        pub fn assign(&self, process_id: u32) -> io::Result<()> {
            // SAFETY: OpenProcess returns an owned handle for the supplied child PID;
            // it remains valid through assignment and is closed on every path.
            unsafe {
                let process = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, process_id);
                if process.is_null() {
                    return Err(io::Error::last_os_error());
                }
                let result = AssignProcessToJobObject(self.0, process);
                let error = if result == 0 {
                    Some(io::Error::last_os_error())
                } else {
                    None
                };
                CloseHandle(process);
                error.map_or(Ok(()), Err)
            }
        }

        pub fn resume_primary_thread(&self, process_id: u32) -> io::Result<()> {
            // SAFETY: Snapshot and thread handles are closed on every path, and the
            // enumeration structure advertises its exact size to Windows.
            unsafe {
                let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
                if snapshot == INVALID_HANDLE_VALUE {
                    return Err(io::Error::last_os_error());
                }
                let mut entry: THREADENTRY32 = std::mem::zeroed();
                entry.dwSize = size_of::<THREADENTRY32>() as u32;
                let mut has_entry = Thread32First(snapshot, &mut entry) != 0;
                while has_entry {
                    if entry.th32OwnerProcessID == process_id {
                        let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                        if thread.is_null() {
                            let error = io::Error::last_os_error();
                            CloseHandle(snapshot);
                            return Err(error);
                        }
                        let result = ResumeThread(thread);
                        let error = (result == u32::MAX).then(io::Error::last_os_error);
                        CloseHandle(thread);
                        CloseHandle(snapshot);
                        return error.map_or(Ok(()), Err);
                    }
                    has_entry = Thread32Next(snapshot, &mut entry) != 0;
                }
                CloseHandle(snapshot);
                Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "找不到暂停的子进程主线程",
                ))
            }
        }

        pub fn terminate(&self) {
            // SAFETY: self.0 is a live job handle owned by this value.
            unsafe {
                TerminateJobObject(self.0, 1);
            }
        }
    }

    impl Drop for WindowsJob {
        fn drop(&mut self) {
            // SAFETY: The handle is owned and closed exactly once here.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ProcessStream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone)]
pub enum ProcessUpdate {
    LogOpened {
        path: PathBuf,
        offset: u64,
    },
    /// The execution's termination row has been written. The interval is now
    /// safe to freeze without including a later process appending to this file.
    LogClosed {
        path: PathBuf,
        end: u64,
        success: bool,
        termination: ProcessTermination,
    },
    Exited {
        executable: PathBuf,
        exit_code: Option<i32>,
    },
    Started {
        process_id: u32,
    },
    Line {
        stream: ProcessStream,
        line: String,
    },
    Heartbeat {
        elapsed_ms: u64,
    },
    Resources(resources::ProcessResources),
}

pub type ProcessObserver = Arc<dyn Fn(ProcessUpdate) + Send + Sync>;

#[derive(Clone)]
pub struct ProcessSpec {
    pub executable: PathBuf,
    pub args: Vec<OsString>,
    pub working_directory: Option<PathBuf>,
    pub log_path: Option<PathBuf>,
    pub observer: Option<ProcessObserver>,
}

#[derive(Debug, Clone)]
pub struct ProcessOutput {
    pub success: bool,
    pub exit_code: Option<i32>,
    pub termination: ProcessTermination,
    pub stdout: String,
    pub stderr: String,
}

/// Status of this invocation, never inferred from an earlier appended log.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessTermination {
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub windows_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub core_dumped: Option<bool>,
}

impl ProcessTermination {
    fn from_status(status: std::process::ExitStatus) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            Self {
                exit_code: status.code(),
                signal: status.signal(),
                core_dumped: status.signal().map(|_| status.core_dumped()),
                ..Self::default()
            }
        }
        #[cfg(not(unix))]
        Self {
            exit_code: status.code(),
            windows_status: status.code().map(|code| format!("0x{:08X}", code as u32)),
            ..Self::default()
        }
    }

    pub fn abnormal(&self) -> bool {
        self.signal.is_some()
            || matches!(
                self.windows_status.as_deref(),
                // Known native exceptions; arbitrary nonzero/negative application
                // exit codes are not proof of a crash or of its underlying cause.
                Some(
                    "0xC0000005"
                        | "0xC000001D"
                        | "0xC0000094"
                        | "0xC0000096"
                        | "0xC00000FD"
                        | "0xC0000374"
                        | "0xC0000409"
                        | "0x80000003"
                )
            )
    }
}

impl std::fmt::Display for ProcessTermination {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "exit_code: {:?}", self.exit_code)?;
        if let Some(status) = &self.windows_status {
            write!(f, " ({status})")?;
        }
        if let Some(signal) = self.signal {
            write!(f, "; signal: {signal}; core_dumped: {:?}", self.core_dumped)?;
        }
        Ok(())
    }
}

impl ProcessOutput {
    /// Returns a bounded tail of the engine output so command errors retain the
    /// useful cause without sending an unbounded log through the Tauri bridge.
    pub fn failure_detail(&self) -> String {
        const MAX_CHARS: usize = 4_096;
        let source = if self.stderr.trim().is_empty() {
            self.stdout.trim()
        } else {
            self.stderr.trim()
        };
        let mut tail = source.chars().rev().take(MAX_CHARS).collect::<Vec<_>>();
        tail.reverse();
        tail.into_iter().collect()
    }
}

#[derive(Clone, Default)]
pub struct ProcessManager {
    output_capture_limit: Option<usize>,
    lifecycle: Option<ProcessObserver>,
    cancellation: CancellationToken,
}

impl ProcessManager {
    pub fn new() -> Self {
        Self {
            output_capture_limit: None,
            cancellation: CancellationToken::new(),
            lifecycle: None,
        }
    }

    pub fn with_lifecycle(mut self, observer: ProcessObserver) -> Self {
        self.lifecycle = Some(observer);
        self
    }

    /// Streaming probes consume every observer event but retain only a bounded
    /// diagnostic tail. Ordinary engine callers retain their existing behavior.
    pub(crate) fn with_output_capture_limit(mut self, limit: usize) -> Self {
        self.output_capture_limit = Some(limit);
        self
    }

    pub fn cancel(&self) {
        self.cancellation.cancel();
    }

    pub fn child_token(&self) -> CancellationToken {
        self.cancellation.child_token()
    }

    pub async fn run(&self, spec: ProcessSpec) -> Result<ProcessOutput> {
        self.run_with_environment(spec, &[]).await
    }

    /// Runs a child with overrides that apply only to that process. The parent
    /// environment is never mutated, which keeps engine-specific GPU settings
    /// from leaking into OOOSplat or other bundled tools.
    pub async fn run_with_environment(
        &self,
        spec: ProcessSpec,
        environment: &[(OsString, OsString)],
    ) -> Result<ProcessOutput> {
        if self.cancellation.is_cancelled() {
            return Err(SplatError::Cancelled);
        }
        if !spec.executable.is_file() {
            return Err(SplatError::EngineMissing(
                spec.executable.display().to_string(),
            ));
        }

        let started = Instant::now();
        #[cfg(windows)]
        let job = windows_job::WindowsJob::create().map_err(|error| {
            SplatError::Process(format!("无法创建 Windows Job Object：{error}"))
        })?;
        let mut command = Command::new(&spec.executable);
        command
            .args(&spec.args)
            .envs(environment.iter().map(|(key, value)| (key, value)))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(directory) = &spec.working_directory {
            command.current_dir(directory);
        }

        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            const CREATE_SUSPENDED: u32 = 0x0000_0004;
            command.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
        }

        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.as_std_mut().process_group(0);
        }

        let mut child = command.spawn().map_err(|error| SplatError::EngineStart {
            engine: spec.executable.display().to_string(),
            detail: error.to_string(),
        })?;
        let process_id = child
            .id()
            .ok_or_else(|| SplatError::Process("无法读取子进程 ID".into()))?;
        #[cfg(unix)]
        let _group = ProcessGroupGuard(process_id);
        #[cfg(windows)]
        {
            if let Err(error) = job.assign(process_id) {
                job.terminate();
                let _ = child.kill().await;
                let _ = child.wait().await;
                return Err(SplatError::Process(format!(
                    "无法把子进程加入 Windows Job Object：{error}"
                )));
            }
            if let Err(error) = job.resume_primary_thread(process_id) {
                job.terminate();
                let _ = child.kill().await;
                let _ = child.wait().await;
                return Err(SplatError::Process(format!(
                    "无法恢复子进程主线程：{error}"
                )));
            }
        }
        if let Some(observer) = &spec.observer {
            observer(ProcessUpdate::Started { process_id });
        }

        let log_file = if let Some(path) = &spec.log_path {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).await?;
            }
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .await?;
            let opened = ProcessUpdate::LogOpened {
                path: path.clone(),
                offset: file.metadata().await?.len(),
            };
            if let Some(observer) = &self.lifecycle {
                observer(opened.clone());
            }
            if let Some(observer) = &spec.observer {
                observer(opened);
            }
            let args = spec
                .args
                .iter()
                .map(|arg| arg.to_string_lossy())
                .collect::<Vec<_>>()
                .join("\n  ");
            file.write_all(
                format!(
                    "executable: {}\narguments:\n  {}\n",
                    spec.executable.display(),
                    args
                )
                .as_bytes(),
            )
            .await?;
            Some(Arc::new(Mutex::new(file)))
        } else {
            None
        };
        let stdout_task = tokio::spawn(pump_stream(
            child.stdout.take().expect("stdout is piped"),
            ProcessStream::Stdout,
            spec.observer.clone(),
            log_file.clone(),
            self.output_capture_limit,
        ));
        let stderr_task = tokio::spawn(pump_stream(
            child.stderr.take().expect("stderr is piped"),
            ProcessStream::Stderr,
            spec.observer.clone(),
            log_file.clone(),
            self.output_capture_limit,
        ));
        let finished = Arc::new(AtomicBool::new(false));
        let resource_task = spec
            .observer
            .clone()
            .map(|observer| resources::spawn(process_id, observer));
        let heartbeat_task = spec.observer.clone().map(|observer| {
            let finished = finished.clone();
            tokio::spawn(async move {
                while !finished.load(Ordering::Relaxed) {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    if !finished.load(Ordering::Relaxed) {
                        observer(ProcessUpdate::Heartbeat {
                            elapsed_ms: started.elapsed().as_millis() as u64,
                        });
                    }
                }
            })
        });

        let mut stream_handles = vec![stdout_task.abort_handle(), stderr_task.abort_handle()];
        if let Some(task) = &heartbeat_task {
            stream_handles.push(task.abort_handle());
        }
        let _streams = StreamTasksGuard {
            finished: finished.clone(),
            handles: stream_handles,
        };
        let status = tokio::select! {
            status = child.wait() => status?,
            _ = self.cancellation.cancelled() => {
                #[cfg(windows)]
                job.terminate();
                #[cfg(unix)]
                {
                    let _ = signal_process_group(process_id, libc::SIGTERM);
                    if tokio::time::timeout(Duration::from_secs(3), child.wait()).await.is_err() {
                        let _ = signal_process_group(process_id, libc::SIGKILL);
                        let _ = child.wait().await;
                    } else {
                        // The direct child can exit while a descendant ignores SIGTERM.
                        // The process-group ID remains usable until the last member exits.
                        let _ = signal_process_group(process_id, libc::SIGKILL);
                    }
                }
                #[cfg(not(unix))]
                {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                }
                finished.store(true, Ordering::Relaxed);
                drop(resource_task);
                stdout_task.abort();
                stderr_task.abort();
                if let Some(task) = heartbeat_task { task.abort(); }
                if let Some(log_path) = &spec.log_path {
                    if let Some(parent) = log_path.parent() { fs::create_dir_all(parent).await?; }
                    let mut file = OpenOptions::new().create(true).append(true).open(log_path).await?;
                    file.write_all(format!("cancelled after {} ms\n\n", started.elapsed().as_millis()).as_bytes()).await?;
                }
                return Err(SplatError::Cancelled);
            }
        };
        finished.store(true, Ordering::Relaxed);
        if let Some(task) = resource_task {
            task.stop().await;
        }
        if let Some(task) = heartbeat_task {
            let _ = task.await;
        }

        let stdout = stdout_task
            .await
            .map_err(|error| SplatError::Process(error.to_string()))??;
        let stderr = stderr_task
            .await
            .map_err(|error| SplatError::Process(error.to_string()))??;
        // A requested cancellation may race with wait() becoming ready. Do not
        // turn our own process termination into a crash/failure diagnostic.
        if self.cancellation.is_cancelled() {
            return Err(SplatError::Cancelled);
        }
        if let Some(observer) = &self.lifecycle {
            observer(ProcessUpdate::Exited {
                executable: spec.executable.clone(),
                exit_code: status.code(),
            });
        }
        let output = ProcessOutput {
            success: status.success(),
            exit_code: status.code(),
            termination: ProcessTermination::from_status(status),
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
        };

        if let Some(file) = log_file {
            let mut file = file.lock().await;
            file.write_all(
                format!(
                    "{}\nelapsed_ms: {}\n\n",
                    output.termination,
                    started.elapsed().as_millis()
                )
                .as_bytes(),
            )
            .await?;
            // Provenance collection is best effort: failure to stat the log
            // must not replace the engine's actual result.
            let end = if file.flush().await.is_ok() {
                file.metadata().await.ok().map(|metadata| metadata.len())
            } else {
                None
            };
            drop(file);
            if let (Some(observer), Some(path), Some(end)) = (&spec.observer, &spec.log_path, end) {
                observer(ProcessUpdate::LogClosed {
                    path: path.clone(),
                    end,
                    success: output.success,
                    termination: output.termination.clone(),
                });
            }
        }
        if let Some(observer) = &spec.observer {
            observer(ProcessUpdate::Exited {
                executable: spec.executable.clone(),
                exit_code: output.exit_code,
            });
        }

        Ok(output)
    }
}

async fn pump_stream<R: AsyncRead + Unpin>(
    reader: R,
    stream: ProcessStream,
    observer: Option<ProcessObserver>,
    log: Option<Arc<Mutex<tokio::fs::File>>>,
    capture_limit: Option<usize>,
) -> std::io::Result<Vec<u8>> {
    let mut reader = BufReader::new(reader);
    let mut collected = std::collections::VecDeque::new();
    let mut line = Vec::new();
    loop {
        line.clear();
        let read = reader.read_until(b'\n', &mut line).await?;
        if read == 0 {
            break;
        }
        collected.extend(line.iter().copied());
        if let Some(limit) = capture_limit {
            let discard = collected.len().saturating_sub(limit);
            collected.drain(..discard);
        }
        if let Some(file) = &log {
            file.lock().await.write_all(&line).await?;
        }
        if let Some(observer) = &observer {
            observer(ProcessUpdate::Line {
                stream,
                line: String::from_utf8_lossy(&line).trim().to_string(),
            });
        }
    }
    Ok(collected.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    #[cfg(any(windows, unix))]
    #[tokio::test]
    async fn scoped_log_interval_is_sealed_after_the_termination_row() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("gps.log");
        std::fs::write(&path, "previous attempt\n").unwrap();
        let start = std::fs::metadata(&path).unwrap().len();
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let received = events.clone();
        #[cfg(windows)]
        let (executable, args) = (
            system_executable("cmd.exe"),
            vec![
                "/D".into(),
                "/C".into(),
                "echo scoped-output & exit /B 7".into(),
            ],
        );
        #[cfg(unix)]
        let (executable, args) = (
            PathBuf::from("/bin/sh"),
            vec!["-c".into(), "printf 'scoped-output\\n'; exit 7".into()],
        );
        let output = ProcessManager::new()
            .run(ProcessSpec {
                executable,
                args,
                working_directory: Some(directory.path().to_owned()),
                log_path: Some(path.clone()),
                observer: Some(Arc::new(move |event| {
                    let name = match event {
                        ProcessUpdate::Started { .. } => "started",
                        ProcessUpdate::LogOpened { offset, .. } => {
                            assert_eq!(offset, start);
                            "opened"
                        }
                        ProcessUpdate::LogClosed {
                            path,
                            end,
                            success,
                            termination,
                        } => {
                            assert!(!success);
                            assert_eq!(termination.exit_code, Some(7));
                            let text = std::fs::read_to_string(&path).unwrap();
                            assert!(text.contains("scoped-output"));
                            assert!(text.contains("exit_code: Some(7)"));
                            assert!(text.contains("elapsed_ms:"));
                            assert_eq!(text.len() as u64, end);
                            "closed"
                        }
                        ProcessUpdate::Exited { .. } => "exited",
                        _ => return,
                    };
                    received.lock().unwrap().push(name);
                })),
            })
            .await
            .unwrap();
        assert!(!output.success);
        assert_eq!(
            *events.lock().unwrap(),
            ["started", "opened", "closed", "exited"]
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn isolated_windows_exit_status_preserves_signed_and_hex_codes() {
        // Controlled helper exits, not a reproduction of COLMAP heap corruption.
        for (code, status, abnormal) in [
            (1, "0x00000001", false),
            (101, "0x00000065", false),
            (-1073740940, "0xC0000374", true),
        ] {
            let output = ProcessManager::new()
                .run(ProcessSpec {
                    executable: system_executable("cmd.exe"),
                    args: vec!["/D".into(), "/C".into(), format!("exit /B {code}").into()],
                    working_directory: None,
                    log_path: None,
                    observer: None,
                })
                .await
                .unwrap();
            assert!(!output.success);
            assert_eq!(output.exit_code, Some(code));
            assert_eq!(output.termination.exit_code, Some(code));
            assert_eq!(output.termination.windows_status.as_deref(), Some(status));
            assert_eq!(output.termination.abnormal(), abnormal);
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unix_signal_exit_retains_signal_without_inventing_oom() {
        let output = ProcessManager::new()
            .run(ProcessSpec {
                executable: PathBuf::from("/bin/sh"),
                args: vec!["-c".into(), "kill -TERM $$".into()],
                working_directory: None,
                log_path: None,
                observer: None,
            })
            .await
            .unwrap();
        assert_eq!(output.termination.exit_code, None);
        assert_eq!(output.termination.signal, Some(libc::SIGTERM));
        assert_eq!(output.termination.core_dumped, Some(false));
        assert!(output.termination.abnormal());
    }

    #[tokio::test]
    async fn known_cancellation_does_not_launch_or_report_a_failure() {
        let manager = ProcessManager::new();
        manager.cancel();
        assert!(matches!(
            manager
                .run(ProcessSpec {
                    executable: PathBuf::from("must-not-be-launched"),
                    args: vec![],
                    working_directory: None,
                    log_path: None,
                    observer: None,
                })
                .await,
            Err(SplatError::Cancelled)
        ));
    }

    #[tokio::test]
    async fn bounded_capture_preserves_all_observer_lines() {
        let lines = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = lines.clone();
        let output = pump_stream(
            &b"frame=0\nframe=1\nframe=2\n"[..],
            ProcessStream::Stdout,
            Some(Arc::new(move |update| {
                if let ProcessUpdate::Line { line, .. } = update {
                    seen.lock().unwrap().push(line);
                }
            })),
            None,
            Some(8),
        )
        .await;
        assert_eq!(output.unwrap(), b"frame=2\n");
        assert_eq!(lines.lock().unwrap().len(), 3);
    }

    #[test]
    fn failure_detail_prefers_stderr_and_keeps_only_a_bounded_tail() {
        let output = ProcessOutput {
            success: false,
            exit_code: Some(1),
            termination: ProcessTermination::default(),
            stdout: "less useful stdout".into(),
            stderr: format!("{}early eof", "x".repeat(5_000)),
        };
        let detail = output.failure_detail();
        assert!(detail.ends_with("early eof"));
        assert_eq!(detail.chars().count(), 4_096);
        assert!(!detail.contains("less useful stdout"));
    }

    #[tokio::test]
    async fn child_environment_overrides_do_not_mutate_the_parent() {
        const KEY: &str = "OOOSPLAT_PROCESS_ENV_ISOLATION_TEST";
        let parent_value = std::env::var_os(KEY);
        let environment = vec![(OsString::from(KEY), OsString::from("child-only"))];

        #[cfg(windows)]
        let spec = ProcessSpec {
            executable: system_executable("cmd.exe"),
            args: vec!["/C".into(), format!("echo %{KEY}%").into()],
            working_directory: None,
            log_path: None,
            observer: None,
        };
        #[cfg(unix)]
        let spec = ProcessSpec {
            executable: PathBuf::from("/bin/sh"),
            args: vec!["-c".into(), format!("printf '%s' \"${KEY}\"").into()],
            working_directory: None,
            log_path: None,
            observer: None,
        };

        let output = ProcessManager::new()
            .run_with_environment(spec, &environment)
            .await
            .unwrap();
        assert!(output.success);
        assert_eq!(output.stdout.trim(), "child-only");
        assert_eq!(std::env::var_os(KEY), parent_value);
    }

    #[cfg(windows)]
    fn system_executable(name: &str) -> PathBuf {
        PathBuf::from(std::env::var_os("SystemRoot").expect("SystemRoot"))
            .join("System32")
            .join(name)
    }

    #[cfg(windows)]
    fn process_has_exited(process_id: u32) -> bool {
        use windows_sys::Win32::{
            Foundation::{CloseHandle, WAIT_OBJECT_0},
            System::Threading::{OpenProcess, WaitForSingleObject},
        };
        // SAFETY: The process handle is opened only for synchronization, checked for null,
        // observed without blocking, and closed exactly once.
        unsafe {
            const SYNCHRONIZE_ACCESS: u32 = 0x0010_0000;
            let process = OpenProcess(SYNCHRONIZE_ACCESS, 0, process_id);
            if process.is_null() {
                return true;
            }
            let result = WaitForSingleObject(process, 0) == WAIT_OBJECT_0;
            CloseHandle(process);
            result
        }
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn suspended_windows_process_is_resumed_after_job_assignment() {
        let output = ProcessManager::new()
            .run(ProcessSpec {
                executable: system_executable("cmd.exe"),
                args: vec!["/C".into(), "echo resumed".into()],
                working_directory: None,
                log_path: None,
                observer: None,
            })
            .await
            .unwrap();
        assert!(output.success);
        assert!(output.stdout.contains("resumed"));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn cancellation_terminates_a_windows_descendant_created_immediately() {
        let descendant_pid = Arc::new(std::sync::Mutex::new(None::<u32>));
        let observer: ProcessObserver = {
            let descendant_pid = descendant_pid.clone();
            Arc::new(move |update| {
                if let ProcessUpdate::Line { line, .. } = update {
                    if let Ok(pid) = line.parse() {
                        *descendant_pid.lock().unwrap() = Some(pid);
                    }
                }
            })
        };
        let manager = ProcessManager::new();
        let running_manager = manager.clone();
        let run = tokio::spawn(async move {
            running_manager
                .run(ProcessSpec {
                    executable: system_executable("WindowsPowerShell\\v1.0\\powershell.exe"),
                    args: vec![
                        "-NoProfile".into(),
                        "-Command".into(),
                        "$p = Start-Process -PassThru -WindowStyle Hidden ping -ArgumentList '-t','127.0.0.1'; $p.Id; Wait-Process -Id $p.Id".into(),
                    ],
                    working_directory: None,
                    log_path: None,
                    observer: Some(observer),
                })
                .await
        });
        for _ in 0..100 {
            if descendant_pid.lock().unwrap().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let process_id = descendant_pid.lock().unwrap().expect("descendant PID");
        manager.cancel();
        assert!(matches!(run.await.unwrap(), Err(SplatError::Cancelled)));
        for _ in 0..100 {
            if process_has_exited(process_id) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("descendant {process_id} escaped the Windows Job Object");
    }

    #[tokio::test]
    async fn streams_stdout_and_stderr_concurrently_and_persists_log() {
        let directory = tempfile::tempdir().unwrap();
        let log_path = directory.path().join("process.log");
        let log = Arc::new(Mutex::new(
            tokio::fs::File::create(&log_path).await.unwrap(),
        ));
        let updates = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let observer: ProcessObserver = {
            let updates = updates.clone();
            Arc::new(move |update| {
                if let ProcessUpdate::Line { line, .. } = update {
                    updates.lock().unwrap().push(line);
                }
            })
        };
        let (mut stdout_writer, stdout_reader) = tokio::io::duplex(256);
        let (mut stderr_writer, stderr_reader) = tokio::io::duplex(256);
        let stdout_task = tokio::spawn(pump_stream(
            stdout_reader,
            ProcessStream::Stdout,
            Some(observer.clone()),
            Some(log.clone()),
            None,
        ));
        let stderr_task = tokio::spawn(pump_stream(
            stderr_reader,
            ProcessStream::Stderr,
            Some(observer),
            Some(log.clone()),
            None,
        ));
        stdout_writer
            .write_all(b"frame=1\nframe=2\n")
            .await
            .unwrap();
        stderr_writer.write_all(b"warning line\n").await.unwrap();
        drop(stdout_writer);
        drop(stderr_writer);
        stdout_task.await.unwrap().unwrap();
        stderr_task.await.unwrap().unwrap();
        log.lock().await.sync_all().await.unwrap();

        let captured = updates.lock().unwrap().clone();
        assert_eq!(captured.len(), 3);
        assert!(captured.contains(&"frame=1".to_string()));
        assert!(captured.contains(&"warning line".to_string()));
        let disk = tokio::fs::read_to_string(log_path).await.unwrap();
        assert!(disk.contains("frame=2"));
        assert!(disk.contains("warning line"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancellation_terminates_descendant_processes() {
        let descendant_pid = Arc::new(std::sync::Mutex::new(None::<u32>));
        let observer: ProcessObserver = {
            let descendant_pid = descendant_pid.clone();
            Arc::new(move |update| {
                if let ProcessUpdate::Line { line, .. } = update {
                    if let Ok(pid) = line.parse() {
                        *descendant_pid.lock().unwrap() = Some(pid);
                    }
                }
            })
        };
        let manager = ProcessManager::new();
        let running_manager = manager.clone();
        let run = tokio::spawn(async move {
            running_manager
                .run(ProcessSpec {
                    executable: PathBuf::from("/bin/sh"),
                    args: vec!["-c".into(), "sleep 30 & echo $!; wait".into()],
                    working_directory: None,
                    log_path: None,
                    observer: Some(observer),
                })
                .await
        });

        for _ in 0..50 {
            if descendant_pid.lock().unwrap().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let pid = descendant_pid.lock().unwrap().expect("descendant PID");
        manager.cancel();
        assert!(matches!(run.await.unwrap(), Err(SplatError::Cancelled)));

        let mut terminated = false;
        for _ in 0..150 {
            if descendant_is_terminated(pid) {
                terminated = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            terminated,
            "descendant {pid} was still running after cancellation"
        );
    }

    /// A killed descendant is reparented to PID 1, and PID 1 does not reap in every
    /// container, so the process can linger as a zombie with its `/proc` entry intact.
    /// The entry being gone and the entry being a zombie both mean the process stopped
    /// running, which is what cancellation has to guarantee.
    #[cfg(target_os = "linux")]
    fn descendant_is_terminated(pid: u32) -> bool {
        let Ok(status) = std::fs::read_to_string(format!("/proc/{pid}/status")) else {
            return true;
        };
        status
            .lines()
            .find_map(|line| line.strip_prefix("State:"))
            .map(|state| state.trim_start().starts_with('Z'))
            .unwrap_or(true)
    }

    /// Unix targets without `/proc` reap orphans through PID 1, so a terminated
    /// descendant disappears outright rather than lingering as a visible zombie.
    #[cfg(all(unix, not(target_os = "linux")))]
    fn descendant_is_terminated(pid: u32) -> bool {
        // SAFETY: signal 0 only runs the existence and permission checks for the PID
        // and delivers nothing, so no process state is observed or modified.
        let alive = unsafe { libc::kill(pid as libc::pid_t, 0) } == 0;
        !alive
    }
}
