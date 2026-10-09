// Ported from upstream Providers/ZaiUsageService.swift (quota route only).
//! Z.ai (international GLM Coding Plan): `GET {host}/api/monitor/usage/quota/limit`
//! with the key as a bearer token.
//!
//! Upstream runs Z.ai and BigModel (mainland) through one service that differs
//! only by host and provider. The shared code takes a `Storefront`, so a sibling
//! (`glmCoding`, host `open.bigmodel.cn`) can reuse it; only Z.ai is wired here.
//! The two are separate accounts: a key for one is refused by the other and
//! must never be sent to the other's host.
//!
//! The reply wraps its payload in an envelope of its own (`success`, `code`),
//! so a refused key can arrive as HTTP 200 with `success: false`.
//!
//! The token-history statistics (`/api/monitor/usage/model-usage`) are `zai_history`.

use async_trait::async_trait;
use serde::Deserialize;

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
    pub const ZAI: Storefront = Storefront { provider: Provider::Zai, host: "https://api.z.ai" };

    fn endpoint(self) -> String {
        format!("{}/api/monitor/usage/quota/limit", self.host)
    }
}

#[derive(Default)]
pub struct Zai;

#[async_trait]
impl UsageService for Zai {
    fn provider(&self) -> Provider {
        Provider::Zai
    }

    async fn fetch(&self, ctx: &FetchContext, account: &AccountKey) -> ProviderUsage {
        fetch_quota(ctx, account, Storefront::ZAI).await
    }
}

/// The whole quota read for one storefront, reusable by a sibling provider.
pub async fn fetch_quota(ctx: &FetchContext, account: &AccountKey, shop: Storefront) -> ProviderUsage {
    // Upstream also looks for a key file for the mainland plan only; never for Z.ai.
    let Some(key) = ctx.api_key(account) else {
        return ProviderUsage::unavailable(account.clone(), Unavailability::ApiKeyMissing);
    };
    match profile::get_bearer(ctx, &shop.endpoint(), key.trim()).await {
        Ok(body) => reading(&body, shop, ctx, account).unwrap_or_else(|reason| ProviderUsage::unavailable(account.clone(), reason)),
        Err(reason) => ProviderUsage::unavailable(account.clone(), reason),
    }
}

#[derive(Deserialize)]
struct Reply {
    /// The service's own verdict, separate from the HTTP status.
    success: Option<bool>,
    code: Option<i64>,
    msg: Option<String>,
    data: Option<Payload>,
}

#[derive(Deserialize)]
struct Payload {
    limits: Option<Vec<Limit>>,
    // The plan name lives under whichever of these keys the tier uses.
    #[serde(rename = "planName")]
    plan_name: Option<String>,
    plan: Option<String>,
    plan_type: Option<String>,
    #[serde(rename = "packageName")]
    package_name: Option<String>,
    level: Option<String>,
}

impl Payload {
    fn plan_label(&self) -> Option<String> {
        [&self.plan_name, &self.plan, &self.plan_type, &self.package_name, &self.level]
            .into_iter()
            .filter_map(|v| v.as_deref().map(|s| s.trim().to_string()))
            .find(|s| !s.is_empty())
    }
}

/// Counts are `f64` so a service that starts reporting `12.5` for `12` does not blank the ring.
#[derive(Deserialize)]
struct Limit {
    #[serde(rename = "type")]
    kind: Option<String>,
    unit: Option<i64>,
    number: Option<i64>,
    percentage: Option<f64>,
    usage: Option<f64>,
    #[serde(rename = "currentValue")]
    current_value: Option<f64>,
    remaining: Option<f64>,
    #[serde(rename = "nextResetTime")]
    next_reset_time: Option<f64>,
}

