// Ported from upstream Sources/Pulse/Usage/Readers/TencentBuddyReader.swift (client "codebuddy"; the shared reader also serves WorkBuddy, see `workbuddy.rs`).
//! CodeBuddy, Tencent's coding agent. Three shapes are read, each dispatched by the file it is in:
//!
//! - a JSONL transcript under `~/.codebuddy/projects/`, with real per-request counts;
//! - an extension log holding `[AgentReporter]` usage lines, read only when no transcript exists;
//! - WorkBuddy's aggregate SQLite database (`workbuddy.rs` only).
//!
//! **Only the transcript is a reconcilable channel.** When one is present it is the only thing
//! returned, and when the excluded fallback held records the transcript is marked partial. With
//! no transcript the fallback stands alone: two same-second lines with the same counts are two
//! requests, never folded.
//!
//! Where it lives on Windows: `%USERPROFILE%\.codebuddy` (the Node-style CLI layout). The extension
//! log's location under `%APPDATA%` is not checked here.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::Value;

use super::SpendSource;
use crate::spend::editorlog::{self, Object, Reported, UsageParts};
use crate::spend::loglines;
use crate::spend::logio;
use crate::spend::record::AgentUsageRecord;
use crate::spend::tally::TokenTally;
use crate::spend::transcripts::Sources;

pub struct CodeBuddy;

impl SpendSource for CodeBuddy {
    fn raw(&self) -> &'static str {
        "codeBuddy"
    }
    fn source_id(&self) -> &'static str {
        "codebuddy"
    }
    fn display_name(&self) -> &'static str {
        "CodeBuddy"
    }
    fn icon(&self) -> Option<&'static str> {
        // CodeBuddy and WorkBuddy are both Tencent's.
        Some("tencent")
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        // WINDOWS-PATH: unverified (the macOS home layout, which Node CLIs keep on Windows too)
        vec![sources.home.join(".codebuddy")]
    }
    fn records(&self, roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        read("codebuddy", roots)
    }
}

/// The shared Tencent reader, for `client` `codebuddy` or `workbuddy`.
pub(crate) fn read(client: &str, roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
    let files = logio::files(roots, &["jsonl", "log"], &["workbuddy.db"], &["binaries"]);

    let mut transcript: Vec<AgentUsageRecord> = Vec::new();
    for file in files.iter().filter(|f| ext(f) == "jsonl") {
        transcript.extend(jsonl(client, file));
    }

    // The transcript wins and the fallback is not appended: the extension log and the aggregate
    // database share no identity with the transcript's message ids, so adding them would double
    // count. When the excluded fallback held records the transcript is marked partial.
    let mut fallback: Vec<AgentUsageRecord> = Vec::new();
    for file in files.iter().filter(|f| ext(f) != "jsonl") {
        let found = if ext(file) == "log" {
            extension_log(client, file, !transcript.is_empty())
        } else {
            workbuddy_database(client, file)
        };
        if !transcript.is_empty() && !found.is_empty() {
            let mut partial = transcript;
            for record in &mut partial {
                record.is_partial = true;
            }
            partial.sort_by_key(|r| r.timestamp);
            return partial;
        }
        fallback.extend(found);
    }
    if !transcript.is_empty() {
        transcript.sort_by_key(|r| r.timestamp);
        return transcript;
    }
    fallback.sort_by_key(|r| r.timestamp);
    fallback
}

fn ext(path: &std::path::Path) -> &str {
    path.extension().and_then(|e| e.to_str()).unwrap_or("")
}

fn total(record: &AgentUsageRecord) -> i64 {
    record.tally.total() + record.unclassified_tokens
}

// MARK: - JSONL transcript

