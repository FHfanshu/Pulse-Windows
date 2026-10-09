// Ported from upstream Sources/Pulse/Usage/Readers/UnslothReader.swift.
//! Unsloth Studio's database, `studio.db`, under `$UNSLOTH_STUDIO_HOME` (else `~/.unsloth/studio`).
//! Two tables carry real counters.
//!
//! **`chat_messages` is the measured chat path.** An assistant message's `metadata_json` holds
//! `$.contextUsage` (prompt, completion, total, cached, cache-write and reasoning counters). The
//! counters are normalized so the four kinds add up to the reported total: a cache read is bounded by
//! the prompt, a cache write by what the read left, and the total is raised to at least
//! `prompt + completion`. Reasoning is inside the completion and is counted once, as output.
//!
//! **`api_usage_events` is the measured API path**, with fewer buckets: it has no cache and no
//! reasoning columns, so those stay zero.
//!
//! The store's own cost and its provider route are not used. Rows with no readable time or no message
//! identity are skipped, never dated 1970.
//!
//! Where it lives on Windows: `%UNSLOTH_STUDIO_HOME%`, else `%USERPROFILE%\.unsloth\studio`.
//! Unverified on a PC.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{clamped, epoch, SpendSource};
use crate::spend::logio;
use crate::spend::record::AgentUsageRecord;
use crate::spend::sqlite;
use crate::spend::tally::TokenTally;
use crate::spend::transcripts::Sources;

pub struct Unsloth;

impl SpendSource for Unsloth {
    fn raw(&self) -> &'static str {
        "unsloth"
    }
    fn source_id(&self) -> &'static str {
        "unsloth"
    }
    fn display_name(&self) -> &'static str {
        "Unsloth"
    }
    fn icon(&self) -> Option<&'static str> {
        Some("unsloth")
    }
    fn env_vars(&self) -> &'static [&'static str] {
        &["UNSLOTH_STUDIO_HOME"]
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        // WINDOWS-PATH: unverified (%USERPROFILE%\.unsloth\studio)
        let root = sources.var("UNSLOTH_STUDIO_HOME").unwrap_or_else(|| sources.home.join(".unsloth").join("studio"));
        vec![root.join("studio.db")]
    }
    fn records(&self, roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        logio::files(roots, &[], &["studio.db"], &[]).iter().flat_map(|file| read(file)).collect()
    }
}

fn read(file: &Path) -> Vec<AgentUsageRecord> {
    let Some(connection) = sqlite::open(file) else { return Vec::new() };
    let mut records = chat(&connection);
    records.extend(api(&connection));
    records
}

/// The chat path: one record per assistant message.
fn chat(connection: &rusqlite::Connection) -> Vec<AgentUsageRecord> {
    let messages = sqlite::columns(connection, "chat_messages");
    let threads = sqlite::columns(connection, "chat_threads");
    let has = |set: &std::collections::HashSet<String>, names: &[&str]| names.iter().all(|n| set.contains(*n));
    if !has(&messages, &["id", "thread_id", "role", "metadata_json", "created_at"]) || !has(&threads, &["id", "model_id"]) {
        return Vec::new();
    }

    let mut records = Vec::new();
    let sql = "SELECT m.id, m.thread_id, m.metadata_json, m.created_at, t.model_id \
               FROM chat_messages m JOIN chat_threads t ON m.thread_id = t.id \
               WHERE m.role = 'assistant'";
    sqlite::each(connection, sql, |row| {
        let Some(message_id) = sqlite::text(row, 0).filter(|id| !id.trim().is_empty()) else { return };
        let Some(at) = sqlite::number(row, 3).filter(|v| *v > 0.0).and_then(epoch) else { return };
        let thread = sqlite::text(row, 1);
        let thread_model = sqlite::text(row, 4);
        let Some(metadata) = sqlite::text(row, 2).and_then(|text| serde_json::from_str::<Value>(&text).ok()) else { return };
        let Some(object) = metadata.as_object() else { return };

        let usage = object.get("contextUsage").and_then(Value::as_object).cloned().unwrap_or_default();
        let details = object.get("responseDetails").and_then(Value::as_object).cloned().unwrap_or_default();
        let model = text_of(details.get("responseModelId"))
            .or_else(|| text_of(usage.get("modelId")))
            .or_else(|| thread_model.filter(|m| !m.trim().is_empty()))
            .unwrap_or_else(|| "unknown".to_string());

        let prompt = clamped(usage.get("promptTokens"));
        let completion = clamped(usage.get("completionTokens"));
        let cache_read = clamped(usage.get("cachedTokens")).min(prompt);
        let cache_write = clamped(usage.get("cacheWriteTokens")).min(prompt - cache_read);
        let total = clamped(usage.get("totalTokens")).max(prompt + completion);
        if total <= 0 {
            return;
        }
        // Reasoning is inside the completion and is counted once, as output.
        let fresh = total - completion - cache_read - cache_write;
        let tally = TokenTally { input: fresh.max(0), cache_write, cache_read, output: completion, ..TokenTally::default() };

        let mut record = AgentUsageRecord::new(at, &model, tally).session(&thread.clone().unwrap_or_else(|| format!("unsloth:chat:{message_id}")));
        record.deduplication_id = Some(format!("unsloth:chat:{message_id}"));
        records.push(record);
    });
    records
}

