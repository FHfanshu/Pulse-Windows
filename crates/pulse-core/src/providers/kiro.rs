// Ported from upstream Providers/KiroUsageService.swift and KiroACPClient.swift.
//! Kiro.
//!
//! Kiro's subscription credit pools, read through Kiro CLI's native Agent Client
//! Protocol: start `kiro-cli acp`, finish the `initialize` handshake, call
//! `_kiro/account/getUsage`, then stop the helper. Kiro owns sign-in and token
//! refresh; Pulse never reads Kiro's tokens or database.
//!
//! Windows differences: the CLI is looked up on PATH (`kiro-cli.exe`,
//! `kiro-cli.cmd`, `kiro.cmd`) and in `%LOCALAPPDATA%\Programs\Kiro`, since a GUI
//! app may inherit a thin PATH. `.cmd` shims go through `cmd /c`, and the helper
//! runs without a console window. Its stderr is discarded rather than drained,
//! which keeps a chatty helper from filling its pipe.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout};

use crate::model::{AccountKey, ProviderUsage, Unavailability, UsageRoute, UsageWindow, WindowKind};
use crate::provider::Provider;
use crate::service::{FetchContext, UsageService};

/// Each request (initialize, then getUsage) gets its own 20-second deadline.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const MONTH_SECONDS: i64 = 30 * 86_400;

#[derive(Default)]
pub struct Kiro;

#[async_trait]
impl UsageService for Kiro {
    fn provider(&self) -> Provider {
        Provider::Kiro
    }

    async fn fetch(&self, ctx: &FetchContext, account: &AccountKey) -> ProviderUsage {
        let usage = match acp_usage(ctx).await {
            Ok(body) => reading(&body, account, ctx.now),
            Err(failure) => ProviderUsage::unavailable(account.clone(), failure_reason(&failure)),
        };
        usage.with_origin(UsageRoute::Helper)
    }
}

// ---- reply ------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Envelope {
    success: bool,
    message: Option<String>,
    data: Option<Payload>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Payload {
    plan_name: Option<String>,
    billing_cycle_reset: Option<String>,
    usage_breakdowns: Vec<Breakdown>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Breakdown {
    resource_type: Option<String>,
    display_name: Option<String>,
    used: Option<f64>,
    limit: Option<f64>,
    percentage: Option<f64>,
    has_limit: Option<bool>,
}

/// Pure: turns the `_kiro/account/getUsage` body into a reading.
fn reading(body: &[u8], account: &AccountKey, now: DateTime<Utc>) -> ProviderUsage {
    let Ok(result) = serde_json::from_slice::<Envelope>(body) else {
        return ProviderUsage::unavailable(account.clone(), Unavailability::UnreadableReply);
    };
    if !result.success {
        return ProviderUsage::unavailable(account.clone(), reason_for_message(result.message.as_deref()));
    }
    let Some(payload) = result.data else {
        return ProviderUsage::unavailable(account.clone(), Unavailability::NoLimitsReported);
    };
    let windows = windows(&payload);
    if windows.is_empty() {
        return ProviderUsage::unavailable(account.clone(), Unavailability::NoLimitsReported);
    }
    let mut usage = ProviderUsage::live(account.clone(), windows, now);
    usage.plan = payload.plan_name;
    usage
}

fn windows(payload: &Payload) -> Vec<UsageWindow> {
    let reset = payload.billing_cycle_reset.as_deref().and_then(parse_reset_date);
    let mut occurrences: HashMap<String, usize> = HashMap::new();
    let mut out = Vec::new();
    for item in &payload.usage_breakdowns {
        if item.has_limit == Some(false) {
            continue;
        }
        let Some(limit) = item.limit.filter(|l| l.is_finite() && *l > 0.0) else { continue };
        let used = match (item.used, item.percentage) {
            (Some(reported), _) if reported.is_finite() => reported,
            (_, Some(percent)) if percent.is_finite() => percent / 100.0 * limit,
            _ => continue,
        };
        // Identity comes from the provider's resource type, not array position.
        let resource = stable_id(item.resource_type.as_deref())
            .or_else(|| stable_id(item.display_name.as_deref()))
            .unwrap_or_else(|| "usage".to_string());
        let seen = occurrences.entry(resource.clone()).or_insert(0);
        let id = if *seen == 0 { resource } else { format!("{resource}.{}", *seen + 1) };
        *seen += 1;

        let mut window = UsageWindow::new(id, WindowKind::Monthly, (used / limit).clamp(0.0, 1.0), MONTH_SECONDS)
            .with_reset(reset)
            .exhausted(used >= limit);
        window.scope = item.display_name.clone().or_else(|| item.resource_type.clone());
        window.reports_length = false;
        out.push(window);
    }
    out
}

fn stable_id(value: Option<&str>) -> Option<String> {
    let trimmed = value?.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_lowercase())
}

