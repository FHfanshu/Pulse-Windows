// Ported from upstream Sources/Pulse/Usage/Readers/AmpSessionReader.swift
// and Sources/Pulse/Usage/Readers/SessionLogPaths.swift (amp).
//! Amp's threads, where the same calls are described twice.
//!
//! `~/.local/share/amp/threads/T-*.json`. A thread holds assistant messages with their own usage,
//! and a `usageLedger.events` series. The ledger is the primary record: an assistant message is
//! matched to a ledger event by `toMessageId` first and by equal model plus equal tokens second,
//! and a matched message is **not** emitted again. Only a message with no event stands alone.
//! Emitting both sides would double every call.
//!
//! A message's own time is a ledger event's RFC 3339 stamp. Amp records no time on a message, so
//! a message without a matching stamped event is placed at the thread's own report time
//! (`created`, milliseconds) and marked aggregate, never given a time derived from its id. With no
//! real time at all, the call is not emitted.
//!
//! Where it lives on Windows: `%USERPROFILE%\.local\share\amp\threads` (`XDG_DATA_HOME` moves the
//! `.local\share` part). Unverified on a real PC.

use std::path::PathBuf;

use serde_json::{Map, Value};

use super::SpendSource;
use crate::spend::logio;
use crate::spend::record::AgentUsageRecord;
use crate::spend::tally::TokenTally;
use crate::spend::transcripts::Sources;

pub struct Amp;

impl SpendSource for Amp {
    fn raw(&self) -> &'static str {
        "amp"
    }
    fn source_id(&self) -> &'static str {
        "amp"
    }
    fn display_name(&self) -> &'static str {
        "Amp"
    }
    fn icon(&self) -> Option<&'static str> {
        Some("amp")
    }
    fn env_vars(&self) -> &'static [&'static str] {
        &["XDG_DATA_HOME"]
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        // WINDOWS-PATH: unverified (`~/.local/share` is the macOS and Linux layout carried over)
        vec![sources.xdg_data_home().join("amp").join("threads")]
    }
    fn records(&self, roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        let mut result = Vec::new();
        let mut incomplete = false;
        for file in logio::files(roots, &["json"], &[], &[]) {
            if !file.file_name().is_some_and(|n| n.to_string_lossy().starts_with("T-")) {
                continue;
            }
            let (records, thread_incomplete) = thread(&file);
            result.extend(records);
            incomplete |= thread_incomplete;
        }
        if incomplete {
            for record in &mut result {
                record.is_partial = true;
            }
        }
        result
    }
}

/// One assistant message's usage, matched or not.
struct Call {
    model: String,
    tally: TokenTally,
    message_id: Option<i64>,
}

/// One ledger event.
struct Event {
    timestamp: Option<chrono::DateTime<chrono::Utc>>,
    model: String,
    tally: TokenTally,
    to_message_id: Option<i64>,
    from_message_id: Option<i64>,
    index: usize,
}

/// One thread file: its records, and whether some usage could not be dated or named.
fn thread(path: &std::path::Path) -> (Vec<AgentUsageRecord>, bool) {
    let Some(root) = logio::json(path).and_then(|v| v.as_object().cloned()) else { return (Vec::new(), false) };
    let thread_id = logio::text(root.get("id")).unwrap_or_else(|| {
        path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
    });
    let created = logio::timestamp(root.get("created"), true);

    let (message_calls, dropped_messages) = messages(&root);
    let (events, dropped_events) = ledger_events(&root);
    let mut incomplete = dropped_messages || dropped_events;
    let mut consumed = vec![false; events.len()];
    let mut unmatched: Vec<Call> = Vec::new();

    for call in message_calls {
        let mut matched = false;
        if let Some(id) = call.message_id {
            if let Some(index) = (0..events.len()).find(|&i| !consumed[i] && events[i].to_message_id == Some(id)) {
                consumed[index] = true;
                matched = true;
            }
        }
        if !matched {
            if let Some(index) = (0..events.len())
                .find(|&i| !consumed[i] && events[i].model == call.model && events[i].tally == call.tally)
            {
                consumed[index] = true;
                matched = true;
            }
        }
        if !matched {
            unmatched.push(call);
        }
    }

    let mut records = Vec::new();
    for event in &events {
        let Some(timestamp) = event.timestamp.or(created) else {
            incomplete = true;
            continue;
        };
        let suffix = event
            .to_message_id
            .or(event.from_message_id)
            .map(|id| id.to_string())
            .unwrap_or_else(|| event.index.to_string());
        let mut record = AgentUsageRecord::new(timestamp, &event.model, event.tally.clone())
            .session(&thread_id)
            .aggregate(event.timestamp.is_none());
        record.deduplication_id = Some(format!("amp:{thread_id}:event:{suffix}"));
        records.push(record);
    }

    for call in unmatched {
        let Some(created) = created else {
            incomplete = true;
            continue;
        };
        let suffix = call.message_id.map(|id| id.to_string()).unwrap_or_else(|| "0".to_string());
        let mut record = AgentUsageRecord::new(created, &call.model, call.tally).session(&thread_id).aggregate(true);
        record.deduplication_id = Some(format!("amp:{thread_id}:message:{suffix}"));
        records.push(record);
    }
    (records, incomplete)
}

