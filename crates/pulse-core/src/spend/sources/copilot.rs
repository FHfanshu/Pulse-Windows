// Ported from upstream Sources/Pulse/Usage/Readers/CopilotLogReader.swift, CopilotLogReaderOTEL.swift,
// CopilotLogReaderDesktop.swift and CopilotLogReaderVSCode.swift.
//! GitHub Copilot: three independent stores, read as one client.
//!
//! - an OpenTelemetry JSONL stream under `~/.copilot/otel/`, where the CLI and monitoring builds write
//!   per-span token counts;
//! - the desktop app's SQLite database `~/.copilot/data.db`, with a `session-state/<id>/events.jsonl`
//!   sidecar carrying the cumulative per-model run totals the database does not;
//! - VS Code's chat session logs under `Code/User/workspaceStorage/<hash>/chatSessions/<uuid>.jsonl`.
//!
//! **A session logged in more than one place is counted once.** The lanes are collected in a fixed
//! order, OTEL, then desktop, then VS Code, and each later lane is filtered against the earlier ones: a
//! desktop session OTEL already named is dropped whole, and a VS Code request is dropped when its dedup
//! key or its `(session, instant)` pair is already present.
//!
//! **A dropped desktop session is not silently zeroed.** The desktop row is a lifetime total and OTEL is
//! per span; when the desktop total is larger, the OTEL records of that session are marked partial.
//!
//! **OTEL.** A chat span outranks an inference log, which outranks an agent-turn log, which outranks an
//! agent-summary span; a higher lane suppresses a lower one that shares a trace or response id. A record
//! with neither id is its own event and marked partial. An OTel chat span counts the cache read inside its
//! input, so the cache read is taken out of input once; reasoning is a subset of output, so it only stands
//! in when output was not reported.
//!
//! **Desktop.** The sidecar's `session.shutdown` snapshots are differenced per model, capped by a per-row
//! budget, and whatever the row's lifetime total leaves unexplained is emitted once at `created_at`. A
//! missing head (a log that does not open with `session.start`) is an unknown baseline, not a delta. Cache
//! write lives only in the sidecar. Reasoning is not added to output; a run that stated reasoning is
//! marked partial.
//!
//! **VS Code.** The chat session file is an append/patch log that is replayed into its request array. Only
//! Copilot's own requests count (a resolved model, or a `copilot/` model id). Prompt tokens still hold the
//! cached prefix, so they are counted unclassified, not as fresh input. A request with no timestamp is
//! skipped, never dated at the epoch.
//!
//! Where it lives on Windows: `%USERPROFILE%\.copilot` for the OTEL stream and the desktop database
//! (`WINDOWS-PATH: unverified`), and `%APPDATA%\Code\User\workspaceStorage` for VS Code's chat sessions.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};
use serde_json::{Map, Value};

use super::{clamped, flexible, flexible_text, push_unique, SpendSource};
use crate::provider::Provider;
use crate::spend::logio;
use crate::spend::record::AgentUsageRecord;
use crate::spend::sqlite;
use crate::spend::tally::TokenTally;
use crate::spend::transcripts::Sources;

pub struct Copilot;

impl SpendSource for Copilot {
    fn raw(&self) -> &'static str {
        "copilot"
    }
    fn source_id(&self) -> &'static str {
        "copilot"
    }
    fn display_name(&self) -> &'static str {
        "GitHub Copilot"
    }
    fn card_provider(&self) -> Option<Provider> {
        Some(Provider::Copilot)
    }
    fn icon(&self) -> Option<&'static str> {
        Some("github")
    }
    fn env_vars(&self) -> &'static [&'static str] {
        &["APPDATA"]
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        let mut roots = Vec::new();
        // WINDOWS-PATH: unverified (the CLI's home folder, and the desktop database inside it)
        let copilot = sources.home.join(".copilot");
        push_unique(&mut roots, copilot.join("otel"));
        push_unique(&mut roots, copilot.join("data.db"));
        push_unique(&mut roots, copilot.join("session-state"));
        // VS Code's chat sessions: %APPDATA%\Code on Windows.
        push_unique(&mut roots, sources.app_data().join("Code").join("User").join("workspaceStorage"));
        // WINDOWS-PATH: unverified (the Linux layout, kept as upstream lists it)
        push_unique(&mut roots, sources.home.join(".config").join("Code").join("User").join("workspaceStorage"));
        roots
    }
    fn records(&self, roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        combine(roots)
    }
}

/// The three lanes, read and filtered in the documented order.
fn combine(roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
    // Lane one: OTEL is the authority for every session it names.
    let otel_files = logio::files(&roots.iter().filter(|r| named(r, "otel")).cloned().collect::<Vec<_>>(), &["jsonl"], &[], &[]);
    let mut otel = otel::records(&otel_files);

    // Lane two: the desktop rows are the lifetime authority for a session and say whether OTEL covered all
    // of it. They are read before either lane is filtered.
    let desktop_records: Vec<AgentUsageRecord> =
        roots.iter().filter(|r| named(r, "data.db")).flat_map(|database| desktop::records(database)).collect();

    let desktop_totals = totals(&desktop_records);
    let otel_totals = totals(&otel);
    // Sessions whose desktop lifetime is larger than OTEL recorded, plus those whose dropped desktop work
    // was itself partial: their OTEL records are a known subset.
    let mut partly_covered: HashSet<String> =
        desktop_records.iter().filter(|r| r.is_partial).filter_map(|r| r.session_id.clone()).collect();
    for (session, counted) in &otel_totals {
        if desktop_totals.get(session).is_some_and(|lifetime| lifetime > counted) {
            partly_covered.insert(session.clone());
        }
    }
    if !partly_covered.is_empty() {
        for record in &mut otel {
            if record.session_id.as_ref().is_some_and(|s| partly_covered.contains(s)) {
                record.is_partial = true;
            }
        }
    }

    let otel_sessions: HashSet<String> = otel.iter().filter_map(|r| r.session_id.clone()).collect();
    let mut accumulated = otel;
    accumulated.extend(
        desktop_records.into_iter().filter(|r| r.session_id.as_ref().map_or(true, |s| !otel_sessions.contains(s))),
    );

    // Lane three: VS Code, filtered by dedup key or by the same session at the same instant.
    let identities: HashSet<String> = accumulated.iter().filter_map(|r| r.deduplication_id.clone()).collect();
    let instants: HashSet<(String, i64)> = accumulated
        .iter()
        .filter_map(|r| r.session_id.as_ref().map(|s| (s.clone(), r.timestamp.timestamp_micros())))
        .collect();
    let vscode_roots: Vec<PathBuf> = roots.iter().filter(|r| is_vscode_root(r)).cloned().collect();
    let vscode = vscode::records(&vscode_roots).into_iter().filter(|record| {
        if record.deduplication_id.as_ref().is_some_and(|id| identities.contains(id)) {
            return false;
        }
        !record
            .session_id
            .as_ref()
            .is_some_and(|s| instants.contains(&(s.clone(), record.timestamp.timestamp_micros())))
    });
    accumulated.extend(vscode);

    accumulated.sort_by(|a, b| {
        a.timestamp
            .cmp(&b.timestamp)
            .then_with(|| a.deduplication_id.as_deref().unwrap_or("").cmp(b.deduplication_id.as_deref().unwrap_or("")))
    });
    accumulated
}

