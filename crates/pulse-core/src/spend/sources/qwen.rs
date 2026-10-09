// Ported from upstream Sources/Pulse/Usage/Readers/QwenSessionReader.swift
// and Sources/Pulse/Usage/Readers/SessionLogPaths.swift (qwen).
//! Qwen Code's chat transcripts.
//!
//! `~/.qwen/projects/<projectPath>/chats/*.jsonl`, one line per event; only an `assistant` line
//! with a `usageMetadata` object carries a count.
//!
//! **The field set is Gemini's documented `usageMetadata`, which Qwen normalizes every backend
//! into.** `promptTokenCount` includes the cached content and `totalTokenCount` is
//! `prompt + candidates + tool + thoughts`. So the fresh input is the prompt minus the cache read,
//! the two are not disjoint, and reasoning (`thoughtsTokenCount`) is billed as output, added once.
//!
//! A declared total is a check when present: it can prove the cache read sits inside the prompt
//! (subtract) or beside it (disjoint). When neither identity holds, the total is carried as
//! unclassified rather than split on a guess.
//!
//! Record identity prefers the line's own message id. Lines without one are keyed by the file
//! fragment's content digest plus their position, so two fragments of one session never collide,
//! while a byte-identical mirror of one file still folds.
//!
//! Where it lives on Windows: `%USERPROFILE%\.qwen\projects`. Unverified on a real PC.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use super::SpendSource;
use crate::spend::logio;
use crate::spend::record::AgentUsageRecord;
use crate::spend::tally::TokenTally;
use crate::spend::transcripts::Sources;

pub struct Qwen;

impl SpendSource for Qwen {
    fn raw(&self) -> &'static str {
        "qwen"
    }
    fn source_id(&self) -> &'static str {
        "qwen"
    }
    fn display_name(&self) -> &'static str {
        "Qwen Code"
    }
    fn icon(&self) -> Option<&'static str> {
        Some("qwen")
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        // WINDOWS-PATH: unverified (`%USERPROFILE%\.qwen` is the macOS layout carried over)
        vec![sources.home.join(".qwen").join("projects")]
    }
    fn records(&self, roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        let mut records = Vec::new();
        let mut incomplete = false;

        for file in logio::files(roots, &["jsonl"], &[], &[]) {
            let Some(fragment) = digest(&file) else { continue };
            let project = project_segment(&file);
            let file_stem = file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let fallback_id = match &project {
                Some(project) => format!("{project}-{file_stem}"),
                None => file_stem,
            };

            let mut emitted = 0usize;
            for row in logio::json_lines(&file) {
                if row.get("type").and_then(Value::as_str) != Some("assistant") {
                    continue;
                }
                let Some(metadata) = row.get("usageMetadata").and_then(Value::as_object) else { continue };
                let Some((tally, unclassified)) = decode(metadata) else { continue };
                if tally.total() <= 0 && unclassified <= 0 {
                    continue;
                }
                let model = logio::text(row.get("model"));
                let timestamp = logio::timestamp(row.get("timestamp"), false);
                let (Some(model), Some(timestamp)) = (model, timestamp) else {
                    // Real usage with no model or no locatable time.
                    incomplete = true;
                    continue;
                };

                let session_id = logio::text(row.get("sessionId")).unwrap_or_else(|| fallback_id.clone());
                let message_id = logio::text(row.get("id")).or_else(|| logio::text(row.get("messageId")));
                let identity = match message_id {
                    Some(id) => format!("{session_id}:{id}"),
                    None => format!("{session_id}:{fragment}:{emitted}"),
                };

                let mut record = AgentUsageRecord::new(timestamp, &model, tally).session(&session_id).unclassified(unclassified);
                record.project = project.clone();
                record.deduplication_id = Some(format!("qwen:{identity}"));
                records.push(record);
                emitted += 1;
            }
        }
        if incomplete {
            for record in &mut records {
                record.is_partial = true;
            }
        }
        records
    }
}

/// The SHA-256 of a file's bytes as hex; None when it cannot be read.
fn digest(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    Some(Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect())
}

/// The `<projectPath>` segment between `projects` and `chats`.
fn project_segment(path: &Path) -> Option<String> {
    let parts: Vec<String> = path.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    let index = parts.iter().rposition(|p| p == "projects")?;
    parts.get(index + 1).filter(|p| !p.is_empty()).cloned()
}

