//! The monthly / yearly recap over IPC: the report for a period, and the window that shows it.
//!
//! Like the rest of Token spend, nothing is read while reading is switched off. The ledgers are
//! kept for half an hour (or until the next day, or until the window is shown again) so moving
//! between periods does not scan the transcripts each time; they are dropped when the recap
//! window closes.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{NaiveDate, Utc};
use pulse_core::recap::periods::{self, Offer};
use pulse_core::recap::{self, Period, Report};
use pulse_core::spend::{self, Calendar, Ledger, ModelPrices, SpendAgent};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};

use crate::state::AppState;

pub const LABEL: &str = "recap";

/// How long a read of the ledgers is trusted: a running month's "to date" must not stop where the
/// first read did.
const FRESH_FOR: Duration = Duration::from_secs(30 * 60);

struct Kept {
    read_at: Instant,
    day: NaiveDate,
    ledgers: Arc<HashMap<SpendAgent, Ledger>>,
}

static KEPT: Mutex<Option<Kept>> = Mutex::new(None);

fn home() -> PathBuf {
    std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_default()
}

fn forget() {
    if let Ok(mut kept) = KEPT.lock() {
        *kept = None;
    }
}

/// The ledgers of every agent, read now or kept from a read that is still fresh.
fn ledgers(calendar: &Calendar) -> Arc<HashMap<SpendAgent, Ledger>> {
    let now = Utc::now();
    let today = calendar.date(now);
    if let Some(kept) = KEPT.lock().ok().and_then(|k| k.as_ref().map(|k| (k.read_at, k.day, k.ledgers.clone()))) {
        if kept.0.elapsed() < FRESH_FOR && kept.1 == today {
            return kept.2;
        }
    }
    let read: HashMap<SpendAgent, Ledger> = SpendAgent::ALL
        .iter()
        .filter_map(|agent| spend::read_ledger(agent.provider(), &home(), now).ok().map(|l| (*agent, l)))
        .collect();
    let read = Arc::new(read);
    if let Ok(mut kept) = KEPT.lock() {
        *kept = Some(Kept { read_at: Instant::now(), day: today, ledgers: read.clone() });
    }
    read
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecapPayload {
    /// Which periods to offer, and the ones to open on.
    pub offer: Offer,
    /// The report for the asked-for period (or the default month).
    pub report: Report,
}

/// One period's recap with the cards' facts. `None` while Token spend reading is off. `period` is
/// "2026-09" or "2026"; absent, the month the window opens on.
#[tauri::command]
pub async fn recap_report(state: State<'_, AppState>, period: Option<String>) -> Result<Option<RecapPayload>, String> {
    if !state.settings().reads_token_spend {
        return Ok(None);
    }
    let asked = match period.as_deref() {
        Some(key) => Some(Period::from_key(key).ok_or_else(|| format!("not a recap period: {key}"))?),
        None => None,
    };
    tauri::async_runtime::spawn_blocking(move || {
        let calendar = Calendar::local();
        let now = Utc::now();
        let ledgers = ledgers(&calendar);
        let today = calendar.date(now);
        let earliest = periods::earliest(&ledgers, &calendar);
        let offer = Offer::of(earliest, today);
        let period = asked.unwrap_or_else(|| periods::default_month(earliest, today));
        let prices = ModelPrices::cached(&pulse_core::paths::data_dir());
        let built = recap::build(period, &ledgers, &prices, now, &calendar).ok_or_else(|| "not a recap period".to_string())?;
        Ok(Some(RecapPayload { offer, report: Report::of(built) }))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Open the recap window, on `period` ("2026-09" or "2026") when given, else on the default
/// month. A window already open is brought forward and told to move.
///
/// Async on purpose: building a webview window inside a synchronous command blocks the main thread
/// WebView2 needs on Windows, and the new window stays white.
#[tauri::command]
pub async fn open_recap(app: AppHandle, period: Option<String>) {
    show_recap(&app, period);
}

pub fn show_recap(app: &AppHandle, period: Option<String>) {
    let app = app.clone();
    // Shown again means read again.
    forget();
    // "month" / "year" ask for the default period of that kind (upstream default_month / default_year).
    let today = chrono::Local::now().date_naive();
    let period = period
        .map(|key| match key.as_str() {
            "month" => periods::default_month(None, today).key(),
            "year" => periods::default_year(None, today).key(),
            _ => key,
        })
        .filter(|key| Period::from_key(key).is_some());
    if let Some(window) = app.get_webview_window(LABEL) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        let _ = app.emit_to(LABEL, "recap-open", period);
        return;
    }
    let path = match &period {
        Some(key) => format!("recap.html?period={key}"),
        None => "recap.html".to_string(),
    };
    let built = WebviewWindowBuilder::new(&app, LABEL, WebviewUrl::App(path.into()))
        .title("Pulse Recap")
        .inner_size(1040.0, 720.0)
        .min_inner_size(860.0, 560.0)
        .center()
        .build();
    if let Err(e) = &built {
        eprintln!("recap window: {e}");
    }
    if let Ok(window) = built {
        window.on_window_event(|event| {
            if matches!(event, tauri::WindowEvent::Destroyed) {
                forget();
            }
        });
    }
}
