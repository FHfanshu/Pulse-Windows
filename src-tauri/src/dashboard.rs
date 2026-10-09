// Ported from upstream Sources/Pulse/App/MenuDashboard.swift and AppDelegate.addDashboard (the window half).
//! The usage dashboard as a tray popup.
//!
//! macOS puts the dashboard at the top of the status item's menu. Windows tray menus cannot host a view,
//! so with "Usage panel in the menu" on, a **left click** on the tray icon opens this borderless,
//! always-on-top flyout above the icon instead (`ui/dashboard.html`); **right click** still opens the
//! tray menu (Refresh, Settings…, Quit, …). With the setting off, left click keeps opening the menu —
//! `TrayIcon::set_show_menu_on_left_click` is switched at runtime by [`sync`].
//!
//! The flyout hides when it loses focus or on Escape, and is sized by its own content: the page reports
//! its size through `dashboard_resize`.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use chrono::Utc;
use pulse_core::settings::AppSettings;
use pulse_core::spend::{self, calendar::Calendar, dashboard::DashboardSpend};
use pulse_core::Provider;
use serde::Serialize;
use tauri::tray::{MouseButton, MouseButtonState, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};

use crate::placement::{Rect, Size};
use crate::state::AppState;
use crate::win;

pub const LABEL: &str = "dashboard";

/// Upstream's `MenuDashboard.width`.
const WIDTH: f64 = 320.0;
const INITIAL_HEIGHT: f64 = 360.0;
/// Clear space between the flyout and the screen edge / taskbar (Windows 11 flyouts sit 12 px off the taskbar).
const EDGE: f64 = 12.0;
/// Between the flyout and a tray icon that is not on the taskbar (the overflow flyout).
const GAP: f64 = 8.0;
/// A blur this soon after showing is the window taking focus, not the user leaving.
const SETTLE: Duration = Duration::from_millis(150);
/// A blur this close to a tray press is that press closing the flyout; its release must not reopen it.
const TOGGLE_WINDOW: Duration = Duration::from_millis(250);

struct Popup {
    /// The tray icon's rectangle in physical screen pixels: left, top, right, bottom.
    anchor: Option<(i32, i32, i32, i32)>,
    /// What the page last reported, DIPs.
    size: (f64, f64),
    shown_at: Option<Instant>,
    /// Focus loss that hid the flyout.
    hid_at: Option<Instant>,
    /// Last left press on the tray icon.
    down_at: Option<Instant>,
}

static POPUP: Mutex<Popup> =
    Mutex::new(Popup { anchor: None, size: (WIDTH, INITIAL_HEIGHT), shown_at: None, hid_at: None, down_at: None });

/// Whether the tray icon's left click opens the dashboard: upstream adds it only when it is switched on and
/// there is something to show.
pub fn is_active(settings: &AppSettings) -> bool {
    settings.shows_menu_dashboard && !settings.hides_tray_icon && !settings.needs_provider_selection()
}

/// Bring the popup in line with the settings: the window exists (hidden) while it is wanted, and the tray
/// icon's left click is given to it. `previous` is None at launch.
pub fn sync(app: &AppHandle, previous: Option<&AppSettings>) {
    let settings = app.state::<AppState>().settings();
    let active = is_active(&settings);
    if previous.is_some_and(|p| is_active(p) == active) {
        return;
    }
    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_show_menu_on_left_click(!active);
    }
    if active {
        // Building a webview from a command or the event loop's own thread can stall WebView2 on Windows.
        let app = app.clone();
        std::thread::spawn(move || {
            let _ = ensure(&app);
        });
    } else if let Some(window) = app.get_webview_window(LABEL) {
        let _ = window.destroy();
    }
}

fn ensure(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    if let Some(window) = app.get_webview_window(LABEL) {
        return Ok(window);
    }
    let window = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("dashboard.html".into()))
        .title("Pulse")
        .decorations(false)
        .shadow(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .focused(false)
        .visible(false)
        .inner_size(WIDTH, INITIAL_HEIGHT)
        .build()?;
    round_corners(&window);
    let handle = app.clone();
    window.on_window_event(move |event| {
        if let WindowEvent::Focused(false) = event {
            lost_focus(&handle);
        }
    });
    Ok(window)
}

/// Windows 11 rounds top-level windows by default only when they are decorated; ask for it.
fn round_corners(window: &WebviewWindow) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND};
    let Ok(hwnd) = window.hwnd() else { return };
    let preference = DWMWCP_ROUND;
    unsafe {
        let _ = DwmSetWindowAttribute(
            HWND(hwnd.0 as _),
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &preference as *const _ as *const std::ffi::c_void,
            std::mem::size_of_val(&preference) as u32,
        );
    }
}

