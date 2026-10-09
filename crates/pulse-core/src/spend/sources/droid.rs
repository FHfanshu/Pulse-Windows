// Ported from upstream Sources/Pulse/Usage/Readers/DroidSessionReader.swift
// and Sources/Pulse/Usage/Readers/SessionLogPaths.swift (droid).
//! Droid's session store: real totals, no per-reply counts.
//!
//! `~/.factory/sessions/<session>.settings.json` holds the session's **cumulative** usage, and a
//! sibling `<session>.jsonl` transcript holds the assistant replies with their times but no
//! tokens. Splitting a session total across replies would invent a per-reply split, so this reader
//! emits **one aggregate record** per session, marked `is_aggregate`: it lands on its day and never
//! in an hour profile.
//!
//! **The input and cache relation is never assumed.** A reported total, when present, is the only
//! authority: it can prove the cache read inside the prompt (subtract) or beside it (disjoint), and
//! a total that proves neither is carried as unclassified. With no total and a positive cache, the
//! reported output is kept, the reported input is carried as unclassified, the cache is not added,
//! and the record is partial. A positive thinking count with no cache also marks the record partial,
//! because its relation to output is undocumented.
//!
//! The time is the store's own `providerLockTimestamp`. A session with recognised usage but no
//! locatable time is not emitted, and the readable siblings are marked partial. The model id is kept
//! as written (minus a `custom:` prefix and a bracketed qualifier); a provider with no usable model
//! gets a valueless `*-unknown` placeholder, never a concrete model that would price another's rates.
//!
//! Where it lives on Windows: `%USERPROFILE%\.factory\sessions`. Unverified on a real PC.

use std::path::{Path, PathBuf};

use regex::Regex;
use serde_json::{Map, Value};

use super::SpendSource;
use crate::spend::logio;
use crate::spend::loglines::LineReader;
use crate::spend::record::AgentUsageRecord;
use crate::spend::tally::TokenTally;
use crate::spend::transcripts::Sources;

pub struct Droid;

impl SpendSource for Droid {
    fn raw(&self) -> &'static str {
        "droid"
    }
    fn source_id(&self) -> &'static str {
        "droid"
    }
    fn display_name(&self) -> &'static str {
        "Droid"
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        // WINDOWS-PATH: unverified (`%USERPROFILE%\.factory` is the macOS layout carried over)
        vec![sources.home.join(".factory").join("sessions")]
    }
    fn records(&self, roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        let mut result = Vec::new();
        let mut incomplete = false;
        for file in logio::files(roots, &["json"], &[], &[]) {
            if !file.file_name().is_some_and(|n| n.to_string_lossy().ends_with(".settings.json")) {
                continue;
            }
            let evaluation = evaluate(&file);
            if let Some(record) = evaluation.record {
                result.push(record);
            }
            incomplete |= evaluation.saw_unusable_usage;
        }
        if incomplete {
            for record in &mut result {
                record.is_partial = true;
            }
        }
        result
    }
}

/// What one settings file contributed: its record, and whether it held usage that could not be counted.
struct Evaluation {
    record: Option<AgentUsageRecord>,
    saw_unusable_usage: bool,
}

fn nothing() -> Evaluation {
    Evaluation { record: None, saw_unusable_usage: false }
}

fn evaluate(path: &Path) -> Evaluation {
    let Some(root) = logio::json(path).and_then(|v| v.as_object().cloned()) else { return nothing() };
    let Some(usage) = root.get("tokenUsage").and_then(Value::as_object) else { return nothing() };
    let Some((tally, unclassified, is_partial)) = decode(usage) else { return nothing() };
    if tally.total() <= 0 && unclassified <= 0 {
        return nothing();
    }

    // From here the file holds recognised usage; a missing locatable field makes it uncountable.
    let unusable = Evaluation { record: None, saw_unusable_usage: true };
    let Some(timestamp) = logio::timestamp(root.get("providerLockTimestamp"), false) else { return unusable };
    let session = session_id(path);
    let model = normalize(logio::text(root.get("model")))
        .or_else(|| transcript_model(path))
        .or_else(|| provider_default(logio::text(root.get("providerLock")).as_deref()));
    let Some(model) = model else { return unusable };

    let mut record = AgentUsageRecord::new(timestamp, &model, tally).session(&session).aggregate(true);
    record.unclassified_tokens = unclassified;
    record.is_partial = is_partial;
    record.deduplication_id = Some(format!("droid:{session}"));
    Evaluation { record: Some(record), saw_unusable_usage: false }
}

