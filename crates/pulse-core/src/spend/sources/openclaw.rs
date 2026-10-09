// Ported from upstream Sources/Pulse/Usage/Readers/OpenClawSessionReader.swift
// and Sources/Pulse/Usage/Readers/SessionLogPaths.swift (openClaw).
//! OpenClaw's two stores and its legacy names.
//!
//! The current store is SQLite, `<agentId>\agent\openclaw-agent.sqlite`, whose `transcript_events`
//! rows hold each event as JSON. The older store is a folder of `*.jsonl` files (with
//! `deleted` and `reset` archives) indexed by `sessions.json`. Both use the same event shape, and
//! `openclaw doctor --fix` imports the logs into the database while leaving the originals, so one
//! call can be present in both. The record identity is therefore **cross-store**: the event id, the
//! timestamp and the input and output counts, which the SQLite row, the retained JSONL line and a
//! `/fork` copy all share. That keeps one call from being counted twice.
//!
//! `reasoningTokens` is a subset of `output`, so it is not a bucket of its own and is never added a
//! second time (it stands in for output only when output is absent). Rows that describe transcript
//! plumbing rather than model output (`openclaw-transcript`, `delivery-mirror`, `gateway-injected`)
//! are skipped.
//!
//! Not read: the Codex app-server rollouts mirrored under an agent (`codex-home`, `cli-auth`), a
//! different format, and Zstandard archives (`.zst`), which have no decoder here.
//!
//! Where it lives on Windows: `%USERPROFILE%\.openclaw\agents`, and the legacy `.clawdbot`,
//! `.moltbot` and `.moldbot` folders beside it. Unverified on a real PC.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde_json::{Map, Value};

use super::SpendSource;
use crate::spend::logio;
use crate::spend::record::AgentUsageRecord;
use crate::spend::sqlite;
use crate::spend::tally::TokenTally;
use crate::spend::transcripts::Sources;

pub struct OpenClaw;

impl SpendSource for OpenClaw {
    fn raw(&self) -> &'static str {
        "openClaw"
    }
    fn source_id(&self) -> &'static str {
        "openclaw"
    }
    fn display_name(&self) -> &'static str {
        "OpenClaw"
    }
    fn icon(&self) -> Option<&'static str> {
        Some("openclaw")
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        // WINDOWS-PATH: unverified (the `%USERPROFILE%` dot-folders are the macOS layout carried over)
        let home = &sources.home;
        vec![
            home.join(".openclaw").join("agents"),
            home.join(".clawdbot"),
            home.join(".moltbot"),
            home.join(".moldbot"),
        ]
    }
    fn records(&self, roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        let (mut records, sqlite_incomplete) = sqlite_records(roots);
        let (jsonl, jsonl_incomplete) = jsonl_records(roots);
        records.extend(jsonl);
        if sqlite_incomplete || jsonl_incomplete {
            for record in &mut records {
                record.is_partial = true;
            }
        }
        records
    }
}

