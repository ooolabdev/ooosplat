//! Caspar is an optional BA accelerator, independent of SIFT GPU support.
use crate::process::{ProcessManager, ProcessOutput, ProcessSpec};
use std::{
    collections::HashMap,
    ffi::OsString,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
    time::Duration,
};
use tokio::sync::{Mutex, OnceCell};

#[derive(Clone, Debug)]
pub struct BaSelection {
    pub gpu_index: Option<u32>,
    pub detail: String,
}

type ProbeCache = Mutex<HashMap<(PathBuf, u32), Arc<OnceCell<BaSelection>>>>;
static CACHE: OnceLock<ProbeCache> = OnceLock::new();

pub async fn select(executable: &Path, compatible_gpu: Option<u32>) -> BaSelection {
    let Some(index) = compatible_gpu.filter(|_| !cfg!(target_os = "macos")) else {
        return ceres("no compatible Caspar GPU (or macOS CPU build)");
    };
    let key = (
        std::fs::canonicalize(executable).unwrap_or_else(|_| executable.into()),
        index,
    );
    let cell = CACHE
        .get_or_init(Default::default)
        .lock()
        .await
        .entry(key)
        .or_default()
        .clone();
    cell.get_or_init(|| probe(executable, index)).await.clone()
}

fn ceres(reason: &str) -> BaSelection {
    BaSelection {
        gpu_index: None,
        detail: format!("BA: Ceres CPU; {reason}"),
    }
}

async fn bounded_command(executable: &Path, args: Vec<OsString>) -> Result<ProcessOutput, String> {
    let manager = ProcessManager::new();
    let environment = [
        ("GLOG_v".into(), "2".into()),
        ("GLOG_logtostderr".into(), "1".into()),
    ];
    let run = manager.run_with_environment(
        ProcessSpec {
            executable: executable.into(),
            args,
            working_directory: executable.parent().map(Path::to_path_buf),
            log_path: None,
            observer: None,
        },
        &environment,
    );
    tokio::pin!(run);
    tokio::select! {
        output = &mut run => output.map_err(|error| error.to_string()),
        _ = tokio::time::sleep(Duration::from_secs(15)) => {
            // Let ProcessManager reap the child/process tree before deleting its fixture.
            manager.cancel();
            let _ = run.await;
            Err("Caspar probe timed out".into())
        }
    }
}

async fn probe(executable: &Path, index: u32) -> BaSelection {
    let result = async {
        let help = bounded_command(executable, vec!["bundle_adjuster".into(), "-h".into()]).await?;
        if !help.success
            || !format!("{}{}", help.stdout, help.stderr)
                .contains("--BundleAdjustmentCaspar.gpu_index")
        {
            return Err("Caspar was not compiled into COLMAP".into());
        }
        let directory = ProbeDirectory(
            std::env::temp_dir().join(format!("ooosplat-caspar-{}", uuid::Uuid::new_v4())),
        );
        let input = directory.0.join("input");
        let output = directory.0.join("output");
        tokio::fs::create_dir_all(&input)
            .await
            .map_err(|e| e.to_string())?;
        tokio::fs::create_dir_all(&output)
            .await
            .map_err(|e| e.to_string())?;
        for (name, contents) in fixture() {
            tokio::fs::write(input.join(name), contents)
                .await
                .map_err(|e| e.to_string())?;
        }
        let run = bounded_command(
            executable,
            vec![
                "bundle_adjuster".into(),
                "--input_path".into(),
                input.into_os_string(),
                "--output_path".into(),
                output.clone().into_os_string(),
                "--BundleAdjustment.backend".into(),
                "CASPAR".into(),
                "--log_level".into(),
                "2".into(),
                "--BundleAdjustmentCaspar.gpu_index".into(),
                index.to_string().into(),
            ],
        )
        .await?;
        if !run.success
            || !output.join("points3D.bin").is_file()
            || !format!("{}{}", run.stdout, run.stderr).contains("Creating Caspar bundle adjuster")
        {
            return Err(format!("Caspar execution failed: {}", run.failure_detail()));
        }
        Ok(())
    }
    .await;
    match result {
        Ok(()) => BaSelection {
            gpu_index: Some(index),
            detail: format!("BA: Caspar GPU {index}; execution probe passed"),
        },
        Err(reason) => ceres(&reason),
    }
}

