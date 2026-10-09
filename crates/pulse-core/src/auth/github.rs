// Ported from upstream Auth/GitHubDeviceLogin.swift.
//! Signing in to GitHub for Copilot's quota, by device code.
//!
//! **A sign-in rather than a pasted token, and that is a security decision.** The endpoint accepts
//! any GitHub OAuth token, so asking someone to paste the one `gh` already holds would work, and
//! that token carries `repo` and `workflow`: the run of their source code, handed over to draw a
//! percentage. This asks for `read:user` and nothing else.
//!
//! Pulse cannot register an OAuth app with GitHub, so it drives the public client the VS Code
//! Copilot plugin uses. The consent page names the editor rather than Pulse, and this is not an
//! official integration. The flow is RFC 8628: ask for a code, show it, poll until the browser is
//! done. Nothing is redirected back to this PC, so there is no local port.
//!
//! **The verification link must not carry the code.** RFC 8628 has a field for a pre-filled
//! address and GitHub deliberately does not send one: pre-filling is the device-code phishing
//! attack (an attacker sends a link carrying *their* code and the victim approves it). Typing the
//! code is what makes the person consent to *this* device. The clipboard is the answer instead.
//! If the service ever offers `verification_uri_complete`, that is its decision to make.

use std::time::Duration;

use serde_json::{json, Value};

const CLIENT_ID: &str = "Iv1.b507a08c87ecfe98";
/// The narrowest scope the quota endpoint will answer for.
const SCOPE: &str = "read:user";
const CODE_URL: &str = "https://github.com/login/device/code";
const TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
/// GitHub's codes last fifteen minutes; stopping sooner would report a failure while the code on
/// screen was still good.
const PATIENCE: Duration = Duration::from_secs(900);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prompt {
    pub user_code: String,
    /// Where to send the browser. The code is not in it, unless the service itself offered one.
    pub verification_url: String,
    pub device_code: String,
    pub interval: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    Refused(String),
    Declined,
    TimedOut,
}

impl Failure {
    /// An upstream English string (passed through `t()` in the UI), or GitHub's own words.
    pub fn message(&self) -> String {
        match self {
            Failure::Refused(said) => said.clone(),
            Failure::Declined => "Sign-in was cancelled.".into(),
            Failure::TimedOut => "The browser didn't come back.".into(),
        }
    }
}

/// What one poll came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Poll {
    Token(String),
    Pending,
    /// GitHub's instruction, within reason: the interval to use from now on.
    SlowDown(Duration),
}

/// Asks GitHub for a code to put on screen.
pub async fn start(http: &reqwest::Client) -> Result<Prompt, Failure> {
    let reply = post(http, CODE_URL, json!({ "client_id": CLIENT_ID, "scope": SCOPE })).await?;
    prompt_from(&reply)
}

/// Polls until the user has finished in the browser, and returns the token. Cancelling is the
/// caller dropping this future.
pub async fn await_token(http: &reqwest::Client, prompt: &Prompt) -> Result<String, Failure> {
    let deadline = tokio::time::Instant::now() + PATIENCE;
    let mut wait = prompt.interval;
    while tokio::time::Instant::now() < deadline {
        tokio::time::sleep(wait).await;
        let reply = post(http, TOKEN_URL, poll_body(&prompt.device_code)).await?;
        match poll(&reply)? {
            Poll::Token(token) => return Ok(token),
            Poll::Pending => {}
            Poll::SlowDown(interval) => wait = interval,
        }
    }
    Err(Failure::TimedOut)
}

// ---- Pure parts ---------------------------------------------------------------------------

pub fn poll_body(device_code: &str) -> Value {
    json!({
        "client_id": CLIENT_ID,
        "device_code": device_code,
        "grant_type": "urn:ietf:params:oauth:grant-type:device_code",
    })
}

pub fn prompt_from(reply: &Value) -> Result<Prompt, Failure> {
    let text = |key: &str| reply.get(key).and_then(Value::as_str).filter(|s| !s.is_empty());
    let (Some(user_code), Some(device_code), Some(verification)) =
        (text("user_code"), text("device_code"), text("verification_uri"))
    else {
        return Err(Failure::Refused(described(reply, "login/device/code", None, None)));
    };
    // Their floor, not ours: polling faster than this earns a `slow_down`.
    let seconds = reply.get("interval").and_then(Value::as_u64).unwrap_or(5).clamp(1, 60);
    Ok(Prompt {
        user_code: user_code.into(),
        // Only if the service offers one itself. Building it here is the thing the page warns about.
        verification_url: text("verification_uri_complete").unwrap_or(verification).into(),
        device_code: device_code.into(),
        interval: Duration::from_secs(seconds),
    })
}

