// Ported from upstream Providers/Profiled/WindsurfUsageService.swift.
//! Windsurf's daily and weekly quota, each a percentage remaining that
//! Windsurf states, read from the endpoint its own profile page calls
//! (Connect over protobuf).
//!
//! Not Devin's route, deliberately: Devin's provider already reads the other
//! ways to this plan, and reading them again would count one account twice.
//!
//! The credential is the four `devin_*` values windsurf.com keeps in
//! `localStorage`, saved as one JSON object. They are sent only to windsurf.com.

use std::time::Duration;

use async_trait::async_trait;
use chrono::DateTime;
use serde_json::Value;

use super::profile;
use crate::model::{AccountKey, ProviderUsage, Unavailability, UsageWindow, WindowKind};
use crate::provider::Provider;
use crate::service::{FetchContext, UsageService};

const ENDPOINT: &str = "https://windsurf.com/_backend/exa.seat_management_pb.SeatManagementService/GetPlanStatus";

#[derive(Default)]
pub struct Windsurf;

#[async_trait]
impl UsageService for Windsurf {
    fn provider(&self) -> Provider {
        Provider::Windsurf
    }

    async fn fetch(&self, ctx: &FetchContext, account: &AccountKey) -> ProviderUsage {
        let Some(session) = ctx.api_key(account).as_deref().and_then(Session::parse) else {
            return ProviderUsage::unavailable(account.clone(), Unavailability::SessionMissing);
        };
        match profile::send(ctx, request(ctx, &session), Unavailability::SessionExpired).await {
            Err(reason) => ProviderUsage::unavailable(account.clone(), reason),
            Ok(body) => reading(&body, ctx, account)
                .unwrap_or_else(|reason| ProviderUsage::unavailable(account.clone(), reason)),
        }
    }
}

// MARK: - The session

/// The four values windsurf.com keeps in `localStorage`. All four are needed.
#[derive(Debug, PartialEq, Eq)]
struct Session {
    token: String,
    auth1: String,
    account_id: String,
    organization_id: String,
}

impl Session {
    /// `{"devin_session_token": ..., "devin_auth1_token": ...,
    /// "devin_account_id": ..., "devin_primary_org_id": ...}`. Any one missing
    /// or blank and there is no session.
    fn parse(text: &str) -> Option<Self> {
        let value: Value = serde_json::from_str(text).ok()?;
        let object = value.as_object()?;
        let field = |key: &str| {
            object
                .get(key)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_string)
        };
        Some(Self {
            token: field("devin_session_token")?,
            auth1: field("devin_auth1_token")?,
            account_id: field("devin_account_id")?,
            organization_id: field("devin_primary_org_id")?,
        })
    }
}

/// The headers windsurf.com's profile page sends, and a body of two fields:
/// the session token (1) and "include top-up status" (2), which the page sets.
fn request(ctx: &FetchContext, session: &Session) -> reqwest::RequestBuilder {
    let mut body = Vec::new();
    write_key(1, 2, &mut body);
    write_varint(session.token.len() as u64, &mut body);
    body.extend_from_slice(session.token.as_bytes());
    write_key(2, 0, &mut body);
    write_varint(1, &mut body);

    ctx.http
        .post(ENDPOINT)
        .timeout(Duration::from_secs(15))
        .header("Content-Type", "application/proto")
        .header("Connect-Protocol-Version", "1")
        .header("Origin", "https://windsurf.com")
        .header("Referer", "https://windsurf.com/profile")
        .header("x-auth-token", &session.token)
        .header("x-devin-session-token", &session.token)
        .header("x-devin-auth1-token", &session.auth1)
        .header("x-devin-account-id", &session.account_id)
        .header("x-devin-primary-org-id", &session.organization_id)
        .body(body)
}

// MARK: - Reading the reply

/// What is read out of `plan_status` (field 1 of the reply).
#[derive(Debug, Default, PartialEq, Eq)]
struct PlanStatus {
    plan_name: Option<String>,
    daily_remaining: Option<u64>,
    weekly_remaining: Option<u64>,
    daily_reset_at: Option<u64>,
    weekly_reset_at: Option<u64>,
}

