// Ported from upstream Sources/Pulse/Usage/Readers/AntigravityCLIReader.swift.
//! Antigravity CLI's conversations: one SQLite database per conversation under
//! `<GEMINI_CLI_HOME or ~/.gemini>/antigravity-cli/conversations/`. The IDE reads the same store
//! shape from its own folders (`antigravity_ide.rs`), through the same code here.
//!
//! The token data is in protobuf blobs, and no `.proto` ships with the CLI, so the wire format is
//! decoded here by hand (`wire`). A truncated buffer, an overlong varint, a length that runs past the
//! end, an unknown wire type and a zero field number all fail the whole message: nothing is read out of
//! bounds and a partial message is never used.
//!
//! **Only fields whose meaning is established are counted.** Usage field `#2` (newly processed input),
//! `#5` (cache read), `#9` (output) and `#10` (thinking) have an observed meaning; `#9 + #10` is the
//! output, so reasoning is counted once. `#1` is read by nothing: its meaning is not established. A
//! turn whose counters are all zero is dropped; nothing is estimated from text or cost.
//!
//! **Every record is partial.** `#1` is excluded and there is no whole-usage total to reconcile against,
//! so the read is a known subset. The flag changes no count.
//!
//! **Timestamps need evidence.** Only the explicit per-generation timestamp (`#9.#4`) and the `steps`
//! table's timestamp are used; the unknown `#9.#10` bytes never become a time. A generation with no such
//! timestamp falls back to the conversation's created-at and is marked aggregate. A file's modification
//! date is never used.
//!
//! Where it lives on Windows: `%GEMINI_CLI_HOME%`, else `%USERPROFILE%\.gemini`, then the `antigravity-cli`
//! folder under it. Unverified on a PC.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use super::SpendSource;
use crate::provider::Provider;
use crate::spend::logio;
use crate::spend::record::AgentUsageRecord;
use crate::spend::sqlite;
use crate::spend::tally::TokenTally;
use crate::spend::transcripts::Sources;

pub struct AntigravityCli;

impl SpendSource for AntigravityCli {
    fn raw(&self) -> &'static str {
        "antigravityCLI"
    }
    fn source_id(&self) -> &'static str {
        "antigravity-cli"
    }
    fn display_name(&self) -> &'static str {
        "Antigravity CLI"
    }
    fn card_provider(&self) -> Option<Provider> {
        Some(Provider::Antigravity)
    }
    fn icon(&self) -> Option<&'static str> {
        Some("antigravity")
    }
    fn env_vars(&self) -> &'static [&'static str] {
        &["GEMINI_CLI_HOME"]
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        conversation_roots(sources, &["antigravity-cli"])
    }
    fn records(&self, roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        records_in(roots)
    }
}

/// The `conversations` folder under each named folder of the Gemini root (`$GEMINI_CLI_HOME`, else
/// `~/.gemini`), whether or not it exists.
pub(crate) fn conversation_roots(sources: &Sources, folders: &[&str]) -> Vec<PathBuf> {
    // WINDOWS-PATH: unverified (%USERPROFILE%\.gemini)
    let root = sources.var("GEMINI_CLI_HOME").unwrap_or_else(|| sources.home.join(".gemini"));
    folders.iter().map(|folder| root.join(folder).join("conversations")).collect()
}

/// Every conversation database under the roots, read.
pub(crate) fn records_in(roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
    logio::files(roots, &["db"], &[], &[]).iter().flat_map(|file| read(file)).collect()
}

/// The label the routing layer shows instead of a model's identity.
const ROUTING_LABEL: &str = "gemini-default";

/// Labels whose model identity is verified. Server-supplied and localisable, so an unknown label is
/// never guessed at.
const LABEL_TABLE: [(&str, &str); 3] = [
    ("Gemini 3.5 Flash (Low)", "gemini-3.5-flash-extra-low"),
    ("Gemini 3.5 Flash (Medium)", "gemini-3.5-flash-medium"),
    ("Gemini 3.5 Flash (High)", "gemini-3.5-flash-high"),
];

#[derive(Default)]
struct Anchor {
    created_at: Option<DateTime<Utc>>,
    workspace: Option<String>,
}