/// A root's own name, which is how the entry point tells its lanes apart.
fn named(path: &Path, name: &str) -> bool {
    path.file_name().is_some_and(|n| n == name)
}

fn is_vscode_root(path: &Path) -> bool {
    named(path, "workspaceStorage") && path.parent().is_some_and(|p| named(p, "User"))
}

/// Every record's counted total, grouped by the session it named, saturating rather than overflowing.
fn totals(records: &[AgentUsageRecord]) -> HashMap<String, i64> {
    let mut totals: HashMap<String, i64> = HashMap::new();
    for record in records {
        let Some(session) = &record.session_id else { continue };
        let entry = totals.entry(session.clone()).or_insert(0);
        let t = &record.tally;
        for value in [t.input, t.cache_write, t.cache_read, t.output, record.unclassified_tokens] {
            *entry = entry.saturating_add(value.max(0));
        }
    }
    totals
}

/// What a record carries beyond its tokens and time.
#[derive(Default)]
struct Shape {
    aggregate: bool,
    partial: bool,
    session: Option<String>,
    session_name: Option<String>,
    title: Option<String>,
    project: Option<String>,
    dedup: Option<String>,
}

/// The record the three lanes share, mirroring `StructuredLogSupport.record`: a model is required, the
/// counts must be positive, and the instant must be after the epoch.
fn make(at: DateTime<Utc>, model: &str, tally: TokenTally, unclassified: i64, shape: Shape) -> Option<AgentUsageRecord> {
    let model = non_blank(Some(model))?;
    if tally.total() <= 0 && unclassified <= 0 {
        return None;
    }
    if at.timestamp_micros() <= 0 {
        return None;
    }
    let mut record = AgentUsageRecord::new(at, &model, tally);
    record.session_id = non_blank(shape.session.as_deref());
    record.session_name = non_blank(shape.session_name.as_deref());
    record.title = non_blank(shape.title.as_deref());
    record.project = non_blank(shape.project.as_deref());
    record.deduplication_id = non_blank(shape.dedup.as_deref());
    record.unclassified_tokens = unclassified.max(0);
    record.is_aggregate = shape.aggregate;
    record.is_partial = shape.partial;
    Some(record)
}

fn non_blank(value: Option<&str>) -> Option<String> {
    value.map(str::trim).filter(|v| !v.is_empty()).map(str::to_string)
}

fn value_text(value: Option<&Value>) -> Option<String> {
    logio::text(value)
}

fn object_get<'a>(map: Option<&'a Map<String, Value>>, key: &str) -> Option<&'a Value> {
    map?.get(key)
}

/// A stable 64-bit FNV-1a hash, hex-encoded: stable across launches, unlike `Hasher`.
fn stable_hash(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:x}")
}

/// `copilot/`-free milliseconds, rounded, for a dedup key that must hold across a re-read.
fn milliseconds(at: DateTime<Utc>) -> i64 {
    (at.timestamp_micros() as f64 / 1000.0).round() as i64
}

/// Copilot's own name for the agent that ran a span, normalized.
fn agent_label(raw: Option<&str>) -> Option<String> {
    let label = non_blank(raw)?;
    if label == "github.copilot.default" {
        return Some("GitHub Copilot".to_string());
    }
    if let Some(rest) = label.strip_prefix("github.copilot.") {
        if rest.is_empty() {
            return Some("GitHub Copilot".to_string());
        }
        let parts: Vec<String> = rest.split('.').filter(|p| !p.is_empty()).map(titlecased).collect();
        return Some(parts.join("-"));
    }
    if label.contains(':') {
        let parts: Vec<String> = label.split(':').map(titlecased).collect();
        return Some(parts.join(": "));
    }
    Some(label)
}

