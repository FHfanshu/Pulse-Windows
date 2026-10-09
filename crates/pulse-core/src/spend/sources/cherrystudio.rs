// Ported from upstream Sources/Pulse/Usage/Readers/CherryStudioReader.swift.
//! Cherry Studio's agent transcripts.
//!
//! A standard Claude Code-shaped JSONL tree under the app's `.claude/projects`, once for the V2
//! layout (`Data/Agents/.claude/projects`) and once for V1 (`.claude/projects`). Cherry appends
//! the **same API call three or four times** as a response streams, each copy with a fresh `uuid`
//! but the same `requestId`, `message.id` and usage, so counting lines would multiply every call.
//!
//! The identity is the call, and the counters are cumulative: records sharing a `requestId`
//! (falling back to `message.id`, then `uuid`) fold into one, each bucket taken as its field-wise
//! maximum, which is what a stream of growing snapshots needs. A record with no identity is kept on
//! its own. Both trees hold the same relative session path; V2 is read first and wins it.
//!
//! Where it lives on Windows: Cherry Studio is an Electron app, so `%APPDATA%\CherryStudio` is
//! read (`WINDOWS-PATH: unverified`), with the `~/Library/Application Support` and `~/.config`
//! spellings also looked at.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{push_unique, SpendSource};
use crate::spend::editorlog;
use crate::spend::logio;
use crate::spend::record::AgentUsageRecord;
use crate::spend::tally::TokenTally;
use crate::spend::transcripts::Sources;

pub struct CherryStudio;

impl SpendSource for CherryStudio {
    fn raw(&self) -> &'static str {
        "cherryStudio"
    }
    fn source_id(&self) -> &'static str {
        "cherrystudio"
    }
    fn display_name(&self) -> &'static str {
        "Cherry Studio"
    }
    fn icon(&self) -> Option<&'static str> {
        Some("cherrystudio")
    }
    fn env_vars(&self) -> &'static [&'static str] {
        &["APPDATA"]
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        // V2 is listed before V1 so a same-named session in both resolves to the current one.
        let mut roots = Vec::new();
        for base in [
            // WINDOWS-PATH: unverified (Electron's %APPDATA% spelling)
            sources.app_data().join("CherryStudio"),
            // WINDOWS-PATH: unverified (the macOS layout, carried over)
            sources.home.join("Library").join("Application Support").join("CherryStudio"),
            // WINDOWS-PATH: unverified (the Linux layout)
            sources.home.join(".config").join("CherryStudio"),
        ] {
            push_unique(&mut roots, base.join("Data").join("Agents").join(".claude").join("projects"));
            push_unique(&mut roots, base.join(".claude").join("projects"));
        }
        roots
    }
    fn records(&self, roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        let mut seen_relative: HashSet<String> = HashSet::new();
        let mut records = Vec::new();
        for root in roots {
            for file in logio::files(std::slice::from_ref(root), &["jsonl"], &[], &[]) {
                let relative = relative_path(&file, root);
                if !seen_relative.insert(relative.clone()) {
                    continue;
                }
                records.extend(parse(&file, &relative));
            }
        }
        records
    }
}

#[derive(Default)]
struct Snapshot {
    input: i64,
    cache_write: i64,
    cache_read: i64,
    output: i64,
    timestamp: Option<chrono::DateTime<chrono::Utc>>,
    model: Option<String>,
}

fn parse(file: &Path, relative: &str) -> Vec<AgentUsageRecord> {
    let session = file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let project = file
        .parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().trim().to_string())
        .filter(|n| !n.is_empty());

    let mut identified: BTreeMap<String, Snapshot> = BTreeMap::new();
    let mut unidentified: Vec<AgentUsageRecord> = Vec::new();

    for row in logio::json_lines(file) {
        if logio::text(row.get("type")).as_deref() != Some("assistant") {
            continue;
        }
        let Some(message) = row.get("message").and_then(Value::as_object) else { continue };
        let Some(usage) = message.get("usage").and_then(Value::as_object) else { continue };

        let input = editorlog::int(usage.get("input_tokens"));
        let cache_read = editorlog::int(usage.get("cache_read_input_tokens"));
        let cache_write = editorlog::int(usage.get("cache_creation_input_tokens"));
        let output = editorlog::int(usage.get("output_tokens"));
        if input + cache_read + cache_write + output <= 0 {
            continue;
        }

        // The timestamp may sit on the record, on its message, or be absent; absent is not newer evidence.
        let timestamp = logio::timestamp(row.get("timestamp"), false).or_else(|| logio::timestamp(message.get("timestamp"), false));

        let identity = logio::text(row.get("requestId"))
            .or_else(|| logio::text(message.get("requestId")))
            .or_else(|| logio::text(message.get("id")))
            .or_else(|| logio::text(row.get("uuid")));

        let Some(model) = logio::text(message.get("model")) else { continue };

        let Some(identity) = identity else {
            let Some(timestamp) = timestamp else { continue };
            let mut record = AgentUsageRecord::new(timestamp, &model, TokenTally::new(input, cache_write, cache_read, output)).session(&session);
            record.project = project.clone();
            unidentified.push(record);
            continue;
        };

        let snapshot = identified.entry(identity).or_default();
        snapshot.input = snapshot.input.max(input);
        snapshot.cache_write = snapshot.cache_write.max(cache_write);
        snapshot.cache_read = snapshot.cache_read.max(cache_read);
        snapshot.output = snapshot.output.max(output);
        snapshot.model = snapshot.model.take().or(Some(model));
        if let Some(timestamp) = timestamp {
            if snapshot.timestamp.map_or(true, |known| timestamp > known) {
                snapshot.timestamp = Some(timestamp);
            }
        }
    }

    let mut records = unidentified;
    for (identity, snapshot) in identified {
        let (Some(timestamp), Some(model)) = (snapshot.timestamp, snapshot.model) else { continue };
        let tally = TokenTally::new(snapshot.input, snapshot.cache_write, snapshot.cache_read, snapshot.output);
        let mut record = AgentUsageRecord::new(timestamp, &model, tally).session(&session);
        record.project = project.clone();
        record.deduplication_id = Some(format!("cherrystudio:{relative}:{identity}"));
        records.push(record);
    }
    records
}

