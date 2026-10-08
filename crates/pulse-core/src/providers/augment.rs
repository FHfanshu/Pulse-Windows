// Ported from upstream Providers/Profiled/AugmentUsageService.swift.
//! Augment Code: credits used against credits available in the billing cycle,
//! read with the browser session the reader pasted in. `/api/credits` is
//! required; `/api/subscription` only adds the plan name and the cycle's end.

use async_trait::async_trait;
use serde::Deserialize;

use super::profile::{self, Fail};
use crate::model::{AccountKey, ProviderUsage, Unavailability, UsageWindow, WindowKind};
use crate::provider::Provider;
use crate::service::{FetchContext, UsageService};

const BASE: &str = "https://app.augmentcode.com";
/// The cookies kept from a pasted header; the first one is required.
const COOKIES: [&str; 2] = ["_session", "web_rpc_proxy_session"];
/// A billing cycle's length is a sort key only, not a length Augment states.
const CYCLE_SORT_SECONDS: i64 = 30 * 86_400;

#[derive(Default)]
pub struct Augment;

#[async_trait]
impl UsageService for Augment {
    fn provider(&self) -> Provider {
        Provider::Augment
    }

    async fn fetch(&self, ctx: &FetchContext, account: &AccountKey) -> ProviderUsage {
        let Some(cookies) = session_cookie(ctx, account) else {
            return ProviderUsage::unavailable(account.clone(), Unavailability::SessionMissing);
        };

        let credits = match get(ctx, "api/credits", &cookies).await {
            Ok(body) => body,
            Err(reason) => return ProviderUsage::unavailable(account.clone(), reason),
        };
        // Optional: without it there is no plan name and no reset, and the figures still stand.
        let subscription = get(ctx, "api/subscription", &cookies).await.ok();

        reading(&credits, subscription.as_deref(), ctx, account)
            .unwrap_or_else(|reason| ProviderUsage::unavailable(account.clone(), reason))
    }
}

/// The pasted cookie header, trimmed and filtered to the session cookies.
/// None when `_session` is not among them.
fn session_cookie(ctx: &FetchContext, account: &AccountKey) -> Option<String> {
    let header = ctx.api_key(account)?;
    profile::keep_cookies(header.trim(), &COOKIES)
}

async fn get(ctx: &FetchContext, path: &str, cookies: &str) -> Result<Vec<u8>, Fail> {
    let request = ctx
        .http
        .get(format!("{BASE}/{path}"))
        .header("Cookie", cookies)
        .header("Accept", "application/json")
        .timeout(std::time::Duration::from_secs(15));
    profile::send(ctx, request, Unavailability::SessionExpired).await
}

