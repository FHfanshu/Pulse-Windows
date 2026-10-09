// Ported from upstream Sources/Pulse/Usage/Readers/MuxUsageReader.swift.
//! Mux's per-workspace usage snapshot.
//!
//! `~/.mux/sessions/<workspaceId>/session-usage.json` holds one cumulative session total per
//! `"<provider>:<model>"` key:
//!
//! ```json
//! { "version": 1,
//!   "byModel": { "anthropic:claude": {
//!     "input":       { "tokens": 10, "cost_usd": 0.01 },
//!     "cached":      { "tokens": 20, "cost_usd": 0.00 },
//!     "cacheCreate": { "tokens":  5, "cost_usd": 0.01 },
//!     "output":      { "tokens": 30, "cost_usd": 0.10 },
//!     "reasoning":   { "tokens":  7, "cost_usd": 0.01 } } },
//!   "lastRequest": { "model": "claude", "timestamp": 1710000000000 } }
//! ```
//!
//! **The snapshot is already one reading per model.** A file is read as one record per model, so
//! the increment from nothing is the whole snapshot.
//!
//! **Reasoning is left out, not added.** The file reports `output` and `reasoning` side by side
//! and gives no token total, so nothing on disk says whether reasoning is inside `output`. The
//! reported output is kept, the reasoning figure is not counted, and the record is marked partial.
//!
//! **One timestamp per session.** `lastRequest.timestamp` is shared by every model record, so every
//! record is aggregate. A file with no such timestamp is skipped, not dated from its modification
//! time.
//!
//! Where it lives on Windows: `%USERPROFILE%\.mux\sessions`, the macOS layout carried over.
//! WINDOWS-PATH: unverified.

use std::path::PathBuf;

use serde_json::Value;

use super::{push_unique, SpendSource};
use crate::spend::logio;
use crate::spend::record::AgentUsageRecord;
use crate::spend::tally::TokenTally;
use crate::spend::transcripts::Sources;

pub struct Mux;

impl SpendSource for Mux {
    fn raw(&self) -> &'static str {
        "mux"
    }
    fn source_id(&self) -> &'static str {
        "mux"
    }
    fn display_name(&self) -> &'static str {
        "Mux"
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        let mut roots = Vec::new();
        // WINDOWS-PATH: unverified
        push_unique(&mut roots, sources.home.join(".mux").join("sessions"));
        roots
    }
    fn records(&self, roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        let mut records = Vec::new();
        for file in logio::files(roots, &[], &["session-usage.json"], &[]) {
            let Some(root) = logio::json(&file) else { continue };
            let Some(by_model) = root.get("byModel").and_then(Value::as_object) else { continue };

            // The same timestamp is every model's. Without a real one the session has no date.
            let last = root.get("lastRequest");
            let Some(at) = logio::timestamp(last.and_then(|l| l.get("timestamp")), true).filter(|t| t.timestamp() > 0) else {
                continue;
            };

            let session = file.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let fallback_model = logio::text(last.and_then(|l| l.get("model")));

            let mut keys: Vec<&String> = by_model.keys().collect();
            keys.sort();
            for key in keys {
                let Some(entry) = by_model.get(key) else { continue };
                let bucket = |name: &str| entry.get(name).and_then(|b| logio::count(b.get("tokens")));

                let reasoning = bucket("reasoning").unwrap_or(0);
                // No total says whether reasoning is inside output, so the reported output is kept
                // and the reasoning count is left out (and the record is marked partial).
                let tally = TokenTally::new(
                    bucket("input").unwrap_or(0),
                    bucket("cacheCreate").unwrap_or(0),
                    bucket("cached").unwrap_or(0),
                    bucket("output").unwrap_or(0),
                );
                let model = model_name(key).or_else(|| fallback_model.clone());
                let Some(model) = model else { continue };
                if tally.total() <= 0 {
                    continue;
                }

                let mut record = AgentUsageRecord::new(at, &model, tally).session(&session).aggregate(true);
                record.is_partial = reasoning > 0;
                record.session_name = Some(session.clone());
                record.deduplication_id = Some(format!("mux:{session}:{key}"));
                records.push(record);
            }
        }
        records
    }
}

