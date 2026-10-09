// Ported from upstream Sources/Pulse/Usage/Readers/ReasonixUsageReader.swift.
//! Reasonix's daily stats files.
//!
//! `~/.reasonix/stats/YYYY-MM-DD.jsonl` (`$REASONIX_STATE_HOME`, or `$REASONIX_HOME/stats`) holds one
//! aggregate per request group: `{ "ts", "model", "prompt", "completion", "reasoning", "cache_hit",
//! "cache_miss", "total", "requests", "turn" }`. The session transcript is deliberately not scanned: it
//! has no authoritative counters and would overlap these.
//!
//! **Only real, non-empty rows count.** A `turn == true` marker, an empty model, and a row whose `total`
//! and `requests` are both non-positive are all skipped.
//!
//! **Cache hit is the read, cache miss the fresh input.** When `cache_miss` is present and positive it is
//! the fresh input; otherwise it is `prompt - cache_hit`. Reasoning is a subset of completion, so the
//! completion is kept whole as output. There is no cache-write figure in this format.
//!
//! `model` is kept exactly as reported, provider prefix included.
//!
//! Where it lives on Windows: `REASONIX_STATE_HOME` (the state directory itself), else
//! `REASONIX_HOME\stats`, else `%USERPROFILE%\.reasonix\stats`. WINDOWS-PATH: unverified.

use std::path::PathBuf;

use serde_json::Value;

use super::{push_unique, SpendSource};
use crate::spend::logio;
use crate::spend::record::AgentUsageRecord;
use crate::spend::tally::TokenTally;
use crate::spend::transcripts::Sources;

pub struct Reasonix;