fn titlecased(part: &str) -> String {
    let mut chars = part.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

// MARK: - OTEL

mod otel {
    use super::*;

    /// The four record kinds, in priority order: a lower rank outranks a higher one.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    enum Lane {
        ChatSpan,
        InferenceLog,
        AgentTurnLog,
        AgentSummarySpan,
    }

    const LANES: [Lane; 4] = [Lane::ChatSpan, Lane::InferenceLog, Lane::AgentTurnLog, Lane::AgentSummarySpan];

    impl Lane {
        fn rank(self) -> u8 {
            match self {
                Lane::ChatSpan => 0,
                Lane::InferenceLog => 1,
                Lane::AgentTurnLog => 2,
                Lane::AgentSummarySpan => 3,
            }
        }
    }

    #[derive(Default, Clone)]
    struct Context {
        model: Option<String>,
        session: Option<String>,
        response: Option<String>,
        agent: Option<String>,
    }

    #[derive(Clone)]
    struct Candidate {
        lane: Lane,
        key: String,
        timestamp: DateTime<Utc>,
        model: String,
        session: Option<String>,
        agent: Option<String>,
        tally: TokenTally,
        unclassified: i64,
        trace: Option<String>,
        response: Option<String>,
        partial: bool,
    }

    impl Candidate {
        /// A duplicate of one event can only add detail, so each bucket takes the larger of the two.
        fn merged(&self, other: &Candidate) -> Candidate {
            let mut result = self.clone();
            result.tally = TokenTally::new(
                self.tally.input.max(other.tally.input),
                self.tally.cache_write.max(other.tally.cache_write),
                self.tally.cache_read.max(other.tally.cache_read),
                self.tally.output.max(other.tally.output),
            );
            result.unclassified = self.unclassified.max(other.unclassified);
            result.partial = self.partial || other.partial;
            result
        }
    }

    pub(super) fn records(files: &[PathBuf]) -> Vec<AgentUsageRecord> {
        let mut candidates: Vec<Candidate> = Vec::new();
        for file in files {
            let rows: Vec<Map<String, Value>> = logio::json_lines(file)
                .filter_map(|value| match value {
                    Value::Object(map) => Some(map),
                    _ => None,
                })
                .collect();
            let context = trace_context(&rows);
            for (index, row) in rows.iter().enumerate() {
                // The ordinal keeps two identity-less rows apart: they are never folded by a shared instant.
                if let Some(candidate) = candidate(row, index, &context, candidates.len()) {
                    candidates.push(candidate);
                }
            }
        }
        resolve(candidates)
    }

    fn lane(row: &Map<String, Value>) -> Option<Lane> {
        let attrs = row.get("attributes").and_then(Value::as_object);
        let kind = value_text(row.get("type"));
        let name = value_text(row.get("name"));
        let operation = value_text(object_get(attrs, "gen_ai.operation.name"));
        let event_name = value_text(object_get(attrs, "event.name"));
        let is_span = kind.as_deref() == Some("span") || (kind.is_none() && name.is_some() && has_span_shape(row));
        if !is_span {
            if event_name.as_deref() == Some("gen_ai.client.inference.operation.details") || body_starts_with(row, "GenAI inference:") {
                return Some(Lane::InferenceLog);
            }
            if event_name.as_deref() == Some("copilot_chat.agent.turn") || body_starts_with(row, "copilot_chat.agent.turn") {
                return Some(Lane::AgentTurnLog);
            }
            return None;
        }
        if operation.as_deref() == Some("chat") || name.as_deref().is_some_and(|n| n.starts_with("chat ")) {
            return Some(Lane::ChatSpan);
        }
        if operation.as_deref() == Some("invoke_agent") || name.as_deref().is_some_and(|n| n.starts_with("invoke_agent ")) {
            return Some(Lane::AgentSummarySpan);
        }
        None
    }

    /// A span with no `type` is recognised by an explicit span identity, timing or kind.
    fn has_span_shape(row: &Map<String, Value>) -> bool {
        let (trace, span) = identification(row);
        if trace.is_some() || span.is_some() {
            return true;
        }
        ["startTime", "endTime", "duration", "kind"].iter().any(|key| row.contains_key(*key))
    }

    fn body_starts_with(row: &Map<String, Value>, prefix: &str) -> bool {
        ["body", "_body"].iter().any(|key| value_text(row.get(*key)).is_some_and(|text| text.starts_with(prefix)))
    }

    /// The W3C non-recording sentinel (empty, or all zeroes) is absent.
    fn valid_id(value: Option<String>) -> Option<String> {
        value.filter(|v| !v.is_empty() && !v.chars().all(|c| c == '0'))
    }

    fn identification(row: &Map<String, Value>) -> (Option<String>, Option<String>) {
        let nested = row.get("spanContext").and_then(Value::as_object);
        let trace = valid_id(value_text(row.get("traceId")).or_else(|| value_text(object_get(nested, "traceId"))));
        let span = valid_id(value_text(row.get("spanId")).or_else(|| value_text(object_get(nested, "spanId"))));
        (trace, span)
    }

    fn model_of(attrs: Option<&Map<String, Value>>) -> Option<String> {
        value_text(object_get(attrs, "gen_ai.response.model")).or_else(|| value_text(object_get(attrs, "gen_ai.request.model")))
    }

    fn session_of(attrs: Option<&Map<String, Value>>) -> Option<String> {
        [
            "gen_ai.conversation.id",
            "copilot_chat.session_id",
            "copilot_chat.chat_session_id",
            "session.id",
            "github.copilot.interaction_id",
            "gen_ai.response.id",
        ]
        .iter()
        .find_map(|key| value_text(object_get(attrs, key)))
    }

    /// The first stated model, session, response and agent across the lines that share a trace id.
    fn trace_context(rows: &[Map<String, Value>]) -> HashMap<String, Context> {
        let mut contexts: HashMap<String, Context> = HashMap::new();
        for row in rows {
            let Some(trace) = identification(row).0 else { continue };
            let attrs = row.get("attributes").and_then(Value::as_object);
            let entry = contexts.entry(trace).or_default();
            if entry.model.is_none() {
                entry.model = model_of(attrs);
            }
            if entry.session.is_none() {
                entry.session = session_of(attrs);
            }
            if entry.response.is_none() {
                entry.response = value_text(object_get(attrs, "gen_ai.response.id"));
            }
            if entry.agent.is_none() {
                entry.agent = value_text(object_get(attrs, "gen_ai.agent.id"));
            }
        }
        contexts
    }

    fn candidate(row: &Map<String, Value>, index: usize, context: &HashMap<String, Context>, ordinal: usize) -> Option<Candidate> {
        let lane = lane(row)?;
        let attrs = row.get("attributes").and_then(Value::as_object);
        let (trace, span) = identification(row);
        let shared = trace.as_ref().and_then(|t| context.get(t));
        // A record with no usable time is not dated: the clock is never used to fill the gap.
        let timestamp = timestamp(row)?;
        let session = session_of(attrs).or_else(|| shared.and_then(|s| s.session.clone()));
        let response = value_text(object_get(attrs, "gen_ai.response.id")).or_else(|| shared.and_then(|s| s.response.clone()));
        let model = model_of(attrs).or_else(|| shared.and_then(|s| s.model.clone()))?;
        let agent = value_text(object_get(attrs, "gen_ai.agent.id")).or_else(|| shared.and_then(|s| s.agent.clone()));
        let (tally, unclassified) = counts(attrs);
        if tally.total() <= 0 && unclassified <= 0 {
            return None;
        }
        let turn = text_value(object_get(attrs, "turn.index")).or_else(|| text_value(object_get(attrs, "copilot_chat.turn.index")));
        let key = dedup_key(lane, trace.as_deref(), span.as_deref(), session.as_deref(), response.as_deref(), index, turn.as_deref(), ordinal);
        Some(Candidate {
            lane,
            key,
            timestamp,
            model,
            session,
            agent,
            tally,
            unclassified,
            // Neither a trace nor a response id means the record cannot be matched against another lane.
            partial: trace.is_none() && response.is_none(),
            trace,
            response,
        })
    }

    /// The product's own identity, never a clock reading: two different requests can share an instant.
    #[allow(clippy::too_many_arguments)]
    fn dedup_key(
        lane: Lane,
        trace: Option<&str>,
        span: Option<&str>,
        session: Option<&str>,
        response: Option<&str>,
        index: usize,
        turn: Option<&str>,
        ordinal: usize,
    ) -> String {
        match lane {
            Lane::ChatSpan | Lane::AgentSummarySpan => {
                if let (Some(trace), Some(span)) = (trace, span) {
                    return format!("{trace}:{span}");
                }
                if let (Some(session), Some(span)) = (session, span) {
                    return format!("span:{session}:{span}");
                }
                format!("span-unnamed:{ordinal}")
            }
            Lane::InferenceLog => {
                if let (Some(trace), Some(span)) = (trace, span) {
                    return format!("log:{trace}:{span}");
                }
                if let Some(response) = response {
                    return format!("log-response:{response}");
                }
                format!("log-unnamed:{ordinal}")
            }
            Lane::AgentTurnLog => match trace {
                Some(trace) => format!("agent-turn:{trace}:{}", turn.map(str::to_string).unwrap_or_else(|| format!("idx-{index}"))),
                None => format!("agent-turn-unnamed:{ordinal}"),
            },
        }
    }

    fn text_value(value: Option<&Value>) -> Option<String> {
        value_text(value).or_else(|| logio::count(value).map(|n| n.to_string()))
    }

    /// The four disjoint kinds, plus a bare total that names no kind.
    fn counts(attrs: Option<&Map<String, Value>>) -> (TokenTally, i64) {
        let count = |key: &str| logio::count(object_get(attrs, key)).unwrap_or(0);
        let input = count("gen_ai.usage.input_tokens");
        let output = count("gen_ai.usage.output_tokens");
        let cache_read = first_positive(attrs, &["gen_ai.usage.cache_read.input_tokens", "gen_ai.usage.cache_read_input_tokens"]);
        let cache_write = first_positive(
            attrs,
            &[
                "gen_ai.usage.cache_write.input_tokens",
                "gen_ai.usage.cache_creation.input_tokens",
                "gen_ai.usage.cache_write_input_tokens",
                "gen_ai.usage.cache_creation_input_tokens",
            ],
        );
        let reasoning = first_positive(attrs, &["gen_ai.usage.reasoning.output_tokens", "gen_ai.usage.reasoning_tokens"]);
        let names_a_kind = input > 0 || output > 0 || cache_read > 0 || cache_write > 0 || reasoning > 0;
        if !names_a_kind {
            // A total with no split is real work Pulse cannot place: unclassified, never input.
            let total = logio::count(object_get(attrs, "gen_ai.usage.total_tokens"))
                .or_else(|| logio::count(object_get(attrs, "gen_ai.usage.total.tokens")))
                .or_else(|| logio::count(object_get(attrs, "total_tokens")))
                .unwrap_or(0);
            return (TokenTally::default(), total.max(0));
        }
        // Input includes the cache read; remove it once. Reasoning stays inside output.
        let fresh_input = (input - cache_read.min(input)).max(0);
        let folded_output = if output > 0 { output } else { reasoning };
        (TokenTally::new(fresh_input, cache_write, cache_read, folded_output), 0)
    }

    fn first_positive(attrs: Option<&Map<String, Value>>, keys: &[&str]) -> i64 {
        keys.iter().map(|key| logio::count(object_get(attrs, key)).unwrap_or(0)).find(|value| *value > 0).unwrap_or(0)
    }

    /// The first usable timing key, in the format's own order.
    fn timestamp(row: &Map<String, Value>) -> Option<DateTime<Utc>> {
        if let Some(at) = logio::timestamp(row.get("startTime"), false) {
            return Some(at);
        }
        if let Some(end) = logio::timestamp(row.get("endTime"), false) {
            return Some(match duration(row) {
                Some(duration) => end - duration,
                None => end,
            });
        }
        for key in ["hrTime", "_hrTime"] {
            if let Some(Value::Array(pair)) = row.get(key) {
                if let Some(seconds) = pair.first().and_then(|v| logio::count(Some(v))) {
                    let nanos = pair.get(1).and_then(|v| logio::count(Some(v))).unwrap_or(0);
                    if let Some(at) = from_parts(seconds, nanos) {
                        return Some(at);
                    }
                }
            }
        }
        for key in ["time", "timestamp", "observedTimestamp"] {
            if let Some(at) = logio::timestamp(row.get(key), false) {
                return Some(at);
            }
        }
        logio::count(row.get("timeUnixNano")).and_then(|nanos| from_parts(0, nanos))
    }

    fn from_parts(seconds: i64, nanos: i64) -> Option<DateTime<Utc>> {
        let micros = seconds.checked_mul(1_000_000)?.checked_add(nanos / 1000)?;
        DateTime::<Utc>::from_timestamp_micros(micros)
    }

    /// A numeric `duration` is milliseconds; an object is an start and end pair.
    fn duration(row: &Map<String, Value>) -> Option<Duration> {
        let value = row.get("duration")?;
        if let Some(milliseconds) = logio::count(Some(value)) {
            return Duration::try_milliseconds(milliseconds);
        }
        let object = value.as_object()?;
        let start = logio::timestamp(object.get("startTime"), false)?;
        let end = logio::timestamp(object.get("endTime"), false)?;
        Some(end - start)
    }

    fn resolve(candidates: Vec<Candidate>) -> Vec<AgentUsageRecord> {
        // Same-key copies collapse first, then a higher lane drops a lower one that shares a trace or response.
        let mut by_key: HashMap<String, Candidate> = HashMap::new();
        for candidate in candidates {
            let key = candidate.key.clone();
            let merged = match by_key.get(&key) {
                Some(existing) => existing.merged(&candidate),
                None => candidate,
            };
            by_key.insert(key, merged);
        }
        let mut deduped: Vec<Candidate> = by_key.into_values().collect();
        deduped.sort_by(|a, b| a.key.cmp(&b.key));

        let mut traces: HashMap<Lane, HashSet<String>> = HashMap::new();
        let mut responses: HashMap<Lane, HashSet<String>> = HashMap::new();
        for candidate in &deduped {
            if let Some(trace) = &candidate.trace {
                traces.entry(candidate.lane).or_default().insert(trace.clone());
            }
            if let Some(response) = &candidate.response {
                responses.entry(candidate.lane).or_default().insert(response.clone());
            }
        }

        let suppressed = |candidate: &Candidate| {
            LANES.iter().any(|higher| {
                if higher.rank() >= candidate.lane.rank() {
                    return false;
                }
                let by_trace = candidate.trace.as_ref().is_some_and(|t| traces.get(higher).is_some_and(|set| set.contains(t)));
                let by_response = candidate.response.as_ref().is_some_and(|r| responses.get(higher).is_some_and(|set| set.contains(r)));
                by_trace || by_response
            })
        };

        deduped
            .into_iter()
            .filter(|candidate| !suppressed(candidate))
            .filter_map(|candidate| {
                let shape = Shape {
                    partial: candidate.partial,
                    session: candidate.session.clone(),
                    session_name: agent_label(candidate.agent.as_deref()),
                    dedup: Some(format!("copilot-otel:{}", candidate.key)),
                    ..Shape::default()
                };
                make(candidate.timestamp, &candidate.model, candidate.tally, candidate.unclassified, shape)
            })
            .collect()
    }
}

