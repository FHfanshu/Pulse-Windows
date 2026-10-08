// Ported from upstream Providers/Profiled/AmpUsageService.swift.
//! Amp: the free daily allowance, a paid tier's monthly agent and Orb
//! allowances, and the individual credit balance.
//!
//! Read with a pasted access token from the read-only RPC Amp's own CLI calls
//! for `amp usage`. The reply carries only `result.displayText`, the lines that
//! command prints, so those lines are what is read. Every figure drawn is one
//! the text states: dollars and hours rather than the rounded percentages
//! beside them. Amp's "time to full" is an estimate, not a reset, and is not read.
//!
//! The lines are read without a regex crate: each shape is a small `Cursor`
//! walk, case-insensitive, matching the upstream patterns.

use chrono::{DateTime, NaiveDate, Utc};
use serde::Deserialize;

use async_trait::async_trait;

use super::profile::{self, Fail};
use crate::model::{AccountKey, CreditAmount, ProviderUsage, Unavailability, UsageWindow, WindowKind};
use crate::provider::Provider;
use crate::service::{FetchContext, UsageService};

const ENDPOINT: &str = "https://ampcode.com/api/internal?userDisplayBalanceInfo";
/// A read-only RPC: it names the method and passes nothing.
const BODY: &str = r#"{"method":"userDisplayBalanceInfo","params":{}}"#;
const DAY: i64 = 86_400;
/// A paid allowance renews with the billing period, which is not a fixed
/// length; thirty days is only a sort key.
const MONTH: i64 = 30 * DAY;

#[derive(Default)]
pub struct Amp;

#[async_trait]
impl UsageService for Amp {
    fn provider(&self) -> Provider {
        Provider::Amp
    }

    async fn fetch(&self, ctx: &FetchContext, account: &AccountKey) -> ProviderUsage {
        let Some(token) = ctx.api_key(account) else {
            return ProviderUsage::unavailable(account.clone(), Unavailability::ApiKeyMissing);
        };
        let token = token.trim().to_string();
        let body = match profile::send(ctx, request(ctx, &token), Unavailability::ApiKeyRefused).await {
            Ok(body) => body,
            Err(reason) => return ProviderUsage::unavailable(account.clone(), reason),
        };
        reading(&body, ctx, account).unwrap_or_else(|reason| ProviderUsage::unavailable(account.clone(), reason))
    }
}

fn request(ctx: &FetchContext, token: &str) -> reqwest::RequestBuilder {
    ctx.http
        .post(ENDPOINT)
        .bearer_auth(token)
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .body(BODY)
}

// MARK: - Reading the reply

#[derive(Deserialize)]
struct Reply {
    ok: Option<bool>,
    result: Option<ResultBody>,
    error: Option<Failure>,
}

#[derive(Deserialize)]
struct ResultBody {
    #[serde(rename = "displayText")]
    display_text: Option<String>,
}

#[derive(Deserialize)]
struct Failure {
    code: Option<String>,
}

/// The envelope: a dead token is an `auth-required` error, not a status code.
pub fn reading(body: &[u8], ctx: &FetchContext, account: &AccountKey) -> std::result::Result<ProviderUsage, Fail> {
    let reply: Reply = profile::decode(body)?;
    if reply.ok == Some(false) {
        let refused = reply.error.and_then(|e| e.code).as_deref() == Some("auth-required");
        return Err(if refused { Unavailability::ApiKeyRefused } else { Unavailability::ServerError });
    }
    let text = match (reply.ok, reply.result.and_then(|r| r.display_text)) {
        (Some(true), Some(text)) if !text.is_empty() => text,
        _ => return Err(Unavailability::UnreadableReply),
    };
    reading_text(&text, ctx, account)
}

