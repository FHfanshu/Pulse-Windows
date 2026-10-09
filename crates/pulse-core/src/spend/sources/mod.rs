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

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use super::agent_archive::AgentArchive;
use super::agent_cache::{self, Stamp};
use super::calendar::Calendar;
use super::ledger::{Ledger, Origin};
use super::prices::PriceTable;
use super::record::{build_ledger, AgentUsageRecord};
use super::transcripts::{Sources, TranscriptReader};
use crate::provider::Provider;

#[derive(Default)]
pub struct SourceRead {
    pub records: Vec<AgentUsageRecord>,
    /// Some compressed history failed to decode; the readable subset is not the whole store.
    pub has_read_limitations: bool,
}

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

    fn read_records(&self, roots: &[PathBuf]) -> SourceRead {
        SourceRead { records: self.records(roots), has_read_limitations: false }
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
    // batch C
    Hermes => hermes::Hermes,
    Goose => goose::Goose,
    Zed => zed::Zed,
    Kiro => kiro::Kiro,
    Crush => crush::Crush,
    Unsloth => unsloth::Unsloth,
    AntigravityCli => antigravity_cli::AntigravityCli,
    AntigravityIde => antigravity_ide::AntigravityIde,
    Micode => micode::Micode,
    DevinDesktop => devin_desktop::DevinDesktop,
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
    // batch E
    Cursor => cursor::Cursor,
    Antigravity => antigravity::Antigravity,
    Trae => trae::Trae,
    Warp => warp::Warp,
    Hindsight => hindsight::Hindsight,
    Mcode => mcode::Mcode,
    Copilot => copilot::Copilot,
}

/// A count out of a JSON value the way the Swift readers took one: a number, whole or not,
/// otherwise zero. (`logio::count` is the strict form for readers that must tell missing from 0.)
pub(crate) fn int(value: Option<&serde_json::Value>) -> i64 {
    match value {
        Some(serde_json::Value::Number(n)) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)).unwrap_or(0),
        _ => 0,
    }
}

/// A numeric epoch in seconds, or milliseconds at or above `1e12`. Zero and negatives are None:
/// an unset column is not 1970, and a record is never dated on a value nobody wrote.
pub(crate) fn epoch(raw: f64) -> Option<DateTime<Utc>> {
    if !raw.is_finite() || raw <= 0.0 {
        return None;
    }
    let seconds = if raw < 1e12 { raw } else { raw / 1000.0 };
    let whole = seconds.trunc() as i64;
    let nanos = ((seconds - seconds.trunc()) * 1e9) as u32;
    DateTime::from_timestamp(whole, nanos)
}