// MARK: - Desktop

mod desktop {
    use super::*;

    /// One session row of the desktop database.
    struct Row {
        id: String,
        title: Option<String>,
        model: Option<String>,
        input: i64,
        output: i64,
        cached: i64,
        reasoning: i64,
        created_at: Option<DateTime<Utc>>,
    }

    /// Token counters for one run or remainder. Reasoning is kept only to decide the partial flag: it is never
    /// placed in a bucket, because the store does not say whether it is already inside output.
    #[derive(Default, Clone, Copy)]
    struct Counts {
        input: i64,
        output: i64,
        cache_read: i64,
        cache_write: i64,
        reasoning: i64,
    }

    impl Counts {
        fn counted(&self) -> i64 {
            self.input + self.output + self.cache_read + self.cache_write
        }

        fn omitted_reasoning(&self) -> bool {
            self.reasoning > 0
        }

        fn add(&mut self, other: Counts) {
            self.input += other.input;
            self.output += other.output;
            self.cache_read += other.cache_read;
            self.cache_write += other.cache_write;
            self.reasoning += other.reasoning;
        }
    }

    pub(super) fn records(database: &Path) -> Vec<AgentUsageRecord> {
        let Some(connection) = sqlite::open(database) else { return Vec::new() };
        if sqlite::columns(&connection, "sessions").is_empty() {
            return Vec::new();
        }
        let sidecar = database.parent().unwrap_or_else(|| Path::new("")).join("session-state");
        let sql = sqlite::select(
            &connection,
            "sessions",
            &["id", "title", "model", "total_input_tokens", "total_output_tokens", "total_cached_tokens", "total_reasoning_tokens", "created_at"],
        );
        let mut rows = Vec::new();
        sqlite::each(&connection, &sql, |row| {
            if let Some(parsed) = row_of(row) {
                rows.push(parsed);
            }
        });
        rows.iter().flat_map(|row| read(row, &sidecar)).collect()
    }

