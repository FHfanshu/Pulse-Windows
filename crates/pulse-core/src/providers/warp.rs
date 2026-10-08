// Ported from upstream Providers/Profiled/WarpUsageService.swift.
//! Warp: the plan's credits for the period, and any add-on credits bought or
//! granted on top.
//!
//! Read with a pasted key from the `GetRequestLimitInfo` GraphQL query Warp's
//! own app sends, a read-only query posted to `app.warp.dev/graphql/v2`. The
//! API still calls credits "requests". The plan's period length is not stated,
//! so it is not claimed; an unlimited plan has no limit to draw. Add-on grants
//! are summed and spent after the plan's, never reset, and the soonest to lapse
//! is shown as an expiry.

use async_trait::async_trait;
use chrono::{DateTime, Local, Utc};
use serde::Deserialize;
use serde_json::json;

use super::profile::{self, Fail};
use crate::model::{AccountKey, Expiry, ProviderUsage, Unavailability, UsageWindow, WindowKind};
use crate::provider::Provider;
use crate::service::{FetchContext, UsageService};

const ENDPOINT: &str = "https://app.warp.dev/graphql/v2?op=GetRequestLimitInfo";
const QUERY: &str = r#"query GetRequestLimitInfo($requestContext: RequestContext!) {
  user(requestContext: $requestContext) {
    __typename
    ... on UserOutput {
      user {
        requestLimitInfo { isUnlimited nextRefreshTime requestLimit requestsUsedSinceLastRefresh }
        bonusGrants { requestCreditsGranted requestCreditsRemaining expiration }
        workspaces { bonusGrantsInfo { grants { requestCreditsGranted requestCreditsRemaining expiration } } }
      }
    }
  }
}"#;

/// What Warp's app sends. The edge limiter answers 429 to a client that does not name itself.
const CLIENT_ID: &str = "warp-app";
const OS_CATEGORY: &str = "macOS";
const OS_NAME: &str = "macOS";
// WINDOWS-PATH: unverified. Upstream sends the running macOS version; this port
// reports no OS version yet, so a fixed value stands in until it is read.
const OS_VERSION: &str = "10.0";
const USER_AGENT: &str = "Warp/1.0";

/// The period's length is not stated; this is a sort key only.
const SORT_KEY_SECONDS: i64 = 30 * 86_400;

#[derive(Default)]
pub struct Warp;

#[async_trait]
impl UsageService for Warp {
    fn provider(&self) -> Provider {
        Provider::Warp
    }

    async fn fetch(&self, ctx: &FetchContext, account: &AccountKey) -> ProviderUsage {
        let Some(key) = ctx.api_key(account) else {
            return ProviderUsage::unavailable(account.clone(), Unavailability::ApiKeyMissing);
        };
        let key = key.trim().to_string();
        let body = match profile::send(ctx, request(ctx, &key), Unavailability::ApiKeyRefused).await {
            Ok(body) => body,
            Err(reason) => return ProviderUsage::unavailable(account.clone(), reason),
        };
        reading(&body, ctx, account).unwrap_or_else(|reason| ProviderUsage::unavailable(account.clone(), reason))
    }
}

fn request(ctx: &FetchContext, key: &str) -> reqwest::RequestBuilder {
    let body = json!({
        "operationName": "GetRequestLimitInfo",
        "query": QUERY,
        "variables": {
            "requestContext": {
                "clientContext": {},
                "osContext": { "category": OS_CATEGORY, "name": OS_NAME, "version": OS_VERSION },
            },
        },
    });
    ctx.http
        .post(ENDPOINT)
        .bearer_auth(key)
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .header("x-warp-client-id", CLIENT_ID)
        .header("x-warp-os-category", OS_CATEGORY)
        .header("x-warp-os-name", OS_NAME)
        .header("x-warp-os-version", OS_VERSION)
        .header("User-Agent", USER_AGENT)
        .body(body.to_string())
}

// MARK: - Reading the reply

#[derive(Deserialize)]
struct Reply {
    data: Option<Payload>,
    errors: Option<Vec<serde::de::IgnoredAny>>,
}

#[derive(Deserialize)]
struct Payload {
    user: Option<Output>,
}

#[derive(Deserialize)]
struct Output {
    user: Option<Account>,
}

#[derive(Deserialize)]
struct Account {
    #[serde(rename = "requestLimitInfo")]
    request_limit_info: Option<Limit>,
    #[serde(rename = "bonusGrants")]
    bonus_grants: Option<Vec<Grant>>,
    workspaces: Option<Vec<Workspace>>,
}

