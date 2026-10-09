// Ported from upstream Sources/Pulse/Usage/Readers/KiroReader.swift.
//! Kiro's CLI session tree, `~/.kiro/sessions/cli/`. Each `*.json` header is a session, and the same
//! stem's `.jsonl` is its conversation.
//!
//! **Only real counters are read.** A turn's `input_token_count` and `output_token_count` are the
//! one measured source in Kiro's formats. The estimates (a context-window percentage, a character
//! count divided by four, the IDE `session.json` tree and the `kiro-cli` SQLite) are not read. A turn
//! whose explicit counters are both zero produces no record: a zero is not a measurement.
//!
//! No cache or reasoning counter exists in Kiro, so none is reported. A turn is dated by its prompt's
//! own time in the `.jsonl`, else by its `end_timestamp`; a turn with neither is skipped.
//!
//! Where it lives on Windows: upstream's `~/.kiro` is read under `%USERPROFILE%`. Unverified on a PC.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::{clamped, epoch, number, SpendSource};
use crate::provider::Provider;
use crate::spend::logio;
use crate::spend::record::AgentUsageRecord;
use crate::spend::tally::TokenTally;
use crate::spend::transcripts::Sources;

pub struct Kiro;

impl SpendSource for Kiro {
    fn raw(&self) -> &'static str {
        "kiro"
    }
    fn source_id(&self) -> &'static str {
        "kiro"
    }
    fn display_name(&self) -> &'static str {
        "Kiro"
    }
    fn card_provider(&self) -> Option<Provider> {
        Some(Provider::Kiro)
    }
    fn icon(&self) -> Option<&'static str> {
        Some("kiro")
    }
    fn reports_cache_reads(&self) -> bool {
        false
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        // WINDOWS-PATH: unverified (the CLI tree under the user profile, as on macOS)
        vec![sources.home.join(".kiro").join("sessions").join("cli")]
    }
    fn records(&self, roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        logio::files(roots, &["json"], &[], &[]).iter().flat_map(|header| read(header)).collect()
    }
}

