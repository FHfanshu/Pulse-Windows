// Ported from upstream Usage/UsageAlerts.swift (AlertMemory, UsageAlert) and Docs/notifications.md.
//! Usage notifications: what Pulse may say, and when.
//!
//! The rules are strict. Pulse says nothing it did not witness; every alert is
//! off by default; a limit already past the line when the setting turns on is
//! announced once; a push route going quiet is never a failure; a reset needs
//! unambiguous evidence ([`UsageWindow::has_turned_over`]).
//!
//! Everything here is pure apart from the engine's own memory: no clock, disk
//! or notification centre. The shell reads the clock, loads and saves the
//! memory (`AlertEngine` is serde), localizes the returned [`Text`] and posts.
//!
//! Not ported yet: "when a service is down" (status pages) and "recap ready".

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::model::{AccountKey, ProviderUsage, Unavailability, UsageRoute, UsageState, UsageWindow, WindowKind};
use crate::settings::AppSettings;

/// Failed passes in a row before saying so.
pub const FAILURES_BEFORE_SAYING: u32 = 3;
/// How old the figures on the panel must be before a stale reading counts against the account.
pub const STALENESS_BEFORE_SAYING_SECONDS: i64 = 1800;
/// A live reading older than this is not judged (upstream `UsageCache.maximumAge`, 24 hours).
pub const MAXIMUM_READING_AGE_SECONDS: i64 = 86_400;

// ---------------------------------------------------------------------------
// Words
// ---------------------------------------------------------------------------

/// Text the shell localizes: keys are upstream's English strings, `%@` filled in order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type", content = "value")]
pub enum Text {
    /// Shown as is (an account name, a figure).
    Plain(String),
    Key { key: String, args: Vec<Text> },
    /// A reset time, formatted by the shell: clock only when today, else date and clock.
    ResetTime(DateTime<Utc>),
    /// Sentences, with a space only where one is wanted (none after a full-width stop).
    Sentences(Vec<Text>),
    /// Parts joined with " · ".
    Dotted(Vec<Text>),
}

impl Text {
    fn key(key: &str) -> Self {
        Text::Key { key: key.to_string(), args: Vec::new() }
    }

    fn key_with(key: &str, args: Vec<Text>) -> Self {
        Text::Key { key: key.to_string(), args }
    }

    fn plain(text: impl Into<String>) -> Self {
        Text::Plain(text.into())
    }
}

/// One system notification, ready to localize and post.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Notification {
    /// Stable per (account, limit, thing said), so a newer one replaces the older.
    pub identifier: String,
    pub account: String,
    pub title: Text,
    pub subtitle: Option<Text>,
    pub body: Text,
}

