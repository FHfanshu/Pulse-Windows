// Ported from upstream Sources/Pulse/Usage/Readers/CapturedHindsightReader.swift.
//! Hindsight's ledger: a JSONL mirror of a self-hosted memory service's `llm-requests`, one record
//! per line.
//!
//! The service's own table is a rolling window (rows are evicted as it runs), so the durable copy
//! is the JSONL an authenticated sync writes. Pulse reads that ledger, never the service.
//!
//! ```json
//! { "id": "…", "started_at": "2026-…Z", "model": "…", "input_tokens": 10, "output_tokens": 4,
//!   "cached_tokens": 2, "total_tokens": 14, "trace_id": "…", "bank": "…", "operation": "…", "scope": "…" }
//! ```
//!
//! A row with no `id`, `model` or `started_at`, a stated total of zero or less, or with neither input
//! nor output is skipped. Cache write is not reported. Reasoning is never split out: the completion
//! count already includes it, so output is kept as stated.
//!
//! Where it lives on Windows: `HINDSIGHT_HOME\usage` when set, else `%USERPROFILE%\.hindsight\usage`.
//! `WINDOWS-PATH: unverified`.

use std::path::PathBuf;

use serde_json::Value;

use super::{push_unique, SpendSource};
use crate::spend::logio;
use crate::spend::record::AgentUsageRecord;
use crate::spend::tally::TokenTally;
use crate::spend::titles::title_from;
use crate::spend::transcripts::Sources;

pub struct Hindsight;

impl SpendSource for Hindsight {
    fn raw(&self) -> &'static str {
        "hindsight"
    }
    fn source_id(&self) -> &'static str {
        "hindsight"
    }
    fn display_name(&self) -> &'static str {
        "Hindsight"
    }
    fn env_vars(&self) -> &'static [&'static str] {
        &["HINDSIGHT_HOME", "APPDATA"]
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        let mut roots = Vec::new();
        // WINDOWS-PATH: unverified
        let base = sources.var("HINDSIGHT_HOME").unwrap_or_else(|| sources.home.join(".hindsight"));
        push_unique(&mut roots, base.join("usage"));
        push_unique(&mut roots, sources.app_data().join("Pulse").join("UsageImports").join("hindsight"));
        roots
    }
    fn records(&self, roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        let mut records = Vec::new();
        for file in logio::files(roots, &["jsonl"], &[], &[]) {
            for line in logio::json_lines(&file) {
                records.extend(record_of(&line));
            }
        }
        records
    }
    fn requires_usage_export(&self) -> bool {
        true
    }
}

fn record_of(line: &Value) -> Option<AgentUsageRecord> {
    // The id is the ledger's dedup identity; without it a re-synced row cannot be folded.
    let id = logio::text(line.get("id"))?;
    let model = logio::text(line.get("model"))?;
    let at = logio::timestamp(line.get("started_at"), false)?;

    // A stated total of zero or less asserts no usage.
    if logio::count(line.get("total_tokens")).is_some_and(|total| total <= 0) {
        return None;
    }

    let input = logio::count(line.get("input_tokens")).unwrap_or(0);
    let output = logio::count(line.get("output_tokens")).unwrap_or(0);
    if input <= 0 && output <= 0 {
        return None;
    }

    let tally = TokenTally::new(input, 0, logio::count(line.get("cached_tokens")).unwrap_or(0), output);
    let mut record = AgentUsageRecord::new(at, &model, tally);
    record.session_id = Some(logio::text(line.get("trace_id")).unwrap_or_else(|| id.clone()));
    record.deduplication_id = Some(format!("hindsight:{id}"));
    // The bank is the workspace the record ran against; operation and scope describe the call.
    record.project = logio::text(line.get("bank"));
    record.title = title(logio::text(line.get("operation")), logio::text(line.get("scope")));
    Some(record)
}

