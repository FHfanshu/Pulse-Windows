// Ported from upstream Providers/Profiled/GeminiUsageService.swift.
//! Gemini CLI's quota: how much of each model's allowance is left, as a
//! fraction Google reports, with the time each one resets.
//!
//! Read with the login Gemini CLI saved in `~/.gemini/oauth_creds.json`. The
//! login is only read, never renewed or written back: an expired token is
//! reported as one, and using Gemini CLI renews it.

use std::collections::BTreeMap;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};

use super::profile::{self, Fail};
use crate::model::{AccountKey, ProviderUsage, Unavailability, UsageWindow, WindowKind};
use crate::provider::Provider;
use crate::service::{FetchContext, UsageService};

const LOAD_CODE_ASSIST: &str = "https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist";
const RETRIEVE_USER_QUOTA: &str = "https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota";

#[derive(Default)]
pub struct Gemini;

#[async_trait]
impl UsageService for Gemini {
    fn provider(&self) -> Provider {
        Provider::Gemini
    }

    async fn fetch(&self, ctx: &FetchContext, account: &AccountKey) -> ProviderUsage {
        // Signed in with a key or with Vertex, the CLI has no Google login to read.
        let folder = ctx.home.join(".gemini");
        if let Ok(settings) = std::fs::read(folder.join("settings.json")) {
            if !uses_google_login(&settings) {
                return ProviderUsage::unavailable(account.clone(), Unavailability::LocalLoginMissing);
            }
        }
        let token = match std::fs::read(folder.join("oauth_creds.json")) {
            Ok(saved) => match access_token(&saved, ctx.now) {
                Ok(token) => token,
                Err(reason) => return ProviderUsage::unavailable(account.clone(), reason),
            },
            Err(_) => return ProviderUsage::unavailable(account.clone(), Unavailability::LocalLoginMissing),
        };

        // The project and the plan are best-effort: a quota asked for without
        // a project is still the account's quota.
        let mut code_assist = CodeAssist::default();
        match post(ctx, LOAD_CODE_ASSIST, &token, &json!({ "metadata": { "ideType": "GEMINI_CLI", "pluginType": "GEMINI" } })).await {
            Ok((401, _)) => return ProviderUsage::unavailable(account.clone(), Unavailability::LocalLoginExpired),
            Ok((status, body)) if !(200..300).contains(&status) && says_unsupported(&text(&body)) => {
                return ProviderUsage::unavailable(account.clone(), Unavailability::NoPlan);
            }
            Ok((status, body)) if (200..300).contains(&status) => code_assist = code_assist_from(&body),
            _ => {}
        }

        let body = match &code_assist.project {
            Some(project) => json!({ "project": project }),
            None => json!({}),
        };
        match post(ctx, RETRIEVE_USER_QUOTA, &token, &body).await {
            Err(reason) => ProviderUsage::unavailable(account.clone(), reason),
            Ok((status, reply)) => match quota_status(status, &reply, code_assist.unsupported_client) {
                Err(reason) => ProviderUsage::unavailable(account.clone(), reason),
                Ok(()) => reading(&reply, code_assist.plan.as_deref(), ctx, account)
                    .unwrap_or_else(|reason| ProviderUsage::unavailable(account.clone(), reason)),
            },
        }
    }
}

/// POST a JSON body with the bearer token. Returns the status and the body for
/// every status, since Google's wording decides some of the outcomes. Only a
/// transport failure is an error here.
async fn post(ctx: &FetchContext, url: &str, token: &str, body: &Value) -> Result<(u16, Vec<u8>), Fail> {
    let request = ctx
        .http
        .post(url)
        .timeout(Duration::from_secs(15))
        .bearer_auth(token)
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .body(serde_json::to_vec(body).unwrap_or_default());
    let reply = request.send().await.map_err(|_| Unavailability::Unreachable)?;
    let status = reply.status().as_u16();
    let bytes = reply.bytes().await.map_err(|_| Unavailability::Unreachable)?;
    Ok((status, bytes.to_vec()))
}

/// Whether the CLI's settings leave it signed in with Google. Only a key or
/// Vertex say otherwise; a file that names neither gets the benefit.
fn uses_google_login(settings: &[u8]) -> bool {
    let Some(object) = serde_json::from_slice::<Value>(settings).ok().and_then(|v| v.as_object().cloned()) else {
        return true;
    };
    let nested = object
        .get("security")
        .and_then(|s| s.get("auth"))
        .and_then(|a| a.get("selectedType"))
        .and_then(Value::as_str);
    let selected = nested.or_else(|| object.get("selectedAuthType").and_then(Value::as_str));
    !matches!(selected, Some("gemini-api-key" | "api-key" | "vertex-ai"))
}

