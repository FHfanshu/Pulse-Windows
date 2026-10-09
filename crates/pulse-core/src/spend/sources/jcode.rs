// Ported from upstream Sources/Pulse/Usage/Readers/JcodeUsageReader.swift.
//! Jcode's sessions and their append-only journals.
//!
//! `~/.jcode/sessions/session_*.json` (or `$JCODE_HOME/sessions`) carries `id`, `provider_key`,
//! `model`, `working_dir` and `messages[]`; the sidecar `session_*.journal.jsonl` carries lines of
//! `{ "meta": {...}, "append_messages": [...] }`. The journal is replayed into the session before
//! anything is emitted, so a message written once and replayed once is one record.
//!
//! **A message's cache shape is settled only by an explicit marker.** The Anthropic-style
//! `cache_creation_input_tokens` key means `input_tokens` is already cache-exclusive; an
//! OpenAI-native details object means the cached tokens are a subset of input. A positive
//! `cache_read_input_tokens` with neither marker is ambiguous: the input is carried as unclassified
//! (never priced as fresh), the cache is not added, and the record is marked partial. Magnitude is
//! never used to infer the convention.
//!
//! **Reasoning is left out and marks the record partial.** Jcode reports `reasoning_output_tokens`
//! beside output and gives no total, so nothing says whether it is inside output. The reported
//! output is kept, the reasoning figure is not counted, and the record is marked partial.
//!
//! A message with no `token_usage` emits nothing, and a message whose timestamp is missing is
//! skipped rather than dated from the file.
//!
//! Where it lives on Windows: `JCODE_HOME` when set, else `%USERPROFILE%\.jcode`.
//! WINDOWS-PATH: unverified.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::{push_unique, SpendSource};
use crate::spend::logio;
use crate::spend::record::AgentUsageRecord;
use crate::spend::tally::TokenTally;
use crate::spend::transcripts::Sources;

pub struct JCode;

impl SpendSource for JCode {
    fn raw(&self) -> &'static str {
        "jcode"
    }
    fn source_id(&self) -> &'static str {
        "jcode"
    }
    fn display_name(&self) -> &'static str {
        "JCode"
    }
    fn env_vars(&self) -> &'static [&'static str] {
        &["JCODE_HOME"]
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        let mut roots = Vec::new();
        // WINDOWS-PATH: unverified (the macOS layout carried over)
        let base = sources.var("JCODE_HOME").unwrap_or_else(|| sources.home.join(".jcode"));
        push_unique(&mut roots, base.join("sessions"));
        roots
    }
    fn records(&self, roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        let mut records = Vec::new();
        for file in logio::files(roots, &["json", "jsonl"], &[], &[]) {
            let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if !name.starts_with("session_") || file.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Some(session) = logio::json(&file).filter(Value::is_object) else { continue };

            let stem = name.strip_suffix(".json").unwrap_or(&name).to_string();
            let session_id = logio::text(session.get("id")).unwrap_or(stem.clone());
            let workspace = logio::text(session.get("working_dir"));
            let session_model = logio::text(session.get("model"));
            let provider = logio::text(session.get("provider_key"));

            let mut messages: Vec<(Value, Option<String>)> = Vec::new();
            if let Some(rows) = session.get("messages").and_then(Value::as_array) {
                messages.extend(rows.iter().map(|m| (m.clone(), session_model.clone())));
            }

            // Journal lines update the model as they are replayed, so a message takes the meta that
            // was current when it was appended.
            let mut journal_model = session_model.clone();
            for line in logio::json_lines(&journal(&file, &stem)) {
                if let Some(model) = logio::text(line.get("meta").and_then(|m| m.get("model"))) {
                    journal_model = Some(model);
                }
                if let Some(appended) = line.get("append_messages").and_then(Value::as_array) {
                    messages.extend(appended.iter().map(|m| (m.clone(), journal_model.clone())));
                }
            }

            for (message, model) in messages {
                let Some(usage) = message.get("token_usage").filter(|u| u.is_object()) else { continue };
                let Some(at) = event_time(message.get("timestamp")) else { continue };
                let Some(model) = model.or_else(|| session_model.clone()) else { continue };

                let reported_input = count(usage, "input_tokens");
                let cache_read = count(usage, "cache_read_input_tokens");
                let cache_write = count(usage, "cache_creation_input_tokens");
                let reported_output = count(usage, "output_tokens");
                let reasoning = count(usage, "reasoning_output_tokens");

                let mut partial = reasoning > 0;
                let mut unclassified = 0;
                let tally = match cache_shape(usage) {
                    // The schema states input excludes the cache.
                    CacheShape::AnthropicDisjoint => TokenTally::new(reported_input, cache_write, cache_read, reported_output),
                    // An explicit native details field states the cache is a subset of the input.
                    CacheShape::OpenAiContained => {
                        TokenTally::new((reported_input - cache_read.min(reported_input)).max(0), cache_write, cache_read, reported_output)
                    }
                    CacheShape::Ambiguous if cache_read > 0 => {
                        // The input may or may not already contain the cache, so it cannot be priced as
                        // fresh: carried unclassified, and the record is partial.
                        unclassified = reported_input;
                        partial = true;
                        TokenTally::new(0, 0, 0, reported_output)
                    }
                    CacheShape::Ambiguous => TokenTally::new(reported_input, 0, 0, reported_output),
                };

                let name = logio::text(message.get("id")).unwrap_or_else(|| {
                    format!(
                        "{session_id}:{}:{model}:{reported_input}:{cache_write}:{cache_read}:{reported_output}",
                        at.timestamp_millis()
                    )
                });

                if model.trim().is_empty() || (tally.total() <= 0 && unclassified <= 0) {
                    continue;
                }
                let mut record = AgentUsageRecord::new(at, &model, tally).session(&session_id).unclassified(unclassified);
                record.is_partial = partial;
                record.session_name = provider.clone();
                record.project = workspace.clone();
                record.deduplication_id = Some(format!("jcode:{session_id}:{name}"));
                records.push(record);
            }
        }
        records
    }
}

