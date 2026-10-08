// Ported from upstream Providers/ClaudeCodeUsageService.swift (endpoint and
// status-line routes; the desktop-app route follows separately).
//! Claude Code.
//!
//! Routes, in automatic order:
//! 1. Usage endpoint with the OAuth token Claude Code saved. On Windows the CLI
//!    keeps it in `%USERPROFILE%\.claude\.credentials.json` — there is no
//!    Keychain step to wait on.
//! 2. Status-line capture: the blob Claude Code pipes to `pulse --statusline`.
//! 3. An actionable unavailable reason.
//!
//! A network/rate-limit/server failure on the endpoint goes straight to the
//! capture: a stumble should not hide a good captured reading.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration as StdDuration, Instant};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::model::{AccountKey, ProviderUsage, Unavailability, UsageRoute, UsageState, UsageWindow, WindowKind};
use crate::provider::Provider;
use crate::service::{FetchContext, UsageService};

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const PROFILE_URL: &str = "https://api.anthropic.com/api/oauth/profile";
const BETA: &str = "oauth-2025-04-20";
const USER_AGENT: &str = "claude-cli (external, cli)";
/// A capture younger than this reads live; older reads stale with an "as of" line.
const CAPTURE_FRESH_FOR_SECONDS: i64 = 10 * 60;
const PLAN_FRESH_FOR: StdDuration = StdDuration::from_secs(6 * 3600);
const RETRY_LIMIT: u32 = 2;

#[derive(Default)]
pub struct ClaudeCode {
    /// Plan name per account id, with when it was asked (a failure is cached too).
    plans: Mutex<HashMap<String, (Option<String>, Instant)>>,
}

enum HttpOutcome {
    Success(ProviderUsage),
    NeedsFreshCredentials,
    Failed(Unavailability),
}

#[async_trait]
impl UsageService for ClaudeCode {
    fn provider(&self) -> Provider {
        Provider::ClaudeCode
    }

    async fn fetch(&self, ctx: &FetchContext, account: &AccountKey) -> ProviderUsage {
        // Added accounts hold their own token and never borrow the CLI's routes.
        if !account.is_primary() {
            return match ctx.api_key(account) {
                Some(token) => self.endpoint_usage(ctx, account, &token).await,
                None => ProviderUsage::unavailable(account.clone(), Unavailability::SignedOut),
            };
        }

        match ctx.source(Provider::ClaudeCode).unwrap_or("automatic") {
            "tooling" => captured_usage(ctx, account).unwrap_or_else(|| {
                ProviderUsage::unavailable(account.clone(), capture_problem(ctx, None))
            }),
            "endpoint" => match credentials(ctx).as_ref().and_then(|c| unexpired_token(c, ctx.now)) {
                Some(token) => self.endpoint_usage(ctx, account, &token).await,
                None => ProviderUsage::unavailable(account.clone(), Unavailability::ClaudeSignInRequired)
                    .with_origin(UsageRoute::Endpoint),
            },
            _ => self.automatic(ctx, account).await,
        }
    }
}

impl ClaudeCode {
    async fn automatic(&self, ctx: &FetchContext, account: &AccountKey) -> ProviderUsage {
        let stored = credentials(ctx);
        let had_credentials = stored.is_some();
        if let Some(token) = stored.as_ref().and_then(|c| unexpired_token(c, ctx.now)) {
            match self.over_http(ctx, account, &token).await {
                HttpOutcome::Success(usage) => return usage,
                HttpOutcome::NeedsFreshCredentials => {}
                HttpOutcome::Failed(reason) => {
                    return captured_usage(ctx, account)
                        .unwrap_or_else(|| ProviderUsage::unavailable(account.clone(), reason).with_origin(UsageRoute::Endpoint));
                }
            }
        }
        captured_usage(ctx, account)
            .unwrap_or_else(|| ProviderUsage::unavailable(account.clone(), capture_problem(ctx, Some(had_credentials))))
    }

    async fn endpoint_usage(&self, ctx: &FetchContext, account: &AccountKey, token: &str) -> ProviderUsage {
        match self.over_http(ctx, account, token).await {
            HttpOutcome::Success(usage) => usage,
            HttpOutcome::NeedsFreshCredentials => {
                ProviderUsage::unavailable(account.clone(), Unavailability::ClaudeLoginExpired).with_origin(UsageRoute::Endpoint)
            }
            HttpOutcome::Failed(reason) => ProviderUsage::unavailable(account.clone(), reason).with_origin(UsageRoute::Endpoint),
        }
    }

