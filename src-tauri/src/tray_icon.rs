// Windows counterpart of upstream App/MenuBarReading.swift's drawing half.
//! The tray icon. A Windows tray slot is a small square, not a strip of menu-bar
//! text, so the three upstream styles become:
//! - figure: the percentage as digits;
//! - ring:   a usage ring;
//! - split:  two concentric rings (shorter window outside).
//! With no reading the icon is Pulse's own mark (the app icon).
//! Glyphs come from Segoe UI Semibold, which every Windows install has.

use std::sync::OnceLock;

use ab_glyph::{Font, FontArc, PxScale, ScaleFont};
use pulse_core::i18n;
use pulse_core::settings::{AppSettings, TrayStyle};
use pulse_core::tray::{self, TrayReading};
use pulse_core::ProviderUsage;
use tauri::image::Image;
use tauri::AppHandle;
use tiny_skia::{Color, LineCap, Paint, PathBuilder, Pixmap, Stroke, Transform};

/// Rendered at 32 px: crisp at 200% scaling, downsampled cleanly at 100%.
const SIZE: u32 = 32;

const GOOD: (u8, u8, u8) = (0, 230, 140);
const CAUTION: (u8, u8, u8) = (255, 194, 38);
const WARNING: (u8, u8, u8) = (255, 79, 66);
const SPENT: (u8, u8, u8) = (217, 23, 33);

fn font() -> Option<&'static FontArc> {
    static FONT: OnceLock<Option<FontArc>> = OnceLock::new();
    FONT.get_or_init(|| {
        let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
        ["seguisb.ttf", "segoeuib.ttf", "segoeui.ttf"]
            .iter()
            .find_map(|f| std::fs::read(format!(r"{windir}\Fonts\{f}")).ok())
            .and_then(|bytes| FontArc::try_from_vec(bytes).ok())
    })
    .as_ref()
}

/// Taskbar theme: light taskbars need dark ink.
fn taskbar_is_light() -> bool {
    use windows::core::w;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
    let mut value: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
            w!("SystemUsesLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    status.is_ok() && value == 1
}

fn tint(used: f64, spent: bool, warning_at: f64) -> (u8, u8, u8) {
    if spent || used >= 1.0 {
        SPENT
    } else if used < 0.5 {
        GOOD
    } else if used < warning_at {
        CAUTION
    } else {
        WARNING
    }
}

fn paint(rgb: (u8, u8, u8), alpha: u8) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(Color::from_rgba8(rgb.0, rgb.1, rgb.2, alpha));
    p.anti_alias = true;
    p
}

/// A ring track plus an arc from 12 o'clock clockwise. `track` is the empty-ring colour, which
/// the reader's exhausted-ring choice turns red for an opted-out spent limit.
fn ring(pixmap: &mut Pixmap, radius: f32, width: f32, fraction: f64, rgb: (u8, u8, u8), track: (u8, u8, u8)) {
    let c = SIZE as f32 / 2.0;
    let stroke = Stroke { width, line_cap: LineCap::Round, ..Stroke::default() };
    if let Some(track_ring) = PathBuilder::from_circle(c, c, radius) {
        pixmap.stroke_path(&track_ring, &paint(track, 70), &stroke, Transform::identity(), None);
    }
    let fraction = fraction.clamp(0.0, 1.0) as f32;
    if fraction <= 0.0 {
        return;
    }
    let mut pb = PathBuilder::new();
    let steps = (fraction * 64.0).ceil().max(2.0) as usize;
    for i in 0..=steps {
        let a = -std::f32::consts::FRAC_PI_2 + fraction * std::f32::consts::TAU * (i as f32 / steps as f32);
        let (x, y) = (c + radius * a.cos(), c + radius * a.sin());
        if i == 0 {
            pb.move_to(x, y)
        } else {
            pb.line_to(x, y)
        }
    }
    if let Some(arc) = pb.finish() {
        pixmap.stroke_path(&arc, &paint(rgb, 255), &stroke, Transform::identity(), None);
    }
}

