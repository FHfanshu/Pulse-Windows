//! Thin Win32 helpers for the panel window. Everything geometric is converted
//! to DIPs here and nowhere else.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use tauri::WebviewWindow;
use windows::Win32::Foundation::{BOOL, COLORREF, HWND, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, MonitorFromPoint, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
    MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetSystemMetrics, IsWindowVisible, GetWindowLongPtrW, GetWindowRect, SetLayeredWindowAttributes, SetWindowLongPtrW, SetWindowPos,
    GWL_EXSTYLE, HWND_TOPMOST, LWA_ALPHA, SM_SWAPBUTTON, SWP_NOACTIVATE, SWP_NOZORDER, WS_EX_APPWINDOW, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT,
};

use crate::outline::Seg;
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

/// The click-through state the panel *wants* (`WS_EX_TRANSPARENT` or not). The sampler sets it as
/// the pointer enters and leaves the drawn shapes; the UI-thread restore below reads it when it
/// runs, so an old queued pass always applies the newest state and cannot undo a newer one. Starts
/// `true`: before the UI reports shapes, the transparent window margins must pass clicks on.
static DESIRED_THROUGH: AtomicBool = AtomicBool::new(true);

/// A restore pass is queued on the UI thread and re-reads the desired state when it runs. At most
/// one pass is in flight, so a stationary pointer does not queue work on every sampler tick.
static INPUT_PENDING: AtomicBool = AtomicBool::new(false);

/// The panel's extended styles for an existing `ex`: drop the taskbar entry (`WS_EX_APPWINDOW`),
/// force the non-activating / tool-window / layered base, then set or clear `WS_EX_TRANSPARENT`
/// from the desired click-through state. Unrelated flags (e.g. `WS_EX_TOPMOST`) are preserved.
/// Pure, so the mapping is testable without a window.
fn panel_ex_style(ex: u32, through: bool) -> u32 {
    let base = (ex & !WS_EX_APPWINDOW.0) | WS_EX_NOACTIVATE.0 | WS_EX_TOOLWINDOW.0 | WS_EX_LAYERED.0;
    if through { base | WS_EX_TRANSPARENT.0 } else { base & !WS_EX_TRANSPARENT.0 }
}

/// Whether Win32 already holds the styles the panel wants. A plain `GetWindowLongPtrW` read; sends
/// no message, so it is safe off the UI thread and cheap enough for every sampler tick.
fn panel_style_matches(hwnd: HWND) -> bool {
    let ex = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 };
    ex == panel_ex_style(ex, DESIRED_THROUGH.load(Ordering::Acquire))
}

/// Writes `panel_ex_style` onto `hwnd`; returns whether it changed. Adding `WS_EX_LAYERED` for the
/// first time also initializes its attributes (a layered window is invisible until it has them),
/// keeping the global alpha opaque — the transparent WebView supplies the per-pixel alpha. A window
/// that was already layered is left with its attributes (and its acrylic) untouched.
fn apply_panel_style(hwnd: HWND, through: bool) -> bool {
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        let want = panel_ex_style(ex, through);
        if want == ex {
            return false;
        }
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, want as isize);
        if want & WS_EX_LAYERED.0 != 0 && ex & WS_EX_LAYERED.0 == 0 {
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
        }
        true
    }
}

/// Apply the latest desired click-through state. UI thread only; the queued pass and the native
/// tests use it. Returns whether anything changed.
fn apply_panel_input(hwnd: HWND) -> bool {
    apply_panel_style(hwnd, DESIRED_THROUGH.load(Ordering::Acquire))
}

/// Claim the panel's native input styles, from the UI thread, after whatever Tao did.
///
/// Windows pitfall: Tao owns `GWL_EXSTYLE` as well. Every time it changes the window's flags it
/// recomputes the extended styles from its *own* cached flags and calls `SetWindowLong(GWL_EXSTYLE)`.
/// `show` does this (`WindowMessage::Show` → `set_visible`), and `skip_taskbar` only removes the
/// taskbar tab — it does not clear `WS_EX_APPWINDOW`. Tao does not know `WS_EX_TOOLWINDOW`, and
/// neither `WS_EX_TRANSPARENT` nor the static `WS_EX_LAYERED` comes from a builder flag, so a shown
/// panel comes back with `WS_EX_APPWINDOW` re-added and `WS_EX_TOOLWINDOW` / `WS_EX_LAYERED` /
/// `WS_EX_TRANSPARENT` gone. (The non-activating `WS_EX_NOACTIVATE` now survives because `create`
/// builds the window `focusable(false)`, which makes Tao's own `to_window_styles` emit it.) Posting
/// the work to the UI thread makes it run after Tao's rewrite when the caller is off the UI thread
/// (both go through the same `show`/task queue), and inline when the caller is already there (having
/// shown first).
///
/// `force` is for after a `show`, where Tao's rewrite is ahead; the sampler leaves it false so a
/// stationary pointer only reads the styles and queues nothing. If the desired state changes while a
/// pass waits or runs, or the write itself fails, the next sampler tick sees the mismatch and queues
/// another pass — so the pass itself stays a single attempt.
pub fn make_panel_window(window: &WebviewWindow) {
    claim_panel_input(window, true);
}

