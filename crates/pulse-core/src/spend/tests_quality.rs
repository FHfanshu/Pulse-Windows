//! Ported from upstream SpendDataQualityTests: one row for a conversation however many files it
//! is, a title in the user's words, no project made of a scratch folder. Every fixture is built in
//! a temporary directory, never read from the user's own agents.

use std::collections::HashMap;
use std::path::Path;

use chrono::{TimeZone, Utc};
use serde_json::{json, Value};

use super::agent::SpendAgent::Codex;
use super::calendar::Calendar;
use super::ledger::{Ledger, Session};
use super::prices::PriceTable;
use super::project::{SessionLabel, UsageProject};
use super::prompt_cache::{PromptCacheLapse, PromptCacheSession};
use super::read_ledger_with;
use super::summary::{SessionRow, SpendSummary};
use super::titles::{codex_title, is_codex_review, title_from};
use super::transcripts::{parse_codex_file, session_file_of, Sources};
use crate::provider::Provider;

fn write(file: &Path, lines: &[String]) {
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, lines.join("\n")).unwrap();
}

fn read(home: &Path, provider: Provider) -> Ledger {
    read_ledger_with(provider, &Sources::new(home), &home.join("cache"), &Calendar::local(), &PriceTable::new(), Utc::now()).unwrap()
}

fn reply(id: &str, time: &str, input: i64, output: i64, sidechain: bool) -> String {
    format!(
        r#"{{"type":"assistant","isSidechain":{sidechain},"timestamp":"2026-09-20T{time}:00Z","message":{{"id":"{id}","model":"m","usage":{{"input_tokens":{input},"output_tokens":{output}}}}}}}"#
    )
}

#[test]
fn a_subagents_transcript_is_part_of_its_parents_session_and_still_counted() {
    let home = tempfile::tempdir().unwrap();
    let project = home.path().join(".claude/projects/-Users-me-Code-Pulse");
    let uuid = "c71d6cd6-71fd-4ffa-9896-1ed30f025090";

    write(
        &project.join(format!("{uuid}.jsonl")),
        &[
            r#"{"type":"user","cwd":"/Users/me/Code/Pulse","message":{"role":"user","content":"fix the ring"}}"#.into(),
            reply("p1", "10:00", 100, 10, false),
        ],
    );
    // A subagent's lines are sidechain, so none could be a title. Its second reply is a copy of
    // one in the parent (the same message id): counted once.
    write(
        &project.join(format!("{uuid}/subagents/agent-a1.jsonl")),
        &[
            r#"{"type":"user","isSidechain":true,"cwd":"/Users/me/Code/Pulse","message":{"role":"user","content":"explore the card"}}"#.into(),
            reply("a1", "10:20", 200, 20, true),
            reply("p1", "10:00", 100, 10, true),
        ],
    );
    write(&project.join(format!("{uuid}/subagents/agent-b1.jsonl")), &[reply("b1", "11:05", 300, 30, true)]);
    // Another conversation in the same project stays its own row.
    write(
        &project.join("other.jsonl"),
        &[
            r#"{"type":"user","cwd":"/Users/me/Code/Pulse","message":{"role":"user","content":"second chat"}}"#.into(),
            reply("o1", "12:00", 5, 5, false),
        ],
    );

    for pass in 0..2 {
        // The second pass reads the per-file cache, stamps and all.
        let ledger = read(home.path(), Provider::ClaudeCode);
        assert_eq!(ledger.all_time().tokens, 110 + 220 + 330 + 10, "pass {pass}");

        assert_eq!(ledger.sessions.len(), 2);
        let merged = ledger.sessions.iter().find(|s| s.name == uuid).unwrap();
        assert!(merged.id.ends_with(&format!("/{uuid}.jsonl")));
        assert_eq!(merged.title.as_deref(), Some("fix the ring"));
        assert_eq!(merged.project.as_ref().unwrap().name, "Pulse");
        assert_eq!(merged.tokens, 110 + 220 + 330);
        assert_eq!(merged.slots.len(), 3);
        assert_eq!(merged.slots.iter().map(|s| s.tokens).sum::<i64>(), merged.tokens);
        // Widened to the earliest and the latest file.
        assert_eq!((merged.end - merged.start).num_seconds(), 60 * 60);
        assert_eq!(ledger.sessions.iter().find(|s| s.name == "other").unwrap().tokens, 10);
    }
}

