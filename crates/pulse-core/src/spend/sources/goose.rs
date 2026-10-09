// Ported from upstream Sources/Pulse/Usage/Readers/GooseReader.swift.
//! Goose's session database, `sessions.db`, under its XDG, macOS or legacy Block data directory.
//! The first candidate that exists is read.
//!
//! ```sql
//! sessions(id, model_config_json, provider_name, created_at, total_tokens,
//!          input_tokens, output_tokens, accumulated_total_tokens,
//!          accumulated_input_tokens, accumulated_output_tokens)
//! ```
//!
//! **The accumulated columns are cumulative and are preferred.** A session that grows between
//! reads changes its figures under the same id, so this is one record per session: an aggregate,
//! not a turn. The plain columns are the fallback for a build that wrote no accumulated ones.
//!
//! **The difference is unclassified, never reasoning.** Goose has no reasoning counter and no
//! cache columns. `total - input - output` is kept as unclassified: counted, never priced, never
//! shown as a kind.
//!
//! A session whose `created_at` cannot be read is skipped rather than dated 1970.
//!
//! Where it lives on Windows: the macOS Application Support folder maps to `%APPDATA%`, so the
//! candidates are read from there, from `$XDG_DATA_HOME` (or `~/.local/share`), and from the Block
//! legacy folders. `GOOSE_PATH_ROOT` moves the root.

use std::path::PathBuf;

use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use rusqlite::Connection;
use serde_json::Value;

use super::SpendSource;
use crate::spend::logio;
use crate::spend::record::AgentUsageRecord;
use crate::spend::sqlite;
use crate::spend::tally::TokenTally;
use crate::spend::transcripts::Sources;

pub struct Goose;

impl SpendSource for Goose {
    fn raw(&self) -> &'static str {
        "goose"
    }
    fn source_id(&self) -> &'static str {
        "goose"
    }
    fn display_name(&self) -> &'static str {
        "Goose"
    }
    fn icon(&self) -> Option<&'static str> {
        Some("goose")
    }
    fn env_vars(&self) -> &'static [&'static str] {
        &["GOOSE_PATH_ROOT", "XDG_DATA_HOME", "APPDATA"]
    }
    fn reports_cache_reads(&self) -> bool {
        false
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        let mut candidates = Vec::new();
        if let Some(root) = sources.var("GOOSE_PATH_ROOT") {
            candidates.push(root.join("data").join("sessions").join("sessions.db"));
        }
        // WINDOWS-PATH: unverified (the macOS Application Support folder is %APPDATA% on Windows)
        candidates.push(sources.app_data().join("goose").join("sessions").join("sessions.db"));
        // WINDOWS-PATH: unverified (the XDG layout)
        candidates.push(sources.xdg_data_home().join("goose").join("sessions").join("sessions.db"));
        // WINDOWS-PATH: unverified (legacy Block root)
        candidates.push(sources.app_data().join("Block").join("goose").join("sessions").join("sessions.db"));
        candidates.push(sources.home.join(".local").join("share").join("Block").join("goose").join("sessions").join("sessions.db"));
        candidates
    }
    fn records(&self, roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        let candidates: Vec<&PathBuf> = roots.iter().filter(|p| p.file_name().is_some_and(|n| n == "sessions.db")).collect();
        match candidates.into_iter().find(|p| p.is_file()) {
            Some(database) => read(database),
            None => Vec::new(),
        }
    }
}

fn read(file: &std::path::Path) -> Vec<AgentUsageRecord> {
    let Some(connection) = sqlite::open(file) else { return Vec::new() };
    let mut records = Vec::new();
    if sqlite::columns(&connection, "sessions").is_empty() {
        return records;
    }
    let sql = select(&connection);
    sqlite::each(&connection, &sql, |row| {
        let Some(id) = sqlite::text(row, 0) else { return };
        let Some(model) = sqlite::text(row, 1).and_then(|config| model_in(&config)) else { return };
        let Some(at) = sqlite::text(row, 2).and_then(|text| utc(&text)) else { return };

        // The accumulated columns are cumulative and preferred; the plain ones are the fallback.
        let input = sqlite::count(row, 6).or_else(|| sqlite::count(row, 3)).unwrap_or(0);
        let output = sqlite::count(row, 7).or_else(|| sqlite::count(row, 4)).unwrap_or(0);
        let total = sqlite::count(row, 8).or_else(|| sqlite::count(row, 5)).unwrap_or(0);
        if input <= 0 && output <= 0 && total <= 0 {
            return;
        }
        // A total larger than the kinds it names: the part that cannot be attributed is kept as such.
        let unclassified = (total - (input + output)).max(0);
        let mut record = AgentUsageRecord::new(at, &model, TokenTally { input, output, ..TokenTally::default() })
            .session(&id)
            .unclassified(unclassified)
            .aggregate(true);
        record.deduplication_id = Some(id);
        records.push(record);
    });
    records
}

/// `model_config_json.model_name`, or None for a config that is not an object or a blank name.
fn model_in(config: &str) -> Option<String> {
    let value: Value = serde_json::from_str(config).ok()?;
    let name = value.get("model_name")?.as_str()?;
    (!name.trim().is_empty()).then(|| name.to_string())
}

