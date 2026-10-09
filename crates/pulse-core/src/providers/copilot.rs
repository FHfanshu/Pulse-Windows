// Ported from upstream Providers/CopilotUsageService.swift.
//! GitHub Copilot's quotas, from the endpoint its editor plugins use:
//! `GET api.github.com/copilot_internal/user` with a GitHub OAuth token sent as
//! `Authorization: token …` (the older scheme; Bearer is refused) and the editor
//! headers the plugins send. Not public API, and it can change without notice.
//!
//! The reply reports what is *left*; the inversion happens in `window`, so
//! everything downstream reads what is gone.
//!
//! Token sources on Windows, in order: the key saved in Pulse, then `gh auth token`,
//! then the Copilot editor plugin's `oauth_token` in `%LOCALAPPDATA%\github-copilot`.

use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use serde_json::{Map, Value};

use super::profile::{self, Fail};
use crate::model::{AccountKey, ProviderUsage, Unavailability, UsageWindow, WindowKind};
use crate::provider::Provider;
use crate::service::{FetchContext, UsageService};

const ENDPOINT: &str = "https://api.github.com/copilot_internal/user";

/// The three quotas, in the order they are worth reading. `completions` is
/// included: on a free plan it is the largest allowance of the three.
const LANES: [(&str, &str); 3] = [
    ("premium_interactions", "Premium requests"),
    ("chat", "Chat"),
    ("completions", "Completions"),
];

/// Seconds used as a sort key only; a calendar month is 28 to 31 days, so
/// `reports_length` is false and the clock arc must not divide by it.
const MONTH_SORT_SECONDS: i64 = 30 * 86_400;

#[derive(Default)]
pub struct Copilot;

#[async_trait]
impl UsageService for Copilot {
    fn provider(&self) -> Provider {
        Provider::Copilot
    }

    async fn fetch(&self, ctx: &FetchContext, account: &AccountKey) -> ProviderUsage {
        let Some(token) = token(ctx, account).await else {
            return ProviderUsage::unavailable(account.clone(), Unavailability::SignedOut);
        };
        // "token", not "Bearer": this endpoint takes the OAuth token in GitHub's
        // older scheme. The editor headers are sent because the endpoint answers
        // a plugin as one; dropping them has been reported to change the reply.
        let request = ctx
            .http
            .get(ENDPOINT)
            .header("Authorization", format!("token {token}"))
            .header("Accept", "application/json")
            .header("X-Github-Api-Version", "2025-04-01")
            .header("Editor-Version", "vscode/1.96.2")
            .header("Editor-Plugin-Version", "copilot-chat/0.26.7")
            .timeout(Duration::from_secs(15));

        match profile::send(ctx, request, Unavailability::SignedOut).await {
            Ok(body) => reading(&body, account, ctx).unwrap_or_else(|reason| ProviderUsage::unavailable(account.clone(), reason)),
            Err(reason) => ProviderUsage::unavailable(account.clone(), reason),
        }
    }
}

/// Upstream reads only the token Pulse saved (GitHub device sign-in or a pasted
/// token). `gh`'s token is deliberately not borrowed: it carries repo and
/// workflow scope this app has no business holding.
async fn token(ctx: &FetchContext, account: &AccountKey) -> Option<String> {
    ctx.api_key(account).map(|k| k.trim().to_string()).filter(|k| !k.is_empty())
}

/// Maps a 200 reply to a reading. Split from the network so tests can drive it.
fn reading(body: &[u8], account: &AccountKey, ctx: &FetchContext) -> Result<ProviderUsage, Fail> {
    let root: Map<String, Value> = profile::decode(body)?;
    let windows = windows(&root);
    if windows.is_empty() {
        return Err(Unavailability::NoLimitsReported);
    }
    Ok(ProviderUsage::live(account.clone(), windows, ctx.now).with_plan(plan_name(root.get("copilot_plan").and_then(Value::as_str))))
}

/// The lanes the account has, in display order. Internal to the module so tests can drive it.
fn windows(root: &Map<String, Value>) -> Vec<UsageWindow> {
    let Some(snapshots) = root.get("quota_snapshots").and_then(Value::as_object) else {
        return Vec::new();
    };
    // One reset for the account, not one per quota: the per-snapshot
    // `quota_reset_at` comes back as 0. It lands on the first of a month.
    let resets = text(root.get("quota_reset_date_utc"))
        .and_then(parse_date)
        .or_else(|| text(root.get("quota_reset_date")).and_then(parse_date));

    LANES
        .iter()
        .filter_map(|(key, scope)| {
            let snapshot = snapshots.get(*key)?.as_object()?;
            window(snapshot, scope, key, resets)
        })
        .collect()
}

