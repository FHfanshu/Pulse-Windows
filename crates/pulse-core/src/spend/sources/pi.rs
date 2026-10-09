// Ported from upstream Sources/Pulse/Usage/Readers/PiFamilySessionReader.swift (with PiTranscript).
//! Pi's session transcripts, and the parser shared with omp, Senpi, Kimchi and Prime Agent.
//!
//! `~/.pi/agent/sessions/**/*.jsonl`, one JSONL file per session: an optional `title` record, a
//! `session` header carrying the session id and working directory, then `message` records whose
//! `message.usage` holds four independent token buckets (`input`, `output`, `cacheRead`,
//! `cacheWrite`) and an optional `totalTokens`. A branch or fork copies prior assistant records
//! into a new file verbatim, so the record identity is session-independent: a copy folds onto
//! its original by `responseId`, or by a composite of the message's own fields.
//!
//! **Reasoning is inside output.** The format documents `reasoning` as a subset of `output`, so
//! it is never a bucket of its own and is never added to output a second time.
//!
//! A malformed session header discards the whole file: without a header there is no session id
//! or working directory, and a record built from the rest would be attributed to nobody.
//!
//! Where it lives on Windows: `%USERPROFILE%\.pi\agent\sessions` (the Node CLI layout carried
//! over). Unverified on a real PC.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::{push_unique, SpendSource};
use crate::spend::logio;
use crate::spend::record::AgentUsageRecord;
use crate::spend::tally::TokenTally;
use crate::spend::transcripts::Sources;

pub struct Pi;

impl SpendSource for Pi {
    fn raw(&self) -> &'static str {
        "pi"
    }
    fn source_id(&self) -> &'static str {
        "pi"
    }
    fn display_name(&self) -> &'static str {
        "Pi"
    }
    fn icon(&self) -> Option<&'static str> {
        Some("pi")
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        // WINDOWS-PATH: unverified (`%USERPROFILE%\.pi` is the macOS layout carried over)
        vec![sources.home.join(".pi").join("agent").join("sessions")]
    }
    fn records(&self, roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        family_records(&Family::PI, roots)
    }
}

/// The header of a session file.
#[derive(Debug, Clone, Default)]
pub(crate) struct Header {
    pub id: Option<String>,
    pub cwd: Option<String>,
    pub parent_session: Option<String>,
    pub rlm_depth: Option<i64>,
}

/// One assistant message with usage.
#[derive(Debug, Clone)]
pub(crate) struct Message {
    pub id: Option<String>,
    pub response_id: Option<String>,
    pub timestamp: Option<DateTime<Utc>>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub tally: TokenTally,
    pub unclassified: i64,
}

/// A `child_usage_attributed` record: a parent message carrying a child's usage.
#[derive(Debug, Clone)]
pub(crate) struct Attribution {
    pub id: Option<String>,
    pub target_id: Option<String>,
    pub child_usage: TokenTally,
    pub aggregate_usage: TokenTally,
}

/// One parsed session file. `path` is in [`key`] form so two spellings of one file compare equal.
#[derive(Debug, Clone)]
pub(crate) struct File {
    pub path: String,
    pub header: Header,
    pub session_info_name: Option<String>,
    pub messages: Vec<Message>,
    pub attributions: Vec<Attribution>,
    pub is_valid: bool,
}

/// A path as a comparison key: one separator style, case-folded, as Windows compares paths.
pub(crate) fn key(path: &Path) -> String {
    path.to_string_lossy().replace('/', "\\").to_lowercase()
}

