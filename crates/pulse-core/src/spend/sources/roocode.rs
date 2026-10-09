// Ported from upstream Sources/Pulse/Usage/Readers/VSCodeTaskLogReader.swift (and EditorLogReaders.swift for the roots).
//! Roo Code's VS Code task logs. The same reader serves Kilo Code and Cline, which keep the same
//! task shape under their own extension ids (`kilocode.rs`, `cline.rs`).
//!
//! Each task is a directory named for the session, holding a `ui_messages.json` array and,
//! usually, an `api_conversation_history.json` beside it. Only the `api_req_started` entries count,
//! and each is one real request: the four token kinds sit under a `text` field that is itself JSON.
//!
//! Where it lives on Windows: VS Code keeps extension storage under
//! `%APPDATA%\Code\User\globalStorage\<extension id>\tasks`, with the `Code - Insiders` and
//! `VSCodium` editors beside it, and a remote session keeps the same tree under
//! `%USERPROFILE%\.vscode-server\data`. Upstream's `~/Library/Application Support/X` maps to
//! `%APPDATA%\X`; the `~/.config/X` spelling is looked at too, unverified on Windows.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{push_unique, SpendSource};
use crate::spend::editorlog;
use crate::spend::logio;
use crate::spend::record::AgentUsageRecord;
use crate::spend::tally::TokenTally;
use crate::spend::transcripts::Sources;

const EDITORS: [&str; 3] = ["Code", "Code - Insiders", "VSCodium"];
const REMOTE_SERVERS: [&str; 2] = [".vscode-server", ".vscode-server-insiders"];

pub struct RooCode;

impl SpendSource for RooCode {
    fn raw(&self) -> &'static str {
        "rooCode"
    }
    fn source_id(&self) -> &'static str {
        "roocode"
    }
    fn display_name(&self) -> &'static str {
        "Roo Code"
    }
    fn icon(&self) -> Option<&'static str> {
        Some("roocode")
    }
    fn env_vars(&self) -> &'static [&'static str] {
        &["APPDATA"]
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        task_roots("rooveterinaryinc.roo-cline", sources)
    }
    fn records(&self, roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        task_records(roots)
    }
}

/// Every task folder a VS Code extension may keep its tasks in, whether or not it exists.
/// Shared with Kilo Code and Cline.
pub(crate) fn task_roots(extension_id: &str, sources: &Sources) -> Vec<PathBuf> {
    let storage = |base: PathBuf| base.join("User").join("globalStorage").join(extension_id).join("tasks");
    let mut roots = Vec::new();
    for editor in EDITORS {
        // WINDOWS-PATH: unverified (the %APPDATA% spelling of the macOS Application Support root)
        push_unique(&mut roots, storage(sources.app_data().join(editor)));
        // WINDOWS-PATH: unverified (the Linux `~/.config` spelling, kept in case an install uses it)
        push_unique(&mut roots, storage(sources.home.join(".config").join(editor)));
    }
    for server in REMOTE_SERVERS {
        // WINDOWS-PATH: unverified
        push_unique(&mut roots, sources.home.join(server).join("data").join("User").join("globalStorage").join(extension_id).join("tasks"));
    }
    roots
}

/// The increments every task folder under `roots` holds. Shared with Kilo Code and Cline.
pub(crate) fn task_records(roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
    let files = logio::files(roots, &[], &["ui_messages.json", "api_conversation_history.json"], &[]);

    // Task folder -> (its messages file, its history file).
    let mut tasks: BTreeMap<PathBuf, (Option<PathBuf>, Option<PathBuf>)> = BTreeMap::new();
    for file in files {
        let Some(directory) = file.parent().map(Path::to_path_buf) else { continue };
        let entry = tasks.entry(directory).or_default();
        match file.file_name().and_then(|n| n.to_str()) {
            Some("ui_messages.json") => entry.0 = Some(file),
            Some("api_conversation_history.json") => entry.1 = Some(file),
            _ => {}
        }
    }

    let mut records = Vec::new();
    for (directory, (messages, history)) in tasks {
        let Some(messages) = messages else { continue };
        let session = directory.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let details = history.as_deref().and_then(EnvironmentDetails::from_file);
        records.extend(parse_messages(&messages, &session, details.as_ref()));
    }
    records
}

