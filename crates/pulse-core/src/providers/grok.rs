// Ported from upstream Providers/GrokUsageService.swift.
//! Grok: the account's weekly credit pool, read with the login the Grok CLI saved.
//!
//! Grok Build's CLI stores an OIDC login in `~/.grok/auth.json`; this asks the
//! CLI's own proxy with it: `GET cli-chat-proxy.grok.com/v1/billing?format=credits`.
//! The reply is the account's pool, shared by every Grok product, so there is one
//! window (`grok-pool`), not one per product.
//!
//! An absent `creditUsagePercent` is a zero the proto3 serialiser dropped, but only
//! inside a period that is actually running; for an ended period it says nothing.
//! The stored token lasts about six hours and nothing renews it for Pulse, so an
//! aged-out login is reported rather than worked around. Neither endpoint is public.
//!
//! Added accounts hold their own login (stored by the sign-in, renewed on use) and never
//! look at the CLI's file.
//!
//! TODO: the Grok Bot route (`GrokBotUsageService.swift`) is not ported.

use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;

use super::profile::{self, Fail};
use crate::auth::{self, StoredLogin};
use crate::model::{AccountKey, ProviderUsage, Unavailability, UsageWindow, WindowKind};
use crate::provider::Provider;
use crate::service::{FetchContext, UsageService};

const BILLING_URL: &str = "https://cli-chat-proxy.grok.com/v1/billing?format=credits";
/// The plan's name comes from here, not from the billing reply.
const SETTINGS_URL: &str = "https://cli-chat-proxy.grok.com/v1/settings";

#[derive(Default)]
pub struct Grok;

#[async_trait]
impl UsageService for Grok {
    fn provider(&self) -> Provider {
        Provider::Grok
    }

    async fn fetch(&self, ctx: &FetchContext, account: &AccountKey) -> ProviderUsage {
        let token = if account.is_primary() {
            match stored_login(&auth_path(ctx), ctx.now) {
                Login::None => return ProviderUsage::unavailable(account.clone(), Unavailability::GrokSignInRequired),
                Login::Expired => return ProviderUsage::unavailable(account.clone(), Unavailability::GrokLoginExpired),
                Login::Usable(token) => token,
            }
        } else {
            // The login Pulse holds (renewed here); a raw token saved by an older build is read
            // when none is stored.
            match auth::usable_login(ctx, account).await {
                StoredLogin::Usable(login) => login.access_token,
                StoredLogin::Expired => return ProviderUsage::unavailable(account.clone(), Unavailability::SignedOut),
                StoredLogin::Missing => match ctx.api_key(account) {
                    Some(token) => token.trim().to_string(),
                    None => return ProviderUsage::unavailable(account.clone(), Unavailability::SignedOut),
                },
            }
        };

        let refused = if account.is_primary() { Unavailability::GrokLoginExpired } else { Unavailability::SignedOut };
        let body = match profile::send(ctx, request(ctx, BILLING_URL, &token), refused).await {
            Ok(body) => body,
            Err(reason) => return ProviderUsage::unavailable(account.clone(), reason),
        };
        match reading(&body, ctx, account) {
            // `prepaidBalance` and `onDemandCap` carry an unnamed unit, so no balance is reported.
            Ok(usage) => usage.with_plan(plan(ctx, &token).await),
            Err(reason) => ProviderUsage::unavailable(account.clone(), reason),
        }
    }
}

// WINDOWS-PATH: unverified (same relative path under the user profile as on macOS)
fn auth_path(ctx: &FetchContext) -> PathBuf {
    ctx.home.join(".grok").join("auth.json")
}

fn request(ctx: &FetchContext, url: &str, token: &str) -> reqwest::RequestBuilder {
    ctx.http
        .get(url)
        .timeout(Duration::from_secs(15))
        .bearer_auth(token)
        // What the CLI sends. Without it the proxy answers the enterprise credit
        // shape, whose `monthlyLimit` is zero on a personal plan.
        .header("x-xai-token-auth", "xai-grok-cli")
        .header("Accept", "application/json")
}

/// A second call, since the billing reply does not name the plan. Optional
/// enrichment on a short budget: a stalled call must not hold the figures back.
async fn plan(ctx: &FetchContext, token: &str) -> Option<String> {
    let reply = request(ctx, SETTINGS_URL, token).timeout(Duration::from_secs(4)).send().await.ok()?;
    if reply.status().as_u16() != 200 {
        return None;
    }
    plan_name(&profile::decode::<Settings>(&reply.bytes().await.ok()?).ok()?)
}

#[derive(Deserialize)]
struct Billing {
    config: Option<Config>,
}