    fn row_of(row: &rusqlite::Row) -> Option<Row> {
        let id = sqlite::text(row, 0)?;
        let input = sqlite::count(row, 3).unwrap_or(0);
        let output = sqlite::count(row, 4).unwrap_or(0);
        let cached = sqlite::count(row, 5).unwrap_or(0);
        let reasoning = sqlite::count(row, 6).unwrap_or(0);
        // A row with no tokens at all is not work.
        if input <= 0 && output <= 0 && cached <= 0 && reasoning <= 0 {
            return None;
        }
        Some(Row {
            id,
            title: sqlite::text(row, 1),
            model: non_blank(sqlite::text(row, 2).as_deref()),
            input,
            output,
            cached,
            reasoning,
            created_at: sqlite::text(row, 7).and_then(|text| flexible_text(&text)),
        })
    }

    fn read(row: &Row, sidecar: &Path) -> Vec<AgentUsageRecord> {
        let events: Vec<Value> = logio::json_lines(&sidecar.join(&row.id).join("events.jsonl")).collect();
        let mut running: HashMap<String, Counts> = HashMap::new();
        let mut applied = Counts::default();
        let mut tracked_model = row.model.clone();
        let mut workspace: Option<String> = None;
        let mut opened_with_start = false;
        let mut saw_first = false;
        let mut records = Vec::new();

        for (index, event) in events.iter().enumerate() {
            let kind = value_text(event.get("type"));
            if !saw_first {
                saw_first = true;
                opened_with_start = kind.as_deref() == Some("session.start");
            }
            match kind.as_deref() {
                Some("session.start") => {
                    if let Some(cwd) = workspace_of(event) {
                        workspace = Some(cwd);
                    }
                }
                Some("session.model_change") => {
                    let next = value_text(event.get("data").and_then(|d| d.get("newModel")));
                    if let Some(next) = next.filter(|n| n != "auto") {
                        tracked_model = Some(next);
                    }
                }
                Some("session.shutdown") => {
                    // A snapshot with no stated time cannot be placed; its tokens fall to the remainder.
                    let Some(timestamp) = event_timestamp(event) else { continue };
                    let data = event.get("data").and_then(Value::as_object);
                    let mut metrics: Vec<(&String, &Value)> = data
                        .and_then(|d| d.get("modelMetrics"))
                        .and_then(Value::as_object)
                        .map(|m| m.iter().collect())
                        .unwrap_or_default();
                    metrics.sort_by(|a, b| a.0.cmp(b.0));
                    let identity = event_identity(event, index);
                    for (tracker, entry) in metrics {
                        let Some(current) = usage(entry) else { continue };
                        let model = resolved_model(tracker, data, tracked_model.as_deref(), row.model.as_deref());
                        let previous = running.get(&model).copied();
                        let delta = subtract(current, previous);
                        running.insert(model.clone(), current);
                        // Without a session.start the earliest snapshot is an unknown baseline, not a delta.
                        if previous.is_none() && !opened_with_start {
                            continue;
                        }
                        let budgeted = budget(delta, applied, row);
                        applied.add(budgeted);
                        if budgeted.counted() <= 0 {
                            continue;
                        }
                        let shape = Shape {
                            aggregate: true,
                            // The run itself stated reasoning Pulse could not place: a known subset.
                            partial: delta.omitted_reasoning(),
                            session: Some(row.id.clone()),
                            title: row.title.clone(),
                            project: workspace.clone(),
                            dedup: Some(format!("copilot-desktop:{}:shutdown:{identity}:{model}", row.id)),
                            ..Shape::default()
                        };
                        if let Some(record) = make(timestamp, &model, tally_of(budgeted), 0, shape) {
                            records.push(record);
                        }
                    }
                }
                _ => {}
            }
        }

        // Whatever the snapshots did not account for is emitted once at the row's immutable creation time.
        let remainder = residual(applied, row);
        if remainder.counted() > 0 {
            if let Some(created) = row.created_at {
                let model = row.model.clone().or_else(|| tracked_model.clone()).unwrap_or_else(|| "auto".to_string());
                let shape = Shape {
                    aggregate: true,
                    partial: remainder.omitted_reasoning(),
                    session: Some(row.id.clone()),
                    title: row.title.clone(),
                    project: workspace.clone(),
                    dedup: Some(format!("copilot-desktop:{}:row", row.id)),
                    ..Shape::default()
                };
                if let Some(record) = make(created, &model, tally_of(remainder), 0, shape) {
                    records.push(record);
                }
            }
        }
        records
    }

