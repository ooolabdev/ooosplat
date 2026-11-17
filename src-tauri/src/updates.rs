//! Notification-only update checks. No installer download, telemetry, or URL opener.
use std::{
    collections::BTreeMap,
    path::PathBuf,
    time::{Duration, Instant},
};

use chrono::{DateTime, Utc};
use reqwest::{redirect::Policy, Url};
use semver::Version;
use serde::{Deserialize, Serialize};
use tokio::{io::AsyncReadExt, sync::Mutex};

const ENDPOINT: &str = "https://ooolab.cn/updates/latest.json";
const MAX_MANIFEST_BYTES: usize = 64 * 1024;
const MAX_CACHE_BYTES: usize = MAX_MANIFEST_BYTES * 2;
const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const CHECK_COOLDOWN: Duration = Duration::from_secs(30);
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    schema_version: u32,
    releases: BTreeMap<String, UpdateRelease>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateRelease {
    pub version: String,
    pub published_at: DateTime<Utc>,
    pub notes: ReleaseNotes,
    pub downloads: DownloadLinks,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ReleaseNotes {
    #[serde(rename = "zh-CN")]
    pub zh_cn: Vec<String>,
    pub en: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadLinks {
    pub github: String,
    pub quark: Option<String>,
    pub quark_code: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum UpdateStatus {
    UpdateAvailable,
    UpToDate,
    PlatformUnavailable,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheckResult {
    pub current_version: String,
    pub status: UpdateStatus,
    pub release: Option<UpdateRelease>,
    pub checked_at: DateTime<Utc>,
    pub from_cache: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheckError {
    pub code: &'static str,
}

fn unavailable() -> UpdateCheckError {
    UpdateCheckError {
        code: "unavailable",
    }
}
fn invalid_manifest() -> UpdateCheckError {
    UpdateCheckError {
        code: "invalidManifest",
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CachedManifest {
    checked_at: DateTime<Utc>,
    manifest: Manifest,
}

#[derive(Default)]
struct CheckState {
    cache_loaded: bool,
    cache: Option<CachedManifest>,
    last_attempt: Option<Instant>,
    last_result: Option<Result<UpdateCheckResult, UpdateCheckError>>,
}

pub struct UpdateService {
    client: Option<reqwest::Client>,
    endpoint: String,
    platform: String,
    current_version: String,
    cache_path: Option<PathBuf>,
    state: Mutex<CheckState>,
}

impl Default for UpdateService {
    fn default() -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(10))
            .https_only(true)
            .redirect(Policy::custom(|attempt| {
                if !allowed_manifest_redirect(attempt.url(), attempt.previous().len()) {
                    attempt.error("Update manifest redirect rejected")
                } else {
                    attempt.follow()
                }
            }))
            .build()
            .ok();
        Self {
            client,
            endpoint: ENDPOINT.into(),
            platform: platform_key(std::env::consts::OS, std::env::consts::ARCH),
            current_version: CURRENT_VERSION.into(),
            cache_path: crate::project::catalog::app_data_root()
                .ok()
                .map(|root| root.join("update-notice.json")),
            state: Mutex::new(CheckState::default()),
        }
    }
}

fn platform_key(os: &str, arch: &str) -> String {
    format!("{os}-{arch}")
}

fn safe_https_url(raw: &str) -> Option<Url> {
    let url = Url::parse(raw).ok()?;
    (url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port_or_known_default() == Some(443))
    .then_some(url)
}

fn allowed_manifest_url(url: &Url) -> bool {
    safe_https_url(url.as_str()).is_some()
        && matches!(url.host_str(), Some("ooolab.cn" | "www.ooolab.cn"))
}

fn allowed_manifest_redirect(url: &Url, hops: usize) -> bool {
    hops <= 3 && allowed_manifest_url(url)
}

fn allowed_download_url(raw: &str, quark: bool) -> bool {
    let Some(url) = safe_https_url(raw) else {
        return false;
    };
    let segments: Vec<_> = url
        .path()
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    if quark {
        url.host_str() == Some("pan.quark.cn")
            && segments.len() == 2
            && segments[0] == "s"
            && segments[1].bytes().all(|byte| byte.is_ascii_alphanumeric())
    } else {
        url.host_str() == Some("github.com")
            && segments.len() >= 5
            && segments[..3] == ["ooolabdev", "ooosplat", "releases"]
            && matches!(segments[3], "tag" | "download")
    }
}

fn parse_version(raw: &str) -> Result<Version, UpdateCheckError> {
    Version::parse(raw.strip_prefix('v').unwrap_or(raw)).map_err(|_| invalid_manifest())
}

fn validate_manifest(manifest: &Manifest) -> Result<(), UpdateCheckError> {
    if manifest.schema_version != 1 {
        return Err(invalid_manifest());
    }
    for release in manifest.releases.values() {
        parse_version(&release.version)?;
        if !allowed_download_url(&release.downloads.github, false)
            || release
                .downloads
                .quark
                .as_deref()
                .is_some_and(|url| !allowed_download_url(url, true))
            || release
                .downloads
                .quark_code
                .as_ref()
                .is_some_and(|code| code.len() > 64)
        {
            return Err(invalid_manifest());
        }
    }
    Ok(())
}

fn evaluate(
    cache: &CachedManifest,
    platform: &str,
    current_version: &str,
    from_cache: bool,
) -> Result<UpdateCheckResult, UpdateCheckError> {
    validate_manifest(&cache.manifest)?;
    let current = parse_version(current_version)?;
    let release = cache.manifest.releases.get(platform).cloned();
    let status = match &release {
        None => UpdateStatus::PlatformUnavailable,
        Some(release) => {
            let latest = parse_version(&release.version)?;
            if latest.pre.is_empty() && latest.cmp_precedence(&current).is_gt() {
                UpdateStatus::UpdateAvailable
            } else {
                UpdateStatus::UpToDate
            }
        }
    };
    Ok(UpdateCheckResult {
        current_version: current_version.into(),
        status,
        release,
        checked_at: cache.checked_at,
        from_cache,
    })
}

fn cache_is_fresh(cache: &CachedManifest, now: DateTime<Utc>) -> bool {
    now.signed_duration_since(cache.checked_at)
        .to_std()
        .is_ok_and(|age| age < CACHE_TTL)
}

impl UpdateService {
    pub async fn check(&self, force: bool) -> Result<UpdateCheckResult, UpdateCheckError> {
        #[cfg(feature = "e2e")]
        if crate::testing::enabled() {
            return Ok(UpdateCheckResult {
                current_version: CURRENT_VERSION.into(),
                status: UpdateStatus::UpToDate,
                release: None,
                checked_at: Utc::now(),
                from_cache: false,
            });
        }
        // Only this service is serialized. No pipeline, preview, or telemetry lock is held.
        // Callers waiting on an in-flight request reuse its result via the cooldown.
        let mut state = self.state.lock().await;
        if !state.cache_loaded {
            state.cache_loaded = true;
            if let Some(path) = &self.cache_path {
                state.cache = read_cache(path).await;
            }
        }
        if !force {
            if let Some(cache) = &state.cache {
                if cache_is_fresh(cache, Utc::now()) {
                    if let Ok(result) = evaluate(cache, &self.platform, &self.current_version, true)
                    {
                        return Ok(result);
                    }
                }
            }
        }
        if state
            .last_attempt
            .is_some_and(|attempt| attempt.elapsed() < CHECK_COOLDOWN)
        {
            return state
                .last_result
                .clone()
                .unwrap_or_else(|| Err(unavailable()));
        }
        state.last_attempt = Some(Instant::now());
        let result = self.fetch().await;
        match result {
            Ok(cache) => {
                let result = evaluate(&cache, &self.platform, &self.current_version, false);
                if result.is_ok() {
                    if let Some(path) = &self.cache_path {
                        // A read-only/unavailable settings directory must not hide a valid notice.
                        if let Some(parent) = path.parent() {
                            if tokio::fs::create_dir_all(parent).await.is_ok() {
                                let _ =
                                    crate::project::manager::atomic_write_json(path, &cache).await;
                            }
                        }
                    }
                    state.cache = Some(cache);
                }
                state.last_result = Some(result.clone());
                result
            }
            Err(error) => {
                state.last_result = Some(Err(error.clone()));
                Err(error)
            }
        }
    }

    async fn fetch(&self) -> Result<CachedManifest, UpdateCheckError> {
        let client = self.client.as_ref().ok_or_else(unavailable)?;
        let mut response = client
            .get(&self.endpoint)
            .header("Accept", "application/json")
            .header("Accept-Encoding", "identity")
            .send()
            .await
            .map_err(|_| unavailable())?;
        if !response.status().is_success() {
            return Err(unavailable());
        }
        if response
            .content_length()
            .is_some_and(|size| size > MAX_MANIFEST_BYTES as u64)
        {
            return Err(invalid_manifest());
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
            if bytes.len() + chunk.len() > MAX_MANIFEST_BYTES {
                return Err(invalid_manifest());
            }
            bytes.extend_from_slice(&chunk);
        }
        let manifest: Manifest = serde_json::from_slice(&bytes).map_err(|_| invalid_manifest())?;
        validate_manifest(&manifest)?;
        Ok(CachedManifest {
            checked_at: Utc::now(),
            manifest,
        })
    }
}

async fn read_cache(path: &std::path::Path) -> Option<CachedManifest> {
    let file = tokio::fs::File::open(path).await.ok()?;
    let mut bytes = Vec::new();
    file.take((MAX_CACHE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .await
        .ok()?;
    if bytes.len() > MAX_CACHE_BYTES {
        return None;
    }
    let cache: CachedManifest = serde_json::from_slice(&bytes).ok()?;
    validate_manifest(&cache.manifest).ok()?;
    Some(cache)
}

#[tauri::command]
pub async fn check_app_update(
    service: tauri::State<'_, UpdateService>,
    force: Option<bool>,
) -> Result<UpdateCheckResult, UpdateCheckError> {
    service.check(force.unwrap_or(false)).await
}

#[cfg(test)]
mod tests;
