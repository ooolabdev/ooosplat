use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

use chrono::{Local, NaiveDate};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::{
    error::{Result, SplatError},
    project::{catalog::app_data_root, manager::atomic_write_json},
};

use super::event::{validate_privacy, TelemetryEvent, TelemetryPayload};

/// The production endpoint is compiled from `config/telemetry-endpoint.txt` by `build.rs`.
/// Invalid or non-HTTPS configuration fails closed and disables network delivery.
pub const TELEMETRY_ENDPOINT: Option<&str> = option_env!("OOOSPLAT_TELEMETRY_ENDPOINT");
const TELEMETRY_TIMEOUT: Duration = Duration::from_secs(4);
const TELEMETRY_SCHEMA_VERSION: u32 = 3;
const CURRENT_APP_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TelemetryDeliveryStatus {
    NotConfigured,
    Configured,
    Debug,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TelemetryPreferences {
    pub analytics_enabled: bool,
    pub consent_decided: bool,
    pub delivery_status: TelemetryDeliveryStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settings_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TelemetryConfig {
    schema_version: u32,
    install_id: Uuid,
    analytics_enabled: bool,
    consent_decided: bool,
    #[serde(default)]
    last_heartbeat_date: Option<NaiveDate>,
    #[serde(default)]
    last_heartbeat_app_version: Option<String>,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            schema_version: TELEMETRY_SCHEMA_VERSION,
            install_id: Uuid::new_v4(),
            analytics_enabled: true,
            consent_decided: true,
            last_heartbeat_date: None,
            last_heartbeat_app_version: None,
        }
    }
}

#[derive(Default)]
struct RuntimeState {
    config: Option<TelemetryConfig>,
    heartbeat_in_flight: bool,
    settings_error: Option<String>,
}

#[derive(Default)]
struct ConsentGate {
    disabled: AtomicBool,
    epoch: AtomicU64,
}

impl ConsentGate {
    fn close(&self) -> u64 {
        self.disabled.store(true, Ordering::SeqCst);
        self.epoch.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn accepts(&self, epoch: u64) -> bool {
        !self.disabled.load(Ordering::SeqCst) && self.epoch.load(Ordering::SeqCst) == epoch
    }
}

#[derive(Clone)]
enum Delivery {
    Disabled,
    Debug,
    Http {
        endpoint: reqwest::Url,
        client: reqwest::Client,
    },
    #[cfg(test)]
    Recording(Arc<std::sync::Mutex<Vec<serde_json::Value>>>),
}

impl Delivery {
    fn from_environment() -> Self {
        if std::env::var("OOOSPLAT_TELEMETRY_DEBUG").as_deref() == Ok("1") {
            return Self::Debug;
        }
        let Some(raw_endpoint) = TELEMETRY_ENDPOINT else {
            return Self::Disabled;
        };
        let Ok(endpoint) = reqwest::Url::parse(raw_endpoint) else {
            return Self::Disabled;
        };
        if endpoint.scheme() != "https" {
            return Self::Disabled;
        }
        let Ok(client) = reqwest::Client::builder()
            .connect_timeout(TELEMETRY_TIMEOUT)
            .timeout(TELEMETRY_TIMEOUT)
            .user_agent(concat!("OOOSplat/", env!("CARGO_PKG_VERSION")))
            .build()
        else {
            return Self::Disabled;
        };
        Self::Http { endpoint, client }
    }

    fn status(&self) -> TelemetryDeliveryStatus {
        match self {
            Self::Disabled => TelemetryDeliveryStatus::NotConfigured,
            Self::Debug => TelemetryDeliveryStatus::Debug,
            Self::Http { .. } => TelemetryDeliveryStatus::Configured,
            #[cfg(test)]
            Self::Recording(_) => TelemetryDeliveryStatus::Debug,
        }
    }

