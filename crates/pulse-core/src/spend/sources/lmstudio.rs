// Ported from upstream Sources/Pulse/Usage/Readers/LMStudioUsageReader.swift.
//! LM Studio's server logs.
//!
//! `~/.lmstudio/server-logs/**/*.log` is pretty-printed OpenAI-compatible server output, so the usage
//! is not one JSON object per line. Each response ends with a balanced `"usage": { ... }` block, and
//! the response `id`, `model` and a local log timestamp sit in the text just before it. This reader
//! finds those blocks, reads the facts around them, and keeps no part of a prompt or a completion.
//!
//! **The timestamp is the log line's own and nothing else.** A block with no local
//! `YYYY-MM-DD HH:MM:SS` prefix is skipped; the file's modification time or the clock is never used.
//!
//! **The prompt is cache-inclusive and completion includes reasoning.** Cache read and cache write are
//! clamped to the prompt, the total is at least prompt + completion, and fresh input is the total minus
//! everything already accounted for. Completion is kept whole as output, since its reasoning detail is
//! a subset the output bucket already counts once. Local inference has no money behind it, so no cost
//! is read.
//!
//! Where it lives on Windows: `LM_STUDIO_HOME` when set, else `%USERPROFILE%\.lmstudio\server-logs`.
//! WINDOWS-PATH: unverified.

use std::path::PathBuf;

use chrono::{DateTime, Local, NaiveDateTime, TimeZone, Utc};
use regex::Regex;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use super::{push_unique, SpendSource};
use crate::spend::logio;
use crate::spend::record::AgentUsageRecord;
use crate::spend::tally::TokenTally;
use crate::spend::transcripts::Sources;

const PROMPT: &[&str] = &["prompt_tokens", "promptTokens", "input_tokens", "inputTokens"];
const COMPLETION: &[&str] = &["completion_tokens", "completionTokens", "output_tokens", "outputTokens"];
const TOTAL: &[&str] = &["total_tokens", "totalTokens"];
const PROMPT_DETAILS: &[&str] = &["prompt_tokens_details", "input_tokens_details", "inputTokensDetails"];

/// How far back from a `"usage"` key the response's facts are looked for.
const CONTEXT_BYTES: usize = 4096;

pub struct LmStudio;

impl SpendSource for LmStudio {
    fn raw(&self) -> &'static str {
        "lmStudio"
    }
    fn source_id(&self) -> &'static str {
        "lmstudio"
    }
    fn display_name(&self) -> &'static str {
        "LM Studio"
    }
    fn icon(&self) -> Option<&'static str> {
        Some("lmstudio")
    }
    fn env_vars(&self) -> &'static [&'static str] {
        &["LM_STUDIO_HOME"]
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        let mut roots = Vec::new();
        // WINDOWS-PATH: unverified (the macOS layout carried over)
        let base = sources.var("LM_STUDIO_HOME").unwrap_or_else(|| sources.home.join(".lmstudio"));
        push_unique(&mut roots, base.join("server-logs"));
        roots
    }
    fn records(&self, roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        let mut records = Vec::new();
        for file in logio::files(roots, &["log"], &[], &[]) {
            let Ok(text) = std::fs::read_to_string(&file) else { continue };
            let path = std::fs::canonicalize(&file).unwrap_or_else(|_| file.clone()).to_string_lossy().into_owned();
            let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();

            let mut cursor = 0;
            while let Some(found) = text[cursor..].find("\"usage\"").map(|p| cursor + p) {
                let after = found + "\"usage\"".len();
                let Some((block, end)) = usage_block(&text, after) else {
                    cursor = after;
                    continue;
                };
                cursor = end;

                let Ok(Value::Object(usage)) = serde_json::from_str::<Value>(block) else { continue };
                let context = preceding_context(&text, found);
                let Some(at) = log_timestamp(context) else { continue };
                let Some(model) = last_capture(r#""model"\s*:\s*"([^"]*)""#, context) else { continue };

                let Some((tally, unclassified)) = counts(&usage) else { continue };
                let identity = last_capture(r#""id"\s*:\s*"([^"]*)""#, context).unwrap_or_else(|| {
                    let digest = Sha256::digest(format!("{path}:{found}:{model}:{}:{}:{}:{}", tally.input, tally.cache_write, tally.cache_read, tally.output));
                    format!("{digest:x}")
                });

                if model.trim().is_empty() || (tally.total() <= 0 && unclassified <= 0) {
                    continue;
                }
                let mut record = AgentUsageRecord::new(at, &model, tally).session(&format!("lmstudio:{path}")).unclassified(unclassified);
                record.session_name = Some(name.clone());
                record.deduplication_id = Some(format!("lmstudio:{identity}"));
                records.push(record);
            }
        }
        records
    }
}

