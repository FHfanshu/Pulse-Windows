// Ported from upstream Auth/OAuthLogin.swift.
//! OAuth with PKCE and a loopback callback (Claude Code), OpenAI's device-code flow (Codex) and
//! RFC 8628's (Grok), plus token renewal.
//!
//! **They are not variations on one flow.** RFC 8628 polls the ordinary token endpoint and gets
//! tokens straight back; OpenAI's polls an endpoint of its own, answers with an authorization code
//! *and the proof key it generated itself*, and that code is then exchanged against a redirect
//! address of theirs. Writing one as a special case of the other means a parser that reads
//! neither reliably.
//!
//! The client ids are public: they ship in every copy of those CLIs. The parameters were read out
//! of the CLIs and the providers' discovery documents (see Docs/providers/authentication.md
//! upstream), because an OAuth flow with one parameter wrong fails in a way that looks like the
//! user's fault.

use std::time::Duration;

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde_json::Value;

use super::loopback::LoopbackCallback;
use super::util::{challenge, described, form_body, jwt_claims, random_token, seconds};
use super::{AccountCredentials, Failure};
use crate::provider::Provider;

/// How a device-code sign-in is shaped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceFlow {
    /// The specification as published: `POST` to this endpoint for a code, then poll the
    /// configuration's own token endpoint.
    Standard { code: &'static str },
    /// OpenAI's own, whose paths hang off this base.
    OpenAi { base: &'static str },
}

#[derive(Debug, Clone, Copy)]
pub struct Configuration {
    pub authorize: &'static str,
    pub token: &'static str,
    pub client_id: &'static str,
    pub scopes: &'static [&'static str],
    /// A port the provider's client is registered for, when it insists on one; `None` takes any
    /// free port.
    pub fixed_port: Option<u16>,
    pub redirect_path: &'static str,
    pub extra_authorize: &'static [(&'static str, &'static str)],
    /// Anthropic's token endpoint takes JSON; OpenAI's takes a form. Neither accepts the other.
    pub sends_json: bool,
    /// Anthropic's exchange carries the `state` back; OpenAI's is the four fields the
    /// specification names and nothing else. Sending one an extra field is not harmless.
    pub exchange_carries_state: bool,
    pub device_flow: Option<DeviceFlow>,
}

impl Configuration {
    pub fn of(provider: Provider) -> Option<Configuration> {
        match provider {
            Provider::ClaudeCode => Some(Configuration {
                authorize: "https://claude.com/cai/oauth/authorize",
                token: "https://platform.claude.com/v1/oauth/token",
                client_id: "9d1c250a-e61b-44d9-88ed-5944d1962f5e",
                // The narrowest set that can read an account's limits. The CLI asks for inference
                // and session scopes as well, which would let Pulse *spend* the plan it is only
                // supposed to be reporting on.
                scopes: &["user:profile"],
                fixed_port: None,
                redirect_path: "/callback",
                extra_authorize: &[("code", "true")],
                sends_json: true,
                exchange_carries_state: true,
                device_flow: None,
            }),
            Provider::Codex => Some(Configuration {
                authorize: "https://auth.openai.com/oauth/authorize",
                token: "https://auth.openai.com/oauth/token",
                client_id: "app_EMoamEEZ73f0CkXaXp7hrann",
                // The full set its own client asks for (codex-rs/login/src/server.rs). A subset
                // ended on OpenAI's error page before the browser ever came back.
                scopes: &["openid", "profile", "email", "offline_access", "api.connectors.read", "api.connectors.invoke"],
                // Not negotiable for the redirect flow: this client is registered for exactly
                // this loopback address. Unused now that Codex signs in by device code.
                fixed_port: Some(1455),
                redirect_path: "/auth/callback",
                extra_authorize: &[
                    ("id_token_add_organizations", "true"),
                    ("codex_cli_simplified_flow", "true"),
                    ("originator", "codex_cli_rs"),
                ],
                sends_json: false,
                exchange_carries_state: false,
                device_flow: Some(DeviceFlow::OpenAi { base: "https://auth.openai.com/api/accounts" }),
            }),
            Provider::Grok => Some(Configuration {
                // From xAI's discovery document (auth.x.ai/.well-known/openid-configuration).
                authorize: "https://auth.x.ai/oauth2/authorize",
                token: "https://auth.x.ai/oauth2/token",
                client_id: "b1a00492-073a-47ea-816f-4c329264a828",
                // `grok-cli:access` is what the CLI's own proxy is gated on, `email` keeps two
                // Grok accounts from both being offered as "Grok". `billing:read` looks right
                // and is refused (`invalid_scope`).
                scopes: &["openid", "email", "offline_access", "grok-cli:access"],
                fixed_port: None,
                redirect_path: "/callback",
                extra_authorize: &[],
                sends_json: false,
                exchange_carries_state: false,
                device_flow: Some(DeviceFlow::Standard { code: "https://auth.x.ai/oauth2/device/code" }),
            }),
            _ => None,
        }
    }

