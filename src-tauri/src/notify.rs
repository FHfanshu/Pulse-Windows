//! Usage notifications on Windows: runs `pulse_core::alerts::AlertEngine` after
//! each refresh pass, localizes what it returns and posts a toast.
//!
//! Nothing is posted unless the matching setting is on; the engine enforces that
//! (`wants_usage_alerts`), this file only carries what it returns to the screen.
//! Engine memory lives in `%APPDATA%\Pulse\alerts.json`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use chrono::{DateTime, Local, Utc};
use pulse_core::alerts::{wants_usage_alerts, AlertEngine, Notification, Text};
use pulse_core::settings::{AppLanguage, AppSettings};
use tauri::{AppHandle, Manager};
use tauri_plugin_notification::NotificationExt;

use crate::state::AppState;

/// The engine and the file it is kept in.
pub struct Alerts {
    engine: Mutex<AlertEngine>,
    path: PathBuf,
}

impl Alerts {
    pub fn load(dir: &std::path::Path) -> Self {
        let path = dir.join("alerts.json");
        let engine = std::fs::read(&path).map(|b| AlertEngine::from_json(&b)).unwrap_or_default();
        Self { engine: Mutex::new(engine), path }
    }

    fn save(&self, engine: &AlertEngine) {
        let tmp = self.path.with_extension("tmp");
        if std::fs::write(&tmp, engine.to_json()).is_ok() {
            let _ = std::fs::rename(&tmp, &self.path);
        }
    }

    /// Judge readings and keep the memory current. Returns what is to be posted.
    fn judge(&self, settings: &AppSettings, readings: &[(pulse_core::ProviderUsage, pulse_core::ProviderUsage)]) -> Vec<Notification> {
        let mut engine = self.engine.lock().unwrap();
        let before = engine.clone();
        let now = Utc::now();
        let posted: Vec<_> = readings.iter().flat_map(|(shown, raw)| engine.observe(shown, raw, settings, now)).collect();
        if *engine != before {
            self.save(&engine);
        }
        posted
    }
}

/// A refresh pass has landed: say whatever it is worth saying.
pub fn after_refresh(app: &AppHandle) {
    let state = app.state::<AppState>();
    let settings = state.settings();
    // Always drained, so passes landed while every alert was off are not judged later.
    let readings = state.store.take_observations();
    if !wants_usage_alerts(&settings) {
        return;
    }
    post_all(app, &settings, state.alerts.judge(&settings, &readings));
}

/// Settings changed: re-judge the readings in hand, so a limit already past a line
/// that was just switched on is announced now, not on the next pass.
pub fn reconsider(app: &AppHandle, previous: &AppSettings) {
    let state = app.state::<AppState>();
    let settings = state.settings();
    let touched = previous.alert_threshold != settings.alert_threshold
        || previous.alerts_on_reset != settings.alerts_on_reset
        || previous.alerts_on_failure != settings.alerts_on_failure
        || previous.low_balance_alerts != settings.low_balance_alerts;
    if !touched || !wants_usage_alerts(&settings) {
        return;
    }
    let readings: Vec<_> = state.store.live_readings().into_iter().map(|u| (u.clone(), u)).collect();
    post_all(app, &settings, state.alerts.judge(&settings, &readings));
}

fn post_all(app: &AppHandle, settings: &AppSettings, notifications: Vec<Notification>) {
    if notifications.is_empty() {
        return;
    }
    let lang = language(settings);
    for note in notifications {
        let title = render(&note.title, lang);
        let body = render(&note.body, lang);
        // A toast has a title and a body: the limit's name goes on the body's first line.
        let body = match &note.subtitle {
            Some(subtitle) => format!("{}\n{body}", render(subtitle, lang)),
            None => body,
        };
        // No sound is named: Windows plays the default one, and its own per-app switch mutes it.
        let _ = app.notification().builder().title(title).body(body).show();
    }
}

