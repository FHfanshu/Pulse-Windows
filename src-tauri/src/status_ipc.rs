//! Service status over IPC, and the "a service is down" notification (upstream
//! `ServiceStatusGroup`, `ServiceStatus.read` and `UsageAlerts.checkServices`).
//!
//! Pages are fetched through `pulse_core::service::http_client`, so the proxy setting applies. The
//! pane reads on opening and every five minutes while it stays open; reads are kept for a minute
//! so reopening a pane does not ask the provider's server again. The outage check runs on its own
//! clock, every five minutes ([`status::CHECK_INTERVAL`]), reading only the state now, one request
//! per page; it does nothing unless the switch is on and a provider with a page is in use.
//! Its memory is `%APPDATA%\Pulse\status-alerts.json`.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use chrono::Utc;
use pulse_core::outage::{self, OutageMemory};
use pulse_core::service::http_client;
use pulse_core::status::{self, ServiceStatus, StatusCache, StatusPage};
use pulse_core::{AccountKey, Provider};
use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::state::AppState;

pub struct StatusState {
    cache: StatusCache,
    outages: Mutex<OutageMemory>,
    path: PathBuf,
    /// One outage check at a time.
    checking: AtomicBool,
}

/// What the pane draws for a provider: who is speaking, where the page is, and what it says.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusView {
    pub page: StatusPage,
    pub address: &'static str,
    pub host: &'static str,
    pub company: &'static str,
    pub day_count: usize,
    /// `None` when the page couldn't be read: the pane says so rather than leaving rows to read
    /// as healthy.
    pub status: Option<ServiceStatus>,
}

/// The provider's status page, read now or kept from a read a moment ago. `None` for a provider
/// with no page of its own. `force` skips the keeping.
#[tauri::command]
pub async fn service_status(
    state: State<'_, AppState>,
    kept: State<'_, StatusState>,
    provider: String,
    force: Option<bool>,
) -> Result<Option<StatusView>, String> {
    let Some(page) = Provider::from_raw(&provider).and_then(StatusPage::for_provider) else { return Ok(None) };
    let http = http_client(&state.settings());
    let status = kept.cache.read(page, &http, force.unwrap_or(false)).await;
    Ok(Some(StatusView {
        page,
        address: page.address(),
        host: page.host(),
        company: page.company(),
        day_count: page.day_count(),
        status,
    }))
}

/// Open the provider's status page in the default browser (the pane's "Open" button). Only the
/// page's own address, never one the UI names.
#[tauri::command]
pub fn open_status_page(provider: String) {
    let Some(page) = Provider::from_raw(&provider).and_then(StatusPage::for_provider) else { return };
    #[cfg(windows)]
    {
        // `rundll32 url.dll,FileProtocolHandler <url>` is the shell's own "open this address": no
        // console window, no extra dependency.
        let _ = std::process::Command::new("rundll32").arg("url.dll,FileProtocolHandler").arg(page.address()).spawn();
    }
    #[cfg(not(windows))]
    let _ = page;
}

/// Register the memory and start the outage loop. Call once from `setup`.
pub fn start(app: &AppHandle) {
    let path = pulse_core::paths::data_dir().join("status-alerts.json");
    let memory = std::fs::read(&path).map(|b| OutageMemory::from_json(&b)).unwrap_or_default();
    app.manage(StatusState {
        cache: StatusCache::default(),
        outages: Mutex::new(memory),
        path,
        checking: AtomicBool::new(false),
    });
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            check_services(&app).await;
            tokio::time::sleep(status::CHECK_INTERVAL).await;
        }
    });
}

/// Check now (the switch was flipped, or a provider turned on or off).
pub fn check_now(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move { check_services(&app).await });
}

fn persist(kept: &StatusState, memory: &OutageMemory) {
    let tmp = kept.path.with_extension("tmp");
    if std::fs::write(&tmp, memory.to_json()).is_ok() {
        let _ = std::fs::rename(&tmp, &kept.path);
    }
}

/// Forgets the pages Pulse is no longer watching: switched off, or the provider off.
fn forget_outages_except(kept: &StatusState, pages: &BTreeSet<StatusPage>) {
    let mut memory = kept.outages.lock().unwrap();
    let before = memory.clone();
    memory.keep_only(pages);
    if *memory != before {
        persist(kept, &memory);
    }
}

/// Asks the status pages of the providers in use whether their service is down, and says so.
/// Only providers switched on: a disabled one is not fetched, and a page that can't be read
/// changes nothing.
async fn check_services(app: &AppHandle) {
    let state = app.state::<AppState>();
    let kept = app.state::<StatusState>();
    let settings = state.settings();
    if !settings.alerts_on_outage {
        forget_outages_except(&kept, &BTreeSet::new());
        return;
    }
    let in_use: BTreeSet<StatusPage> = settings
        .enabled_accounts
        .iter()
        .filter_map(|id| AccountKey::from_id(id))
        .filter_map(|key| StatusPage::for_provider(key.provider))
        .collect();
    forget_outages_except(&kept, &in_use);
    if in_use.is_empty() || kept.checking.swap(true, Ordering::AcqRel) {
        return;
    }

    let http = http_client(&settings);
    let now = Utc::now();
    let reads: Vec<_> = in_use
        .iter()
        .map(|&page| {
            let http = http.clone();
            tauri::async_runtime::spawn(async move { (page, status::current(page, &http, now).await) })
        })
        .collect();
    for read in reads {
        if let Ok((page, Some(components))) = read.await {
            consider(app, page, &components);
        }
    }
    kept.checking.store(false, Ordering::Release);
}

fn consider(app: &AppHandle, page: StatusPage, components: &[status::Component]) {
    let state = app.state::<AppState>();
    let kept = app.state::<StatusState>();
    let settings = state.settings();
    // Switched off while the page was being read.
    if !settings.alerts_on_outage {
        return;
    }
    let watched: Vec<_> = components.iter().filter(|c| page.notifies_about(c)).cloned().collect();
    let change = {
        let mut memory = kept.outages.lock().unwrap();
        let before = memory.clone();
        let change = memory.changes(&watched, page);
        if *memory != before {
            persist(&kept, &memory);
        }
        change
    };
    if let Some(note) = outage::notification(page, &change) {
        crate::notify::post(app, &settings, &note);
    }
}
