// Ported from upstream Sources/Pulse/Usage/Readers/AugmentUsageReader.swift.
//! Augment's per-session chat history.
//!
//! `~/.augment/sessions/<sessionId>.json` holds `sessionId`, `agentState.modelId` and `chatHistory[]`.
//! Each turn is `{ "finishedAt", "completed", "sequenceId", "exchange": { "model_id", "request_id",
//! "response_nodes": [{ "token_usage": {...} }] } }`.
//!
//! **Only completed turns count.** An aborted or in-progress turn can carry a partial streamed total;
//! emitting it would put a snapshot of unfinished work into the ledger.
//!
//! **The last non-empty `token_usage` wins, never the sum.** A turn's response is streamed as several
//! nodes whose usage is cumulative; adding them would multiply the turn.
//!
//! Input and cache are independent here, so they are read straight across. Credits are a cost, not a
//! kind, and are left for Pulse's price table. `finishedAt` is the only time the turn has.
//!
//! Where it lives on Windows: `%USERPROFILE%\.augment\sessions`, the macOS layout carried over.
//! WINDOWS-PATH: unverified.

use std::path::PathBuf;

use serde_json::Value;

use super::{push_unique, SpendSource};
use crate::provider::Provider;
use crate::spend::logio;
use crate::spend::record::AgentUsageRecord;
use crate::spend::tally::TokenTally;
use crate::spend::transcripts::Sources;

pub struct Augment;

impl SpendSource for Augment {
    fn raw(&self) -> &'static str {
        "augment"
    }
    fn source_id(&self) -> &'static str {
        "augment"
    }
    fn display_name(&self) -> &'static str {
        "Augment"
    }
    fn card_provider(&self) -> Option<Provider> {
        Some(Provider::Augment)
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        let mut roots = Vec::new();
        // WINDOWS-PATH: unverified (the macOS layout carried over)
        push_unique(&mut roots, sources.home.join(".augment").join("sessions"));
        roots
    }
    fn records(&self, roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        let mut records = Vec::new();
        for file in logio::files(roots, &["json"], &[], &[]) {
            let Some(session) = logio::json(&file).filter(Value::is_object) else { continue };

            let stem = file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let session_id = logio::text(session.get("sessionId")).unwrap_or(stem);
            let agent_model = logio::text(session.get("agentState").and_then(|a| a.get("modelId")));
            let Some(turns) = session.get("chatHistory").and_then(Value::as_array) else { continue };

            for (index, turn) in turns.iter().enumerate() {
                // `completed` must be exactly true; absent or false is not a finished turn.
                if turn.get("completed").and_then(Value::as_bool) != Some(true) {
                    continue;
                }
                let Some(exchange) = turn.get("exchange").filter(|e| e.is_object()) else { continue };
                let Some(usage) = last_usage(exchange) else { continue };
                let Some(at) = logio::timestamp(turn.get("finishedAt"), false).filter(|t| t.timestamp() > 0) else { continue };

                let tally = tally_of(usage);
                let Some(model) = logio::text(exchange.get("model_id")).or_else(|| agent_model.clone()) else { continue };
                let identity = logio::text(exchange.get("request_id"))
                    .or_else(|| logio::text(turn.get("sequenceId")))
                    .unwrap_or_else(|| index.to_string());

                let mut record = AgentUsageRecord::new(at, &model, tally).session(&session_id);
                record.session_name = Some(session_id.clone());
                record.deduplication_id = Some(format!("augment:{session_id}:{identity}"));
                records.push(record);
            }
        }
        records
    }
}

/// The four kinds of one `token_usage` object.
fn tally_of(usage: &Value) -> TokenTally {
    let count = |key: &str| logio::count(usage.get(key)).unwrap_or(0);
    TokenTally::new(
        count("input_tokens"),
        count("cache_creation_input_tokens"),
        count("cache_read_input_tokens"),
        count("output_tokens"),
    )
}