/// Set the pointer-driven click-through state and bring the window in line. Cheap when nothing
/// changed.
fn claim_panel_input(window: &WebviewWindow, force: bool) {
    let Some(hwnd) = hwnd(window) else { return };
    if !force && (INPUT_PENDING.load(Ordering::Acquire) || panel_style_matches(hwnd)) {
        return;
    }
    // A forced call (after a show) queues its own pass even if another is pending: it has to run
    // after the show, which the pending pass may have been queued before. `HWND` is not `Send`;
    // carry its address into the pass.
    if INPUT_PENDING.swap(true, Ordering::AcqRel) && !force {
        return;
    }
    let address = hwnd.0 as isize;
    let queued = window.run_on_main_thread(move || {
        apply_panel_input(HWND(address as *mut _));
        INPUT_PENDING.store(false, Ordering::Release);
    });
    if queued.is_err() {
        INPUT_PENDING.store(false, Ordering::Release);
    }
}

/// Windows difference: the panel's click-through *region* is the shape that is actually drawn, not the
/// window's rectangle. macOS routes clicks through the transparent pixels of a non-opaque window by
/// itself; Windows does not, so the panel toggles `WS_EX_TRANSPARENT` (which passes input to windows
/// underneath, in other processes too — `WM_NCHITTEST`'s `HTTRANSPARENT` would only skip windows on
/// this thread) as the pointer enters and leaves the visible shapes. The test that decides that is the
/// same SVG outline the acrylic is clipped to (`crate::outline`), so a card's full-width hover band and
/// a rail's rectangular corners no longer take clicks meant for the app behind them.
///
/// One shape's path in its own space together with where it sits in the window (DIPs).
struct InputShape {
    segs: Vec<Seg>,
    x: f64,
    y: f64,
    s: f64,
}

#[derive(Default)]
struct InputRegion {
    rail: Option<InputShape>,
    card: Option<InputShape>,
}

impl InputRegion {
    /// Whether a window-local DIP point is inside either visible shape.
    fn hit(&self, local: (f64, f64)) -> bool {
        let inside = |shape: &Option<InputShape>| {
            shape.as_ref().is_some_and(|shape| {
                let sx = (local.0 - shape.x) / shape.s;
                let sy = (local.1 - shape.y) / shape.s;
                crate::outline::contains(&shape.segs, sx as f32, sy as f32)
            })
        };
        inside(&self.rail) || inside(&self.card)
    }
}

/// The shapes the UI last reported, ready to test, with the `(epoch, seq)` snapshot they came from so
/// the sampler re-parses a path only when it moved. `host_backdrop` on the UI thread writes it and the
/// 60 Hz sampler reads it; the sampler is at most one sample behind, so the region and the shape being
/// drawn can differ for a frame while a shape moves. The writer takes only this lock; the sampler may
/// hold the panel lock as well, which the writer never takes, so the order cannot deadlock.
static INPUT_REGION: Mutex<InputState> =
    Mutex::new(InputState { epoch: 0, seq: 0, ready: false, region: InputRegion { rail: None, card: None } });

struct InputState {
    epoch: u64,
    seq: u64,
    /// Whether the UI has reported shapes at all. Separates "nothing to test against yet" (the sampler
    /// falls back to the rects) from a reported region that happens to have no shapes (nothing takes
    /// input), so an empty report never falls back to the coarser rectangles.
    ready: bool,
    region: InputRegion,
}

fn input_shape(outline: Option<&crate::outline::Outline>) -> Option<InputShape> {
    let outline = outline?;
    // A faded-out card must not take a click, and a degenerate scale would divide the point away.
    if outline.o <= 0.01 || !outline.s.is_finite() || outline.s <= 0.0 {
        return None;
    }
    Some(InputShape { segs: crate::outline::parse(&outline.d)?, x: outline.x, y: outline.y, s: outline.s })
}

