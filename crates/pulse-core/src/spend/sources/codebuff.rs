// Ported from upstream Sources/Pulse/Usage/Readers/CodebuffUsageReader.swift.
//! Codebuff's chat logs, under its `manicode*` product trees.
//!
//! `<root>/projects/<project>/chats/<chatId>/chat-messages.json` is a top-level array of messages.
//! The chat id is the chat's ISO start with the time colons written as dashes; the project and
//! channel come from the path, so a session is `<channel>/<project>/<chatId>`.
//!
//! **Usage is merged, not summed.** The same provider numbers are copied into several places
//! (`metadata.usage`, `metadata.codebuff.usage`, and the assistant rows' `providerOptions` in the
//! run-state history). Each field is taken from the first source that reported a non-zero value,
//! so a zero in a higher-priority copy cannot mask the real count below it.
//!
//! **Credits are not tokens.** `credits` is a cost the product keeps beside the counts and is not
//! read as usage.
//!
//! Freebuff chats share these trees and are read by `freebuff.rs`; a chat whose assistant rows
//! carry authoritative usage is counted here whatever its agent type.
//!
//! Where it lives on Windows: `CODEBUFF_DATA_DIR` when set, else `%USERPROFILE%\.config\manicode`
//! (and `-dev` / `-staging`). WINDOWS-PATH: unverified.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{push_unique, SpendSource};
use crate::spend::logio;
use crate::spend::record::AgentUsageRecord;
use crate::spend::tally::TokenTally;
use crate::spend::transcripts::Sources;

const INPUT: &[&str] = &["inputTokens", "input_tokens", "promptTokens", "prompt_tokens"];
const OUTPUT: &[&str] = &["outputTokens", "output_tokens", "completionTokens", "completion_tokens"];
const CACHE_READ: &[&str] = &["cacheReadInputTokens", "cache_read_input_tokens", "cachedTokensCreated", "cached_tokens_created"];
const CACHE_WRITE: &[&str] = &["cacheCreationInputTokens", "cache_creation_input_tokens", "cacheCreationTokens", "cache_creation_tokens"];

pub struct Codebuff;

impl SpendSource for Codebuff {
    fn raw(&self) -> &'static str {
        "codebuff"
    }
    fn source_id(&self) -> &'static str {
        "codebuff"
    }
    fn display_name(&self) -> &'static str {
        "Codebuff"
    }
    fn env_vars(&self) -> &'static [&'static str] {
        &["CODEBUFF_DATA_DIR"]
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        if let Some(dir) = sources.var("CODEBUFF_DATA_DIR") {
            return vec![dir];
        }
        let mut roots = Vec::new();
        // WINDOWS-PATH: unverified (the macOS layout carried over)
        for product in ["manicode", "manicode-dev", "manicode-staging"] {
            push_unique(&mut roots, sources.home.join(".config").join(product));
        }
        roots
    }
    fn records(&self, roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        let mut records = Vec::new();
        for file in logio::files(roots, &[], &["chat-messages.json"], &[]) {
            let Some(Value::Array(messages)) = logio::json(&file) else { continue };
            let (channel, project, chat_id) = location(&file);
            let session = format!("{channel}/{project}/{chat_id}");

            for (index, message) in messages.iter().enumerate() {
                if !is_assistant(message) {
                    continue;
                }
                let sources = usage_sources(message);
                if sources.is_empty() {
                    continue;
                }

                // Codebuff's input is the provider's `prompt_tokens`, which already holds the cached
                // prefix, so the cache read is taken out of it.
                let read = merged(&sources, CACHE_READ);
                let tally = TokenTally::new(
                    (merged(&sources, INPUT) - read).max(0),
                    merged(&sources, CACHE_WRITE),
                    read,
                    merged(&sources, OUTPUT),
                );
                if tally.total() <= 0 {
                    continue;
                }
                let Some(model) = model(message, &sources) else { continue };
                let Some(at) = timestamp(message, &chat_id) else { continue };

                let identity = logio::text(message.get("id")).unwrap_or_else(|| {
                    format!(
                        "{session}:{index}:{}:{model}:{}:{}:{}:{}",
                        at.timestamp_millis(),
                        tally.input,
                        tally.cache_write,
                        tally.cache_read,
                        tally.output
                    )
                });

                let mut record = AgentUsageRecord::new(at, &model, tally).session(&session);
                record.session_name = Some(chat_id.clone());
                record.deduplication_id = Some(format!("codebuff:{session}:{identity}"));
                records.push(record);
            }
        }
        records
    }
}

/// Whether a row is the assistant's own. `variant` is the newer marker and `role` the older; either
/// is enough.
fn is_assistant(message: &Value) -> bool {
    [logio::text(message.get("variant")), logio::text(message.get("role"))]
        .into_iter()
        .flatten()
        .any(|marker| matches!(marker.to_lowercase().as_str(), "ai" | "agent" | "assistant"))
}

