// Ported from upstream Sources/Pulse/Usage/PromptCacheLapse.swift and CodexPromptCache.swift.
//! When the prompt cache behind a conversation lapses.
//!
//! Claude Code: read, not assumed. Every reply it logs states which cache tier it wrote to
//! (`cache_creation.ephemeral_1h_input_tokens` or `ephemeral_5m_input_tokens`) and when it was
//! made. A cache lives its tier's length from the last request that used it, since a hit renews
//! it. So the lapse is the last request plus the tier that session last wrote: both figures from
//! the log, nothing from a table. The request's time, not the reply's: a reply is logged when it
//! has finished streaming, and the cache is counted from when the request was made.
//!
//! Codex: its log does not say how long a cache is kept. What makes it readable anyway is
//! OpenAI's own documentation: on GPT-5.6 and later a cached prefix remains eligible for reuse
//! for 30 minutes after its most recent write or reuse, though OpenAI may retain it longer. The
//! log does say which model each turn used. So for those models the lapse is the last request
//! plus the documented thirty minutes, a floor, said as "at least". Earlier models are left out.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::project::{SessionLabel, UsageProject};
use super::transcripts::{project_of, Sources};
use super::titles::{codex_title, is_codex_review, text_in, title_from};
use crate::provider::Provider;

/// When the last request that read or wrote the cache was made, and how long that cache lives.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptCacheLapse {
    pub last_request: DateTime<Utc>,
    /// The tier's length in seconds: an hour or five minutes.
    pub lifetime: f64,
    /// Whether `lifetime` is a floor the provider guarantees rather than the length it keeps a
    /// cache (OpenAI's "at least 30 minutes"). Said as "at least" wherever it is shown, and its
    /// end as "may have lapsed", never "expired".
    pub is_minimum: bool,
    pub expires_at: DateTime<Utc>,
}

impl PromptCacheLapse {
    pub const HOUR: f64 = 3600.0;
    pub const FIVE_MINUTES: f64 = 300.0;
    /// Past this (seconds), a lapsed cache is not worth a line: the session is over.
    pub const STALE_AFTER: f64 = 24.0 * 3600.0;

    pub fn new(last_request: DateTime<Utc>, lifetime: f64) -> Self {
        Self::with_minimum(last_request, lifetime, false)
    }

    pub fn with_minimum(last_request: DateTime<Utc>, lifetime: f64, is_minimum: bool) -> Self {
        Self {
            last_request,
            lifetime,
            is_minimum,
            expires_at: last_request + Duration::milliseconds((lifetime * 1000.0) as i64),
        }
    }

    /// "1 hr", "38 min": whole minutes, rounded up, so the last minute still reads as one rather
    /// than nothing. English short style; the UI localises.
    pub fn duration_text(seconds: f64) -> String {
        let minutes = ((seconds / 60.0).ceil() as i64).max(1);
        if seconds >= 3600.0 {
            let (hours, rest) = (minutes / 60, minutes % 60);
            if rest == 0 {
                format!("{hours} hr")
            } else {
                format!("{hours} hr, {rest} min")
            }
        } else {
            format!("{minutes} min")
        }
    }
}

/// One conversation whose cache Pulse can time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptCacheSession {
    /// The transcript's path.
    pub id: String,
    /// A rename, or the opening prompt cut to a line.
    pub title: Option<String>,
    /// A session Codex ran to review another's action.
    pub is_review: bool,
    /// The directory it ran in, by its last folder.
    pub project: Option<String>,
    pub lapse: PromptCacheLapse,
}

impl PromptCacheSession {
    /// What to call this conversation on a line of its own.
    pub fn display_name(&self) -> String {
        SessionLabel::text(self.title.as_deref(), self.is_review, self.project.as_deref())
    }
}

/// Every conversation still holding a cache, and what to say when none is.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptCacheReading {
    /// Soonest to lapse first.
    pub live: Vec<PromptCacheSession>,
    /// The newest session's lapse, when no session's cache is alive.
    pub last_lapsed: Option<PromptCacheLapse>,
}

impl PromptCacheReading {
    pub fn none() -> Self {
        Self::default()
    }

    /// The providers whose logs let a cache be timed: Claude Code by the tier each reply
    /// records, Codex by the model each turn names.
    pub fn supports(provider: Provider) -> bool {
        matches!(provider, Provider::ClaudeCode | Provider::Codex)
    }

    /// The conversations whose cache is still alive at `now`, soonest first. Filtered again at
    /// read time, so one that lapses while a card is open drops out of the count before the next
    /// read finds it gone.
    pub fn alive(&self, now: DateTime<Utc>) -> Vec<PromptCacheSession> {
        let mut alive: Vec<PromptCacheSession> = self.live.iter().filter(|s| s.lapse.expires_at > now).cloned().collect();
        alive.sort_by_key(|s| s.lapse.expires_at);
        alive
    }
}

/// A read for one of the supported providers; empty for any other.
pub fn read_for(provider: Provider, sources: &Sources, now: DateTime<Utc>) -> PromptCacheReading {
    match provider {
        Provider::ClaudeCode => ClaudePromptCache::read(sources, now),
        Provider::Codex => CodexPromptCache::read(sources, now),
        _ => PromptCacheReading::none(),
    }
}

fn parse_iso(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text).ok().map(|d| d.with_timezone(&Utc))
}

fn seconds_between(from: DateTime<Utc>, to: DateTime<Utc>) -> f64 {
    (to - from).num_milliseconds() as f64 / 1000.0
}

