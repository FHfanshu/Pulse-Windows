// Ported from upstream Sources/Pulse/Usage/UsageLedger.swift (UsageLedgerReader, FileCache).
//! Reads Claude Code's and Codex's own transcripts and adds them up.
//!
//! Scanning is kept off the price list on purpose: the cache holds tokens per model per
//! quarter-hour, and money is worked out afterwards, so a price change costs nothing to apply
//! where caching the money would have meant rescanning a few hundred megabytes.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::calendar::Calendar;
use super::ledger::{price_buckets, session_slots, slot_key, slot_start, Buckets, Ledger, Session};
use super::loglines::{for_each_line, for_each_line_in};
use super::prices::{ModelPriceLookup, PriceTable};
use super::project::UsageProject;
use super::tally::{ReplyTiming, TokenTally};
use super::titles::{codex_title, is_codex_review, text_in, title_from};
use crate::provider::Provider;

/// The number in `ledger-<n>-<provider>.json`. Part of the contract: bump it whenever what an
/// entry holds changes, because a file that has not changed is never read again.
pub const CACHE_VERSION: u32 = 9;

pub type Timings = HashMap<String, HashMap<String, ReplyTiming>>;

// MARK: - Where the transcripts are

/// The folders transcripts are read from, with `CLAUDE_CONFIG_DIR` and `CODEX_HOME` honoured.
#[derive(Debug, Clone)]
pub struct Sources {
    pub home: PathBuf,
    pub claude_config_dir: Option<PathBuf>,
    pub codex_home: Option<PathBuf>,
}

impl Sources {
    /// `home` with no environment overrides.
    pub fn new(home: impl Into<PathBuf>) -> Self {
        Self { home: home.into(), claude_config_dir: None, codex_home: None }
    }

    /// `home`, plus the overrides in this process's environment.
    pub fn from_env(home: impl Into<PathBuf>) -> Self {
        let var = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from);
        Self { home: home.into(), claude_config_dir: var("CLAUDE_CONFIG_DIR"), codex_home: var("CODEX_HOME") }
    }

    pub fn claude_root(&self) -> PathBuf {
        self.claude_config_dir.clone().unwrap_or_else(|| self.home.join(".claude")).join("projects")
    }

    pub fn codex_root(&self) -> PathBuf {
        self.codex_home.clone().unwrap_or_else(|| self.home.join(".codex"))
    }

    /// Codex moves a session it archives out of `sessions` into `archived_sessions`; the work in
    /// it was still done.
    pub fn roots(&self, provider: Provider) -> Vec<PathBuf> {
        match provider {
            Provider::ClaudeCode => vec![self.claude_root()],
            Provider::Codex => {
                let root = self.codex_root();
                vec![root.join("sessions"), root.join("archived_sessions")]
            }
            _ => Vec::new(),
        }
    }
}

/// A file's size and modification time: a log file is rewritten only by being appended to, so
/// the two together are enough to know nothing changed.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Stamp {
    pub size: u64,
    pub modified: f64,
}

impl Stamp {
    fn of(metadata: &std::fs::Metadata) -> Option<Stamp> {
        let modified = metadata.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_secs_f64();
        Some(Stamp { size: metadata.len(), modified })
    }
}

/// Every `.jsonl` file under the roots, skipping dot-folders and dot-files.
pub fn list_transcripts(roots: &[PathBuf]) -> Vec<(PathBuf, Stamp)> {
    let mut found = Vec::new();
    let mut pending: Vec<PathBuf> = roots.iter().rev().cloned().collect();
    while let Some(directory) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else { continue };
        for entry in entries.flatten() {
            let name = entry.file_name();
            if name.to_string_lossy().starts_with('.') {
                continue;
            }
            let Ok(kind) = entry.file_type() else { continue };
            let path = entry.path();
            if kind.is_dir() {
                pending.push(path);
            } else if kind.is_file() && path.extension().is_some_and(|e| e == "jsonl") {
                if let Some(stamp) = entry.metadata().ok().as_ref().and_then(Stamp::of) {
                    found.push((path, stamp));
                }
            }
        }
    }
    found
}

// MARK: - What a scan holds

/// One Claude Code reply: where it landed, on which model, what it cost in tokens, and how long
/// it took where that could be timed.
///
/// Kept by message id because a reply can be in more than one file. Resuming or forking a
/// conversation starts a new transcript that opens with a copy of the old one's history: the
/// same message ids at the same times. Added up file by file, every resumed session counted its
/// history again (28% of the tokens counted on the machine upstream found this on were copies).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScannedReply {
    pub slot: String,
    pub model: String,
    pub tally: TokenTally,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timing: Option<ReplyTiming>,
    /// A Codex reading's running total. Every counted reading claims it; a reading from a
    /// forked rollout is dropped when an earlier file already did, because it is the parent's
    /// request played back. Only a fork's readings are ever dropped this way: two unrelated
    /// sessions can reach the same small total.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub running_total: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_fork: Option<bool>,
}

/// What a transcript says about itself: what it was called and where it ran, beside the counts.
/// Both come out of the same pass and are cached with it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Scanned {
    pub days: Buckets,
    /// The conversation's own name, where the CLI keeps one.
    pub title: Option<String>,
    /// The directory it ran in, as the transcript states it. Not decoded from the folder name:
    /// Claude Code replaces every separator with a dash, which cannot be reversed.
    pub cwd: Option<String>,
    /// A Codex session that opens with a review request instead of the user's words.
    pub is_review: bool,
    /// Timed replies by quarter-hour and then model.
    pub timings: Timings,
    /// Claude Code's replies by message id, and a Codex fork's readings by the running total
    /// they reached, kept apart from `days` so that `scan` can count each once across files.
    pub replies: HashMap<String, ScannedReply>,
    /// The running totals a Codex session that is not a fork reached, which its forks' replayed
    /// readings repeat.
    pub running_totals: Vec<String>,
}