/// Remember the visible shapes as the panel's input region. UI thread, from `host_backdrop`, which the
/// UI reports every frame the shapes move.
fn remember_input(shapes: &crate::outline::Shapes) {
    let mut state = INPUT_REGION.lock().unwrap();
    if state.ready && (state.epoch, state.seq) == (shapes.epoch, shapes.seq) {
        return;
    }
    state.epoch = shapes.epoch;
    state.seq = shapes.seq;
    state.ready = true;
    state.region = InputRegion {
        rail: input_shape(shapes.rail.as_ref()),
        card: input_shape(shapes.card.as_ref()),
    };
}

/// Whether a window-local DIP point belongs to the panel. `None` until the UI has reported its shapes,
/// when the caller falls back to the rougher rects `set_hit_rects` carries. Once they have been
/// reported the outlines decide — a report with no shapes means nothing is interactive. Readiness and
/// the test share one lock, so the two cannot disagree about which frame they saw.
pub fn input_hit(local: (f64, f64)) -> Option<bool> {
    let state = INPUT_REGION.lock().unwrap();
    if state.ready {
        Some(state.region.hit(local))
    } else {
        None
    }
}

pub fn set_click_through(window: &WebviewWindow, through: bool) {
    DESIRED_THROUGH.store(through, Ordering::Release);
    claim_panel_input(window, false);
}

/// Places the window at `frame` (DIPs on a monitor of `scale`), rounding size up.
pub fn set_frame_dip(window: &WebviewWindow, frame: Rect, scale: f64) {
    let Some(hwnd) = hwnd(window) else { return };
    let (x, y, right, bottom) = frame_px(frame, scale);
    unsafe {
        let _ = SetWindowPos(hwnd, HWND_TOPMOST, x, y, right - x, bottom - y, SWP_NOACTIVATE | cross_thread(hwnd));
    }
}

/// Windows pitfall: `SetWindowPos` (and `SetWindowLongPtr`) from another thread than the window's
/// *sends* messages to its thread and waits. The panel is moved from the sampler threads while they hold
/// the panel lock, and the UI thread takes that lock in its sync commands, so waiting for it deadlocked
/// the app ("Not Responding"). Off the UI thread the request is posted instead; calls keep their order.
fn cross_thread(hwnd: HWND) -> windows::Win32::UI::WindowsAndMessaging::SET_WINDOW_POS_FLAGS {
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowThreadProcessId, SWP_ASYNCWINDOWPOS, SET_WINDOW_POS_FLAGS};
    if unsafe { GetWindowThreadProcessId(hwnd, None) != GetCurrentThreadId() } {
        SWP_ASYNCWINDOWPOS
    } else {
        SET_WINDOW_POS_FLAGS(0)
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
            let _ = SetWindowPos(hwnd, HWND_TOPMOST, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOACTIVATE | cross_thread(hwnd));
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

/// `HKCU\…\Run\Pulse`: start at login. What the value should be and when to change it is
/// `pulse_core::login_item`; this is only the registry.
pub mod login_item {
    use windows::core::{w, HSTRING, PCWSTR};
    use windows::Win32::System::Registry::{
        RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_BINARY, RRF_RT_REG_SZ,
    };

    const KEY: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Run");
    /// Where Windows' Startup apps page and Task Manager record that the user switched an entry off.
    const APPROVED_KEY: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run");
    const NAME: PCWSTR = w!("Pulse");

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

    /// Write the Run value, `command` being `pulse_core::login_item::command`.
    pub fn write(command: &str) {
        unsafe {
            let value = HSTRING::from(command);
            let bytes = std::slice::from_raw_parts(value.as_ptr().cast::<u8>(), (value.len() + 1) * 2);
            let _ = RegSetKeyValueW(HKEY_CURRENT_USER, KEY, NAME, REG_SZ.0, Some(bytes.as_ptr().cast()), bytes.len() as u32);
        }
    }

    pub fn remove() {
        unsafe {
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, KEY, NAME);
        }
    }

    /// The Startup apps toggle's record for Pulse, if Windows has written one.
    pub fn startup_approved() -> Option<Vec<u8>> {
        let mut buf = [0u8; 32];
        let mut size = buf.len() as u32;
        unsafe {
            RegGetValueW(HKEY_CURRENT_USER, APPROVED_KEY, NAME, RRF_RT_REG_BINARY, None, Some(buf.as_mut_ptr().cast()), Some(&mut size))
                .ok()
                .ok()?;
        }
        Some(buf[..(size as usize).min(buf.len())].to_vec())
    }

    /// Forget the user's earlier "off" in Windows: a missing record means enabled. Done when the
    /// user switches Pulse on themselves, or the entry would stay off without a word.
    pub fn clear_startup_approved() {
        unsafe {
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, APPROVED_KEY, NAME);
        }
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

/// Windows difference: the acrylic behind the panel's shapes is a native Windows.UI.Composition layer
/// under the WebView (CSS `backdrop-filter` only sees the page, never the desktop). One layer per
/// shape (the rail, the open card), each a host-backdrop brush clipped to that shape's outline, so
/// the blur follows the SVG edge exactly. Owned by the UI thread and updated in place as the shapes
/// move. Composition uses physical pixels; WebView layout uses DIPs.
struct Layer {
    sprite: windows::UI::Composition::SpriteVisual,
    geometry: windows::UI::Composition::CompositionPathGeometry,
    /// What the layer was last set to (outline, window size, scale); the panel moves 60 times a second
    /// without any of these changing, and then nothing is done.
    applied: std::cell::RefCell<Option<(Option<crate::outline::Outline>, [f32; 2], u64)>>,
}

struct HostBackdrop {
    rail: Layer,
    card: Layer,
    factory: windows::Win32::Graphics::Direct2D::ID2D1Factory,
    _root: windows::UI::Composition::ContainerVisual,
    _target: windows::UI::Composition::Desktop::DesktopWindowTarget,
    _compositor: windows::UI::Composition::Compositor,
    _controller: Option<windows::System::DispatcherQueueController>,
}

thread_local! {
    static HOST_BACKDROP: std::cell::RefCell<Option<HostBackdrop>> = const { std::cell::RefCell::new(None) };
}

/// Hands Composition a Direct2D geometry as a `CompositionPath` source.
#[windows::core::implement(
    windows::Graphics::IGeometrySource2D,
    windows::Win32::System::WinRT::Graphics::Direct2D::IGeometrySource2DInterop
)]
struct GeometrySource(windows::Win32::Graphics::Direct2D::ID2D1Geometry);

