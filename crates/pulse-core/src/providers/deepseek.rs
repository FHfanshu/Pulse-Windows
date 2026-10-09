// Ported from upstream Providers/DeepSeekUsageService.swift, DeepSeekBalanceBasis.swift and Usage/BalanceRing.swift.
//! DeepSeek: a prepaid balance from `GET https://api.deepseek.com/user/balance`.
//!
//! The reply is money and nothing else (no allowance, window or reset), so a
//! ring needs a denominator that DeepSeek did not give. The reader picks where
//! it comes from (`BalanceBasis`): the peak balance Pulse watched since the last
//! top-up (default), none at all, or a budget they typed. Every ring drawn this
//! way carries an `Estimate` saying so.
//!
//! The web-console route (`deepseek_console`: sign-in token, wallets, spend history) answers the
//! balance when there is no key or the key route could not. The reader's preferred currency
//! (`deepSeekCurrency` upstream) has no settings field here, so the ring follows
//! the first currency with money in it.

use std::collections::BTreeMap;
use std::sync::Mutex;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::profile;
use crate::model::{AccountKey, CreditAmount, Estimate, ProviderUsage, Unavailability, UsageRoute, UsageState, UsageWindow, WindowKind};
use crate::provider::Provider;
use crate::service::{FetchContext, UsageService};

const ENDPOINT: &str = "https://api.deepseek.com/user/balance";

/// Where a balance ring's denominator comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BalanceBasis {
    SinceTopUp,
    BalanceOnly,
    Budget,
}

impl BalanceBasis {
    /// Unknown or missing values take the default, like upstream's raw-value init.
    pub fn from_setting(raw: Option<&str>) -> Self {
        match raw {
            Some("balanceOnly") => BalanceBasis::BalanceOnly,
            Some("budget") => BalanceBasis::Budget,
            _ => BalanceBasis::SinceTopUp,
        }
    }
}

/// The highest balance seen since it last rose, per currency.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Mark {
    pub peak: f64,
    /// When the peak was first seen (the top-up), not when it was last read.
    pub set_at: DateTime<Utc>,
}

/// The mark after seeing `balance`: a first sight or a rise resets it, else unchanged.
pub fn advanced(mark: Option<Mark>, balance: f64, now: DateTime<Utc>) -> Mark {
    match mark {
        Some(m) if balance <= m.peak => m,
        _ => Mark { peak: balance.max(0.0), set_at: now },
    }
}

/// Fraction of the peak that is gone; None where the peak gives no denominator.
pub fn used_fraction(balance: f64, peak: f64) -> Option<f64> {
    (peak > 0.0).then(|| ((peak - balance) / peak).clamp(0.0, 1.0))
}

/// The one window a balance can have, or None where the basis gives no denominator.
/// No length and no reset: prepaid money does not turn over.
pub fn balance_window(balance: f64, basis: BalanceBasis, budget: Option<f64>, peak: f64, exhausted: bool) -> Option<UsageWindow> {
    let (fraction, estimate) = match basis {
        BalanceBasis::BalanceOnly => return None,
        BalanceBasis::SinceTopUp => (used_fraction(balance, peak)?, Estimate::SinceTopUp),
        BalanceBasis::Budget => {
            // Finite, not just positive: an infinite budget would make the fraction NaN.
            let budget = budget.filter(|b| b.is_finite() && *b > 0.0)?;
            (((budget - balance) / budget).clamp(0.0, 1.0), Estimate::YourBudget)
        }
    };
    let mut window = UsageWindow::new("balance", WindowKind::Balance, fraction, 30 * 86_400).exhausted(exhausted);
    window.reports_length = false;
    window.estimate = Some(estimate);
    Some(window)
}

#[derive(Default)]
pub struct DeepSeek {
    /// Watched marks by currency, loaded from disk on first use.
    marks: Mutex<Option<BTreeMap<String, Mark>>>,
}

fn marks_file() -> std::path::PathBuf {
    crate::paths::data_dir().join("deepseek-baseline.json")
}