    async fn deliver(&self, payload: serde_json::Value) -> bool {
        match self {
            Self::Disabled => false,
            Self::Debug => {
                eprintln!(
                    "OOOSPLAT_TELEMETRY_DEBUG {}",
                    serde_json::to_string(&payload).unwrap_or_else(|_| "{}".into())
                );
                true
            }
            Self::Http { endpoint, client } => client
                .post(endpoint.clone())
                .json(&payload)
                .send()
                .await
                .is_ok_and(|response| response.status().is_success()),
            #[cfg(test)]
            Self::Recording(events) => {
                events
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push(payload);
                true
            }
        }
    }
}

#[derive(Clone)]
pub struct TelemetryService {
    config_path: Option<PathBuf>,
    state: Arc<Mutex<RuntimeState>>,
    delivery: Delivery,
    consent_gate: Arc<ConsentGate>,
}

impl Default for TelemetryService {
    fn default() -> Self {
        Self::new()
    }
}

impl TelemetryService {
    pub fn new() -> Self {
        let (config_path, settings_error) = match app_data_root() {
            Ok(root) => (Some(root.join("telemetry.json")), None),
            Err(error) => (None, Some(error.to_string())),
        };
        Self {
            config_path,
            state: Arc::new(Mutex::new(RuntimeState {
                settings_error,
                ..Default::default()
            })),
            delivery: Delivery::from_environment(),
            consent_gate: Arc::new(ConsentGate::default()),
        }
    }

    pub async fn initialize(&self) -> Result<TelemetryPreferences> {
        let preferences = self.preferences().await?;
        if preferences.analytics_enabled {
            self.spawn_daily_active();
        }
        Ok(preferences)
    }

    pub async fn preferences(&self) -> Result<TelemetryPreferences> {
        let mut state = self.state.lock().await;
        let _ = self.ensure_config(&mut state).await;
        Ok(self.preferences_for(&state))
    }

    pub async fn set_consent(&self, enabled: bool) -> Result<TelemetryPreferences> {
        // A close must take effect even while another operation is awaiting disk
        // I/O. A successful enable is the only operation that opens this gate.
        let epoch = self.consent_gate.close();
        let preferences = {
            let mut state = self.state.lock().await;
            // Only an explicit settings action may retry a previously failed
            // read. Background events never repair or replace an unreadable file.
            if state.settings_error.take().is_some() {
                state.config = None;
            }
            let Ok(config) = self.ensure_config(&mut state).await else {
                return Ok(self.preferences_for(&state));
            };
            let mut snapshot = config.clone();
            snapshot.analytics_enabled = enabled;
            snapshot.consent_decided = true;
            if !enabled {
                // Preserve this session's opt-out even when saving it fails.
                *config = snapshot.clone();
            }
            match self.save_config(&snapshot).await {
                Ok(()) => {
                    state.config = Some(snapshot);
                    state.settings_error = None;
                    if enabled && self.consent_gate.epoch.load(Ordering::SeqCst) == epoch {
                        self.consent_gate.disabled.store(false, Ordering::SeqCst);
                    }
                }
                Err(error) => self.fail_closed(&mut state, &error),
            }
            self.preferences_for(&state)
        };
        if preferences.analytics_enabled {
            self.spawn_daily_active();
        }
        Ok(preferences)
    }

    /// Enqueues anonymous telemetry as a strictly best-effort side channel.
    ///
    /// Delivery success, rejection, serialization/configuration failures, and network errors are
    /// intentionally unobservable to callers: telemetry must never change a pipeline result,
    /// emit a user-facing pipeline event, or block completion while the HTTP request is running.
    pub fn track(&self, event: TelemetryEvent) {
        let service = self.clone();
        let epoch = self.consent_gate.epoch.load(Ordering::SeqCst);
        tauri::async_runtime::spawn(async move {
            let _ = service.deliver_if_enabled_at(event, epoch).await;
        });
    }

    pub fn spawn_daily_active(&self) {
        let service = self.clone();
        let epoch = self.consent_gate.epoch.load(Ordering::SeqCst);
        tauri::async_runtime::spawn(async move {
            service
                .daily_active_on_at(Local::now().date_naive(), epoch)
                .await;
        });
    }

    #[cfg(test)]
    async fn daily_active_on(&self, today: NaiveDate) {
        let epoch = self.consent_gate.epoch.load(Ordering::SeqCst);
        self.daily_active_on_at(today, epoch).await;
    }

