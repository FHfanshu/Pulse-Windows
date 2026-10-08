// Ported from upstream Providers/Profiled/OpenAIPlatformUsageService.swift.
//! OpenAI API: the prepaid credit left on the account, and nothing else.
//!
//! Read with a pasted key from the legacy billing route `credit_grants`. A key
//! that route turns away is not yet a bad key: Admin and project keys are
//! refused there by design, so the Admin costs route is asked before anything
//! is said. A key that reads costs works and reports no limits. No ring is
//! drawn: the balance is shown as a balance.

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;

use super::profile::{self, Fail};
use crate::model::{AccountKey, ProviderUsage, Unavailability};
use crate::provider::Provider;
use crate::service::{FetchContext, UsageService};

const CREDIT_GRANTS: &str = "https://api.openai.com/v1/dashboard/billing/credit_grants";

/// The figures are dollars; the route names no currency because the platform bills in one.
const CURRENCY: &str = "USD";

#[derive(Default)]
pub struct OpenAiPlatform;

#[async_trait]
impl UsageService for OpenAiPlatform {
    fn provider(&self) -> Provider {
        Provider::OpenAiPlatform
    }

    async fn fetch(&self, ctx: &FetchContext, account: &AccountKey) -> ProviderUsage {
        let Some(key) = ctx.api_key(account) else {
            return ProviderUsage::unavailable(account.clone(), Unavailability::ApiKeyMissing);
        };
        let key = key.trim().to_string();

        let (status, body) = match balance_request(ctx, &key).await {
            Ok(reply) => reply,
            Err(reason) => return ProviderUsage::unavailable(account.clone(), reason),
        };
        match balance_status(status) {
            BalanceAnswer::Reply => {
                reading(&body, ctx, account).unwrap_or_else(|reason| ProviderUsage::unavailable(account.clone(), reason))
            }
            BalanceAnswer::Failed(reason) => ProviderUsage::unavailable(account.clone(), reason),
            // Refused, or a route this key's kind cannot see: ask the Admin API whether the key is any good.
            BalanceAnswer::Ask => {
                let probe = profile::get_bearer(ctx, &costs_probe(ctx.now), &key).await.map(|_| ());
                ProviderUsage::unavailable(account.clone(), after_probe(probe))
            }
        }
    }
}

/// GET the balance route and return its status and body. Unlike `profile::send`,
/// a 404 is not an error here: it is one of the answers that leads to the probe.
async fn balance_request(ctx: &FetchContext, key: &str) -> Result<(u16, Vec<u8>), Fail> {
    let reply = ctx
        .http
        .get(CREDIT_GRANTS)
        .bearer_auth(key)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|_| Unavailability::Unreachable)?;
    let status = reply.status().as_u16();
    let body = reply.bytes().await.map_err(|_| Unavailability::Unreachable)?;
    Ok((status, body.to_vec()))
}

