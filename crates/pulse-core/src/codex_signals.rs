// Ported from upstream Sources/Pulse/Usage/CodexSignals.swift.
//! Signs, in this PC's Codex sessions, that the work was quietly given less than was asked for.
//! **Signs, not proof**, and only what Codex writes down.
//!
//! The one fact that would settle it, which model the server actually ran, is never saved: Codex
//! reads it from a response header and keeps the warning it raises on a mismatch out of the session
//! file (`ModelReroute` is transient in `codex-rs/rollout/src/policy.rs`). Finding it means sending
//! a probe or sitting in the traffic, and Pulse does neither: it reads. What *is* written leaves
//! two kinds of trace, and this reads both.
//!
//! **Reasoning cut short.** Every response's `token_count` event carries
//! `last_token_usage.reasoning_output_tokens`. Reasoning that stops on its own lands on any count;
//! reasoning that is cut lands on 516, 1034, 1552... : `518*n - 2`, a lattice found in
//! openai/codex#30364 over 390k responses and reproduced upstream (gpt-5.5: 41.6% of responses that
//! reached 516 stopped exactly on it; the codex models, none). By chance about one in 518 would. The
//! step and the offset are empirical; nobody has published a cause. Counted per model, as a share of
//! the responses that got that far.
//!
//! **Settings changed under the session.** From Codex 0.144 every change the user makes is written
//! as `thread_settings_applied`, and takes effect at the next `task_started`. A turn whose
//! `turn_context` runs a different model, a lower reasoning effort, or a smaller context window than
//! the settings in force when it started was changed by something other than the user. Sessions
//! written before 0.144 record no such event (a `/model` there is indistinguishable from a silent
//! switch) so they are not judged at all; nor is a newer one with none applied, nor Codex's own
//! helpers (sessions with a `source` or `parent_thread_id`, turns whose `root_turn_id` is another's,
//! the `auto-review` model).
//!
//! **A fork copies its parent first**, every copied line stamped with the moment of the fork; those
//! lines are skipped, or a month-old response would be counted twice and dated today.
//! All of it is request-side: what Codex asked for, not what the server ran.
//!
//! Windows: the sessions are `%USERPROFILE%\.codex\sessions` and `archived_sessions` (or under
//! `CODEX_HOME`); each file's facts are kept in memory against its size and modification time, so
//! opening the pane again, or widening the span, reads only what changed.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use serde_json::{Map, Value};

use crate::spend::loglines::LineReader;

pub const FEWEST_REACHED: i64 = 20;
pub const FEWEST_HITS: i64 = 5;
/// Twenty-five times chance (1 in 518).
pub const SUSPICIOUS_SHARE: f64 = 0.05;
/// The first Codex that writes the user's own changes down.
pub const FIRST_JUDGED_VERSION: (i64, i64, i64) = (0, 144, 0);
/// How long after a fork's header its copied history is still being written: measured, every copied
/// line carries the fork's own moment.
pub const REPLAY_WINDOW_SECONDS: i64 = 2;

/// `518*n - 2` for some n >= 1.
pub fn is_on_lattice(reasoning: i64) -> bool {
    reasoning >= 516 && (reasoning + 2) % 518 == 0
}

/// One model's responses over the span.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Truncation {
    pub model: String,
    /// Responses with any reasoning at all.
    pub responses: i64,
    /// Responses whose reasoning reached 516, the first lattice point.
    pub reached_lattice: i64,
    /// Of those, the ones that stopped exactly on a lattice point.
    pub on_lattice: i64,
}

impl Truncation {
    pub fn new(model: impl Into<String>) -> Self {
        Self { model: model.into(), responses: 0, reached_lattice: 0, on_lattice: 0 }
    }

    pub fn share(&self) -> Option<f64> {
        (self.reached_lattice > 0).then(|| self.on_lattice as f64 / self.reached_lattice as f64)
    }

    /// Enough responses reached the lattice to say anything about it.
    pub fn is_measurable(&self) -> bool {
        self.reached_lattice >= FEWEST_REACHED
    }