    fn scope(&self) -> String {
        self.scopes.join(" ")
    }
}

/// What to put in front of the user while a device-code sign-in is waiting: a short code, and
/// where to type it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevicePrompt {
    pub user_code: String,
    pub verification_url: String,
    /// RFC 8628's `device_code`, or OpenAI's `device_auth_id`.
    pub device_auth_id: String,
    pub interval: Duration,
    /// Whether `verification_url` already carries the code. Used where the service offers it
    /// (xAI does), never constructed where it does not (GitHub and OpenAI send none).
    pub prefilled: bool,
}

/// What one poll of a standard device flow came to.
#[derive(Debug, Clone, PartialEq)]
pub enum DeviceOutcome {
    Granted(AccountCredentials),
    Pending,
    SlowDown,
}

pub struct OAuthLogin;

impl OAuthLogin {
    /// How long to hold the callback open. Long enough to find a password and a second factor,
    /// short enough that an abandoned sign-in releases the button.
    pub const PATIENCE: Duration = Duration::from_secs(300);
    /// How long a device-code sign-in polls (both services' codes last about this long).
    pub const DEVICE_PATIENCE: Duration = Duration::from_secs(15 * 60);
    const DEFAULT_INTERVAL: f64 = 5.0;

    /// Whether this provider is signed in to by showing a code rather than by sending the
    /// browser back here.
    pub fn uses_device_code(provider: Provider) -> bool {
        Configuration::of(provider).is_some_and(|c| c.device_flow.is_some())
    }

    // ---- Device code ----------------------------------------------------------------------

    /// Asks the provider for a code to show the user.
    pub async fn start_device(http: &reqwest::Client, provider: Provider) -> Result<DevicePrompt, Failure> {
        let config = Configuration::of(provider).ok_or(Failure::Unsupported)?;
        match config.device_flow.ok_or(Failure::Unsupported)? {
            DeviceFlow::Standard { code } => {
                let body = standard_code_body(&config);
                let (status, bytes) = send(http, code, FORM, body).await?;
                let json = json_of(&bytes);
                if status != 200 {
                    return Err(Failure::Refused(refusal(code.rsplit('/').next().unwrap_or(code), status, json.as_ref())));
                }
                standard_prompt(&json.ok_or(Failure::UnreadableReply)?)
            }
            DeviceFlow::OpenAi { base } => {
                let url = format!("{base}/deviceauth/usercode");
                let body = serde_json::json!({ "client_id": config.client_id }).to_string();
                let (status, bytes) = send(http, &url, JSON, body).await?;
                if status != 200 {
                    return Err(Failure::Refused(described(status, &bytes, "usercode")));
                }
                openai_prompt(&json_of(&bytes).ok_or(Failure::UnreadableReply)?)
            }
        }
    }

    /// Waits for the user to enter that code, then turns what comes back into tokens.
    pub async fn await_device(
        http: &reqwest::Client,
        prompt: &DevicePrompt,
        provider: Provider,
    ) -> Result<AccountCredentials, Failure> {
        let config = Configuration::of(provider).ok_or(Failure::Unsupported)?;
        match config.device_flow.ok_or(Failure::Unsupported)? {
            DeviceFlow::Standard { .. } => Self::await_standard(http, prompt, &config).await,
            DeviceFlow::OpenAi { base } => Self::await_openai(http, prompt, &config, base).await,
        }
    }

