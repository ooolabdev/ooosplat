use std::{
    path::PathBuf,
    process::Stdio,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

use super::{ProcessObserver, ProcessUpdate};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuResources {
    pub uuid: String,
    pub name: String,
    pub utilization_percent: Option<f32>,
    pub memory_used_mib: Option<u64>,
    pub memory_total_mib: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessResources {
    pub process_id: u32,
    pub sampled_at: DateTime<Utc>,
    pub cpu_percent: Option<f32>,
    pub memory_bytes: Option<u64>,
    pub gpus: Vec<GpuResources>,
    pub gpu_status: String,
}

struct Collector {
    system: System,
    pid: Pid,
    initialized: bool,
    cpu_count: usize,
}

impl Collector {
    fn new(pid: u32) -> Self {
        let mut system = System::new();
        system.refresh_cpu_all();
        let cpu_count = system.cpus().len().max(1);
        Self {
            system,
            pid: Pid::from_u32(pid),
            initialized: false,
            cpu_count,
        }
    }

    fn sample(&mut self) -> ProcessResources {
        self.system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[self.pid]),
            true,
            ProcessRefreshKind::nothing().with_cpu().with_memory(),
        );
        let process = self.system.process(self.pid);
        let cpu_percent = process
            .filter(|_| self.initialized)
            .map(|process| (process.cpu_usage() / self.cpu_count as f32).clamp(0.0, 100.0));
        self.initialized = process.is_some();
        ProcessResources {
            process_id: self.pid.as_u32(),
            sampled_at: Utc::now(),
            cpu_percent,
            memory_bytes: process.map(|process| process.memory()),
            gpus: Vec::new(),
            gpu_status: "unavailable".into(),
        }
    }
}

pub(super) struct SamplingTask {
    task: tokio::task::JoinHandle<()>,
    stopped: Arc<AtomicBool>,
}

impl Drop for SamplingTask {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        self.task.abort();
    }
}

impl SamplingTask {
    pub(super) async fn stop(mut self) {
        self.stopped.store(true, Ordering::Release);
        self.task.abort();
        let _ = (&mut self.task).await;
    }
}

pub(super) fn spawn(pid: u32, observer: ProcessObserver) -> SamplingTask {
    let stopped = Arc::new(AtomicBool::new(false));
    let stop = stopped.clone();
    let task = tokio::spawn(async move {
        let Ok(mut collector) = tokio::task::spawn_blocking(move || Collector::new(pid)).await
        else {
            return;
        };
        #[cfg(not(target_os = "macos"))]
        let smi = crate::engines::health::nvidia_smi_candidates()
            .into_iter()
            .find(|path| path.is_file());
        #[cfg(target_os = "macos")]
        let smi: Option<PathBuf> = None;
        let mut interval = tokio::time::interval(Duration::from_secs(2));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        while !stop.load(Ordering::Acquire) {
            interval.tick().await;
            let collected = tokio::task::spawn_blocking(move || {
                let sample = collector.sample();
                (collector, sample)
            })
            .await;
            let Ok((next, mut sample)) = collected else {
                break;
            };
            collector = next;
            if let Some(path) = &smi {
                match query_gpus(path).await {
                    Some(gpus) => {
                        sample.gpus = gpus;
                        sample.gpu_status = "available".into();
                    }
                    None => sample.gpu_status = "unavailable".into(),
                }
            } else {
                sample.gpu_status = "unsupported".into();
            }
            if !stop.load(Ordering::Acquire) {
                observer(ProcessUpdate::Resources(sample));
            }
        }
    });
    SamplingTask { task, stopped }
}

async fn query_gpus(path: &PathBuf) -> Option<Vec<GpuResources>> {
    let mut command = tokio::process::Command::new(path);
    command
        .args([
            "--query-gpu=uuid,name,utilization.gpu,memory.used,memory.total",
            "--format=csv,noheader,nounits",
        ])
        .stdin(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    collect_gpu_output(command, Duration::from_secs(2)).await
}

async fn collect_gpu_output(
    mut command: tokio::process::Command,
    timeout: Duration,
) -> Option<Vec<GpuResources>> {
    command.kill_on_drop(true);
    let output = tokio::time::timeout(timeout, command.output())
        .await
        .ok()?
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_gpus(&String::from_utf8_lossy(&output.stdout))
}

fn parse_gpus(text: &str) -> Option<Vec<GpuResources>> {
    let mut gpus = Vec::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let fields: Vec<_> = line.split(',').map(str::trim).collect();
        if fields.len() < 5 || !fields[0].starts_with("GPU-") {
            return None;
        }
        let count = fields.len();
        gpus.push(GpuResources {
            uuid: fields[0].into(),
            name: fields[1..count - 3].join(", "),
            utilization_percent: fields[count - 3]
                .parse::<f32>()
                .ok()
                .filter(|value| value.is_finite() && (0.0..=100.0).contains(value)),
            memory_used_mib: fields[count - 2].parse().ok(),
            memory_total_mib: fields[count - 1].parse().ok(),
        });
    }
    (!gpus.is_empty()).then_some(gpus)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_multiple_cards_and_preserves_unavailable_fields() {
        let gpus =
            parse_gpus("GPU-a, RTX A, 25, 100, 8192\nGPU-b, RTX B, N/A, N/A, 4096\n").unwrap();
        assert_eq!(gpus.len(), 2);
        assert_eq!(gpus[0].utilization_percent, Some(25.0));
        assert_eq!(gpus[1].memory_used_mib, None);
        assert_eq!(gpus[1].utilization_percent, None);
        assert!(parse_gpus("broken").is_none());
        assert!(parse_gpus("").is_none());
    }

    #[test]
    fn first_cpu_sample_and_missing_pid_are_unknown() {
        let mut collector = Collector::new(std::process::id());
        let first = collector.sample();
        assert_eq!(first.cpu_percent, None);
        assert!(first.memory_bytes.is_some());
        let mut absent = Collector::new(u32::MAX);
        assert_eq!(absent.sample().memory_bytes, None);
        assert_eq!(absent.sample().cpu_percent, None);
    }

    #[tokio::test]
    async fn dropping_sampler_stops_output() {
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let received = count.clone();
        let task = spawn(
            std::process::id(),
            Arc::new(move |_| {
                received.fetch_add(1, Ordering::Relaxed);
            }),
        );
        drop(task);
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(count.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn query_timeout_and_failed_start_are_unavailable() {
        let missing = tokio::process::Command::new("ooosplat-missing-nvidia-smi-test");
        assert!(collect_gpu_output(missing, Duration::from_millis(10))
            .await
            .is_none());
    }
}