impl DeepSeek {
    /// Advance the mark for `currency` and persist it when it moved.
    fn advance(&self, currency: &str, balance: f64, now: DateTime<Utc>) -> Mark {
        let mut guard = self.marks.lock().unwrap();
        let marks = guard.get_or_insert_with(|| {
            std::fs::read(marks_file()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
        });
        let mark = advanced(marks.get(currency).copied(), balance, now);
        if marks.get(currency) != Some(&mark) {
            marks.insert(currency.to_string(), mark);
            if let Ok(bytes) = serde_json::to_vec(&*marks) {
                let _ = std::fs::create_dir_all(crate::paths::data_dir());
                let _ = std::fs::write(marks_file(), bytes);
            }
        }
        mark
    }
}

#[async_trait]
impl UsageService for DeepSeek {
    fn provider(&self) -> Provider {
        Provider::DeepSeek
    }

    async fn fetch(&self, ctx: &FetchContext, account: &AccountKey) -> ProviderUsage {
        let Some(key) = ctx.api_key(account) else {
            // No key: the console's sign-in is the only credential, if it is kept.
            return match self.from_console(ctx, account).await {
                ConsoleBalance::Read(usage) => usage,
                // The sign-in has lapsed: say so, rather than asking for a key nobody needs.
                ConsoleBalance::SignedOut => ProviderUsage::unavailable(account.clone(), Unavailability::SessionExpired),
                ConsoleBalance::None => ProviderUsage::unavailable(account.clone(), Unavailability::ApiKeyMissing),
            };
        };
        let keyed = match profile::get_bearer(ctx, ENDPOINT, key.trim()).await {
            Ok(body) => match profile::decode::<Reply>(&body) {
                Ok(reply) => self.reading(ctx, account, &reply),
                Err(reason) => ProviderUsage::unavailable(account.clone(), reason),
            },
            Err(reason) => ProviderUsage::unavailable(account.clone(), reason),
        };
        // The console stands in only for a key route that did not answer.
        if let UsageState::Unavailable(reason) = keyed.state {
            if matches!(
                reason,
                Unavailability::ApiKeyRefused | Unavailability::Unreachable | Unavailability::ServerError | Unavailability::UnreadableReply
            ) {
                if let ConsoleBalance::Read(console) = self.from_console(ctx, account).await {
                    return console;
                }
            }
        }
        keyed
    }
}

enum ConsoleBalance {
    Read(ProviderUsage),
    SignedOut,
    /// No console token kept, or the console did not answer: the key route's own reason is the one
    /// worth showing then.
    None,
}

impl DeepSeek {
    /// The balance out of the console's wallets (`deepseek_console`), when its sign-in is kept.
    async fn from_console(&self, ctx: &FetchContext, account: &AccountKey) -> ConsoleBalance {
        let Some(token) = ctx.secrets.get(&super::deepseek_console::secret_id()).filter(|t| !t.trim().is_empty()) else {
            return ConsoleBalance::None;
        };
        match super::deepseek_console::balance(&ctx.http, token.trim()).await {
            Ok(reply) => {
                let usage = self.reading(ctx, account, &reply);
                if usage.state != UsageState::Live {
                    return ConsoleBalance::None;
                }
                ConsoleBalance::Read(usage.with_origin(UsageRoute::BrowserSession))
            }
            Err(super::deepseek_console::Failure::SignedOut) => ConsoleBalance::SignedOut,
            Err(super::deepseek_console::Failure::Failed) => ConsoleBalance::None,
        }
    }

