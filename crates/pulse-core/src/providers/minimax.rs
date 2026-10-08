// Ported from upstream Providers/MiniMaxUsageService.swift (international storefront only).
//! MiniMax Coding Plan: `GET {host}/v1/token_plan/remains` with the key as a bearer
//! token, falling back to the older `/v1/api/openplatform/coding_plan/remains`.
//!
//! Upstream runs the international (`api.minimax.io`) and mainland
//! (`api.minimaxi.com`) storefronts through one service. The shared code takes a
//! `Storefront`, so a `minimaxCN` sibling can reuse it; only the international
//! one is wired here. They are separate accounts: never send one's key to the other.
//!
//! Things about the reply worth knowing:
//! - It reports what is *left*; the inversion to "used" happens here.
//! - Figures arrive as strings or numbers interchangeably.
//! - Lanes the plan does not include come back as status 3, zero counts, 100% left;
//!   they are dropped rather than drawn as a ring pinned at 0%.

use async_trait::async_trait;
use serde_json::{Map, Value};

use super::profile::{self, Fail};
use crate::model::{AccountKey, ProviderUsage, Unavailability, UsageWindow, WindowKind};
use crate::provider::Provider;
use crate::service::{FetchContext, UsageService};

/// Which storefront a request is for: it decides the host and the account the answer belongs to.
#[derive(Debug, Clone, Copy)]
pub struct Storefront {
    pub provider: Provider,
    pub host: &'static str,
}

impl Storefront {
    pub const INTERNATIONAL: Storefront = Storefront { provider: Provider::Minimax, host: "https://api.minimax.io" };

    /// The current path first, then the one it replaced.
    fn endpoints(self) -> [String; 2] {
        [format!("{}/v1/token_plan/remains", self.host), format!("{}/v1/api/openplatform/coding_plan/remains", self.host)]
    }
}

const PLAN_KEYS: [&str; 4] = ["current_subscribe_title", "plan_name", "combo_title", "current_plan_title"];
const BALANCE_KEYS: [&str; 5] = ["points_balance", "point_balance", "credits_balance", "credit_balance", "balance"];

#[derive(Default)]
pub struct Minimax;

#[async_trait]
impl UsageService for Minimax {
    fn provider(&self) -> Provider {
        Provider::Minimax
    }

    async fn fetch(&self, ctx: &FetchContext, account: &AccountKey) -> ProviderUsage {
        fetch_plan(ctx, account, Storefront::INTERNATIONAL).await
    }
}

/// The whole plan read for one storefront, reusable by a sibling provider.
pub async fn fetch_plan(ctx: &FetchContext, account: &AccountKey, shop: Storefront) -> ProviderUsage {
    let Some(key) = ctx.api_key(account) else {
        return ProviderUsage::unavailable(account.clone(), Unavailability::ApiKeyMissing);
    };
    let key = key.trim();

    // Both paths are tried on any failure: an account on the older plan answers
    // 401 on the current path, not 404. The first reason is kept, so a refused
    // key is not masked by a server error from a path the plan does not use.
    let mut first_problem: Option<Fail> = None;
    for endpoint in shop.endpoints() {
        match profile::get_bearer(ctx, &endpoint, key).await {
            Ok(body) => match reading(&body, shop, ctx, account) {
                Ok(usage) => return usage,
                Err(reason) => first_problem = first_problem.or(Some(reason)),
            },
            Err(reason) => first_problem = first_problem.or(Some(reason)),
        }
    }
    ProviderUsage::unavailable(account.clone(), first_problem.unwrap_or(Unavailability::Unreachable))
}

fn reading(body: &[u8], shop: Storefront, ctx: &FetchContext, account: &AccountKey) -> Result<ProviderUsage, Fail> {
    let root: Value = profile::decode(body)?;
    let root = root.as_object().ok_or(Unavailability::UnreadableReply)?;

    // The service's own verdict, which is not the HTTP status.
    if let Some(verdict) = status_verdict(root.get("base_resp").and_then(Value::as_object)) {
        return Err(verdict);
    }

    let payload = payload_of(root);
    let windows = windows(Some(payload), shop.provider);
    if windows.is_empty() {
        return Err(Unavailability::NoLimitsReported);
    }
    let mut usage = profile::reading(account, windows, ctx).with_plan(first(Some(payload), &PLAN_KEYS));
    usage.credit_balance = balance(Some(payload), &BALANCE_KEYS);
    Ok(usage)
}