#[derive(Default)]
struct Steps {
    by_response: HashMap<String, DateTime<Utc>>,
    by_index: HashMap<i64, DateTime<Utc>>,
}

#[derive(Default, Clone)]
struct Usage {
    /// `#2`: newly-processed (non-cached) input.
    fresh: i64,
    cache_read: i64,
    output: i64,
    reasoning: i64,
    response_id: Option<String>,
}

impl Usage {
    fn counted(&self) -> i64 {
        self.fresh + self.cache_read + self.output + self.reasoning
    }
}

struct Generation {
    index: i64,
    model: Option<String>,
    label: Option<String>,
    usage: Option<Usage>,
    explicit: Option<DateTime<Utc>>,
}

/// One conversation database.
fn read(file: &Path) -> Vec<AgentUsageRecord> {
    let Some(connection) = sqlite::open(file) else { return Vec::new() };
    let session = file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let anchor = trajectory(&connection);
    let steps = step_timestamps(&connection);
    let generations = generations(&connection);

    // The models a label was seen with, and the file's single model if it has exactly one. Used only to
    // recover an identity the routing label threw away.
    let mut labels: HashMap<String, HashSet<String>> = HashMap::new();
    let mut models: HashSet<String> = HashSet::new();
    for generation in &generations {
        let Some(model) = generation.model.as_deref().filter(|m| *m != ROUTING_LABEL) else { continue };
        models.insert(model.to_string());
        if let Some(label) = &generation.label {
            labels.entry(label.clone()).or_default().insert(model.to_string());
        }
    }
    let sole_model = (models.len() == 1).then(|| models.iter().next().cloned()).flatten();

    let mut seen: HashSet<String> = HashSet::new();
    let mut records = Vec::new();
    for generation in &generations {
        let Some(usage) = generation.usage.as_ref().filter(|u| u.counted() > 0) else { continue };
        if let Some(response) = &usage.response_id {
            if !seen.insert(response.clone()) {
                continue;
            }
        }
        // An exact event time, or the session anchor as an aggregate. The unknown `#9.#10` bytes never
        // become a turn time.
        let event = generation
            .explicit
            .or_else(|| usage.response_id.as_ref().and_then(|r| steps.by_response.get(r).copied()))
            .or_else(|| steps.by_index.get(&generation.index).copied());
        let Some(at) = event.or(anchor.created_at) else { continue };

        let model = resolved_model(generation, &labels, sole_model.as_deref());
        let tally = TokenTally { input: usage.fresh, cache_read: usage.cache_read, output: usage.output + usage.reasoning, ..TokenTally::default() };
        let mut record = AgentUsageRecord::new(at, &model, tally).session(&session).aggregate(event.is_none());
        record.project = anchor.workspace.clone();
        record.deduplication_id = Some(match &usage.response_id {
            Some(response) => format!("antigravity:{session}:{response}"),
            None => format!("antigravity:{session}:{}", generation.index),
        });
        record.is_partial = true;
        records.push(record);
    }
    records
}

/// The model identity: the machine id first; then a sibling row's id recovered through the label; then
/// the file's single model; then the verified label table; then `unknown`. No vendor default is invented.
fn resolved_model(generation: &Generation, labels: &HashMap<String, HashSet<String>>, sole: Option<&str>) -> String {
    if let Some(model) = generation.model.as_deref().filter(|m| *m != ROUTING_LABEL) {
        return model.to_string();
    }
    if let Some(label) = &generation.label {
        if let Some(models) = labels.get(label) {
            if models.len() == 1 {
                if let Some(only) = models.iter().next() {
                    return only.clone();
                }
            }
        }
    }
    if let Some(sole) = sole {
        return sole.to_string();
    }
    if let Some(label) = &generation.label {
        if let Some((_, mapped)) = LABEL_TABLE.iter().find(|(l, _)| l == label) {
            return (*mapped).to_string();
        }
    }
    generation.model.clone().unwrap_or_else(|| "unknown".to_string())
}