impl windows::Graphics::IGeometrySource2D_Impl for GeometrySource_Impl {}

impl windows::Win32::System::WinRT::Graphics::Direct2D::IGeometrySource2DInterop_Impl for GeometrySource_Impl {
    fn GetGeometry(&self) -> windows::core::Result<windows::Win32::Graphics::Direct2D::ID2D1Geometry> {
        Ok(self.0.clone())
    }
    fn TryGetGeometryUsingFactory(
        &self,
        _factory: Option<&windows::Win32::Graphics::Direct2D::ID2D1Factory>,
    ) -> windows::core::Result<windows::Win32::Graphics::Direct2D::ID2D1Geometry> {
        Err(windows::Win32::Foundation::E_NOTIMPL.into())
    }
}

impl HostBackdrop {
    fn new(hwnd: HWND) -> windows::core::Result<Self> {
        use windows::core::Interface;
        use windows::Win32::Graphics::Direct2D::{D2D1CreateFactory, D2D1_FACTORY_TYPE_SINGLE_THREADED};
        use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWINDOWATTRIBUTE};
        use windows::Win32::System::WinRT::Composition::ICompositorDesktopInterop;
        use windows::Win32::System::WinRT::{CreateDispatcherQueueController, DispatcherQueueOptions, DQTAT_COM_NONE, DQTYPE_THREAD_CURRENT};
        use windows::UI::Composition::Compositor;
        let on = BOOL(1);
        unsafe { DwmSetWindowAttribute(hwnd, DWMWINDOWATTRIBUTE(17), (&on as *const BOOL).cast(), std::mem::size_of::<BOOL>() as u32)? };
        // The thread can already have a queue; creating a second one fails.
        let controller = if windows::System::DispatcherQueue::GetForCurrentThread().is_ok() {
            None
        } else {
            Some(unsafe { CreateDispatcherQueueController(DispatcherQueueOptions {
                dwSize: std::mem::size_of::<DispatcherQueueOptions>() as u32,
                threadType: DQTYPE_THREAD_CURRENT,
                apartmentType: DQTAT_COM_NONE,
            })? })
        };
        let factory = unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)? };
        let compositor = Compositor::new()?;
        let interop: ICompositorDesktopInterop = compositor.cast()?;
        let target = unsafe { interop.CreateDesktopWindowTarget(hwnd, false)? };
        let root = compositor.CreateContainerVisual()?;
        target.SetRoot(&root)?;
        let rail = Layer::new(&compositor, &root)?;
        let card = Layer::new(&compositor, &root)?;
        Ok(Self { rail, card, factory, _root: root, _target: target, _compositor: compositor, _controller: controller })
    }

    /// A Direct2D path geometry of `segs` as a `CompositionPath`.
    fn path(&self, segs: &[crate::outline::Seg]) -> windows::core::Result<windows::UI::Composition::CompositionPath> {
        use crate::outline::Seg;
        use windows::core::Interface;
        use windows::Win32::Graphics::Direct2D::Common::{D2D1_BEZIER_SEGMENT, D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_END_CLOSED, D2D_POINT_2F};
        use windows::Win32::Graphics::Direct2D::ID2D1Geometry;
        use windows::UI::Composition::CompositionPath;
        let pt = |p: [f32; 2]| D2D_POINT_2F { x: p[0], y: p[1] };
        let geometry = unsafe { self.factory.CreatePathGeometry()? };
        let sink = unsafe { geometry.Open()? };
        let mut figure = false;
        unsafe {
            for seg in segs {
                match *seg {
                    Seg::Move(p) => {
                        if figure {
                            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
                        }
                        sink.BeginFigure(pt(p), D2D1_FIGURE_BEGIN_FILLED);
                        figure = true;
                    }
                    Seg::Line(p) => sink.AddLine(pt(p)),
                    Seg::Cubic(a, b, c) => sink.AddBezier(&D2D1_BEZIER_SEGMENT { point1: pt(a), point2: pt(b), point3: pt(c) }),
                    Seg::Close => {
                        if figure {
                            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
                        }
                        figure = false;
                    }
                }
            }
            if figure {
                sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            }
            sink.Close()?;
        }
        let source: windows::Graphics::IGeometrySource2D = GeometrySource(geometry.cast::<ID2D1Geometry>()?).into();
        CompositionPath::Create(&source)
    }
}

