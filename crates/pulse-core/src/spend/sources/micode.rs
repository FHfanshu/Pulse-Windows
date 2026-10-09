// Ported from upstream Sources/Pulse/Usage/Readers/MicodeReader.swift.
//! MiMo Code's OpenCode-shaped database, `mimocode*.db` under the XDG data directory, unioned with the
//! Orca hook's shared copy.
//!
//! ```sql
//! message(id TEXT, session_id TEXT, data TEXT)   -- data is JSON
//! session(id, directory)                          -- optional, on older DBs
//! ```
//!
//! The assistant message's JSON is the shape OpenCode and Kilo write: `modelID`, `providerID`,
//! `tokens.{input,output,reasoning,cache{read,write}}` and `time.created`. Both epochs are tolerated
//! (milliseconds on current builds, seconds on older ones). Each message is an increment. Reasoning is
//! folded into output once, by the same rule OpenCode's reader uses.
//!
//! The store's own cost is ignored. A message's identity is its embedded `id` when it has one, **not
//! namespaced by database**, so the same message in two copies folds to one; without one, the row id,
//! namespaced by the database path, is the identity.
//!
//! Where it lives on Windows: `$XDG_DATA_HOME\mimocode` (or `%USERPROFILE%\.local\share\mimocode`), and the
//! Orca hook copy under `%APPDATA%\orca` (the macOS Application Support folder). The Orca path is unverified.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use super::opencode::output;
use super::{clamped, epoch, number, SpendSource};
use crate::spend::logio;
use crate::spend::record::AgentUsageRecord;
use crate::spend::sqlite;
use crate::spend::tally::TokenTally;
use crate::spend::transcripts::Sources;

pub struct Micode;

impl SpendSource for Micode {
    fn raw(&self) -> &'static str {
        "micode"
    }
    fn source_id(&self) -> &'static str {
        "micode"
    }
    fn display_name(&self) -> &'static str {
        "MiMo Code"
    }
    fn env_vars(&self) -> &'static [&'static str] {
        &["XDG_DATA_HOME", "APPDATA"]
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        vec![
            // WINDOWS-PATH: unverified (the XDG layout)
            sources.xdg_data_home().join("mimocode"),
            // WINDOWS-PATH: unverified (the Orca hook's shared copy, under the Application Support folder)
            sources.app_data().join("orca").join("mimocode-hooks").join("shared").join("data"),
        ]
    }
    fn records(&self, roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        logio::files(roots, &["db"], &[], &[])
            .iter()
            .filter(|file| {
                let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                name.starts_with("mimocode") && name.ends_with(".db")
            })
            .flat_map(|file| read(file))
            .collect()
    }
}

fn read(file: &Path) -> Vec<AgentUsageRecord> {
    let Some(connection) = sqlite::open(file) else { return Vec::new() };
    if sqlite::columns(&connection, "message").is_empty() {
        return Vec::new();
    }
    let directories = directories(&connection);
    let mut records = Vec::new();
    sqlite::each(&connection, "SELECT id, session_id, data FROM message", |row| {
        let (Some(row_id), Some(session_column), Some(data)) = (sqlite::text(row, 0), sqlite::text(row, 1), sqlite::text(row, 2)) else {
            return;
        };
        let Ok(root) = serde_json::from_str::<Value>(&data) else { return };
        let Some(payload) = root.as_object() else { return };
        if payload.get("role").and_then(Value::as_str) != Some("assistant") {
            return;
        }
        let Some(model) = text(payload.get("modelID")) else { return };

        let Some(time) = payload.get("time").and_then(Value::as_object) else { return };
        let Some(at) = time.get("created").and_then(number).and_then(epoch) else { return };

        let counts = payload.get("tokens").and_then(Value::as_object).cloned().unwrap_or_default();
        let clean = sanitized(&counts);
        let tally = TokenTally {
            input: clamped(counts.get("input")),
            cache_write: clamped(counts.get("cache").and_then(|c| c.get("write"))),
            cache_read: clamped(counts.get("cache").and_then(|c| c.get("read"))),
            // Reasoning is billed as output and counted once.
            output: output(&clean, false),
            ..TokenTally::default()
        };
        if tally.total() <= 0 {
            return;
        }

        let session = text(payload.get("sessionID")).or_else(|| text(payload.get("session_id"))).unwrap_or(session_column.clone());
        let workspace = payload
            .get("path")
            .and_then(Value::as_object)
            .and_then(|p| text(p.get("root")))
            .or_else(|| directories.get(&session_column).cloned());

        let mut record = AgentUsageRecord::new(at, &model, tally).session(&session);
        record.project = workspace;
        // An embedded id is the product's own identity, not namespaced by database.
        record.deduplication_id = Some(match text(payload.get("id")) {
            Some(embedded) => embedded,
            None => format!("micode:{}:{row_id}", file.display()),
        });
        records.push(record);
    });
    records
}

/// The token counts with every number clamped the way the readers clamp, so the shared OpenCode rule
/// sees the same figures.
fn sanitized(counts: &Map<String, Value>) -> Map<String, Value> {
    let mut clean = Map::new();
    for key in ["input", "output", "reasoning", "total"] {
        clean.insert(key.to_string(), Value::from(clamped(counts.get(key))));
    }
    let mut cache = Map::new();
    let source = counts.get("cache");
    for key in ["read", "write"] {
        cache.insert(key.to_string(), Value::from(clamped(source.and_then(|c| c.get(key)))));
    }
    clean.insert("cache".to_string(), Value::Object(cache));
    clean
}