impl Scanned {
    /// `days` with the replies folded in: one file's own figures.
    pub fn all_days(&self) -> Buckets {
        let mut days = self.days.clone();
        for reply in self.replies.values() {
            *days.entry(reply.slot.clone()).or_default().entry(reply.model.clone()).or_default() += &reply.tally;
        }
        days
    }

    /// `timings` with the replies' own folded in.
    pub fn all_timings(&self) -> Timings {
        let mut timings = self.timings.clone();
        for reply in self.replies.values() {
            let Some(timing) = reply.timing else { continue };
            let entry = timings.entry(reply.slot.clone()).or_default().entry(reply.model.clone()).or_default();
            *entry = *entry + timing;
        }
        timings
    }
}

// MARK: - Line helpers

fn contains(line: &[u8], needle: &[u8]) -> bool {
    memchr::memmem::find(line, needle).is_some()
}

fn int(value: Option<&Value>) -> i64 {
    match value {
        Some(Value::Number(n)) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)).unwrap_or(0),
        _ => 0,
    }
}

fn parse_iso(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text).ok().map(|d| d.with_timezone(&Utc))
}

/// The first `"timestamp":"..."` value in a line, cut out without parsing the rest of it.
fn timestamp_in(line: &[u8]) -> Option<DateTime<Utc>> {
    const KEY: &[u8] = b"\"timestamp\":\"";
    let start = memchr::memmem::find(line, KEY)? + KEY.len();
    let length = memchr::memchr(b'"', &line[start..])?;
    std::str::from_utf8(&line[start..start + length]).ok().and_then(parse_iso)
}

/// The quarter-hour key of consecutive timestamps, remembered: most lines fall in the one before.
struct SlotMemo {
    calendar: Calendar,
    quarter: i64,
    key: String,
}

impl SlotMemo {
    fn new(calendar: &Calendar) -> Self {
        Self { calendar: calendar.clone(), quarter: i64::MIN, key: String::new() }
    }

    fn key(&mut self, at: DateTime<Utc>) -> String {
        let quarter = at.timestamp().div_euclid(15 * 60);
        if quarter != self.quarter {
            self.quarter = quarter;
            self.key = slot_key(at, &self.calendar);
        }
        self.key.clone()
    }
}

fn add_to(buckets: &mut Buckets, slot: &str, model: &str, tally: &TokenTally) {
    match buckets.get_mut(slot) {
        Some(models) => match models.get_mut(model) {
            Some(existing) => *existing += tally,
            None => {
                models.insert(model.to_string(), tally.clone());
            }
        },
        None => {
            buckets.insert(slot.to_string(), HashMap::from([(model.to_string(), tally.clone())]));
        }
    }
}

fn add_timing(timings: &mut Timings, slot: &str, model: &str, timing: ReplyTiming) {
    let entry = timings.entry(slot.to_string()).or_default().entry(model.to_string()).or_default();
    *entry = *entry + timing;
}

fn seconds_between(from: DateTime<Utc>, to: DateTime<Utc>) -> f64 {
    (to - from).num_microseconds().map_or_else(|| (to - from).num_milliseconds() as f64 / 1000.0, |us| us as f64 / 1_000_000.0)
}

// MARK: - Claude Code

/// Times Claude Code's replies as their lines go by: from the last line that sent a request to
/// the last line of the reply. Not before the previous reply finished: a tool result can land
/// between two lines of the reply that asked for it; the next request still waits for the reply
/// to end.
#[derive(Default)]
struct ReplyClock {
    /// By message id, so the timing travels with its reply.
    timings: HashMap<String, ReplyTiming>,
    pending: Option<DateTime<Utc>>,
    open: Option<OpenReply>,
    last_finished: Option<DateTime<Utc>>,
    done: HashSet<String>,
}

struct OpenReply {
    id: String,
    sent: Option<DateTime<Utc>>,
    finished: DateTime<Utc>,
    output: i64,
}

impl ReplyClock {
    fn sent(&mut self, at: DateTime<Utc>) {
        self.pending = Some(at);
    }

    fn reply(&mut self, id: &str, output: i64, at: DateTime<Utc>) {
        if let Some(open) = self.open.as_mut().filter(|o| o.id == id) {
            open.finished = at;
            open.output = output;
            return;
        }
        self.finish();
        // A reply seen again after another (a resumed session's copy) was timed the first time.
        if self.done.contains(id) {
            return;
        }
        let sent = self.pending.map(|start| self.last_finished.map_or(start, |last| last.max(start)));
        self.pending = None;
        self.open = Some(OpenReply { id: id.to_string(), sent, finished: at, output });
    }

    fn finish(&mut self) {
        let Some(reply) = self.open.take() else { return };
        self.done.insert(reply.id.clone());
        self.last_finished = Some(reply.finished);
        let Some(sent) = reply.sent else { return };
        if let Some(timing) = ReplyTiming::reply(reply.output, seconds_between(sent, reply.finished)) {
            self.timings.insert(reply.id, timing);
        }
    }
}

#[derive(Deserialize)]
struct UsageLine {
    #[serde(rename = "type")]
    kind: Option<String>,
    timestamp: Option<String>,
    message: Option<UsageMessage>,
}

