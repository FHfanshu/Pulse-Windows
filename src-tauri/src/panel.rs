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
    last_buttons: (bool, bool),
    pointer_dirty: bool,
}

impl PanelState {
    fn display_update(&mut self, monitor: win::MonitorInfo, frame_px: Option<(i32, i32, i32, i32)>) -> Option<Layout> {
        let geometry = self.geometry?;
        let changed = self.monitor.as_ref() != Some(&monitor);
        // Keep an active drag authoritative unless its coordinate system changed.
        if !changed && self.press.is_some() {
            return None;
        }
        let layout = self.placement.layout(monitor.work_dip(), &geometry);
        if !changed && frame_px == Some(win::frame_px(layout.frame, monitor.scale)) {
            return None;
        }
        self.press = None;
        self.last_pointer = None;
        self.last_buttons = (false, false);
        self.pointer_dirty = true;
        self.monitor = Some(monitor);
        self.layout = Some(layout);
        Some(layout)
    }

    fn record_pointer(&mut self, point: Option<(i32, i32)>, pressed: bool, dragging: bool) -> bool {
        let changed = self.pointer_dirty || point != self.last_pointer || (pressed, dragging) != self.last_buttons;
        self.pointer_dirty = false;
        self.last_pointer = point;
        self.last_buttons = (pressed, dragging);
        changed
    }
}

struct Press {
    /// Pointer minus rail origin, screen DIPs.
    grab: (f64, f64),
    start: (f64, f64),
    local: (f64, f64),
    dragging: bool,
}

pub type SharedPanel = Arc<Mutex<PanelState>>;

/// The pointer is on the panel or a press is under way: not a moment to move it.
pub fn is_busy(state: &PanelState) -> bool {
    state.press.is_some() || state.last_pointer.is_some()
}

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

/// Display changes do not change the device name. Refresh the cached DPI and
/// work area even while hidden, and repair a native DPI-suggested window frame.
pub fn reconcile_display(app: &AppHandle, shared: &SharedPanel) {
    let Some(window) = app.get_webview_window(LABEL) else { return };
    let mut state = shared.lock().unwrap();
    let monitor = win::monitor_named(state.placement.display.as_deref()).unwrap_or_else(win::primary_monitor);
    let Some(layout) = state.display_update(monitor.clone(), win::window_rect_px(&window)) else { return };
    apply(&window, &monitor, &layout);
    drop(state);
    // The sampler reports a fresh pointer even if it has not moved. Emitting a
    // reset here could race and overwrite that fresh event after unlocking.
    let _ = app.emit_to(LABEL, "panel-layout", layout);
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
    if !win::is_visible(&window) {
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
    // A stationary release must clear dragging in React too; otherwise the
    // hover card remains suppressed until a later pointer movement.
    let changed = state.record_pointer(rounded, pressed, dragging);
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
    let _ = app.emit("placement-changed", &placement);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::placement::Shapes;
    use windows::Win32::Foundation::RECT;

    fn monitor() -> win::MonitorInfo {
        win::MonitorInfo {
            name: "display".into(),
            bounds: RECT { left: 0, top: 0, right: 1920, bottom: 1080 },
            work: RECT { left: 0, top: 0, right: 1920, bottom: 1040 },
            scale: 1.0,
        }
    }

    fn state() -> PanelState {
        let shapes = Shapes { panel: Size { w: 342.0, h: 600.0 }, rail: Size { w: 64.0, h: 400.0 } };
        let mut state = PanelState {
            geometry: Some(Geometry {
                vertical_docked: shapes, vertical_free: shapes,
                horizontal_docked: shapes, horizontal_free: shapes,
            }),
            ..Default::default()
        };
        state.display_update(monitor(), None).unwrap();
        state
    }

    fn press() -> Press {
        Press { grab: (32.0, 20.0), start: (1800.0, 400.0), local: (310.0, 120.0), dragging: true }
    }

    #[test]
    fn same_display_dpi_change_reanchors_and_refreshes_hit_coordinates() {
        let mut state = state();
        state.press = Some(press());
        state.record_pointer(Some((1240, 480)), true, true);
        let mut target = monitor();
        target.scale = 1.5;
        let old_frame = win::frame_px(state.layout.unwrap().frame, 1.0);
        let layout = state.display_update(target.clone(), Some(old_frame)).unwrap();
        assert_eq!(win::frame_px(layout.frame, target.scale).2, target.work.right);
        assert_eq!(state.monitor.as_ref().unwrap().scale, 1.5);
        assert!(state.press.is_none() && state.last_pointer.is_none());
        assert!(state.record_pointer(None, false, false));
        // The pointer at the newly scaled ring still resolves to window-local DIPs.
        let ring_x_px = (layout.frame.x + layout.rail.x + 32.0) * target.scale;
        let local_x = (ring_x_px - layout.frame.x * target.scale) / state.monitor.unwrap().scale;
        assert!(layout.rail.contains(local_x, layout.rail.y + 20.0));
    }

    #[test]
    fn resolution_work_area_and_monitor_removal_recompute_layout() {
        let mut state = state();
        for target in [
            win::MonitorInfo { bounds: RECT { right: 2560, bottom: 1440, ..monitor().bounds }, work: RECT { right: 2560, bottom: 1400, ..monitor().work }, ..monitor() },
            win::MonitorInfo { work: RECT { left: 80, top: 40, right: 1840, bottom: 1000 }, ..monitor() },
            win::MonitorInfo { name: "fallback display".into(), ..monitor() },
        ] {
            let layout = state.display_update(target.clone(), None).unwrap();
            assert_eq!(layout.frame.right(), target.work_dip().right());
            assert_eq!(state.monitor.as_ref(), Some(&target));
            assert_eq!(state.placement.vertical_ratio, 0.5);
            assert_eq!(state.placement.dock, Dock::Edge(Edge::Right));
        }
    }

    #[test]
    fn native_frame_drift_is_repaired_without_interrupting_an_unchanged_drag() {
        let mut state = state();
        let expected = win::frame_px(state.layout.unwrap().frame, 1.0);
        assert!(state.display_update(monitor(), Some(expected)).is_none());
        state.press = Some(press());
        assert!(state.display_update(monitor(), Some((0, 0, 342, 600))).is_none());
        assert!(state.press.is_some());
        state.press = None;
        let layout = state.display_update(monitor(), Some((0, 0, 342, 600))).unwrap();
        assert_eq!(win::frame_px(layout.frame, 1.0), expected);
    }

    #[test]
    fn stationary_drag_release_reports_state_change_for_hover_card() {
        let mut state = state();
        let point = Some((1240, 480));
        assert!(state.record_pointer(point, true, true));
        assert!(!state.record_pointer(point, true, true));
        assert!(state.record_pointer(point, false, false));
        assert!(!state.record_pointer(point, false, false));
    }
}
