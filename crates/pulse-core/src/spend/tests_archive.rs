//! Ported from upstream UsageArchiveTests: a tool deleting its old records must not take them out
//! of Token spend or the recaps. Transcripts are built by hand in temporary homes.

use std::collections::HashMap;
use std::path::Path;

use super::archive::TranscriptArchive;
use super::calendar::Calendar;
use super::ledger::Ledger;
use super::prices::{ModelPrice, PriceTable};
use super::read_ledger_with;
use super::transcripts::Sources;
use crate::provider::Provider;

fn prices() -> PriceTable {
    HashMap::from([
        ("claude-test".to_string(), ModelPrice::new(3.0, 15.0, Some(0.3), Some(3.75), Some("Claude Test"))),
        ("gpt-5".to_string(), ModelPrice::new(1.25, 10.0, Some(0.125), None, Some("GPT-5"))),
    ])
}

fn write(lines: &[String], path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, lines.join("\n") + "\n").unwrap();
}

fn read(home: &Path, provider: Provider) -> Ledger {
    let mut ledger =
        read_ledger_with(provider, &Sources::new(home), &home.join("cache"), &Calendar::utc(2), &prices(), chrono::Utc::now())
            .unwrap();
    ledger.sessions.sort_by(|a, b| a.id.cmp(&b.id));
    // When it was read is not what it holds.
    ledger.read_at = None;
    ledger
}