impl Layer {
    fn new(compositor: &windows::UI::Composition::Compositor, root: &windows::UI::Composition::ContainerVisual) -> windows::core::Result<Self> {
        let sprite = compositor.CreateSpriteVisual()?;
        sprite.SetBrush(&compositor.CreateHostBackdropBrush()?)?;
        let geometry = compositor.CreatePathGeometry()?;
        sprite.SetClip(&compositor.CreateGeometricClipWithGeometry(&geometry)?)?;
        sprite.SetIsVisible(false)?;
        root.Children()?.InsertAtTop(&sprite)?;
        Ok(Self { sprite, geometry, applied: Default::default() })
    }

    /// Clips the layer to `outline` (window DIPs; `px` is the window's scale) or hides it.
    fn set(&self, backdrop: &HostBackdrop, size: windows::Foundation::Numerics::Vector2, outline: Option<&crate::outline::Outline>, px: f64, layer: &str) -> windows::core::Result<()> {
        let key = (outline.cloned(), [size.X, size.Y], px.to_bits());
        if self.applied.borrow().as_ref() == Some(&key) {
            return Ok(());
        }
        let segs = outline.and_then(|o| crate::outline::parse(&o.d).map(|segs| crate::outline::place(&segs, o, px)));
        match (outline, segs) {
            (Some(outline), Some(segs)) => {
                self.geometry.SetPath(&backdrop.path(&segs)?)?;
                // The sprite covers the window; the clip is the shape.
                self.sprite.SetSize(size)?;
                self.sprite.SetOpacity(outline.o.clamp(0.0, 1.0) as f32)?;
                self.sprite.SetIsVisible(true)?;
            }
            _ => self.sprite.SetIsVisible(false)?,
        }
        if std::env::var_os("PULSE_BACKDROP_TRACE").is_some() {
            let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis();
            eprintln!("LAYER {at} {layer} visible={} opacity={}", self.sprite.IsVisible().unwrap_or(false), self.sprite.Opacity().unwrap_or(0.0));
        }
        *self.applied.borrow_mut() = Some(key);
        Ok(())
    }

    fn hide(&self) -> windows::core::Result<()> {
        if self.applied.borrow_mut().take().is_some() {
            self.sprite.SetIsVisible(false)?;
        }
        Ok(())
    }
}

