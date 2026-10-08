#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod panel;
mod placement;
mod store;
mod win;

use std::sync::{Arc, Mutex};

use pulse_core::ProviderUsage;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, State, WebviewUrl, WebviewWindowBuilder};

use panel::{PanelState, SharedPanel};
use placement::{Geometry, Rect};

#[tauri::command]
fn get_snapshot() -> Vec<ProviderUsage> {
    pulse_core::mock::sample_usages()
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
    if first {
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
        .inner_size(900.0, 680.0)
        .min_inner_size(720.0, 480.0)
        .build();
}

fn main() {
    let shared: SharedPanel = Arc::new(Mutex::new(PanelState::default()));

    tauri::Builder::default()
        .manage(shared.clone())
        .invoke_handler(tauri::generate_handler![
            get_snapshot,
            set_geometry,
            set_hit_rects,
            rail_press,
            open_settings
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            shared.lock().unwrap().placement = store::load_placement(&handle);
            panel::create(&handle)?;
            panel::start_sampler(handle.clone(), shared.clone());

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
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Pulse");
}