/// Kiro's date-only reset, taken as midnight UTC.
fn parse_reset_date(text: &str) -> Option<DateTime<Utc>> {
    let date = NaiveDate::parse_from_str(text, "%Y-%m-%d").ok()?;
    Some(DateTime::<Utc>::from_naive_utc_and_offset(date.and_hms_opt(0, 0, 0)?, Utc))
}

fn failure_reason(failure: &Failure) -> Unavailability {
    match failure {
        Failure::ExecutableNotFound => Unavailability::KiroNotInstalled,
        Failure::Server(message) => reason_for_message(Some(message)),
        Failure::StartFailed | Failure::TimedOut | Failure::Closed => Unavailability::Unreachable,
    }
}

fn reason_for_message(message: Option<&str>) -> Unavailability {
    let text = message.unwrap_or_default().to_lowercase();
    if ["sign in", "not authenticated", "login"].iter().any(|k| text.contains(k)) {
        Unavailability::KiroSignInRequired
    } else if ["method not found", "unsupported", "agent-engine"].iter().any(|k| text.contains(k)) {
        Unavailability::KiroVersionUnsupported
    } else {
        Unavailability::UnreadableReply
    }
}

// ---- ACP helper -------------------------------------------------------------

#[derive(Debug, PartialEq)]
enum Failure {
    ExecutableNotFound,
    StartFailed,
    TimedOut,
    Closed,
    Server(String),
}

/// Start, ask, stop. The helper lives only for this one reading.
async fn acp_usage(ctx: &FetchContext) -> Result<Vec<u8>, Failure> {
    let executable = locate_kiro(ctx).ok_or(Failure::ExecutableNotFound)?;
    let mut helper = Helper::start(&executable)?;
    let outcome = helper.usage().await;
    helper.shut_down().await;
    outcome
}