fn digits(pixmap: &mut Pixmap, text: &str, rgb: (u8, u8, u8)) {
    let Some(font) = font() else { return };
    // Three digits need a narrower size than two.
    let px = if text.chars().count() >= 3 { 15.0 } else { 21.0 };
    let scaled = font.as_scaled(PxScale::from(px));
    let width: f32 = text.chars().map(|ch| scaled.h_advance(font.glyph_id(ch))).sum();
    let mut x = (SIZE as f32 - width) / 2.0;
    let baseline = (SIZE as f32 + scaled.ascent() + scaled.descent()) / 2.0;
    let pixels = pixmap.pixels_mut();
    for ch in text.chars() {
        let glyph = font.glyph_id(ch).with_scale_and_position(px, ab_glyph::point(x, baseline));
        x += scaled.h_advance(glyph.id);
        let Some(outlined) = font.outline_glyph(glyph) else { continue };
        let bounds = outlined.px_bounds();
        outlined.draw(|gx, gy, coverage| {
            let (px_, py_) = (bounds.min.x as i32 + gx as i32, bounds.min.y as i32 + gy as i32);
            if px_ < 0 || py_ < 0 || px_ >= SIZE as i32 || py_ >= SIZE as i32 {
                return;
            }
            let a = (coverage.clamp(0.0, 1.0) * 255.0).round() as u8;
            let idx = (py_ as u32 * SIZE + px_ as u32) as usize;
            let premul = |c: u8| ((c as u16 * a as u16) / 255) as u8;
            if let Some(p) = tiny_skia::PremultipliedColorU8::from_rgba(premul(rgb.0), premul(rgb.1), premul(rgb.2), a) {
                if a > pixels[idx].alpha() {
                    pixels[idx] = p;
                }
            }
        });
    }
}

fn render(reading: &TrayReading, settings: &AppSettings) -> Option<Image<'static>> {
    let mut pixmap = Pixmap::new(SIZE, SIZE)?;
    let warning_at = settings.warning_threshold as f64 / 100.0;
    let ink = if taskbar_is_light() { (0, 0, 0) } else { (255, 255, 255) };
    let remaining = settings.shows_remaining;
    let keeps_filled = settings.keeps_exhausted_ring_filled;
    // Windows difference (owner-approved #29): opted out while counting down, a spent ring is
    // drawn empty and its track turns red, so the warning is not lost with the arc.
    let track_of = |spent: bool| if remaining && !keeps_filled && spent { SPENT } else { ink };

    match (settings.tray_style, &reading.window) {
        (TrayStyle::Ring, Some(w)) => ring(
            &mut pixmap,
            12.0,
            5.0,
            w.drawn_fraction(remaining, keeps_filled, false),
            tint(w.used_fraction, w.is_spent(), warning_at),
            track_of(w.is_spent()),
        ),
        (TrayStyle::Split, Some(_)) if reading.split.len() == 2 => {
            let (outer, inner) = (&reading.split[0].0, &reading.split[1].0);
            ring(&mut pixmap, 13.0, 4.0, outer.drawn_fraction(remaining, keeps_filled, false), tint(outer.used_fraction, outer.is_spent(), warning_at), track_of(outer.is_spent()));
            ring(&mut pixmap, 6.5, 4.0, inner.drawn_fraction(remaining, keeps_filled, false), tint(inner.used_fraction, inner.is_spent(), warning_at), track_of(inner.is_spent()));
        }
        (_, Some(w)) => {
            let rgb = if reading.is_alert { WARNING } else { ink };
            digits(&mut pixmap, &w.percent_value(remaining).to_string(), rgb);
        }
        (_, None) => return None,
    }
    // tiny-skia stores premultiplied colour; the tray wants straight RGBA.
    let rgba: Vec<u8> = pixmap
        .pixels()
        .iter()
        .flat_map(|p| {
            let c = p.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        })
        .collect();
    Some(Image::new_owned(rgba, SIZE, SIZE))
}

fn tooltip(reading: Option<&TrayReading>, settings: &AppSettings) -> String {
    let lang = i18n::resolve(settings.language);
    let Some(reading) = reading else { return "Pulse".into() };
    let figure = match &reading.window {
        Some(w) if settings.shows_remaining => {
            i18n::t(lang, "%@ left, %@", &[&format!("{}%", w.percent_value(true)), &i18n::window_name(lang, w)])
        }
        Some(w) => i18n::t(lang, "%@ used, %@", &[&format!("{}%", w.percent_value(false)), &i18n::window_name(lang, w)]),
        None => reading.money.clone().unwrap_or_else(|| i18n::t(lang, "No reading", &[])),
    };
    format!("{}\n{figure}", reading.account.provider.display_name())
}

