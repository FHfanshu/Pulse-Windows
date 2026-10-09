//! Token spend over IPC: the Token spend pane, the detailed card's activity
//! section and prompt-cache countdowns. Reading transcripts is blocking work and
//! runs on the blocking pool. Nothing here reads anything while Token spend is
//! switched off (upstream: local records are read only with Token spend on).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use pulse_core::recap::periods;
use pulse_core::spend::{
    self, activity::TokenActivity, model_summary::ModelSpendSummary, summary::SpendSummary, Calendar, Ledger,
    LedgerDay, PromptCacheReading, SpendAgent,
};
use pulse_core::Provider;
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::state::AppState;

fn home() -> PathBuf {
    std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_default()
}

/// The last finished scan (upstream `AgentLedgers.kept`): ledgers, not transcripts. One snapshot,
/// replaced and never copied; dropped when Token spend is switched off or the Settings window goes
/// (`spend_release`).
struct Snapshot {
    ledgers: HashMap<SpendAgent, Ledger>,
    /// Sources that are present: installed, or used at least once.
    present: Vec<SpendAgent>,
    at: DateTime<Utc>,
}

static KEPT: Mutex<Option<Arc<Snapshot>>> = Mutex::new(None);

/// A kept scan younger than this is shown and not reread when the pane opens
/// (upstream `SpendWarmer.paneFreshness`).
const PANE_FRESHNESS_SECONDS: i64 = 2 * 60;

/// "Reading Codex…", 1/2: which agent the scan is on.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Progress {
    agent: SpendAgent,
    /// The product name, so the row can be drawn before the first overview has arrived.
    name: &'static str,
    index: usize,
    total: usize,
}

/// Reads every present agent's ledger and keeps the result. `app` is given for a visible scan,
/// which reports its progress; a quiet one (rereading behind figures already on screen) is not.
fn scan(app: Option<&AppHandle>) -> Arc<Snapshot> {
    let now = Utc::now();
    let present = spend::present_agents(&home());
    let mut ledgers = HashMap::new();
    for (index, agent) in present.iter().enumerate() {
        if let Some(app) = app {
            let _ = app.emit("spend-progress", Progress { agent: *agent, name: agent.display_name(), index, total: present.len() });
        }
        ledgers.insert(*agent, spend::read_agent_ledger(*agent, &home(), now));
    }
    let snapshot = Arc::new(Snapshot { ledgers, present, at: now });
    *KEPT.lock().unwrap() = Some(snapshot.clone());
    snapshot
}

fn kept() -> Option<Arc<Snapshot>> {
    KEPT.lock().unwrap().clone()
}

/// Lets the kept scan go: Token spend was switched off, or the Settings window closed.
#[tauri::command]
pub fn spend_release() {
    *KEPT.lock().unwrap() = None;
}

/// One transcript's row in the session list. The session's own quarter-hours stay behind: the
/// pane draws none of them, and they are most of a session's size.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionOut {
    pub agent: SpendAgent,
    pub title: Option<String>,
    pub is_review: bool,
    /// The project as the list shows it (it can carry a parent folder where names collide).
    pub project: Option<String>,
    pub end: DateTime<Utc>,
    pub tokens: i64,
    pub cost: f64,
    pub unpriced_tokens: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectOut {
    pub name: String,
    pub tokens: i64,
    pub cost: f64,
    pub unpriced_tokens: i64,
    pub sessions: usize,
    pub last_used: DateTime<Utc>,
}

/// The project and session lists of the summary on screen: newest sessions first, heaviest
/// projects first.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Lists {
    pub projects: Vec<ProjectOut>,
    pub sessions: Vec<SessionOut>,
}

impl Lists {
    fn of(summary: &SpendSummary) -> Self {
        Self {
            projects: summary
                .projects
                .iter()
                .map(|p| ProjectOut {
                    name: p.name.clone(),
                    tokens: p.tokens,
                    cost: p.cost,
                    unpriced_tokens: p.unpriced_tokens,
                    sessions: p.sessions,
                    last_used: p.last_used,
                })
                .collect(),
            sessions: summary
                .sessions
                .iter()
                .map(|row| SessionOut {
                    agent: row.agent,
                    title: row.session.title.clone(),
                    is_review: row.session.is_review,
                    project: summary.project_name(row),
                    end: row.session.end,
                    tokens: row.session.tokens,
                    cost: row.session.cost,
                    unpriced_tokens: row.session.unpriced_tokens,
                })
                .collect(),
        }
    }
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
    /// The project and session lists of the summary on screen (the agent's when one is focused);
    /// absent while a model is, which draws neither.
    pub lists: Option<Lists>,
    /// Present sources that produced no records at all, named together at the foot of the combined
    /// page so a silent source is not mistaken for a zero reading.
    pub no_records: Vec<SpendAgent>,
    /// Whether any present source held compressed history a reader could not decode.
    pub has_read_limitations: bool,
    /// Every agent the pane can name (id, product name, mark), so a new source needs no change in
    /// the UI to be listed, named and drawn.
    pub agents: Vec<AgentInfo>,
    /// The periods the two recap buttons open, as keys ("2026-09", "2026"): the ones the recap
    /// window would open on by itself (`RecapPeriods.defaultMonth` / `defaultYear`).
    pub recap: RecapKeys,
}