/// The last `tail_bytes` of a file (or all of it), the file's size, and the open handle.
fn read_tail(file: &Path, tail_bytes: u64) -> Option<(String, u64, File)> {
    let mut handle = File::open(file).ok()?;
    let size = handle.seek(SeekFrom::End(0)).ok()?;
    handle.seek(SeekFrom::Start(size.saturating_sub(tail_bytes))).ok()?;
    let mut bytes = Vec::new();
    handle.read_to_end(&mut bytes).ok()?;
    Some((String::from_utf8_lossy(&bytes).into_owned(), size, handle))
}

fn read_head(handle: &mut File, head_bytes: usize) -> String {
    if handle.seek(SeekFrom::Start(0)).is_err() {
        return String::new();
    }
    let mut bytes = Vec::new();
    let _ = handle.take(head_bytes as u64).read_to_end(&mut bytes);
    String::from_utf8_lossy(&bytes).into_owned()
}

fn modified(path: &Path) -> Option<DateTime<Utc>> {
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    let time: SystemTime = metadata.modified().ok()?;
    Some(DateTime::<Utc>::from(time))
}

fn is_hidden(name: &std::ffi::OsStr) -> bool {
    name.to_string_lossy().starts_with('.')
}

// MARK: - Claude Code

pub struct ClaudePromptCache;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Entry {
    uuid: Option<String>,
    parent_uuid: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    timestamp: Option<String>,
    is_sidechain: Option<bool>,
    message: Option<EntryMessage>,
}

#[derive(Deserialize)]
struct EntryMessage {
    usage: Option<EntryUsage>,
}

#[derive(Deserialize)]
struct EntryUsage {
    cache_read_input_tokens: Option<i64>,
    cache_creation_input_tokens: Option<i64>,
    cache_creation: Option<Creation>,
}

#[derive(Deserialize)]
struct Creation {
    ephemeral_1h_input_tokens: Option<i64>,
    ephemeral_5m_input_tokens: Option<i64>,
}

impl ClaudePromptCache {
    /// Enough of a session's end to hold its last few replies, and a cache write among them,
    /// without reading a long transcript whole.
    pub const TAIL_BYTES: u64 = 512 * 1024;
    /// Where a conversation's opening prompt is, in all but the longest pastes.
    pub const HEAD_BYTES: usize = 64 * 1024;
    /// A response can take this long to arrive and no longer, in what is believed about the log.
    /// A longer gap between a reply and the message that prompted it means the two are not the
    /// pair they look like (a resumed session, a compaction) and the reply's own time is used.
    pub const LONGEST_RESPONSE: f64 = 20.0 * 60.0;

    /// Every main session whose cache is still alive, soonest to lapse first, and, when none is,
    /// the one that lapsed last.
    ///
    /// Main sessions sit one folder down (`projects/<project>/<id>.jsonl`); anything deeper is a
    /// subagent's, and is not looked at. Only files written in the last hour can hold a live
    /// cache (a request can be no later than the file's last write, and no tier outlives an
    /// hour), so only those, and the newest file for the lapsed case, are opened.
    pub fn read(sources: &Sources, now: DateTime<Utc>) -> PromptCacheReading {
        let files = Self::main_sessions(&sources.claude_root());
        let window = Duration::seconds(PromptCacheLapse::HOUR as i64 + 60);

        let mut live: Vec<PromptCacheSession> = files
            .iter()
            .filter(|(_, modified)| now - *modified <= window)
            .filter_map(|(path, _)| Self::session(path))
            .filter(|session| session.lapse.expires_at > now)
            .collect();
        live.sort_by_key(|s| s.lapse.expires_at);
        if !live.is_empty() {
            return PromptCacheReading { live, last_lapsed: None };
        }

        let newest = files.iter().max_by_key(|(_, modified)| *modified);
        PromptCacheReading { live: Vec::new(), last_lapsed: newest.and_then(|(path, _)| Self::session(path)).map(|s| s.lapse) }
    }

    fn main_sessions(root: &Path) -> Vec<(PathBuf, DateTime<Utc>)> {
        let mut found = Vec::new();
        let Ok(projects) = std::fs::read_dir(root) else { return found };
        for project in projects.flatten() {
            if is_hidden(&project.file_name()) {
                continue;
            }
            let Ok(files) = std::fs::read_dir(project.path()) else { continue };
            for file in files.flatten() {
                let path = file.path();
                if is_hidden(&file.file_name()) || path.extension().is_none_or(|e| e != "jsonl") {
                    continue;
                }
                if let Some(modified) = modified(&path) {
                    found.push((path, modified));
                }
            }
        }
        found
    }

    /// One session's lapse, named: the tail gives the lapse, a rename and the directory; the head
    /// gives the opening prompt when there is no rename.
    fn session(file: &Path) -> Option<PromptCacheSession> {
        let (tail, size, mut handle) = read_tail(file, Self::TAIL_BYTES)?;
        let lapse = Self::lapse_in_tail(&tail)?;

        let whole = size <= Self::TAIL_BYTES;
        let mut names = SessionNames::from(&tail, whole);
        if (names.title.is_none() || names.cwd.is_none()) && !whole {
            let opening = SessionNames::from(&read_head(&mut handle, Self::HEAD_BYTES), true);
            names.title = names.title.or(opening.title);
            names.cwd = names.cwd.or(opening.cwd);
        }
        let path = file.to_string_lossy();
        let project = UsageProject::new(names.cwd.as_deref()).or_else(|| project_of(&path, Provider::ClaudeCode));
        Some(PromptCacheSession {
            id: path.into_owned(),
            title: names.title,
            is_review: false,
            project: project.map(|p| p.name),
            lapse,
        })
    }