    /// The record's tokens. Input is gross until here: the cache is taken out only for the record, so the
    /// prefix is not counted twice. Reasoning is never added to output.
    fn tally_of(counts: Counts) -> TokenTally {
        TokenTally::new(
            (counts.input - counts.cache_read - counts.cache_write).max(0),
            counts.cache_write,
            counts.cache_read,
            counts.output.max(0),
        )
    }

    /// The sidecar's next increment, capped so it can never carry a row past its own lifetime total.
    fn budget(delta: Counts, applied: Counts, row: &Row) -> Counts {
        Counts {
            input: delta.input.min((row.input - applied.input).max(0)),
            output: delta.output.min((row.output - applied.output).max(0)),
            cache_read: delta.cache_read.min((row.cached - applied.cache_read).max(0)),
            cache_write: delta.cache_write,
            reasoning: delta.reasoning.min((row.reasoning - applied.reasoning).max(0)),
        }
    }

    fn residual(applied: Counts, row: &Row) -> Counts {
        Counts {
            input: (row.input - applied.input).max(0),
            output: (row.output - applied.output).max(0),
            cache_read: (row.cached - applied.cache_read).max(0),
            cache_write: 0,
            reasoning: (row.reasoning - applied.reasoning).max(0),
        }
    }

    fn subtract(current: Counts, previous: Option<Counts>) -> Counts {
        let Some(previous) = previous else { return current };
        Counts {
            input: (current.input - previous.input).max(0),
            output: (current.output - previous.output).max(0),
            cache_read: (current.cache_read - previous.cache_read).max(0),
            cache_write: (current.cache_write - previous.cache_write).max(0),
            reasoning: (current.reasoning - previous.reasoning).max(0),
        }
    }

    fn workspace_of(event: &Value) -> Option<String> {
        value_text(event.get("data").and_then(|d| d.get("context")).and_then(|c| c.get("cwd")))
    }

    fn usage(entry: &Value) -> Option<Counts> {
        let usage = entry.get("usage")?.as_object()?;
        Some(Counts {
            input: clamped(usage.get("inputTokens")),
            output: clamped(usage.get("outputTokens")),
            cache_read: clamped(usage.get("cacheReadTokens")),
            cache_write: clamped(usage.get("cacheWriteTokens")),
            reasoning: clamped(usage.get("reasoningTokens")),
        })
    }

    /// A tracker key of `""` or `"auto"` defers to the run's current model, then the last change, then the row.
    fn resolved_model(tracker: &str, data: Option<&Map<String, Value>>, tracked: Option<&str>, row: Option<&str>) -> String {
        if !tracker.is_empty() && tracker != "auto" {
            return tracker.to_string();
        }
        if let Some(current) = value_text(object_get(data, "currentModel")) {
            return current;
        }
        if let Some(tracked) = tracked {
            return tracked.to_string();
        }
        if let Some(row) = row {
            return row.to_string();
        }
        "auto".to_string()
    }

    fn event_timestamp(event: &Value) -> Option<DateTime<Utc>> {
        flexible(event.get("timestamp")).or_else(|| flexible(event.get("data").and_then(|d| d.get("timestamp"))))
    }

    /// The envelope's own id, else a stable hash of the event, else its position.
    fn event_identity(event: &Value, index: usize) -> String {
        value_text(event.get("id")).unwrap_or_else(|| {
            if event.is_object() {
                stable_hash(&event.to_string())
            } else {
                format!("idx-{index}")
            }
        })
    }
}

// MARK: - VS Code

mod vscode {
    use super::*;

    pub(super) fn records(roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        let files: Vec<PathBuf> = logio::files(roots, &["jsonl"], &[], &[])
            .into_iter()
            .filter(|file| file.parent().is_some_and(|p| named(p, "chatSessions")))
            .collect();
        let mut records = Vec::new();
        for file in files {
            records.extend(records_of(&file));
        }
        records
    }

    fn records_of(file: &Path) -> Vec<AgentUsageRecord> {
        let session = file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let workspace = workspace_of(file);
        let requests = reconstruct(logio::json_lines(file));
        let mut used: HashSet<String> = HashSet::new();
        let mut records = Vec::new();
        for request in &requests {
            let Some(mut built) = record(request, &session, workspace.as_deref()) else { continue };
            built.deduplication_id = unique_id(built.deduplication_id.take(), &mut used);
            records.push(built);
        }
        records
    }

