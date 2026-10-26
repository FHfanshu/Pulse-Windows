// Ported from upstream App/AppDelegate.swift (menu, shortcuts, menu bar entry
// point rule), App/LoginItem.swift, App/GlobalShortcut.swift and
// Panel/ActiveDisplayFollower.swift + FloatingPanelController.settingsChanged.
//! Everything app-level that follows a setting: start at login, whether the
//! panel is on screen (switch, provider choice, full-screen apps), following
//! the pointer's display, global shortcuts and the tray menu.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use pulse_core::i18n;
use pulse_core::login_item::{self, Plan};
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
pub const UPDATE: &str = "update";
pub const QUIT: &str = "quit";

/// A full-screen window covers the panel's display (and the setting says to hide there).
static COVERED: AtomicBool = AtomicBool::new(false);
/// At least one global shortcut was accepted by the system.
static SHORTCUT_REGISTERED: AtomicBool = AtomicBool::new(false);
/// A restore has hidden the panel and will show it after `RESTORE_GAP`; the watcher leaves it hidden.
static RESTORING: AtomicBool = AtomicBool::new(false);

/// Bring the app in line with `settings`; `previous` is None at launch.
pub fn apply(app: &AppHandle, previous: Option<&AppSettings>) {
    let state = app.state::<AppState>();
    let settings = state.settings();

    if previous.is_none_or(|p| p.open_settings_shortcut != settings.open_settings_shortcut || p.toggle_panel_shortcut != settings.toggle_panel_shortcut) {
        apply_shortcuts(app, &settings);
    }
    if previous.is_none_or(|p| p.launch_at_login != settings.launch_at_login) {
        // At launch Windows' Startup apps toggle may switch the setting off; a switch the user has
        // just flipped is theirs.
        apply_login_item(app, previous.is_none());
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

/// Bring the Run value in line with the setting (decisions: `pulse_core::login_item::plan`).
///
/// An installed build only: a development build must never put itself in the Run key or take the
/// installed one's place. `follow_windows` is whether Windows' own Startup toggle may switch the
/// setting off (at launch, and when Settings > General is opened), as opposed to the user having
/// just flipped the setting in Pulse.
fn apply_login_item(app: &AppHandle, follow_windows: bool) {
    if cfg!(debug_assertions) {
        return;
    }
    let state = app.state::<AppState>();
    let settings = state.settings();
    let Ok(exe) = std::env::current_exe() else { return };
    let wanted = login_item::command(&exe);

    let mut disabled = login_item::startup_disabled(win::login_item::startup_approved().as_deref());
    if settings.launch_at_login && disabled && !follow_windows {
        // The user has just switched Pulse on here after switching it off in Windows.
        win::login_item::clear_startup_approved();
        disabled = false;
    }
    let current = win::login_item::current();
    match login_item::plan(settings.launch_at_login, current.as_deref(), &wanted, disabled, follow_windows) {
        Plan::Nothing => {}
        Plan::Write => {
            win::login_item::write(&wanted);
            // A record left from an earlier "off" would keep the new value off.
            win::login_item::clear_startup_approved();
        }
        Plan::Remove => win::login_item::remove(),
        Plan::SwitchOff => {
            let mut off = (*settings).clone();
            off.launch_at_login = false;
            state.save_settings(off);
            let _ = app.emit("settings-changed", &*state.settings());
        }
    }
}

/// Settings > General opening: has the user switched Pulse off in Windows since it started?
#[tauri::command]
pub fn sync_login_item(app: AppHandle) {
    apply_login_item(&app, true);
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
    sync_panel_presentation(app, false);
}

fn sync_panel_presentation(app: &AppHandle, restore: bool) {
    let next = app.clone();
    let _ = app.run_on_main_thread(move || apply_panel_presentation(&next, restore));
}

/// How long a restore keeps the panel hidden before showing it again. A hide and a show in the same
/// UI-thread pass can be coalesced by DWM and WebView2 after a session switch, leaving the window
/// "visible" but undrawn; switching the panel off and on by hand always has a gap between them.
const RESTORE_GAP: Duration = Duration::from_millis(150);

/// Windows difference: all hide/show/style operations run together on the UI thread. After a
/// session switch, force the same hide/show cycle that manually toggling the panel uses, even if
/// Win32 still considers the missing window visible. Re-read the policy when queued work runs.
fn apply_panel_presentation(app: &AppHandle, restore: bool) {
    let Some(window) = app.get_webview_window(panel::LABEL) else { return };
    let settings = app.state::<AppState>().settings();
    // Before the UI has reported its geometry the window has nowhere to go;
    // `set_geometry` shows it the first time.
    if app.state::<SharedPanel>().lock().unwrap().geometry.is_none() {
        return;
    }
    let shown = win::is_visible(&window);
    let wanted = panel_should_show(&settings);
    if wanted && restore {
        if win::primary_monitor().is_none() { return; }
        // Hide now (the window and WebView2's own visibility), show on a later pass, as the switch does.
        let _ = window.hide();
        win::set_webview_visible(&window, false);
        RESTORING.store(true, Ordering::Release);
        let next = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(RESTORE_GAP);
            RESTORING.store(false, Ordering::Release);
            sync_panel_presentation(&next, false);
        });
    } else if wanted && !shown {
        if win::primary_monitor().is_none() { return; }
        let _ = window.show();
        win::set_webview_visible(&window, true);
        win::make_panel_window(&window);
        win::raise_topmost(&window);
        panel::place(app, &app.state::<SharedPanel>());
    } else if !wanted && shown {
        let _ = window.hide();
    }
}