/// The last `response_nodes` entry whose `token_usage` reports anything.
fn last_usage(exchange: &Value) -> Option<&Value> {
    let nodes = exchange.get("response_nodes").and_then(Value::as_array)?;
    nodes
        .iter()
        .rev()
        .filter_map(|node| node.get("token_usage").filter(|u| u.is_object()))
        .find(|usage| tally_of(usage).total() > 0)
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

    fn session(home: &std::path::Path, id: &str, body: Value) {
        let folder = home.join(".augment").join("sessions");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join(format!("{id}.json")), body.to_string()).unwrap();
    }

    fn turn(sequence: &str, finished: &str, completed: Value, nodes: Value) -> Value {
        json!({"sequenceId": sequence, "finishedAt": finished, "completed": completed,
            "exchange": {"model_id": "priced", "request_id": format!("req-{sequence}"), "response_nodes": nodes}})
    }

    #[test]
    fn the_last_non_empty_usage_wins_and_is_not_summed_across_streamed_nodes() {
        let home = tempfile::tempdir().unwrap();
        session(home.path(), "s1", json!({"sessionId": "s1", "agentState": {"modelId": "priced"}, "chatHistory": [
            turn("1", "2026-09-14T10:00:00Z", json!(true), json!([
                {"token_usage": {"input_tokens": 10, "output_tokens": 1}},
                {"token_usage": {"input_tokens": 40, "cache_creation_input_tokens": 5, "cache_read_input_tokens": 20, "output_tokens": 9}},
                {"token_usage": {}},
            ])),
        ]}));
        let records = Augment.records(&Augment.inputs(&Home::new(home.path())));
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].tally, TokenTally::new(40, 5, 20, 9));
        assert_eq!(records[0].session_name.as_deref(), Some("s1"));
        assert_eq!(Augment.card_provider(), Some(Provider::Augment));
    }

    #[test]
    fn an_unfinished_turn_a_turn_without_a_time_and_a_turn_without_counts_are_skipped() {
        let home = tempfile::tempdir().unwrap();
        session(home.path(), "s2", json!({"sessionId": "s2", "agentState": {"modelId": "priced"}, "chatHistory": [
            turn("a", "2026-09-14T10:00:00Z", json!(false), json!([{"token_usage": {"input_tokens": 5}}])),
            turn("b", "2026-09-14T10:00:00Z", json!("true"), json!([{"token_usage": {"input_tokens": 5}}])),
            turn("c", "0", json!(true), json!([{"token_usage": {"input_tokens": 5}}])),
            turn("d", "2026-09-14T10:00:00Z", json!(true), json!([{"token_usage": {"input_tokens": 0}}])),
            turn("e", "2026-09-14T10:00:00Z", json!(true), json!([{"token_usage": {"input_tokens": 6}}])),
        ]}));
        let records = Augment.records(&Augment.inputs(&Home::new(home.path())));
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].tally.input, 6);
    }

    #[test]
    fn the_same_request_twice_counts_once_and_a_missing_model_falls_back_to_the_agent() {
        let home = tempfile::tempdir().unwrap();
        let mut repeated = turn("x", "2026-09-14T10:00:00Z", json!(true), json!([{"token_usage": {"output_tokens": 4}}]));
        repeated["exchange"]["model_id"] = Value::Null;
        session(home.path(), "s3", json!({"sessionId": "s3", "agentState": {"modelId": "priced"}, "chatHistory": [repeated.clone(), repeated]}));
        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::Augment, &Home::new(home.path()), cache.path(), &Calendar::utc(2), &PriceTable::new());
        assert_eq!(ledger.days.iter().map(|d| d.tokens).sum::<i64>(), 4);
    }

    #[test]
    fn a_missing_store_is_an_empty_ledger() {
        let empty = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::Augment, &Home::new(empty.path()), cache.path(), &Calendar::utc(2), &PriceTable::new());
        assert!(ledger.days.is_empty());
    }

    #[test]
    fn a_session_buckets_reach_today_and_only_todays_counts() {
        use chrono::{Duration, Utc};
        let home = tempfile::tempdir().unwrap();
        let calendar = Calendar::local();
        let now = Utc::now();
        let today = (calendar.start_of_day(now) + Duration::hours(12)).to_rfc3339();
        let yesterday = (calendar.add_days(calendar.start_of_day(now), -1) + Duration::hours(12)).to_rfc3339();
        session(home.path(), "s4", json!({"sessionId": "s4", "agentState": {"modelId": "priced"}, "chatHistory": [
            turn("t", &today, json!(true), json!([{"token_usage": {"input_tokens": 100}}])),
            turn("y", &yesterday, json!(true), json!([{"token_usage": {"input_tokens": 900}}])),
        ]}));
        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::Augment, &Home::new(home.path()), cache.path(), &calendar, &PriceTable::new());
        let summary = SpendSummary::of(&std::collections::HashMap::from([(SpendAgent::Augment, ledger)]), Some(1), now, &calendar);
        assert_eq!(summary.tokens, 100);
        assert_eq!(summary.sessions[0].session.unpriced_tokens, 100);
    }
}
