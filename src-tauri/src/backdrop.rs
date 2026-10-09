//! The colours behind the panel, for its glass (a Windows stand-in for upstream's Liquid Glass).
//! Windows has no backdrop that follows the panel's shapes (DWM's backdrop and the acrylic accent
//! fill the whole transparent window), and reading the screen *under* the panel would mean keeping
//! the panel out of screen capture, which hides it from screenshots and remote desktop. So Pulse
//! reads a thin strip just *beside* its window, where it never draws: the window behind shows there,
//! and its colours carry on under the panel. The strip is averaged into a few bands from top to
//! bottom and sent to `ui/src/panel/glass.tsx` as `backdrop-tint`, four times a second, only when
//! it changed. Nothing runs while glass is off or the panel hidden.

use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager};

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
        let mut last: Option<Vec<[u8; 3]>> = None;
        loop {
            let glass = app.state::<AppState>().settings().uses_glass;
            let Some(window) = app.get_webview_window(panel::LABEL) else {
                std::thread::sleep(IDLE);
                continue;
            };
            if !glass || !win::is_visible(&window) {
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