// ---------------------------------------------------------------------------
// Localization (the locale JSON files, keyed by upstream's English strings)
// ---------------------------------------------------------------------------

const LANGUAGES: [(&str, &str); 5] = [
    ("en", include_str!("../../locales/en.json")),
    ("zh-Hans", include_str!("../../locales/zh-Hans.json")),
    ("zh-Hant", include_str!("../../locales/zh-Hant.json")),
    ("ja", include_str!("../../locales/ja.json")),
    ("ko", include_str!("../../locales/ko.json")),
];

fn table(lang: &str) -> &'static HashMap<String, String> {
    static TABLES: OnceLock<HashMap<&'static str, HashMap<String, String>>> = OnceLock::new();
    let tables = TABLES.get_or_init(|| {
        LANGUAGES
            .iter()
            .map(|(name, json)| (*name, serde_json::from_str(json).unwrap_or_default()))
            .collect()
    });
    tables.get(lang).unwrap_or_else(|| &tables["en"])
}

/// The language the notification is written in: the setting, or for "system" the OS UI language.
pub fn language(settings: &AppSettings) -> &'static str {
    match settings.language {
        AppLanguage::System => system_language(),
        AppLanguage::En => "en",
        AppLanguage::ZhHans => "zh-Hans",
        AppLanguage::ZhHant => "zh-Hant",
        AppLanguage::Ja => "ja",
        AppLanguage::Ko => "ko",
    }
}

#[cfg(windows)]
fn system_language() -> &'static str {
    // LANGID: low ten bits the primary language, the rest the sublanguage.
    let id = unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() };
    match (id & 0x3ff, id >> 10) {
        // Taiwan, Hong Kong and Macao write traditional; mainland and Singapore simplified.
        (0x04, 0x01 | 0x03 | 0x05) => "zh-Hant",
        (0x04, _) => "zh-Hans",
        (0x11, _) => "ja",
        (0x12, _) => "ko",
        _ => "en",
    }
}

#[cfg(not(windows))]
fn system_language() -> &'static str {
    "en"
}

/// `String.localized`: look the key up (falling back to English, then the key itself)
/// and fill `%@` placeholders in order.
fn localized(key: &str, args: &[String], lang: &str) -> String {
    let format = table(lang).get(key).or_else(|| table("en").get(key)).map_or(key, String::as_str);
    let mut out = String::with_capacity(format.len());
    let mut next = 0;
    let mut chars = format.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        // `%@` or `%2$@`.
        let mut look = chars.clone();
        let mut digits = String::new();
        while let Some(d) = look.peek().copied().filter(char::is_ascii_digit) {
            digits.push(d);
            look.next();
        }
        let positional = !digits.is_empty() && look.peek() == Some(&'$');
        if positional {
            look.next();
        }
        if (digits.is_empty() || positional) && look.peek() == Some(&'@') {
            look.next();
            let index = if positional { digits.parse::<usize>().unwrap_or(1) - 1 } else { next };
            if !positional {
                next += 1;
            }
            out.push_str(args.get(index).map_or("", String::as_str));
            chars = look;
        } else {
            out.push(c);
        }
    }
    out
}

pub fn render(text: &Text, lang: &str) -> String {
    match text {
        Text::Plain(s) => s.clone(),
        Text::Key { key, args } => {
            let args: Vec<String> = args.iter().map(|a| render(a, lang)).collect();
            localized(key, &args, lang)
        }
        Text::ResetTime(at) => format_reset_time(*at, lang),
        Text::Dotted(parts) => parts.iter().map(|p| render(p, lang)).collect::<Vec<_>>().join(" · "),
        Text::Sentences(parts) => {
            let mut out = String::new();
            for part in parts.iter().map(|p| render(p, lang)).filter(|s| !s.is_empty()) {
                // A full-width stop carries its own trailing space.
                if !out.is_empty() && !out.ends_with('。') {
                    out.push(' ');
                }
                out.push_str(&part);
            }
            out
        }
    }
}

