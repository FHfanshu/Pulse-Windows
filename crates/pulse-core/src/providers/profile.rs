// Ported from upstream Providers/ProviderProfile.swift.
//! Shared plumbing for "profiled" providers: one credential, one or two HTTP
//! requests, one parse function. Each profiled provider is a small module in
//! `providers/` that implements `UsageService` with these helpers.

use chrono::{DateTime, Utc};
use serde::de::DeserializeOwned;

use crate::model::{AccountKey, CreditAmount, ProviderUsage, Unavailability, UsageRoute, UsageWindow};
use crate::service::FetchContext;

/// A failed request, already classified.
pub type Fail = Unavailability;

/// GET with `Authorization: Bearer <token>`. Redirects are never followed: for
/// every service here a redirect goes to a sign-in page, so it means refused.
pub async fn get_bearer(ctx: &FetchContext, url: &str, token: &str) -> Result<Vec<u8>, Fail> {
    send(ctx, ctx.http.get(url).bearer_auth(token).header("Accept", "application/json"), Unavailability::ApiKeyRefused).await
}

/// Send a prepared request and classify the reply. `refused` is what a 401/403/3xx means
/// for this credential (an API key, a session, a local login).
pub async fn send(_ctx: &FetchContext, request: reqwest::RequestBuilder, refused: Unavailability) -> Result<Vec<u8>, Fail> {
    let reply = request.send().await.map_err(|_| Unavailability::Unreachable)?;
    let status = reply.status().as_u16();
    let body = reply.bytes().await.map_err(|_| Unavailability::Unreachable)?;
    match status {
        200..=299 => Ok(body.to_vec()),
        300..=399 | 401 | 403 => Err(refused),
        429 => Err(Unavailability::RateLimited),
        _ => Err(Unavailability::ServerError),
    }
}

pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, Fail> {
    serde_json::from_slice(bytes).map_err(|_| Unavailability::UnreadableReply)
}

/// ISO-8601 with or without fractional seconds.
pub fn date(text: Option<&str>) -> Option<DateTime<Utc>> {
    let text = text?.trim();
    if text.is_empty() {
        return None;
    }
    DateTime::parse_from_rfc3339(text).ok().map(|d| d.with_timezone(&Utc))
}

/// Unix seconds or milliseconds, whichever the number looks like.
pub fn epoch(value: f64) -> Option<DateTime<Utc>> {
    if !value.is_finite() || value <= 0.0 {
        return None;
    }
    let ms = if value > 1e12 { value } else { value * 1000.0 };
    DateTime::from_timestamp_millis(ms as i64)
}

pub fn trimmed(s: Option<String>) -> Option<String> {
    s.map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

/// A live reading for `account`.
pub fn reading(account: &AccountKey, windows: Vec<UsageWindow>, ctx: &FetchContext) -> ProviderUsage {
    ProviderUsage::live(account.clone(), windows, ctx.now)
}

/// A money-only reading (prepaid balance, no allowance). The UI formats the
/// amount in the reader's locale; `credit_balance` is the plain fallback.
pub fn balance_reading(account: &AccountKey, amount: f64, currency: &str, ctx: &FetchContext) -> ProviderUsage {
    let mut usage = ProviderUsage::live(account.clone(), Vec::new(), ctx.now);
    usage.credit_balance = Some(format!("{currency} {amount:.2}"));
    usage.credit_remaining = Some(CreditAmount { amount, currency: currency.to_string() });
    usage.origin = Some(UsageRoute::ApiKey);
    usage
}

/// Keep only the cookies a provider needs from a pasted `Cookie:` header.
/// The first pattern (alternatives split by `|`) must be present; `*` is a prefix match.
pub fn keep_cookies(header: &str, names: &[&str]) -> Option<String> {
    let first = names.first()?;
    let required: Vec<&str> = first.split('|').collect();
    let patterns: Vec<&str> = names.iter().flat_map(|n| n.split('|')).collect();
    let matches = |name: &str, pattern: &str| match pattern.strip_suffix('*') {
        Some(prefix) => !prefix.is_empty() && name.starts_with(prefix) && name.len() > prefix.len(),
        None => name == pattern,
    };
    let pairs: Vec<(String, String)> = header
        .split(';')
        .filter_map(|part| {
            let (name, value) = part.trim().split_once('=')?;
            (patterns.iter().any(|p| matches(name, p)) && !value.is_empty()).then(|| (name.to_string(), value.to_string()))
        })
        .collect();
    if !pairs.iter().any(|(n, _)| required.iter().any(|r| matches(n, r))) {
        return None;
    }
    let mut seen = std::collections::BTreeSet::new();
    Some(
        pairs
            .into_iter()
            .filter(|(n, _)| seen.insert(n.clone()))
            .map(|(n, v)| format!("{n}={v}"))
            .collect::<Vec<_>>()
            .join("; "),
    )
}

#[cfg(test)]
pub mod test_support {
    use std::sync::Arc;

    use crate::secrets::MemorySecrets;
    use crate::service::FetchContext;
    use crate::settings::AppSettings;

    /// A context with no network use and a fixed clock, for parse tests.
    pub fn context() -> FetchContext {
        let mut ctx = FetchContext::from_env(Arc::new(AppSettings::default()), Arc::new(MemorySecrets::default()));
        ctx.now = chrono::DateTime::parse_from_rfc3339("2026-10-01T12:00:00Z").unwrap().with_timezone(&chrono::Utc);
        ctx
    }

    /// Read `tests/fixtures/<name>` (copied verbatim from upstream Tests/PulseTests/Fixtures).
    pub fn fixture(name: &str) -> Vec<u8> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name);
        std::fs::read(&path).unwrap_or_else(|_| panic!("missing fixture {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_only_named_cookies_and_requires_the_first() {
        assert_eq!(
            keep_cookies("a=1; session=x; other=2; session=y", &["session", "a"]).as_deref(),
            Some("a=1; session=x")
        );
        assert_eq!(keep_cookies("a=1", &["session", "a"]), None);
        assert_eq!(keep_cookies("sb-abc=1", &["sb-*"]).as_deref(), Some("sb-abc=1"));
    }

    #[test]
    fn epoch_accepts_seconds_and_millis() {
        assert_eq!(epoch(1_700_000_000.0), epoch(1_700_000_000_000.0));
    }
}
