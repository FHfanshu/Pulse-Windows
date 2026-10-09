// Ported from upstream Sources/Pulse/Usage/Readers/CapturedMcodeReader.swift.
//! MiniMax Code: a captured headless stream, the JSONL that `mcode exec --output-format stream-json`
//! writes, in `headless/mcode/*.jsonl`.
//!
//! There is no native local usage file. A user or a wrapper must run the CLI with the stream-json
//! format and capture the output here; Pulse only reads the capture.
//!
//! **Usage is buffered per `turnId` and emitted only when a matching `exec.result` supplies the
//! model.** A stream that never names a model is unusable, and Pulse does not guess one. Every
//! buffered message of a turn is emitted, because `exec.result` names the turn's model but does not
//! say the turn held one request.
//!
//! **A message is merged only by its own identity, never by its counts.** A later line with the same
//! `id` / `messageId` / `responseId` replaces the earlier restatement; a line with no id is its own
//! message, even when its counts equal another's.
//!
//! The file is read line by line: a leading BOM, or a bad line, does not lose the rest.
//!
//! Where it lives on Windows: `TOKSCALE_CONFIG_DIR` when set, else `%USERPROFILE%\.config\tokscale`,
//! then `\headless\mcode`. `WINDOWS-PATH: unverified`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde_json::{Map, Value};

use super::{push_unique, SpendSource};
use crate::spend::logio;
use crate::spend::record::AgentUsageRecord;
use crate::spend::tally::TokenTally;
use crate::spend::transcripts::Sources;

pub struct Mcode;

impl SpendSource for Mcode {
    fn raw(&self) -> &'static str {
        "mcode"
    }
    fn source_id(&self) -> &'static str {
        "mcode"
    }
    fn display_name(&self) -> &'static str {
        "MiniMax Code"
    }
    fn icon(&self) -> Option<&'static str> {
        Some("minimax")
    }
    fn env_vars(&self) -> &'static [&'static str] {
        &["TOKSCALE_CONFIG_DIR", "APPDATA"]
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        let mut roots = Vec::new();
        // WINDOWS-PATH: unverified
        let cache = sources.var("TOKSCALE_CONFIG_DIR").unwrap_or_else(|| sources.home.join(".config").join("tokscale"));
        push_unique(&mut roots, cache.join("headless").join("mcode"));
        push_unique(&mut roots, sources.app_data().join("Pulse").join("UsageImports").join("mcode"));
        roots
    }
    fn records(&self, roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        logio::files(roots, &["jsonl"], &[], &[]).iter().flat_map(|file| records_of(file)).collect()
    }
    fn requires_usage_export(&self) -> bool {
        true
    }
}

/// One assistant message's usage, before a model is known.
struct Usage {
    input: i64,
    output: i64,
    cache_read: i64,
    cache_write: i64,
    /// A stated total with no per-kind split: real tokens that cannot be placed in a bucket.
    unclassified: i64,
    timestamp: Option<DateTime<Utc>>,
    /// The message's own id where the stream states one.
    identity: Option<String>,
}

/// A model-bearing `exec.result`, which is what unlocks a turn's usage.
struct TurnResult {
    session: String,
    model: String,
}

