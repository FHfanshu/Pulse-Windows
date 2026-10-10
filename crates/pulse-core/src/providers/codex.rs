// Ported from upstream Providers/CodexUsageService.swift and CodexAppServer.swift.
//! Codex.
//!
//! Routes (automatic):
//! 1. HTTP usage endpoint with the OAuth token Codex saved in `~/.codex/auth.json`.
//! 2. On a missing or refused token: `codex app-server`, Codex's own JSON-RPC
//!    protocol over stdio, which signs in on its own terms.
//!
//! The two answers use different field names, hence two parsers. Window kinds
//! come from durations, never from "primary"/"secondary". Spent flags describe
//! a whole group, so they are pinned to the group's fullest window.
//!
//! Windows differences: `codex` is usually an npm shim (`codex.cmd`), which has
//! to be started through `cmd /c`; the app server is started per request and
//! stopped after it answers rather than kept running.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::auth::{self, StoredLogin};
use crate::model::{AccountKey, ProviderUsage, Unavailability, UsageRoute, UsageState, UsageWindow, WindowKind};
use crate::provider::Provider;
use crate::service::{FetchContext, UsageService};

const USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const RETRY_LIMIT: u32 = 2;
const RPC_DEADLINE: Duration = Duration::from_secs(20);

#[derive(Default)]
pub struct Codex;

enum HttpOutcome {
    Success(ProviderUsage),
    NeedsFreshCredentials,
    Failed(Unavailability),
}

struct Credentials {
    access_token: String,
    account_id: String,
}

#[async_trait]
impl UsageService for Codex {
    fn provider(&self) -> Provider {
        Provider::Codex
    }

    async fn fetch(&self, ctx: &FetchContext, account: &AccountKey) -> ProviderUsage {
        // Added accounts: the login Pulse holds (renewed here). A raw token saved by an older
        // build, "<access token> <account id>", is read when no login is stored.
        if !account.is_primary() {
            let credentials = match auth::usable_login(ctx, account).await {
                StoredLogin::Usable(login) => {
                    Credentials { access_token: login.access_token, account_id: login.account_id.unwrap_or_default() }
                }
                StoredLogin::Expired => return ProviderUsage::unavailable(account.clone(), Unavailability::SignedOut),
                StoredLogin::Missing => {
                    let Some(stored) = ctx.api_key(account) else {
                        return ProviderUsage::unavailable(account.clone(), Unavailability::SignedOut);
                    };
                    let mut parts = stored.split_whitespace();
                    Credentials {
                        access_token: parts.next().unwrap_or_default().to_string(),
                        account_id: parts.next().unwrap_or_default().to_string(),
                    }
                }
            };
            return match over_http(ctx, &credentials, account).await {
                HttpOutcome::Success(u) => u,
                HttpOutcome::NeedsFreshCredentials => ProviderUsage::unavailable(account.clone(), Unavailability::SignInRequired),
                HttpOutcome::Failed(r) => ProviderUsage::unavailable(account.clone(), r),
            };
        }

        let source = ctx.source(Provider::Codex).unwrap_or("automatic");
        if source == "tooling" {
            return via_app_server(ctx, account).await;
        }
        let outcome = match load_credentials(ctx) {
            Some(credentials) => over_http(ctx, &credentials, account).await,
            None => HttpOutcome::NeedsFreshCredentials,
        };
        match outcome {
            HttpOutcome::Success(usage) => usage,
            HttpOutcome::NeedsFreshCredentials if source == "endpoint" => {
                ProviderUsage::unavailable(account.clone(), Unavailability::SignInRequired).with_origin(UsageRoute::Endpoint)
            }
            HttpOutcome::NeedsFreshCredentials => via_app_server(ctx, account).await,
            HttpOutcome::Failed(reason) => ProviderUsage::unavailable(account.clone(), reason).with_origin(UsageRoute::Endpoint),
        }
    }
}