// ---------------------------------------------------------------------------
// What can be said
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertKind {
    /// Went past the figure the user asked to be told about.
    Approaching { percent: u8 },
    /// The provider says it is spent, or the share rounds down to 100.
    Spent,
    /// A window that was warned about has unambiguously turned over.
    Reset,
    /// A prepaid balance fell under the figure set for the account.
    LowBalance,
    /// Several passes in a row failed; the reason when there is one.
    Unreadable(Option<Unavailability>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Alert {
    pub account: AccountKey,
    pub kind: AlertKind,
    /// The limit it is about, or none when about the account as a whole.
    pub window: Option<UsageWindow>,
    /// Only for `LowBalance`: the provider's own display string for the balance.
    pub remaining: Option<String>,
}

impl Alert {
    fn new(account: &AccountKey, kind: AlertKind, window: Option<&UsageWindow>) -> Self {
        Self { account: account.clone(), kind, window: window.cloned(), remaining: None }
    }

    pub fn identifier(&self) -> String {
        let limit = self.window.as_ref().map_or("-", |w| w.id.as_str());
        let what = match self.kind {
            AlertKind::Approaching { percent } => format!("approaching-{percent}"),
            AlertKind::Spent => "spent".into(),
            AlertKind::Reset => {
                let at = self.window.as_ref().and_then(|w| w.resets_at).map_or(0, |d| d.timestamp());
                format!("reset-{at}")
            }
            AlertKind::LowBalance => "low-balance".into(),
            AlertKind::Unreadable(_) => "unreadable".into(),
        };
        format!("{}|{limit}|{what}", self.account.id())
    }

    /// The notification's words. `label` is the account's name; `shows_remaining`
    /// is the "Show what's left" setting.
    pub fn notification(&self, label: &str, shows_remaining: bool) -> Notification {
        let subtitle = match self.kind {
            AlertKind::Unreadable(_) => Some(Text::key("Can't be read")),
            AlertKind::LowBalance => Some(Text::key("Balance")),
            _ => self.window.as_ref().map(window_name),
        };
        Notification {
            identifier: self.identifier(),
            account: self.account.id(),
            title: Text::plain(label),
            subtitle,
            body: self.body(shows_remaining),
        }
    }

    fn body(&self, shows_remaining: bool) -> Text {
        match self.kind {
            AlertKind::Unreadable(Some(reason)) => Text::key(unavailability_info(reason).1),
            AlertKind::Unreadable(None) => {
                Text::key("The last few checks didn't get through, so the panel is showing older figures.")
            }
            AlertKind::Approaching { .. } => {
                let Some(window) = &self.window else { return Text::plain("") };
                let figure = Text::plain(format!("{}%", window.percent_value(shows_remaining)));
                let said = Text::key_with(if shows_remaining { "%@ left." } else { "%@ used." }, vec![figure]);
                Text::Sentences(vec![said, reset_sentence(window)])
            }
            // The figure and nothing else: credit does not come back on its own.
            AlertKind::LowBalance => {
                Text::key_with("%@ left.", vec![Text::plain(self.remaining.clone().unwrap_or_default())])
            }
            AlertKind::Spent => {
                let reset = self.window.as_ref().map_or(Text::plain(""), reset_sentence);
                Text::Sentences(vec![Text::key("This limit is spent."), reset])
            }
            AlertKind::Reset => Text::key("This limit has reset."),
        }
    }
}

/// When it comes back, when the provider says; nothing at all when it doesn't.
fn reset_sentence(window: &UsageWindow) -> Text {
    match window.resets_at {
        Some(at) => Text::key_with("Resets %@", vec![Text::ResetTime(at)]),
        None => Text::plain(""),
    }
}

/// Upstream `UsageWindow.name`, as keys.
pub fn window_name(w: &UsageWindow) -> Text {
    if let Some(label) = &w.label {
        return Text::plain(label.clone());
    }
    let base = match w.kind {
        WindowKind::FiveHour => Text::key("5-hour limit"),
        WindowKind::Weekly => Text::key("Weekly limit"),
        WindowKind::Spend => Text::key("Spend limit"),
        WindowKind::Balance => Text::key("Balance"),
        WindowKind::Daily => Text::key("Daily limit"),
        WindowKind::Messages => Text::key("Message allowance"),
        WindowKind::Monthly => Text::key("Monthly limit"),
        WindowKind::TopUp => Text::key("Top-up pack"),
        WindowKind::Credits => Text::key("Credit allowance"),
        WindowKind::SharedCredits => Text::key("Team credits"),
        WindowKind::Other(s) if s >= 86_400 && s % 86_400 == 0 => {
            Text::key_with("%@-day limit", vec![Text::plain((s / 86_400).to_string())])
        }
        WindowKind::Other(s) => {
            let hours = ((s as f64 / 3600.0).round() as i64).max(1);
            Text::key_with("%@-hour limit", vec![Text::plain(hours.to_string())])
        }
    };
    let mut parts = vec![base];
    if let Some(scope) = &w.scope {
        parts.push(Text::plain(scope.clone()));
    }
    if let Some(estimate) = w.estimate {
        parts.push(Text::key(match estimate {
            crate::model::Estimate::PlanPrice => "estimated",
            crate::model::Estimate::SinceTopUp => "since top-up",
            crate::model::Estimate::YourBudget => "of your budget",
        }));
    }
    if parts.len() == 1 {
        parts.remove(0)
    } else {
        Text::Dotted(parts)
    }
}

// ---------------------------------------------------------------------------
// Failures
// ---------------------------------------------------------------------------

/// What a reading with no figures in it does to a run of failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    /// Something that was working has stopped.
    Failure,
    /// The provider answered; whatever else is true, it can be reached.
    Answered,
    /// A setup step nobody has taken, or an app that is not open: not news, not evidence.
    Neutral,
}

/// Standing and the English message key (the same sentence the card shows) of a reason.
///
/// Exhaustive on purpose: a new reason must be classified by someone who has read
/// `Docs/notifications.md`, not defaulted.
pub fn unavailability_info(reason: Unavailability) -> (Standing, &'static str) {
    use Standing::{Answered, Failure, Neutral};
    use Unavailability as U;
    match reason {
        U::ClaudeLoginExpired => {
            (Failure, "Claude Code's saved login expired. Use Claude Code, or connect the status line.")
        }
        U::CursorLoginExpired => (Failure, "Cursor's saved login was refused. Open Cursor to renew it."),
        U::GrokLoginExpired => (Failure, "Grok's saved login expired. Use Grok to renew it."),
        U::SignedOut => (Failure, "Sign in to this account again in Settings."),
        U::ApiKeyRefused => (Failure, "That key was refused. Check it in Settings."),
        U::Unreachable => (Failure, "The service didn't respond."),
        U::UnreadableReply => (Failure, "Couldn't read the reply."),
        U::RateLimited => (Failure, "Checking too often — easing off."),
        U::ServerError => (Failure, "The service returned an error."),
        U::CodexServerFailed => (Failure, "Couldn't start the Codex helper."),
        U::SessionExpired => (
            Failure,
            "The browser session expired. Sign in on the website, then read it again in Settings.",
        ),
        U::LocalLoginExpired => {
            (Failure, "The saved login has expired. Sign in again with the service's own app or tool.")
        }

        // The provider replied: a complete answer ends an outage as surely as a figure.
        U::NoLimitsReported => (Answered, "No limits reported."),
        U::ZaiNoCodingPlan => (Answered, "That key works. The account has no Coding Plan running on it."),
        U::NoPlan => (Answered, "This account has no plan with usage limits."),

        U::Loading => (Neutral, "Loading…"),
        U::NotConnected => (Neutral, "Connect Claude Code in Settings to see usage."),
        U::AwaitingResponse => (Neutral, "Waiting for the next Claude Code response."),
        U::SignInRequired => (Neutral, "Sign in to Codex to see usage."),
        U::ClaudeSignInRequired => (Neutral, "Sign in to Claude Code to see usage."),
        U::CodexNotInstalled => (Neutral, "Codex isn't installed."),
        U::KiroNotInstalled => (Neutral, "Kiro CLI isn't installed."),
        U::KiroVersionUnsupported => (Neutral, "Update Kiro CLI to read subscription usage."),
        U::KiroSignInRequired => (Neutral, "Sign in to Kiro CLI to see usage."),
        U::AntigravityNotRunning => (Neutral, "Open Antigravity to see its usage."),
        U::AntigravityNotAnswering => {
            (Neutral, "Antigravity is open but didn't answer. Restarting it usually helps.")
        }
        U::CursorSignInRequired => (Neutral, "Sign in to Cursor to see usage."),
        U::GrokSignInRequired => (Neutral, "Sign in to Grok to see usage."),
        U::NotSignedIn => (Neutral, "Sign in from Settings to see usage."),
        U::ServerAddressMissing => (Neutral, "Add the server address in Settings."),
        U::ServerAddressRefused => (
            Neutral,
            "That address can't be used. It needs https://, unless the server is on your own network.",
        ),
        U::ApiKeyMissing => (Neutral, "Add an API key in Settings."),
        U::SessionMissing => (Neutral, "Read a browser session in Settings."),
        U::LocalLoginMissing => (Neutral, "Sign in with this service's own app or command-line tool first."),
        U::LocalAppMissing => (Neutral, "Nothing saved on this Mac yet. Open the service's app once, then retry."),
    }
}

/// Whether a stale reading may count against the account.
///
/// False for a push route (Claude Code's status line writes only while a session
/// runs) and for a reading out of an app's own saved state: an old figure there
/// says nobody used the tool, not that a check failed.
pub fn stale_means_failure(raw: &ProviderUsage) -> bool {
    !matches!(raw.origin, Some(UsageRoute::StatusLine | UsageRoute::LocalStore))
}

// ---------------------------------------------------------------------------
// Memory
// ---------------------------------------------------------------------------

/// What was last seen of one limit.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WindowMemory {
    /// Highest step already announced for the window that is currently open; 0 for nothing.
    pub announced: u8,
    /// The share the previous pass saw, so a turnover can be spotted.
    pub fraction: f64,
    pub resets_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AccountMemory {
    pub windows: BTreeMap<String, WindowMemory>,
    /// Consecutive passes that could not produce a current reading.
    pub failures: u32,
    /// Whether this run of failures has been reported. Cleared by the first reading that works.
    pub reported_failure: bool,
    /// The low-balance line already warned about (the figure, not a flag, so moving the line warns again).
    pub low_balance_warned_for: Option<f64>,
}

/// The settings one account is judged by.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rules {
    /// Percent, or none for "off".
    pub threshold: Option<u8>,
    pub announces_reset: bool,
    pub announces_failure: bool,
    /// The balance to warn below, or none for no warning on this account.
    pub low_balance: Option<f64>,
    pub stale_means_failure: bool,
}

impl Rules {
    /// Everything off.
    pub const OFF: Rules = Rules {
        threshold: None,
        announces_reset: false,
        announces_failure: false,
        low_balance: None,
        stale_means_failure: true,
    };