/// `"<provider>:<model>"` → the model after the first colon. The provider is routing, not part of
/// the model's name, and is dropped rather than folded into a key no price list could match.
fn model_name(key: &str) -> Option<String> {
    let model = match key.split_once(':') {
        Some((_, model)) => model,
        None => key,
    };
    let model = model.trim();
    (!model.is_empty()).then(|| model.to_string())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::spend::calendar::Calendar;
    use crate::spend::prices::PriceTable;
    use crate::spend::sources::{read_agent, SpendAgent};
    use crate::spend::summary::SpendSummary;

    fn snapshot(home: &std::path::Path, workspace: &str, body: Value) {
        let folder = home.join(".mux").join("sessions").join(workspace);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("session-usage.json"), body.to_string()).unwrap();
    }

    fn entry(input: i64, cache_create: i64, cached: i64, output: i64, reasoning: i64) -> Value {
        json!({
            "input": {"tokens": input, "cost_usd": 0.0},
            "cacheCreate": {"tokens": cache_create, "cost_usd": 0.0},
            "cached": {"tokens": cached, "cost_usd": 0.0},
            "output": {"tokens": output, "cost_usd": 0.0},
            "reasoning": {"tokens": reasoning, "cost_usd": 0.0},
        })
    }

    #[test]
    fn each_model_is_one_aggregate_record_with_reasoning_left_out_and_marked_partial() {
        let home = tempfile::tempdir().unwrap();
        snapshot(
            home.path(),
            "ws1",
            json!({"byModel": {"anthropic:claude": entry(10, 5, 20, 30, 7)}, "lastRequest": {"model": "claude", "timestamp": 1_789_372_800_000i64}}),
        );
        let records = Mux.records(&Mux.inputs(&Sources::new(home.path())));
        assert_eq!(records.len(), 1);
        let record = &records[0];
        assert_eq!(record.tally, TokenTally::new(10, 5, 20, 30));
        assert_eq!(record.model, "claude");
        assert!(record.is_aggregate);
        assert!(record.is_partial);
        assert_eq!(record.session_id.as_deref(), Some("ws1"));
        assert_eq!(record.deduplication_id.as_deref(), Some("mux:ws1:anthropic:claude"));
    }

    #[test]
    fn a_session_without_a_real_time_is_skipped_and_a_store_that_is_missing_is_empty() {
        let home = tempfile::tempdir().unwrap();
        snapshot(home.path(), "no-time", json!({"byModel": {"a:b": entry(1, 0, 0, 1, 0)}, "lastRequest": {"model": "b"}}));
        snapshot(home.path(), "zero-time", json!({"byModel": {"a:b": entry(1, 0, 0, 1, 0)}, "lastRequest": {"timestamp": 0}}));
        assert!(Mux.records(&Mux.inputs(&Sources::new(home.path()))).is_empty());

        let cache = tempfile::tempdir().unwrap();
        let empty = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::Mux, &Sources::new(empty.path()), cache.path(), &Calendar::utc(2), &PriceTable::new());
        assert!(ledger.days.is_empty());
    }

    #[test]
    fn a_model_is_the_part_after_the_provider_and_the_fallback_names_a_bare_key() {
        assert_eq!(model_name("anthropic:claude-x").as_deref(), Some("claude-x"));
        assert_eq!(model_name("bare").as_deref(), Some("bare"));
        assert_eq!(model_name("p: ").as_deref(), None);
    }

    #[test]
    fn a_session_buckets_reach_today_and_only_todays_counts() {
        use chrono::{Duration, Utc};
        let home = tempfile::tempdir().unwrap();
        let calendar = Calendar::local();
        let now = Utc::now();
        let today = (calendar.start_of_day(now) + Duration::hours(12)).timestamp_millis();
        let yesterday = (calendar.add_days(calendar.start_of_day(now), -1) + Duration::hours(12)).timestamp_millis();
        let folder = home.path().join(".mux").join("sessions").join("ws-a");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(
            folder.join("session-usage.json"),
            json!({"byModel": {"p:priced": entry(100, 0, 0, 0, 0)}, "lastRequest": {"model": "priced", "timestamp": today}}).to_string(),
        )
        .unwrap();
        let other = home.path().join(".mux").join("sessions").join("ws-b");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(
            other.join("session-usage.json"),
            json!({"byModel": {"p:priced": entry(900, 0, 0, 0, 0)}, "lastRequest": {"model": "priced", "timestamp": yesterday}}).to_string(),
        )
        .unwrap();
        let cache = tempfile::tempdir().unwrap();
        let sources = Sources::new(home.path());
        let ledger = read_agent(SpendAgent::Mux, &sources, cache.path(), &calendar, &PriceTable::new());
        let summary = SpendSummary::of(&std::collections::HashMap::from([(SpendAgent::Mux, ledger)]), Some(1), now, &calendar);
        // The summary is today's: yesterday's 900 is in the ledger but not in today's figures.
        assert_eq!(summary.tokens, 100);
        assert_eq!(summary.sessions.iter().map(|s| s.session.unpriced_tokens).sum::<i64>(), 100);
    }
}
