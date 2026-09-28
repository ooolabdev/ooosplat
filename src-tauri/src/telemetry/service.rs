use std::{path::PathBuf, sync::Arc, time::Duration};

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
}

impl Default for TelemetryService {
    fn default() -> Self {
        Self::new()
    }
}

impl TelemetryService {
    pub fn new() -> Self {
        Self {
            config_path: app_data_root().ok().map(|root| root.join("telemetry.json")),
            state: Arc::new(Mutex::new(RuntimeState::default())),
            delivery: Delivery::from_environment(),
        }
    }

    pub async fn initialize(&self) -> Result<TelemetryPreferences> {
        let preferences = self.preferences().await?;
        self.spawn_daily_active();
        Ok(preferences)
    }

    pub async fn preferences(&self) -> Result<TelemetryPreferences> {
        let mut state = self.state.lock().await;
        let config = self.ensure_config(&mut state).await?;
        Ok(self.preferences_for(config))
    }

    pub async fn set_consent(&self, enabled: bool) -> Result<TelemetryPreferences> {
        let preferences = {
            let mut state = self.state.lock().await;
            let config = self.ensure_config(&mut state).await?;
            config.analytics_enabled = enabled;
            config.consent_decided = true;
            let snapshot = config.clone();
            self.save_config(&snapshot).await?;
            self.preferences_for(config)
        };
        if enabled {
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
        tauri::async_runtime::spawn(async move {
            let _ = service.deliver_if_enabled(event).await;
        });
    }

    pub fn spawn_daily_active(&self) {
        let service = self.clone();
        tauri::async_runtime::spawn(async move {
            service.daily_active_on(Local::now().date_naive()).await;
        });
    }

    async fn daily_active_on(&self, today: NaiveDate) {
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
            if !enabled || heartbeat_already_sent || state.heartbeat_in_flight {
                return;
            }
            state.heartbeat_in_flight = true;
            install_id
        };

        let delivered = self
            .deliver_payload(TelemetryPayload::new(
                install_id,
                TelemetryEvent::DailyActive,
            ))
            .await;

        let mut state = self.state.lock().await;
        state.heartbeat_in_flight = false;
        let Some(config) = state.config.as_mut() else {
            return;
        };
        if delivered && config.analytics_enabled {
            config.last_heartbeat_date = Some(today);
            config.last_heartbeat_app_version = Some(CURRENT_APP_VERSION.to_owned());
            let snapshot = config.clone();
            let _ = self.save_config(&snapshot).await;
        }
    }

    async fn deliver_if_enabled(&self, event: TelemetryEvent) -> bool {
        let install_id = {
            let mut state = self.state.lock().await;
            let Ok(config) = self.ensure_config(&mut state).await else {
                return false;
            };
            if !config.analytics_enabled {
                return false;
            }
            config.install_id
        };
        self.deliver_payload(TelemetryPayload::new(install_id, event))
            .await
    }

    async fn deliver_payload(&self, payload: TelemetryPayload) -> bool {
        let Ok(value) = serde_json::to_value(payload) else {
            return false;
        };
        if !validate_privacy(&value) {
            return false;
        }
        self.delivery.deliver(value).await
    }

    async fn ensure_config<'a>(
        &self,
        state: &'a mut RuntimeState,
    ) -> Result<&'a mut TelemetryConfig> {
        if state.config.is_none() {
            let mut needs_write = false;
            let mut config = match &self.config_path {
                Some(path) if path.is_file() => match tokio::fs::read(path).await {
                    Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|_| {
                        needs_write = true;
                        TelemetryConfig::default()
                    }),
                    Err(_) => {
                        needs_write = true;
                        TelemetryConfig::default()
                    }
                },
                Some(_) => {
                    needs_write = true;
                    TelemetryConfig::default()
                }
                None => TelemetryConfig::default(),
            };
            if config.schema_version < TELEMETRY_SCHEMA_VERSION {
                config.schema_version = TELEMETRY_SCHEMA_VERSION;
                needs_write = true;
            }
            if needs_write {
                self.save_config(&config).await?;
            }
            state.config = Some(config);
        }
        state
            .config
            .as_mut()
            .ok_or_else(|| SplatError::Process("无法初始化匿名统计设置".into()))
    }

    async fn save_config(&self, config: &TelemetryConfig) -> Result<()> {
        let Some(path) = &self.config_path else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        atomic_write_json(path, config).await
    }

    fn preferences_for(&self, config: &TelemetryConfig) -> TelemetryPreferences {
        TelemetryPreferences {
            analytics_enabled: config.analytics_enabled,
            consent_decided: config.consent_decided,
            delivery_status: self.delivery.status(),
        }
    }

    #[cfg(test)]
    pub(super) fn recording(
        path: PathBuf,
    ) -> (Self, Arc<std::sync::Mutex<Vec<serde_json::Value>>>) {
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        (
            Self {
                config_path: Some(path),
                state: Arc::new(Mutex::new(RuntimeState::default())),
                delivery: Delivery::Recording(events.clone()),
            },
            events,
        )
    }

    #[cfg(test)]
    pub(super) async fn enable_for_test(&self) {
        self.preferences().await.unwrap();
        let mut state = self.state.lock().await;
        state.config.as_mut().unwrap().analytics_enabled = true;
        state.config.as_mut().unwrap().consent_decided = true;
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
    async fn planner_evaluation_delivery_failure_is_silent_and_keeps_analytics_enabled() {
        let directory = tempfile::tempdir().unwrap();
        let service = TelemetryService {
            config_path: Some(directory.path().join("telemetry.json")),
            state: Arc::new(Mutex::new(RuntimeState::default())),
            delivery: Delivery::Disabled,
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
