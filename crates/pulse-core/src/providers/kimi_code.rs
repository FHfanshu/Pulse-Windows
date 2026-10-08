// Ported from upstream Providers/KimiCodeUsageService.swift.
//! Kimi Code: limits from `GET https://api.kimi.com/coding/v1/usages` with the pasted API key.
//!
//! The reply has two kinds of limit that are not the same figure:
//! - `limits[]`: timed windows, each stating a duration and a unit;
//! - `usage`: the weekly allowance, with a reset time and no stated length
//!   (the window rolls), so its seven days only sort it and are not reported.
//!
//! Counts arrive as strings, and a `limits[]` detail may give `remaining`
//! instead of `used`.

use async_trait::async_trait;
use serde::Deserialize;

use super::profile::{self, Fail};
use crate::model::{AccountKey, ProviderUsage, Unavailability, UsageWindow, WindowKind};
use crate::provider::Provider;
use crate::service::{FetchContext, UsageService};

const ENDPOINT: &str = "https://api.kimi.com/coding/v1/usages";

#[derive(Default)]
pub struct KimiCode;

#[async_trait]
impl UsageService for KimiCode {
    fn provider(&self) -> Provider {
        Provider::KimiCode
    }

    async fn fetch(&self, ctx: &FetchContext, account: &AccountKey) -> ProviderUsage {
        let Some(key) = ctx.api_key(account) else {
            return ProviderUsage::unavailable(account.clone(), Unavailability::ApiKeyMissing);
        };
        match profile::get_bearer(ctx, ENDPOINT, key.trim()).await {
            Ok(body) => reading(&body, ctx, account).unwrap_or_else(|reason| ProviderUsage::unavailable(account.clone(), reason)),
            Err(reason) => ProviderUsage::unavailable(account.clone(), reason),
        }
    }
}

#[derive(Deserialize)]
struct Reply {
    user: Option<User>,
    usage: Option<Detail>,
    limits: Option<Vec<Limit>>,
}

#[derive(Deserialize)]
struct User {
    membership: Option<Membership>,
}

#[derive(Deserialize)]
struct Membership {
    level: Option<String>,
}

#[derive(Deserialize)]
struct Detail {
    limit: Option<String>,
    used: Option<String>,
    remaining: Option<String>,
    #[serde(rename = "resetTime")]
    reset_time: Option<String>,
}

#[derive(Deserialize)]
struct Limit {
    window: Option<Length>,
    detail: Option<Detail>,
}

#[derive(Deserialize)]
struct Length {
    duration: Option<i64>,
    #[serde(rename = "timeUnit")]
    time_unit: Option<String>,
}

fn reading(body: &[u8], ctx: &FetchContext, account: &AccountKey) -> Result<ProviderUsage, Fail> {
    let reply: Reply = profile::decode(body)?;
    let windows = windows(&reply);
    if windows.is_empty() {
        return Err(Unavailability::NoLimitsReported);
    }
    let plan = plan_name(reply.user.as_ref().and_then(|u| u.membership.as_ref()).and_then(|m| m.level.as_deref()));
    Ok(profile::reading(account, windows, ctx).with_plan(plan))
}

fn windows(reply: &Reply) -> Vec<UsageWindow> {
    let mut found = Vec::new();

    // Timed windows first, named by the length the service states. The position
    // is in the id too: equal lengths must not collapse into one row.
    for (index, limit) in reply.limits.iter().flatten().enumerate() {
        let Some(seconds) = duration_of(limit.window.as_ref()) else { continue };
        let id = format!("limit.{index}.{seconds}");
        if let Some(window) = window(limit.detail.as_ref(), id, kind_for(seconds), seconds, true) {
            found.push(window);
        }
    }

    // The weekly allowance carries no length; the seconds only sort it.
    if let Some(weekly) = window(reply.usage.as_ref(), "weekly".into(), WindowKind::Weekly, 7 * 86_400, false) {
        found.push(weekly);
    }

    found.sort_by_key(|w| w.window_seconds);
    found
}