    async fn daily_active_on_at(&self, today: NaiveDate, epoch: u64) {
        let install_id = {
            let mut state = self.state.lock().await;
            if self.ensure_config(&mut state).await.is_err() {
                return;
            }
            let (enabled, heartbeat_already_sent, install_id) = state
                .config
                .as_ref()
                .map(|config| {
                    (
                        config.analytics_enabled,
                        config.last_heartbeat_date == Some(today)
                            && config.last_heartbeat_app_version.as_deref()
                                == Some(CURRENT_APP_VERSION),
                        config.install_id,
                    )
                })
                .unwrap_or((false, false, Uuid::nil()));
            if !enabled
                || !self.consent_gate.accepts(epoch)
                || heartbeat_already_sent
                || state.heartbeat_in_flight
            {
                return;
            }
            state.heartbeat_in_flight = true;
            install_id
        };

        let delivered = self
            .deliver_payload(
                TelemetryPayload::new(install_id, TelemetryEvent::DailyActive),
                epoch,
            )
            .await;

        let mut state = self.state.lock().await;
        state.heartbeat_in_flight = false;
        let Some(config) = state.config.as_mut() else {
            return;
        };
        if delivered && config.analytics_enabled && self.consent_gate.accepts(epoch) {
            config.last_heartbeat_date = Some(today);
            config.last_heartbeat_app_version = Some(CURRENT_APP_VERSION.to_owned());
            let snapshot = config.clone();
            let _ = self.save_config(&snapshot).await;
        }
    }

    #[cfg(test)]
    async fn deliver_if_enabled(&self, event: TelemetryEvent) -> bool {
        let epoch = self.consent_gate.epoch.load(Ordering::SeqCst);
        self.deliver_if_enabled_at(event, epoch).await
    }

    async fn deliver_if_enabled_at(&self, event: TelemetryEvent, epoch: u64) -> bool {
        let install_id = {
            let mut state = self.state.lock().await;
            let Ok(config) = self.ensure_config(&mut state).await else {
                return false;
            };
            if !config.analytics_enabled || !self.consent_gate.accepts(epoch) {
                return false;
            }
            config.install_id
        };
        self.deliver_payload(TelemetryPayload::new(install_id, event), epoch)
            .await
    }

    async fn deliver_payload(&self, payload: TelemetryPayload, epoch: u64) -> bool {
        let Ok(value) = serde_json::to_value(payload) else {
            return false;
        };
        if !validate_privacy(&value) {
            return false;
        }
        // Recheck after preparation, immediately before dispatch. Requests
        // already handed to the HTTP client cannot be recalled by opting out.
        {
            let state = self.state.lock().await;
            if state.settings_error.is_some()
                || !state.config.as_ref().is_some_and(|c| c.analytics_enabled)
                || !self.consent_gate.accepts(epoch)
            {
                return false;
            }
        }
        if !self.consent_gate.accepts(epoch) {
            return false;
        }
        self.delivery.deliver(value).await
    }

