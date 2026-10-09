// Ported from upstream Auth/CursorWebLogin.swift (and `CursorAppLogin.expiry(of:)`).
//! Signing in to a second Cursor account, which is what a second Grok Bot allowance is.
//!
//! **This is not OAuth.** Cursor has no authorize/token pair for a third party: it has a login page
//! that takes a challenge and a nonce, and a polling endpoint that hands the tokens back once the
//! browser has finished:
//!
//! 1. `GET cursor.com/loginDeepControl?challenge=...&uuid=...&mode=login&...` opens in the browser.
//!    Nothing comes back to this PC, so there is no loopback port.
//! 2. `GET api2.cursor.sh/auth/poll?uuid=...&verifier=...` answers 404 while the browser has not
//!    finished and 200 with `accessToken` / `refreshToken` once it has.
//!
//! The proof key *is* PKCE's: a random 32 bytes base64url-encoded is the verifier, and the
//! challenge is the base64url of its SHA-256 of **the encoded string**, not of the raw bytes. Read
//! out of Cursor's own client. Not public API.
//!
//! No provider in this build uses it yet (Grok Bot is not ported); the flow is kept so the
//! provider can be added without redoing the sign-in.

use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
use serde_json::Value;

use super::util::{challenge, form_body, jwt_claims, random_token};
use super::{AccountCredentials, Failure};

const WEBSITE: &str = "https://cursor.com";
const BACKEND: &str = "https://api2.cursor.sh";
/// Cursor's own client gives up after 150 tries at two seconds; the same order.
const PATIENCE: Duration = Duration::from_secs(300);
const INTERVAL: Duration = Duration::from_secs(2);

/// What the user is sent to, and what the poll needs afterwards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt {
    pub login_url: String,
    pub uuid: String,
    pub verifier: String,
}

pub fn start() -> Attempt {
    let verifier = random_token();
    let uuid = uuid::Uuid::new_v4().to_string().to_lowercase();
    attempt(verifier, uuid)
}

fn attempt(verifier: String, uuid: String) -> Attempt {
    let query = form_body(&[
        ("challenge", challenge(&verifier).as_str()),
        ("uuid", uuid.as_str()),
        ("mode", "login"),
        // What the page says it is signing in to. "sand" is Cursor's own name for Grok Bot.
        ("redirectTarget", "sand"),
        ("supportsSelectedTeamLogin", "true"),
    ]);
    Attempt { login_url: format!("{WEBSITE}/loginDeepControl?{query}"), uuid, verifier }
}

pub fn poll_url(attempt: &Attempt) -> String {
    format!("{BACKEND}/auth/poll?{}", form_body(&[("uuid", attempt.uuid.as_str()), ("verifier", attempt.verifier.as_str())]))
}

/// Opens the page (through `open`) and waits for the browser to finish with it.
pub async fn sign_in(http: &reqwest::Client, open: &(dyn Fn(&str) + Send + Sync)) -> Result<AccountCredentials, Failure> {
    let attempt = start();
    open(&attempt.login_url);

    let deadline = tokio::time::Instant::now() + PATIENCE;
    while tokio::time::Instant::now() < deadline {
        tokio::time::sleep(INTERVAL).await;
        // Every failure but a refusal is a stumble: this poll runs for minutes and one dropped
        // connection is not a reason to send the user back to the start.
        let Ok(reply) = http
            .get(poll_url(&attempt))
            .timeout(Duration::from_secs(20))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .send()
            .await
        else {
            continue;
        };
        let status = reply.status().as_u16();
        let Ok(bytes) = reply.bytes().await else { continue };
        if let Some(credentials) = poll_reply(status, &bytes)? {
            return Ok(credentials);
        }
    }
    Err(Failure::TimedOut)
}