#[derive(Deserialize)]
struct UsageMessage {
    id: Option<Value>,
    model: Option<String>,
    usage: Option<Value>,
}

struct ClaudeParser {
    scanned: Scanned,
    days: Buckets,
    replies: HashMap<String, ScannedReply>,
    clock: ReplyClock,
    slots: SlotMemo,
}

impl ClaudeParser {
    fn new(calendar: &Calendar) -> Self {
        Self {
            scanned: Scanned::default(),
            days: Buckets::new(),
            replies: HashMap::new(),
            clock: ReplyClock::default(),
            slots: SlotMemo::new(calendar),
        }
    }

    fn line(&mut self, line: &[u8]) {
        // A request goes out on the user's message or the last tool result. Only the time is
        // wanted, and a tool result can be a whole file, so it is cut out of the line rather
        // than parsed. The top-level `type` and `timestamp` are the only ones a user line has
        // unescaped.
        let is_user = contains(line, b"\"type\":\"user\"");
        if is_user {
            if let Some(sent) = timestamp_in(line) {
                self.clock.sent(sent);
            }
        }

        // What the session is called and where it ran. A user-set title can arrive long after
        // the opening prompt (Claude Code writes `customTitle` when the conversation is
        // renamed), so that one is looked for on every line and the last valid one wins. `cwd`
        // and the opening prompt are only needed once each, which keeps the ordinary line from
        // being parsed twice.
        let renamed = contains(line, b"\"customTitle\"");
        let wants_cwd = self.scanned.cwd.is_none() && contains(line, b"\"cwd\"");
        let wants_title = self.scanned.title.is_none() && is_user;
        if renamed || wants_cwd || wants_title {
            if let Ok(Value::Object(root)) = serde_json::from_slice::<Value>(line) {
                if self.scanned.cwd.is_none() {
                    if let Some(cwd) = root.get("cwd").and_then(Value::as_str).filter(|c| !c.is_empty()) {
                        self.scanned.cwd = Some(cwd.to_string());
                    }
                }
                // The user's own name outranks the opening prompt, and a later rename outranks
                // an earlier one. An unreadable custom title (empty, or an envelope) leaves the
                // title that was already found rather than clearing it.
                let custom = if renamed {
                    root.get("customTitle").and_then(Value::as_str).and_then(title_from)
                } else {
                    None
                };
                if let Some(title) = custom {
                    self.scanned.title = Some(title);
                } else if self.scanned.title.is_none()
                    && root.get("type").and_then(Value::as_str) == Some("user")
                    && root.get("isSidechain").and_then(Value::as_bool) != Some(true)
                {
                    if let Some(title) = root
                        .get("message")
                        .and_then(Value::as_object)
                        .and_then(|message| text_in(message.get("content")))
                        .and_then(|text| title_from(&text))
                    {
                        self.scanned.title = Some(title);
                    }
                }
            }
        }

        if !contains(line, b"\"usage\"") {
            return;
        }
        let Ok(parsed) = serde_json::from_slice::<UsageLine>(line) else { return };
        let (Some("assistant"), Some(timestamp), Some(message)) = (parsed.kind.as_deref(), parsed.timestamp, parsed.message)
        else {
            return;
        };
        let (Some(Value::Object(usage)), Some(model)) = (message.usage, message.model) else { return };
        // Placeholders Claude Code writes for its own errors; no request was made, so there is
        // nothing to price.
        if model == "<synthetic>" {
            return;
        }
        let Some(at) = parse_iso(&timestamp) else { return };
        let slot = self.slots.key(at);

        let written = int(usage.get("cache_creation_input_tokens"));
        let tally = TokenTally {
            input: int(usage.get("input_tokens")),
            cache_write: written,
            cache_read: int(usage.get("cache_read_input_tokens")),
            output: int(usage.get("output_tokens")),
            cache_write_1h: int(usage.get("cache_creation").and_then(|c| c.get("ephemeral_1h_input_tokens"))).min(written),
            replies_without_cache_fields: i64::from(
                !usage.contains_key("cache_read_input_tokens") && !usage.contains_key("cache_creation_input_tokens"),
            ),
            context_bands: HashMap::new(),
        }
        // One reply is one request; all it was sent is its context.
        .request(int(usage.get("input_tokens")) + written + int(usage.get("cache_read_input_tokens")));

        let Some(id) = message.id.as_ref().and_then(Value::as_str) else {
            if tally.total() > 0 {
                add_to(&mut self.days, &slot, &model, &tally);
            }
            return;
        };
        // Placed at its first line, counted at its last. A reply is written over several lines,
        // one per content block, each carrying the usage so far; the output count climbs to the
        // last. Keeping the first line's missed about a quarter of the output.
        let placed = self.replies.get(id).map_or(slot, |r| r.slot.clone());
        let output = tally.output;
        self.clock.reply(id, output, at);
        self.replies.insert(
            id.to_string(),
            ScannedReply { slot: placed, model, tally, timing: None, running_total: None, from_fork: None },
        );
    }

    fn finish(mut self) -> Scanned {
        self.clock.finish();
        for (id, timing) in std::mem::take(&mut self.clock.timings) {
            if let Some(reply) = self.replies.get_mut(&id) {
                reply.timing = Some(timing);
            }
        }
        self.scanned.days = self.days;
        self.scanned.replies = self.replies.into_iter().filter(|(_, r)| r.tally.total() > 0).collect();
        self.scanned
    }
}

