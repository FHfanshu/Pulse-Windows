//! Thin Win32 helpers for the panel window. Everything geometric is converted
//! to DIPs here and nowhere else.

use std::sync::OnceLock;

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
        // Windows difference: a layered window is invisible until its attributes are initialized.
        // Keep global alpha opaque; the transparent WebView supplies the panel's per-pixel alpha.
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
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
    fn set(&self, backdrop: &HostBackdrop, size: windows::Foundation::Numerics::Vector2, outline: Option<&crate::outline::Outline>, px: f64) -> windows::core::Result<()> {
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
        surface.rail.set(surface, size, shapes.rail.as_ref(), px)?;
        surface.card.set(surface, size, shapes.card.as_ref(), px)?;
        Ok(true)
    })
}

/// Whether this is a remote desktop session, where the host backdrop is not rendered.
pub fn remote_session() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::SM_REMOTESESSION;
    unsafe { GetSystemMetrics(SM_REMOTESESSION) != 0 }
}