#[test]
fn subagents_whose_parent_file_is_gone_still_make_one_session_named_for_the_project_folder() {
    let home = tempfile::tempdir().unwrap();
    let project = home.path().join(".claude/projects/-Users-me-Code-Pulse");
    let uuid = "11111111-2222-3333-4444-555555555555";
    write(&project.join(format!("{uuid}/subagents/agent-a.jsonl")), &[reply("a", "10:00", 10, 1, true)]);
    write(&project.join(format!("{uuid}/subagents/agent-b.jsonl")), &[reply("b", "10:30", 20, 2, true)]);

    let ledger = read(home.path(), Provider::ClaudeCode);
    assert_eq!(ledger.sessions.len(), 1);
    assert_eq!(ledger.sessions[0].tokens, 33);
    assert_eq!(ledger.sessions[0].name, uuid);
    // The folder of the project, not the `subagents` folder.
    assert_eq!(ledger.sessions[0].project.as_ref().unwrap().name, "Pulse");
}

#[test]
fn only_a_claude_code_subagent_path_is_folded() {
    assert_eq!(session_file_of("/h/.claude/projects/-p/abc/subagents/agent-1.jsonl"), "/h/.claude/projects/-p/abc.jsonl");
    assert_eq!(session_file_of("/h/.claude/projects/-p/abc/subagents/workflows/agent-1.jsonl"), "/h/.claude/projects/-p/abc.jsonl");
    assert_eq!(session_file_of("/h/.claude/projects/-p/abc.jsonl"), "/h/.claude/projects/-p/abc.jsonl");
    assert_eq!(session_file_of("/h/.claude/projects/-p/subagents-notes.jsonl"), "/h/.claude/projects/-p/subagents-notes.jsonl");
}

fn part(text: &str) -> Value {
    json!([{"type": "input_text", "text": text}])
}

#[test]
fn context_codex_injects_is_not_a_title_the_users_first_words_are() {
    let title = |text: &str| codex_title(Some(&part(text)));
    assert_eq!(title("# AGENTS.md instructions for /Users/me/Code/Pulse\n\n<INSTRUCTIONS>\nBe brief.\n</INSTRUCTIONS>"), None);
    assert_eq!(title("<environment_context>\n  <cwd>/Users/me</cwd>\n</environment_context>"), None);
    assert_eq!(title("<user_instructions>be brief</user_instructions>"), None);
    assert_eq!(title("<turn_aborted>The user interrupted</turn_aborted>"), None);
    assert_eq!(title("<recommended_plugins> Here are some"), None);
    assert_eq!(title("The following is the Codex agent history whose request action you are assessing."), None);
    assert_eq!(title("# Files mentioned by the user:\n## a.png: /tmp/a.png"), None);
    assert_eq!(title("fix the ring").as_deref(), Some("fix the ring"));
    // The IDE extension puts its context first; the request follows it.
    assert_eq!(
        title("# Context from my IDE setup:\n\n## Active file: a.swift\n\n## Open tabs:\n- a.swift\n\n## My request for Codex:\nfix the ring").as_deref(),
        Some("fix the ring")
    );
    assert_eq!(title("# Context from my IDE setup:\n\n## Open tabs:\n- a.swift"), None);
    // Only the first part is read: the rest of a review prompt is a transcript.
    let two = json!([{"type": "input_text", "text": "# AGENTS.md instructions for /x"}, {"type": "input_text", "text": "something else"}]);
    assert_eq!(codex_title(Some(&two)), None);
}

