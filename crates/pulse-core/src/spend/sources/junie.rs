// Ported from upstream Sources/Pulse/Usage/Readers/JunieUsageReader.swift.
//! JetBrains Junie's event stream.
//!
//! `~/.junie/sessions/<session-id>/events.jsonl` carries one event per line. A usage event is one
//! whose `event.agentEvent.kind` is `LlmResponseMetadataEvent` and which holds a `modelUsage[]`; every
//! entry in that array is one provider call. Prompt and lifecycle events carry no usage and are
//! skipped.
//!
//! **The timestamp is anchored to the response's start.** `timestampMs` is the response's end and
//! `usage.time` is its latency, so a row with a positive latency is dated at `timestampMs - time`. A
//! row with no latency keeps the end time. A zero `timestampMs` is unset, and the record falls back to
//! the session id's own date (`session-YYMMDD-HHMMSS`, local time).
//!
//! **Reasoning is left out.** Junie lists reasoning beside output with no total or statement of
//! containment, so the reported output is kept, the reasoning figure is not counted, and the record is
//! marked partial. Cost is the product's own dollars and is not read as tokens.
//!
//! Where it lives on Windows: `%USERPROFILE%\.junie\sessions`, the macOS layout carried over.
//! WINDOWS-PATH: unverified.

use std::path::PathBuf;

use chrono::{DateTime, Local, NaiveDateTime, TimeZone, Utc};
use serde_json::Value;

use super::{push_unique, SpendSource};
use crate::spend::logio;
use crate::spend::record::AgentUsageRecord;
use crate::spend::tally::TokenTally;
use crate::spend::transcripts::Sources;

const INPUT: &[&str] = &["inputTokens", "input"];
const OUTPUT: &[&str] = &["outputTokens", "output"];
const CACHE_READ: &[&str] = &["cacheInputTokens", "cacheReadInputTokens", "cacheRead"];
const CACHE_WRITE: &[&str] = &["cacheCreateTokens", "cacheCreationInputTokens", "cacheWrite"];
const REASONING: &[&str] = &["reasoningTokens", "reasoningOutputTokens", "thinkingTokens"];

pub struct Junie;

impl SpendSource for Junie {
    fn raw(&self) -> &'static str {
        "junie"
    }
    fn source_id(&self) -> &'static str {
        "junie"
    }
    fn display_name(&self) -> &'static str {
        "Junie"
    }
    fn icon(&self) -> Option<&'static str> {
        Some("junie")
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        let mut roots = Vec::new();
        // WINDOWS-PATH: unverified (the macOS layout carried over)
        push_unique(&mut roots, sources.home.join(".junie").join("sessions"));
        roots
    }
    fn records(&self, roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        let mut records = Vec::new();
        for file in logio::files(roots, &[], &["events.jsonl"], &[]) {
            let session = file.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            // The session id encodes its own start: the real fallback when `timestampMs` is unset.
            let session_start = session_id_time(&session);

            for row in logio::json_lines(&file) {
                let Some(agent_event) = row.get("event").and_then(|e| e.get("agentEvent")).filter(|a| a.is_object()) else { continue };
                if logio::text(agent_event.get("kind")).as_deref() != Some("LlmResponseMetadataEvent") {
                    continue;
                }
                let Some(usages) = agent_event.get("modelUsage").and_then(Value::as_array) else { continue };

                let end = logio::timestamp(row.get("timestampMs"), true).filter(|t| t.timestamp() > 0);
                let agent = agent_event.get("agent");
                let agent_name = logio::text(agent.and_then(|a| a.get("name"))).or_else(|| logio::text(agent.and_then(|a| a.get("id"))));

                for (index, entry) in usages.iter().enumerate() {
                    let Some(model) = logio::text(entry.get("model")) else { continue };
                    let latency = logio::count(entry.get("time")).unwrap_or(0);
                    let Some(at) = event_time(end, latency, session_start) else { continue };

                    let reasoning = merged(entry, REASONING);
                    let tally = TokenTally::new(merged(entry, INPUT), merged(entry, CACHE_WRITE), merged(entry, CACHE_READ), merged(entry, OUTPUT));
                    let cost = logio::count(entry.get("cost")).unwrap_or(0);

                    let identity = format!(
                        "{session}:{}:{model}:{}:{}:{}:{}:{cost}:{index}",
                        at.timestamp(),
                        tally.input,
                        tally.cache_write,
                        tally.cache_read,
                        tally.output
                    );
                    if tally.total() <= 0 {
                        continue;
                    }
                    let mut record = AgentUsageRecord::new(at, &model, tally).session(&session);
                    record.is_partial = reasoning > 0;
                    record.session_name = agent_name.clone();
                    record.deduplication_id = Some(format!("junie:{identity}"));
                    records.push(record);
                }
            }
        }
        records
    }
}

/// The first non-zero count for `aliases` in one usage entry.
fn merged(entry: &Value, aliases: &[&str]) -> i64 {
    aliases.iter().find_map(|alias| logio::count(entry.get(*alias)).filter(|v| *v > 0)).unwrap_or(0)
}