#[derive(Deserialize)]
struct Limit {
    #[serde(rename = "isUnlimited")]
    is_unlimited: Option<bool>,
    #[serde(rename = "nextRefreshTime")]
    next_refresh_time: Option<String>,
    #[serde(rename = "requestLimit")]
    request_limit: Option<f64>,
    #[serde(rename = "requestsUsedSinceLastRefresh")]
    requests_used_since_last_refresh: Option<f64>,
}

#[derive(Deserialize)]
struct Workspace {
    #[serde(rename = "bonusGrantsInfo")]
    bonus_grants_info: Option<WorkspaceGrants>,
}

#[derive(Deserialize)]
struct WorkspaceGrants {
    grants: Option<Vec<Grant>>,
}

#[derive(Deserialize)]
struct Grant {
    #[serde(rename = "requestCreditsGranted")]
    granted: Option<f64>,
    #[serde(rename = "requestCreditsRemaining")]
    remaining: Option<f64>,
    expiration: Option<String>,
}

pub fn reading(body: &[u8], ctx: &FetchContext, account: &AccountKey) -> Result<ProviderUsage, Fail> {
    reading_at(body, ctx.now, account)
}

/// GraphQL answers a failed query with a 200 and a list of errors.
pub fn reading_at(body: &[u8], now: DateTime<Utc>, account: &AccountKey) -> Result<ProviderUsage, Fail> {
    let reply: Reply = profile::decode(body)?;
    if reply.errors.is_some_and(|errors| !errors.is_empty()) {
        return Err(Unavailability::ServerError);
    }
    let account_data = reply
        .data
        .and_then(|d| d.user)
        .and_then(|o| o.user)
        .ok_or(Unavailability::UnreadableReply)?;
    let limit = account_data.request_limit_info.ok_or(Unavailability::UnreadableReply)?;

    let mut windows = Vec::new();
    if let Some(plan) = plan_window(&limit) {
        windows.push(plan);
    }

    // Every grant with both figures: the user's own and each workspace's.
    let grants: Vec<(f64, f64, Option<DateTime<Utc>>)> = account_data
        .bonus_grants
        .unwrap_or_default()
        .into_iter()
        .chain(
            account_data
                .workspaces
                .unwrap_or_default()
                .into_iter()
                .flat_map(|w| w.bonus_grants_info.and_then(|i| i.grants).unwrap_or_default()),
        )
        .filter_map(|g| {
            let granted = g.granted.filter(|v| v.is_finite() && *v > 0.0)?;
            let left = g.remaining.filter(|v| v.is_finite() && *v >= 0.0)?;
            Some((granted, left, profile::date(g.expiration.as_deref())))
        })
        .collect();
    let granted: f64 = grants.iter().map(|g| g.0).sum();
    if granted > 0.0 {
        let left: f64 = grants.iter().map(|g| g.1).sum();
        let mut pack = UsageWindow::new("warp.addon", WindowKind::TopUp, ((granted - left).max(0.0)) / granted, SORT_KEY_SECONDS);
        pack.reports_length = false;
        pack.is_exhausted = left <= 0.0;
        pack.next_expiry = soonest_expiry(
            &grants.iter().filter_map(|g| g.2.map(|at| (g.1, at))).collect::<Vec<_>>(),
            now,
        );
        windows.push(pack);
    }

    if windows.is_empty() {
        return Err(Unavailability::NoLimitsReported);
    }
    Ok(ProviderUsage::live(account.clone(), windows, now))
}

/// The plan's credits: stated used and cap, refilling at the stated refresh time.
fn plan_window(limit: &Limit) -> Option<UsageWindow> {
    if limit.is_unlimited == Some(true) {
        return None;
    }
    let cap = limit.request_limit.filter(|v| v.is_finite() && *v > 0.0)?;
    let used = limit.requests_used_since_last_refresh.filter(|v| v.is_finite() && *v >= 0.0)?;
    let mut window = UsageWindow::new("warp.credits", WindowKind::Credits, used / cap, SORT_KEY_SECONDS)
        .with_reset(profile::date(limit.next_refresh_time.as_deref()));
    window.reports_length = false;
    Some(window.exhausted(used >= cap))
}