fn message(role: &str, text: &str) -> String {
    json!({"timestamp": "2026-09-20T10:00:00Z", "type": "response_item",
        "payload": {"type": "message", "role": role, "content": [{"type": "input_text", "text": text}]}})
    .to_string()
}

const COUNT: &str = r#"{"timestamp":"2026-09-20T10:00:00Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"output_tokens":10},"last_token_usage":{"input_tokens":100,"output_tokens":10}}}}"#;
const MODEL: &str = r#"{"timestamp":"2026-09-20T10:00:00Z","type":"turn_context","payload":{"model":"m"}}"#;

#[test]
fn a_rollouts_title_is_its_first_real_user_message() {
    let header = r#"{"timestamp":"2026-09-20T10:00:00Z","type":"session_meta","payload":{"id":"s1","cwd":"/work/api"}}"#;
    let root = tempfile::tempdir().unwrap();
    let real = root.path().join("real.jsonl");
    write(
        &real,
        &[
            header.into(),
            message("developer", "<permissions instructions>"),
            message("user", "# AGENTS.md instructions for /work/api\n\n<INSTRUCTIONS>\nBe brief.\n</INSTRUCTIONS>"),
            message("user", "<environment_context>\n<cwd>/work/api</cwd>\n</environment_context>"),
            message("user", "add a retry to the client"),
            message("user", "and a test"),
            COUNT.into(),
        ],
    );
    let only = root.path().join("only-context.jsonl");
    write(
        &only,
        &[header.into(), message("user", "# AGENTS.md instructions for /work/api"), message("user", "<environment_context></environment_context>"), COUNT.into()],
    );
    assert_eq!(parse_codex_file(&real, &Calendar::local()).title.as_deref(), Some("add a retry to the client"));
    assert_eq!(parse_codex_file(&only, &Calendar::local()).title, None);
}

#[test]
fn a_tools_scratch_folder_is_no_project_a_real_one_under_similar_names_is() {
    for path in [
        "/Users/me/Documents/Codex/2026-10-04/https-gist-githubusercontent-com-me-fe4d",
        "/Users/me/Documents/Codex/2026-10-04/make-a-poster/assets",
        "/Users/me/Documents/deepseek-harness/default-workspace",
        "/Users/me/.dsh/deepseek-harness/default-workspace/",
    ] {
        assert!(UsageProject::is_scratch_directory(path), "{path}");
        assert!(UsageProject::new(Some(path)).is_none(), "{path}");
    }
    for path in [
        "/Users/me/Documents/Codex",
        "/Users/me/Documents/Codex/2026-10-04",
        "/Users/me/Documents/Codex/notes/slug",
        "/Users/me/Documents/Codex/2026-1-04/slug",
        "/Users/me/Documents/Codex/20261004xx/slug",
        "/Users/me/Code/Codex/2026-10-04/slug",
        "/Users/me/Documents/deepseek-harness",
        "/Users/me/Documents/deepseek-harness/my-project",
        "/Users/me/Code/default-workspace",
    ] {
        assert!(!UsageProject::is_scratch_directory(path), "{path}");
        assert!(UsageProject::new(Some(path)).is_some(), "{path}");
    }
}

