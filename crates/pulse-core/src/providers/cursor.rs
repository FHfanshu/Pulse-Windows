// Ported from upstream Providers/CursorUsageService.swift and Auth/CursorAppLogin.swift.
//! Cursor.
//!
//! No credential is stored: the request cookie is *built* from the token the
//! editor keeps in its VS Code global state (`state.vscdb`, key
//! `cursorAuth/accessToken`). On Windows that database is
//! `%APPDATA%\Cursor\User\globalStorage\state.vscdb`.
//!
//! The plan is two pools — Cursor's own models and everything else — reported
//! as percentages (0.0267 means 0.0267%). `plan.used/limit` is a different
//! denominator (cash value) and is only used when no pools are reported.

use std::path::PathBuf;

use async_trait::async_trait;
use base64::Engine;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;

use crate::model::{AccountKey, CreditAmount, ProviderUsage, Unavailability, UsageRoute, UsageWindow, WindowKind};
use crate::provider::Provider;
use crate::service::{FetchContext, UsageService};

const ENDPOINT: &str = "https://cursor.com/api/usage-summary";
const TOKEN_KEY: &str = "cursorAuth/accessToken";
/// Don't send a token this close to expiring.
const EXPIRY_HEADROOM_SECONDS: i64 = 60;
const CYCLE_SECONDS: i64 = 30 * 86_400;

#[derive(Default)]
pub struct Cursor;

#[async_trait]
impl UsageService for Cursor {
    fn provider(&self) -> Provider {
        Provider::Cursor
    }

    async fn fetch(&self, ctx: &FetchContext, account: &AccountKey) -> ProviderUsage {
        let unavailable = |reason| ProviderUsage::unavailable(account.clone(), reason);
        let token = access_token(&database(ctx));
        let Some(cookie) = token.as_deref().and_then(|t| session_cookie(t, ctx.now)) else {
            return unavailable(if token.is_some() { Unavailability::CursorLoginExpired } else { Unavailability::CursorSignInRequired });
        };

        // The shared client follows redirects; a cookie must never be carried to another host.
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .unwrap_or_else(|_| ctx.http.clone());
        let reply = match client.get(ENDPOINT).header("Cookie", cookie).header("Accept", "application/json").send().await {
            Ok(r) => r,
            Err(_) => return unavailable(Unavailability::Unreachable),
        };
        match reply.status().as_u16() {
            200 => {}
            401 | 403 => return unavailable(Unavailability::CursorLoginExpired),
            429 => return unavailable(Unavailability::RateLimited),
            _ => return unavailable(Unavailability::ServerError),
        }
        let Ok(body) = reply.bytes().await else { return unavailable(Unavailability::Unreachable) };
        match serde_json::from_slice::<Reply>(&body) {
            Ok(r) => reading(&r, account, ctx.now),
            Err(_) => unavailable(Unavailability::UnreadableReply),
        }
    }
}

fn database(ctx: &FetchContext) -> PathBuf {
    ctx.app_data.join("Cursor").join("User").join("globalStorage").join("state.vscdb")
}

/// The editor's token, opened read-only in place (Cursor keeps the file in WAL mode,
/// so a copy would miss the newest writes).
fn access_token(path: &std::path::Path) -> Option<String> {
    if !path.is_file() {
        return None;
    }
    let db = rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    let value: rusqlite::types::Value = db
        .query_row("SELECT value FROM ItemTable WHERE key = ?1 LIMIT 1", [TOKEN_KEY], |row| row.get(0))
        .ok()?;
    match value {
        rusqlite::types::Value::Text(t) => Some(t),
        rusqlite::types::Value::Blob(b) => text_in(&b),
        _ => None,
    }
    .map(|t| t.trim().trim_matches('"').to_string())
    .filter(|t| !t.is_empty())
}