/// An ISO 8601 timestamp, or Goose's own `yyyy-MM-dd HH:mm:ss` and `yyyy-MM-dd`, both in UTC.
fn utc(text: &str) -> Option<DateTime<Utc>> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(at) = logio::timestamp(Some(&Value::String(trimmed.to_string())), false) {
        return Some(at);
    }
    if let Ok(naive) = NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%d %H:%M:%S") {
        return Some(naive.and_utc());
    }
    NaiveDate::parse_from_str(trimmed, "%Y-%m-%d").ok()?.and_hms_opt(0, 0, 0).map(|naive| naive.and_utc())
}

/// The declared columns in a fixed order, NULL for any an older schema lacks.
fn select(connection: &Connection) -> String {
    sqlite::select(
        connection,
        "sessions",
        &[
            "id",
            "model_config_json",
            "created_at",
            "input_tokens",
            "output_tokens",
            "total_tokens",
            "accumulated_input_tokens",
            "accumulated_output_tokens",
            "accumulated_total_tokens",
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spend::calendar::Calendar;
    use crate::spend::prices::PriceTable;
    use crate::spend::sources::{read_agent, SpendAgent};
    use crate::spend::sqlite::tests::database;

    const SCHEMA: &str = "CREATE TABLE sessions (id TEXT, model_config_json TEXT, provider_name TEXT, created_at TEXT, total_tokens INTEGER, input_tokens INTEGER, output_tokens INTEGER, accumulated_total_tokens INTEGER, accumulated_input_tokens INTEGER, accumulated_output_tokens INTEGER)";

    fn store(root: &std::path::Path, rows: &[&str]) -> PathBuf {
        let folder = root.join(".local").join("share").join("goose").join("sessions");
        std::fs::create_dir_all(&folder).unwrap();
        let file = folder.join("sessions.db");
        let mut sql = vec![SCHEMA];
        sql.extend_from_slice(rows);
        database(&file, &sql);
        file
    }

    fn run(home: &std::path::Path) -> Vec<AgentUsageRecord> {
        Goose.records(&Goose.inputs(&Sources::new(home)))
    }

    #[test]
    fn accumulated_columns_win_and_the_remainder_is_unclassified_not_reasoning() {
        let home = tempfile::tempdir().unwrap();
        store(
            home.path(),
            &[
                r#"INSERT INTO sessions VALUES ('s1','{"model_name":"gpt-x"}','p','2026-09-14 10:00:00',NULL,5,6,NULL,100,20)"#,
                // Only the plain columns: the fallback.
                r#"INSERT INTO sessions VALUES ('s2','{"model_name":"gpt-x"}','p','2026-09-14',60,40,10,NULL,NULL,NULL)"#,
            ],
        );
        let records = run(home.path());
        let s1 = records.iter().find(|r| r.session_id.as_deref() == Some("s1")).unwrap();
        assert_eq!(s1.tally, TokenTally::new(100, 0, 0, 20));
        assert_eq!(s1.unclassified_tokens, 0, "no total reported, so nothing is left over");
        assert!(s1.is_aggregate);
        let s2 = records.iter().find(|r| r.session_id.as_deref() == Some("s2")).unwrap();
        assert_eq!(s2.tally, TokenTally::new(40, 0, 0, 10));
        assert_eq!(s2.unclassified_tokens, 10);
        assert_eq!(s2.timestamp.format("%Y-%m-%d %H:%M").to_string(), "2026-09-14 00:00");
    }

    #[test]
    fn a_row_with_no_model_no_date_or_no_counts_is_skipped() {
        let home = tempfile::tempdir().unwrap();
        store(
            home.path(),
            &[
                r#"INSERT INTO sessions VALUES ('nomodel','{"other":1}','p','2026-09-14 10:00:00',NULL,5,6,NULL,NULL,NULL)"#,
                r#"INSERT INTO sessions VALUES ('nodate','{"model_name":"m"}','p',NULL,NULL,5,6,NULL,NULL,NULL)"#,
                r#"INSERT INTO sessions VALUES ('garbage','{"model_name":"m"}','p','not a date',NULL,5,6,NULL,NULL,NULL)"#,
                r#"INSERT INTO sessions VALUES ('nocounts','{"model_name":"m"}','p','2026-09-14 10:00:00',0,0,0,0,0,0)"#,
            ],
        );
        assert!(run(home.path()).is_empty());
    }

    #[test]
    fn a_missing_store_is_an_empty_ledger_and_the_root_override_moves_it() {
        let home = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::Goose, &Sources::new(home.path()), cache.path(), &Calendar::utc(2), &PriceTable::new());
        assert!(ledger.days.is_empty());

        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("data").join("sessions");
        std::fs::create_dir_all(&folder).unwrap();
        database(
            &folder.join("sessions.db"),
            &[SCHEMA, r#"INSERT INTO sessions VALUES ('o','{"model_name":"m"}','p','2026-09-14 10:00:00',NULL,3,4,NULL,NULL,NULL)"#],
        );
        let sources = Sources::new(home.path()).with_var("GOOSE_PATH_ROOT", root.path());
        let records = Goose.records(&Goose.inputs(&sources));
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].tally.total(), 7);
    }
}