/// `plan_status` fields: 1 `plan_info` (whose 2 is the plan's name), 14 and 15
/// the daily and weekly percentage **remaining**, 17 and 18 their resets in
/// Unix seconds. Everything else is skipped.
fn plan_status(data: &[u8]) -> Option<PlanStatus> {
    let mut reply = Reader::new(data);
    let mut found: Option<Vec<u8>> = None;
    while let Some(field) = reply.next() {
        if let (1, WireValue::Bytes(bytes)) = (field.number, field.value) {
            found = Some(bytes);
        }
    }
    if !reply.complete {
        return None;
    }
    let found = found?;

    let mut status = PlanStatus::default();
    let mut reader = Reader::new(&found);
    while let Some(field) = reader.next() {
        match (field.number, field.value) {
            (1, WireValue::Bytes(info)) => {
                let mut plan = Reader::new(&info);
                while let Some(inner) = plan.next() {
                    if let (2, WireValue::Bytes(name)) = (inner.number, inner.value) {
                        status.plan_name = String::from_utf8(name).ok();
                    }
                }
            }
            (14, WireValue::Varint(value)) => status.daily_remaining = Some(value),
            (15, WireValue::Varint(value)) => status.weekly_remaining = Some(value),
            (17, WireValue::Varint(value)) => status.daily_reset_at = Some(value),
            (18, WireValue::Varint(value)) => status.weekly_reset_at = Some(value),
            _ => {}
        }
    }
    reader.complete.then_some(status)
}

fn reading(body: &[u8], ctx: &FetchContext, account: &AccountKey) -> Result<ProviderUsage, Unavailability> {
    let status = plan_status(body).ok_or(Unavailability::UnreadableReply)?;

    // Protobuf does not write a zero, so a quota at 0% and a quota the plan
    // does not have look alike: an absent one is left off, not guessed spent.
    let quotas = [
        ("daily", WindowKind::Daily, 86_400, status.daily_remaining, status.daily_reset_at),
        ("weekly", WindowKind::Weekly, 7 * 86_400, status.weekly_remaining, status.weekly_reset_at),
    ];
    let windows: Vec<UsageWindow> = quotas
        .into_iter()
        .filter_map(|(id, kind, seconds, remaining, reset)| {
            let remaining = remaining.filter(|r| *r <= 100)?;
            let used = (100 - remaining) as f64 / 100.0;
            let resets_at = reset.filter(|r| *r > 0).and_then(|r| DateTime::from_timestamp(r as i64, 0));
            Some(
                UsageWindow::new(format!("windsurf.{id}"), kind, used, seconds)
                    .with_reset(resets_at)
                    .exhausted(remaining == 0),
            )
        })
        .collect();

    if windows.is_empty() {
        return Err(Unavailability::NoLimitsReported);
    }
    let plan = status.plan_name.as_deref().map(str::trim).filter(|p| !p.is_empty()).map(str::to_string);
    Ok(profile::reading(account, windows, ctx).with_plan(plan))
}

// MARK: - Protobuf

/// Just enough protobuf to read a reply and write a request: varints and
/// length-delimited fields, with the fixed-width ones skipped.
#[derive(Debug, PartialEq)]
enum WireValue {
    Varint(u64),
    Bytes(Vec<u8>),
    Skipped,
}

struct Field {
    number: u64,
    value: WireValue,
}

