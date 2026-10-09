//! Glue for the account pane: what a rate-limit window is worth in money (upstream
//! `EstimatedValueGroup`), which browsers this PC has (the "Read from browser" picker) and
//! opening a help page.

use std::path::PathBuf;

use chrono::Utc;
use pulse_core::spend::{self, budget};
use serde::Serialize;
use tauri::State;

use crate::state::AppState;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowEstimate {
    /// `UsageWindow.id` the figure belongs to.
    pub window: String,
    /// What this PC spent since the window opened.
    pub spent: f64,
    /// What the whole window is worth.
    pub full: f64,
    /// Seen spent where this PC's logs cannot see (upstream `UsageStore.usedElsewhere`): no figure
    /// (`spent` and `full` are zero), and the pane says why.
    pub elsewhere: bool,
}

fn home() -> PathBuf {
    std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_default()
}

/// The estimated value of each of an account's windows, from the local transcripts. Empty while
/// Token spend is off, for a provider that keeps no transcripts, and for an added account (its
/// transcripts belong to whichever login the CLI holds, not to it).
///
/// `card` is the detailed card's asking: it leaves out a provider that states its limits in money
/// (`budget::estimates_value`) and the windows seen spent elsewhere, which only the settings pane lists.
#[tauri::command]
pub async fn estimated_value(state: State<'_, AppState>, account: String, card: Option<bool>) -> Result<Vec<WindowEstimate>, String> {
    let card = card.unwrap_or(false);
    if !state.settings().reads_token_spend {
        return Ok(Vec::new());
    }
    let Some(usage) = state.snapshot().into_iter().find(|u| u.account.id() == account) else {
        return Ok(Vec::new());
    };
    let provider = usage.account.provider;
    if !usage.account.is_primary() || !spend::supports(provider) || (card && !budget::estimates_value(provider)) {
        return Ok(Vec::new());
    }
    let elsewhere: Vec<bool> = usage.windows.iter().map(|w| state.store.used_elsewhere(w, &usage.account)).collect();
    tauri::async_runtime::spawn_blocking(move || {
        let now = Utc::now();
        let ledger = spend::read_ledger(provider, &home(), now).ok()?;
        Some(
            usage
                .windows
                .iter()
                .zip(elsewhere)
                .filter_map(|(w, elsewhere)| {
                    // A window seen spent off this PC keeps its row, saying why there is no figure: a
                    // value that quietly vanished would read as a bug.
                    if elsewhere {
                        return (!card).then(|| WindowEstimate { window: w.id.clone(), spent: 0.0, full: 0.0, elsewhere });
                    }
                    budget::estimate(w, &ledger, usage.observed_at, now)
                        .map(|e| WindowEstimate { window: w.id.clone(), spent: e.spent, full: e.full, elsewhere })
                })
                .collect::<Vec<_>>(),
        )
    })
    .await
    .map(Option::unwrap_or_default)
    .map_err(|e| e.to_string())
}

/// The browsers installed on this PC, as the ids the `sessionBrowsers` setting stores.
#[tauri::command]
pub fn installed_browsers() -> Vec<&'static str> {
    let env = |name: &str| std::env::var_os(name).map(PathBuf::from).unwrap_or_default();
    let (local, program_files, program_files_x86) = (env("LOCALAPPDATA"), env("ProgramFiles"), env("ProgramFiles(x86)"));
    let candidates: [(&'static str, &str); 5] = [
        ("firefox", "Mozilla Firefox/firefox.exe"),
        ("chrome", "Google/Chrome/Application/chrome.exe"),
        ("edge", "Microsoft/Edge/Application/msedge.exe"),
        ("brave", "BraveSoftware/Brave-Browser/Application/brave.exe"),
        ("vivaldi", "Vivaldi/Application/vivaldi.exe"),
    ];
    candidates
        .into_iter()
        .filter(|(_, path)| {
            [&local, &program_files, &program_files_x86].iter().any(|root| !root.as_os_str().is_empty() && root.join(path).exists())
        })
        .map(|(id, _)| id)
        .collect()
}

/// Opens an https page in the default browser. Anything else is refused.
#[tauri::command]
pub fn open_external(url: String) -> Result<(), String> {
    if !url.starts_with("https://") || url.contains(char::is_whitespace) {
        return Err("only https links can be opened".into());
    }
    std::process::Command::new("rundll32")
        .args(["url.dll,FileProtocolHandler", &url])
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}
