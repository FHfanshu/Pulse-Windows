//! What is behind the panel, for its glass (a Windows stand-in for upstream's Liquid Glass).
//!
//! Windows difference: the real thing is acrylic. A WebView cannot blur the desktop (CSS
//! `backdrop-filter` only sees the page), so the blur is a native Windows.UI.Composition layer under
//! the transparent WebView: a host-backdrop brush clipped to each shape's outline (`win::HostBackdrop`,
//! one layer for the rail or docked berth, one for the open card). `ui/src/panel/nativeShapes.ts` sends
//! the outlines (the same SVG paths the shapes are drawn with, every animation frame while they move) to
//! `set_backdrop_shapes`; the UI thread applies the latest. `ui/src/panel/glass.tsx` lays the tint over it.
//! DWM's own backdrops and accent acrylic cannot do this: they ignore window regions and fill the whole
//! window. And the panel is never excluded from screen capture, so it stays visible to RDP and remote tools.
//!
//! The fallback is the older stand-in, used whenever the native layer is not: Composition could not be
//! created, "Transparency effects" is off in Windows (the host backdrop would be solid), or this is a
//! remote desktop session. Reading the screen *under* the panel is no option (it would mean keeping the
//! panel out of screen capture), so Pulse reads a thin strip just *beside* its window, where it never
//! draws: the window behind shows there, and its colours carry on under the panel. The strip is averaged
//! into a few bands from top to bottom and sent to glass.tsx as `backdrop-tint`, four times a second, only
//! when it changed. Nothing runs while glass is off, the panel hidden or the native layer in use.
//!
//! `PULSE_NO_NATIVE_ACRYLIC` (testing only) forces the fallback.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager};

use crate::outline::Shapes;
use crate::panel;
use crate::state::AppState;
use crate::win;

/// How often the strip is read.
const INTERVAL: Duration = Duration::from_millis(250);
const IDLE: Duration = Duration::from_millis(500);
/// The strip's width beside the window, and its gap from it, in physical pixels.
const STRIP: i32 = 6;
const GAP: i32 = 2;
/// Bands from top to bottom.
const BANDS: i32 = 16;

pub fn start(app: AppHandle) {
    std::thread::spawn(move || {
        // Held for the thread's life: the event below is delivered for as long as it is.
        let settings = windows::UI::ViewManagement::UISettings::new().ok();
        let handler_app = app.clone();
        let _subscription = settings.as_ref().and_then(|s| {
            s.AdvancedEffectsEnabledChanged(&windows::Foundation::TypedEventHandler::new(
                move |sender: &Option<windows::UI::ViewManagement::UISettings>, _| {
                    if let Some(sender) = sender {
                        set_effects(&handler_app, sender.AdvancedEffectsEnabled().unwrap_or(true), win::remote_session());
                    }
                    Ok(())
                },
            ))
            .ok()
        });
        let mut last: Option<Vec<[u8; 3]>> = None;
        loop {
            // A remote session can start or end without any event Pulse hears.
            let transparency = settings.as_ref().and_then(|s| s.AdvancedEffectsEnabled().ok()).unwrap_or(true);
            set_effects(&app, transparency, win::remote_session());
            let glass = app.state::<AppState>().settings().uses_glass;
            let Some(window) = app.get_webview_window(panel::LABEL) else {
                std::thread::sleep(IDLE);
                continue;
            };
            if !glass || !win::is_visible(&window) || NATIVE_ACTIVE.load(Ordering::Relaxed) {
                last = None;
                std::thread::sleep(IDLE);
                continue;
            }
            if let Some(bands) = sample(&window) {
                if last.as_ref() != Some(&bands) {
                    let colours: Vec<String> = bands.iter().map(|[r, g, b]| format!("{r},{g},{b}")).collect();
                    let _ = app.emit_to(panel::LABEL, "backdrop-tint", colours);
                    last = Some(bands);
                }
            }
            std::thread::sleep(INTERVAL);
        }
    });
}

/// The strip on whichever side of the window has room on its display (the left, unless the panel
/// is against the display's left edge), averaged into bands.
fn sample(window: &tauri::WebviewWindow) -> Option<Vec<[u8; 3]>> {
    let (l, t, r, b) = win::window_rect_px(window)?;
    let display = win::monitor_at(((l + r) / 2, (t + b) / 2)).bounds;
    let x = if l - GAP - STRIP >= display.left { l - GAP - STRIP } else { (r + GAP).min(display.right - STRIP) };
    let top = t.max(display.top);
    let bottom = b.min(display.bottom);
    if bottom - top < BANDS {
        return None;
    }
    let pixels = win::capture_screen((x, top, x + STRIP, bottom), 1, BANDS)?;
    Some(pixels.chunks_exact(4).map(|p| [p[2], p[1], p[0]]).collect())
}