/// Where `kiro-cli` may be on Windows. The PATH entries come first, then the
/// Kiro install folder. WINDOWS-PATH: unverified for `Programs\Kiro`.
fn locate_kiro(ctx: &FetchContext) -> Option<PathBuf> {
    let names = ["kiro-cli.exe", "kiro-cli.cmd", "kiro.cmd"];
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    dirs.push(ctx.local_app_data.join("Programs").join("Kiro"));
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
    // The folder Kiro was found in leads PATH, so a shim finds its own runtime.
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

struct Helper {
    child: Child,
    stdin: ChildStdin,
    lines: Lines<BufReader<ChildStdout>>,
    // IDs belong to this connection, so a late reply can never match a newer request.
    next_id: i64,
}

impl Helper {
    fn start(executable: &Path) -> Result<Self, Failure> {
        let mut child = command_for(executable)
            .args(["acp", "--agent-engine", "v3", "--auth-method", "cli"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| Failure::StartFailed)?;
        let stdin = child.stdin.take().ok_or(Failure::StartFailed)?;
        let stdout = child.stdout.take().ok_or(Failure::StartFailed)?;
        Ok(Self { child, stdin, lines: BufReader::new(stdout).lines(), next_id: 1 })
    }

    async fn usage(&mut self) -> Result<Vec<u8>, Failure> {
        // Kiro installs its auth connection while handling initialize, so
        // getUsage is only sent once that reply has arrived.
        self.request(
            "initialize",
            json!({
                "protocolVersion": 1,
                "clientCapabilities": {},
                "clientInfo": {"name": "Pulse", "version": "0.1"}
            }),
        )
        .await?;
        let result = self.request("_kiro/account/getUsage", json!({})).await?;
        serde_json::to_vec(&result).map_err(|_| Failure::StartFailed)
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value, Failure> {
        let id = self.next_id;
        self.next_id += 1;
        let mut line = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string();
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).await.map_err(|_| Failure::Closed)?;
        self.stdin.flush().await.map_err(|_| Failure::Closed)?;
        tokio::time::timeout(REQUEST_TIMEOUT, self.await_reply(id))
            .await
            .unwrap_or(Err(Failure::TimedOut))
    }

    /// Newline-delimited JSON. Lines that are not objects or not this request's reply are skipped.
    async fn await_reply(&mut self, id: i64) -> Result<Value, Failure> {
        loop {
            let line = match self.lines.next_line().await {
                Ok(Some(line)) => line,
                _ => return Err(Failure::Closed),
            };
            let Ok(message) = serde_json::from_str::<Value>(&line) else { continue };
            if message.get("id").and_then(Value::as_i64) != Some(id) {
                continue;
            }
            if let Some(error) = message.get("error").and_then(Value::as_object) {
                let text = error.get("message").and_then(Value::as_str).unwrap_or("unknown");
                return Err(Failure::Server(text.to_string()));
            }
            return Ok(message.get("result").filter(|r| r.is_object()).cloned().unwrap_or_else(|| json!({})));
        }
    }

    async fn shut_down(mut self) {
        let _ = self.child.kill().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::profile::test_support::fixture;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-10-01T12:00:00Z").unwrap().with_timezone(&Utc)
    }

    fn account() -> AccountKey {
        AccountKey::primary(Provider::Kiro)
    }

    fn payload(value: Value) -> Payload {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn acp_usage_maps_every_bounded_credit_pool() {
        let usage = reading(&fixture("kiro-pro-plus-usage.json"), &account(), now());
        assert_eq!(usage.state, crate::model::UsageState::Live);
        assert_eq!(usage.plan.as_deref(), Some("KIRO PRO+"));
        assert_eq!(usage.windows.len(), 2);
        assert_eq!(usage.windows.iter().map(|w| w.id.as_str()).collect::<Vec<_>>(), ["credit", "bonus_credit"]);
        assert_eq!(
            usage.windows.iter().map(|w| w.scope.as_deref()).collect::<Vec<_>>(),
            [Some("Credits"), Some("Bonus credits")]
        );
        assert!((usage.windows[0].used_fraction - 0.061725).abs() < 0.000_001);
        assert_eq!(usage.windows[1].used_fraction, 0.25);
        assert!(usage.windows.iter().all(|w| w.kind == WindowKind::Monthly && !w.reports_length));
        assert!(usage.windows.iter().all(|w| w.resets_at.is_some()));
    }

    #[test]
    fn malformed_limits_are_skipped_rather_than_invented() {
        let p = payload(json!({
            "planName": "test",
            "billingCycleReset": "not-a-date",
            "usageBreakdowns": [
                {"resourceType": "ZERO", "used": 1, "limit": 0, "hasLimit": true},
                {"resourceType": "UNBOUNDED", "used": 1, "limit": 10, "hasLimit": false},
                {"resourceType": "PERCENT", "displayName": "Percent only", "limit": 80, "percentage": 12.5, "hasLimit": true}
            ]
        }));
        let windows = windows(&p);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].used_fraction, 0.125);
        assert_eq!(windows[0].resets_at, None);
    }

    #[test]
    fn credit_pool_identities_survive_an_array_reorder() {
        let body: Value = serde_json::from_slice(&fixture("kiro-pro-plus-usage.json")).unwrap();
        let mut reversed = body.clone();
        let rows = reversed["data"]["usageBreakdowns"].as_array_mut().unwrap();
        rows.reverse();

        let key = |p: &Payload| -> HashMap<String, String> {
            windows(p).into_iter().filter_map(|w| w.scope.map(|s| (s, w.id))).collect()
        };
        let original = key(&payload(body["data"].clone()));
        let reordered = key(&payload(reversed["data"].clone()));
        assert_eq!(reordered, original);
    }

    #[test]
    fn acp_failures_have_provider_specific_remedies() {
        assert_eq!(failure_reason(&Failure::ExecutableNotFound), Unavailability::KiroNotInstalled);
        assert_eq!(failure_reason(&Failure::Server("Method not found".into())), Unavailability::KiroVersionUnsupported);
        assert_eq!(failure_reason(&Failure::Server("Please sign in".into())), Unavailability::KiroSignInRequired);
        assert_eq!(failure_reason(&Failure::TimedOut), Unavailability::Unreachable);
        assert_eq!(failure_reason(&Failure::Closed), Unavailability::Unreachable);
    }

    #[test]
    fn unsuccessful_or_empty_replies_say_why() {
        let signed_out = br#"{"success": false, "message": "Not authenticated"}"#;
        let usage = reading(signed_out, &account(), now());
        assert_eq!(usage.state, crate::model::UsageState::Unavailable(Unavailability::KiroSignInRequired));

        let no_data = br#"{"success": true, "message": "ok", "data": null}"#;
        let usage = reading(no_data, &account(), now());
        assert_eq!(usage.state, crate::model::UsageState::Unavailable(Unavailability::NoLimitsReported));

        let nothing_usable = br#"{"success": true, "data": {"usageBreakdowns": [{"resourceType": "X", "limit": 0}]}}"#;
        let usage = reading(nothing_usable, &account(), now());
        assert_eq!(usage.state, crate::model::UsageState::Unavailable(Unavailability::NoLimitsReported));

        let garbage = br#"{"nope": true}"#;
        let usage = reading(garbage, &account(), now());
        assert_eq!(usage.state, crate::model::UsageState::Unavailable(Unavailability::UnreadableReply));
    }
}