    /// RFC 8628's other half: poll the ordinary token endpoint until it stops saying "not yet".
    /// `slow_down` is an instruction, not a failure: the interval grows by the five seconds the
    /// specification names and the poll continues.
    async fn await_standard(
        http: &reqwest::Client,
        prompt: &DevicePrompt,
        config: &Configuration,
    ) -> Result<AccountCredentials, Failure> {
        let mut wait = prompt.interval;
        let deadline = tokio::time::Instant::now() + Self::DEVICE_PATIENCE;
        while tokio::time::Instant::now() < deadline {
            tokio::time::sleep(wait).await;
            let body = standard_poll_body(config, &prompt.device_auth_id);
            let (status, bytes) = send(http, config.token, FORM, body).await?;
            match standard_poll(status, &bytes, Utc::now())? {
                DeviceOutcome::Granted(credentials) => return Ok(credentials),
                DeviceOutcome::Pending => {}
                DeviceOutcome::SlowDown => wait += Duration::from_secs(5),
            }
        }
        Err(Failure::TimedOut)
    }

    async fn await_openai(
        http: &reqwest::Client,
        prompt: &DevicePrompt,
        config: &Configuration,
        base: &str,
    ) -> Result<AccountCredentials, Failure> {
        let deadline = tokio::time::Instant::now() + Self::DEVICE_PATIENCE;
        let url = format!("{base}/deviceauth/token");
        while tokio::time::Instant::now() < deadline {
            let body = serde_json::json!({ "device_auth_id": prompt.device_auth_id, "user_code": prompt.user_code }).to_string();
            let (status, bytes) = send(http, &url, JSON, body).await?;
            if let Some((code, verifier)) = openai_poll(status, &bytes)? {
                let pairs = openai_exchange_pairs(config, &code, &verifier);
                return post_token(http, config, &pairs, Utc::now()).await;
            }
            tokio::time::sleep(prompt.interval).await;
        }
        Err(Failure::TimedOut)
    }

    // ---- Browser sign-in ------------------------------------------------------------------

    /// Opens the provider's consent page (through `open`) and waits for the browser to come
    /// back. Returns the tokens; storing them is the caller's business.
    ///
    /// The listener is bound first: the port is part of the redirect address, and the redirect
    /// address is part of the request the browser is about to be sent to. Both halves of the
    /// exchange have to name the same one. `lang` picks the language of the page the browser
    /// lands on.
    pub async fn sign_in(
        http: &reqwest::Client,
        provider: Provider,
        lang: &str,
        open: &(dyn Fn(&str) + Send + Sync),
    ) -> Result<AccountCredentials, Failure> {
        let config = Configuration::of(provider).ok_or(Failure::Unsupported)?;
        let verifier = random_token();
        let state = random_token();

        let listener = LoopbackCallback::bind(config.fixed_port, config.redirect_path).await?;
        let redirect = format!("http://localhost:{}{}", listener.port(), config.redirect_path);
        let url = authorize_url(&config, &redirect, &challenge(&verifier), &state);
        open(&url);

        let code = listener.await_code(&state, Self::PATIENCE, lang).await?;
        let pairs = exchange_pairs(&config, &code, &verifier, &redirect, &state);
        post_token(http, &config, &pairs, Utc::now()).await
    }

    // ---- Renewal --------------------------------------------------------------------------

    /// Renews an account's access token.
    ///
    /// The provider may hand back a new refresh token; when it does not, the old one stays valid
    /// and is carried forward. Nothing here touches the CLI's own stored login, so a renewal
    /// cannot sign the user out of it.
    pub async fn refresh(
        http: &reqwest::Client,
        credentials: &AccountCredentials,
        provider: Provider,
        now: DateTime<Utc>,
    ) -> Result<AccountCredentials, Failure> {
        let config = Configuration::of(provider).ok_or(Failure::Unsupported)?;
        let pairs = refresh_pairs(&config, &credentials.refresh_token);
        let renewed = post_token(http, &config, &pairs, now).await?;
        Ok(carry_forward(renewed, credentials))
    }
}

// ---- Request building (pure) --------------------------------------------------------------

const FORM: &str = "application/x-www-form-urlencoded";
const JSON: &str = "application/json";

type Pairs = Vec<(String, String)>;

