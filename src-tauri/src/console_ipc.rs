// Ported from upstream Settings/DeepSeekConsoleGroup.swift and OpenCodeConsoleGroup.swift (the
// reading half), and Settings/AccountHistoryModel.swift's provider-side histories.
//! The consoles' second credential: a signed-in session for DeepSeek's and OpenCode's web
//! consoles, kept beside the API key and never instead of it.
//!
//! **Windows.** macOS reads the session out of an installed browser. A Chromium browser on Windows
//! encrypts its cookies against other processes, so the sign-in happens in a WebView2 window of
//! Pulse's own, with its own profile folder (`%APPDATA%\Pulse\WebView2-consoles`), and the session
//! is read from that window:
//!
//! - DeepSeek keeps its token in the page's `localStorage` (`userToken`). A script run in the page
//!   hands it back by starting a navigation to a made-up address (`pulse-capture.invalid/<nonce>`)
//!   that the window's navigation handler recognises and cancels, so the token never reaches the
//!   network and the remote page is granted no IPC at all.
//! - OpenCode keeps two cookies, one HttpOnly, which the window's cookie jar yields directly.
//!
//! The window is created hidden and shown only if the session it already holds is not accepted
//! (nobody is asked to sign in twice). Each candidate is tried against the console before it is
//! kept, so the pane says whether it works rather than only that something was stored. Only the
//! DeepSeek token and the two OpenCode cookies are kept, encrypted (DPAPI) by the secret store.
//!
//! Also here: the provider-side histories the account pane's "Usage history" draws (DeepSeek's
//! console, OpenCode's request log, Z.ai's statistics), as the same card shape as the transcript
//! histories (`pulse_core::history::AccountHistory`).

use std::collections::{BTreeMap, HashSet};
use std::hash::{BuildHasher, Hasher};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::Utc;
use pulse_core::history::{AccountHistory, HistoryRead};
use pulse_core::providers::deepseek_console::{self, Failure};
use pulse_core::providers::opencode_console::{self, Read, Resolved, WorkspaceCache};
use pulse_core::providers::opencode_console_history::{OpenCodeConsoleHistory, Progress};
use pulse_core::providers::zai::Storefront;
use pulse_core::providers::zai_history;
use pulse_core::service::{http_client, FetchContext, HttpClient};
use pulse_core::spend::Calendar;
use pulse_core::{AccountKey, Provider};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::state::{self, AppState};

/// How long a sign-in window may stay open waiting for the reader before it is given up on.
const PATIENCE: Duration = Duration::from_secs(10 * 60);
/// How long the hidden window is given to show a session it already holds, before it is shown.
const GRACE: Duration = Duration::from_secs(4);
const POLL: Duration = Duration::from_millis(1500);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Console {
    DeepSeek,
    OpenCode,
}

impl Console {
    fn from_raw(provider: &str) -> Result<Self, String> {
        match Provider::from_raw(provider) {
            Some(Provider::DeepSeek) => Ok(Console::DeepSeek),
            Some(Provider::OpenCodeGo) => Ok(Console::OpenCode),
            _ => Err(format!("{provider} has no console session")),
        }
    }

    fn provider(self) -> Provider {
        match self {
            Console::DeepSeek => Provider::DeepSeek,
            Console::OpenCode => Provider::OpenCodeGo,
        }
    }

    fn secret_id(self) -> String {
        match self {
            Console::DeepSeek => deepseek_console::secret_id(),
            Console::OpenCode => opencode_console::secret_id(),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Console::DeepSeek => "console-deepseek",
            Console::OpenCode => "console-opencode",
        }
    }

    fn url(self) -> &'static str {
        match self {
            Console::DeepSeek => "https://platform.deepseek.com/",
            Console::OpenCode => "https://opencode.ai/auth",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Console::DeepSeek => "DeepSeek console",
            Console::OpenCode => "OpenCode console",
        }
    }
}

/// What a read came to; the pane says it in the reader's language.
#[derive(Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum ReadOutcome {
    /// A session was found, the console accepted it and it is kept. OpenCode names the workspace.
    Read { workspace: Option<String> },
    /// A session was found and kept, but the console did not answer. It is tried again on the
    /// next refresh.
    Unanswered,
    /// The window was closed (or left alone too long) with no accepted sign-in.
    NotFound,
    /// A sign-in window for this console is already open.
    Busy,
}

fn client(state: &AppState) -> HttpClient {
    let client = http_client(&state.settings());
    opencode_console::set_http(client.clone());
    client
}

/// Whether a session is kept for the console (upstream `hasSession`).
#[tauri::command]
pub fn console_session(state: State<AppState>, provider: String) -> Result<bool, String> {
    let console = Console::from_raw(&provider)?;
    Ok(state.secrets.get(&console.secret_id()).is_some_and(|v| !v.trim().is_empty()))
}