/// The outlines to clip the native layers to, as the UI last sent them.
static SHAPES: Mutex<Shapes> = Mutex::new(Shapes { seq: 0, epoch: 0, rail: None, card: None });
/// A pass on the UI thread is already queued; it will read `SHAPES` when it runs.
static QUEUED: AtomicBool = AtomicBool::new(false);
/// What the UI is told: the native layers are showing, so draw only the tint over them.
static NATIVE_ACTIVE: AtomicBool = AtomicBool::new(false);
/// Composition could not be set up; the fallback stays for the session.
static NATIVE_FAILED: AtomicBool = AtomicBool::new(false);
/// Windows will render the blur: transparency effects on, not a remote session.
static EFFECTS: AtomicBool = AtomicBool::new(true);

#[tauri::command]
pub fn get_native_backdrop() -> bool {
    NATIVE_ACTIVE.load(Ordering::Relaxed)
}

/// Windows difference: retain clips while a new producer starts, and reject delayed old commands.
#[tauri::command]
pub fn begin_backdrop_shapes() -> u64 {
    SHAPES.lock().unwrap().begin()
}

/// The WebView shows what it was told about a frame later than a native layer is moved (its picture
/// goes through the renderer and the GPU process first). Applied at once, the blur leads the shape it
/// belongs to by a frame or two while it moves; held back by one frame (measured with real drags over
/// a striped window: none, and 33 ms lags) they stay together.
const WEBVIEW_LAG: Duration = Duration::from_millis(16);

/// The UI's outlines, once per animation frame while a shape moves. Only the latest is applied: if
/// the UI thread is behind, the frames in between are dropped, and so are frames that arrive late
/// (`seq` counts them in the order the UI made them).
#[tauri::command]
pub async fn set_backdrop_shapes(app: AppHandle, shapes: Shapes) {
    tokio::time::sleep(WEBVIEW_LAG).await;
    {
        let mut latest = SHAPES.lock().unwrap();
        if !latest.accept(shapes) {
            return;
        }
    }
    sync_native(&app);
}

fn set_effects(app: &AppHandle, transparency: bool, remote: bool) {
    let on = transparency && !remote;
    if EFFECTS.swap(on, Ordering::Relaxed) != on {
        sync_native(app);
    }
}

/// Bring the native layers in line with the settings and the latest outlines. Safe to call from any
/// thread, holding no lock; the work runs on the UI thread, which never takes the panel lock.
pub fn sync_native(app: &AppHandle) {
    if QUEUED.swap(true, Ordering::AcqRel) {
        return;
    }
    let next = app.clone();
    let queued = app.run_on_main_thread(move || {
        QUEUED.store(false, Ordering::Release);
        apply(&next);
    });
    if queued.is_err() {
        QUEUED.store(false, Ordering::Release);
    }
}

fn apply(app: &AppHandle) {
    let Some(window) = app.get_webview_window(panel::LABEL) else { return };
    let wanted = app.state::<AppState>().settings().uses_glass
        && EFFECTS.load(Ordering::Relaxed)
        && !NATIVE_FAILED.load(Ordering::Relaxed)
        && std::env::var_os("PULSE_NO_NATIVE_ACRYLIC").is_none();
    let shapes = SHAPES.lock().unwrap().clone();
    let active = match win::host_backdrop(&window, wanted, &shapes) {
        // The UI keeps its fallback until an actual rail clip is installed. Outlines are sent even
        // before activation, so this readiness condition cannot deadlock the initial handshake.
        Ok(active) => active && shapes.rail.as_ref().is_some_and(|o| crate::outline::parse(&o.d).is_some()),
        Err(error) => {
            // An unsupported OS or a failed target keeps the strip-sampled glass, not a clear hole.
            eprintln!("native acrylic unavailable: {error}");
            let _ = win::host_backdrop(&window, false, &shapes);
            NATIVE_FAILED.store(true, Ordering::Relaxed);
            false
        }
    };
    if std::env::var_os("PULSE_BACKDROP_TRACE").is_some() {
        let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis();
        eprintln!("BACKDROP {at} epoch={} seq={} active={active} rail={:?} card={:?}", shapes.epoch, shapes.seq, shapes.rail, shapes.card);
    }
    if NATIVE_ACTIVE.swap(active, Ordering::Relaxed) != active {
        let _ = app.emit_to(panel::LABEL, "native-backdrop", active);
    }
}