    /// Many times what chance would put there, and not a stray hit or two.
    pub fn is_suspicious(&self) -> bool {
        self.is_measurable() && self.on_lattice >= FEWEST_HITS && self.share().unwrap_or(0.0) >= SUSPICIOUS_SHARE
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum ChangeKind {
    Model { asked: String, ran: String },
    Effort { asked: String, ran: String },
    ContextWindow { was: i64, now: i64 },
}

/// A turn that ran on less than the settings in force when it started.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    #[serde(flatten)]
    pub kind: ChangeKind,
    pub date: DateTime<Utc>,
    pub session: String,
    /// Its place in the session, so two changes written in the same instant are still two rows.
    pub order: usize,
}

impl Change {
    pub fn id(&self) -> String {
        format!("{}|{}", self.session, self.order)
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct CodexSignals {
    pub truncation: Vec<Truncation>,
    pub changes: Vec<Change>,
    /// Sessions in the span, and how many of them were new enough to judge for changed settings.
    pub sessions: i64,
    pub judged_sessions: i64,
}

/// One response, as much as the counts need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub date: DateTime<Utc>,
    pub model: String,
    pub reasoning: i64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct FileFacts {
    pub responses: Vec<Response>,
    pub changes: Vec<Change>,
    pub last_date: Option<DateTime<Utc>>,
    pub is_judged: bool,
}

/// Codex's reviewer, which runs sessions of its own by design: not a model anybody picked, and not
/// one to report on.
pub fn is_codexs_own(model: &str) -> bool {
    model.contains("auto-review")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    size: u64,
    modified: std::time::Duration,
}

/// Reads the signs out of `~/.codex/sessions` and `~/.codex/archived_sessions`.
///
/// Blocking: call it from `spawn_blocking`.
pub struct CodexSignalReader {
    roots: Vec<PathBuf>,
    cache: Mutex<HashMap<PathBuf, (Stamp, Arc<FileFacts>)>>,
}

impl CodexSignalReader {
    pub fn new(roots: Vec<PathBuf>) -> Self {
        Self { roots, cache: Mutex::new(HashMap::new()) }
    }

    /// The signs since `since` (everything when None). `cancelled` is asked between files and
    /// lines; a cancelled read returns None and keeps nothing from the file it stopped in.
    pub fn read(&self, since: Option<DateTime<Utc>>, cancelled: &dyn Fn() -> bool) -> Option<CodexSignals> {
        let mut signals = CodexSignals::default();
        let mut by_model: HashMap<String, Truncation> = HashMap::new();

        for (file, modified) in self.files() {
            if cancelled() {
                return None;
            }
            if let (Some(since), Some(modified)) = (since, modified) {
                if modified < since {
                    continue;
                }
            }
            let Some(facts) = self.facts(&file, cancelled) else {
                if cancelled() {
                    return None;
                }
                continue;
            };
            if let (Some(since), Some(last)) = (since, facts.last_date) {
                if last < since {
                    continue;
                }
            }
            signals.sessions += 1;
            if facts.is_judged {
                signals.judged_sessions += 1;
            }
            for response in facts.responses.iter().filter(|r| since.map_or(true, |s| r.date >= s)) {
                if is_codexs_own(&response.model) {
                    continue;
                }
                let entry = by_model.entry(response.model.clone()).or_insert_with(|| Truncation::new(response.model.clone()));
                entry.responses += 1;
                if response.reasoning >= 516 {
                    entry.reached_lattice += 1;
                }
                if is_on_lattice(response.reasoning) {
                    entry.on_lattice += 1;
                }
            }
            signals.changes.extend(facts.changes.iter().filter(|c| since.map_or(true, |s| c.date >= s)).cloned());
        }

        signals.truncation = by_model.into_values().collect();
        // Suspicious models first, then the busiest; the name settles a tie so the order is stable.
        signals.truncation.sort_by(|a, b| {
            (b.is_suspicious(), b.responses, &a.model).cmp(&(a.is_suspicious(), a.responses, &b.model))
        });
        signals.changes.sort_by(|a, b| b.date.cmp(&a.date).then_with(|| a.id().cmp(&b.id())));
        Some(signals)
    }

    /// Every `rollout-*.jsonl` under the roots, with its modification time.
    fn files(&self) -> Vec<(PathBuf, Option<DateTime<Utc>>)> {
        let mut found = Vec::new();
        let mut pending: Vec<PathBuf> = self.roots.iter().rev().cloned().collect();
        while let Some(directory) = pending.pop() {
            let Ok(entries) = std::fs::read_dir(&directory) else { continue };
            for entry in entries.flatten() {
                let Ok(kind) = entry.file_type() else { continue };
                let path = entry.path();
                if kind.is_dir() {
                    pending.push(path);
                    continue;
                }
                let name = entry.file_name().to_string_lossy().into_owned();
                if !(kind.is_file() && name.starts_with("rollout-") && name.ends_with(".jsonl")) {
                    continue;
                }
                let modified = std::fs::metadata(&path).ok().and_then(|m| m.modified().ok()).map(DateTime::<Utc>::from);
                found.push((path, modified));
            }
        }
        found.sort_by(|a, b| a.0.cmp(&b.0));
        found
    }

    fn facts(&self, file: &Path, cancelled: &dyn Fn() -> bool) -> Option<Arc<FileFacts>> {
        let metadata = std::fs::metadata(file).ok()?;
        let stamp = Stamp { size: metadata.len(), modified: metadata.modified().ok()?.duration_since(UNIX_EPOCH).ok()? };
        if let Some((kept, facts)) = self.cache.lock().unwrap().get(file) {
            if *kept == stamp {
                return Some(facts.clone());
            }
        }
        let session = file.file_name()?.to_string_lossy().into_owned();
        let source = std::fs::File::open(file).ok()?;
        let facts = Arc::new(parse(LineReader::new(source), &session, cancelled));
        // A cancelled read stops yielding lines, so what came back is a part of the file: kept, it
        // would stand for the whole until it changed.
        if cancelled() {
            return None;
        }
        self.cache.lock().unwrap().insert(file.to_path_buf(), (stamp, facts.clone()));
        Some(facts)
    }
}

const WANTED: [&str; 5] = ["\"session_meta\"", "\"turn_context\"", "\"thread_settings_applied\"", "\"task_started\"", "\"token_count\""];

const EFFORTS: [&str; 6] = ["none", "minimal", "low", "medium", "high", "xhigh"];

#[derive(Default, Clone)]
struct Settings {
    model: Option<String>,
    effort: Option<String>,
}

fn text(value: Option<&Value>) -> Option<String> {
    value?.as_str().map(str::to_string)
}

fn int(value: Option<&Value>) -> Option<i64> {
    let number = value?.as_number()?;
    number.as_i64().or_else(|| number.as_f64().filter(|f| f.is_finite()).map(|f| f as i64))
}

/// One session file's facts from its lines. `session` names it in the changes it yields.
pub fn parse<R: std::io::Read>(mut lines: LineReader<R>, session: &str, cancelled: &dyn Fn() -> bool) -> FileFacts {
    let wanted: Vec<memchr::memmem::Finder> = WANTED.iter().map(|w| memchr::memmem::Finder::new(w.as_bytes()).into_owned()).collect();

    let mut facts = FileFacts::default();
    let mut version: Option<(i64, i64, i64)> = None;
    let mut model: Option<String> = None;
    let mut last_totals: Option<Vec<i64>> = None;

    // Settings: the latest the user applied, the ones in force when the running turn started, and
    // the previous turn's own.
    let mut applied = Settings::default();
    let mut at_start = Settings::default();
    let mut previous = Settings::default();
    let mut sub_turns: HashSet<String> = HashSet::new();
    let mut window: Option<(i64, Option<String>)> = None;
    let mut applied_any = false;
    let mut is_own_session = false;
    // A fork starts by copying the session it came from, every copied line stamped with the moment
    // of the fork; those are the parent's responses and settings, counted in the parent's own file.
    let mut replay_until: Option<DateTime<Utc>> = None;

    let empty = Map::new();
    let mut seen = 0usize;
    while let Some(line) = lines.next_line() {
        seen += 1;
        if seen % 1024 == 0 && cancelled() {
            break;
        }
        if !wanted.iter().any(|w| w.find(line).is_some()) {
            continue;
        }
        let Ok(Value::Object(root)) = serde_json::from_slice::<Value>(line) else { continue };
        let payload = root.get("payload").and_then(Value::as_object).unwrap_or(&empty);
        let date = root.get("timestamp").and_then(Value::as_str).and_then(parse_date);
        if let Some(date) = date {
            facts.last_date = Some(facts.last_date.map_or(date, |last| last.max(date)));
        }
        let kind = root.get("type").and_then(Value::as_str);
        if kind != Some("session_meta") {
            if let (Some(until), Some(date)) = (replay_until, date) {
                if date <= until {
                    continue;
                }
            }
        }

        match kind {
            Some("session_meta") => {
                // A resumed session writes its header again; only the first says what the
                // session is.
                window = None;
                if version.is_some() {
                    continue;
                }
                version = payload.get("cli_version").and_then(Value::as_str).and_then(parse_version);
                facts.is_judged = version.is_some_and(|v| v >= FIRST_JUDGED_VERSION);
                // Codex's own helpers (a guardian, a sub-agent) run on models Codex picks, not the
                // user.
                if payload.get("source").is_some_and(Value::is_object) || payload.get("parent_thread_id").is_some_and(Value::is_string) {
                    is_own_session = true;
                }
                if payload.get("forked_from_id").is_some_and(Value::is_string) {
                    if let Some(date) = date {
                        replay_until = Some(date + Duration::seconds(REPLAY_WINDOW_SECONDS));
                    }
                }
            }

            Some("turn_context") => {
                let ran = text(payload.get("model"));
                if let Some(ran) = &ran {
                    model = Some(ran.clone());
                }
                let Some(date) = date.filter(|_| facts.is_judged) else { continue };
                if text(payload.get("turn_id")).is_some_and(|turn| sub_turns.contains(&turn)) {
                    continue;
                }
                let effort = text(payload.get("effort")).or_else(|| {
                    text(payload.get("collaboration_mode").and_then(|m| m.get("settings")).and_then(|s| s.get("reasoning_effort")))
                });
                let asked_model = at_start.model.clone().or_else(|| previous.model.clone());
                let asked_effort = at_start.effort.clone().or_else(|| previous.effort.clone());
                let own_model = ran.as_deref().is_some_and(is_codexs_own);
                if let (Some(ran), Some(asked)) = (&ran, &asked_model) {
                    if ran != asked && !own_model && !is_codexs_own(asked) {
                        facts.changes.push(Change {
                            kind: ChangeKind::Model { asked: asked.clone(), ran: ran.clone() },
                            date,
                            session: session.to_string(),
                            order: 0,
                        });
                    }
                }
                if let (Some(effort), Some(asked), false) = (&effort, &asked_effort, own_model) {
                    let have = EFFORTS.iter().position(|e| e == effort);
                    let want = EFFORTS.iter().position(|e| e == asked);
                    if let (Some(have), Some(want)) = (have, want) {
                        if have < want {
                            facts.changes.push(Change {
                                kind: ChangeKind::Effort { asked: asked.clone(), ran: effort.clone() },
                                date,
                                session: session.to_string(),
                                order: 0,
                            });
                        }
                    }
                }
                previous = Settings { model: ran.or(previous.model), effort: effort.or(previous.effort) };
            }

            Some("event_msg") => match payload.get("type").and_then(Value::as_str) {
                Some("task_started") => {
                    if let (Some(turn), Some(origin)) = (text(payload.get("turn_id")), text(payload.get("root_turn_id"))) {
                        if !origin.is_empty() && origin != turn {
                            sub_turns.insert(turn);
                        }
                    }
                    at_start = applied.clone();
                }
                Some("thread_settings_applied") => {
                    applied_any = true;
                    let settings = payload.get("thread_settings").and_then(Value::as_object).unwrap_or(&empty);
                    applied = Settings {
                        model: text(settings.get("model")).or(applied.model),
                        effort: text(settings.get("reasoning_effort")).or(applied.effort),
                    };
                }
                Some("token_count") => {
                    let Some(info) = payload.get("info").and_then(Value::as_object) else { continue };
                    if facts.is_judged {
                        if let Some(size) = int(info.get("model_context_window")).filter(|s| *s > 0) {
                            if let (Some((was, was_model)), Some(date)) = (&window, date) {
                                if size < *was && *was_model == model {
                                    facts.changes.push(Change {
                                        kind: ChangeKind::ContextWindow { was: *was, now: size },
                                        date,
                                        session: session.to_string(),
                                        order: 0,
                                    });
                                }
                            }
                            window = Some((size, model.clone()));
                        }
                    }
                    // The same count is written more than once; a repeat of the running totals is
                    // not another response.
                    let totals = info.get("total_token_usage").and_then(Value::as_object).map(|usage| {
                        ["input_tokens", "output_tokens", "reasoning_output_tokens", "total_tokens"]
                            .map(|k| int(usage.get(k)).unwrap_or(0))
                            .to_vec()
                    });
                    if let Some(totals) = totals {
                        if last_totals.as_ref() == Some(&totals) {
                            continue;
                        }
                        last_totals = Some(totals);
                    }
                    let Some(last) = info.get("last_token_usage").and_then(Value::as_object) else { continue };
                    let Some(reasoning) = int(last.get("reasoning_output_tokens")).filter(|r| *r > 0) else { continue };
                    let (Some(model), Some(date)) = (&model, date) else { continue };
                    facts.responses.push(Response { date, model: model.clone(), reasoning });
                }
                _ => {}
            },
            _ => {}
        }
    }

    // Judged only where the user's own changes are on record: a session with none applied is
    // compared turn to turn alone, and could not tell a `/model` from a swap. Codex's helpers are
    // not the user's.
    facts.is_judged = facts.is_judged && applied_any && !is_own_session;
    if facts.is_judged {
        for (index, change) in facts.changes.iter_mut().enumerate() {
            change.order = index;
        }
    } else {
        facts.changes.clear();
    }
    facts
}

/// `0.146.0-alpha.3.1` is (0, 146, 0): the pre-release tag is ignored.
pub fn parse_version(text: &str) -> Option<(i64, i64, i64)> {
    let numbers: Vec<i64> = text.split('-').next()?.split('.').filter_map(|p| p.parse().ok()).collect();
    (numbers.len() >= 3).then(|| (numbers[0], numbers[1], numbers[2]))
}

fn parse_date(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text).ok().map(|d| d.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{SecondsFormat, TimeZone};

    fn line(kind: &str, payload: &str, at: i64) -> String {
        let stamp = Utc.timestamp_opt(1_790_000_000 + at, 0).unwrap().to_rfc3339_opts(SecondsFormat::Millis, true);
        format!(r#"{{"timestamp":"{stamp}","type":"{kind}","payload":{payload}}}"#)
    }

    fn meta(version: &str) -> String {
        line("session_meta", &format!(r#"{{"id":"s","cli_version":"{version}"}}"#), 0)
    }

    fn started(turn: &str, root: Option<&str>, at: i64) -> String {
        line(
            "event_msg",
            &format!(r#"{{"type":"task_started","turn_id":"{turn}","root_turn_id":"{}"}}"#, root.unwrap_or(turn)),
            at,
        )
    }

    fn context(turn: &str, model: &str, effort: &str, at: i64) -> String {
        line("turn_context", &format!(r#"{{"turn_id":"{turn}","model":"{model}","effort":"{effort}"}}"#), at)
    }

    fn applied(model: &str, effort: &str, at: i64) -> String {
        line(
            "event_msg",
            &format!(r#"{{"type":"thread_settings_applied","thread_settings":{{"model":"{model}","reasoning_effort":"{effort}"}}}}"#),
            at,
        )
    }

    /// A response's counts; `total` is the running total that tells repeats apart.
    fn count(reasoning: i64, total: i64, window: i64, at: i64) -> String {
        line(
            "event_msg",
            &format!(
                r#"{{"type":"token_count","info":{{"model_context_window":{window},"last_token_usage":{{"input_tokens":10,"output_tokens":{},"reasoning_output_tokens":{reasoning}}},"total_token_usage":{{"input_tokens":{total},"output_tokens":{total},"reasoning_output_tokens":{total},"total_tokens":{total}}}}}}}"#,
                reasoning + 5
            ),
            at,
        )
    }

    fn parse_lines(lines: &[String]) -> FileFacts {
        parse(LineReader::new(lines.join("\n").into_bytes().as_slice()), "s", &|| false)
    }

    fn kinds(facts: &FileFacts) -> Vec<ChangeKind> {
        facts.changes.iter().map(|c| c.kind.clone()).collect()
    }

    #[test]
    fn the_lattice_is_516_1034_1552_and_nothing_between() {
        for on in [516, 1_034, 1_552] {
            assert!(is_on_lattice(on), "{on}");
        }
        for off in [514, 517, 1_036] {
            assert!(!is_on_lattice(off), "{off}");
        }
    }

    #[test]
    fn each_response_counted_once_under_the_model_in_force_a_repeated_count_is_not_another() {
        let facts = parse_lines(&[
            meta("0.120.0"),
            context("a", "gpt-5.5", "high", 1),
            count(516, 100, 272_000, 2),
            count(516, 100, 272_000, 3),
            count(0, 150, 272_000, 4),
            context("b", "gpt-5.3-codex", "high", 5),
            count(700, 200, 272_000, 6),
        ]);
        assert_eq!(facts.responses.iter().map(|r| r.model.as_str()).collect::<Vec<_>>(), ["gpt-5.5", "gpt-5.3-codex"]);
        assert_eq!(facts.responses.iter().map(|r| r.reasoning).collect::<Vec<_>>(), [516, 700]);
        assert!(!facts.is_judged);
        assert!(facts.changes.is_empty());
    }

    #[test]
    fn a_turn_that_ran_on_another_model_or_less_effort_than_was_set_is_a_change_the_users_own_is_not() {
        let facts = parse_lines(&[
            meta("0.146.0-alpha.3"),
            started("t1", None, 1),
            context("t1", "gpt-6-astra", "high", 1),
            applied("gpt-6-astra", "high", 2),
            // Run on another model with nothing applied.
            started("t2", None, 3),
            context("t2", "gpt-5.6-luna", "high", 3),
            // The user switches to sol and down to medium: not a change.
            applied("gpt-6-sol", "medium", 4),
            started("t3", None, 5),
            context("t3", "gpt-6-sol", "medium", 5),
            // Effort lowered under the turn.
            started("t4", None, 6),
            context("t4", "gpt-6-sol", "low", 6),
            // A sub-agent's turn on its own model is Codex's business.
            started("t5", Some("t4"), 7),
            context("t5", "gpt-5.6-luna", "high", 7),
        ]);
        assert!(facts.is_judged);
        assert_eq!(
            kinds(&facts),
            vec![
                ChangeKind::Model { asked: "gpt-6-astra".into(), ran: "gpt-5.6-luna".into() },
                ChangeKind::Effort { asked: "medium".into(), ran: "low".into() },
            ]
        );
        assert_eq!(facts.changes.iter().map(|c| c.order).collect::<Vec<_>>(), [0, 1]);
    }

    #[test]
    fn a_setting_applied_while_a_turn_runs_takes_effect_at_the_next_turn_not_that_one() {
        let facts = parse_lines(&[
            meta("0.146.0"),
            started("t1", None, 1),
            applied("gpt-6-sol", "medium", 1),
            context("t1", "gpt-6-sol", "medium", 1),
            started("t2", None, 2),
            applied("gpt-6-sol", "high", 3),
            context("t2", "gpt-6-sol", "medium", 3),
            started("t3", None, 4),
            context("t3", "gpt-6-sol", "high", 4),
        ]);
        assert!(facts.changes.is_empty());
    }

    #[test]
    fn codex_before_0_144_writes_no_user_changes_so_its_sessions_are_not_judged() {
        let facts = parse_lines(&[
            meta("0.119.0-alpha.28"),
            started("t1", None, 1),
            context("t1", "gpt-5.4", "high", 1),
            started("t2", None, 2),
            context("t2", "gpt-5.4-mini", "high", 2),
        ]);
        assert!(!facts.is_judged);
        assert!(facts.changes.is_empty());
    }

    #[test]
    fn a_context_window_that_shrinks_under_the_same_model_is_a_change() {
        let facts = parse_lines(&[
            meta("0.146.0"),
            started("t1", None, 1),
            applied("gpt-6-sol", "high", 1),
            context("t1", "gpt-6-sol", "high", 1),
            count(10, 10, 272_000, 2),
            count(10, 20, 128_000, 3),
        ]);
        assert_eq!(kinds(&facts), vec![ChangeKind::ContextWindow { was: 272_000, now: 128_000 }]);
    }

    #[test]
    fn a_session_with_no_settings_on_record_or_one_of_codexs_helpers_is_not_judged() {
        let swap = [
            started("t1", None, 1),
            context("t1", "gpt-6-sol", "high", 1),
            started("t2", None, 2),
            context("t2", "gpt-5.6-luna", "high", 2),
        ];
        let none = parse_lines(&[vec![meta("0.146.0")], swap.to_vec()].concat());
        assert!(!none.is_judged);
        assert!(none.changes.is_empty());
        let helper = parse_lines(
            &[
                vec![
                    line("session_meta", r#"{"id":"s","cli_version":"0.146.0","source":{"subagent":{"other":"guardian"}}}"#, 0),
                    applied("gpt-6-sol", "high", 0),
                ],
                swap.to_vec(),
            ]
            .concat(),
        );
        assert!(!helper.is_judged);
        assert!(helper.changes.is_empty());
    }

    #[test]
    fn a_forks_copy_of_its_parent_is_skipped_its_own_work_after_the_copy_counts() {
        let facts = parse_lines(&[
            line("session_meta", r#"{"id":"f","cli_version":"0.146.0","forked_from_id":"p"}"#, 100),
            // The parent's history, copied at the moment of the fork.
            context("old", "gpt-5.5", "high", 100),
            count(516, 1_000, 272_000, 100),
            count(516, 2_000, 272_000, 101),
            // The fork's own turn.
            started("new", None, 160),
            applied("gpt-6-sol", "high", 160),
            context("new", "gpt-6-sol", "high", 160),
            count(700, 3_000, 272_000, 170),
        ]);
        assert_eq!(facts.responses.iter().map(|r| r.model.as_str()).collect::<Vec<_>>(), ["gpt-6-sol"]);
        assert_eq!(facts.responses.iter().map(|r| r.reasoning).collect::<Vec<_>>(), [700]);
        assert!(facts.is_judged);
    }

    #[test]
    fn per_model_the_share_of_long_replies_on_the_lattice_a_few_is_too_few_many_is_a_sign() {
        let root = tempfile::tempdir().unwrap();
        let day = root.path().join("2026").join("10").join("01");
        std::fs::create_dir_all(&day).unwrap();

        let mut lines = vec![meta("0.146.0"), context("a", "gpt-5.5", "high", 1)];
        let mut total = 0;
        // 30 long replies, 12 of them on the lattice; plus short ones.
        for index in 0..30 {
            total += 1_000;
            lines.push(count(if index < 12 { 516 } else { 600 + index }, total, 272_000, 10 + index));
        }
        for index in 0..5 {
            total += 1_000;
            lines.push(count(40, total, 272_000, 100 + index));
        }
        lines.push(context("b", "gpt-5.3-codex", "high", 200));
        for index in 0..3 {
            total += 1_000;
            lines.push(count(516, total, 272_000, 201 + index));
        }
        std::fs::write(day.join("rollout-x.jsonl"), lines.join("\n")).unwrap();
        // Not a rollout: never read.
        std::fs::write(day.join("notes.jsonl"), lines.join("\n")).unwrap();

        let signals = CodexSignalReader::new(vec![root.path().to_path_buf()]).read(None, &|| false).unwrap();
        let five = signals.truncation.iter().find(|t| t.model == "gpt-5.5").unwrap();
        assert_eq!((five.responses, five.reached_lattice, five.on_lattice), (35, 30, 12));
        assert!(five.is_suspicious());
        let codex = signals.truncation.iter().find(|t| t.model == "gpt-5.3-codex").unwrap();
        assert!(!codex.is_measurable());
        assert!(!codex.is_suspicious());
        assert_eq!(signals.truncation[0].model, "gpt-5.5");
        assert_eq!(signals.sessions, 1);
    }

    #[test]
    fn codexs_reviewer_is_not_a_model_anybody_picked() {
        let root = tempfile::tempdir().unwrap();
        let lines = [meta("0.120.0"), context("a", "gpt-5.5-auto-review", "high", 1), count(516, 100, 272_000, 2)];
        std::fs::write(root.path().join("rollout-a.jsonl"), lines.join("\n")).unwrap();
        let signals = CodexSignalReader::new(vec![root.path().to_path_buf()]).read(None, &|| false).unwrap();
        assert!(signals.truncation.is_empty());
    }

    #[test]
    fn the_span_cuts_responses_and_changes_by_their_own_dates_and_sessions_by_their_last() {
        let root = tempfile::tempdir().unwrap();
        let lines = [meta("0.120.0"), context("a", "gpt-5.5", "high", 1), count(600, 100, 272_000, 10), count(600, 200, 272_000, 5_000)];
        std::fs::write(root.path().join("rollout-a.jsonl"), lines.join("\n")).unwrap();
        let reader = CodexSignalReader::new(vec![root.path().to_path_buf()]);
        let since = Utc.timestamp_opt(1_790_000_000 + 1_000, 0).unwrap();
        // The file was written just now, so only its lines are cut by date.
        let signals = reader.read(Some(since), &|| false).unwrap();
        assert_eq!(signals.truncation[0].responses, 1);
        let later = Utc.timestamp_opt(1_790_000_000 + 9_000, 0).unwrap();
        let none = reader.read(Some(later), &|| false).unwrap();
        assert_eq!(none.sessions, 0);
        assert!(none.truncation.is_empty());
    }

    #[test]
    fn a_cancelled_read_keeps_nothing_so_the_next_read_sees_the_whole_file() {
        let root = tempfile::tempdir().unwrap();
        let mut lines = vec![meta("0.120.0"), context("a", "gpt-5.5", "high", 1)];
        for index in 0..5_000 {
            lines.push(count(600, (index + 1) * 10, 272_000, 2 + index));
        }
        std::fs::write(root.path().join("rollout-a.jsonl"), lines.join("\n")).unwrap();

        let reader = CodexSignalReader::new(vec![root.path().to_path_buf()]);
        assert!(reader.read(None, &|| true).is_none());
        let whole = reader.read(None, &|| false).unwrap();
        assert_eq!(whole.truncation[0].responses, 5_000);
    }

    #[test]
    fn a_file_that_changed_is_read_again() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("rollout-a.jsonl");
        let mut lines = vec![meta("0.120.0"), context("a", "gpt-5.5", "high", 1), count(600, 10, 272_000, 2)];
        std::fs::write(&file, lines.join("\n")).unwrap();
        let reader = CodexSignalReader::new(vec![root.path().to_path_buf()]);
        assert_eq!(reader.read(None, &|| false).unwrap().truncation[0].responses, 1);
        lines.push(count(600, 20, 272_000, 3));
        std::fs::write(&file, lines.join("\n")).unwrap();
        assert_eq!(reader.read(None, &|| false).unwrap().truncation[0].responses, 2);
    }

    #[test]
    fn versions_read_with_or_without_a_pre_release_tag() {
        assert_eq!(parse_version("0.146.0-alpha.3.1"), Some((0, 146, 0)));
        assert_eq!(parse_version("0.58.0"), Some((0, 58, 0)));
        assert_eq!(parse_version("dev"), None);
    }
}
