// Ported from upstream Sources/Pulse/Usage/Readers/CapturedTraeReader.swift.
//! Trae's usage cache: a raw JSON **array** dumped from the Trae usage API, `trae-cache/sessions/*.json`.
//!
//! There is no native local file; the array is an export that needs an authenticated sync. Pulse
//! reads the dumped array, whatever the file is called, as soon as the root parses as an array.
//!
//! `model_name` is empty when the system picked a model per turn (Auto mode); the row is then named
//! `trae-<mode>` and the true per-turn model is not recoverable. `extra_info` carries the four real
//! counters, and anything else in it is left alone.
//!
//! **Equal rows are not merged, but overlapping pages are.** The dump has no per-row id, so two rows
//! for one session in the same second inside one page stay two rows. Across pages the same row is
//! reconciled by content multiset, and an unconfirmed increment in a shared region is marked partial.
//! A byte-identical page is folded outright.
//!
//! Where it lives on Windows: `TOKSCALE_CONFIG_DIR` when set, else `%USERPROFILE%\.config\tokscale`,
//! then `\trae-cache\sessions`. `WINDOWS-PATH: unverified`.

use std::path::PathBuf;

use serde_json::Value;

use super::{push_unique, reconcile, replay_distinct, SpendSource};
use crate::spend::logio;
use crate::spend::record::AgentUsageRecord;
use crate::spend::tally::TokenTally;
use crate::spend::transcripts::Sources;

pub struct Trae;

impl SpendSource for Trae {
    fn raw(&self) -> &'static str {
        "trae"
    }
    fn source_id(&self) -> &'static str {
        "trae"
    }
    fn display_name(&self) -> &'static str {
        "Trae"
    }
    fn icon(&self) -> Option<&'static str> {
        Some("trae")
    }
    fn env_vars(&self) -> &'static [&'static str] {
        &["TOKSCALE_CONFIG_DIR", "APPDATA"]
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        let mut roots = Vec::new();
        // WINDOWS-PATH: unverified
        let cache = sources.var("TOKSCALE_CONFIG_DIR").unwrap_or_else(|| sources.home.join(".config").join("tokscale"));
        push_unique(&mut roots, cache.join("trae-cache").join("sessions"));
        push_unique(&mut roots, sources.app_data().join("Pulse").join("UsageImports").join("trae"));
        roots
    }
    fn records(&self, roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        let files = replay_distinct(&logio::files(roots, &["json"], &[], &[]));
        let per_file: Vec<Vec<AgentUsageRecord>> = files.iter().map(|file| records_of(file)).collect();
        let (records, overlapped) = reconcile(&per_file, signature);
        if overlapped {
            records.into_iter().map(|mut record| {
                record.is_partial = true;
                record
            }).collect()
        } else {
            records
        }
    }
    fn requires_usage_export(&self) -> bool {
        true
    }
}

/// A row's content identity, used only to reconcile overlapping pages.
fn signature(record: &AgentUsageRecord) -> String {
    format!(
        "{}:{}:{}:{}:{}:{}:{}",
        record.session_id.as_deref().unwrap_or(""),
        record.timestamp.timestamp_micros(),
        record.model,
        record.tally.input,
        record.tally.cache_write,
        record.tally.cache_read,
        record.tally.output,
    )
}

/// One page's rows. A file whose root is not an array is not this format and yields nothing.
fn records_of(file: &std::path::Path) -> Vec<AgentUsageRecord> {
    let Some(Value::Array(sessions)) = logio::json(file) else { return Vec::new() };
    sessions.iter().filter_map(record_of).collect()
}

fn record_of(session: &Value) -> Option<AgentUsageRecord> {
    let session_id = logio::text(session.get("session_id"))?;
    // `usage_time` is epoch seconds by this schema; a non-positive value is not a usable time.
    let at = logio::timestamp(session.get("usage_time"), false).filter(|at| at.timestamp() > 0)?;

    let extra = session.get("extra_info").filter(|v| v.is_object());
    let field = |name: &str| extra.and_then(|e| logio::count(e.get(name))).unwrap_or(0);
    let tally = TokenTally::new(field("input_token"), field("cache_write_token"), field("cache_read_token"), field("output_token"));
    // All-zero totals assert no usage.
    if tally.total() <= 0 {
        return None;
    }

    let mut record = AgentUsageRecord::new(at, &normalized(&model_name(session)), tally);
    // The session id names the session; it is not a row identity, so it never becomes a dedup id.
    record.session_id = Some(session_id);
    Some(record)
}

fn model_name(session: &Value) -> String {
    if let Some(name) = logio::text(session.get("model_name")) {
        return name;
    }
    if let Some(mode) = logio::text(session.get("mode")) {
        return format!("trae-{mode}");
    }
    "trae-unknown".to_string()
}

