// Ported from upstream Sources/Pulse/Usage/ProviderUsage.swift and MonitoredAccount.swift.
//! The usage model every provider reports into and every surface reads from.
//!
//! Rule carried over from upstream: Pulse never invents a percentage. Every
//! figure here is one the provider reported, except windows carrying an
//! `estimate`, which say so on screen.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::provider::Provider;

/// Provider plus which login. The primary account's id is the provider's raw value.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AccountKey {
    pub provider: Provider,
    #[serde(default)]
    pub slot: String,
}

impl AccountKey {
    pub fn primary(provider: Provider) -> Self {
        Self { provider, slot: String::new() }
    }

    pub fn id(&self) -> String {
        if self.slot.is_empty() {
            self.provider.raw().to_string()
        } else {
            format!("{}#{}", self.provider.raw(), self.slot)
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        let mut parts = id.splitn(2, '#');
        let provider = Provider::from_raw(parts.next()?)?;
        Some(Self { provider, slot: parts.next().unwrap_or("").to_string() })
    }

    pub fn is_primary(&self) -> bool {
        self.slot.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type", content = "seconds")]
pub enum WindowKind {
    FiveHour,
    Weekly,
    Spend,
    /// Prepaid credit; not a limit and never "turns over".
    Balance,
    Daily,
    Messages,
    Monthly,
    TopUp,
    Credits,
    SharedCredits,
    Other(i64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Estimate {
    PlanPrice,
    SinceTopUp,
    YourBudget,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Expiry {
    pub amount: f64,
    pub at: DateTime<Utc>,
}

/// One limit exactly as a provider reports it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageWindow {
    pub id: String,
    pub kind: WindowKind,
    pub scope: Option<String>,
    pub used_fraction: f64,
    pub window_seconds: i64,
    pub resets_at: Option<DateTime<Utc>>,
    /// Whether `window_seconds` was stated by the provider (vs a sort key).
    #[serde(default = "yes")]
    pub reports_length: bool,
    #[serde(default)]
    pub estimate: Option<Estimate>,
    #[serde(default)]
    pub is_exhausted: bool,
    #[serde(default)]
    pub next_expiry: Option<Expiry>,
    /// Extensions only: a name the reporter gave this limit.
    #[serde(default)]
    pub label: Option<String>,
}

fn yes() -> bool {
    true
}

/// Longest length a reply may state for a window: ten years.
pub const LONGEST_LENGTH: i64 = 10 * 366 * 86_400;

impl UsageWindow {
    pub fn new(id: impl Into<String>, kind: WindowKind, used_fraction: f64, window_seconds: i64) -> Self {
        Self {
            id: id.into(),
            kind,
            scope: None,
            used_fraction,
            window_seconds,
            resets_at: None,
            reports_length: true,
            estimate: None,
            is_exhausted: false,
            next_expiry: None,
            label: None,
        }
    }

    pub fn with_reset(mut self, at: Option<DateTime<Utc>>) -> Self {
        self.resets_at = at;
        self
    }

    pub fn with_scope(mut self, scope: impl Into<String>) -> Self {
        self.scope = Some(scope.into());
        self
    }

    pub fn exhausted(mut self, exhausted: bool) -> Self {
        self.is_exhausted = exhausted;
        self
    }

    /// `count` units of `unit_seconds`, or None when it overflows or exceeds any plausible window.
    pub fn length(count: i64, unit_seconds: i64) -> Option<i64> {
        if count <= 0 || unit_seconds <= 0 {
            return None;
        }
        let seconds = count.checked_mul(unit_seconds)?;
        (seconds <= LONGEST_LENGTH).then_some(seconds)
    }

    pub fn is_estimated(&self) -> bool {
        self.estimate.is_some()
    }

    pub fn remaining_fraction(&self) -> f64 {
        (1.0 - self.used_fraction).clamp(0.0, 1.0)
    }

    /// Whole percentage that never rounds "some" to 0 or "not all" to 100.
    pub fn percent_value(&self, remaining: bool) -> i64 {
        figure(if remaining { self.remaining_fraction() } else { self.used_fraction })
    }

    /// Spent by the provider's own flag, or a figure at/over 100%.
    pub fn is_spent(&self) -> bool {
        self.is_exhausted || self.used_fraction >= 1.0
    }

    /// How full a surface draws this limit's arc, with the reader's exhausted-ring choice.
    ///
    /// `spent_fills` is the surface's upstream default for a spent limit that is being kept
    /// filled: the panel ring draws it full (`true`), the tray ring and the dashboard bar keep
    /// the used figure instead (`false`). Windows difference (owner-approved #29): with
    /// `keeps_filled` off, a countdown fraction is the true remainder, including 0% — the red
    /// track, not a full arc, carries the warning. Consumed mode is unchanged either way.
    pub fn drawn_fraction(&self, remaining: bool, keeps_filled: bool, spent_fills: bool) -> f64 {
        let used = if self.used_fraction.is_finite() { self.used_fraction.clamp(0.0, 1.0) } else { 0.0 };
        if remaining && !keeps_filled && self.is_spent() {
            return 1.0 - used;
        }
        if self.is_spent() {
            return if spent_fills { 1.0 } else { used };
        }
        if remaining {
            1.0 - used
        } else {
            used
        }
    }

    /// How much of the window has elapsed; None unless both reset and a stated length are known.
    pub fn elapsed_fraction(&self, now: DateTime<Utc>) -> Option<f64> {
        let resets_at = self.resets_at?;
        if !self.reports_length || self.window_seconds <= 0 {
            return None;
        }
        let remaining = (resets_at - now).num_milliseconds() as f64 / 1000.0;
        Some((1.0 - remaining / self.window_seconds as f64).clamp(0.0, 1.0))
    }

    /// Whether this window has unambiguously turned over since it was seen at `fraction`.
    pub fn has_turned_over(&self, fraction: f64, previous_reset: Option<DateTime<Utc>>) -> bool {
        if matches!(self.kind, WindowKind::Balance | WindowKind::TopUp) {
            return false;
        }
        let moved_on = match (self.resets_at, previous_reset) {
            (Some(new), Some(old)) => (new - old).num_seconds() > 60,
            _ => false,
        };
        if matches!(self.kind, WindowKind::Credits | WindowKind::SharedCredits) {
            return moved_on;
        }
        moved_on || fraction - self.used_fraction >= 0.4
    }
}

fn figure(fraction: f64) -> i64 {
    if !fraction.is_finite() {
        return 0;
    }
    let percent = fraction.clamp(0.0, 1.0) * 100.0;
    if percent <= 0.0 {
        return 0;
    }
    if percent >= 100.0 {
        return 100;
    }
    percent.round().clamp(1.0, 99.0) as i64
}

/// Why there is nothing to show. Serialized as the upstream case name; the UI
/// turns it into the localized message using the same English keys as upstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Unavailability {
    Loading,
    NotConnected,
    AwaitingResponse,
    NoLimitsReported,
    SignInRequired,
    ClaudeSignInRequired,
    ClaudeLoginExpired,
    CodexNotInstalled,
    CodexServerFailed,
    KiroNotInstalled,
    KiroVersionUnsupported,
    KiroSignInRequired,
    AntigravityNotRunning,
    AntigravityNotAnswering,
    CursorSignInRequired,
    CursorLoginExpired,
    GrokSignInRequired,
    GrokLoginExpired,
    SignedOut,
    NotSignedIn,
    ZaiNoCodingPlan,
    ServerAddressMissing,
    ServerAddressRefused,
    ApiKeyMissing,
    ApiKeyRefused,
    Unreachable,
    UnreadableReply,
    RateLimited,
    ServerError,
    SessionMissing,
    SessionExpired,
    LocalLoginMissing,
    LocalLoginExpired,
    LocalAppMissing,
    NoPlan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind", content = "reason")]
pub enum UsageState {
    Live,
    Stale,
    Unavailable(Unavailability),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreditAmount {
    pub amount: f64,
    /// ISO currency code as the provider gave it.
    pub currency: String,
}

/// Which route produced a reading (OAuth endpoint, local log, browser session, …).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UsageRoute {
    Endpoint,
    StatusLine,
    LocalLogin,
    BrowserSession,
    ApiKey,
    Helper,
    LocalStore,
}

/// Everything Pulse currently knows about one account.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUsage {
    pub account: AccountKey,
    /// Windows difference: verified billing identity for local spend matching. Kept only in
    /// memory: never sent to the UI, copied by --json, or restored from a stale usage cache.
    #[serde(skip)]
    pub spend_identity: Option<crate::spend::account::AccountIdentity>,
    /// Ordered as the provider reports them.
    pub windows: Vec<UsageWindow>,
    pub observed_at: Option<DateTime<Utc>>,
    pub state: UsageState,
    pub plan: Option<String>,
    /// Display string; may be prose ("Unlimited"). Never compare on it.
    pub credit_balance: Option<String>,
    #[serde(default)]
    pub credit_remaining: Option<CreditAmount>,
    #[serde(default)]
    pub origin: Option<UsageRoute>,
    #[serde(default)]
    pub is_cached: bool,
}

/// Maximum age a cached reading may be shown at all (upstream `UsageCache.maximumAge`).
pub const CACHE_MAXIMUM_AGE_SECONDS: i64 = 7 * 86_400;

impl ProviderUsage {
    pub fn unavailable(account: AccountKey, reason: Unavailability) -> Self {
        Self {
            account,
            spend_identity: None,
            windows: Vec::new(),
            observed_at: None,
            state: UsageState::Unavailable(reason),
            plan: None,
            credit_balance: None,
            credit_remaining: None,
            origin: None,
            is_cached: false,
        }
    }

    pub fn live(account: AccountKey, windows: Vec<UsageWindow>, now: DateTime<Utc>) -> Self {
        Self {
            account,
            spend_identity: None,
            windows,
            observed_at: Some(now),
            state: UsageState::Live,
            plan: None,
            credit_balance: None,
            credit_remaining: None,
            origin: None,
            is_cached: false,
        }
    }

    pub fn with_plan(mut self, plan: Option<String>) -> Self {
        self.plan = plan.filter(|p| !p.trim().is_empty());
        self
    }

    pub fn with_origin(mut self, origin: UsageRoute) -> Self {
        self.origin = Some(origin);
        self
    }

    pub fn reports_something(&self) -> bool {
        !self.windows.is_empty() || self.credit_balance.is_some()
    }

    /// The reading as it stands at `now`: reset windows dropped, too-old readings gone.
    pub fn current(&self, now: DateTime<Utc>) -> Option<Self> {
        if let Some(observed) = self.observed_at {
            if (now - observed).num_seconds() > CACHE_MAXIMUM_AGE_SECONDS {
                return None;
            }
        }
        let kept: Vec<_> = self
            .windows
            .iter()
            .filter(|w| w.resets_at.map_or(true, |r| r > now))
            .cloned()
            .collect();
        if kept.is_empty() && self.credit_balance.is_none() {
            return None;
        }
        let mut copy = self.clone();
        copy.windows = kept;
        Some(copy)
    }

    /// The window the rail ring shows: a pinned one, else the five-hour limit, else the fullest.
    ///
    /// Windows difference (owner-approved): upstream shows the fullest limit. The five-hour
    /// window is the one that moves while working, so it comes first; among several, the
    /// account-wide (unscoped) one, then the fullest. The second ring still carries the fullest
    /// of the rest, so a weekly limit stays in sight.
    pub fn headline_window(&self, preferring: Option<&str>) -> Option<&UsageWindow> {
        if let Some(id) = preferring {
            if let Some(pinned) = self.windows.iter().find(|w| w.id == id) {
                return Some(pinned);
            }
        }
        let five_hour = |w: &&UsageWindow| w.kind == WindowKind::FiveHour;
        max_by_used(self.windows.iter().filter(five_hour).filter(|w| w.scope.is_none()))
            .or_else(|| max_by_used(self.windows.iter().filter(five_hour)))
            .or_else(|| max_by_used(self.windows.iter()))
    }

    /// The fullest limit in the headline's own model group, else the fullest of the rest.
    pub fn second_window(&self, preferring: Option<&str>) -> Option<&UsageWindow> {
        let headline = self.headline_window(preferring)?;
        if self.windows.len() < 2 {
            return None;
        }
        let rest: Vec<_> = self.windows.iter().filter(|w| w.id != headline.id).collect();
        let same_group: Vec<_> = rest.iter().copied().filter(|w| w.scope == headline.scope).collect();
        max_by_used(if same_group.is_empty() { rest } else { same_group }.into_iter())
    }
}

/// Like Swift's `max(by:)`: the first of equals wins.
fn max_by_used<'a>(iter: impl Iterator<Item = &'a UsageWindow>) -> Option<&'a UsageWindow> {
    iter.fold(None, |best: Option<&UsageWindow>, w| match best {
        Some(b) if b.used_fraction >= w.used_fraction => Some(b),
        _ => Some(w),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_never_rounds_away_some_or_all() {
        let w = |f| UsageWindow::new("a", WindowKind::Weekly, f, 604_800);
        assert_eq!(w(0.0).percent_value(false), 0);
        assert_eq!(w(0.0003).percent_value(false), 1);
        assert_eq!(w(0.996).percent_value(false), 99);
        assert_eq!(w(0.996).percent_value(true), 1);
        assert_eq!(w(1.0).percent_value(false), 100);
        assert_eq!(w(f64::NAN).percent_value(false), 0);
    }

    #[test]
    fn exhausted_ring_fill_choice_shows_the_true_remainder() {
        let w = |f: f64, exhausted: bool| UsageWindow::new("a", WindowKind::Weekly, f, 604_800).exhausted(exhausted);

        // Kept filled (the default): the panel ring fills a spent limit, the tray ring and
        // dashboard bar keep the used figure. Consumed mode is the same either way.
        assert_eq!(w(0.5, true).drawn_fraction(true, true, true), 1.0);
        assert_eq!(w(0.5, true).drawn_fraction(true, true, false), 0.5);
        assert_eq!(w(0.5, true).drawn_fraction(false, true, true), 1.0);
        assert_eq!(w(0.5, true).drawn_fraction(false, true, false), 0.5);

        // Opted out: a countdown fraction is the true remainder, including 0%. Both surfaces
        // agree, and consumed mode does not change.
        assert_eq!(w(0.5, true).drawn_fraction(true, false, true), 0.5);
        assert_eq!(w(0.5, true).drawn_fraction(true, false, false), 0.5);
        assert_eq!(w(0.5, true).drawn_fraction(false, false, true), 1.0);
        assert_eq!(w(0.5, true).drawn_fraction(false, false, false), 0.5);
    }

    #[test]
    fn drawn_fraction_clamps_and_counts_down_to_zero() {
        let spent = |f: f64| UsageWindow::new("a", WindowKind::FiveHour, f, 18_000).exhausted(true);
        let plain = |f: f64| UsageWindow::new("a", WindowKind::FiveHour, f, 18_000);

        // At 100% and over: nothing is left, so an opted-out countdown arc is empty.
        assert_eq!(spent(1.0).drawn_fraction(true, false, true), 0.0);
        assert_eq!(spent(1.4).drawn_fraction(true, false, true), 0.0);
        assert_eq!(plain(1.4).drawn_fraction(true, false, false), 0.0);
        // Negative clamps: empty used, full while counting down.
        assert_eq!(plain(-0.2).drawn_fraction(false, false, false), 0.0);
        assert_eq!(plain(-0.2).drawn_fraction(true, false, false), 1.0);
        // Near zero keeps the sliver.
        assert!((plain(0.004).drawn_fraction(true, false, false) - 0.996).abs() < 1e-9);
        // Near 100% used, the actual remainder is near zero and is what both surfaces draw;
        // kept filled, the same limit still draws full.
        assert!((plain(0.996).drawn_fraction(true, false, false) - 0.004).abs() < 1e-9);
        assert!((spent(0.996).drawn_fraction(true, false, true) - 0.004).abs() < 1e-9);
        assert!((spent(0.996).drawn_fraction(true, false, false) - 0.004).abs() < 1e-9);
        assert_eq!(spent(0.996).drawn_fraction(true, true, true), 1.0);
        // A plain 0% is not spent, so it stays full while counting down even opted out.
        assert_eq!(plain(0.0).drawn_fraction(true, false, true), 1.0);
    }

    #[test]
    fn headline_keeps_first_of_equals() {
        let usage = ProviderUsage::live(
            AccountKey::primary(Provider::ClaudeCode),
            vec![
                UsageWindow::new("five", WindowKind::FiveHour, 0.2, 18_000),
                UsageWindow::new("week", WindowKind::Weekly, 0.2, 604_800),
            ],
            Utc::now(),
        );
        assert_eq!(usage.headline_window(None).unwrap().id, "five");
        assert_eq!(usage.second_window(None).unwrap().id, "week");
    }

    #[test]
    fn headline_prefers_the_five_hour_limit() {
        let mut week = UsageWindow::new("week", WindowKind::Weekly, 0.8, 604_800);
        let usage = |windows| ProviderUsage::live(AccountKey::primary(Provider::Codex), windows, Utc::now());
        let mut scoped = UsageWindow::new("spark.five", WindowKind::FiveHour, 0.9, 18_000);
        scoped.scope = Some("spark".into());
        let u = usage(vec![week.clone(), scoped.clone(), UsageWindow::new("five", WindowKind::FiveHour, 0.1, 18_000)]);
        assert_eq!(u.headline_window(None).unwrap().id, "five", "the account-wide five-hour limit first");
        assert_eq!(u.second_window(None).unwrap().id, "week", "the weekly limit stays on the second ring");
        assert_eq!(u.headline_window(Some("week")).unwrap().id, "week", "a pin still wins");
        assert_eq!(usage(vec![week.clone(), scoped]).headline_window(None).unwrap().id, "spark.five");
        week.used_fraction = 0.2;
        let u = usage(vec![week, UsageWindow::new("day", WindowKind::Daily, 0.5, 86_400)]);
        assert_eq!(u.headline_window(None).unwrap().id, "day", "without one, the fullest");
    }

    #[test]
    fn balance_never_turns_over() {
        let w = UsageWindow::new("b", WindowKind::Balance, 0.1, 0);
        assert!(!w.has_turned_over(0.9, None));
    }

    #[test]
    fn account_id_round_trips() {
        let key = AccountKey { provider: Provider::Codex, slot: "work".into() };
        assert_eq!(AccountKey::from_id(&key.id()), Some(key));
    }
}