/// The run-state history's assistant rows, newest first.
fn history(message: &Value) -> Vec<&Value> {
    let main = message
        .get("metadata")
        .and_then(|m| m.get("runState"))
        .and_then(|r| r.get("sessionState"))
        .and_then(|s| s.get("mainAgentState"));
    let mut rows: Vec<&Value> = main.and_then(|m| m.get("messageHistory")).and_then(Value::as_array).map(|a| a.iter().collect()).unwrap_or_default();
    rows.reverse();
    rows
}

/// Every usage object the message carries, highest priority first.
fn usage_sources(message: &Value) -> Vec<&Value> {
    let mut sources = Vec::new();
    let Some(metadata) = message.get("metadata").filter(|m| m.is_object()) else { return sources };

    if let Some(usage) = metadata.get("usage").filter(|u| u.is_object()) {
        sources.push(usage);
    }
    if let Some(usage) = metadata.get("codebuff").and_then(|c| c.get("usage")).filter(|u| u.is_object()) {
        sources.push(usage);
    }
    for row in history(message) {
        let Some(providers) = row.get("providerOptions").filter(|p| p.is_object()) else { continue };
        if let Some(usage) = providers.get("usage").filter(|u| u.is_object()) {
            sources.push(usage);
        }
        if let Some(usage) = providers.get("codebuff").and_then(|c| c.get("usage")).filter(|u| u.is_object()) {
            sources.push(usage);
        }
    }
    sources
}

/// The first non-zero count for `aliases` across `sources`, in order.
fn merged(sources: &[&Value], aliases: &[&str]) -> i64 {
    for source in sources {
        for alias in aliases {
            let value = logio::count(source.get(*alias));
            if let Some(value) = value.filter(|v| *v > 0) {
                return value;
            }
        }
    }
    0
}

/// `metadata.model`, then the last history row's `providerOptions.codebuff.model`, then the usage
/// object's own `model`.
fn model(message: &Value, sources: &[&Value]) -> Option<String> {
    if let Some(named) = logio::text(message.get("metadata").and_then(|m| m.get("model"))) {
        return Some(named);
    }
    for row in history(message) {
        if let Some(name) = logio::text(row.get("providerOptions").and_then(|p| p.get("codebuff")).and_then(|c| c.get("model"))) {
            return Some(name);
        }
    }
    sources.iter().find_map(|source| logio::text(source.get("model")))
}

/// The message's own time, else its `createdAt`, else the metadata's, else the chat id restored to
/// ISO 8601. A zero is "unset" and falls through.
fn timestamp(message: &Value, chat_id: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    let event = |value: Option<&Value>| logio::timestamp(value, false).filter(|t| t.timestamp() > 0);
    event(message.get("timestamp"))
        .or_else(|| event(message.get("createdAt")))
        .or_else(|| event(message.get("metadata").and_then(|m| m.get("timestamp"))))
        .or_else(|| iso_from_chat_id(chat_id))
}

/// An ISO 8601 chat id whose time separators were written as `-`, restored to a date. Only the
/// `HH-MM-SS` after the `T` becomes `:`.
fn iso_from_chat_id(chat_id: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    let (head, tail) = chat_id.split_once('T')?;
    let text = format!("{head}T{}", tail.replace('-', ":"));
    logio::timestamp(Some(&Value::String(text)), false).filter(|t| t.timestamp() > 0)
}