    /// From the end of a session backwards: the latest reply that touched the cache gives the
    /// time, and the latest one that wrote to it gives the tier (often the same reply, not
    /// always, since a hit writes nothing).
    ///
    /// The time is the request's: the message that prompted that reply, found by walking back
    /// through the reply's own blocks (one response is logged as several lines) to the first
    /// entry that is not one. Where that entry is not in reach of the tail, or is implausibly far
    /// back, the reply's own time stands: later than the request, so never the earlier lapse by
    /// mistake.
    ///
    /// None when no reply touched the cache, or none in reach says which tier it wrote: a tier
    /// Pulse would otherwise have to assume. The first line of a tail that starts mid-file does
    /// not parse and is skipped with the rest of what is not a reply.
    pub fn lapse_in_tail(text: &str) -> Option<PromptCacheLapse> {
        let lines: Vec<&str> = text.split('\n').filter(|l| !l.is_empty()).collect();
        let mut last_request: Option<DateTime<Utc>> = None;
        for line in lines.iter().rev().filter(|l| l.contains("\"usage\"")) {
            let Ok(entry) = serde_json::from_str::<Entry>(line) else { continue };
            if entry.kind.as_deref() != Some("assistant") || entry.is_sidechain == Some(true) {
                continue;
            }
            let Some(usage) = entry.message.as_ref().and_then(|m| m.usage.as_ref()) else { continue };

            let touched = usage.cache_read_input_tokens.unwrap_or(0) > 0 || usage.cache_creation_input_tokens.unwrap_or(0) > 0;
            if last_request.is_none() {
                if !touched {
                    continue;
                }
                let Some(replied_at) = entry.timestamp.as_deref().and_then(parse_iso) else { continue };
                last_request = Some(Self::request_time(&entry, replied_at, &lines).unwrap_or(replied_at));
            }
            if let (Some(request), Some(wrote)) = (last_request, usage.cache_creation.as_ref()) {
                if wrote.ephemeral_1h_input_tokens.unwrap_or(0) > 0 {
                    return Some(PromptCacheLapse::new(request, PromptCacheLapse::HOUR));
                }
                if wrote.ephemeral_5m_input_tokens.unwrap_or(0) > 0 {
                    return Some(PromptCacheLapse::new(request, PromptCacheLapse::FIVE_MINUTES));
                }
            }
        }
        None
    }

    /// The time of the message that prompted `reply`, when it is in the tail and the gap is one a
    /// response can have.
    fn request_time(reply: &Entry, replied_at: DateTime<Utc>, lines: &[&str]) -> Option<DateTime<Utc>> {
        let mut parent = reply.parent_uuid.clone();
        // One response is a handful of lines; a longer chain is not one.
        for _ in 0..64 {
            let id = parent?;
            let entry = Self::entry_with_uuid(&id, lines)?;
            if entry.kind.as_deref() != Some("assistant") {
                let at = entry.timestamp.as_deref().and_then(parse_iso)?;
                let gap = seconds_between(at, replied_at);
                return (0.0..=Self::LONGEST_RESPONSE).contains(&gap).then_some(at);
            }
            parent = entry.parent_uuid;
        }
        None
    }

    /// Found by its own `"uuid"` field, which a `parentUuid` elsewhere cannot match: the key is
    /// spelled with a capital there.
    fn entry_with_uuid(id: &str, lines: &[&str]) -> Option<Entry> {
        let needle = format!("\"uuid\":\"{id}\"");
        for line in lines.iter().rev().filter(|l| l.contains(&needle)) {
            if let Ok(entry) = serde_json::from_str::<Entry>(line) {
                if entry.uuid.as_deref() == Some(id) {
                    return Some(entry);
                }
            }
        }
        None
    }
}

/// What a session is called and where it ran, the way the Token spend pane names it: a rename
/// (the last one) over the opening prompt.
#[derive(Default)]
struct SessionNames {
    title: Option<String>,
    cwd: Option<String>,
}

impl SessionNames {
    /// `starts_at_beginning`: whether the text is the file's start, where the first user line is
    /// the opening prompt. In a tail it is only the latest turn's, which names nothing.
    fn from(text: &str, starts_at_beginning: bool) -> SessionNames {
        let mut names = SessionNames::default();
        let mut opening: Option<String> = None;
        for line in text.split('\n') {
            let Ok(Value::Object(root)) = serde_json::from_str::<Value>(line) else { continue };
            if names.cwd.is_none() {
                if let Some(cwd) = root.get("cwd").and_then(Value::as_str).filter(|c| !c.is_empty()) {
                    names.cwd = Some(cwd.to_string());
                }
            }
            if let Some(title) = root.get("customTitle").and_then(Value::as_str).and_then(title_from) {
                names.title = Some(title);
            }
            if opening.is_none()
                && root.get("type").and_then(Value::as_str) == Some("user")
                && root.get("isSidechain").and_then(Value::as_bool) != Some(true)
            {
                opening = root
                    .get("message")
                    .and_then(Value::as_object)
                    .and_then(|m| text_in(m.get("content")))
                    .and_then(|t| title_from(&t));
            }
        }
        if names.title.is_none() && starts_at_beginning {
            names.title = opening;
        }
        names
    }
}