/// After a session or display event: when to restore (`RESTORE_AFTER` from the latest event). The
/// first pass lets a burst of connect / unlock / desktop-ready settle; the second catches desktops
/// (RDP reconnects, streaming tools' virtual displays) that are not drawing yet at the first.
const RESTORE_AFTER: [Duration; 2] = [Duration::from_millis(750), Duration::from_millis(3000)];

/// The restore passes still due, earliest first. A new event replaces them.
fn schedule_restores(event: Instant) -> Vec<Instant> {
    RESTORE_AFTER.iter().map(|d| event + *d).collect()
}

/// Whether a pass is due at `now`; drops every pass that is (several due at once run as one).
fn take_due_restore(pending: &mut Vec<Instant>, now: Instant) -> bool {
    let before = pending.len();
    pending.retain(|at| *at > now);
    pending.len() != before
}

/// Full-screen hiding and display following, sampled every quarter second
/// (upstream `ActiveDisplayFollower.interval`).
pub fn start_watcher(app: AppHandle) {
    std::thread::spawn(move || {
        let mut restores = Vec::new();
        loop {
            std::thread::sleep(Duration::from_millis(250));
            let settings = app.state::<AppState>().settings();
            let shared = app.state::<SharedPanel>().inner().clone();
            let Some(window) = app.get_webview_window(panel::LABEL) else { continue };

            crate::session::watch(&window);
            if crate::session::take_restore_request() {
                // Windows may send connect, unlock and desktop-ready in a burst. Let the desktop and
                // display topology settle, then recover using the latest geometry.
                restores = schedule_restores(Instant::now());
            }
            // With no usable monitor yet, a due pass waits for one.
            let restore = win::primary_monitor().is_some() && take_due_restore(&mut restores, Instant::now());

            panel::reconcile_display(&app, &shared);

            let panel_display = shared.lock().unwrap().monitor.as_ref().map(|m| m.name.clone());
            let covered = settings.hides_in_full_screen
                && panel_display.is_some()
                && win::fullscreen_monitor(Some(&window)) == panel_display;
            let changed = COVERED.swap(covered, Ordering::Relaxed) != covered;
            let mismatch = !RESTORING.load(Ordering::Acquire) && panel_should_show(&settings) != win::is_visible(&window);
            if changed || restore || mismatch {
                sync_panel_presentation(&app, restore || (changed && !covered));
            }

            if settings.follows_active_display && panel_should_show(&settings) && win::monitor_count() > 1 {
                follow_pointer(&app, &shared);
            }
        }
    });
}

