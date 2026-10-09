// Ported from upstream Sources/Pulse/Usage/Readers/GJCUsageReader.swift.
//! Gajae Code's (`gjc`) JSONL session files.
//!
//! A transcript opens with `{"type":"session","id","timestamp","cwd"}` and then carries
//! `{"type":"message","id","message":{"role","model","provider","timestamp","usage":{"input","output",
//! "cacheRead","cacheWrite","totalTokens"}}}`. Service-tier and unknown event types are ignored.
//!
//! **Only assistant rows with a model and usage emit.** A user row has no usage; a message with no
//! model names nothing to price.
//!
//! **Identity is the entry id, and only that.** A row with an `id` folds against the same id
//! (`<session>:<entryId>`), so a replay of that row counts once. A row without an id is its own
//! request and is always counted: identical figures are not evidence that two calls are one. A
//! byte-for-byte mirror file (the same session id and file name at two depths) is read once.
//!
//! `cost.total` is the product's own dollars and is not read as tokens. Reasoning is not exposed by
//! this usage shape, so output is taken as reported.
//!
//! Where it lives on Windows: `GJC_CODING_AGENT_DIR` or `%USERPROFILE%\.gjc\agent`, `<X>\agent` for
//! `GJC_CONFIG_DIR` / `PI_CONFIG_DIR`, and `$XDG_DATA_HOME\gjc\sessions` (default
//! `%USERPROFILE%\.local\share`). WINDOWS-PATH: unverified.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{push_unique, SpendSource};
use crate::spend::logio;
use crate::spend::record::AgentUsageRecord;
use crate::spend::tally::TokenTally;
use crate::spend::transcripts::Sources;

pub struct Gjc;

impl SpendSource for Gjc {
    fn raw(&self) -> &'static str {
        "gjc"
    }
    fn source_id(&self) -> &'static str {
        "gjc"
    }
    fn display_name(&self) -> &'static str {
        "Gajae Code"
    }
    fn env_vars(&self) -> &'static [&'static str] {
        &["GJC_CODING_AGENT_DIR", "GJC_CONFIG_DIR", "PI_CONFIG_DIR", "XDG_DATA_HOME"]
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        let mut roots = Vec::new();
        // WINDOWS-PATH: unverified
        let agent = sources.var("GJC_CODING_AGENT_DIR").unwrap_or_else(|| sources.home.join(".gjc").join("agent"));
        push_unique(&mut roots, agent.join("sessions"));
        for key in ["GJC_CONFIG_DIR", "PI_CONFIG_DIR"] {
            if let Some(base) = sources.var(key) {
                push_unique(&mut roots, base.join("agent").join("sessions"));
            }
        }
        // `$XDG_DATA_HOME/gjc/sessions`, or its default `~/.local/share` spelling.
        push_unique(&mut roots, sources.xdg_data_home().join("gjc").join("sessions"));
        roots
    }
    fn records(&self, roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        let mut records = Vec::new();
        // A session id and file name map to the digests already read; only a byte-for-byte mirror of
        // one of them is skipped.
        let mut seen_files: HashMap<String, HashSet<String>> = HashMap::new();

        for file in logio::files(roots, &["jsonl"], &[], &[]) {
            let Ok(bytes) = std::fs::read(&file) else { continue };
            let digest = format!("{:x}", Sha256::digest(&bytes));

            // The header is read first so a mirror can be recognised before any of its lines is counted.
            let mut header_id: Option<String> = None;
            let mut workspace: Option<String> = None;
            for event in logio::json_lines(&file).filter(|e| logio::text(e.get("type")).as_deref() == Some("session")) {
                if header_id.is_none() {
                    header_id = logio::text(event.get("id"));
                }
                if workspace.is_none() {
                    workspace = logio::text(event.get("cwd"));
                }
            }

            // One session written twice at two depths shares both its id and its file name. That is read
            // once only when the bytes are identical.
            if let Some(id) = &header_id {
                let mirror = format!("{id}|{}", file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
                if !seen_files.entry(mirror).or_default().insert(digest) {
                    continue;
                }
            }

            let session = header_id.clone().unwrap_or_else(|| file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());

            for event in logio::json_lines(&file) {
                if logio::text(event.get("type")).as_deref() != Some("message") {
                    continue;
                }
                let Some(message) = event.get("message").filter(|m| m.is_object()) else { continue };
                if logio::text(message.get("role")).map(|r| r.to_lowercase()).as_deref() != Some("assistant") {
                    continue;
                }
                let Some(usage) = message.get("usage").filter(|u| u.is_object()) else { continue };
                let Some(model) = logio::text(message.get("model")) else { continue };
                let Some(at) = timestamp(message, &event) else { continue };

                let count = |key: &str| logio::count(usage.get(key)).unwrap_or(0);
                let tally = TokenTally::new(count("input"), count("cacheWrite"), count("cacheRead"), count("output"));
                if tally.total() <= 0 {
                    continue;
                }

                // Only a real entry id is an identity. Without one the record carries none.
                let identity = logio::text(event.get("id")).map(|id| format!("gjc:{session}:{id}"));

                let mut record = AgentUsageRecord::new(at, &model, tally).session(&session);
                record.session_name = Some(session.clone());
                record.project = workspace.clone();
                record.deduplication_id = identity;
                records.push(record);
            }
        }
        records
    }
}