    async fn over_http(&self, ctx: &FetchContext, account: &AccountKey, token: &str) -> HttpOutcome {
        for attempt in 0..=RETRY_LIMIT {
            let request = ctx
                .http
                .get(USAGE_URL)
                .bearer_auth(token)
                .header("anthropic-beta", BETA)
                .header("User-Agent", USER_AGENT)
                .header("Accept", "application/json");
            match request.send().await {
                Ok(reply) => {
                    return match reply.status().as_u16() {
                        200 => match reply.json::<Value>().await {
                            Ok(root) => {
                                let plan = self.plan(ctx, account, token).await;
                                let mut usage = parse(&root, account, ctx.now);
                                usage.plan = plan;
                                HttpOutcome::Success(usage.with_origin(UsageRoute::Endpoint))
                            }
                            Err(_) => HttpOutcome::Failed(Unavailability::UnreadableReply),
                        },
                        401 | 403 => HttpOutcome::NeedsFreshCredentials,
                        429 => HttpOutcome::Failed(Unavailability::RateLimited),
                        _ => HttpOutcome::Failed(Unavailability::ServerError),
                    };
                }
                Err(e) if attempt < RETRY_LIMIT && (e.is_timeout() || e.is_connect()) => {
                    tokio::time::sleep(StdDuration::from_millis(600 * (attempt as u64 + 1))).await;
                }
                Err(_) => return HttpOutcome::Failed(Unavailability::Unreachable),
            }
        }
        HttpOutcome::Failed(Unavailability::Unreachable)
    }

    /// The plan name, asked at most every six hours per account.
    async fn plan(&self, ctx: &FetchContext, account: &AccountKey, token: &str) -> Option<String> {
        if let Some((name, at)) = self.plans.lock().unwrap().get(&account.id()) {
            if at.elapsed() < PLAN_FRESH_FOR {
                return name.clone();
            }
        }
        let fetched = async {
            let reply = ctx
                .http
                .get(PROFILE_URL)
                .bearer_auth(token)
                .header("anthropic-beta", BETA)
                .header("User-Agent", USER_AGENT)
                .header("Accept", "application/json")
                .send()
                .await
                .ok()?;
            if reply.status().as_u16() != 200 {
                return None;
            }
            plan_name(&reply.json::<Value>().await.ok()?)
        }
        .await;
        self.plans.lock().unwrap().insert(account.id(), (fetched.clone(), Instant::now()));
        fetched
    }
}

fn credentials_path(ctx: &FetchContext) -> PathBuf {
    // Claude Code honours CLAUDE_CONFIG_DIR; otherwise ~/.claude.
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| ctx.home.join(".claude"))
        .join(".credentials.json")
}

fn credentials(ctx: &FetchContext) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(credentials_path(ctx)).ok()?).ok()
}

fn unexpired_token(credentials: &Value, now: DateTime<Utc>) -> Option<String> {
    let oauth = credentials.get("claudeAiOauth")?;
    let token = oauth.get("accessToken")?.as_str()?.to_string();
    if let Some(expires_ms) = oauth.get("expiresAt").and_then(Value::as_f64) {
        if DateTime::from_timestamp_millis(expires_ms as i64).is_some_and(|at| at <= now) {
            return None;
        }
    }
    Some(token)
}

/// Why there is nothing, when the capture has nothing either.
fn capture_problem(ctx: &FetchContext, had_credentials: Option<bool>) -> Unavailability {
    if status_line_installed(ctx) {
        return Unavailability::AwaitingResponse;
    }
    match had_credentials {
        Some(true) => Unavailability::ClaudeLoginExpired,
        Some(false) => Unavailability::ClaudeSignInRequired,
        None => Unavailability::NotConnected,
    }
}

fn status_line_installed(ctx: &FetchContext) -> bool {
    crate::statusline::is_installed(&ctx.home)
}

/// Parse the usage endpoint's reply. The `limits` array is preferred: only it
/// carries per-model weekly windows. The top-level fields are the fallback.
pub fn parse(root: &Value, account: &AccountKey, now: DateTime<Utc>) -> ProviderUsage {
    let mut windows: Vec<UsageWindow> = root
        .get("limits")
        .and_then(Value::as_array)
        .map(|limits| limits.iter().filter_map(window_from_limit).collect())
        .unwrap_or_default();

    if windows.is_empty() {
        for (key, kind, seconds) in [("five_hour", WindowKind::FiveHour, 5 * 3600), ("seven_day", WindowKind::Weekly, 7 * 86_400)] {
            let Some(node) = root.get(key) else { continue };
            let Some(percent) = node.get("utilization").and_then(Value::as_f64) else { continue };
            windows.push(
                UsageWindow::new(format!("claudeCode.{key}"), kind, percent / 100.0, seconds)
                    .with_reset(node.get("resets_at").and_then(Value::as_str).and_then(iso))
                    .exhausted(is_spent(node)),
            );
        }
    }

    let mut usage = ProviderUsage::live(account.clone(), windows, now);
    if usage.windows.is_empty() {
        usage.state = UsageState::Unavailable(Unavailability::NoLimitsReported);
    }
    usage
}

