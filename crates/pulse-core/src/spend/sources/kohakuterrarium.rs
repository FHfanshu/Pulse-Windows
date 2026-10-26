// Windows difference: no upstream source; KohakuTerrarium (kt) sessions, added at the owner's request
//! KohakuTerrarium (`kt`), the agent framework, which keeps each session in one SQLite database:
//! `%USERPROFILE%\.kohakuterrarium\sessions\<id>.kohakutr`.
//!
//! Every table is `key BLOB, value BLOB`, and a value is a KohakuVault blob: a ten-byte header
//! (`89 4B 01 <encoding> 00 00 00 00 56 4B`, encoding 4 for MessagePack, 6 for UTF-8 text), then
//! the value itself. `meta` holds the session, `events` holds every frame of it under
//! `<agent>:e<number>`, and a model call is its own frame:
//!
//! ```text
//! { "type": "token_usage", "ts": 1792941771.79, "event_id": 36,
//!   "prompt_tokens": 20950, "cached_tokens": 19840,
//!   "completion_tokens": 531, "total_tokens": 21481 }
//! ```
//!
//! **`prompt_tokens` includes `cached_tokens`**, the OpenAI arrangement kt passes through
//! unchanged, so fresh input is the difference and `cached_tokens` is the cache read. kt counts no
//! cache write, so that kind stays zero.
//!
//! **`turn_token_usage` is not counted.** It is one turn's rollup of the same `token_usage`
//! frames, so counting both would count every token twice; the `turn_rollup` table holds the same
//! sums and is not read either.
//!
//! **The event key names the creature** (`root:e000019`), because the usage frame itself does not.
//! The model comes from that creature's `activity:session_info` frame — the one a model switch
//! re-emits — else from `meta.config_snapshot.model`, which kt leaves empty for a creature that
//! was given a profile rather than a model. A call with no model anywhere is counted under the
//! placeholder below and never costed: another creature's rate would be a guess.
//!
//! Where it lives on Windows: `KT_SESSION_DIR` when set (kt's own override, highest precedence),
//! else `$KT_CONFIG_DIR\sessions` when kt was re-homed, else `%USERPROFILE%\.kohakuterrarium\sessions`
//! (checked on a real machine). `session_id` and `pwd` (the project) come from `meta`; the title is
//! `meta.name` where the session was given one, else its first `user_input`. One frame found in two
//! copies of a session counts once, by `(session_id, event key)`.
//!
//! **`sessions\mirror` is not read.** It holds copies of the sessions other machines of a kt lab
//! ran (their `meta.hostname` is not this PC's), and of this PC's own, already read from the folder
//! above it: counting it would put another machine's spend on this one. The sidecars beside a session —
//! `*.drives`, `*.lock`, `*-wal`, `*-shm` — and `.kt-index.kvault` are not sessions and are not
//! opened.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use rmpv::Value;

use super::{push_unique, SpendSource};
use crate::spend::record::AgentUsageRecord;
use crate::spend::sqlite;
use crate::spend::tally::TokenTally;
use crate::spend::titles::title_from;
use crate::spend::transcripts::Sources;

/// The placeholder model: no price list can match it.
const MODEL: &str = "kohakuterrarium (unnamed)";

/// KohakuVault's header ahead of every value: `89 4B`, the value's encoding, four flag bytes
/// (set only by compression and encryption), then the `VK` tag.
const HEADER: usize = 10;

pub struct KohakuTerrarium;

