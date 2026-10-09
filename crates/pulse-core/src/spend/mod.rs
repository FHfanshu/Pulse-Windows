//! Token spend: what the coding agents have spent on this machine.
//!
//! Ported from upstream's `Usage/` ledger family. Claude Code's transcripts
//! (`%USERPROFILE%\.claude\projects\**\*.jsonl`, or `CLAUDE_CONFIG_DIR`) and Codex's rollouts
//! (`%USERPROFILE%\.codex\sessions`, or `CODEX_HOME`) are streamed line by line, only the lines
//! that can carry usage are parsed, and the per-file result is cached under
//! [`crate::paths::data_dir`] (`ledger-9-<provider>.json`) keyed on size and modification time,
//! so a second read costs a directory listing.
//!
//! Money is a translation, not a bill: the tokens priced at models.dev's published API rates
//! ([`prices`]). The price table is read from its on-disk cache by [`read_ledger`]; call
//! [`refresh_prices`] now and then (it refetches at most once a day) to keep it current.

pub mod activity;
pub mod agent;
pub mod budget;
pub mod burn;
pub mod calendar;
pub mod elsewhere;
pub mod ledger;
pub mod logio;
pub mod loglines;
pub mod model_summary;
pub mod prices;
pub mod project;
pub mod prompt_cache;
pub mod record;
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
use transcripts::{Sources, TranscriptReader};

#[derive(Debug, thiserror::Error)]
pub enum SpendError {
    /// Only Claude Code and Codex leave transcripts this module reads.
    #[error("{0:?} leaves no transcripts Pulse reads")]
    Unsupported(Provider),
}

/// Whether the provider's spend can be read here.
pub fn supports(provider: Provider) -> bool {
    SpendAgent::from_provider(provider).is_some()
}

/// The provider's ledger, read from the transcripts under `home` (honouring `CLAUDE_CONFIG_DIR`
/// and `CODEX_HOME`), priced with the table on disk, days cut in the local time zone. Unchanged
/// transcripts are served from the per-file cache. An absent folder is an empty ledger.
///
/// Blocking: call it from `spawn_blocking`.
pub fn read_ledger(provider: Provider, home: &Path, now: DateTime<Utc>) -> Result<Ledger, SpendError> {
    let dir = crate::paths::data_dir();
    let prices = ModelPrices::cached(&dir);
    read_ledger_with(provider, &Sources::from_env(home), &dir, &Calendar::local(), &prices, now)
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
    if !supports(provider) {
        return Err(SpendError::Unsupported(provider));
    }
    let reader = TranscriptReader::new(sources.clone(), cache_directory.to_path_buf(), calendar.clone());
    let mut ledger = reader.ledger(provider, prices);
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
