//! App-wide state shared by commands and background tasks.

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use pulse_core::secrets::{FileSecrets, SecretStore};
use pulse_core::settings::AppSettings;
use pulse_core::store::UsageStore;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Notify;

pub struct AppState {
    pub settings: RwLock<Arc<AppSettings>>,
    pub store: Arc<UsageStore>,
    pub secrets: Arc<dyn SecretStore>,
    /// Wakes the refresh loop early (settings changed, manual refresh, panel looked at).
    pub wake: Notify,
    pub dir: PathBuf,
    /// Usage notification engine and its memory (`alerts.json`).
    pub alerts: crate::notify::Alerts,
    /// Development aid: `PULSE_MOCK=1` shows fixed sample readings.
    pub mock: bool,
}

impl AppState {
    pub fn load() -> Self {
        let dir = pulse_core::paths::data_dir();
        let _ = std::fs::create_dir_all(&dir);
        let settings: AppSettings = std::fs::read(dir.join("settings.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let secrets: Arc<dyn SecretStore> = Arc::new(FileSecrets::new(dir.join("keys.dat")));
        let store = Arc::new(
            UsageStore::new(pulse_core::providers::registry(), secrets.clone(), Some(dir.join("cache.json")))
                .with_elsewhere_file(dir.join("used-elsewhere.json")),
        );
        Self {
            settings: RwLock::new(Arc::new(settings.normalized())),
            store,
            secrets,
            wake: Notify::new(),
            alerts: crate::notify::Alerts::load(&dir),
            dir,
            mock: std::env::var("PULSE_MOCK").is_ok_and(|v| v == "1"),
        }
    }

    pub fn settings(&self) -> Arc<AppSettings> {
        self.settings.read().unwrap().clone()
    }

    pub fn save_settings(&self, settings: AppSettings) {
        let settings = settings.normalized();
        if let Ok(bytes) = serde_json::to_vec_pretty(&settings) {
            let path = self.dir.join("settings.json");
            let tmp = path.with_extension("tmp");
            if std::fs::write(&tmp, bytes).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
        }
        *self.settings.write().unwrap() = Arc::new(settings);
    }

    pub fn snapshot(&self) -> Vec<pulse_core::ProviderUsage> {
        if self.mock {
            return pulse_core::mock::sample_usages();
        }
        self.store.snapshot(&self.settings().ordered_enabled_accounts())
    }
}

pub fn emit_usage(app: &AppHandle) {
    let state = app.state::<AppState>();
    let payload = UsagePayload { usages: state.snapshot(), refreshing: state.store.refreshing() };
    crate::tray_icon::update(app, &payload.usages, &state.settings());
    let _ = app.emit("usage-changed", payload);
}

#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UsagePayload {
    pub usages: Vec<pulse_core::ProviderUsage>,
    pub refreshing: Vec<String>,
}

/// One-shot adaptive refresh loop (upstream `UsageStore.scheduleNext`).
pub fn start_refresh_loop(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            let state = app.state::<AppState>();
            let settings = state.settings();
            if !settings.needs_provider_selection() && !state.mock {
                emit_usage(&app);
                state.store.refresh_all(settings.clone()).await;
                emit_usage(&app);
                crate::notify::after_refresh(&app);
            }
            let wait = state.store.next_interval(&settings).max(30) as u64;
            tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_secs(wait)) => {}
                _ = state.wake.notified() => {}
            }
        }
    });
}
