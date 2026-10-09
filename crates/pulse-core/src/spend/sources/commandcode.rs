// Ported from upstream Sources/Pulse/Usage/Readers/CommandCodeReader.swift.
//! Command Code's transcripts: one JSONL per session under `projects/<slug>/`.
//!
//! The modern format is a **tree**: every entry names a `parentId`, and `/rewind` moves the leaf
//! while the abandoned replies keep their usage on disk. Replies on every branch carry real work,
//! because rewinding context does not refund tokens already consumed. A message's id folds replays;
//! ancestry only resolves the model a reply used, so a model change on another branch cannot
//! reprice it.
//!
//! The modern `usage` object names all four buckets and is cache-exclusive, so it is read as
//! reported. A legacy line with no `usage` block contributes **nothing**: its counts would have to
//! be estimated from string lengths, which are not reported tokens.
//!
//! Where it lives on Windows: `%USERPROFILE%\.commandcode` (the Node-style CLI layout, unverified).
//! The `auth.json` beside it is a credential and is never read.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::SpendSource;
use crate::spend::editorlog;
use crate::spend::logio;
use crate::spend::record::AgentUsageRecord;
use crate::spend::tally::TokenTally;
use crate::spend::transcripts::Sources;

pub struct CommandCode;

impl SpendSource for CommandCode {
    fn raw(&self) -> &'static str {
        "commandCode"
    }
    fn source_id(&self) -> &'static str {
        "commandcode"
    }
    fn display_name(&self) -> &'static str {
        "Command Code"
    }
    fn icon(&self) -> Option<&'static str> {
        Some("commandcode")
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        // WINDOWS-PATH: unverified
        let base = sources.home.join(".commandcode");
        // The legacy model fallback lives in config.json, so it is an input too.
        vec![base.join("projects"), base.join("config.json")]
    }
    fn records(&self, roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        let config_model = config_model(roots);
        let mut files = logio::files(roots, &["jsonl"], &[], &[]);
        files.retain(|f| !f.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with(".checkpoints.jsonl")));
        files.sort();
        files.iter().flat_map(|file| parse(file, config_model.as_deref())).collect()
    }
}

struct Entry {
    index: usize,
    id: Option<String>,
    parent: Option<String>,
    kind: Option<String>,
    model: Option<String>,
    role: Option<String>,
    timestamp: Option<chrono::DateTime<chrono::Utc>>,
    session: Option<String>,
    tally: Option<TokenTally>,
}