/// The access token in Gemini CLI's credentials, or why it can't be used.
/// `expiry_date` is in milliseconds. A token past it is not sent.
fn access_token(data: &[u8], now: DateTime<Utc>) -> Result<String, Fail> {
    let Some(object) = serde_json::from_slice::<Value>(data).ok().and_then(|v| v.as_object().cloned()) else {
        return Err(Unavailability::LocalLoginMissing);
    };
    let token = object
        .get("access_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .ok_or(Unavailability::LocalLoginExpired)?;
    if let Some(milliseconds) = object.get("expiry_date").and_then(Value::as_f64) {
        if milliseconds / 1000.0 <= now.timestamp_millis() as f64 / 1000.0 {
            return Err(Unavailability::LocalLoginExpired);
        }
    }
    Ok(token.to_string())
}

#[derive(Debug, Default, PartialEq, Eq)]
struct CodeAssist {
    project: Option<String>,
    /// Google's own name for the plan, when it gives one.
    plan: Option<String>,
    /// Google lists this client as no longer serving the account's tier.
    unsupported_client: bool,
}

fn code_assist_from(body: &[u8]) -> CodeAssist {
    let Some(object) = serde_json::from_slice::<Value>(body).ok().and_then(|v| v.as_object().cloned()) else {
        return CodeAssist::default();
    };

    let companion = object.get("cloudaicompanionProject");
    let raw_project = companion.and_then(Value::as_str).map(str::to_string).or_else(|| {
        let project = companion?.as_object()?;
        project
            .get("id")
            .and_then(Value::as_str)
            .or_else(|| project.get("projectId").and_then(Value::as_str))
            .map(str::to_string)
    });
    let project = raw_project.map(|p| p.trim().to_string()).filter(|p| !p.is_empty());

    // A named paid tier is the most specific thing Google says about the plan.
    let tier_name = |key: &str| object.get(key).and_then(|t| t.get("name")).and_then(Value::as_str);
    let paid = tier_name("paidTier");
    let current = tier_name("currentTier");
    let plan = [paid, current]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|p| !p.is_empty())
        .map(str::to_string);

    let ineligible = object.get("ineligibleTiers").and_then(Value::as_array).cloned().unwrap_or_default();
    let unsupported = paid.is_none()
        && object.get("currentTier").is_none()
        && ineligible.iter().any(|tier| {
            ["reasonCode", "reasonMessage"]
                .iter()
                .filter_map(|key| tier.get(*key).and_then(Value::as_str))
                .any(says_unsupported)
        });

    CodeAssist { project, plan, unsupported_client: unsupported }
}

/// Google's wording for "this client no longer serves this account".
fn says_unsupported(text: &str) -> bool {
    let text = text.to_lowercase();
    text.contains("unsupported_client") || text.contains("ineligibletiererror")
}

fn text(body: &[u8]) -> String {
    String::from_utf8_lossy(body).into_owned()
}

/// What the quota call's status means. A 403 is a refused login unless Google
/// said the account's tier is no longer served, which means there is no plan
/// to read and signing in again would not change that.
fn quota_status(status: u16, body: &[u8], unsupported_client: bool) -> Result<(), Fail> {
    if status == 403 && (unsupported_client || says_unsupported(&text(body))) {
        return Err(Unavailability::NoPlan);
    }
    match status {
        200..=299 => Ok(()),
        300..=399 | 401 | 403 => Err(Unavailability::LocalLoginExpired),
        429 => Err(Unavailability::RateLimited),
        _ => Err(Unavailability::ServerError),
    }
}

#[derive(Deserialize)]
struct Reply {
    buckets: Option<Vec<Bucket>>,
}

#[derive(Deserialize)]
struct Bucket {
    #[serde(rename = "modelId")]
    model_id: Option<String>,
    #[serde(rename = "remainingFraction")]
    remaining_fraction: Option<f64>,
    #[serde(rename = "resetTime")]
    reset_time: Option<String>,
}