/// Not every non-zero status is a bad key: 1004 is the credential one, and the
/// message text can say so too. Everything else is the service having a bad day.
fn status_verdict(base: Option<&Map<String, Value>>) -> Option<Unavailability> {
    let status = number(base.and_then(|b| b.get("status_code"))).unwrap_or(0.0);
    if status == 0.0 {
        return None;
    }
    let said = base.and_then(|b| b.get("status_msg")).and_then(Value::as_str).unwrap_or("").to_lowercase();
    let credential = status == 1004.0 || ["token", "auth", "login", "cookie", "credential"].iter().any(|w| said.contains(w));
    Some(if credential { Unavailability::ApiKeyRefused } else { Unavailability::ServerError })
}

/// The payload is not always wrapped: some replies put the fields at the root.
fn payload_of(root: &Map<String, Value>) -> &Map<String, Value> {
    root.get("data").and_then(Value::as_object).unwrap_or(root)
}

fn windows(payload: Option<&Map<String, Value>>, provider: Provider) -> Vec<UsageWindow> {
    let models = payload.and_then(|p| p.get("model_remains")).and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]);
    let mut found = Vec::new();
    for (index, model) in models.iter().enumerate() {
        let Some(model) = model.as_object() else { continue };
        let name = model.get("model_name").and_then(Value::as_str).map(str::trim).filter(|n| !n.is_empty());
        // "general" is the plan itself, not a model: left unscoped.
        let scope = name.filter(|n| n.to_lowercase() != "general");
        // The id must be unique within a reading; the position settles it when the name cannot.
        let key = name.map(String::from).unwrap_or_else(|| format!("lane{index}"));
        found.extend(interval(model, scope, &key, provider));
        found.extend(weekly(model, scope, &key, provider));
    }
    found.sort_by_key(|w| w.window_seconds);
    found
}

fn field(model: &Map<String, Value>, key: &str) -> Option<f64> {
    number(model.get(key))
}

/// The short window. Its length is measured from the timestamps, the only
/// statement of it the reply makes; without them it is dropped, not invented.
fn interval(model: &Map<String, Value>, scope: Option<&str>, key: &str, provider: Provider) -> Option<UsageWindow> {
    let remaining_percent = field(model, "current_interval_remaining_percent");
    let total = field(model, "current_interval_total_count");
    if is_unavailable(field(model, "current_interval_status"), total, remaining_percent) {
        return None;
    }
    let used = spent(remaining_percent, total, field(model, "current_interval_usage_count"))?;
    let (start, end) = (field(model, "start_time")?, field(model, "end_time")?);
    if end <= start {
        return None;
    }
    // Sub-second intervals would floor to zero.
    let seconds = ((end - start) / 1000.0) as i64;
    if seconds <= 0 {
        return None;
    }
    let mut window = UsageWindow::new(format!("{}.{key}.interval", provider.raw()), kind_for(seconds), used / 100.0, seconds)
        .with_reset(millis(end));
    window.scope = scope.map(String::from);
    Some(window)
}

/// The weekly window names its own length, so it survives missing timestamps.
fn weekly(model: &Map<String, Value>, scope: Option<&str>, key: &str, provider: Provider) -> Option<UsageWindow> {
    let remaining_percent = field(model, "current_weekly_remaining_percent");
    let total = field(model, "current_weekly_total_count");
    if is_unavailable(field(model, "current_weekly_status"), total, remaining_percent) {
        return None;
    }
    let used = spent(remaining_percent, total, field(model, "current_weekly_usage_count"))?;
    let mut window = UsageWindow::new(format!("{}.{key}.weekly", provider.raw()), WindowKind::Weekly, used / 100.0, 7 * 86_400)
        .with_reset(field(model, "weekly_end_time").and_then(millis));
    window.scope = scope.map(String::from);
    Some(window)
}

/// A lane the schema has but this subscription does not (or an unlimited one).
fn is_unavailable(status: Option<f64>, total: Option<f64>, remaining_percent: Option<f64>) -> bool {
    status == Some(3.0) && total.unwrap_or(0.0) == 0.0 && remaining_percent.unwrap_or(0.0) >= 100.0
}