fn trajectory(connection: &rusqlite::Connection) -> Anchor {
    let mut anchor = Anchor::default();
    sqlite::each(connection, "SELECT data FROM trajectory_metadata_blob LIMIT 1", |row| {
        let Some(message) = sqlite::blob(row, 0).and_then(|b| wire::decode(&b)) else { return };
        anchor.created_at = timestamp(message.nested(2).as_ref());
        if let Some(uri) = message.nested(1).and_then(|folder| folder.string(1)) {
            anchor.workspace = Some(path_from_uri(&uri));
        }
    });
    anchor
}

fn step_timestamps(connection: &rusqlite::Connection) -> Steps {
    let mut steps = Steps::default();
    sqlite::each(connection, "SELECT metadata FROM steps WHERE step_type = 15", |row| {
        let Some(message) = sqlite::blob(row, 0).and_then(|b| wire::decode(&b)) else { return };
        let Some(at) = timestamp(message.nested(1).as_ref()) else { return };
        if let Some(response) = message.nested(9).and_then(|m| m.string(11)) {
            steps.by_response.insert(response, at);
        }
        if let Some(index) = message.nested(20).and_then(|m| m.varint(3)) {
            steps.by_index.insert(saturating(index), at);
        }
    });
    steps
}

fn generations(connection: &rusqlite::Connection) -> Vec<Generation> {
    let mut generations = Vec::new();
    sqlite::each(connection, "SELECT idx, data FROM gen_metadata ORDER BY idx", |row| {
        let Some(index) = sqlite::count(row, 0) else { return };
        let Some(message) = sqlite::blob(row, 1).and_then(|b| wire::decode(&b)) else { return };
        let Some(chat) = message.nested(1) else { return };

        let explicit = timestamp(chat.nested(9).and_then(|m| m.nested(4)).as_ref());
        let usage = chat.nested(4).map(|u| Usage {
            fresh: saturating(u.varint(2).unwrap_or(0)),
            cache_read: saturating(u.varint(5).unwrap_or(0)),
            output: saturating(u.varint(9).unwrap_or(0)),
            reasoning: saturating(u.varint(10).unwrap_or(0)),
            response_id: u.string(11),
        });
        generations.push(Generation { index, model: chat.string(19), label: chat.string(21), usage, explicit });
    });
    generations
}

/// `#1` seconds and `#2` nanoseconds. A non-positive or absurd seconds value is not a time.
fn timestamp(message: Option<&wire::Message>) -> Option<DateTime<Utc>> {
    let message = message?;
    let seconds = message.varint(1).filter(|s| *s > 0 && *s < 1_000_000_000_000)?;
    let nanos = message.varint(2).unwrap_or(0);
    let extra = (nanos / 1_000_000_000) as i64;
    DateTime::from_timestamp(seconds as i64 + extra, (nanos % 1_000_000_000) as u32)
}

