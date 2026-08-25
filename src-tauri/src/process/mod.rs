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

#[cfg(windows)]
mod windows_job {
    use std::{io, mem::size_of, ptr};

    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE},
        System::{
            JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
                SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
                JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            },
            Threading::{OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE},
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
    Started { process_id: u32 },
    Line { stream: ProcessStream, line: String },
    Heartbeat { elapsed_ms: u64 },
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
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone, Default)]
pub struct ProcessManager {
    cancellation: CancellationToken,
}

impl ProcessManager {
    pub fn new() -> Self {
        Self {
            cancellation: CancellationToken::new(),
        }
    }

    pub fn cancel(&self) {
        self.cancellation.cancel();
    }

    pub fn child_token(&self) -> CancellationToken {
        self.cancellation.child_token()
    }

    pub async fn run(&self, spec: ProcessSpec) -> Result<ProcessOutput> {
        if !spec.executable.is_file() {
            return Err(SplatError::EngineMissing(
                spec.executable.display().to_string(),
            ));
        }

        let started = Instant::now();
        let mut command = Command::new(&spec.executable);
        command
            .args(&spec.args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(directory) = &spec.working_directory {
            command.current_dir(directory);
        }

        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
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
        if let Some(observer) = &spec.observer {
            observer(ProcessUpdate::Started { process_id });
        }

        #[cfg(windows)]
        let job = {
            let job = windows_job::WindowsJob::create().map_err(|error| {
                SplatError::Process(format!("无法创建 Windows Job Object：{error}"))
            })?;
            if let Err(error) = job.assign(process_id) {
                let _ = child.kill().await;
                return Err(SplatError::Process(format!(
                    "无法把子进程加入 Windows Job Object：{error}"
                )));
            }
            job
        };

        let log_file = if let Some(path) = &spec.log_path {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).await?;
            }
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .await?;
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
        ));
        let stderr_task = tokio::spawn(pump_stream(
            child.stderr.take().expect("stderr is piped"),
            ProcessStream::Stderr,
            spec.observer.clone(),
            log_file.clone(),
        ));
        let finished = Arc::new(AtomicBool::new(false));
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
        if let Some(task) = heartbeat_task {
            let _ = task.await;
        }

        let stdout = stdout_task
            .await
            .map_err(|error| SplatError::Process(error.to_string()))??;
        let stderr = stderr_task
            .await
            .map_err(|error| SplatError::Process(error.to_string()))??;
        let output = ProcessOutput {
            success: status.success(),
            exit_code: status.code(),
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
        };

        if let Some(file) = log_file {
            file.lock()
                .await
                .write_all(
                    format!(
                        "exit_code: {:?}\nelapsed_ms: {}\n\n",
                        output.exit_code,
                        started.elapsed().as_millis()
                    )
                    .as_bytes(),
                )
                .await?;
        }

        Ok(output)
    }
}

async fn pump_stream<R: AsyncRead + Unpin>(
    reader: R,
    stream: ProcessStream,
    observer: Option<ProcessObserver>,
    log: Option<Arc<Mutex<tokio::fs::File>>>,
) -> std::io::Result<Vec<u8>> {
    let mut reader = BufReader::new(reader);
    let mut collected = Vec::new();
    let mut line = Vec::new();
    loop {
        line.clear();
        let read = reader.read_until(b'\n', &mut line).await?;
        if read == 0 {
            break;
        }
        collected.extend_from_slice(&line);
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
    Ok(collected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

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
        ));
        let stderr_task = tokio::spawn(pump_stream(
            stderr_reader,
            ProcessStream::Stderr,
            Some(observer),
            Some(log.clone()),
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
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!PathBuf::from(format!("/proc/{pid}")).exists());
    }
}