fn reading(body: &[u8], shop: Storefront, ctx: &FetchContext, account: &AccountKey) -> Result<ProviderUsage, Fail> {
    let reply: Reply = profile::decode(body)?;
    // A refused key is a perfectly good HTTP 200; the envelope is the only place it shows.
    if reply.success != Some(true) || reply.code != Some(200) {
        return Err(problem(&reply));
    }
    let limits = reply.data.as_ref().and_then(|d| d.limits.as_deref()).unwrap_or(&[]);
    let windows = windows(limits, shop.provider);
    if windows.is_empty() {
        return Err(Unavailability::NoLimitsReported);
    }
    Ok(profile::reading(account, windows, ctx).with_plan(reply.data.as_ref().and_then(Payload::plan_label)))
}

/// What the envelope's refusal actually was. Not every refusal is about the key:
/// reporting a 500 or a rate limit as a bad key sends people to check a good one.
fn problem(reply: &Reply) -> Unavailability {
    let said = reply.msg.as_deref().unwrap_or("").to_lowercase();

    // First, because the code is useless here: a working key with no running
    // subscription answers the vendor's generic 500 with this sentence.
    if said.contains("coding plan") {
        return Unavailability::ZaiNoCodingPlan;
    }

    // English from api.z.ai, Chinese from the mainland host. Matched as text
    // because the vendor's code list is private and incomplete.
    const AUTH_WORDS: [&str; 13] = [
        "token", "auth", "key", "unauthor", "forbidden", "credential", "身份验证", "鉴权", "认证", "令牌", "未授权", "无权限", "密钥",
    ];
    if AUTH_WORDS.iter().any(|w| said.contains(w)) {
        return Unavailability::ApiKeyRefused;
    }

    match reply.code.unwrap_or(0) {
        401 | 403 => Unavailability::ApiKeyRefused,
        429 => Unavailability::RateLimited,
        // Zhipu's 1000-series is authentication; sending someone to check
        // their key is the better mistake than calling a working service broken.
        1000..=1099 => Unavailability::ApiKeyRefused,
        _ => Unavailability::ServerError,
    }
}

fn windows(limits: &[Limit], provider: Provider) -> Vec<UsageWindow> {
    let mut found: Vec<UsageWindow> =
        limits.iter().enumerate().filter_map(|(index, limit)| window(limit, index, provider)).collect();
    // Shortest first, so a five-hour limit is read before a weekly one.
    found.sort_by_key(|w| w.window_seconds);
    found
}

fn window(limit: &Limit, index: usize, provider: Provider) -> Option<UsageWindow> {
    // Only these three carry a quota; anything else is left out, not guessed at.
    let kind = limit.kind.as_deref().filter(|t| matches!(*t, "TOKENS_LIMIT" | "CREDIT_LIMIT" | "TIME_LIMIT"))?;
    let (unit, number) = (limit.unit?, limit.number?);
    let minutes = minutes(unit, number, kind)?;
    let used = used_percent(limit)?;

    let mut window = UsageWindow::new(
        // The index is in the id: two limits can share a type and a duration.
        format!("{}.{kind}.{unit}-{number}.{index}", provider.raw()),
        window_kind(minutes),
        used / 100.0,
        minutes * 60,
    )
    .with_reset(limit.next_reset_time.and_then(|ms| chrono::DateTime::from_timestamp_millis(ms as i64)));
    // The MCP lane is a different allowance from the coding quota.
    if kind == "TIME_LIMIT" {
        window = window.with_scope("MCP");
    }
    Some(window)
}

/// How long the window runs, or None for an unknown unit (never an invented length).
fn minutes(unit: i64, number: i64, kind: &str) -> Option<i64> {
    // A monthly MCP allowance is reported as "1 minute": a marker, not a duration.
    if kind == "TIME_LIMIT" && unit == 5 && number == 1 {
        return Some(30 * 24 * 60);
    }
    let per_unit = match unit {
        1 => 1440,
        3 => 60,
        5 => 1,
        6 => 10_080,
        _ => return None,
    };
    UsageWindow::length(number, per_unit * 60).map(|s| s / 60)
}

fn window_kind(minutes: i64) -> WindowKind {
    match minutes {
        300 => WindowKind::FiveHour,
        10_080 => WindowKind::Weekly,
        43_200 => WindowKind::Monthly,
        other => WindowKind::Other(other * 60),
    }
}