/// Move the panel onto the display the pointer is on, keeping its ratios.
/// Not while the pointer is on the panel or a drag is under way: the move is
/// offered again on the next tick instead.
fn follow_pointer(app: &AppHandle, shared: &SharedPanel) {
    let Some(cursor) = win::cursor_pos() else { return };
    let Some(target) = win::monitor_at(cursor) else { return };
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
    // Built there too: menus wrap muda's `Rc` the same way.
    let handle = app.clone();
    crate::tray_icon::with_tray(app, move |tray| {
        if let Ok(menu) = build_menu(&handle) {
            let _ = tray.set_menu(Some(menu));
        }
    });
}

fn build_menu(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let settings = app.state::<AppState>().settings();
    let lang = i18n::resolve(settings.language);
    let t = |key: &str| i18n::t(lang, key, &[]);
    let menu = Menu::new(app)?;
    if settings.needs_provider_selection() {
        menu.append(&MenuItem::with_id(app, CHOOSE, t("Choose services to start monitoring…"), true, None::<&str>)?)?;
        menu.append(&PredefinedMenuItem::separator(app)?)?;
    }
    // Upstream: "Pulse X is available", after the choice of services. Windows difference: it opens
    // Settings > About, where "Update to X" is, instead of Sparkle's window.
    if let Some(newer) = crate::update_ipc::newer_version(app) {
        menu.append(&MenuItem::with_id(app, UPDATE, i18n::t(lang, "Pulse %@ is available", &[&newer]), true, None::<&str>)?)?;
        menu.append(&PredefinedMenuItem::separator(app)?)?;
    }
    if !settings.needs_provider_selection() {
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
        UPDATE => crate::show_settings(app, Some("about".into())),
        CHOOSE => crate::show_chooser(app),
        REFRESH => app.state::<AppState>().wake.notify_one(),
        TOGGLE_PANEL => toggle_panel_setting(app),
        QUIT => app.exit(0),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_event_restores_twice_then_stops() {
        let event = Instant::now();
        let mut pending = schedule_restores(event);
        assert!(!take_due_restore(&mut pending, event + Duration::from_millis(500)), "the burst settles first");
        assert!(take_due_restore(&mut pending, event + Duration::from_millis(800)));
        assert!(!take_due_restore(&mut pending, event + Duration::from_millis(1000)), "each pass runs once");
        assert!(take_due_restore(&mut pending, event + Duration::from_millis(3100)), "a late desktop is caught");
        assert!(pending.is_empty());
        assert!(!take_due_restore(&mut pending, event + Duration::from_secs(10)));
    }

    #[test]
    fn passes_due_together_run_as_one_and_a_new_event_restarts() {
        let event = Instant::now();
        let mut pending = schedule_restores(event);
        // The watcher was held up (no monitor): both passes are due, and one restore covers them.
        assert!(take_due_restore(&mut pending, event + Duration::from_secs(5)));
        assert!(pending.is_empty());
        let mut pending = schedule_restores(event);
        assert!(take_due_restore(&mut pending, event + Duration::from_millis(800)));
        // Another connect / display change before the late pass replaces it with a full schedule.
        let again = event + Duration::from_millis(2000);
        pending = schedule_restores(again);
        assert!(!take_due_restore(&mut pending, again + Duration::from_millis(500)), "it settles again");
        assert!(take_due_restore(&mut pending, again + Duration::from_millis(800)));
        assert!(take_due_restore(&mut pending, again + Duration::from_millis(3100)));
    }
}