/// The balanced `{ ... }` after a `"usage"` key that ends at `after`, and the index just past it.
fn usage_block(text: &str, after: usize) -> Option<(&str, usize)> {
    let bytes = text.as_bytes();
    let skip_space = |mut i: usize| {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        i
    };
    let mut i = skip_space(after);
    if bytes.get(i) != Some(&b':') {
        return None;
    }
    i = skip_space(i + 1);
    if bytes.get(i) != Some(&b'{') {
        return None;
    }

    let start = i;
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    while i < bytes.len() {
        let c = bytes[i];
        if in_string {
            if escaped {
                escaped = false;
            } else if c == b'\\' {
                escaped = true;
            } else if c == b'"' {
                in_string = false;
            }
        } else if c == b'"' {
            in_string = true;
        } else if c == b'{' {
            depth += 1;
        } else if c == b'}' {
            depth -= 1;
            if depth == 0 {
                return Some((&text[start..=i], i + 1));
            }
        }
        i += 1;
    }
    None
}

/// Up to `CONTEXT_BYTES` of text before `start`, cut on a character boundary.
fn preceding_context(text: &str, start: usize) -> &str {
    let mut from = start.saturating_sub(CONTEXT_BYTES);
    while !text.is_char_boundary(from) {
        from += 1;
    }
    &text[from..start]
}

/// The last capture of `pattern` in `text`, so an id repeated in a listing resolves to the one nearest
/// the block. Blank is absent.
fn last_capture(pattern: &str, text: &str) -> Option<String> {
    let regex = Regex::new(pattern).ok()?;
    let captured = regex.captures_iter(text).last()?.get(1)?.as_str().trim();
    (!captured.is_empty()).then(|| captured.to_string())
}

/// The local `YYYY-MM-DD HH:MM:SS` prefix nearest the block, read in this machine's own time zone.
fn log_timestamp(text: &str) -> Option<DateTime<Utc>> {
    let raw = last_capture(r"(\d{4}-\d{2}-\d{2}[ T]\d{2}:\d{2}:\d{2})", text)?;
    let naive = NaiveDateTime::parse_from_str(&raw.replace('T', " "), "%Y-%m-%d %H:%M:%S").ok()?;
    Local.from_local_datetime(&naive).earliest().map(|t| t.with_timezone(&Utc)).filter(|t| t.timestamp() > 0)
}

/// The first key that is present as a count, as the Swift reader took it.
fn first_count(container: &Map<String, Value>, keys: &[&str]) -> Option<i64> {
    keys.iter().find_map(|key| logio::count(container.get(*key)))
}

/// The first named object, if any.
fn first_object<'a>(usage: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a Map<String, Value>> {
    keys.iter().find_map(|key| usage.get(*key).and_then(Value::as_object))
}