fn lost_focus(app: &AppHandle) {
    let Some(window) = app.get_webview_window(LABEL) else { return };
    {
        let popup = POPUP.lock().unwrap();
        if popup.shown_at.is_some_and(|t| t.elapsed() < SETTLE) {
            return;
        }
    }
    if win::is_visible(&window) {
        POPUP.lock().unwrap().hid_at = Some(Instant::now());
        let _ = window.hide();
    }
}

/// The tray icon's own events. Only the left button is ours; the right button belongs to the tray menu.
pub fn on_tray_event(app: &AppHandle, event: TrayIconEvent) {
    let TrayIconEvent::Click { button: MouseButton::Left, button_state, rect, position, .. } = event else { return };
    if !is_active(&app.state::<AppState>().settings()) {
        return;
    }
    match button_state {
        MouseButtonState::Down => POPUP.lock().unwrap().down_at = Some(Instant::now()),
        MouseButtonState::Up => {
            // The icon's rectangle arrives in physical pixels already; the scale factor only matters for logical units.
            let origin = rect.position.to_physical::<f64>(1.0);
            let extent = rect.size.to_physical::<f64>(1.0);
            let anchor = if extent.width < 1.0 || extent.height < 1.0 {
                (position.x as i32, position.y as i32, position.x as i32, position.y as i32)
            } else {
                let (left, top) = (origin.x.round() as i32, origin.y.round() as i32);
                (left, top, left + extent.width as i32, top + extent.height as i32)
            };
            open(app, anchor);
        }
    }
}

fn open(app: &AppHandle, anchor: (i32, i32, i32, i32)) {
    let Some(window) = app.get_webview_window(LABEL) else {
        // Not built yet (just switched on): build it, then show.
        let app = app.clone();
        std::thread::spawn(move || {
            if ensure(&app).is_ok() {
                open(&app, anchor);
            }
        });
        return;
    };
    {
        let mut popup = POPUP.lock().unwrap();
        // The press that took focus away is the one that closes it.
        let closed_by_this_click = match (popup.hid_at.take(), popup.down_at) {
            (Some(hid), Some(down)) => hid + TOGGLE_WINDOW >= down,
            _ => false,
        };
        if closed_by_this_click {
            return;
        }
        popup.anchor = Some(anchor);
        popup.shown_at = Some(Instant::now());
    }
    if win::is_visible(&window) {
        let _ = window.hide();
        return;
    }
    place(&window);
    let _ = window.show();
    let _ = window.set_focus();
    win::raise_topmost(&window);
    let _ = app.emit_to(LABEL, "dashboard-shown", ());
}

fn place(window: &WebviewWindow) {
    let (anchor, (width, height)) = {
        let popup = POPUP.lock().unwrap();
        let Some(anchor) = popup.anchor else { return };
        (anchor, popup.size)
    };
    let monitor = win::monitor_at(((anchor.0 + anchor.2) / 2, (anchor.1 + anchor.3) / 2));
    let scale = monitor.scale;
    let icon = Rect {
        x: anchor.0 as f64 / scale,
        y: anchor.1 as f64 / scale,
        w: (anchor.2 - anchor.0) as f64 / scale,
        h: (anchor.3 - anchor.1) as f64 / scale,
    };
    let frame = popup_frame(icon, monitor.work_dip(), Size { w: width, h: height });
    // Twice: moving to a monitor of another scale makes the window re-derive its size from the old one.
    win::set_frame_dip(window, frame, scale);
    win::set_frame_dip(window, frame, scale);
}

