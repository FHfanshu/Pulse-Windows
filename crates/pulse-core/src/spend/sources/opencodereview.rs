// Ported from upstream Sources/Pulse/Usage/Readers/OpenCodeReviewReader.swift.
//! OpenCodeReview's session logs: one JSONL per session under `sessions/<encoded-repo>/`. A
//! `session_start` line names the working directory and default model; each `llm_response` carries
//! the response's own usage.
//!
//! **The cache relation is not assumed.** The product's own resolver treats the cache counts as
//! *included* in `prompt_tokens` under one provider's semantics and *exclusive* under another, and
//! the persisted usage writes no total and no record of which path the cache came from. The store's
//! own total, where a variant carries one, is what settles it:
//!
//! - `total == prompt + completion + cacheRead + cacheWrite`: the four kinds are disjoint.
//! - `total == prompt + completion` (and not the above): the cache is inside the prompt, so it is
//!   taken out of input.
//! - any other total: unclassified, with an empty tally, rather than a split that could double count.
//! - no total and a positive cache count: the relation is unproven, so no record is emitted, and the
//!   readable remainder is marked partial.
//! - no total and no cache count: nothing can overlap; input and completion are counted as reported.
//!
//! A replayed `uuid` folds once. A line with no `uuid` takes the file's content digest and its line
//! position, so a byte-identical mirror folds while two different files that restart at line zero
//! are both kept.
//!
//! Where it lives on Windows: `%USERPROFILE%\.opencodereview\sessions` (unverified).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::SpendSource;
use crate::spend::editorlog::{self, Object};
use crate::spend::logio;
use crate::spend::record::AgentUsageRecord;
use crate::spend::tally::TokenTally;
use crate::spend::transcripts::Sources;

pub struct OpenCodeReview;

impl SpendSource for OpenCodeReview {
    fn raw(&self) -> &'static str {
        "openCodeReview"
    }
    fn source_id(&self) -> &'static str {
        "opencodereview"
    }
    fn display_name(&self) -> &'static str {
        "OpenCodeReview"
    }
    fn icon(&self) -> Option<&'static str> {
        Some("opencode")
    }
    fn price_vendor(&self) -> Option<&'static str> {
        Some("opencode-go")
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        // WINDOWS-PATH: unverified
        vec![sources.home.join(".opencodereview").join("sessions")]
    }
    fn records(&self, roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        let mut files = logio::files(roots, &["jsonl"], &[], &[]);
        files.sort();

        let mut seen: HashSet<String> = HashSet::new();
        let mut records = Vec::new();
        let mut skipped_unprovable = false;
        for file in &files {
            let (parsed, skipped) = parse(file);
            skipped_unprovable |= skipped;
            for record in parsed {
                if let Some(id) = &record.deduplication_id {
                    if !seen.insert(id.clone()) {
                        continue;
                    }
                }
                records.push(record);
            }
        }

        // A recognized usage was dropped because its cache relation could not be proven, so what
        // remains is a confirmed subset, not the whole.
        if skipped_unprovable {
            for record in &mut records {
                record.is_partial = true;
            }
        }
        records
    }
}