struct Reader<'a> {
    bytes: &'a [u8],
    index: usize,
    /// False once anything could not be read; a reader that stopped early has
    /// not read the message.
    complete: bool,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, index: 0, complete: true }
    }

    fn next(&mut self) -> Option<Field> {
        if !self.complete || self.index >= self.bytes.len() {
            return None;
        }
        let Some(key) = self.varint() else { return self.fail() };
        let number = key >> 3;
        if number == 0 {
            return self.fail();
        }
        match key & 0x07 {
            0 => match self.varint() {
                Some(value) => Some(Field { number, value: WireValue::Varint(value) }),
                None => self.fail(),
            },
            1 => self.skip(8, number),
            2 => {
                let Some(length) = self.varint() else { return self.fail() };
                if length > (self.bytes.len() - self.index) as u64 {
                    return self.fail();
                }
                let start = self.index;
                self.index += length as usize;
                Some(Field { number, value: WireValue::Bytes(self.bytes[start..self.index].to_vec()) })
            }
            5 => self.skip(4, number),
            _ => self.fail(),
        }
    }

    fn skip(&mut self, count: usize, number: u64) -> Option<Field> {
        if self.bytes.len() - self.index < count {
            return self.fail();
        }
        self.index += count;
        Some(Field { number, value: WireValue::Skipped })
    }

    fn fail(&mut self) -> Option<Field> {
        self.complete = false;
        None
    }

    fn varint(&mut self) -> Option<u64> {
        let mut result = 0u64;
        let mut shift = 0u32;
        while self.index < self.bytes.len() && shift < 64 {
            let byte = self.bytes[self.index];
            self.index += 1;
            result |= u64::from(byte & 0x7F) << shift;
            if byte & 0x80 == 0 {
                return Some(result);
            }
            shift += 7;
        }
        None
    }
}

fn write_key(number: u64, wire: u64, out: &mut Vec<u8>) {
    write_varint((number << 3) | wire, out);
}