/// One day of the organization's costs, asked only to learn whether the key is an Admin key.
/// The figures in the reply are not read.
fn costs_probe(now: DateTime<Utc>) -> String {
    let start = (now - Duration::days(1)).timestamp();
    format!("https://api.openai.com/v1/organization/costs?start_time={start}&limit=1")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BalanceAnswer {
    /// Read the body.
    Reply,
    /// The route says nothing about the key itself; ask the probe.
    Ask,
    /// An answer about the service, not the key.
    Failed(Fail),
}

fn balance_status(status: u16) -> BalanceAnswer {
    match status {
        200..=299 => BalanceAnswer::Reply,
        429 => BalanceAnswer::Failed(Unavailability::RateLimited),
        500..=u16::MAX => BalanceAnswer::Failed(Unavailability::ServerError),
        _ => BalanceAnswer::Ask,
    }
}

/// What the probe's result means for the account. A key the costs route accepts works,
/// but it reports no limits.
fn after_probe(probe: Result<(), Fail>) -> Fail {
    match probe {
        Ok(()) => Unavailability::NoLimitsReported,
        Err(reason) => reason,
    }
}

#[derive(Deserialize)]
struct Reply {
    total_granted: Option<f64>,
    total_used: Option<f64>,
    total_available: Option<f64>,
}

fn reading(body: &[u8], ctx: &FetchContext, account: &AccountKey) -> Result<ProviderUsage, Fail> {
    let reply: Reply = profile::decode(body)?;
    if reply.total_granted.is_none() && reply.total_used.is_none() && reply.total_available.is_none() {
        return Err(Unavailability::UnreadableReply);
    }
    // A balance that is not one is left off, and then there is nothing to show.
    let available = reply
        .total_available
        .filter(|v| v.is_finite() && *v >= 0.0)
        .ok_or(Unavailability::NoLimitsReported)?;
    Ok(profile::balance_reading(account, available, CURRENCY, ctx))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::profile::test_support::{context, fixture};

    fn account() -> AccountKey {
        AccountKey::primary(Provider::OpenAiPlatform)
    }

    #[test]
    fn reads_the_balance_left_in_dollars_with_no_windows() {
        let ctx = context();
        let usage = reading(&fixture("openai-api-credit-grants.json"), &ctx, &account()).unwrap();
        assert_eq!(usage.state, crate::model::UsageState::Live);
        assert_eq!(usage.account, account());
        assert!(usage.windows.is_empty());
        let credit = usage.credit_remaining.unwrap();
        assert_eq!(credit.amount, 12.3);
        assert_eq!(credit.currency, "USD");
        assert_eq!(usage.credit_balance.as_deref(), Some("USD 12.30"));
    }

    #[test]
    fn a_reply_that_is_not_a_credit_summary_is_unreadable() {
        let ctx = context();
        for json in ["not json", "{}", "[]"] {
            assert_eq!(
                reading(json.as_bytes(), &ctx, &account()).unwrap_err(),
                Unavailability::UnreadableReply,
                "{json}"
            );
        }
    }

    #[test]
    fn a_balance_that_is_not_one_is_no_limits() {
        let ctx = context();
        for json in [r#"{"total_granted":5,"total_used":7,"total_available":-2}"#, r#"{"total_granted":5,"total_used":1}"#] {
            let usage = reading(json.as_bytes(), &ctx, &account()).err();
            assert_eq!(usage, Some(Unavailability::NoLimitsReported), "{json}");
        }
    }

    #[test]
    fn a_reply_that_reads_the_balance_is_read_and_others_are_asked_or_failed() {
        assert_eq!(balance_status(200), BalanceAnswer::Reply);
        // Admin and project keys, and retired routes, are refused here and asked on the probe.
        assert_eq!(balance_status(401), BalanceAnswer::Ask);
        assert_eq!(balance_status(403), BalanceAnswer::Ask);
        assert_eq!(balance_status(404), BalanceAnswer::Ask);
        assert_eq!(balance_status(429), BalanceAnswer::Failed(Unavailability::RateLimited));
        assert_eq!(balance_status(503), BalanceAnswer::Failed(Unavailability::ServerError));
    }

    #[test]
    fn an_admin_key_that_reads_costs_works_and_reports_no_limits() {
        assert_eq!(after_probe(Ok(())), Unavailability::NoLimitsReported);
    }

    #[test]
    fn a_key_both_routes_refuse_is_refused() {
        assert_eq!(after_probe(Err(Unavailability::ApiKeyRefused)), Unavailability::ApiKeyRefused);
    }

    #[test]
    fn easing_off_is_not_taken_for_a_refusal() {
        assert_eq!(balance_status(429), BalanceAnswer::Failed(Unavailability::RateLimited));
        assert_eq!(after_probe(Err(Unavailability::RateLimited)), Unavailability::RateLimited);
    }

    #[test]
    fn the_probe_asks_for_one_day_of_costs_and_nothing_else() {
        let now = DateTime::parse_from_rfc3339("2026-10-01T12:00:00Z").unwrap().with_timezone(&Utc);
        let start = now.timestamp() - 86_400;
        assert_eq!(costs_probe(now), format!("https://api.openai.com/v1/organization/costs?start_time={start}&limit=1"));
    }
}