fn read(header_file: &Path) -> Vec<AgentUsageRecord> {
    let Some(header) = logio::json(header_file) else { return Vec::new() };
    let Some(header) = header.as_object() else { return Vec::new() };

    let session = header
        .get("session_id")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| header_file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
    let model = model_id(&header).unwrap_or_else(|| "auto".to_string());
    let project = header.get("cwd").and_then(Value::as_str).filter(|c| !c.trim().is_empty()).map(str::to_string);
    let prompts = prompt_timestamps(header_file);

    let Some(turns) = header
        .get("session_state")
        .and_then(|state| state.get("conversation_metadata"))
        .and_then(|meta| meta.get("user_turn_metadatas"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };

    let mut records = Vec::new();
    for (index, turn) in turns.iter().enumerate() {
        let input = clamped(turn.get("input_token_count"));
        let output = clamped(turn.get("output_token_count"));
        // Both zero: nothing measured here.
        if input <= 0 && output <= 0 {
            continue;
        }
        // The prompt's own time, else the turn's end time.
        let prompt = turn
            .get("message_ids")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter_map(|id| prompts.get(id).copied())
            .min();
        let end = turn.get("end_timestamp").and_then(number).and_then(epoch);
        let Some(at) = prompt.or(end) else { continue };

        let mut record = AgentUsageRecord::new(at, &model, TokenTally { input, output, ..TokenTally::default() }).session(&session);
        record.project = project.clone();
        record.deduplication_id = Some(format!("{session}:{index}"));
        records.push(record);
    }
    records
}

/// `session_state.rts_model_state.model_info.model_id`, when it names one.
fn model_id(header: &serde_json::Map<String, Value>) -> Option<String> {
    let id = header.get("session_state")?.get("rts_model_state")?.get("model_info")?.get("model_id")?.as_str()?;
    (!id.trim().is_empty()).then(|| id.to_string())
}

/// The conversation sidecar's prompt times by message id.
fn prompt_timestamps(header_file: &Path) -> HashMap<String, DateTime<Utc>> {
    let sidecar = header_file.with_extension("jsonl");
    let mut times = HashMap::new();
    for line in logio::json_lines(&sidecar) {
        if line.get("kind").and_then(Value::as_str) != Some("Prompt") {
            continue;
        }
        let Some(data) = line.get("data") else { continue };
        let Some(message) = data.get("message_id").and_then(Value::as_str) else { continue };
        let Some(seconds) = data.get("meta").and_then(|meta| meta.get("timestamp")).and_then(number) else { continue };
        if let Some(at) = epoch(seconds) {
            times.insert(message.to_string(), at);
        }
    }
    times
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::spend::calendar::Calendar;
    use crate::spend::prices::PriceTable;
    use crate::spend::sources::{read_agent, SpendAgent};

    fn session_dir(home: &Path) -> PathBuf {
        let dir = home.join(".kiro").join("sessions").join("cli");
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn header(turns: Value) -> Value {
        json!({
            "session_id": "abc",
            "cwd": "C:\\Users\\me\\code\\Pulse",
            "session_state": {
                "rts_model_state": {"model_info": {"model_id": "claude-sonnet"}},
                "conversation_metadata": {"user_turn_metadatas": turns}
            }
        })
    }

    #[test]
    fn explicit_counters_are_read_and_a_turn_dated_by_its_prompt_or_its_end() {
        let home = tempfile::tempdir().unwrap();
        let dir = session_dir(home.path());
        let turns = json!([
            {"input_token_count": 100, "output_token_count": 20, "message_ids": ["m1"], "end_timestamp": 1_789_000_000},
            {"input_token_count": 0, "output_token_count": 0, "message_ids": ["m2"], "end_timestamp": 1_789_000_100},
            {"input_token_count": "7", "output_token_count": 3, "message_ids": ["nope"], "end_timestamp": 1_789_372_800}
        ]);
        std::fs::write(dir.join("abc.json"), header(turns).to_string()).unwrap();
        std::fs::write(
            dir.join("abc.jsonl"),
            format!(
                "{}\n{}\n",
                json!({"kind": "Prompt", "data": {"message_id": "m1", "meta": {"timestamp": 1_789_372_000}}}),
                json!({"kind": "AssistantMessage", "data": {"message_id": "m1"}})
            ),
        )
        .unwrap();

        let records = Kiro.records(&Kiro.inputs(&Sources::new(home.path())));
        assert_eq!(records.len(), 2, "the zero turn is no record");
        let first = &records[0];
        assert_eq!(first.model, "claude-sonnet");
        assert_eq!(first.tally, TokenTally::new(100, 0, 0, 20));
        assert_eq!(first.timestamp.timestamp(), 1_789_372_000, "the prompt's own time wins");
        assert_eq!(first.session_id.as_deref(), Some("abc"));
        assert_eq!(first.project.as_deref(), Some("C:\\Users\\me\\code\\Pulse"));
        assert_eq!(first.deduplication_id.as_deref(), Some("abc:0"));
        let third = &records[1];
        assert_eq!(third.tally, TokenTally::new(7, 0, 0, 3));
        assert_eq!(third.timestamp.timestamp(), 1_789_372_800, "no prompt line, so the end time");
    }

    #[test]
    fn a_header_with_no_model_is_auto_and_a_missing_tree_is_an_empty_ledger() {
        let home = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::Kiro, &Sources::new(home.path()), cache.path(), &Calendar::utc(2), &PriceTable::new());
        assert!(ledger.days.is_empty());

        let dir = session_dir(home.path());
        let bare = json!({"session_state": {"conversation_metadata": {"user_turn_metadatas": [
            {"input_token_count": 5, "output_token_count": 0, "end_timestamp": 1_789_372_800}
        ]}}});
        std::fs::write(dir.join("bare.json"), bare.to_string()).unwrap();
        let records = Kiro.records(&Kiro.inputs(&Sources::new(home.path())));
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].model, "auto");
        assert_eq!(records[0].session_id.as_deref(), Some("bare"), "no session_id: the file stem names it");
    }
}