/// Signs in (or reads the session the sign-in window already holds) and keeps it.
#[tauri::command]
pub async fn console_read(app: AppHandle, provider: String) -> Result<ReadOutcome, String> {
    let console = Console::from_raw(&provider)?;
    if app.get_webview_window(console.label()).is_some() {
        if let Some(window) = app.get_webview_window(console.label()) {
            let _ = window.show();
            let _ = window.set_focus();
        }
        return Ok(ReadOutcome::Busy);
    }
    let state = app.state::<AppState>();
    let http = client(&state);
    let nonce = random_nonce();
    let captured: Arc<Mutex<Option<String>>> = Arc::default();
    let window = match console {
        // The navigation handler has to be there before the page loads.
        Console::DeepSeek => open_capturing(&app, console, &nonce, captured.clone())?,
        Console::OpenCode => open(&app, console)?,
    };

    let started = Instant::now();
    let mut shown = false;
    let mut tried: HashSet<String> = HashSet::new();
    let outcome = loop {
        if app.get_webview_window(console.label()).is_none() {
            break ReadOutcome::NotFound;
        }
        if started.elapsed() > PATIENCE {
            break ReadOutcome::NotFound;
        }
        let candidate = match console {
            Console::DeepSeek => {
                let _ = window.eval(capture_script(&nonce));
                tokio::time::sleep(POLL).await;
                let hex = captured.lock().unwrap().take();
                hex.and_then(|h| unhex(&h)).and_then(|entry| deepseek_console::token_from_entry(&entry))
            }
            Console::OpenCode => {
                tokio::time::sleep(POLL).await;
                let jar = window.clone();
                let cookies = tauri::async_runtime::spawn_blocking(move || {
                    Url::parse("https://opencode.ai/").ok().and_then(|url| jar.cookies_for_url(url).ok()).unwrap_or_default()
                })
                .await
                .unwrap_or_default();
                let jar: BTreeMap<String, String> =
                    cookies.iter().map(|c| (c.name().to_string(), c.value().to_string())).collect();
                opencode_console::header_from_cookies(&jar)
            }
        };
        if let Some(candidate) = candidate.filter(|c| tried.insert(c.clone())) {
            match check(console, &http, &candidate).await {
                Check::Accepted(workspace) => {
                    keep(&app, console, &candidate).await;
                    break ReadOutcome::Read { workspace };
                }
                Check::Unanswered => {
                    keep(&app, console, &candidate).await;
                    break ReadOutcome::Unanswered;
                }
                // Turned away: a lapsed session left in the window's profile. Let the reader sign in.
                Check::Refused => {}
            }
        }
        if !shown && started.elapsed() > GRACE {
            let _ = window.show();
            let _ = window.set_focus();
            shown = true;
        }
    };
    let _ = window.destroy();
    Ok(outcome)
}

enum Check {
    Accepted(Option<String>),
    Refused,
    Unanswered,
}

/// Tried at once, so the row says whether the session works rather than only that it was stored.
async fn check(console: Console, http: &HttpClient, candidate: &str) -> Check {
    match console {
        Console::DeepSeek => match deepseek_console::balance(http, candidate).await {
            Ok(_) => Check::Accepted(None),
            Err(Failure::SignedOut) => Check::Refused,
            Err(Failure::Failed) => Check::Unanswered,
        },
        Console::OpenCode => {
            WorkspaceCache::shared().forget();
            match WorkspaceCache::shared().resolve(http, candidate).await {
                Resolved::Workspace(workspace) => Check::Accepted(Some(workspace.name.unwrap_or(workspace.id))),
                Resolved::SignedOut => Check::Refused,
                Resolved::Failed => Check::Unanswered,
            }
        }
    }
}

/// Stores the session in its own slot beside the key and has the account read again.
async fn keep(app: &AppHandle, console: Console, session: &str) {
    let state = app.state::<AppState>();
    state.secrets.set(&console.secret_id(), Some(session));
    forget_history(console).await;
    refresh_account(app, console);
}

async fn forget_history(console: Console) {
    match console {
        Console::DeepSeek => deepseek_console::History::shared().forget().await,
        Console::OpenCode => OpenCodeConsoleHistory::shared().forget(),
    }
}

/// The limits (OpenCode) and the balance (DeepSeek) can now be read without a key.
fn refresh_account(app: &AppHandle, console: Console) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        state.store.refresh(state.settings(), &console.provider().account_id()).await;
        state::emit_usage(&app);
    });
}

/// Forgets the kept session (upstream `remove()`).
#[tauri::command]
pub async fn console_remove(app: AppHandle, provider: String) -> Result<(), String> {
    let console = Console::from_raw(&provider)?;
    let state = app.state::<AppState>();
    state.secrets.set(&console.secret_id(), None);
    if console == Console::OpenCode {
        WorkspaceCache::shared().forget();
    }
    forget_history(console).await;
    refresh_account(&app, console);
    Ok(())
}

// MARK: - The sign-in window

fn profile_folder() -> std::path::PathBuf {
    pulse_core::paths::data_dir().join("WebView2-consoles")
}

