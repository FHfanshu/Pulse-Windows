// Ported from upstream Settings/CodexSignalsGroup.swift (the reading half).
//! "Signs of a weaker model" over IPC: reads this PC's Codex sessions
//! (`%USERPROFILE%\.codex\sessions`, or under `CODEX_HOME`) and reports what
//! `pulse_core::codex_signals` finds in them. Nothing leaves the PC.
//!
//! Reading is blocking and can take seconds the first time (a few hundred megabytes of sessions);
//! each file's facts are kept in memory against its size and modification time, so asking again or
//! widening the span reads only what changed. A newer request cancels the older one between lines.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

use chrono::{Duration, Utc};
use pulse_core::codex_signals::{Change, CodexSignalReader, Truncation};
use pulse_core::spend::transcripts::Sources;
use pulse_core::Provider;
use serde::Serialize;

/// One model's row with the verdicts worked out, so the UI does no arithmetic of its own.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TruncationView {
    pub model: String,
    pub responses: i64,
    pub reached_lattice: i64,
    pub on_lattice: i64,
    pub share: Option<f64>,
    pub is_measurable: bool,
    pub is_suspicious: bool,
}

impl From<Truncation> for TruncationView {
    fn from(t: Truncation) -> Self {
        Self {
            share: t.share(),
            is_measurable: t.is_measurable(),
            is_suspicious: t.is_suspicious(),
            model: t.model,
            responses: t.responses,
            reached_lattice: t.reached_lattice,
            on_lattice: t.on_lattice,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignalsView {
    pub truncation: Vec<TruncationView>,
    pub changes: Vec<Change>,
    pub sessions: i64,
    pub judged_sessions: i64,
}

fn reader() -> &'static CodexSignalReader {
    static READER: OnceLock<CodexSignalReader> = OnceLock::new();
    READER.get_or_init(|| {
        let home = std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_default();
        CodexSignalReader::new(Sources::from_env(home).roots(Provider::Codex))
    })
}

static GENERATION: AtomicU64 = AtomicU64::new(0);

/// The signs over the last `days` days (everything when null). Null comes back when a newer
/// request replaced this one, so the caller keeps waiting for that one's answer.
#[tauri::command]
pub async fn codex_signals(days: Option<u32>) -> Result<Option<SignalsView>, String> {
    let mine = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let since = days.map(|d| Utc::now() - Duration::days(i64::from(d)));
    let read = tauri::async_runtime::spawn_blocking(move || {
        reader().read(since, &|| GENERATION.load(Ordering::SeqCst) != mine)
    })
    .await
    .map_err(|e| e.to_string())?;
    Ok(read.map(|signals| SignalsView {
        truncation: signals.truncation.into_iter().map(TruncationView::from).collect(),
        changes: signals.changes,
        sessions: signals.sessions,
        judged_sessions: signals.judged_sessions,
    }))
}