/// How much of the lane is gone, 0...100. Everything is stated as what is left:
/// the percentage, and despite its name `*_usage_count` too. The percentage is
/// preferred; the counts are the older endpoint's fallback.
fn spent(percent_remaining: Option<f64>, total: Option<f64>, left: Option<f64>) -> Option<f64> {
    if let Some(p) = percent_remaining {
        return Some((100.0 - p).clamp(0.0, 100.0));
    }
    let total = total.filter(|t| *t > 0.0)?;
    Some(((total - left?) / total * 100.0).clamp(0.0, 100.0))
}

fn millis(ms: f64) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::from_timestamp_millis(ms as i64)
}

fn kind_for(seconds: i64) -> WindowKind {
    match seconds {
        18_000 => WindowKind::FiveHour,
        604_800 => WindowKind::Weekly,
        2_592_000 => WindowKind::Monthly,
        other => WindowKind::Other(other),
    }
}

/// The first of several names that carries a usable string; which one varies by account.
fn first(payload: Option<&Map<String, Value>>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .filter_map(|k| payload?.get(*k)?.as_str())
        .map(str::trim)
        .find(|s| !s.is_empty())
        .map(String::from)
}

fn balance(payload: Option<&Map<String, Value>>, keys: &[&str]) -> Option<String> {
    let points = keys.iter().filter_map(|k| number(payload?.get(*k))).find(|p| *p > 0.0)?;
    Some(format!("{} points", group_thousands(points as i64)))
}

fn group_thousands(n: i64) -> String {
    let digits = n.abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 { format!("-{out}") } else { out }
}