fn parse_messages(messages: &Path, session: &str, details: Option<&EnvironmentDetails>) -> Vec<AgentUsageRecord> {
    let Some(Value::Array(entries)) = logio::json(messages) else { return Vec::new() };

    let mut records = Vec::new();
    for entry in entries.iter().filter_map(Value::as_object) {
        if logio::text(entry.get("type")).as_deref() != Some("say") || logio::text(entry.get("say")).as_deref() != Some("api_req_started") {
            continue;
        }
        // `ts` is milliseconds as a string or a number; an unparseable one skips the entry.
        let Some(at) = logio::timestamp(entry.get("ts"), true) else { continue };
        let Some(text) = logio::text(entry.get("text")) else { continue };
        let Some(usage) = editorlog::json_object(&text) else { continue };

        let tally = TokenTally {
            input: editorlog::int(usage.get("tokensIn")),
            cache_write: editorlog::int(usage.get("cacheWrites")),
            cache_read: editorlog::int(usage.get("cacheReads")),
            output: editorlog::int(usage.get("tokensOut")),
            ..TokenTally::default()
        };
        if tally.total() <= 0 {
            continue;
        }

        // The entry's own model wins; the history's last `<model>` tag is the fallback.
        let model_info = entry.get("modelInfo").and_then(Value::as_object);
        let model = logio::text(model_info.and_then(|m| m.get("modelId")))
            .or_else(|| details.and_then(|d| d.model.clone()));
        let Some(model) = model else { continue };

        let mut record = AgentUsageRecord::new(at, &model, tally).session(session);
        record.session_name = details.and_then(|d| d.agent.clone());
        records.push(record);
    }
    records
}

/// The `<model>`, `<slug>` and `<name>` tags the history writes inside its `<environment_details>`
/// blocks. The only fallbacks used when an entry names no model.
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct EnvironmentDetails {
    pub model: Option<String>,
    pub agent: Option<String>,
}

impl EnvironmentDetails {
    fn from_file(path: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(path).ok()?;
        Some(Self::parse(&text))
    }

    pub(crate) fn parse(text: &str) -> Self {
        let mut details = Self::default();
        for block in blocks(text) {
            if let Some(model) = last_tag("model", block) {
                details.model = Some(model);
            }
            // The slug is the product's handle for the agent; the name is a fallback.
            if let Some(agent) = last_tag("slug", block).or_else(|| last_tag("name", block)) {
                details.agent = Some(agent);
            }
        }
        details
    }
}

fn blocks(text: &str) -> Vec<&str> {
    const OPEN: &str = "<environment_details>";
    const CLOSE: &str = "</environment_details>";
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find(OPEN) {
        let after = &rest[open + OPEN.len()..];
        let Some(close) = after.find(CLOSE) else { break };
        found.push(&after[..close]);
        rest = &after[close + CLOSE.len()..];
    }
    found
}