impl SpendSource for KohakuTerrarium {
    fn raw(&self) -> &'static str {
        "kohakuTerrarium"
    }
    fn source_id(&self) -> &'static str {
        "kohakuterrarium"
    }
    fn display_name(&self) -> &'static str {
        "KohakuTerrarium"
    }
    fn env_vars(&self) -> &'static [&'static str] {
        &["KT_SESSION_DIR", "KT_CONFIG_DIR"]
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        let mut roots = Vec::new();
        // `KT_SESSION_DIR` is where kt puts sessions when it is set, and it wins outright.
        if let Some(directory) = sources.var("KT_SESSION_DIR") {
            push_unique(&mut roots, directory);
        }
        // `KT_CONFIG_DIR` re-homes the whole config tree, `sessions` included.
        let config = sources.var("KT_CONFIG_DIR").unwrap_or_else(|| sources.home.join(".kohakuterrarium"));
        push_unique(&mut roots, config.join("sessions"));
        roots
    }
    fn records(&self, roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        let mut records = Vec::new();
        // One frame is one call in whichever copy of the session it is found.
        let mut seen: HashSet<String> = HashSet::new();
        for file in files(roots) {
            let Some(connection) = sqlite::open(&file) else { continue };
            let session = Session::read(&connection, &file);

            // The model each creature reported last, which is the model it was calling with.
            let mut models: HashMap<String, String> = HashMap::new();
            // The session's first request from the user, the title where `meta` gives none.
            let mut asked: Option<(f64, String)> = None;
            let mut calls: Vec<Call> = Vec::new();
            // Event numbers are assigned in the order the frames were written, so reading in key
            // order reads the session in the order it happened.
            sqlite::each(&connection, "SELECT key, value FROM events ORDER BY key", |row| {
                let (Some(key), Some(bytes)) = (sqlite::text(row, 0), sqlite::blob(row, 1)) else { return };
                let Some(event) = unpack(&bytes) else { return };
                let agent = key.split(':').next().unwrap_or_default();
                match field(&event, "type").and_then(Value::as_str) {
                    // A turn's rollup of the frames read below: counting both counts them twice.
                    Some("turn_token_usage") => {}
                    // The key is what two copies of a session agree on.
                    Some("token_usage") => calls.push(Call { agent: agent.to_string(), id: key.clone(), event }),
                    Some("user_input") => {
                        let Some(at) = number(&event, "ts") else { return };
                        if asked.as_ref().is_none_or(|(first, _)| at < *first) {
                            if let Some(title) = field(&event, "content").and_then(content_text).and_then(|t| title_from(&t)) {
                                asked = Some((at, title));
                            }
                        }
                    }
                    Some("session_info" | "activity:session_info") => {
                        if let Some(model) = text(&event, "model").or_else(|| text(&event, "llm_name")) {
                            models.insert(agent.to_string(), model);
                        }
                    }
                    _ => {}
                }
            });

            let title = session.title.clone().or(asked.map(|(_, title)| title));
            for call in calls {
                let Some(at) = number(&call.event, "ts").and_then(super::epoch) else { continue };
                let (prompt, cached) = (count(&call.event, "prompt_tokens"), count(&call.event, "cached_tokens"));
                // `prompt_tokens` includes the cached part: the remainder is the fresh input.
                let tally = TokenTally::new(prompt - cached, 0, cached, count(&call.event, "completion_tokens"));
                if tally.total() <= 0 {
                    continue;
                }
                if !seen.insert(format!("{}:{}", session.id, call.id)) {
                    continue;
                }
                let model = models.get(&call.agent).or(session.model.as_ref()).map_or(MODEL, String::as_str);
                let mut record = AgentUsageRecord::new(at, model, tally).session(&session.id);
                record.session_name = Some(session.id.clone());
                record.title = title.clone();
                record.project = session.pwd.clone();
                records.push(record);
            }
        }
        records
    }
}

/// One `token_usage` frame and the creature whose key it was stored under.
struct Call {
    agent: String,
    id: String,
    event: Value,
}

/// What `meta` says about the database as a whole.
#[derive(Default)]
struct Session {
    id: String,
    /// The folder the session was started in.
    pwd: Option<String>,
    /// The name the session was given.
    title: Option<String>,
    /// The model the session's configuration names, where it names one at all.
    model: Option<String>,
}

impl Session {
    fn read(connection: &rusqlite::Connection, file: &Path) -> Session {
        let mut session = Session::default();
        sqlite::each(connection, "SELECT key, value FROM meta", |row| {
            let (Some(key), Some(bytes)) = (sqlite::text(row, 0), sqlite::blob(row, 1)) else { return };
            let Some(value) = unpack(&bytes) else { return };
            match key.as_str() {
                "session_id" => session.id = string(&value).unwrap_or_default().to_string(),
                "pwd" => session.pwd = string(&value).map(str::to_string),
                "name" => session.title = string(&value).map(str::to_string),
                // An empty `model` is a creature configured by profile, which names no model.
                "config_snapshot" => session.model = text(&value, "model"),
                _ => {}
            }
        });
        if session.id.is_empty() {
            session.id = file.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        }
        session
    }
}

