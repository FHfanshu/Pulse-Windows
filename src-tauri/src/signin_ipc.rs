// Ported from upstream Settings/ProviderSignInModel.swift (the model the shell holds) and the
// account-removing half of Settings/AccountsGroup.swift.
//! Signing in from Settings: another subscription of a provider that can have more than one (the
//! browser flow, or a device code), and Copilot's own GitHub sign-in.
//!
//! The state lives here, not in a pane. A device-code sign-in polls for fifteen minutes, and the
//! Cancel button, the code and the error have to be where the reader left them when they come back
//! to the provider's pane. Every change is announced with the `signin-changed` event carrying the
//! whole [`SignInState`]; `signin_state` reads it on mount.
//!
//! A sign-in that succeeds for an added account stores the login (`login:<id>` in the secret
//! store), adds the account to `settings.extraAccounts`, saves, emits `settings-changed`, wakes the
//! refresh loop and asks Settings to open `account:<id>` through `settings-navigate`.

use std::sync::Mutex;

use pulse_core::auth::{self, github, AccountCredentials, OAuthLogin, SecretLogins};
use pulse_core::model::AccountKey;
use pulse_core::service::http_client;
use pulse_core::Provider;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::state::{self, AppState};

/// The key Copilot's GitHub token is saved under (its primary account id).
const COPILOT_KEY: &str = "copilot";

/// The code to type, and where to type it.
#[derive(Serialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PromptInfo {
    pub user_code: String,
    pub verification_url: String,
    /// Whether the link already carries the code (xAI's does; GitHub's and OpenAI's never do).
    pub prefilled: bool,
}

#[derive(Serialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SignInError {
    pub provider: String,
    /// An upstream English string or the provider's own words; the UI passes it through `t()`.
    pub message: String,
}

/// Everything the Settings sign-in rows draw (upstream `ProviderSignInModel`'s properties).
#[derive(Serialize, Clone, Default, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SignInState {
    /// The provider an added-account sign-in is currently open for.
    pub signing_in: Option<String>,
    pub sign_in_error: Option<SignInError>,
    /// Shown while a device-code sign-in is waiting.
    pub device_prompt: Option<PromptInfo>,
    /// Copilot's GitHub sign-in is waiting.
    pub github_active: bool,
    pub github_prompt: Option<PromptInfo>,
    pub github_error: Option<String>,
    /// A GitHub token is saved for Copilot.
    pub github_signed_in: bool,
}

struct Attempt {
    id: u64,
    provider: Provider,
    task: tauri::async_runtime::JoinHandle<()>,
}

#[derive(Default)]
struct Runtime {
    next_id: u64,
    added: Option<Attempt>,
    github: Option<Attempt>,
    sign_in_error: Option<SignInError>,
    device_prompt: Option<PromptInfo>,
    github_prompt: Option<PromptInfo>,
    github_error: Option<String>,
}

static RUNTIME: Mutex<Runtime> = Mutex::new(Runtime {
    next_id: 0,
    added: None,
    github: None,
    sign_in_error: None,
    device_prompt: None,
    github_prompt: None,
    github_error: None,
});

fn runtime() -> std::sync::MutexGuard<'static, Runtime> {
    RUNTIME.lock().unwrap_or_else(|e| e.into_inner())
}

fn current_state(app: &AppHandle) -> SignInState {
    let rt = runtime();
    let signed_in = app.state::<AppState>().secrets.get(COPILOT_KEY).is_some_and(|t| !t.trim().is_empty());
    SignInState {
        signing_in: rt.added.as_ref().map(|a| a.provider.raw().to_string()),
        sign_in_error: rt.sign_in_error.clone(),
        device_prompt: rt.device_prompt.clone(),
        github_active: rt.github.is_some(),
        github_prompt: rt.github_prompt.clone(),
        github_error: rt.github_error.clone(),
        github_signed_in: signed_in,
    }
}

fn announce(app: &AppHandle) {
    let _ = app.emit("signin-changed", current_state(app));
}

/// Whether the attempt is still the one on screen. A cancelled one has already had its state
/// cleared by the button that cancelled it, and by the time it unwinds the user may well have
/// started another.
fn is_current(github_flow: bool, id: u64) -> bool {
    let rt = runtime();
    let slot = if github_flow { &rt.github } else { &rt.added };
    slot.as_ref().is_some_and(|a| a.id == id)
}