/// Parses one file. A malformed header is fatal to the whole file.
pub(crate) fn parse(path: &Path) -> File {
    let mut header: Option<Header> = None;
    let mut session_info_name = None;
    let mut messages = Vec::new();
    let mut attributions = Vec::new();
    let mut malformed = false;

    for row in logio::json_lines(path) {
        let kind = row.get("type").and_then(Value::as_str);

        if header.is_none() {
            // A descendant may open with title metadata; anything else is the header or the file is malformed.
            if kind == Some("title") {
                continue;
            }
            match (kind, logio::text(row.get("id"))) {
                (Some("session"), Some(id)) => {
                    header = Some(Header {
                        id: Some(id),
                        cwd: logio::text(row.get("cwd")),
                        parent_session: logio::text(row.get("parentSession")),
                        rlm_depth: logio::count(row.get("rlmDepth")),
                    });
                }
                _ => {
                    malformed = true;
                    break;
                }
            }
            continue;
        }

        match kind {
            Some("session_info") => {
                if let Some(name) = logio::text(row.get("name")) {
                    session_info_name = Some(name);
                }
            }
            Some("message") => messages.extend(parse_message(&row)),
            Some("child_usage_attributed") => attributions.extend(parse_attribution(&row)),
            _ => {}
        }
    }

    File {
        path: key(path),
        header: header.clone().unwrap_or_default(),
        session_info_name,
        messages,
        attributions,
        is_valid: !malformed && header.is_some(),
    }
}

/// The working directories recorded by the session headers under `roots`.
pub(crate) fn cwd_values(roots: &[PathBuf]) -> Vec<String> {
    logio::files(roots, &["jsonl"], &[], &[])
        .into_iter()
        .filter_map(|file| {
            for row in logio::json_lines(&file) {
                if row.get("type").and_then(Value::as_str) == Some("title") {
                    continue;
                }
                if row.get("type").and_then(Value::as_str) != Some("session") || logio::text(row.get("id")).is_none() {
                    return None;
                }
                return logio::text(row.get("cwd"));
            }
            None
        })
        .collect()
}

/// `<cwd>/.omo/senpi-task/children` for every working directory a session header in `roots` recorded.
pub(crate) fn senpi_children(roots: &[PathBuf]) -> Vec<PathBuf> {
    cwd_values(roots).into_iter().map(|cwd| PathBuf::from(cwd).join(".omo").join("senpi-task").join("children")).collect()
}

/// The four buckets, plus any reported total that the four do not explain.
///
/// A total the known buckets already account for yields nothing unclassified. A total standing in
/// for a kind nobody named is carried as unclassified, never poured into input.
pub(crate) fn usage(object: &Value) -> (TokenTally, i64) {
    let input = logio::count(object.get("input"));
    let output = logio::count(object.get("output"));
    let cache_read = logio::count(object.get("cacheRead"));
    let cache_write = logio::count(object.get("cacheWrite"));
    let total = logio::count(object.get("totalTokens"));

    let tally = TokenTally::new(input.unwrap_or(0), cache_write.unwrap_or(0), cache_read.unwrap_or(0), output.unwrap_or(0));
    let any_known = input.is_some() || output.is_some() || cache_read.is_some() || cache_write.is_some();
    let Some(total) = total else { return (tally, 0) };
    if !any_known {
        return (TokenTally::default(), total);
    }
    let remainder = total - tally.total();
    (tally, remainder.max(0))
}

fn parse_message(row: &Value) -> Option<Message> {
    let message = row.get("message").filter(|m| m.is_object())?;
    if message.get("role").and_then(Value::as_str) != Some("assistant") {
        return None;
    }
    let usage_object = message.get("usage").filter(|u| u.is_object())?;
    let (tally, unclassified) = usage(usage_object);
    if tally.total() <= 0 && unclassified <= 0 {
        return None;
    }
    Some(Message {
        id: logio::text(row.get("id")),
        response_id: logio::text(message.get("responseId")),
        timestamp: logio::timestamp(row.get("timestamp"), false).or_else(|| logio::timestamp(message.get("timestamp"), false)),
        provider: logio::text(message.get("provider")),
        model: logio::text(message.get("model")),
        tally,
        unclassified,
    })
}