fn last_tag(tag: &str, text: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut value = None;
    let mut rest = text;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start + open.len()..];
        let Some(end) = after.find(&close) else { break };
        let content = after[..end].trim();
        if !content.is_empty() {
            value = Some(content.to_string());
        }
        rest = &after[end + close.len()..];
    }
    value
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::spend::calendar::Calendar;
    use crate::spend::prices::PriceTable;
    use crate::spend::sources::{read_agent, SpendAgent};

    /// 2026-09-10 00:00 UTC, and the hours of that day the fixtures use.
    const MIDNIGHT: i64 = 1_789_344_000;
    fn at_hour(hour: i64) -> i64 {
        MIDNIGHT + hour * 3600
    }

    fn usage(input: i64, output: i64, cache_read: i64, cache_write: i64) -> String {
        json!({"cost": 0, "tokensIn": input, "tokensOut": output, "cacheReads": cache_read, "cacheWrites": cache_write}).to_string()
    }

    fn task(home: &Path, name: &str) -> PathBuf {
        let dir = home.join("AppData").join("Roaming").join("Code").join("User").join("globalStorage").join("rooveterinaryinc.roo-cline").join("tasks").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let entries = vec![
            // A request, with its own model.
            json!({"type":"say","say":"api_req_started","ts":at_hour(9)*1000,"text":usage(100, 40, 30, 20),"modelInfo":{"providerId":"anthropic","modelId":"claude-sonnet"}}),
            // No modelInfo: the history's model tag is the fallback. ISO time.
            json!({"type":"say","say":"api_req_started","ts":"2026-09-14T10:00:00Z","text":usage(5, 6, 7, 8)}),
            // Not a request.
            json!({"type":"say","say":"text","ts":at_hour(10)*1000,"text":"hello"}),
            json!({"type":"ask","say":"api_req_started","ts":at_hour(10)*1000,"text":usage(1, 1, 1, 1)}),
            // No time: skipped, never dated at the epoch.
            json!({"type":"say","say":"api_req_started","text":usage(9, 9, 0, 0)}),
            // Unreadable usage text, and all-zero usage: no record.
            json!({"type":"say","say":"api_req_started","ts":at_hour(10)*1000,"text":"not json"}),
            json!({"type":"say","say":"api_req_started","ts":at_hour(10)*1000,"text":usage(0, 0, 0, 0)}),
        ];
        std::fs::write(dir.join("ui_messages.json"), Value::Array(entries).to_string()).unwrap();
        std::fs::write(
            dir.join("api_conversation_history.json"),
            r#"[{"role":"user","content":"go <environment_details>\n<model>claude-sonnet</model>\n<slug>roo</slug>\n<name>Roo Code</name>\n</environment_details>"}]"#,
        )
        .unwrap();
        dir
    }

    #[test]
    fn only_api_requests_count_with_each_bucket_mapped_and_the_model_fallback() {
        let home = tempfile::tempdir().unwrap();
        task(home.path(), "task-a");

        let records = task_records(&[home.path().join("AppData/Roaming/Code/User/globalStorage/rooveterinaryinc.roo-cline/tasks")]);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].tally, TokenTally::new(100, 20, 30, 40));
        assert_eq!(records[0].model, "claude-sonnet");
        assert_eq!(records[0].session_id.as_deref(), Some("task-a"));
        assert_eq!(records[0].session_name.as_deref(), Some("roo"));
        assert_eq!(records[0].timestamp.timestamp(), at_hour(9));
        assert_eq!(records[1].tally, TokenTally::new(5, 8, 7, 6));
        assert_eq!(records[1].model, "claude-sonnet");
        assert_eq!(records[1].timestamp.timestamp(), at_hour(10));
    }

    #[test]
    fn the_environment_block_tags_parse_and_the_last_value_wins() {
        let details = EnvironmentDetails::parse("<environment_details><model>a</model><slug>s1</slug></environment_details><environment_details><model>b</model><name>N</name></environment_details>");
        assert_eq!(details, EnvironmentDetails { model: Some("b".into()), agent: Some("N".into()) });
        assert_eq!(EnvironmentDetails::parse("<environment_details><model>  </model>"), EnvironmentDetails::default());
    }

    #[test]
    fn the_editor_roots_include_insiders_vscodium_and_remote_servers_and_a_missing_store_is_empty() {
        let home = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let sources = Sources::new(home.path());
        let roots = RooCode.inputs(&sources);
        assert!(roots.contains(&home.path().join("AppData/Roaming/Code - Insiders/User/globalStorage/rooveterinaryinc.roo-cline/tasks")));
        assert!(roots.contains(&home.path().join("AppData/Roaming/VSCodium/User/globalStorage/rooveterinaryinc.roo-cline/tasks")));
        assert!(roots.contains(&home.path().join(".vscode-server/data/User/globalStorage/rooveterinaryinc.roo-cline/tasks")));
        let ledger = read_agent(SpendAgent::RooCode, &sources, cache.path(), &Calendar::utc(2), &PriceTable::new());
        assert!(ledger.days.is_empty());
    }

    #[test]
    fn a_task_read_through_the_registry_lands_on_its_day_and_names_the_session() {
        let home = tempfile::tempdir().unwrap();
        task(home.path(), "task-b");
        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::RooCode, &Sources::new(home.path()), cache.path(), &Calendar::utc(2), &PriceTable::new());
        assert!(ledger.days.iter().map(|d| d.tokens).sum::<i64>() > 0);
        assert_eq!(ledger.sessions[0].name, "roo");
        assert_eq!(ledger.unpriced_models, ["claude-sonnet"]);
    }
}