#[tauri::command]
pub fn signin_state(app: AppHandle) -> SignInState {
    current_state(&app)
}

/// Starts a sign-in. `provider` is `"copilot"` for GitHub's device flow, otherwise a provider that
/// supports multiple accounts; `replacing` is the id of an existing added account to sign in to
/// again (omit it to add a new one).
#[tauri::command]
pub fn signin_start(app: AppHandle, provider: String, replacing: Option<String>) -> Result<(), String> {
    let provider = Provider::from_raw(&provider).ok_or("unknown provider")?;
    if provider == Provider::Copilot {
        start_github(&app)
    } else {
        let replacing = replacing.as_deref().map(|id| AccountKey::from_id(id).ok_or("unknown account")).transpose()?;
        start_added(&app, provider, replacing)
    }
}

/// The Cancel on a sign-in: `"copilot"` for GitHub's, any other provider for the added-account one.
#[tauri::command]
pub fn signin_cancel(app: AppHandle, provider: String) {
    {
        let mut rt = runtime();
        if provider == COPILOT_KEY {
            if let Some(attempt) = rt.github.take() {
                attempt.task.abort();
            }
            rt.github_prompt = None;
            rt.github_error = None;
        } else {
            if let Some(attempt) = rt.added.take() {
                attempt.task.abort();
            }
            rt.device_prompt = None;
        }
    }
    announce(&app);
}

/// Sign out. Copilot: forgets the token and asks for a fresh reading. An added account: forgets
/// its login but keeps the account (it reads "signed out" until signed in again; Remove deletes it).
#[tauri::command]
pub fn signin_sign_out(app: AppHandle, account: String) -> Result<(), String> {
    let key = AccountKey::from_id(&account).ok_or("unknown account")?;
    let state = app.state::<AppState>();
    if key.provider == Provider::Copilot {
        state.secrets.set(COPILOT_KEY, None);
        runtime().github_error = None;
    } else {
        SecretLogins::new(state.secrets.as_ref()).set(&key, None);
        // A raw token saved by an older build would otherwise keep the account signed in.
        state.secrets.set(&key.id(), None);
    }
    announce(&app);
    refresh_account(&app, key.id());
    Ok(())
}

/// Remove account: forgets its login, takes it off the rail and drops every per-account setting.
#[tauri::command]
pub fn signin_remove_account(app: AppHandle, account: String) -> Result<(), String> {
    let key = AccountKey::from_id(&account).ok_or("unknown account")?;
    if key.is_primary() {
        return Err("a provider's first account cannot be removed".into());
    }
    let state = app.state::<AppState>();
    SecretLogins::new(state.secrets.as_ref()).set(&key, None);
    state.secrets.set(&key.id(), None);

    let previous = state.settings();
    let mut settings = (*previous).clone();
    settings.remove_account(&key);
    commit_settings(&app, &previous, settings);
    Ok(())
}

/// Opens the page of the sign-in in progress ("Open page"). The address is the one the provider
/// handed back for this sign-in; a webview-supplied URL is never opened.
#[tauri::command]
pub fn signin_open_page(provider: String) {
    let rt = runtime();
    let prompt = if provider == COPILOT_KEY { rt.github_prompt.as_ref() } else { rt.device_prompt.as_ref() };
    if let Some(prompt) = prompt {
        open_url(&prompt.verification_url);
    }
}

/// Copies the code of the sign-in in progress ("Copy").
#[tauri::command]
pub fn signin_copy_code(provider: String) {
    let code = {
        let rt = runtime();
        let prompt = if provider == COPILOT_KEY { rt.github_prompt.as_ref() } else { rt.device_prompt.as_ref() };
        prompt.map(|p| p.user_code.clone())
    };
    if let Some(code) = code {
        copy_to_clipboard(&code);
    }
}

// ---- Added account -----------------------------------------------------------------------

fn start_added(app: &AppHandle, provider: Provider, replacing: Option<AccountKey>) -> Result<(), String> {
    if !provider.supports_multiple_accounts() {
        return Err(provider_unsupported());
    }
    {
        let mut rt = runtime();
        if rt.added.is_some() {
            return Err("Finish or close the sign-in already running, then try again.".into());
        }
        rt.next_id += 1;
        let id = rt.next_id;
        rt.sign_in_error = None;
        rt.device_prompt = None;
        let handle = app.clone();
        let task = tauri::async_runtime::spawn(async move { run_added(handle, id, provider, replacing).await });
        rt.added = Some(Attempt { id, provider, task });
    }
    announce(app);
    Ok(())
}