/// The lines Amp prints. A line this build does not know is skipped, never guessed at.
pub fn reading_text(text: &str, ctx: &FetchContext, account: &AccountKey) -> std::result::Result<ProviderUsage, Fail> {
    let cleaned = clean(text);
    let lines: Vec<&str> = cleaned.lines().collect();
    let mut windows: Vec<UsageWindow> = Vec::new();
    let mut plan: Option<String> = None;
    let mut credits: Option<f64> = None;

    // The dollar form is exact; the percentage one is rounded, so it counts only without it.
    let free = lines.iter().copied().find_map(free_dollars).or_else(|| lines.iter().copied().find_map(free_percent));
    windows.extend(free);

    for line in &lines {
        if let Some((name, found)) = tier(line) {
            plan = plan.or(Some(name));
            windows.extend(found);
        } else if let Some((name, found)) = subscription(line) {
            plan = plan.or(Some(name));
            windows.extend(found);
        } else if credits.is_none() {
            credits = individual_credits(line).filter(|v| *v >= 0.0);
        }
    }

    if windows.is_empty() && credits.is_none() {
        return Err(if looks_signed_out(text) { Unavailability::ApiKeyRefused } else { Unavailability::UnreadableReply });
    }
    // Tier and subscription lines describe the same allowance; the first one printed stands.
    let mut seen = std::collections::HashSet::new();
    windows.retain(|w| seen.insert(w.id.clone()));
    windows.sort_by_key(|w| w.window_seconds);

    let mut usage = profile::reading(account, windows, ctx).with_plan(plan);
    if let Some(amount) = credits {
        usage.credit_balance = Some(dollars(amount));
        usage.credit_remaining = Some(CreditAmount { amount, currency: "USD".to_string() });
    }
    Ok(usage)
}

// MARK: - The lines

/// "Amp Free: $6/$10 remaining (replenishes +$0.5/hour)". It refills hourly and never
/// turns over, so it has no reset and no stated length.
fn free_dollars(line: &str) -> Option<UsageWindow> {
    let mut c = Cursor::new(line);
    c.ws();
    if !c.eat("Amp Free:") {
        return None;
    }
    c.ws();
    c.eat("$");
    let remaining = c.number()?;
    c.ws();
    if !c.eat("/") {
        return None;
    }
    c.ws();
    c.eat("$");
    let limit = c.number()?;
    if !c.ws() || !c.eat("remaining") {
        return None;
    }
    let used = used_fraction(remaining, limit)?;
    let mut window = UsageWindow::new("amp.free", WindowKind::Credits, used, DAY).with_scope("Amp Free");
    window.reports_length = false;
    Some(window.exhausted(used >= 1.0))
}

/// "Amp Free: 61% remaining today (resets daily)". Daily is stated; the hour it turns
/// over is not, so no reset is given.
fn free_percent(line: &str) -> Option<UsageWindow> {
    let mut c = Cursor::new(line);
    c.ws();
    if !c.eat("Amp Free:") {
        return None;
    }
    c.ws();
    let remaining = c.number()?;
    c.ws();
    if !c.eat("%") || !c.ws() || !c.eat("remaining") {
        return None;
    }
    if remaining < 0.0 {
        return None;
    }
    // Optional " today" and " (resets daily)"; either one states the daily length.
    let mut reports = false;
    let mut after = c;
    let mut today = c;
    if today.ws() && today.eat("today") {
        reports = true;
        after = today;
    }
    let mut daily = after;
    daily.ws();
    if daily.eat("(resets daily)") {
        reports = true;
    }
    let used = (100.0 - remaining.min(100.0)).max(0.0) / 100.0;
    let mut window = UsageWindow::new("amp.free", WindowKind::Daily, used, DAY).with_scope("Amp Free");
    window.reports_length = reports;
    Some(window.exhausted(used >= 1.0))
}