fn builder(app: &AppHandle, console: Console) -> Result<WebviewWindowBuilder<'_, tauri::Wry, AppHandle>, String> {
    let url = Url::parse(console.url()).map_err(|e| e.to_string())?;
    Ok(WebviewWindowBuilder::new(app, console.label(), WebviewUrl::External(url))
        .title(console.title())
        .inner_size(520.0, 740.0)
        .center()
        .visible(false)
        .data_directory(profile_folder()))
}

fn open(app: &AppHandle, console: Console) -> Result<WebviewWindow, String> {
    builder(app, console)?.build().map_err(|e| e.to_string())
}

/// DeepSeek's window: its navigation handler takes the token the page script hands over.
fn open_capturing(
    app: &AppHandle,
    console: Console,
    nonce: &str,
    captured: Arc<Mutex<Option<String>>>,
) -> Result<WebviewWindow, String> {
    let path = format!("/{nonce}");
    builder(app, console)?
        .on_navigation(move |url| {
            if url.host_str() == Some("pulse-capture.invalid") && url.path() == path {
                if let Some(hex) = url.fragment() {
                    *captured.lock().unwrap() = Some(hex.to_string());
                }
                return false;
            }
            true
        })
        .build()
        .map_err(|e| e.to_string())
}

/// Reads `localStorage.userToken` on the console's origin and hands it back, hex-encoded, in the
/// fragment of a navigation the window cancels.
fn capture_script(nonce: &str) -> String {
    format!(
        "(function(){{try{{if(location.origin!=='https://platform.deepseek.com')return;\
         var v=localStorage.getItem('userToken');if(!v)return;\
         var h=Array.from(new TextEncoder().encode(v)).map(function(b){{return b.toString(16).padStart(2,'0')}}).join('');\
         location.href='https://pulse-capture.invalid/{nonce}#'+h;}}catch(e){{}}}})();"
    )
}

fn random_nonce() -> String {
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u128(Utc::now().timestamp_nanos_opt().unwrap_or_default() as u128);
    format!("{:016x}", hasher.finish())
}

fn unhex(text: &str) -> Option<String> {
    if text.len() % 2 != 0 || text.len() > 64 * 1024 {
        return None;
    }
    let bytes: Option<Vec<u8>> = (0..text.len()).step_by(2).map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok()).collect();
    String::from_utf8(bytes?).ok()
}

// MARK: - Provider-side histories

/// The account's "Usage history" for a provider that is asked rather than scanned: DeepSeek's
/// console (30 days, by day, with what was charged), OpenCode Go's request log and Z.ai's
/// statistics. `currency` is the reader's ring currency for DeepSeek. OpenCode's arrives in pieces:
/// each saved or arriving batch is also emitted as `provider-history-progress {id, history}`.
#[tauri::command]
pub async fn provider_history(app: AppHandle, id: String, currency: Option<String>) -> Result<AccountHistory, String> {
    let account = AccountKey::from_id(&id).ok_or_else(|| format!("not an account: {id}"))?;
    let provider = account.provider;
    let state = app.state::<AppState>();
    let settings = state.settings();
    let none = |read| Ok(AccountHistory::empty(provider, read));
    if !account.is_primary() || !matches!(provider, Provider::DeepSeek | Provider::OpenCodeGo | Provider::Zai) {
        return none(HistoryRead::Unsupported);
    }
    if !settings.enabled_accounts.contains(&id) {
        return none(HistoryRead::NotAsked);
    }
    let http = client(&state);
    let now = Utc::now();
    let card = |read, ledger: &pulse_core::spend::Ledger| Ok(AccountHistory::of(provider, read, ledger, now, &Calendar::local()));
    match provider {
        Provider::DeepSeek => {
            let Some(token) = state.secrets.get(&deepseek_console::secret_id()).filter(|t| !t.trim().is_empty()) else {
                return none(HistoryRead::NotConfigured);
            };
            match deepseek_console::History::shared().ledger(&http, token.trim(), currency.as_deref()).await {
                deepseek_console::Read::Answered(ledger) => card(HistoryRead::Answered, &ledger),
                _ => none(HistoryRead::Failed),
            }
        }
        Provider::OpenCodeGo => {
            let Some(cookie) = state.secrets.get(&opencode_console::secret_id()).filter(|t| !t.trim().is_empty()) else {
                return none(HistoryRead::NotConfigured);
            };
            let (emitter, emitted) = (app.clone(), id.clone());
            let progress: Progress = Arc::new(move |ledger| {
                let history = AccountHistory::of(Provider::OpenCodeGo, HistoryRead::Answered, &ledger, Utc::now(), &Calendar::local());
                let _ = emitter.emit("provider-history-progress", serde_json::json!({ "id": emitted, "history": history }));
            });
            match OpenCodeConsoleHistory::shared().ledger(cookie.trim(), now, Some(progress)).await {
                Read::Answered(ledger) => card(HistoryRead::Answered, &ledger),
                _ => none(HistoryRead::Failed),
            }
        }
        _ => {
            let ctx = FetchContext::from_env(settings, state.secrets.clone());
            match zai_history::history(&ctx, &account, Storefront::ZAI).await {
                (read, Some(ledger)) => card(read, &ledger),
                (read, None) => none(read),
            }
        }
    }
}