fn records_of(file: &Path) -> Vec<AgentUsageRecord> {
    let mut buffered: HashMap<String, Vec<Usage>> = HashMap::new();
    let mut positions: HashMap<String, HashMap<String, usize>> = HashMap::new();
    let mut results: HashMap<String, TurnResult> = HashMap::new();
    let mut order: Vec<String> = Vec::new();

    for object in objects(file) {
        let Some(kind) = logio::text(object.get("type")) else { continue };
        if kind == "message" {
            let Some(message) = object.get("message").and_then(Value::as_object) else { continue };
            if logio::text(message.get("role")).as_deref() != Some("assistant") {
                continue;
            }
            let Some(turn) = logio::text(message.get("turnId")) else { continue };
            let Some(usage) = message.get("usage").and_then(Value::as_object) else { continue };
            let Some(entry) = usage_of(usage, stream_time(message.get("timestamp")), identity(message)) else { continue };
            if !buffered.contains_key(&turn) {
                order.push(turn.clone());
            }
            merge(entry, &turn, &mut buffered, &mut positions);
        } else if kind == "exec.result" {
            let Some(session) = logio::text(object.get("sessionId")) else { continue };
            let Some(turn) = logio::text(object.get("turnId")) else { continue };
            let Some(model) = object.get("model").and_then(Value::as_object) else { continue };
            if logio::text(model.get("providerId")).is_none() {
                continue;
            }
            let Some(model_id) = logio::text(model.get("modelId")) else { continue };
            results.insert(turn, TurnResult { session, model: model_id });
        }
    }

    let mut records = Vec::new();
    for turn in &order {
        let (Some(result), Some(entries)) = (results.get(turn), buffered.get(turn)) else { continue };
        for (index, entry) in entries.iter().enumerate() {
            let Some(at) = entry.timestamp else { continue };
            let tally = TokenTally::new(entry.input, entry.cache_write, entry.cache_read, entry.output);
            let mut record = AgentUsageRecord::new(at, &result.model, tally).session(&result.session);
            record.unclassified_tokens = entry.unclassified;
            record.deduplication_id = Some(format!(
                "mcode:{}:{turn}:{index}:{}:{}:{}:{}",
                result.session, entry.input, entry.output, entry.cache_read, entry.cache_write
            ));
            records.push(record);
        }
    }
    records
}

/// Appends a message, or replaces the earlier restatement of the same id. Two different messages,
/// with or without equal counts, are always both kept.
fn merge(entry: Usage, turn: &str, buffered: &mut HashMap<String, Vec<Usage>>, positions: &mut HashMap<String, HashMap<String, usize>>) {
    let entries = buffered.entry(turn.to_string()).or_default();
    if let Some(identity) = &entry.identity {
        if let Some(&index) = positions.get(turn).and_then(|map| map.get(identity)) {
            if index < entries.len() {
                entries[index] = entry;
                return;
            }
        }
    }
    let index = entries.len();
    let identity = entry.identity.clone();
    entries.push(entry);
    if let Some(identity) = identity {
        positions.entry(turn.to_string()).or_default().insert(identity, index);
    }
}

/// The message's own id, preferring the most specific field the stream offers.
fn identity(message: &Map<String, Value>) -> Option<String> {
    logio::text(message.get("id")).or_else(|| logio::text(message.get("messageId"))).or_else(|| logio::text(message.get("responseId")))
}

/// The message's usage, or None when it asserts nothing usable.
fn usage_of(object: &Map<String, Value>, timestamp: Option<DateTime<Utc>>, identity: Option<String>) -> Option<Usage> {
    let count = |key: &str| logio::count(object.get(key)).unwrap_or(0);
    let total = count("totalTokens");
    let unclassified_only = |tokens: i64| Usage { input: 0, output: 0, cache_read: 0, cache_write: 0, unclassified: tokens, timestamp, identity: identity.clone() };

    let buckets = ["inputTokens", "outputTokens", "cacheReadTokens", "cacheWriteTokens"];
    if buckets.iter().any(|key| object.contains_key(*key)) {
        let entry = Usage {
            input: count("inputTokens"),
            output: count("outputTokens"),
            cache_read: count("cacheReadTokens"),
            cache_write: count("cacheWriteTokens"),
            unclassified: 0,
            timestamp,
            identity: identity.clone(),
        };
        if entry.input + entry.output + entry.cache_read + entry.cache_write > 0 {
            return Some(entry);
        }
        // Buckets present but empty, with a stated total: the split is not reported, so the tokens
        // are unclassified rather than fake input.
        return (total > 0).then(|| unclassified_only(total));
    }
    // No per-kind counter at all: only a stated total exists.
    (total > 0).then(|| unclassified_only(total))
}