/// The soonest parts to lapse: everything ending on the same local day as the first one,
/// added up. Only parts still ahead of `now` with something left in them count.
fn soonest_expiry(parts: &[(f64, DateTime<Utc>)], now: DateTime<Utc>) -> Option<Expiry> {
    let ahead: Vec<&(f64, DateTime<Utc>)> = parts.iter().filter(|p| p.1 > now && p.0 > 0.0).collect();
    let first = ahead.iter().map(|p| p.1).min()?;
    let day = first.with_timezone(&Local).date_naive();
    let amount = ahead
        .iter()
        .filter(|p| p.1.with_timezone(&Local).date_naive() == day)
        .map(|p| p.0)
        .sum();
    Some(Expiry { amount, at: first })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::UsageState;
    use crate::providers::profile::test_support::fixture;

    fn account() -> AccountKey {
        AccountKey::primary(Provider::Warp)
    }

    /// Before every expiry in the fixture.
    fn now() -> DateTime<Utc> {
        profile::date(Some("2026-09-25T00:00:00Z")).unwrap()
    }

    fn read(json: &str) -> Result<ProviderUsage, Fail> {
        reading_at(json.as_bytes(), now(), &account())
    }

    #[test]
    fn the_plans_credits_and_the_add_on_credits_each_as_stated() {
        let usage = reading_at(&fixture("warp-request-limit.json"), now(), &account()).unwrap();

        assert_eq!(usage.state, UsageState::Live);
        assert_eq!(usage.account, account());
        assert_eq!(usage.windows.iter().map(|w| w.kind).collect::<Vec<_>>(), vec![WindowKind::Credits, WindowKind::TopUp]);

        let plan = &usage.windows[0];
        assert_eq!(plan.used_fraction, 0.25);
        assert_eq!(plan.resets_at, profile::date(Some("2026-10-01T00:00:00Z")));
        // A refill date is stated; the period's length is not.
        assert!(!plan.reports_length);

        // The user's grant and the workspace's, added up: 1,500 granted, 900 left.
        let add_on = &usage.windows[1];
        assert_eq!(add_on.used_fraction, 600.0 / 1_500.0);
        assert_eq!(add_on.resets_at, None);
        assert_eq!(
            add_on.next_expiry,
            Some(Expiry { amount: 400.0, at: profile::date(Some("2026-10-15T12:00:00Z")).unwrap() })
        );
    }

    #[test]
    fn an_unlimited_plan_has_no_limit_to_draw() {
        let json = r#"{"data":{"user":{"__typename":"UserOutput","user":{"requestLimitInfo":{"isUnlimited":true,"requestLimit":0,"requestsUsedSinceLastRefresh":12}}}}}"#;
        assert_eq!(read(json).unwrap_err(), Unavailability::NoLimitsReported);
    }

    #[test]
    fn a_limit_with_no_figure_is_left_off_and_nothing_left_is_no_limits() {
        let cases = [
            r#"{"data":{"user":{"user":{"requestLimitInfo":{"isUnlimited":false,"requestLimit":0,"requestsUsedSinceLastRefresh":0}}}}}"#,
            r#"{"data":{"user":{"user":{"requestLimitInfo":{"requestLimit":100,"requestsUsedSinceLastRefresh":-1},"bonusGrants":[{"requestCreditsGranted":100,"requestCreditsRemaining":-5}]}}}}"#,
            r#"{"data":{"user":{"user":{"requestLimitInfo":{"requestLimit":100}}}}}"#,
        ];
        for json in cases {
            assert_eq!(read(json).unwrap_err(), Unavailability::NoLimitsReported, "{json}");
        }
    }

    #[test]
    fn a_reply_without_the_limit_cant_be_read() {
        for json in ["not json", "{}", r#"{"data":{"user":{"__typename":"UserFacingError"}}}"#] {
            assert_eq!(read(json).unwrap_err(), Unavailability::UnreadableReply, "{json}");
        }
    }

    #[test]
    fn graphql_errors_answered_with_a_200_are_the_services_error() {
        assert_eq!(read(r#"{"errors":[{"message":"Something went wrong"}],"data":null}"#).unwrap_err(), Unavailability::ServerError);
    }

    #[test]
    fn the_query_is_a_read_posted_with_the_key_and_the_client_headers_warps_edge_asks_for() {
        let built = request(&crate::providers::profile::test_support::context(), "wk-key").build().unwrap();
        assert_eq!(built.method().as_str(), "POST");
        assert_eq!(built.headers().get("Authorization").unwrap(), "Bearer wk-key");
        assert_eq!(built.headers().get("User-Agent").unwrap(), "Warp/1.0");
        assert_eq!(built.headers().get("x-warp-client-id").unwrap(), "warp-app");
        let body: serde_json::Value = serde_json::from_slice(built.body().unwrap().as_bytes().unwrap()).unwrap();
        assert_eq!(body["operationName"], "GetRequestLimitInfo");
        assert!(body["query"].as_str().unwrap().starts_with("query "));
    }
}