    /// Keeps the format's own key when unique, and separates a genuine collision with `#n`, so two distinct
    /// requests sharing a session and a millisecond both count.
    fn unique_id(base: Option<String>, used: &mut HashSet<String>) -> Option<String> {
        let base = base?;
        if used.insert(base.clone()) {
            return Some(base);
        }
        let mut suffix = 2;
        while used.contains(&format!("{base}#{suffix}")) {
            suffix += 1;
        }
        let id = format!("{base}#{suffix}");
        used.insert(id.clone());
        Some(id)
    }

    fn record(request: &Value, session: &str, workspace: Option<&str>) -> Option<AgentUsageRecord> {
        let metadata = request.get("result").and_then(|r| r.get("metadata")).and_then(Value::as_object);
        let resolved = value_text(object_get(metadata, "resolvedModel"));
        let model_id = value_text(request.get("modelId"));
        // Not a Copilot request: an editor-served model with neither a resolved model nor the product's prefix.
        if resolved.is_none() && !model_id.as_deref().is_some_and(|m| m.starts_with("copilot/")) {
            return None;
        }
        let model = resolved.or_else(|| stripped(model_id.as_deref())).unwrap_or_else(|| "auto".to_string());
        let prompt = logio::count(request.get("promptTokens"))
            .or_else(|| logio::count(object_get(metadata, "promptTokens")))
            .unwrap_or(0);
        let completion = logio::count(request.get("completionTokens"))
            .or_else(|| logio::count(object_get(metadata, "outputTokens")))
            .unwrap_or(0);
        let thinking = thinking_tokens(metadata);
        // Missing time is not a date: an untimed request is skipped, never placed at the epoch or on the clock.
        let at = logio::timestamp(request.get("timestamp"), true).or_else(|| logio::timestamp(object_get(metadata, "timestamp"), true))?;
        let tally = TokenTally::new(0, 0, 0, completion + thinking);
        if tally.total() <= 0 && prompt <= 0 {
            return None;
        }
        let shape = Shape {
            session: Some(session.to_string()),
            session_name: Some(session.to_string()),
            project: workspace.map(str::to_string),
            dedup: Some(format!("copilot-vscode:{session}:{}", milliseconds(at))),
            ..Shape::default()
        };
        // The prompt count holds the cached prefix, so it is unclassified, not fresh input.
        make(at, &model, tally, prompt, shape)
    }

    fn stripped(model_id: Option<&str>) -> Option<String> {
        let tail = model_id?.strip_prefix("copilot/")?;
        (!tail.is_empty()).then(|| tail.to_string())
    }

    fn thinking_tokens(metadata: Option<&Map<String, Value>>) -> i64 {
        let Some(Value::Array(rounds)) = object_get(metadata, "toolCallRounds") else { return 0 };
        rounds
            .iter()
            .filter_map(|round| round.get("thinking"))
            .map(|thinking| logio::count(thinking.get("tokens")).unwrap_or(0))
            .sum()
    }

    /// The workspace folder named by the sibling `<hash>/workspace.json`, as a Windows path where it is a file URI.
    fn workspace_of(file: &Path) -> Option<String> {
        let hash_directory = file.parent()?.parent()?;
        let object = logio::json(&hash_directory.join("workspace.json"))?;
        let uri = value_text(object.get("folder")).or_else(|| value_text(object.get("workspace")))?;
        Some(file_uri_path(&uri))
    }

    /// `file:///c%3A/Users/me` becomes `c:/Users/me`; a remote or an unknown form is returned as stated.
    pub(super) fn file_uri_path(uri: &str) -> String {
        let Some(rest) = uri.strip_prefix("file://") else { return uri.to_string() };
        let (host, path) = match rest.find('/') {
            Some(slash) => (&rest[..slash], &rest[slash..]),
            None => (rest, ""),
        };
        if !(host.is_empty() || host == "localhost") {
            return uri.to_string();
        }
        let decoded = percent_decode(path);
        let bytes = decoded.as_bytes();
        // A Windows drive: "/c:/..." reads as "c:/...".
        if bytes.len() >= 3 && bytes[0] == b'/' && bytes[1].is_ascii_alphabetic() && bytes[2] == b':' {
            return decoded[1..].to_string();
        }
        decoded
    }