/// The call's start when the response end is known, else the session's own start. A latency that would
/// place the start at or before the epoch is not used.
fn event_time(end: Option<DateTime<Utc>>, latency: i64, fallback: Option<DateTime<Utc>>) -> Option<DateTime<Utc>> {
    let Some(end) = end else { return fallback };
    if latency <= 0 {
        return Some(end);
    }
    let start = end - chrono::Duration::milliseconds(latency);
    Some(if start.timestamp() > 0 { start } else { end })
}

/// `session-YYMMDD-HHMMSS` in local time → its start, or None.
fn session_id_time(id: &str) -> Option<DateTime<Utc>> {
    let (_, rest) = id.split_once("session-")?;
    let stamp: String = rest.chars().take(13).collect();
    let naive = NaiveDateTime::parse_from_str(&stamp, "%y%m%d-%H%M%S").ok()?;
    Local.from_local_datetime(&naive).earliest().map(|t| t.with_timezone(&Utc))
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

    const SESSION: &str = "session-260914-102030";

    fn events(home: &std::path::Path, session: &str, rows: &[Value]) {
        let folder = home.join(".junie").join("sessions").join(session);
        std::fs::create_dir_all(&folder).unwrap();
        let text: Vec<String> = rows.iter().map(Value::to_string).collect();
        std::fs::write(folder.join("events.jsonl"), text.join("\n")).unwrap();
    }

    fn usage_row(timestamp_ms: i64, usages: Value) -> Value {
        json!({"timestampMs": timestamp_ms, "event": {"agentEvent": {"kind": "LlmResponseMetadataEvent", "agent": {"name": "Junie"}, "modelUsage": usages}}})
    }

    #[test]
    fn the_start_of_the_call_is_the_time_and_reasoning_is_left_out_and_partial() {
        let home = tempfile::tempdir().unwrap();
        let end = 1_789_381_230_000i64;
        events(home.path(), SESSION, &[usage_row(end, json!([
            {"model": "priced", "time": 2000, "inputTokens": 100, "cacheCreateTokens": 5, "cacheInputTokens": 20, "outputTokens": 30, "reasoningTokens": 7},
        ]))]);
        let records = Junie.records(&Junie.inputs(&Home::new(home.path())));
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].tally, TokenTally::new(100, 5, 20, 30));
        assert!(records[0].is_partial);
        assert_eq!(records[0].timestamp.timestamp_millis(), end - 2000);
        assert_eq!(records[0].session_name.as_deref(), Some("Junie"));
    }

    #[test]
    fn the_short_spelling_is_read_and_an_unset_time_falls_back_to_the_session_id() {
        let home = tempfile::tempdir().unwrap();
        events(home.path(), SESSION, &[usage_row(0, json!([{"model": "priced", "input": 8, "output": 2}]))]);
        let records = Junie.records(&Junie.inputs(&Home::new(home.path())));
        assert_eq!(records[0].tally, TokenTally::new(8, 0, 0, 2));
        let expected = Local.with_ymd_and_hms(2026, 9, 14, 10, 20, 30).unwrap().timestamp();
        assert_eq!(records[0].timestamp.timestamp(), expected);
        assert!(!records[0].is_partial);
    }

    #[test]
    fn a_non_usage_event_and_an_entry_without_a_model_are_skipped() {
        let home = tempfile::tempdir().unwrap();
        events(home.path(), SESSION, &[
            json!({"timestampMs": 1_789_381_230_000i64, "event": {"agentEvent": {"kind": "UserPromptEvent"}}}),
            usage_row(1_789_381_230_000, json!([{"inputTokens": 50}, {"model": "priced", "inputTokens": 4}])),
        ]);
        let records = Junie.records(&Junie.inputs(&Home::new(home.path())));
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].tally.input, 4);
    }

    #[test]
    fn a_missing_store_is_an_empty_ledger() {
        let empty = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::Junie, &Home::new(empty.path()), cache.path(), &Calendar::utc(2), &PriceTable::new());
        assert!(ledger.days.is_empty());
    }

    #[test]
    fn a_session_buckets_reach_today_and_only_todays_counts() {
        use chrono::Duration;
        let home = tempfile::tempdir().unwrap();
        let calendar = Calendar::local();
        let now = Utc::now();
        let ms = |day: DateTime<Utc>| (calendar.start_of_day(day) + Duration::hours(12)).timestamp_millis();
        let yesterday = calendar.add_days(calendar.start_of_day(now), -1);
        events(home.path(), "session-260914-102030", &[usage_row(ms(now), json!([{"model": "priced", "inputTokens": 100}])), usage_row(ms(yesterday), json!([{"model": "priced", "inputTokens": 900}]))]);
        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::Junie, &Home::new(home.path()), cache.path(), &calendar, &PriceTable::new());
        let summary = SpendSummary::of(&std::collections::HashMap::from([(SpendAgent::Junie, ledger)]), Some(1), now, &calendar);
        assert_eq!(summary.tokens, 100);
        assert_eq!(summary.sessions[0].session.unpriced_tokens, 100);
    }
}