fn pairs(items: &[(&str, &str)]) -> Pairs {
    items.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

/// The authorize request: provider-specific items first, then the standard ones, in upstream's
/// order. Values are strictly percent-encoded (a space is `%20`).
pub fn authorize_url(config: &Configuration, redirect: &str, challenge: &str, state: &str) -> String {
    let mut query: Pairs = pairs(config.extra_authorize);
    query.extend(pairs(&[
        ("client_id", config.client_id),
        ("response_type", "code"),
        ("redirect_uri", redirect),
        ("scope", &config.scope()),
        ("code_challenge", challenge),
        ("code_challenge_method", "S256"),
        ("state", state),
    ]));
    format!("{}?{}", config.authorize, form_body(&query))
}

pub fn exchange_pairs(config: &Configuration, code: &str, verifier: &str, redirect: &str, state: &str) -> Pairs {
    let mut body = pairs(&[
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect),
        ("client_id", config.client_id),
        ("code_verifier", verifier),
    ]);
    if config.exchange_carries_state {
        body.push(("state".into(), state.into()));
    }
    body
}

pub fn refresh_pairs(config: &Configuration, refresh_token: &str) -> Pairs {
    pairs(&[
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", config.client_id),
        ("scope", &config.scope()),
    ])
}

pub fn standard_code_body(config: &Configuration) -> String {
    form_body(&[("client_id", config.client_id), ("scope", &config.scope())])
}

pub fn standard_poll_body(config: &Configuration, device_code: &str) -> String {
    form_body(&[
        ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ("device_code", device_code),
        ("client_id", config.client_id),
    ])
}

/// OpenAI's exchange of the code the provider generated, against a redirect address of theirs.
pub fn openai_exchange_pairs(config: &Configuration, code: &str, verifier: &str) -> Pairs {
    pairs(&[
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", "https://auth.openai.com/deviceauth/callback"),
        ("client_id", config.client_id),
        ("code_verifier", verifier),
    ])
}

/// The token request's content type and body: JSON for Anthropic, a strictly encoded form for the
/// rest.
pub fn token_request(config: &Configuration, items: &Pairs) -> (&'static str, String) {
    if config.sends_json {
        let map: serde_json::Map<String, Value> = items.iter().map(|(k, v)| (k.clone(), Value::String(v.clone()))).collect();
        (JSON, Value::Object(map).to_string())
    } else {
        (FORM, form_body(items))
    }
}

// ---- Reply parsing (pure) -----------------------------------------------------------------

fn json_of(bytes: &[u8]) -> Option<Value> {
    serde_json::from_slice(bytes).ok()
}

fn refusal(step: &str, status: u16, json: Option<&Value>) -> String {
    let said = json.and_then(|j| j.get("error_description").or_else(|| j.get("error"))).and_then(Value::as_str);
    match said {
        Some(said) => format!("{step}: HTTP {status} — {said}"),
        None => format!("{step}: HTTP {status}"),
    }
}

fn interval_of(reply: &Value) -> Duration {
    Duration::from_secs_f64(seconds(reply.get("interval")).unwrap_or(OAuthLogin::DEFAULT_INTERVAL).max(1.0))
}

/// RFC 8628's reply says everything: the code, where to type it, how often to ask, and the
/// handle to ask with.
pub fn standard_prompt(reply: &Value) -> Result<DevicePrompt, Failure> {
    let text = |key: &str| reply.get(key).and_then(Value::as_str).filter(|s| !s.is_empty());
    let (Some(code), Some(handle), Some(page)) = (text("user_code"), text("device_code"), text("verification_uri")) else {
        return Err(Failure::UnreadableReply);
    };
    let complete = text("verification_uri_complete");
    Ok(DevicePrompt {
        user_code: code.into(),
        verification_url: complete.unwrap_or(page).into(),
        device_auth_id: handle.into(),
        interval: interval_of(reply),
        prefilled: complete.is_some(),
    })
}

pub fn openai_prompt(reply: &Value) -> Result<DevicePrompt, Failure> {
    let text = |key: &str| reply.get(key).and_then(Value::as_str).filter(|s| !s.is_empty());
    let (Some(code), Some(id)) = (text("user_code").or_else(|| text("usercode")), text("device_auth_id")) else {
        return Err(Failure::UnreadableReply);
    };
    Ok(DevicePrompt {
        user_code: code.into(),
        verification_url: "https://auth.openai.com/codex/device".into(),
        device_auth_id: id.into(),
        interval: interval_of(reply),
        prefilled: false,
    })
}