/// Channel, project and chat id from `…/<channel>/projects/<project>/chats/<chatId>/`.
fn location(file: &Path) -> (String, String, String) {
    let name = |p: Option<&Path>| p.and_then(Path::file_name).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let chat_id = name(file.parent());
    let parts: Vec<String> = file.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    if let Some(index) = parts.iter().rposition(|p| p == "projects") {
        if index > 0 && index + 1 < parts.len() {
            return (parts[index - 1].clone(), parts[index + 1].clone(), chat_id);
        }
    }
    // A tree that was renamed or moved: the directory above `chats` is the project, and the config
    // directory above it names the channel.
    let chats = file.parent().and_then(Path::parent);
    (name(chats.and_then(Path::parent)), name(chats), chat_id)
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

    const CHAT: &str = "2026-09-14T10-20-30.000Z";

    fn chat(home: &Path, product: &str, messages: &Value) -> PathBuf {
        let folder = home.join(".config").join(product).join("projects").join("Pulse").join("chats").join(CHAT);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("chat-messages.json"), messages.to_string()).unwrap();
        folder
    }

    fn reply(id: &str, usage: Value) -> Value {
        json!({"id": id, "variant": "ai", "timestamp": "2026-09-14T10:21:00Z", "metadata": {"model": "priced", "usage": usage}})
    }

    #[test]
    fn cache_read_comes_out_of_prompt_tokens_and_the_four_kinds_land_where_upstream_says() {
        let home = tempfile::tempdir().unwrap();
        chat(home.path(), "manicode", &json!([
            reply("m1", json!({"prompt_tokens": 100, "cache_read_input_tokens": 30, "cache_creation_input_tokens": 5, "completion_tokens": 40})),
        ]));
        let records = Codebuff.records(&Codebuff.inputs(&Home::new(home.path())));
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].tally, TokenTally::new(70, 5, 30, 40));
        assert_eq!(records[0].model, "priced");
        assert_eq!(records[0].session_name.as_deref(), Some(CHAT));
    }

    #[test]
    fn merged_fields_take_the_first_non_zero_copy_and_a_message_is_not_double_counted() {
        let home = tempfile::tempdir().unwrap();
        let row = json!({"id": "m1", "role": "assistant", "timestamp": "2026-09-14T10:21:00Z",
            "metadata": {"model": "priced", "usage": {"inputTokens": 0, "outputTokens": 0},
                "codebuff": {"usage": {"inputTokens": 50, "outputTokens": 9}}}});
        chat(home.path(), "manicode", &json!([row.clone(), row]));
        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::Codebuff, &Home::new(home.path()), cache.path(), &Calendar::utc(2), &PriceTable::new());
        assert_eq!(ledger.days.iter().map(|d| d.tokens).sum::<i64>(), 59);
    }

    #[test]
    fn a_row_that_is_not_a_reply_has_no_time_or_has_no_counts_is_skipped() {
        let home = tempfile::tempdir().unwrap();
        let user = json!({"id": "u", "variant": "user", "metadata": {"model": "priced", "usage": {"inputTokens": 9}}});
        let empty = json!({"id": "e", "variant": "ai", "timestamp": "2026-09-14T10:21:00Z", "metadata": {"model": "priced", "usage": {"inputTokens": 0}}});
        let model_less = json!({"id": "n", "variant": "ai", "timestamp": "2026-09-14T10:21:00Z", "metadata": {"usage": {"inputTokens": 4}}});
        // The chat id is not a date and no row carries a time: nothing to date it by.
        let folder = home.path().join(".config").join("manicode").join("projects").join("Pulse").join("chats").join("no-date");
        std::fs::create_dir_all(&folder).unwrap();
        let untimed = json!([{"id": "t", "variant": "ai", "metadata": {"model": "priced", "usage": {"inputTokens": 3}}}]);
        std::fs::write(folder.join("chat-messages.json"), untimed.to_string()).unwrap();
        chat(home.path(), "manicode", &json!([user, empty, model_less]));
        assert!(Codebuff.records(&Codebuff.inputs(&Home::new(home.path()))).is_empty());
    }

    #[test]
    fn a_missing_store_is_an_empty_ledger_and_the_override_moves_it() {
        let empty = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::Codebuff, &Home::new(empty.path()), cache.path(), &Calendar::utc(2), &PriceTable::new());
        assert!(ledger.days.is_empty());

        let home = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let folder = elsewhere.path().join("projects").join("Pulse").join("chats").join(CHAT);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("chat-messages.json"), json!([reply("x", json!({"inputTokens": 7}))]).to_string()).unwrap();
        let sources = Home::new(home.path()).with_var("CODEBUFF_DATA_DIR", elsewhere.path());
        let records = Codebuff.records(&Codebuff.inputs(&sources));
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].tally.input, 7);
    }

    #[test]
    fn a_session_buckets_reach_today_and_only_todays_counts() {
        use chrono::{Duration, Utc};
        let home = tempfile::tempdir().unwrap();
        let calendar = Calendar::local();
        let now = Utc::now();
        let stamp = |day: chrono::DateTime<Utc>| (calendar.start_of_day(day) + Duration::hours(12)).to_rfc3339();
        let today = stamp(now);
        let yesterday = stamp(calendar.add_days(calendar.start_of_day(now), -1));
        let reply_at = |id: &str, at: &str, input: i64| json!({"id": id, "variant": "ai", "timestamp": at, "metadata": {"model": "priced", "usage": {"inputTokens": input}}});
        chat(home.path(), "manicode", &json!([reply_at("a", &today, 100), reply_at("b", &yesterday, 900)]));
        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::Codebuff, &Home::new(home.path()), cache.path(), &calendar, &PriceTable::new());
        let summary = SpendSummary::of(&std::collections::HashMap::from([(SpendAgent::Codebuff, ledger)]), Some(1), now, &calendar);
        // The summary is today's: yesterday's 900 is in the ledger but not in today's figures.
        assert_eq!(summary.tokens, 100);
        assert_eq!(summary.sessions[0].session.unpriced_tokens, 100);
    }

    #[test]
    fn the_chat_id_restores_its_time_and_the_path_names_channel_and_project() {
        assert_eq!(iso_from_chat_id(CHAT).map(|t| t.timestamp()), Some(1_789_381_230));
        let file = PathBuf::from("/home/me/.config/manicode-dev/projects/Pulse/chats/x/chat-messages.json");
        assert_eq!(location(&file), ("manicode-dev".into(), "Pulse".into(), "x".into()));
    }
}
