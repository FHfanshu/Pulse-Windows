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

#[derive(Debug, Clone, PartialEq)]
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
    let (x, y, right, bottom) = frame_px(frame, scale);
    unsafe {
        let _ = SetWindowPos(hwnd, HWND_TOPMOST, x, y, right - x, bottom - y, SWP_NOACTIVATE);
    }
}

pub fn frame_px(frame: Rect, scale: f64) -> (i32, i32, i32, i32) {
    let x = (frame.x * scale).round() as i32;
    let y = (frame.y * scale).round() as i32;
    (x, y, x + (frame.w * scale).ceil() as i32, y + (frame.h * scale).ceil() as i32)
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

pub fn monitor_count() -> usize {
    all_monitors().len()
}

/// The monitor a full-screen window in front covers, if any: the foreground
/// window's rect contains the whole monitor (taskbar included, which a merely
/// maximized window does not). The desktop and the taskbar don't count.
pub fn fullscreen_monitor(except: Option<&WebviewWindow>) -> Option<String> {
    use windows::Win32::Graphics::Gdi::{MonitorFromWindow, MONITOR_DEFAULTTONULL};
    use windows::Win32::UI::WindowsAndMessaging::{GetClassNameW, GetForegroundWindow};
    unsafe {
        let fg = GetForegroundWindow();
        if fg.0.is_null() || except.and_then(hwnd).is_some_and(|h| h == fg) {
            return None;
        }
        let mut class = [0u16; 64];
        let len = GetClassNameW(fg, &mut class) as usize;
        let class = String::from_utf16_lossy(&class[..len]);
        if matches!(class.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd") {
            return None;
        }
        let mut r = RECT::default();
        GetWindowRect(fg, &mut r).ok()?;
        let monitor = info(MonitorFromWindow(fg, MONITOR_DEFAULTTONULL))?;
        let b = monitor.bounds;
        (r.left <= b.left && r.top <= b.top && r.right >= b.right && r.bottom >= b.bottom).then_some(monitor.name)
    }
}

/// `HKCU\…\Run\Pulse`: start at login.
pub mod login_item {
    use windows::core::{w, HSTRING, PCWSTR};
    use windows::Win32::System::Registry::{
        RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ,
    };

    const KEY: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Run");
    const NAME: PCWSTR = w!("Pulse");

    pub fn command(exe: &std::path::Path) -> String {
        format!("\"{}\"", exe.display())
    }

    pub fn current() -> Option<String> {
        let mut buf = vec![0u16; 1024];
        let mut size = (buf.len() * 2) as u32;
        unsafe {
            RegGetValueW(HKEY_CURRENT_USER, KEY, NAME, RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr().cast()), Some(&mut size))
                .ok()
                .ok()?;
        }
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..len]))
    }

    pub fn set(enabled: bool, exe: &std::path::Path) {
        unsafe {
            if enabled {
                let value = HSTRING::from(command(exe));
                let bytes = std::slice::from_raw_parts(value.as_ptr().cast::<u8>(), (value.len() + 1) * 2);
                let _ = RegSetKeyValueW(HKEY_CURRENT_USER, KEY, NAME, REG_SZ.0, Some(bytes.as_ptr().cast()), bytes.len() as u32);
            } else {
                let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, KEY, NAME);
            }
        }
    }
}

/// Keep the window out of screen captures (and so out of [`capture_screen`]'s own reading of
/// what is behind it). Windows 10 2004 or later; earlier builds ignore it.
pub fn exclude_from_capture(window: &WebviewWindow, exclude: bool) {
    use windows::Win32::UI::WindowsAndMessaging::{SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE, WDA_NONE};
    let Some(hwnd) = hwnd(window) else { return };
    unsafe {
        let _ = SetWindowDisplayAffinity(hwnd, if exclude { WDA_EXCLUDEFROMCAPTURE } else { WDA_NONE });
    }
}

/// The screen inside `rect` (physical pixels: left, top, right, bottom), shrunk to `w` x `h` with
/// halftone averaging. BGRA rows, top row first.
pub fn capture_screen(rect: (i32, i32, i32, i32), w: i32, h: i32) -> Option<Vec<u8>> {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject, SetBrushOrgEx,
        SetStretchBltMode, StretchBlt, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HALFTONE, SRCCOPY,
    };
    unsafe {
        let screen = GetDC(HWND::default());
        let memory = CreateCompatibleDC(screen);
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let mut out = None;
        if let Ok(bitmap) = CreateDIBSection(memory, &info, DIB_RGB_COLORS, &mut bits, HANDLE::default(), 0) {
            let previous = SelectObject(memory, bitmap);
            SetStretchBltMode(memory, HALFTONE);
            let _ = SetBrushOrgEx(memory, 0, 0, None);
            let copied = StretchBlt(memory, 0, 0, w, h, screen, rect.0, rect.1, rect.2 - rect.0, rect.3 - rect.1, SRCCOPY);
            if copied.as_bool() && !bits.is_null() {
                out = Some(std::slice::from_raw_parts(bits as *const u8, (w * h * 4) as usize).to_vec());
            }
            SelectObject(memory, previous);
            let _ = DeleteObject(bitmap);
        }
        let _ = DeleteDC(memory);
        ReleaseDC(HWND::default(), screen);
        out
    }
}
