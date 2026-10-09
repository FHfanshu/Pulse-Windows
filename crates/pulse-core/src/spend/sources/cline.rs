// Ported from upstream Sources/Pulse/Usage/Readers/VSCodeTaskLogReader.swift (client "cline") and Sources/Pulse/Usage/Readers/ClineCLIReader.swift.
//! Cline, in two stores:
//!
//! - The VS Code extension's task logs (`saoudrizwan.claude-dev`), the same shape as Roo Code's
//!   (`roocode.rs`).
//! - The Cline CLI's session store: a root with one directory per session, holding
//!   `<session>.messages.json` and a sibling `<session>.json` manifest. Only assistant messages
//!   with a `metrics` object count. The store's `inputTokens` is **cache-inclusive**, so fresh
//!   input is what is left after the two cache counts, clamped at zero.
//!
//! The CLI root comes from the environment the CLI itself checks, in order
//! (`CLINE_SESSION_DATA_DIR`, `CLINE_DATA_DIR\sessions`, `CLINE_DIR\data\sessions`), blank values
//! ignored; else `%USERPROFILE%\.cline\data\sessions`. A missing `ts` is not backfilled from a
//! file's date: the message is not a record.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use super::roocode::{task_records, task_roots};
use super::{push_unique, SpendSource};
use crate::spend::editorlog;
use crate::spend::logio;
use crate::spend::record::AgentUsageRecord;
use crate::spend::tally::TokenTally;
use crate::spend::transcripts::Sources;

pub struct Cline;

impl SpendSource for Cline {
    fn raw(&self) -> &'static str {
        "cline"
    }
    fn source_id(&self) -> &'static str {
        "cline"
    }
    fn display_name(&self) -> &'static str {
        "Cline"
    }
    fn icon(&self) -> Option<&'static str> {
        Some("cline")
    }
    fn price_vendor(&self) -> Option<&'static str> {
        Some("cline-pass")
    }
    fn env_vars(&self) -> &'static [&'static str] {
        &["APPDATA", "CLINE_SESSION_DATA_DIR", "CLINE_DATA_DIR", "CLINE_DIR"]
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        let mut roots = task_roots("saoudrizwan.claude-dev", sources);
        push_unique(&mut roots, cli_root(sources));
        roots
    }
    fn records(&self, roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        let mut records = task_records(roots);
        records.extend(cli_records(roots));
        records
    }
}

/// The CLI's session root, from the environment the CLI itself reads, else the home folder.
fn cli_root(sources: &Sources) -> PathBuf {
    if let Some(value) = sources.var("CLINE_SESSION_DATA_DIR") {
        return value;
    }
    if let Some(value) = sources.var("CLINE_DATA_DIR") {
        return value.join("sessions");
    }
    if let Some(value) = sources.var("CLINE_DIR") {
        return value.join("data").join("sessions");
    }
    sources.home.join(".cline").join("data").join("sessions")
}

fn cli_records(roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
    let mut files: Vec<PathBuf> = logio::files(roots, &["json"], &[], &[])
        .into_iter()
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with(".messages.json")))
        .collect();
    files.sort();
    files.iter().flat_map(|file| parse_session(file)).collect()
}