/// A count from a protobuf varint, clamped to `i64::MAX`.
fn saturating(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// A `file://` URI as a path, or the string unchanged when it is not one. The percent-escapes are
/// decoded; a drive letter after the `file:///` is kept as Windows writes it.
fn path_from_uri(uri: &str) -> String {
    let Some(rest) = uri.strip_prefix("file://") else { return uri.to_string() };
    let rest = match rest.strip_prefix('/') {
        Some(after) if after.len() >= 2 && after.as_bytes()[1] == b':' => after,
        _ => rest,
    };
    percent_decode(rest)
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if let Some(byte) = text.get(i + 1..i + 3).and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A minimal, bounds-checked protobuf wire decoder. Only the wire format is implemented: the message
/// schemas differ per client and none of their `.proto` files ship. An unsupported wire type (the
/// deprecated groups) or any malformed byte fails the whole message rather than returning a partial one.
pub(crate) mod wire {
    use std::collections::HashMap;

    #[derive(Debug, Clone, PartialEq)]
    pub enum Value {
        Varint(u64),
        Fixed64(u64),
        Length(Vec<u8>),
        Fixed32(u32),
    }

    #[derive(Debug, Clone, Default, PartialEq)]
    pub struct Message {
        fields: HashMap<u64, Vec<Value>>,
    }

    impl Message {
        fn first(&self, number: u64) -> Option<&Value> {
            self.fields.get(&number)?.first()
        }

        pub fn varint(&self, number: u64) -> Option<u64> {
            match self.first(number)? {
                Value::Varint(value) => Some(*value),
                _ => None,
            }
        }

        /// A length-delimited field as a trimmed, non-empty UTF-8 string.
        pub fn string(&self, number: u64) -> Option<String> {
            let bytes = match self.first(number)? {
                Value::Length(bytes) => bytes,
                _ => return None,
            };
            let text = String::from_utf8(bytes.clone()).ok()?;
            let trimmed = text.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        }

        /// A length-delimited field decoded as a nested message.
        pub fn nested(&self, number: u64) -> Option<Message> {
            match self.first(number)? {
                Value::Length(bytes) => decode(bytes),
                _ => None,
            }
        }
    }

    pub fn decode(bytes: &[u8]) -> Option<Message> {
        let mut reader = Reader { bytes, index: 0 };
        let mut message = Message::default();
        while reader.index < bytes.len() {
            let (number, value) = reader.field()?;
            message.fields.entry(number).or_default().push(value);
        }
        Some(message)
    }

    struct Reader<'a> {
        bytes: &'a [u8],
        index: usize,
    }

    impl Reader<'_> {
        fn field(&mut self) -> Option<(u64, Value)> {
            let tag = self.varint()?;
            let number = tag >> 3;
            // Field zero is not a field.
            if number == 0 {
                return None;
            }
            let value = match tag & 7 {
                0 => Value::Varint(self.varint()?),
                1 => Value::Fixed64(self.fixed(8)?),
                2 => {
                    let length = self.varint()?;
                    let remaining = (self.bytes.len() - self.index) as u64;
                    if length > remaining {
                        return None;
                    }
                    let end = self.index + length as usize;
                    let slice = self.bytes[self.index..end].to_vec();
                    self.index = end;
                    Value::Length(slice)
                }
                5 => Value::Fixed32(self.fixed(4)? as u32),
                // Wire types 3 and 4 are the deprecated groups.
                _ => return None,
            };
            Some((number, value))
        }

        /// A base-128 varint, refused past ten bytes or when the tenth would overflow.
        fn varint(&mut self) -> Option<u64> {
            let mut result: u64 = 0;
            for i in 0..10u32 {
                let byte = *self.bytes.get(self.index)?;
                self.index += 1;
                let shift = 7 * i;
                if shift == 63 && byte > 1 {
                    return None;
                }
                result |= u64::from(byte & 0x7F) << shift;
                if byte & 0x80 == 0 {
                    return Some(result);
                }
            }
            None
        }

        /// A little-endian fixed-width integer of `width` bytes (4 or 8).
        fn fixed(&mut self, width: usize) -> Option<u64> {
            let end = self.index.checked_add(width)?;
            let slice = self.bytes.get(self.index..end)?;
            self.index = end;
            Some(slice.iter().rev().fold(0u64, |acc, byte| (acc << 8) | u64::from(*byte)))
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn varint(mut value: u64) -> Vec<u8> {
            let mut out = Vec::new();
            loop {
                let byte = (value & 0x7F) as u8;
                value >>= 7;
                if value == 0 {
                    out.push(byte);
                    return out;
                }
                out.push(byte | 0x80);
            }
        }

        #[test]
        fn fields_nest_and_repeat_and_a_malformed_buffer_fails_whole() {
            // #1 varint 300, #2 length "hi", #3 nested { #1 varint 7 }
            let mut bytes = vec![0x08];
            bytes.extend(varint(300));
            bytes.extend([0x12, 2, b'h', b'i']);
            bytes.extend([0x1A, 2, 0x08, 7]);
            let message = decode(&bytes).unwrap();
            assert_eq!(message.varint(1), Some(300));
            assert_eq!(message.string(2).as_deref(), Some("hi"));
            assert_eq!(message.nested(3).unwrap().varint(1), Some(7));

            // A length that runs past the end.
            assert!(decode(&[0x12, 9, b'x']).is_none());
            // A truncated varint.
            assert!(decode(&[0x08, 0x80]).is_none());
            // An overlong varint (eleven continuation bytes).
            assert!(decode(&[0x08, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x01]).is_none());
            // Field zero, a group wire type, and a truncated fixed64.
            assert!(decode(&[0x00, 0x01]).is_none());
            assert!(decode(&[0x0B]).is_none());
            assert!(decode(&[0x09, 1, 2, 3]).is_none());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::wire::{decode, Message};
    use super::*;
    use crate::spend::calendar::Calendar;
    use crate::spend::prices::PriceTable;
    use crate::spend::sources::{read_agent, SpendAgent};
    use crate::spend::sqlite::tests::database;

    /// Protobuf bytes built by hand for the tests.
    fn varint(mut value: u64) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let byte = (value & 0x7F) as u8;
            value >>= 7;
            if value == 0 {
                out.push(byte);
                return out;
            }
            out.push(byte | 0x80);
        }
    }
    fn int(number: u64, value: u64) -> Vec<u8> {
        [varint(number << 3), varint(value)].concat()
    }
    fn bytes(number: u64, value: &[u8]) -> Vec<u8> {
        [varint((number << 3) | 2), varint(value.len() as u64), value.to_vec()].concat()
    }
    fn text(number: u64, value: &str) -> Vec<u8> {
        bytes(number, value.as_bytes())
    }
    fn stamp(seconds: u64) -> Vec<u8> {
        int(1, seconds)
    }
    fn hex(data: &[u8]) -> String {
        format!("X'{}'", data.iter().map(|b| format!("{b:02X}")).collect::<String>())
    }

    /// One generation: chat #19 model, #21 label, #4 usage { #2, #5, #9, #10, #11 }, optional #9.#4 time.
    fn generation(model: &str, label: &str, usage: (u64, u64, u64, u64), response: &str, at: Option<u64>) -> Vec<u8> {
        let mut usage_bytes = [int(2, usage.0), int(5, usage.1), int(9, usage.2), int(10, usage.3)].concat();
        usage_bytes.extend(text(11, response));
        let mut chat = [bytes(4, &usage_bytes), text(19, model), text(21, label)].concat();
        if let Some(at) = at {
            chat.extend(bytes(9, &bytes(4, &stamp(at))));
        }
        bytes(1, &chat)
    }

    fn conversation(home: &Path, id: &str, statements: Vec<String>) -> PathBuf {
        let folder = home.join(".gemini").join("antigravity-cli").join("conversations");
        std::fs::create_dir_all(&folder).unwrap();
        let file = folder.join(format!("{id}.db"));
        let mut sql = vec![
            "CREATE TABLE trajectory_metadata_blob (data BLOB)".to_string(),
            "CREATE TABLE steps (step_type INTEGER, metadata BLOB)".to_string(),
            "CREATE TABLE gen_metadata (idx INTEGER, data BLOB)".to_string(),
        ];
        sql.extend(statements);
        database(&file, &sql.iter().map(String::as_str).collect::<Vec<_>>());
        file
    }

    fn run(home: &Path) -> Vec<AgentUsageRecord> {
        AntigravityCli.records(&AntigravityCli.inputs(&Sources::new(home)))
    }

    #[test]
    fn counted_fields_land_in_the_kinds_with_reasoning_in_output_and_every_record_partial() {
        let home = tempfile::tempdir().unwrap();
        let generated = generation("gemini-x", "label-a", (100, 40, 20, 5), "r1", Some(1_789_372_800));
        conversation(home.path(), "c1", vec![format!("INSERT INTO gen_metadata VALUES (0, {})", hex(&generated))]);

        let records = run(home.path());
        assert_eq!(records.len(), 1);
        let record = &records[0];
        assert_eq!(record.model, "gemini-x");
        assert_eq!(record.tally, TokenTally::new(100, 0, 40, 25));
        assert!(record.is_partial);
        assert!(!record.is_aggregate, "an explicit time is an event");
        assert_eq!(record.timestamp.timestamp(), 1_789_372_800);
        assert_eq!(record.deduplication_id.as_deref(), Some("antigravity:c1:r1"));
    }

    #[test]
    fn a_turn_without_its_own_time_uses_the_step_then_the_conversation_as_an_aggregate() {
        let home = tempfile::tempdir().unwrap();
        // The step's own time (#1) and its response id (#9.#11).
        let with_response = [bytes(1, &stamp(1_789_000_000)), bytes(9, &text(11, "r2"))].concat();
        let trajectory = [bytes(2, &stamp(1_788_000_000)), bytes(1, &text(1, "file:///C:/Users/me/code/Pulse"))].concat();
        conversation(
            home.path(),
            "c2",
            vec![
                format!("INSERT INTO trajectory_metadata_blob VALUES ({})", hex(&trajectory)),
                format!("INSERT INTO steps VALUES (15, {})", hex(&with_response)),
                format!("INSERT INTO gen_metadata VALUES (0, {})", hex(&generation("m", "l", (10, 0, 0, 0), "r2", None))),
                format!("INSERT INTO gen_metadata VALUES (1, {})", hex(&generation("m", "l", (0, 0, 0, 0), "r3", None))),
                format!("INSERT INTO gen_metadata VALUES (2, {})", hex(&generation("m", "l", (1, 0, 0, 0), "r4", None))),
            ],
        );
        let records = run(home.path());
        let r2 = records.iter().find(|r| r.deduplication_id.as_deref() == Some("antigravity:c2:r2")).unwrap();
        assert_eq!(r2.timestamp.timestamp(), 1_789_000_000, "the step's time");
        assert!(!r2.is_aggregate);
        assert_eq!(r2.project.as_deref(), Some("C:/Users/me/code/Pulse"));
        // No step and no time of its own: the conversation's created-at, as an aggregate.
        let r4 = records.iter().find(|r| r.deduplication_id.as_deref() == Some("antigravity:c2:r4")).unwrap();
        assert_eq!(r4.timestamp.timestamp(), 1_788_000_000);
        assert!(r4.is_aggregate);
        // A generation with no counted usage is no record.
        assert_eq!(records.len(), 2);
    }

    #[test]
    fn a_repeated_response_counts_once_and_the_routing_label_takes_the_identity_of_its_siblings() {
        let home = tempfile::tempdir().unwrap();
        conversation(
            home.path(),
            "c3",
            vec![
                format!("INSERT INTO gen_metadata VALUES (0, {})", hex(&generation("gemini-real", "Gemini 3.5 Flash (Low)", (5, 0, 0, 0), "dup", Some(1_789_372_800)))),
                format!("INSERT INTO gen_metadata VALUES (1, {})", hex(&generation("gemini-default", "Gemini 3.5 Flash (Low)", (7, 0, 0, 0), "dup", Some(1_789_372_801)))),
                format!("INSERT INTO gen_metadata VALUES (2, {})", hex(&generation("gemini-default", "Gemini 3.5 Flash (Low)", (9, 0, 0, 0), "new", Some(1_789_372_802)))),
            ],
        );
        let records = run(home.path());
        assert_eq!(records.len(), 2, "the duplicate response is one turn");
        assert!(records.iter().all(|r| r.model == "gemini-real"), "the routing label is recovered from the sibling");
    }

    #[test]
    fn a_missing_store_is_empty_and_the_ide_folders_are_separate_inputs() {
        let home = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::AntigravityCli, &Sources::new(home.path()), cache.path(), &Calendar::utc(2), &PriceTable::new());
        assert!(ledger.days.is_empty());

        let ide = crate::spend::sources::antigravity_ide::AntigravityIde.inputs(&Sources::new(home.path()));
        assert_eq!(ide.len(), 2);
        assert!(ide.iter().any(|p| p.ends_with("antigravity/conversations")));
        assert!(ide.iter().any(|p| p.ends_with("antigravity-ide/conversations")));
    }

    #[test]
    fn the_wire_fixtures_decode_to_the_fields_they_were_built_from() {
        let message: Message = decode(&generation("m", "l", (1, 2, 3, 4), "r", None)).unwrap();
        assert_eq!(message.nested(1).unwrap().nested(4).unwrap().varint(9), Some(3));
    }
}