// MARK: - Codex

pub struct CodexPromptCache;

impl CodexPromptCache {
    /// OpenAI's stated minimum on GPT-5.6 and later (seconds).
    pub const GUARANTEED: f64 = 30.0 * 60.0;
    /// Enough of a session's end to hold its last turn's context line, its requests and their
    /// usage, without reading a long rollout whole.
    pub const TAIL_BYTES: u64 = 512 * 1024;
    pub const HEAD_BYTES: usize = 64 * 1024;

    /// Whether OpenAI states a cache lifetime for this model: `gpt-5.6` and every later version,
    /// whatever comes after the number (`gpt-6-sol`). Anything else (earlier GPTs, other
    /// families, Codex's own review model) has none, and is not timed.
    pub fn states_lifetime(model: &str) -> bool {
        let Some(rest) = model.strip_prefix("gpt-") else { return false };
        let version: String = rest.chars().take_while(|c| c.is_numeric() || *c == '.').collect();
        let parts: Vec<i64> = version.split('.').filter(|p| !p.is_empty()).filter_map(|p| p.parse().ok()).collect();
        let Some(&major) = parts.first() else { return false };
        let minor = parts.get(1).copied().unwrap_or(0);
        major > 5 || (major == 5 && minor >= 6)
    }

    /// Every session whose guaranteed time is still running, soonest to end first, and, when
    /// none is, the one whose time ended last.
    ///
    /// Rollouts are filed under the day they started, so a long session can still be written to
    /// from an old folder; every rollout is listed and only those written in the last half hour
    /// are opened, plus the newest.
    pub fn read(sources: &Sources, now: DateTime<Utc>) -> PromptCacheReading {
        let files = Self::rollouts(&sources.codex_root().join("sessions"));
        let window = Duration::seconds(Self::GUARANTEED as i64 + 60);

        let mut live: Vec<PromptCacheSession> = files
            .iter()
            .filter(|(_, modified)| now - *modified <= window)
            .filter_map(|(path, _)| Self::session(path))
            .filter(|session| session.lapse.expires_at > now)
            .collect();
        live.sort_by_key(|s| s.lapse.expires_at);
        if !live.is_empty() {
            return PromptCacheReading { live, last_lapsed: None };
        }

        let newest = files.iter().max_by_key(|(_, modified)| *modified);
        PromptCacheReading { live: Vec::new(), last_lapsed: newest.and_then(|(path, _)| Self::session(path)).map(|s| s.lapse) }
    }

    fn rollouts(root: &Path) -> Vec<(PathBuf, DateTime<Utc>)> {
        let mut found = Vec::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(directory) = pending.pop() {
            let Ok(entries) = std::fs::read_dir(&directory) else { continue };
            for entry in entries.flatten() {
                if is_hidden(&entry.file_name()) {
                    continue;
                }
                let Ok(kind) = entry.file_type() else { continue };
                let path = entry.path();
                if kind.is_dir() {
                    pending.push(path);
                } else if path.extension().is_some_and(|e| e == "jsonl") {
                    if let Some(modified) = modified(&path) {
                        found.push((path, modified));
                    }
                }
            }
        }
        found
    }

    fn session(file: &Path) -> Option<PromptCacheSession> {
        let (tail, size, mut handle) = read_tail(file, Self::TAIL_BYTES)?;
        let lapse = Self::lapse_in_tail(&tail)?;

        // The header (directory) and the opening prompt are at the start.
        let head = if size <= Self::TAIL_BYTES { tail } else { read_head(&mut handle, Self::HEAD_BYTES) };
        let (title, cwd, is_review) = Self::names_in_head(&head);
        Some(PromptCacheSession {
            id: file.to_string_lossy().into_owned(),
            title,
            is_review,
            project: UsageProject::new(cwd.as_deref()).map(|p| p.name),
            lapse,
        })
    }

    /// The directory from the session header, and the opening prompt: the first user message that
    /// is words rather than an envelope, the way the Token spend pane names a Codex session.
    fn names_in_head(text: &str) -> (Option<String>, Option<String>, bool) {
        let mut title: Option<String> = None;
        let mut cwd: Option<String> = None;
        let mut is_review = false;
        for line in text.split('\n') {
            if !((title.is_none() && !is_review) || cwd.is_none()) {
                break;
            }
            if !(line.contains("\"cwd\"") || line.contains("\"role\":\"user\"")) {
                continue;
            }
            let Ok(root) = serde_json::from_str::<Value>(line) else { continue };
            let Some(payload) = root.get("payload").filter(|p| p.is_object()) else { continue };
            if cwd.is_none() {
                if let Some(found) = payload.get("cwd").and_then(Value::as_str).filter(|c| !c.is_empty()) {
                    cwd = Some(found.to_string());
                }
            }
            if title.is_none()
                && !is_review
                && payload.get("type").and_then(Value::as_str) == Some("message")
                && payload.get("role").and_then(Value::as_str) == Some("user")
            {
                if is_codex_review(payload.get("content")) {
                    is_review = true;
                } else {
                    title = codex_title(payload.get("content"));
                }
            }
        }
        (title, cwd, is_review)
    }

