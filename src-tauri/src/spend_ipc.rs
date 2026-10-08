//! Token spend over IPC: the Token spend pane, the detailed card's activity
//! section and prompt-cache countdowns. Reading transcripts is blocking work and
//! runs on the blocking pool. Nothing here reads anything while Token spend is
//! switched off (upstream: local records are read only with Token spend on).

use std::collections::HashMap;
use std::path::PathBuf;

use chrono::Utc;
use pulse_core::spend::{
    self, activity::TokenActivity, model_summary::ModelSpendSummary, summary::SpendSummary, Calendar, Ledger, LedgerDay, PromptCacheReading,
    SpendAgent,
};
use pulse_core::Provider;
use serde::Serialize;
use tauri::State;

use crate::state::AppState;

fn home() -> PathBuf {
    std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_default()
}

fn read_all() -> HashMap<SpendAgent, Ledger> {
    let now = Utc::now();
    SpendAgent::ALL
        .iter()
        .filter_map(|agent| spend::read_ledger(agent.provider(), &home(), now).ok().map(|l| (*agent, l)))
        .collect()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpendOverview {
    /// Every agent added up.
    pub summary: SpendSummary,
    /// The year, over the focused agent when there is one, else over all of them.
    pub activity: TokenActivity,
    /// The same span for the focused agent alone; present only when `agent` was asked for.
    pub agent: Option<SpendSummary>,
    /// One model's figures, narrowed to the focused agent first; present only when `model` was asked for.
    pub model: Option<ModelSpendSummary>,
}

/// The Token spend pane: combined and per-agent figures over the last `over_last` days (None = all).
/// `agent` and `model` ask for the drill-downs. Everything is counted from the same ledgers with
/// one `now` and one calendar, so a detail always adds up to the row it was opened from.
#[tauri::command]
pub async fn spend_overview(
    state: State<'_, AppState>,
    over_last: Option<usize>,
    agent: Option<SpendAgent>,
    model: Option<String>,
) -> Result<Option<SpendOverview>, String> {
    if !state.settings().reads_token_spend {
        return Ok(None);
    }
    tauri::async_runtime::spawn_blocking(move || {
        let ledgers = read_all();
        let now = Utc::now();
        let calendar = Calendar::local();
        let mut summary = SpendSummary::of(&ledgers, over_last, now, &calendar);
        let narrowed: HashMap<SpendAgent, Ledger> = match agent {
            Some(a) => ledgers.into_iter().filter(|(k, _)| *k == a).collect(),
            None => ledgers,
        };
        let mut focused = agent.map(|_| SpendSummary::of(&narrowed, over_last, now, &calendar));
        // The pane draws neither the session list nor the project list, and every session carries
        // its quarter-hours: leave both out of what crosses the IPC.
        for s in std::iter::once(&mut summary).chain(focused.as_mut()) {
            s.sessions = Vec::new();
            s.projects = Vec::new();
        }
        Some(SpendOverview {
            summary,
            activity: TokenActivity::of(&narrowed, now, &calendar),
            agent: focused,
            model: model.map(|name| ModelSpendSummary::of(&narrowed, &name, over_last, now, &calendar)),
        })
    })
    .await
    .map_err(|e| e.to_string())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Figure {
    pub tokens: i64,
    pub cost: f64,
}

/// What the detailed card's activity section draws for one provider.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CardSpend {
    pub today: Figure,
    pub week: Figure,
    pub month: Figure,
    /// 31 days, oldest first, today last.
    pub days: Vec<LedgerDay>,
    pub top_model: Option<(String, f64)>,
    pub cache_hit_rate: Option<f64>,
    pub currency: Option<String>,
}

const SPAN: usize = 31;

#[tauri::command]
pub async fn card_spend(state: State<'_, AppState>, provider: String) -> Result<Option<CardSpend>, String> {
    let Some(provider) = Provider::from_raw(&provider) else { return Ok(None) };
    if !state.settings().reads_token_spend || !spend::supports(provider) {
        return Ok(None);
    }
    tauri::async_runtime::spawn_blocking(move || {
        let now = Utc::now();
        let ledger = spend::read_ledger(provider, &home(), now).ok()?;
        let figure = |t: spend::ledger::SpanTotal| Figure { tokens: t.tokens, cost: t.cost };
        Some(CardSpend {
            today: ledger.today(now).map(|d| Figure { tokens: d.tokens, cost: d.cost }).unwrap_or(Figure { tokens: 0, cost: 0.0 }),
            week: figure(ledger.total(7, now)),
            month: figure(ledger.total(SPAN, now)),
            days: ledger.recent(SPAN, now),
            top_model: ledger.top_model(SPAN, now),
            cache_hit_rate: ledger.cache_hit_rate(SPAN, now),
            currency: ledger.currency.clone(),
        })
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn prompt_cache(state: State<'_, AppState>, provider: String) -> Result<Option<PromptCacheReading>, String> {
    let Some(provider) = Provider::from_raw(&provider) else { return Ok(None) };
    if !state.settings().reads_token_spend {
        return Ok(None);
    }
    tauri::async_runtime::spawn_blocking(move || Some(spend::prompt_cache(provider, &home(), Utc::now())))
        .await
        .map_err(|e| e.to_string())
}

/// Keep the price table current (models.dev, refetched when older than a day).
pub fn start_price_refresh() {
    tauri::async_runtime::spawn(async {
        loop {
            let _ = spend::refresh_prices().await;
            tokio::time::sleep(std::time::Duration::from_secs(6 * 3600)).await;
        }
    });
}