fn window_from_limit(limit: &Value) -> Option<UsageWindow> {
    let kind_name = limit.get("kind")?.as_str()?;
    let percent = limit.get("percent")?.as_f64()?;
    let (kind, seconds) = match kind_name {
        "session" => (WindowKind::FiveHour, 5 * 3600),
        "weekly_all" | "weekly_scoped" => (WindowKind::Weekly, 7 * 86_400),
        _ => return None,
    };
    let scope = limit.pointer("/scope/model/display_name").and_then(Value::as_str).map(str::to_string);
    let mut window = UsageWindow::new(
        format!("claudeCode.{kind_name}.{}", scope.as_deref().unwrap_or("all")),
        kind,
        percent / 100.0,
        seconds,
    )
    .with_reset(limit.get("resets_at").and_then(Value::as_str).and_then(iso))
    .exhausted(is_spent(limit));
    window.scope = scope;
    Some(window)
}

/// A `locked_reason` is spent; so is any severity not known to mean "still has room".
/// `warning` is not spent — Claude Code raises it with a quarter of the limit left.
fn is_spent(limit: &Value) -> bool {
    if limit.get("locked_reason").is_some_and(|v| !v.is_null()) {
        return true;
    }
    let Some(severity) = limit.get("severity").and_then(Value::as_str) else { return false };
    !matches!(severity.to_ascii_lowercase().as_str(), "normal" | "ok" | "none" | "healthy" | "warning" | "warn")
}

fn iso(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text).ok().map(|d| d.with_timezone(&Utc))
}

/// Plan from `/api/oauth/profile`: `subscription_type` first, else the tier, with its multiplier.
pub fn plan_name(root: &Value) -> Option<String> {
    let organization = root.get("organization");
    let tier = organization
        .and_then(|o| o.get("rate_limit_tier"))
        .and_then(Value::as_str)
        .map(str::to_ascii_lowercase);
    let multiplier = tier
        .as_deref()
        .and_then(|t| ["20x", "5x"].into_iter().find(|m| t.ends_with(&format!("_{m}"))));
    let with_multiplier = |name: String| match multiplier {
        Some(m) => format!("{name} {m}"),
        None => name,
    };

    let stated = organization
        .and_then(|o| o.get("subscription_type"))
        .or_else(|| root.get("subscription_type"))
        .and_then(Value::as_str);
    if let Some(stated) = stated {
        return Some(with_multiplier(tidy(stated)));
    }
    let Some(tier) = tier else {
        return organization.and_then(|o| o.get("organization_type")).and_then(Value::as_str).map(tidy);
    };
    let base = if tier.contains("max") {
        "Max".to_string()
    } else if tier.contains("team") {
        "Team".to_string()
    } else if tier.contains("enterprise") {
        "Enterprise".to_string()
    } else if tier.contains("pro") {
        "Pro".to_string()
    } else if tier.contains("free") {
        "Free".to_string()
    } else {
        tidy(&tier)
    };
    Some(with_multiplier(base))
}

