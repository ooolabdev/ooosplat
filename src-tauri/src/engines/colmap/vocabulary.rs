use crate::error::{Result, SplatError};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    path::{Path, PathBuf},
    sync::OnceLock,
};
use tokio_util::sync::CancellationToken;

#[derive(Deserialize)]
struct VocabularyIdentity {
    path: String,
    bytes: u64,
    sha256: String,
}

fn identity() -> &'static VocabularyIdentity {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct RuntimeLock {
        offline_vocabulary: VocabularyIdentity,
    }
    static IDENTITY: OnceLock<VocabularyIdentity> = OnceLock::new();
    IDENTITY.get_or_init(|| {
        serde_json::from_str::<RuntimeLock>(include_str!("../../../../engines/colmap-runtime.json"))
            .expect("bundled COLMAP vocabulary identity must be valid")
            .offline_vocabulary
    })
}

fn resource_error(detail: impl std::fmt::Display) -> SplatError {
    SplatError::Process(format!(
        "闭环检测所需的辅助文件缺失或损坏。请重新安装 OOOSplat，或关闭自动优化后重试。\n{detail}"
    ))
}

fn bundled_path(executable: &Path) -> Result<PathBuf> {
    let root = executable
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| resource_error("无法定位 COLMAP 资源目录"))?;
    // All locked runtimes place bin/colmap beside lib/colmap. This also covers
    // the older macOS engines/macos/arm64/bin layout without using PATH or cwd.
    std::path::absolute(root.join(&identity().path)).map_err(resource_error)
}

fn verify(
    path: &Path,
    expected: &VocabularyIdentity,
    cancellation: &CancellationToken,
) -> Result<()> {
    if cancellation.is_cancelled() {
        return Err(SplatError::Cancelled);
    }
    let mut file = std::fs::File::open(path).map_err(resource_error)?;
    let metadata = file.metadata().map_err(resource_error)?;
    if !metadata.is_file() || metadata.len() != expected.bytes {
        return Err(resource_error("词汇树文件大小与引擎锁不一致"));
    }
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 256 * 1024];
    let mut bytes = 0u64;
    loop {
        if cancellation.is_cancelled() {
            return Err(SplatError::Cancelled);
        }
        let count = file.read(&mut buffer).map_err(resource_error)?;
        if count == 0 {
            break;
        }
        bytes += count as u64;
        if bytes > expected.bytes {
            return Err(resource_error("词汇树文件在读取期间发生变化"));
        }
        hasher.update(&buffer[..count]);
    }
    if bytes != expected.bytes || format!("{:x}", hasher.finalize()) != expected.sha256 {
        return Err(resource_error("词汇树 SHA256 与引擎锁不一致"));
    }
    if cancellation.is_cancelled() {
        return Err(SplatError::Cancelled);
    }
    Ok(())
}

pub(super) async fn prepare(executable: &Path, cancellation: CancellationToken) -> Result<PathBuf> {
    #[cfg(feature = "e2e")]
    if crate::testing::enabled() {
        return crate::testing::engines_root().map(|root| root.join("vocabulary.bin"));
    }
    if cancellation.is_cancelled() {
        return Err(SplatError::Cancelled);
    }
    let path = bundled_path(executable)?;
    let path_to_check = path.clone();
    let worker_token = cancellation.clone();
    // Hash only when matching actually needs this resource. Do not add file IO
    // to background engine/GPU probes or block the async executor with 69 MiB.
    let worker =
        tokio::task::spawn_blocking(move || verify(&path_to_check, identity(), &worker_token));
    tokio::select! {
        biased;
        _ = cancellation.cancelled() => Err(SplatError::Cancelled),
        result = worker => {
            result.map_err(resource_error)??;
            Ok(path)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_paths_follow_the_executable_on_all_platforms() {
        let directory = tempfile::tempdir().unwrap();
        for layout in [
            "engines/colmap/bin/colmap.exe",
            "engines/linux/colmap/bin/colmap",
            "engines/macos/arm64/colmap/bin/colmap",
            "engines/macos/arm64/bin/colmap",
        ] {
            let executable = directory.path().join("中文 工程 with spaces").join(layout);
            let path = bundled_path(&executable).unwrap();
            assert!(path.is_absolute());
            assert_eq!(
                path,
                executable
                    .parent()
                    .unwrap()
                    .parent()
                    .unwrap()
                    .join(&identity().path)
            );
        }
        assert_eq!(identity().sha256.len(), 64);
        assert_eq!(identity().bytes, 72_412_636);
    }

    #[test]
    fn validation_rejects_missing_truncated_and_same_size_corruption_without_large_allocations() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("tree.bin");
        let data = b"test vocabulary";
        let expected = VocabularyIdentity {
            path: "tree.bin".into(),
            bytes: data.len() as u64,
            sha256: format!("{:x}", Sha256::digest(data)),
        };
        let cancellation = CancellationToken::new();
        assert!(verify(&path, &expected, &cancellation)
            .unwrap_err()
            .to_string()
            .contains("关闭自动优化"));
        std::fs::write(&path, data).unwrap();
        verify(&path, &expected, &cancellation).unwrap();
        std::fs::write(&path, &data[..data.len() - 1]).unwrap();
        assert!(verify(&path, &expected, &cancellation).is_err());
        std::fs::write(&path, vec![0; data.len()]).unwrap();
        assert!(verify(&path, &expected, &cancellation).is_err());
    }

    #[tokio::test]
    async fn cancelled_validation_is_cancelled_not_a_resource_error() {
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        assert!(matches!(
            prepare(Path::new("missing/bin/colmap.exe"), cancellation.clone()).await,
            Err(SplatError::Cancelled)
        ));
        assert!(matches!(
            verify(Path::new("missing"), identity(), &cancellation),
            Err(SplatError::Cancelled)
        ));
    }
}