/// A JSON number, or a numeric string, as a finite float.
pub(crate) fn number(value: &serde_json::Value) -> Option<f64> {
    match value {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
    .filter(|n| n.is_finite())
}

/// A token count from JSON, the way the Swift readers clamped one: a number or numeric string,
/// truncated; negative, absent or non-numeric is zero.
pub(crate) fn clamped(value: Option<&serde_json::Value>) -> i64 {
    match value.and_then(number) {
        Some(n) if n > 0.0 => n.trunc() as i64,
        _ => 0,
    }
}

/// An ISO 8601 string or a numeric epoch, as text. A numeric string is an epoch, never an ISO date.
pub(crate) fn flexible_text(text: &str) -> Option<DateTime<Utc>> {
    let trimmed = text.trim();
    if let Ok(number) = trimmed.parse::<f64>() {
        return epoch(number);
    }
    crate::spend::logio::timestamp(Some(&serde_json::Value::String(trimmed.to_string())), false)
}

/// A timestamp from a JSON string (ISO 8601 or a numeric epoch) or a JSON number; None otherwise.
pub(crate) fn flexible(value: Option<&serde_json::Value>) -> Option<DateTime<Utc>> {
    match value? {
        serde_json::Value::String(text) => flexible_text(text),
        serde_json::Value::Number(number) => epoch(number.as_f64()?),
        _ => None,
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

/// The multiset union of the records read from several files of one scope (the Group E exports).
///
/// `{A}` and `{A, B}` yield `A` and `B`, never two `A`s: a row found in several files is kept once
/// per the most any single file holds. Two equal rows inside one file are both kept, because that
/// file itself says there were two. The flag is true when some row appeared in more than one file,
/// so the caller can mark the scope partial rather than add the shared increment again. `signature`
/// is a row's content identity; it only counts, it never drops a row a single file reported twice.
pub(crate) fn reconcile(files: &[Vec<AgentUsageRecord>], signature: impl Fn(&AgentUsageRecord) -> String) -> (Vec<AgentUsageRecord>, bool) {
    let mut order: Vec<String> = Vec::new();
    let mut representative: HashMap<String, AgentUsageRecord> = HashMap::new();
    let mut maximum: HashMap<String, usize> = HashMap::new();
    let mut total: HashMap<String, usize> = HashMap::new();

    for records in files {
        let mut per_file: HashMap<String, usize> = HashMap::new();
        for record in records {
            let key = signature(record);
            *per_file.entry(key.clone()).or_insert(0) += 1;
            if !representative.contains_key(&key) {
                representative.insert(key.clone(), record.clone());
                order.push(key);
            }
        }
        for (key, count) in per_file {
            let most = maximum.entry(key.clone()).or_insert(0);
            *most = (*most).max(count);
            *total.entry(key).or_insert(0) += count;
        }
    }

    let mut out = Vec::new();
    let mut overlapped = false;
    for key in order {
        let emit = maximum.get(&key).copied().unwrap_or(0);
        if total.get(&key).copied().unwrap_or(0) > emit {
            overlapped = true;
        }
        if let Some(record) = representative.get(&key) {
            for _ in 0..emit {
                out.push(record.clone());
            }
        }
    }
    (out, overlapped)
}

/// The files whose bytes differ, in path order: a byte-identical export written to two roots is one
/// export, read once. Rows inside one file are untouched.
pub(crate) fn replay_distinct(files: &[PathBuf]) -> Vec<PathBuf> {
    use sha2::{Digest, Sha256};
    let mut sorted: Vec<&PathBuf> = files.iter().collect();
    sorted.sort();
    let mut seen = std::collections::HashSet::new();
    let mut distinct = Vec::new();
    for file in sorted {
        let Ok(bytes) = std::fs::read(file) else { continue };
        if seen.insert(Sha256::digest(&bytes).to_vec()) {
            distinct.push(file.clone());
        }
    }
    distinct
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
    let saved = agent_cache::load(agent, cache_directory);
    if let Some((stamp, ledger)) = &saved {
        if *stamp == before {
            return kept(agent, ledger, None, true, cache_directory, calendar, prices);
        }
    }

    let read = source.read_records(&roots);
    let origin = if source.requires_usage_export() { Origin::ImportedRecords } else { Origin::LocalTranscripts };
    let mut ledger = build_ledger(&read.records, prices, agent.raw(), source.price_vendor(), calendar, origin);
    ledger.has_read_limitations = read.has_read_limitations;
    ledger.reports_cache_reads = source.reports_cache_reads();

    let unchanged = Stamp::of(&existing(sources), prices) == before;
    if unchanged {
        agent_cache::save(agent, cache_directory, &before, &ledger);
    }
    // Only a read of a store that held still, and that decoded in full, may raise the marks. The
    // cache this read replaces was itself such a read: what it held and this one does not is
    // what the store has deleted since.
    let stable = unchanged && !read.has_read_limitations;
    kept(agent, &ledger, saved.as_ref().map(|(_, l)| l), stable, cache_directory, calendar, prices)
}

/// A live ledger with the agent's kept history added in, after a stable read has raised the
/// marks ([`AgentArchive`]). An archive that cannot be read leaves the ledger as read and is not
/// written over.
fn kept(
    agent: SpendAgent,
    live: &Ledger,
    previous: Option<&Ledger>,
    stable: bool,
    directory: &Path,
    calendar: &Calendar,
    prices: &PriceTable,
) -> Ledger {
    if !AgentArchive::keeps(agent) {
        return live.clone();
    }
    let Some(mut archive) = AgentArchive::load(agent, directory) else { return live.clone() };
    if stable && archive.absorb(live, previous, calendar) {
        archive.save(agent, directory);
    }
    archive.merged(live, prices, agent.source().price_vendor(), calendar)
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