/// A small, fixed display-name table. An unrecognised name passes through unchanged, so it is
/// counted and reported as unpriced rather than disguised.
fn normalized(name: &str) -> String {
    let trimmed = name.trim();
    let lower = trimmed.to_lowercase();

    if lower.starts_with("gpt-5") {
        return spaced(&lower);
    }
    if lower.starts_with("gemini 3.1") || lower.starts_with("gemini-3.1") {
        return spaced(&lower);
    }
    if lower == "glm 5.1" || lower == "glm-5.1" {
        return "glm-5.1".to_string();
    }
    // Anthropic spells the version with dashes, so `Claude Sonnet 4.5` becomes `claude-sonnet-4-5`.
    if lower.starts_with("claude sonnet 4.5") {
        return spaced(&lower.replace("4.5", "4-5"));
    }
    if lower.starts_with("claude sonnet 4.6") {
        return spaced(&lower.replace("4.6", "4-6"));
    }
    trimmed.to_string()
}

fn spaced(value: &str) -> String {
    value.split([' ', '_']).filter(|part| !part.is_empty()).collect::<Vec<_>>().join("-")
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use chrono::Utc;
    use serde_json::json;

    use super::*;
    use crate::spend::calendar::Calendar;
    use crate::spend::prices::PriceTable;
    use crate::spend::sources::{read_agent, SpendAgent};

    fn row(session: &str, seconds: i64, model: &str, input: i64, output: i64) -> Value {
        json!({"session_id": session, "usage_time": seconds, "model_name": model,
               "extra_info": {"input_token": input, "cache_write_token": 0, "cache_read_token": 0, "output_token": output}})
    }

    fn write(path: PathBuf, rows: &[Value]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, Value::Array(rows.to_vec()).to_string()).unwrap();
    }

    #[test]
    fn rows_read_their_four_counters_and_auto_mode_is_named_by_its_mode() {
        let home = tempfile::tempdir().unwrap();
        let now = Utc::now().timestamp();
        let folder = home.path().join(".config").join("tokscale").join("trae-cache").join("sessions");
        write(
            folder.join("page.json"),
            &[
                row("s1", now, "Claude Sonnet 4.5", 10, 5),
                json!({"session_id": "s2", "usage_time": now, "mode": "auto",
                       "extra_info": {"input_token": 7, "cache_write_token": 1, "cache_read_token": 2, "output_token": 3}}),
                // No time, and an all-zero row: each skipped.
                json!({"session_id": "s3", "extra_info": {"input_token": 4, "output_token": 4}}),
                row("s4", now, "gpt 5.1", 0, 0),
            ],
        );
        let cache = tempfile::tempdir().unwrap();
        let prices: PriceTable = HashMap::new();
        let ledger = read_agent(SpendAgent::Trae, &Sources::new(home.path()), cache.path(), &Calendar::utc(2), &prices);
        let total: i64 = ledger.days.iter().map(|d| d.tokens).sum();
        assert_eq!(total, 15 + 13);
        assert_eq!(normalized("Claude Sonnet 4.5"), "claude-sonnet-4-5");
        assert_eq!(normalized("gpt-5 mini"), "gpt-5-mini");
        assert_eq!(normalized("GPT 5 mini"), "GPT 5 mini");
        assert_eq!(model_name(&json!({"mode": "auto"})), "trae-auto");
    }

    #[test]
    fn a_page_repeated_byte_for_byte_counts_once_and_an_overlap_is_reconciled() {
        let home = tempfile::tempdir().unwrap();
        let now = Utc::now().timestamp();
        let folder = home.path().join(".config").join("tokscale").join("trae-cache").join("sessions");
        let a = row("s1", now, "m", 10, 0);
        let b = row("s1", now + 60, "m", 20, 0);
        write(folder.join("one.json"), std::slice::from_ref(&a));
        write(folder.join("two.json"), &[a.clone(), b.clone()]);
        write(folder.join("copy-of-one.json"), std::slice::from_ref(&a));
        let cache = tempfile::tempdir().unwrap();
        let prices: PriceTable = HashMap::new();
        let ledger = read_agent(SpendAgent::Trae, &Sources::new(home.path()), cache.path(), &Calendar::utc(2), &prices);
        // {A} and {A, B} give A and B, once each.
        assert_eq!(ledger.days.iter().map(|d| d.tokens).sum::<i64>(), 30);
    }

    #[test]
    fn a_missing_store_is_empty_and_a_non_array_root_yields_nothing() {
        let home = tempfile::tempdir().unwrap();
        let folder = home.path().join(".config").join("tokscale").join("trae-cache").join("sessions");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("object.json"), r#"{"session_id":"x"}"#).unwrap();
        let cache = tempfile::tempdir().unwrap();
        let prices: PriceTable = HashMap::new();
        let ledger = read_agent(SpendAgent::Trae, &Sources::new(home.path()), cache.path(), &Calendar::utc(2), &prices);
        assert!(ledger.days.iter().all(|d| d.tokens == 0));
        let empty = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::Trae, &Sources::new(empty.path()), cache.path(), &Calendar::utc(2), &prices);
        assert!(ledger.days.is_empty());
    }
}
