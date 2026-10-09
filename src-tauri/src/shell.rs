// Ported from upstream App/AppDelegate.swift (menu, shortcuts, menu bar entry
// point rule), App/LoginItem.swift, App/GlobalShortcut.swift and
// Panel/ActiveDisplayFollower.swift + FloatingPanelController.settingsChanged.
//! Everything app-level that follows a setting: start at login, whether the
//! panel is on screen (switch, provider choice, full-screen apps), following
//! the pointer's display, global shortcuts and the tray menu.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use pulse_core::i18n;
use pulse_core::settings::AppSettings;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::{AppHandle, Emitter, Manager, Wry};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

use crate::panel::{self, SharedPanel};
use crate::state::AppState;
use crate::win;

pub const CHOOSE: &str = "choose";
pub const REFRESH: &str = "refresh";
pub const TOGGLE_PANEL: &str = "panel";
pub const SETTINGS: &str = "settings";
pub const QUIT: &str = "quit";

/// A full-screen window covers the panel's display (and the setting says to hide there).
static COVERED: AtomicBool = AtomicBool::new(false);
/// At least one global shortcut was accepted by the system.
static SHORTCUT_REGISTERED: AtomicBool = AtomicBool::new(false);

/// Bring the app in line with `settings`; `previous` is None at launch.
pub fn apply(app: &AppHandle, previous: Option<&AppSettings>) {
    let state = app.state::<AppState>();
    let settings = state.settings();

    if previous.is_none_or(|p| p.open_settings_shortcut != settings.open_settings_shortcut || p.toggle_panel_shortcut != settings.toggle_panel_shortcut) {
        apply_shortcuts(app, &settings);
    }
    if previous.is_none_or(|p| p.launch_at_login != settings.launch_at_login) {
        apply_login_item(settings.launch_at_login);
    }

    // Upstream `restoreMenuBarEntryPointIfNeeded`: hiding the tray icon must
    // never leave the app with no way back in.
    if settings.hides_tray_icon && tray_must_remain_visible(&settings) {
        let mut fixed = (*settings).clone();
        fixed.hides_tray_icon = false;
        state.save_settings(fixed);
        let _ = app.emit("settings-changed", &*state.settings());
    }

    sync_panel_visibility(app);
    rebuild_menu(app);
}

fn tray_must_remain_visible(settings: &AppSettings) -> bool {
    let panel_visible = !settings.needs_provider_selection() && settings.is_panel_visible;
    !panel_visible && !SHORTCUT_REGISTERED.load(Ordering::Relaxed)
}

/// Whether a registered shortcut is an entry point (for the settings UI's warning).
#[tauri::command]
pub fn shortcut_status(app: AppHandle) -> serde_json::Value {
    let settings = app.state::<AppState>().settings();
    let gs = app.global_shortcut();
    let ok = |s: &Option<pulse_core::settings::GlobalShortcut>| {
        s.as_ref().map(|s| gs.is_registered(s.accelerator.as_str()))
    };
    serde_json::json!({
        "openSettings": ok(&settings.open_settings_shortcut),
        "togglePanel": ok(&settings.toggle_panel_shortcut),
    })
}

// ---------------------------------------------------------------------------
// Start at login
// ---------------------------------------------------------------------------

fn apply_login_item(enabled: bool) {
    // A development build must not put itself in the Run key.
    if cfg!(debug_assertions) {
        return;
    }
    let Ok(exe) = std::env::current_exe() else { return };
    let wanted = enabled.then(|| win::login_item::command(&exe));
    if win::login_item::current() != wanted {
        win::login_item::set(enabled, &exe);
    }
}

// ---------------------------------------------------------------------------
// Panel visibility
// ---------------------------------------------------------------------------

/// The panel is on screen when the switch is on, a provider is chosen, and no
/// full-screen app covers its display.
pub fn panel_should_show(settings: &AppSettings) -> bool {
    settings.is_panel_visible && !settings.needs_provider_selection() && !COVERED.load(Ordering::Relaxed)
}

pub fn sync_panel_visibility(app: &AppHandle) {
    let Some(window) = app.get_webview_window(panel::LABEL) else { return };
    let settings = app.state::<AppState>().settings();
    // Before the UI has reported its geometry the window has nowhere to go;
    // `set_geometry` shows it the first time.
    if app.state::<SharedPanel>().lock().unwrap().geometry.is_none() {
        return;
    }
    let shown = win::is_visible(&window);
    let wanted = panel_should_show(&settings);
    if wanted && !shown {
        let _ = window.show();
        win::make_panel_window(&window);
        win::raise_topmost(&window);
        panel::place(app, &app.state::<SharedPanel>());
    } else if !wanted && shown {
        let _ = window.hide();
    }
}