/// UI thread only. `active` false hides the native layers without leaking/recreating the target.
/// Ok(false) when it is not showing; Err when the layers cannot be made.
pub fn host_backdrop(window: &WebviewWindow, active: bool, shapes: &crate::outline::Shapes) -> windows::core::Result<bool> {
    use windows::Foundation::Numerics::Vector2;
    use windows::Win32::UI::HiDpi::GetDpiForWindow;
    use windows::Win32::UI::WindowsAndMessaging::GetClientRect;
    // The same shapes clip the acrylic and define what may take input, so this runs even when no
    // native layer is wanted (glass off, transparency effects off, remote session).
    remember_input(shapes);
    let Some(hwnd) = hwnd(window) else { return Ok(false) };
    HOST_BACKDROP.with(|slot| {
        let mut slot = slot.borrow_mut();
        if !active {
            if let Some(surface) = slot.as_ref() {
                surface.rail.hide()?;
                surface.card.hide()?;
            }
            return Ok(false);
        }
        if slot.is_none() {
            *slot = Some(HostBackdrop::new(hwnd)?);
        }
        let surface = slot.as_ref().unwrap();
        let mut client = RECT::default();
        unsafe { GetClientRect(hwnd, &mut client)? };
        let size = Vector2 { X: (client.right - client.left) as f32, Y: (client.bottom - client.top) as f32 };
        let px = (unsafe { GetDpiForWindow(hwnd) }.max(96) as f64) / 96.0;
        surface.rail.set(surface, size, shapes.rail.as_ref(), px, "rail")?;
        surface.card.set(surface, size, shapes.card.as_ref(), px, "card")?;
        Ok(true)
    })
}

/// Whether this is a remote desktop session, where the host backdrop is not rendered.
pub fn remote_session() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::SM_REMOTESESSION;
    unsafe { GetSystemMetrics(SM_REMOTESESSION) != 0 }
}

#[cfg(test)]
mod input_tests {
    use super::*;
    use crate::outline::Outline;
    use std::sync::OnceLock;
    use windows::core::w;
    use windows::Win32::Foundation::{HANDLE, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::StationsAndDesktops::{
        GetProcessWindowStation, GetUserObjectInformationW, USEROBJECTFLAGS, UOI_FLAGS,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, HMENU, RegisterClassW, SetWindowPos, WindowFromPoint,
        WNDCLASSW, WSF_VISIBLE, WS_EX_ACCEPTFILES, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_WINDOWEDGE, WS_POPUP,
        WS_VISIBLE,
    };

    /// Windows difference, being tested: the panel is a rectangle whose visible shapes fill only part of
    /// it. The sampler clears `WS_EX_TRANSPARENT` when the pointer is inside a shape and sets it when the
    /// pointer is not; this test drives that pair of calls against a real, isolated HWND and asks Win32
    /// itself (`WindowFromPoint`) which window owns the point. The shapes and the toggle come from the
    /// same functions the app uses, so it fails if either the geometry or the style stops working.
    const CLASS: windows::core::PCWSTR = w!("PulseInputRegionTest");

    /// The region is process-wide state (as it is in the app); the tests that drive it run one at a time.
    static REGION_LOCK: Mutex<()> = Mutex::new(());

    unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }

    /// An isolated test window, destroyed when it goes out of scope however the test ends: a failed
    /// `assert` must not leave a topmost layered window on the desktop.
    struct TestWindow(HWND);
    impl Drop for TestWindow {
        fn drop(&mut self) {
            let _ = unsafe { DestroyWindow(self.0) };
        }
    }

    /// Whether this process has a visible desktop to make windows on. False under a service or another
    /// non-interactive session, where `CreateWindowExW` fails for reasons the geometry test is not about.
    fn interactive_desktop() -> bool {
        unsafe {
            let Ok(station) = GetProcessWindowStation() else { return false };
            let mut flags = USEROBJECTFLAGS::default();
            let read = GetUserObjectInformationW(
                HANDLE(station.0),
                UOI_FLAGS,
                Some((&mut flags as *mut USEROBJECTFLAGS).cast()),
                std::mem::size_of::<USEROBJECTFLAGS>() as u32,
                None,
            );
            read.is_ok() && flags.dwFlags & WSF_VISIBLE as u32 != 0
        }
    }