/// `None` while the browser has not finished. **404 is "not yet", not "wrong address"**: the nonce
/// has nothing filed against it until the page completes. A 403 carrying an error is the one
/// refusal that ends the attempt.
pub fn poll_reply(status: u16, body: &[u8]) -> Result<Option<AccountCredentials>, Failure> {
    let json: Option<Value> = serde_json::from_slice(body).ok();
    if status == 403 {
        if let Some(said) = json.as_ref().and_then(|j| j.get("error")).and_then(Value::as_str) {
            return Err(Failure::Refused(format!("auth/poll: {said}")));
        }
    }
    if status != 200 {
        return Ok(None);
    }
    let text = |key: &str| json.as_ref().and_then(|j| j.get(key)).and_then(Value::as_str).map(String::from);
    let (Some(access), Some(refresh)) = (text("accessToken"), text("refreshToken")) else {
        return Err(Failure::UnreadableReply);
    };
    // The reply states no lifetime and does not need to: the token says so itself (about 60 days).
    let expires_at = expiry(&access).ok_or(Failure::UnreadableReply)?;
    Ok(Some(AccountCredentials {
        access_token: access,
        refresh_token: refresh,
        expires_at,
        // The token carries no email, so the account gets a number and the user can rename it.
        account_name: None,
        account_id: None,
    }))
}

/// When a token stops being accepted, read from the token itself.
pub fn expiry(token: &str) -> Option<DateTime<Utc>> {
    let seconds = jwt_claims(token)?.get("exp")?.as_f64()?;
    Utc.timestamp_opt(seconds as i64, 0).single()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::util::base64url;
    use sha2::{Digest, Sha256};

    fn token(exp: i64) -> String {
        format!("e30.{}.sig", base64url(format!(r#"{{"sub":"auth0|user_1","exp":{exp}}}"#).as_bytes()))
    }

    #[test]
    fn the_login_url_carries_the_challenge_of_the_encoded_verifier() {
        let attempt = attempt("VERIFIER-STRING".into(), "0b0c6c1e-0000-4000-8000-000000000001".into());
        let expected = base64url(&Sha256::digest(b"VERIFIER-STRING"));
        assert_eq!(
            attempt.login_url,
            format!(
                "https://cursor.com/loginDeepControl?challenge={expected}&uuid=0b0c6c1e-0000-4000-8000-000000000001\
                 &mode=login&redirectTarget=sand&supportsSelectedTeamLogin=true"
            )
        );
        assert_eq!(
            poll_url(&attempt),
            "https://api2.cursor.sh/auth/poll?uuid=0b0c6c1e-0000-4000-8000-000000000001&verifier=VERIFIER-STRING"
        );
    }

    #[test]
    fn a_fresh_attempt_is_random_and_lowercase() {
        let (a, b) = (start(), start());
        assert_ne!(a.verifier, b.verifier);
        assert_eq!(a.uuid, a.uuid.to_lowercase());
        assert_eq!(a.uuid.len(), 36);
    }

    #[test]
    fn not_yet_is_a_404_and_only_a_403_with_an_error_ends_it() {
        assert_eq!(poll_reply(404, b""), Ok(None));
        assert_eq!(poll_reply(500, b"oops"), Ok(None));
        assert_eq!(poll_reply(403, b"<html>"), Ok(None));
        assert_eq!(poll_reply(403, br#"{"error":"Not authorized"}"#), Err(Failure::Refused("auth/poll: Not authorized".into())));
    }

    #[test]
    fn a_finished_login_reads_its_lifetime_from_the_token() {
        let access = token(1_900_000_000);
        let body = format!(r#"{{"accessToken":"{access}","refreshToken":"rt"}}"#);
        let credentials = poll_reply(200, body.as_bytes()).unwrap().unwrap();
        assert_eq!(credentials.refresh_token, "rt");
        assert_eq!(credentials.expires_at.timestamp(), 1_900_000_000);
        assert!(credentials.account_name.is_none());

        assert_eq!(poll_reply(200, br#"{"accessToken":"opaque","refreshToken":"rt"}"#), Err(Failure::UnreadableReply));
        assert_eq!(poll_reply(200, b"{}"), Err(Failure::UnreadableReply));
    }
}