/// The external-API path. There are no cache or reasoning columns.
fn api(connection: &rusqlite::Connection) -> Vec<AgentUsageRecord> {
    let columns = sqlite::columns(connection, "api_usage_events");
    let needed = ["id", "endpoint", "model", "prompt_tokens", "completion_tokens", "total_tokens", "created_at"];
    if !needed.iter().all(|n| columns.contains(*n)) {
        return Vec::new();
    }

    let mut records = Vec::new();
    let sql = "SELECT id, endpoint, model, prompt_tokens, completion_tokens, total_tokens, created_at FROM api_usage_events";
    sqlite::each(connection, sql, |row| {
        let Some(id) = sqlite::text(row, 0).filter(|id| !id.trim().is_empty()) else { return };
        let Some(model) = sqlite::text(row, 2).filter(|m| !m.trim().is_empty()) else { return };
        let Some(at) = sqlite::number(row, 6).filter(|v| *v > 0.0).and_then(epoch) else { return };

        let prompt = sqlite::count(row, 3).unwrap_or(0);
        let completion = sqlite::count(row, 4).unwrap_or(0);
        let total = sqlite::count(row, 5).unwrap_or(0).max(prompt + completion);
        if total <= 0 {
            return;
        }
        let tally = TokenTally { input: (total - completion).max(0), output: completion, ..TokenTally::default() };
        let mut record = AgentUsageRecord::new(at, &model, tally).session("unsloth:api");
        record.title = sqlite::text(row, 1).filter(|e| !e.trim().is_empty());
        record.deduplication_id = Some(format!("unsloth:api:{id}"));
        records.push(record);
    });
    records
}

fn text_of(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).filter(|s| !s.trim().is_empty()).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spend::sqlite::tests::database;

    fn store(home: &Path) -> PathBuf {
        let folder = home.join(".unsloth").join("studio");
        std::fs::create_dir_all(&folder).unwrap();
        folder.join("studio.db")
    }

    const SCHEMA: [&str; 3] = [
        "CREATE TABLE chat_threads (id TEXT, model_id TEXT)",
        "CREATE TABLE chat_messages (id TEXT, thread_id TEXT, role TEXT, metadata_json TEXT, created_at REAL)",
        "CREATE TABLE api_usage_events (id TEXT, endpoint TEXT, model TEXT, prompt_tokens INTEGER, completion_tokens INTEGER, total_tokens INTEGER, created_at REAL)",
    ];

    fn run(home: &Path) -> Vec<AgentUsageRecord> {
        Unsloth.records(&Unsloth.inputs(&Sources::new(home)))
    }

    #[test]
    fn chat_counters_are_normalized_so_the_kinds_add_up_to_the_total() {
        let home = tempfile::tempdir().unwrap();
        let mut sql: Vec<&str> = SCHEMA.to_vec();
        sql.push("INSERT INTO chat_threads VALUES ('th1','thread-model')");
        // prompt 100 (cache read 30, write 50 of the rest), completion 40 with reasoning inside it, total 150.
        sql.push(r#"INSERT INTO chat_messages VALUES ('m1','th1','assistant','{"contextUsage":{"promptTokens":100,"completionTokens":40,"totalTokens":150,"cachedTokens":30,"cacheWriteTokens":50,"reasoningTokens":10},"responseDetails":{"responseModelId":"model-a"}}',1789372800)"#);
        // A user message and a message with no time: nothing.
        sql.push(r#"INSERT INTO chat_messages VALUES ('u1','th1','user','{"contextUsage":{"promptTokens":9}}',1789372800)"#);
        sql.push(r#"INSERT INTO chat_messages VALUES ('m2','th1','assistant','{"contextUsage":{"promptTokens":9}}',0)"#);
        database(&store(home.path()), &sql);

        let records = run(home.path());
        assert_eq!(records.len(), 1);
        let record = &records[0];
        assert_eq!(record.model, "model-a");
        // fresh = 150 - 40 - 30 - 50 = 30
        assert_eq!(record.tally, TokenTally::new(30, 50, 30, 40));
        assert_eq!(record.tally.total(), 150);
        assert_eq!(record.session_id.as_deref(), Some("th1"));
        assert_eq!(record.deduplication_id.as_deref(), Some("unsloth:chat:m1"));
    }

    #[test]
    fn api_events_have_no_cache_and_a_total_below_the_parts_is_raised() {
        let home = tempfile::tempdir().unwrap();
        let mut sql: Vec<&str> = SCHEMA.to_vec();
        sql.push("INSERT INTO api_usage_events VALUES ('e1','/v1/chat','api-m',60,20,10,1789372800)");
        sql.push("INSERT INTO api_usage_events VALUES ('e2','/v1/chat','api-m',0,0,0,1789372800)");
        database(&store(home.path()), &sql);

        let records = run(home.path());
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].tally, TokenTally::new(60, 0, 0, 20));
        assert_eq!(records[0].session_id.as_deref(), Some("unsloth:api"));
        assert_eq!(records[0].title.as_deref(), Some("/v1/chat"));
    }

    #[test]
    fn a_missing_store_is_no_records_and_the_override_moves_it() {
        let home = tempfile::tempdir().unwrap();
        assert!(run(home.path()).is_empty());

        let elsewhere = tempfile::tempdir().unwrap();
        let mut sql: Vec<&str> = SCHEMA.to_vec();
        sql.push("INSERT INTO api_usage_events VALUES ('e1','/v1','m',5,5,10,1789372800)");
        database(&elsewhere.path().join("studio.db"), &sql);
        let sources = Sources::new(home.path()).with_var("UNSLOTH_STUDIO_HOME", elsewhere.path());
        assert_eq!(Unsloth.records(&Unsloth.inputs(&sources)).len(), 1);
    }
}
