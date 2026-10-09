//! Token spend: what the coding agents have spent on this machine.
//!
//! Ported from upstream's `Usage/` ledger family. Every agent is a [`sources::SpendSource`]: one
//! file in `sources/` and one line in its registry (`docs/spend-source-brief.md`).
//!
//! Claude Code's transcripts (`%USERPROFILE%\.claude\projects\**\*.jsonl`, or
//! `CLAUDE_CONFIG_DIR`) and Codex's rollouts (`%USERPROFILE%\.codex\sessions`, or `CODEX_HOME`)
//! are streamed line by line, only the lines that can carry usage are parsed, and the per-file
//! result is cached under [`crate::paths::data_dir`] (`ledger-9-<provider>.json`) keyed on size
//! and modification time, so a second read costs a directory listing. The other agents
//! (OpenCode, Kilo CLI, Grok Build, Kimi CLI, Devin) hand over normalised records, which the
//! shared [`record::build_ledger`] prices exactly as the transcripts are, and the finished
//! ledger is kept (`agent-<n>-<agent>.json`, see [`agent_cache`]) while their stores are unchanged.
//!
//! Money is a translation, not a bill: the tokens priced at models.dev's published API rates
//! ([`prices`]). The price table is read from its on-disk cache by [`read_ledger`]; call
//! [`refresh_prices`] now and then (it refetches at most once a day) to keep it current.

pub mod activity;
pub mod agent;
pub mod agent_cache;
pub mod budget;
pub mod burn;
pub mod calendar;
pub mod elsewhere;
pub mod dashboard;
pub mod ledger;
pub mod logio;
pub mod loglines;
pub mod model_summary;
pub mod prices;
pub mod project;
pub mod prompt_cache;
pub mod record;
pub mod sources;
pub mod sqlite;
pub mod summary;
pub mod tally;
pub mod titles;
pub mod transcripts;

#[cfg(test)]
mod tests_ledger;
#[cfg(test)]
mod tests_model;
#[cfg(test)]
mod tests_quality;
#[cfg(test)]
mod tests_record;
#[cfg(test)]
mod tests_stream;
#[cfg(test)]
mod tests_summary;

use std::path::Path;

use chrono::{DateTime, Utc};

pub use agent::SpendAgent;
pub use calendar::Calendar;
pub use ledger::{Ledger, LedgerDay, Origin, Session, Slot};
pub use prices::{ModelPrice, ModelPrices, PriceTable};
pub use prompt_cache::{PromptCacheLapse, PromptCacheReading, PromptCacheSession};
pub use tally::{ReplyTiming, TokenCost, TokenTally};

use crate::provider::Provider;
use transcripts::Sources;

#[derive(Debug, thiserror::Error)]
pub enum SpendError {
    /// Only Claude Code and Codex are read as transcripts by provider.
    #[error("{0:?} leaves no transcripts Pulse reads")]
    Unsupported(Provider),
}

/// Whether the provider's own transcripts are read here (Claude Code and Codex): the ones with a
/// per-provider history.
pub fn supports(provider: Provider) -> bool {
    SpendAgent::from_provider(provider).is_some()
}

/// Whether the provider's detailed card has spend to show: its transcripts, or the records of an
/// agent that borrows its name (OpenCode on OpenCode Go, Kimi CLI on Kimi Code, Grok Build on
/// Grok).
pub fn supports_card(provider: Provider) -> bool {
    supports(provider) || !SpendAgent::for_card(provider).is_empty()
}

/// The provider's ledger, read from the transcripts under `home` (honouring `CLAUDE_CONFIG_DIR`
/// and `CODEX_HOME`), priced with the table on disk, days cut in the local time zone. Unchanged
/// transcripts are served from the per-file cache. An absent folder is an empty ledger.
///
/// Blocking: call it from `spawn_blocking`.
pub fn read_ledger(provider: Provider, home: &Path, now: DateTime<Utc>) -> Result<Ledger, SpendError> {
    let agent = SpendAgent::from_provider(provider).ok_or(SpendError::Unsupported(provider))?;
    Ok(read_agent_ledger(agent, home, now))
}

/// Any agent's ledger, the way [`read_ledger`] reads a provider's. Blocking.
pub fn read_agent_ledger(agent: SpendAgent, home: &Path, now: DateTime<Utc>) -> Ledger {
    sources::read_agent_now(agent, home, now)
}

/// The agents with something on this PC to read: installed, or at least used once.
pub fn present_agents(home: &Path) -> Vec<SpendAgent> {
    let sources = Sources::from_env(home);
    SpendAgent::ALL.iter().copied().filter(|agent| agent.is_present(&sources)).collect()
}

/// Every present agent's ledger, as the recap and the notices count them. Blocking.
pub fn read_present_ledgers(home: &Path, now: DateTime<Utc>) -> std::collections::HashMap<SpendAgent, Ledger> {
    present_agents(home).into_iter().map(|agent| (agent, read_agent_ledger(agent, home, now))).collect()
}

/// What the provider's detailed card draws from: its transcripts' ledger, or the agents that
/// borrow its name added up day by day (upstream `CardHistorySource`). None when nothing on this
/// PC feeds the card.
///
/// Blocking: call it from `spawn_blocking`.
pub fn read_card_ledger(provider: Provider, home: &Path, now: DateTime<Utc>) -> Option<Ledger> {
    let dir = crate::paths::data_dir();
    let prices = ModelPrices::cached(&dir);
    read_card_ledger_with(provider, &Sources::from_env(home), &dir, &Calendar::local(), &prices, now)
}

/// [`read_card_ledger`] with every input explicit.
pub fn read_card_ledger_with(
    provider: Provider,
    sources: &Sources,
    cache_directory: &Path,
    calendar: &Calendar,
    prices: &PriceTable,
    now: DateTime<Utc>,
) -> Option<Ledger> {
    if supports(provider) {
        return read_ledger_with(provider, sources, cache_directory, calendar, prices, now).ok();
    }
    let agents = SpendAgent::for_card(provider);
    if agents.is_empty() {
        return None;
    }
    let ledgers: Vec<Ledger> = agents.iter().map(|agent| sources::read_agent(*agent, sources, cache_directory, calendar, prices)).collect();
    let mut combined = Ledger::adding(&ledgers, calendar);
    combined.reports_cache_reads = agents.iter().any(|a| a.reports_cache_reads());
    combined.read_at = Some(now);
    Some(combined)
}

/// [`read_ledger`] with every input explicit: transcript locations, cache folder, calendar and
/// price table.
pub fn read_ledger_with(
    provider: Provider,
    sources: &Sources,
    cache_directory: &Path,
    calendar: &Calendar,
    prices: &PriceTable,
    now: DateTime<Utc>,
) -> Result<Ledger, SpendError> {
    let agent = SpendAgent::from_provider(provider).ok_or(SpendError::Unsupported(provider))?;
    let mut ledger = sources::read_agent(agent, sources, cache_directory, calendar, prices);
    ledger.read_at = Some(now);
    Ok(ledger)
}

/// When the prompt cache behind each of the provider's conversations lapses. Empty for a provider
/// whose log does not allow it (only Claude Code and Codex do).
pub fn prompt_cache(provider: Provider, home: &Path, now: DateTime<Utc>) -> PromptCacheReading {
    prompt_cache::read_for(provider, &Sources::from_env(home), now)
}

/// The price table, refetched from models.dev when the saved one is older than a day.
pub async fn refresh_prices() -> PriceTable {
    ModelPrices::shared().prices().await
}