/// "Amp Megawatt Tier: agent usage $18.57 of $20 remaining (93%), orb usage 732.8h of
/// 750h a1.small orb hours remaining (98%) - period 2026-09-13 to 2026-10-13, ...".
fn tier(line: &str) -> Option<(String, Vec<UsageWindow>)> {
    let mut c = Cursor::new(line);
    c.ws();
    if !c.eat("Amp") || !c.ws() {
        return None;
    }
    let head = c.rest;
    let lower = head.to_ascii_lowercase();
    // The plan name is the shortest text before " Tier:" for which the rest of the line fits.
    for (idx, _) in lower.match_indices("tier:") {
        if idx == 0 || !head[..idx].ends_with(char::is_whitespace) {
            continue;
        }
        let name = head[..idx].trim();
        if name.is_empty() {
            continue;
        }
        let mut t = Cursor::new(&head[idx + "tier:".len()..]);
        t.ws();
        if !t.eat("agent usage") || !t.ws() || !t.eat("$") {
            continue;
        }
        let Some(agent_remaining) = t.number() else { continue };
        if !t.ws() || !t.eat("of") || !t.ws() || !t.eat("$") {
            continue;
        }
        let Some(agent_limit) = t.number() else { continue };
        if !t.ws() || !t.eat("remaining") || t.rest.chars().next().is_some_and(is_word) {
            continue;
        }
        let rest = t.rest;
        let resets_at = period_end(rest);
        let mut windows = Vec::new();
        if let Some(used) = used_fraction(agent_remaining, agent_limit) {
            windows.push(monthly("amp.agent", None, used, resets_at));
        }
        // Only the unit Amp names its allowance in; another size of machine is another allowance.
        if let Some(used) = orb_used(rest) {
            windows.push(monthly("amp.orb", Some("Orb"), used, resets_at));
        }
        return Some((name.to_string(), windows));
    }
    None
}

/// The older wording, in percentages remaining: "Amp Megawatt Subscription: 68% other
/// usage and 97% orb usage remaining - resets upon renewal in 5 days", or
/// "Subscription Megawatt: ...".
fn subscription(line: &str) -> Option<(String, Vec<UsageWindow>)> {
    let mut c = Cursor::new(line);
    c.ws();
    if c.eat("Amp") && c.ws() {
        let head = c.rest;
        let lower = head.to_ascii_lowercase();
        for (idx, _) in lower.match_indices("subscription:") {
            if idx == 0 || !head[..idx].ends_with(char::is_whitespace) {
                continue;
            }
            let name = head[..idx].trim();
            if name.is_empty() {
                continue;
            }
            if let Some(found) = subscription_tail(&head[idx + "subscription".len()..]) {
                return Some(subscription_reading(name, found));
            }
        }
    }
    let mut c = Cursor::new(line);
    c.ws();
    if c.eat("Subscription") && c.ws() {
        let head = c.rest;
        for (idx, _) in head.match_indices(':') {
            let name = head[..idx].trim();
            if name.is_empty() {
                continue;
            }
            if let Some(found) = subscription_tail(&head[idx..]) {
                return Some(subscription_reading(name, found));
            }
        }
    }
    None
}

/// Text starting at the colon: "NN% other usage and NN% orb usage remaining".
fn subscription_tail(text: &str) -> Option<(f64, f64)> {
    let mut c = Cursor::new(text);
    if !c.eat(":") {
        return None;
    }
    c.ws();
    let agent = c.number()?;
    c.ws();
    if !c.eat("%") || !c.ws() || !c.eat("other usage and") || !c.ws() {
        return None;
    }
    let orb = c.number()?;
    c.ws();
    if !c.eat("%") || !c.ws() || !c.eat("orb usage remaining") {
        return None;
    }
    Some((agent, orb))
}

fn subscription_reading(name: &str, (agent, orb): (f64, f64)) -> (String, Vec<UsageWindow>) {
    let mut windows = Vec::new();
    if agent >= 0.0 {
        windows.push(monthly("amp.agent", None, percent_used(agent), None));
    }
    if orb >= 0.0 {
        windows.push(monthly("amp.orb", Some("Orb"), percent_used(orb), None));
    }
    (name.to_string(), windows)
}

/// "Individual credits: $1,020.50 remaining (replenishes automatically)".
fn individual_credits(line: &str) -> Option<f64> {
    let mut c = Cursor::new(line);
    c.ws();
    if !c.eat("Individual credits:") {
        return None;
    }
    c.ws();
    c.eat("$");
    let amount = c.number()?;
    if !c.ws() || !c.eat("remaining") {
        return None;
    }
    Some(amount)
}

/// A paid allowance: stated dates only, so no reset is given beyond the period end.
fn monthly(id: &str, scope: Option<&str>, used: f64, resets_at: Option<DateTime<Utc>>) -> UsageWindow {
    let mut window = UsageWindow::new(id, WindowKind::Monthly, used, MONTH).with_reset(resets_at);
    window.reports_length = false;
    if let Some(scope) = scope {
        window = window.with_scope(scope);
    }
    window.exhausted(used >= 1.0)
}

