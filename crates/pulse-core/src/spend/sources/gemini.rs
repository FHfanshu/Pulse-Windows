// Ported from upstream Sources/Pulse/Usage/Readers/GeminiSessionReader.swift
// and Sources/Pulse/Usage/Readers/SessionLogPaths.swift (gemini).
//! Gemini CLI's three on-disk shapes.
//!
//! - a legacy whole-session JSON (`session-*.json`),
//! - the current chat recording (`tmp/<id>/chats/<file>.json`),
//! - a headless JSONL stream whose `init` line names the model and session and whose `tokens` or
//!   `stats` lines carry usage.
//!
//! The token keys are aliases (`prompt` / `input_tokens` / `promptTokenCount` and so on), and the
//! cache relation is **shape-specific**, so the decoders are kept apart rather than blended. Tool
//! tokens are real counters and both shapes fold them into fresh input. Where a headless object
//! names neither tool nor thought tokens but states a total, what the total holds beyond input and
//! output is counted unclassified. Reasoning is additive on top of output; cache writes are zero.
//!
//! Where it lives on Windows: `GEMINI_CLI_HOME` when set, else `%USERPROFILE%\.gemini\tmp`.
//! Unverified on a real PC.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde_json::{Map, Value};

use super::SpendSource;
use crate::spend::logio;
use crate::spend::record::AgentUsageRecord;
use crate::spend::tally::TokenTally;
use crate::spend::transcripts::Sources;

pub struct Gemini;

impl SpendSource for Gemini {
    fn raw(&self) -> &'static str {
        "gemini"
    }
    fn source_id(&self) -> &'static str {
        "gemini"
    }
    fn display_name(&self) -> &'static str {
        "Gemini CLI"
    }
    fn icon(&self) -> Option<&'static str> {
        Some("gemini")
    }
    fn env_vars(&self) -> &'static [&'static str] {
        &["GEMINI_CLI_HOME"]
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        // WINDOWS-PATH: unverified (`%USERPROFILE%\.gemini` is the macOS layout carried over)
        let base = sources.var("GEMINI_CLI_HOME").unwrap_or_else(|| sources.home.join(".gemini"));
        vec![base.join("tmp")]
    }
    fn records(&self, roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        let mut records = Vec::new();
        let mut incomplete = false;
        for file in logio::files(roots, &["json", "jsonl"], &[], &[]) {
            let parsed = if file.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                headless(&file)
            } else if file.file_name().is_some_and(|n| n.to_string_lossy().starts_with("session-")) || is_chat_path(&file) {
                session(&file)
            } else {
                continue;
            };
            records.extend(parsed.0);
            incomplete |= parsed.1;
        }
        if incomplete {
            for record in &mut records {
                record.is_partial = true;
            }
        }
        records
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    Session,
    Headless,
}

/// The current chat recording's exact `.../tmp/<id>/chats/<file>.json` shape. A JSON file anywhere
/// else is not accepted: a different shape read as this one would attribute the wrong session.
fn is_chat_path(path: &Path) -> bool {
    let parts: Vec<String> = path.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    match parts.iter().rposition(|p| p == "chats") {
        Some(chats) => chats >= 2 && parts[chats - 2] == "tmp" && !parts[chats - 1].is_empty(),
        None => false,
    }
}

