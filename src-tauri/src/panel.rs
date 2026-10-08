// Ported from upstream Panel/FloatingPanelController.swift, PanelPointerWatcher.swift
// and the drag half of FloatingPanel.sendEvent.
//! The floating panel window: transparent, always on top, never activated.
//!
//! Input model (same as upstream, re-expressed for Win32):
//! - The window is click-through (`WS_EX_TRANSPARENT`) everywhere except the
//!   hit rects the UI reports (rail, open card). A sampler on its own thread
//!   reads the cursor and flips the style, so the desktop behind the
//!   transparent margin keeps working.
//! - Hover enter *and* leave come from that sampler, never from DOM events:
//!   a non-activated WebView does not reliably deliver mouseleave.
//! - A press on the rail starts a drag that the sampler carries until the
//!   button is released; a release without movement is a click.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::placement::{Dock, Edge, Geometry, Layout, Placement, Rect, Size, DOCK_DISTANCE};
use crate::win;

pub const LABEL: &str = "panel";

/// Movement before a press becomes a drag, in DIPs.
const DRAG_SLOP: f64 = 3.0;

#[derive(Default)]
pub struct PanelState {
    pub geometry: Option<Geometry>,
    pub placement: Placement,
    pub layout: Option<Layout>,
    pub hit_rects: Vec<Rect>,
    pub grab_area: Option<Rect>,
    pub monitor: Option<win::MonitorInfo>,
    press: Option<Press>,
    last_pointer: Option<(i32, i32)>,
    click_through: Option<bool>,
}

struct Press {
    /// Pointer minus rail origin, screen DIPs.
    grab: (f64, f64),
    start: (f64, f64),
    local: (f64, f64),
    dragging: bool,
}

pub type SharedPanel = Arc<Mutex<PanelState>>;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct PointerEvent {
    /// Window-local DIPs, or None once the pointer has left the window.
    point: Option<(f64, f64)>,
    pressed: bool,
    dragging: bool,
}

pub fn create(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    let window = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("panel.html".into()))
        .title("Pulse")
        .transparent(true)
        .decorations(false)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .focused(false)
        .visible(false)
        .inner_size(400.0, 600.0)
        .build()?;
    win::make_panel_window(&window);
    Ok(window)
}

/// Re-place the window from the stored placement and the UI's geometry.
pub fn place(app: &AppHandle, shared: &SharedPanel) {
    let Some(window) = app.get_webview_window(LABEL) else { return };
    let mut state = shared.lock().unwrap();
    let Some(geometry) = state.geometry else { return };
    let monitor = win::monitor_named(state.placement.display.as_deref()).unwrap_or_else(win::primary_monitor);
    let layout = state.placement.layout(monitor.work_dip(), &geometry);
    apply(&window, &monitor, &layout);
    state.monitor = Some(monitor);
    state.layout = Some(layout);
    drop(state);
    let _ = app.emit_to(LABEL, "panel-layout", layout);
}

fn apply(window: &WebviewWindow, monitor: &win::MonitorInfo, layout: &Layout) {
    win::set_frame_dip(window, layout.frame, monitor.scale);
}

pub fn start_sampler(app: AppHandle, shared: SharedPanel) {
    std::thread::spawn(move || {
        let mut last_active = Instant::now();
        loop {
            let near = tick(&app, &shared);
            if near {
                last_active = Instant::now();
            }
            // 60 Hz while the pointer is on or near the panel, ~8 Hz otherwise.
            let idle = last_active.elapsed() > Duration::from_millis(500);
            std::thread::sleep(Duration::from_millis(if idle { 120 } else { 16 }));
        }
    });
}

/// One sampler pass. Returns whether the pointer is over or near the window.
fn tick(app: &AppHandle, shared: &SharedPanel) -> bool {
    let Some(window) = app.get_webview_window(LABEL) else { return false };
    if !window.is_visible().unwrap_or(false) {
        return false;
    }
    let Some(cursor) = win::cursor_pos() else { return false };
    let button = win::left_button_down();

    let mut state = shared.lock().unwrap();
    let Some(frame_px) = win::window_rect_px(&window) else { return false };
    let scale = state.monitor.as_ref().map(|m| m.scale).unwrap_or(1.0);
    let local = (
        (cursor.0 - frame_px.0) as f64 / scale,
        (cursor.1 - frame_px.1) as f64 / scale,
    );
    let frame_w = (frame_px.2 - frame_px.0) as f64 / scale;
    let frame_h = (frame_px.3 - frame_px.1) as f64 / scale;
    let inside_window = local.0 >= 0.0 && local.1 >= 0.0 && local.0 < frame_w && local.1 < frame_h;
    let screen_dip = (cursor.0 as f64 / scale, cursor.1 as f64 / scale);

    // Drag carry / release.
    let mut click: Option<(f64, f64)> = None;
    if let Some(press) = state.press.as_mut() {
        if button {
            let moved = (screen_dip.0 - press.start.0).hypot(screen_dip.1 - press.start.1);
            if press.dragging || moved > DRAG_SLOP {
                press.dragging = true;
                let grab = press.grab;
                drop(state);
                carry(app, shared, &window, cursor, grab);
                state = shared.lock().unwrap();
            }
        } else {
            let press = state.press.take().unwrap();
            if !press.dragging {
                click = Some(press.local);
            }
        }
    }

    let over_hit = inside_window && state.hit_rects.iter().any(|r| r.contains(local.0, local.1));
    let pressed = state.press.is_some();
    let dragging = state.press.as_ref().is_some_and(|p| p.dragging);
    let wants_input = over_hit || pressed;
    if state.click_through != Some(!wants_input) {
        win::set_click_through(&window, !wants_input);
        state.click_through = Some(!wants_input);
    }

    let rounded = inside_window.then(|| ((local.0 * 4.0) as i32, (local.1 * 4.0) as i32));
    let changed = rounded != state.last_pointer;
    state.last_pointer = rounded;
    drop(state);

    if changed || click.is_some() {
        let _ = app.emit_to(
            LABEL,
            "pointer",
            PointerEvent { point: inside_window.then_some(local), pressed, dragging },
        );
    }
    if let Some(point) = click {
        let _ = app.emit_to(LABEL, "rail-click", point);
    }
    inside_window || pressed
}