/// The message's own unix-millisecond time, else the envelope's RFC 3339 time. Zero is unset.
fn timestamp(message: &Value, event: &Value) -> Option<DateTime<Utc>> {
    logio::timestamp(message.get("timestamp"), true)
        .filter(|t| t.timestamp() > 0)
        .or_else(|| logio::timestamp(event.get("timestamp"), false).filter(|t| t.timestamp() > 0))
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

    fn store(dir: &std::path::Path, lines: &[Value]) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let file = dir.join("session.jsonl");
        let text: Vec<String> = lines.iter().map(Value::to_string).collect();
        std::fs::write(&file, text.join("\n")).unwrap();
        file
    }

    fn header() -> Value {
        json!({"type": "session", "id": "sess-1", "timestamp": "2026-09-14T10:00:00Z", "cwd": "C:\\Users\\me\\Code\\Pulse"})
    }

    fn row(id: Option<&str>, usage: Value) -> Value {
        let mut event = json!({"type": "message", "message": {"role": "assistant", "model": "priced", "timestamp": 1_789_381_200_000i64, "usage": usage}});
        if let Some(id) = id {
            event["id"] = json!(id);
        }
        event
    }

    fn sessions_dir(home: &std::path::Path) -> PathBuf {
        home.join(".gjc").join("agent").join("sessions")
    }

    #[test]
    fn the_four_kinds_and_the_header_fields_come_through() {
        let home = tempfile::tempdir().unwrap();
        store(&sessions_dir(home.path()).join("a"), &[header(), row(Some("e1"), json!({"input": 10, "cacheWrite": 5, "cacheRead": 20, "output": 30, "totalTokens": 65}))]);
        let records = Gjc.records(&Gjc.inputs(&Home::new(home.path())));
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].tally, TokenTally::new(10, 5, 20, 30));
        assert_eq!(records[0].session_id.as_deref(), Some("sess-1"));
        assert_eq!(records[0].project.as_deref(), Some("C:\\Users\\me\\Code\\Pulse"));
        assert_eq!(records[0].deduplication_id.as_deref(), Some("gjc:sess-1:e1"));
    }

    #[test]
    fn a_user_row_a_row_without_a_model_or_a_time_is_skipped_and_the_envelope_time_is_the_fallback() {
        let home = tempfile::tempdir().unwrap();
        let user = json!({"type": "message", "message": {"role": "user", "model": "priced", "usage": {"input": 9}}});
        let no_model = json!({"type": "message", "message": {"role": "assistant", "usage": {"input": 9}, "timestamp": 1_789_381_200_000i64}});
        let no_time = json!({"type": "message", "message": {"role": "assistant", "model": "priced", "usage": {"input": 9}}});
        let envelope = json!({"type": "message", "timestamp": "2026-09-14T10:00:00Z", "message": {"role": "assistant", "model": "priced", "usage": {"input": 9}}});
        store(&sessions_dir(home.path()).join("a"), &[header(), user, no_model, no_time, envelope]);
        let records = Gjc.records(&Gjc.inputs(&Home::new(home.path())));
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].tally.input, 9);
    }

    #[test]
    fn a_replayed_id_counts_once_and_an_id_less_row_is_always_its_own_request() {
        let home = tempfile::tempdir().unwrap();
        let usage = json!({"input": 4, "output": 1});
        store(&sessions_dir(home.path()).join("a"), &[header(), row(Some("same"), usage.clone()), row(Some("same"), usage.clone()), row(None, usage.clone()), row(None, usage)]);
        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::Gjc, &Home::new(home.path()), cache.path(), &Calendar::utc(2), &PriceTable::new());
        // one "same" plus two id-less rows: three requests of 5 tokens.
        assert_eq!(ledger.days.iter().map(|d| d.tokens).sum::<i64>(), 15);
    }

    #[test]
    fn a_byte_identical_mirror_is_read_once_and_a_fuller_copy_with_the_same_name_is_read_in_full() {
        let home = tempfile::tempdir().unwrap();
        let lines = [header(), row(Some("m1"), json!({"input": 3, "output": 1}))];
        store(&sessions_dir(home.path()).join("a"), &lines);
        store(&sessions_dir(home.path()).join("b"), &lines);
        assert_eq!(Gjc.records(&Gjc.inputs(&Home::new(home.path()))).len(), 1);

        let fuller = [header(), row(Some("m1"), json!({"input": 3, "output": 1})), row(Some("m2"), json!({"input": 6, "output": 2}))];
        store(&sessions_dir(home.path()).join("c"), &fuller);
        // a and b are identical, so b is skipped; c is not a mirror, so it is parsed in full and its "m1" folds in the builder.
        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::Gjc, &Home::new(home.path()), cache.path(), &Calendar::utc(2), &PriceTable::new());
        assert_eq!(ledger.days.iter().map(|d| d.tokens).sum::<i64>(), 4 + 8);
    }

    #[test]
    fn a_missing_store_is_empty_and_the_agent_dir_override_moves_it() {
        let empty = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::Gjc, &Home::new(empty.path()), cache.path(), &Calendar::utc(2), &PriceTable::new());
        assert!(ledger.days.is_empty());

        let home = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        store(&elsewhere.path().join("sessions"), &[header(), row(Some("x"), json!({"input": 2}))]);
        let sources = Home::new(home.path()).with_var("GJC_CODING_AGENT_DIR", elsewhere.path());
        assert_eq!(Gjc.records(&Gjc.inputs(&sources)).len(), 1);
    }

    #[test]
    fn a_session_buckets_reach_today_and_only_todays_counts() {
        use chrono::Duration;
        let home = tempfile::tempdir().unwrap();
        let calendar = Calendar::local();
        let now = Utc::now();
        let ms = |day: DateTime<Utc>| (calendar.start_of_day(day) + Duration::hours(12)).timestamp_millis();
        let at = |id: &str, when: i64, input: i64| json!({"type": "message", "id": id, "message": {"role": "assistant", "model": "priced", "timestamp": when, "usage": {"input": input}}});
        let yesterday = calendar.add_days(calendar.start_of_day(now), -1);
        store(&sessions_dir(home.path()).join("a"), &[header(), at("t", ms(now), 100), at("y", ms(yesterday), 900)]);
        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::Gjc, &Home::new(home.path()), cache.path(), &calendar, &PriceTable::new());
        let summary = SpendSummary::of(&std::collections::HashMap::from([(SpendAgent::Gjc, ledger)]), Some(1), now, &calendar);
        assert_eq!(summary.tokens, 100);
        assert_eq!(summary.sessions[0].session.unpriced_tokens, 100);
    }
}