fn window(snapshot: &Map<String, Value>, scope: &str, key: &str, resets: Option<DateTime<Utc>>) -> Option<UsageWindow> {
    let entitlement = number(snapshot.get("entitlement")).unwrap_or(0.0);
    let unlimited = snapshot.get("unlimited").and_then(Value::as_bool).unwrap_or(false);
    let remaining_count = number(snapshot.get("remaining")).unwrap_or(0.0);
    let percent_remaining = number(snapshot.get("percent_remaining"));

    // A quota the plan does not include is left out, not drawn at 100%. It comes
    // back with `has_quota: false`, nothing issued and `percent_remaining: 0`.
    let has_quota = snapshot.get("has_quota").and_then(Value::as_bool);
    if has_quota == Some(false) {
        return None;
    }

    // Without that flag, an older reply is told apart by the percentage: a lane
    // the plan excludes reads 100% remaining with nothing issued. A lane that has
    // run out also has nothing left, and must not be dropped.
    if has_quota.is_none()
        && !unlimited
        && entitlement <= 0.0
        && remaining_count <= 0.0
        && percent_remaining.unwrap_or(0.0) >= 100.0
    {
        return None;
    }

    // An unlimited lane has no share to show, so there is no ring to draw.
    if unlimited {
        return None;
    }
    let remaining = percent_remaining?;

    let mut usage = UsageWindow::new(
        format!("copilot.{key}"),
        WindowKind::Monthly,
        ((100.0 - remaining) / 100.0).clamp(0.0, 1.0),
        MONTH_SORT_SECONDS,
    )
    .with_reset(resets)
    .with_scope(scope);
    usage.reports_length = false;
    // Spent is not over the allowance: a lane with overage permitted keeps working
    // past its included share and is billed for it, so it is not exhausted.
    let overage_permitted = snapshot.get("overage_permitted").and_then(Value::as_bool).unwrap_or(false);
    Some(usage.exhausted(remaining <= 0.0 && !overage_permitted))
}

/// GitHub's internal plan names, tidied. Unknown names pass through rather than blanking.
fn plan_name(raw: Option<&str>) -> Option<String> {
    let raw = raw?.trim();
    if raw.is_empty() {
        return None;
    }
    Some(match raw.to_lowercase().as_str() {
        "individual" => "Individual".to_string(),
        "free" => "Free".to_string(),
        "business" => "Business".to_string(),
        "enterprise" => "Enterprise".to_string(),
        _ => raw.to_string(),
    })
}

/// ISO-8601 with an offset, or the bare `yyyy-MM-dd` older replies give (midnight UTC).
fn parse_date(text: &str) -> Option<DateTime<Utc>> {
    if let Some(date) = profile::date(Some(text)) {
        return Some(date);
    }
    let day = NaiveDate::parse_from_str(text.trim(), "%Y-%m-%d").ok()?;
    Some(Utc.from_utc_datetime(&day.and_hms_opt(0, 0, 0)?))
}

fn text(value: Option<&Value>) -> Option<&str> {
    value.and_then(Value::as_str)
}

