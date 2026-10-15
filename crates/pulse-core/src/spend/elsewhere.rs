// Ported from upstream Sources/Pulse/Usage/ElsewhereWatch.swift.
//! Notices a limit being spent where this PC's logs cannot see it (another computer on the same
//! account, or the website) so the value estimate can stand down instead of dividing a part of
//! the spending by the whole of the percentage.
//!
//! The evidence is a rise with nothing behind it. Between two live readings of one window in one
//! cycle, the percentage climbed at least `MINIMUM_RISE` while this PC's logs show next to nothing
//! spent from a little before the first reading (`LAG`: a request's percentage can land a reading
//! late) to the second. Two points, not one: the providers report whole percentages, and a
//! one-point tick can come from a sliver of work. Once seen, the cycle stays marked until the
//! window resets: work that stopped being visible did not stop counting.
//!
//! What this cannot see is the two machines working at the same time: the local spend is then
//! real, only short. That case keeps an estimate that is too low, and the caption under the
//! estimate still says so.
//!
//! The upstream watch reads the ledger itself; here the caller does (it is blocking work), so
//! [`ElsewhereWatch::observe`] returns the [`Check`]s to make and [`ElsewhereWatch::resolve`]
//! takes their answers.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use chrono::{DateTime, Duration, Utc};

use crate::model::{AccountKey, ProviderUsage, UsageState, UsageWindow};
use crate::provider::Provider;

pub const MINIMUM_RISE: f64 = 0.02;
/// Less than this spent locally while the percentage rose two points is nothing: a two-point rise
/// is dollars on every plan Pulse has seen.
pub const QUIET_SPEND: f64 = 0.05;
/// Fewer tokens than this in the span is no work at all: a single request re-reads its whole
/// context, tens of thousands of tokens.
pub const QUIET_TOKENS: f64 = 1_000.0;
const LAG_SECONDS: i64 = 15 * 60;
/// Resets are reported to the second and drift by a fraction of one between readings; a cycle is
/// the same cycle within this.
const SAME_CYCLE_SECONDS: i64 = 120;

#[derive(Debug, Clone)]
struct Point {
    fraction: f64,
    at: DateTime<Utc>,
    resets_at: DateTime<Utc>,
    identity: Option<super::account::AccountIdentity>,
}

/// A rise worth looking into: what this PC spent between `from` and `to` decides whether it was
/// spent here.
#[derive(Debug, Clone)]
pub struct Check {
    key: String,
    pub provider: Provider,
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    rise: f64,
    resets_at: DateTime<Utc>,
}

#[derive(Default)]
struct Inner {
    /// The cycle each window was seen spent elsewhere in, by its reset.
    cycles: HashMap<String, DateTime<Utc>>,
    last: HashMap<String, Point>,
}

pub struct ElsewhereWatch {
    inner: Mutex<Inner>,
    file: Option<PathBuf>,
}

impl ElsewhereWatch {
    /// `file` keeps the marks across launches (`used-elsewhere.json`); marks already past their
    /// reset are dropped on load.
    pub fn new(file: Option<PathBuf>) -> Self {
        let mut inner = Inner::default();
        if let Some(saved) = file
            .as_ref()
            .and_then(|f| std::fs::read(f).ok())
            .and_then(|b| serde_json::from_slice::<HashMap<String, DateTime<Utc>>>(&b).ok())
        {
            let now = Utc::now();
            inner.cycles = saved.into_iter().filter(|(_, at)| *at > now).collect();
        }
        Self { inner: Mutex::new(inner), file }
    }

    /// Whether the rise between two readings had nothing on this PC behind it. Tokens as well as
    /// money: a model with no published price spends nothing in dollars however hard it works, and
    /// a first launch offline has no prices at all; measured in money alone, a day's real work
    /// here read as somebody else's.
    pub fn spent_elsewhere(rise: f64, local_cost: f64, local_tokens: f64) -> bool {
        rise >= MINIMUM_RISE - 1e-9 && local_cost < QUIET_SPEND && local_tokens < QUIET_TOKENS
    }