/// Clock only when the reset is today, else date and clock (upstream "jmm" / "MMMdjmm").
fn format_reset_time(at: DateTime<Utc>, lang: &str) -> String {
    let local = at.with_timezone(&Local);
    let today = local.date_naive() == Local::now().date_naive();
    let pattern = match (lang, today) {
        ("en", true) => "%-I:%M %p",
        ("en", false) => "%b %-d, %-I:%M %p",
        ("ko", false) => "%-m월 %-d일 %H:%M",
        ("zh-Hans" | "zh-Hant" | "ja", false) => "%-m月%-d日 %H:%M",
        (_, _) => "%H:%M",
    };
    local.format(pattern).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone};
    use pulse_core::alerts::{AlertEngine, Text};
    use pulse_core::model::{AccountKey, ProviderUsage, UsageWindow, WindowKind};
    use pulse_core::Provider;

    fn key(k: &str, args: Vec<Text>) -> Text {
        Text::Key { key: k.into(), args }
    }

    #[test]
    fn placeholders_are_filled_in_order_and_falls_back_to_english() {
        let used = key("%@ used.", vec![Text::Plain("92%".into())]);
        assert_eq!(render(&used, "en"), "92% used.");
        assert_ne!(render(&used, "zh-Hans"), "92% used.");
        assert!(render(&used, "zh-Hans").contains("92%"));
        assert_eq!(render(&key("A key nobody translated", vec![]), "ja"), "A key nobody translated");
    }

    #[test]
    fn sentences_have_a_space_only_where_one_is_wanted() {
        let joined = Text::Sentences(vec![Text::Plain("92% used.".into()), Text::Plain("".into()), Text::Plain("Resets 3 PM".into())]);
        assert_eq!(render(&joined, "en"), "92% used. Resets 3 PM");
        let cjk = Text::Sentences(vec![Text::Plain("已用 92%。".into()), Text::Plain("3 点重置".into())]);
        assert_eq!(render(&cjk, "zh-Hans"), "已用 92%。3 点重置");
    }

    #[test]
    fn every_key_the_engine_can_emit_is_in_the_locale_files() {
        let english = table("en");
        for key in [
            "%@ used.", "%@ left.", "Resets %@", "This limit is spent.", "This limit has reset.", "Can't be read",
            "Balance", "The last few checks didn't get through, so the panel is showing older figures.",
            "5-hour limit", "Weekly limit", "Spend limit", "Daily limit", "Message allowance", "Monthly limit",
            "Top-up pack", "Credit allowance", "Team credits", "%@-day limit", "%@-hour limit", "estimated",
            "since top-up", "of your budget",
        ] {
            assert!(english.contains_key(key), "missing locale key {key:?}");
        }
        for reason in ["The service didn't respond.", "That key was refused. Check it in Settings.", "Couldn't read the reply."] {
            assert!(english.contains_key(reason), "missing locale key {reason:?}");
        }
    }

    #[test]
    fn an_engine_notification_renders_end_to_end() {
        let settings = AppSettings { alert_threshold: Some(90), ..AppSettings::default() };
        let now = Utc.timestamp_opt(1_800_000_000, 0).unwrap();
        let window = UsageWindow::new("w", WindowKind::Weekly, 0.93, 604_800).with_reset(Some(now + Duration::hours(2)));
        let reading = ProviderUsage::live(AccountKey::primary(Provider::ClaudeCode), vec![window], now);
        let mut engine = AlertEngine::default();
        let posted = engine.observe(&reading, &reading, &settings, now);
        assert_eq!(posted.len(), 1);
        assert_eq!(render(&posted[0].title, "en"), "Claude Code");
        assert_eq!(render(posted[0].subtitle.as_ref().unwrap(), "en"), "Weekly limit");
        assert!(render(&posted[0].body, "en").starts_with("93% used. Resets "));
    }
}