/// How a usage object says its cached tokens relate to its input.
enum CacheShape {
    AnthropicDisjoint,
    OpenAiContained,
    Ambiguous,
}

/// Only an explicit marker settles it. Anything else is ambiguous.
fn cache_shape(usage: &Value) -> CacheShape {
    if usage.get("cache_creation_input_tokens").is_some() {
        return CacheShape::AnthropicDisjoint;
    }
    let native = ["prompt_tokens_details", "promptTokensDetails", "input_tokens_details", "inputTokensDetails"];
    if native.iter().any(|key| usage.get(*key).is_some()) {
        return CacheShape::OpenAiContained;
    }
    CacheShape::Ambiguous
}

/// A count under `key`, or zero.
fn count(usage: &Value, key: &str) -> i64 {
    logio::count(usage.get(key)).unwrap_or(0)
}

/// The timestamp in seconds; zero is "unset".
fn event_time(value: Option<&Value>) -> Option<DateTime<Utc>> {
    logio::timestamp(value, false).filter(|t| t.timestamp() > 0)
}

/// The journal beside a session file, whether or not it exists.
fn journal(file: &Path, stem: &str) -> PathBuf {
    file.parent().unwrap_or(Path::new("")).join(format!("{stem}.journal.jsonl"))
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

    fn session(home: &Path, id: &str, body: Value) -> PathBuf {
        let folder = home.join(".jcode").join("sessions");
        std::fs::create_dir_all(&folder).unwrap();
        let file = folder.join(format!("{id}.json"));
        std::fs::write(&file, body.to_string()).unwrap();
        file
    }

    fn message(id: &str, at: &str, usage: Value) -> Value {
        json!({"id": id, "timestamp": at, "token_usage": usage})
    }

    #[test]
    fn each_cache_shape_lands_where_upstream_says() {
        let home = tempfile::tempdir().unwrap();
        session(home.path(), "session_a", json!({"id": "a", "model": "priced", "working_dir": "C:\\Users\\me\\Code\\Pulse", "provider_key": "p", "messages": [
            message("anthropic", "2026-09-14T10:00:00Z", json!({"input_tokens": 100, "cache_creation_input_tokens": 20, "cache_read_input_tokens": 300, "output_tokens": 15})),
            message("openai", "2026-09-14T10:01:00Z", json!({"input_tokens": 300, "cache_read_input_tokens": 100, "prompt_tokens_details": {"cached_tokens": 100}, "output_tokens": 5})),
            message("ambiguous-cache", "2026-09-14T10:02:00Z", json!({"input_tokens": 500, "cache_read_input_tokens": 40, "output_tokens": 6})),
            message("ambiguous-fresh", "2026-09-14T10:03:00Z", json!({"input_tokens": 70, "output_tokens": 7})),
        ]}));
        let records = JCode.records(&JCode.inputs(&Home::new(home.path())));
        assert_eq!(records.len(), 4);
        assert_eq!(records[0].tally, TokenTally::new(100, 20, 300, 15));
        assert!(!records[0].is_partial);
        assert_eq!(records[1].tally, TokenTally::new(200, 0, 100, 5));
        // The ambiguous cache: input unclassified, cache not added, partial.
        assert_eq!(records[2].tally, TokenTally::new(0, 0, 0, 6));
        assert_eq!(records[2].unclassified_tokens, 500);
        assert!(records[2].is_partial);
        assert_eq!(records[3].tally, TokenTally::new(70, 0, 0, 7));
        assert_eq!(records[0].project.as_deref(), Some("C:\\Users\\me\\Code\\Pulse"));
        assert_eq!(records[0].session_name.as_deref(), Some("p"));
    }

    #[test]
    fn reasoning_is_left_out_and_marks_the_record_partial() {
        let home = tempfile::tempdir().unwrap();
        session(home.path(), "session_r", json!({"id": "r", "model": "priced", "messages": [
            message("m", "2026-09-14T10:00:00Z", json!({"input_tokens": 10, "cache_creation_input_tokens": 0, "output_tokens": 30, "reasoning_output_tokens": 7})),
        ]}));
        let records = JCode.records(&JCode.inputs(&Home::new(home.path())));
        assert_eq!(records[0].tally, TokenTally::new(10, 0, 0, 30));
        assert!(records[0].is_partial);
    }

    #[test]
    fn a_message_without_usage_or_time_is_skipped_and_a_journal_replay_counts_once() {
        let home = tempfile::tempdir().unwrap();
        session(home.path(), "session_j", json!({"id": "j", "model": "old", "messages": [
            message("shared", "2026-09-14T10:00:00Z", json!({"input_tokens": 9, "cache_creation_input_tokens": 0, "output_tokens": 1})),
            json!({"id": "no-usage", "timestamp": "2026-09-14T10:00:00Z"}),
            json!({"id": "no-time", "token_usage": {"input_tokens": 50, "output_tokens": 50, "cache_creation_input_tokens": 0}}),
        ]}));
        let journal = home.path().join(".jcode").join("sessions").join("session_j.journal.jsonl");
        let lines = [
            json!({"meta": {"model": "new"}, "append_messages": [
                {"id": "shared", "timestamp": "2026-09-14T10:00:00Z", "token_usage": {"input_tokens": 9, "cache_creation_input_tokens": 0, "output_tokens": 1}},
                {"id": "appended", "timestamp": "2026-09-14T10:05:00Z", "token_usage": {"input_tokens": 4, "cache_creation_input_tokens": 0, "output_tokens": 2}},
            ]}),
        ];
        std::fs::write(&journal, lines.iter().map(Value::to_string).collect::<Vec<_>>().join("\n")).unwrap();

        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::JCode, &Home::new(home.path()), cache.path(), &Calendar::utc(2), &PriceTable::new());
        // "shared" is the same message twice (session and journal) and counts once; "appended" counts.
        assert_eq!(ledger.days.iter().map(|d| d.tokens).sum::<i64>(), 9 + 1 + 4 + 2);
        assert!(ledger.unpriced_models.contains(&"new".to_string()));
    }

    #[test]
    fn a_missing_store_is_empty_and_the_home_override_moves_it() {
        let empty = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::JCode, &Home::new(empty.path()), cache.path(), &Calendar::utc(2), &PriceTable::new());
        assert!(ledger.days.is_empty());

        let home = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(elsewhere.path().join("sessions")).unwrap();
        std::fs::write(
            elsewhere.path().join("sessions").join("session_x.json"),
            json!({"id": "x", "model": "m", "messages": [message("x1", "2026-09-14T10:00:00Z", json!({"input_tokens": 3, "output_tokens": 1}))]}).to_string(),
        )
        .unwrap();
        let sources = Home::new(home.path()).with_var("JCODE_HOME", elsewhere.path());
        assert_eq!(JCode.records(&JCode.inputs(&sources)).len(), 1);
    }

    #[test]
    fn a_session_buckets_reach_today_and_only_todays_counts() {
        use chrono::Duration;
        let home = tempfile::tempdir().unwrap();
        let calendar = Calendar::local();
        let now = Utc::now();
        let today = (calendar.start_of_day(now) + Duration::hours(12)).to_rfc3339();
        let yesterday = (calendar.add_days(calendar.start_of_day(now), -1) + Duration::hours(12)).to_rfc3339();
        session(home.path(), "session_t", json!({"id": "t", "model": "priced", "messages": [
            message("a", &today, json!({"input_tokens": 100, "cache_creation_input_tokens": 0, "output_tokens": 0})),
            message("b", &yesterday, json!({"input_tokens": 900, "cache_creation_input_tokens": 0, "output_tokens": 0})),
        ]}));
        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::JCode, &Home::new(home.path()), cache.path(), &calendar, &PriceTable::new());
        let summary = SpendSummary::of(&std::collections::HashMap::from([(SpendAgent::JCode, ledger)]), Some(1), now, &calendar);
        assert_eq!(summary.tokens, 100);
        assert_eq!(summary.sessions[0].session.unpriced_tokens, 100);
    }
}