fn parse(file: &Path, config_model: Option<&str>) -> Vec<AgentUsageRecord> {
    let stem = file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let mut header_session: Option<String> = None;
    let mut entries: Vec<Entry> = Vec::new();

    for (index, row) in logio::json_lines(file).enumerate() {
        let kind = logio::text(row.get("type"));
        if kind.as_deref() == Some("session") {
            header_session = logio::text(row.get("id")).or(header_session);
            continue;
        }
        let message = row.get("message").and_then(Value::as_object);
        let role = message.and_then(|m| logio::text(m.get("role"))).or_else(|| logio::text(row.get("role")));
        let tally = row.get("usage").and_then(Value::as_object).map(|usage| TokenTally {
            input: editorlog::int(usage.get("inputTokens")),
            cache_write: editorlog::int(usage.get("cacheWriteTokens")),
            cache_read: editorlog::int(usage.get("cacheReadTokens")),
            output: editorlog::int(usage.get("outputTokens")),
            ..TokenTally::default()
        });
        let is_reply = role.as_deref() == Some("assistant");
        entries.push(Entry {
            index,
            id: logio::text(row.get("id")),
            parent: logio::text(row.get("parentId")),
            kind,
            model: editorlog::model_id(logio::text(row.get("model"))),
            role,
            timestamp: if is_reply { logio::timestamp(row.get("timestamp"), false) } else { None },
            session: logio::text(row.get("sessionId")),
            tally,
        });
    }

    let has_tree = entries.iter().any(|e| e.parent.is_some());
    let mut by_id: HashMap<String, usize> = HashMap::new();
    for (position, entry) in entries.iter().enumerate() {
        if let Some(id) = &entry.id {
            by_id.insert(id.clone(), position);
        }
    }
    let mut inherited: HashMap<String, String> = HashMap::new();
    let mut current_model: Option<String> = None;
    let mut records = Vec::new();

    for (position, entry) in entries.iter().enumerate() {
        if entry.kind.as_deref() == Some("model_change") {
            if let Some(model) = &entry.model {
                current_model = Some(model.clone());
            }
            continue;
        }

        let (true, Some(timestamp), Some(tally)) = (entry.role.as_deref() == Some("assistant"), entry.timestamp, entry.tally.clone()) else {
            continue;
        };
        if tally.total() <= 0 {
            continue;
        }

        let model = entry
            .model
            .clone()
            .or_else(|| if has_tree { ancestor_model(position, &entries, &by_id, &mut inherited) } else { current_model.clone() })
            .or_else(|| config_model.map(str::to_string));
        let Some(model) = model else { continue };

        let session = header_session.clone().or_else(|| entry.session.clone()).unwrap_or_else(|| stem.clone());
        // An explicit message id names the call even if an export changes its timestamp; the
        // builder folds replays across files once.
        let identity = match &entry.id {
            Some(id) => format!("commandcode:{session}:{id}"),
            None => format!("commandcode:{session}:line{}:{}", entry.index, timestamp.timestamp_millis()),
        };

        let mut record = AgentUsageRecord::new(timestamp, &model, tally).session(&session);
        record.deduplication_id = Some(identity);
        records.push(record);
    }
    records
}

/// The nearest stated model on this reply's own ancestry, not the last model change in file order.
/// Memoized so long branches stay linear; a missing parent or a cycle stops without borrowing a
/// sibling's model.
fn ancestor_model(position: usize, entries: &[Entry], by_id: &HashMap<String, usize>, cache: &mut HashMap<String, String>) -> Option<String> {
    let mut visited: HashSet<String> = HashSet::new();
    let mut cursor = entries[position].parent.clone();
    let mut model: Option<String> = None;
    while let Some(id) = cursor.take() {
        if !visited.insert(id.clone()) {
            break;
        }
        if let Some(cached) = cache.get(&id) {
            model = Some(cached.clone());
            break;
        }
        let Some(&ancestor) = by_id.get(&id) else { break };
        if let Some(stated) = &entries[ancestor].model {
            model = Some(stated.clone());
            break;
        }
        cursor = entries[ancestor].parent.clone();
    }
    if let Some(found) = &model {
        for id in visited {
            cache.insert(id, found.clone());
        }
    }
    model
}