/// One file's records, and whether any usage was dropped as unprovable.
fn parse(file: &Path) -> (Vec<AgentUsageRecord>, bool) {
    let Ok(bytes) = std::fs::read(file) else { return (Vec::new(), false) };
    // The file's own digest: a deterministic fragment identity, shared by byte-identical mirrors.
    let fragment = editorlog::digest(&bytes);
    let stem = file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let rows: Vec<Object> = logio::json_lines(file).filter_map(|v| v.as_object().cloned()).collect();

    let mut session_from_start: Option<String> = None;
    let mut cwd: Option<String> = None;
    let mut start_model: Option<String> = None;
    for row in rows.iter().filter(|r| logio::text(r.get("type")).as_deref() == Some("session_start")) {
        session_from_start = logio::text(row.get("sessionId")).or(session_from_start);
        cwd = logio::text(row.get("cwd")).or(cwd);
        start_model = editorlog::model_id(logio::text(row.get("model"))).or(start_model);
    }

    let mut records = Vec::new();
    let mut skipped = false;
    for (index, row) in rows.iter().enumerate() {
        if logio::text(row.get("type")).as_deref() != Some("llm_response") {
            continue;
        }
        let Some(end) = logio::timestamp(row.get("timestamp"), true) else { continue };
        let Some(usage) = row.get("usage").and_then(|u| u.as_object()) else { continue };

        let Some(parts) = reduce(usage) else {
            skipped = true;
            continue;
        };
        if parts.tally.total() + parts.unclassified <= 0 {
            continue;
        }

        // The request starts at its end minus its positive duration.
        let duration = logio::count(row.get("duration_ms")).unwrap_or(0);
        let start = if duration > 0 { end - chrono::Duration::milliseconds(duration) } else { end };

        let session = logio::text(row.get("sessionId")).or_else(|| session_from_start.clone()).unwrap_or_else(|| stem.clone());
        let Some(model) = editorlog::model_id(logio::text(row.get("model"))).or_else(|| start_model.clone()) else { continue };

        // The record's own uuid is a real identity; without one, the file digest and the line keep
        // two fragments apart while still folding a byte-identical mirror.
        let identity = match logio::text(row.get("uuid")) {
            Some(uuid) => format!("opencodereview:{session}:uuid:{uuid}"),
            None => format!("opencodereview:{session}:fragment:{fragment}:line:{index}"),
        };

        let mut record = AgentUsageRecord::new(start, &model, parts.tally).session(&session).unclassified(parts.unclassified);
        record.project = cwd.clone();
        record.deduplication_id = Some(identity);
        records.push(record);
    }
    (records, skipped)
}

