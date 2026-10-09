//! The token spend sources: one file each, one line in [`registry!`].
//!
//! A source is an agent that leaves a record of its work on this PC. Its file says what it is
//! (id, name, mark, which provider's card it feeds), where its records are (`inputs`) and how to
//! read them (`records`); everything else (the whole-ledger cache, pricing, sessions, the
//! pane, the recap, the card) is shared. `docs/spend-source-brief.md` is the how-to.
//!
//! Claude Code and Codex are sources too, but they stream their transcripts through the
//! per-file cache in [`super::transcripts`] instead of handing over records; they say so with
//! [`SpendSource::transcript_provider`].

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use super::agent_cache::{self, Stamp};
use super::calendar::Calendar;
use super::ledger::{Ledger, Origin};
use super::prices::PriceTable;
use super::record::{build_ledger, AgentUsageRecord};
use super::transcripts::{Sources, TranscriptReader};
use crate::provider::Provider;

/// What the rest of Pulse needs to know about one agent, and how to read it.
pub trait SpendSource: Sync {
    /// The persisted id: the Swift case name upstream (`openCode`), the serialized form of
    /// [`SpendAgent`](super::SpendAgent) and part of the cache file name. Never change it.
    fn raw(&self) -> &'static str;

    /// The canonical id of upstream's catalogue (`opencode`).
    fn source_id(&self) -> &'static str;

    /// Product name, left untranslated.
    fn display_name(&self) -> &'static str;

    /// The provider whose detailed card shows this agent's spend, where Pulse has that provider.
    /// Only a provider that already exists is ever named: an agent gets no ring by appearing here.
    fn card_provider(&self) -> Option<Provider> {
        None
    }

    /// The file stem of the agent's mark in `assets/icons`, where the set has one.
    fn icon(&self) -> Option<&'static str> {
        None
    }

    /// The models.dev plan vendor whose rates apply when no first-party provider prices a model.
    /// None for an agent that calls the model vendors directly.
    fn price_vendor(&self) -> Option<&'static str> {
        None
    }

    /// Environment variables `inputs` reads through [`Sources::var`]. They are collected into
    /// [`Sources::from_env`]; a variable not named here is never seen.
    fn env_vars(&self) -> &'static [&'static str] {
        &[]
    }

    /// Every file or folder this agent is read from, whether or not it exists. A missing root is
    /// named so the cache can watch it: a store that appears later invalidates exactly this
    /// agent. Windows locations are resolved here (see `docs/spend-source-brief.md`).
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf>;

    /// The increments of work in the roots that exist now. Never invents a count; a reader that
    /// cannot decode something contributes no record for it.
    fn records(&self, _roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        Vec::new()
    }

    /// Whether the store records the prompt cache at all. False for a store with no cache column.
    fn reports_cache_reads(&self) -> bool {
        true
    }

    /// Whether the store is an export or capture rather than a native log.
    fn requires_usage_export(&self) -> bool {
        false
    }

    /// Read off a real machine with real work in it upstream.
    fn has_captured_validation(&self) -> bool {
        false
    }

    /// Set only by Claude Code and Codex: the provider whose transcripts are read line by line
    /// with the per-file cache instead of through `records`.
    fn transcript_provider(&self) -> Option<Provider> {
        None
    }
}

/// The registry. One line per source: the `SpendAgent` variant it becomes, and the module (a file
/// in this folder) and unit struct that implement [`SpendSource`]. The order is the order the
/// pane lists them in.
macro_rules! registry {
    ($($variant:ident => $module:ident :: $source:ident),* $(,)?) => {
        $( pub mod $module; )*

        /// A coding agent that leaves a record of its work on this machine.
        ///
        /// Not a `Provider`, deliberately: a provider is something Pulse can draw a ring for; an
        /// agent is something that has spent tokens here. The two lists overlap and are not the
        /// same.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum SpendAgent { $($variant),* }

        impl SpendAgent {
            /// Every registered agent, in registry order.
            pub const ALL: &'static [SpendAgent] = &[$(SpendAgent::$variant),*];

            /// The implementation behind this agent.
            pub fn source(self) -> &'static dyn SpendSource {
                match self { $(SpendAgent::$variant => &$module::$source),* }
            }
        }
    };
}