/// `operation / scope` where both are present and differ, otherwise whichever one is stated.
fn title(operation: Option<String>, scope: Option<String>) -> Option<String> {
    match (operation, scope) {
        (Some(operation), Some(scope)) if operation == scope => title_from(&operation),
        (Some(operation), Some(scope)) => title_from(&format!("{operation} / {scope}")),
        (Some(operation), None) => title_from(&operation),
        (None, Some(scope)) => title_from(&scope),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use chrono::Utc;
    use serde_json::json;

    use super::*;
    use crate::spend::calendar::Calendar;
    use crate::spend::prices::{ModelPrice, PriceTable};
    use crate::spend::sources::{read_agent, SpendAgent};

    fn priced() -> PriceTable {
        HashMap::from([("hs-model".to_string(), ModelPrice::new(1.0, 10.0, Some(0.1), Some(1.0), Some("Hs")))])
    }

    fn write(home: &std::path::Path, lines: &[Value]) {
        let folder = home.join(".hindsight").join("usage");
        std::fs::create_dir_all(&folder).unwrap();
        let text: Vec<String> = lines.iter().map(Value::to_string).collect();
        std::fs::write(folder.join("ledger.jsonl"), text.join("\n")).unwrap();
    }

    #[test]
    fn the_kinds_land_where_upstream_says_and_output_is_kept_as_stated() {
        let home = tempfile::tempdir().unwrap();
        let at = Utc::now().to_rfc3339();
        write(
            home.path(),
            &[
                json!({"id":"r1","started_at":at,"model":"hs-model","input_tokens":100,"output_tokens":20,
                       "cached_tokens":30,"total_tokens":150,"trace_id":"t1","bank":"Pulse","operation":"retain","scope":"notes"}),
                // No id, no time, and a stated zero total: each is skipped.
                json!({"started_at":at,"model":"hs-model","input_tokens":5,"output_tokens":5}),
                json!({"id":"r3","model":"hs-model","input_tokens":5,"output_tokens":5}),
                json!({"id":"r4","started_at":at,"model":"hs-model","input_tokens":0,"output_tokens":0,"total_tokens":0}),
            ],
        );
        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::Hindsight, &Sources::new(home.path()), cache.path(), &Calendar::utc(2), &priced());
        let day = ledger.days.iter().find(|d| d.tokens > 0).unwrap();
        assert_eq!(day.tally, TokenTally::new(100, 0, 30, 20));
        let session = &ledger.sessions[0];
        assert_eq!(session.project.as_ref().unwrap().name, "Pulse");
        assert_eq!(session.title.as_deref(), Some("retain / notes"));
    }

    #[test]
    fn a_row_present_in_two_files_counts_once_and_a_missing_store_is_empty() {
        let home = tempfile::tempdir().unwrap();
        let at = Utc::now().to_rfc3339();
        let row = json!({"id":"same","started_at":at,"model":"hs-model","input_tokens":7,"output_tokens":1});
        write(home.path(), std::slice::from_ref(&row));
        std::fs::write(home.path().join(".hindsight").join("usage").join("copy.jsonl"), row.to_string()).unwrap();
        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::Hindsight, &Sources::new(home.path()), cache.path(), &Calendar::utc(2), &priced());
        assert_eq!(ledger.days.iter().map(|d| d.tokens).sum::<i64>(), 8);

        // A home with no store, read with no history kept.
        let (empty, cache) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let ledger = read_agent(SpendAgent::Hindsight, &Sources::new(empty.path()), cache.path(), &Calendar::utc(2), &priced());
        assert!(ledger.days.is_empty());
    }

    #[test]
    fn the_home_override_moves_the_store() {
        let home = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let folder = elsewhere.path().join("usage");
        std::fs::create_dir_all(&folder).unwrap();
        let at = Utc::now().to_rfc3339();
        std::fs::write(
            folder.join("ledger.jsonl"),
            json!({"id":"x","started_at":at,"model":"hs-model","input_tokens":3,"output_tokens":3}).to_string(),
        )
        .unwrap();
        let sources = Sources::new(home.path()).with_var("HINDSIGHT_HOME", elsewhere.path());
        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::Hindsight, &sources, cache.path(), &Calendar::utc(2), &priced());
        assert_eq!(ledger.days.iter().map(|d| d.tokens).sum::<i64>(), 6);
    }
}