fn user(time: &str, text: &str) -> String {
    format!(r#"{{"type":"user","timestamp":"{time}","cwd":"/work/pulse","message":{{"role":"user","content":"{text}"}}}}"#)
}

fn reply(id: &str, time: &str, input: i64, output: i64) -> String {
    format!(
        r#"{{"type":"assistant","timestamp":"{time}","message":{{"id":"{id}","model":"claude-test","usage":{{"input_tokens":{input},"cache_read_input_tokens":2000,"cache_creation_input_tokens":500,"cache_creation":{{"ephemeral_1h_input_tokens":200}},"output_tokens":{output}}}}}}}"#
    )
}

#[test]
fn a_deleted_claude_code_transcript_keeps_its_figures_and_its_resumed_copy_is_not_counted_again() {
    let home = tempfile::tempdir().unwrap();
    let project = home.path().join(".claude/projects/-work-pulse");
    let original = project.join("a.jsonl");
    let resumed = project.join("b.jsonl");
    let opening = vec![
        user("2026-09-01T10:00:00Z", "Fix the build"),
        reply("m1", "2026-09-01T10:00:40Z", 100, 300),
        user("2026-09-01T10:20:00Z", "And the tests"),
        reply("m2", "2026-09-01T10:21:00Z", 50, 150),
    ];
    write(&opening, &original);
    // Resuming opens a new transcript with a copy of the old history (the same ids at the same
    // times) and goes on the next day.
    let mut continued = opening.clone();
    continued.extend([user("2026-09-02T09:00:00Z", "Ship it"), reply("m3", "2026-09-02T09:00:30Z", 100, 300)]);
    write(&continued, &resumed);

    let before = read(home.path(), Provider::ClaudeCode);
    assert_eq!(before.all_time().tokens, (100 + 2000 + 500 + 300) * 2 + (50 + 2000 + 500 + 150));
    assert!(before.all_time().cost > 0.0);
    assert_eq!(before.sessions.len(), 2);
    assert!(before.slots.iter().any(|s| !s.timings.is_empty()));

    std::fs::remove_file(&original).unwrap();
    let after = read(home.path(), Provider::ClaudeCode);
    // Days, money, quarter-hours with their timings, and both rows.
    assert_eq!(after, before);
    let kept = after.sessions.iter().find(|s| s.id.ends_with("a.jsonl")).unwrap();
    assert_eq!(kept.title.as_deref(), Some("Fix the build"));

    // Read again, and with the copy gone too.
    assert_eq!(read(home.path(), Provider::ClaudeCode), before);
    std::fs::remove_file(&resumed).unwrap();
    let both_gone = read(home.path(), Provider::ClaudeCode);
    assert_eq!(both_gone.days, before.days);
    assert_eq!(both_gone.slots, before.slots);
    assert_eq!(both_gone.sessions.len(), 2);

    // The copy back (from a backup, or a folder that was away for a scan): the kept original
    // still claims its replies.
    write(&continued, &resumed);
    assert_eq!(read(home.path(), Provider::ClaudeCode).days, before.days);

    // A transcript put back where it was is read from the file, not kept as well.
    write(&opening, &original);
    assert_eq!(read(home.path(), Provider::ClaudeCode).days, before.days);
}

fn codex_count(time: &str, input: i64, output: i64, last: (i64, i64)) -> String {
    format!(
        r#"{{"timestamp":"2026-01-02T{time}Z","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":{input},"cached_input_tokens":0,"output_tokens":{output},"total_tokens":{}}},"last_token_usage":{{"input_tokens":{},"cached_input_tokens":0,"output_tokens":{},"total_tokens":{}}}}}}}}}"#,
        input + output,
        last.0,
        last.1,
        last.0 + last.1
    )
}

const CODEX_MODEL: &str = r#"{"timestamp":"2026-01-02T09:00:00Z","type":"turn_context","payload":{"model":"gpt-5"}}"#;

#[test]
fn a_deleted_codex_parent_still_holds_back_its_forks_replay_and_a_moved_rollout_is_not_counted_twice() {
    let home = tempfile::tempdir().unwrap();
    let sessions = home.path().join(".codex/sessions/2026/01/02");
    let parent = sessions.join("rollout-parent.jsonl");
    write(
        &[
            r#"{"timestamp":"2026-01-02T09:00:00Z","type":"session_meta","payload":{"id":"parent"}}"#.to_string(),
            CODEX_MODEL.to_string(),
            codex_count("09:00:00", 1_000, 100, (1_000, 100)),
            codex_count("09:05:00", 2_000, 200, (1_000, 100)),
        ],
        &parent,
    );
    // An older fork: it replays the parent's reading in an ordinary turn, known only by the
    // running total it repeats.
    let fork = sessions.join("rollout-child.jsonl");
    write(
        &[
            r#"{"timestamp":"2026-01-02T09:10:00Z","type":"session_meta","payload":{"id":"child","forked_from_id":"parent"}}"#.to_string(),
            CODEX_MODEL.to_string(),
            codex_count("09:10:00", 1_000, 100, (0, 0)),
            r#"{"timestamp":"2026-01-02T09:10:00Z","type":"event_msg","payload":{"type":"task_started","turn_id":"019f8f06-dfd5-7cd2-a871-b31a926f92b7"}}"#.to_string(),
            codex_count("09:10:00", 2_000, 200, (1_000, 100)),
            codex_count("09:10:00", 2_500, 250, (500, 50)),
        ],
        &fork,
    );

    let before = read(home.path(), Provider::Codex);
    assert_eq!(before.all_time().tokens, 2_200 + 550);

    std::fs::remove_file(&parent).unwrap();
    let deleted = read(home.path(), Provider::Codex);
    assert_eq!(deleted.days, before.days);
    assert_eq!(deleted.all_time().tokens, 2_200 + 550);

    // Codex moves a session it archives; the same rollout under another folder is the same work.
    let moved = home.path().join(".codex/archived_sessions/rollout-child.jsonl");
    std::fs::create_dir_all(moved.parent().unwrap()).unwrap();
    std::fs::rename(&fork, &moved).unwrap();
    let after_move = read(home.path(), Provider::Codex);
    assert_eq!(after_move.days, before.days);
    assert_eq!(after_move.all_time().tokens, 2_200 + 550);
}

#[test]
fn a_cache_written_for_another_home_is_not_kept_into_this_ones_archive() {
    let home = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let cache = home.path().join("cache");
    write(
        &[user("2026-09-01T10:00:00Z", "Hi"), reply("x1", "2026-09-01T10:00:40Z", 100, 300)],
        &other.path().join(".claude/projects/p/s.jsonl"),
    );
    let calendar = Calendar::utc(2);
    let now = chrono::Utc::now();
    read_ledger_with(Provider::ClaudeCode, &Sources::new(other.path()), &cache, &calendar, &prices(), now).unwrap();
    let mine = read_ledger_with(Provider::ClaudeCode, &Sources::new(home.path()), &cache, &calendar, &prices(), now).unwrap();
    assert_eq!(mine.all_time().tokens, 0);
    assert!(TranscriptArchive::load(Provider::ClaudeCode, &cache).unwrap().files.is_empty());
}

#[test]
fn an_unreadable_archive_is_not_overwritten_and_the_gone_file_stays_counted_from_the_cache() {
    let home = tempfile::tempdir().unwrap();
    let file = home.path().join(".claude/projects/p/s.jsonl");
    write(&[user("2026-09-01T10:00:00Z", "Hi"), reply("u1", "2026-09-01T10:00:40Z", 100, 300)], &file);
    let before = read(home.path(), Provider::ClaudeCode);
    let archive = home.path().join("cache").join(TranscriptArchive::file_name(Provider::ClaudeCode));
    std::fs::write(&archive, b"{ written by a later build").unwrap();

    std::fs::remove_file(&file).unwrap();
    assert_eq!(read(home.path(), Provider::ClaudeCode).days, before.days);
    assert_eq!(read(home.path(), Provider::ClaudeCode).days, before.days);
    assert_eq!(std::fs::read(&archive).unwrap(), b"{ written by a later build");
}