    /// Takes a live reading in. Only the providers whose spending Pulse reads from this PC's
    /// transcripts, and only account-wide windows: the ones the estimate is made for.
    pub fn observe(&self, reading: &ProviderUsage, account: &AccountKey, now: DateTime<Utc>) -> Vec<Check> {
        let Some(observed_at) = reading.observed_at else { return Vec::new() };
        if !super::supports(account.provider) || reading.state != UsageState::Live {
            return Vec::new();
        }
        let mut checks = Vec::new();
        let mut inner = self.inner.lock().unwrap();
        for window in reading.windows.iter().filter(|w| w.scope.is_none()) {
            let Some(resets_at) = window.resets_at.filter(|r| *r > now) else { continue };
            let key = key(account, &window.id);
            let point = Point { fraction: window.used_fraction, at: observed_at, resets_at, identity: reading.spend_identity.clone() };
            let previous = inner.last.insert(key.clone(), point.clone());
            let Some(previous) = previous else { continue };
            // Windows difference: signing in again can retain a Pulse slot and reset time;
            // the percentages of two billing identities are still not a rise in one account.
            if previous.identity != point.identity
                || (previous.resets_at - resets_at).num_seconds().abs() >= SAME_CYCLE_SECONDS
                || observed_at <= previous.at
                || point.fraction - previous.fraction < MINIMUM_RISE - 1e-9
                || is_marked(&inner, &key, resets_at)
            {
                continue;
            }
            checks.push(Check {
                key,
                provider: account.provider,
                from: previous.at - Duration::seconds(LAG_SECONDS),
                to: observed_at,
                rise: point.fraction - previous.fraction,
                resets_at,
            });
        }
        checks
    }

    /// The answer to a [`Check`]: what this PC spent over its span. Marks the window's cycle when
    /// the rise had nothing behind it.
    pub fn resolve(&self, check: &Check, local_cost: f64, local_tokens: f64) -> bool {
        if !Self::spent_elsewhere(check.rise, local_cost, local_tokens) {
            return false;
        }
        let mut inner = self.inner.lock().unwrap();
        let now = Utc::now();
        inner.cycles.retain(|_, at| *at > now);
        inner.cycles.insert(check.key.clone(), check.resets_at);
        if let Some(file) = &self.file {
            if let Ok(bytes) = serde_json::to_vec(&inner.cycles) {
                if let Some(dir) = file.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let tmp = file.with_extension("tmp");
                if std::fs::write(&tmp, bytes).is_ok() {
                    let _ = std::fs::rename(&tmp, file);
                }
            }
        }
        true
    }

    /// Whether this window's current cycle has been seen spent elsewhere.
    pub fn used_elsewhere(&self, window: &UsageWindow, account: &AccountKey) -> bool {
        let Some(resets_at) = window.resets_at else { return false };
        is_marked(&self.inner.lock().unwrap(), &key(account, &window.id), resets_at)
    }
}

fn is_marked(inner: &Inner, key: &str, resets_at: DateTime<Utc>) -> bool {
    inner.cycles.get(key).is_some_and(|at| (*at - resets_at).num_seconds().abs() < SAME_CYCLE_SECONDS)
}

