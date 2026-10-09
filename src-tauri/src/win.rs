//! Thin Win32 helpers for the panel window. Everything geometric is converted
//! to DIPs here and nowhere else.

use std::sync::OnceLock;

use tauri::WebviewWindow;
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, MonitorFromPoint, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
    MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetSystemMetrics, IsWindowVisible, GetWindowLongPtrW, GetWindowRect, SetWindowLongPtrW, SetWindowPos,
    GWL_EXSTYLE, HWND_TOPMOST, SM_SWAPBUTTON, SWP_NOACTIVATE, SWP_NOZORDER, WS_EX_APPWINDOW, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT,
};

use crate::placement::Rect;

#[derive(Debug, Clone)]
pub struct MonitorInfo {
    pub name: String,
    /// Physical pixels.
    pub bounds: RECT,
    pub work: RECT,
    pub scale: f64,
}

impl MonitorInfo {
    fn dip(&self, r: RECT) -> Rect {
        Rect {
            x: r.left as f64 / self.scale,
            y: r.top as f64 / self.scale,
            w: (r.right - r.left) as f64 / self.scale,
            h: (r.bottom - r.top) as f64 / self.scale,
        }
    }
    pub fn work_dip(&self) -> Rect {
        self.dip(self.work)
    }
    pub fn bounds_dip(&self) -> Rect {
        self.dip(self.bounds)
    }
}

/// The panel's HWND, read once on the main thread. `WebviewWindow::hwnd()` is a
/// getter that round-trips through the main thread, so calling it from the
/// sampler while a command on the main thread waits for the panel lock deadlocks.
static PANEL_HWND: OnceLock<isize> = OnceLock::new();

fn hwnd(window: &WebviewWindow) -> Option<HWND> {
    if window.label() == crate::panel::LABEL {
        if let Some(h) = PANEL_HWND.get() {
            return Some(HWND(*h as _));
        }
        let h = window.hwnd().ok()?.0 as isize;
        let _ = PANEL_HWND.set(h);
        return Some(HWND(h as _));
    }
    window.hwnd().ok().map(|h| HWND(h.0 as _))
}

/// Whether the window is shown, straight from Win32 (no main-thread round trip).
pub fn is_visible(window: &WebviewWindow) -> bool {
    hwnd(window).is_some_and(|h| unsafe { IsWindowVisible(h).as_bool() })
}

pub fn make_panel_window(window: &WebviewWindow) {
    let Some(hwnd) = hwnd(window) else { return };
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        let ex = (ex & !WS_EX_APPWINDOW.0) | WS_EX_NOACTIVATE.0 | WS_EX_TOOLWINDOW.0 | WS_EX_LAYERED.0;
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex as isize);
    }
}

pub fn set_click_through(window: &WebviewWindow, through: bool) {
    let Some(hwnd) = hwnd(window) else { return };
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        let ex = if through { ex | WS_EX_TRANSPARENT.0 } else { ex & !WS_EX_TRANSPARENT.0 };
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex as isize);
    }
}

/// Places the window at `frame` (DIPs on a monitor of `scale`), rounding size up.
pub fn set_frame_dip(window: &WebviewWindow, frame: Rect, scale: f64) {
    let Some(hwnd) = hwnd(window) else { return };
    let x = (frame.x * scale).round() as i32;
    let y = (frame.y * scale).round() as i32;
    let w = (frame.w * scale).ceil() as i32;
    let h = (frame.h * scale).ceil() as i32;
    unsafe {
        let _ = SetWindowPos(hwnd, HWND_TOPMOST, x, y, w, h, SWP_NOACTIVATE);
    }
}

pub fn raise_topmost(window: &WebviewWindow) {
    let Some(hwnd) = hwnd(window) else { return };
    let mut r = RECT::default();
    unsafe {
        if GetWindowRect(hwnd, &mut r).is_ok() {
            let _ = SetWindowPos(hwnd, HWND_TOPMOST, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOACTIVATE);
        }
    }
    let _ = SWP_NOZORDER;
}

pub fn window_rect_px(window: &WebviewWindow) -> Option<(i32, i32, i32, i32)> {
    let hwnd = hwnd(window)?;
    let mut r = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut r).ok()? };
    Some((r.left, r.top, r.right, r.bottom))
}

pub fn cursor_pos() -> Option<(i32, i32)> {
    let mut p = POINT::default();
    unsafe { GetCursorPos(&mut p).ok()? };
    Some((p.x, p.y))
}

/// The *primary* button, honouring swapped mouse buttons.
pub fn left_button_down() -> bool {
    unsafe {
        let swapped = GetSystemMetrics(SM_SWAPBUTTON) != 0;
        let vk = if swapped { 0x02 } else { VK_LBUTTON.0 as i32 };
        (GetAsyncKeyState(vk) as u16 & 0x8000) != 0
    }
}

fn info(monitor: HMONITOR) -> Option<MonitorInfo> {
    unsafe {
        let mut ex = MONITORINFOEXW::default();
        ex.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if !GetMonitorInfoW(monitor, &mut ex as *mut MONITORINFOEXW as *mut MONITORINFO).as_bool() {
            return None;
        }
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        let len = ex.szDevice.iter().position(|&c| c == 0).unwrap_or(ex.szDevice.len());
        Some(MonitorInfo {
            name: String::from_utf16_lossy(&ex.szDevice[..len]),
            bounds: ex.monitorInfo.rcMonitor,
            work: ex.monitorInfo.rcWork,
            scale: dx as f64 / 96.0,
        })
    }
}

pub fn primary_monitor() -> MonitorInfo {
    unsafe { info(MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY)) }.expect("primary monitor")
}

pub fn monitor_at(px: (i32, i32)) -> MonitorInfo {
    unsafe { info(MonitorFromPoint(POINT { x: px.0, y: px.1 }, MONITOR_DEFAULTTONEAREST)) }
        .unwrap_or_else(primary_monitor)
}

pub fn all_monitors() -> Vec<MonitorInfo> {
    unsafe extern "system" fn collect(m: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        let list = &mut *(data.0 as *mut Vec<HMONITOR>);
        list.push(m);
        BOOL(1)
    }
    let mut handles: Vec<HMONITOR> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(collect), LPARAM(&mut handles as *mut _ as isize));
    }
    handles.into_iter().filter_map(info).collect()
}

pub fn monitor_named(name: Option<&str>) -> Option<MonitorInfo> {
    let name = name?;
    all_monitors().into_iter().find(|m| m.name == name)
}