fn provider_unsupported() -> String {
    auth::Failure::Unsupported.message()
}

/// Runs the sign-in, then keeps whatever came back.
async fn run_added(app: AppHandle, id: u64, provider: Provider, replacing: Option<AccountKey>) {
    let outcome = obtain_credentials(&app, id, provider).await;
    if !is_current(false, id) {
        return;
    }
    let result = match outcome {
        Ok(credentials) => keep_added(&app, provider, replacing, credentials),
        Err(message) => Err(message),
    };
    {
        let mut rt = runtime();
        if let Err(message) = result {
            rt.sign_in_error = Some(SignInError { provider: provider.raw().to_string(), message });
        }
        rt.added = None;
        rt.device_prompt = None;
    }
    announce(&app);
}

async fn obtain_credentials(app: &AppHandle, id: u64, provider: Provider) -> Result<AccountCredentials, String> {
    let settings = app.state::<AppState>().settings();
    let http = http_client(&settings);
    let lang = crate::notify::language(&settings);
    let fail = |f: auth::Failure| f.message();

    if OAuthLogin::uses_device_code(provider) {
        // A code shown on the provider's own page: no local port to collide with the CLI's
        // sign-in, and nothing redirected back to this PC. Whether the page fills the code in
        // itself is the provider's decision, so the code goes on the clipboard either way and the
        // row's subtitle follows `prefilled`.
        let prompt = OAuthLogin::start_device(&http, provider).await.map_err(fail)?;
        if !is_current(false, id) {
            return Err(auth::Failure::Cancelled.message());
        }
        runtime().device_prompt = Some(PromptInfo {
            user_code: prompt.user_code.clone(),
            verification_url: prompt.verification_url.clone(),
            prefilled: prompt.prefilled,
        });
        announce(app);
        copy_to_clipboard(&prompt.user_code);
        open_url(&prompt.verification_url);
        OAuthLogin::await_device(&http, &prompt, provider).await.map_err(fail)
    } else {
        OAuthLogin::sign_in(&http, provider, lang, &open_url).await.map_err(fail)
    }
}

/// Stores the login and the account. Seeded from whatever the provider said about the account, so
/// two subscriptions are not both offered as "Codex".
fn keep_added(app: &AppHandle, provider: Provider, replacing: Option<AccountKey>, credentials: AccountCredentials) -> Result<(), String> {
    let state = app.state::<AppState>();
    let previous = state.settings();
    let mut settings = (*previous).clone();
    let logins = SecretLogins::new(state.secrets.as_ref());
    let save_failed = || "Couldn't save the login on this PC.".to_string();

    let account = match replacing {
        // Removed while the browser was open: there is nothing to sign in to any more.
        Some(existing) if !settings.has_account(&existing) => return Ok(()),
        Some(existing) => {
            if !logins.set(&existing, Some(&credentials)) {
                return Err(save_failed());
            }
            existing
        }
        None => {
            let label = settings.label_for_new(&credentials, provider);
            let key = settings.add_account(provider, &label);
            if !logins.set(&key, Some(&credentials)) {
                logins.set(&key, None);
                return Err(save_failed());
            }
            key
        }
    };

    let added = settings != *previous;
    if added {
        commit_settings(app, &previous, settings);
    } else {
        refresh_account(app, account.id());
    }
    navigate_settings(app, &account);
    Ok(())
}

/// Saves settings changed here and tells everyone, the way `update_settings` does.
fn commit_settings(app: &AppHandle, previous: &pulse_core::settings::AppSettings, settings: pulse_core::settings::AppSettings) {
    let state = app.state::<AppState>();
    state.save_settings(settings);
    crate::shell::apply(app, Some(previous));
    let _ = app.emit("settings-changed", &*state.settings());
    state::emit_usage(app);
    // The set of rings changed: a provider newly switched on has to be fetched.
    state.wake.notify_one();
}

/// Asks the Settings window, if open, to show the account's pane.
fn navigate_settings(app: &AppHandle, account: &AccountKey) {
    if app.get_webview_window("settings").is_some() {
        let _ = app.emit_to("settings", "settings-navigate", format!("account:{}", account.id()));
    }
}