/// The file's path under its root, with `/` separators, so the same relative session in two trees
/// compares equal.
fn relative_path(file: &Path, root: &Path) -> String {
    let relative = file.strip_prefix(root).unwrap_or(file);
    relative.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::spend::calendar::Calendar;
    use crate::spend::prices::PriceTable;
    use crate::spend::sources::{read_agent, SpendAgent};

    const MIDNIGHT: i64 = 1_789_344_000;

    fn transcript(home: &Path, tree: &str, name: &str) -> PathBuf {
        home.join(tree).join("sess").join(format!("{name}.jsonl"))
    }

    fn write(path: &Path, lines: &[Value]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let text: Vec<String> = lines.iter().map(Value::to_string).collect();
        std::fs::write(path, text.join("\n")).unwrap();
    }

    fn reply(request: &str, input: i64, output: i64) -> Value {
        json!({"type":"assistant","uuid":format!("u-{request}-{input}"),"requestId":request,"timestamp":MIDNIGHT,
               "message":{"model":"claude-x","usage":{"input_tokens":input,"output_tokens":output,"cache_read_input_tokens":6,"cache_creation_input_tokens":4}}})
    }

    fn v2(home: &Path) -> PathBuf {
        home.join("AppData/Roaming/CherryStudio/Data/Agents/.claude/projects")
    }

    #[test]
    fn a_streamed_call_written_several_times_counts_once_at_its_largest_snapshot() {
        let home = tempfile::tempdir().unwrap();
        // The same call three times, each a growing snapshot, under one requestId.
        write(
            &transcript(home.path(), "AppData/Roaming/CherryStudio/Data/Agents/.claude/projects", "pulse"),
            &[reply("r1", 10, 1), reply("r1", 100, 40), reply("r1", 100, 5), reply("r2", 7, 3)],
        );
        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::CherryStudio, &Sources::new(home.path()), cache.path(), &Calendar::utc(2), &PriceTable::new());
        // r1: max(input)=100, max(output)=40, cache 6 and 4; r2: 7 + 6 + 4 + 3.
        assert_eq!(ledger.days.iter().map(|d| d.tokens).sum::<i64>(), (100 + 6 + 4 + 40) + (7 + 6 + 4 + 3));
        assert_eq!(ledger.sessions[0].project.as_ref().unwrap().name, "sess");
    }

    #[test]
    fn a_reply_with_no_identity_is_kept_each_time_and_a_non_reply_or_unpriced_row_is_not() {
        let home = tempfile::tempdir().unwrap();
        let identityless = json!({"type":"assistant","timestamp":MIDNIGHT,"message":{"model":"m","usage":{"input_tokens":3,"output_tokens":1}}});
        write(
            &transcript(home.path(), "AppData/Roaming/CherryStudio/Data/Agents/.claude/projects", "pulse"),
            &[
                identityless.clone(),
                identityless,
                json!({"type":"user","requestId":"x","timestamp":MIDNIGHT,"message":{"usage":{"input_tokens":9}}}),
                json!({"type":"assistant","requestId":"no-time","message":{"model":"m","usage":{"input_tokens":9}}}),
                json!({"type":"assistant","requestId":"no-model","timestamp":MIDNIGHT,"message":{"usage":{"input_tokens":9}}}),
            ],
        );
        let records = parse(&transcript(home.path(), "AppData/Roaming/CherryStudio/Data/Agents/.claude/projects", "pulse"), "sess/pulse.jsonl");
        assert_eq!(records.len(), 2);
        assert!(records.iter().all(|r| r.tally == TokenTally::new(3, 0, 0, 1)));
    }

    #[test]
    fn the_v2_tree_wins_a_relative_session_it_shares_with_v1() {
        let home = tempfile::tempdir().unwrap();
        write(&transcript(home.path(), "AppData/Roaming/CherryStudio/Data/Agents/.claude/projects", "pulse"), &[reply("v2", 1, 1)]);
        write(&transcript(home.path(), "AppData/Roaming/CherryStudio/.claude/projects", "pulse"), &[reply("v1", 999, 999)]);
        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::CherryStudio, &Sources::new(home.path()), cache.path(), &Calendar::utc(2), &PriceTable::new());
        assert_eq!(ledger.days.iter().map(|d| d.tokens).sum::<i64>(), 1 + 1 + 6 + 4);
    }

    #[test]
    fn the_store_is_missing_then_empty_and_the_appdata_root_moves_with_the_environment() {
        let home = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        assert!(read_agent(SpendAgent::CherryStudio, &Sources::new(home.path()), cache.path(), &Calendar::utc(2), &PriceTable::new()).days.is_empty());
        let elsewhere = tempfile::tempdir().unwrap();
        let inputs = CherryStudio.inputs(&Sources::new(home.path()).with_var("APPDATA", elsewhere.path()));
        assert_eq!(inputs[0], elsewhere.path().join("CherryStudio/Data/Agents/.claude/projects"));
        assert!(v2(home.path()).to_string_lossy().contains("CherryStudio"));
    }
}
