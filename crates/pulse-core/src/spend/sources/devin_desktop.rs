// Ported from upstream Sources/Pulse/Usage/Readers/DevinDesktopReader.swift.
//! Devin Desktop's ACP event captures: `*.ndjson` under the app's `User/acp-events` folder. The CLI's
//! session database (`devin.rs`) is read only as a **lookup**, and as the store whose counted sessions
//! a Desktop capture can mirror.
//!
//! **Two shapes, told apart by evidence.** The canonical ACP `usage_update` carries its figures under
//! `notification._meta`: `inputTokens` is the complete prompt **including** `cachedReadTokens`, output
//! accumulates per step, and the cached buckets overwrite. One aggregate record is emitted per file:
//! fresh input is `inputTokens - cachedReadTokens`, output is the summed output, and the cache read and
//! write are the last values reported. A capture with no `_meta` figures falls back to a usage object
//! under the metadata locations and emits **one record per metric-bearing event**.
//!
//! **The database supplies identity, not extra tokens on this route.** Its `sessions.title` maps to
//! `{id, model, working_directory}`, so a Desktop file whose name is an unrelated UUID recovers a stable
//! session id, model and workspace. A title held by more than one session is ambiguous and ignored. A
//! capture matched to a session the CLI counts is a mirror of that usage and is excluded.
//!
//! `reasoning` is never reported here. A missing timestamp is skipped, not filled from the modification
//! date. `adaptive` is a routing mode, not a model, and an unresolved file is `unknown`.
//!
//! Presence: the CLI database is watched only once a Desktop `acp-events` folder exists, because its own
//! existence is not evidence of Desktop.
//!
//! Where it lives on Windows: `%APPDATA%\Devin\User\acp-events` and `%USERPROFILE%\.config\devin\User\acp-events`.
//! Unverified on a PC.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use super::devin::DevinCli;
use super::{flexible, SpendSource};
use crate::spend::logio;
use crate::spend::record::AgentUsageRecord;
use crate::spend::sqlite;
use crate::spend::tally::TokenTally;
use crate::spend::transcripts::Sources;

pub struct DevinDesktop;

impl SpendSource for DevinDesktop {
    fn raw(&self) -> &'static str {
        "devinDesktop"
    }
    fn source_id(&self) -> &'static str {
        "devin-desktop"
    }
    fn display_name(&self) -> &'static str {
        "Devin Desktop"
    }
    fn icon(&self) -> Option<&'static str> {
        Some("devin")
    }
    fn env_vars(&self) -> &'static [&'static str] {
        &["APPDATA"]
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        // WINDOWS-PATH: unverified
        let events = vec![
            sources.app_data().join("Devin").join("User").join("acp-events"),
            // WINDOWS-PATH: unverified
            sources.home.join(".config").join("devin").join("User").join("acp-events"),
        ];
        let mut roots = events.clone();
        // The CLI database is the lookup, and is watched only once a Desktop event folder exists.
        if events.iter().any(|folder| folder.is_dir()) {
            roots.push(sources.home.join(".local").join("share").join("devin").join("cli").join("sessions.db"));
            roots.push(sources.app_data().join("devin").join("cli").join("sessions.db"));
        }
        roots
    }
    fn records(&self, roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        let lookup = sessions(roots);
        logio::files(roots, &["ndjson"], &[], &[]).iter().flat_map(|file| read(file, &lookup)).collect()
    }
}

/// A CLI session with its title.
#[derive(Clone)]
struct Session {
    id: String,
    model: Option<String>,
    directory: Option<String>,
    has_counted_usage: bool,
}