/// The session databases under `roots`, in a stable order, leaving out each root's `mirror`.
fn files(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut ordered: Vec<&PathBuf> = roots.iter().collect();
    ordered.sort();
    ordered.dedup();
    let mut found = Vec::new();
    for root in ordered {
        let mut pending = vec![root.clone()];
        while let Some(directory) = pending.pop() {
            let Ok(entries) = std::fs::read_dir(&directory) else { continue };
            for entry in entries.flatten() {
                let Ok(kind) = entry.file_type() else { continue };
                let path = entry.path();
                if kind.is_dir() {
                    if !(directory == *root && entry.file_name() == "mirror") {
                        pending.push(path);
                    }
                } else if kind.is_file() && is_session(&entry.file_name().to_string_lossy()) {
                    found.push(path);
                }
            }
        }
    }
    found.sort();
    found
}

fn is_session(name: &str) -> bool {
    name.ends_with(".kohakutr") || name.contains(".kohakutr.v")
}

/// The value a KohakuVault blob holds, or None for anything that is not one: no header, flags set
/// by compression or encryption, a table key, or an encoding this store does not use.
fn unpack(bytes: &[u8]) -> Option<Value> {
    if bytes.len() < HEADER || bytes[..3] != [0x89, 0x4B, 0x01] || bytes[4..8] != [0, 0, 0, 0] || &bytes[8..10] != b"VK" {
        return None;
    }
    let body = &bytes[HEADER..];
    match bytes[3] {
        4 => rmpv::decode::read_value(&mut &body[..]).ok(),
        6 => std::str::from_utf8(body).ok().map(Value::from),
        _ => None,
    }
}

/// A map's value under a string key.
fn field<'a>(value: &'a Value, name: &str) -> Option<&'a Value> {
    value.as_map()?.iter().find(|(key, _)| key.as_str() == Some(name)).map(|(_, value)| value)
}

/// A message's text: a string, or the first `text` of a list of parts.
fn content_text(content: &Value) -> Option<String> {
    if let Some(text) = content.as_str() {
        return Some(text.to_string());
    }
    content.as_array()?.iter().find_map(|part| field(part, "text").and_then(Value::as_str)).map(str::to_string)
}

/// A string value, trimmed; blank is absent.
fn string(value: &Value) -> Option<&str> {
    value.as_str().map(str::trim).filter(|text| !text.is_empty())
}

/// A map's string under `name`, trimmed; blank is absent.
fn text(value: &Value, name: &str) -> Option<String> {
    field(value, name).and_then(string).map(str::to_string)
}

/// A whole count under `name`; absent or of another type is zero.
fn count(value: &Value, name: &str) -> i64 {
    match field(value, name) {
        Some(Value::Integer(number)) => number.as_i64().unwrap_or(0),
        Some(Value::F64(number)) => *number as i64,
        _ => 0,
    }
}