/// "period 2026-09-13 to 2026-10-13": the end date, as the start of that day in UTC.
/// The countdown beside it is rounded and moves each refresh, so it is not used.
fn period_end(text: &str) -> Option<DateTime<Utc>> {
    let lower = text.to_ascii_lowercase();
    let (idx, _) = lower.match_indices("period").next()?;
    if text[..idx].chars().next_back().is_some_and(is_word) {
        return None;
    }
    let mut c = Cursor::new(&text[idx + "period".len()..]);
    if !c.ws() {
        return None;
    }
    let start = c.date()?;
    if !c.ws() || !c.eat("to") || !c.ws() {
        return None;
    }
    let end = c.date()?;
    if c.rest.chars().next().is_some_and(is_word) {
        return None;
    }
    let start = NaiveDate::parse_from_str(start, "%Y-%m-%d").ok()?;
    let end = NaiveDate::parse_from_str(end, "%Y-%m-%d").ok()?;
    if end <= start {
        return None;
    }
    Some(end.and_hms_opt(0, 0, 0)?.and_utc())
}

/// "orb usage 732.8h of 750h a1.small orb hours remaining": the used fraction of
/// the a1.small Orb allowance, or nothing when another size is named.
fn orb_used(text: &str) -> Option<f64> {
    let lower = text.to_ascii_lowercase();
    let (idx, _) = lower.match_indices("orb usage").next()?;
    if text[..idx].chars().next_back().is_some_and(is_word) {
        return None;
    }
    let mut c = Cursor::new(&text[idx + "orb usage".len()..]);
    if !c.ws() {
        return None;
    }
    let remaining = c.number()?;
    if !c.eat("h") || !c.ws() || !c.eat("of") || !c.ws() {
        return None;
    }
    let limit = c.number()?;
    if !c.eat("h") || !c.ws() || !c.eat("a1.small orb hours remaining") {
        return None;
    }
    if c.rest.chars().next().is_some_and(is_word) {
        return None;
    }
    used_fraction(remaining, limit)
}

// MARK: - Helpers

/// Used fraction from what is left of a stated limit. More left than the limit is
/// nothing used, not a negative.
fn used_fraction(remaining: f64, limit: f64) -> Option<f64> {
    (remaining.is_finite() && limit.is_finite() && remaining >= 0.0 && limit > 0.0)
        .then(|| (limit - remaining).max(0.0) / limit)
}

/// Used fraction from a percentage remaining.
fn percent_used(remaining: f64) -> f64 {
    (100.0 - remaining.min(100.0)).max(0.0) / 100.0
}

fn dollars(amount: f64) -> String {
    format!("USD {amount:.2}")
}