/// One poll of the standard flow. **A refusal and a "still waiting" arrive with the same HTTP
/// 400**; only the body's `error` tells them apart, so the body is read before anything is
/// concluded from the status.
pub fn standard_poll(status: u16, body: &[u8], now: DateTime<Utc>) -> Result<DeviceOutcome, Failure> {
    let json = json_of(body);
    if status == 200 {
        return credentials_from(&json.ok_or(Failure::UnreadableReply)?, now).map(DeviceOutcome::Granted);
    }
    match json.as_ref().and_then(|j| j.get("error")).and_then(Value::as_str) {
        Some("authorization_pending") => Ok(DeviceOutcome::Pending),
        Some("slow_down") => Ok(DeviceOutcome::SlowDown),
        _ => Err(Failure::Refused(refusal("oauth/token", status, json.as_ref()))),
    }
}

/// OpenAI's poll: `None` while the user has not finished (403 or 404 is that provider's own
/// convention for "still waiting"), the authorization code and its verifier once they have.
pub fn openai_poll(status: u16, body: &[u8]) -> Result<Option<(String, String)>, Failure> {
    if status == 403 || status == 404 {
        return Ok(None);
    }
    if status != 200 {
        return Err(Failure::Refused(described(status, body, "deviceauth/token")));
    }
    let reply = json_of(body).ok_or(Failure::UnreadableReply)?;
    let text = |key: &str| reply.get(key).and_then(Value::as_str).map(String::from);
    match (text("authorization_code"), text("code_verifier")) {
        (Some(code), Some(verifier)) => Ok(Some((code, verifier))),
        _ => Err(Failure::UnreadableReply),
    }
}

/// A token endpoint's answer. The status is read **before** the body is parsed: a provider that
/// answers a refusal with an HTML error page has plenty to say, and reading the body first threw
/// all of it away as "couldn't read the reply".
pub fn token_reply(status: u16, body: &[u8], now: DateTime<Utc>) -> Result<AccountCredentials, Failure> {
    let json = json_of(body);
    if status != 200 {
        return Err(Failure::Refused(refusal("oauth/token", status, json.as_ref())));
    }
    credentials_from(&json.ok_or(Failure::UnreadableReply)?, now)
}

/// A token reply turned into a login. Shared with the device-code poll, which reaches the same
/// endpoint by a different grant.
pub fn credentials_from(json: &Value, now: DateTime<Utc>) -> Result<AccountCredentials, Failure> {
    let access = json.get("access_token").and_then(Value::as_str).ok_or(Failure::UnreadableReply)?;
    let lifetime = json.get("expires_in").and_then(Value::as_f64).ok_or(Failure::UnreadableReply)?;
    Ok(AccountCredentials {
        access_token: access.into(),
        refresh_token: json.get("refresh_token").and_then(Value::as_str).unwrap_or_default().into(),
        expires_at: now + ChronoDuration::milliseconds((lifetime * 1000.0) as i64),
        account_name: account_name(json),
        account_id: account_id(access),
    })
}

/// A renewal keeps the old refresh token when none comes back, and what it knew of the account.
pub fn carry_forward(mut renewed: AccountCredentials, old: &AccountCredentials) -> AccountCredentials {
    if renewed.refresh_token.is_empty() {
        renewed.refresh_token = old.refresh_token.clone();
    }
    renewed.account_name = renewed.account_name.or_else(|| old.account_name.clone());
    renewed.account_id = renewed.account_id.or_else(|| old.account_id.clone());
    renewed
}

/// Whatever the reply says about who this is, so two subscriptions are not both offered as
/// "Codex". OpenAI returns an id token with an email in it; Anthropic names the account.
fn account_name(json: &Value) -> Option<String> {
    let non_empty = |v: Option<&Value>| v.and_then(Value::as_str).filter(|s| !s.is_empty()).map(String::from);
    if let Some(claims) = json.get("id_token").and_then(Value::as_str).and_then(jwt_claims) {
        if let Some(email) = non_empty(claims.get("email")) {
            return Some(email);
        }
    }
    non_empty(json.get("account").and_then(|a| a.get("email_address")))
}