/// The UI saw a press inside the grab area at window-local `local`.
pub fn press(shared: &SharedPanel, local: (f64, f64)) {
    let mut state = shared.lock().unwrap();
    let (Some(layout), Some(monitor)) = (state.layout, state.monitor.clone()) else { return };
    let Some(grab_area) = state.grab_area.or(Some(layout.rail)) else { return };
    if !grab_area.contains(local.0, local.1) {
        return;
    }
    let Some(cursor) = win::cursor_pos() else { return };
    let screen = (cursor.0 as f64 / monitor.scale, cursor.1 as f64 / monitor.scale);
    let rail_origin = (layout.frame.x + layout.rail.x, layout.frame.y + layout.rail.y);
    state.press = Some(Press {
        grab: (screen.0 - rail_origin.0, screen.1 - rail_origin.1),
        start: screen,
        local,
        dragging: false,
    });
}

/// Upstream `FloatingPanel.carry()`: choose the landing dock from where the
/// pointer is, re-centre the grab when the rail changes shape, record, re-place.
fn carry(app: &AppHandle, shared: &SharedPanel, window: &WebviewWindow, cursor_px: (i32, i32), grab: (f64, f64)) {
    let monitor = win::monitor_at(cursor_px);
    let visible = monitor.work_dip();
    let pointer = (cursor_px.0 as f64 / monitor.scale, cursor_px.1 as f64 / monitor.scale);

    let mut state = shared.lock().unwrap();
    let (Some(geometry), Some(layout)) = (state.geometry, state.layout) else { return };
    let rail = Size { w: layout.rail.w, h: layout.rail.h };
    let current_vertical = state.placement.edge().is_vertical();

    let wanted = (
        (pointer.0 - grab.0).max(visible.x).min(visible.right() - rail.w),
        (pointer.1 - grab.1).max(visible.y).min(visible.bottom() - rail.h),
    );
    let rail_bottom = pointer.1 - grab.1 + rail.h;
    let stays_on_bottom = state.placement.dock == Dock::Edge(Edge::Bottom)
        && monitor.bounds_dip().bottom() - rail_bottom <= DOCK_DISTANCE;

    let dock = if pointer.1 - visible.y <= DOCK_DISTANCE {
        Dock::Edge(Edge::Top)
    } else if stays_on_bottom || monitor.bounds_dip().bottom() - pointer.1 <= DOCK_DISTANCE {
        Dock::Edge(Edge::Bottom)
    } else if visible.bottom() - pointer.1 <= DOCK_DISTANCE {
        Dock::Floating(false)
    } else if wanted.0 - visible.x <= DOCK_DISTANCE {
        Dock::Edge(Edge::Left)
    } else if visible.right() - (wanted.0 + rail.w) <= DOCK_DISTANCE {
        Dock::Edge(Edge::Right)
    } else {
        Dock::Floating(current_vertical)
    };

    let landing_vertical = match dock {
        Dock::Edge(e) => e.is_vertical(),
        Dock::Floating(v) => v,
    };
    let landing_rail = geometry.for_dock(landing_vertical, matches!(dock, Dock::Edge(_))).rail;

    let mut new_grab = grab;
    let origin = if landing_vertical != current_vertical {
        new_grab = (landing_rail.w / 2.0, landing_rail.h / 2.0);
        (pointer.0 - new_grab.0, pointer.1 - new_grab.1)
    } else if landing_rail != rail {
        new_grab = (
            grab.0 - (rail.w - landing_rail.w) / 2.0,
            grab.1 - (rail.h - landing_rail.h) / 2.0,
        );
        (
            (pointer.0 - new_grab.0).max(visible.x).min(visible.right() - landing_rail.w),
            (pointer.1 - new_grab.1).max(visible.y).min(visible.bottom() - landing_rail.h),
        )
    } else {
        wanted
    };

    let (h, v) = Placement::ratios(origin, visible, landing_rail);
    state.placement = Placement { dock, horizontal_ratio: h, vertical_ratio: v, display: Some(monitor.name.clone()) };
    if let Some(press) = state.press.as_mut() {
        press.grab = new_grab;
    }
    let layout = state.placement.layout(visible, &geometry);
    apply(window, &monitor, &layout);
    state.monitor = Some(monitor);
    state.layout = Some(layout);
    let placement = state.placement.clone();
    drop(state);
    crate::store::save_placement(app, &placement);
    let _ = app.emit_to(LABEL, "panel-layout", layout);
}