fn jsonl(client: &str, file: &std::path::Path) -> Vec<AgentUsageRecord> {
    let stem = file.file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_string();
    // Identified records fold by their message identity; the more complete snapshot wins.
    let mut identified: BTreeMap<String, AgentUsageRecord> = BTreeMap::new();
    let mut unidentified: Vec<AgentUsageRecord> = Vec::new();

    for row in logio::json_lines(file) {
        let Some(timestamp) = logio::timestamp(row.get("timestamp"), true) else { continue };
        let kind = logio::text(row.get("type"));
        let role = logio::text(row.get("role"));
        if !((kind.as_deref() == Some("message") && role.as_deref() == Some("assistant")) || kind.as_deref() == Some("function_call")) {
            continue;
        }
        // A record the product itself calls incomplete is not counted.
        if let Some(status) = logio::text(row.get("status")) {
            if status != "completed" {
                continue;
            }
        }

        let empty = Object::new();
        let message = row.get("message").and_then(Value::as_object).unwrap_or(&empty);
        let provider = row.get("providerData").and_then(Value::as_object).unwrap_or(&empty);
        let Some(usage) = message
            .get("usage")
            .and_then(Value::as_object)
            .or_else(|| provider.get("usage").and_then(Value::as_object))
            .or_else(|| provider.get("rawUsage").and_then(Value::as_object))
        else {
            continue;
        };

        let parts = transcript_usage(usage);
        if parts.tally.total() + parts.unclassified <= 0 {
            continue;
        }

        let session = logio::text(row.get("sessionId")).unwrap_or_else(|| stem.clone());
        let model = editorlog::model_id(
            logio::text(provider.get("model"))
                .or_else(|| logio::text(provider.get("requestModelId")))
                .or_else(|| logio::text(message.get("model"))),
        )
        .unwrap_or_else(|| client.to_string());

        let identity = logio::text(provider.get("messageId"))
            .or_else(|| logio::text(provider.get("traceId")))
            .or_else(|| logio::text(row.get("id")));

        let mut record = AgentUsageRecord::new(timestamp, &model, parts.tally).session(&session);
        record.unclassified_tokens = parts.unclassified;
        record.project = logio::text(row.get("cwd"));

        match identity {
            Some(identity) => {
                let key = format!("{client}:{session}:{identity}");
                record.deduplication_id = Some(key.clone());
                if identified.get(&key).is_some_and(|existing| total(existing) >= total(&record)) {
                    continue;
                }
                identified.insert(key, record);
            }
            None => unidentified.push(record),
        }
    }

    let mut records: Vec<AgentUsageRecord> = identified.into_values().collect();
    records.extend(unidentified);
    records
}

fn transcript_usage(usage: &Object) -> UsageParts {
    editorlog::combine(&Reported {
        input: editorlog::first_count(usage, &["input_tokens", "inputTokens", "prompt_tokens"]),
        output: editorlog::first_count(usage, &["output_tokens", "outputTokens", "completion_tokens"]),
        cache_read: editorlog::first_count(
            usage,
            &["cache_read_input_tokens", "cacheReadInputTokens", "cacheTokens", "prompt_cache_hit_tokens", "cached_tokens"],
        ),
        cache_write: editorlog::first_count(
            usage,
            &["cache_creation_input_tokens", "cacheCreationInputTokens", "cachedWriteTokens", "prompt_cache_write_tokens"],
        ),
        reasoning: editorlog::first_count(usage, &["completion_thinking_tokens", "completionThinkingTokens", "reasoningTokens"]),
        total: editorlog::first_count(usage, &["total_tokens", "totalTokens"]),
        exclusive_input: editorlog::first_count(usage, &["cachedMissTokens", "cacheMissTokens"]),
        input_excludes_cache: false,
    })
}

// MARK: - Extension log

/// The `[CraftInvokableAgent]` model lines and `[AgentReporter]` usage lines. With `first_only`,
/// stops after the first record (the fallback then only proves that a partial flag is due).
fn extension_log(client: &str, file: &std::path::Path, first_only: bool) -> Vec<AgentUsageRecord> {
    let workspace = file.file_name().and_then(|n| n.to_str()).and_then(|name| name.split("__").next()).and_then(nonblank);

    let mut models: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut records = Vec::new();

    loglines::for_each_line(file, |line| {
        if first_only && !records.is_empty() {
            return;
        }
        let Ok(raw) = std::str::from_utf8(line) else { return };
        if !(raw.contains("[CraftInvokableAgent]") || raw.contains("[AgentReporter]")) {
            return;
        }
        let Some(timestamp) = editorlog::naive_timestamp(raw) else { return };

        if let Some((agent, model)) = prepared(raw) {
            models.insert(agent, model);
            return;
        }
        let Some((agent, usage)) = reported(raw) else { return };

        let parts = extension_usage(&usage);
        if parts.tally.total() + parts.unclassified <= 0 {
            return;
        }
        // The line carries no message id: two real requests in one second with equal counts are
        // two records, never folded by timestamp.
        let model = editorlog::model_id(models.get(&agent).cloned()).unwrap_or_else(|| client.to_string());
        let mut record = AgentUsageRecord::new(timestamp, &model, parts.tally).session(&agent);
        record.unclassified_tokens = parts.unclassified;
        record.project = workspace.clone();
        records.push(record);
    });
    records
}