    /// From the end of a rollout backwards: the latest request that read or wrote the cache, the
    /// model its turn ran on, and when it was sent.
    ///
    /// The request's time is the input that prompted it (the person's message or a tool's
    /// output), not the usage line, which is written once the answer is in. None when no request
    /// touched the cache, when the model is not in reach of the tail, or when OpenAI states no
    /// lifetime for it.
    pub fn lapse_in_tail(text: &str) -> Option<PromptCacheLapse> {
        let lines: Vec<Line> = text.split('\n').filter_map(Line::parse).collect();
        let used = lines.iter().rposition(|l| l.touched_cache)?;
        let answered_at = lines[used].timestamp?;

        let before = &lines[..used];
        let model = before.iter().rev().find_map(|l| l.model.as_deref())?;
        if !Self::states_lifetime(model) {
            return None;
        }

        let mut sent_at = answered_at;
        if let Some(input) = before.iter().rev().find(|l| l.is_input).and_then(|l| l.timestamp) {
            let gap = seconds_between(input, answered_at);
            if (0.0..=ClaudePromptCache::LONGEST_RESPONSE).contains(&gap) {
                sent_at = input;
            }
        }
        Some(PromptCacheLapse::with_minimum(sent_at, Self::GUARANTEED, true))
    }
}

/// One line of a rollout, reduced to what the lapse needs.
struct Line {
    timestamp: Option<DateTime<Utc>>,
    /// A request's usage, with any of it read from or written to the cache.
    touched_cache: bool,
    /// The model a turn ran on: its context line, or the session header.
    model: Option<String>,
    /// Something sent to the model: the person's message or a tool's output.
    is_input: bool,
}

