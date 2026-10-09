// Ported from upstream Sources/Pulse/Usage/Readers/ZCodeReader.swift.
//! ZCode's transcripts and its v2 usage database. Two stores are read: JSONL under `projects/`, and
//! the `model_usage` table of `cli/db/db.sqlite`.
//!
//! **JSONL:** a reported `total` settles whether input and output already contain the cache and
//! reasoning counts. The cache overlap is removed from input only when the total says input
//! includes it, and reasoning is folded into output only when the total counts it separately. A
//! total that fits neither shape is kept whole as unclassified.
//!
//! **Database:** the schema documents `input_tokens` as cache-inclusive and `output_tokens` as
//! reasoning-inclusive, so the cache overlap is always removed from input and `reasoning_tokens` is
//! never added a second time. A `computed_total_tokens` beyond `input + output` is kept as
//! unclassified.
//!
//! **A total with no split is not input.** A legacy line with no usage block contributes nothing:
//! its counts would have to be estimated from string lengths, which are not reported tokens.
//!
//! Verified on Windows: `%USERPROFILE%\.zcode\cli\db\db.sqlite`.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::SpendSource;
use crate::spend::editorlog::{self, Object, Reported, UsageParts};
use crate::spend::logio;
use crate::spend::record::AgentUsageRecord;
use crate::spend::sqlite;
use crate::spend::tally::TokenTally;
use crate::spend::transcripts::Sources;

pub struct ZCode;

impl SpendSource for ZCode {
    fn raw(&self) -> &'static str {
        "zcode"
    }
    fn source_id(&self) -> &'static str {
        "zcode"
    }
    fn display_name(&self) -> &'static str {
        "ZCode"
    }
    fn has_captured_validation(&self) -> bool {
        true
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        let base = sources.home.join(".zcode");
        vec![base.join("projects"), base.join("cli").join("db").join("db.sqlite")]
    }
    fn records(&self, roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        let mut records = Vec::new();
        for file in logio::files(roots, &["jsonl"], &["db.sqlite"], &[]) {
            if file.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                records.extend(jsonl(&file));
            } else if file.file_name().and_then(|n| n.to_str()) == Some("db.sqlite") {
                records.extend(database(&file));
            }
        }
        records.sort_by_key(|r| r.timestamp);
        records
    }
}

/// A string column read as a count, as the Swift reader parsed it.
fn text_count(text: Option<String>) -> Option<i64> {
    logio::count(text.map(Value::String).as_ref())
}

/// A string column read as an epoch in milliseconds, or an ISO instant.
fn text_timestamp(text: Option<String>) -> Option<DateTime<Utc>> {
    logio::timestamp(text.map(Value::String).as_ref(), true)
}

// MARK: - JSONL transcripts

fn jsonl(file: &Path) -> Vec<AgentUsageRecord> {
    let stem = file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let path = file.to_string_lossy().into_owned();
    let mut records = Vec::new();

    for (index, row) in logio::json_lines(file).enumerate() {
        let Some(timestamp) = logio::timestamp(row.get("timestamp"), true) else { continue };
        let Some(model) = editorlog::model_id(logio::text(row.get("model"))) else { continue };

        let empty = Object::new();
        // The `usage` object wins when it yields a split; otherwise the `token_usage` spelling is tried.
        let mut parts = usage(row.get("usage").and_then(Value::as_object).unwrap_or(&empty));
        if parts.tally.total() + parts.unclassified == 0 {
            parts = usage(row.get("token_usage").and_then(Value::as_object).unwrap_or(&empty));
        }
        if parts.tally.total() + parts.unclassified <= 0 {
            continue;
        }

        let session = logio::text(row.get("sessionId")).unwrap_or_else(|| stem.clone());
        let mut record = AgentUsageRecord::new(timestamp, &model, parts.tally).session(&session).unclassified(parts.unclassified);
        record.deduplication_id = Some(format!("zcode:{path}:{index}:{session}:{}", timestamp.timestamp_millis()));
        records.push(record);
    }
    records
}