#[derive(Deserialize)]
struct Config {
    #[serde(rename = "currentPeriod")]
    current_period: Option<Period>,
    /// How much of the pool is gone, 0...100. Omitted when zero.
    #[serde(rename = "creditUsagePercent")]
    credit_usage_percent: Option<f64>,
    #[serde(rename = "billingPeriodStart")]
    billing_period_start: Option<String>,
    #[serde(rename = "billingPeriodEnd")]
    billing_period_end: Option<String>,
}

#[derive(Deserialize)]
struct Period {
    start: Option<String>,
    end: Option<String>,
}

#[derive(Deserialize)]
struct Settings {
    subscription_tier_display: Option<String>,
}

fn plan_name(settings: &Settings) -> Option<String> {
    profile::trimmed(settings.subscription_tier_display.clone())
}

fn reading(body: &[u8], ctx: &FetchContext, account: &AccountKey) -> Result<ProviderUsage, Fail> {
    let billing: Billing = profile::decode(body)?;
    let config = billing.config.ok_or(Unavailability::UnreadableReply)?;
    let window = window(&config, ctx.now).ok_or(Unavailability::NoLimitsReported)?;
    Ok(profile::reading(account, vec![window], ctx))
}

/// The one window the pool amounts to. Both ends of the period are stated, so the
/// length is measured from them; `currentPeriod` wins over the flat pair because
/// it says which period is current.
fn window(config: &Config, now: DateTime<Utc>) -> Option<UsageWindow> {
    let period = config.current_period.as_ref();
    let start = profile::date(period.and_then(|p| p.start.as_deref()).or(config.billing_period_start.as_deref()))?;
    let end = profile::date(period.and_then(|p| p.end.as_deref()).or(config.billing_period_end.as_deref()))?;
    if end <= start {
        return None;
    }
    let seconds = ((end - start).num_milliseconds() as f64 / 1000.0).round() as i64;

    // Absent means zero only inside a running period; after it ended it says nothing.
    let running = start <= now && now <= end;
    let percent = config.credit_usage_percent.filter(|p| p.is_finite()).or(running.then_some(0.0))?;

    Some(
        UsageWindow::new("grok-pool", kind(seconds), (percent / 100.0).clamp(0.0, 1.0), seconds)
            .with_reset(Some(end))
            // Grok's own figure saying the pool is gone.
            .exhausted(percent >= 100.0),
    )
}

/// Named from the measured length rather than the reply's `type` string; an
/// unfamiliar length is still shown, under its own duration.
fn kind(seconds: i64) -> WindowKind {
    match seconds {
        s if (6 * 86_400..=8 * 86_400).contains(&s) => WindowKind::Weekly,
        s if (27 * 86_400..=32 * 86_400).contains(&s) => WindowKind::Monthly,
        s => WindowKind::Other(s),
    }
}

#[derive(Debug, PartialEq)]
enum Login {
    None,
    Expired,
    Usable(String),
}

/// What the CLI wrote at its last `grok login`: entries keyed by issuer and client
/// id, so the freshest unexpired one is taken. An aged-out entry is evidence of a
/// stale login, which differs from never having signed in.
fn stored_login(path: &std::path::Path, now: DateTime<Utc>) -> Login {
    let Ok(bytes) = std::fs::read(path) else { return Login::None };
    login_from(&bytes, now)
}