/// `sessions.title` to the sessions that carry it. A title held by more than one session is ambiguous,
/// and `resolve` ignores it.
fn sessions(roots: &[PathBuf]) -> HashMap<String, Vec<Session>> {
    let mut table: HashMap<String, Vec<Session>> = HashMap::new();
    for file in logio::files(roots, &[], &["sessions.db"], &[]) {
        let Some(connection) = sqlite::open(&file) else { continue };
        let columns = sqlite::columns(&connection, "sessions");
        if !columns.contains("id") || !columns.contains("title") {
            continue;
        }
        let model = if columns.contains("model") { "model" } else { "NULL" };
        let directory = if columns.contains("working_directory") { "working_directory" } else { "NULL" };
        let counted = if is_devin_cli_store(&file) { counted_sessions(&file) } else { HashSet::new() };
        let sql = format!("SELECT id, title, {model}, {directory} FROM sessions");
        sqlite::each(&connection, &sql, |row| {
            let (Some(id), Some(title)) = (sqlite::text(row, 0), sqlite::text(row, 1)) else { return };
            let title = title.trim().to_string();
            if title.is_empty() {
                return;
            }
            table.entry(title).or_default().push(Session {
                model: sqlite::text(row, 2).filter(|m| !m.trim().is_empty()),
                directory: sqlite::text(row, 3).filter(|d| !d.trim().is_empty()),
                has_counted_usage: counted.contains(&id),
                id,
            });
        });
    }
    table
}

/// The CLI's own database: `.../devin/cli/sessions.db`.
fn is_devin_cli_store(file: &Path) -> bool {
    let parts: Vec<String> = file.components().rev().take(3).map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    parts.len() == 3 && parts[0] == "sessions.db" && parts[1] == "cli" && parts[2] == "devin"
}

/// The sessions whose usage the CLI counts: every session the CLI's own reader turns into a record.
fn counted_sessions(file: &Path) -> HashSet<String> {
    DevinCli.records(&[file.to_path_buf()]).into_iter().filter_map(|record| record.session_id).collect()
}

fn read(file: &Path, lookup: &HashMap<String, Vec<Session>>) -> Vec<AgentUsageRecord> {
    let mut title: Option<String> = None;
    let mut records = Vec::new();

    // The canonical aggregate across the file.
    let mut latest_input: Option<i64> = None;
    let mut latest_read = 0i64;
    let mut latest_write = 0i64;
    let mut summed_output = 0i64;
    let mut model: Option<String> = None;
    let mut timestamp: Option<chrono::DateTime<chrono::Utc>> = None;

    for (index, line) in logio::json_lines(file).enumerate() {
        let Some(notification) = line.get("notification").and_then(Value::as_object) else { continue };

        if notification.get("sessionUpdate").and_then(Value::as_str) == Some("session_info_update") {
            if let Some(stated) = logio::text(notification.get("title")) {
                title = Some(stated);
            }
        }

        if let Some(canonical) = canonical(notification) {
            // `inputTokens` is the complete prompt, so it overwrites; the cached buckets overwrite too;
            // output is summed per step.
            if let Some(input) = canonical.input {
                latest_input = Some(input);
            }
            if let Some(read) = canonical.cache_read {
                latest_read = read;
            }
            if let Some(write) = canonical.cache_write {
                latest_write = write;
            }
            summed_output += canonical.output;
            model = logio::text(notification.get("notification_model")).or_else(|| logio::text(canonical.meta.get("cognition.ai/model"))).or(model);
            timestamp = canonical.timestamp.or(timestamp);
            continue;
        }

        // A non-canonical event: one record per event, keyed by its line index.
        if let Some(record) = legacy(notification, index, file, title.as_deref(), lookup) {
            records.push(record);
        }
    }

    if let Some(input) = latest_input {
        let fresh = (input - latest_read).max(0);
        let tally = TokenTally { input: fresh, cache_write: latest_write, cache_read: latest_read, output: summed_output, ..TokenTally::default() };
        if let Some(at) = timestamp.filter(|_| tally.total() > 0) {
            let resolved = resolve(title.as_deref(), lookup);
            if resolved.is_some_and(|s| s.has_counted_usage) {
                return records;
            }
            let mut record = AgentUsageRecord::new(at, &model_name(model.or_else(|| resolved.and_then(|s| s.model.clone()))), tally)
                .session(&session_id(resolved, file))
                .aggregate(true);
            record.project = resolved.and_then(|s| s.directory.clone());
            record.deduplication_id = Some(format!("devin-desktop:{}:usage", file.display()));
            records.push(record);
        }
    }
    records
}

