#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod panel;
mod placement;
mod state;
mod store;
mod tray_icon;
mod win;

use std::sync::{Arc, Mutex};

use pulse_core::settings::AppSettings;
use pulse_core::Provider;
use serde::Serialize;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};

use panel::{PanelState, SharedPanel};
use placement::{Geometry, Rect};
use state::{AppState, UsagePayload};

#[tauri::command]
fn get_snapshot(state: State<AppState>) -> UsagePayload {
    UsagePayload { usages: state.snapshot(), refreshing: state.store.refreshing() }
}

#[tauri::command]
fn get_settings(state: State<AppState>) -> AppSettings {
    (*state.settings()).clone()
}

/// Merge a partial settings object (camelCase keys, upstream names) and persist it.
#[tauri::command]
fn update_settings(app: AppHandle, state: State<AppState>, patch: serde_json::Value) -> Result<AppSettings, String> {
    let current = serde_json::to_value(&*state.settings()).map_err(|e| e.to_string())?;
    let mut merged = current;
    if let (Some(target), Some(patch)) = (merged.as_object_mut(), patch.as_object()) {
        for (k, v) in patch {
            target.insert(k.clone(), v.clone());
        }
    }
    let settings: AppSettings = serde_json::from_value(merged).map_err(|e| e.to_string())?;
    let refetch = {
        let old = state.settings();
        old.enabled_accounts != settings.enabled_accounts
            || old.sources != settings.sources
            || old.server_addresses != settings.server_addresses
            || old.network_proxy != settings.network_proxy
            || old.refresh_interval != settings.refresh_interval
    };
    state.save_settings(settings.clone());
    let _ = app.emit("settings-changed", &settings);
    state::emit_usage(&app);
    if refetch {
        state.wake.notify_one();
    }
    Ok(settings)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderInfo {
    id: &'static str,
    name: &'static str,
    icon: &'static str,
    multiple_accounts: bool,
}

#[tauri::command]
fn list_providers() -> Vec<ProviderInfo> {
    Provider::ALL
        .iter()
        .map(|p| ProviderInfo {
            id: p.raw(),
            name: p.display_name(),
            icon: p.icon_resource(),
            multiple_accounts: p.supports_multiple_accounts(),
        })
        .collect()
}

#[tauri::command]
fn has_secret(state: State<AppState>, id: String) -> bool {
    state.secrets.get(&id).is_some_and(|v| !v.trim().is_empty())
}

#[tauri::command]
fn set_secret(state: State<AppState>, id: String, value: Option<String>) {
    let value = value.map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    state.secrets.set(&id, value.as_deref());
    state.wake.notify_one();
}

#[tauri::command]
async fn refresh(app: AppHandle, account: Option<String>) -> Result<(), String> {
    let state = app.state::<AppState>();
    match account {
        Some(id) => {
            let store = state.store.clone();
            let settings = state.settings();
            state::emit_usage(&app);
            let app2 = app.clone();
            tauri::async_runtime::spawn(async move {
                store.refresh(settings, &id).await;
                state::emit_usage(&app2);
            });
        }
        None => state.wake.notify_one(),
    }
    Ok(())
}

#[tauri::command]
fn note_looked(state: State<AppState>) {
    state.store.note_looked();
}

#[tauri::command]
fn set_geometry(app: AppHandle, shared: State<SharedPanel>, geometry: Geometry) {
    let first = {
        let mut state = shared.lock().unwrap();
        let first = state.geometry.is_none();
        state.geometry = Some(geometry);
        first
    };
    panel::place(&app, &shared);
    if first && app.state::<AppState>().settings().is_panel_visible {
        if let Some(w) = app.get_webview_window(panel::LABEL) {
            let _ = w.show();
            // Tauri rewrites the extended style while showing; claim it back afterwards.
            win::make_panel_window(&w);
            win::raise_topmost(&w);
        }
    }
}

#[tauri::command]
fn set_hit_rects(shared: State<SharedPanel>, rects: Vec<Rect>, grab: Option<Rect>) {
    let mut state = shared.lock().unwrap();
    state.hit_rects = rects;
    state.grab_area = grab;
}

#[tauri::command]
fn rail_press(shared: State<SharedPanel>, x: f64, y: f64) {
    panel::press(&shared, (x, y));
}

fn home_dir() -> std::path::PathBuf {
    std::env::var_os("USERPROFILE").map(Into::into).unwrap_or_default()
}

#[tauri::command]
fn status_line_installed() -> bool {
    pulse_core::statusline::is_installed(&home_dir())
}

/// Connect or disconnect Claude Code's status line (Claude Code pane).
#[tauri::command]
fn set_status_line(state: State<AppState>, connected: bool) -> Result<bool, String> {
    let home = home_dir();
    let result = if connected {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        pulse_core::statusline::install(&home, &exe)
    } else {
        pulse_core::statusline::uninstall(&home)
    };
    result.map_err(|e| format!("{e:?}"))?;
    state.wake.notify_one();
    Ok(pulse_core::statusline::is_installed(&home))
}

#[tauri::command]
fn get_placement(shared: State<SharedPanel>) -> placement::Placement {
    shared.lock().unwrap().placement.clone()
}

/// Settings > Position: move the rail to another dock, keeping its ratios (upstream `PanelPlacement.update(dock:)`).
#[tauri::command]
fn set_dock(app: AppHandle, shared: State<SharedPanel>, dock: placement::Dock) {
    let placement = {
        let mut state = shared.lock().unwrap();
        state.placement.dock = dock;
        state.placement.clone()
    };
    store::save_placement(&app, &placement);
    panel::place(&app, &shared);
    let _ = app.emit("placement-changed", &placement);
}

#[tauri::command]
fn open_settings(app: AppHandle) {
    show_settings(&app);
}

fn show_settings(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("settings") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    let _ = WebviewWindowBuilder::new(app, "settings", WebviewUrl::App("settings.html".into()))
        .title("Pulse Settings")
        .inner_size(920.0, 680.0)
        .min_inner_size(720.0, 480.0)
        .build();
}

fn main() {
    // Headless modes run before any window exists.
    if std::env::args().any(|a| a == pulse_core::statusline::MODE_ARGUMENT) {
        pulse_core::statusline::run_as_status_line();
        return;
    }
    if let Ok(exe) = std::env::current_exe() {
        pulse_core::statusline::repair_path_if_needed(&home_dir(), &exe);
    }

    let shared: SharedPanel = Arc::new(Mutex::new(PanelState::default()));

    tauri::Builder::default()
        .manage(shared.clone())
        .invoke_handler(tauri::generate_handler![
            get_snapshot,
            get_settings,
            update_settings,
            list_providers,
            has_secret,
            set_secret,
            refresh,
            note_looked,
            set_geometry,
            set_hit_rects,
            rail_press,
            get_placement,
            set_dock,
            status_line_installed,
            set_status_line,
            open_settings
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            let state = AppState::load();
            let needs_choice = state.settings().needs_provider_selection() && !state.mock;
            app.manage(state);

            shared.lock().unwrap().placement = store::load_placement(&handle);
            panel::create(&handle)?;
            panel::start_sampler(handle.clone(), shared.clone());
            state::start_refresh_loop(handle.clone());

            let settings = MenuItem::with_id(app, "settings", "Settings…", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit Pulse", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&settings, &quit])?;
            TrayIconBuilder::with_id("main")
                .icon(app.default_window_icon().cloned().expect("icon"))
                .tooltip("Pulse")
                .menu(&menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "settings" => show_settings(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;

            // Upstream opens the provider chooser on first launch; until it exists, Settings.
            if needs_choice {
                show_settings(&handle);
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Pulse");
}