/// Strips terminal colour codes and Markdown bold, which the text may carry.
fn clean(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\u{1b}' && chars.get(i + 1) == Some(&'[') {
            let mut j = i + 2;
            while j < chars.len() && (chars[j].is_ascii_digit() || chars[j] == ';') {
                j += 1;
            }
            if j < chars.len() && chars[j].is_ascii_alphabetic() {
                i = j + 1;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out.replace("**", "")
}

fn looks_signed_out(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    !lower.contains("signed in as") && (lower.contains("sign in") || lower.contains("log in") || lower.contains("login"))
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// A forward-only reader over one line. Literals match case-insensitively, as
/// the upstream patterns do.
#[derive(Clone, Copy)]
struct Cursor<'a> {
    rest: &'a str,
}

impl<'a> Cursor<'a> {
    fn new(text: &'a str) -> Self {
        Self { rest: text }
    }

    /// Skips whitespace; true when there was some.
    fn ws(&mut self) -> bool {
        let trimmed = self.rest.trim_start();
        let skipped = trimmed.len() != self.rest.len();
        self.rest = trimmed;
        skipped
    }

    /// Consumes `literal` if the text starts with it, ignoring ASCII case.
    fn eat(&mut self, literal: &str) -> bool {
        match self.rest.get(..literal.len()) {
            Some(head) if head.eq_ignore_ascii_case(literal) => {
                self.rest = &self.rest[literal.len()..];
                true
            }
            _ => false,
        }
    }

    /// `[0-9][0-9,]*(\.[0-9]+)?`, with commas taken out before reading.
    fn number(&mut self) -> Option<f64> {
        let bytes = self.rest.as_bytes();
        if !bytes.first()?.is_ascii_digit() {
            return None;
        }
        let mut end = 1;
        while end < bytes.len() && (bytes[end].is_ascii_digit() || bytes[end] == b',') {
            end += 1;
        }
        if end + 1 < bytes.len() && bytes[end] == b'.' && bytes[end + 1].is_ascii_digit() {
            end += 1;
            while end < bytes.len() && bytes[end].is_ascii_digit() {
                end += 1;
            }
        }
        let text = &self.rest[..end];
        self.rest = &self.rest[end..];
        text.replace(',', "").parse().ok()
    }

    /// `\d{4}-\d{2}-\d{2}`, returned as written.
    fn date(&mut self) -> Option<&'a str> {
        let bytes = self.rest.as_bytes();
        let shape = bytes.len() >= 10
            && bytes[..4].iter().all(u8::is_ascii_digit)
            && bytes[4] == b'-'
            && bytes[5..7].iter().all(u8::is_ascii_digit)
            && bytes[7] == b'-'
            && bytes[8..10].iter().all(u8::is_ascii_digit);
        if !shape {
            return None;
        }
        let (date, rest) = self.rest.split_at(10);
        self.rest = rest;
        Some(date)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::UsageState;
    use crate::providers::profile::test_support::{context, fixture};

    fn account() -> AccountKey {
        AccountKey::primary(Provider::Amp)
    }

    fn envelope(text: &str) -> Vec<u8> {
        serde_json::json!({"ok": true, "result": {"displayText": text}}).to_string().into_bytes()
    }

    #[test]
    fn a_paid_tier_reads_its_agent_dollars_and_orb_hours_beside_amp_free_and_the_credits() {
        let ctx = context();
        let usage = reading(&fixture("amp-balance-tier.json"), &ctx, &account()).unwrap();

        assert_eq!(usage.state, UsageState::Live);
        assert_eq!(usage.account, account());
        assert_eq!(usage.plan.as_deref(), Some("Megawatt"));

        let free = &usage.windows[0];
        assert_eq!(free.id, "amp.free");
        assert_eq!(free.kind, WindowKind::Daily);
        assert_eq!(free.scope.as_deref(), Some("Amp Free"));
        assert_eq!(free.used_fraction, 0.39);
        assert!(free.reports_length);
        // Daily is stated; the hour it turns over is not.
        assert_eq!(free.resets_at, None);

        let reset = profile::date(Some("2026-10-13T00:00:00Z"));
        let agent = usage.windows.iter().find(|w| w.id == "amp.agent").unwrap();
        assert_eq!(agent.kind, WindowKind::Monthly);
        assert_eq!(agent.scope, None);
        assert!((agent.used_fraction - 1.43 / 20.0).abs() < 1e-9);
        assert!(!agent.reports_length);
        assert_eq!(agent.resets_at, reset);

        let orb = usage.windows.iter().find(|w| w.id == "amp.orb").unwrap();
        assert_eq!(orb.scope.as_deref(), Some("Orb"));
        assert!((orb.used_fraction - 17.2 / 750.0).abs() < 1e-9);
        assert_eq!(orb.resets_at, reset);

        assert_eq!(usage.credit_remaining, Some(CreditAmount { amount: 1_020.5, currency: "USD".into() }));
        assert_eq!(usage.credit_balance.as_deref(), Some("USD 1020.50"));
    }

    #[test]
    fn the_older_wording_reads_too_dollars_win_over_the_rounded_free_percentage_and_workspaces_are_left_off() {
        let ctx = context();
        let usage = reading(&fixture("amp-balance-legacy.json"), &ctx, &account()).unwrap();

        assert_eq!(usage.state, UsageState::Live);
        assert_eq!(usage.plan.as_deref(), Some("Megawatt"));
        let free = usage.windows.iter().find(|w| w.id == "amp.free").unwrap();
        // Refilled by the hour: no reset and no length to claim.
        assert_eq!(free.kind, WindowKind::Credits);
        assert_eq!(free.used_fraction, 0.4);
        assert!(!free.reports_length);
        assert_eq!(free.resets_at, None);
        assert_eq!(usage.windows.iter().find(|w| w.id == "amp.agent").unwrap().used_fraction, 0.03);
        assert_eq!(usage.windows.iter().find(|w| w.id == "amp.orb").unwrap().used_fraction, 0.0);
        // The countdown alone is rounded and moves; it is not a reset.
        assert!(usage.windows.iter().all(|w| w.resets_at.is_none()));
        assert_eq!(usage.credit_balance, None);
        assert_eq!(usage.credit_remaining, None);
    }

    #[test]
    fn a_limit_of_nothing_is_left_off_and_the_credits_beside_it_still_show() {
        let ctx = context();
        let text = "Amp Megawatt Tier: agent usage $0 of $0 remaining - resets upon renewal in 27 days\nIndividual credits: $12 remaining";
        let usage = reading(&envelope(text), &ctx, &account()).unwrap();
        assert_eq!(usage.state, UsageState::Live);
        assert!(usage.windows.is_empty());
        assert_eq!(usage.credit_remaining, Some(CreditAmount { amount: 12.0, currency: "USD".into() }));
    }

    #[test]
    fn more_left_than_the_limit_is_nothing_used_not_a_negative() {
        let ctx = context();
        let text = "Amp Example Tier: agent usage $1,100 of $1,000 remaining - resets upon renewal in 1 month";
        let usage = reading(&envelope(text), &ctx, &account()).unwrap();
        assert_eq!(usage.windows.iter().map(|w| w.used_fraction).collect::<Vec<_>>(), vec![0.0]);
    }

    #[test]
    fn only_a1_small_orb_hours_are_an_allowance_pulse_reads() {
        let ctx = context();
        let text = "Amp Example Tier: agent usage $3 of $20 remaining (15%), orb usage 12h of 50h a1.large orb hours remaining - resets upon renewal in 2 days";
        let usage = reading(&envelope(text), &ctx, &account()).unwrap();
        assert_eq!(usage.windows.iter().map(|w| w.id.as_str()).collect::<Vec<_>>(), vec!["amp.agent"]);
        assert_eq!(usage.windows[0].used_fraction, 0.85);
    }

    #[test]
    fn a_dead_token_is_refused_whether_amp_says_so_in_the_envelope_or_in_the_text() {
        let ctx = context();
        let envelope_refused = br#"{"ok":false,"error":{"code":"auth-required","message":"Sign in"}}"#;
        assert_eq!(reading(envelope_refused, &ctx, &account()).unwrap_err(), Unavailability::ApiKeyRefused);
        assert_eq!(reading(&envelope("Please log in to use Amp."), &ctx, &account()).unwrap_err(), Unavailability::ApiKeyRefused);
    }

    #[test]
    fn any_other_failure_in_the_envelope_is_the_services() {
        let ctx = context();
        let envelope_failed = br#"{"ok":false,"error":{"code":"internal","message":"oops"}}"#;
        assert_eq!(reading(envelope_failed, &ctx, &account()).unwrap_err(), Unavailability::ServerError);
    }

    #[test]
    fn a_reply_that_isnt_one_cant_be_read() {
        let ctx = context();
        let cases: [&[u8]; 5] = [
            b"not json",
            br#"{"ok":true}"#,
            br#"{"ok":true,"result":{"displayText":""}}"#,
            br#"{"ok":true,"result":{"displayText":"Signed in as a@b.c\nSomething new"}}"#,
            br#"{"result":{"displayText":"Amp Free: 61% remaining today"}}"#,
        ];
        for body in cases {
            assert_eq!(reading(body, &ctx, &account()).unwrap_err(), Unavailability::UnreadableReply, "{}", String::from_utf8_lossy(body));
        }
    }

    #[test]
    fn the_request_is_the_read_only_rpc_with_the_token_as_a_bearer_to_amp_only() {
        let built = request(&context(), "abc").build().unwrap();
        assert_eq!(built.method().as_str(), "POST");
        assert_eq!(built.url().host_str(), Some("ampcode.com"));
        assert_eq!(built.headers().get("Authorization").unwrap(), "Bearer abc");
        let body: serde_json::Value = serde_json::from_slice(built.body().unwrap().as_bytes().unwrap()).unwrap();
        assert_eq!(body["method"], "userDisplayBalanceInfo");
    }
}