/// A number under `name`; `ts` is a fractional unix time.
fn number(value: &Value, name: &str) -> Option<f64> {
    match field(value, name)? {
        Value::Integer(number) => number.as_i64().map(|number| number as f64),
        Value::F64(number) => Some(*number),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spend::calendar::Calendar;
    use crate::spend::prices::PriceTable;
    use crate::spend::sources::{read_agent, SpendAgent};

    /// KohakuVault's header for one encoding: 4 MessagePack, 6 UTF-8 text.
    fn sealed(encoding: u8) -> Vec<u8> {
        vec![0x89, 0x4B, 0x01, encoding, 0, 0, 0, 0, b'V', b'K']
    }

    fn packed(value: Value) -> Vec<u8> {
        let mut bytes = sealed(4);
        rmpv::encode::write_value(&mut bytes, &value).unwrap();
        bytes
    }

    fn stored(text: &str) -> Vec<u8> {
        let mut bytes = sealed(6);
        bytes.extend_from_slice(text.as_bytes());
        bytes
    }

    fn map(pairs: Vec<(&str, Value)>) -> Value {
        Value::Map(pairs.into_iter().map(|(key, value)| (Value::from(key), value)).collect())
    }

    fn usage(kind: &str, at: f64, prompt: i64, cached: i64, completion: i64) -> Vec<u8> {
        packed(map(vec![
            ("type", Value::from(kind)),
            ("ts", Value::from(at)),
            ("prompt_tokens", Value::from(prompt)),
            ("cached_tokens", Value::from(cached)),
            ("completion_tokens", Value::from(completion)),
            ("total_tokens", Value::from(prompt + completion)),
        ]))
    }

    fn described(model: &str, at: f64) -> Vec<u8> {
        packed(map(vec![("type", Value::from("activity:session_info")), ("model", Value::from(model)), ("ts", Value::from(at))]))
    }

    /// A session database with kt's own schema.
    fn store(path: &Path, meta: &[(&str, Vec<u8>)], events: &[(&str, Vec<u8>)]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let connection = rusqlite::Connection::open(path).unwrap();
        for table in ["meta", "events", "state", "turn_rollup"] {
            connection.execute(&format!("CREATE TABLE {table} (key BLOB PRIMARY KEY NOT NULL, value BLOB NOT NULL)"), []).unwrap();
        }
        for (table, rows) in [("meta", meta), ("events", events)] {
            for (key, value) in rows {
                connection.execute(&format!("INSERT INTO {table} VALUES (?1, ?2)"), rusqlite::params![key.as_bytes(), value]).unwrap();
            }
        }
    }

    fn sessions(home: &Path) -> PathBuf {
        home.join(".kohakuterrarium").join("sessions")
    }

    fn read(home: &Path) -> crate::spend::ledger::Ledger {
        let cache = tempfile::tempdir().unwrap();
        read_agent(SpendAgent::KohakuTerrarium, &Sources::new(home), cache.path(), &Calendar::utc(2), &PriceTable::new())
    }

    /// 2026-09-12T12:00:00Z, a real time rather than an epoch.
    const AT: f64 = 1_789_214_400.5;

    #[test]
    fn a_call_is_counted_with_the_cache_read_taken_out_of_its_prompt() {
        let home = tempfile::tempdir().unwrap();
        let folder = sessions(home.path());
        let meta = [
            ("session_id", stored("graph_1")),
            ("created_at", stored("2026-09-12T11:59:00+00:00")),
            ("pwd", stored("C:\\Users\\me\\code\\Pulse")),
            ("name", stored("Tidy the workspace")),
            ("config_snapshot", packed(map(vec![("model", Value::from("")), ("provider", Value::from(""))]))),
        ];
        let events = [
            ("root:e000001", usage("token_usage", AT, 100, 60, 10)),
            ("root:e000002", usage("token_usage", AT + 5.0, 40, 0, 4)),
            // A turn's rollup repeats the frames above: counting it too would count them twice.
            ("root:e000003", usage("turn_token_usage", AT + 6.0, 140, 60, 14)),
            // A frame with nothing metered, and one that is not a frame at all.
            ("root:e000004", usage("token_usage", AT + 7.0, 0, 0, 0)),
            ("root:e000005", b"not a KohakuVault value".to_vec()),
        ];
        store(&folder.join("graph_1.kohakutr"), &meta, &events);
        // The sidecars beside a session, and the shared index, are not sessions.
        std::fs::write(folder.join("graph_1.kohakutr.drives"), b"no header").unwrap();
        std::fs::write(folder.join("graph_1.kohakutr-wal"), b"no header").unwrap();
        std::fs::write(folder.join(".kt-index.kvault"), b"no header").unwrap();

        let ledger = read(home.path());
        // (40 fresh + 60 read + 10 out) + (40 + 4); the rollup adds nothing.
        assert_eq!(ledger.all_time().tokens, 110 + 44);
        assert_eq!(ledger.days[0].tally, TokenTally::new(80, 0, 60, 14));
        let session = &ledger.sessions[0];
        assert_eq!(session.name, "graph_1");
        assert_eq!(session.title.as_deref(), Some("Tidy the workspace"));
        assert_eq!(session.project.as_ref().unwrap().name, "Pulse");
        assert_eq!(ledger.sessions.len(), 1);
    }

    #[test]
    fn a_creatures_model_comes_from_its_session_info_frame_and_is_a_placeholder_without_one() {
        let home = tempfile::tempdir().unwrap();
        let events = [
            // The frame a model switch re-emits names the model the creature calls with.
            ("root:e000001", described("magpie/glm-flash", AT)),
            ("root:e000002", usage("token_usage", AT + 1.0, 10, 0, 1)),
            // Another creature of the same graph, which reported no model of its own.
            ("swe:e000001", usage("token_usage", AT + 2.0, 20, 0, 2)),
        ];
        store(&sessions(home.path()).join("graph_1.kohakutr"), &[("session_id", stored("graph_1"))], &events);

        let records = KohakuTerrarium.records(&[sessions(home.path())]);
        let model_of = |at: f64| records.iter().find(|r| r.timestamp.timestamp() == at as i64).unwrap().model.clone();
        assert_eq!(model_of(AT + 1.0), "magpie/glm-flash");
        assert_eq!(model_of(AT + 2.0), MODEL);

        // A configuration that does name a model serves when the session reported none.
        let configured = tempfile::tempdir().unwrap();
        let meta = [("config_snapshot", packed(map(vec![("model", Value::from("deepseek-v4.1-flash"))])))];
        store(&sessions(configured.path()).join("a.kohakutr"), &meta, &[("root:e1", usage("token_usage", AT, 10, 0, 1))]);
        let records = KohakuTerrarium.records(&[sessions(configured.path())]);
        assert_eq!(records[0].model, "deepseek-v4.1-flash");
        // No session id in `meta`: the file names the session.
        assert_eq!(records[0].session_id.as_deref(), Some("a"));
    }

    #[test]
    fn an_unnamed_session_is_titled_by_its_first_request() {
        let home = tempfile::tempdir().unwrap();
        let ask = |at: f64, text: &str| packed(map(vec![("type", Value::from("user_input")), ("ts", Value::from(at)), ("content", Value::from(vec![map(vec![("text", Value::from(text))])]))]));
        let events = [
            ("worker:e000001", ask(AT + 9.0, "A later request")),
            ("root:e000001", ask(AT, "Fix the ring")),
            ("root:e000002", usage("token_usage", AT + 1.0, 10, 0, 1)),
        ];
        store(&sessions(home.path()).join("graph_1.kohakutr"), &[("session_id", stored("graph_1"))], &events);
        assert_eq!(read(home.path()).sessions[0].title.as_deref(), Some("Fix the ring"));
    }

    #[test]
    fn one_session_copied_twice_counts_its_calls_once() {
        let home = tempfile::tempdir().unwrap();
        let meta = [("session_id", stored("graph_1"))];
        let mut events: Vec<(&str, Vec<u8>)> = vec![("root:e000001", usage("token_usage", AT, 30, 10, 3))];
        events.push(("root:e000002", usage("token_usage", AT + 1.0, 20, 0, 2)));
        let folder = sessions(home.path());
        store(&folder.join("graph_1.kohakutr"), &meta, &events);
        // A second copy holds the same frames under the same session id.
        store(&folder.join("copy").join("graph_1.kohakutr"), &meta, &events);
        // Another machine's session, mirrored here: not this PC's spend.
        store(&folder.join("mirror").join("graph_2.kohakutr"), &[("session_id", stored("graph_2"))], &[("root:e000001", usage("token_usage", AT, 999, 0, 9))]);

        assert_eq!(read(home.path()).all_time().tokens, 33 + 22);
    }

    #[test]
    fn kts_own_session_override_moves_the_store() {
        let home = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        store(
            &elsewhere.path().join("r1.kohakutr"),
            &[("session_id", stored("r1"))],
            &[("root:e000001", usage("token_usage", AT, 7, 0, 1))],
        );
        let cache = tempfile::tempdir().unwrap();
        let sources = Sources::new(home.path()).with_var("KT_SESSION_DIR", elsewhere.path());
        let ledger = read_agent(SpendAgent::KohakuTerrarium, &sources, cache.path(), &Calendar::utc(2), &PriceTable::new());
        assert_eq!(ledger.all_time().tokens, 8);
        // The default location is still named, so a store that appears there later is noticed.
        assert_eq!(KohakuTerrarium.inputs(&Sources::new(home.path())), [sessions(home.path())]);
        assert_eq!(KohakuTerrarium.inputs(&sources).len(), 2);
    }

    #[test]
    fn values_that_are_not_kohakuvault_blobs_are_not_read() {
        assert!(unpack(b"\x81\xa4type\xabtoken_usage").is_none());
        // A flag byte is set: compressed or encrypted, not plain MessagePack.
        assert!(unpack(&[0x89, 0x4B, 0x01, 0x04, 1, 0, 0, 0, b'V', b'K', 0x80]).is_none());
        // A truncated body is no value either.
        assert!(unpack(&sealed(4)).is_none());
        assert_eq!(unpack(&stored("graph_1")).unwrap().as_str(), Some("graph_1"));
    }

    #[test]
    fn a_store_that_is_not_there_is_not_an_empty_account() {
        let home = tempfile::tempdir().unwrap();
        assert!(read(home.path()).days.is_empty());
        assert!(!SpendAgent::KohakuTerrarium.is_present(&Sources::new(home.path())));
    }
}