fn codex_home(ctx: &FetchContext) -> PathBuf {
    std::env::var_os("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|| ctx.home.join(".codex"))
}

fn load_credentials(ctx: &FetchContext) -> Option<Credentials> {
    let root: Value = serde_json::from_slice(&std::fs::read(codex_home(ctx).join("auth.json")).ok()?).ok()?;
    let tokens = root.get("tokens")?;
    Some(Credentials {
        access_token: tokens.get("access_token")?.as_str()?.to_string(),
        account_id: tokens.get("account_id")?.as_str()?.to_string(),
    })
}

async fn over_http(ctx: &FetchContext, credentials: &Credentials, account: &AccountKey) -> HttpOutcome {
    for attempt in 0..=RETRY_LIMIT {
        let mut request = ctx.http.get(USAGE_URL).bearer_auth(&credentials.access_token).header("Accept", "application/json");
        if !credentials.account_id.is_empty() {
            request = request.header("ChatGPT-Account-Id", &credentials.account_id);
        }
        match request.send().await {
            Ok(reply) => {
                return match reply.status().as_u16() {
                    200 => match reply.json::<Value>().await {
                        Ok(root) => {
                            let mut usage = parse_usage_response(&root, account, ctx.now).with_origin(UsageRoute::Endpoint);
                            // Windows difference: match the account actually used by this request,
                            // including the workspace header, rather than Pulse's account slot.
                            usage.spend_identity = crate::spend::account::AccountIdentity::codex_token(&credentials.access_token, &credentials.account_id);
                            HttpOutcome::Success(usage)
                        }
                        Err(_) => HttpOutcome::Failed(Unavailability::UnreadableReply),
                    },
                    401 | 403 => HttpOutcome::NeedsFreshCredentials,
                    429 => HttpOutcome::Failed(Unavailability::RateLimited),
                    _ => HttpOutcome::Failed(Unavailability::ServerError),
                };
            }
            Err(e) if attempt < RETRY_LIMIT && (e.is_timeout() || e.is_connect()) => {
                tokio::time::sleep(Duration::from_millis(600 * (attempt as u64 + 1))).await;
            }
            Err(_) => return HttpOutcome::Failed(Unavailability::Unreachable),
        }
    }
    HttpOutcome::Failed(Unavailability::Unreachable)
}

fn kind(seconds: i64) -> WindowKind {
    match seconds {
        18_000 => WindowKind::FiveHour,
        604_800 => WindowKind::Weekly,
        s => WindowKind::Other(s),
    }
}

fn number(v: Option<&Value>) -> Option<f64> {
    v.and_then(Value::as_f64)
}

fn not_null(v: Option<&Value>) -> bool {
    v.is_some_and(|v| !v.is_null())
}

/// Mark only the fullest window of a group as spent.
fn marking_spent(mut windows: Vec<UsageWindow>, spent: bool) -> Vec<UsageWindow> {
    if !spent {
        return windows;
    }
    let fullest = windows
        .iter()
        .enumerate()
        .fold(None, |best: Option<(usize, f64)>, (i, w)| match best {
            Some((_, f)) if f >= w.used_fraction => best,
            _ => Some((i, w.used_fraction)),
        })
        .map(|(i, _)| i);
    if let Some(i) = fullest {
        windows[i].is_exhausted = true;
    }
    windows
}

fn is_group_spent(limit: &Value) -> bool {
    limit.get("limit_reached").and_then(Value::as_bool) == Some(true) || limit.get("allowed").and_then(Value::as_bool) == Some(false)
}

fn http_windows(limit: &Value, id_prefix: &str, scope: Option<&str>) -> Vec<UsageWindow> {
    ["primary_window", "secondary_window"]
        .into_iter()
        .filter_map(|slot| {
            let node = limit.get(slot)?;
            let percent = number(node.get("used_percent"))?;
            let seconds = number(node.get("limit_window_seconds")).map(|s| s as i64);
            let mut w = UsageWindow::new(
                format!("{id_prefix}.{slot}"),
                seconds.map(kind).unwrap_or(WindowKind::Other(0)),
                percent / 100.0,
                seconds.unwrap_or(0),
            )
            .with_reset(number(node.get("reset_at")).and_then(|s| DateTime::from_timestamp(s as i64, 0)));
            w.scope = scope.map(str::to_string);
            Some(w)
        })
        .collect()
}

pub fn parse_usage_response(root: &Value, account: &AccountKey, now: DateTime<Utc>) -> ProviderUsage {
    let mut windows = Vec::new();
    let spend_reached = root.pointer("/spend_control/reached").and_then(Value::as_bool) == Some(true);
    if let Some(limit) = root.get("rate_limit").filter(|v| v.is_object()) {
        let spent = is_group_spent(limit) || not_null(root.get("rate_limit_reached_type")) || spend_reached;
        windows.extend(marking_spent(http_windows(limit, "codex", None), spent));
    }
    for extra in root.get("additional_rate_limits").and_then(Value::as_array).into_iter().flatten() {
        let Some(limit) = extra.get("rate_limit").filter(|v| v.is_object()) else { continue };
        let label = extra.get("limit_name").and_then(Value::as_str);
        let key = extra.get("metered_feature").and_then(Value::as_str).or(label).unwrap_or("extra");
        windows.extend(marking_spent(http_windows(limit, key, label), is_group_spent(limit)));
    }
    finish(account, windows, root.get("plan_type").and_then(Value::as_str), credit_balance(root.get("credits")), now)
}

pub fn parse_app_server_response(result: &Value, account: &AccountKey, now: DateTime<Utc>) -> ProviderUsage {
    let mut groups: Vec<(String, &Value)> = match result.get("rateLimitsByLimitId").and_then(Value::as_object) {
        Some(map) => map.iter().map(|(k, v)| (k.clone(), v)).collect(),
        None => result.get("rateLimits").map(|v| vec![("codex".to_string(), v)]).unwrap_or_default(),
    };
    // Unnamed (account-wide) groups first, then by key.
    groups.sort_by(|a, b| {
        let named = |v: &Value| v.get("limitName").and_then(Value::as_str).is_some();
        named(a.1).cmp(&named(b.1)).then_with(|| a.0.cmp(&b.0))
    });
    let ordinary_refused = result.get("ordinaryUsageAllowed").and_then(Value::as_bool) == Some(false);

    let mut windows = Vec::new();
    let mut plan: Option<String> = None;
    let mut credits: Option<String> = None;
    for (key, group) in groups {
        let scope = group.get("limitName").and_then(Value::as_str);
        let of_group: Vec<UsageWindow> = ["primary", "secondary"]
            .into_iter()
            .filter_map(|slot| {
                let node = group.get(slot)?;
                let percent = number(node.get("usedPercent"))?;
                let minutes = number(node.get("windowDurationMins")).map(|m| m as i64);
                let mut w = UsageWindow::new(
                    format!("{key}.{slot}"),
                    minutes.map(|m| kind(m * 60)).unwrap_or(WindowKind::Other(0)),
                    percent / 100.0,
                    minutes.unwrap_or(0) * 60,
                )
                .with_reset(number(node.get("resetsAt")).and_then(|s| DateTime::from_timestamp(s as i64, 0)));
                w.scope = scope.map(str::to_string);
                Some(w)
            })
            .collect();
        let spent = group.get("spendControlReached").and_then(Value::as_bool) == Some(true)
            || not_null(group.get("rateLimitReachedType"))
            || ordinary_refused;
        windows.extend(marking_spent(of_group, spent));
        plan = plan.or_else(|| group.get("planType").and_then(Value::as_str).map(str::to_string));
        credits = credits.or_else(|| credit_balance(group.get("credits")));
    }
    finish(account, windows, plan.as_deref(), credits, now)
}

fn finish(account: &AccountKey, windows: Vec<UsageWindow>, plan: Option<&str>, credits: Option<String>, now: DateTime<Utc>) -> ProviderUsage {
    let mut usage = ProviderUsage::live(account.clone(), windows, now).with_plan(plan.map(plan_name));
    usage.credit_balance = credits;
    if usage.windows.is_empty() {
        usage.state = UsageState::Unavailable(Unavailability::NoLimitsReported);
    }
    usage
}

pub fn plan_name(raw: &str) -> String {
    match raw.to_ascii_lowercase().as_str() {
        "free" => "Free",
        "go" => "Go",
        "plus" => "Plus",
        "pro" => "Pro",
        "prolite" => "Pro 5x",
        "team" => "Team",
        "business" => "Business",
        "enterprise" => "Enterprise",
        "edu" => "Edu",
        _ => return raw.to_string(),
    }
    .to_string()
}

/// Credits, not money: a plain number with at most two places. Unlimited is no balance.
pub fn credit_balance(node: Option<&Value>) -> Option<String> {
    let node = node?;
    if node.get("unlimited").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let raw = node.get("balance")?;
    let value = raw.as_f64().or_else(|| raw.as_str().and_then(|s| s.trim().parse::<f64>().ok()));
    match value.filter(|v| v.is_finite()) {
        Some(v) => {
            let text = format!("{:.2}", v);
            Some(text.trim_end_matches('0').trim_end_matches('.').to_string())
        }
        None => raw.as_str().map(str::to_string),
    }
}

// ---- codex app-server -------------------------------------------------------

/// Where `codex` may be on Windows. A GUI app inherits a thin PATH, so the usual
/// install folders are checked explicitly.
fn locate_codex(ctx: &FetchContext) -> Option<PathBuf> {
    let names = ["codex.exe", "codex.cmd", "codex.bat"];
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    dirs.extend([
        ctx.app_data.join("npm"),
        ctx.local_app_data.join("pnpm"),
        ctx.home.join(".bun").join("bin"),
        ctx.home.join(".volta").join("bin"),
        ctx.home.join(".local").join("bin"),
        ctx.local_app_data.join("Programs").join("codex"),
    ]);
    dirs.iter().flat_map(|d| names.iter().map(move |n| d.join(n))).find(|p| p.is_file())
}

fn command_for(path: &Path) -> tokio::process::Command {
    let is_script = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"));
    let mut cmd = if is_script {
        let mut c = tokio::process::Command::new("cmd");
        c.arg("/d").arg("/c").arg(path);
        c
    } else {
        tokio::process::Command::new(path)
    };
    // The folder codex was found in leads PATH, so an npm shim finds its `node`.
    if let Some(dir) = path.parent() {
        let mut paths = vec![dir.to_path_buf()];
        if let Some(existing) = std::env::var_os("PATH") {
            paths.extend(std::env::split_paths(&existing));
        }
        if let Ok(joined) = std::env::join_paths(paths) {
            cmd.env("PATH", joined);
        }
    }
    #[cfg(windows)]
    {
        // CREATE_NO_WINDOW: no console flashes up behind the panel.
        cmd.creation_flags(0x0800_0000);
    }
    cmd
}

enum RpcError {
    NotFound,
    StartFailed,
    TimedOut,
    Server(String),
}

async fn rate_limits_rpc(ctx: &FetchContext) -> Result<Value, RpcError> {
    let path = locate_codex(ctx).ok_or(RpcError::NotFound)?;
    let mut child = command_for(&path)
        .arg("app-server")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| RpcError::StartFailed)?;
    let mut stdin = child.stdin.take().ok_or(RpcError::StartFailed)?;
    let stdout = child.stdout.take().ok_or(RpcError::StartFailed)?;
    let mut lines = BufReader::new(stdout).lines();

    let messages = [
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
               "params": {"clientInfo": {"name": "Pulse", "title": "Pulse", "version": env!("CARGO_PKG_VERSION")}}}),
        json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "account/rateLimits/read", "params": {}}),
    ];
    for m in &messages {
        let mut line = m.to_string();
        line.push('\n');
        stdin.write_all(line.as_bytes()).await.map_err(|_| RpcError::StartFailed)?;
    }
    stdin.flush().await.map_err(|_| RpcError::StartFailed)?;

    let read = async {
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(message) = serde_json::from_str::<Value>(&line) else { continue };
            if message.get("id").and_then(Value::as_i64) != Some(2) {
                continue;
            }
            if let Some(error) = message.get("error") {
                let text = error.get("message").and_then(Value::as_str).unwrap_or_default().to_string();
                return Err(RpcError::Server(text));
            }
            return Ok(message.get("result").cloned().unwrap_or(Value::Null));
        }
        Err(RpcError::StartFailed)
    };
    let result = tokio::time::timeout(RPC_DEADLINE, read).await.map_err(|_| RpcError::TimedOut)?;
    let _ = child.kill().await;
    result
}