fn extension_usage(usage: &Object) -> UsageParts {
    editorlog::combine(&Reported {
        input: editorlog::first_count(usage, &["inputTokens", "prompt_tokens"]),
        output: editorlog::first_count(usage, &["outputTokens", "output_tokens"]),
        cache_read: editorlog::first_count(usage, &["cacheTokens", "cachedReadTokens", "cache_read_input_tokens"]),
        cache_write: editorlog::first_count(usage, &["cachedWriteTokens", "cacheCreationTokens", "cache_creation_input_tokens"]),
        reasoning: editorlog::first_count(usage, &["reasoningTokens", "completionThinkingTokens"]),
        total: editorlog::first_count(usage, &["totalTokens", "total_tokens"]),
        exclusive_input: editorlog::first_count(usage, &["cachedMissTokens", "cacheMissTokens"]),
        input_excludes_cache: false,
    })
}

/// `[CraftInvokableAgent][agent] ... Model prepared: provider/model (model)`: the agent and model.
fn prepared(line: &str) -> Option<(String, String)> {
    let marker = line.find("[CraftInvokableAgent]")? + "[CraftInvokableAgent]".len();
    let agent = bracketed(line, marker)?;
    let label = marker + line[marker..].find("Model prepared:")? + "Model prepared:".len();
    let rest = &line[label..];
    let open = rest.rfind('(')?;
    let close = rest.rfind(')')?;
    if open >= close {
        return None;
    }
    let model = rest[open + 1..close].trim().to_string();
    (!model.is_empty()).then_some((agent, model))
}

/// `[AgentReporter][agent] ... usage: {...}`: the agent and its usage object.
fn reported(line: &str) -> Option<(String, Object)> {
    let marker = line.find("[AgentReporter]")? + "[AgentReporter]".len();
    let agent = bracketed(line, marker)?;
    let at = marker + line[marker..].find("usage:")? + "usage:".len();
    let usage = editorlog::brace_object(&line[at..])?;
    Some((agent, usage))
}

/// The first `[value]` at or after `from`, where the agent id sits.
fn bracketed(line: &str, from: usize) -> Option<String> {
    let open = from + line[from..].find('[')? + 1;
    let close = open + line[open..].find(']')?;
    nonblank(&line[open..close])
}