/// Decodes one Gemini-shaped `usageMetadata` object: the tally and the unclassified remainder.
/// None when nothing countable was reported.
fn decode(metadata: &Map<String, Value>) -> Option<(TokenTally, i64)> {
    let prompt = logio::count(metadata.get("promptTokenCount"));
    let candidates = logio::count(metadata.get("candidatesTokenCount"));
    let thoughts = logio::count(metadata.get("thoughtsTokenCount"));
    let cached = logio::count(metadata.get("cachedContentTokenCount"));
    let total = logio::count(metadata.get("totalTokenCount"))
        .or_else(|| logio::count(metadata.get("total")))
        .or_else(|| logio::count(metadata.get("total_tokens")));

    if prompt.is_none() && candidates.is_none() && thoughts.is_none() && cached.is_none() && total.is_none() {
        return None;
    }

    let prompt = prompt.unwrap_or(0);
    let cached = cached.unwrap_or(0);
    // Gemini bills thinking as output; the output bucket holds it once.
    let output = candidates.unwrap_or(0) + thoughts.unwrap_or(0);

    let Some(total) = total else {
        // No total: the documented semantics (the prompt includes the cached content) decide the buckets.
        return Some((TokenTally::new((prompt - cached).max(0), 0, cached, output), 0));
    };
    let included = prompt + output;
    let disjoint = prompt + cached + output;
    if cached == 0 {
        return Some(if total == included {
            (TokenTally::new(prompt, 0, 0, output), 0)
        } else {
            (TokenTally::default(), total)
        });
    }
    if total == included && total != disjoint {
        // Proven: the cache read is inside the prompt.
        return Some((TokenTally::new((prompt - cached).max(0), 0, cached, output), 0));
    }
    if total == disjoint && total != included {
        // Proven: the cache read sits beside the prompt.
        return Some((TokenTally::new(prompt, 0, cached, output), 0));
    }
    // A total that matches neither identity: keep the total, name no kind.
    Some((TokenTally::default(), total))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::spend::calendar::Calendar;
    use crate::spend::prices::{ModelPrice, PriceTable};
    use crate::spend::sources::{read_agent, SpendAgent};

    fn prices() -> PriceTable {
        HashMap::from([("qwen3-coder".to_string(), ModelPrice::new(1.0, 10.0, Some(0.1), Some(1.0), Some("Qwen3 Coder")))])
    }

    fn write(home: &Path, relative: &str, text: &str) {
        let path = home.join(".qwen/projects").join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn records_of(home: &Path) -> Vec<AgentUsageRecord> {
        Qwen.records(&[home.join(".qwen/projects")])
    }

    fn ledger_tokens(home: &Path) -> i64 {
        let cache = tempfile::tempdir().unwrap();
        read_agent(SpendAgent::Qwen, &Sources::new(home), cache.path(), &Calendar::utc(2), &prices()).days.iter().map(|d| d.tokens).sum()
    }

    #[test]
    fn prompt_is_cache_inclusive_and_thoughts_fold_into_output() {
        let home = tempfile::tempdir().unwrap();
        write(
            home.path(),
            "myproject/chats/session.jsonl",
            r#"{"type":"user","timestamp":"2026-01-02T03:00:00Z","sessionId":"q-1","usageMetadata":{"promptTokenCount":1}}
{"type":"assistant","timestamp":"2026-01-02T03:04:05Z","sessionId":"q-1","model":"qwen3-coder","usageMetadata":{"promptTokenCount":100,"candidatesTokenCount":20,"thoughtsTokenCount":5,"cachedContentTokenCount":30}}
{"type":"assistant","model":"qwen3-coder","usageMetadata":{"promptTokenCount":7}}"#,
        );
        let records = records_of(home.path());
        assert_eq!(records.len(), 1);
        let record = &records[0];
        assert_eq!(record.tally, TokenTally::new(70, 0, 30, 25));
        assert_eq!(record.project.as_deref(), Some("myproject"));
        assert_eq!(record.session_id.as_deref(), Some("q-1"));
        assert!(record.deduplication_id.as_deref().is_some_and(|d| d.starts_with("qwen:q-1:")));
    }

    #[test]
    fn a_total_equal_to_prompt_plus_output_proves_the_cache_is_inside_the_prompt() {
        let home = tempfile::tempdir().unwrap();
        write(
            home.path(),
            "p/chats/a.jsonl",
            r#"{"type":"assistant","id":"m-1","timestamp":"2026-01-02T03:04:05Z","sessionId":"q-inc","model":"qwen3-coder","usageMetadata":{"promptTokenCount":100,"candidatesTokenCount":10,"cachedContentTokenCount":40,"totalTokenCount":110}}"#,
        );
        let record = records_of(home.path()).into_iter().next().unwrap();
        assert_eq!(record.tally, TokenTally::new(60, 0, 40, 10));
        assert_eq!(record.tally.total(), 110);
    }

    #[test]
    fn a_total_equal_to_the_disjoint_sum_proves_the_cache_is_beside_the_prompt() {
        let home = tempfile::tempdir().unwrap();
        write(
            home.path(),
            "p/chats/b.jsonl",
            r#"{"type":"assistant","id":"m-2","timestamp":"2026-01-02T03:04:05Z","sessionId":"q-dis","model":"qwen3-coder","usageMetadata":{"promptTokenCount":100,"candidatesTokenCount":10,"cachedContentTokenCount":40,"totalTokenCount":150}}"#,
        );
        let record = records_of(home.path()).into_iter().next().unwrap();
        assert_eq!(record.tally, TokenTally::new(100, 0, 40, 10));
    }

    #[test]
    fn a_total_that_settles_nothing_becomes_unclassified_never_a_guessed_kind() {
        let home = tempfile::tempdir().unwrap();
        write(
            home.path(),
            "p/chats/c.jsonl",
            r#"{"type":"assistant","id":"m-3","timestamp":"2026-01-02T03:04:05Z","sessionId":"q-unk","model":"qwen3-coder","usageMetadata":{"promptTokenCount":100,"candidatesTokenCount":10,"cachedContentTokenCount":40,"totalTokenCount":200}}"#,
        );
        let record = records_of(home.path()).into_iter().next().unwrap();
        assert_eq!(record.tally, TokenTally::default());
        assert_eq!(record.unclassified_tokens, 200);
    }

    #[test]
    fn two_fragments_of_one_session_keep_their_records_with_and_without_ids() {
        let home = tempfile::tempdir().unwrap();
        write(
            home.path(),
            "p/chats/a.jsonl",
            r#"{"type":"assistant","id":"frag-a","timestamp":"2026-01-02T03:04:05Z","sessionId":"q-split","model":"qwen3-coder","usageMetadata":{"promptTokenCount":100,"candidatesTokenCount":10}}"#,
        );
        write(
            home.path(),
            "p/chats/b.jsonl",
            r#"{"type":"assistant","id":"frag-b","timestamp":"2026-01-02T03:05:05Z","sessionId":"q-split","model":"qwen3-coder","usageMetadata":{"promptTokenCount":50,"candidatesTokenCount":5}}"#,
        );
        let with_ids = records_of(home.path());
        assert_eq!(with_ids.len(), 2);
        let ids: std::collections::HashSet<_> = with_ids.iter().filter_map(|r| r.deduplication_id.clone()).collect();
        assert_eq!(ids.len(), 2);
        assert_eq!(ledger_tokens(home.path()), 165);

        // No ids at all: two fragments must not collide on index 0.
        let bare = tempfile::tempdir().unwrap();
        write(
            bare.path(),
            "bare/p/chats/c.jsonl",
            r#"{"type":"assistant","timestamp":"2026-01-02T03:04:05Z","sessionId":"q-bare","model":"qwen3-coder","usageMetadata":{"promptTokenCount":7,"candidatesTokenCount":1}}"#,
        );
        write(
            bare.path(),
            "bare/p/chats/d.jsonl",
            r#"{"type":"assistant","timestamp":"2026-01-02T03:05:05Z","sessionId":"q-bare","model":"qwen3-coder","usageMetadata":{"promptTokenCount":9,"candidatesTokenCount":2}}"#,
        );
        assert_eq!(records_of(bare.path()).len(), 2);
        assert_eq!(ledger_tokens(bare.path()), 19);
    }

    #[test]
    fn a_byte_identical_fragment_mirror_folds_to_one() {
        let home = tempfile::tempdir().unwrap();
        let line = r#"{"type":"assistant","timestamp":"2026-01-02T03:04:05Z","sessionId":"q-mirror","model":"qwen3-coder","usageMetadata":{"promptTokenCount":100,"candidatesTokenCount":10}}"#;
        write(home.path(), "one/p/chats/a.jsonl", line);
        write(home.path(), "two/p/chats/a.jsonl", line);
        let records = records_of(home.path());
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].deduplication_id, records[1].deduplication_id);
        assert_eq!(ledger_tokens(home.path()), 110);
    }

    #[test]
    fn a_missing_store_is_empty() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(ledger_tokens(home.path()), 0);
    }
}