    fn percent_decode(text: &str) -> String {
        let bytes = text.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index] == b'%' && index + 2 < bytes.len() {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok();
                if let Some(byte) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    out.push(byte);
                    index += 3;
                    continue;
                }
            }
            out.push(bytes[index]);
            index += 1;
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    /// Replays the append/patch log into the request array it describes.
    fn reconstruct(rows: impl Iterator<Item = Value>) -> Vec<Value> {
        let mut requests: Vec<Value> = Vec::new();
        for row in rows {
            let Some(kind) = logio::count(row.get("kind")) else { continue };
            match kind {
                0 => {
                    if let Some(appended) = row.get("v").and_then(|v| v.get("requests")).and_then(Value::as_array) {
                        requests.extend(appended.iter().cloned());
                    }
                }
                1 => {
                    let Some(path) = row.get("k").and_then(Value::as_array) else { continue };
                    if path.len() < 2 || path[0].as_str() != Some("requests") {
                        continue;
                    }
                    let Some(index) = logio::count(Some(&path[1])).and_then(|i| usize::try_from(i).ok()) else { continue };
                    if index >= requests.len() {
                        continue;
                    }
                    let value = row.get("v").cloned().unwrap_or(Value::Null);
                    if path.len() == 2 {
                        requests[index] = value;
                    } else {
                        mutate(&mut requests[index], &path[2..], value);
                    }
                }
                2 => {
                    let is_requests = row.get("k").and_then(Value::as_array).is_some_and(|p| p.len() == 1 && p[0].as_str() == Some("requests"));
                    if let (true, Some(appended)) = (is_requests, row.get("v").and_then(Value::as_array)) {
                        requests.extend(appended.iter().cloned());
                    }
                }
                _ => {}
            }
        }
        requests.into_iter().filter(Value::is_object).collect()
    }

    /// Writes `value` at a nested path, creating the containers the path names. A scalar, a missing key or an
    /// out-of-range index is not a path into anything, so the write is dropped.
    fn mutate(container: &mut Value, path: &[Value], value: Value) {
        let Some((head, rest)) = path.split_first() else {
            *container = value;
            return;
        };
        match container {
            Value::Array(array) => {
                let Some(index) = logio::count(Some(head)).and_then(|i| usize::try_from(i).ok()) else { return };
                if index >= array.len() {
                    return;
                }
                if rest.is_empty() {
                    array[index] = value;
                } else {
                    mutate(&mut array[index], rest, value);
                }
            }
            Value::Object(map) => {
                let Some(key) = head.as_str() else { return };
                if rest.is_empty() {
                    map.insert(key.to_string(), value);
                } else {
                    let mut child = map.get(key).cloned().unwrap_or_else(|| {
                        if logio::count(rest.first()).is_some() {
                            Value::Array(Vec::new())
                        } else {
                            Value::Object(Map::new())
                        }
                    });
                    mutate(&mut child, rest, value);
                    map.insert(key.to_string(), child);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use serde_json::json;

    use super::*;
    use crate::spend::calendar::Calendar;
    use crate::spend::prices::PriceTable;
    use crate::spend::sources::{read_agent, SpendAgent};
    use crate::spend::sqlite::tests::database;

    fn write_lines(path: &Path, lines: &[Value]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let text: Vec<String> = lines.iter().map(Value::to_string).collect();
        std::fs::write(path, text.join("\n")).unwrap();
    }

    fn total(home: &Path) -> (i64, bool) {
        let cache = tempfile::tempdir().unwrap();
        let prices: PriceTable = HashMap::new();
        let ledger = read_agent(SpendAgent::Copilot, &Sources::new(home), cache.path(), &Calendar::utc(2), &prices);
        (ledger.days.iter().map(|d| d.tokens).sum(), ledger.has_partial_counts)
    }

    #[test]
    fn the_desktop_lifetime_is_differenced_from_the_sidecar_and_the_rest_lands_at_creation() {
        let home = tempfile::tempdir().unwrap();
        let now = Utc::now().timestamp();
        std::fs::create_dir_all(home.path().join(".copilot")).unwrap();
        database(
            &home.path().join(".copilot").join("data.db"),
            &[
                "CREATE TABLE sessions (id TEXT, title TEXT, model TEXT, total_input_tokens INTEGER, total_output_tokens INTEGER, total_cached_tokens INTEGER, total_reasoning_tokens INTEGER, created_at TEXT)",
                "INSERT INTO sessions VALUES ('d1', 'Desk', 'gpt-5', 100, 20, 30, 5, '2026-03-01T10:00:00Z')",
            ],
        );
        write_lines(
            &home.path().join(".copilot").join("session-state").join("d1").join("events.jsonl"),
            &[
                json!({"type": "session.start", "data": {"context": {"cwd": "C:\\Users\\me\\code\\Pulse"}}}),
                json!({"type": "session.shutdown", "timestamp": now, "data": {"modelMetrics": {"gpt-5": {"usage": {
                    "inputTokens": 60, "outputTokens": 10, "cacheReadTokens": 20, "cacheWriteTokens": 0, "reasoningTokens": 3}}}}}),
            ],
        );
        // Lifetime: 100 input (30 of it cached) + 20 output = 120 counted; the snapshot covers 70, the
        // remainder (50) lands at creation. Reasoning was stated, so the record is partial.
        assert_eq!(total(home.path()), (120, true));
    }

    #[test]
    fn an_otel_span_wins_its_trace_and_the_vscode_request_at_its_instant_is_dropped() {
        let home = tempfile::tempdir().unwrap();
        let at = Utc::now().timestamp() / 60 * 60;
        let trace = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        write_lines(
            &home.path().join(".copilot").join("otel").join("spans.jsonl"),
            &[
                json!({"type": "span", "name": "chat gpt-5", "traceId": trace, "spanId": "bbbbbbbbbbbbbbbb", "startTime": at,
                       "attributes": {"gen_ai.operation.name": "chat", "gen_ai.response.model": "gpt-5", "gen_ai.conversation.id": "sess-o",
                                      "gen_ai.usage.input_tokens": 100, "gen_ai.usage.cache_read.input_tokens": 40, "gen_ai.usage.output_tokens": 10}}),
                // The inference log of the same trace is suppressed by the chat span above it.
                json!({"type": "log", "traceId": trace, "timestamp": at, "attributes": {"event.name": "gen_ai.client.inference.operation.details",
                       "gen_ai.usage.input_tokens": 500, "gen_ai.usage.output_tokens": 50}}),
            ],
        );
        let chats = home.path().join("AppData").join("Roaming").join("Code").join("User").join("workspaceStorage").join("hash1");
        write_lines(&chats.join("workspace.json"), &[json!({"folder": "file:///C%3A/Users/me/code/Pulse"})]);
        let session = chats.join("chatSessions").join("sess-o.jsonl");
        write_lines(
            &session,
            &[
                // Request one is at the span's instant and is dropped; request two is later and counts its prompt
                // as unclassified and its completion as output; request three has no time and is skipped.
                json!({"kind": 0, "v": {"requests": [{"timestamp": at * 1000, "modelId": "copilot/gpt-5", "promptTokens": 999, "completionTokens": 5}]}}),
                json!({"kind": 2, "k": ["requests"], "v": [{"timestamp": (at + 60) * 1000, "promptTokens": 20, "completionTokens": 5}]}),
                json!({"kind": 1, "k": ["requests", 1, "result"], "v": {"metadata": {"resolvedModel": "gpt-5"}}}),
                json!({"kind": 2, "k": ["requests"], "v": [{"modelId": "copilot/gpt-5", "completionTokens": 1}]}),
            ],
        );
        // OTEL: 60 fresh + 40 cached + 10 output = 110. VS Code request two: 20 unclassified + 5 output = 25.
        assert_eq!(total(home.path()), (135, false));
    }

    #[test]
    fn agent_labels_and_percent_decoded_workspace_paths_read_as_copilot_writes_them() {
        assert_eq!(agent_label(Some("github.copilot.default")).as_deref(), Some("GitHub Copilot"));
        assert_eq!(agent_label(Some("github.copilot.explore.fast")).as_deref(), Some("Explore-Fast"));
        assert_eq!(agent_label(Some("Plugin:team:slug")).as_deref(), Some("Plugin: Team: Slug"));
        assert_eq!(vscode::file_uri_path("file:///c%3A/Users/me/code"), "c:/Users/me/code");
        assert_eq!(stable_hash("x"), stable_hash("x"));
    }

    #[test]
    fn a_missing_store_is_an_empty_ledger() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(total(home.path()), (0, false));
    }
}