/// A path with forward slashes, so the store's own `/sessions/` checks hold on Windows.
fn forward(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// A text value that is present and not blank.
fn clean(value: Option<String>) -> Option<String> {
    value.map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

/// The model bookkeeping an event carries, if any: `(model, provider)`.
fn model_bookkeeping(row: &Map<String, Value>) -> Option<(Option<String>, Option<String>)> {
    match row.get("type").and_then(Value::as_str) {
        Some("model_change") => Some((logio::text(row.get("modelId")), logio::text(row.get("provider")))),
        Some("custom") => {
            if row.get("customType").and_then(Value::as_str) != Some("model-snapshot") {
                return None;
            }
            let data = row.get("data").and_then(Value::as_object)?;
            Some((logio::text(data.get("modelId")), logio::text(data.get("provider"))))
        }
        _ => None,
    }
}

/// The camelCase usage object: the tally, and the unclassified remainder. A bare total with no
/// named kind is carried as unclassified, never poured into input.
fn usage(object: &Map<String, Value>) -> (TokenTally, i64) {
    let input = logio::count(object.get("input"));
    let output = logio::count(object.get("output"));
    let cache_read = logio::count(object.get("cacheRead"));
    let cache_write = logio::count(object.get("cacheWrite"));
    let reasoning = logio::count(object.get("reasoningTokens")).unwrap_or(0);
    let total = logio::count(object.get("totalTokens"));

    let any_known = input.is_some() || output.is_some() || cache_read.is_some() || cache_write.is_some();
    let tally = TokenTally::new(
        input.unwrap_or(0),
        cache_write.unwrap_or(0),
        cache_read.unwrap_or(0),
        output.unwrap_or(reasoning),
    );
    let Some(total) = total else { return (tally, 0) };
    if !any_known {
        return (TokenTally::default(), total);
    }
    let remainder = total - tally.total();
    (tally, remainder.max(0))
}

/// Rows that describe the mirror rather than a call the model made.
fn is_artifact(row: &Map<String, Value>, message: &Map<String, Value>) -> bool {
    if logio::text(row.get("api")).as_deref() == Some("openclaw-transcript") {
        return true;
    }
    if logio::text(message.get("api")).as_deref() == Some("openclaw-transcript") {
        return true;
    }
    let provider = logio::text(message.get("provider")).or_else(|| logio::text(row.get("provider")));
    let model = logio::text(message.get("model")).or_else(|| logio::text(row.get("model")));
    provider.as_deref() == Some("openclaw") && matches!(model.as_deref(), Some("delivery-mirror") | Some("gateway-injected"))
}

/// One assistant usage event.
struct MessageEvent {
    event_id: Option<String>,
    model: Option<String>,
    timestamp: Option<DateTime<Utc>>,
    tally: TokenTally,
    unclassified: i64,
}

fn message_event(row: &Map<String, Value>) -> Option<MessageEvent> {
    if row.get("type").and_then(Value::as_str) != Some("message") {
        return None;
    }
    let message = row.get("message").and_then(Value::as_object)?;
    if message.get("role").and_then(Value::as_str) != Some("assistant") {
        return None;
    }
    if is_artifact(row, message) {
        return None;
    }
    let usage_object = message.get("usage").and_then(Value::as_object)?;
    let (tally, unclassified) = usage(usage_object);
    if tally.total() <= 0 && unclassified <= 0 {
        return None;
    }
    Some(MessageEvent {
        event_id: logio::text(row.get("id")),
        model: logio::text(message.get("model")),
        timestamp: logio::timestamp(message.get("timestamp"), true).or_else(|| logio::timestamp(row.get("timestamp"), true)),
        tally,
        unclassified,
    })
}

/// The record for one event. The identity is the same in both stores.
fn record(session: Option<&str>, event_id: &str, timestamp: DateTime<Utc>, model: &str, event: &MessageEvent) -> AgentUsageRecord {
    let mut record = AgentUsageRecord::new(timestamp, model, event.tally.clone()).unclassified(event.unclassified);
    record.session_id = session.map(str::to_string);
    let milliseconds = (timestamp.timestamp_micros() as f64 / 1000.0).round() as i64;
    record.deduplication_id = Some(format!(
        "openclaw:{event_id}:{milliseconds}:{}:{}",
        event.tally.input, event.tally.output
    ));
    record
}

/// Session metadata: `session_windows`, or the older `sessions` table when that is empty or absent.
fn session_metadata(connection: &rusqlite::Connection) -> HashMap<String, (Option<String>, Option<String>)> {
    fn read(connection: &rusqlite::Connection, sql: &str, map: &mut HashMap<String, (Option<String>, Option<String>)>) {
        sqlite::each(connection, sql, |row| {
            if let Some(id) = sqlite::text(row, 0) {
                map.insert(id, (sqlite::text(row, 2), sqlite::text(row, 1)));
            }
        });
    }
    let mut map = HashMap::new();
    read(connection, "SELECT session_id, model_provider, model FROM session_windows", &mut map);
    if map.is_empty() {
        read(connection, "SELECT session_id, model_provider, model FROM sessions", &mut map);
    }
    map
}

/// The records of every `openclaw-agent.sqlite` under the roots, and whether one was incomplete.
fn sqlite_records(roots: &[PathBuf]) -> (Vec<AgentUsageRecord>, bool) {
    let mut records = Vec::new();
    let mut incomplete = false;
    for database in logio::files(roots, &["sqlite"], &[], &[]) {
        if file_name(&database) != "openclaw-agent.sqlite" {
            continue;
        }
        let Some(connection) = sqlite::open(&database) else { continue };
        let metadata = session_metadata(&connection);
        let mut carried: HashMap<String, (Option<String>, Option<String>)> = HashMap::new();

        sqlite::each(
            &connection,
            r#"SELECT session_id, seq, event_json FROM transcript_events
               WHERE event_json LIKE '%"usage"%' OR event_json LIKE '%"model_change"%'
               OR event_json LIKE '%model-snapshot%' ORDER BY session_id, seq"#,
            |row| {
                let (Some(session), Some(json)) = (sqlite::text(row, 0), sqlite::text(row, 2)) else { return };
                let Ok(Value::Object(object)) = serde_json::from_str::<Value>(&json) else { return };
                let seq = sqlite::integer(row, 1);
                if let Some(update) = model_bookkeeping(&object) {
                    carried.insert(session.clone(), update);
                }
                let Some(event) = message_event(&object) else { return };

                let carried_model = carried.get(&session).and_then(|c| c.0.clone());
                let meta_model = metadata.get(&session).and_then(|m| m.0.clone());
                let model = clean(event.model.clone().or(carried_model).or(meta_model));
                let (Some(model), Some(timestamp)) = (model, event.timestamp) else {
                    // Real usage with no model or no locatable time.
                    incomplete = true;
                    return;
                };
                let event_id = event.event_id.clone().unwrap_or_else(|| format!("seq-{seq}"));
                records.push(record(Some(&session), &event_id, timestamp, &model, &event));
            },
        );
    }
    (records, incomplete)
}

/// The legacy `sessions.json` registry: the basename of each `sessionFile`, to its `sessionId`.
fn session_registry(roots: &[PathBuf]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for file in logio::files(roots, &[], &["sessions.json"], &[]) {
        let Some(root) = logio::json(&file).and_then(|v| v.as_object().cloned()) else { continue };
        for value in root.values() {
            let Some(entry) = value.as_object() else { continue };
            let (Some(session_id), Some(session_file)) = (logio::text(entry.get("sessionId")), logio::text(entry.get("sessionFile"))) else {
                continue;
            };
            let base = session_file.rsplit(['/', '\\']).next().unwrap_or(&session_file).to_string();
            map.insert(base, session_id);
        }
    }
    map
}

/// A legacy transcript: a `.jsonl` file under a `sessions` folder (or the imported originals under
/// `session-sqlite-import-archive`), not a Codex mirror and not a Zstandard archive.
fn is_openclaw_jsonl(path: &Path) -> bool {
    let name = file_name(path);
    if !name.contains(".jsonl") || name.ends_with(".zst") {
        return false;
    }
    let text = forward(path);
    if !(text.contains("/sessions/") || text.contains("/session-sqlite-import-archive/")) {
        return false;
    }
    !(text.contains("/codex-home/") || text.contains("/cli-auth/"))
}

/// The session id of a legacy transcript named by its file: the name before `.jsonl`.
fn url_session_id(path: &Path) -> String {
    let name = file_name(path);
    match name.find(".jsonl") {
        Some(at) => name[..at].to_string(),
        None => path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
    }
}

/// The records of the legacy JSONL store, and whether one was incomplete.
fn jsonl_records(roots: &[PathBuf]) -> (Vec<AgentUsageRecord>, bool) {
    let registry = session_registry(roots);
    let mut records = Vec::new();
    let mut incomplete = false;

    for file in logio::files(roots, &[], &[], &[]).into_iter().filter(|p| is_openclaw_jsonl(p)) {
        let session = registry.get(&file_name(&file)).cloned().unwrap_or_else(|| url_session_id(&file));
        let mut carried: (Option<String>, Option<String>) = (None, None);

        for (index, row) in logio::json_lines(&file).enumerate() {
            let row = match row { Value::Object(object) => object, _ => continue };
            if let Some(update) = model_bookkeeping(&row) {
                carried = update;
            }
            let Some(event) = message_event(&row) else { continue };
            let model = clean(event.model.clone().or_else(|| carried.0.clone()));
            let (Some(model), Some(timestamp)) = (model, event.timestamp) else {
                incomplete = true;
                continue;
            };
            let event_id = event.event_id.clone().unwrap_or_else(|| format!("line-{index}"));
            records.push(record(Some(&session), &event_id, timestamp, &model, &event));
        }
    }
    (records, incomplete)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spend::calendar::Calendar;
    use crate::spend::prices::{ModelPrice, PriceTable};
    use crate::spend::sources::{read_agent, SpendAgent};
    use crate::spend::sqlite::tests::database;

    fn prices() -> PriceTable {
        HashMap::from([("gpt-5".to_string(), ModelPrice::new(1.0, 10.0, Some(0.1), Some(1.0), Some("GPT-5")))])
    }

    fn agent_root(home: &Path) -> PathBuf {
        home.join(".openclaw/agents/ag1/agent")
    }

    #[test]
    fn the_sqlite_store_reads_its_session_metadata_and_carried_model_changes_and_reasoning_is_not_added_twice() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(agent_root(home.path())).unwrap();
        database(
            &agent_root(home.path()).join("openclaw-agent.sqlite"),
            &[
                "CREATE TABLE session_windows (session_id TEXT PRIMARY KEY, session_key TEXT, previous_session_id TEXT, created_at INTEGER, updated_at INTEGER, model_provider TEXT, model TEXT, agent_harness_id TEXT)",
                "CREATE TABLE transcript_events (session_id TEXT, seq INTEGER, event_json TEXT, created_at INTEGER, PRIMARY KEY (session_id, seq))",
                "INSERT INTO session_windows VALUES ('s1','k1',NULL,1700000000000,1700000000000,'openai','gpt-5','h1')",
                r#"INSERT INTO transcript_events VALUES ('s1',1,'{"type":"message","id":"e1","message":{"role":"assistant","timestamp":1700000000000,"usage":{"input":100,"output":50,"cacheRead":0,"cacheWrite":0,"totalTokens":150,"reasoningTokens":20}}}',1700000000000)"#,
                r#"INSERT INTO transcript_events VALUES ('s1',2,'{"type":"message","api":"openclaw-transcript","id":"e2","message":{"role":"assistant","timestamp":1700000001000,"usage":{"input":9999,"output":9999}}}',1700000001000)"#,
                r#"INSERT INTO transcript_events VALUES ('s1',3,'{"type":"model_change","id":"e3","modelId":"helper","provider":"openai"}',1700000002000)"#,
                r#"INSERT INTO transcript_events VALUES ('s1',4,'{"type":"message","id":"e4","message":{"role":"assistant","timestamp":1700000003000,"usage":{"input":10,"output":1}}}',1700000003000)"#,
            ],
        );
        let roots = OpenClaw.inputs(&Sources::new(home.path()));
        let records = OpenClaw.records(&roots);
        assert_eq!(records.len(), 2);
        let first = records.iter().find(|r| r.deduplication_id.as_deref().is_some_and(|d| d.starts_with("openclaw:e1:"))).unwrap();
        assert_eq!(first.model, "gpt-5");
        assert_eq!(first.tally, TokenTally::new(100, 0, 0, 50));
        let carried = records.iter().find(|r| r.deduplication_id.as_deref().is_some_and(|d| d.starts_with("openclaw:e4:"))).unwrap();
        assert_eq!(carried.model, "helper");
        assert_eq!(carried.session_id.as_deref(), Some("s1"));
    }

    #[test]
    fn the_same_event_in_sqlite_and_a_legacy_jsonl_folds_to_one() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(agent_root(home.path())).unwrap();
        database(
            &agent_root(home.path()).join("openclaw-agent.sqlite"),
            &[
                "CREATE TABLE transcript_events (session_id TEXT, seq INTEGER, event_json TEXT, created_at INTEGER, PRIMARY KEY (session_id, seq))",
                r#"INSERT INTO transcript_events VALUES ('s1',1,'{"type":"message","id":"evt-1","message":{"role":"assistant","model":"gpt-5","timestamp":1700000000000,"usage":{"input":10,"output":1,"cacheRead":0,"cacheWrite":0,"totalTokens":11}}}',1700000000000)"#,
            ],
        );
        let sessions = home.path().join(".openclaw/agents/ag1/sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(
            sessions.join("legacy.jsonl"),
            r#"{"type":"message","id":"evt-1","message":{"role":"assistant","model":"gpt-5","timestamp":1700000000000,"usage":{"input":10,"output":1,"cacheRead":0,"cacheWrite":0,"totalTokens":11}}}"#,
        )
        .unwrap();

        let roots = OpenClaw.inputs(&Sources::new(home.path()));
        let records = OpenClaw.records(&roots);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].deduplication_id, records[1].deduplication_id);

        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::OpenClaw, &Sources::new(home.path()), cache.path(), &Calendar::utc(2), &prices());
        assert_eq!(ledger.days.iter().map(|d| d.tokens).sum::<i64>(), 11);
    }

    #[test]
    fn a_missing_store_is_empty_and_the_legacy_names_are_all_read() {
        let home = tempfile::tempdir().unwrap();
        let roots = OpenClaw.inputs(&Sources::new(home.path()));
        assert_eq!(roots.len(), 4);
        assert!(OpenClaw.records(&roots).is_empty());
    }

    #[test]
    fn codex_mirrors_and_archives_are_not_openclaw_events() {
        assert!(is_openclaw_jsonl(Path::new("C:/h/.openclaw/agents/a/sessions/x.jsonl")));
        assert!(!is_openclaw_jsonl(Path::new("C:/h/.openclaw/agents/a/sessions/x.jsonl.zst")));
        assert!(!is_openclaw_jsonl(Path::new("C:/h/.openclaw/agents/a/agent/codex-home/sessions/x.jsonl")));
        assert!(!is_openclaw_jsonl(Path::new("C:/h/.openclaw/agents/a/agent/cli-auth/sessions/x.jsonl")));
    }
}