/// Claude Code writes one JSON object per message, each assistant reply carrying the token
/// counts for the request that produced it.
pub fn parse_claude_code(data: &[u8], calendar: &Calendar) -> Scanned {
    let mut parser = ClaudeParser::new(calendar);
    for_each_line_in(data, |line| parser.line(line));
    parser.finish()
}

pub fn parse_claude_code_file(path: &Path, calendar: &Calendar) -> Scanned {
    let mut parser = ClaudeParser::new(calendar);
    for_each_line(path, |line| parser.line(line));
    parser.finish()
}

// MARK: - Codex

/// What the model writes in a Codex rollout, as opposed to what is handed to it.
/// `"function_call"` with its closing quote is not `"function_call_output"`.
const CODEX_MODEL_ITEMS: [&[u8]; 6] = [
    b"\"role\":\"assistant\"",
    b"\"type\":\"reasoning\"",
    b"\"type\":\"function_call\"",
    b"\"type\":\"custom_tool_call\"",
    b"\"type\":\"web_search_call\"",
    b"\"type\":\"local_shell_call\"",
];

/// Times Codex's replies: from the request to the model's last item.
///
/// The count arrives late: Codex writes a reply's `token_count` once the tool it asked for has
/// run, after that tool's output, which is also the next request going out. So a request closes
/// the reply before it, and the count that follows is paired with that reply.
#[derive(Default)]
struct CodexReplyClock {
    timings: Timings,
    sent: Option<DateTime<Utc>>,
    wrote: Option<DateTime<Utc>>,
    closed: Option<(DateTime<Utc>, DateTime<Utc>)>,
}

impl CodexReplyClock {
    fn sent(&mut self, at: DateTime<Utc>) {
        if let (Some(sent), Some(wrote)) = (self.sent, self.wrote) {
            if wrote > sent {
                self.closed = Some((sent, wrote));
            }
        }
        self.sent = Some(at);
        self.wrote = None;
    }

    fn wrote(&mut self, at: DateTime<Utc>) {
        if self.sent.is_some() {
            self.wrote = Some(at);
        }
    }

    fn first_token(&mut self, after_seconds: f64, slot: &str, model: &str) {
        if let Some(timing) = ReplyTiming::first_token(after_seconds) {
            add_timing(&mut self.timings, slot, model, timing);
        }
    }

    fn counted(&mut self, output: i64, slot: &str, model: &str) {
        let mut span = self.closed.take();
        if span.is_none() {
            if let (Some(sent), Some(wrote)) = (self.sent, self.wrote) {
                if wrote > sent {
                    span = Some((sent, wrote));
                    self.sent = None;
                    self.wrote = None;
                }
            }
        }
        let Some((sent, finished)) = span else { return };
        if let Some(timing) = ReplyTiming::reply(output, seconds_between(sent, finished)) {
            add_timing(&mut self.timings, slot, model, timing);
        }
    }
}

#[derive(Clone, Copy, Default)]
struct CodexCounts {
    input: i64,
    cached: i64,
    cache_write: i64,
    output: i64,
}

impl CodexCounts {
    fn of(usage: &Value) -> Self {
        Self {
            input: int(usage.get("input_tokens")),
            cached: int(usage.get("cached_input_tokens")),
            cache_write: int(usage.get("cache_write_input_tokens")),
            output: int(usage.get("output_tokens")),
        }
    }

    fn zip(self, other: Self, f: impl Fn(i64, i64) -> i64) -> Self {
        Self {
            input: f(self.input, other.input),
            cached: f(self.cached, other.cached),
            cache_write: f(self.cache_write, other.cache_write),
            output: f(self.output, other.output),
        }
    }
}

#[derive(Deserialize)]
struct CodexLine {
    timestamp: Option<String>,
    payload: Option<CodexPayload>,
}

#[derive(Deserialize)]
struct CodexPayload {
    model: Option<Value>,
    #[serde(rename = "type")]
    kind: Option<Value>,
    info: Option<Value>,
}

static ANONYMOUS_SESSIONS: AtomicU64 = AtomicU64::new(0);

struct CodexParser {
    scanned: Scanned,
    days: Buckets,
    replies: HashMap<String, ScannedReply>,
    running_totals: Vec<String>,
    model: Option<String>,
    previous: Option<CodexCounts>,
    clock: CodexReplyClock,
    last_count_slot: Option<String>,
    forked: bool,
    replaying: bool,
    /// Which session a reading belongs to. A rollout with no header gets one of its own, so it
    /// is never taken for another file's.
    session: String,
    headed: bool,
    slots: SlotMemo,
}

impl CodexParser {
    fn new(calendar: &Calendar) -> Self {
        Self {
            scanned: Scanned::default(),
            days: Buckets::new(),
            replies: HashMap::new(),
            running_totals: Vec::new(),
            model: None,
            previous: None,
            clock: CodexReplyClock::default(),
            last_count_slot: None,
            forked: false,
            replaying: false,
            session: format!("anonymous-{}-{}", std::process::id(), ANONYMOUS_SESSIONS.fetch_add(1, Ordering::Relaxed)),
            headed: false,
            slots: SlotMemo::new(calendar),
        }
    }