impl Line {
    fn parse(line: &str) -> Option<Line> {
        let Ok(Value::Object(root)) = serde_json::from_str::<Value>(line) else { return None };
        let kind = root.get("type").and_then(Value::as_str);
        let empty = Value::Object(Default::default());
        let payload = root.get("payload").filter(|p| p.is_object()).unwrap_or(&empty);
        let event = payload.get("type").and_then(Value::as_str);
        let timestamp = root.get("timestamp").and_then(Value::as_str).and_then(parse_iso);

        // A request's usage arrives twice (its own record and the running count after it) and
        // either says the same of the cache.
        let usage = if kind == Some("token_usage_record") {
            payload.get("usage")
        } else if event == Some("token_count") {
            payload.get("info").and_then(|i| i.get("last_token_usage"))
        } else {
            None
        };
        let touched_cache = usage.is_some_and(|u| {
            u.get("cached_input_tokens").and_then(Value::as_i64).unwrap_or(0) > 0
                || u.get("cache_write_input_tokens").and_then(Value::as_i64).unwrap_or(0) > 0
        });

        let model = match kind {
            Some("turn_context") => payload.get("model").and_then(Value::as_str).map(str::to_string),
            Some("session_meta") => payload
                .get("base_instructions")
                .and_then(|b| b.get("provenance"))
                .and_then(|p| p.get("model"))
                .and_then(Value::as_str)
                .map(str::to_string),
            _ => None,
        };

        let is_input = (kind == Some("response_item")
            && (event == Some("function_call_output")
                || event == Some("custom_tool_call_output")
                || (event == Some("message") && payload.get("role").and_then(Value::as_str) == Some("user"))))
            || (kind == Some("event_msg") && matches!(event, Some("user_message") | Some("task_started")));

        Some(Line { timestamp, touched_cache, model, is_input })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::fmt::Write as _;

    fn date(text: &str) -> DateTime<Utc> {
        parse_iso(text).unwrap()
    }

    fn reply(at: &str, read: i64, hour: i64, five: i64, sidechain: bool) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"{at}","isSidechain":{sidechain},"message":{{"usage":{{"input_tokens":3,"cache_read_input_tokens":{read},"cache_creation_input_tokens":{},"cache_creation":{{"ephemeral_1h_input_tokens":{hour},"ephemeral_5m_input_tokens":{five}}},"output_tokens":9}}}}}}"#,
            hour + five
        )
    }

    fn written(at: &str, hour: i64) -> String {
        reply(at, 0, hour, 0, false)
    }

    #[test]
    fn an_hour_tier_lasts_an_hour_from_the_last_request() {
        let lapse = ClaudePromptCache::lapse_in_tail(
            &[written("2026-09-30T10:00:00.000Z", 500), reply("2026-09-30T10:20:00.000Z", 9_000, 200, 0, false)].join("\n"),
        )
        .unwrap();
        assert_eq!(lapse.lifetime, PromptCacheLapse::HOUR);
        assert_eq!(lapse.expires_at, date("2026-09-30T11:20:00Z"));
    }

    #[test]
    fn a_hit_renews_the_tier_an_earlier_reply_wrote() {
        // The last reply only read the cache; the tier comes from the one before.
        let lapse = ClaudePromptCache::lapse_in_tail(
            &[reply("2026-09-30T10:00:00.000Z", 0, 0, 800, false), reply("2026-09-30T10:03:00.000Z", 9_000, 0, 0, false)].join("\n"),
        )
        .unwrap();
        assert_eq!(lapse.lifetime, PromptCacheLapse::FIVE_MINUTES);
        assert_eq!(lapse.last_request, date("2026-09-30T10:03:00Z"));
    }

    #[test]
    fn subagent_lines_and_other_entries_are_not_the_session() {
        let lapse = ClaudePromptCache::lapse_in_tail(
            &[
                "half a line cut off by the tail\"usage\"".to_string(),
                written("2026-09-30T10:00:00.000Z", 500),
                r#"{"type":"user","timestamp":"2026-09-30T10:30:00.000Z","message":{"usage":{}}}"#.to_string(),
                reply("2026-09-30T10:40:00.000Z", 0, 0, 300, true),
            ]
            .join("\n"),
        )
        .unwrap();
        assert_eq!(lapse.last_request, date("2026-09-30T10:00:00Z"));
        assert_eq!(lapse.lifetime, PromptCacheLapse::HOUR);
    }

    #[test]
    fn no_tier_in_reach_is_no_lapse() {
        // Replies that only read the cache never say how long it is kept.
        assert!(ClaudePromptCache::lapse_in_tail(&reply("2026-09-30T10:00:00.000Z", 9_000, 0, 0, false)).is_none());
        assert!(ClaudePromptCache::lapse_in_tail(&reply("2026-09-30T10:00:00.000Z", 0, 0, 0, false)).is_none());
        assert!(ClaudePromptCache::lapse_in_tail("").is_none());
    }

    // The request's time, not the reply's.

    fn block(at: &str, uuid: &str, parent: Option<&str>, hour: i64, read: i64) -> String {
        let parent = parent.map_or("null".to_string(), |p| format!("\"{p}\""));
        format!(
            r#"{{"uuid":"{uuid}","parentUuid":{parent},"type":"assistant","timestamp":"{at}","isSidechain":false,"message":{{"usage":{{"cache_read_input_tokens":{read},"cache_creation_input_tokens":{hour},"cache_creation":{{"ephemeral_1h_input_tokens":{hour},"ephemeral_5m_input_tokens":0}}}}}}}}"#
        )
    }

    fn prompt(at: &str, uuid: &str, kind: &str) -> String {
        format!(r#"{{"uuid":"{uuid}","parentUuid":null,"type":"{kind}","timestamp":"{at}","message":{{"content":"x"}}}}"#)
    }

    #[test]
    fn the_cache_is_counted_from_the_request_not_from_the_end_of_the_answer() {
        // Asked at 10:00:00; the answer streamed for ninety seconds, as two logged blocks, and was
        // written down at 10:01:30.
        let lapse = ClaudePromptCache::lapse_in_tail(
            &[
                prompt("2026-09-30T10:00:00.000Z", "ask", "user"),
                block("2026-09-30T10:00:05.000Z", "b1", Some("ask"), 300, 0),
                block("2026-09-30T10:01:30.000Z", "b2", Some("b1"), 300, 0),
            ]
            .join("\n"),
        )
        .unwrap();
        assert_eq!(lapse.last_request, date("2026-09-30T10:00:00Z"));
        assert_eq!(lapse.expires_at, date("2026-09-30T11:00:00Z"));
    }

    #[test]
    fn a_tool_result_is_the_request_too() {
        let lapse = ClaudePromptCache::lapse_in_tail(
            &[
                prompt("2026-09-30T10:00:00.000Z", "ask", "user"),
                block("2026-09-30T10:00:10.000Z", "b1", Some("ask"), 300, 0),
                prompt("2026-09-30T10:12:00.000Z", "tool", "user"),
                block("2026-09-30T10:12:20.000Z", "b2", Some("tool"), 0, 9_000),
            ]
            .join("\n"),
        )
        .unwrap();
        assert_eq!(lapse.last_request, date("2026-09-30T10:12:00Z"));
        assert_eq!(lapse.lifetime, PromptCacheLapse::HOUR);
    }

    #[test]
    fn the_replys_own_time_stands_when_the_request_is_not_in_reach_or_not_plausible() {
        // The prompting entry is not in the tail.
        let cut = ClaudePromptCache::lapse_in_tail(&block("2026-09-30T10:01:30.000Z", "b", Some("gone"), 300, 0)).unwrap();
        assert_eq!(cut.last_request, date("2026-09-30T10:01:30Z"));

        // Forty minutes before the reply is not the message it answered.
        let far = ClaudePromptCache::lapse_in_tail(
            &[prompt("2026-09-30T09:00:00.000Z", "ask", "user"), block("2026-09-30T09:40:00.000Z", "b", Some("ask"), 300, 0)].join("\n"),
        )
        .unwrap();
        assert_eq!(far.last_request, date("2026-09-30T09:40:00Z"));
    }

    // Every conversation.

    fn now() -> DateTime<Utc> {
        date("2026-09-30T12:00:00Z")
    }

    fn iso(minutes_ago: f64) -> String {
        (now() - Duration::milliseconds((minutes_ago * 60_000.0) as i64)).format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
    }

    fn write(home: &Path, relative: &str, minutes_ago: f64, lines: &[String]) {
        let file = home.join(relative);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, lines.join("\n")).unwrap();
        let handle = std::fs::OpenOptions::new().write(true).open(&file).unwrap();
        let when = SystemTime::from(now() - Duration::milliseconds((minutes_ago * 60_000.0) as i64));
        handle.set_modified(when).unwrap();
    }

    fn opening(text: &str) -> String {
        format!(r#"{{"type":"user","cwd":"/Users/me/Code/Pulse","message":{{"role":"user","content":"{text}"}}}}"#)
    }

    #[test]
    fn every_live_conversation_soonest_first_subagents_left_out() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let claude = |name: &str| format!(".claude/projects/{name}");
        // Three conversations side by side, each on the hour tier.
        write(home, &claude("-Users-me-Code-Pulse/a.jsonl"), 50.0, &[opening("Fix the card"), written(&iso(50.0), 400)]);
        write(
            home,
            &claude("-Users-me-Code-Pulse/b.jsonl"),
            5.0,
            &[opening("Write the docs"), written(&iso(5.0), 400), r#"{"type":"custom-title","customTitle":"Docs pass"}"#.to_string()],
        );
        write(home, &claude("-Users-me-Code-Site/c.jsonl"), 20.0, &[opening("Ship the site"), written(&iso(20.0), 400)]);
        // A subagent a folder deeper, just now, on the short tier: not a conversation.
        write(home, &claude("-Users-me-Code-Pulse/b/subagents/agent-1.jsonl"), 1.0, &[reply(&iso(1.0), 0, 0, 300, false)]);
        // One from yesterday, long lapsed.
        write(home, &claude("-Users-me-Code-Pulse/old.jsonl"), 1440.0, &[opening("Old"), written(&iso(1440.0), 400)]);

        let reading = ClaudePromptCache::read(&Sources::new(home), now());
        let titles: Vec<_> = reading.live.iter().map(|s| s.title.clone()).collect();
        assert_eq!(titles, vec![Some("Fix the card".into()), Some("Ship the site".into()), Some("Docs pass".into())]);
        let expires: Vec<_> = reading.live.iter().map(|s| s.lapse.expires_at).collect();
        assert_eq!(expires, [10, 40, 55].map(|m| now() + Duration::minutes(m)).to_vec());
        assert_eq!(reading.live[0].project.as_deref(), Some("Pulse"));
        assert!(reading.last_lapsed.is_none());
    }

    #[test]
    fn with_none_alive_the_newest_lapse_is_kept() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        write(home, ".claude/projects/p/a.jsonl", 90.0, &[written(&iso(90.0), 400)]);
        write(home, ".claude/projects/p/b.jsonl", 30.0, &[reply(&iso(30.0), 0, 0, 400, false)]);
        let reading = ClaudePromptCache::read(&Sources::new(home), now());
        assert!(reading.live.is_empty());
        assert_eq!(reading.last_lapsed.unwrap().expires_at, now() - Duration::minutes(25));
    }

    #[test]
    fn claude_config_dir_is_honoured() {
        let home = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        write(elsewhere.path(), "projects/p/a.jsonl", 5.0, &[written(&iso(5.0), 400)]);
        let mut sources = Sources::new(home.path());
        assert!(ClaudePromptCache::read(&sources, now()).live.is_empty());
        sources.claude_config_dir = Some(elsewhere.path().to_path_buf());
        assert_eq!(ClaudePromptCache::read(&sources, now()).live.len(), 1);
    }

    #[test]
    fn a_reading_names_the_conversation_about_to_lapse() {
        let session = |name: Option<&str>, project: &str, expires_in: i64| PromptCacheSession {
            id: name.unwrap_or("-").to_string(),
            title: name.map(str::to_string),
            is_review: false,
            project: Some(project.to_string()),
            lapse: PromptCacheLapse::new(now() + Duration::minutes(expires_in - 60), PromptCacheLapse::HOUR),
        };
        let reading = PromptCacheReading {
            live: vec![session(Some("Docs pass"), "Pulse", 55), session(Some("Fix the card"), "Pulse", 10), session(None, "Site", 40)],
            last_lapsed: None,
        };
        let names: Vec<_> = reading.alive(now()).iter().map(PromptCacheSession::display_name).collect();
        assert_eq!(names, ["Fix the card", "Site", "Docs pass"]);
        // Ten minutes later the first has lapsed and the next is named.
        assert_eq!(reading.alive(now() + Duration::minutes(11))[0].display_name(), "Site");
        assert!(reading.alive(now() + Duration::minutes(56)).is_empty());
    }

    #[test]
    fn durations_round_up_to_whole_minutes() {
        assert_eq!(PromptCacheLapse::duration_text(3600.0), "1 hr");
        assert_eq!(PromptCacheLapse::duration_text(38.0 * 60.0 - 10.0), "38 min");
        assert_eq!(PromptCacheLapse::duration_text(5.0), "1 min");
        assert_eq!(PromptCacheLapse::duration_text(3600.0 + 60.0 * 5.0), "1 hr, 5 min");
    }

    // Codex.

    fn header(cwd: &str) -> String {
        format!(r#"{{"timestamp":"2026-09-30T09:00:00.000Z","type":"session_meta","payload":{{"cwd":"{cwd}"}}}}"#)
    }

    fn context(model: &str, ago: f64) -> String {
        format!(r#"{{"timestamp":"{}","type":"turn_context","payload":{{"model":"{model}"}}}}"#, iso(ago))
    }

    fn ask(text: &str, ago: f64) -> String {
        format!(
            r#"{{"timestamp":"{}","type":"response_item","payload":{{"type":"message","role":"user","content":[{{"type":"input_text","text":"{text}"}}]}}}}"#,
            iso(ago)
        )
    }

    fn tool_output(ago: f64) -> String {
        format!(r#"{{"timestamp":"{}","type":"response_item","payload":{{"type":"custom_tool_call_output","output":"ok"}}}}"#, iso(ago))
    }

    fn usage(ago: f64, cached: i64) -> String {
        format!(
            r#"{{"timestamp":"{}","type":"token_usage_record","payload":{{"usage":{{"input_tokens":10000,"cached_input_tokens":{cached},"cache_write_input_tokens":0,"output_tokens":20}}}}}}"#,
            iso(ago)
        )
    }

    #[test]
    fn only_models_openai_states_a_lifetime_for_are_timed() {
        for model in ["gpt-5.6", "gpt-5.6-sol", "gpt-6-sol", "gpt-6.1-sol", "gpt-7"] {
            assert!(CodexPromptCache::states_lifetime(model), "{model}");
        }
        for model in ["gpt-5.5", "gpt-5", "gpt-5-codex", "codex-auto-review", "o3", "gpt-"] {
            assert!(!CodexPromptCache::states_lifetime(model), "{model}");
        }
    }

    #[test]
    fn thirty_minutes_from_the_request_as_a_floor() {
        // Asked 10 minutes ago; a tool ran and its output went back 4 minutes ago; that answer's
        // usage was logged 3 minutes ago.
        let lapse = CodexPromptCache::lapse_in_tail(
            &[header("/Users/me/Code/Pulse"), context("gpt-6-sol", 10.0), ask("Fix the build", 10.0), usage(9.5, 9_000), tool_output(4.0), usage(3.0, 9_000)]
                .join("\n"),
        )
        .unwrap();
        assert!(lapse.is_minimum);
        assert_eq!(lapse.lifetime, 30.0 * 60.0);
        assert_eq!(lapse.last_request, now() - Duration::minutes(4));
    }

    #[test]
    fn earlier_models_and_untouched_caches_are_not_timed() {
        assert!(CodexPromptCache::lapse_in_tail(&[context("gpt-5.5", 10.0), ask("x", 10.0), usage(9.0, 9_000)].join("\n")).is_none());
        assert!(CodexPromptCache::lapse_in_tail(&[context("gpt-6-sol", 10.0), ask("x", 10.0), usage(9.0, 0)].join("\n")).is_none());
        // No model in reach of the tail: nothing to look the lifetime up by.
        assert!(CodexPromptCache::lapse_in_tail(&[ask("x", 10.0), usage(9.0, 9_000)].join("\n")).is_none());
    }

    #[test]
    fn the_latest_turns_model_decides() {
        // Switched from a timed model to an untimed one: the last request is not timed.
        assert!(CodexPromptCache::lapse_in_tail(
            &[context("gpt-6-sol", 20.0), ask("a", 20.0), usage(19.0, 9_000), context("gpt-5.5", 5.0), ask("b", 5.0), usage(4.0, 9_000)].join("\n")
        )
        .is_none());
    }

    fn write_rollout(home: &Path, name: &str, minutes_ago: f64, lines: &[String]) {
        write(home, &format!(".codex/sessions/2026/09/29/{name}.jsonl"), minutes_ago, lines);
    }

    #[test]
    fn every_conversation_inside_its_floor_soonest_first() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        // Filed under the day it started, still written to now.
        write_rollout(home, "rollout-a", 25.0, &[header("/Users/me/Code/Pulse"), context("gpt-6-sol", 26.0), ask("Ship the docs", 26.0), usage(25.0, 9_000)]);
        write_rollout(
            home,
            "rollout-b",
            5.0,
            &[
                header("/Users/me/Code/Site"),
                context("gpt-5.6-sol", 6.0),
                ask("<environment_context>…</environment_context>", 6.0),
                ask("Tidy the site", 6.0),
                usage(5.0, 9_000),
            ],
        );
        // An earlier model: not timed, even though it is recent.
        write_rollout(home, "rollout-c", 2.0, &[header("/Users/me/Code/Pulse"), context("gpt-5.5", 3.0), ask("Old model", 3.0), usage(2.0, 9_000)]);

        let reading = CodexPromptCache::read(&Sources::new(home), now());
        let titles: Vec<_> = reading.live.iter().map(|s| s.title.clone()).collect();
        assert_eq!(titles, vec![Some("Ship the docs".into()), Some("Tidy the site".into())]);
        let projects: Vec<_> = reading.live.iter().map(|s| s.project.clone()).collect();
        assert_eq!(projects, vec![Some("Pulse".into()), Some("Site".into())]);
        assert_eq!(reading.live[0].lapse.expires_at, now() + Duration::minutes(4));
    }

    #[test]
    fn past_the_floor_the_newest_end_is_kept() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        write_rollout(home, "rollout-a", 50.0, &[context("gpt-6-sol", 50.0), ask("x", 50.0), usage(50.0, 9_000)]);
        let reading = CodexPromptCache::read(&Sources::new(home), now());
        assert!(reading.live.is_empty());
        let lapsed = reading.last_lapsed.unwrap();
        assert_eq!(lapsed.expires_at, now() - Duration::minutes(20));
        assert!(lapsed.is_minimum);
    }

    #[test]
    fn codex_home_is_honoured_and_unsupported_providers_read_nothing() {
        let home = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        write(elsewhere.path(), "sessions/2026/09/29/rollout-a.jsonl", 5.0, &[context("gpt-6-sol", 6.0), ask("x", 6.0), usage(5.0, 9_000)]);
        let mut sources = Sources::new(home.path());
        assert!(read_for(Provider::Codex, &sources, now()).live.is_empty());
        sources.codex_home = Some(elsewhere.path().to_path_buf());
        assert_eq!(read_for(Provider::Codex, &sources, now()).live.len(), 1);
        assert_eq!(read_for(Provider::Cursor, &sources, now()), PromptCacheReading::none());
        let _ = (Utc.timestamp_opt(0, 0), write!(String::new(), ""));
    }
}