/// A BLOB row is UTF-16LE without a BOM; read as UTF-8 it would carry a NUL after every character.
fn text_in(bytes: &[u8]) -> Option<String> {
    if bytes.len() >= 2 && bytes.len() % 2 == 0 && bytes[0] != 0 && bytes[1] == 0 {
        let wide: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        return String::from_utf16(&wide).ok();
    }
    String::from_utf8(bytes.to_vec()).ok()
}

fn claims(token: &str) -> Option<Value> {
    let mut parts = token.split('.');
    let (_, payload, _) = (parts.next()?, parts.next()?, parts.next()?);
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// `WorkosCursorSessionToken=<account>%3A%3A<token>`, where the account is the tail of `sub` after `|`.
pub fn session_cookie(token: &str, now: DateTime<Utc>) -> Option<String> {
    let claims = claims(token)?;
    let expiry = claims.get("exp")?.as_f64()?;
    if expiry as i64 - now.timestamp() <= EXPIRY_HEADROOM_SECONDS {
        return None;
    }
    let account = claims.get("sub")?.as_str()?.rsplit('|').next()?.to_string();
    if account.is_empty() {
        return None;
    }
    Some(format!("WorkosCursorSessionToken={account}%3A%3A{token}"))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Allowance {
    enabled: Option<bool>,
    used: Option<f64>,
    limit: Option<f64>,
    remaining: Option<f64>,
    auto_percent_used: Option<f64>,
    api_percent_used: Option<f64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Individual {
    plan: Option<Allowance>,
    on_demand: Option<Allowance>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Team {
    pooled: Option<Allowance>,
    on_demand: Option<Allowance>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Reply {
    billing_cycle_end: Option<String>,
    membership_type: Option<String>,
    individual_usage: Option<Individual>,
    team_usage: Option<Team>,
}

fn pool(percent: Option<f64>, id: &str, scope: &str, resets: Option<DateTime<Utc>>) -> Option<UsageWindow> {
    let percent = percent.filter(|p| p.is_finite())?;
    let mut w = UsageWindow::new(id, WindowKind::Monthly, (percent / 100.0).clamp(0.0, 1.0), CYCLE_SECONDS)
        .with_scope(scope)
        .with_reset(resets)
        .exhausted(percent >= 100.0);
    w.reports_length = false;
    Some(w)
}

fn money(allowance: Option<&Allowance>, id: &str, kind: WindowKind, resets: Option<DateTime<Utc>>) -> Option<UsageWindow> {
    let a = allowance?;
    if a.enabled == Some(false) {
        return None;
    }
    let (used, limit) = (a.used?, a.limit?);
    if limit <= 0.0 {
        return None;
    }
    let mut w = UsageWindow::new(id, kind, (used / limit).clamp(0.0, 1.0), CYCLE_SECONDS)
        .with_reset(resets)
        .exhausted(used >= limit);
    w.reports_length = false;
    Some(w)
}

fn reading(reply: &Reply, account: &AccountKey, now: DateTime<Utc>) -> ProviderUsage {
    let resets = reply
        .billing_cycle_end
        .as_deref()
        .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
        .map(|d| d.with_timezone(&Utc));
    let plan = reply
        .individual_usage
        .as_ref()
        .and_then(|i| i.plan.as_ref())
        .or_else(|| reply.team_usage.as_ref().and_then(|t| t.pooled.as_ref()));

    let mut windows: Vec<UsageWindow> = [
        pool(plan.and_then(|p| p.auto_percent_used), "cursorModels", "Cursor Models", resets),
        pool(plan.and_then(|p| p.api_percent_used), "otherModels", "Other Models", resets),
    ]
    .into_iter()
    .flatten()
    .collect();
    if windows.is_empty() {
        windows.extend(money(plan, "plan", WindowKind::Monthly, resets));
    }
    let on_demand = reply
        .individual_usage
        .as_ref()
        .and_then(|i| i.on_demand.as_ref())
        .or_else(|| reply.team_usage.as_ref().and_then(|t| t.on_demand.as_ref()));
    windows.extend(money(on_demand, "onDemand", WindowKind::Spend, resets));

    if windows.is_empty() {
        return ProviderUsage::unavailable(account.clone(), Unavailability::NoLimitsReported);
    }
    let mut usage = ProviderUsage::live(account.clone(), windows, now)
        .with_plan(plan_name(reply.membership_type.as_deref()))
        .with_origin(UsageRoute::LocalLogin);
    // `remaining` is in cents.
    if let Some(remaining) = plan.and_then(|p| p.remaining).filter(|r| r.is_finite()) {
        let dollars = remaining / 100.0;
        usage.credit_balance = Some(format!("${dollars:.2}"));
        usage.credit_remaining = Some(CreditAmount { amount: dollars, currency: "USD".into() });
    }
    usage
}

/// "pro_plus" → "Pro+", "free_trial" → "Free Trial".
pub fn plan_name(membership: Option<&str>) -> Option<String> {
    let membership = membership.filter(|m| !m.is_empty())?;
    let mut name = String::new();
    for part in membership.split('_') {
        if part == "plus" {
            name.push('+');
            continue;
        }
        let mut chars = part.chars();
        let Some(first) = chars.next() else { continue };
        if !name.is_empty() {
            name.push(' ');
        }
        name.extend(first.to_uppercase());
        name.push_str(&chars.as_str().to_lowercase());
    }
    Some(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::profile::test_support::fixture;
    use crate::model::UsageState;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-09-20T12:00:00Z").unwrap().with_timezone(&Utc)
    }
    fn account() -> AccountKey {
        AccountKey::primary(Provider::Cursor)
    }
    fn parse(name: &str) -> ProviderUsage {
        reading(&serde_json::from_slice(&fixture(name)).unwrap(), &account(), now())
    }

    #[test]
    fn normal_reply_has_two_pools() {
        let usage = parse("cursor-normal.json");
        let scopes: Vec<_> = usage.windows.iter().filter_map(|w| w.scope.as_deref()).collect();
        assert!(scopes.contains(&"Cursor Models") && scopes.contains(&"Other Models"));
        assert!(usage.windows.iter().all(|w| !w.reports_length));
    }

    #[test]
    fn every_fixture_parses_without_panicking() {
        for name in ["cursor-exhausted.json", "cursor-fallback-no-pools.json", "cursor-missing-fields.json", "cursor-ondemand.json", "cursor-team.json"] {
            let usage = parse(name);
            assert!(matches!(usage.state, UsageState::Live | UsageState::Unavailable(_)), "{name}");
        }
        assert!(parse("cursor-exhausted.json").windows.iter().any(|w| w.is_exhausted));
        assert!(parse("cursor-ondemand.json").windows.iter().any(|w| w.kind == WindowKind::Spend));
    }

    #[test]
    fn plan_names() {
        assert_eq!(plan_name(Some("pro_plus")).as_deref(), Some("Pro+"));
        assert_eq!(plan_name(Some("free_trial")).as_deref(), Some("Free Trial"));
        assert_eq!(plan_name(Some("")), None);
    }

    #[test]
    fn cookie_from_token_and_expiry_headroom() {
        let enc = |v: &str| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v);
        let token = format!("h.{}.s", enc(&format!(r#"{{"sub":"auth0|user_42","exp":{}}}"#, now().timestamp() + 3600)));
        assert_eq!(session_cookie(&token, now()), Some(format!("WorkosCursorSessionToken=user_42%3A%3A{token}")));
        let soon = format!("h.{}.s", enc(&format!(r#"{{"sub":"a|b","exp":{}}}"#, now().timestamp() + 30)));
        assert_eq!(session_cookie(&soon, now()), None);
    }

    #[test]
    fn utf16_blob_reads_as_text() {
        let bytes: Vec<u8> = "abc".encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        assert_eq!(text_in(&bytes).as_deref(), Some("abc"));
        assert_eq!(text_in(b"abc").as_deref(), Some("abc"));
    }
}