    fn a_window(rect: RECT) -> Option<TestWindow> {
        static ATOM: OnceLock<u16> = OnceLock::new();
        let atom = *ATOM.get_or_init(|| unsafe {
            RegisterClassW(&WNDCLASSW { lpfnWndProc: Some(wndproc), lpszClassName: CLASS, ..Default::default() })
        });
        if atom == 0 {
            return None;
        }
        unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_LAYERED,
                CLASS,
                w!("PulseInputRegionTest"),
                WS_POPUP | WS_VISIBLE,
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
                HWND::default(),
                HMENU::default(),
                windows::Win32::Foundation::HINSTANCE::default(),
                None,
            )
            .ok()
            .map(TestWindow)
        }
    }

    #[test]
    fn only_the_drawn_shape_takes_input() {
        let _guard = REGION_LOCK.lock().unwrap();

        // The outline decides with or without a window: a point on the drawn rail is the panel's, a
        // point in the window's transparent corner is not. These never depend on the desktop.
        let shapes = crate::outline::Shapes {
            epoch: 1,
            seq: 1,
            rail: Some(Outline { d: "M50 50L150 50L150 150L50 150Z".into(), x: 0.0, y: 0.0, s: 1.0, o: 1.0 }),
            card: None,
        };
        remember_input(&shapes);
        assert_eq!(input_hit((100.0, 100.0)), Some(true), "the drawn rail is the panel's");
        assert_eq!(input_hit((10.0, 10.0)), Some(false), "the rectangle's transparent corner is not");

        if !interactive_desktop() {
            eprintln!("skipping the WindowFromPoint half: no interactive desktop");
            return;
        }
        // A session that advertises a desktop must be able to make this window; a failure here is a
        // real failure, not a silent pass.
        let overlay = a_window(RECT { left: 40, top: 40, right: 240, bottom: 240 })
            .expect("interactive desktop, but CreateWindowExW failed");
        let _ = unsafe { SetLayeredWindowAttributes(overlay.0, COLORREF(0), 255, LWA_ALPHA) };
        let _ = unsafe { SetWindowPos(overlay.0, HWND_TOPMOST, 40, 40, 200, 200, SWP_NOACTIVATE) };

        // Pointer on the shape: the window is interactive, so it owns the point.
        apply_panel_style(overlay.0, false);
        assert_eq!(unsafe { WindowFromPoint(POINT { x: 40 + 100, y: 40 + 100 }) }, overlay.0);

        // Pointer in the window's transparent corner: it goes click-through, and the point belongs to
        // whatever is behind (its identity is not asserted: another window may well be there).
        apply_panel_style(overlay.0, true);
        assert_ne!(unsafe { WindowFromPoint(POINT { x: 40 + 10, y: 40 + 10 }) }, overlay.0);
        // `overlay` drops here, destroying the window.
    }

    /// The style mapping is pure: it forces the panel's base bits, clears `WS_EX_APPWINDOW`, toggles
    /// `WS_EX_TRANSPARENT` from the desired state, and leaves unrelated flags alone.
    #[test]
    fn panel_style_sets_the_base_and_preserves_unrelated_flags() {
        // Tao's styles after a show: WINDOWEDGE | ACCEPTFILES | APPWINDOW | TOPMOST.
        let shown = WS_EX_APPWINDOW.0 | WS_EX_WINDOWEDGE.0 | WS_EX_ACCEPTFILES.0 | WS_EX_TOPMOST.0;
        let unrelated = WS_EX_WINDOWEDGE.0 | WS_EX_ACCEPTFILES.0 | WS_EX_TOPMOST.0;

        let through = panel_ex_style(shown, true);
        assert_eq!(through & WS_EX_APPWINDOW.0, 0, "the taskbar entry is dropped");
        assert_ne!(through & WS_EX_TRANSPARENT.0, 0, "click-through is set");
        for bit in [WS_EX_NOACTIVATE.0, WS_EX_TOOLWINDOW.0, WS_EX_LAYERED.0] {
            assert_ne!(through & bit, 0, "the non-activating layered base is forced");
        }
        assert_eq!(through & unrelated, unrelated, "unrelated flags survive");
        assert_eq!(panel_ex_style(through, true), through, "the mapping is idempotent");

        let interactive = panel_ex_style(shown, false);
        assert_eq!(interactive & WS_EX_TRANSPARENT.0, 0, "click-through is cleared");
        assert_eq!(interactive & unrelated, unrelated);
        // The desired state only ever differs in the transparent bit.
        assert_eq!(interactive & !WS_EX_TRANSPARENT.0, through & !WS_EX_TRANSPARENT.0);
    }

    /// Windows regression for issue #30: when Tao shows the window it rewrites `GWL_EXSTYLE` from its
    /// cached flags, and `skip_taskbar` does not clear `WS_EX_APPWINDOW`, so the panel's base bits and
    /// `WS_EX_TRANSPARENT` are wiped. The production restore must put them back from the desired state
    /// alone, with no pointer movement to trigger it, in both directions.
    #[test]
    fn a_tao_show_style_rewrite_is_recovered_without_moving_the_pointer() {
        let _guard = REGION_LOCK.lock().unwrap();
        if !interactive_desktop() {
            eprintln!("skipping the restore half: no interactive desktop");
            return;
        }
        let overlay = a_window(RECT { left: 40, top: 40, right: 240, bottom: 240 })
            .expect("interactive desktop, but CreateWindowExW failed");
        let _ = unsafe { SetLayeredWindowAttributes(overlay.0, COLORREF(0), 255, LWA_ALPHA) };
        let _ = unsafe { SetWindowPos(overlay.0, HWND_TOPMOST, 40, 40, 200, 200, SWP_NOACTIVATE) };

        // The exact value observed from a Tao show on the installed 0.1.4 panel.
        const TAO_SHOWN: u32 = WS_EX_APPWINDOW.0 | WS_EX_WINDOWEDGE.0 | WS_EX_ACCEPTFILES.0 | WS_EX_TOPMOST.0;
        let write = |ex: u32| unsafe { SetWindowLongPtrW(overlay.0, GWL_EXSTYLE, ex as isize) };
        let current = || unsafe { GetWindowLongPtrW(overlay.0, GWL_EXSTYLE) as u32 };

        // The pointer is not over the panel: click-through is wanted, and it was already wanted, so
        // nothing but the rewrite happened. The restore still has to repair it.
        DESIRED_THROUGH.store(true, Ordering::Release);
        write(TAO_SHOWN);
        assert!(apply_panel_input(overlay.0), "the rewrite is detected and repaired");
        let fixed = current();
        assert_eq!(fixed & WS_EX_APPWINDOW.0, 0, "the taskbar entry is gone");
        for (bit, name) in [
            (WS_EX_NOACTIVATE.0, "WS_EX_NOACTIVATE"),
            (WS_EX_TOOLWINDOW.0, "WS_EX_TOOLWINDOW"),
            (WS_EX_LAYERED.0, "WS_EX_LAYERED"),
            (WS_EX_TRANSPARENT.0, "WS_EX_TRANSPARENT"),
        ] {
            assert_ne!(fixed & bit, 0, "{name} is back");
        }
        assert_ne!(fixed & WS_EX_TOPMOST.0, 0, "unrelated flags are preserved");
        // Click-through: no point of the rectangle is the panel's anymore.
        assert_ne!(unsafe { WindowFromPoint(POINT { x: 40 + 100, y: 40 + 100 }) }, overlay.0);

        // The other direction: input is wanted, the same rewrite happens, and the recovered window
        // must take clicks again.
        DESIRED_THROUGH.store(false, Ordering::Release);
        write(TAO_SHOWN);
        assert!(apply_panel_input(overlay.0));
        assert_eq!(current() & WS_EX_TRANSPARENT.0, 0, "click-through is cleared");
        assert_eq!(unsafe { WindowFromPoint(POINT { x: 40 + 100, y: 40 + 100 }) }, overlay.0);
        // `overlay` drops here, destroying the window.
    }

    #[test]
    fn the_region_scales_and_moves_with_the_shape() {
        let _guard = REGION_LOCK.lock().unwrap();
        // A card outline 100 x 100 in its own space, drawn at 2x (a 200% DPI monitor) at window
        // DIP (200, 50): its right edge is at 200 + 100 * 2 = 400.
        let card = |o: f64| Outline { d: "M0 0L100 0L100 100L0 100Z".into(), x: 200.0, y: 50.0, s: 2.0, o };
        remember_input(&crate::outline::Shapes { epoch: 7, seq: 1, rail: None, card: Some(card(1.0)) });
        assert_eq!(input_hit((250.0, 100.0)), Some(true), "the middle of the scaled card is the panel's");
        assert_eq!(input_hit((410.0, 100.0)), Some(false), "past its right edge");
        assert_eq!(input_hit((150.0, 100.0)), Some(false), "left of it");
        // A card that has faded out must not take (or keep) a click.
        remember_input(&crate::outline::Shapes { epoch: 7, seq: 2, rail: None, card: Some(card(0.0)) });
        assert_eq!(input_hit((250.0, 100.0)), Some(false), "a faded card is not interactive");
        // Once the UI has reported, an empty region is authoritative: a miss, not the older rectangles.
        remember_input(&crate::outline::Shapes { epoch: 7, seq: 3, rail: None, card: None });
        assert_eq!(input_hit((250.0, 100.0)), Some(false), "an empty report stays click-through");
    }
}