/// The assistant messages with usage. A message whose usage has no model is dropped and flagged.
fn messages(root: &Map<String, Value>) -> (Vec<Call>, bool) {
    let rows: Vec<Value> = root
        .get("messages")
        .and_then(Value::as_array)
        .filter(|a| a.iter().all(Value::is_object))
        .cloned()
        .unwrap_or_default();
    let mut calls = Vec::new();
    let mut dropped = false;
    for row in &rows {
        if row.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(usage) = row.get("usage").and_then(Value::as_object) else { continue };
        let count = |key: &str| logio::count(usage.get(key)).unwrap_or(0);
        let tally = TokenTally::new(
            count("inputTokens"),
            count("cacheCreationInputTokens"),
            count("cacheReadInputTokens"),
            count("outputTokens"),
        );
        if tally.total() <= 0 {
            continue;
        }
        let Some(model) = logio::text(usage.get("model")) else {
            dropped = true;
            continue;
        };
        calls.push(Call { model, tally, message_id: logio::count(row.get("messageId")) });
    }
    (calls, dropped)
}

/// The ledger's events, in order. An event whose usage has no model is dropped and flagged.
fn ledger_events(root: &Map<String, Value>) -> (Vec<Event>, bool) {
    let rows: Vec<Value> = root
        .get("usageLedger")
        .and_then(|l| l.get("events"))
        .and_then(Value::as_array)
        .filter(|a| a.iter().all(Value::is_object))
        .cloned()
        .unwrap_or_default();
    let mut events = Vec::new();
    let mut dropped = false;
    for (index, row) in rows.iter().enumerate() {
        let empty = Map::new();
        let tokens = row.get("tokens").and_then(Value::as_object).unwrap_or(&empty);
        let count = |key: &str| logio::count(tokens.get(key)).unwrap_or(0);
        let tally = TokenTally::new(
            count("input"),
            count("cacheCreationInputTokens"),
            count("cacheReadInputTokens"),
            count("output"),
        );
        if tally.total() <= 0 {
            continue;
        }
        let Some(model) = logio::text(row.get("model")) else {
            dropped = true;
            continue;
        };
        events.push(Event {
            timestamp: logio::timestamp(row.get("timestamp"), false),
            model,
            tally,
            to_message_id: logio::count(row.get("toMessageId")),
            from_message_id: logio::count(row.get("fromMessageId")),
            index,
        });
    }
    (events, dropped)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn threads(home: &std::path::Path) -> PathBuf {
        home.join(".local/share/amp/threads")
    }

    fn write(path: PathBuf, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn a_matched_message_merges_into_its_ledger_event_and_the_rest_dates_at_the_threads_report() {
        let home = tempfile::tempdir().unwrap();
        write(
            threads(home.path()).join("T-9.json"),
            r#"{"id":"thread-9","created":1700000000000,"messages":[
              {"role":"assistant","messageId":7,"usage":{"model":"gpt-5","inputTokens":100,"outputTokens":10,"cacheReadInputTokens":0,"cacheCreationInputTokens":0,"credits":1.5}},
              {"role":"assistant","messageId":8,"usage":{"model":"gpt-5","inputTokens":50,"outputTokens":5,"cacheReadInputTokens":0,"cacheCreationInputTokens":0}}
            ],"usageLedger":{"events":[
              {"timestamp":"2026-01-02T03:04:05Z","model":"gpt-5","credits":1.5,"tokens":{"input":100,"output":10,"cacheReadInputTokens":0,"cacheCreationInputTokens":0},"operationType":"chat","fromMessageId":6,"toMessageId":7}
            ]}}"#,
        );
        let records = Amp.records(&[threads(home.path())]);
        assert_eq!(records.len(), 2);
        assert!(records.iter().any(|r| r.tally == TokenTally::new(100, 0, 0, 10)));
        let appended = records.iter().find(|r| r.deduplication_id.as_deref() == Some("amp:thread-9:message:8")).unwrap();
        assert_eq!(appended.tally, TokenTally::new(50, 0, 0, 5));
        // The thread's own report time, marked aggregate, not created plus the id.
        assert_eq!(appended.timestamp.timestamp_millis(), 1_700_000_000_000);
        assert!(appended.is_aggregate);
    }

    #[test]
    fn amp_never_derives_a_time_from_a_message_id_and_drops_a_call_with_no_real_time() {
        let home = tempfile::tempdir().unwrap();
        write(
            threads(home.path()).join("T-created.json"),
            r#"{"id":"t-created","created":1700000000000,"messages":[{"role":"assistant","messageId":42,"usage":{"model":"gpt-5","inputTokens":10,"outputTokens":1}}],"usageLedger":{"events":[]}}"#,
        );
        write(
            threads(home.path()).join("T-none.json"),
            r#"{"id":"t-none","messages":[{"role":"assistant","messageId":5,"usage":{"model":"gpt-5","inputTokens":10,"outputTokens":1}}],"usageLedger":{"events":[]}}"#,
        );
        let records = Amp.records(&[threads(home.path())]);
        assert_eq!(records.len(), 1);
        let record = &records[0];
        assert_eq!(record.timestamp.timestamp_millis(), 1_700_000_000_000);
        assert!(record.is_aggregate);
        // The undatable thread's usage is not silently dropped: the readable sibling is flagged partial.
        assert!(record.is_partial);
        assert_eq!(record.deduplication_id.as_deref(), Some("amp:t-created:message:42"));
    }
}