/// One row per model, at the least any of its buckets has left (with that
/// bucket's reset). Google states the reset but never the length, so the
/// windows are daily only as a sort key and no length is claimed.
fn reading(body: &[u8], plan: Option<&str>, ctx: &FetchContext, account: &AccountKey) -> Result<ProviderUsage, Fail> {
    let reply: Reply = profile::decode(body)?;
    let buckets = reply.buckets.ok_or(Unavailability::UnreadableReply)?;

    let mut least: BTreeMap<String, (f64, Option<String>)> = BTreeMap::new();
    for bucket in buckets {
        let Some(model) = bucket.model_id.map(|m| m.trim().to_string()).filter(|m| !m.is_empty()) else {
            continue;
        };
        let Some(remaining) = bucket.remaining_fraction.filter(|r| r.is_finite() && (0.0..=1.0).contains(r)) else {
            continue;
        };
        if least.get(&model).is_some_and(|(known, _)| *known <= remaining) {
            continue;
        }
        least.insert(model, (remaining, bucket.reset_time));
    }

    let windows: Vec<UsageWindow> = least
        .into_iter()
        .map(|(model, (remaining, reset))| {
            let mut window = UsageWindow::new(format!("gemini.{model}"), WindowKind::Daily, 1.0 - remaining, 86_400)
                .with_scope(model)
                .with_reset(profile::date(reset.as_deref()))
                .exhausted(remaining <= 0.0);
            window.reports_length = false;
            window
        })
        .collect();

    if windows.is_empty() {
        return Err(Unavailability::NoLimitsReported);
    }
    Ok(profile::reading(account, windows, ctx).with_plan(plan.map(str::to_string)))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;
    use crate::providers::profile::test_support::{context, fixture};

    fn account() -> AccountKey {
        AccountKey::primary(Provider::Gemini)
    }

    /// A home folder of its own, with `.gemini` holding the given files.
    fn home(files: &[(&str, &str)]) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let home = std::env::temp_dir().join(format!(
            "pulse-gemini-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let folder = home.join(".gemini");
        std::fs::create_dir_all(&folder).unwrap();
        for (name, contents) in files {
            std::fs::write(folder.join(name), contents).unwrap();
        }
        home
    }

    #[test]
    fn each_model_is_its_own_row_at_the_least_any_bucket_has_left() {
        let ctx = context();
        let usage = reading(&fixture("gemini-retrieve-user-quota.json"), Some("Gemini Code Assist Standard"), &ctx, &account()).unwrap();

        assert_eq!(usage.account, account());
        assert_eq!(usage.plan.as_deref(), Some("Gemini Code Assist Standard"));
        let scopes: Vec<_> = usage.windows.iter().filter_map(|w| w.scope.as_deref()).collect();
        assert_eq!(scopes, ["gemini-2.5-flash", "gemini-2.5-flash-lite", "gemini-2.5-pro"]);
        let used: Vec<_> = usage.windows.iter().map(|w| w.used_fraction).collect();
        assert_eq!(used, [0.125, 1.0, 0.5]);
        let exhausted: Vec<_> = usage.windows.iter().map(|w| w.is_exhausted).collect();
        assert_eq!(exhausted, [false, true, false]);
        // The pro row is its input-token bucket, with less left, and that bucket's reset.
        assert_eq!(usage.windows[2].resets_at, profile::date(Some("2026-07-17T01:00:00Z")));
        assert_eq!(usage.windows[1].resets_at, profile::date(Some("2026-07-17T00:00:00Z")));
    }

    #[test]
    fn google_states_a_reset_and_no_length_so_none_is_claimed() {
        let ctx = context();
        let usage = reading(&fixture("gemini-retrieve-user-quota.json"), None, &ctx, &account()).unwrap();
        assert!(usage.windows.iter().all(|w| w.kind == WindowKind::Daily && !w.reports_length));
    }

    #[test]
    fn a_bucket_with_no_model_or_a_bad_fraction_is_left_off() {
        let ctx = context();
        let usage = reading(&fixture("gemini-retrieve-user-quota.json"), None, &ctx, &account()).unwrap();
        assert!(!usage.windows.iter().any(|w| w.scope.as_deref() == Some("gemini-2.0-flash")));
        assert_eq!(usage.windows.len(), 3);

        let none = br#"{"buckets":[{"modelId":"gemini-2.5-pro","remainingFraction":1.4},{"remainingFraction":0.2}]}"#;
        assert_eq!(reading(none, None, &ctx, &account()).unwrap_err(), Unavailability::NoLimitsReported);
        let empty = br#"{"buckets":[]}"#;
        assert_eq!(reading(empty, None, &ctx, &account()).unwrap_err(), Unavailability::NoLimitsReported);
    }

    #[test]
    fn a_reply_that_is_not_a_quota_cannot_be_read() {
        let ctx = context();
        for json in [r#"{"error":"nope"}"#, "[]", "not json"] {
            assert_eq!(reading(json.as_bytes(), None, &ctx, &account()).unwrap_err(), Unavailability::UnreadableReply);
        }
    }

    #[test]
    fn the_account_lookup_gives_the_project_and_googles_plan_name() {
        let account = code_assist_from(&fixture("gemini-load-code-assist.json"));
        assert_eq!(account.project.as_deref(), Some("cloudaicompanion-123"));
        assert_eq!(account.plan.as_deref(), Some("Gemini Code Assist in Google One AI Pro"));
        assert!(!account.unsupported_client);

        let bare = code_assist_from(br#"{"currentTier":{"id":"free-tier","name":"Gemini Code Assist for individuals"},"cloudaicompanionProject":"  "}"#);
        assert_eq!(bare.project, None);
        assert_eq!(bare.plan.as_deref(), Some("Gemini Code Assist for individuals"));
    }

    #[test]
    fn an_account_google_moved_off_gemini_cli_has_no_plan_to_read() {
        let account = code_assist_from(&fixture("gemini-load-code-assist-unsupported.json"));
        assert!(account.unsupported_client);
        assert_eq!(account.plan, None);

        let refused = br#"{"error":{"status":"PERMISSION_DENIED"}}"#;
        assert_eq!(quota_status(403, refused, true), Err(Unavailability::NoPlan));
        // Without that, a 403 is a login Google would not take.
        assert_eq!(quota_status(403, refused, false), Err(Unavailability::LocalLoginExpired));
        // And Google can say it in the refusal itself.
        let said = br#"{"error":{"message":"IneligibleTierError"}}"#;
        assert_eq!(quota_status(403, said, false), Err(Unavailability::NoPlan));
    }

    #[test]
    fn statuses_on_the_quota_call() {
        assert_eq!(quota_status(401, b"", false), Err(Unavailability::LocalLoginExpired));
        assert_eq!(quota_status(429, b"", false), Err(Unavailability::RateLimited));
        assert_eq!(quota_status(500, b"", false), Err(Unavailability::ServerError));
        assert_eq!(quota_status(200, b"{}", false), Ok(()));
    }

    #[test]
    fn the_saved_login_is_used_only_while_it_is_current() {
        let now = DateTime::from_timestamp(1_800_000_000, 0).unwrap();
        let later = now.timestamp_millis() + 60_000;
        let earlier = now.timestamp_millis() - 60_000;

        assert_eq!(
            access_token(format!(r#"{{"access_token":"ya29.token","expiry_date":{later}}}"#).as_bytes(), now),
            Ok("ya29.token".to_string())
        );
        assert_eq!(
            access_token(format!(r#"{{"access_token":"ya29.token","expiry_date":{earlier}}}"#).as_bytes(), now),
            Err(Unavailability::LocalLoginExpired)
        );
        assert_eq!(access_token(br#"{"refresh_token":"1//r"}"#, now), Err(Unavailability::LocalLoginExpired));
        assert_eq!(access_token(b"not json", now), Err(Unavailability::LocalLoginMissing));
    }

    #[test]
    fn a_key_or_vertex_sign_in_is_not_a_google_login() {
        let cases = [
            (r#"{"security":{"auth":{"selectedType":"gemini-api-key"}}}"#, false),
            (r#"{"security":{"auth":{"selectedType":"vertex-ai"}}}"#, false),
            (r#"{"selectedAuthType":"gemini-api-key"}"#, false),
            (r#"{"security":{"auth":{"selectedType":"oauth-personal"}}}"#, true),
            (r#"{"theme":"dark"}"#, true),
        ];
        for (settings, google) in cases {
            assert_eq!(uses_google_login(settings.as_bytes()), google, "{settings}");
        }
    }

    #[tokio::test]
    async fn no_login_or_a_key_sign_in_is_asked_for_not_sent() {
        let mut ctx = context();
        let empty = home(&[]);
        ctx.home = empty.clone();
        assert_eq!(Gemini.fetch(&ctx, &account()).await.state, crate::model::UsageState::Unavailable(Unavailability::LocalLoginMissing));
        std::fs::remove_dir_all(&empty).ok();

        let keyed = home(&[
            ("settings.json", r#"{"security":{"auth":{"selectedType":"gemini-api-key"}}}"#),
            ("oauth_creds.json", r#"{"access_token":"ya29.old","expiry_date":9999999999999}"#),
        ]);
        ctx.home = keyed.clone();
        assert_eq!(Gemini.fetch(&ctx, &account()).await.state, crate::model::UsageState::Unavailable(Unavailability::LocalLoginMissing));
        std::fs::remove_dir_all(&keyed).ok();
    }

    #[tokio::test]
    async fn an_expired_login_is_reported_never_renewed_and_never_sent() {
        let mut ctx = context();
        let original = r#"{"access_token":"ya29.old","refresh_token":"1//r","expiry_date":1000}"#;
        let stale = home(&[("oauth_creds.json", original)]);
        ctx.home = stale.clone();
        assert_eq!(Gemini.fetch(&ctx, &account()).await.state, crate::model::UsageState::Unavailable(Unavailability::LocalLoginExpired));
        // Left exactly as the CLI wrote it.
        let after = std::fs::read_to_string(stale.join(".gemini").join("oauth_creds.json")).unwrap();
        assert_eq!(after, original);
        std::fs::remove_dir_all(&stale).ok();
    }
}