fn refresh_account(app: &AppHandle, id: String) {
    let state = app.state::<AppState>();
    let (store, settings) = (state.store.clone(), state.settings());
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        store.refresh(settings, &id).await;
        state::emit_usage(&app);
        crate::notify::after_refresh(&app);
    });
}

// ---- Copilot -----------------------------------------------------------------------------

/// GitHub's device flow, for Copilot's quota.
fn start_github(app: &AppHandle) -> Result<(), String> {
    {
        let mut rt = runtime();
        if rt.github.is_some() {
            return Ok(());
        }
        rt.next_id += 1;
        let id = rt.next_id;
        rt.github_error = None;
        rt.github_prompt = None;
        let handle = app.clone();
        let task = tauri::async_runtime::spawn(async move { run_github(handle, id).await });
        rt.github = Some(Attempt { id, provider: Provider::Copilot, task });
    }
    announce(app);
    Ok(())
}

async fn run_github(app: AppHandle, id: u64) {
    let error = github_flow(&app, id).await.err();
    if !is_current(true, id) {
        return;
    }
    {
        let mut rt = runtime();
        rt.github_error = error;
        rt.github = None;
        rt.github_prompt = None;
    }
    announce(&app);
}

async fn github_flow(app: &AppHandle, id: u64) -> Result<(), String> {
    let state = app.state::<AppState>();
    let http = http_client(&state.settings());
    let prompt = github::start(&http).await.map_err(|f| f.message())?;
    if !is_current(true, id) {
        return Ok(());
    }
    runtime().github_prompt = Some(PromptInfo {
        user_code: prompt.user_code.clone(),
        verification_url: prompt.verification_url.clone(),
        prefilled: false,
    });
    announce(app);
    // The clipboard is the whole convenience here: GitHub will not pre-fill its field from a
    // link, deliberately, because that is the device-code phishing attack. A paste still leaves
    // the consent where it belongs.
    copy_to_clipboard(&prompt.user_code);
    open_url(&prompt.verification_url);

    let token = github::await_token(&http, &prompt).await.map_err(|f| f.message())?;
    if !is_current(true, id) {
        return Ok(());
    }
    state.secrets.set(COPILOT_KEY, Some(&token));
    if state.secrets.get(COPILOT_KEY).as_deref() != Some(token.as_str()) {
        return Err("Couldn't save the login on this PC.".into());
    }
    refresh_account(app, COPILOT_KEY.to_string());
    Ok(())
}

// ---- Windows: browser and clipboard -----------------------------------------------------

/// Opens an http(s) address in the default browser. Addresses come from provider replies, so
/// anything but a web address is refused rather than handed to the shell.
pub fn open_url(url: &str) {
    if !(url.starts_with("https://") || url.starts_with("http://")) || url.contains(['\r', '\n', '\0']) {
        return;
    }
    use windows::core::{w, PCWSTR};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let wide: Vec<u16> = url.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        ShellExecuteW(None, w!("open"), PCWSTR(wide.as_ptr()), None, None, SW_SHOWNORMAL);
    }
}

/// Puts text on the system clipboard (`CF_UNICODETEXT`).
pub fn copy_to_clipboard(text: &str) {
    use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
    use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    use windows::Win32::System::Ole::CF_UNICODETEXT;

    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        // Another process can hold the clipboard for a moment; a few short tries.
        let mut opened = false;
        for _ in 0..5 {
            if OpenClipboard(None).is_ok() {
                opened = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        if !opened {
            return;
        }
        let _ = EmptyClipboard();
        let bytes = wide.len() * std::mem::size_of::<u16>();
        if let Ok(memory) = GlobalAlloc(GMEM_MOVEABLE, bytes) {
            let target = GlobalLock(memory) as *mut u16;
            if target.is_null() {
                let _ = GlobalFree(memory);
            } else {
                std::ptr::copy_nonoverlapping(wide.as_ptr(), target, wide.len());
                let _ = GlobalUnlock(memory);
                // On success the system owns the memory.
                if SetClipboardData(CF_UNICODETEXT.0 as u32, HANDLE(memory.0)).is_err() {
                    let _ = GlobalFree(HGLOBAL(memory.0));
                }
            }
        }
        let _ = CloseClipboard();
    }
}