impl SpendSource for Reasonix {
    fn raw(&self) -> &'static str {
        "reasonix"
    }
    fn source_id(&self) -> &'static str {
        "reasonix"
    }
    fn display_name(&self) -> &'static str {
        "Reasonix"
    }
    fn env_vars(&self) -> &'static [&'static str] {
        &["REASONIX_STATE_HOME", "REASONIX_HOME"]
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        let mut roots = Vec::new();
        // WINDOWS-PATH: unverified (the macOS layout carried over)
        let stats = if let Some(state) = sources.var("REASONIX_STATE_HOME") {
            state
        } else if let Some(base) = sources.var("REASONIX_HOME") {
            base.join("stats")
        } else {
            sources.home.join(".reasonix").join("stats")
        };
        push_unique(&mut roots, stats);
        roots
    }
    fn records(&self, roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        let mut records = Vec::new();
        for file in logio::files(roots, &["jsonl"], &[], &[]) {
            let path = std::fs::canonicalize(&file).unwrap_or_else(|_| file.clone()).to_string_lossy().into_owned();
            let Ok(text) = std::fs::read_to_string(&file) else { continue };

            let mut line_index = 0;
            for line in text.lines().filter(|l| !l.trim().is_empty()) {
                line_index += 1;
                let Ok(row @ Value::Object(_)) = serde_json::from_str::<Value>(line) else { continue };

                // `turn` is a marker, not a call.
                if row.get("turn").and_then(Value::as_bool) == Some(true) {
                    continue;
                }
                let Some(model) = logio::text(row.get("model")) else { continue };

                let total = logio::count(row.get("total"));
                let requests = logio::count(row.get("requests"));
                if total.unwrap_or(0) <= 0 && requests.unwrap_or(0) <= 0 {
                    continue;
                }
                let Some(at) = logio::timestamp(row.get("ts"), false).filter(|t| t.timestamp() > 0) else { continue };

                let prompt = logio::count(row.get("prompt"));
                let completion = logio::count(row.get("completion"));
                let cache_read = logio::count(row.get("cache_hit")).unwrap_or(0);

                let (tally, unclassified) = if prompt.is_none() && completion.is_none() {
                    // A bare total: real, but with no split to place it in.
                    (TokenTally::default(), total.unwrap_or(0))
                } else {
                    let miss = logio::count(row.get("cache_miss")).unwrap_or(0);
                    let input = if miss > 0 { miss } else { (prompt.unwrap_or(0) - cache_read).max(0) };
                    // Reasoning is inside completion, which is kept whole.
                    (TokenTally::new(input, 0, cache_read, completion.unwrap_or(0)), 0)
                };

                if model.trim().is_empty() || (tally.total() <= 0 && unclassified <= 0) {
                    continue;
                }
                let mut record = AgentUsageRecord::new(at, &model, tally).session(&format!("reasonix-stats:{path}")).unclassified(unclassified);
                record.session_name = Some("reasonix".into());
                record.deduplication_id = Some(format!("reasonix:{path}:{line_index}:{}:{}", requests.unwrap_or(0), total.unwrap_or(0)));
                records.push(record);
            }
        }
        records
    }
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};
    use serde_json::json;

    use super::*;
    use crate::spend::calendar::Calendar;
    use crate::spend::prices::PriceTable;
    use crate::spend::sources::{read_agent, SpendAgent};
    use crate::spend::summary::SpendSummary;
    use crate::spend::transcripts::Sources as Home;

    const TS: i64 = 1_789_381_230;

    fn stats(dir: &std::path::Path, name: &str, rows: &[Value]) {
        std::fs::create_dir_all(dir).unwrap();
        let text: Vec<String> = rows.iter().map(Value::to_string).collect();
        std::fs::write(dir.join(name), text.join("\n")).unwrap();
    }

    fn home_stats(home: &std::path::Path) -> PathBuf {
        home.join(".reasonix").join("stats")
    }

    #[test]
    fn cache_hit_is_the_read_and_the_fresh_input_is_what_is_left_and_completion_is_kept_whole() {
        let home = tempfile::tempdir().unwrap();
        stats(&home_stats(home.path()), "2026-09-14.jsonl", &[json!({"ts": TS, "model": "deepseek/priced", "prompt": 100, "completion": 40, "reasoning": 6, "cache_hit": 30, "total": 140, "requests": 1})]);
        let records = Reasonix.records(&Reasonix.inputs(&Home::new(home.path())));
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].tally, TokenTally::new(70, 0, 30, 40));
        // The provider prefix is kept exactly as reported.
        assert_eq!(records[0].model, "deepseek/priced");
        assert_eq!(records[0].session_name.as_deref(), Some("reasonix"));
    }

    #[test]
    fn a_positive_cache_miss_is_the_fresh_input_and_a_bare_total_is_unclassified() {
        let home = tempfile::tempdir().unwrap();
        stats(&home_stats(home.path()), "2026-09-14.jsonl", &[
            json!({"ts": TS, "model": "priced", "prompt": 100, "completion": 1, "cache_hit": 30, "cache_miss": 60, "total": 101, "requests": 1}),
            json!({"ts": TS, "model": "priced", "total": 25, "requests": 1}),
        ]);
        let records = Reasonix.records(&Reasonix.inputs(&Home::new(home.path())));
        assert_eq!(records[0].tally, TokenTally::new(60, 0, 30, 1));
        assert_eq!(records[1].tally.total(), 0);
        assert_eq!(records[1].unclassified_tokens, 25);
    }

    #[test]
    fn a_turn_marker_an_empty_model_an_empty_total_and_an_undated_row_are_skipped() {
        let home = tempfile::tempdir().unwrap();
        stats(&home_stats(home.path()), "2026-09-14.jsonl", &[
            json!({"ts": TS, "model": "priced", "turn": true, "total": 5, "requests": 1}),
            json!({"ts": TS, "model": "", "prompt": 5, "completion": 1}),
            json!({"ts": TS, "model": "priced", "total": 0, "requests": 0, "prompt": 5, "completion": 1}),
            json!({"model": "priced", "prompt": 5, "completion": 1, "total": 6, "requests": 1}),
        ]);
        assert!(Reasonix.records(&Reasonix.inputs(&Home::new(home.path()))).is_empty());
    }

    #[test]
    fn a_missing_store_is_empty_and_both_overrides_move_it() {
        let home = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::Reasonix, &Home::new(home.path()), cache.path(), &Calendar::utc(2), &PriceTable::new());
        assert!(ledger.days.is_empty());

        let elsewhere = tempfile::tempdir().unwrap();
        stats(&elsewhere.path().join("stats"), "a.jsonl", &[json!({"ts": TS, "model": "m", "prompt": 4, "completion": 1, "total": 5, "requests": 1})]);
        let by_home = Home::new(home.path()).with_var("REASONIX_HOME", elsewhere.path());
        assert_eq!(Reasonix.records(&Reasonix.inputs(&by_home)).len(), 1);

        let state = tempfile::tempdir().unwrap();
        stats(state.path(), "b.jsonl", &[json!({"ts": TS, "model": "m", "prompt": 4, "completion": 1, "total": 5, "requests": 1})]);
        let by_state = Home::new(home.path()).with_var("REASONIX_STATE_HOME", state.path());
        assert_eq!(Reasonix.records(&Reasonix.inputs(&by_state)).len(), 1);
    }

    #[test]
    fn a_session_buckets_reach_today_and_only_todays_counts() {
        let home = tempfile::tempdir().unwrap();
        let calendar = Calendar::local();
        let now = Utc::now();
        let secs = |day: chrono::DateTime<Utc>| (calendar.start_of_day(day) + Duration::hours(12)).timestamp();
        let yesterday = calendar.add_days(calendar.start_of_day(now), -1);
        stats(&home_stats(home.path()), "today.jsonl", &[json!({"ts": secs(now), "model": "priced", "prompt": 100, "completion": 0, "total": 100, "requests": 1})]);
        stats(&home_stats(home.path()), "old.jsonl", &[json!({"ts": secs(yesterday), "model": "priced", "prompt": 900, "completion": 0, "total": 900, "requests": 1})]);
        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::Reasonix, &Home::new(home.path()), cache.path(), &calendar, &PriceTable::new());
        let summary = SpendSummary::of(&std::collections::HashMap::from([(SpendAgent::Reasonix, ledger)]), Some(1), now, &calendar);
        assert_eq!(summary.tokens, 100);
        assert_eq!(summary.sessions[0].session.unpriced_tokens, 100);
    }
}