fn window(detail: Option<&Detail>, id: String, kind: WindowKind, seconds: i64, reports_length: bool) -> Option<UsageWindow> {
    let detail = detail?;
    let limit = number(detail.limit.as_deref()).filter(|l| *l > 0.0)?;
    // `used` when given, else what the limit and remainder imply.
    let used = number(detail.used.as_deref()).or_else(|| number(detail.remaining.as_deref()).map(|r| limit - r))?;

    let mut window = UsageWindow::new(id, kind, (used / limit).clamp(0.0, 1.0), seconds)
        .with_reset(profile::date(detail.reset_time.as_deref()))
        .exhausted(used >= limit);
    window.reports_length = reports_length;
    Some(window)
}

/// A window's length in seconds, or None for an unknown unit: inventing one
/// would put a figure under a heading that is not true.
fn duration_of(window: Option<&Length>) -> Option<i64> {
    let window = window?;
    let duration = window.duration.filter(|d| *d > 0)?;
    let unit = match window.time_unit.as_deref()? {
        "TIME_UNIT_SECOND" => 1,
        "TIME_UNIT_MINUTE" => 60,
        "TIME_UNIT_HOUR" => 3_600,
        "TIME_UNIT_DAY" => 86_400,
        _ => return None,
    };
    UsageWindow::length(duration, unit)
}

fn kind_for(seconds: i64) -> WindowKind {
    match seconds {
        18_000 => WindowKind::FiveHour,
        604_800 => WindowKind::Weekly,
        2_592_000 => WindowKind::Monthly,
        other => WindowKind::Other(other),
    }
}

/// "LEVEL_INTERMEDIATE" becomes "Intermediate"; unfamiliar tiers pass through tidied.
fn plan_name(level: Option<&str>) -> Option<String> {
    let level = level.filter(|l| !l.is_empty())?;
    let bare = level.strip_prefix("LEVEL_").unwrap_or(level);
    Some(
        bare.split('_')
            .filter(|part| !part.is_empty())
            .map(|part| {
                let mut chars = part.chars();
                let first = chars.next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
                format!("{first}{}", chars.as_str().to_lowercase())
            })
            .collect::<Vec<_>>()
            .join(" "),
    )
}

