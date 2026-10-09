// Ported from upstream Providers/OpenCodeGoUsageService.swift.
//! OpenCode Go: `GET https://opencode.ai/zen/go/v1/usage` with an API key.
//!
//! The key comes from, in order:
//! 1. the key pasted into Settings (it wins: whoever typed one meant it);
//! 2. what the `opencode` CLI saved in `~/.local/share/opencode/auth.json`.
//!
//! The endpoint is undocumented and can change without notice. The reply's
//! `percent` is how much is gone (no inversion), `status` other than "ok"
//! means spent, and the window the reply calls "rolling" is the five-hour one.
//!
//! **A third way in: the console's session** (`opencode_console`), signed in once from Settings.
//! The console answers the same three limits at `GET /console/api/go/status`, in money rather than
//! in percent. It is asked when there is no key at all, or when the key route turned the key away
//! or did not answer, never instead of a key that works, so nobody who set one up is moved off it.
//! The request-log history is `opencode_console`'s.

use std::path::PathBuf;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;

use super::profile::{self, Fail};
use super::opencode_console as console;
use crate::model::{AccountKey, ProviderUsage, Unavailability, UsageRoute, UsageState, UsageWindow, WindowKind};
use crate::provider::Provider;
use crate::service::{FetchContext, UsageService};

const ENDPOINT: &str = "https://opencode.ai/zen/go/v1/usage";

#[derive(Default)]
pub struct OpenCodeGo;

#[async_trait]
impl UsageService for OpenCodeGo {
    fn provider(&self) -> Provider {
        Provider::OpenCodeGo
    }

    async fn fetch(&self, ctx: &FetchContext, account: &AccountKey) -> ProviderUsage {
        let cookie = ctx.secrets.get(&console::secret_id()).filter(|c| !c.trim().is_empty());
        let Some(key) = ctx.api_key(account).or_else(|| stored_key(ctx)) else {
            return match cookie {
                Some(cookie) => fetch_console(ctx, account, &cookie).await,
                None => ProviderUsage::unavailable(account.clone(), Unavailability::ApiKeyMissing),
            };
        };
        let by_key = match profile::get_bearer(ctx, ENDPOINT, key.trim()).await {
            Ok(body) => reading(&body, ctx, account).unwrap_or_else(|reason| ProviderUsage::unavailable(account.clone(), reason)),
            Err(reason) => ProviderUsage::unavailable(account.clone(), reason),
        };
        // The console stands in only where the key could not answer. A reading the key did get,
        // even one with no limits in it, stands.
        if let (UsageState::Unavailable(reason), Some(cookie)) = (by_key.state, cookie) {
            if matches!(
                reason,
                Unavailability::ApiKeyRefused | Unavailability::Unreachable | Unavailability::ServerError | Unavailability::UnreadableReply
            ) {
                let by_console = fetch_console(ctx, account, &cookie).await;
                if by_console.state == UsageState::Live {
                    return by_console;
                }
            }
        }
        by_key
    }
}

// WINDOWS-PATH: unverified (same relative path under the user profile as on macOS/Linux)
fn auth_path(ctx: &FetchContext) -> PathBuf {
    ctx.home.join(".local").join("share").join("opencode").join("auth.json")
}

/// The key `opencode` wrote when it signed in.
fn stored_key(ctx: &FetchContext) -> Option<String> {
    key_from_auth(&std::fs::read(auth_path(ctx)).ok()?)
}

fn key_from_auth(bytes: &[u8]) -> Option<String> {
    let root: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    let key = root.get("opencode-go")?.get("key")?.as_str()?;
    (!key.is_empty()).then(|| key.to_string())
}