    fn line(&mut self, line: &[u8]) {
        // The rollout's own header is its first; any later one is history.
        if !self.headed && contains(line, b"\"session_meta\"") {
            if let Ok(root) = serde_json::from_slice::<Value>(line) {
                if root.get("type").and_then(Value::as_str) == Some("session_meta") {
                    if let Some(payload) = root.get("payload").filter(|p| p.is_object()) {
                        self.headed = true;
                        if let Some(id) = payload.get("id").and_then(Value::as_str).filter(|s| !s.is_empty()) {
                            self.session = id.to_string();
                        }
                        if payload.get("forked_from_id").and_then(Value::as_str).is_some_and(|s| !s.is_empty()) {
                            self.forked = true;
                        }
                    }
                }
            }
        }

        // A turn begins; in a fork, the one Codex calls `rollout-N` is the parent's history
        // played back.
        if contains(line, b"\"task_started\"") {
            if let Ok(root) = serde_json::from_slice::<Value>(line) {
                if let Some(payload) = root.get("payload").filter(|p| p.get("type").and_then(Value::as_str) == Some("task_started")) {
                    let turn = payload.get("turn_id").and_then(Value::as_str).unwrap_or("");
                    self.replaying = self.forked && turn.starts_with("rollout-");
                    return;
                }
            }
        }

        // What a reply's timing needs: when each request went out (a tool's output, or the
        // user's message) and when the model last wrote. Cut out of the line, as Claude Code's
        // are: a tool's output can be long.
        if contains(line, b"\"type\":\"response_item\"") {
            if let Some(at) = timestamp_in(line) {
                if contains(line, b"_call_output\"") || contains(line, b"\"role\":\"user\"") {
                    self.clock.sent(at);
                } else if CODEX_MODEL_ITEMS.iter().any(|item| contains(line, item)) {
                    self.clock.wrote(at);
                }
            }
        }

        if (self.scanned.title.is_none() && !self.scanned.is_review) || self.scanned.cwd.is_none() {
            // The directory is stated once in the session header; the opening prompt is a
            // `response_item` whose payload is a message with the user's role on it.
            if contains(line, b"\"cwd\"") || contains(line, b"\"role\":\"user\"") {
                if let Ok(root) = serde_json::from_slice::<Value>(line) {
                    if let Some(payload) = root.get("payload").filter(|p| p.is_object()) {
                        if self.scanned.cwd.is_none() {
                            if let Some(cwd) = payload.get("cwd").and_then(Value::as_str).filter(|c| !c.is_empty()) {
                                self.scanned.cwd = Some(cwd.to_string());
                            }
                        }
                        if self.scanned.title.is_none()
                            && !self.scanned.is_review
                            && payload.get("type").and_then(Value::as_str) == Some("message")
                            && payload.get("role").and_then(Value::as_str) == Some("user")
                        {
                            // A review session's first words are the review request: it is
                            // marked, and has no title.
                            if is_codex_review(payload.get("content")) {
                                self.scanned.is_review = true;
                            } else if let Some(title) = codex_title(payload.get("content")) {
                                self.scanned.title = Some(title);
                            }
                        }
                    }
                }
            }
        }

        // Codex's own measure of the wait for the first token, once per turn, put down to the
        // model in force.
        if contains(line, b"\"task_complete\"") {
            if let Ok(root) = serde_json::from_slice::<Value>(line) {
                if let Some(payload) = root.get("payload").filter(|p| p.get("type").and_then(Value::as_str) == Some("task_complete")) {
                    if let (Some(wait), Some(model)) = (payload.get("time_to_first_token_ms").and_then(Value::as_f64), self.model.clone()) {
                        // Put with the turn's last count, so it lands in a quarter-hour that
                        // has the turn's tokens: slots hold only work.
                        let slot = self.last_count_slot.clone().or_else(|| {
                            root.get("timestamp").and_then(Value::as_str).and_then(parse_iso).map(|at| self.slots.key(at))
                        });
                        if let Some(slot) = slot {
                            self.clock.first_token(wait / 1000.0, &slot, &model);
                        }
                        return;
                    }
                }
            }
        }

        let is_count = contains(line, b"\"token_count\"");
        if !is_count && !contains(line, b"\"model\"") {
            return;
        }
        let Ok(parsed) = serde_json::from_slice::<CodexLine>(line) else { return };
        let payload = parsed.payload.unwrap_or(CodexPayload { model: None, kind: None, info: None });

        // The model can change mid-session; usage is attributed to whichever was in force when
        // the reading was taken.
        if let Some(named) = payload.model.as_ref().and_then(Value::as_str) {
            self.model = Some(named.to_string());
        }

        if !is_count || payload.kind.as_ref().and_then(Value::as_str) != Some("token_count") {
            return;
        }
        let info = payload.info.as_ref();
        let Some(totals) = info.and_then(|i| i.get("total_token_usage")).filter(|t| t.is_object()) else { return };
        let Some(timestamp) = parsed.timestamp else { return };
        let Some(at) = parse_iso(&timestamp) else { return };
        let Some(model) = self.model.clone() else { return };
        let slot = self.slots.key(at);

        let current = CodexCounts::of(totals);
        let last_usage = info.and_then(|i| i.get("last_token_usage")).filter(|l| l.is_object());
        // A fork's first reading is its parent's total plus its own first request; only the
        // request is the fork's.
        if self.previous.is_none() && self.forked {
            let last = last_usage.map(CodexCounts::of);
            self.previous = Some(match last {
                Some(last) => current.zip(last, |total, own| (total - own).max(0)),
                None => current,
            });
        }
        let delta = current.zip(self.previous.unwrap_or_default(), |now, before| (now - before).max(0));
        self.previous = Some(current);
        if self.replaying {
            return;
        }

        // Codex counts cached tokens inside its input figure; the price list treats them as two
        // separate rates.
        let tally = TokenTally {
            input: (delta.input - delta.cached).max(0),
            cache_write: delta.cache_write,
            cache_read: delta.cached,
            output: delta.output,
            ..TokenTally::default()
        }
        // A reading is one request, and its own input (cached part included) is how full that
        // request's context was.
        .request(int(last_usage.and_then(|l| l.get("input_tokens"))));
        if tally.total() == 0 {
            return;
        }

        // The running total this reading brought the session to: a copy of it in a fork's
        // rollout is the parent's request.
        let total = format!("{}:{}:{}:{}", current.input, current.cached, current.output, int(totals.get("total_tokens")));
        // Only a fork's readings can be copies, so only a fork keeps them one by one; any other
        // session adds its readings up and keeps just the totals they reached, for its forks to
        // be checked against.
        if self.forked {
            let id = format!("codex:{}:{}", self.session, total);
            if self.replies.contains_key(&id) {
                return;
            }
            self.replies.insert(
                id,
                ScannedReply {
                    slot: slot.clone(),
                    model: model.clone(),
                    tally: tally.clone(),
                    timing: None,
                    running_total: Some(total),
                    from_fork: Some(true),
                },
            );
        } else {
            add_to(&mut self.days, &slot, &model, &tally);
            self.running_totals.push(total);
        }
        self.clock.counted(tally.output, &slot, &model);
        self.last_count_slot = Some(slot);
    }

