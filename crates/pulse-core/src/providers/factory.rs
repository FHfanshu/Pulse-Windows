// Ported from upstream Providers/Profiled/FactoryUsageService.swift.
//! Factory (the Droid agent): a pasted `fk-` API key read in three steps. `auth/me`
//! says whether the key is good and names the plan. `billing/limits` covers the
//! token-rate-limits billing, when the account is on it. Any other account, or a
//! failed limits call, falls through to the older Standard/Premium usage.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;

use super::profile::{self, Fail};
use crate::model::{AccountKey, CreditAmount, ProviderUsage, Unavailability, UsageWindow, WindowKind};
use crate::provider::Provider;
use crate::service::{FetchContext, UsageService};

const BASE: &str = "https://api.factory.ai";
/// Above this an allowance is Factory's way of saying "unlimited".
const UNLIMITED: f64 = 1e12;

#[derive(Default)]
pub struct Factory;

#[async_trait]
impl UsageService for Factory {
    fn provider(&self) -> Provider {
        Provider::Factory
    }

    async fn fetch(&self, ctx: &FetchContext, account: &AccountKey) -> ProviderUsage {
        let Some(key) = ctx.api_key(account) else {
            return ProviderUsage::unavailable(account.clone(), Unavailability::ApiKeyMissing);
        };
        let key = key.trim().to_string();

        let me = match get(ctx, "api/app/auth/me", &key, &[]).await {
            Ok(body) => body,
            Err(reason) => return ProviderUsage::unavailable(account.clone(), reason),
        };
        let (plan, user_id) = account_from(&me);

        // The newer billing, when the account is on it. Any other answer, a failure included,
        // means the older route should be asked, as Factory's own app does.
        if let Ok(body) = get(ctx, "api/billing/limits", &key, &[]).await {
            if let Some(usage) = limits_reading(&body, plan.clone(), ctx, account) {
                return usage;
            }
        }

        let mut query = vec![("useCache", "true".to_string())];
        if let Some(id) = user_id {
            query.push(("userId", id));
        }
        match get(ctx, "api/organization/subscription/usage", &key, &query).await {
            Ok(body) => allowance_reading(&body, plan, ctx, account),
            Err(reason) => ProviderUsage::unavailable(account.clone(), reason),
        }
    }
}

/// Bearer-authenticated GET with the headers Factory's web app sends.
async fn get(ctx: &FetchContext, path: &str, key: &str, query: &[(&str, String)]) -> Result<Vec<u8>, Fail> {
    let request = ctx
        .http
        .get(format!("{BASE}/{path}"))
        .query(query)
        .bearer_auth(key)
        .header("Accept", "application/json")
        .header("x-factory-client", "web-app")
        .header("Origin", "https://app.factory.ai")
        .header("Referer", "https://app.factory.ai/");
    profile::send(ctx, request, Unavailability::ApiKeyRefused).await
}