async fn via_app_server(ctx: &FetchContext, account: &AccountKey) -> ProviderUsage {
    let usage = match rate_limits_rpc(ctx).await {
        Ok(result) => parse_app_server_response(&result, account, ctx.now),
        Err(RpcError::NotFound) => ProviderUsage::unavailable(account.clone(), Unavailability::SignInRequired),
        Err(RpcError::StartFailed) => ProviderUsage::unavailable(account.clone(), Unavailability::CodexServerFailed),
        Err(RpcError::TimedOut) => ProviderUsage::unavailable(account.clone(), Unavailability::Unreachable),
        Err(RpcError::Server(message)) => {
            let text = message.to_ascii_lowercase();
            let is_auth = ["auth", "login", "sign in", "unauthor", "credential"].iter().any(|k| text.contains(k));
            ProviderUsage::unavailable(account.clone(), if is_auth { Unavailability::SignInRequired } else { Unavailability::ServerError })
        }
    };
    usage.with_origin(UsageRoute::Helper)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-10-01T12:00:00Z").unwrap().with_timezone(&Utc)
    }
    fn account() -> AccountKey {
        AccountKey::primary(Provider::Codex)
    }

    #[test]
    fn http_reply_kinds_from_duration_and_spent_on_fullest() {
        let root = json!({
            "plan_type": "prolite",
            "rate_limit": {
                "limit_reached": true,
                "primary_window": {"used_percent": 100, "limit_window_seconds": 18000, "reset_at": 1790000000},
                "secondary_window": {"used_percent": 41, "limit_window_seconds": 604800, "reset_at": 1790500000}
            },
            "additional_rate_limits": [
                {"limit_name": "GPT-5.6-Codex-Spark", "metered_feature": "spark",
                 "rate_limit": {"primary_window": {"used_percent": 3, "limit_window_seconds": 604800}}}
            ],
            "credits": {"balance": "2500.0000000000", "unlimited": false}
        });
        let usage = parse_usage_response(&root, &account(), now());
        assert_eq!(usage.plan.as_deref(), Some("Pro 5x"));
        assert_eq!(usage.windows.len(), 3);
        assert_eq!(usage.windows[0].kind, WindowKind::FiveHour);
        assert!(usage.windows[0].is_exhausted);
        assert!(!usage.windows[1].is_exhausted, "only the fullest window of a group is spent");
        assert_eq!(usage.windows[2].scope.as_deref(), Some("GPT-5.6-Codex-Spark"));
        assert_eq!(usage.windows[2].id, "spark.primary_window");
        assert_eq!(usage.credit_balance.as_deref(), Some("2500"));
    }

    #[test]
    fn pro_without_five_hour_window_is_fine() {
        let root = json!({"rate_limit": {"primary_window": {"used_percent": 10, "limit_window_seconds": 604800}}});
        let usage = parse_usage_response(&root, &account(), now());
        assert_eq!(usage.windows.len(), 1);
        assert_eq!(usage.windows[0].kind, WindowKind::Weekly);
    }

    #[test]
    fn app_server_reply_orders_unnamed_groups_first() {
        let result = json!({
            "rateLimitsByLimitId": {
                "a_named": {"limitName": "Spark", "primary": {"usedPercent": 5, "windowDurationMins": 10080}},
                "codex": {"planType": "plus", "primary": {"usedPercent": 60, "windowDurationMins": 300, "resetsAt": 1790000000},
                          "secondary": {"usedPercent": 20, "windowDurationMins": 10080}, "rateLimitReachedType": "primary"}
            }
        });
        let usage = parse_app_server_response(&result, &account(), now());
        assert_eq!(usage.windows[0].id, "codex.primary");
        assert!(usage.windows[0].is_exhausted);
        assert_eq!(usage.windows[2].scope.as_deref(), Some("Spark"));
        assert_eq!(usage.plan.as_deref(), Some("Plus"));
    }

    #[test]
    fn credit_balance_rules() {
        assert_eq!(credit_balance(Some(&json!({"balance": "12.3456"}))).as_deref(), Some("12.35"));
        assert_eq!(credit_balance(Some(&json!({"balance": 5, "unlimited": true}))), None);
        assert_eq!(credit_balance(Some(&json!({"balance": "lots"}))).as_deref(), Some("lots"));
    }

    #[test]
    fn empty_reply_says_no_limits() {
        let usage = parse_usage_response(&json!({}), &account(), now());
        assert_eq!(usage.state, UsageState::Unavailable(Unavailability::NoLimitsReported));
    }
}