    /// The rules for one account from the settings. `stale_means_failure` comes from the reading.
    pub fn from_settings(settings: &AppSettings, account: &AccountKey, stale_means_failure: bool) -> Self {
        Self {
            threshold: settings.alert_threshold.filter(|p| (1..100).contains(p)),
            announces_reset: settings.alerts_on_reset,
            announces_failure: settings.alerts_on_failure,
            low_balance: settings.low_balance_alerts.get(&account.id()).copied().filter(|v| *v > 0.0),
            stale_means_failure,
        }
    }
}

/// Whether any rule about readings is on (upstream `wantsUsageAlerts`). Nothing is tracked otherwise.
pub fn wants_usage_alerts(settings: &AppSettings) -> bool {
    settings.alert_threshold.is_some_and(|p| (1..100).contains(&p))
        || settings.alerts_on_reset
        || settings.alerts_on_failure
        || settings.low_balance_alerts.values().any(|v| *v > 0.0)
}

/// What Pulse has already said, so that it does not say it again. Persisted:
/// without it every relaunch would re-announce everything already over the line.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AlertMemory {
    /// Keyed by account id.
    pub accounts: BTreeMap<String, AccountMemory>,
}

impl AlertMemory {
    /// What to say about one reading, if anything, and the record of having said it.
    ///
    /// `reading` is what the panel shows; `raw` is what the provider's service
    /// actually returned before a failure was swapped for the last good figures.
    /// Failures are classified on `raw`, limits are judged on live readings only.
    pub fn alerts(
        &mut self,
        reading: &ProviderUsage,
        raw: UsageState,
        account: &AccountKey,
        rules: &Rules,
        now: DateTime<Utc>,
    ) -> Vec<Alert> {
        let mut record = self.accounts.get(&account.id()).cloned().unwrap_or_default();
        let mut produced = Vec::new();

        // One more pass that produced nothing current.
        let count_failure = |record: &mut AccountMemory, produced: &mut Vec<Alert>, reason: Option<Unavailability>| {
            record.failures += 1;
            if !rules.announces_failure || record.failures < FAILURES_BEFORE_SAYING || record.reported_failure {
                return;
            }
            record.reported_failure = true;
            produced.push(Alert::new(account, AlertKind::Unreadable(reason), None));
        };
        // The provider answered. Whatever it said, it can be reached.
        let succeeded = |record: &mut AccountMemory| {
            record.failures = 0;
            record.reported_failure = false;
        };

        match raw {
            UsageState::Live => succeeded(&mut record),
            UsageState::Stale => {
                // A service reporting its own staleness: judged on the age of the figures,
                // and only where staleness can mean failure at all.
                if rules.stale_means_failure {
                    if let Some(observed) = reading.observed_at {
                        if (now - observed).num_seconds() > STALENESS_BEFORE_SAYING_SECONDS {
                            count_failure(&mut record, &mut produced, None);
                        }
                    }
                }
            }
            UsageState::Unavailable(reason) => match unavailability_info(reason).0 {
                Standing::Failure => {
                    // Cached figures stay protected for their first thirty minutes.
                    let protected = reading.state == UsageState::Stale
                        && reading
                            .observed_at
                            .is_some_and(|o| (now - o).num_seconds() <= STALENESS_BEFORE_SAYING_SECONDS);
                    if !protected {
                        count_failure(&mut record, &mut produced, Some(reason));
                    }
                }
                Standing::Answered => succeeded(&mut record),
                Standing::Neutral => {}
            },
        }

        let live = raw == UsageState::Live && reading.state == UsageState::Live;

        // Money, judged on live readings only: a stale reading carries whatever the
        // cache last banked, and the account may have been topped up since.
        if let (Some(line), true, Some(remaining), Some(formatted)) =
            (rules.low_balance, live, &reading.credit_remaining, &reading.credit_balance)
        {
            if remaining.amount < line {
                if record.low_balance_warned_for != Some(line) {
                    record.low_balance_warned_for = Some(line);
                    let mut alert = Alert::new(account, AlertKind::LowBalance, None);
                    alert.remaining = Some(formatted.clone());
                    produced.push(alert);
                }
            } else {
                // Back over the line: a top-up, or the line moved down.
                record.low_balance_warned_for = None;
            }
        }

        // Limits are judged on live readings only, dated and no older than a day.
        let fresh = reading.observed_at.is_some_and(|o| (now - o).num_seconds() <= MAXIMUM_READING_AGE_SECONDS);
        if !live || rules.threshold.is_none() || !fresh {
            self.accounts.insert(account.id(), record);
            return produced;
        }

        for window in &reading.windows {
            // A previously live snapshot can outlast its window between polls.
            if window.resets_at.is_some_and(|r| r <= now) {
                continue;
            }
            let seen = record.windows.get(&window.id).cloned();
            // A first sighting starts at nothing announced, so a limit already past the
            // line is said once, now. The copy is a status, not an event.
            let mut memory = seen.clone().unwrap_or_default();

            if let Some(seen) = &seen {
                if window.has_turned_over(seen.fraction, seen.resets_at) {
                    // Only for a limit that was worth mentioning on the way up.
                    if rules.announces_reset && seen.announced > 0 {
                        produced.push(Alert::new(account, AlertKind::Reset, Some(window)));
                    }
                    // The step is cleared by this evidence, not by the drop alone.
                    memory.announced = 0;
                }
            }

            if let Some(step) = step_for(window, rules.threshold) {
                if step > memory.announced {
                    memory.announced = step;
                    let kind = if step >= 100 { AlertKind::Spent } else { AlertKind::Approaching { percent: step } };
                    produced.push(Alert::new(account, kind, Some(window)));
                }
            }

            memory.fraction = window.used_fraction;
            memory.resets_at = window.resets_at;
            record.windows.insert(window.id.clone(), memory);
        }

        self.accounts.insert(account.id(), record);
        produced
    }
}

/// The highest step a window has reached: the chosen threshold, 100, or neither.
///
/// 100 is the provider's word: `is_exhausted`, or a share that reaches 100 rounding
/// down (99.6% stays 99). A balance or top-up pack never reaches 100 by arithmetic:
/// its denominator is Pulse's own observation or the reader's typed figure.
fn step_for(window: &UsageWindow, threshold: Option<u8>) -> Option<u8> {
    let floor = if window.used_fraction.is_finite() {
        (window.used_fraction * 100.0).floor().clamp(0.0, 1000.0) as i64
    } else {
        0
    };
    let reached = if window.is_exhausted {
        100
    } else if matches!(window.kind, WindowKind::Balance | WindowKind::TopUp) {
        floor.min(99)
    } else {
        floor
    };
    if reached >= 100 {
        return Some(100);
    }
    threshold.filter(|p| reached >= i64::from(*p))
}