/// How much of the limit is gone, 0...100. Counts are finer than the whole-number
/// `percentage`, so they win when present; a limit with no figure at all is None,
/// never zero.
fn used_percent(limit: &Limit) -> Option<f64> {
    if let Some(usage) = limit.usage.filter(|u| *u > 0.0) {
        let used = match (limit.remaining, limit.current_value) {
            // `currentValue` is the spend itself; `remaining` is what is left.
            (Some(remaining), current) => Some((usage - remaining).max(current.unwrap_or(usage - remaining))),
            (None, Some(current)) => Some(current),
            (None, None) => None,
        };
        if let Some(used) = used {
            return Some((used.min(usage) / usage * 100.0).clamp(0.0, 100.0));
        }
    }
    limit.percentage.map(|p| p.clamp(0.0, 100.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::profile::test_support::{context, fixture};

    fn account() -> AccountKey {
        AccountKey::primary(Provider::Zai)
    }

    fn envelope(code: Option<i64>, msg: Option<&str>) -> Reply {
        Reply { success: Some(false), code, msg: msg.map(String::from), data: None }
    }

    fn quota_reply() -> Reply {
        serde_json::from_slice(&fixture("glm-coding-plan-quota.json")).unwrap()
    }

    fn quota_windows() -> Vec<UsageWindow> {
        windows(quota_reply().data.unwrap().limits.as_deref().unwrap(), Provider::Zai)
    }

    #[test]
    fn a_key_of_the_wrong_shape() {
        assert_eq!(problem(&envelope(Some(401), Some("令牌已过期或验证不正确"))), Unavailability::ApiKeyRefused);
    }

    #[test]
    fn a_well_formed_key_the_host_does_not_know() {
        assert_eq!(problem(&envelope(Some(1000), Some("身份验证失败。"))), Unavailability::ApiKeyRefused);
    }

    #[test]
    fn no_authorization_header_reached_the_server() {
        let said = "Header中未收到Authorization参数，无法进行身份验证。";
        assert_eq!(problem(&envelope(Some(1001), Some(said))), Unavailability::ApiKeyRefused);
    }

    #[test]
    fn the_international_host_says_the_same_in_english() {
        assert_eq!(problem(&envelope(Some(401), Some("token expired or incorrect"))), Unavailability::ApiKeyRefused);
        assert_eq!(problem(&envelope(Some(999), Some("invalid api key"))), Unavailability::ApiKeyRefused);
    }

    #[test]
    fn chinese_wording_carries_when_the_code_is_unknown() {
        for said in ["鉴权失败", "认证信息有误", "未授权的请求", "密钥无效", "无权限访问该接口"] {
            assert_eq!(problem(&envelope(Some(12_345), Some(said))), Unavailability::ApiKeyRefused, "missed: {said}");
        }
    }

    #[test]
    fn a_key_that_works_on_an_account_with_no_plan_is_not_a_fault() {
        assert_eq!(problem(&envelope(Some(500), Some("当前用户不存在coding plan"))), Unavailability::ZaiNoCodingPlan);
        assert_eq!(problem(&envelope(Some(500), Some("user has no coding plan"))), Unavailability::ZaiNoCodingPlan);
    }

    #[test]
    fn rate_limiting_and_server_faults_are_still_told_apart() {
        assert_eq!(problem(&envelope(Some(429), Some("too many requests"))), Unavailability::RateLimited);
        assert_eq!(problem(&envelope(Some(500), Some("内部错误"))), Unavailability::ServerError);
        assert_eq!(problem(&envelope(None, None)), Unavailability::ServerError);
    }

    #[test]
    fn both_limits_are_read_shortest_first() {
        let w = quota_windows();
        assert_eq!(w.len(), 2);
        assert_eq!(w.iter().map(|w| w.kind).collect::<Vec<_>>(), [WindowKind::FiveHour, WindowKind::Weekly]);
    }

    #[test]
    fn unit_and_number_are_a_real_duration() {
        let w = quota_windows();
        assert_eq!(w[0].window_seconds, 5 * 3_600);
        assert_eq!(w[1].window_seconds, 7 * 86_400);
        assert!(w.iter().all(|w| w.reports_length));
    }

    #[test]
    fn an_untouched_plan_reads_as_nothing_used() {
        let w = quota_windows();
        assert!(w.iter().all(|w| w.used_fraction == 0.0));
        assert!(w.iter().all(|w| !w.is_exhausted));
    }

    #[test]
    fn the_reset_stamp_is_milliseconds_and_only_one_limit_has_one() {
        let w = quota_windows();
        assert_eq!(w[0].resets_at, None);
        assert_eq!(w[1].resets_at, chrono::DateTime::from_timestamp_millis(1_789_373_585_999));
    }

    #[test]
    fn the_tier_is_named_from_level() {
        assert_eq!(quota_reply().data.unwrap().plan_label().as_deref(), Some("lite"));
    }

    #[test]
    fn ids_are_unique() {
        let w = quota_windows();
        let ids: std::collections::BTreeSet<_> = w.iter().map(|w| w.id.clone()).collect();
        assert_eq!(ids.len(), w.len());
        assert!(w[0].id.starts_with("zai.CREDIT_LIMIT."));
    }

    #[test]
    fn a_limit_whose_number_overflows_its_unit_is_dropped() {
        let json = format!(
            r#"{{"code":200,"data":{{"limits":[
              {{"type":"TOKENS_LIMIT","unit":1,"number":{},"percentage":10}},
              {{"type":"TOKENS_LIMIT","unit":3,"number":5,"percentage":10}}
            ]}},"success":true}}"#,
            i64::MAX
        );
        let reply: Reply = serde_json::from_str(&json).unwrap();
        let w = windows(reply.data.unwrap().limits.as_deref().unwrap(), Provider::Zai);
        assert_eq!(w.iter().map(|w| w.window_seconds).collect::<Vec<_>>(), [5 * 3_600]);
    }

    #[test]
    fn the_mcp_lane_is_scoped_and_a_figureless_limit_is_dropped() {
        let json = r#"{"code":200,"success":true,"data":{"limits":[
          {"type":"TIME_LIMIT","unit":5,"number":1,"percentage":40},
          {"type":"TOKENS_LIMIT","unit":3,"number":5},
          {"type":"OTHER_LIMIT","unit":3,"number":5,"percentage":5}
        ]}}"#;
        let ctx = context();
        let usage = reading(json.as_bytes(), Storefront::ZAI, &ctx, &account()).unwrap();
        assert_eq!(usage.windows.len(), 1);
        assert_eq!(usage.windows[0].scope.as_deref(), Some("MCP"));
        assert_eq!(usage.windows[0].kind, WindowKind::Monthly);
        assert!((usage.windows[0].used_fraction - 0.4).abs() < 1e-9);
    }

    #[test]
    fn counts_win_over_the_whole_number_percentage() {
        let limit = Limit {
            kind: None,
            unit: None,
            number: None,
            percentage: Some(10.0),
            usage: Some(1000.0),
            current_value: Some(125.0),
            remaining: Some(900.0),
            next_reset_time: None,
        };
        assert_eq!(used_percent(&limit), Some(12.5));
    }

    #[test]
    fn the_envelope_decides_not_the_http_status() {
        let ctx = context();
        let refused = br#"{"success":false,"code":1000,"msg":"x"}"#;
        assert_eq!(reading(refused, Storefront::ZAI, &ctx, &account()).unwrap_err(), Unavailability::ApiKeyRefused);
        let empty = br#"{"success":true,"code":200,"data":{"limits":[]}}"#;
        assert_eq!(reading(empty, Storefront::ZAI, &ctx, &account()).unwrap_err(), Unavailability::NoLimitsReported);
        assert_eq!(reading(b"<html>", Storefront::ZAI, &ctx, &account()).unwrap_err(), Unavailability::UnreadableReply);
        let ok = reading(&fixture("glm-coding-plan-quota.json"), Storefront::ZAI, &ctx, &account()).unwrap();
        assert_eq!(ok.plan.as_deref(), Some("lite"));
    }
}
