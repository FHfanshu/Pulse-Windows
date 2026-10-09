//! Mica for the panel's glass (a Windows stand-in for upstream's Liquid Glass). Windows has no
//! backdrop that follows the panel's shapes (DWM's backdrop and the acrylic accent fill the whole
//! transparent window), so the panel draws Mica itself, the way the taskbar's material works: the
//! desktop wallpaper, blurred until only its colours are left, under a dark tint. Nothing reads the
//! screen, so the panel shows in screenshots, recordings and remote desktop as it is.
//!
//! Two events for `ui/src/panel/glass.tsx`: `wallpaper` (the image as a data URL, how it is fitted,
//! and the desktop colour) when the wallpaper changes, and `wallpaper-place` (where the window is on
//! its display) when the panel moves. Nothing runs while glass is off.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use base64::Engine;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Listener, Manager};

use crate::panel;
use crate::state::AppState;
use crate::win;

/// How often the panel's position is looked at; the wallpaper every `WALLPAPER_EVERY` looks.
const PLACE_EVERY: Duration = Duration::from_millis(200);
const WALLPAPER_EVERY: u32 = 15;
/// A wallpaper file larger than this is not sent; the desktop colour stands in.
const MAXIMUM_BYTES: u64 = 40 * 1024 * 1024;

#[derive(Serialize, Clone)]
struct Wallpaper {
    image: Option<String>,
    style: &'static str,
    colour: String,
}

#[derive(Serialize, Clone, Copy, PartialEq)]
struct Place {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    scale: f64,
}

/// What a sent wallpaper was: the file and its modification and size, the fit, the colour.
type Sent = (Option<(PathBuf, SystemTime, u64)>, &'static str, (u8, u8, u8));

pub fn start(app: AppHandle) {
    // The panel asks once it is listening (a load or a reload), so nothing sent before is lost.
    let asked = Arc::new(AtomicBool::new(false));
    let flag = asked.clone();
    app.listen("wallpaper-request", move |_| flag.store(true, Ordering::Relaxed));
    std::thread::spawn(move || {
        let mut sent_wallpaper: Option<Sent> = None;
        let mut sent_place: Option<Place> = None;
        let mut tick = 0u32;
        loop {
            std::thread::sleep(PLACE_EVERY);
            if asked.swap(false, Ordering::Relaxed) {
                sent_wallpaper = None;
                sent_place = None;
            }
            if !app.state::<AppState>().settings().uses_glass {
                sent_wallpaper = None;
                sent_place = None;
                continue;
            }
            let Some(window) = app.get_webview_window(panel::LABEL) else { continue };

            if sent_wallpaper.is_none() || tick % WALLPAPER_EVERY == 0 {
                let paper = win::wallpaper();
                let stamp = paper.file.as_ref().and_then(|f| {
                    let meta = std::fs::metadata(f).ok()?;
                    Some((f.clone(), meta.modified().ok()?, meta.len()))
                });
                let key: Sent = (stamp.clone(), paper.style, paper.colour);
                if sent_wallpaper.as_ref() != Some(&key) {
                    let image = stamp.filter(|s| s.2 <= MAXIMUM_BYTES).and_then(|s| data_url(&s.0));
                    let (r, g, b) = paper.colour;
                    let payload = Wallpaper { image, style: paper.style, colour: format!("rgb({r},{g},{b})") };
                    let _ = app.emit_to(panel::LABEL, "wallpaper", payload);
                    sent_wallpaper = Some(key);
                    sent_place = None;
                }
            }
            tick = tick.wrapping_add(1);

            if let Some((x, y, width, height, scale)) = win::place_on_display(&window) {
                let place = Place { x, y, width, height, scale };
                if sent_place != Some(place) {
                    let _ = app.emit_to(panel::LABEL, "wallpaper-place", place);
                    sent_place = Some(place);
                }
            }
        }
    });
}

/// The file as a data URL, its type taken from its first bytes (`TranscodedWallpaper` has no
/// extension).
fn data_url(file: &std::path::Path) -> Option<String> {
    let bytes = std::fs::read(file).ok()?;
    let mime = match bytes.get(..4)? {
        [0xFF, 0xD8, ..] => "image/jpeg",
        [0x89, b'P', b'N', b'G'] => "image/png",
        [b'B', b'M', ..] => "image/bmp",
        [b'R', b'I', b'F', b'F'] => "image/webp",
        _ => return None,
    };
    Some(format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
}