/// The canonical ACP usage under `notification._meta`, or None when the event does not carry it.
struct Canonical<'a> {
    input: Option<i64>,
    cache_read: Option<i64>,
    cache_write: Option<i64>,
    output: i64,
    timestamp: Option<chrono::DateTime<chrono::Utc>>,
    meta: &'a Map<String, Value>,
}

fn canonical(notification: &Map<String, Value>) -> Option<Canonical<'_>> {
    if notification.get("sessionUpdate").and_then(Value::as_str) != Some("usage_update") {
        return None;
    }
    let meta = notification.get("_meta").and_then(Value::as_object)?;
    let prefix = "cognition.ai/";
    let input = logio::count(meta.get(&format!("{prefix}inputTokens")));
    let cache_read = logio::count(meta.get(&format!("{prefix}cachedReadTokens")));
    let cache_write = logio::count(meta.get(&format!("{prefix}cachedWriteTokens")));
    let output = logio::count(meta.get(&format!("{prefix}outputTokens")));
    if input.is_none() && cache_read.is_none() && cache_write.is_none() && output.is_none() {
        return None;
    }
    Some(Canonical {
        input,
        cache_read,
        cache_write,
        output: output.unwrap_or(0),
        timestamp: flexible(notification.get("created_at")),
        meta,
    })
}

/// The legacy shape: a usage object under one of the metadata locations, with underscored field names.
fn legacy(
    notification: &Map<String, Value>,
    index: usize,
    file: &Path,
    title: Option<&str>,
    lookup: &HashMap<String, Vec<Session>>,
) -> Option<AgentUsageRecord> {
    let content = notification.get("content").and_then(Value::as_object);
    let content_metadata = content.and_then(|c| c.get("metadata")).and_then(Value::as_object);
    let metadata = notification.get("metadata").and_then(Value::as_object);

    let candidates: Vec<Option<&Map<String, Value>>> = vec![
        content_metadata.and_then(|m| m.get("metrics")).and_then(Value::as_object),
        metadata.and_then(|m| m.get("metrics")).and_then(Value::as_object),
        notification.get("metrics").and_then(Value::as_object),
        content_metadata,
        metadata,
    ];
    let keys = ["input_tokens", "output_tokens", "cache_read_tokens", "cache_creation_tokens"];

    for usage in candidates.into_iter().flatten() {
        if !keys.iter().any(|k| logio::count(usage.get(*k)).is_some()) {
            continue;
        }
        let tally = TokenTally {
            input: logio::count(usage.get("input_tokens")).unwrap_or(0),
            cache_write: logio::count(usage.get("cache_creation_tokens")).unwrap_or(0),
            cache_read: logio::count(usage.get("cache_read_tokens")).unwrap_or(0),
            output: logio::count(usage.get("output_tokens")).unwrap_or(0),
            ..TokenTally::default()
        };
        if tally.total() <= 0 {
            continue;
        }

        let at = flexible(content_metadata.and_then(|m| m.get("created_at")))
            .or_else(|| flexible(metadata.and_then(|m| m.get("created_at"))))
            .or_else(|| flexible(notification.get("created_at")))
            .or_else(|| flexible(notification.get("timestamp")))?;

        let hinted = logio::text(content_metadata.and_then(|m| m.get("generation_model")))
            .or_else(|| logio::text(metadata.and_then(|m| m.get("generation_model"))))
            .or_else(|| logio::text(notification.get("_meta").and_then(|m| m.get("cognition.ai/model"))));
        let resolved = resolve(title, lookup);
        if resolved.is_some_and(|s| s.has_counted_usage) {
            return None;
        }

        let mut record = AgentUsageRecord::new(at, &model_name(hinted.or_else(|| resolved.and_then(|s| s.model.clone()))), tally)
            .session(&session_id(resolved, file));
        record.project = resolved.and_then(|s| s.directory.clone());
        record.deduplication_id = Some(format!("devin-desktop:{}:{index}", file.display()));
        return Some(record);
    }
    None
}