    fn finish(mut self) -> Scanned {
        self.scanned.days = self.days;
        self.scanned.replies = self.replies;
        self.scanned.running_totals = self.running_totals;
        self.scanned.timings = self.clock.timings;
        self.scanned
    }
}

/// Codex reports a running total for the session rather than a figure per turn, so each reading
/// is differenced against the one before it. The running total only ever climbs, which makes the
/// differences safe to add up, and it sidesteps the duplicate readings that summing Codex's own
/// per-turn field would double-count.
///
/// A forked session opens with its parent's running total. Codex Desktop's sub-agents start a new
/// rollout (`session_meta.forked_from_id`) whose first reading is the parent's whole total so
/// far, and whose first turn (`task_started` with a `rollout-N` id rather than a UUID) replays
/// the parent's readings one by one. So a fork's first reading counts only its own request, its
/// replayed turn counts nothing, and every counted reading is kept by the running total it
/// brought the session to so `scan` counts a reading copied into another file once.
pub fn parse_codex(data: &[u8], calendar: &Calendar) -> Scanned {
    let mut parser = CodexParser::new(calendar);
    for_each_line_in(data, |line| parser.line(line));
    parser.finish()
}

pub fn parse_codex_file(path: &Path, calendar: &Calendar) -> Scanned {
    let mut parser = CodexParser::new(calendar);
    for_each_line(path, |line| parser.line(line));
    parser.finish()
}

fn parse_file(path: &Path, provider: Provider, calendar: &Calendar) -> Scanned {
    match provider {
        Provider::ClaudeCode => parse_claude_code_file(path, calendar),
        Provider::Codex => parse_codex_file(path, calendar),
        _ => Scanned::default(),
    }
}

// MARK: - The per-file cache

/// What has already been counted, so opening settings a second time doesn't re-read a few
/// hundred megabytes of transcripts. Keyed on path; valid while size and modification time hold.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileCache {
    pub files: HashMap<String, Entry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub stamp: Stamp,
    pub days: Buckets,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_review: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timings: Option<Timings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replies: Option<HashMap<String, ScannedReply>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub running_totals: Option<Vec<String>>,
}

impl Entry {
    /// A Codex rollout forked from another session.
    fn is_fork(&self) -> bool {
        self.replies.as_ref().is_some_and(|r| r.values().any(|reply| reply.from_fork == Some(true)))
    }

    /// The earliest quarter-hour with work in it.
    fn first_slot(&self) -> Option<&str> {
        let day = self.days.keys().min().map(String::as_str);
        let reply = self.replies.as_ref().and_then(|r| r.values().map(|reply| reply.slot.as_str()).min());
        match (day, reply) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }
}

impl FileCache {
    pub fn file_name(provider: Provider) -> String {
        format!("ledger-{CACHE_VERSION}-{}.json", provider.raw())
    }

    pub fn load(provider: Provider, directory: &Path) -> FileCache {
        std::fs::read(directory.join(Self::file_name(provider)))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, provider: Provider, directory: &Path) {
        if std::fs::create_dir_all(directory).is_err() {
            return;
        }
        if let Ok(bytes) = serde_json::to_vec(self) {
            super::prices::write_atomically(&directory.join(Self::file_name(provider)), &bytes);
        }
    }
}

// MARK: - The reader

/// One scan: every buckets-by-quarter-hour total, the timings, and each file's counted entry.
pub struct Scan {
    pub buckets: Buckets,
    pub timings: Timings,
    pub files: HashMap<String, Entry>,
}

/// Reads one provider's transcripts, with the per-file cache.
pub struct TranscriptReader {
    pub sources: Sources,
    pub cache_directory: PathBuf,
    pub calendar: Calendar,
}

impl TranscriptReader {
    pub fn new(sources: Sources, cache_directory: PathBuf, calendar: Calendar) -> Self {
        Self { sources, cache_directory, calendar }
    }

    /// The provider's ledger, priced afresh from the cached raw tokens.
    pub fn ledger(&self, provider: Provider, prices: &PriceTable) -> Ledger {
        let scanned = self.scan(provider);
        let mut ledger = price_buckets(&scanned.buckets, prices, &self.calendar, None, &scanned.timings);
        ledger.sessions = self.sessions(&scanned.files, provider, prices);
        ledger
    }

