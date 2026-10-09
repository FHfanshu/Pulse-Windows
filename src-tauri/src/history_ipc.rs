//! The account pane's "Usage history" over IPC (upstream `AccountHistoryModel`).
//!
//! Reading transcripts is blocking work and runs on the blocking pool. Upstream's pane reads
//! whenever it is opened, whatever the Token spend setting says (that switch is about the panel's
//! and the Token spend pane's own reading), so this does too.

use std::path::PathBuf;

use chrono::Utc;
use pulse_core::history::{self, AccountHistory};
use pulse_core::AccountKey;
use tauri::State;

use crate::state::AppState;

fn home() -> PathBuf {
    std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_default()
}

/// What the account's history card draws, for the account `id` ("codex", or "codex#2" for an added
/// one, which is never asked: transcripts do not say which account was signed in at the time).
/// `read` is `unsupported` for a provider whose history this build does not read, and the card is
/// then left out.
#[tauri::command]
pub async fn account_history(state: State<'_, AppState>, id: String) -> Result<AccountHistory, String> {
    let account = AccountKey::from_id(&id).ok_or_else(|| format!("not an account: {id}"))?;
    let enabled = state.settings().enabled_accounts.contains(&id);
    tauri::async_runtime::spawn_blocking(move || {
        history::read(account.provider, enabled, account.is_primary(), &home(), Utc::now())
    })
    .await
    .map_err(|e| e.to_string())
}
