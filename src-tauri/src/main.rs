#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod account_ipc;
mod dashboard;
mod history_ipc;
mod notifications_ipc;
mod notify;
mod panel;
mod placement;
mod recap_ipc;
mod shell;
mod signin_ipc;
mod spend_ipc;
mod state;
mod status_ipc;
mod store;
mod tray_icon;
mod win;

use std::sync::{Arc, Mutex};

use pulse_core::settings::AppSettings;
use pulse_core::Provider;
use serde::Serialize;
use tauri::menu::Menu;
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
    let previous = state.settings();
    state.save_settings(settings.clone());
    shell::apply(&app, Some(&previous));
    dashboard::sync(&app, Some(&previous));
    notify::reconsider(&app, &previous);
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
                notify::after_refresh(&app2);
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
    if first && shell::panel_should_show(&app.state::<AppState>().settings()) {
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

/// Open Settings, optionally on one pane (`"spend"`, `"notifications"`, `"account:codex"`…).
#[tauri::command]
fn open_settings(app: AppHandle, pane: Option<String>) {
    show_settings(&app, pane);
}

/// Presence-only scan for the chooser: which providers left a folder on this PC.
#[tauri::command]
fn detect_providers() -> Vec<&'static str> {
    let env_path = |name: &str| std::env::var_os(name).map(std::path::PathBuf::from).unwrap_or_default();
    pulse_core::discovery::detected(&home_dir(), &env_path("APPDATA"), &env_path("LOCALAPPDATA"))
        .into_iter()
        .map(|p| p.raw())
        .collect()
}

/// Write bytes the UI produced (a recap PNG) to a path the user picked in a save dialog.
#[tauri::command]
fn save_file(path: String, bytes: Vec<u8>) -> Result<(), String> {
    // Only images: this is not a general file-writing door for the webview.
    if !path.to_ascii_lowercase().ends_with(".png") {
        return Err("only .png files are written".into());
    }
    std::fs::write(path, bytes).map_err(|e| e.to_string())
}

#[tauri::command]
fn open_chooser(app: AppHandle) {
    show_chooser(&app);
}

pub(crate) fn show_chooser(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("chooser") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    let _ = WebviewWindowBuilder::new(app, "chooser", WebviewUrl::App("chooser.html".into()))
        .title("Pulse")
        .inner_size(560.0, 640.0)
        .min_inner_size(480.0, 420.0)
        .center()
        .build();
}

pub(crate) fn show_settings(app: &AppHandle, pane: Option<String>) {
    if let Some(w) = app.get_webview_window("settings") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        if let Some(pane) = pane {
            let _ = app.emit_to("settings", "settings-navigate", pane);
        }
        return;
    }
    let page = match pane {
        Some(pane) => format!("settings.html?pane={}", pane.replace(':', "%3A")),
        None => "settings.html".into(),
    };
    let _ = WebviewWindowBuilder::new(app, "settings", WebviewUrl::App(page.into()))
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
    // Used by the installer's uninstall step, and handy from a terminal.
    for (flag, connect) in [("--statusline-install", true), ("--statusline-uninstall", false)] {
        if std::env::args().any(|a| a == flag) {
            let result = match (connect, std::env::current_exe()) {
                (true, Ok(exe)) => pulse_core::statusline::install(&home_dir(), &exe),
                (false, _) => pulse_core::statusline::uninstall(&home_dir()),
                (true, Err(_)) => std::process::exit(1),
            };
            std::process::exit(if result.is_ok() { 0 } else { 1 });
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        pulse_core::statusline::repair_path_if_needed(&home_dir(), &exe);
    }

    let shared: SharedPanel = Arc::new(Mutex::new(PanelState::default()));

    tauri::Builder::default()
        // One Pulse at a time: launching it again opens Settings in the one already running.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_settings(app, None)))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
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
            account_ipc::estimated_value,
            account_ipc::installed_browsers,
            account_ipc::open_external,
            spend_ipc::spend_overview,
            spend_ipc::spend_release,
            spend_ipc::card_spend,
            spend_ipc::prompt_cache,
            recap_ipc::recap_report,
            recap_ipc::open_recap,
            open_settings,
            history_ipc::account_history,
            status_ipc::service_status,
            status_ipc::open_status_page,
            notifications_ipc::reports_spendable_balance,
            detect_providers,
            open_chooser,
            shell::shortcut_status,
            signin_ipc::signin_state,
            signin_ipc::signin_start,
            signin_ipc::signin_cancel,
            signin_ipc::signin_sign_out,
            signin_ipc::signin_remove_account,
            signin_ipc::signin_open_page,
            signin_ipc::signin_copy_code,
            shell::set_glass_region,
            save_file,
            dashboard::dashboard_resize,
            dashboard::dashboard_hide,
            dashboard::dashboard_spend,
            dashboard::open_url,
            dashboard::quit_app
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
            spend_ipc::start_price_refresh();
            status_ipc::start(&handle);
            notifications_ipc::start(&handle);

            TrayIconBuilder::with_id("main")
                .icon(app.default_window_icon().cloned().expect("icon"))
                .tooltip("Pulse")
                .menu(&Menu::new(app)?)
                .on_menu_event(|app, event| shell::on_menu_event(app, event.id.as_ref()))
                .on_tray_icon_event(|tray, event| dashboard::on_tray_event(tray.app_handle(), event))
                .build(app)?;
            shell::apply(&handle, None);
            dashboard::sync(&handle, None);
            shell::start_watcher(handle.clone());

            // First launch with nothing chosen: the provider chooser, not Settings.
            if needs_choice {
                show_chooser(&handle);
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Pulse");
}