/// Every figure here can be a string or a number, sometimes in the same field
/// across replies, so nothing may assume which.
fn number(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
    .filter(|v| v.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::profile::test_support::fixture;
    use serde_json::json;

    fn root(name: &str) -> Map<String, Value> {
        serde_json::from_slice::<Value>(&fixture(name)).unwrap().as_object().unwrap().clone()
    }

    fn windows_of(name: &str) -> Vec<UsageWindow> {
        let root = root(name);
        windows(Some(payload_of(&root)), Provider::Minimax)
    }

    #[test]
    fn a_normal_reply_gives_the_plan_its_windows_shortest_first() {
        let w = windows_of("minimax-normal.json");
        assert_eq!(
            w.iter().map(|w| w.id.as_str()).collect::<Vec<_>>(),
            ["minimax.general.interval", "minimax.general.weekly", "minimax.creative-writer.interval"]
        );
        assert_eq!(w[0].scope, None);
        assert_eq!(w[2].scope.as_deref(), Some("creative-writer"));
    }

    #[test]
    fn remaining_percent_is_inverted_string_or_number_alike() {
        let w = windows_of("minimax-normal.json");
        let general = w.iter().find(|w| w.id == "minimax.general.interval").unwrap();
        let weekly = w.iter().find(|w| w.id == "minimax.general.weekly").unwrap();
        assert!((general.used_fraction - 0.04).abs() < 1e-6);
        assert!((weekly.used_fraction - 0.25).abs() < 1e-6);
    }

    #[test]
    fn the_intervals_length_is_measured_from_its_timestamps() {
        let w = windows_of("minimax-normal.json");
        let general = w.iter().find(|w| w.id == "minimax.general.interval").unwrap();
        let creative = w.iter().find(|w| w.id == "minimax.creative-writer.interval").unwrap();
        assert_eq!(general.kind, WindowKind::FiveHour);
        assert_eq!(general.window_seconds, 5 * 3_600);
        assert_eq!(creative.kind, WindowKind::Monthly);
        assert_eq!(creative.window_seconds, 30 * 86_400);
    }

    #[test]
    fn the_weekly_window_names_its_own_length_and_reset() {
        let w = windows_of("minimax-normal.json");
        let weekly = w.iter().find(|w| w.id == "minimax.general.weekly").unwrap();
        assert_eq!(weekly.kind, WindowKind::Weekly);
        assert_eq!(weekly.window_seconds, 7 * 86_400);
        assert_eq!(weekly.resets_at, chrono::DateTime::from_timestamp(1_767_830_400, 0));
        assert!(weekly.reports_length);
    }

    #[test]
    fn a_lane_the_subscription_does_not_include_draws_nothing() {
        assert!(!windows_of("minimax-normal.json").iter().any(|w| w.scope.as_deref() == Some("video")));
    }

    #[test]
    fn the_older_endpoints_usage_count_is_what_remains() {
        let w = windows_of("minimax-old-endpoint-unwrapped.json");
        // 100 total, 80 left: 20 spent.
        assert!((w[0].used_fraction - 0.20).abs() < 1e-6);
    }

    #[test]
    fn a_reply_with_no_data_wrapper_is_still_read() {
        let root = root("minimax-old-endpoint-unwrapped.json");
        assert!(payload_of(&root).contains_key("model_remains"));
    }

    #[test]
    fn status_zero_is_no_problem() {
        let ok = json!({"status_code": 0});
        assert_eq!(status_verdict(ok.as_object()), None);
        assert_eq!(status_verdict(None), None);
    }

    #[test]
    fn status_1004_is_a_refused_key_and_others_a_server_problem() {
        assert_eq!(status_verdict(json!({"status_code": 1004}).as_object()), Some(Unavailability::ApiKeyRefused));
        assert_eq!(
            status_verdict(json!({"status_code": 2013, "status_msg": "internal error"}).as_object()),
            Some(Unavailability::ServerError)
        );
    }

    #[test]
    fn a_status_message_naming_a_credential_is_read_as_one() {
        let base = json!({"status_code": 2049, "status_msg": "invalid API token"});
        assert_eq!(status_verdict(base.as_object()), Some(Unavailability::ApiKeyRefused));
    }

    #[test]
    fn the_plan_name_is_the_first_field_that_carries_one() {
        let payload = json!({"plan_name": "", "combo_title": "  ", "current_plan_title": "Coding Plan"});
        assert_eq!(first(payload.as_object(), &PLAN_KEYS).as_deref(), Some("Coding Plan"));
        let root = root("minimax-normal.json");
        assert_eq!(first(Some(payload_of(&root)), &PLAN_KEYS).as_deref(), Some("Pro Plan"));
    }

    #[test]
    fn the_credit_balance_reads_a_string_figure() {
        let root = root("minimax-normal.json");
        assert_eq!(balance(Some(payload_of(&root)), &BALANCE_KEYS).as_deref(), Some("12,345 points"));
    }

    #[test]
    fn a_balance_of_zero_or_absent_draws_no_figure() {
        assert_eq!(balance(json!({"points_balance": 0}).as_object(), &["points_balance"]), None);
        assert_eq!(balance(json!({}).as_object(), &["points_balance"]), None);
    }

    #[test]
    fn the_sibling_storefront_gets_its_own_id_prefix() {
        let root = root("minimax-normal.json");
        let sibling = windows(Some(payload_of(&root)), Provider::Zai);
        assert!(sibling[0].id.starts_with("zai."));
        assert!(windows_of("minimax-normal.json")[0].id.starts_with("minimax."));
    }

    #[test]
    fn a_reply_with_no_model_remains_produces_no_windows() {
        assert!(windows(json!({}).as_object(), Provider::Minimax).is_empty());
        assert!(windows(None, Provider::Minimax).is_empty());
    }

    #[test]
    fn whole_replies_become_readings_or_reasons() {
        let ctx = profile::test_support::context();
        let account = AccountKey::primary(Provider::Minimax);
        let shop = Storefront::INTERNATIONAL;
        let usage = reading(&fixture("minimax-normal.json"), shop, &ctx, &account).unwrap();
        assert_eq!(usage.plan.as_deref(), Some("Pro Plan"));
        assert_eq!(usage.credit_balance.as_deref(), Some("12,345 points"));
        assert_eq!(reading(b"nope", shop, &ctx, &account).unwrap_err(), Unavailability::UnreadableReply);
        assert_eq!(reading(br#"{"base_resp":{"status_code":1004}}"#, shop, &ctx, &account).unwrap_err(), Unavailability::ApiKeyRefused);
        assert_eq!(reading(br#"{"data":{"model_remains":[]}}"#, shop, &ctx, &account).unwrap_err(), Unavailability::NoLimitsReported);
    }
}
