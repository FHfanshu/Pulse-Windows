// Ported from upstream Sources/Pulse/Usage/Readers/FxUsageReader.swift.
//! Fx's per-session usage snapshot.
//!
//! `~/.fx/sessions/<sessionId>/usage-v2.json` holds a `snapshot` and a `models` array; `session.json`
//! in the same directory carries the session's timing and workspace, and `~/.fx/sessions/index.json`
//! carries its title.
//!
//! **One record per model**, each a cumulative session total, so the whole snapshot is the increment
//! and the builder folds a session reached twice. A snapshot whose `models` array is empty but whose
//! aggregate has usage still emits once under `fx-unknown`.
//!
//! **Reasoning is left out, not added.** Fx reports a per-model `reasoning_tokens` beside
//! `output_tokens` and states no total, so the reported output is kept, the reasoning figure is not
//! counted, and the record is marked partial.
//!
//! **One timestamp per session**, `updated_at_ms` else `created_at_ms`; a snapshot with neither is
//! skipped. Every record is aggregate because none has its own call time. `total_cost` is Fx's own
//! dollars and is not read as tokens.
//!
//! Where it lives on Windows: `%USERPROFILE%\.fx\sessions`, the macOS layout carried over.
//! WINDOWS-PATH: unverified.

use std::collections::HashMap;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::{push_unique, SpendSource};
use crate::spend::logio;
use crate::spend::record::AgentUsageRecord;
use crate::spend::tally::TokenTally;
use crate::spend::transcripts::Sources;

pub struct Fx;

impl SpendSource for Fx {
    fn raw(&self) -> &'static str {
        "fx"
    }
    fn source_id(&self) -> &'static str {
        "fx"
    }
    fn display_name(&self) -> &'static str {
        "FX"
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        let mut roots = Vec::new();
        // WINDOWS-PATH: unverified (the macOS layout carried over)
        push_unique(&mut roots, sources.home.join(".fx").join("sessions"));
        roots
    }
    fn records(&self, roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        let titles = titles(roots);
        let mut records = Vec::new();

        for file in logio::files(roots, &[], &["usage-v2.json"], &[]) {
            let Some(root) = logio::json(&file).filter(Value::is_object) else { continue };
            let Some(snapshot) = root.get("snapshot").filter(|s| s.is_object()) else { continue };

            let directory = file.parent().map(PathBuf::from).unwrap_or_default();
            let folder = directory.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let session_id = logio::text(root.get("session_id")).unwrap_or(folder);
            let sidecar = logio::json(&directory.join("session.json")).filter(Value::is_object);
            let sidecar = sidecar.as_ref();

            // A zero `updated_at_ms` is unset, so `created_at_ms` is tried.
            let Some(at) = event_time(sidecar.and_then(|s| s.get("updated_at_ms")))
                .or_else(|| event_time(sidecar.and_then(|s| s.get("created_at_ms"))))
            else {
                continue;
            };

            let workspace = logio::text(sidecar.and_then(|s| s.get("workspace_root")));
            let title = titles.get(&session_id).cloned();

            let models = snapshot
                .get("models")
                .and_then(Value::as_array)
                .filter(|models| !models.is_empty() && models.iter().all(Value::is_object));
            // Each model's own counts, or the snapshot's aggregate as "fx-unknown" when nothing is grouped.
            let entries: Vec<(String, &Value)> = match models {
                Some(models) => models.iter().map(|entry| (logio::text(entry.get("model")).unwrap_or_else(|| "fx-unknown".into()), entry)).collect(),
                None => vec![("fx-unknown".to_string(), snapshot)],
            };

            for (model, counts) in entries {
                let count = |key: &str| logio::count(counts.get(key)).unwrap_or(0);
                let reasoning = count("reasoning_tokens");
                let tally = TokenTally::new(count("input_tokens"), count("cache_write_tokens"), count("cache_read_tokens"), count("output_tokens"));
                if tally.total() <= 0 {
                    continue;
                }
                let mut record = AgentUsageRecord::new(at, &model, tally).session(&session_id).aggregate(true);
                record.is_partial = reasoning > 0;
                record.session_name = Some(session_id.clone());
                record.title = title.clone();
                record.project = workspace.clone();
                record.deduplication_id = Some(format!("fx:{session_id}:{model}"));
                records.push(record);
            }
        }
        records
    }
}

/// A timestamp in milliseconds; zero is unset.
fn event_time(value: Option<&Value>) -> Option<DateTime<Utc>> {
    logio::timestamp(value, true).filter(|t| t.timestamp() > 0)
}

