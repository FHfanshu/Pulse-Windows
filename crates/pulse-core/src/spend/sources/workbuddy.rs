// Ported from upstream Sources/Pulse/Usage/Readers/TencentBuddyReader.swift (client "workbuddy").
//! WorkBuddy, Tencent's coding agent. Its transcripts and extension logs are read by the shared
//! Tencent reader in `codebuddy.rs`; this file says where WorkBuddy keeps them.
//!
//! WorkBuddy's aggregate `workbuddy.db` is read only as a fallback when no transcript exists. Each
//! row is one session's whole total, placed at its own write time and counted as **unclassified**
//! tokens, never as fresh input, and never spread across hours nobody measured.
//!
//! Where it lives on Windows: `%USERPROFILE%\.workbuddy` (the 5.0 tree) and
//! `%USERPROFILE%\.workbuddy-ai` (the 5.5 tree), both read; the Windows location is unverified.

use std::path::PathBuf;

use super::codebuddy;
use super::SpendSource;
use crate::spend::record::AgentUsageRecord;
use crate::spend::transcripts::Sources;

pub struct WorkBuddy;

impl SpendSource for WorkBuddy {
    fn raw(&self) -> &'static str {
        "workBuddy"
    }
    fn source_id(&self) -> &'static str {
        "workbuddy"
    }
    fn display_name(&self) -> &'static str {
        "WorkBuddy"
    }
    fn icon(&self) -> Option<&'static str> {
        // CodeBuddy and WorkBuddy are both Tencent's.
        Some("tencent")
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        // WINDOWS-PATH: unverified
        vec![sources.home.join(".workbuddy"), sources.home.join(".workbuddy-ai")]
    }
    fn records(&self, roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        codebuddy::read("workbuddy", roots)
    }
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};
    use serde_json::json;

    use crate::spend::calendar::Calendar;
    use crate::spend::prices::{ModelPrice, PriceTable};
    use crate::spend::sources::{read_agent, SpendAgent};
    use crate::spend::sqlite::tests::database;
    use crate::spend::summary::SpendSummary;
    use crate::spend::transcripts::Sources;
    use crate::spend::tally::TokenTally;

    fn store(home: &std::path::Path, tree: &str) -> std::path::PathBuf {
        let folder = home.join(tree);
        std::fs::create_dir_all(&folder).unwrap();
        folder.join("workbuddy.db")
    }

    #[test]
    fn the_database_fallback_is_a_session_aggregate_counted_as_unclassified_and_the_ai_tree_is_read() {
        let home = tempfile::tempdir().unwrap();
        // 2026-09-14 10:00 UTC in milliseconds, and the same instant in seconds.
        let ms = 1_789_380_000_000_i64;
        database(
            &store(home.path(), ".workbuddy-ai"),
            &[
                "CREATE TABLE sessions (id TEXT, cwd TEXT, model TEXT)",
                "CREATE TABLE session_usage (session_id TEXT, used INTEGER, updated_at INTEGER)",
                "INSERT INTO sessions VALUES ('w1','C:\\Users\\me\\code\\Pulse','tencent/hy-2')",
                &format!("INSERT INTO session_usage VALUES ('w1', 4242, {ms})"),
                // Seconds, and a zero usage row: the second is counted, the zero is not.
                "INSERT INTO session_usage VALUES ('w1', 0, 1789380000)",
                "INSERT INTO session_usage VALUES ('w1', 77, 1789380001)",
            ],
        );
        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::WorkBuddy, &Sources::new(home.path()), cache.path(), &Calendar::utc(2), &PriceTable::new());
        // Unclassified: counted as tokens, never as fresh input, so no kind is filled.
        assert_eq!(ledger.days.iter().map(|d| d.tokens).sum::<i64>(), 4242 + 77);
        assert_eq!(ledger.sessions[0].project.as_ref().unwrap().name, "Pulse");
        assert_eq!(ledger.unpriced_models, ["hy-2"]);
    }

    #[test]
    fn a_transcript_beside_the_database_wins_and_the_database_is_not_appended() {
        let home = tempfile::tempdir().unwrap();
        let transcript = home.path().join(".workbuddy").join("projects").join("p").join("s.jsonl");
        std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
        std::fs::write(
            &transcript,
            json!({"timestamp":1_789_380_000_000_i64,"type":"message","role":"assistant","sessionId":"s","message":{"usage":{"input_tokens":5,"output_tokens":5}},"providerData":{"messageId":"t1"}}).to_string(),
        )
        .unwrap();
        database(
            &store(home.path(), ".workbuddy"),
            &[
                "CREATE TABLE sessions (id TEXT, cwd TEXT, model TEXT)",
                "CREATE TABLE session_usage (session_id TEXT, used INTEGER, updated_at INTEGER)",
                "INSERT INTO session_usage VALUES ('s', 999, 1789380000)",
            ],
        );
        let records = super::super::codebuddy::read("workbuddy", &[home.path().join(".workbuddy")]);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].tally, TokenTally::new(5, 0, 0, 5));
        // The database held a record, so the transcript is only the confirmed subset.
        assert!(records[0].is_partial);
    }

    #[test]
    fn a_session_with_a_price_reaches_today_and_only_todays_counts() {
        let home = tempfile::tempdir().unwrap();
        let calendar = Calendar::local();
        let now = Utc::now();
        let today = (calendar.start_of_day(now) + Duration::hours(12)).timestamp();
        database(
            &store(home.path(), ".workbuddy"),
            &[
                "CREATE TABLE sessions (id TEXT, cwd TEXT, model TEXT)",
                "CREATE TABLE session_usage (session_id TEXT, used INTEGER, updated_at INTEGER)",
                "INSERT INTO sessions VALUES ('s','/Users/me/Code/Pulse','priced')",
                &format!("INSERT INTO session_usage VALUES ('s', 100, {today})"),
            ],
        );
        let cache = tempfile::tempdir().unwrap();
        let prices: PriceTable = std::collections::HashMap::from([("priced".to_string(), ModelPrice::new(1_000.0, 10_000.0, Some(100.0), Some(1_000.0), Some("Priced")))]);
        let ledger = read_agent(SpendAgent::WorkBuddy, &Sources::new(home.path()), cache.path(), &calendar, &prices);
        let summary = SpendSummary::of(&std::collections::HashMap::from([(SpendAgent::WorkBuddy, ledger)]), Some(1), now, &calendar);
        assert_eq!(summary.tokens, 100);
        assert_eq!(summary.sessions[0].session.tokens, 100);
    }
}
