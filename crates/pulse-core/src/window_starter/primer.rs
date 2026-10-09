// Ported from upstream Sources/Pulse/Usage/WindowPrimer.swift (the decisions; the timer lives in
// src-tauri/src/window_starter_ipc.rs).
//! When a usage window is started: by the clock, not by guessing from the reply.
//!
//! A window that has reset but not restarted is reported differently by each provider, and not
//! documented by either. What is certain is the reset time the last reading stated: once it has
//! passed, the window is over. So each eligible window's reset is remembered, the scheduler wakes
//! just after it, and at that moment one "hi" is sent: unless a reading since shows a later reset,
//! which means the user has started it already.
//!
//! Off by default, per provider, and only after the reader has read what it does (the pane's
//! confirmation). Only the first account of each: the tools send as whoever they are signed in as,
//! which is that account.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Duration, Utc};

use crate::model::{AccountKey, UsageWindow, WindowKind};
use crate::provider::Provider;
use crate::settings::PrimerHours;
use crate::spend::Calendar;

/// A reading can arrive a little after the reset it stated; the window is given this long to be
/// over before it is started again.
pub const GRACE_SECONDS: i64 = 60;

/// The soonest the scheduler comes back for a reset that is already due.
pub const MINIMUM_DELAY_SECONDS: i64 = 5;

/// The providers the starter acts for.
pub const PROVIDERS: [Provider; 2] = [Provider::ClaudeCode, Provider::Codex];

impl PrimerHours {
    /// Whether `at` is inside the hours, by `calendar`'s clock.
    ///
    /// A window started at 3 a.m. resets at 8, while somebody is asleep, and again five hours
    /// later. Limiting it to waking hours costs nothing that matters and keeps the account from
    /// looking like a machine that never sleeps.
    pub fn contains(&self, at: DateTime<Utc>, calendar: &Calendar) -> bool {
        let hour = calendar.hour(at) as u8;
        if self.start == self.end {
            return true;
        }
        if self.start < self.end {
            (self.start..self.end).contains(&hour)
        } else {
            hour >= self.start || hour < self.end
        }
    }

    /// The next moment at or after `at` inside these hours.
    pub fn next(&self, at: DateTime<Utc>, calendar: &Calendar) -> DateTime<Utc> {
        if self.contains(at, calendar) {
            return at;
        }
        let day = calendar.date(at);
        let today = day
            .and_hms_opt(u32::from(self.start), 0, 0)
            .map(|local| calendar.from_local(local))
            .unwrap_or(at);
        if today > at {
            today
        } else {
            calendar.add_days(today, 1)
        }
    }
}

/// The windows starting helps. Claude Code's week resets at a fixed hour whatever anybody does, so
/// only its five hours; Codex's five hours and week both wait for a first message. Account-wide
/// limits only.
pub fn eligible(window: &UsageWindow, provider: Provider) -> bool {
    if window.scope.is_some() || window.estimate.is_some() {
        return false;
    }
    match provider {
        Provider::ClaudeCode => window.kind == WindowKind::FiveHour,
        Provider::Codex => matches!(window.kind, WindowKind::FiveHour | WindowKind::Weekly),
        _ => false,
    }
}

/// Whether a window whose last known reset was `reset` should be started now. `current` is the
/// window as the latest reading has it.
pub fn is_due(
    reset: Option<DateTime<Utc>>,
    current: Option<&UsageWindow>,
    now: DateTime<Utc>,
    hours: &PrimerHours,
    calendar: &Calendar,
) -> bool {
    let Some(reset) = reset else { return false };
    let grace = Duration::seconds(GRACE_SECONDS);
    if now < reset + grace || !hours.contains(now, calendar) {
        return false;
    }
    // A later reset than the one that passed: somebody has used it since.
    if let Some(next) = current.and_then(|w| w.resets_at) {
        if next > reset + grace {
            return false;
        }
    }
    true
}

/// The noted resets of the providers that will still be started.
pub fn kept(resets: &BTreeMap<String, DateTime<Utc>>, starting: &BTreeSet<Provider>) -> BTreeMap<String, DateTime<Utc>> {
    let prefixes: Vec<String> = starting.iter().map(|p| format!("{}|", AccountKey::primary(*p).id())).collect();
    resets.iter().filter(|(key, _)| prefixes.iter().any(|p| key.starts_with(p))).map(|(k, v)| (k.clone(), *v)).collect()
}

fn key(account: &AccountKey, window: &UsageWindow) -> String {
    format!("{}|{}", account.id(), window.id)
}

/// The reset each eligible window was last seen to be heading for.
#[derive(Debug, Default)]
pub struct Primer {
    resets: BTreeMap<String, DateTime<Utc>>,
}

impl Primer {
    /// Notes every eligible window's coming reset from a reading.
    pub fn record(&mut self, account: &AccountKey, windows: &[UsageWindow], now: DateTime<Utc>) {
        for window in windows.iter().filter(|w| eligible(w, account.provider)) {
            if let Some(reset) = window.resets_at.filter(|r| *r > now) {
                self.resets.insert(key(account, window), reset);
            }
        }
    }