fn login_from(bytes: &[u8], now: DateTime<Utc>) -> Login {
    let Ok(serde_json::Value::Object(root)) = serde_json::from_slice(bytes) else { return Login::None };

    let mut saw_entry = false;
    let mut best: Option<(String, DateTime<Utc>)> = None;
    for entry in root.values().filter_map(|v| v.as_object()) {
        let Some(token) = entry.get("key").and_then(|k| k.as_str()).filter(|k| !k.is_empty()) else { continue };
        saw_entry = true;

        // No expiry stated is not expired: the token is undated and the service
        // decides. It is a last resort that cannot outrank a dated one.
        let Some(expiry) = profile::date(entry.get("expires_at").and_then(|e| e.as_str())) else {
            if best.is_none() {
                best = Some((token.to_string(), DateTime::<Utc>::MIN_UTC));
            }
            continue;
        };
        if expiry <= now {
            continue;
        }
        if best.as_ref().map_or(true, |(_, current)| expiry > *current) {
            best = Some((token.to_string(), expiry));
        }
    }

    match best {
        Some((token, _)) => Login::Usable(token),
        None if saw_entry => Login::Expired,
        None => Login::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::profile::test_support::{context, fixture};
    use chrono::TimeZone;

    fn config(name: &str) -> Config {
        serde_json::from_slice::<Billing>(&fixture(name)).unwrap().config.unwrap()
    }

    fn now() -> DateTime<Utc> {
        context().now
    }

    fn login(name: &str) -> Login {
        login_from(&fixture(name), now())
    }

    #[test]
    fn no_auth_file_at_all_is_never_signed_in() {
        let missing = std::env::temp_dir().join("pulse-grok-test-missing").join("auth.json");
        assert_eq!(stored_login(&missing, now()), Login::None);
    }

    #[test]
    fn only_expired_entries_read_as_expired_not_absent() {
        assert_eq!(login("grok-auth-expired.json"), Login::Expired);
    }

    #[test]
    fn the_freshest_unexpired_entry_wins() {
        assert_eq!(login("grok-auth-multiple.json"), Login::Usable("fresh-token".into()));
    }

    #[test]
    fn an_undated_entry_is_usable_when_it_is_all_there_is() {
        assert_eq!(login("grok-auth-undated-only.json"), Login::Usable("undated-token".into()));
    }

    #[test]
    fn an_entry_without_a_key_is_not_a_sign_in() {
        assert_eq!(login("grok-auth-no-key.json"), Login::None);
        assert_eq!(login_from(b"not json", now()), Login::None);
    }

    #[test]
    fn a_normal_reply_becomes_one_weekly_window() {
        let w = window(&config("grok-billing-weekly.json"), now()).unwrap();
        assert_eq!(w.id, "grok-pool");
        assert_eq!(w.kind, WindowKind::Weekly);
        assert_eq!(w.scope, None);
        assert!((w.used_fraction - 0.425).abs() < 1e-6);
        assert_eq!(w.window_seconds, 7 * 86_400);
        assert_eq!(w.resets_at, Some(Utc.with_ymd_and_hms(2026, 1, 8, 0, 0, 0).unwrap()));
        assert!(w.reports_length);
        // No flag like DeepSeek's `is_available`, so nothing may call the account spent.
        assert!(!w.is_exhausted);
    }

    #[test]
    fn around_thirty_days_is_monthly() {
        assert_eq!(window(&config("grok-billing-monthly.json"), now()).unwrap().kind, WindowKind::Monthly);
    }

    #[test]
    fn an_unfamiliar_length_is_shown_under_its_own_duration() {
        assert_eq!(window(&config("grok-billing-other-length.json"), now()).unwrap().kind, WindowKind::Other(2 * 86_400));
    }

    #[test]
    fn an_absent_percentage_inside_a_running_period_is_zero() {
        assert_eq!(window(&config("grok-billing-percent-absent-active.json"), now()).unwrap().used_fraction, 0.0);
    }

    #[test]
    fn an_absent_percentage_in_an_ended_period_gives_no_window() {
        assert!(window(&config("grok-billing-percent-absent-ended.json"), now()).is_none());
    }

    #[test]
    fn no_timestamps_gives_no_window() {
        assert!(window(&config("grok-billing-no-timestamps.json"), now()).is_none());
    }

    #[test]
    fn a_percentage_at_or_over_100_fills_the_ring_and_marks_it_spent() {
        let w = window(&config("grok-billing-exhausted.json"), now()).unwrap();
        assert_eq!(w.used_fraction, 1.0);
        assert!(w.is_exhausted);
    }

    #[test]
    fn just_under_100_is_not_spent_and_exactly_100_is() {
        for (percent, spent) in [(99.9, false), (100.0, true)] {
            let json = format!(
                r#"{{"config":{{"currentPeriod":{{"start":"2026-01-01T00:00:00Z","end":"2026-01-08T00:00:00Z"}},"creditUsagePercent":{percent}}}}}"#
            );
            let config = serde_json::from_str::<Billing>(&json).unwrap().config.unwrap();
            assert_eq!(window(&config, now()).unwrap().is_exhausted, spent, "{percent}");
        }
    }

    #[test]
    fn the_plan_name_is_read_and_trimmed() {
        let settings: Settings = serde_json::from_slice(&fixture("grok-settings.json")).unwrap();
        assert_eq!(plan_name(&settings).as_deref(), Some("SuperGrok"));
        let blank: Settings = serde_json::from_slice(&fixture("grok-settings-blank.json")).unwrap();
        assert_eq!(plan_name(&blank), None);
        let missing: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(plan_name(&missing), None);
    }

    #[test]
    fn a_reply_with_no_config_is_unreadable() {
        let ctx = context();
        let account = AccountKey::primary(Provider::Grok);
        assert!(serde_json::from_str::<Billing>("{}").unwrap().config.is_none());
        assert_eq!(reading(b"{}", &ctx, &account).unwrap_err(), Unavailability::UnreadableReply);
        assert_eq!(reading(&fixture("grok-billing-no-timestamps.json"), &ctx, &account).unwrap_err(), Unavailability::NoLimitsReported);
        assert_eq!(reading(&fixture("grok-billing-weekly.json"), &ctx, &account).unwrap().windows.len(), 1);
    }
}