struct ProbeDirectory(PathBuf);
impl Drop for ProbeDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// Non-planar, shared SIMPLE_RADIAL model; no image files or database are needed
// by bundle_adjuster. Text-model legacy loading creates trivial rigs/frames.
fn fixture() -> [(&'static str, String); 3] {
    let points = [
        (-0.5, -0.3, 3.0),
        (0.4, -0.2, 3.5),
        (-0.2, 0.4, 4.0),
        (0.5, 0.3, 4.5),
        (0.0, -0.5, 5.0),
        (-0.4, 0.1, 5.5),
        (0.3, 0.0, 3.8),
        (0.1, 0.5, 4.8),
    ];
    let mut images = String::new();
    for id in 1..=3 {
        let tx = (id - 1) as f64 * 0.25;
        images.push_str(&format!("{id} 1 0 0 0 {tx} 0 0 1 probe{id}.png\n"));
        for (i, (x, y, z)) in points.iter().enumerate() {
            images.push_str(&format!(
                "{} {} {} ",
                500.0 * (x + tx) / z + 320.0,
                500.0 * y / z + 240.0,
                i + 1
            ));
        }
        images.push('\n');
    }
    let mut model_points = String::new();
    for (i, (x, y, z)) in points.iter().enumerate() {
        model_points.push_str(&format!(
            "{} {} {y} {z} 128 128 128 0 1 {i} 2 {i} 3 {i}\n",
            i + 1,
            x + 0.002
        ));
    }
    [
        (
            "cameras.txt",
            "1 SIMPLE_RADIAL 640 480 500 320 240 0\n".into(),
        ),
        ("images.txt", images),
        ("points3D.txt", model_points),
    ]
}

pub fn append_mapper_options(args: &mut Vec<OsString>, selection: &BaSelection) {
    // Ceres CPU is the upstream default, also preserving older developer CLIs.
    if let Some(index) = selection.gpu_index {
        args.extend([
            "--Mapper.ba_local_backend".into(),
            "CASPAR".into(),
            "--Mapper.ba_global_backend".into(),
            "CASPAR".into(),
            "--Mapper.ba_gpu_index".into(),
            index.to_string().into(),
            "--log_level".into(),
            "2".into(),
        ]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn caspar_sets_both_backends_without_changing_iterations() {
        let mut args = vec![];
        append_mapper_options(
            &mut args,
            &BaSelection {
                gpu_index: Some(2),
                detail: String::new(),
            },
        );
        assert_eq!(
            args,
            [
                "--Mapper.ba_local_backend",
                "CASPAR",
                "--Mapper.ba_global_backend",
                "CASPAR",
                "--Mapper.ba_gpu_index",
                "2",
                "--log_level",
                "2"
            ]
            .map(OsString::from)
        );
        append_mapper_options(&mut args, &ceres("probe failed"));
        assert_eq!(args.len(), 8);
    }
    #[tokio::test]
    async fn cpu_selection_does_not_start_colmap() {
        let result = select(Path::new("missing-colmap"), None).await;
        assert_eq!(result.gpu_index, None);
        assert!(result.detail.contains("Ceres CPU"));
    }
    #[cfg(not(target_os = "macos"))]
    #[tokio::test]
    async fn failed_probe_is_cached_and_does_not_change_the_sift_device() {
        let executable =
            std::env::temp_dir().join(format!("missing-colmap-{}", uuid::Uuid::new_v4()));
        let sift_device = Some(2);
        let (first, second) = tokio::join!(
            select(&executable, sift_device),
            select(&executable, sift_device)
        );
        assert_eq!(first.gpu_index, None);
        assert_eq!(first.detail, second.detail);
        assert!(first.detail.contains("Ceres CPU"));
        assert_eq!(sift_device, Some(2));
        let cache = CACHE.get().unwrap().lock().await;
        assert!(cache.get(&(executable, 2)).unwrap().get().is_some());
    }
    #[test]
    fn fixture_has_observations_and_bidirectional_tracks() {
        let files = fixture();
        assert!(files[0].1.contains("SIMPLE_RADIAL"));
        assert_eq!(files[1].1.lines().count(), 6);
        assert_eq!(files[2].1.lines().count(), 8);
    }
}