    /// **A reset nobody will act on is not something to wake for.** Turning the starter off, or the
    /// account, left its noted resets here; once they passed, every timer came due at once, `fire`
    /// skipped them, and the loop woke again in five seconds, for as long as the app ran.
    pub fn forget_unstarted(&mut self, starting: &BTreeSet<Provider>) {
        self.resets = kept(&self.resets, starting);
    }

    /// How long until something may be due: the earliest remembered reset plus its grace, moved to
    /// the start of the allowed hours if it falls outside them. None with nothing remembered.
    pub fn next_wake(&self, now: DateTime<Utc>, hours: &PrimerHours, calendar: &Calendar) -> Option<std::time::Duration> {
        let grace = Duration::seconds(GRACE_SECONDS);
        let soonest = self.resets.values().map(|reset| hours.next((*reset + grace).max(now), calendar)).min()?;
        let seconds = (soonest - now).num_seconds().max(MINIMUM_DELAY_SECONDS);
        Some(std::time::Duration::from_secs(seconds as u64))
    }

    /// Whether any window of `account` is due; the due ones are forgotten, because one message
    /// starts every window of the account at once.
    pub fn take_due(
        &mut self,
        account: &AccountKey,
        windows: &[UsageWindow],
        now: DateTime<Utc>,
        hours: &PrimerHours,
        calendar: &Calendar,
    ) -> bool {
        let prefix = format!("{}|", account.id());
        let due: Vec<String> = self
            .resets
            .iter()
            .filter(|(k, _)| k.starts_with(&prefix))
            .filter(|(k, reset)| {
                let id = &k[prefix.len()..];
                is_due(Some(**reset), windows.iter().find(|w| w.id == id), now, hours, calendar)
            })
            .map(|(k, _)| k.clone())
            .collect();
        for k in &due {
            self.resets.remove(k);
        }
        !due.is_empty()
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.resets.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn calendar() -> Calendar {
        Calendar::utc(2)
    }

    fn at(hour: u32, minute: u32, day: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, day, hour, minute, 0).unwrap()
    }

    fn window(kind: WindowKind, resets: Option<DateTime<Utc>>, scope: Option<&str>) -> UsageWindow {
        let mut w = UsageWindow::new("w", kind, 0.0, 18_000).with_reset(resets);
        w.scope = scope.map(str::to_string);
        w
    }

    const DAY: PrimerHours = PrimerHours { start: 7, end: 23 };

    #[test]
    fn daytime_hours_overnight_hours_and_all_day() {
        let c = calendar();
        assert!(DAY.contains(at(7, 0, 1), &c));
        assert!(!DAY.contains(at(23, 0, 1), &c));
        assert!(!DAY.contains(at(3, 0, 1), &c));
        let night = PrimerHours { start: 22, end: 6 };
        assert!(night.contains(at(23, 0, 1), &c));
        assert!(night.contains(at(2, 0, 1), &c));
        assert!(!night.contains(at(12, 0, 1), &c));
        assert!(PrimerHours { start: 9, end: 9 }.contains(at(3, 0, 1), &c));
    }

    #[test]
    fn outside_the_hours_the_next_start_is_when_they_open_today_or_tomorrow() {
        let c = calendar();
        assert_eq!(DAY.next(at(3, 0, 1), &c), at(7, 0, 1));
        assert_eq!(DAY.next(at(23, 30, 1), &c), at(7, 0, 2));
        assert_eq!(DAY.next(at(12, 0, 1), &c), at(12, 0, 1));
    }

    #[test]
    fn due_once_the_reset_and_its_grace_have_passed_inside_the_hours() {
        let c = calendar();
        let reset = Some(at(10, 0, 1));
        assert!(!is_due(reset, None, at(10, 0, 1), &DAY, &c));
        assert!(is_due(reset, None, at(10, 2, 1), &DAY, &c));
        assert!(!is_due(None, None, at(10, 2, 1), &DAY, &c));
    }

    #[test]
    fn not_due_outside_the_hours() {
        assert!(!is_due(Some(at(1, 0, 1)), None, at(2, 0, 1), &DAY, &calendar()));
    }

    #[test]
    fn not_due_when_a_reading_since_shows_a_later_reset_it_has_been_used() {
        let c = calendar();
        let reset = Some(at(10, 0, 1));
        let restarted = window(WindowKind::FiveHour, Some(at(15, 30, 1)), None);
        assert!(!is_due(reset, Some(&restarted), at(11, 0, 1), &DAY, &c));
        // The same reset, still reported, is not a restart.
        let stale = window(WindowKind::FiveHour, reset, None);
        assert!(is_due(reset, Some(&stale), at(11, 0, 1), &DAY, &c));
    }