fn write_varint(mut value: u64, out: &mut Vec<u8>) {
    while value >= 0x80 {
        out.push((value as u8 & 0x7F) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::profile::test_support::{context, fixture};

    fn account() -> AccountKey {
        AccountKey::primary(Provider::Windsurf)
    }

    const SESSION: &str = r#"{"devin_session_token":"devin-session-token$abc","devin_auth1_token":"auth1_xyz","devin_account_id":"account-123","devin_primary_org_id":"org-456"}"#;

    /// Decode the fixture's base64 body (the crate has no base64 dependency).
    fn fixture_reply() -> Vec<u8> {
        let json: Value = serde_json::from_slice(&fixture("windsurf-plan-status.json")).unwrap();
        let text = json["base64"].as_str().unwrap();
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let (mut out, mut buffer, mut bits) = (Vec::new(), 0u32, 0u32);
        for c in text.bytes().filter(|c| *c != b'=') {
            let value = ALPHABET.iter().position(|a| *a == c).unwrap() as u32;
            buffer = (buffer << 6) | value;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push((buffer >> bits) as u8);
                buffer &= (1 << bits) - 1;
            }
        }
        out
    }

    /// A reply whose `plan_status` holds the given varint fields.
    fn reply(fields: &[(u64, u64)]) -> Vec<u8> {
        let mut status = Vec::new();
        for (number, value) in fields {
            write_key(*number, 0, &mut status);
            write_varint(*value, &mut status);
        }
        let mut reply = Vec::new();
        write_key(1, 2, &mut reply);
        write_varint(status.len() as u64, &mut reply);
        reply.extend_from_slice(&status);
        reply
    }

    #[test]
    fn daily_and_weekly_are_used_100_minus_remaining_with_their_resets() {
        let usage = reading(&fixture_reply(), &context(), &account()).unwrap();

        assert_eq!(usage.account, account());
        assert_eq!(usage.plan.as_deref(), Some("Pro"));
        let kinds: Vec<_> = usage.windows.iter().map(|w| w.kind).collect();
        assert_eq!(kinds, [WindowKind::Daily, WindowKind::Weekly]);
        let used: Vec<_> = usage.windows.iter().map(|w| w.used_fraction).collect();
        assert_eq!(used, [0.32, 0.16]);
        let resets: Vec<_> = usage.windows.iter().map(|w| w.resets_at.map(|r| r.timestamp())).collect();
        assert_eq!(resets, [Some(1_777_900_000), Some(1_778_000_000)]);
        // Daily and weekly are the service naming the lengths.
        assert!(usage.windows.iter().all(|w| w.reports_length));
    }

    #[test]
    fn fields_this_does_not_read_are_skipped_not_fatal() {
        let status = plan_status(&fixture_reply()).unwrap();
        assert_eq!(status.daily_remaining, Some(68));
        assert_eq!(status.weekly_remaining, Some(84));
    }

    #[test]
    fn a_quota_left_out_of_the_reply_is_left_off() {
        let usage = reading(&reply(&[(15, 40), (18, 1_778_000_000)]), &context(), &account()).unwrap();
        assert_eq!(usage.windows.iter().map(|w| w.kind).collect::<Vec<_>>(), [WindowKind::Weekly]);
        assert_eq!(usage.windows[0].used_fraction, 0.6);
    }

    #[test]
    fn a_figure_over_100_is_not_a_percentage_and_nothing_left_is_no_limits() {
        assert_eq!(
            reading(&reply(&[(14, 250)]), &context(), &account()).unwrap_err(),
            Unavailability::NoLimitsReported
        );
    }

    #[test]
    fn a_reply_that_cannot_be_read() {
        let ctx = context();
        let bad: [&[u8]; 4] = [&[], b"not protobuf at all", &[0x0A, 0x05, 0x70], &[0x08, 0x01]];
        for data in bad {
            assert_eq!(reading(data, &ctx, &account()).unwrap_err(), Unavailability::UnreadableReply);
        }
    }

    #[test]
    fn the_pasted_session_is_the_four_devin_values() {
        let parsed = Session::parse(SESSION).unwrap();
        assert_eq!(parsed.token, "devin-session-token$abc");
        assert_eq!(parsed.auth1, "auth1_xyz");
        assert_eq!(parsed.account_id, "account-123");
        assert_eq!(parsed.organization_id, "org-456");
    }

    #[test]
    fn anything_short_of_all_four_is_no_session() {
        let cases = [
            r#"{"devin_session_token":"t","devin_auth1_token":"a","devin_account_id":"c"}"#,
            r#"{"devin_session_token":"t","devin_auth1_token":"a","devin_account_id":"c","devin_primary_org_id":" "}"#,
            "devin_session_token=t",
            "[]",
        ];
        for text in cases {
            assert_eq!(Session::parse(text), None, "{text}");
        }
    }

    #[test]
    fn the_request_carries_the_session_in_the_headers_and_the_body() {
        let ctx = context();
        let session = Session::parse(SESSION).unwrap();
        let built = request(&ctx, &session).build().unwrap();

        assert_eq!(built.url().host_str(), Some("windsurf.com"));
        assert_eq!(built.method(), reqwest::Method::POST);
        assert_eq!(built.timeout(), Some(&Duration::from_secs(15)));
        let header = |name: &str| built.headers().get(name).unwrap().to_str().unwrap().to_string();
        assert_eq!(header("Content-Type"), "application/proto");
        assert_eq!(header("Connect-Protocol-Version"), "1");
        assert_eq!(header("x-auth-token"), "devin-session-token$abc");
        assert_eq!(header("x-devin-auth1-token"), "auth1_xyz");
        assert_eq!(header("x-devin-account-id"), "account-123");
        assert_eq!(header("x-devin-primary-org-id"), "org-456");

        let body = built.body().unwrap().as_bytes().unwrap();
        let mut reader = Reader::new(body);
        assert_eq!(reader.next().map(|f| f.value), Some(WireValue::Bytes(b"devin-session-token$abc".to_vec())));
        assert_eq!(reader.next().map(|f| f.value), Some(WireValue::Varint(1)));
        assert!(reader.next().is_none());
        assert!(reader.complete);
    }

    #[tokio::test]
    async fn no_session_is_asked_for_not_sent() {
        let ctx = context();
        assert_eq!(
            Windsurf.fetch(&ctx, &account()).await.state,
            crate::model::UsageState::Unavailable(Unavailability::SessionMissing)
        );
    }

    #[tokio::test]
    async fn a_saved_credential_that_is_not_the_four_values_is_never_sent() {
        let ctx = context();
        ctx.secrets.set(&account().id(), Some("sk-not-a-session"));
        assert_eq!(
            Windsurf.fetch(&ctx, &account()).await.state,
            crate::model::UsageState::Unavailable(Unavailability::SessionMissing)
        );
    }
}