    async fn ensure_config<'a>(
        &self,
        state: &'a mut RuntimeState,
    ) -> Result<&'a mut TelemetryConfig> {
        if state.config.is_none() {
            if let Some(error) = &state.settings_error {
                return Err(SplatError::Process(error.clone()));
            }
            match self.load_config().await {
                Ok(config) => state.config = Some(config),
                Err(error) => {
                    self.fail_closed(state, &error);
                    return Err(error);
                }
            }
        }
        state
            .config
            .as_mut()
            .ok_or_else(|| SplatError::Process("无法初始化匿名统计设置".into()))
    }

    async fn load_config(&self) -> Result<TelemetryConfig> {
        let path = self
            .config_path
            .as_ref()
            .ok_or_else(|| SplatError::Process("无法初始化匿名统计设置".into()))?;
        let (mut config, mut needs_write): (TelemetryConfig, bool) =
            match tokio::fs::symlink_metadata(path).await {
                Ok(_) => (
                    serde_json::from_slice(&tokio::fs::read(path).await?)?,
                    false,
                ),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    (TelemetryConfig::default(), true)
                }
                Err(error) => return Err(error.into()),
            };
        if config.schema_version < TELEMETRY_SCHEMA_VERSION {
            config.schema_version = TELEMETRY_SCHEMA_VERSION;
            needs_write = true;
        }
        if needs_write {
            self.save_config(&config).await?;
        }
        Ok(config)
    }

    fn fail_closed(&self, state: &mut RuntimeState, error: &SplatError) {
        // An earlier I/O failure cannot invalidate a later explicit request
        // already waiting for this lock. Only user actions advance the epoch.
        self.consent_gate.disabled.store(true, Ordering::SeqCst);
        if let Some(config) = state.config.as_mut() {
            config.analytics_enabled = false;
        }
        state.settings_error = Some(error.to_string());
    }

    async fn save_config(&self, config: &TelemetryConfig) -> Result<()> {
        let path = self
            .config_path
            .as_ref()
            .ok_or_else(|| SplatError::Process("无法初始化匿名统计设置".into()))?;
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        atomic_write_json(path, config).await
    }

    fn preferences_for(&self, state: &RuntimeState) -> TelemetryPreferences {
        TelemetryPreferences {
            analytics_enabled: state.config.as_ref().is_some_and(|c| c.analytics_enabled)
                && state.settings_error.is_none()
                && !self.consent_gate.disabled.load(Ordering::SeqCst),
            consent_decided: state.config.as_ref().is_none_or(|c| c.consent_decided),
            delivery_status: self.delivery.status(),
            settings_error: state.settings_error.clone(),
        }
    }

    #[cfg(test)]
    pub(crate) fn recording(
        path: PathBuf,
    ) -> (Self, Arc<std::sync::Mutex<Vec<serde_json::Value>>>) {
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        (
            Self {
                config_path: Some(path),
                state: Arc::new(Mutex::new(RuntimeState::default())),
                delivery: Delivery::Recording(events.clone()),
                consent_gate: Arc::new(ConsentGate::default()),
            },
            events,
        )
    }

    #[cfg(test)]
    pub(crate) async fn enable_for_test(&self) {
        self.preferences().await.unwrap();
        let mut state = self.state.lock().await;
        state.config.as_mut().unwrap().analytics_enabled = true;
        state.config.as_mut().unwrap().consent_decided = true;
        state.settings_error = None;
        self.consent_gate.disabled.store(false, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        pipeline::effectiveness::PlannerEffectivenessSnapshot,
        telemetry::event::{
            TelemetryInputType, TelemetryPlannerOutcome, TelemetryQuality, TelemetryRunKind,
        },
    };

    #[test]
    fn production_endpoint_is_compiled_from_the_public_project_config() {
        let configured = include_str!("../../../config/telemetry-endpoint.txt").trim();
        assert_eq!(TELEMETRY_ENDPOINT, Some(configured));
    }

    #[tokio::test]
    async fn creates_and_persists_a_random_install_id() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("telemetry.json");
        let (first, _) = TelemetryService::recording(path.clone());
        first.preferences().await.unwrap();
        let first_config: TelemetryConfig =
            serde_json::from_slice(&tokio::fs::read(&path).await.unwrap()).unwrap();
        assert!(!first_config.install_id.is_nil());

        let (second, _) = TelemetryService::recording(path);
        second.preferences().await.unwrap();
        let second_config = second.state.lock().await.config.clone().unwrap();
        assert_eq!(first_config.install_id, second_config.install_id);
        assert!(second_config.analytics_enabled);
        assert!(second_config.consent_decided);
    }

    #[tokio::test]
    async fn daily_active_is_sent_at_most_once_per_day() {
        let directory = tempfile::tempdir().unwrap();
        let (service, events) =
            TelemetryService::recording(directory.path().join("telemetry.json"));
        service.enable_for_test().await;
        let day_one = NaiveDate::from_ymd_opt(2026, 8, 30).unwrap();
        service.daily_active_on(day_one).await;
        service.daily_active_on(day_one).await;
        let day_two = NaiveDate::from_ymd_opt(2026, 8, 31).unwrap();
        service.daily_active_on(day_two).await;

        let recorded = events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(recorded.len(), 2);
        assert!(recorded
            .iter()
            .all(|value| value["event"] == "daily_active"));
    }

    #[tokio::test]
    async fn upgrading_on_the_same_day_sends_a_new_daily_active() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("telemetry.json");
        let today = NaiveDate::from_ymd_opt(2026, 9, 8).unwrap();
        let install_id = Uuid::new_v4();
        tokio::fs::write(
            &path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "schemaVersion": 1,
                "installId": install_id,
                "analyticsEnabled": true,
                "consentDecided": true,
                "lastHeartbeatDate": today
            }))
            .unwrap(),
        )
        .await
        .unwrap();

        let (service, events) = TelemetryService::recording(path.clone());
        service.daily_active_on(today).await;
        service.daily_active_on(today).await;

        {
            let recorded = events
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            assert_eq!(recorded.len(), 1);
            assert_eq!(recorded[0]["event"], "daily_active");
            assert_eq!(recorded[0]["appVersion"], CURRENT_APP_VERSION);
        }

        let config: TelemetryConfig =
            serde_json::from_slice(&tokio::fs::read(path).await.unwrap()).unwrap();
        assert_eq!(config.schema_version, TELEMETRY_SCHEMA_VERSION);
        assert_eq!(config.last_heartbeat_date, Some(today));
        assert_eq!(
            config.last_heartbeat_app_version.as_deref(),
            Some(CURRENT_APP_VERSION)
        );
    }

    #[tokio::test]
    async fn disabled_analytics_never_reaches_delivery() {
        let directory = tempfile::tempdir().unwrap();
        let (service, events) =
            TelemetryService::recording(directory.path().join("telemetry.json"));
        service.set_consent(false).await.unwrap();
        assert!(
            !service
                .deliver_if_enabled(TelemetryEvent::DailyActive)
                .await
        );
        assert!(events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_empty());
    }

    #[tokio::test]
    async fn an_existing_opt_out_is_never_overridden() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("telemetry.json");
        let (first, _) = TelemetryService::recording(path.clone());
        first.set_consent(false).await.unwrap();

        let (second, _) = TelemetryService::recording(path);
        let preferences = second.preferences().await.unwrap();
        assert!(!preferences.analytics_enabled);
        assert!(preferences.consent_decided);
    }

    #[tokio::test]
    async fn damaged_settings_fail_closed_without_replacing_the_file_or_retrying_background_reads()
    {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("telemetry.json");
        let damaged = b"{existing settings are incomplete";
        tokio::fs::write(&path, damaged).await.unwrap();
        let (service, events) = TelemetryService::recording(path.clone());
        let preferences = service.preferences().await.unwrap();
        assert!(!preferences.analytics_enabled);
        assert!(preferences.consent_decided);
        assert!(preferences.settings_error.is_some());
        assert!(service.state.lock().await.config.is_none());
        assert_eq!(tokio::fs::read(&path).await.unwrap(), damaged);

        // Even an external repair is not automatically treated as new consent.
        let saved = TelemetryConfig::default();
        tokio::fs::write(&path, serde_json::to_vec(&saved).unwrap())
            .await
            .unwrap();
        assert!(!service.preferences().await.unwrap().analytics_enabled);
        assert!(
            !service
                .deliver_if_enabled(TelemetryEvent::DailyActive)
                .await
        );
        service
            .daily_active_on(NaiveDate::from_ymd_opt(2026, 11, 11).unwrap())
            .await;
        assert!(events.lock().unwrap().is_empty());
        assert!(service.state.lock().await.config.is_none());
        assert_eq!(
            serde_json::from_slice::<TelemetryConfig>(&tokio::fs::read(&path).await.unwrap())
                .unwrap()
                .install_id,
            saved.install_id,
        );

        let preferences = service.set_consent(false).await.unwrap();
        assert!(!preferences.analytics_enabled);
        assert!(preferences.settings_error.is_none());
        assert_eq!(
            service
                .state
                .lock()
                .await
                .config
                .as_ref()
                .unwrap()
                .install_id,
            saved.install_id,
        );
    }

    #[tokio::test]
    async fn unreadable_settings_are_not_mistaken_for_a_first_install() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("telemetry.json");
        tokio::fs::create_dir(&path).await.unwrap();
        tokio::fs::write(path.join("keep"), b"existing contents")
            .await
            .unwrap();
        let (service, events) = TelemetryService::recording(path.clone());
        for _ in 0..2 {
            let preferences = service.preferences().await.unwrap();
            assert!(!preferences.analytics_enabled);
            assert!(preferences.settings_error.is_some());
            assert!(service.state.lock().await.config.is_none());
        }
        assert!(
            !service
                .deliver_if_enabled(TelemetryEvent::DailyActive)
                .await
        );
        assert!(events.lock().unwrap().is_empty());
        assert_eq!(
            tokio::fs::read(path.join("keep")).await.unwrap(),
            b"existing contents"
        );
    }

    #[tokio::test]
    async fn saving_either_consent_failure_keeps_the_session_disabled() {
        for enabling in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("telemetry.json");
            let (service, events) = TelemetryService::recording(path.clone());
            service.preferences().await.unwrap();
            if enabling {
                service.set_consent(false).await.unwrap();
            }
            let install_id = service
                .state
                .lock()
                .await
                .config
                .as_ref()
                .unwrap()
                .install_id;
            tokio::fs::remove_file(&path).await.unwrap();
            tokio::fs::create_dir(&path).await.unwrap();
            tokio::fs::write(path.join("keep"), b"do not overwrite")
                .await
                .unwrap();

            let preferences = service.set_consent(enabling).await.unwrap();
            assert!(!preferences.analytics_enabled);
            assert!(preferences.settings_error.is_some());
            assert!(!service.preferences().await.unwrap().analytics_enabled);
            assert!(
                !service
                    .deliver_if_enabled(TelemetryEvent::DailyActive)
                    .await
            );
            assert!(events.lock().unwrap().is_empty());
            let state = service.state.lock().await;
            assert_eq!(state.config.as_ref().unwrap().install_id, install_id);
            assert!(!state.config.as_ref().unwrap().analytics_enabled);
            assert_eq!(
                tokio::fs::read(path.join("keep")).await.unwrap(),
                b"do not overwrite"
            );
        }
    }

    #[tokio::test]
    async fn disabling_closes_the_dispatch_gate_before_waiting_for_settings_io() {
        let directory = tempfile::tempdir().unwrap();
        let (service, _) = TelemetryService::recording(directory.path().join("telemetry.json"));
        assert!(service.preferences().await.unwrap().analytics_enabled);
        let state = service.state.lock().await;
        let previous_epoch = service.consent_gate.epoch.load(Ordering::SeqCst);
        let pending_service = service.clone();
        let pending = tokio::spawn(async move { pending_service.set_consent(false).await });
        tokio::task::yield_now().await;
        assert!(service.consent_gate.disabled.load(Ordering::SeqCst));
        assert_ne!(
            service.consent_gate.epoch.load(Ordering::SeqCst),
            previous_epoch
        );
        assert!(!pending.is_finished());
        drop(state);
        assert!(!pending.await.unwrap().unwrap().analytics_enabled);
    }

    #[tokio::test]
    async fn failed_opt_out_does_not_silently_cancel_a_later_explicit_enable() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("telemetry.json");
        let (service, _) = TelemetryService::recording(path.clone());
        service.preferences().await.unwrap();
        let state = service.state.lock().await;
        let saved = state.config.as_ref().unwrap().clone();
        tokio::fs::remove_file(&path).await.unwrap();
        tokio::fs::create_dir(&path).await.unwrap();
        tokio::fs::write(path.join("keep"), b"blocked setting file")
            .await
            .unwrap();
        let first = service.clone();
        let opt_out = tokio::spawn(async move { first.set_consent(false).await });
        tokio::task::yield_now().await;

        // FIFO mutex ordering puts the actual disk repair between the two
        // settings operations. Both requests are already queued before the
        // first operation encounters its real directory-replacement error.
        let repair_service = service.clone();
        let repair_path = path.clone();
        let saved_id = saved.install_id;
        let repair = tokio::spawn(async move {
            let state = repair_service.state.lock().await;
            assert!(state.settings_error.is_some());
            tokio::fs::remove_file(repair_path.join("keep"))
                .await
                .unwrap();
            tokio::fs::remove_dir(&repair_path).await.unwrap();
            tokio::fs::write(&repair_path, serde_json::to_vec(&saved).unwrap())
                .await
                .unwrap();
        });
        tokio::task::yield_now().await;
        let last = service.clone();
        let enable = tokio::spawn(async move { last.set_consent(true).await });
        tokio::task::yield_now().await;
        assert!(!opt_out.is_finished());
        assert!(!enable.is_finished());
        drop(state);

        let failed = opt_out.await.unwrap().unwrap();
        assert!(!failed.analytics_enabled);
        assert!(failed.settings_error.is_some());
        repair.await.unwrap();
        let enabled = enable.await.unwrap().unwrap();
        assert!(enabled.analytics_enabled);
        assert!(enabled.settings_error.is_none());
        assert_eq!(
            service
                .state
                .lock()
                .await
                .config
                .as_ref()
                .unwrap()
                .install_id,
            saved_id
        );
    }

    #[tokio::test]
    async fn queued_events_and_heartbeats_do_not_cross_an_opt_out_epoch() {
        let directory = tempfile::tempdir().unwrap();
        let (service, events) =
            TelemetryService::recording(directory.path().join("telemetry.json"));
        service.preferences().await.unwrap();
        let old_epoch = service.consent_gate.epoch.load(Ordering::SeqCst);
        let event = || TelemetryEvent::GenerationStarted {
            quality_preset: TelemetryQuality::Balanced,
            input_type: TelemetryInputType::Video,
        };
        service.set_consent(false).await.unwrap();
        assert!(service.set_consent(true).await.unwrap().analytics_enabled);
        assert!(!service.deliver_if_enabled_at(event(), old_epoch).await);
        assert!(
            !service
                .deliver_payload(TelemetryPayload::new(Uuid::new_v4(), event()), old_epoch)
                .await
        );
        service
            .daily_active_on_at(NaiveDate::from_ymd_opt(2026, 11, 10).unwrap(), old_epoch)
            .await;
        assert!(service.deliver_if_enabled(event()).await);
        let recorded = events.lock().unwrap();
        assert_eq!(
            recorded
                .iter()
                .filter(|value| value["event"] == "generation_started")
                .count(),
            1
        );
        assert!(!recorded
            .iter()
            .any(|value| value["installId"] == Uuid::nil().to_string()));
    }

    #[tokio::test]
    async fn planner_evaluation_delivery_failure_is_silent_and_keeps_analytics_enabled() {
        let directory = tempfile::tempdir().unwrap();
        let service = TelemetryService {
            config_path: Some(directory.path().join("telemetry.json")),
            state: Arc::new(Mutex::new(RuntimeState::default())),
            delivery: Delivery::Disabled,
            consent_gate: Arc::new(ConsentGate::default()),
        };

        assert!(service.preferences().await.unwrap().analytics_enabled);
        assert!(
            !service
                .deliver_if_enabled(TelemetryEvent::PlannerEvaluation {
                    planner_schema_version: 1,
                    run_id: Uuid::new_v4(),
                    run_kind: TelemetryRunKind::New,
                    outcome: TelemetryPlannerOutcome::Completed,
                    planner_enabled: true,
                    planner_version: Some("quality_v2_planner_v1"),
                    quality_preset: TelemetryQuality::High,
                    input_type: TelemetryInputType::Video,
                    total_duration_ms: 1_000,
                    failure_stage: None,
                    error_code: None,
                    snapshot: Box::new(PlannerEffectivenessSnapshot::default()),
                })
                .await
        );
        assert!(service.preferences().await.unwrap().analytics_enabled);
    }
}
