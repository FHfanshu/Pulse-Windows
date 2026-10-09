//! Liquid Glass's live blur. Windows has no backdrop that follows the panel's shapes (DWM's
//! backdrop and the acrylic accent fill the whole transparent window), so Pulse reads what is
//! behind the panel itself: the screen under the window, with the panel kept out of the capture,
//! shrunk to an eighth (the blur throws the detail away anyway), about fifteen times a second.
//! The UI clips it to the rail and the card and blurs it (`ui/src/panel/glass.tsx`). A frame
//! that has not changed is not sent; nothing runs while glass is off or the panel hidden.

use std::hash::{Hash, Hasher};
use std::time::Duration;

use base64::Engine;
use tauri::{AppHandle, Emitter, Manager};

use crate::panel;
use crate::state::AppState;
use crate::win;

/// Physical pixels per backdrop pixel, each way.
const SHRINK: i32 = 8;
const INTERVAL: Duration = Duration::from_millis(66);
const IDLE: Duration = Duration::from_millis(500);

pub fn start(app: AppHandle) {
    std::thread::spawn(move || {
        let mut excluded: Option<bool> = None;
        let mut last = 0u64;
        loop {
            let glass = app.state::<AppState>().settings().uses_glass;
            let Some(window) = app.get_webview_window(panel::LABEL) else {
                std::thread::sleep(IDLE);
                continue;
            };
            if excluded != Some(glass) {
                // The window's owner thread sets its display affinity.
                let target = window.clone();
                let _ = app.run_on_main_thread(move || win::exclude_from_capture(&target, glass));
                excluded = Some(glass);
                last = 0;
                if !glass {
                    let _ = app.emit_to(panel::LABEL, "backdrop", None::<String>);
                }
            }
            if !glass || !win::is_visible(&window) {
                std::thread::sleep(IDLE);
                continue;
            }
            if let Some(rect) = win::window_rect_px(&window) {
                let w = ((rect.2 - rect.0) / SHRINK).max(1);
                let h = ((rect.3 - rect.1) / SHRINK).max(1);
                if let Some(pixels) = win::capture_screen(rect, w, h) {
                    let mut hasher = std::collections::hash_map::DefaultHasher::new();
                    (rect, &pixels).hash(&mut hasher);
                    let hash = hasher.finish();
                    if hash != last {
                        last = hash;
                        let _ = app.emit_to(panel::LABEL, "backdrop", Some(data_url(&pixels, w, h)));
                    }
                }
            }
            std::thread::sleep(INTERVAL);
        }
    });
}

/// A top-down 32-bit BMP: the bytes as captured, behind a 54-byte header. Opaque (the alpha byte
/// of a BI_RGB bitmap is ignored).
fn data_url(bgra: &[u8], w: i32, h: i32) -> String {
    let mut bmp = Vec::with_capacity(54 + bgra.len());
    let le32 = |v: &mut Vec<u8>, n: u32| v.extend_from_slice(&n.to_le_bytes());
    bmp.extend_from_slice(b"BM");
    le32(&mut bmp, 54 + bgra.len() as u32);
    le32(&mut bmp, 0);
    le32(&mut bmp, 54);
    le32(&mut bmp, 40);
    le32(&mut bmp, w as u32);
    le32(&mut bmp, (-h) as u32);
    bmp.extend_from_slice(&1u16.to_le_bytes());
    bmp.extend_from_slice(&32u16.to_le_bytes());
    for _ in 0..6 {
        le32(&mut bmp, 0);
    }
    bmp.extend_from_slice(bgra);
    format!("data:image/bmp;base64,{}", base64::engine::general_purpose::STANDARD.encode(bmp))
}