    /// Parses what changed, counts each reply once across files, and writes the cache when a
    /// file was added, changed or removed.
    pub fn scan(&self, provider: Provider) -> Scan {
        let mut cache = FileCache::load(provider, &self.cache_directory);
        let mut fresh: HashMap<String, Entry> = HashMap::new();
        let mut stale: Vec<(String, PathBuf, Stamp)> = Vec::new();
        let mut changed = false;

        for (path, stamp) in list_transcripts(&self.sources.roots(provider)) {
            let key = path.to_string_lossy().into_owned();
            match cache.files.remove(&key) {
                Some(known) if known.stamp == stamp => {
                    fresh.insert(key, known);
                }
                _ => stale.push((key, path, stamp)),
            }
        }
        if !stale.is_empty() {
            changed = true;
            for (key, entry) in self.parse_all(provider, stale) {
                fresh.insert(key, entry);
            }
        }

        // Each reply once, for the file it appeared in first: the original conversation, not a
        // resumed or forked copy of its history. Files are taken oldest work first, then by
        // name, then by path, so the choice is stable. A Codex fork goes after every session
        // that is not one: it can open in the quarter-hour its parent did, and its parent's
        // readings must already be counted when its copies of them come by.
        let mut ordered: Vec<(&String, &Entry, String, bool)> = fresh
            .iter()
            .map(|(key, entry)| (key, entry, entry.first_slot().unwrap_or("").to_string(), entry.is_fork()))
            .collect();
        let file_name = |key: &str| Path::new(key).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        ordered.sort_by(|a, b| {
            a.3.cmp(&b.3)
                .then_with(|| a.2.cmp(&b.2))
                .then_with(|| file_name(a.0).cmp(&file_name(b.0)))
                .then_with(|| a.0.cmp(b.0))
        });

        let mut buckets = Buckets::new();
        let mut timings = Timings::new();
        let mut claimed: HashSet<&str> = HashSet::new();
        let mut totals: HashSet<&str> = HashSet::new();
        let mut counted: HashMap<String, Entry> = HashMap::new();
        for (key, entry, _, fork) in ordered {
            let mut days = entry.days.clone();
            // A fork's lines carry the instant it was made, not when its work ran, and its
            // replayed readings are dropped below: its timings would time nothing real.
            let mut file_timings: Timings = if fork { Timings::new() } else { entry.timings.clone().unwrap_or_default() };
            // An original session's totals are claimed before any fork is read: the forks come
            // after every file that is not one.
            if let Some(running) = &entry.running_totals {
                totals.extend(running.iter().map(String::as_str));
            }
            if let Some(replies) = &entry.replies {
                for (id, reply) in replies {
                    if reply.from_fork == Some(true) {
                        if let Some(total) = &reply.running_total {
                            if totals.contains(total.as_str()) {
                                continue;
                            }
                        }
                    }
                    if !claimed.insert(id.as_str()) {
                        continue;
                    }
                    if let Some(total) = &reply.running_total {
                        totals.insert(total.as_str());
                    }
                    add_to(&mut days, &reply.slot, &reply.model, &reply.tally);
                    if let Some(timing) = reply.timing {
                        add_timing(&mut file_timings, &reply.slot, &reply.model, timing);
                    }
                }
            }
            counted.insert(
                key.clone(),
                Entry {
                    stamp: entry.stamp,
                    days: days.clone(),
                    title: entry.title.clone(),
                    cwd: entry.cwd.clone(),
                    is_review: entry.is_review,
                    timings: Some(file_timings.clone()),
                    replies: None,
                    running_totals: None,
                },
            );
            for (slot, models) in &days {
                for (model, tally) in models {
                    add_to(&mut buckets, slot, model, tally);
                }
            }
            for (slot, models) in &file_timings {
                for (model, timing) in models {
                    add_timing(&mut timings, slot, model, *timing);
                }
            }
        }

        // Entries left in the old cache are deleted files. Repricing unchanged transcripts does
        // not change this raw-token cache or warrant a write.
        if changed || !cache.files.is_empty() {
            cache.files = fresh;
            cache.save(provider, &self.cache_directory);
        }
        // The sessions are cut from the counted entries, so a resumed conversation's row holds
        // only what was done in it.
        Scan { buckets, timings, files: counted }
    }

    /// Parses the files that changed, several at a time.
    fn parse_all(&self, provider: Provider, stale: Vec<(String, PathBuf, Stamp)>) -> Vec<(String, Entry)> {
        let workers = std::thread::available_parallelism().map_or(2, usize::from).clamp(1, 8).min(stale.len());
        let next = AtomicUsize::new(0);
        let results: Mutex<Vec<(String, Entry)>> = Mutex::new(Vec::with_capacity(stale.len()));
        let work = || loop {
            let index = next.fetch_add(1, Ordering::Relaxed);
            let Some((key, path, stamp)) = stale.get(index) else { break };
            let scanned = parse_file(path, provider, &self.calendar);
            let entry = Entry {
                stamp: *stamp,
                days: scanned.days,
                title: scanned.title,
                cwd: scanned.cwd,
                is_review: scanned.is_review.then_some(true),
                timings: Some(scanned.timings),
                replies: Some(scanned.replies),
                running_totals: (!scanned.running_totals.is_empty()).then_some(scanned.running_totals),
            };
            results.lock().expect("scan results").push((key.clone(), entry));
        };
        if workers <= 1 {
            work();
        } else {
            std::thread::scope(|scope| {
                for _ in 0..workers {
                    scope.spawn(work);
                }
            });
        }
        results.into_inner().expect("scan results")
    }