/// The account the token was issued for, which Codex's usage endpoint wants in a header. It is
/// nested in a namespaced claim, and only the access token carries it.
fn account_id(access_token: &str) -> Option<String> {
    jwt_claims(access_token)?
        .get("https://api.openai.com/auth")?
        .get("chatgpt_account_id")?
        .as_str()
        .map(String::from)
}

// ---- Network ------------------------------------------------------------------------------

/// One POST. Cancellation is the caller dropping the future; a transport failure is "the service
/// didn't respond".
async fn send(http: &reqwest::Client, url: &str, content_type: &str, body: String) -> Result<(u16, Vec<u8>), Failure> {
    let reply = http
        .post(url)
        .timeout(Duration::from_secs(30))
        .header("Content-Type", content_type)
        .header("Accept", "application/json")
        .body(body)
        .send()
        .await
        .map_err(|_| Failure::Refused("The service didn't respond.".into()))?;
    let status = reply.status().as_u16();
    let bytes = reply.bytes().await.map_err(|_| Failure::Refused("The service didn't respond.".into()))?;
    Ok((status, bytes.to_vec()))
}

async fn post_token(
    http: &reqwest::Client,
    config: &Configuration,
    items: &Pairs,
    now: DateTime<Utc>,
) -> Result<AccountCredentials, Failure> {
    let (content_type, body) = token_request(config, items);
    let (status, bytes) = send(http, config.token, content_type, body).await?;
    token_reply(status, &bytes, now)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::util::base64url;
    use serde_json::json;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-10-09T12:00:00Z").unwrap().with_timezone(&Utc)
    }

    fn jwt(claims: Value) -> String {
        format!("{}.{}.sig", base64url(b"{}"), base64url(claims.to_string().as_bytes()))
    }

    #[test]
    fn only_the_three_oauth_providers_have_a_configuration() {
        for provider in Provider::ALL {
            let has = Configuration::of(*provider).is_some();
            assert_eq!(has, matches!(provider, Provider::ClaudeCode | Provider::Codex | Provider::Grok), "{provider:?}");
            // Exactly the providers that allow extra accounts.
            assert_eq!(has, provider.supports_multiple_accounts(), "{provider:?}");
        }
        assert!(!OAuthLogin::uses_device_code(Provider::ClaudeCode));
        assert!(OAuthLogin::uses_device_code(Provider::Codex));
        assert!(OAuthLogin::uses_device_code(Provider::Grok));
    }

    #[test]
    fn claude_authorize_request_matches_the_published_client() {
        let config = Configuration::of(Provider::ClaudeCode).unwrap();
        let url = authorize_url(&config, "http://localhost:53682/callback", "CHAL", "STATE");
        assert_eq!(
            url,
            "https://claude.com/cai/oauth/authorize?code=true&client_id=9d1c250a-e61b-44d9-88ed-5944d1962f5e\
             &response_type=code&redirect_uri=http%3A%2F%2Flocalhost%3A53682%2Fcallback&scope=user%3Aprofile\
             &code_challenge=CHAL&code_challenge_method=S256&state=STATE"
        );
    }

    #[test]
    fn codex_redirect_flow_asks_for_the_full_scope_set_on_its_fixed_port() {
        let config = Configuration::of(Provider::Codex).unwrap();
        assert_eq!(config.fixed_port, Some(1455));
        let url = authorize_url(&config, "http://localhost:1455/auth/callback", "C", "S");
        assert!(url.starts_with("https://auth.openai.com/oauth/authorize?id_token_add_organizations=true&codex_cli_simplified_flow=true&originator=codex_cli_rs&client_id="));
        assert!(url.contains("scope=openid%20profile%20email%20offline_access%20api.connectors.read%20api.connectors.invoke"));
    }

    #[test]
    fn exchange_carries_state_only_where_the_provider_takes_it() {
        let claude = Configuration::of(Provider::ClaudeCode).unwrap();
        let codex = Configuration::of(Provider::Codex).unwrap();
        let with = exchange_pairs(&claude, "CODE", "VER", "http://localhost:1/callback", "ST");
        assert!(with.contains(&("state".into(), "ST".into())));
        let without = exchange_pairs(&codex, "CODE", "VER", "http://localhost:1455/auth/callback", "ST");
        assert!(!without.iter().any(|(k, _)| k == "state"));
        assert_eq!(without.len(), 5);
    }

    #[test]
    fn token_requests_are_json_for_anthropic_and_a_strict_form_for_the_rest() {
        let claude = Configuration::of(Provider::ClaudeCode).unwrap();
        let (kind, body) = token_request(&claude, &refresh_pairs(&claude, "r1"));
        assert_eq!(kind, "application/json");
        let parsed: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["grant_type"], "refresh_token");
        assert_eq!(parsed["refresh_token"], "r1");
        assert_eq!(parsed["scope"], "user:profile");

        let grok = Configuration::of(Provider::Grok).unwrap();
        let (kind, body) = token_request(&grok, &refresh_pairs(&grok, "a+b/c"));
        assert_eq!(kind, "application/x-www-form-urlencoded");
        assert!(body.contains("refresh_token=a%2Bb%2Fc"));
        assert!(body.contains("scope=openid%20email%20offline_access%20grok-cli%3Aaccess"));
    }

    #[test]
    fn standard_device_request_and_prompt() {
        let grok = Configuration::of(Provider::Grok).unwrap();
        assert_eq!(
            standard_code_body(&grok),
            "client_id=b1a00492-073a-47ea-816f-4c329264a828&scope=openid%20email%20offline_access%20grok-cli%3Aaccess"
        );
        assert_eq!(
            standard_poll_body(&grok, "DEV"),
            "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code&device_code=DEV&client_id=b1a00492-073a-47ea-816f-4c329264a828"
        );

        let reply = json!({
            "user_code": "ABCD-EFGH", "device_code": "dev-1", "verification_uri": "https://auth.x.ai/device",
            "verification_uri_complete": "https://auth.x.ai/device?user_code=ABCD-EFGH", "interval": 7
        });
        let prompt = standard_prompt(&reply).unwrap();
        assert_eq!(prompt.user_code, "ABCD-EFGH");
        assert!(prompt.prefilled);
        assert_eq!(prompt.verification_url, "https://auth.x.ai/device?user_code=ABCD-EFGH");
        assert_eq!(prompt.interval, Duration::from_secs(7));

        // No pre-filled link and a string interval below the floor.
        let plain = standard_prompt(&json!({"user_code": "X", "device_code": "d", "verification_uri": "https://v", "interval": "0"})).unwrap();
        assert!(!plain.prefilled);
        assert_eq!(plain.verification_url, "https://v");
        assert_eq!(plain.interval, Duration::from_secs(1));
        assert_eq!(standard_prompt(&json!({"user_code": "X"})), Err(Failure::UnreadableReply));
    }

    #[test]
    fn standard_poll_tells_waiting_from_refusal_by_the_body() {
        let pending = br#"{"error":"authorization_pending"}"#;
        assert_eq!(standard_poll(400, pending, now()), Ok(DeviceOutcome::Pending));
        assert_eq!(standard_poll(400, br#"{"error":"slow_down"}"#, now()), Ok(DeviceOutcome::SlowDown));
        let denied = standard_poll(400, br#"{"error":"access_denied","error_description":"User declined"}"#, now());
        assert_eq!(denied, Err(Failure::Refused("oauth/token: HTTP 400 — User declined".into())));
        assert_eq!(standard_poll(502, b"<html>", now()), Err(Failure::Refused("oauth/token: HTTP 502".into())));

        let granted = standard_poll(200, br#"{"access_token":"at","refresh_token":"rt","expires_in":21600}"#, now()).unwrap();
        let DeviceOutcome::Granted(credentials) = granted else { panic!("expected a grant") };
        assert_eq!(credentials.access_token, "at");
        assert_eq!(credentials.expires_at, now() + ChronoDuration::hours(6));
    }

    #[test]
    fn openai_device_prompt_and_poll() {
        let prompt = openai_prompt(&json!({"usercode": "WXYZ-1234", "device_auth_id": "auth-1", "interval": "3"})).unwrap();
        assert_eq!(prompt.user_code, "WXYZ-1234");
        assert_eq!(prompt.verification_url, "https://auth.openai.com/codex/device");
        assert_eq!(prompt.interval, Duration::from_secs(3));
        assert!(!prompt.prefilled);
        assert_eq!(openai_prompt(&json!({"user_code": "A"})), Err(Failure::UnreadableReply));

        // 403 and 404 both mean "still waiting".
        assert_eq!(openai_poll(403, b""), Ok(None));
        assert_eq!(openai_poll(404, b"{}"), Ok(None));
        assert!(matches!(openai_poll(500, b"boom"), Err(Failure::Refused(_))));
        assert_eq!(
            openai_poll(200, br#"{"authorization_code":"ac","code_verifier":"cv","code_challenge":"x"}"#),
            Ok(Some(("ac".into(), "cv".into())))
        );
        assert_eq!(openai_poll(200, br#"{"authorization_code":"ac"}"#), Err(Failure::UnreadableReply));

        let codex = Configuration::of(Provider::Codex).unwrap();
        let (kind, body) = token_request(&codex, &openai_exchange_pairs(&codex, "ac", "cv"));
        assert_eq!(kind, "application/x-www-form-urlencoded");
        assert_eq!(
            body,
            "grant_type=authorization_code&code=ac&redirect_uri=https%3A%2F%2Fauth.openai.com%2Fdeviceauth%2Fcallback\
             &client_id=app_EMoamEEZ73f0CkXaXp7hrann&code_verifier=cv"
        );
    }

    #[test]
    fn codex_reply_names_the_account_by_email_and_id() {
        let access = jwt(json!({"https://api.openai.com/auth": {"chatgpt_account_id": "acct-9"}}));
        let id_token = jwt(json!({"email": "ann@example.com"}));
        let reply = json!({"access_token": access, "refresh_token": "rt", "expires_in": 3600, "id_token": id_token});
        let credentials = credentials_from(&reply, now()).unwrap();
        assert_eq!(credentials.account_name.as_deref(), Some("ann@example.com"));
        assert_eq!(credentials.account_id.as_deref(), Some("acct-9"));
        assert_eq!(credentials.expires_at, now() + ChronoDuration::hours(1));
    }

    #[test]
    fn anthropic_reply_names_the_account_from_its_account_object() {
        let reply = json!({"access_token": "opaque", "refresh_token": "rt", "expires_in": 18000.5, "account": {"email_address": "bo@example.com"}});
        let credentials = credentials_from(&reply, now()).unwrap();
        assert_eq!(credentials.account_name.as_deref(), Some("bo@example.com"));
        assert!(credentials.account_id.is_none());
    }

    #[test]
    fn token_replies_report_the_step_status_and_provider_words() {
        let body = br#"{"error":"invalid_scope","error_description":"Scope 'billing:read' is not allowed for this client"}"#;
        assert_eq!(
            token_reply(400, body, now()),
            Err(Failure::Refused("oauth/token: HTTP 400 — Scope 'billing:read' is not allowed for this client".into()))
        );
        assert_eq!(token_reply(200, b"<html>", now()), Err(Failure::UnreadableReply));
        assert_eq!(token_reply(200, br#"{"access_token":"x"}"#, now()), Err(Failure::UnreadableReply));
    }

    #[test]
    fn a_renewal_keeps_the_old_refresh_token_and_account_when_none_is_sent() {
        let old = AccountCredentials {
            access_token: "old".into(),
            refresh_token: "keep-me".into(),
            expires_at: now(),
            account_name: Some("ann@example.com".into()),
            account_id: Some("acct-9".into()),
        };
        let renewed = credentials_from(&json!({"access_token": "new", "expires_in": 60}), now()).unwrap();
        let merged = carry_forward(renewed, &old);
        assert_eq!(merged.access_token, "new");
        assert_eq!(merged.refresh_token, "keep-me");
        assert_eq!(merged.account_name.as_deref(), Some("ann@example.com"));
        assert_eq!(merged.account_id.as_deref(), Some("acct-9"));

        let rotated = credentials_from(&json!({"access_token": "new", "refresh_token": "rotated", "expires_in": 60}), now()).unwrap();
        assert_eq!(carry_forward(rotated, &old).refresh_token, "rotated");
    }
}