registry! {
    ClaudeCode => claude::ClaudeCode,
    Codex => codex::Codex,
    OpenCode => opencode::OpenCode,
    KiloCli => kilo::KiloCli,
    Grok => grok::Grok,
    KimiCli => kimi::KimiCli,
    DevinCli => devin::DevinCli,
    // batch A
    Pi => pi::Pi,
    Omp => omp::Omp,
    Senpi => senpi::Senpi,
    Kimchi => kimchi::Kimchi,
    PrimeAgent => prime_agent::PrimeAgent,
    Gemini => gemini::Gemini,
    Qwen => qwen::Qwen,
    Amp => amp::Amp,
    Droid => droid::Droid,
    OpenClaw => openclaw::OpenClaw,
    // batch B
    RooCode => roocode::RooCode,
    KiloCode => kilocode::KiloCode,
    Cline => cline::Cline,
    CodeBuddy => codebuddy::CodeBuddy,
    WorkBuddy => workbuddy::WorkBuddy,
    CherryStudio => cherrystudio::CherryStudio,
    CommandCode => commandcode::CommandCode,
    OpenCodeReview => opencodereview::OpenCodeReview,
    ZCode => zcode::ZCode,
    // batch D
    Mux => mux::Mux,
    Codebuff => codebuff::Codebuff,
    Freebuff => freebuff::Freebuff,
    JCode => jcode::JCode,
    Augment => augment::Augment,
    Gjc => gjc::Gjc,
    Junie => junie::Junie,
    Dsh => dsh::Dsh,
    Fx => fx::Fx,
    LmStudio => lmstudio::LmStudio,
    Reasonix => reasonix::Reasonix,
}

/// A count out of a JSON value the way the Swift readers took one: a number, whole or not,
/// otherwise zero. (`logio::count` is the strict form for readers that must tell missing from 0.)
pub(crate) fn int(value: Option<&serde_json::Value>) -> i64 {
    match value {
        Some(serde_json::Value::Number(n)) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)).unwrap_or(0),
        _ => 0,
    }
}

/// Lists a folder's entries as paths, sorted; empty when it cannot be read.
pub(crate) fn list(directory: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(directory).into_iter().flatten().flatten().map(|e| e.path()).collect();
    paths.sort();
    paths
}

/// Appends `path` unless it is already there.
pub(crate) fn push_unique(roots: &mut Vec<PathBuf>, path: PathBuf) {
    if !roots.contains(&path) {
        roots.push(path);
    }
}

/// The agent's ledger, cut in `calendar` and priced with `prices`.
///
/// A transcript agent goes through the per-file cache ([`TranscriptReader`]). Any other is read
/// into records and built by the shared [`build_ledger`], and the finished ledger is kept under
/// `cache_directory`, valid while the stores' files and the price table are unchanged. A store
/// that changed while it was being read is not written down: the stamp from before would no
/// longer describe it.
pub fn read_agent(agent: SpendAgent, sources: &Sources, cache_directory: &Path, calendar: &Calendar, prices: &PriceTable) -> Ledger {
    let source = agent.source();
    if let Some(provider) = source.transcript_provider() {
        return TranscriptReader::new(sources.clone(), cache_directory.to_path_buf(), calendar.clone()).ledger(provider, prices);
    }

    let existing = |sources: &Sources| -> Vec<PathBuf> { source.inputs(sources).into_iter().filter(|p| p.exists()).collect() };
    let roots = existing(sources);
    if roots.is_empty() {
        return Ledger::empty();
    }
    let before = Stamp::of(&roots, prices);
    if let Some((stamp, ledger)) = agent_cache::load(agent, cache_directory) {
        if stamp == before {
            return ledger;
        }
    }

    let records = source.records(&roots);
    let origin = if source.requires_usage_export() { Origin::ImportedRecords } else { Origin::LocalTranscripts };
    let mut ledger = build_ledger(&records, prices, agent.raw(), source.price_vendor(), calendar, origin);
    ledger.reports_cache_reads = source.reports_cache_reads();

    if Stamp::of(&existing(sources), prices) == before {
        agent_cache::save(agent, cache_directory, &before, &ledger);
    }
    ledger
}

/// [`read_agent`] with the data folder, the local calendar and the price table on disk, stamped
/// as read at `now`.
pub fn read_agent_now(agent: SpendAgent, home: &Path, now: DateTime<Utc>) -> Ledger {
    let directory = crate::paths::data_dir();
    let prices = super::ModelPrices::cached(&directory);
    let mut ledger = read_agent(agent, &Sources::from_env(home), &directory, &Calendar::local(), &prices);
    ledger.read_at = Some(now);
    ledger
}