fn parse_session(file: &Path) -> Vec<AgentUsageRecord> {
    let name = file.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    let stem = name.strip_suffix(".messages.json").unwrap_or(name).to_string();
    let Some(root) = logio::json(file).and_then(|v| v.as_object().cloned()) else { return Vec::new() };
    let manifest: Option<Map<String, Value>> = file
        .parent()
        .map(|dir| dir.join(format!("{stem}.json")))
        .and_then(|path| logio::json(&path))
        .and_then(|v| v.as_object().cloned());
    let manifest_text = |key: &str| logio::text(manifest.as_ref().and_then(|m| m.get(key)));

    let messages = root.get("messages").and_then(Value::as_array).cloned().unwrap_or_default();

    let session = logio::text(root.get("sessionId")).or_else(|| manifest_text("sessionId")).unwrap_or_else(|| stem.clone());
    let project = manifest_text("workspace_root").or_else(|| manifest_text("cwd"));
    let title = manifest
        .as_ref()
        .and_then(|m| m.get("metadata"))
        .and_then(Value::as_object)
        .and_then(|meta| logio::text(meta.get("title")));
    let manifest_model = manifest_text("model");

    let mut records = Vec::new();
    for (index, message) in messages.iter().enumerate() {
        let Some(message) = message.as_object() else { continue };
        if logio::text(message.get("role")).as_deref() != Some("assistant") {
            continue;
        }
        let Some(metrics) = message.get("metrics").and_then(Value::as_object) else { continue };
        let Some(at) = logio::timestamp(message.get("ts"), true) else { continue };

        let cache_read = editorlog::int(metrics.get("cacheReadTokens"));
        let cache_write = editorlog::int(metrics.get("cacheWriteTokens"));
        let tally = TokenTally {
            input: (editorlog::int(metrics.get("inputTokens")) - cache_read - cache_write).max(0),
            cache_write,
            cache_read,
            output: editorlog::int(metrics.get("outputTokens")),
            ..TokenTally::default()
        };
        if tally.total() <= 0 {
            continue;
        }

        let model_info = message.get("modelInfo").and_then(Value::as_object);
        let Some(model) = logio::text(model_info.and_then(|m| m.get("id"))).or_else(|| manifest_model.clone()) else { continue };

        // The message's own id is the identity; a message with none keeps its line position.
        let message_id = logio::text(message.get("id")).unwrap_or_else(|| format!("line{index}"));
        let mut record = AgentUsageRecord::new(at, &model, tally).session(&session);
        record.title = title.clone();
        record.project = project.clone();
        record.deduplication_id = Some(format!("cline:{session}:{message_id}"));
        records.push(record);
    }
    records
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::spend::calendar::Calendar;
    use crate::spend::prices::PriceTable;
    use crate::spend::sources::{read_agent, SpendAgent};

    const MIDNIGHT: i64 = 1_789_344_000;
    fn at_ms(hour: i64) -> i64 {
        (MIDNIGHT + hour * 3600) * 1000
    }

    fn session(home: &Path, name: &str, messages: Vec<Value>, manifest: Option<Value>) {
        let dir = home.join(".cline").join("data").join("sessions").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{name}.messages.json")), json!({"agent": "cline", "messages": messages}).to_string()).unwrap();
        if let Some(manifest) = manifest {
            std::fs::write(dir.join(format!("{name}.json")), manifest.to_string()).unwrap();
        }
    }

    #[test]
    fn only_assistant_metrics_count_and_input_is_cache_exclusive_with_the_manifest_metadata() {
        let home = tempfile::tempdir().unwrap();
        session(
            home.path(),
            "sess-1",
            vec![
                json!({"id":"m1","role":"assistant","ts":at_ms(9),"modelInfo":{"id":"gpt-5"},"metrics":{"inputTokens":100,"outputTokens":40,"cacheReadTokens":30,"cacheWriteTokens":20}}),
                // A user turn carries no metrics; assistant metrics with no time, and none at all, are not records.
                json!({"id":"u1","role":"user","ts":at_ms(9),"content":"go"}),
                json!({"id":"m2","role":"assistant","metrics":{"inputTokens":9,"outputTokens":9}}),
                json!({"id":"m3","role":"assistant","ts":at_ms(9)}),
            ],
            Some(json!({"sessionId":"sess-1","model":"gpt-5","cwd":"/work","workspace_root":"/work/pulse","metadata":{"title":"Fix the ring"}})),
        );
        let records = cli_records(&[home.path().join(".cline/data/sessions")]);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].tally, TokenTally::new(50, 20, 30, 40));
        assert_eq!(records[0].model, "gpt-5");
        assert_eq!(records[0].session_id.as_deref(), Some("sess-1"));
        assert_eq!(records[0].project.as_deref(), Some("/work/pulse"));
        assert_eq!(records[0].title.as_deref(), Some("Fix the ring"));
        assert_eq!(records[0].deduplication_id.as_deref(), Some("cline:sess-1:m1"));
    }

    #[test]
    fn an_input_below_the_cache_clamps_at_zero_and_a_missing_manifest_falls_back_to_the_stem() {
        let home = tempfile::tempdir().unwrap();
        session(
            home.path(),
            "s2",
            vec![json!({"id":"m1","role":"assistant","ts":at_ms(9),"modelInfo":{"id":"gpt-5"},"metrics":{"inputTokens":10,"outputTokens":4,"cacheReadTokens":6,"cacheWriteTokens":8}})],
            None,
        );
        let records = cli_records(&[home.path().join(".cline/data/sessions")]);
        assert_eq!(records[0].tally, TokenTally::new(0, 8, 6, 4));
        assert_eq!(records[0].session_id.as_deref(), Some("s2"));
    }

    #[test]
    fn the_session_id_falls_back_from_the_messages_file_to_the_manifest() {
        let home = tempfile::tempdir().unwrap();
        session(
            home.path(),
            "s3",
            vec![json!({"id":"m1","role":"assistant","ts":at_ms(9),"modelInfo":{"id":"gpt-5"},"metrics":{"inputTokens":5,"outputTokens":1}})],
            Some(json!({"sessionId":"manifest-session","model":"gpt-5"})),
        );
        let records = cli_records(&[home.path().join(".cline/data/sessions")]);
        assert_eq!(records[0].session_id.as_deref(), Some("manifest-session"));
    }

    #[test]
    fn the_environment_overrides_win_and_a_blank_one_is_ignored() {
        let home = tempfile::tempdir().unwrap();
        let sources = Sources::new(home.path()).with_var("CLINE_DATA_DIR", "/data/cline");
        assert_eq!(cli_root(&sources), PathBuf::from("/data/cline").join("sessions"));
        let sources = Sources::new(home.path()).with_var("CLINE_SESSION_DATA_DIR", "/data/cline-sessions").with_var("CLINE_DATA_DIR", "/data/cline");
        assert_eq!(cli_root(&sources), PathBuf::from("/data/cline-sessions"));
        assert_eq!(cli_root(&Sources::new(home.path())), home.path().join(".cline").join("data").join("sessions"));
    }

    #[test]
    fn a_store_that_is_not_there_is_empty_and_the_vscode_and_cli_stores_are_both_read() {
        let home = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let sources = Sources::new(home.path());
        assert!(read_agent(SpendAgent::Cline, &sources, cache.path(), &Calendar::utc(2), &PriceTable::new()).days.is_empty());

        session(
            home.path(),
            "s4",
            vec![json!({"id":"m1","role":"assistant","ts":at_ms(9),"modelInfo":{"id":"gpt-5"},"metrics":{"inputTokens":5,"outputTokens":1}})],
            None,
        );
        let ledger = read_agent(SpendAgent::Cline, &sources, cache.path(), &Calendar::utc(2), &PriceTable::new());
        assert_eq!(ledger.days.iter().map(|d| d.tokens).sum::<i64>(), 6);
    }
}