    #[test]
    fn claude_codes_five_hours_only_codexs_five_hours_and_week_nothing_scoped_or_inferred() {
        assert!(eligible(&window(WindowKind::FiveHour, None, None), Provider::ClaudeCode));
        assert!(!eligible(&window(WindowKind::Weekly, None, None), Provider::ClaudeCode));
        assert!(eligible(&window(WindowKind::FiveHour, None, None), Provider::Codex));
        assert!(eligible(&window(WindowKind::Weekly, None, None), Provider::Codex));
        assert!(!eligible(&window(WindowKind::Weekly, None, Some("Spark")), Provider::Codex));
        assert!(!eligible(&window(WindowKind::FiveHour, None, None), Provider::Cursor));
        let mut estimated = window(WindowKind::FiveHour, None, None);
        estimated.estimate = Some(crate::model::Estimate::PlanPrice);
        assert!(!eligible(&estimated, Provider::ClaudeCode));
    }

    #[test]
    fn a_provider_no_longer_started_keeps_no_noted_reset_to_wake_for() {
        let reset = at(10, 0, 1);
        let claude = AccountKey::primary(Provider::ClaudeCode).id();
        let codex = AccountKey::primary(Provider::Codex).id();
        let noted: BTreeMap<String, DateTime<Utc>> = [
            (format!("{claude}|five_hour"), reset),
            (format!("{codex}|primary"), reset),
            (format!("{codex}|secondary"), reset),
        ]
        .into();
        let only_codex: BTreeSet<Provider> = [Provider::Codex].into();
        let kept_keys: Vec<String> = kept(&noted, &only_codex).into_keys().collect();
        assert_eq!(kept_keys, vec![format!("{codex}|primary"), format!("{codex}|secondary")]);
        assert!(kept(&noted, &BTreeSet::new()).is_empty());
    }

    #[test]
    fn the_scheduler_remembers_future_resets_wakes_after_the_grace_and_forgets_what_it_started() {
        let c = calendar();
        let account = AccountKey::primary(Provider::ClaudeCode);
        let mut primer = Primer::default();
        let now = at(9, 0, 1);

        // A reset already past, a scoped window and a week are not remembered for Claude Code.
        primer.record(
            &account,
            &[
                window(WindowKind::FiveHour, Some(at(8, 0, 1)), None),
                window(WindowKind::Weekly, Some(at(12, 0, 1)), None),
                window(WindowKind::FiveHour, Some(at(12, 0, 1)), Some("Opus")),
            ],
            now,
        );
        assert_eq!(primer.len(), 0);
        assert_eq!(primer.next_wake(now, &DAY, &c), None);

        let five = window(WindowKind::FiveHour, Some(at(10, 0, 1)), None);
        primer.record(&account, std::slice::from_ref(&five), now);
        assert_eq!(primer.len(), 1);
        assert_eq!(primer.next_wake(now, &DAY, &c), Some(std::time::Duration::from_secs(3_660)));

        // Not due until the grace has passed.
        assert!(!primer.take_due(&account, &[], at(10, 0, 1), &DAY, &c));
        assert!(primer.take_due(&account, &[], at(10, 1, 1), &DAY, &c));
        assert_eq!(primer.len(), 0, "started windows are forgotten until the next reading names a new reset");
    }

    #[test]
    fn a_reset_at_night_waits_for_the_hours_to_open_and_a_due_one_is_not_waited_for() {
        let c = calendar();
        let account = AccountKey::primary(Provider::Codex);
        let mut primer = Primer::default();
        let now = at(1, 0, 1);
        primer.record(&account, &[window(WindowKind::FiveHour, Some(at(3, 0, 1)), None)], now);
        assert_eq!(primer.next_wake(now, &DAY, &c), Some(std::time::Duration::from_secs(6 * 3_600)));
        // Already past the reset and inside the hours: the minimum delay, not a busy loop.
        assert_eq!(primer.next_wake(at(8, 0, 1), &DAY, &c), Some(std::time::Duration::from_secs(5)));
    }

    #[test]
    fn switching_the_starter_off_drops_the_noted_resets() {
        let c = calendar();
        let account = AccountKey::primary(Provider::Codex);
        let mut primer = Primer::default();
        primer.record(&account, &[window(WindowKind::FiveHour, Some(at(10, 0, 1)), None)], at(9, 0, 1));
        primer.forget_unstarted(&BTreeSet::new());
        assert_eq!(primer.next_wake(at(9, 0, 1), &DAY, &c), None);
    }

    #[test]
    fn a_later_reset_on_the_latest_reading_means_the_reader_started_it() {
        let c = calendar();
        let account = AccountKey::primary(Provider::Codex);
        let mut primer = Primer::default();
        primer.record(&account, &[window(WindowKind::FiveHour, Some(at(10, 0, 1)), None)], at(9, 0, 1));
        let used = window(WindowKind::FiveHour, Some(at(15, 10, 1)), None);
        assert!(!primer.take_due(&account, std::slice::from_ref(&used), at(10, 30, 1), &DAY, &c));
    }
}