    /// A reading from either route's reply, drawn by exactly the same rule.
    fn reading(&self, ctx: &FetchContext, account: &AccountKey, reply: &Reply) -> ProviderUsage {
        let Some(purse) = purse_from(reply, None) else {
            return ProviderUsage::unavailable(account.clone(), Unavailability::NoLimitsReported);
        };

        // Advanced on every reading whatever the basis, so switching later finds a peak.
        let mark = self.advance(&purse.currency, purse.total, ctx.now);
        let id = account.id();
        let basis = BalanceBasis::from_setting(ctx.settings.balance_bases.get(&id).map(String::as_str));
        let budget = ctx.settings.balance_budgets.get(&id).copied();
        let windows = balance_window(purse.total, basis, budget, mark.peak, reply.is_available == Some(false)).into_iter().collect();

        let mut usage = profile::reading(account, windows, ctx);
        usage.credit_balance = Some(format!("{} {:.2}", purse.currency, purse.total));
        usage.credit_remaining = Some(CreditAmount { amount: purse.total, currency: purse.currency });
        usage
    }
}

/// The key route's reply, and the shape the console's wallets are turned into.
#[derive(Deserialize)]
pub struct Reply {
    pub is_available: Option<bool>,
    pub balance_infos: Option<Vec<Info>>,
}

#[derive(Deserialize)]
pub struct Info {
    pub currency: Option<String>,
    pub total_balance: Option<String>,
    pub granted_balance: Option<String>,
    pub topped_up_balance: Option<String>,
}

/// One currency's money, strings turned into numbers.
#[derive(Debug, Clone, PartialEq)]
struct Purse {
    currency: String,
    total: f64,
    #[allow(dead_code)]
    granted: Option<f64>,
    #[allow(dead_code)]
    topped_up: Option<f64>,
}

/// Money arrives as a string. Absent or unparseable is absent, never zero.
fn money(text: Option<&str>) -> Option<f64> {
    text?.trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

fn purse_of(info: &Info) -> Option<Purse> {
    let currency = info.currency.clone().filter(|c| !c.is_empty())?;
    Some(Purse {
        currency,
        total: money(info.total_balance.as_deref())?,
        granted: money(info.granted_balance.as_deref()),
        topped_up: money(info.topped_up_balance.as_deref()),
    })
}

/// The currency the ring follows: the reader's choice if held, else the first
/// with money in it, else the first. Currencies are never compared by size.
fn purse_from(reply: &Reply, preferring: Option<&str>) -> Option<Purse> {
    let purses: Vec<Purse> = reply.balance_infos.iter().flatten().filter_map(purse_of).collect();
    if let Some(chosen) = preferring.and_then(|c| purses.iter().find(|p| p.currency == c)) {
        return Some(chosen.clone());
    }
    purses.iter().find(|p| p.total > 0.0).or_else(|| purses.first()).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::profile::test_support::fixture;

    fn reply(name: &str) -> Reply {
        serde_json::from_slice(&fixture(name)).unwrap()
    }

    fn purse(name: &str, currency: Option<&str>) -> Purse {
        purse_from(&reply(name), currency).unwrap()
    }

    fn windows(p: &Purse, basis: BalanceBasis, budget: Option<f64>, peak: f64, available: bool) -> Vec<UsageWindow> {
        balance_window(p.total, basis, budget, peak, !available).into_iter().collect()
    }

    #[test]
    fn money_is_parsed_from_strings() {
        let p = purse("deepseek-balance.json", None);
        assert_eq!(p.currency, "CNY");
        assert_eq!(p.total, 42.30);
        assert_eq!(p.granted, Some(10.0));
        assert_eq!(p.topped_up, Some(32.30));
    }

    #[test]
    fn absent_money_is_not_zero() {
        assert_eq!(money(None), None);
        assert_eq!(money(Some("")), None);
        assert_eq!(money(Some("  ")), None);
        assert_eq!(money(Some("not a number")), None);
        assert_eq!(money(Some("0.00")), Some(0.0));
    }

    #[test]
    fn an_entry_without_a_total_is_dropped() {
        let r: Reply = serde_json::from_str(r#"{"is_available":true,"balance_infos":[{"currency":"CNY"}]}"#).unwrap();
        assert_eq!(purse_from(&r, None), None);
    }

    #[test]
    fn currency_selection() {
        assert_eq!(purse("deepseek-two-currencies.json", None).currency, "CNY");
        assert_eq!(purse("deepseek-two-currencies.json", Some("USD")).currency, "USD");
        // A choice the account does not hold is not honoured over one it does.
        assert_eq!(purse("deepseek-two-currencies.json", Some("EUR")).currency, "CNY");
    }

    #[test]
    fn balance_only_draws_no_window() {
        let p = purse("deepseek-balance.json", None);
        assert!(windows(&p, BalanceBasis::BalanceOnly, None, 0.0, true).is_empty());
    }

    #[test]
    fn since_top_up_measures_against_the_observed_peak() {
        let p = purse("deepseek-balance.json", None);
        let w = windows(&p, BalanceBasis::SinceTopUp, None, 100.0, true).remove(0);
        assert!((w.used_fraction - (100.0 - 42.30) / 100.0).abs() < 1e-6);
        assert_eq!(w.percent_value(false), 58);
        assert!(w.is_estimated());
        assert_eq!(w.estimate, Some(Estimate::SinceTopUp));
        assert_eq!(w.scope, None);
        assert!(!w.reports_length);
        assert_eq!(w.resets_at, None);
        assert_eq!(w.elapsed_fraction(Utc::now()), None);
    }

    #[test]
    fn a_peak_of_zero_draws_nothing() {
        let p = purse("deepseek-balance.json", None);
        assert!(windows(&p, BalanceBasis::SinceTopUp, None, 0.0, true).is_empty());
    }

    #[test]
    fn a_budget_must_be_a_figure() {
        let p = purse("deepseek-balance.json", None);
        for budget in [None, Some(0.0), Some(-5.0)] {
            assert!(windows(&p, BalanceBasis::Budget, budget, 0.0, true).is_empty(), "budget {budget:?}");
        }
    }

    #[test]
    fn budget_measures_against_the_readers_figure() {
        let p = purse("deepseek-balance.json", None);
        let w = windows(&p, BalanceBasis::Budget, Some(50.0), 0.0, true).remove(0);
        assert!((w.used_fraction - (50.0 - 42.30) / 50.0).abs() < 1e-6);
        assert_eq!(w.estimate, Some(Estimate::YourBudget));
        assert_eq!(w.scope, None);
        assert!(w.is_estimated());
    }

    #[test]
    fn every_window_is_flagged_estimated() {
        let p = purse("deepseek-balance.json", None);
        let mut all = windows(&p, BalanceBasis::SinceTopUp, None, 100.0, true);
        all.extend(windows(&p, BalanceBasis::Budget, Some(50.0), 0.0, true));
        assert_eq!(all.len(), 2);
        assert!(all.iter().all(UsageWindow::is_estimated));
    }

    #[test]
    fn spent_is_the_providers_flag_not_the_arithmetic() {
        let spent = purse("deepseek-spent.json", None);
        let w = windows(&spent, BalanceBasis::SinceTopUp, None, 100.0, false).remove(0);
        assert_eq!(w.used_fraction, 1.0);
        assert!(w.is_exhausted);

        let p = purse("deepseek-balance.json", None);
        let ambitious = windows(&p, BalanceBasis::Budget, Some(1_000.0), 0.0, true).remove(0);
        assert!(ambitious.used_fraction > 0.95);
        assert!(!ambitious.is_exhausted);

        let overfull = windows(&p, BalanceBasis::Budget, Some(20.0), 0.0, true).remove(0);
        assert_eq!(overfull.used_fraction, 0.0);
    }

    #[test]
    fn a_non_finite_budget_draws_nothing() {
        let p = purse("deepseek-balance.json", None);
        for budget in [f64::INFINITY, f64::NEG_INFINITY, f64::NAN] {
            assert!(windows(&p, BalanceBasis::Budget, Some(budget), 0.0, true).is_empty(), "budget {budget}");
        }
        let w = UsageWindow::new("x", WindowKind::Balance, f64::NAN, 0);
        assert_eq!(w.percent_value(false), 0);
    }

    #[test]
    fn the_mark_follows_top_ups() {
        let t = |s| DateTime::from_timestamp(s, 0).unwrap();
        let first = advanced(None, 100.0, t(1_000));
        assert_eq!((first.peak, first.set_at), (100.0, t(1_000)));
        // Spending leaves the mark, and its date, alone.
        let spending = advanced(Some(first), 42.30, t(2_000));
        assert_eq!(spending, first);
        // Only a rise is a top-up.
        let topped = advanced(Some(spending), 150.0, t(2_000));
        assert_eq!((topped.peak, topped.set_at), (150.0, t(2_000)));
    }

    #[test]
    fn a_negative_balance_is_not_a_negative_denominator() {
        let mark = advanced(None, -5.0, Utc::now());
        assert_eq!(mark.peak, 0.0);
        assert_eq!(used_fraction(-5.0, 0.0), None);
    }

    #[test]
    fn basis_setting_defaults_to_since_top_up() {
        assert_eq!(BalanceBasis::from_setting(None), BalanceBasis::SinceTopUp);
        assert_eq!(BalanceBasis::from_setting(Some("budget")), BalanceBasis::Budget);
        assert_eq!(BalanceBasis::from_setting(Some("balanceOnly")), BalanceBasis::BalanceOnly);
    }
}