fn parse_attribution(row: &Value) -> Option<Attribution> {
    let child = row.get("childUsage").filter(|v| v.is_object())?;
    let aggregate = row.get("aggregateUsage").filter(|v| v.is_object())?;
    let child_usage = usage(child).0;
    let aggregate_usage = usage(aggregate).0;
    if child_usage.total() <= 0 && aggregate_usage.total() <= 0 {
        return None;
    }
    Some(Attribution {
        id: logio::text(row.get("id")),
        target_id: logio::text(row.get("targetId")),
        child_usage,
        aggregate_usage,
    })
}

/// How a Pi-shaped client differs from the others. Pi, omp, Senpi and Kimchi are configurations of
/// one reader rather than four copies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Dedup {
    /// A fork copy of a message folds onto its original by response id, or by the message's own fields.
    CrossSession,
    /// Kimchi's older scheme keeps each session's namespace separate.
    SessionScoped,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Family {
    pub client: &'static str,
    pub dedup: Dedup,
    /// Senpi's OmO task children, which live outside its sessions tree under each header's cwd.
    pub discovers_children: bool,
    /// Senpi treats `session_info.name` as a human title.
    pub session_info_is_title: bool,
    pub provider_fallback: Option<&'static str>,
}

impl Family {
    pub(crate) const PI: Family = Family {
        client: "pi",
        dedup: Dedup::CrossSession,
        discovers_children: false,
        session_info_is_title: false,
        provider_fallback: None,
    };
    pub(crate) const OMP: Family = Family {
        client: "omp",
        dedup: Dedup::CrossSession,
        discovers_children: false,
        session_info_is_title: false,
        provider_fallback: Some("omp"),
    };
    pub(crate) const SENPI: Family = Family {
        client: "senpi",
        dedup: Dedup::CrossSession,
        discovers_children: true,
        session_info_is_title: true,
        provider_fallback: None,
    };
    pub(crate) const KIMCHI: Family = Family {
        client: "kimchi",
        dedup: Dedup::SessionScoped,
        discovers_children: false,
        session_info_is_title: false,
        provider_fallback: None,
    };
}

/// The records of one Pi-shaped family over its roots. A message with real usage but no usable
/// header, time or model makes the whole read partial rather than being dropped silently.
pub(crate) fn family_records(family: &Family, roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
    let mut files: Vec<File> = logio::files(roots, &["jsonl"], &[], &[]).iter().map(|p| parse(p)).collect();

    if family.discovers_children {
        let known: HashSet<String> = files.iter().map(|f| f.path.clone()).collect();
        for path in logio::files(&senpi_children(roots), &["jsonl"], &[], &[]) {
            if !known.contains(&key(&path)) {
                files.push(parse(&path));
            }
        }
    }

    let mut records = Vec::new();
    let mut incomplete = false;
    for file in &files {
        let session_id = if file.is_valid { file.header.id.clone() } else { None };
        for message in &file.messages {
            let (Some(session_id), Some(timestamp), Some(model)) = (session_id.as_ref(), message.timestamp, message.model.as_ref()) else {
                // Real usage with no usable header, time or model: a readable subset, not the whole story.
                incomplete = true;
                continue;
            };
            let mut record = AgentUsageRecord::new(timestamp, model, message.tally.clone())
                .session(session_id)
                .unclassified(message.unclassified);
            record.title = if family.session_info_is_title { file.session_info_name.clone() } else { None };
            record.project = file.header.cwd.clone();
            record.deduplication_id = deduplication_id(family, session_id, message);
            records.push(record);
        }
    }
    if incomplete {
        for record in &mut records {
            record.is_partial = true;
        }
    }
    records
}

fn deduplication_id(family: &Family, session_id: &str, message: &Message) -> Option<String> {
    let client = family.client;
    match family.dedup {
        // No message identity means no stable key; each occurrence stands on its own.
        Dedup::SessionScoped => message.id.as_ref().map(|id| format!("{client}:{session_id}:{id}")),
        Dedup::CrossSession => {
            if let Some(response) = &message.response_id {
                return Some(format!("{client}:response:{response}"));
            }
            let timestamp = message.timestamp?;
            let milliseconds = (timestamp.timestamp_micros() as f64 / 1000.0).round() as i64;
            let provider = message.provider.clone().or_else(|| family.provider_fallback.map(str::to_string)).unwrap_or_default();
            let tally = &message.tally;
            Some(format!(
                "{client}:message:{}:{milliseconds}:{provider}:{}:{}:{}:{}:{}",
                message.id.clone().unwrap_or_default(),
                message.model.clone().unwrap_or_default(),
                tally.input,
                tally.output,
                tally.cache_read,
                tally.cache_write,
            ))
        }
    }
}