fn stem(path: &Path) -> String {
    path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

fn record(
    at: DateTime<Utc>,
    model: &str,
    tally: TokenTally,
    session: &str,
    dedup: Option<String>,
    unclassified: i64,
) -> AgentUsageRecord {
    let mut record = AgentUsageRecord::new(at, model, tally).session(session).unclassified(unclassified);
    record.deduplication_id = dedup;
    record
}

/// A whole session JSON: `{ "sessionId", "messages": [{ "type": "gemini", "tokens", ... }] }`.
fn session(path: &Path) -> (Vec<AgentUsageRecord>, bool) {
    let Some(root) = logio::json(path).and_then(|v| v.as_object().cloned()) else { return (Vec::new(), false) };
    let session_id = logio::text(root.get("sessionId"))
        .or_else(|| logio::text(root.get("session_id")))
        .unwrap_or_else(|| stem(path));
    // The whole list is taken only when every entry is an object, as the Swift cast requires.
    let messages: Vec<Value> = root
        .get("messages")
        .and_then(Value::as_array)
        .filter(|a| a.iter().all(Value::is_object))
        .cloned()
        .unwrap_or_default();

    let mut records = Vec::new();
    let mut incomplete = false;
    for message in &messages {
        if message.get("type").and_then(Value::as_str) != Some("gemini") {
            continue;
        }
        let Some(tokens) = message.get("tokens").and_then(Value::as_object) else { continue };
        let usage = decode(tokens, Shape::Session, false);
        if usage.0.total() <= 0 && usage.1 <= 0 {
            continue;
        }
        let model = logio::text(message.get("model"));
        let timestamp = logio::timestamp(message.get("timestamp"), false).or_else(|| logio::timestamp(message.get("created_at"), false));
        let (Some(model), Some(timestamp)) = (model, timestamp) else {
            // Real usage with no model or no locatable time.
            incomplete = true;
            continue;
        };
        let dedup = logio::text(message.get("id")).map(|id| format!("gemini:session:{session_id}:{id}"));
        records.push(record(timestamp, &model, usage.0, &session_id, dedup, usage.1));
    }
    (records, incomplete)
}

/// Keeps one record per request id: a re-export of the same call replaces its original in place.
#[derive(Default)]
struct Submitted {
    records: Vec<AgentUsageRecord>,
    index_by_id: HashMap<String, usize>,
}

impl Submitted {
    fn submit(&mut self, record: AgentUsageRecord, id: Option<String>) {
        if let Some(id) = id {
            if let Some(&existing) = self.index_by_id.get(&id) {
                self.records[existing] = record;
                return;
            }
            self.index_by_id.insert(id, self.records.len());
        }
        self.records.push(record);
    }
}

/// A headless JSONL stream: an `init` line names the model and session; `tokens` lines and
/// `stats` lines (`models` per model, or one flat object) carry the usage.
fn headless(path: &Path) -> (Vec<AgentUsageRecord>, bool) {
    let file_stem = stem(path);
    let mut current_model: Option<String> = None;
    let mut current_session: Option<String> = None;
    let mut submitted = Submitted::default();
    let mut incomplete = false;

    for (line, row) in logio::json_lines(path).enumerate() {
        if row.get("type").and_then(Value::as_str) == Some("init") {
            if let Some(model) = logio::text(row.get("model")) {
                current_model = Some(model);
            }
            if let Some(session) = logio::text(row.get("session_id")).or_else(|| logio::text(row.get("sessionId"))) {
                current_session = Some(session);
            }
            continue;
        }

        let session_id = logio::text(row.get("session_id"))
            .or_else(|| logio::text(row.get("sessionId")))
            .or_else(|| current_session.clone())
            .unwrap_or_else(|| file_stem.clone());
        let line_id = logio::text(row.get("id"));
        let line_time = logio::timestamp(row.get("timestamp"), false).or_else(|| logio::timestamp(row.get("created_at"), false));
        let line_dedup = line_id.as_ref().map(|id| format!("gemini:line:{id}"));
        let headless_dedup = format!("gemini:headless:{file_stem}:{line}");

        if let Some(tokens) = row.get("tokens").and_then(Value::as_object) {
            let usage = decode(tokens, Shape::Headless, true);
            if usage.0.total() <= 0 && usage.1 <= 0 {
                continue;
            }
            let model = logio::text(row.get("model")).or_else(|| current_model.clone());
            match (model, line_time) {
                (Some(model), Some(timestamp)) => {
                    let dedup = line_dedup.clone().unwrap_or_else(|| headless_dedup.clone());
                    submitted.submit(record(timestamp, &model, usage.0, &session_id, Some(dedup), usage.1), line_id.clone());
                }
                _ => incomplete = true,
            }
            continue;
        }

        let stats = row.get("stats").and_then(Value::as_object).or_else(|| {
            row.get("result").and_then(|r| r.get("stats")).and_then(Value::as_object)
        });
        let Some(stats) = stats else { continue };

        let models = stats
            .get("models")
            .and_then(Value::as_object)
            .filter(|m| m.values().all(Value::is_object));
        if let Some(models) = models {
            for (model, entry) in models {
                let Some(entry) = entry.as_object() else { continue };
                // `--output-format json` nests a model's counts under `tokens`; `stream-json` puts them on the model itself.
                let counts = entry.get("tokens").and_then(Value::as_object).unwrap_or(entry);
                let usage = decode(counts, Shape::Headless, false);
                if usage.0.total() <= 0 && usage.1 <= 0 {
                    continue;
                }
                let Some(timestamp) = logio::timestamp(entry.get("timestamp"), false).or(line_time) else {
                    incomplete = true;
                    continue;
                };
                let id = line_id.as_ref().map(|id| format!("{id}:{model}"));
                let dedup = id
                    .as_ref()
                    .map(|i| format!("gemini:line:{i}"))
                    .unwrap_or_else(|| format!("{headless_dedup}:{model}"));
                submitted.submit(record(timestamp, model, usage.0, &session_id, Some(dedup), usage.1), id);
            }
        } else {
            let usage = decode(stats, Shape::Headless, false);
            if usage.0.total() <= 0 && usage.1 <= 0 {
                continue;
            }
            let model = logio::text(stats.get("model")).or_else(|| current_model.clone());
            let timestamp = logio::timestamp(stats.get("timestamp"), false).or(line_time);
            let (Some(model), Some(timestamp)) = (model, timestamp) else {
                incomplete = true;
                continue;
            };
            let dedup = line_dedup.clone().unwrap_or_else(|| headless_dedup.clone());
            submitted.submit(record(timestamp, &model, usage.0, &session_id, Some(dedup), usage.1), line_id.clone());
        }
    }
    (submitted.records, incomplete)
}

/// Decodes a Gemini usage object into disjoint buckets, returning the tally and the unclassified
/// remainder.
///
/// The session shape's cache overlap is proven only by a total that equals the non-cache sum. The
/// headless shape treats an input under a prompt-style key (or a `tokens` wrapper) as
/// cache-inclusive, and a bare `input` field as already net. A bare total with no named kind is
/// carried as unclassified rather than guessed into input.
fn decode(object: &Map<String, Value>, shape: Shape, token_wrapper: bool) -> (TokenTally, i64) {
    // The first key present, with the key it was found under (present includes an explicit null).
    let first = |keys: &[&'static str]| -> Option<(&Value, &'static str)> {
        keys.iter().find_map(|key| object.get(*key).map(|value| (value, *key)))
    };
    let input = first(&["input", "prompt", "input_tokens", "prompt_tokens", "promptTokenCount"]);
    let output = first(&["output", "candidates", "output_tokens", "completion_tokens", "candidatesTokenCount"]);
    let cached = first(&["cached", "cached_tokens", "cachedContentTokenCount"]);
    let reasoning = first(&["thoughts", "reasoning", "thoughts_tokens"]);
    let tool = first(&["tool", "tool_tokens"]);
    let total = first(&["total", "totalTokenCount", "total_tokens"]);

    let count = |entry: &Option<(&Value, &'static str)>| logio::count(entry.as_ref().map(|(v, _)| *v)).unwrap_or(0);
    let input_count = count(&input);
    let output_count = count(&output);
    let cached_count = count(&cached);
    let reasoning_count = count(&reasoning);
    let tool_count = count(&tool);
    let total_count = logio::count(total.map(|(v, _)| v));

    let any_kind = input.is_some() || output.is_some() || cached.is_some() || reasoning.is_some() || tool.is_some();
    if !any_kind {
        return (TokenTally::default(), total_count.unwrap_or(0));
    }

    let output_bucket = output_count + reasoning_count;
    match shape {
        Shape::Session => {
            let raw = input_count + tool_count;
            // A reported total is the authority when present. Without one, a prompt-style input key
            // still means the prompt includes the cached content; a net `input` field is left alone.
            if let Some(total) = total_count {
                let sum = raw + output_count + reasoning_count;
                let cache_inclusive = total == sum && total != sum + cached_count;
                let fresh = if cache_inclusive { (raw - cached_count).max(0) } else { raw };
                return (TokenTally::new(fresh, 0, cached_count, output_bucket), 0);
            }
            let prompt_style = input.is_some_and(|(_, key)| key != "input");
            let fresh = if prompt_style { (raw - cached_count).max(0) } else { raw };
            (TokenTally::new(fresh, 0, cached_count, output_bucket), 0)
        }
        Shape::Headless => {
            let cache_inclusive = token_wrapper || input.is_some_and(|(_, key)| key != "input");
            // Tool-use prompt tokens are input the model was sent, counted apart from the prompt by Gemini.
            let fresh = (if cache_inclusive { (input_count - cached_count).max(0) } else { input_count }) + tool_count;
            // `stream-json` names neither thoughts nor tool tokens, and its `output_tokens` is the
            // candidates alone; its total still holds both. What the total has beyond input and
            // output is real work that cannot be told apart, so it is counted unclassified.
            let mut unclassified = 0;
            if reasoning.is_none() && tool.is_none() {
                if let Some(total) = total_count {
                    let gross = if cache_inclusive { input_count } else { input_count + cached_count };
                    unclassified = (total - gross - output_count).max(0);
                }
            }
            (TokenTally::new(fresh, 0, cached_count, output_bucket), unclassified)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::spend::calendar::Calendar;
    use crate::spend::prices::{ModelPrice, PriceTable};
    use crate::spend::sources::{read_agent, SpendAgent};
    use crate::spend::transcripts::Sources;

    fn prices() -> PriceTable {
        HashMap::from([("gemini-2.5-pro".to_string(), ModelPrice::new(1.0, 10.0, Some(0.1), Some(1.0), Some("Gemini 2.5 Pro")))])
    }

    /// Writes `text` under the Gemini root of `home` and returns the records read from it.
    fn read(home: &Path, relative: &str, text: &str) -> Vec<AgentUsageRecord> {
        let path = home.join(".gemini/tmp").join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        Gemini.records(&[home.join(".gemini/tmp")])
    }

    fn by_dedup<'a>(records: &'a [AgentUsageRecord], id: &str) -> &'a AgentUsageRecord {
        records.iter().find(|r| r.deduplication_id.as_deref() == Some(id)).unwrap()
    }

    #[test]
    fn a_session_folds_the_tool_into_input_and_strips_the_cache_only_when_the_total_proves_it() {
        let home = tempfile::tempdir().unwrap();
        let records = read(
            home.path(),
            "abc/chats/chat.json",
            r#"{"sessionId":"g-1","messages":[
              {"id":"g1","timestamp":"2026-01-02T03:04:05Z","type":"gemini","model":"gemini-2.5-pro","tokens":{"input":100,"output":20,"thoughts":5,"cached":30,"tool":10,"total":135}},
              {"id":"g2","timestamp":"2026-01-02T03:05:05Z","type":"gemini","model":"gemini-2.5-pro","tokens":{"input":100,"output":20,"thoughts":5,"cached":30,"tool":10}},
              {"id":"g3","timestamp":"2026-01-02T03:06:05Z","type":"user","tokens":{"input":999}}
            ]}"#,
        );
        assert_eq!(records.len(), 2);
        assert_eq!(by_dedup(&records, "gemini:session:g-1:g1").tally, TokenTally::new(80, 0, 30, 25));
        assert_eq!(by_dedup(&records, "gemini:session:g-1:g2").tally, TokenTally::new(110, 0, 30, 25));
    }

    #[test]
    fn a_bare_total_is_unclassified_and_a_legacy_session_file_is_accepted() {
        let home = tempfile::tempdir().unwrap();
        let records = read(
            home.path(),
            "session-old.json",
            r#"{"sessionId":"legacy","messages":[
              {"id":"l1","created_at":"2026-01-02T03:04:05Z","type":"gemini","model":"gemini-2.0-flash","tokens":{"totalTokenCount":500}}
            ]}"#,
        );
        let record = &records[0];
        assert_eq!(record.tally, TokenTally::default());
        assert_eq!(record.unclassified_tokens, 500);
        assert_eq!(record.session_id.as_deref(), Some("legacy"));
    }

    #[test]
    fn headless_stats_emit_one_record_per_model_and_a_re_exported_line_id_replaces_its_original() {
        let home = tempfile::tempdir().unwrap();
        let records = read(
            home.path(),
            "headless.jsonl",
            r#"{"type":"init","model":"gemini-2.5-flash","session_id":"sess-h"}
{"type":"gemini","id":"call-1","timestamp":"2026-01-02T03:04:05Z","tokens":{"input":10,"output":2}}
{"type":"gemini","id":"call-1","timestamp":"2026-01-02T03:04:05Z","tokens":{"input":99,"output":2}}
{"timestamp":"2026-01-02T03:05:05Z","result":{"stats":{"models":{"gemini-2.5-flash":{"prompt_tokens":100,"candidates_tokens":20,"thoughts_tokens":5,"cached_tokens":30},"gemini-2.5-pro":{"promptTokenCount":40,"candidatesTokenCount":8}}}}}"#,
        );
        assert_eq!(records.len(), 3);
        assert_eq!(records.iter().filter(|r| r.model == "gemini-2.5-flash").count(), 2);
        assert_eq!(by_dedup(&records, "gemini:line:call-1").tally.input, 99);
        assert!(records.iter().any(|r| r.model == "gemini-2.5-pro" && r.tally.input == 40 && r.tally.output == 8));
    }

    #[test]
    fn stream_json_keeps_the_thoughts_and_tool_tokens_it_does_not_name_and_json_nests_counts_under_tokens() {
        let home = tempfile::tempdir().unwrap();
        let records = read(
            home.path(),
            "runs.jsonl",
            r#"{"type":"result","id":"r1","timestamp":"2026-01-02T03:05:05Z","stats":{"models":{"gemini-2.5-pro":{"total_tokens":145,"input_tokens":100,"output_tokens":20,"cached":30,"input":70}}}}
{"id":"r2","timestamp":"2026-01-02T03:06:05Z","stats":{"models":{"gemini-2.5-flash":{"api":{"totalRequests":1},"tokens":{"input":70,"prompt":100,"candidates":20,"total":145,"cached":30,"thoughts":15,"tool":10}}}}}"#,
        );
        let stream = records.iter().find(|r| r.model == "gemini-2.5-pro").unwrap();
        assert_eq!(stream.tally, TokenTally::new(70, 0, 30, 20));
        assert_eq!(stream.unclassified_tokens, 25);
        let json = records.iter().find(|r| r.model == "gemini-2.5-flash").unwrap();
        assert_eq!(json.tally, TokenTally::new(80, 0, 30, 35));
        assert_eq!(json.unclassified_tokens, 0);
    }

    #[test]
    fn a_prompt_style_key_is_cache_inclusive_even_without_a_total_and_a_net_input_is_not() {
        let home = tempfile::tempdir().unwrap();
        let records = read(
            home.path(),
            "session-prompt.json",
            r#"{"sessionId":"gp","messages":[
              {"id":"p1","timestamp":"2026-01-02T03:04:05Z","type":"gemini","model":"gemini-2.5-pro","tokens":{"prompt":100,"output":10,"cached":40}},
              {"id":"p2","timestamp":"2026-01-02T03:05:05Z","type":"gemini","model":"gemini-2.5-pro","tokens":{"input":100,"output":10,"cached":40}}
            ]}"#,
        );
        assert_eq!(by_dedup(&records, "gemini:session:gp:p1").tally, TokenTally::new(60, 0, 40, 10));
        assert_eq!(by_dedup(&records, "gemini:session:gp:p2").tally, TokenTally::new(100, 0, 40, 10));
    }

    #[test]
    fn a_chat_recording_is_only_accepted_at_its_own_shape_and_a_missing_store_is_empty() {
        assert!(is_chat_path(Path::new("D:/home/.gemini/tmp/abc/chats/chat.json")));
        assert!(!is_chat_path(Path::new("D:/home/.gemini/chats/chat.json")));
        let home = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::Gemini, &Sources::new(home.path()), cache.path(), &Calendar::utc(2), &prices());
        assert!(ledger.days.is_empty());
    }

    #[test]
    fn the_gemini_cli_home_override_moves_the_store() {
        let home = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let sources = Sources::new(home.path()).with_var("GEMINI_CLI_HOME", elsewhere.path());
        assert_eq!(Gemini.inputs(&sources), vec![elsewhere.path().join("tmp")]);
    }
}