fn nonblank(text: &str) -> Option<String> {
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// WorkBuddy's SQLite fallback, read only for the `workbuddy` client. Rows are session-level
/// totals placed at their own write time and counted as unclassified, never as fresh input.
pub(crate) fn workbuddy_database(client: &str, file: &std::path::Path) -> Vec<AgentUsageRecord> {
    use crate::spend::sqlite;

    if client != "workbuddy" {
        return Vec::new();
    }
    let Some(connection) = sqlite::open(file) else { return Vec::new() };

    let as_count = |text: Option<String>| text.and_then(|t| t.trim().parse::<i64>().ok()).filter(|v| *v >= 0);

    let mut sessions: std::collections::HashMap<String, (Option<String>, Option<String>)> = std::collections::HashMap::new();
    sqlite::each(&connection, "SELECT id, cwd, model FROM sessions", |row| {
        if let Some(id) = sqlite::text(row, 0) {
            sessions.insert(id, (sqlite::text(row, 1), sqlite::text(row, 2)));
        }
    });

    let mut found = Vec::new();
    sqlite::each(&connection, "SELECT session_id, used, updated_at FROM session_usage", |row| {
        let Some(session_id) = sqlite::text(row, 0) else { return };
        let Some(used) = as_count(sqlite::text(row, 1)).filter(|v| *v > 0) else { return };
        let Some(updated) = as_count(sqlite::text(row, 2)).filter(|v| *v > 0) else { return };
        let Some(at) = editorlog::auto_epoch(Some(updated)) else { return };

        let (cwd, model) = sessions.get(&session_id).cloned().unwrap_or((None, None));
        let mut record = AgentUsageRecord::new(at, &editorlog::model_id(model).unwrap_or_else(|| "auto".to_string()), TokenTally::default()).session(&session_id);
        record.unclassified_tokens = used;
        record.is_aggregate = true;
        record.project = cwd.as_deref().and_then(nonblank);
        // One row is the session's whole aggregate, so the identity includes the write that made it.
        record.deduplication_id = Some(format!("workbuddy:{session_id}:{updated}"));
        found.push(record);
    });
    found
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use chrono::{Local, TimeZone, Utc};
    use serde_json::json;

    use super::*;
    use crate::spend::calendar::Calendar;
    use crate::spend::prices::PriceTable;
    use crate::spend::sources::{read_agent, SpendAgent};

    const MIDNIGHT: i64 = 1_789_344_000; // 2026-09-14 00:00 UTC

    fn write_lines(path: &Path, lines: &[Value]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let text: Vec<String> = lines.iter().map(Value::to_string).collect();
        std::fs::write(path, text.join("\n")).unwrap();
    }

    fn transcript(home: &Path) -> PathBuf {
        home.join(".codebuddy").join("projects").join("p").join("sess-a.jsonl")
    }

    #[test]
    fn the_transcript_reads_each_kind_and_folds_a_replayed_message_to_its_fuller_snapshot() {
        let home = tempfile::tempdir().unwrap();
        let usage = json!({"input_tokens":100,"output_tokens":40,"cache_read_input_tokens":30,"cache_creation_input_tokens":20});
        write_lines(
            &transcript(home.path()),
            &[
                json!({"timestamp":MIDNIGHT*1000,"type":"message","role":"assistant","sessionId":"sess-a","cwd":"/work/pulse",
                       "message":{"usage":usage},"providerData":{"model":"tencent/gpt-x","messageId":"m1"}}),
                // The same message replayed with a smaller snapshot: the fuller one stays.
                json!({"timestamp":MIDNIGHT*1000,"type":"message","role":"assistant","sessionId":"sess-a",
                       "message":{"usage":{"input_tokens":10,"output_tokens":1}},"providerData":{"model":"gpt-x","messageId":"m1"}}),
                // Not a reply, not completed, no usage, no time: none counts.
                json!({"timestamp":MIDNIGHT*1000,"type":"message","role":"user","message":{"usage":{"input_tokens":5}}}),
                json!({"timestamp":MIDNIGHT*1000,"type":"message","role":"assistant","status":"streaming","message":{"usage":{"input_tokens":5}}}),
                json!({"timestamp":MIDNIGHT*1000,"type":"message","role":"assistant","message":{}}),
                json!({"type":"message","role":"assistant","message":{"usage":{"input_tokens":5}}}),
                // An unidentified call is kept every time.
                json!({"timestamp":(MIDNIGHT+60)*1000,"type":"function_call","providerData":{"rawUsage":{"prompt_tokens":7,"completion_tokens":2}}}),
            ],
        );
        let records = read("codebuddy", &[home.path().join(".codebuddy")]);
        assert_eq!(records.len(), 2);
        let folded = records.iter().find(|r| r.deduplication_id.is_some()).unwrap();
        // No total to settle the overlap, so the reported input is kept as it is.
        assert_eq!(folded.tally, TokenTally::new(100, 20, 30, 40));
        assert_eq!(folded.model, "gpt-x");
        assert_eq!(folded.project.as_deref(), Some("/work/pulse"));
        assert_eq!(folded.session_id.as_deref(), Some("sess-a"));
        assert!(!folded.is_partial);
        let kept = records.iter().find(|r| r.deduplication_id.is_none()).unwrap();
        assert_eq!(kept.tally, TokenTally::new(7, 0, 0, 2));
        assert_eq!(kept.model, "codebuddy");
    }

    #[test]
    fn a_bare_total_is_unclassified_and_a_cache_inside_input_is_taken_out() {
        let home = tempfile::tempdir().unwrap();
        write_lines(
            &transcript(home.path()),
            &[
                json!({"timestamp":MIDNIGHT*1000,"type":"message","role":"assistant","sessionId":"s",
                       "message":{"usage":{"total_tokens":900}},"providerData":{"messageId":"bare"}}),
                json!({"timestamp":(MIDNIGHT+1)*1000,"type":"message","role":"assistant","sessionId":"s",
                       "message":{"usage":{"input_tokens":100,"output_tokens":10,"cache_read_input_tokens":30,"total_tokens":110}},"providerData":{"messageId":"inc","model":"m"}}),
            ],
        );
        let mut records = read("codebuddy", &[home.path().join(".codebuddy")]);
        records.sort_by_key(|r| r.timestamp);
        assert_eq!(records[0].tally, TokenTally::default());
        assert_eq!(records[0].unclassified_tokens, 900);
        // The reported total of 110 is input + output, so the cache is inside input and comes out of it.
        assert_eq!(records[1].tally, TokenTally::new(70, 0, 30, 10));
    }

    #[test]
    fn a_transcript_present_makes_the_extension_log_a_partial_fallback_not_an_addition() {
        let home = tempfile::tempdir().unwrap();
        write_lines(
            &transcript(home.path()),
            &[json!({"timestamp":MIDNIGHT*1000,"type":"message","role":"assistant","sessionId":"sess-a","message":{"usage":{"input_tokens":5,"output_tokens":5}},"providerData":{"messageId":"t1"}})],
        );
        let log = home.path().join(".codebuddy").join("logs").join("proj__ws.log");
        let at = Local.with_ymd_and_hms(2026, 9, 14, 10, 0, 0).single().unwrap();
        std::fs::create_dir_all(log.parent().unwrap()).unwrap();
        std::fs::write(
            &log,
            format!(
                "{stamp} [CraftInvokableAgent][agent-1] Model prepared: tencent/glm (glm-5)\n{stamp} [AgentReporter][agent-1] usage: {{\"inputTokens\":100,\"outputTokens\":9}}\n",
                stamp = at.format("%Y/%m/%d %H:%M:%S%.3f")
            ),
        )
        .unwrap();
        let records = read("codebuddy", &[home.path().join(".codebuddy")]);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].tally, TokenTally::new(5, 0, 0, 5));
        assert!(records[0].is_partial);
    }

    #[test]
    fn with_no_transcript_the_extension_log_counts_each_line_and_names_the_model_it_was_prepared_with() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join(".codebuddy").join("logs").join("proj__ws.log");
        let at = Local.with_ymd_and_hms(2026, 9, 14, 10, 0, 0).single().unwrap();
        let stamp = at.format("%Y/%m/%d %H:%M:%S%.3f");
        std::fs::create_dir_all(log.parent().unwrap()).unwrap();
        std::fs::write(
            &log,
            format!(
                "{stamp} [CraftInvokableAgent][agent-1] Model prepared: tencent/glm (glm-5)\n\
                 {stamp} [AgentReporter][agent-1] usage: {{\"inputTokens\":100,\"cacheTokens\":30,\"outputTokens\":9}}\n\
                 {stamp} [AgentReporter][agent-1] usage: {{\"inputTokens\":100,\"cacheTokens\":30,\"outputTokens\":9}}\n\
                 no time [AgentReporter][agent-1] usage: {{\"inputTokens\":1}}\n"
            ),
        )
        .unwrap();
        let records = read("codebuddy", &[home.path().join(".codebuddy")]);
        // Two real identical lines are two requests; the line with no time is not one.
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].tally, TokenTally::new(100, 0, 30, 9));
        assert_eq!(records[0].model, "glm-5");
        assert_eq!(records[0].project.as_deref(), Some("proj"));
        assert_eq!(records[0].timestamp, at.with_timezone(&Utc));
        assert!(!records[0].is_partial);
    }

    #[test]
    fn a_missing_store_is_an_empty_ledger_and_a_binaries_folder_is_not_read() {
        let home = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let sources = Sources::new(home.path());
        assert!(read_agent(SpendAgent::CodeBuddy, &sources, cache.path(), &Calendar::utc(2), &PriceTable::new()).days.is_empty());
        let log = home.path().join(".codebuddy").join("binaries").join("x.log");
        std::fs::create_dir_all(log.parent().unwrap()).unwrap();
        std::fs::write(&log, "2026/09/14 10:00:00.000 [AgentReporter][a] usage: {\"inputTokens\":4}\n").unwrap();
        assert!(read("codebuddy", &[home.path().join(".codebuddy")]).is_empty());
    }
}