/// The legacy model fallback in `config.json`, when one is among the roots.
fn config_model(roots: &[PathBuf]) -> Option<String> {
    roots
        .iter()
        .filter(|root| root.file_name().and_then(|n| n.to_str()) == Some("config.json"))
        .find_map(|root| logio::json(root).and_then(|v| editorlog::model_id(logio::text(v.get("model")))))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::spend::calendar::Calendar;
    use crate::spend::prices::PriceTable;
    use crate::spend::sources::{read_agent, SpendAgent};

    const MIDNIGHT: i64 = 1_789_344_000;

    fn write(path: &Path, lines: &[Value]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let text: Vec<String> = lines.iter().map(Value::to_string).collect();
        std::fs::write(path, text.join("\n")).unwrap();
    }

    fn reply(id: &str, parent: Option<&str>, at: i64, input: i64, output: i64) -> Value {
        let mut value = json!({"type":"message","id":id,"timestamp":at,"sessionId":"sess-1","model":"vendor/gpt-z",
            "message":{"role":"assistant"},"usage":{"inputTokens":input,"outputTokens":output,"cacheReadTokens":2,"cacheWriteTokens":1}});
        if let Some(parent) = parent {
            value["parentId"] = json!(parent);
        }
        value
    }

    fn projects(home: &Path) -> PathBuf {
        home.join(".commandcode").join("projects").join("slug")
    }

    #[test]
    fn an_abandoned_branch_still_counts_and_a_model_follows_its_own_ancestry_not_the_last_change() {
        let home = tempfile::tempdir().unwrap();
        write(
            &projects(home.path()).join("sess-1.jsonl"),
            &[
                json!({"type":"session","id":"sess-1"}),
                json!({"type":"model_change","model":"openai/gpt-a"}),
                json!({"type":"message","id":"u1","message":{"role":"user"}}),
                json!({"type":"message","id":"a1","parentId":"u1","model":"openai/gpt-a","message":{"role":"assistant"},"timestamp":MIDNIGHT,"usage":{"inputTokens":100,"outputTokens":10,"cacheReadTokens":0,"cacheWriteTokens":0}}),
                // A rewound branch from a1: the model changes later in the file, but b1's own ancestry says gpt-a.
                json!({"type":"model_change","model":"other/gpt-b"}),
                json!({"type":"message","id":"b1","parentId":"a1","message":{"role":"assistant"},"timestamp":MIDNIGHT+1,"usage":{"inputTokens":50,"outputTokens":5,"cacheReadTokens":0,"cacheWriteTokens":0}}),
            ],
        );
        let records = parse(&projects(home.path()).join("sess-1.jsonl"), None);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].model, "gpt-a");
        // The abandoned branch still counts, and it keeps the model of its own ancestry.
        assert_eq!(records[1].tally, TokenTally::new(50, 0, 0, 5));
        assert_eq!(records[1].model, "gpt-a");
    }

    #[test]
    fn a_legacy_line_with_no_usage_contributes_nothing_and_the_config_model_is_the_fallback() {
        let home = tempfile::tempdir().unwrap();
        write(
            &home.path().join(".commandcode").join("config.json").to_path_buf(),
            &[json!({"model":"provider/fallback-model"})],
        );
        write(
            &projects(home.path()).join("legacy.jsonl"),
            &[
                json!({"type":"message","id":"l1","message":{"role":"assistant","content":"hello there"},"timestamp":MIDNIGHT}),
                json!({"type":"message","id":"l2","message":{"role":"assistant"},"timestamp":MIDNIGHT,"usage":{"inputTokens":4,"outputTokens":2}}),
            ],
        );
        let roots = vec![projects(home.path()), home.path().join(".commandcode").join("config.json")];
        let records = read_commandcode(&roots);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].tally, TokenTally::new(4, 0, 0, 2));
        assert_eq!(records[0].model, "fallback-model");
    }

    #[test]
    fn a_replayed_message_in_two_files_is_counted_once_and_checkpoints_are_not_read() {
        let home = tempfile::tempdir().unwrap();
        let line = reply("dup", None, MIDNIGHT, 10, 1);
        write(&projects(home.path()).join("a.jsonl"), std::slice::from_ref(&line));
        write(&projects(home.path()).join("b.jsonl"), std::slice::from_ref(&line));
        write(&projects(home.path()).join("c.checkpoints.jsonl"), &[reply("cp", None, MIDNIGHT, 99, 99)]);
        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::CommandCode, &Sources::new(home.path()), cache.path(), &Calendar::utc(2), &PriceTable::new());
        // One call: 10 input, 1 output, 2 cache read, 1 cache write.
        assert_eq!(ledger.days.iter().map(|d| d.tokens).sum::<i64>(), 14);
    }

    #[test]
    fn a_missing_store_is_an_empty_ledger() {
        let home = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        assert!(read_agent(SpendAgent::CommandCode, &Sources::new(home.path()), cache.path(), &Calendar::utc(2), &PriceTable::new()).days.is_empty());
    }

    fn read_commandcode(roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        CommandCode.records(roots)
    }
}