/// Adds a path to the roots unless it is there already, comparing by [`key`].
pub(crate) fn push_unique_path(roots: &mut Vec<PathBuf>, path: PathBuf) {
    if !roots.iter().any(|r| key(r) == key(&path)) {
        push_unique(roots, path);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::Path;

    /// One Pi-shaped session file with a single assistant message.
    pub(crate) fn pi_file(
        id: &str,
        cwd: &str,
        message_id: &str,
        response_id: Option<&str>,
        model: &str,
        counts: [i64; 4],
    ) -> String {
        let [input, output, cache_read, cache_write] = counts;
        let response = response_id.map(|r| format!(",\"responseId\":\"{r}\"")).unwrap_or_default();
        format!(
            "{{\"type\":\"session\",\"id\":\"{id}\",\"timestamp\":\"2026-01-02T03:05:00Z\",\"cwd\":\"{cwd}\"}}\n\
             {{\"type\":\"message\",\"id\":\"{message_id}\",\"timestamp\":\"2026-01-02T03:05:00Z\",\"message\":{{\"role\":\"assistant\",\"model\":\"{model}\",\"provider\":\"openai\"{response},\"usage\":{{\"input\":{input},\"output\":{output},\"cacheRead\":{cache_read},\"cacheWrite\":{cache_write},\"totalTokens\":{}}}}}}}",
            input + output + cache_read + cache_write
        )
    }

    pub(crate) fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
}

#[cfg(test)]
mod pi_tests {
    use std::collections::HashMap;

    use super::tests::{pi_file, write};
    use super::*;
    use crate::spend::calendar::Calendar;
    use crate::spend::prices::{ModelPrice, PriceTable};
    use crate::spend::sources::{read_agent, SpendAgent};
    use crate::spend::transcripts::Sources;

    fn prices() -> PriceTable {
        HashMap::from([("gpt-5".to_string(), ModelPrice::new(1.0, 10.0, Some(0.1), Some(1.0), Some("GPT-5")))])
    }

    fn ledger_tokens(home: &Path, agent: SpendAgent) -> i64 {
        let cache = tempfile::tempdir().unwrap();
        read_agent(agent, &Sources::new(home), cache.path(), &Calendar::utc(2), &prices()).days.iter().map(|d| d.tokens).sum()
    }

    #[test]
    fn pi_reads_the_four_buckets_and_leaves_reasoning_inside_output() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join(".pi/agent/sessions");
        write(
            &root.join("encoded-cwd/session.jsonl"),
            &pi_file("sess-a", "/work/pulse", "m1", Some("resp-1"), "gpt-5", [100, 40, 30, 20]),
        );
        let records = family_records(&Family::PI, std::slice::from_ref(&root));
        assert_eq!(records.len(), 1);
        let record = &records[0];
        assert_eq!(record.tally, TokenTally::new(100, 20, 30, 40));
        assert_eq!(record.model, "gpt-5");
        assert_eq!(record.session_id.as_deref(), Some("sess-a"));
        assert_eq!(record.project.as_deref(), Some("/work/pulse"));
        assert_eq!(record.deduplication_id.as_deref(), Some("pi:response:resp-1"));
        assert_eq!(record.unclassified_tokens, 0);
    }

    #[test]
    fn a_bare_total_is_unclassified_never_poured_into_input() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join(".pi/agent/sessions");
        write(
            &root.join("s.jsonl"),
            "{\"type\":\"session\",\"id\":\"s\",\"timestamp\":\"2026-01-02T03:05:00Z\",\"cwd\":\"/work/x\"}\n\
             {\"type\":\"message\",\"id\":\"m\",\"timestamp\":\"2026-01-02T03:05:01Z\",\"message\":{\"role\":\"assistant\",\"model\":\"gpt-5\",\"usage\":{\"totalTokens\":77}}}",
        );
        let record = family_records(&Family::PI, std::slice::from_ref(&root)).into_iter().next().unwrap();
        assert_eq!(record.tally, TokenTally::default());
        assert_eq!(record.unclassified_tokens, 77);
    }

    #[test]
    fn a_remainder_beside_known_buckets_is_unclassified_and_an_explained_total_is_not() {
        let explained = usage(&serde_json::json!({"input": 10, "output": 5, "totalTokens": 15}));
        assert_eq!(explained, (TokenTally::new(10, 0, 0, 5), 0));
        let remainder = usage(&serde_json::json!({"input": 10, "output": 5, "totalTokens": 20}));
        assert_eq!(remainder, (TokenTally::new(10, 0, 0, 5), 5));
    }

    #[test]
    fn a_malformed_header_discards_the_whole_file_and_title_metadata_may_precede_it() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join(".pi/agent/sessions");
        // A message before any session header is not a session.
        write(
            &root.join("no-header.jsonl"),
            "{\"type\":\"message\",\"id\":\"m\",\"timestamp\":\"2026-01-02T03:05:00Z\",\"message\":{\"role\":\"assistant\",\"model\":\"gpt-5\",\"usage\":{\"input\":5}}}",
        );
        assert!(family_records(&Family::PI, std::slice::from_ref(&root)).is_empty());

        // A title record ahead of a real header is ordinary and kept.
        write(
            &root.join("with-title.jsonl"),
            "{\"type\":\"title\",\"title\":\"The ring\"}\n\
             {\"type\":\"session\",\"id\":\"ok\",\"timestamp\":\"2026-01-02T03:05:00Z\",\"cwd\":\"/w\"}\n\
             {\"type\":\"message\",\"id\":\"m\",\"timestamp\":\"2026-01-02T03:05:01Z\",\"message\":{\"role\":\"assistant\",\"model\":\"gpt-5\",\"usage\":{\"input\":5}}}",
        );
        assert_eq!(family_records(&Family::PI, std::slice::from_ref(&root)).len(), 1);
    }

    #[test]
    fn a_fork_copy_of_one_message_folds_onto_its_original_by_response_id() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join(".pi/agent/sessions");
        write(&root.join("a.jsonl"), &pi_file("a", "/w", "m1", Some("resp-shared"), "gpt-5", [100, 10, 0, 0]));
        write(&root.join("b.jsonl"), &pi_file("b", "/w", "m-other", Some("resp-shared"), "gpt-5", [100, 10, 0, 0]));
        assert_eq!(family_records(&Family::PI, std::slice::from_ref(&root)).len(), 2);
        assert_eq!(ledger_tokens(home.path(), SpendAgent::Pi), 110);
    }

    #[test]
    fn a_missing_store_is_an_empty_ledger_and_an_override_moves_it() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(ledger_tokens(home.path(), SpendAgent::Pi), 0);
        let elsewhere = tempfile::tempdir().unwrap();
        write(&elsewhere.path().join("cwd/session.jsonl"), &pi_file("s", "/w", "m1", None, "gpt-5", [100, 40, 30, 20]));
        // Pi takes no override: the shared PI_CODING_AGENT_DIR is read by Pi and omp, so neither honours it.
        let cache = tempfile::tempdir().unwrap();
        let sources = Sources::new(home.path()).with_var("PI_CODING_AGENT_DIR", elsewhere.path());
        let ledger = read_agent(SpendAgent::Pi, &sources, cache.path(), &Calendar::utc(2), &prices());
        assert_eq!(ledger.days.iter().map(|d| d.tokens).sum::<i64>(), 0);
    }
}