fn usage(object: &Object) -> UsageParts {
    editorlog::combine(&Reported {
        input: editorlog::first_count(object, &["input_tokens", "prompt_tokens", "inputTokens"]),
        output: editorlog::first_count(object, &["output_tokens", "completion_tokens", "outputTokens"]),
        cache_read: editorlog::first_count(object, &["input_cache_read", "cache_read_tokens", "cacheReadTokens"]),
        cache_write: editorlog::first_count(object, &["input_cache_creation", "cache_write_tokens", "cacheCreationTokens"]),
        reasoning: editorlog::first_count(object, &["reasoningTokens", "reasoning_tokens"]),
        total: editorlog::first_count(object, &["totalTokens", "total_tokens"]),
        exclusive_input: None,
        input_excludes_cache: false,
    })
}

// MARK: - The v2 database

fn database(file: &Path) -> Vec<AgentUsageRecord> {
    let Some(connection) = sqlite::open(file) else { return Vec::new() };
    let columns = sqlite::columns(&connection, "model_usage");
    if columns.is_empty() {
        return Vec::new();
    }
    let session_columns = sqlite::columns(&connection, "session");
    let has_session = columns.contains("session_id") && session_columns.contains("id");

    // Only the columns the schema actually has are selected, so a legacy table still reads.
    let prefix = if has_session { "mu." } else { "" };
    let mut labels: Vec<&str> = Vec::new();
    let mut expressions: Vec<String> = Vec::new();
    for name in [
        "id", "session_id", "model_id", "started_at", "completed_at", "input_tokens", "output_tokens", "reasoning_tokens",
        "cache_read_input_tokens", "cache_creation_input_tokens", "computed_total_tokens",
    ] {
        if columns.contains(name) {
            labels.push(name);
            expressions.push(format!("{prefix}{name}"));
        }
    }
    if has_session {
        for name in ["directory", "path", "title", "slug"] {
            if session_columns.contains(name) {
                labels.push(name);
                expressions.push(format!("s.{name}"));
            }
        }
    }
    let from = if has_session { " FROM model_usage mu LEFT JOIN session s ON s.id = mu.session_id" } else { " FROM model_usage" };
    let sql = format!("SELECT {}{from}", expressions.join(", "));

    let mut found = Vec::new();
    let path = file.to_string_lossy().into_owned();
    sqlite::each(&connection, &sql, |row| {
        let column = |label: &str| labels.iter().position(|l| *l == label).and_then(|index| sqlite::text(row, index));
        let count = |label: &str| text_count(column(label));

        let started = text_timestamp(column("started_at"));
        let completed = text_timestamp(column("completed_at"));
        let (Some(timestamp), Some(session_id)) = (started.or(completed), column("session_id")) else { return };

        let raw_input = count("input_tokens").unwrap_or(0);
        let raw_output = count("output_tokens").unwrap_or(0);
        let cache_read = count("cache_read_input_tokens").unwrap_or(0);
        let cache_write = count("cache_creation_input_tokens").unwrap_or(0);
        let computed = count("computed_total_tokens");

        // The schema documents input as cache-inclusive and output as reasoning-inclusive, so the cache
        // overlap comes out of input and reasoning is never added a second time.
        let fresh_input = (raw_input - cache_read - cache_write).max(0);
        let unclassified = computed.filter(|c| *c > raw_input + raw_output).map_or(0, |c| c - (raw_input + raw_output));

        let identifier = column("id").unwrap_or_else(|| format!("{session_id}#{}", started.map_or(0, |s| s.timestamp_millis())));
        let model = editorlog::model_id(column("model_id")).unwrap_or_else(|| "auto".to_string());
        let project = column("directory").or_else(|| column("path")).filter(|p| !p.trim().is_empty());

        let mut record = AgentUsageRecord::new(timestamp, &model, TokenTally::new(fresh_input, cache_write, cache_read, raw_output))
            .session(&session_id)
            .unclassified(unclassified);
        record.project = project;
        record.title = column("title").filter(|s| !s.trim().is_empty());
        record.session_name = column("slug").filter(|s| !s.trim().is_empty());
        record.deduplication_id = Some(format!("zcode:{path}:{identifier}"));
        found.push(record);
    });
    found
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::spend::calendar::Calendar;
    use crate::spend::prices::PriceTable;
    use crate::spend::sources::{read_agent, SpendAgent};
    use crate::spend::sqlite::tests::database as make_database;

    const MIDNIGHT_MS: i64 = 1_789_344_000_000;

    fn transcript(home: &Path, name: &str) -> PathBuf {
        home.join(".zcode").join("projects").join("p").join(format!("{name}.jsonl"))
    }

    fn write(path: &Path, lines: &[Value]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let text: Vec<String> = lines.iter().map(Value::to_string).collect();
        std::fs::write(path, text.join("\n")).unwrap();
    }

    #[test]
    fn a_jsonl_total_settles_the_split_and_a_token_usage_spelling_is_the_fallback() {
        let home = tempfile::tempdir().unwrap();
        write(
            &transcript(home.path(), "sess"),
            &[
                // total = input + output: the cache is inside input.
                json!({"timestamp":MIDNIGHT_MS,"model":"zai/glm-x","sessionId":"s1","usage":{"input_tokens":100,"output_tokens":10,"input_cache_read":30,"totalTokens":110}}),
                // total = every kind: reasoning is its own bucket.
                json!({"timestamp":MIDNIGHT_MS+1,"model":"glm-x","sessionId":"s1","usage":{"input_tokens":100,"output_tokens":10,"reasoning_tokens":5,"input_cache_read":30,"total_tokens":145}}),
                // The alternate spelling, used only when `usage` yields nothing.
                json!({"timestamp":MIDNIGHT_MS+2,"model":"glm-x","sessionId":"s1","usage":{},"token_usage":{"prompt_tokens":7,"completion_tokens":3}}),
                // A total that fits neither shape: unclassified, not input.
                json!({"timestamp":MIDNIGHT_MS+3,"model":"glm-x","sessionId":"s1","usage":{"input_tokens":4,"output_tokens":1,"total_tokens":50}}),
                // A legacy line with no usage at all, and one with no time: nothing.
                json!({"timestamp":MIDNIGHT_MS+4,"model":"glm-x","sessionId":"s1","content":"text only"}),
                json!({"model":"glm-x","usage":{"input_tokens":9}}),
            ],
        );
        let mut records = jsonl(&transcript(home.path(), "sess"));
        records.sort_by_key(|r| r.timestamp);
        assert_eq!(records.len(), 4);
        assert_eq!(records[0].tally, TokenTally::new(70, 0, 30, 10));
        assert_eq!(records[0].model, "glm-x");
        assert_eq!(records[1].tally, TokenTally::new(100, 0, 30, 15));
        assert_eq!(records[2].tally, TokenTally::new(7, 0, 0, 3));
        // The named kinds stay where they are; the total's remainder beyond them is unclassified.
        assert_eq!(records[3].tally, TokenTally::new(4, 0, 0, 1));
        assert_eq!(records[3].unclassified_tokens, 45);
    }

    #[test]
    fn the_v2_database_removes_the_cache_from_input_and_keeps_a_computed_remainder_as_unclassified() {
        let home = tempfile::tempdir().unwrap();
        let db = home.path().join(".zcode").join("cli").join("db").join("db.sqlite");
        std::fs::create_dir_all(db.parent().unwrap()).unwrap();
        make_database(
            &db,
            &[
                "CREATE TABLE session (id TEXT, directory TEXT, path TEXT)",
                "CREATE TABLE model_usage (id TEXT, session_id TEXT, model_id TEXT, started_at INTEGER, completed_at INTEGER, input_tokens INTEGER, output_tokens INTEGER, reasoning_tokens INTEGER, cache_read_input_tokens INTEGER, cache_creation_input_tokens INTEGER, computed_total_tokens INTEGER)",
                "INSERT INTO session VALUES ('s1','C:\\Users\\me\\code\\Pulse','')",
                // input 100 includes 30 cache read and 20 cache write: fresh 50; output 10 includes reasoning; computed 200 leaves 90 unclassified.
                &format!("INSERT INTO model_usage VALUES ('m1','s1','zai/glm-x',{MIDNIGHT_MS},NULL,100,10,5,30,20,200)"),
                // The same id again is one call.
                &format!("INSERT INTO model_usage VALUES ('m1','s1','zai/glm-x',{MIDNIGHT_MS},NULL,100,10,5,30,20,200)"),
                // Completed time only: used when there is no start. A row with no session is skipped.
                &format!("INSERT INTO model_usage VALUES ('m2','s1',NULL,NULL,{},7,2,0,0,0,NULL)", MIDNIGHT_MS + 1000),
                "INSERT INTO model_usage VALUES ('m3',NULL,'glm',1789344000000,NULL,5,5,0,0,0,NULL)",
            ],
        );
        // Rows are not deduplicated here; the shared builder folds the repeated id.
        let mut records = database(&db);
        records.sort_by_key(|r| r.timestamp);
        assert_eq!(records.len(), 3);
        assert_eq!(records[0].tally, TokenTally::new(50, 20, 30, 10));
        assert_eq!(records[0].unclassified_tokens, 200 - 110);
        assert_eq!(records[0].model, "glm-x");
        assert_eq!(records[0].project.as_deref(), Some("C:\\Users\\me\\code\\Pulse"));
        assert_eq!(records[0].deduplication_id, records[1].deduplication_id);
        // A completed-only row is dated by its completion, and a model-less row is "auto".
        assert_eq!(records[2].tally, TokenTally::new(7, 0, 0, 2));
        assert_eq!(records[2].model, "auto");
    }

    #[test]
    fn a_legacy_table_without_the_computed_column_still_reads_and_the_store_is_missing_then_empty() {
        let home = tempfile::tempdir().unwrap();
        let db = home.path().join(".zcode").join("cli").join("db").join("db.sqlite");
        std::fs::create_dir_all(db.parent().unwrap()).unwrap();
        make_database(
            &db,
            &[
                "CREATE TABLE model_usage (id TEXT, session_id TEXT, model_id TEXT, started_at INTEGER, input_tokens INTEGER, output_tokens INTEGER)",
                &format!("INSERT INTO model_usage VALUES ('old','s9','m',{MIDNIGHT_MS},12,3)"),
            ],
        );
        let records = database(&db);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].tally, TokenTally::new(12, 0, 0, 3));

        let empty = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        assert!(read_agent(SpendAgent::ZCode, &Sources::new(empty.path()), cache.path(), &Calendar::utc(2), &PriceTable::new()).days.is_empty());
    }
    #[test]
    fn session_titles_and_projects_survive_a_legacy_session_schema_without_path() {
        let home = tempfile::tempdir().unwrap();
        let db = home.path().join(".zcode/cli/db/db.sqlite");
        std::fs::create_dir_all(db.parent().unwrap()).unwrap();
        make_database(&db,&[
            "CREATE TABLE session (id TEXT, directory TEXT, title TEXT, slug TEXT)",
            "CREATE TABLE model_usage (id TEXT, session_id TEXT, model_id TEXT, started_at INTEGER, input_tokens INTEGER, output_tokens INTEGER)",
            "INSERT INTO session VALUES ('s','C:\\code\\Pulse','Fix dock','dock')",
            &format!("INSERT INTO model_usage VALUES ('m','s','model',{MIDNIGHT_MS},12,3)"),
        ]);
        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::ZCode,&Sources::new(home.path()),cache.path(),&Calendar::utc(2),&PriceTable::new());
        assert_eq!(ledger.all_time().tokens,15);
        assert_eq!(ledger.sessions.len(),1);
        assert_eq!(ledger.sessions[0].title.as_deref(),Some("Fix dock"));
        assert_eq!(ledger.sessions[0].name,"dock");
        assert_eq!(ledger.sessions[0].project.as_ref().unwrap().name,"Pulse");
    }

}
