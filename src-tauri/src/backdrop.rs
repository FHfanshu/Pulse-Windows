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

/// Experiment (PULSE_ACRYLIC_TEST=1): DWM acrylic clipped to the rail by a window region.
pub fn acrylic_experiment(app: AppHandle) {
    if std::env::var_os("PULSE_ACRYLIC_TEST").is_none() {
        return;
    }
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(4));
        let Some(window) = app.get_webview_window(panel::LABEL) else { return };
        let shared = app.state::<panel::SharedPanel>().inner().clone();
        let (rail, scale) = {
            let s = shared.lock().unwrap();
            (s.layout.map(|l| l.rail), s.monitor.as_ref().map(|m| m.scale).unwrap_or(1.0))
        };
        let Some(rail) = rail else { return };
        let r = rail.w.min(rail.h) / 2.0;
        let mut points = Vec::new();
        let corners = [
            (rail.x + rail.w - r, rail.y + r, -90.0f64),
            (rail.x + rail.w - r, rail.y + rail.h - r, 0.0),
            (rail.x + r, rail.y + rail.h - r, 90.0),
            (rail.x + r, rail.y + r, 180.0),
        ];
        for (cx, cy, start) in corners {
            for k in 0..=12 {
                let a = (start + 90.0 * k as f64 / 12.0).to_radians();
                points.push((((cx + r * a.cos()) * scale).round() as i32, ((cy + r * a.sin()) * scale).round() as i32));
            }
        }
        eprintln!("acrylic test: rail {rail:?} scale {scale} points {}", points.len());
        let (x, y, w, h) = ((rail.x * scale) as f32, (rail.y * scale) as f32, (rail.w * scale) as f32, (rail.h * scale) as f32);
        let _ = app.run_on_main_thread(move || {
            let result = win::host_backdrop_experiment(&window, x, y, w, h, w / 2.0);
            eprintln!("acrylic test: host backdrop {result:?}");
        });
    });
}