/// One usage object reduced to buckets. `None` when the cache relation cannot be proven.
fn reduce(usage: &Object) -> Option<editorlog::UsageParts> {
    let prompt = editorlog::first_count(usage, &["prompt_tokens", "promptTokens"]);
    let completion = editorlog::first_count(usage, &["completion_tokens", "completionTokens"]);
    let cache_read = editorlog::first_count(usage, &["cache_read_tokens", "cacheReadTokens"]);
    let cache_write = editorlog::first_count(usage, &["cache_write_tokens", "cacheWriteTokens"]);
    let total = editorlog::first_count(usage, &["total_tokens", "totalTokens", "total"]);

    let names_a_kind = prompt.is_some() || completion.is_some() || cache_read.is_some() || cache_write.is_some();
    if !names_a_kind {
        if let Some(total) = total.filter(|t| *t > 0) {
            return Some(editorlog::UsageParts { tally: TokenTally::default(), unclassified: total });
        }
    }

    let (prompt, completion) = (prompt.unwrap_or(0), completion.unwrap_or(0));
    let (cache_read, cache_write) = (cache_read.unwrap_or(0), cache_write.unwrap_or(0));
    let disjoint = prompt + completion + cache_read + cache_write;

    if let Some(total) = total.filter(|t| *t > 0) {
        if total == disjoint {
            return Some(editorlog::UsageParts { tally: TokenTally::new(prompt, cache_write, cache_read, completion), unclassified: 0 });
        }
        if total == prompt + completion {
            // The prompt already contains the cache counts.
            let fresh = (prompt - cache_read - cache_write).max(0);
            return Some(editorlog::UsageParts { tally: TokenTally::new(fresh, cache_write, cache_read, completion), unclassified: 0 });
        }
        // Neither shape: keep the total whole rather than price a split that could double count.
        return Some(editorlog::UsageParts { tally: TokenTally::default(), unclassified: total });
    }

    // No total to settle the overlap. With no cache there is nothing that could overlap.
    if cache_read == 0 && cache_write == 0 {
        return Some(editorlog::UsageParts { tally: TokenTally::new(prompt, 0, 0, completion), unclassified: 0 });
    }
    None
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

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

    fn session(home: &Path, name: &str) -> PathBuf {
        home.join(".opencodereview").join("sessions").join("repo").join(format!("{name}.jsonl"))
    }

    fn start(model: &str) -> Value {
        json!({"type":"session_start","sessionId":"s1","cwd":"/work/pulse","model":format!("vendor/{model}")})
    }

    #[test]
    fn the_store_total_settles_whether_the_cache_is_inside_the_prompt() {
        let home = tempfile::tempdir().unwrap();
        write(
            &session(home.path(), "s1"),
            &[
                start("model-x"),
                // total = prompt + completion: the cache is inside the prompt and comes out.
                json!({"type":"llm_response","uuid":"a","timestamp":MIDNIGHT*1000,"usage":{"prompt_tokens":100,"completion_tokens":10,"cache_read_tokens":30,"total_tokens":110}}),
                // total = every kind: disjoint, counted as reported.
                json!({"type":"llm_response","uuid":"b","timestamp":(MIDNIGHT+1)*1000,"usage":{"prompt_tokens":100,"completion_tokens":10,"cache_read_tokens":30,"cache_write_tokens":5,"total_tokens":145}}),
                // A total that fits neither: unclassified, empty tally.
                json!({"type":"llm_response","uuid":"c","timestamp":(MIDNIGHT+2)*1000,"usage":{"prompt_tokens":100,"completion_tokens":10,"total_tokens":999}}),
            ],
        );
        let (records, _) = parse(&session(home.path(), "s1"));
        assert_eq!(records.len(), 3);
        assert_eq!(records[0].tally, TokenTally::new(70, 0, 30, 10));
        assert_eq!(records[1].tally, TokenTally::new(100, 5, 30, 10));
        assert_eq!(records[2].tally, TokenTally::default());
        assert_eq!(records[2].unclassified_tokens, 999);
        assert_eq!(records[0].model, "model-x");
        assert_eq!(records[0].project.as_deref(), Some("/work/pulse"));
    }

    #[test]
    fn a_cache_with_no_total_is_unprovable_and_the_readable_rest_is_partial() {
        let home = tempfile::tempdir().unwrap();
        write(
            &session(home.path(), "s1"),
            &[
                start("model-x"),
                json!({"type":"llm_response","uuid":"p","timestamp":MIDNIGHT*1000,"usage":{"prompt_tokens":100,"completion_tokens":10,"cache_read_tokens":30}}),
                json!({"type":"llm_response","uuid":"q","timestamp":(MIDNIGHT+1)*1000,"usage":{"prompt_tokens":7,"completion_tokens":3}}),
            ],
        );
        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::OpenCodeReview, &Sources::new(home.path()), cache.path(), &Calendar::utc(2), &PriceTable::new());
        // Only the provable request is counted, and the ledger says it is partial.
        assert_eq!(ledger.days.iter().map(|d| d.tokens).sum::<i64>(), 10);
        assert!(ledger.has_partial_counts);
    }

    #[test]
    fn a_duration_moves_the_start_back_and_a_mirrored_file_folds_while_two_fragments_do_not() {
        let home = tempfile::tempdir().unwrap();
        let lines = [
            start("model-x"),
            json!({"type":"llm_response","uuid":"u1","timestamp":MIDNIGHT*1000,"duration_ms":4000,"usage":{"prompt_tokens":5,"completion_tokens":1}}),
            // No uuid: identified by its file's digest and line.
            json!({"type":"llm_response","timestamp":MIDNIGHT*1000,"usage":{"prompt_tokens":5,"completion_tokens":1}}),
        ];
        write(&session(home.path(), "s1"), &lines);
        // A byte-identical mirror folds; a different file with the same session restarts at line zero and is kept.
        write(&session(home.path(), "s1-mirror"), &lines);
        write(&session(home.path(), "s1-other"), &[start("model-x"), json!({"type":"llm_response","timestamp":MIDNIGHT*1000,"usage":{"prompt_tokens":9,"completion_tokens":1}})]);
        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::OpenCodeReview, &Sources::new(home.path()), cache.path(), &Calendar::utc(2), &PriceTable::new());
        // u1 once (6), the digest line of s1 once (6), and s1-other's line (10).
        assert_eq!(ledger.days.iter().map(|d| d.tokens).sum::<i64>(), 6 + 6 + 10);
        let (first, _) = parse(&session(home.path(), "s1"));
        assert_eq!(first[0].timestamp.timestamp(), MIDNIGHT - 4);
    }

    #[test]
    fn a_store_that_is_not_there_is_an_empty_ledger() {
        let home = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        assert!(read_agent(SpendAgent::OpenCodeReview, &Sources::new(home.path()), cache.path(), &Calendar::utc(2), &PriceTable::new()).days.is_empty());
    }
}