/// Runs `f` with the tray icon on the UI thread. Every tray access goes through here: Tauri's
/// `TrayIcon` is marked Send but wraps tray-icon's `Rc`, so looking it up, calling it (each call
/// clones it first) or dropping it on another thread races the count. Refresh tasks did so
/// concurrently and corrupted the heap (the v0.1.5-0.1.6 crashes).
pub fn with_tray(app: &AppHandle, f: impl FnOnce(&tauri::tray::TrayIcon) + Send + 'static) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(tray) = handle.tray_by_id("main") {
            f(&tray);
        }
    });
}

/// Redraw the tray icon from the current readings. Drawn on the calling thread; applied on the UI thread.
pub fn update(app: &AppHandle, usages: &[ProviderUsage], settings: &AppSettings) {
    let visible = !settings.hides_tray_icon;
    let reading = if settings.shows_usage_in_tray {
        tray::choose(usages, settings.tray_account.as_deref(), |a| settings.pinned_windows.get(&a.id()).cloned(), settings.warning_threshold as f64 / 100.0)
    } else {
        None
    };
    let icon = reading.as_ref().and_then(|r| render(r, settings)).or_else(|| app.default_window_icon().cloned().map(Image::to_owned));
    let tip = tooltip(reading.as_ref(), settings);
    with_tray(app, move |tray| {
        let _ = tray.set_visible(visible);
        let _ = tray.set_icon(icon);
        let _ = tray.set_tooltip(Some(tip));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulse_core::{AccountKey, Provider, UsageWindow, WindowKind};

    fn reading(used: f64) -> TrayReading {
        let w = UsageWindow::new("w", WindowKind::FiveHour, used, 18_000);
        TrayReading { account: AccountKey::primary(Provider::Codex), window: Some(w.clone()), money: None, is_alert: used >= 0.75, split: vec![(w.clone(), false), (w, false)] }
    }

    fn opaque_pixels(image: &Image) -> usize {
        image.rgba().chunks(4).filter(|p| p[3] > 40).count()
    }

    /// Spent red, `rgb(217, 23, 33)`, or the warning red beside it.
    fn has_red(image: &Image) -> bool {
        image.rgba().chunks(4).any(|p| p[3] > 40 && p[0] > 150 && p[1] < 90 && p[2] < 90)
    }

    #[test]
    fn opted_out_spent_ring_keeps_a_red_track() {
        let w = UsageWindow::new("w", WindowKind::FiveHour, 1.0, 18_000).exhausted(true);
        let spent = TrayReading { account: AccountKey::primary(Provider::Codex), window: Some(w), money: None, is_alert: true, split: vec![] };
        let settings = AppSettings { tray_style: TrayStyle::Ring, shows_remaining: true, keeps_exhausted_ring_filled: false, ..AppSettings::default() };
        let image = render(&spent, &settings).expect("image");
        assert!(opaque_pixels(&image) > 20, "an opted-out spent ring still draws its track");
        assert!(has_red(&image), "the track turns red");
    }

    #[test]
    fn the_choice_only_changes_a_countdown_ring() {
        let reading = TrayReading {
            account: AccountKey::primary(Provider::Codex),
            window: Some(UsageWindow::new("w", WindowKind::FiveHour, 0.5, 18_000).exhausted(true)),
            money: None,
            is_alert: true,
            split: vec![],
        };
        let kept = AppSettings { tray_style: TrayStyle::Ring, shows_remaining: false, ..AppSettings::default() };
        let opted_out = AppSettings { keeps_exhausted_ring_filled: false, ..kept.clone() };
        assert_eq!(
            render(&reading, &kept).unwrap().rgba(),
            render(&reading, &opted_out).unwrap().rgba(),
            "consumed mode draws the same ring either way",
        );
        let down = AppSettings { shows_remaining: true, ..kept.clone() };
        let down_out = AppSettings { shows_remaining: true, keeps_exhausted_ring_filled: false, ..kept };
        assert_ne!(
            render(&reading, &down).unwrap().rgba(),
            render(&reading, &down_out).unwrap().rgba(),
            "counting down, the choice changes the ring",
        );
    }

    #[test]
    fn every_style_draws_something() {
        for style in [TrayStyle::Figure, TrayStyle::Ring, TrayStyle::Split] {
            let settings = AppSettings { tray_style: style, ..AppSettings::default() };
            for used in [0.0, 0.42, 1.0] {
                let image = render(&reading(used), &settings).expect("image");
                assert_eq!(image.width(), SIZE);
                assert!(opaque_pixels(&image) > 20, "{style:?} at {used} drew almost nothing");
            }
        }
    }
}
