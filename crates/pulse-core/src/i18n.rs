//! Localized strings for text Rust draws itself (tray tooltip, notifications).
//! Same tables and keys as the UI: upstream's English strings, `%@` placeholders.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::settings::AppLanguage;

static TABLES: OnceLock<HashMap<&'static str, HashMap<String, String>>> = OnceLock::new();

fn tables() -> &'static HashMap<&'static str, HashMap<String, String>> {
    TABLES.get_or_init(|| {
        let raw: [(&str, &str); 5] = [
            ("en", include_str!("../../../locales/en.json")),
            ("zh-Hans", include_str!("../../../locales/zh-Hans.json")),
            ("zh-Hant", include_str!("../../../locales/zh-Hant.json")),
            ("ja", include_str!("../../../locales/ja.json")),
            ("ko", include_str!("../../../locales/ko.json")),
        ];
        raw.into_iter().map(|(code, json)| (code, serde_json::from_str(json).unwrap_or_default())).collect()
    })
}

/// The table code for a language setting; "system" follows the Windows UI language.
pub fn resolve(language: AppLanguage) -> &'static str {
    match language {
        AppLanguage::En => "en",
        AppLanguage::ZhHans => "zh-Hans",
        AppLanguage::ZhHant => "zh-Hant",
        AppLanguage::Ja => "ja",
        AppLanguage::Ko => "ko",
        AppLanguage::System => system_language(),
    }
}

fn system_language() -> &'static str {
    let tag = sys_locale().to_ascii_lowercase();
    if tag.starts_with("zh") {
        if ["tw", "hk", "mo", "hant"].iter().any(|t| tag.contains(t)) {
            "zh-Hant"
        } else {
            "zh-Hans"
        }
    } else if tag.starts_with("ja") {
        "ja"
    } else if tag.starts_with("ko") {
        "ko"
    } else {
        "en"
    }
}

#[cfg(windows)]
fn sys_locale() -> String {
    use windows::Win32::Globalization::GetUserDefaultLocaleName;
    let mut buf = [0u16; 85];
    let len = unsafe { GetUserDefaultLocaleName(&mut buf) };
    if len > 1 {
        String::from_utf16_lossy(&buf[..(len as usize - 1)])
    } else {
        "en".into()
    }
}

#[cfg(not(windows))]
fn sys_locale() -> String {
    std::env::var("LANG").unwrap_or_else(|_| "en".into())
}

/// `t(lang, "Resets %@", &["9:00"])`.
pub fn t(lang: &str, key: &str, args: &[&str]) -> String {
    let all = tables();
    let format = all
        .get(lang)
        .and_then(|t| t.get(key))
        .or_else(|| all.get("en").and_then(|t| t.get(key)))
        .map(String::as_str)
        .unwrap_or(key);
    let mut out = String::with_capacity(format.len());
    let mut next = 0;
    let mut rest = format;
    while let Some(at) = rest.find('%') {
        out.push_str(&rest[..at]);
        let tail = &rest[at + 1..];
        // `%@` or positional `%1$@`.
        let (index, consumed) = if tail.starts_with('@') {
            let i = next;
            next += 1;
            (Some(i), 1)
        } else if let Some(end) = tail.find("$@") {
            match tail[..end].parse::<usize>() {
                Ok(n) if n > 0 => (Some(n - 1), end + 2),
                _ => (None, 0),
            }
        } else {
            (None, 0)
        };
        match index {
            Some(i) => {
                out.push_str(args.get(i).copied().unwrap_or(""));
                rest = &tail[consumed..];
            }
            None => {
                out.push('%');
                rest = tail;
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_placeholders_and_falls_back_to_english() {
        assert_eq!(t("zh-Hans", "%@ used, %@", &["89%", "5 小时限额"]), "已用 89%，5 小时限额");
        assert_eq!(t("xx", "No reading", &[]), "No reading");
        assert_eq!(t("en", "not a key 100%", &[]), "not a key 100%");
    }
}

/// A window's name in `lang` (upstream `UsageWindow.name`).
pub fn window_name(lang: &str, w: &crate::model::UsageWindow) -> String {
    use crate::model::{Estimate, WindowKind};
    if let Some(label) = &w.label {
        return label.clone();
    }
    let base = match w.kind {
        WindowKind::FiveHour => t(lang, "5-hour limit", &[]),
        WindowKind::Weekly => t(lang, "Weekly limit", &[]),
        WindowKind::Spend => t(lang, "Spend limit", &[]),
        WindowKind::Balance => t(lang, "Balance", &[]),
        WindowKind::Daily => t(lang, "Daily limit", &[]),
        WindowKind::Messages => t(lang, "Message allowance", &[]),
        WindowKind::Monthly => t(lang, "Monthly limit", &[]),
        WindowKind::TopUp => t(lang, "Top-up pack", &[]),
        WindowKind::Credits => t(lang, "Credit allowance", &[]),
        WindowKind::SharedCredits => t(lang, "Team credits", &[]),
        WindowKind::Other(s) if s >= 86_400 && s % 86_400 == 0 => t(lang, "%@-day limit", &[&(s / 86_400).to_string()]),
        WindowKind::Other(s) => t(lang, "%@-hour limit", &[&((s as f64 / 3600.0).round().max(1.0) as i64).to_string()]),
    };
    let scoped = match &w.scope {
        Some(scope) => format!("{base} · {scope}"),
        None => base,
    };
    match w.estimate {
        Some(Estimate::PlanPrice) => format!("{scoped} · {}", t(lang, "estimated", &[])),
        Some(Estimate::SinceTopUp) => format!("{scoped} · {}", t(lang, "since top-up", &[])),
        Some(Estimate::YourBudget) => format!("{scoped} · {}", t(lang, "of your budget", &[])),
        None => scoped,
    }
}