fn key(account: &AccountKey, window: &str) -> String {
    format!("{}|{window}", account.id())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::WindowKind;
    use chrono::TimeZone;

    fn at(seconds: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(1_800_000_000 + seconds, 0).unwrap()
    }

    fn account() -> AccountKey {
        AccountKey::primary(Provider::ClaudeCode)
    }

    fn reading(fraction: f64, observed: i64, resets: i64) -> ProviderUsage {
        let mut usage = ProviderUsage::unavailable(account(), crate::model::Unavailability::Loading);
        usage.state = UsageState::Live;
        usage.observed_at = Some(at(observed));
        usage.windows = vec![UsageWindow::new("weekly", WindowKind::Weekly, fraction, 7 * 86_400).with_reset(Some(at(resets)))];
        usage
    }

    #[test]
    fn a_rise_with_nothing_spent_here_marks_the_cycle() {
        let watch = ElsewhereWatch::new(None);
        let now = at(0);
        assert!(watch.observe(&reading(0.10, 0, 100_000), &account(), now).is_empty());
        let checks = watch.observe(&reading(0.13, 600, 100_000), &account(), now);
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].from, at(0) - Duration::seconds(900));
        assert_eq!(checks[0].to, at(600));

        let window = reading(0.13, 600, 100_000).windows.remove(0);
        assert!(!watch.used_elsewhere(&window, &account()));
        assert!(watch.resolve(&checks[0], 0.0, 0.0));
        assert!(watch.used_elsewhere(&window, &account()));
        // The same cycle with a reset that drifted by a second is the same cycle.
        let drifted = reading(0.13, 600, 100_001).windows.remove(0);
        assert!(watch.used_elsewhere(&drifted, &account()));
        // The next cycle is not marked.
        let next = reading(0.01, 600, 100_000 + 7 * 86_400).windows.remove(0);
        assert!(!watch.used_elsewhere(&next, &account()));
    }

    #[test]
    fn work_done_here_is_not_elsewhere() {
        assert!(!ElsewhereWatch::spent_elsewhere(0.05, 1.50, 0.0));
        assert!(!ElsewhereWatch::spent_elsewhere(0.05, 0.0, 50_000.0));
        assert!(ElsewhereWatch::spent_elsewhere(0.02, 0.0, 10.0));
        assert!(!ElsewhereWatch::spent_elsewhere(0.01, 0.0, 0.0));
        let watch = ElsewhereWatch::new(None);
        watch.observe(&reading(0.10, 0, 100_000), &account(), at(0));
        let checks = watch.observe(&reading(0.20, 600, 100_000), &account(), at(0));
        assert!(!watch.resolve(&checks[0], 3.0, 400_000.0));
    }

    #[test]
    fn a_one_point_tick_and_a_new_cycle_are_not_evidence() {
        let watch = ElsewhereWatch::new(None);
        let now = at(0);
        watch.observe(&reading(0.10, 0, 100_000), &account(), now);
        assert!(watch.observe(&reading(0.11, 600, 100_000), &account(), now).is_empty());
        // A reset that moved by a whole cycle is another window.
        assert!(watch.observe(&reading(0.40, 1200, 100_000 + 7 * 86_400), &account(), now).is_empty());
    }

    #[test]
    fn signing_in_again_with_the_same_reset_does_not_look_like_spending_elsewhere() {
        let watch = ElsewhereWatch::new(None);
        let identity = |id| super::super::account::AccountIdentity::claude_profile(&serde_json::json!({"account": {"uuid": id}, "organization": {"uuid": "o"}}));
        let mut first = reading(0.1, 0, 100_000);
        first.spend_identity = identity("a");
        watch.observe(&first, &account(), at(0));
        let mut second = reading(0.5, 600, 100_000);
        second.spend_identity = identity("b");
        assert!(watch.observe(&second, &account(), at(600)).is_empty());
    }

    #[test]
    fn only_account_wide_live_readings_of_transcript_providers_count() {
        let watch = ElsewhereWatch::new(None);
        let other = AccountKey::primary(Provider::Cursor);
        let mut usage = reading(0.1, 0, 100_000);
        usage.account = other.clone();
        assert!(watch.observe(&usage, &other, at(0)).is_empty());
        let mut scoped = reading(0.1, 0, 100_000);
        scoped.windows[0].scope = Some("Opus".into());
        watch.observe(&scoped, &account(), at(0));
        scoped.windows[0].used_fraction = 0.5;
        scoped.observed_at = Some(at(600));
        assert!(watch.observe(&scoped, &account(), at(0)).is_empty());
    }

    #[test]
    fn marks_survive_a_restart_until_their_reset() {
        let dir = std::env::temp_dir().join(format!("pulse-elsewhere-{}", std::process::id()));
        let file = dir.join("used-elsewhere.json");
        let _ = std::fs::remove_dir_all(&dir);
        let future = Utc::now() + Duration::days(3);
        let window = UsageWindow::new("weekly", WindowKind::Weekly, 0.3, 7 * 86_400).with_reset(Some(future));
        {
            let watch = ElsewhereWatch::new(Some(file.clone()));
            let check = Check {
                key: key(&account(), "weekly"),
                provider: Provider::ClaudeCode,
                from: Utc::now(),
                to: Utc::now(),
                rise: 0.05,
                resets_at: future,
            };
            assert!(watch.resolve(&check, 0.0, 0.0));
        }
        assert!(ElsewhereWatch::new(Some(file)).used_elsewhere(&window, &account()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