/// `index.json`'s `sessions[<id>].title`, where present.
fn titles(roots: &[PathBuf]) -> HashMap<String, String> {
    let mut titles = HashMap::new();
    for file in logio::files(roots, &[], &["index.json"], &[]) {
        let Some(root) = logio::json(&file) else { continue };
        let Some(sessions) = root.get("sessions").and_then(Value::as_object) else { continue };
        for (id, value) in sessions {
            if let Some(title) = logio::text(value.get("title")) {
                titles.insert(id.clone(), title);
            }
        }
    }
    titles
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::spend::calendar::Calendar;
    use crate::spend::prices::PriceTable;
    use crate::spend::sources::{read_agent, SpendAgent};
    use crate::spend::summary::SpendSummary;
    use crate::spend::transcripts::Sources as Home;

    const MS: i64 = 1_789_381_230_000;

    fn session(home: &std::path::Path, id: &str, usage: Value, sidecar: Value) {
        let dir = home.join(".fx").join("sessions").join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("usage-v2.json"), json!({"session_id": id, "snapshot": usage}).to_string()).unwrap();
        std::fs::write(dir.join("session.json"), sidecar.to_string()).unwrap();
    }

    #[test]
    fn each_model_is_one_aggregate_record_titled_from_the_index_with_reasoning_left_out() {
        let home = tempfile::tempdir().unwrap();
        session(home.path(), "f1", json!({"models": [
            {"model": "priced", "input_tokens": 10, "cache_write_tokens": 2, "cache_read_tokens": 30, "output_tokens": 40, "reasoning_tokens": 6},
        ]}), json!({"updated_at_ms": MS, "created_at_ms": 1, "workspace_root": "C:\\Users\\me\\Code\\Pulse"}));
        std::fs::write(home.path().join(".fx").join("sessions").join("index.json"), json!({"sessions": {"f1": {"title": "Fix the ring"}}}).to_string()).unwrap();
        let records = Fx.records(&Fx.inputs(&Home::new(home.path())));
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].tally, TokenTally::new(10, 2, 30, 40));
        assert!(records[0].is_aggregate);
        assert!(records[0].is_partial);
        assert_eq!(records[0].title.as_deref(), Some("Fix the ring"));
        assert_eq!(records[0].project.as_deref(), Some("C:\\Users\\me\\Code\\Pulse"));
    }

    #[test]
    fn a_snapshot_without_models_is_one_fx_unknown_record_and_a_zero_updated_time_falls_back_to_created() {
        let home = tempfile::tempdir().unwrap();
        session(home.path(), "f2", json!({"input_tokens": 5, "output_tokens": 3}), json!({"updated_at_ms": 0, "created_at_ms": MS}));
        let records = Fx.records(&Fx.inputs(&Home::new(home.path())));
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].model, "fx-unknown");
        assert_eq!(records[0].tally, TokenTally::new(5, 0, 0, 3));
        assert_eq!(records[0].timestamp.timestamp_millis(), MS);
    }

    #[test]
    fn a_session_with_no_time_is_skipped_and_a_missing_store_is_empty() {
        let home = tempfile::tempdir().unwrap();
        session(home.path(), "f3", json!({"input_tokens": 5}), json!({"workspace_root": "/x"}));
        assert!(Fx.records(&Fx.inputs(&Home::new(home.path()))).is_empty());

        let empty = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::Fx, &Home::new(empty.path()), cache.path(), &Calendar::utc(2), &PriceTable::new());
        assert!(ledger.days.is_empty());
    }

    #[test]
    fn a_session_buckets_reach_today_and_only_todays_counts() {
        use chrono::Duration;
        let home = tempfile::tempdir().unwrap();
        let calendar = Calendar::local();
        let now = Utc::now();
        let ms = |day: DateTime<Utc>| (calendar.start_of_day(day) + Duration::hours(12)).timestamp_millis();
        session(home.path(), "today", json!({"models": [{"model": "priced", "input_tokens": 100}]}), json!({"updated_at_ms": ms(now)}));
        let yesterday = calendar.add_days(calendar.start_of_day(now), -1);
        session(home.path(), "old", json!({"models": [{"model": "priced", "input_tokens": 900}]}), json!({"updated_at_ms": ms(yesterday)}));
        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::Fx, &Home::new(home.path()), cache.path(), &calendar, &PriceTable::new());
        let summary = SpendSummary::of(&std::collections::HashMap::from([(SpendAgent::Fx, ledger)]), Some(1), now, &calendar);
        assert_eq!(summary.tokens, 100);
        assert_eq!(summary.sessions[0].session.unpriced_tokens, 100);
    }
}