#[test]
fn a_codex_chat_in_a_scratch_folder_has_no_project_and_neither_does_one_read_from_an_old_cache() {
    let home = tempfile::tempdir().unwrap();
    let scratch = "/Users/me/Documents/Codex/2026-10-04/some-prompt";
    write(
        &home.path().join(".codex/sessions/2026/09/20/rollout-a.jsonl"),
        &[
            format!(r#"{{"timestamp":"2026-09-20T10:00:00Z","type":"session_meta","payload":{{"id":"s","cwd":"{scratch}"}}}}"#),
            MODEL.into(),
            COUNT.into(),
        ],
    );
    let ledger = read(home.path(), Provider::Codex);
    assert_eq!(ledger.sessions.len(), 1);
    assert_eq!(ledger.sessions[0].project, None);

    // A project kept before the rule, as an old cache holds it, is corrected as it is read.
    let old: UsageProject = serde_json::from_str(r#"{"identity":{"directory":"/Users/me/Documents/deepseek-harness/default-workspace"},"name":"default-workspace"}"#).unwrap();
    assert_eq!(old.unless_scratch(), None);
}

#[test]
fn a_link_on_the_first_line_is_not_the_title_when_words_follow_it() {
    assert_eq!(title_from("[https://example.com/a/b/c](https://example.com/a/b/c)\ncheck the proxy rules").as_deref(), Some("check the proxy rules"));
    assert_eq!(title_from("[https://example.com/a/b/c]\n\ncheck the proxy rules\nand the logs").as_deref(), Some("check the proxy rules"));
    assert_eq!(title_from("https://example.com/a/b/c\n帮我检查一下规则").as_deref(), Some("帮我检查一下规则"));
    assert_eq!(title_from("<https://example.com/a>\nread this").as_deref(), Some("read this"));
    // A first line of words keeps the whole message on one line, as before.
    assert_eq!(title_from("read\nhttps://example.com/a").as_deref(), Some("read https://example.com/a"));
}

#[test]
fn a_message_that_is_only_a_link_reads_as_host_first_segment() {
    assert_eq!(title_from("https://gist.example.org/someone/0123456789abcdef0123456789abcdef").as_deref(), Some("gist.example.org/someone/…"));
    assert_eq!(
        title_from("[https://gist.example.org/someone/0123456789abcdef](https://gist.example.org/someone/0123456789abcdef)").as_deref(),
        Some("gist.example.org/someone/…")
    );
    assert_eq!(title_from("[https://example.com/docs?q=1#top]").as_deref(), Some("example.com/docs"));
    assert_eq!(title_from("https://example.com/docs/").as_deref(), Some("example.com/docs"));
    assert_eq!(title_from("https://example.com").as_deref(), Some("example.com"));
    assert_eq!(title_from("  https://example.com/a/b \n https://example.com/c ").as_deref(), Some("example.com/a/…"));
}

#[test]
fn markdown_links_read_as_their_text_and_whitespace_is_collapsed() {
    assert_eq!(title_from("see [the docs](https://example.com/docs) for  details").as_deref(), Some("see the docs for details"));
    assert_eq!(title_from("open [https://example.com/x] now").as_deref(), Some("open https://example.com/x now"));
    assert_eq!(title_from("fix\t\tthe   ring\r\nand the card").as_deref(), Some("fix the ring and the card"));
    assert_eq!(title_from("[](https://example.com/x)").as_deref(), Some("example.com/x"));
    // A link is not an envelope, but a command envelope still is.
    assert_eq!(title_from("<command-name>/init</command-name>"), None);
    // The length limit is the same after cleaning.
    let long = title_from(&format!("[{}](https://example.com)", "word ".repeat(40))).unwrap();
    assert_eq!(long.chars().count(), 70);
    assert!(long.ends_with('…'));
}

const REVIEW_OPENING: &str = "The following is the Codex agent history whose request action you are assessing.";

#[test]
fn a_codex_session_that_opens_with_a_review_request_is_marked_and_keeps_no_title() {
    assert!(is_codex_review(Some(&part(REVIEW_OPENING))));
    assert!(!is_codex_review(Some(&part("fix the ring"))));
    assert!(!is_codex_review(Some(&json!("# AGENTS.md instructions for /x"))));

    let home = tempfile::tempdir().unwrap();
    let scratch = "/Users/me/Documents/Codex/2026-10-04/some-prompt";
    write(
        &home.path().join(".codex/sessions/2026/09/20/rollout-2026-09-20T10-00-00-r1.jsonl"),
        &[
            format!(r#"{{"timestamp":"2026-09-20T10:00:00Z","type":"session_meta","payload":{{"id":"r1","cwd":"{scratch}"}}}}"#),
            MODEL.into(),
            message("user", "# AGENTS.md instructions for /work/api"),
            message("user", REVIEW_OPENING),
            // Anything after the request is the transcript under review.
            message("user", "fix the ring"),
            COUNT.into(),
        ],
    );
    write(
        &home.path().join(".codex/sessions/2026/09/20/rollout-2026-09-20T11-00-00-n1.jsonl"),
        &[
            r#"{"timestamp":"2026-09-20T11:00:00Z","type":"session_meta","payload":{"id":"n1","cwd":"/work/api"}}"#.into(),
            MODEL.into(),
            message("user", "add a retry"),
            COUNT.replace("10:00:00", "11:00:00"),
        ],
    );
    // Read fresh, and again from the cache it wrote: the flag travels.
    for pass in 0..2 {
        let ledger = read(home.path(), Provider::Codex);
        let review = ledger.sessions.iter().find(|s| s.name.ends_with("r1")).unwrap_or_else(|| panic!("pass {pass}"));
        assert!(review.is_review);
        assert_eq!(review.title, None);
        assert_eq!(review.project, None);
        let ordinary = ledger.sessions.iter().find(|s| s.name.ends_with("n1")).unwrap();
        assert!(!ordinary.is_review);
        assert_eq!(ordinary.title.as_deref(), Some("add a retry"));
    }
    assert!(home.path().join("cache/ledger-9-codex.json").exists());
}

fn bare_session(is_review: bool) -> Session {
    let at = Utc.timestamp_opt(1_789_372_800, 0).unwrap();
    Session {
        id: "/h/.codex/sessions/rollout-2026-10-04T20-42-25-01a106ef.jsonl".into(),
        name: "rollout-2026-10-04T20-42-25-01a106ef".into(),
        title: None,
        is_review,
        project: None,
        start: at,
        end: at,
        tokens: 10,
        cost: 0.0,
        unpriced_tokens: 0,
        slots: vec![],
        days: vec![],
    }
}

#[test]
fn a_session_is_never_named_for_its_file() {
    assert_eq!(SessionLabel::text(Some("Fix the ring"), false, Some("Pulse")), "Fix the ring");
    assert_eq!(SessionLabel::text(None, false, Some("Pulse")), "Pulse");
    assert_eq!(SessionLabel::text(None, true, None), "Codex review");
    assert_eq!(SessionLabel::text(None, false, None), "Untitled conversation");

    let untitled = bare_session(false);
    let label = SessionLabel::text(untitled.title.as_deref(), untitled.is_review, None);
    assert_eq!(label, "Untitled conversation");
    assert!(!label.contains("rollout"));
    assert!(!SessionLabel::names_itself(untitled.title.as_deref(), untitled.is_review));
    // The project is the label here, so a subtitle must not repeat it.
    assert_eq!(SessionLabel::text(None, false, Some("Pulse")), "Pulse");

    let review = bare_session(true);
    assert_eq!(SessionLabel::text(review.title.as_deref(), review.is_review, None), "Codex review");
    assert!(SessionLabel::names_itself(review.title.as_deref(), review.is_review));

    // The prompt-cache lists read through the same rule.
    let lapse = PromptCacheLapse::new(Utc::now(), PromptCacheLapse::HOUR);
    let session = |is_review| PromptCacheSession { id: "a".into(), title: None, is_review, project: None, lapse };
    assert_eq!(session(false).display_name(), "Untitled conversation");
    assert_eq!(session(true).display_name(), "Codex review");
}

#[test]
fn the_summarys_session_rows_keep_the_review_flag() {
    let mut ledger = Ledger::empty();
    ledger.sessions = vec![bare_session(true)];
    let kept = SessionRow { agent: Codex, session: ledger.sessions[0].clone() };
    assert!(kept.session.is_review);
    // And through the summary's own windowing.
    let day = Utc.timestamp_opt(1_789_372_800, 0).unwrap();
    let summary = SpendSummary::of(&HashMap::from([(Codex, ledger)]), None, day, &Calendar::utc(2));
    assert!(summary.sessions.iter().all(|s| s.session.is_review));
}