/// Figures have arrived as both `200` and `200.0`, and sometimes as strings.
fn number(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::profile::test_support::{context, fixture};

    fn account() -> AccountKey {
        AccountKey::primary(Provider::Copilot)
    }

    fn root(name: &str) -> Map<String, Value> {
        serde_json::from_slice(&fixture(name)).expect("fixture is a JSON object")
    }

    fn reset_date() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap()
    }

    #[test]
    fn a_normal_reply_becomes_three_quotas_inverted_from_what_is_left() {
        let root = root("copilot-normal.json");
        let windows = windows(&root);

        let ids: Vec<&str> = windows.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(ids, ["copilot.premium_interactions", "copilot.chat", "copilot.completions"]);
        let scopes: Vec<Option<&str>> = windows.iter().map(|w| w.scope.as_deref()).collect();
        assert_eq!(scopes, [Some("Premium requests"), Some("Chat"), Some("Completions")]);
        assert!(windows.iter().all(|w| w.kind == WindowKind::Monthly));
        assert!(windows.iter().all(|w| !w.reports_length));
        assert!(windows.iter().all(|w| w.resets_at == Some(reset_date())));

        // percent_remaining 70/90/95 means 30%/10%/5% gone.
        assert!((windows[0].used_fraction - 0.30).abs() < 1e-6);
        assert!((windows[1].used_fraction - 0.10).abs() < 1e-6);
        assert!((windows[2].used_fraction - 0.05).abs() < 1e-6);
        assert!(windows.iter().all(|w| !w.is_exhausted));

        assert_eq!(plan_name(root["copilot_plan"].as_str()).as_deref(), Some("Individual"));
    }

    #[test]
    fn an_unissued_quota_is_dropped_a_spent_one_is_not_and_an_unlimited_one_has_no_ring() {
        let windows = windows(&root("copilot-unissued-and-spent.json"));
        let ids: Vec<&str> = windows.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(ids, ["copilot.chat"]);
        assert_eq!(windows[0].used_fraction, 1.0);
        assert!(windows[0].is_exhausted);
    }

    #[test]
    fn a_lane_with_overage_permitted_is_not_read_as_spent() {
        let windows = windows(&root("copilot-overage.json"));
        let ids: Vec<&str> = windows.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(ids, ["copilot.chat"]);
        assert_eq!(windows[0].used_fraction, 1.0);
        assert!(!windows[0].is_exhausted);
    }

    #[test]
    fn without_has_quota_a_placeholder_lane_is_dropped_and_a_spent_one_is_not() {
        let windows = windows(&root("copilot-older-shape.json"));
        let ids: Vec<&str> = windows.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(ids, ["copilot.completions"]);
        assert_eq!(windows[0].used_fraction, 1.0);
        assert!(windows[0].is_exhausted);
        assert_eq!(windows[0].resets_at, Some(reset_date()), "the bare yyyy-MM-dd reset date still parses");
    }

    #[test]
    fn an_empty_reply_draws_nothing_and_has_no_plan() {
        let root = root("copilot-empty.json");
        assert!(windows(&root).is_empty());
        assert_eq!(plan_name(root.get("copilot_plan").and_then(Value::as_str)), None);
    }

    #[test]
    fn a_reply_without_quota_snapshots_is_the_same_as_an_empty_one() {
        assert!(windows(&Map::new()).is_empty());
        let mut only_plan = Map::new();
        only_plan.insert("copilot_plan".into(), Value::String("individual".into()));
        assert!(windows(&only_plan).is_empty());
    }

    #[test]
    fn known_plan_names_are_tidied_and_unfamiliar_ones_pass_through() {
        assert_eq!(plan_name(Some("individual")).as_deref(), Some("Individual"));
        assert_eq!(plan_name(Some("FREE")).as_deref(), Some("Free"));
        assert_eq!(plan_name(Some("business")).as_deref(), Some("Business"));
        assert_eq!(plan_name(Some("enterprise")).as_deref(), Some("Enterprise"));
        assert_eq!(plan_name(Some("  business  ")).as_deref(), Some("Business"));
        assert_eq!(plan_name(Some("MysteryPlan")).as_deref(), Some("MysteryPlan"));
        assert_eq!(plan_name(Some("")), None);
        assert_eq!(plan_name(None), None);
    }

    #[test]
    fn a_reading_carries_the_plan_and_an_empty_reply_is_no_limits_reported() {
        let ctx = context();
        let usage = reading(&fixture("copilot-normal.json"), &account(), &ctx).unwrap();
        assert_eq!(usage.windows.len(), 3);
        assert_eq!(usage.plan.as_deref(), Some("Individual"));
        assert_eq!(usage.observed_at, Some(ctx.now));

        assert_eq!(
            reading(&fixture("copilot-empty.json"), &account(), &ctx).unwrap_err(),
            Unavailability::NoLimitsReported
        );
    }

    #[test]
    fn a_reply_that_is_not_a_json_object_is_unreadable() {
        let ctx = context();
        assert_eq!(reading(b"<html>rate limited</html>", &account(), &ctx).unwrap_err(), Unavailability::UnreadableReply);
        assert_eq!(reading(b"[1,2]", &account(), &ctx).unwrap_err(), Unavailability::UnreadableReply);
    }


}