/// The CLI session whose title is this file's, when exactly one matches.
fn resolve<'a>(title: Option<&str>, lookup: &'a HashMap<String, Vec<Session>>) -> Option<&'a Session> {
    let matches = lookup.get(title?)?;
    (matches.len() == 1).then(|| &matches[0])
}

/// The matched CLI session's id, else the file's stem.
fn session_id(resolved: Option<&Session>, file: &Path) -> String {
    match resolved {
        Some(session) => session.id.clone(),
        None => file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
    }
}

/// `adaptive` is a routing mode, not a model; an unresolved file is `unknown`, an unpriced name, rather
/// than a concrete model that would be priced at the wrong rate.
fn model_name(value: Option<String>) -> String {
    match value {
        Some(model) if model != "adaptive" => model,
        _ => "unknown".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::spend::calendar::Calendar;
    use crate::spend::prices::PriceTable;
    use crate::spend::sources::{read_agent, SpendAgent};
    use crate::spend::sqlite::tests::database;

    fn events_folder(home: &Path) -> PathBuf {
        let folder = home.join(".config").join("devin").join("User").join("acp-events");
        std::fs::create_dir_all(&folder).unwrap();
        folder
    }

    fn cli_store(home: &Path, rows: &[&str]) -> PathBuf {
        let folder = home.join(".local").join("share").join("devin").join("cli");
        std::fs::create_dir_all(&folder).unwrap();
        let file = folder.join("sessions.db");
        let mut sql = vec![
            "CREATE TABLE sessions (id TEXT, title TEXT, model TEXT, working_directory TEXT)".to_string(),
            "CREATE TABLE message_nodes (session_id TEXT, chat_message TEXT, created_at INTEGER)".to_string(),
        ];
        sql.extend(rows.iter().map(|r| r.to_string()));
        database(&file, &sql.iter().map(String::as_str).collect::<Vec<_>>());
        file
    }

    fn lines(values: &[Value]) -> String {
        values.iter().map(|v| v.to_string()).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn the_canonical_usage_is_one_aggregate_per_file_with_fresh_input_and_summed_output() {
        let home = tempfile::tempdir().unwrap();
        let folder = events_folder(home.path());
        let body = lines(&[
            json!({"notification": {"sessionUpdate": "session_info_update", "title": "Fix ring"}}),
            json!({"notification": {"sessionUpdate": "usage_update", "created_at": "2026-09-14T10:00:00Z", "notification_model": "gpt-z",
                "_meta": {"cognition.ai/inputTokens": 100, "cognition.ai/cachedReadTokens": 30, "cognition.ai/cachedWriteTokens": 5, "cognition.ai/outputTokens": 7}}}),
            json!({"notification": {"sessionUpdate": "usage_update", "created_at": "2026-09-14T10:05:00Z",
                "_meta": {"cognition.ai/inputTokens": 150, "cognition.ai/cachedReadTokens": 60, "cognition.ai/cachedWriteTokens": 6, "cognition.ai/outputTokens": 3, "cognition.ai/model": "other"}}}),
        ]);
        std::fs::write(folder.join("0f3c.ndjson"), body).unwrap();

        let records = DevinDesktop.records(&DevinDesktop.inputs(&Sources::new(home.path())));
        assert_eq!(records.len(), 1);
        let record = &records[0];
        // fresh = 150 - 60; output 7 + 3; cache write and read are the last values reported.
        assert_eq!(record.tally, TokenTally::new(90, 6, 60, 10));
        assert!(record.is_aggregate);
        // The latest model hint wins: the second event's own metadata names "other".
        assert_eq!(record.model, "other");
    }

    #[test]
    fn a_legacy_event_is_one_record_each_and_a_mirrored_session_is_excluded() {
        let home = tempfile::tempdir().unwrap();
        let folder = events_folder(home.path());
        // The CLI counts session "cli-1" for the title "Mirrored": its Desktop capture is a mirror.
        let message = json!({"role": "assistant", "metadata": {"generation_model": "m", "metrics": {"input_tokens": 4, "output_tokens": 1}}}).to_string().replace('\'', "''");
        cli_store(
            home.path(),
            &[
                "INSERT INTO sessions VALUES ('cli-1','Mirrored','m','C:\\Users\\me\\code\\Pulse')",
                "INSERT INTO sessions VALUES ('cli-2','Other','m','C:\\Users\\me\\code\\Other')",
                &format!("INSERT INTO message_nodes VALUES ('cli-1','{message}',1789372800)"),
            ],
        );
        std::fs::write(
            folder.join("mirror.ndjson"),
            lines(&[
                json!({"notification": {"sessionUpdate": "session_info_update", "title": "Mirrored"}}),
                json!({"notification": {"content": {"metadata": {"created_at": 1_789_372_800, "metrics": {"input_tokens": 9, "output_tokens": 2}}}}}),
            ]),
        )
        .unwrap();
        std::fs::write(
            folder.join("other.ndjson"),
            lines(&[
                json!({"notification": {"sessionUpdate": "session_info_update", "title": "Other"}}),
                json!({"notification": {"content": {"metadata": {"created_at": 1_789_372_801, "generation_model": "adaptive", "metrics": {"input_tokens": 9, "output_tokens": 2}}}}}),
                json!({"notification": {"content": {"metadata": {"created_at": 1_789_372_802, "metrics": {"input_tokens": 1, "output_tokens": 0}}}}}),
                json!({"notification": {"content": {"metadata": {"metrics": {"input_tokens": 1}}}}}),
            ]),
        )
        .unwrap();

        let records = DevinDesktop.records(&DevinDesktop.inputs(&Sources::new(home.path())));
        assert!(records.iter().all(|r| !r.deduplication_id.as_deref().unwrap_or("").contains("mirror.ndjson")), "the mirror is excluded");
        let other: Vec<&AgentUsageRecord> = records.iter().filter(|r| r.session_id.as_deref() == Some("cli-2")).collect();
        assert_eq!(other.len(), 2, "two legacy events with a time; the one with no time is skipped");
        // The event that names "adaptive" is unknown, not a model; the next one falls back to the CLI session's model.
        let adaptive = other.iter().find(|r| r.timestamp.timestamp() == 1_789_372_801).unwrap();
        assert_eq!(adaptive.model, "unknown", "adaptive is a routing mode, not a model");
        let plain = other.iter().find(|r| r.timestamp.timestamp() == 1_789_372_802).unwrap();
        assert_eq!(plain.model, "m");
        assert!(other.iter().all(|r| r.project.as_deref() == Some("C:\\Users\\me\\code\\Other")));
    }

    #[test]
    fn no_events_folder_means_no_records_even_with_a_cli_database_and_a_missing_store_is_empty() {
        let home = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::DevinDesktop, &Sources::new(home.path()), cache.path(), &Calendar::utc(2), &PriceTable::new());
        assert!(ledger.days.is_empty());

        cli_store(home.path(), &[]);
        let sources = Sources::new(home.path());
        assert!(!DevinDesktop.inputs(&sources).iter().any(|p| p.ends_with("sessions.db")), "the CLI database is not watched without a Desktop folder");
    }
}