// ---------------------------------------------------------------------------
// The engine
// ---------------------------------------------------------------------------

/// The memory plus the gate in front of it. Serialise it to survive restarts.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AlertEngine {
    pub memory: AlertMemory,
}

impl AlertEngine {
    pub fn from_json(bytes: &[u8]) -> Self {
        serde_json::from_slice(bytes).unwrap_or_default()
    }

    pub fn to_json(&self) -> Vec<u8> {
        serde_json::to_vec_pretty(self).unwrap_or_default()
    }

    /// One reading has landed. Returns the notifications to post, already gated by
    /// the settings: with no rule on, nothing is tracked and nothing returned.
    pub fn observe(
        &mut self,
        shown: &ProviderUsage,
        raw: &ProviderUsage,
        settings: &AppSettings,
        now: DateTime<Utc>,
    ) -> Vec<Notification> {
        if !wants_usage_alerts(settings) {
            return Vec::new();
        }
        let account = &shown.account;
        let rules = Rules::from_settings(settings, account, stale_means_failure(raw));
        let label = account_label(settings, account);
        self.memory
            .alerts(shown, raw.state, account, &rules, now)
            .iter()
            .map(|alert| alert.notification(&label, settings.shows_remaining))
            .collect()
    }
}

/// Upstream `AppSettings.label(for:)`: the name given to an added account, else the provider's.
pub fn account_label(settings: &AppSettings, account: &AccountKey) -> String {
    let id = account.id();
    settings
        .extra_accounts
        .iter()
        .find(|extra| extra.id == id)
        .map(|extra| extra.name.clone())
        .unwrap_or_else(|| account.provider.display_name().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CreditAmount, Estimate};
    use crate::provider::Provider;
    use chrono::{Duration, TimeZone};

    fn account() -> AccountKey {
        AccountKey::primary(Provider::ClaudeCode)
    }

    fn now() -> DateTime<Utc> {
        Utc.timestamp_opt(1_800_000_000, 0).unwrap()
    }

    fn window(id: &str, used: f64, resets_at: Option<DateTime<Utc>>, exhausted: bool) -> UsageWindow {
        UsageWindow::new(id, WindowKind::Weekly, used, 7 * 86_400).with_reset(resets_at).exhausted(exhausted)
    }

    fn weekly(used: f64) -> UsageWindow {
        window("weekly", used, None, false)
    }

    fn live(windows: Vec<UsageWindow>) -> ProviderUsage {
        ProviderUsage::live(account(), windows, now())
    }

    fn live_at(windows: Vec<UsageWindow>, at: DateTime<Utc>) -> ProviderUsage {
        let mut usage = live(windows);
        usage.observed_at = Some(at);
        usage
    }

    fn balance(amount: f64, state: UsageState) -> ProviderUsage {
        let mut usage = ProviderUsage::live(account(), vec![], now());
        usage.state = state;
        usage.credit_balance = Some(format!("¥{amount}"));
        usage.credit_remaining = Some(CreditAmount { amount, currency: "CNY".into() });
        usage
    }

    fn unavailable(reason: Unavailability) -> ProviderUsage {
        ProviderUsage::unavailable(account(), reason)
    }

    fn stale(observed_at: DateTime<Utc>) -> ProviderUsage {
        let mut usage = live_at(vec![weekly(0.5)], observed_at);
        usage.state = UsageState::Stale;
        usage
    }

    /// Everything on, threshold at 90, unless a test says otherwise.
    fn rules() -> Rules {
        Rules {
            threshold: Some(90),
            announces_reset: true,
            announces_failure: true,
            low_balance: None,
            stale_means_failure: true,
        }
    }

    fn run(memory: &mut AlertMemory, reading: &ProviderUsage) -> Vec<Alert> {
        run_with(memory, reading, None, &rules(), now())
    }

    fn run_with(
        memory: &mut AlertMemory,
        reading: &ProviderUsage,
        raw: Option<&ProviderUsage>,
        rules: &Rules,
        at: DateTime<Utc>,
    ) -> Vec<Alert> {
        memory.alerts(reading, raw.unwrap_or(reading).state, &account(), rules, at)
    }

    fn kinds(alerts: &[Alert]) -> Vec<AlertKind> {
        alerts.iter().map(|a| a.kind).collect()
    }

    // --- Low balance -------------------------------------------------------

    #[test]
    fn low_balance_is_said_once() {
        let mut memory = AlertMemory::default();
        let r = Rules { low_balance: Some(20.0), ..rules() };
        let first = run_with(&mut memory, &balance(9.40, UsageState::Live), None, &r, now());
        assert_eq!(kinds(&first), vec![AlertKind::LowBalance]);
        assert_eq!(first[0].remaining.as_deref(), Some("¥9.4"));
        assert!(run_with(&mut memory, &balance(8.0, UsageState::Live), None, &r, now()).is_empty());
        assert!(run_with(&mut memory, &balance(0.5, UsageState::Live), None, &r, now()).is_empty());
    }

    #[test]
    fn a_healthy_balance_is_silent() {
        let mut memory = AlertMemory::default();
        let r = Rules { low_balance: Some(20.0), ..rules() };
        assert!(run_with(&mut memory, &balance(50.0, UsageState::Live), None, &r, now()).is_empty());
    }

    #[test]
    fn topping_up_rearms_the_warning() {
        let mut memory = AlertMemory::default();
        let r = Rules { low_balance: Some(20.0), ..rules() };
        assert_eq!(run_with(&mut memory, &balance(9.40, UsageState::Live), None, &r, now()).len(), 1);
        assert!(run_with(&mut memory, &balance(100.0, UsageState::Live), None, &r, now()).is_empty());
        assert_eq!(run_with(&mut memory, &balance(5.0, UsageState::Live), None, &r, now()).len(), 1);
    }

    #[test]
    fn moving_the_line_warns_again() {
        let mut memory = AlertMemory::default();
        let at = |line| Rules { low_balance: Some(line), ..rules() };
        let reading = balance(9.40, UsageState::Live);
        assert_eq!(run_with(&mut memory, &reading, None, &at(10.0), now()).len(), 1);
        assert!(run_with(&mut memory, &reading, None, &at(10.0), now()).is_empty());
        assert_eq!(run_with(&mut memory, &reading, None, &at(50.0), now()).len(), 1);
    }

    #[test]
    fn no_line_set_is_no_warning() {
        let mut memory = AlertMemory::default();
        assert!(run(&mut memory, &balance(0.01, UsageState::Live)).is_empty());
    }

    #[test]
    fn a_stale_balance_is_not_warned_about() {
        let mut memory = AlertMemory::default();
        let r = Rules { low_balance: Some(20.0), ..rules() };
        assert!(run_with(&mut memory, &balance(9.40, UsageState::Stale), None, &r, now()).is_empty());
    }

    #[test]
    fn a_balance_with_no_figure_is_not_compared() {
        let mut memory = AlertMemory::default();
        let r = Rules { low_balance: Some(20.0), ..rules() };
        let mut unlimited = live(vec![]);
        unlimited.credit_balance = Some("Unlimited".into());
        assert!(run_with(&mut memory, &unlimited, None, &r, now()).is_empty());
    }

    // --- Prepaid credit is not a limit ------------------------------------

    fn balance_window(used: f64) -> UsageWindow {
        let mut w = UsageWindow::new("balance", WindowKind::Balance, used, 30 * 86_400);
        w.reports_length = false;
        w.estimate = Some(Estimate::SinceTopUp);
        w
    }

    #[test]
    fn a_balance_never_resets() {
        let mut memory = AlertMemory::default();
        assert_eq!(kinds(&run(&mut memory, &live(vec![balance_window(0.90)]))), vec![AlertKind::Approaching { percent: 90 }]);
        let after = run(&mut memory, &live(vec![balance_window(0.167)]));
        assert!(after.is_empty());
    }

    #[test]
    fn a_clamped_balance_is_not_spent() {
        let mut memory = AlertMemory::default();
        let alerts = run(&mut memory, &live(vec![balance_window(1.0)]));
        assert_eq!(kinds(&alerts), vec![AlertKind::Approaching { percent: 90 }]);
    }

    #[test]
    fn the_providers_own_verdict_on_a_balance_is_announced() {
        let mut memory = AlertMemory::default();
        let mut w = balance_window(1.0);
        w.is_exhausted = true;
        assert!(kinds(&run(&mut memory, &live(vec![w]))).contains(&AlertKind::Spent));
    }

    #[test]
    fn a_top_up_pack_is_not_a_limit_either() {
        let mut memory = AlertMemory::default();
        let pack = |used| UsageWindow::new("pack", WindowKind::TopUp, used, 0);
        assert_eq!(run(&mut memory, &live(vec![pack(1.0)])).len(), 1);
        // Buying a second pack drops the share forty points: nothing turned over.
        assert!(run(&mut memory, &live(vec![pack(0.3)])).is_empty());
    }

    #[test]
    fn credits_reset_only_when_the_date_moves() {
        let mut memory = AlertMemory::default();
        let at = now() + Duration::days(20);
        let credits = |used, resets| {
            UsageWindow::new("credits", WindowKind::Credits, used, 30 * 86_400).with_reset(Some(resets))
        };
        assert_eq!(run(&mut memory, &live(vec![credits(0.95, at)])).len(), 1);
        // A pack bought on top: the share falls, the date does not move.
        assert!(run(&mut memory, &live(vec![credits(0.2, at)])).is_empty());
        let renewed = run(&mut memory, &live(vec![credits(0.1, at + Duration::days(30))]));
        assert_eq!(kinds(&renewed), vec![AlertKind::Reset]);
    }

    // --- Thresholds --------------------------------------------------------

    #[test]
    fn a_limit_already_past_the_line_is_announced_once_immediately() {
        let mut memory = AlertMemory::default();
        let first = run(&mut memory, &live(vec![weekly(0.93)]));
        assert_eq!(kinds(&first), vec![AlertKind::Approaching { percent: 90 }]);
        assert!(run(&mut memory, &live(vec![weekly(0.94)])).is_empty());
    }

    #[test]
    fn a_limit_under_the_line_says_nothing() {
        let mut memory = AlertMemory::default();
        assert!(run(&mut memory, &live(vec![weekly(0.42)])).is_empty());
    }

    #[test]
    fn crossing_the_line_while_watching_announces() {
        let mut memory = AlertMemory::default();
        assert!(run(&mut memory, &live(vec![weekly(0.80)])).is_empty());
        assert_eq!(kinds(&run(&mut memory, &live(vec![weekly(0.91)]))), vec![AlertKind::Approaching { percent: 90 }]);
    }

    #[test]
    fn off_means_silent_however_full_the_limit_is() {
        let mut memory = AlertMemory::default();
        let r = Rules { threshold: None, ..rules() };
        let reading = live(vec![window("weekly", 1.0, None, true)]);
        assert!(run_with(&mut memory, &reading, None, &r, now()).is_empty());
    }

    #[test]
    fn an_already_reset_window_is_never_announced() {
        let mut memory = AlertMemory::default();
        let expired = window("weekly", 1.0, Some(now() - Duration::seconds(1)), true);
        assert!(run(&mut memory, &live(vec![expired.clone()])).is_empty());
        assert!(memory.accounts[&account().id()].windows.is_empty());

        let current = window("weekly-current", 0.93, Some(now() + Duration::hours(1)), false);
        let produced = run(&mut memory, &live(vec![expired, current]));
        assert_eq!(produced.len(), 1);
        assert_eq!(produced[0].window.as_ref().unwrap().id, "weekly-current");
    }

    #[test]
    fn a_snapshot_older_than_the_cache_lifetime_is_not_current() {
        let mut memory = AlertMemory::default();
        let old = live_at(vec![weekly(1.0)], now() - Duration::seconds(86_401));
        assert!(run(&mut memory, &old).is_empty());
        assert_eq!(run(&mut memory, &live(vec![weekly(0.93)])).len(), 1);
    }

    #[test]
    fn a_reading_with_no_date_is_not_judged() {
        let mut memory = AlertMemory::default();
        let mut undated = live(vec![weekly(0.99)]);
        undated.observed_at = None;
        assert!(run(&mut memory, &undated).is_empty());
    }

    #[test]
    fn lowering_the_threshold_announces_the_step_now_crossed() {
        let mut memory = AlertMemory::default();
        assert!(run(&mut memory, &live(vec![weekly(0.80)])).is_empty());
        let lowered = Rules { threshold: Some(75), ..rules() };
        let produced = run_with(&mut memory, &live(vec![weekly(0.80)]), None, &lowered, now());
        assert_eq!(kinds(&produced), vec![AlertKind::Approaching { percent: 75 }]);
    }

    // --- Spent is the provider's word -------------------------------------

    #[test]
    fn exhausted_is_spent_whatever_the_fraction_says() {
        let mut memory = AlertMemory::default();
        let alerts = run(&mut memory, &live(vec![window("weekly", 0.5, None, true)]));
        assert_eq!(kinds(&alerts), vec![AlertKind::Spent]);
    }

    #[test]
    fn ninety_nine_point_six_is_not_spent() {
        let mut memory = AlertMemory::default();
        let alerts = run(&mut memory, &live(vec![weekly(0.996)]));
        assert_eq!(kinds(&alerts), vec![AlertKind::Approaching { percent: 90 }]);
    }

    #[test]
    fn spent_follows_the_warning_rather_than_replacing_it() {
        let mut memory = AlertMemory::default();
        assert_eq!(kinds(&run(&mut memory, &live(vec![weekly(0.91)]))), vec![AlertKind::Approaching { percent: 90 }]);
        assert_eq!(kinds(&run(&mut memory, &live(vec![weekly(1.0)]))), vec![AlertKind::Spent]);
        assert!(run(&mut memory, &live(vec![weekly(1.0)])).is_empty());
    }

    // --- Resets -----------------------------------------------------------

    #[test]
    fn a_reset_time_that_moved_forward_is_a_new_window() {
        let mut memory = AlertMemory::default();
        let first = now() + Duration::hours(1);
        run(&mut memory, &live(vec![window("weekly", 0.95, Some(first), false)]));
        let next = window("weekly", 0.0, Some(first + Duration::days(7)), false);
        assert_eq!(kinds(&run(&mut memory, &live(vec![next]))), vec![AlertKind::Reset]);
    }

    #[test]
    fn a_rolling_window_sliding_down_is_not_a_reset() {
        let mut memory = AlertMemory::default();
        run(&mut memory, &live(vec![weekly(0.95)]));
        assert!(run(&mut memory, &live(vec![weekly(0.89)])).is_empty());
    }

    #[test]
    fn a_window_oscillating_across_the_line_is_announced_once() {
        let mut memory = AlertMemory::default();
        assert_eq!(run(&mut memory, &live(vec![weekly(0.95)])).len(), 1);
        for used in [0.89, 0.93, 0.88, 0.97] {
            assert!(run(&mut memory, &live(vec![weekly(used)])).is_empty(), "{used} announced again");
        }
    }

    #[test]
    fn a_real_turnover_rearms_the_step_after_an_oscillation() {
        let mut memory = AlertMemory::default();
        run(&mut memory, &live(vec![weekly(0.95)]));
        run(&mut memory, &live(vec![weekly(0.89)]));
        run(&mut memory, &live(vec![weekly(0.93)]));
        assert_eq!(kinds(&run(&mut memory, &live(vec![weekly(0.05)]))), vec![AlertKind::Reset]);
        assert_eq!(kinds(&run(&mut memory, &live(vec![weekly(0.94)]))), vec![AlertKind::Approaching { percent: 90 }]);
    }

    #[test]
    fn a_forty_point_drop_with_no_reset_time_is_a_reset() {
        let mut memory = AlertMemory::default();
        run(&mut memory, &live(vec![weekly(0.95)]));
        assert_eq!(kinds(&run(&mut memory, &live(vec![weekly(0.10)]))), vec![AlertKind::Reset]);
    }

    #[test]
    fn a_window_never_warned_about_resets_quietly() {
        let mut memory = AlertMemory::default();
        run(&mut memory, &live(vec![weekly(0.50)]));
        assert!(run(&mut memory, &live(vec![weekly(0.05)])).is_empty());
    }

    #[test]
    fn a_reset_clears_the_step_so_the_next_crossing_is_announced() {
        let mut memory = AlertMemory::default();
        run(&mut memory, &live(vec![weekly(0.95)]));
        run(&mut memory, &live(vec![weekly(0.05)]));
        assert_eq!(kinds(&run(&mut memory, &live(vec![weekly(0.92)]))), vec![AlertKind::Approaching { percent: 90 }]);
    }

    #[test]
    fn the_reset_switch_off_means_no_reset_alert_but_the_step_still_clears() {
        let mut memory = AlertMemory::default();
        let r = Rules { announces_reset: false, ..rules() };
        run_with(&mut memory, &live(vec![weekly(0.95)]), None, &r, now());
        assert!(run_with(&mut memory, &live(vec![weekly(0.10)]), None, &r, now()).is_empty());
        let again = run_with(&mut memory, &live(vec![weekly(0.92)]), None, &r, now());
        assert_eq!(kinds(&again), vec![AlertKind::Approaching { percent: 90 }]);
    }

    // --- Failures ---------------------------------------------------------

    #[test]
    fn three_failed_passes_in_a_row_then_one_alert_and_silence() {
        let mut memory = AlertMemory::default();
        let down = unavailable(Unavailability::Unreachable);
        assert!(run(&mut memory, &down).is_empty());
        assert!(run(&mut memory, &down).is_empty());
        let third = run(&mut memory, &down);
        assert_eq!(kinds(&third), vec![AlertKind::Unreadable(Some(Unavailability::Unreachable))]);
        assert!(run(&mut memory, &down).is_empty());
    }

    #[test]
    fn a_reading_that_works_clears_the_streak() {
        let mut memory = AlertMemory::default();
        let down = unavailable(Unavailability::Unreachable);
        for _ in 0..3 {
            run(&mut memory, &down);
        }
        run(&mut memory, &live(vec![weekly(0.1)]));
        assert!(run(&mut memory, &down).is_empty());
        assert!(run(&mut memory, &down).is_empty());
        assert_eq!(run(&mut memory, &down).len(), 1);
    }

    #[test]
    fn a_setup_step_nobody_has_taken_is_not_a_failure() {
        use Unavailability as U;
        for reason in [
            U::ApiKeyMissing,
            U::NotSignedIn,
            U::AntigravityNotRunning,
            U::NoLimitsReported,
            U::Loading,
            U::CodexNotInstalled,
            U::SessionMissing,
            U::ServerAddressMissing,
        ] {
            let mut memory = AlertMemory::default();
            for _ in 0..5 {
                assert!(run(&mut memory, &unavailable(reason)).is_empty(), "{reason:?} should never alert");
            }
        }
    }

    #[test]
    fn a_credential_that_went_bad_is_a_failure() {
        use Unavailability as U;
        for reason in [
            U::ClaudeLoginExpired,
            U::CursorLoginExpired,
            U::GrokLoginExpired,
            U::SignedOut,
            U::ApiKeyRefused,
            U::ServerError,
            U::SessionExpired,
            U::LocalLoginExpired,
        ] {
            let mut memory = AlertMemory::default();
            run(&mut memory, &unavailable(reason));
            run(&mut memory, &unavailable(reason));
            assert_eq!(run(&mut memory, &unavailable(reason)).len(), 1, "{reason:?} should alert");
        }
    }

    #[test]
    fn failures_are_counted_while_the_limit_threshold_is_off() {
        let mut memory = AlertMemory::default();
        let r = Rules { threshold: None, ..rules() };
        let down = unavailable(Unavailability::Unreachable);
        for _ in 0..2 {
            run_with(&mut memory, &down, None, &r, now());
        }
        assert_eq!(run_with(&mut memory, &down, None, &r, now()).len(), 1);
    }

    #[test]
    fn the_failure_switch_off_means_silence() {
        let mut memory = AlertMemory::default();
        let r = Rules { announces_failure: false, ..rules() };
        for _ in 0..5 {
            assert!(run_with(&mut memory, &unavailable(Unavailability::Unreachable), None, &r, now()).is_empty());
        }
    }

    #[test]
    fn fresh_figures_from_the_cache_are_not_a_failure() {
        let mut memory = AlertMemory::default();
        let recent = stale(now() - Duration::seconds(120));
        for _ in 0..5 {
            assert!(run(&mut memory, &recent).is_empty());
        }
    }

    #[test]
    fn a_push_route_going_quiet_is_never_a_failure() {
        let mut memory = AlertMemory::default();
        let r = Rules { stale_means_failure: false, ..rules() };
        let ancient = stale(now() - Duration::hours(6));
        for _ in 0..6 {
            assert!(run_with(&mut memory, &ancient, None, &r, now()).is_empty());
        }
    }

    #[test]
    fn only_the_status_line_and_saved_state_routes_are_spared() {
        let mut raw = stale(now());
        raw.origin = Some(UsageRoute::StatusLine);
        assert!(!stale_means_failure(&raw));
        raw.origin = Some(UsageRoute::LocalStore);
        assert!(!stale_means_failure(&raw));
        for origin in [Some(UsageRoute::Endpoint), Some(UsageRoute::ApiKey), Some(UsageRoute::BrowserSession), None] {
            raw.origin = origin;
            assert!(stale_means_failure(&raw), "{origin:?} should count");
        }
    }

    #[test]
    fn a_complete_answer_ends_a_run_of_failures() {
        use Unavailability as U;
        for reason in [U::ZaiNoCodingPlan, U::NoLimitsReported, U::NoPlan] {
            let mut memory = AlertMemory::default();
            let down = unavailable(U::Unreachable);
            for _ in 0..3 {
                run(&mut memory, &down);
            }
            run(&mut memory, &unavailable(reason));
            let mut produced = Vec::new();
            for _ in 0..3 {
                produced.extend(run(&mut memory, &down));
            }
            assert_eq!(produced.len(), 1, "{reason:?} left the reported mark stuck");
        }
    }

    #[test]
    fn an_answer_breaks_a_run_that_was_not_yet_reported() {
        let mut memory = AlertMemory::default();
        let down = unavailable(Unavailability::Unreachable);
        run(&mut memory, &down);
        run(&mut memory, &down);
        assert!(run(&mut memory, &unavailable(Unavailability::NoLimitsReported)).is_empty());
        assert!(run(&mut memory, &down).is_empty());
        assert!(run(&mut memory, &down).is_empty());
        assert_eq!(run(&mut memory, &down).len(), 1);
    }

    #[test]
    fn antigravity_open_but_refusing_is_not_a_failure() {
        for reason in [Unavailability::AntigravityNotAnswering, Unavailability::AntigravityNotRunning] {
            let mut memory = AlertMemory::default();
            for _ in 0..5 {
                assert!(run(&mut memory, &unavailable(reason)).is_empty(), "{reason:?} alerted");
            }
        }
    }

    #[test]
    fn figures_older_than_half_an_hour_are_a_failure() {
        let mut memory = AlertMemory::default();
        let old = stale(now() - Duration::hours(1));
        run(&mut memory, &old);
        run(&mut memory, &old);
        assert_eq!(kinds(&run(&mut memory, &old)), vec![AlertKind::Unreadable(None)]);
    }

    #[test]
    fn a_stale_reading_never_drives_the_limit_rules() {
        let mut memory = AlertMemory::default();
        run(&mut memory, &live(vec![weekly(0.95)]));
        let mut cached = live_at(vec![weekly(0.10)], now() - Duration::seconds(60));
        cached.state = UsageState::Stale;
        assert!(run(&mut memory, &cached).is_empty());
    }

    #[test]
    fn each_limit_is_judged_on_its_own() {
        let mut memory = AlertMemory::default();
        let alerts = run(&mut memory, &live(vec![weekly(0.95), window("5h", 0.20, None, false)]));
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].window.as_ref().unwrap().id, "weekly");
    }

    // --- Through the store's cache (the failure replaced by the last good figures)

    #[test]
    fn quitting_antigravity_is_not_an_outage_however_old_the_cached_figures() {
        let mut memory = AlertMemory::default();
        let closed = unavailable(Unavailability::AntigravityNotRunning);
        // The panel keeps showing two-hour-old figures marked stale.
        let shown = stale(now() - Duration::hours(2));
        for _ in 0..6 {
            assert!(run_with(&mut memory, &shown, Some(&closed), &rules(), now()).is_empty());
        }
    }

    #[test]
    fn a_real_outage_behind_the_same_cache_still_alerts() {
        let mut memory = AlertMemory::default();
        let down = unavailable(Unavailability::Unreachable);
        let shown = stale(now() - Duration::hours(2));
        let mut produced = Vec::new();
        for _ in 0..3 {
            produced.extend(run_with(&mut memory, &shown, Some(&down), &rules(), now()));
        }
        assert_eq!(kinds(&produced), vec![AlertKind::Unreadable(Some(Unavailability::Unreachable))]);
    }

    #[test]
    fn real_failures_behind_fresh_cached_figures_keep_the_thirty_minute_grace() {
        let mut memory = AlertMemory::default();
        let down = unavailable(Unavailability::Unreachable);
        let shown = stale(now());
        let r = Rules { threshold: None, announces_reset: false, ..rules() };
        for seconds in [120, 240, 360, 1800] {
            let at = now() + Duration::seconds(seconds);
            assert!(run_with(&mut memory, &shown, Some(&down), &r, at).is_empty());
        }
        assert_eq!(memory.accounts[&account().id()].failures, 0);
        let mut produced = Vec::new();
        for seconds in [1801, 1921, 2041] {
            let at = now() + Duration::seconds(seconds);
            produced.extend(run_with(&mut memory, &shown, Some(&down), &r, at));
        }
        assert_eq!(kinds(&produced), vec![AlertKind::Unreadable(Some(Unavailability::Unreachable))]);
    }

    #[test]
    fn cached_figures_never_drive_the_limit_rules_whatever_the_panel_shows() {
        let mut memory = AlertMemory::default();
        assert_eq!(run(&mut memory, &live(vec![weekly(0.95)])).len(), 1);
        // The fetch failed; the panel shows the banked 95% again, marked stale.
        let down = unavailable(Unavailability::Unreachable);
        let mut shown = live(vec![weekly(0.95)]);
        shown.state = UsageState::Stale;
        assert!(run_with(&mut memory, &shown, Some(&down), &rules(), now()).is_empty());
    }

    // --- Engine: gating, memory, words ------------------------------------

    fn all_on() -> AppSettings {
        AppSettings {
            alert_threshold: Some(90),
            alerts_on_reset: true,
            alerts_on_failure: true,
            ..AppSettings::default()
        }
    }

    #[test]
    fn with_every_setting_off_nothing_is_posted_or_remembered() {
        let mut engine = AlertEngine::default();
        let settings = AppSettings::default();
        assert!(!wants_usage_alerts(&settings));
        let reading = live(vec![window("weekly", 1.0, None, true)]);
        assert!(engine.observe(&reading, &reading, &settings, now()).is_empty());
        for _ in 0..5 {
            let down = unavailable(Unavailability::Unreachable);
            assert!(engine.observe(&down, &down, &settings, now()).is_empty());
        }
        assert!(engine.memory.accounts.is_empty());
    }

    #[test]
    fn each_kind_is_gated_by_its_own_setting() {
        let down = unavailable(Unavailability::Unreachable);
        let high = live(vec![weekly(0.95)]);

        // Only failures on: a full limit is silent, an outage is not.
        let mut engine = AlertEngine::default();
        let only_failures = AppSettings { alerts_on_failure: true, ..AppSettings::default() };
        assert!(engine.observe(&high, &high, &only_failures, now()).is_empty());
        let mut posted = 0;
        for _ in 0..3 {
            posted += engine.observe(&down, &down, &only_failures, now()).len();
        }
        assert_eq!(posted, 1);

        // Only the threshold on: an outage is silent however long.
        let mut engine = AlertEngine::default();
        let only_threshold = AppSettings { alert_threshold: Some(90), ..AppSettings::default() };
        for _ in 0..6 {
            assert!(engine.observe(&down, &down, &only_threshold, now()).is_empty());
        }
        assert_eq!(engine.observe(&high, &high, &only_threshold, now()).len(), 1);

        // Only reset on: nothing is ever announced on the way up, so no reset either.
        let mut engine = AlertEngine::default();
        let only_reset = AppSettings { alerts_on_reset: true, ..AppSettings::default() };
        assert!(engine.observe(&high, &high, &only_reset, now()).is_empty());
        let low = live(vec![weekly(0.05)]);
        assert!(engine.observe(&low, &low, &only_reset, now()).is_empty());
    }

    #[test]
    fn low_balance_is_per_account() {
        let mut settings = AppSettings::default();
        settings.low_balance_alerts.insert(account().id(), 20.0);
        assert!(wants_usage_alerts(&settings));
        let mut engine = AlertEngine::default();
        let low = balance(9.4, UsageState::Live);
        let posted = engine.observe(&low, &low, &settings, now());
        assert_eq!(posted.len(), 1);
        assert_eq!(posted[0].subtitle, Some(Text::key("Balance")));

        let mut other = low.clone();
        other.account = AccountKey::primary(Provider::Codex);
        assert!(engine.observe(&other, &other, &settings, now()).is_empty());

        settings.low_balance_alerts.insert(account().id(), 0.0);
        assert!(!wants_usage_alerts(&settings));
    }

    #[test]
    fn memory_survives_a_restart() {
        let settings = all_on();
        let mut engine = AlertEngine::default();
        let reading = live(vec![weekly(0.95)]);
        assert_eq!(engine.observe(&reading, &reading, &settings, now()).len(), 1);

        let mut restarted = AlertEngine::from_json(&engine.to_json());
        assert_eq!(restarted, engine);
        // Already said: a relaunch must not say it again.
        assert!(restarted.observe(&reading, &reading, &settings, now()).is_empty());
        // And a damaged file starts from nothing rather than failing.
        assert_eq!(AlertEngine::from_json(b"not json"), AlertEngine::default());
    }

    #[test]
    fn an_old_memory_file_with_missing_fields_still_loads() {
        let engine = AlertEngine::from_json(br#"{"memory":{"accounts":{"claudeCode":{"windows":{"w":{"announced":90}}}}}}"#);
        assert_eq!(engine.memory.accounts["claudeCode"].windows["w"].announced, 90);
        assert_eq!(engine.memory.accounts["claudeCode"].failures, 0);
    }

    #[test]
    fn the_notification_says_only_what_was_witnessed() {
        let settings = all_on();
        let mut engine = AlertEngine::default();
        let reset = now() + Duration::hours(3);
        let reading = live(vec![window("weekly", 0.925, Some(reset), false)]);
        let posted = engine.observe(&reading, &reading, &settings, now());
        assert_eq!(posted.len(), 1);
        let note = &posted[0];
        assert_eq!(note.title, Text::plain("Claude Code"));
        assert_eq!(note.subtitle, Some(Text::key("Weekly limit")));
        assert_eq!(
            note.body,
            Text::Sentences(vec![
                Text::key_with("%@ used.", vec![Text::plain("93%")]),
                Text::key_with("Resets %@", vec![Text::ResetTime(reset)]),
            ])
        );
        assert_eq!(note.identifier, "claudeCode|weekly|approaching-90");
    }

    #[test]
    fn no_reset_time_means_no_reset_sentence_and_remaining_flips_the_figure() {
        let settings = AppSettings { shows_remaining: true, ..all_on() };
        let mut engine = AlertEngine::default();
        let reading = live(vec![weekly(0.95)]);
        let note = &engine.observe(&reading, &reading, &settings, now())[0];
        assert_eq!(
            note.body,
            Text::Sentences(vec![Text::key_with("%@ left.", vec![Text::plain("5%")]), Text::plain("")])
        );
    }

    #[test]
    fn unreadable_uses_the_cards_own_sentence_or_the_generic_one() {
        let alert = Alert::new(&account(), AlertKind::Unreadable(Some(Unavailability::ApiKeyRefused)), None);
        let note = alert.notification("X", false);
        assert_eq!(note.subtitle, Some(Text::key("Can't be read")));
        assert_eq!(note.body, Text::key("That key was refused. Check it in Settings."));
        let generic = Alert::new(&account(), AlertKind::Unreadable(None), None).notification("X", false);
        assert_eq!(
            generic.body,
            Text::key("The last few checks didn't get through, so the panel is showing older figures.")
        );
    }

    #[test]
    fn added_accounts_are_named_by_the_reader() {
        let mut settings = AppSettings::default();
        let key = AccountKey { provider: Provider::ClaudeCode, slot: "work".into() };
        settings.extra_accounts.push(crate::settings::ExtraAccount {
            id: key.id(),
            provider: "claudeCode".into(),
            name: "Work".into(),
        });
        assert_eq!(account_label(&settings, &key), "Work");
        assert_eq!(account_label(&settings, &account()), "Claude Code");
    }

    #[test]
    fn window_names_match_the_card() {
        let mut w = UsageWindow::new("a", WindowKind::Other(3 * 86_400), 0.1, 3 * 86_400).with_scope("Opus");
        assert_eq!(
            window_name(&w),
            Text::Dotted(vec![Text::key_with("%@-day limit", vec![Text::plain("3")]), Text::plain("Opus")])
        );
        w.kind = WindowKind::Other(5400);
        w.scope = None;
        assert_eq!(window_name(&w), Text::key_with("%@-hour limit", vec![Text::plain("2")]));
    }
}