/// Every JSON object line of the capture, tolerating a BOM and a bad line.
fn objects(file: &Path) -> Vec<Map<String, Value>> {
    let Ok(bytes) = std::fs::read(file) else { return Vec::new() };
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&bytes);
    bytes
        .split(|byte| *byte == b'\n')
        .filter_map(|line| match serde_json::from_slice::<Value>(line) {
            Ok(Value::Object(object)) => Some(object),
            _ => None,
        })
        .collect()
}

/// A stream timestamp in seconds or milliseconds: values below the millisecond threshold are seconds.
/// Nothing is inferred from the clock or the file's modification date.
fn stream_time(value: Option<&Value>) -> Option<DateTime<Utc>> {
    let raw = match value? {
        Value::Number(number) => number.as_f64()?,
        Value::String(text) => text.trim().parse::<f64>().ok()?,
        _ => return None,
    };
    if !raw.is_finite() || raw <= 0.0 {
        return None;
    }
    let seconds = if raw < 10_000_000_000.0 { raw } else { raw / 1000.0 };
    DateTime::<Utc>::from_timestamp_micros((seconds * 1_000_000.0).round() as i64)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use chrono::Utc;
    use serde_json::json;

    use super::*;
    use crate::spend::calendar::Calendar;
    use crate::spend::prices::PriceTable;
    use crate::spend::sources::{read_agent, SpendAgent};

    fn message(turn: &str, id: Option<&str>, input: i64, output: i64, at: i64) -> Value {
        let mut message = json!({"role": "assistant", "turnId": turn, "timestamp": at,
                                 "usage": {"inputTokens": input, "outputTokens": output}});
        if let Some(id) = id {
            message["id"] = json!(id);
        }
        json!({"type": "message", "message": message})
    }

    fn result(turn: &str, model: &str) -> Value {
        json!({"type": "exec.result", "sessionId": "sess-1", "turnId": turn,
               "model": {"providerId": "minimax", "modelId": model}})
    }

    fn write(home: &Path, lines: &[Value]) {
        let folder = home.join(".config").join("tokscale").join("headless").join("mcode");
        std::fs::create_dir_all(&folder).unwrap();
        let mut text = String::from("\u{feff}");
        for line in lines {
            text.push_str(&line.to_string());
            text.push('\n');
        }
        text.push_str("this line is not json\n");
        std::fs::write(folder.join("capture.jsonl"), text).unwrap();
    }

    #[test]
    fn usage_is_kept_only_for_a_turn_with_a_model_and_a_bad_line_loses_nothing() {
        let home = tempfile::tempdir().unwrap();
        let now = Utc::now().timestamp();
        write(
            home.path(),
            &[
                message("t1", Some("m1"), 100, 10, now),
                message("t1", Some("m1"), 100, 40, now),
                // Two messages with no id and equal counts stay two.
                message("t1", None, 5, 5, now),
                message("t1", None, 5, 5, now),
                result("t1", "MiniMax-M2"),
                // A turn that never names a model is unusable.
                message("t2", None, 999, 999, now),
            ],
        );
        let cache = tempfile::tempdir().unwrap();
        let prices: PriceTable = HashMap::new();
        let ledger = read_agent(SpendAgent::Mcode, &Sources::new(home.path()), cache.path(), &Calendar::utc(2), &prices);
        // m1 restated: 140 (the later line replaces the earlier); two unidentified 10s: 20.
        assert_eq!(ledger.days.iter().map(|d| d.tokens).sum::<i64>(), 160);
    }

    #[test]
    fn seconds_and_milliseconds_are_both_read_and_a_missing_store_is_empty() {
        assert!(stream_time(Some(&json!(1_700_000_000))).is_some());
        assert_eq!(stream_time(Some(&json!(1_700_000_000_000i64))), stream_time(Some(&json!(1_700_000_000))));
        assert_eq!(stream_time(Some(&json!("nope"))), None);
        let home = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let prices: PriceTable = HashMap::new();
        let ledger = read_agent(SpendAgent::Mcode, &Sources::new(home.path()), cache.path(), &Calendar::utc(2), &prices);
        assert!(ledger.days.is_empty());
    }
}