    /// One row per conversation, priced the same way the days are.
    ///
    /// A conversation can be several files. Claude Code writes each subagent's transcript beside
    /// its parent's (`session_file_of`); those files' tokens are real and are in the days,
    /// models and projects like any other, but as rows they were a "Pulse" session apiece. They
    /// are folded into the parent's row here (summed, the span widened) after `scan` has counted
    /// every reply once, so nothing is counted twice.
    fn sessions(&self, files: &HashMap<String, Entry>, provider: Provider, prices: &PriceTable) -> Vec<Session> {
        #[derive(Default)]
        struct Rollup {
            tokens: i64,
            cost: f64,
            unpriced: i64,
            start: Option<DateTime<Utc>>,
            end: Option<DateTime<Utc>>,
            /// By quarter-hour key: tokens, cost, unpriced.
            slots: HashMap<String, (i64, f64, i64)>,
            /// Where the conversation ran and what it was called: the parent's own when it has
            /// them, else the first subagent's (by path, so the choice is stable).
            title: Option<String>,
            cwd: Option<String>,
            is_review: bool,
            parent_seen: bool,
        }

        let mut rollups: HashMap<String, Rollup> = HashMap::new();
        let mut lookup = ModelPriceLookup::new(prices);
        let mut sorted: Vec<(&String, &Entry)> = files.iter().collect();
        sorted.sort_by(|a, b| a.0.cmp(b.0));

        for (path, entry) in sorted {
            let normalized = path.replace('\\', "/");
            let group = if provider == Provider::ClaudeCode { session_file_of(&normalized) } else { normalized.clone() };
            let rollup = rollups.entry(group.clone()).or_default();

            for (key, models) in &entry.days {
                let Some(at) = slot_start(key, &self.calendar) else { continue };
                let slot = rollup.slots.entry(key.clone()).or_insert((0, 0.0, 0));
                let before = slot.0;
                for (model, tally) in models {
                    let total = tally.total();
                    rollup.tokens += total;
                    slot.0 += total;
                    if let Some(price) = lookup.price(model, None) {
                        let money = tally.cost_at(price);
                        rollup.cost += money;
                        slot.1 += money;
                    } else {
                        rollup.unpriced += total;
                        slot.2 += total;
                    }
                }
                // A quarter-hour with no tokens in it is not when the conversation ran.
                if slot.0 <= before {
                    continue;
                }
                rollup.start = Some(rollup.start.map_or(at, |s| s.min(at)));
                rollup.end = Some(rollup.end.map_or(at, |e| e.max(at)));
            }
            // A parent whose every reply was counted in another file still names the
            // conversation, so this is not skipped for having no work of its own left.
            if group == normalized {
                rollup.parent_seen = true;
                rollup.title = entry.title.clone().or(rollup.title.take());
                rollup.cwd = entry.cwd.clone().or(rollup.cwd.take());
                rollup.is_review = entry.is_review == Some(true);
            } else if !rollup.parent_seen {
                rollup.title = rollup.title.take().or_else(|| entry.title.clone());
                rollup.cwd = rollup.cwd.take().or_else(|| entry.cwd.clone());
            }
        }

        let mut sessions: Vec<Session> = Vec::new();
        for (path, rollup) in rollups {
            let (true, Some(start), Some(end)) = (rollup.tokens > 0, rollup.start, rollup.end) else { continue };
            let name = Path::new(&path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            sessions.push(Session {
                project: UsageProject::new(rollup.cwd.as_deref()).or_else(|| project_of(&path, provider)),
                slots: session_slots(&rollup.slots, &self.calendar),
                id: path,
                name,
                title: rollup.title,
                is_review: rollup.is_review,
                start,
                end,
                tokens: rollup.tokens,
                cost: rollup.cost,
                unpriced_tokens: rollup.unpriced,
                days: Vec::new(),
            });
        }
        sessions.sort_by(|a, b| b.end.cmp(&a.end).then_with(|| a.id.cmp(&b.id)));
        sessions
    }
}

/// The transcript a Claude Code file belongs to: its own path, or for a subagent's
/// (`<project>/<session>/subagents/agent-<id>.jsonl`) the parent's, `<project>/<session>.jsonl`,
/// whether or not that file is still there. A string rule, like the rest of the grouping; paths
/// use `/`.
pub fn session_file_of(path: &str) -> String {
    match path.find("/subagents/") {
        Some(at) if at > 0 && path.ends_with(".jsonl") => format!("{}.jsonl", &path[..at]),
        _ => path.to_string(),
    }
}

/// The fallback for a transcript that states no `cwd`.
///
/// Claude Code names a project's directory for its path with every separator replaced by a dash
/// (`-Users-me-Code-Pulse`), and the last segment of that is the best guess available. It is a
/// guess, which is why the stated `cwd` is preferred wherever there is one. Codex files sit
/// under a date and carry no directory in the path at all.
pub fn project_of(file: &str, provider: Provider) -> Option<UsageProject> {
    if provider != Provider::ClaudeCode {
        return None;
    }
    let normalized = file.replace('\\', "/");
    let (folder_path, _) = normalized.rsplit_once('/')?;
    let folder = folder_path.rsplit('/').next().unwrap_or(folder_path);
    let last = folder.split('-').filter(|part| !part.is_empty()).next_back()?;
    Some(UsageProject::source(folder_path, last))
}