// Who is asking

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Me {
    organization: Option<Organization>,
    user_profile: Option<Profile>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Organization {
    subscription: Option<Subscription>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Subscription {
    factory_tier: Option<String>,
    orb_subscription: Option<Orb>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Orb {
    plan: Option<Named>,
}

#[derive(Deserialize)]
struct Named {
    name: Option<String>,
}

#[derive(Deserialize)]
struct Profile {
    id: Option<String>,
}

/// The plan's name (or the capitalised tier when there is no name) and the user's id.
/// Neither is needed for a reading, so a reply without them is not a failure.
fn account_from(body: &[u8]) -> (Option<String>, Option<String>) {
    let Ok(me) = serde_json::from_slice::<Me>(body) else {
        return (None, None);
    };
    let subscription = me.organization.and_then(|o| o.subscription);
    let orb_name = subscription
        .as_ref()
        .and_then(|s| s.orb_subscription.as_ref())
        .and_then(|o| o.plan.as_ref())
        .and_then(|p| p.name.clone());
    let tier = subscription.and_then(|s| s.factory_tier).map(|t| capitalized(&t));
    let plan = [orb_name, tier].into_iter().flatten().find_map(|name| profile::trimmed(Some(name)));
    let user_id = me.user_profile.and_then(|p| profile::trimmed(p.id));
    (plan, user_id)
}

/// Each word with a capital first letter and the rest lower case, as Swift's `capitalized`.
fn capitalized(text: &str) -> String {
    text.split(' ')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

// Token-rate-limits billing

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Limits {
    uses_token_rate_limits_billing: Option<bool>,
    limits: Option<Pools>,
    extra_usage_balance_cents: Option<i64>,
    extra_usage_allowed: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Pools {
    standard: Option<Pool>,
    core: Option<Pool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Pool {
    five_hour: Option<LimitWindow>,
    weekly: Option<LimitWindow>,
    monthly: Option<LimitWindow>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LimitWindow {
    used_percent: Option<f64>,
    /// Kept raw: Factory writes it as seconds, milliseconds, a numeric string or ISO 8601.
    window_end: Option<Value>,
    seconds_remaining: Option<f64>,
}

/// A date Factory writes in any of its four shapes; anything else is no date.
fn factory_date(value: Option<&Value>) -> Option<DateTime<Utc>> {
    match value? {
        Value::Number(n) => profile::epoch(n.as_f64()?),
        Value::String(s) => s
            .trim()
            .parse::<f64>()
            .ok()
            .and_then(profile::epoch)
            .or_else(|| profile::date(Some(s))),
        _ => None,
    }
}

/// The newer billing's reading, or None when this account is not on it and the older route
/// should be asked. A reply on that billing with nothing readable is an unavailable reading.
fn limits_reading(body: &[u8], plan: Option<String>, ctx: &FetchContext, account: &AccountKey) -> Option<ProviderUsage> {
    let reply: Limits = serde_json::from_slice(body).ok()?;
    if reply.uses_token_rate_limits_billing != Some(true) {
        return None;
    }
    let pools = reply.limits.as_ref()?;

    let mut windows = pools.standard.as_ref().map(|p| windows_of(p, None, ctx)).unwrap_or_default();
    // Core is its own pool of models. It is drawn only once it holds something, so an empty
    // pool with no clock (an account not using it) stays off the card.
    if let Some(core) = pools.core.as_ref() {
        let core_windows = windows_of(core, Some("Core"), ctx);
        if core_windows.iter().any(|w| w.used_fraction > 0.0 || w.resets_at.is_some()) {
            windows.extend(core_windows);
        }
    }

    let balance = extra_usage(&reply);
    if windows.is_empty() && balance.is_none() {
        return Some(ProviderUsage::unavailable(account.clone(), Unavailability::NoLimitsReported));
    }
    let mut usage = profile::reading(account, windows, ctx).with_plan(plan);
    if let Some(amount) = balance {
        usage.credit_balance = Some(format!("{} {:.2}", amount.currency, amount.amount));
        usage.credit_remaining = Some(amount);
    }
    Some(usage)
}

fn windows_of(pool: &Pool, scope: Option<&str>, ctx: &FetchContext) -> Vec<UsageWindow> {
    // (window, name in the id, kind, length in seconds, whether that length is stated)
    let shapes = [
        (pool.five_hour.as_ref(), "fiveHour", WindowKind::FiveHour, 5 * 3_600, true),
        (pool.weekly.as_ref(), "weekly", WindowKind::Weekly, 7 * 86_400, true),
        // A billing month, so thirty days is only a sort key.
        (pool.monthly.as_ref(), "monthly", WindowKind::Monthly, 30 * 86_400, false),
    ];
    let prefix = scope.map(|s| format!("{}.", s.to_lowercase())).unwrap_or_default();

    shapes
        .into_iter()
        .filter_map(|(window, name, kind, seconds, reports_length)| {
            let window = window?;
            let percent = window.used_percent.filter(|p| p.is_finite() && *p >= 0.0)?;

            let by_seconds = window.seconds_remaining.filter(|s| s.is_finite() && *s > 0.0);
            let end = factory_date(window.window_end.as_ref());
            let reset = match (by_seconds, end) {
                (Some(s), _) => Some(ctx.now + chrono::Duration::milliseconds((s * 1000.0) as i64)),
                (None, Some(e)) if e > ctx.now => Some(e),
                _ => None,
            };
            // The window this figure belongs to is over and no new one is stated: leave it off.
            if reset.is_none() && end.is_some() {
                return None;
            }

            let mut usage = UsageWindow::new(
                format!("factory.{prefix}{name}"),
                kind,
                percent / 100.0,
                seconds,
            )
            .with_reset(reset)
            .exhausted(percent >= 100.0);
            usage.reports_length = reports_length;
            if let Some(s) = scope {
                usage = usage.with_scope(s);
            }
            Some(usage)
        })
        .collect()
}

/// Extra usage bought on top of the limits, in US cents. Shown when the account can use it or
/// already holds some.
fn extra_usage(reply: &Limits) -> Option<CreditAmount> {
    let cents = reply.extra_usage_balance_cents.filter(|c| *c >= 0)?;
    if cents > 0 || reply.extra_usage_allowed == Some(true) {
        Some(CreditAmount { amount: cents as f64 / 100.0, currency: "USD".to_string() })
    } else {
        None
    }
}

// Older billing: Standard and Premium allowances

#[derive(Deserialize)]
struct Usage {
    usage: Option<Period>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Period {
    start_date: Option<Value>,
    end_date: Option<Value>,
    standard: Option<Tokens>,
    premium: Option<Tokens>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Tokens {
    user_tokens: Option<f64>,
    total_allowance: Option<f64>,
    used_ratio: Option<f64>,
}

fn allowance_reading(body: &[u8], plan: Option<String>, ctx: &FetchContext, account: &AccountKey) -> ProviderUsage {
    let period = match serde_json::from_slice::<Usage>(body).ok().and_then(|u| u.usage) {
        Some(period) => period,
        None => return ProviderUsage::unavailable(account.clone(), Unavailability::UnreadableReply),
    };

    let end = factory_date(period.end_date.as_ref());
    // The period's length is stated only when both ends are; otherwise thirty days is a sort key.
    let stated = match (factory_date(period.start_date.as_ref()), end) {
        (Some(start), Some(end)) => Some((end - start).num_seconds()).filter(|s| *s > 0),
        _ => None,
    };

    let pools = [("standard", "Standard", period.standard), ("premium", "Premium", period.premium)];
    let windows: Vec<UsageWindow> = pools
        .into_iter()
        .filter_map(|(id, scope, tokens)| {
            let fraction = fraction_of(&tokens?)?;
            let mut window = UsageWindow::new(
                format!("factory.{id}"),
                WindowKind::Monthly,
                fraction,
                stated.unwrap_or(30 * 86_400),
            )
            .with_reset(end)
            .with_scope(scope)
            .exhausted(fraction >= 1.0);
            window.reports_length = stated.is_some();
            Some(window)
        })
        .collect();

    if windows.is_empty() {
        return ProviderUsage::unavailable(account.clone(), Unavailability::NoLimitsReported);
    }
    profile::reading(account, windows, ctx).with_plan(plan)
}

/// Factory's own ratio when it is usable; otherwise tokens used against the allowance.
fn fraction_of(tokens: &Tokens) -> Option<f64> {
    let used = tokens.user_tokens.filter(|u| u.is_finite() && *u >= 0.0);
    let allowance = tokens
        .total_allowance
        .filter(|a| a.is_finite() && *a > 0.0 && *a <= UNLIMITED);

    if let Some(ratio) = tokens.used_ratio.filter(|r| r.is_finite() && (-0.001..=1.001).contains(r)) {
        // A zero ratio beside tokens used and a real allowance is a cache that has not caught up.
        // A zero with no allowance is a pool this plan does not have. Neither is a figure.
        let lagging = ratio <= 0.0 && used.unwrap_or(0.0) > 0.0 && allowance.is_some();
        let absent = ratio <= 0.0 && allowance.is_none();
        if !lagging && !absent {
            return Some(ratio.clamp(0.0, 1.0));
        }
    }
    Some(used? / allowance?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::profile::test_support::{context, fixture};
    use crate::model::UsageState;

    fn account() -> AccountKey {
        AccountKey::primary(Provider::Factory)
    }

    /// Before every reset in the fixtures, so none of them has passed.
    fn ctx() -> FetchContext {
        let mut ctx = context();
        ctx.now = DateTime::from_timestamp(1_789_000_000, 0).unwrap();
        ctx
    }

    fn limits(json: &str) -> Option<ProviderUsage> {
        limits_reading(json.as_bytes(), None, &ctx(), &account())
    }

    fn allowances(json: &str) -> ProviderUsage {
        allowance_reading(json.as_bytes(), None, &ctx(), &account())
    }

    #[test]
    fn the_plan_name_and_user_id_come_from_auth_me() {
        let (plan, user) = account_from(&fixture("factory-auth-me.json"));
        assert_eq!(plan.as_deref(), Some("Team"));
        assert_eq!(user.as_deref(), Some("user_123"));
    }

    #[test]
    fn without_a_plan_name_the_tier_stands_in_and_without_either_nothing_does() {
        let tier_only = br#"{"organization":{"subscription":{"factoryTier":"enterprise"}}}"#;
        assert_eq!(account_from(tier_only).0.as_deref(), Some("Enterprise"));
        assert_eq!(account_from(b"not json"), (None, None));
    }

    #[test]
    fn both_pools_are_read_as_reported_standard_first() {
        let usage = limits_reading(&fixture("factory-billing-limits.json"), Some("Team".into()), &ctx(), &account()).unwrap();

        assert_eq!(usage.state, UsageState::Live);
        assert_eq!(usage.account, account());
        assert_eq!(usage.plan.as_deref(), Some("Team"));
        let kinds: Vec<WindowKind> = usage.windows.iter().map(|w| w.kind).collect();
        assert_eq!(
            kinds,
            [WindowKind::FiveHour, WindowKind::Weekly, WindowKind::Monthly, WindowKind::FiveHour, WindowKind::Weekly, WindowKind::Monthly]
        );
        let fractions: Vec<f64> = usage.windows.iter().map(|w| w.used_fraction).collect();
        assert_eq!(fractions, [0.12, 0.34, 0.56, 0.07, 0.08, 0.09]);
        let scopes: Vec<Option<&str>> = usage.windows.iter().map(|w| w.scope.as_deref()).collect();
        assert_eq!(scopes, [None, None, None, Some("Core"), Some("Core"), Some("Core")]);
        let ids: std::collections::BTreeSet<&str> = usage.windows.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(ids.len(), 6);
    }

    #[test]
    fn each_reset_comes_from_seconds_remaining_then_an_end_in_millis_or_iso() {
        let usage = limits_reading(&fixture("factory-billing-limits.json"), None, &ctx(), &account()).unwrap();
        let now = ctx().now;
        assert_eq!(usage.windows[0].resets_at, Some(now + chrono::Duration::seconds(3_600)));
        assert_eq!(usage.windows[1].resets_at, DateTime::from_timestamp(1_790_000_000, 0));
        assert_eq!(usage.windows[2].resets_at, profile::date(Some("2026-10-20T00:00:00Z")));
        // A billing month is not a stated length; five hours and a week are.
        let stated: Vec<bool> = usage.windows.iter().map(|w| w.reports_length).collect();
        assert_eq!(stated, [true, true, false, true, true, false]);
    }

    #[test]
    fn the_extra_usage_balance_is_money_in_us_cents() {
        let usage = limits_reading(&fixture("factory-billing-limits.json"), None, &ctx(), &account()).unwrap();
        assert_eq!(usage.credit_remaining, Some(CreditAmount { amount: 25.0, currency: "USD".into() }));
        assert!(usage.credit_balance.as_deref().unwrap_or("").contains("25"));
    }

    #[test]
    fn no_extra_usage_offered_and_none_bought_gives_no_balance_rather_than_zero() {
        let json = r#"{"usesTokenRateLimitsBilling":true,"extraUsageBalanceCents":0,"extraUsageAllowed":false,"limits":{"standard":{"fiveHour":{"usedPercent":40,"secondsRemaining":60}}}}"#;
        let usage = limits(json).unwrap();
        assert_eq!(usage.credit_balance, None);
        assert_eq!(usage.credit_remaining, None);
        let fractions: Vec<f64> = usage.windows.iter().map(|w| w.used_fraction).collect();
        assert_eq!(fractions, [0.4]);
    }

    #[test]
    fn a_window_whose_stated_end_has_passed_is_left_off_not_drawn_at_zero() {
        let json = r#"{"usesTokenRateLimitsBilling":true,"limits":{"standard":{"fiveHour":{"usedPercent":80,"windowEnd":1700000000},"weekly":{"usedPercent":30,"secondsRemaining":86400}}}}"#;
        let kinds: Vec<WindowKind> = limits(json).unwrap().windows.iter().map(|w| w.kind).collect();
        assert_eq!(kinds, [WindowKind::Weekly]);
    }

    #[test]
    fn a_core_pool_with_nothing_in_it_and_no_clock_is_not_drawn() {
        let json = r#"{"usesTokenRateLimitsBilling":true,"limits":{"standard":{"weekly":{"usedPercent":30,"secondsRemaining":86400}},"core":{"fiveHour":{"usedPercent":0},"weekly":{"usedPercent":0},"monthly":{"usedPercent":0}}}}"#;
        let usage = limits(json).unwrap();
        assert!(usage.windows.iter().all(|w| w.scope.is_none()));
    }

    #[test]
    fn a_spent_limit_says_so() {
        let json = r#"{"usesTokenRateLimitsBilling":true,"limits":{"standard":{"fiveHour":{"usedPercent":100,"secondsRemaining":600}}}}"#;
        assert!(limits(json).unwrap().windows[0].is_exhausted);
    }

    #[test]
    fn an_account_not_on_the_newer_billing_is_sent_to_the_older_route() {
        for json in [
            r#"{"usesTokenRateLimitsBilling":false,"limits":{"standard":{"weekly":{"usedPercent":3}}}}"#,
            r#"{"usesTokenRateLimitsBilling":true}"#,
            r#"{"extraUsageBalanceCents":0}"#,
            "not json",
        ] {
            assert!(limits(json).is_none(), "{json}");
        }
    }

    #[test]
    fn figures_that_are_not_figures_are_left_off_and_nothing_left_is_no_limits() {
        let json = r#"{"usesTokenRateLimitsBilling":true,"limits":{"standard":{"fiveHour":{"usedPercent":-5,"secondsRemaining":60},"weekly":{"secondsRemaining":60}}}}"#;
        assert_eq!(limits(json).unwrap().state, UsageState::Unavailable(Unavailability::NoLimitsReported));
    }

    #[test]
    fn standard_and_premium_are_read_from_the_ratio_over_the_stated_period() {
        let usage = allowance_reading(&fixture("factory-subscription-usage.json"), Some("Team".into()), &ctx(), &account());

        assert_eq!(usage.state, UsageState::Live);
        assert_eq!(usage.plan.as_deref(), Some("Team"));
        let scopes: Vec<Option<&str>> = usage.windows.iter().map(|w| w.scope.as_deref()).collect();
        assert_eq!(scopes, [Some("Standard"), Some("Premium")]);
        let fractions: Vec<f64> = usage.windows.iter().map(|w| w.used_fraction).collect();
        assert_eq!(fractions, [0.1, 0.2]);
        assert!(usage.windows.iter().all(|w| w.kind == WindowKind::Monthly));
        // Milliseconds; both ends are stated, so the period's length is too.
        assert_eq!(usage.windows[0].resets_at, DateTime::from_timestamp(1_702_592_000, 0));
        assert_eq!(usage.windows[0].window_seconds, 30 * 86_400);
        assert!(usage.windows[0].reports_length);
    }

    #[test]
    fn without_a_start_the_period_length_is_a_sort_key_not_a_claim() {
        let window = allowances(r#"{"usage":{"endDate":1702592000000,"standard":{"usedRatio":0.5}}}"#).windows[0].clone();
        assert_eq!(window.used_fraction, 0.5);
        assert!(!window.reports_length);
    }

    #[test]
    fn a_zero_ratio_beside_tokens_used_and_a_real_allowance_is_a_lagging_cache() {
        let json = r#"{"usage":{"standard":{"userTokens":5000000,"totalAllowance":20000000,"usedRatio":0}}}"#;
        assert_eq!(allowances(json).windows[0].used_fraction, 0.25);
    }

    #[test]
    fn a_ratio_out_of_scale_is_not_read_the_counts_are_when_both_are_reported() {
        let counted = r#"{"usage":{"standard":{"userTokens":50,"totalAllowance":100,"usedRatio":1.5}}}"#;
        assert_eq!(allowances(counted).windows[0].used_fraction, 0.5);
        // A ratio of 10.0 with no allowance has no known scale, so it is nothing.
        let guessed = r#"{"usage":{"standard":{"userTokens":0,"totalAllowance":0,"usedRatio":10.0}}}"#;
        assert_eq!(allowances(guessed).state, UsageState::Unavailable(Unavailability::NoLimitsReported));
    }

    #[test]
    fn an_unlimited_allowance_draws_no_ring() {
        let json = r#"{"usage":{"standard":{"userTokens":50000000,"totalAllowance":2000000000000}}}"#;
        assert_eq!(allowances(json).state, UsageState::Unavailable(Unavailability::NoLimitsReported));
    }

    #[test]
    fn a_pool_the_plan_does_not_have_is_left_off_not_drawn_at_zero() {
        let json = r#"{"usage":{"standard":{"usedRatio":0.3},"premium":{"userTokens":0,"totalAllowance":0,"usedRatio":0}}}"#;
        let usage = allowances(json);
        let scopes: Vec<Option<&str>> = usage.windows.iter().map(|w| w.scope.as_deref()).collect();
        assert_eq!(scopes, [Some("Standard")]);
    }

    #[test]
    fn a_reply_that_cannot_be_read() {
        for json in ["not json", "{}", r#"{"usage":null}"#] {
            assert_eq!(allowances(json).state, UsageState::Unavailable(Unavailability::UnreadableReply), "{json}");
        }
    }
}