/// GitHub answers 200 while it waits, with the specification's own error words.
pub fn poll(reply: &Value) -> Result<Poll, Failure> {
    if let Some(token) = reply.get("access_token").and_then(Value::as_str).filter(|t| !t.is_empty()) {
        return Ok(Poll::Token(token.into()));
    }
    match reply.get("error").and_then(Value::as_str) {
        None | Some("authorization_pending") => Ok(Poll::Pending),
        Some("slow_down") => {
            // Their instruction, and ignoring it gets the attempt refused, but bounded: the
            // deadline is only tested at the top of the loop, so an interval taken on trust could
            // park the poll in one sleep for longer than the code lives.
            let seconds = reply.get("interval").and_then(Value::as_u64).unwrap_or(10).clamp(1, 60);
            Ok(Poll::SlowDown(Duration::from_secs(seconds)))
        }
        Some("access_denied") => Err(Failure::Declined),
        Some("expired_token") => Err(Failure::TimedOut),
        Some(error) => Err(Failure::Refused(described(reply, "login/oauth/access_token", None, Some(error)))),
    }
}

/// GitHub's own words where it has them: "device_flow_disabled" says something a generic failure
/// cannot.
pub fn described(reply: &Value, step: &str, status: Option<u16>, error: Option<&str>) -> String {
    let said = reply
        .get("error_description")
        .and_then(Value::as_str)
        .or(error)
        .or_else(|| reply.get("error").and_then(Value::as_str));
    let prefix = match status {
        Some(status) => format!("{step}: HTTP {status}"),
        None => step.to_string(),
    };
    match said {
        Some(said) => format!("{prefix} — {said}"),
        None => prefix,
    }
}

async fn post(http: &reqwest::Client, url: &str, body: Value) -> Result<Value, Failure> {
    let reply = http
        .post(url)
        .timeout(Duration::from_secs(20))
        // Without this GitHub answers form-encoded, which parses as nothing.
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .map_err(|_| Failure::Refused("The service didn't respond.".into()))?;
    let status = reply.status().as_u16();
    let bytes = reply.bytes().await.map_err(|_| Failure::Refused("The service didn't respond.".into()))?;
    let json: Option<Value> = serde_json::from_slice(&bytes).ok();
    // A device flow answers 200 while it waits, so only a real failure status is one, and even
    // then the body usually names the reason.
    let step = url.rsplit('/').next().unwrap_or(url);
    if status >= 400 {
        return Err(Failure::Refused(described(&json.unwrap_or(Value::Null), step, Some(status), None)));
    }
    json.ok_or_else(|| Failure::Refused("Couldn't read the reply.".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prompt_never_invents_a_prefilled_link() {
        let reply = json!({"user_code": "WDJB-MJHT", "device_code": "dc", "verification_uri": "https://github.com/login/device", "interval": 5, "expires_in": 900});
        let prompt = prompt_from(&reply).unwrap();
        assert_eq!(prompt.user_code, "WDJB-MJHT");
        assert_eq!(prompt.verification_url, "https://github.com/login/device");
        assert!(!prompt.verification_url.contains("WDJB"));
        assert_eq!(prompt.interval, Duration::from_secs(5));

        let offered = json!({"user_code": "A", "device_code": "d", "verification_uri": "https://v", "verification_uri_complete": "https://v?c=A", "interval": 500});
        let prompt = prompt_from(&offered).unwrap();
        assert_eq!(prompt.verification_url, "https://v?c=A");
        assert_eq!(prompt.interval, Duration::from_secs(60));
    }

    #[test]
    fn a_reply_without_a_code_reports_githubs_words() {
        let reply = json!({"error": "device_flow_disabled", "error_description": "Device Flow must be explicitly enabled"});
        assert_eq!(
            prompt_from(&reply),
            Err(Failure::Refused("login/device/code — Device Flow must be explicitly enabled".into()))
        );
    }

    #[test]
    fn requests_ask_for_read_user_only() {
        let body = poll_body("dc");
        assert_eq!(body["client_id"], "Iv1.b507a08c87ecfe98");
        assert_eq!(body["grant_type"], "urn:ietf:params:oauth:grant-type:device_code");
        assert_eq!(SCOPE, "read:user");
    }

    #[test]
    fn polling_follows_the_specifications_words() {
        assert_eq!(poll(&json!({"access_token": "gho_x", "token_type": "bearer"})), Ok(Poll::Token("gho_x".into())));
        assert_eq!(poll(&json!({"error": "authorization_pending"})), Ok(Poll::Pending));
        assert_eq!(poll(&json!({})), Ok(Poll::Pending));
        assert_eq!(poll(&json!({"error": "slow_down", "interval": 10})), Ok(Poll::SlowDown(Duration::from_secs(10))));
        assert_eq!(poll(&json!({"error": "slow_down", "interval": 9000})), Ok(Poll::SlowDown(Duration::from_secs(60))));
        assert_eq!(poll(&json!({"error": "access_denied"})), Err(Failure::Declined));
        assert_eq!(poll(&json!({"error": "expired_token"})), Err(Failure::TimedOut));
        assert_eq!(
            poll(&json!({"error": "incorrect_client_credentials", "error_description": "The client_id is wrong"})),
            Err(Failure::Refused("login/oauth/access_token — The client_id is wrong".into()))
        );
    }

    #[test]
    fn failures_say_what_the_ui_translates() {
        assert_eq!(Failure::Declined.message(), "Sign-in was cancelled.");
        assert_eq!(Failure::TimedOut.message(), "The browser didn't come back.");
    }
}