#[derive(Deserialize)]
struct Reply {
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct Usage {
    rolling: Option<Reported>,
    weekly: Option<Reported>,
    monthly: Option<Reported>,
}

#[derive(Deserialize)]
struct Reported {
    status: Option<String>,
    /// How much is gone, 0...100.
    percent: Option<f64>,
    #[serde(rename = "resetsAt")]
    resets_at: Option<String>,
}

fn reading(body: &[u8], ctx: &FetchContext, account: &AccountKey) -> Result<ProviderUsage, Fail> {
    let reply: Reply = profile::decode(body)?;
    let windows = windows(&reply);
    if windows.is_empty() {
        return Err(Unavailability::NoLimitsReported);
    }
    // The reply carries limits only: no plan name and no balance, so none is invented.
    Ok(profile::reading(account, windows, ctx))
}

/// Shortest window first: the one about to bite leads.
fn windows(reply: &Reply) -> Vec<UsageWindow> {
    let Some(usage) = &reply.usage else { return Vec::new() };
    [
        window(usage.rolling.as_ref(), "rolling", WindowKind::FiveHour, 5 * 3_600, true),
        window(usage.weekly.as_ref(), "weekly", WindowKind::Weekly, 7 * 86_400, true),
        // A month is not thirty days and the reply states no start, so the
        // length only orders the row.
        window(usage.monthly.as_ref(), "monthly", WindowKind::Monthly, 30 * 86_400, false),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn window(reported: Option<&Reported>, id: &str, kind: WindowKind, seconds: i64, reports_length: bool) -> Option<UsageWindow> {
    let reported = reported?;
    let percent = reported.percent.filter(|p| p.is_finite())?;
    let mut window = UsageWindow::new(id, kind, (percent / 100.0).clamp(0.0, 1.0), seconds)
        .with_reset(profile::date(reported.resets_at.as_deref()))
        // The provider's own verdict: anything but "ok" counts as spent.
        .exhausted(reported.status.as_deref().unwrap_or("ok").to_lowercase() != "ok");
    window.reports_length = reports_length;
    Some(window)
}

// MARK: the console

const CONSOLE_STATUS: &str = "https://opencode.ai/console/api/go/status";

/// The plan's limits from the console's session. The workspace is resolved first
/// (`WorkspaceCache`), because every console route wants it in `x-org-id`.
pub async fn fetch_console(ctx: &FetchContext, account: &AccountKey, cookie: &str) -> ProviderUsage {
    let org = match console::WorkspaceCache::shared().resolve(&ctx.http, cookie).await {
        console::Resolved::Workspace(workspace) => workspace.id,
        console::Resolved::SignedOut => return ProviderUsage::unavailable(account.clone(), Unavailability::SessionExpired),
        console::Resolved::Failed => return ProviderUsage::unavailable(account.clone(), Unavailability::ServerError),
    };
    let request = ctx
        .http
        .get(CONSOLE_STATUS)
        .header("Cookie", cookie)
        .header("x-org-id", org)
        .header("Accept", "application/json");
    let body = match profile::send(ctx, request, Unavailability::SessionExpired).await {
        Ok(body) => body,
        Err(reason) => return ProviderUsage::unavailable(account.clone(), reason),
    };
    console_reading(&body, ctx, account)
}

/// The console's reply as a reading; a sign-in page in place of JSON is a lapsed session.
fn console_reading(body: &[u8], ctx: &FetchContext, account: &AccountKey) -> ProviderUsage {
    let Ok(status) = serde_json::from_slice::<ConsoleStatus>(body) else {
        let signed_out = body[..body.len().min(64)].contains(&b'<');
        return ProviderUsage::unavailable(
            account.clone(),
            if signed_out { Unavailability::SessionExpired } else { Unavailability::UnreadableReply },
        );
    };
    let windows = console_windows(&status);
    if windows.is_empty() {
        let reason = if status.access.is_none() { Unavailability::NoPlan } else { Unavailability::NoLimitsReported };
        return ProviderUsage::unavailable(account.clone(), reason);
    }
    // The console names the product it bills; the key's reply names none.
    profile::reading(account, windows, ctx)
        .with_plan((status.product.as_deref() == Some("go")).then(|| "Go".to_string()))
        .with_origin(UsageRoute::BrowserSession)
}

/// Whether a workspace holds a Go subscription, for choosing among several.
pub async fn has_go_access(http: &reqwest::Client, cookie: &str, org: &str) -> bool {
    let request = http.get(CONSOLE_STATUS).header("Cookie", cookie).header("x-org-id", org).timeout(std::time::Duration::from_secs(15));
    let Ok(reply) = request.send().await else { return false };
    if reply.status().as_u16() != 200 {
        return false;
    }
    let Ok(bytes) = reply.bytes().await else { return false };
    serde_json::from_slice::<ConsoleStatus>(&bytes).is_ok_and(|s| s.access.is_some())
}

// MARK: the console's reply

/// The console's answer: three meters, each in micro-cents used against a limit.
/// Only these fields are read; the reply also names the payment method and account.
#[derive(Deserialize)]
pub struct ConsoleStatus {
    pub product: Option<String>,
    access: Option<Access>,
}

#[derive(Deserialize)]
struct Access {
    #[serde(rename = "startsAt")]
    starts_at: Option<String>,
    #[serde(rename = "endsAt")]
    ends_at: Option<String>,
    meters: Option<Meters>,
}

#[derive(Deserialize)]
struct Meters {
    #[serde(rename = "fiveHour")]
    five_hour: Option<Meter>,
    week: Option<Meter>,
    month: Option<Meter>,
}

/// Counts are strings in the reply: a micro-cent count can pass what JSON numbers carry exactly.
#[derive(Deserialize)]
struct Meter {
    #[serde(rename = "startsAt")]
    starts_at: Option<String>,
    #[serde(rename = "resetsAt")]
    resets_at: Option<String>,
    #[serde(rename = "limitMicroCents")]
    limit_micro_cents: Option<String>,
    #[serde(rename = "usedMicroCents")]
    used_micro_cents: Option<String>,
}

/// The same three windows, under the same ids, as the key's reply, so a pinned
/// limit survives whichever route answered. Lengths are measured where the
/// console states a start; the month starts with the billing period when the
/// two end together.
pub fn console_windows(status: &ConsoleStatus) -> Vec<UsageWindow> {
    let Some(access) = &status.access else { return Vec::new() };
    let Some(meters) = &access.meters else { return Vec::new() };
    let month_resets = meters.month.as_ref().and_then(|m| m.resets_at.as_deref());
    let period_start = if access.ends_at.is_some() && access.ends_at.as_deref() == month_resets { access.starts_at.as_deref() } else { None };
    [
        console_window(meters.five_hour.as_ref(), "rolling", WindowKind::FiveHour, 5 * 3_600, meters.five_hour.as_ref().and_then(|m| m.starts_at.as_deref())),
        console_window(meters.week.as_ref(), "weekly", WindowKind::Weekly, 7 * 86_400, meters.week.as_ref().and_then(|m| m.starts_at.as_deref())),
        console_window(meters.month.as_ref(), "monthly", WindowKind::Monthly, 30 * 86_400, period_start),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn console_window(meter: Option<&Meter>, id: &str, kind: WindowKind, seconds: i64, starts_at: Option<&str>) -> Option<UsageWindow> {
    let meter = meter?;
    let limit = meter.limit_micro_cents.as_deref()?.trim().parse::<f64>().ok().filter(|l| l.is_finite() && *l > 0.0)?;
    let used = meter.used_micro_cents.as_deref()?.trim().parse::<f64>().ok().filter(|u| u.is_finite())?;

    let resets: Option<DateTime<Utc>> = profile::date(meter.resets_at.as_deref());
    let start = profile::date(starts_at);
    let measured = resets.zip(start).map(|(end, start)| (end - start).num_seconds()).filter(|s| *s > 0);

    let mut window = UsageWindow::new(id, kind, (used / limit).clamp(0.0, 1.0), measured.unwrap_or(seconds))
        .with_reset(resets)
        .exhausted(used >= limit);
    // A stated start gives a real length; otherwise the nominal one only sorts.
    window.reports_length = measured.is_some();
    Some(window)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::profile::test_support::{context, fixture};
    use chrono::TimeZone;

    fn reply(name: &str) -> Reply {
        serde_json::from_slice(&fixture(name)).unwrap()
    }

    fn decode(json: &str) -> Reply {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn a_normal_reply_becomes_rolling_weekly_monthly() {
        let w = windows(&reply("opencode-go-normal.json"));
        assert_eq!(w.iter().map(|w| w.id.as_str()).collect::<Vec<_>>(), ["rolling", "weekly", "monthly"]);
        assert_eq!(w.iter().map(|w| w.kind).collect::<Vec<_>>(), [WindowKind::FiveHour, WindowKind::Weekly, WindowKind::Monthly]);
        assert!((w[0].used_fraction - 0.425).abs() < 1e-6);
        assert!((w[1].used_fraction - 0.10).abs() < 1e-6);
        assert!((w[2].used_fraction - 0.05).abs() < 1e-6);
        assert!(w.iter().all(|w| !w.is_exhausted));
        assert_eq!(w.iter().map(|w| w.reports_length).collect::<Vec<_>>(), [true, true, false]);
        // Milliseconds and bare-second stamps both parse.
        assert_eq!(w[0].resets_at, Some(Utc.with_ymd_and_hms(2026, 9, 26, 18, 0, 0).unwrap()));
        assert_eq!(w[2].resets_at, Some(Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap()));
    }

    #[test]
    fn a_window_with_no_percent_is_left_out() {
        let w = windows(&reply("opencode-go-partial.json"));
        assert_eq!(w.iter().map(|w| w.id.as_str()).collect::<Vec<_>>(), ["rolling"]);
        assert!((w[0].used_fraction - 0.12).abs() < 1e-6);
    }

    #[test]
    fn an_empty_or_missing_usage_block_draws_nothing() {
        assert!(windows(&reply("opencode-go-empty.json")).is_empty());
        assert!(windows(&decode("{}")).is_empty());
    }

    #[test]
    fn a_status_other_than_ok_is_exhausted_whatever_the_percentage() {
        let w = windows(&reply("opencode-go-blocked.json"));
        let rolling = w.iter().find(|w| w.id == "rolling").unwrap();
        assert!((rolling.used_fraction - 0.08).abs() < 1e-6);
        assert!(rolling.is_exhausted);
    }

    #[test]
    fn a_missing_status_defaults_to_ok() {
        let r = decode(r#"{"usage":{"rolling":{"percent":1,"resetsAt":"2026-09-26T18:00:00.000Z"}}}"#);
        assert!(!windows(&r)[0].is_exhausted);
    }

    #[test]
    fn readings_and_reasons() {
        let ctx = context();
        let account = AccountKey::primary(Provider::OpenCodeGo);
        assert_eq!(reading(b"not json at all", &ctx, &account).unwrap_err(), Unavailability::UnreadableReply);
        assert_eq!(reading(&fixture("opencode-go-empty.json"), &ctx, &account).unwrap_err(), Unavailability::NoLimitsReported);
        let usage = reading(&fixture("opencode-go-normal.json"), &ctx, &account).unwrap();
        assert_eq!(usage.windows.len(), 3);
        assert_eq!(usage.plan, None);
    }

    #[test]
    fn the_saved_key_is_read_from_the_cli_login() {
        assert_eq!(key_from_auth(br#"{"opencode-go":{"type":"api","key":"sk-1"}}"#).as_deref(), Some("sk-1"));
        assert_eq!(key_from_auth(br#"{"opencode-go":{"key":""}}"#), None);
        assert_eq!(key_from_auth(br#"{"other":{"key":"x"}}"#), None);
        assert_eq!(key_from_auth(b"junk"), None);
    }

    #[test]
    fn the_console_limits_are_the_keys_three_windows() {
        let json = r#"{"subscriberUserId":"acc_X","product":"go","paymentMethodKind":"card",
         "access":{"startsAt":"2026-09-14T02:01:39.000Z","endsAt":"2026-10-14T02:01:39.000Z",
          "meters":{
           "fiveHour":{"startsAt":"2026-10-01T10:06:17.039Z","resetsAt":"2026-10-01T15:06:17.039Z","limitMicroCents":"1200000000","usedMicroCents":"641286"},
           "week":{"startsAt":"2026-09-28T00:00:00.000Z","resetsAt":"2026-10-05T00:00:00.000Z","limitMicroCents":"3000000000","usedMicroCents":"3000000000"},
           "month":{"resetsAt":"2026-10-14T02:01:39.000Z","limitMicroCents":"6000000000","usedMicroCents":"1283388501"}}}}"#;
        let status: ConsoleStatus = serde_json::from_str(json).unwrap();
        let w = console_windows(&status);

        // The ids the key route uses, so a pinned limit survives either route.
        assert_eq!(w.iter().map(|w| w.id.as_str()).collect::<Vec<_>>(), ["rolling", "weekly", "monthly"]);
        assert!((w[0].used_fraction - 641_286.0 / 1_200_000_000.0).abs() < 1e-12);
        assert_eq!(w[0].window_seconds, 5 * 3600);
        assert!(w[0].reports_length);
        assert!(!w[0].is_exhausted);
        assert!(w[1].is_exhausted);
        assert_eq!(w[1].used_fraction, 1.0);
        // The month starts with the billing period, which ends with it.
        assert!(w[2].reports_length);
        assert_eq!(w[2].window_seconds, 30 * 86_400);
    }

    #[test]
    fn a_console_reading_names_the_plan_and_the_route_and_a_sign_in_page_is_a_lapsed_session() {
        let ctx = context();
        let account = AccountKey::primary(Provider::OpenCodeGo);
        let json = br#"{"product":"go","access":{"meters":{"fiveHour":{"resetsAt":"2026-10-01T15:06:17.039Z","limitMicroCents":"100","usedMicroCents":"50"}}}}"#;
        let usage = console_reading(json, &ctx, &account);
        assert_eq!(usage.state, UsageState::Live);
        assert_eq!(usage.plan.as_deref(), Some("Go"));
        assert_eq!(usage.origin, Some(UsageRoute::BrowserSession));
        assert_eq!(console_reading(b"<!doctype html>", &ctx, &account).state, UsageState::Unavailable(Unavailability::SessionExpired));
        assert_eq!(console_reading(br#"{"product":null}"#, &ctx, &account).state, UsageState::Unavailable(Unavailability::NoPlan));
        assert_eq!(console_reading(b"nonsense", &ctx, &account).state, UsageState::Unavailable(Unavailability::UnreadableReply));
    }

    #[test]
    fn no_plan_is_no_console_windows() {
        let status: ConsoleStatus = serde_json::from_str(r#"{"product":null}"#).unwrap();
        assert!(console_windows(&status).is_empty());
    }
}