/// `session.id` to its `directory`, where the older schema keeps it.
fn directories(connection: &rusqlite::Connection) -> HashMap<String, String> {
    let mut rows = HashMap::new();
    if !sqlite::columns(connection, "session").contains("directory") {
        return rows;
    }
    sqlite::each(connection, "SELECT id, directory FROM session", |row| {
        if let (Some(id), Some(directory)) = (sqlite::text(row, 0), sqlite::text(row, 1)) {
            rows.insert(id, directory);
        }
    });
    rows
}

fn text(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).filter(|s| !s.trim().is_empty()).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::spend::calendar::Calendar;
    use crate::spend::prices::PriceTable;
    use crate::spend::sources::{read_agent, SpendAgent};
    use crate::spend::sqlite::tests::database;

    const SCHEMA: [&str; 2] = [
        "CREATE TABLE message (id TEXT, session_id TEXT, data TEXT)",
        "CREATE TABLE session (id TEXT, directory TEXT)",
    ];

    fn assistant(id: Option<&str>, created: i64, tokens: Value) -> String {
        let mut message = json!({"role": "assistant", "modelID": "mimo-v2.5", "time": {"created": created}, "tokens": tokens});
        if let Some(id) = id {
            message["id"] = json!(id);
        }
        message.to_string().replace('\'', "''")
    }

    fn db(home: &Path, rows: Vec<String>) -> PathBuf {
        let folder = home.join(".local").join("share").join("mimocode");
        std::fs::create_dir_all(&folder).unwrap();
        let file = folder.join("mimocode.db");
        let mut sql: Vec<String> = SCHEMA.iter().map(|s| s.to_string()).collect();
        sql.push("INSERT INTO session VALUES ('s1','C:\\Users\\me\\code\\Pulse')".into());
        sql.extend(rows);
        database(&file, &sql.iter().map(String::as_str).collect::<Vec<_>>());
        file
    }

    #[test]
    fn counts_are_read_with_reasoning_once_and_both_epochs_are_dated_and_costs_ignored() {
        let home = tempfile::tempdir().unwrap();
        let rows = vec![
            // Milliseconds; reasoning 10 beside output 40 with no total: kept beside (output + reasoning).
            format!("INSERT INTO message VALUES ('r1','s1','{}')", assistant(Some("e1"), 1_789_372_800_000, json!({"input": 100, "output": 40, "reasoning": 10, "cache": {"read": 5, "write": 2}, "total": 0, "cost": 9.9}))),
            // Seconds, a folded total (total = input + output + cache): reasoning already inside output.
            format!("INSERT INTO message VALUES ('r2','s1','{}')", assistant(Some("e2"), 1_789_372_800, json!({"input": 50, "output": 20, "reasoning": 5, "cache": {"read": 0, "write": 0}, "total": 70}))),
            // The same embedded id in a second database: one message.
            // A user message and a message with no time: nothing.
            format!("INSERT INTO message VALUES ('u','s1','{}')", json!({"role": "user", "modelID": "x", "time": {"created": 1789372800000i64}, "tokens": {"input": 9}})),
            format!("INSERT INTO message VALUES ('z','s1','{}')", json!({"role": "assistant", "modelID": "x", "tokens": {"input": 9}})),
        ];
        db(home.path(), rows);

        let records = Micode.records(&Micode.inputs(&Sources::new(home.path())));
        assert_eq!(records.len(), 2);
        let first = records.iter().find(|r| r.deduplication_id.as_deref() == Some("e1")).unwrap();
        assert_eq!(first.tally, TokenTally::new(100, 2, 5, 50));
        assert_eq!(first.timestamp.timestamp(), 1_789_372_800);
        assert_eq!(first.project.as_deref(), Some("C:\\Users\\me\\code\\Pulse"));
        let second = records.iter().find(|r| r.deduplication_id.as_deref() == Some("e2")).unwrap();
        assert_eq!(second.tally, TokenTally::new(50, 0, 0, 20));
    }

    #[test]
    fn a_message_in_two_databases_is_one_message_and_a_row_without_an_id_is_namespaced() {
        let home = tempfile::tempdir().unwrap();
        let row = format!("INSERT INTO message VALUES ('r1','s1','{}')", assistant(Some("same"), 1_789_372_800, json!({"input": 10, "output": 1})));
        db(home.path(), vec![row.clone()]);
        let orca = home.path().join("AppData").join("Roaming").join("orca").join("mimocode-hooks").join("shared").join("data");
        std::fs::create_dir_all(&orca).unwrap();
        let mut sql: Vec<String> = SCHEMA.iter().map(|s| s.to_string()).collect();
        sql.push(row);
        sql.push(format!("INSERT INTO message VALUES ('bare','s1','{}')", assistant(None, 1_789_372_800, json!({"input": 3, "output": 0}))));
        database(&orca.join("mimocode-orca.db"), &sql.iter().map(String::as_str).collect::<Vec<_>>());

        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::Micode, &Sources::new(home.path()), cache.path(), &Calendar::utc(2), &PriceTable::new());
        assert_eq!(ledger.days.iter().map(|d| d.tokens).sum::<i64>(), 11 + 3, "the shared id counts once, the id-less row counts");
    }

    #[test]
    fn a_missing_store_is_an_empty_ledger() {
        let home = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::Micode, &Sources::new(home.path()), cache.path(), &Calendar::utc(2), &PriceTable::new());
        assert!(ledger.days.is_empty());
    }
}