#[derive(Deserialize)]
struct Credits {
    #[serde(rename = "usageUnitsConsumedThisBillingCycle")]
    consumed: Option<f64>,
    #[serde(rename = "usageUnitsAvailable")]
    available: Option<f64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Subscription {
    plan_name: Option<String>,
    billing_period_end: Option<String>,
}

fn reading(
    credits: &[u8],
    subscription: Option<&[u8]>,
    ctx: &FetchContext,
    account: &AccountKey,
) -> Result<ProviderUsage, Fail> {
    // A body that is not a JSON object (a sign-in page served with a 200, say) is not a reply.
    let is_object = matches!(serde_json::from_slice::<serde_json::Value>(credits), Ok(v) if v.is_object());
    if !is_object {
        return Err(Unavailability::UnreadableReply);
    }
    let reply: Credits = profile::decode(credits)?;

    let about: Option<Subscription> = subscription.and_then(|b| serde_json::from_slice(b).ok());
    let plan = about
        .as_ref()
        .and_then(|a| profile::trimmed(a.plan_name.clone()));
    let resets_at = about.as_ref().and_then(|a| profile::date(a.billing_period_end.as_deref()));

    let (Some(used), Some(available)) = (reply.consumed, reply.available) else {
        return Err(Unavailability::NoLimitsReported);
    };
    if !used.is_finite() || used < 0.0 || !available.is_finite() || available <= 0.0 {
        return Err(Unavailability::NoLimitsReported);
    }

    let mut window = UsageWindow::new("augment.credits", WindowKind::Credits, used / available, CYCLE_SORT_SECONDS)
        .with_reset(resets_at)
        .exhausted(used >= available);
    window.reports_length = false;
    Ok(profile::reading(account, vec![window], ctx).with_plan(plan))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::UsageState;
    use crate::providers::profile::test_support::{context, fixture};

    fn account() -> AccountKey {
        AccountKey::primary(Provider::Augment)
    }

    fn reading_json(credits: &str, subscription: Option<&[u8]>) -> Result<ProviderUsage, Fail> {
        reading(credits.as_bytes(), subscription, &context(), &account())
    }

    #[test]
    fn credits_used_against_available_with_the_cycle_end_as_reset() {
        let ctx = context();
        let usage = reading(&fixture("augment-credits.json"), Some(&fixture("augment-subscription.json")), &ctx, &account())
            .unwrap();

        assert_eq!(usage.state, UsageState::Live);
        assert_eq!(usage.account, account());
        assert_eq!(usage.plan.as_deref(), Some("Max"));
        assert_eq!(usage.windows.len(), 1);
        let window = &usage.windows[0];
        assert_eq!(window.kind, WindowKind::Credits);
        assert_eq!(window.used_fraction, 130_946.0 / 450_000.0);
        assert_eq!(window.resets_at, profile::date(Some("2026-06-09T00:00:00Z")));
        // A billing cycle is not a length Augment states.
        assert!(!window.reports_length);
        assert!(!window.is_exhausted);
    }

    #[test]
    fn without_the_subscription_the_figures_still_stand_with_no_plan_and_no_reset() {
        let usage = reading(&fixture("augment-credits.json"), None, &context(), &account()).unwrap();
        assert_eq!(usage.state, UsageState::Live);
        assert_eq!(usage.plan, None);
        assert_eq!(usage.windows[0].resets_at, None);

        let garbled = reading_json(r#"{"usageUnitsConsumedThisBillingCycle":1,"usageUnitsAvailable":4}"#, Some(b"<html>")).unwrap();
        assert_eq!(garbled.windows[0].used_fraction, 0.25);
        assert_eq!(garbled.plan, None);
    }

    #[test]
    fn no_stated_allowance_draws_nothing() {
        for json in [
            r#"{"usageUnitsRemaining":15,"usageUnitsConsumedThisBillingCycle":10}"#,
            r#"{"usageUnitsRemaining":15,"usageUnitsConsumedThisBillingCycle":10,"usageUnitsAvailable":0}"#,
            r#"{"usageUnitsRemaining":15,"usageUnitsConsumedThisBillingCycle":-10,"usageUnitsAvailable":100}"#,
            r#"{"usageUnitsRemaining":15,"usageUnitsAvailable":100}"#,
        ] {
            assert_eq!(reading_json(json, None).unwrap_err(), Unavailability::NoLimitsReported, "{json}");
        }
    }

    #[test]
    fn a_spent_allowance_says_so() {
        let usage = reading_json(r#"{"usageUnitsConsumedThisBillingCycle":120,"usageUnitsAvailable":100}"#, None).unwrap();
        assert!(usage.windows[0].is_exhausted);
        assert_eq!(usage.windows[0].used_fraction, 1.2);
    }

    #[test]
    fn a_reply_that_cannot_be_read() {
        for body in ["not json", "<html><body>Sign in</body></html>", "[]"] {
            assert_eq!(reading_json(body, None).unwrap_err(), Unavailability::UnreadableReply, "{body}");
        }
    }

    #[test]
    fn only_the_session_cookies_are_kept_and_session_must_be_one() {
        let kept = session_cookie_from("_ga=1; _session=abc; web_rpc_proxy_session=def; other=2");
        assert_eq!(kept.as_deref(), Some("_session=abc; web_rpc_proxy_session=def"));
        assert_eq!(session_cookie_from("web_rpc_proxy_session=def"), None);
    }

    #[test]
    fn no_session_is_asked_for_not_sent() {
        assert_eq!(session_cookie_from("   "), None);
    }

    fn session_cookie_from(header: &str) -> Option<String> {
        profile::keep_cookies(header.trim(), &COOKIES)
    }
}