fn tidy(raw: &str) -> String {
    raw.split(['_', '-'])
        .filter(|p| !p.is_empty())
        .map(|p| {
            let mut c = p.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The last status-line capture, with windows that have already reset dropped.
fn captured_usage(ctx: &FetchContext, account: &AccountKey) -> Option<ProviderUsage> {
    let bytes = std::fs::read(crate::paths::claude_status_line_capture()).ok()?;
    parse_capture(&serde_json::from_slice(&bytes).ok()?, account, ctx.now)
}

pub fn parse_capture(root: &Value, account: &AccountKey, now: DateTime<Utc>) -> Option<ProviderUsage> {
    let limits = root.get("rate_limits")?;
    let windows: Vec<UsageWindow> = [
        ("five_hour", WindowKind::FiveHour, 5 * 3600),
        ("seven_day", WindowKind::Weekly, 7 * 86_400),
        ("spend_limit", WindowKind::Spend, 0),
    ]
    .into_iter()
    .filter_map(|(key, kind, seconds)| {
        let node = limits.get(key)?;
        let percent = percent(node.get("used_percentage")?)?;
        let resets = node.get("resets_at").and_then(Value::as_f64).and_then(|s| DateTime::from_timestamp(s as i64, 0));
        if resets.is_some_and(|r| r <= now) {
            return None;
        }
        Some(UsageWindow::new(format!("claudeCode.{key}"), kind, percent / 100.0, seconds).with_reset(resets))
    })
    .collect();
    if windows.is_empty() {
        return None;
    }
    let captured = root.get("capturedAt").and_then(Value::as_f64).and_then(|s| DateTime::from_timestamp(s as i64, 0));
    let fresh = captured.is_some_and(|c| (now - c).num_seconds() <= CAPTURE_FRESH_FOR_SECONDS);
    let mut usage = ProviderUsage::live(account.clone(), windows, now);
    usage.observed_at = captured;
    usage.state = if fresh { UsageState::Live } else { UsageState::Stale };
    Some(usage.with_origin(UsageRoute::StatusLine))
}

/// A percentage as Claude Code writes it: a number, or a numeric string.
fn percent(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().trim_end_matches('%').parse().ok(),
        _ => None,
    }
    .filter(|p: &f64| p.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn now() -> DateTime<Utc> {
        iso("2026-10-01T12:00:00Z").unwrap()
    }
    fn account() -> AccountKey {
        AccountKey::primary(Provider::ClaudeCode)
    }

    #[test]
    fn limits_array_carries_scoped_weekly_windows() {
        let root = json!({
            "limits": [
                {"kind": "session", "percent": 12.0, "resets_at": "2026-10-01T15:00:00Z", "severity": "normal"},
                {"kind": "weekly_all", "percent": 40, "resets_at": "2026-10-05T00:00:00Z"},
                {"kind": "weekly_scoped", "percent": 76, "severity": "warning", "scope": {"model": {"display_name": "Opus"}}},
                {"kind": "something_new", "percent": 1}
            ],
            "five_hour": {"utilization": 99}
        });
        let usage = parse(&root, &account(), now());
        assert_eq!(usage.windows.len(), 3);
        assert_eq!(usage.windows[0].kind, WindowKind::FiveHour);
        assert_eq!(usage.windows[2].scope.as_deref(), Some("Opus"));
        assert_eq!(usage.windows[2].id, "claudeCode.weekly_scoped.Opus");
        assert!(!usage.windows[2].is_exhausted, "warning still has room");
    }

    #[test]
    fn falls_back_to_top_level_fields_and_reads_locks() {
        let root = json!({"five_hour": {"utilization": 100, "resets_at": "2026-10-01T13:00:00Z", "locked_reason": "rate_limited"}});
        let usage = parse(&root, &account(), now());
        assert_eq!(usage.windows.len(), 1);
        assert!(usage.windows[0].is_exhausted);
    }

    #[test]
    fn nothing_reported_is_said_so() {
        let usage = parse(&json!({}), &account(), now());
        assert_eq!(usage.state, UsageState::Unavailable(Unavailability::NoLimitsReported));
    }

    #[test]
    fn plan_names() {
        assert_eq!(plan_name(&json!({"organization": {"rate_limit_tier": "default_claude_max_5x"}})).as_deref(), Some("Max 5x"));
        assert_eq!(plan_name(&json!({"organization": {"subscription_type": "claude_pro"}})).as_deref(), Some("Claude Pro"));
        assert_eq!(plan_name(&json!({})), None);
    }

    #[test]
    fn expired_token_is_not_used() {
        let creds = json!({"claudeAiOauth": {"accessToken": "t", "expiresAt": 1.0}});
        assert_eq!(unexpired_token(&creds, now()), None);
        let creds = json!({"claudeAiOauth": {"accessToken": "t", "expiresAt": 4_102_444_800_000.0}});
        assert_eq!(unexpired_token(&creds, now()).as_deref(), Some("t"));
    }

    #[test]
    fn capture_drops_reset_windows_and_ages_to_stale() {
        let root = json!({
            "capturedAt": now().timestamp() - 3600,
            "rate_limits": {
                "five_hour": {"used_percentage": "9", "resets_at": now().timestamp() - 60},
                "seven_day": {"used_percentage": 30, "resets_at": now().timestamp() + 86_400}
            }
        });
        let usage = parse_capture(&root, &account(), now()).unwrap();
        assert_eq!(usage.windows.len(), 1);
        assert_eq!(usage.windows[0].kind, WindowKind::Weekly);
        assert_eq!(usage.state, UsageState::Stale);

        let all_reset = json!({"rate_limits": {"five_hour": {"used_percentage": 9, "resets_at": now().timestamp() - 60}}});
        assert!(parse_capture(&all_reset, &account(), now()).is_none());
    }
}