/// The tally and unclassified remainder of one usage object, with the cache and completion clamps
/// applied; None when it carries no count at all.
fn counts(usage: &Map<String, Value>) -> Option<(TokenTally, i64)> {
    let prompt = first_count(usage, PROMPT);
    let completion = first_count(usage, COMPLETION);
    let reported_total = first_count(usage, TOTAL);
    let empty = Map::new();
    let prompt_details = first_object(usage, PROMPT_DETAILS).unwrap_or(&empty);

    let cached = first_count(prompt_details, &["cached_tokens"]).or_else(|| first_count(usage, &["cached_tokens"]));
    let cache_creation = first_count(prompt_details, &["cache_creation_input_tokens"]).or_else(|| first_count(usage, &["cache_creation_input_tokens"]));
    // `reasoning_tokens` is a detail of completion, which the output bucket already counts once, so it
    // is not read.

    // A bare total with no prompt or completion is real but cannot be classified.
    if prompt.is_none() && completion.is_none() {
        return reported_total.filter(|t| *t > 0).map(|total| (TokenTally::default(), total));
    }

    let prompt_tokens = prompt.unwrap_or(0);
    let completion_tokens = completion.unwrap_or(0);
    let cache_read = cached.unwrap_or(0).min(prompt_tokens);
    let cache_write = cache_creation.unwrap_or(0).min((prompt_tokens - cache_read).max(0));
    let total = reported_total.unwrap_or(0).max(prompt_tokens + completion_tokens);
    let input = (total - completion_tokens - cache_read - cache_write).max(0);
    Some((TokenTally::new(input, cache_write, cache_read, completion_tokens), 0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spend::calendar::Calendar;
    use crate::spend::prices::PriceTable;
    use crate::spend::sources::{read_agent, SpendAgent};
    use crate::spend::summary::SpendSummary;
    use crate::spend::transcripts::Sources as Home;

    fn log(home: &std::path::Path, name: &str, text: &str) {
        let dir = home.join(".lmstudio").join("server-logs").join("2026-09");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(name), text).unwrap();
    }

    fn response(stamp: &str, id: &str, usage: &str) -> String {
        format!(
            "{stamp} [INFO] POST /v1/chat/completions\n{{\n  \"id\": \"{id}\",\n  \"model\": \"priced\",\n  \"usage\": {usage}\n}}\n"
        )
    }

    #[test]
    fn the_prompt_is_cache_inclusive_and_completion_is_kept_whole_with_reasoning_inside() {
        let home = tempfile::tempdir().unwrap();
        let usage = r#"{"prompt_tokens": 100, "prompt_tokens_details": {"cached_tokens": 30}, "completion_tokens": 40, "completion_tokens_details": {"reasoning_tokens": 6}}"#;
        log(home.path(), "server.log", &response("2026-09-14 10:20:30", "chat-1", usage));
        let records = LmStudio.records(&LmStudio.inputs(&Home::new(home.path())));
        assert_eq!(records.len(), 1);
        // total = 140, input = 140 - 40 - 30 = 70, cache read 30, output 40 (reasoning is already in it).
        assert_eq!(records[0].tally, TokenTally::new(70, 0, 30, 40));
        assert_eq!(records[0].deduplication_id.as_deref(), Some("lmstudio:chat-1"));
        assert_eq!(records[0].timestamp.with_timezone(&Local).format("%H:%M:%S").to_string(), "10:20:30");
    }

    #[test]
    fn a_bare_total_is_unclassified_and_a_block_with_no_time_or_model_is_skipped() {
        let home = tempfile::tempdir().unwrap();
        log(home.path(), "server.log", &response("2026-09-14 10:20:30", "bare", r#"{"total_tokens": 25}"#));
        // No time before the block, and no model in its context: neither is counted.
        log(home.path(), "rest.log", "{\"id\": \"untimed\", \"model\": \"priced\", \"usage\": {\"prompt_tokens\": 9, \"completion_tokens\": 1}}\n");
        log(home.path(), "nomodel.log", "2026-09-14 10:21:00 no model here\n\"usage\": {\"prompt_tokens\": 9, \"completion_tokens\": 1}\n");
        let records = LmStudio.records(&LmStudio.inputs(&Home::new(home.path())));
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].tally.total(), 0);
        assert_eq!(records[0].unclassified_tokens, 25);
    }

    #[test]
    fn a_repeated_id_counts_once_and_a_missing_store_is_empty() {
        let home = tempfile::tempdir().unwrap();
        let usage = r#"{"prompt_tokens": 4, "completion_tokens": 1}"#;
        let text = format!("{}{}", response("2026-09-14 10:20:30", "same", usage), response("2026-09-14 10:20:31", "same", usage));
        log(home.path(), "server.log", &text);
        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::LmStudio, &Home::new(home.path()), cache.path(), &Calendar::utc(2), &PriceTable::new());
        assert_eq!(ledger.days.iter().map(|d| d.tokens).sum::<i64>(), 5);

        // A home with no store, read with no history kept.
        let (empty, cache) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let ledger = read_agent(SpendAgent::LmStudio, &Home::new(empty.path()), cache.path(), &Calendar::utc(2), &PriceTable::new());
        assert!(ledger.days.is_empty());
    }

    #[test]
    fn the_home_override_moves_the_store_and_a_string_in_the_body_does_not_end_the_block() {
        let home = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let dir = elsewhere.path().join("server-logs");
        std::fs::create_dir_all(&dir).unwrap();
        let text = response("2026-09-14 10:20:30", "brace", r#"{"prompt_tokens": 3, "completion_tokens": 2, "note": "a } inside \" quotes"}"#);
        std::fs::write(dir.join("a.log"), text).unwrap();
        let sources = Home::new(home.path()).with_var("LM_STUDIO_HOME", elsewhere.path());
        let records = LmStudio.records(&LmStudio.inputs(&sources));
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].tally.total(), 5);
    }

    #[test]
    fn a_session_buckets_reach_today_and_only_todays_counts() {
        use chrono::Duration;
        let home = tempfile::tempdir().unwrap();
        let calendar = Calendar::local();
        let now = Utc::now();
        let stamp = |day: DateTime<Utc>| {
            let local = calendar.start_of_day(day) + Duration::hours(12);
            local.with_timezone(&Local).format("%Y-%m-%d %H:%M:%S").to_string()
        };
        let yesterday = calendar.add_days(calendar.start_of_day(now), -1);
        let usage = r#"{"prompt_tokens": 100, "completion_tokens": 0}"#;
        let text = format!("{}{}", response(&stamp(now), "t", usage), response(&stamp(yesterday), "y", r#"{"prompt_tokens": 900, "completion_tokens": 0}"#));
        log(home.path(), "server.log", &text);
        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::LmStudio, &Home::new(home.path()), cache.path(), &calendar, &PriceTable::new());
        let summary = SpendSummary::of(&std::collections::HashMap::from([(SpendAgent::LmStudio, ledger)]), Some(1), now, &calendar);
        assert_eq!(summary.tokens, 100);
        assert_eq!(summary.sessions[0].session.unpriced_tokens, 100);
    }

}
