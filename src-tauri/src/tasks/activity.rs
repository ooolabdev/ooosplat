//! Local, best-effort activity retention. Raw engine output remains in its own logs.
use super::logs::{self, RegisteredLog};
use crate::pipeline::{EventKind, PipelineEvent};
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncWriteExt, BufWriter},
    sync::{mpsc, oneshot},
};
use uuid::Uuid;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RunHeader {
    pub activity_version: u32,
    pub task_id: Uuid,
    pub run_id: Uuid,
    pub project_id: Option<Uuid>,
    pub started_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RunFooter {
    pub activity_closed: bool,
    pub run_id: Uuid,
    pub complete: bool,
}

const MAX_PENDING_BYTES: usize = 512 * 1024;
const MAX_LINE_BYTES: usize = 64 * 1024;
type Registered = Arc<dyn Fn(RegisteredLog) + Send + Sync>;

enum Message {
    Line(Vec<u8>),
    Close(oneshot::Sender<()>),
}
struct State {
    receiver: Option<mpsc::Receiver<Message>>,
    log: Option<RegisteredLog>,
}

#[derive(Clone)]
pub(super) struct Journal {
    sender: mpsc::Sender<Message>,
    state: Arc<Mutex<State>>,
    queued_bytes: Arc<AtomicUsize>,
    healthy: Arc<AtomicBool>,
    closing: Arc<AtomicBool>,
}

impl Journal {
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::channel(256);
        Self {
            sender,
            state: Arc::new(Mutex::new(State {
                receiver: Some(receiver),
                log: None,
            })),
            queued_bytes: Arc::new(AtomicUsize::new(0)),
            healthy: Arc::new(AtomicBool::new(true)),
            closing: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn push(&self, event: &PipelineEvent) {
        if self.closing.load(Ordering::Acquire)
            || matches!(event.kind, EventKind::Runtime | EventKind::Heartbeat)
        {
            return;
        }
        let mut event = event.clone();
        event.runtime = None;
        event.acceleration = None;
        let Ok(mut bytes) = serde_json::to_vec(&event) else {
            self.healthy.store(false, Ordering::Release);
            return;
        };
        bytes.push(b'\n');
        if bytes.len() > MAX_LINE_BYTES
            || self
                .queued_bytes
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                    current
                        .checked_add(bytes.len())
                        .filter(|n| *n <= MAX_PENDING_BYTES)
                })
                .is_err()
        {
            self.healthy.store(false, Ordering::Release);
            return;
        }
        let size = bytes.len();
        if self.sender.try_send(Message::Line(bytes)).is_err() {
            self.queued_bytes.fetch_sub(size, Ordering::AcqRel);
            self.healthy.store(false, Ordering::Release);
        }
    }

    pub fn bind(&self, project: PathBuf, header: RunHeader, registered: Registered) {
        let receiver = self
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .receiver
            .take();
        let Some(receiver) = receiver else {
            return;
        };
        let journal = self.clone();
        tokio::spawn(async move {
            if let Err(error) = journal.write(project, header, receiver, registered).await {
                journal.healthy.store(false, Ordering::Release);
                tracing::warn!(error_kind = ?error.kind(), "Could not retain task activity log");
            }
        });
    }

    async fn write(
        &self,
        project: PathBuf,
        header: RunHeader,
        mut receiver: mpsc::Receiver<Message>,
        registered: Registered,
    ) -> std::io::Result<()> {
        let project = tokio::fs::canonicalize(project).await?;
        let root = project.join("logs");
        match tokio::fs::symlink_metadata(&root).await {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(std::io::ErrorKind::PermissionDenied.into());
            }
            Ok(_) => (),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error),
        }
        tokio::fs::create_dir_all(&root).await?;
        let root = tokio::fs::canonicalize(root).await?;
        if root.parent() != Some(project.as_path()) {
            return Err(std::io::ErrorKind::PermissionDenied.into());
        }
        let path = root.join(format!("activity-{}.log", header.run_id));
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .await?;
        let mut line = serde_json::to_vec(&header).map_err(std::io::Error::other)?;
        line.push(b'\n');
        file.write_all(&line).await?;
        let mut log = logs::register(&root, &path, 0).map_err(std::io::Error::other)?;
        log.activity = true;
        log.activity_complete = self.healthy.load(Ordering::Acquire);
        self.state.lock().unwrap_or_else(|p| p.into_inner()).log = Some(log.clone());
        registered(log.clone());
        let mut file = BufWriter::with_capacity(64 * 1024, file);
        let mut interval = tokio::time::interval(Duration::from_millis(250));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = interval.tick() => { file.flush().await?; }
                message = receiver.recv() => match message {
                    Some(Message::Line(bytes)) => {
                        self.queued_bytes.fetch_sub(bytes.len(), Ordering::AcqRel);
                        file.write_all(&bytes).await?;
                    }
                    Some(Message::Close(done)) => {
                        let footer = RunFooter { activity_closed: true, run_id: header.run_id, complete: self.healthy.load(Ordering::Acquire) };
                        let mut line = serde_json::to_vec(&footer).map_err(std::io::Error::other)?;
                        line.push(b'\n'); file.write_all(&line).await?;
                        file.flush().await?;
                        file.get_ref().sync_all().await?;
                        log.end = Some(file.get_ref().metadata().await?.len());
                        log.activity_complete = self.healthy.load(Ordering::Acquire);
                        self.state.lock().unwrap_or_else(|p| p.into_inner()).log = Some(log.clone());
                        registered(log);
                        let _ = done.send(());
                        return Ok(());
                    }
                    None => { file.flush().await?; return Ok(()); }
                }
            }
        }
    }

    pub fn snapshot(&self) -> Option<RegisteredLog> {
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .log
            .clone()
            .map(|mut log| {
                log.activity_complete &= self.healthy.load(Ordering::Acquire);
                log
            })
    }

    pub async fn close(&self) -> Option<RegisteredLog> {
        self.closing.store(true, Ordering::Release);
        if self
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .receiver
            .take()
            .is_some()
        {
            self.healthy.store(false, Ordering::Release);
            return None;
        }
        let (done, finished) = oneshot::channel();
        let flushed = tokio::time::timeout(Duration::from_secs(5), async {
            self.sender
                .send(Message::Close(done))
                .await
                .map_err(|_| ())?;
            finished.await.map_err(|_| ())
        })
        .await;
        if !matches!(flushed, Ok(Ok(()))) {
            self.healthy.store(false, Ordering::Release);
        }
        self.snapshot()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::{EventLevel, PipelineStage};

    #[tokio::test]
    async fn retains_more_than_the_ui_window_in_order_and_closes_the_owned_run() {
        let root = tempfile::tempdir().unwrap();
        let journal = Journal::new();
        let task = Uuid::new_v4();
        let run = Uuid::new_v4();
        journal.bind(
            root.path().into(),
            RunHeader {
                activity_version: 1,
                task_id: task,
                run_id: run,
                project_id: None,
                started_at: chrono::Utc::now(),
            },
            Arc::new(|_| {}),
        );
        for sequence in 1..=620 {
            let mut event =
                PipelineEvent::mapped(PipelineStage::Matching, 0.0, format!("activity {sequence}"));
            event.task_id = Some(task);
            event.run_id = Some(run);
            event.sequence = sequence;
            event.kind = EventKind::Log;
            event.level = EventLevel::Info;
            journal.push(&event);
            if sequence % 50 == 0 {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        }
        let log = journal.close().await.unwrap();
        assert!(log.activity_complete);
        let bytes = tokio::fs::read(&log.path).await.unwrap();
        let events = bytes
            .split(|b| *b == b'\n')
            .filter_map(|line| serde_json::from_slice::<PipelineEvent>(line).ok())
            .collect::<Vec<_>>();
        assert_eq!(events.len(), 620);
        assert_eq!(events[0].sequence, 1);
        assert_eq!(events[619].sequence, 620);
        assert_eq!(log.end, Some(bytes.len() as u64));
    }

    #[tokio::test]
    async fn queue_overflow_and_unwritable_roots_are_best_effort_not_generation_failures() {
        let journal = Journal::new();
        let mut event = PipelineEvent::mapped(PipelineStage::Matching, 0.0, "x".repeat(5000));
        event.kind = EventKind::Log;
        for _ in 0..1000 {
            journal.push(&event);
        }
        assert!(journal.queued_bytes.load(Ordering::Acquire) <= MAX_PENDING_BYTES);
        assert!(!journal.healthy.load(Ordering::Acquire));
        assert!(journal.close().await.is_none());
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("not-a-directory");
        tokio::fs::write(&project, b"keep").await.unwrap();
        let journal = Journal::new();
        journal.bind(
            project.clone(),
            RunHeader {
                activity_version: 1,
                task_id: Uuid::new_v4(),
                run_id: Uuid::new_v4(),
                project_id: None,
                started_at: chrono::Utc::now(),
            },
            Arc::new(|_| {}),
        );
        journal.push(&event);
        assert!(journal.close().await.is_none());
        assert_eq!(tokio::fs::read(project).await.unwrap(), b"keep");
    }

    #[cfg(any(windows, unix))]
    #[tokio::test]
    async fn redirected_log_directories_are_rejected_even_inside_the_project() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project");
        let destination = project.join("other-data");
        std::fs::create_dir_all(&destination).unwrap();
        #[cfg(windows)]
        let link = std::os::windows::fs::symlink_dir(&destination, project.join("logs"));
        #[cfg(unix)]
        let link = std::os::unix::fs::symlink(&destination, project.join("logs"));
        if let Err(error) = link {
            if error.kind() == std::io::ErrorKind::PermissionDenied
                || error.raw_os_error() == Some(1314)
            {
                eprintln!("activity log symlink branch not exercised: {error}");
                return;
            }
            panic!("could not create the owned symlink fixture: {error}");
        }
        let journal = Journal::new();
        journal.bind(
            project,
            RunHeader {
                activity_version: 1,
                task_id: Uuid::new_v4(),
                run_id: Uuid::new_v4(),
                project_id: None,
                started_at: chrono::Utc::now(),
            },
            Arc::new(|_| {}),
        );
        assert!(journal.close().await.is_none());
        assert_eq!(std::fs::read_dir(destination).unwrap().count(), 0);
    }
}