/// The file name with `.settings.json` removed.
fn session_id(path: &Path) -> String {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    match name.strip_suffix(".settings.json") {
        Some(stem) => stem.to_string(),
        None => path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
    }
}

/// Keeps the model id as the store wrote it. Only an explicit `custom:` transport prefix and a
/// bracketed qualifier are removed; case and punctuation (`claude-opus-4.5`) are part of pricing.
fn normalize(raw: Option<String>) -> Option<String> {
    let mut value = raw?;
    if let Some(rest) = value.strip_prefix("custom:") {
        value = rest.to_string();
    }
    let bracket = Regex::new(r"\[[^\]]*\]").expect("a fixed pattern");
    let value = bracket.replace_all(&value, "").into_owned();
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// A transcript system-reminder line names the model when the settings file does not:
/// `Model: <name>` in the sibling `<session>.jsonl`.
fn transcript_model(settings: &Path) -> Option<String> {
    let transcript = settings.parent()?.join(format!("{}.jsonl", session_id(settings)));
    let file = std::fs::File::open(transcript).ok()?;
    let mut lines = LineReader::new(file);
    while let Some(line) = lines.next_line() {
        let Ok(text) = std::str::from_utf8(line) else { continue };
        let Some(position) = text.find("Model:") else { continue };
        let remainder = text[position + "Model:".len()..].trim_start_matches([' ', '\t']);
        let name: String = remainder
            .chars()
            .take_while(|c| !c.is_whitespace() && *c != '<' && *c != '"' && *c != '\\')
            .collect();
        if let Some(normalized) = normalize(Some(name)) {
            return Some(normalized);
        }
    }
    None
}

/// A valueless placeholder for a provider with no usable model. Never a concrete model.
fn provider_default(provider: Option<&str>) -> Option<String> {
    let provider = provider?.to_lowercase();
    if provider.is_empty() {
        return None;
    }
    let family = if provider.contains("anthropic") || provider.contains("claude") {
        "claude".to_string()
    } else if provider.contains("openai") || provider.contains("gpt") {
        "gpt".to_string()
    } else if provider.contains("google") || provider.contains("gemini") {
        "gemini".to_string()
    } else if provider.contains("xai") || provider.contains("grok") {
        "grok".to_string()
    } else {
        provider
    };
    Some(format!("{family}-unknown"))
}

/// Splits one `tokenUsage` object: the tally, the unclassified remainder, and whether the split is
/// partial. None when nothing countable was reported.
fn decode(usage: &Map<String, Value>) -> Option<(TokenTally, i64, bool)> {
    let input = logio::count(usage.get("inputTokens"));
    let output = logio::count(usage.get("outputTokens"));
    let thinking = logio::count(usage.get("thinkingTokens"));
    let cache_write = logio::count(usage.get("cacheCreationTokens"));
    let cache_read = logio::count(usage.get("cacheReadTokens"));
    let total = logio::count(usage.get("totalTokens"))
        .or_else(|| logio::count(usage.get("total")))
        .or_else(|| logio::count(usage.get("total_tokens")));

    if input.is_none() && output.is_none() && thinking.is_none() && cache_write.is_none() && cache_read.is_none() && total.is_none() {
        return None;
    }

    let i = input.unwrap_or(0);
    let o = output.unwrap_or(0);
    let k = thinking.unwrap_or(0);
    let cw = cache_write.unwrap_or(0);
    let cr = cache_read.unwrap_or(0);
    let output_bucket = output.or(thinking).unwrap_or(0);

    if let Some(total) = total {
        // Reasoning inside or independent of output, cache inside or beside input: four candidate
        // identities. A total that matches exactly one settles both.
        let variants = [
            (i, cw, cr, o + k),
            ((i - cr - cw).max(0), cw, cr, o + k),
            (i, cw, cr, o),
            ((i - cr - cw).max(0), cw, cr, o),
        ];
        for (input, cache_write, cache_read, output) in variants {
            if input + cache_write + cache_read + output == total {
                return Some((TokenTally::new(input, cache_write, cache_read, output), 0, false));
            }
        }
        return Some((TokenTally::default(), total, false));
    }

    if cr == 0 && cw == 0 {
        // No cache ambiguity. A positive thinking count has an undocumented relation to output, so it marks the record partial.
        return Some((TokenTally::new(i, 0, 0, output_bucket), 0, k > 0));
    }

    // A positive cache with no total: the relation is not provable. Keep the reported output, carry the
    // reported input as a known unknown, add nothing for the cache, and say the count is partial.
    Some((TokenTally { output: output_bucket, ..TokenTally::default() }, i, true))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::spend::calendar::Calendar;
    use crate::spend::prices::{ModelPrice, PriceTable};
    use crate::spend::sources::{read_agent, SpendAgent};

    fn prices() -> PriceTable {
        HashMap::from([("gpt-5.1".to_string(), ModelPrice::new(1.0, 10.0, Some(0.1), Some(1.0), Some("GPT-5.1")))])
    }

    fn write(home: &Path, name: &str, text: &str) {
        let path = home.join(".factory/sessions").join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn records_of(home: &Path) -> Vec<AgentUsageRecord> {
        Droid.records(&[home.join(".factory/sessions")])
    }

    fn by_session<'a>(records: &'a [AgentUsageRecord], session: &str) -> &'a AgentUsageRecord {
        records.iter().find(|r| r.session_id.as_deref() == Some(session)).unwrap()
    }

    #[test]
    fn with_a_cache_and_no_total_only_the_output_is_kept_and_the_input_is_unclassified_and_partial() {
        let home = tempfile::tempdir().unwrap();
        write(
            home.path(),
            "anthropic.settings.json",
            r#"{"model":"custom:claude-opus-4.5[1m]","providerLock":"anthropic","providerLockTimestamp":"2026-01-02T03:04:05Z","tokenUsage":{"inputTokens":100,"outputTokens":20,"cacheCreationTokens":5,"cacheReadTokens":7,"thinkingTokens":3}}"#,
        );
        write(
            home.path(),
            "openai.settings.json",
            r#"{"model":"gpt-5.1","providerLock":"openai","providerLockTimestamp":"2026-01-02T03:04:05Z","tokenUsage":{"inputTokens":100,"outputTokens":20,"cacheCreationTokens":0,"cacheReadTokens":40,"thinkingTokens":0}}"#,
        );
        let records = records_of(home.path());
        assert_eq!(records.len(), 2);
        for record in &records {
            assert_eq!(record.tally.cache_read, 0);
            assert_eq!(record.tally.cache_write, 0);
            assert_eq!(record.tally.input, 0);
            assert_eq!(record.unclassified_tokens, 100);
            assert!(record.is_partial);
            assert!(record.is_aggregate);
        }
        let anthropic = by_session(&records, "anthropic");
        assert_eq!(anthropic.model, "claude-opus-4.5");
        assert_eq!(anthropic.tally, TokenTally { output: 20, ..TokenTally::default() });
    }

    #[test]
    fn thinking_without_a_cache_marks_the_record_partial() {
        let home = tempfile::tempdir().unwrap();
        write(
            home.path(),
            "thinking.settings.json",
            r#"{"model":"m","providerLock":"anthropic","providerLockTimestamp":"2026-01-02T03:04:05Z","tokenUsage":{"inputTokens":100,"outputTokens":20,"cacheCreationTokens":0,"cacheReadTokens":0,"thinkingTokens":3}}"#,
        );
        write(
            home.path(),
            "plain.settings.json",
            r#"{"model":"m","providerLock":"anthropic","providerLockTimestamp":"2026-01-02T03:04:05Z","tokenUsage":{"inputTokens":100,"outputTokens":20,"cacheCreationTokens":0,"cacheReadTokens":0,"thinkingTokens":0}}"#,
        );
        let records = records_of(home.path());
        let thinking = by_session(&records, "thinking");
        assert_eq!(thinking.tally, TokenTally::new(100, 0, 0, 20));
        assert!(thinking.is_partial);
        let plain = by_session(&records, "plain");
        assert_eq!(plain.tally, TokenTally::new(100, 0, 0, 20));
        assert!(!plain.is_partial);
    }

    #[test]
    fn a_total_settles_inclusion_disjointness_and_the_unresolvable_case() {
        let home = tempfile::tempdir().unwrap();
        write(
            home.path(),
            "included.settings.json",
            r#"{"model":"m","providerLock":"openai","providerLockTimestamp":"2026-01-02T03:04:05Z","tokenUsage":{"inputTokens":100,"outputTokens":20,"cacheReadTokens":40,"totalTokens":120}}"#,
        );
        write(
            home.path(),
            "disjoint.settings.json",
            r#"{"model":"m","providerLock":"openai","providerLockTimestamp":"2026-01-02T03:04:05Z","tokenUsage":{"inputTokens":100,"outputTokens":20,"cacheReadTokens":40,"totalTokens":160}}"#,
        );
        write(
            home.path(),
            "unclear.settings.json",
            r#"{"model":"m","providerLock":"openai","providerLockTimestamp":"2026-01-02T03:04:05Z","tokenUsage":{"inputTokens":100,"outputTokens":20,"cacheReadTokens":40,"totalTokens":999}}"#,
        );
        let records = records_of(home.path());
        let included = by_session(&records, "included");
        assert_eq!(included.tally, TokenTally::new(60, 0, 40, 20));
        assert!(!included.is_partial);
        let disjoint = by_session(&records, "disjoint");
        assert_eq!(disjoint.tally, TokenTally::new(100, 0, 40, 20));
        let unclear = by_session(&records, "unclear");
        assert_eq!(unclear.tally, TokenTally::default());
        assert_eq!(unclear.unclassified_tokens, 999);
        assert!(!unclear.is_partial);
    }

    #[test]
    fn a_session_with_usage_but_no_time_marks_the_readable_siblings_partial() {
        let home = tempfile::tempdir().unwrap();
        write(
            home.path(),
            "readable.settings.json",
            r#"{"model":"m","providerLock":"anthropic","providerLockTimestamp":"2026-01-02T03:04:05Z","tokenUsage":{"inputTokens":10,"outputTokens":1,"cacheCreationTokens":0,"cacheReadTokens":0,"thinkingTokens":0}}"#,
        );
        write(
            home.path(),
            "undated.settings.json",
            r#"{"model":"m","providerLock":"anthropic","tokenUsage":{"inputTokens":99,"outputTokens":9,"cacheCreationTokens":0,"cacheReadTokens":0,"thinkingTokens":0}}"#,
        );
        let records = records_of(home.path());
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].session_id.as_deref(), Some("readable"));
        assert!(records[0].is_partial);
    }

    #[test]
    fn the_model_falls_back_to_the_transcript_and_then_to_a_valueless_provider_placeholder() {
        let home = tempfile::tempdir().unwrap();
        write(
            home.path(),
            "with-transcript.settings.json",
            r#"{"model":"","providerLock":"openai","providerLockTimestamp":"2026-01-02T03:04:05Z","tokenUsage":{"inputTokens":10,"outputTokens":1,"cacheCreationTokens":0,"cacheReadTokens":0,"thinkingTokens":0}}"#,
        );
        write(
            home.path(),
            "with-transcript.jsonl",
            r#"{"type":"message","message":{"role":"assistant"},"timestamp":"2026-01-02T03:04:00Z","content":"<system-reminder>Model: gpt-5.1-codex</system-reminder>"}"#,
        );
        write(
            home.path(),
            "defaulted.settings.json",
            r#"{"providerLock":"anthropic","providerLockTimestamp":"2026-01-02T03:04:05Z","tokenUsage":{"inputTokens":10,"outputTokens":1,"cacheCreationTokens":0,"cacheReadTokens":0,"thinkingTokens":0}}"#,
        );
        let records = records_of(home.path());
        assert!(records.iter().any(|r| r.session_id.as_deref() == Some("with-transcript") && r.model == "gpt-5.1-codex"));
        assert!(records.iter().any(|r| r.session_id.as_deref() == Some("defaulted") && r.model == "claude-unknown"));
        assert!(records.iter().all(|r| !r.is_partial));
    }

    #[test]
    fn a_session_is_one_aggregate_record_priced_once_and_a_missing_store_is_empty() {
        let home = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let empty = read_agent(SpendAgent::Droid, &crate::spend::transcripts::Sources::new(home.path()), cache.path(), &Calendar::utc(2), &prices());
        assert!(empty.days.is_empty());
        write(
            home.path(),
            "one.settings.json",
            r#"{"model":"gpt-5.1","providerLock":"openai","providerLockTimestamp":"2026-01-02T03:04:05Z","tokenUsage":{"inputTokens":100,"outputTokens":20,"totalTokens":120}}"#,
        );
        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::Droid, &crate::spend::transcripts::Sources::new(home.path()), cache.path(), &Calendar::utc(2), &prices());
        assert_eq!(ledger.days.iter().map(|d| d.tokens).sum::<i64>(), 120);
    }

    #[test]
    fn the_model_normalizer_strips_only_the_custom_prefix_and_brackets() {
        assert_eq!(normalize(Some("custom:claude-opus-4.5[1m]".into())).as_deref(), Some("claude-opus-4.5"));
        assert_eq!(normalize(Some("  ".into())), None);
        assert_eq!(provider_default(Some("OpenAI")).as_deref(), Some("gpt-unknown"));
    }
}