/// Where the flyout goes for a tray icon at `icon`, on a monitor whose usable area is `work` (all DIPs):
/// centred on the icon and above it for a bottom taskbar, below for a top one, beside it for a side one,
/// always inside the work area.
pub fn popup_frame(icon: Rect, work: Rect, size: Size) -> Rect {
    let clamp = |v: f64, lo: f64, hi: f64| v.max(lo).min(hi.max(lo));
    let w = size.w.min(work.w - 2.0 * EDGE).max(1.0);
    let h = size.h.min(work.h - 2.0 * EDGE).max(1.0);
    let cx = icon.x + icon.w / 2.0;
    let cy = icon.y + icon.h / 2.0;
    let (x, y) = if cx < work.x || cx >= work.right() {
        // A taskbar on the left or right edge: the icon is outside the work area, so sit against that edge.
        let x = if cx < work.x { work.x + EDGE } else { work.right() - w - EDGE };
        (x, cy - h / 2.0)
    } else {
        let x = cx - w / 2.0;
        let y = if cy >= work.y + work.h / 2.0 {
            (icon.y - GAP).min(work.bottom() - EDGE) - h
        } else {
            (icon.bottom() + GAP).max(work.y + EDGE)
        };
        (x, y)
    };
    Rect {
        x: clamp(x, work.x + EDGE, work.right() - w - EDGE),
        y: clamp(y, work.y + EDGE, work.bottom() - h - EDGE),
        w,
        h,
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// The page's content size, DIPs. Keeps the anchor, so a taller tab grows upward from a bottom taskbar.
#[tauri::command]
pub fn dashboard_resize(app: AppHandle, width: f64, height: f64) {
    if !(width.is_finite() && height.is_finite()) || width < 1.0 || height < 1.0 {
        return;
    }
    POPUP.lock().unwrap().size = (width, height);
    if let Some(window) = app.get_webview_window(LABEL) {
        if win::is_visible(&window) {
            place(&window);
        }
    }
}

/// Escape, or an item that was chosen (upstream's menu closes after every item).
#[tauri::command]
pub fn dashboard_hide(app: AppHandle) {
    if let Some(window) = app.get_webview_window(LABEL) {
        let _ = window.hide();
    }
}

#[tauri::command]
pub fn quit_app(app: AppHandle) {
    app.exit(0);
}

/// Open a provider's usage page in the default browser (upstream `NSWorkspace.shared.open`). Web addresses only.
#[tauri::command]
pub fn open_url(url: String) -> Result<(), String> {
    use windows::core::{w, HSTRING};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    if !url.starts_with("https://") || url.chars().any(|c| c.is_control()) {
        return Err("only https addresses are opened".into());
    }
    let target = HSTRING::from(url);
    let result = unsafe { ShellExecuteW(None, w!("open"), &target, None, None, SW_SHOWNORMAL) };
    // Values above 32 mean success.
    if result.0 as isize > 32 {
        Ok(())
    } else {
        Err("couldn't open the address".into())
    }
}

#[derive(Serialize)]
#[serde(transparent)]
pub struct Spend(DashboardSpend);

fn home() -> std::path::PathBuf {
    std::env::var_os("USERPROFILE").map(Into::into).unwrap_or_default()
}

/// An account tab's spend figures, read from the provider's local transcripts. Nothing is read with Token
/// spend off (upstream: the tab's only read is the ledger, and only with that switch on).
#[tauri::command]
pub async fn dashboard_spend(state: State<'_, AppState>, provider: String) -> Result<Option<Spend>, String> {
    let Some(provider) = Provider::from_raw(&provider) else { return Ok(None) };
    if !state.settings().reads_token_spend || !spend::supports(provider) {
        return Ok(None);
    }
    tauri::async_runtime::spawn_blocking(move || {
        let now = Utc::now();
        let ledger = spend::read_ledger(provider, &home(), now).ok()?;
        DashboardSpend::of(&ledger, now, &Calendar::local()).map(Spend)
    })
    .await
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: Rect = Rect { x: 0.0, y: 0.0, w: 1920.0, h: 1040.0 };
    const SIZE: Size = Size { w: 320.0, h: 400.0 };

    #[test]
    fn bottom_taskbar_puts_it_above_the_icon() {
        let icon = Rect { x: 1700.0, y: 1046.0, w: 24.0, h: 24.0 };
        let f = popup_frame(icon, WORK, SIZE);
        assert_eq!(f.bottom(), WORK.bottom() - EDGE);
        assert!((f.x + f.w / 2.0 - 1712.0).abs() < 0.01);
    }

    #[test]
    fn it_stays_inside_the_work_area() {
        let icon = Rect { x: 1900.0, y: 1046.0, w: 20.0, h: 24.0 };
        let f = popup_frame(icon, WORK, SIZE);
        assert!(f.right() <= WORK.right() - EDGE);
    }

    #[test]
    fn top_taskbar_puts_it_below_and_side_taskbar_beside() {
        let top_work = Rect { x: 0.0, y: 48.0, w: 1920.0, h: 1032.0 };
        let f = popup_frame(Rect { x: 1700.0, y: 12.0, w: 24.0, h: 24.0 }, top_work, SIZE);
        assert_eq!(f.y, top_work.y + EDGE);
        let right_work = Rect { x: 0.0, y: 0.0, w: 1872.0, h: 1080.0 };
        let f = popup_frame(Rect { x: 1884.0, y: 900.0, w: 24.0, h: 24.0 }, right_work, SIZE);
        assert_eq!(f.right(), right_work.right() - EDGE);
    }

    #[test]
    fn taller_than_the_screen_is_cut_to_fit() {
        let f = popup_frame(Rect { x: 960.0, y: 1046.0, w: 24.0, h: 24.0 }, WORK, Size { w: 320.0, h: 5000.0 });
        assert!(f.h <= WORK.h - 2.0 * EDGE);
        assert!(f.y >= WORK.y + EDGE);
    }
}