/// Full-screen hiding and display following, sampled every quarter second
/// (upstream `ActiveDisplayFollower.interval`).
pub fn start_watcher(app: AppHandle) {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(250));
        let settings = app.state::<AppState>().settings();
        let shared = app.state::<SharedPanel>().inner().clone();
        let Some(window) = app.get_webview_window(panel::LABEL) else { continue };

        panel::reconcile_display(&app, &shared);

        let panel_display = shared.lock().unwrap().monitor.as_ref().map(|m| m.name.clone());
        let covered = settings.hides_in_full_screen
            && panel_display.is_some()
            && win::fullscreen_monitor(Some(&window)) == panel_display;
        if COVERED.swap(covered, Ordering::Relaxed) != covered {
            sync_panel_visibility(&app);
        }

        if settings.follows_active_display && panel_should_show(&settings) && win::monitor_count() > 1 {
            follow_pointer(&app, &shared);
        }
    });
}

/// Move the panel onto the display the pointer is on, keeping its ratios.
/// Not while the pointer is on the panel or a drag is under way: the move is
/// offered again on the next tick instead.
fn follow_pointer(app: &AppHandle, shared: &SharedPanel) {
    let Some(cursor) = win::cursor_pos() else { return };
    let target = win::monitor_at(cursor);
    let placement = {
        let mut state = shared.lock().unwrap();
        if state.monitor.as_ref().is_some_and(|m| m.name == target.name) || panel::is_busy(&state) {
            return;
        }
        state.placement.display = Some(target.name.clone());
        state.placement.clone()
    };
    crate::store::save_placement(app, &placement);
    panel::place(app, shared);
    let _ = app.emit("placement-changed", &placement);
}

// ---------------------------------------------------------------------------
// Global shortcuts
// ---------------------------------------------------------------------------

fn apply_shortcuts(app: &AppHandle, settings: &AppSettings) {
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();
    let mut any = false;
    if let Some(s) = &settings.open_settings_shortcut {
        any |= gs
            .on_shortcut(s.accelerator.as_str(), |app, _, event| {
                if event.state() == ShortcutState::Pressed {
                    crate::show_settings(app, None);
                }
            })
            .is_ok();
    }
    if let Some(s) = &settings.toggle_panel_shortcut {
        any |= gs
            .on_shortcut(s.accelerator.as_str(), |app, _, event| {
                if event.state() == ShortcutState::Pressed {
                    toggle_panel_setting(app);
                }
            })
            .is_ok();
    }
    SHORTCUT_REGISTERED.store(any, Ordering::Relaxed);
}

/// Through the setting, so the panel stays the way the switch says across a launch.
pub fn toggle_panel_setting(app: &AppHandle) {
    let state = app.state::<AppState>();
    let previous = state.settings();
    let mut next = (*previous).clone();
    next.is_panel_visible = !next.is_panel_visible;
    state.save_settings(next);
    let _ = app.emit("settings-changed", &*state.settings());
    apply(app, Some(&previous));
}

// ---------------------------------------------------------------------------
// Tray menu
// ---------------------------------------------------------------------------

/// Rebuilt whenever settings change (upstream rebuilds on every open).
pub fn rebuild_menu(app: &AppHandle) {
    let Some(tray) = app.tray_by_id("main") else { return };
    if let Ok(menu) = build_menu(app) {
        let _ = tray.set_menu(Some(menu));
    }
}

fn build_menu(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let settings = app.state::<AppState>().settings();
    let lang = i18n::resolve(settings.language);
    let t = |key: &str| i18n::t(lang, key, &[]);
    let menu = Menu::new(app)?;
    if settings.needs_provider_selection() {
        menu.append(&MenuItem::with_id(app, CHOOSE, t("Choose services to start monitoring…"), true, None::<&str>)?)?;
        menu.append(&PredefinedMenuItem::separator(app)?)?;
    } else {
        menu.append(&MenuItem::with_id(app, REFRESH, t("Refresh"), true, None::<&str>)?)?;
        menu.append(&PredefinedMenuItem::separator(app)?)?;
        menu.append(&CheckMenuItem::with_id(
            app,
            TOGGLE_PANEL,
            t("Show floating panel"),
            true,
            settings.is_panel_visible,
            None::<&str>,
        )?)?;
    }
    menu.append(&MenuItem::with_id(app, SETTINGS, t("Settings…"), true, None::<&str>)?)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItem::with_id(app, QUIT, t("Quit Pulse"), true, None::<&str>)?)?;
    Ok(menu)
}

pub fn on_menu_event(app: &AppHandle, id: &str) {
    match id {
        SETTINGS => crate::show_settings(app, None),
        CHOOSE => crate::show_chooser(app),
        REFRESH => app.state::<AppState>().wake.notify_one(),
        TOGGLE_PANEL => toggle_panel_setting(app),
        QUIT => app.exit(0),
        _ => {}
    }
}
