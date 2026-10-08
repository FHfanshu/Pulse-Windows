// Ported from upstream App/MenuBarReading.swift.
//! Which figure the tray icon shows: the chosen account while it is on the rail,
//! else the fullest headline window among shown accounts. Unavailable readings
//! and inferred (estimated) windows are skipped.

use crate::model::{AccountKey, ProviderUsage, UsageState, UsageWindow, WindowKind};

#[derive(Debug, Clone, PartialEq)]
pub struct TrayReading {
    pub account: AccountKey,
    pub window: Option<UsageWindow>,
    /// A money-only account's balance, already formatted.
    pub money: Option<String>,
    pub is_alert: bool,
    /// Up to two unscoped windows of different lengths, shortest first ("split" style).
    pub split: Vec<(UsageWindow, bool)>,
}

const SPLIT_KINDS: [WindowKind; 4] = [WindowKind::FiveHour, WindowKind::Daily, WindowKind::Weekly, WindowKind::Monthly];

pub fn of(usage: &ProviderUsage, pinned: Option<&str>, warning_at: f64) -> TrayReading {
    let alert = |w: &UsageWindow| w.is_spent() || w.used_fraction >= warning_at;
    let empty = TrayReading { account: usage.account.clone(), window: None, money: None, is_alert: false, split: Vec::new() };
    if matches!(usage.state, UsageState::Unavailable(_)) {
        return empty;
    }
    if let Some(window) = usage.headline_window(pinned).filter(|w| w.estimate.is_none()) {
        return TrayReading {
            is_alert: alert(window),
            window: Some(window.clone()),
            split: split_windows(&usage.windows).into_iter().map(|w| { let a = alert(&w); (w, a) }).collect(),
            ..empty
        };
    }
    TrayReading { money: usage.credit_balance.clone(), ..empty }
}

pub fn choose(usages: &[ProviderUsage], chosen: Option<&str>, pinned: impl Fn(&AccountKey) -> Option<String>, warning_at: f64) -> Option<TrayReading> {
    if let Some(id) = chosen {
        if let Some(u) = usages.iter().find(|u| u.account.id() == id) {
            return Some(of(u, pinned(&u.account).as_deref(), warning_at));
        }
    }
    let mut best: Option<TrayReading> = None;
    for u in usages {
        let reading = of(u, pinned(&u.account).as_deref(), warning_at);
        let Some(w) = &reading.window else { continue };
        if best.as_ref().and_then(|b| b.window.as_ref()).is_some_and(|f| w.used_fraction <= f.used_fraction) {
            continue;
        }
        best = Some(reading);
    }
    best
}

pub fn split_windows(windows: &[UsageWindow]) -> Vec<UsageWindow> {
    let mut seen: Vec<WindowKind> = Vec::new();
    let mut picked: Vec<UsageWindow> = windows
        .iter()
        .filter(|w| {
            let keep = w.scope.is_none() && w.estimate.is_none() && SPLIT_KINDS.contains(&w.kind) && !seen.contains(&w.kind);
            if keep {
                seen.push(w.kind);
            }
            keep
        })
        .cloned()
        .collect();
    picked.sort_by_key(|w| SPLIT_KINDS.iter().position(|k| *k == w.kind).unwrap_or(9));
    picked.truncate(2);
    picked
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::Provider;
    use chrono::Utc;

    #[test]
    fn picks_the_fullest_and_skips_unavailable() {
        let now = Utc::now();
        let a = ProviderUsage::live(AccountKey::primary(Provider::Codex), vec![UsageWindow::new("w", WindowKind::Weekly, 0.3, 1)], now);
        let b = ProviderUsage::live(AccountKey::primary(Provider::ClaudeCode), vec![UsageWindow::new("f", WindowKind::FiveHour, 0.8, 1)], now);
        let c = ProviderUsage::unavailable(AccountKey::primary(Provider::Cursor), crate::model::Unavailability::Loading);
        let r = choose(&[a, b, c], None, |_| None, 0.75).unwrap();
        assert_eq!(r.account.provider, Provider::ClaudeCode);
        assert!(r.is_alert);
    }

    #[test]
    fn split_keeps_two_unscoped_kinds_shortest_first() {
        let ws = vec![
            UsageWindow::new("w", WindowKind::Weekly, 0.3, 1),
            UsageWindow::new("s", WindowKind::Weekly, 0.3, 1).with_scope("Opus"),
            UsageWindow::new("f", WindowKind::FiveHour, 0.1, 1),
        ];
        let ids: Vec<_> = split_windows(&ws).into_iter().map(|w| w.id).collect();
        assert_eq!(ids, ["f", "w"]);
    }
}