fn number(text: Option<&str>) -> Option<f64> {
    text?.parse::<f64>().ok().filter(|v| v.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::profile::test_support::fixture;
    use chrono::{TimeZone, Utc};

    fn reply(name: &str) -> Reply {
        serde_json::from_slice(&fixture(name)).unwrap()
    }

    fn decode(json: &str) -> Reply {
        serde_json::from_str(json).unwrap()
    }

    fn level(r: &Reply) -> Option<&str> {
        r.user.as_ref()?.membership.as_ref()?.level.as_deref()
    }

    #[test]
    fn a_normal_reply_sorts_timed_limits_ahead_of_the_weekly_allowance() {
        let r = reply("kimi-code-normal.json");
        let w = windows(&r);
        assert_eq!(w.iter().map(|w| w.id.as_str()).collect::<Vec<_>>(), ["limit.0.18000", "weekly", "limit.1.2592000"]);
        assert_eq!(w.iter().map(|w| w.kind).collect::<Vec<_>>(), [WindowKind::FiveHour, WindowKind::Weekly, WindowKind::Monthly]);

        assert!((w[0].used_fraction - 0.4).abs() < 1e-6);
        assert!(w[0].reports_length);
        assert_eq!(w[0].resets_at, Some(Utc.with_ymd_and_hms(2026, 9, 26, 18, 0, 0).unwrap()));

        assert!((w[1].used_fraction - 0.25).abs() < 1e-6);
        assert!(!w[1].reports_length);
        assert_eq!(w[1].resets_at, Some(Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap()));

        // No `used`: limit - remaining = 5000 - 1500.
        assert!((w[2].used_fraction - 0.7).abs() < 1e-6);
        assert!(w[2].reports_length);

        assert_eq!(plan_name(level(&r)).as_deref(), Some("Intermediate"));
    }

    #[test]
    fn an_unrecognised_time_unit_is_dropped() {
        let w = windows(&reply("kimi-code-normal.json"));
        assert!(!w.iter().any(|w| w.id.starts_with("limit.2")));
        assert_eq!(w.len(), 3);
    }

    #[test]
    fn counts_arrive_as_strings() {
        let r = reply("kimi-code-normal.json");
        assert_eq!(r.usage.as_ref().unwrap().used.as_deref(), Some("250"));
        assert!(windows(&r).iter().all(|w| w.used_fraction > 0.0));
    }

    #[test]
    fn missing_fields_draw_nothing() {
        let r = reply("kimi-code-missing-fields.json");
        assert!(windows(&r).is_empty());
        assert_eq!(plan_name(level(&r)), None);
    }

    #[test]
    fn spent_is_used_at_or_over_the_limit() {
        let w = windows(&reply("kimi-code-exhausted.json"));
        let weekly = w.iter().find(|w| w.id == "weekly").unwrap();
        assert_eq!(weekly.used_fraction, 1.0);
        assert!(weekly.is_exhausted);
        assert_eq!(plan_name(Some("LEVEL_ADVANCED")).as_deref(), Some("Advanced"));
    }

    #[test]
    fn minutes_and_seconds_convert() {
        let r = decode(
            r#"{"limits":[
              {"window":{"duration":90,"timeUnit":"TIME_UNIT_SECOND"},"detail":{"limit":"10","used":"5"}},
              {"window":{"duration":3,"timeUnit":"TIME_UNIT_MINUTE"},"detail":{"limit":"10","used":"5"}}
            ]}"#,
        );
        let w = windows(&r);
        assert_eq!(w.iter().map(|w| w.id.as_str()).collect::<Vec<_>>(), ["limit.0.90", "limit.1.180"]);
        assert_eq!(w.iter().map(|w| w.kind).collect::<Vec<_>>(), [WindowKind::Other(90), WindowKind::Other(180)]);
    }

    #[test]
    fn an_unnamed_window_length_is_kept() {
        let r = decode(r#"{"limits":[{"window":{"duration":3,"timeUnit":"TIME_UNIT_HOUR"},"detail":{"limit":"10","used":"5"}}]}"#);
        assert_eq!(windows(&r).iter().map(|w| w.kind).collect::<Vec<_>>(), [WindowKind::Other(3 * 3_600)]);
    }

    #[test]
    fn a_non_positive_duration_is_dropped() {
        let r = decode(
            r#"{"limits":[
              {"window":{"duration":0,"timeUnit":"TIME_UNIT_HOUR"},"detail":{"limit":"10","used":"5"}},
              {"window":{"duration":-5,"timeUnit":"TIME_UNIT_DAY"},"detail":{"limit":"10","used":"5"}}
            ]}"#,
        );
        assert!(windows(&r).is_empty());
    }

    #[test]
    fn plan_names() {
        assert_eq!(plan_name(Some("LEVEL_INTERMEDIATE")).as_deref(), Some("Intermediate"));
        assert_eq!(plan_name(Some("LEVEL_ADVANCED_PLUS")).as_deref(), Some("Advanced Plus"));
        assert_eq!(plan_name(Some("mystery")).as_deref(), Some("Mystery"));
        assert_eq!(plan_name(Some("")), None);
        assert_eq!(plan_name(None), None);
    }

    #[test]
    fn garbage_is_unreadable_and_an_empty_reply_reports_no_limits() {
        let ctx = crate::providers::profile::test_support::context();
        let account = AccountKey::primary(Provider::KimiCode);
        assert_eq!(reading(b"not json at all", &ctx, &account).unwrap_err(), Unavailability::UnreadableReply);
        assert_eq!(reading(b"{}", &ctx, &account).unwrap_err(), Unavailability::NoLimitsReported);
    }
}