/// One agent as the UI names and draws it.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInfo {
    pub id: SpendAgent,
    pub name: &'static str,
    /// The file stem of its mark in `assets/icons`, where the set has one.
    pub icon: Option<&'static str>,
}

impl AgentInfo {
    fn all() -> Vec<AgentInfo> {
        SpendAgent::ALL.iter().map(|a| AgentInfo { id: *a, name: a.display_name(), icon: a.icon_resource() }).collect()
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecapKeys {
    pub month: String,
    pub year: String,
}

/// The Token spend pane: combined and per-agent figures over the last `over_last` days (None = all).
/// `agent` and `model` ask for the drill-downs. Everything is counted from the same ledgers with
/// one `now` and one calendar, so a detail always adds up to the row it was opened from.
///
/// The scan is kept between calls: a sidebar visit or a new span reuses it, and only `rescan` (or
/// a scan older than two minutes, which is read again without the progress row) reads again.
#[tauri::command]
pub async fn spend_overview(
    app: AppHandle,
    state: State<'_, AppState>,
    over_last: Option<usize>,
    agent: Option<SpendAgent>,
    model: Option<String>,
    rescan: bool,
) -> Result<Option<SpendOverview>, String> {
    if !state.settings().reads_token_spend {
        spend_release();
        return Ok(None);
    }
    tauri::async_runtime::spawn_blocking(move || {
        let now = Utc::now();
        let snapshot = match kept() {
            Some(k) if !rescan && (now - k.at).num_seconds() < PANE_FRESHNESS_SECONDS => k,
            Some(_) if !rescan => scan(None),
            _ => scan(Some(&app)),
        };
        let calendar = Calendar::local();
        let mut summary = SpendSummary::of(&snapshot.ledgers, over_last, now, &calendar);
        let narrowed: HashMap<SpendAgent, Ledger> = match agent {
            Some(a) => snapshot.ledgers.iter().filter(|(k, _)| **k == a).map(|(k, v)| (*k, v.clone())).collect(),
            None => HashMap::new(),
        };
        let scope = if agent.is_some() { &narrowed } else { &snapshot.ledgers };
        let mut focused = agent.map(|_| SpendSummary::of(scope, over_last, now, &calendar));

        // The lists of the summary on screen only, and none under a model.
        let lists = match (&model, &focused) {
            (Some(_), _) => None,
            (None, Some(f)) => Some(Lists::of(f)),
            (None, None) => Some(Lists::of(&summary)),
        };
        // Every session carries its quarter-hours, and the pane draws them nowhere.
        for s in std::iter::once(&mut summary).chain(focused.as_mut()) {
            s.sessions = Vec::new();
            s.projects = Vec::new();
        }
        let no_records = snapshot
            .present
            .iter()
            .copied()
            .filter(|a| snapshot.ledgers.get(a).is_none_or(|l| l.all_time().tokens == 0))
            .collect();
        Some(SpendOverview {
            summary,
            activity: TokenActivity::of(scope, now, &calendar),
            agent: focused,
            model: model.map(|name| ModelSpendSummary::of(scope, &name, over_last, now, &calendar)),
            lists,
            no_records,
            has_read_limitations: snapshot.ledgers.values().any(|l| l.has_read_limitations),
            agents: AgentInfo::all(),
            recap: {
                let earliest = periods::earliest(&snapshot.ledgers, &calendar);
                let offer = periods::Offer::of(earliest, chrono::Local::now().date_naive());
                RecapKeys { month: offer.default_month, year: offer.default_year }
            },
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
    if !state.settings().reads_token_spend || !spend::supports_card(provider) {
        return Ok(None);
    }
    tauri::async_runtime::spawn_blocking(move || {
        let now = Utc::now();
        let ledger = spend::read_card_ledger(provider, &home(), now)?;
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

/// The providers whose detailed card has spend to show: the two with transcripts and every one
/// an agent borrows the name of. No IO.
#[tauri::command]
pub fn spend_card_providers() -> Vec<String> {
    Provider::ALL.iter().copied().filter(|p| spend::supports_card(*p)).map(|p| p.raw().to_string()).collect()
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
