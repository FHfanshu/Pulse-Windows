// Ported from upstream App/AppUpdate.swift.
//! Pulse's own updates: the checks (at launch and every two hours, and when About opens or asks),
//! and the install the user starts with the "Update to X" button.
//!
//! Windows difference: upstream's Sparkle puts up its own windows; here the About pane draws the
//! state (`pulse_core::update::UpdateState`, kept in [`UpdateHandle`] and pushed on
//! `update-changed`) and the install is `tauri-plugin-updater`: it downloads the signed NSIS
//! installer from the GitHub release, verifies the signature against the public key in
//! `tauri.conf.json`, and starts the installer in passive mode (a progress bar, no pages, no UAC
//! as the install is per user). Pulse exits as the installer starts and the installer starts it
//! again when it is done. **Nothing is installed without the user's click**, as upstream.
//!
//! All of it lives here, in one process, so two windows asking at once get one check.

use std::sync::Mutex;
use std::time::Duration;

use pulse_core::update::{self, UpdateState, CHECK_INTERVAL};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::state::AppState;

/// A check that has not answered by now has not reached the feed.
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);
/// After launch, once the app has settled (upstream waits for its first network probe).
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(20);
/// Development builds read this feed instead, so the About pane can be tried against a local file.
const FEED_OVERRIDE_VARIABLE: &str = "PULSE_UPDATE_FEED";

pub struct UpdateHandle {
    state: Mutex<UpdateState>,
    /// What the last check found, kept for the install.
    pending: tokio::sync::Mutex<Option<Update>>,
}

impl UpdateHandle {
    pub fn new(version: &str) -> Self {
        let can_check = update::can_check(cfg!(debug_assertions), feed_override().as_deref());
        Self { state: Mutex::new(UpdateState::new(version, can_check)), pending: tokio::sync::Mutex::new(None) }
    }

    fn snapshot(&self) -> UpdateState {
        self.state.lock().unwrap().clone()
    }
}

/// Only a development build reads the override: an installed one must never be pointed elsewhere.
fn feed_override() -> Option<String> {
    cfg!(debug_assertions).then(|| std::env::var(FEED_OVERRIDE_VARIABLE).ok()).flatten().filter(|f| !f.trim().is_empty())
}

/// The version of the update on offer, for the tray menu.
pub fn newer_version(app: &AppHandle) -> Option<String> {
    app.try_state::<UpdateHandle>()?.snapshot().newer
}

fn changed(app: &AppHandle) {
    let _ = app.emit("update-changed", app.state::<UpdateHandle>().snapshot());
}

/// Edit the state, then tell the windows and the tray menu.
fn with_state<T>(app: &AppHandle, edit: impl FnOnce(&mut UpdateState) -> T) -> T {
    let out = edit(&mut app.state::<UpdateHandle>().state.lock().unwrap());
    changed(app);
    out
}

#[tauri::command]
pub fn update_state(handle: State<UpdateHandle>) -> UpdateState {
    handle.snapshot()
}

/// Ask now. "Check now" and the probe when About opens are the same call: the answer goes to the
/// subtitle either way, and a check already under way answers for both.
#[tauri::command]
pub async fn update_check(app: AppHandle) {
    check(&app).await;
}

async fn check(app: &AppHandle) {
    if !with_state(app, |s| s.begin_check()) {
        return;
    }
    let result = tokio::time::timeout(CHECK_TIMEOUT, find(app)).await;
    let found = match result {
        Ok(Ok(found)) => found,
        _ => {
            with_state(app, |s| s.finish_check(Err(())));
            return;
        }
    };
    let version = found.as_ref().map(|u| u.version.clone());
    *app.state::<UpdateHandle>().pending.lock().await = found;
    with_state(app, |s| s.finish_check(Ok(version)));
    crate::shell::rebuild_menu(app);
}

async fn find(app: &AppHandle) -> tauri_plugin_updater::Result<Option<Update>> {
    let mut builder = app.updater_builder();
    if let Some(feed) = feed_override() {
        if let Ok(url) = feed.parse() {
            builder = builder.endpoints(vec![url])?;
        }
    }
    let settings = app.state::<AppState>().settings();
    let proxy = &settings.network_proxy;
    if proxy.enabled && !proxy.host.is_empty() {
        let scheme = if proxy.scheme.is_empty() { "http" } else { proxy.scheme.as_str() };
        if let Ok(url) = format!("{scheme}://{}:{}", proxy.host, proxy.port).parse() {
            builder = builder.proxy(url);
        }
    }
    // The process exits as the installer starts; take the tray icon away first so it does not
    // linger in the notification area until the pointer passes over it.
    let handle = app.clone();
    builder = builder.on_before_exit(move || {
        let _ = handle.remove_tray_by_id("main");
    });
    builder.build()?.check().await
}

/// "Update to X": download, verify, and hand over to the installer. Pulse exits when the
/// installer starts, so this returns only when something went wrong.
#[tauri::command]
pub async fn update_install(app: AppHandle) {
    if !with_state(&app, |s| s.begin_install()) {
        return;
    }
    let Some(found) = app.state::<UpdateHandle>().pending.lock().await.clone() else {
        with_state(&app, |s| s.install_failed());
        return;
    };
    let progress = app.clone();
    let mut last_step = u64::MAX;
    let finished = app.clone();
    let result = found
        .download_and_install(
            move |chunk, total| {
                let step = with_state_quiet(&progress, |s| {
                    s.downloaded_chunk(chunk as u64, total);
                    // One event per percent, not one per network read.
                    (s.fraction().unwrap_or(0.0) * 100.0) as u64
                });
                if step != last_step {
                    last_step = step;
                    changed(&progress);
                }
            },
            move || {
                with_state(&finished, |s| s.installing());
            },
        )
        .await;
    // Windows exits inside a successful install; coming back means it did not start.
    if result.is_err() {
        with_state(&app, |s| s.install_failed());
    }
}

fn with_state_quiet<T>(app: &AppHandle, edit: impl FnOnce(&mut UpdateState) -> T) -> T {
    edit(&mut app.state::<UpdateHandle>().state.lock().unwrap())
}

/// The two-hourly schedule (upstream `SUScheduledCheckInterval`), when "Check automatically" is on.
pub fn start(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK_DELAY).await;
        loop {
            if app.state::<AppState>().settings().checks_updates_automatically {
                check(&app).await;
            }
            tokio::time::sleep(CHECK_INTERVAL).await;
        }
    });
}
