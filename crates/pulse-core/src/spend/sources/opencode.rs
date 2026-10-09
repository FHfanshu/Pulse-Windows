// Ported from upstream Sources/Pulse/Usage/OpenCodeStore.swift.
//! OpenCode's store, and Kilo CLI's (`kilo.rs`): the same schema, because Kilo is a fork of it
//! down to the migrations.
//!
//! ```sql
//! session(id, project_id, slug, directory, title, ...)
//! message(id, session_id, time_created, time_updated, data)
//! ```
//!
//! `data` is the message as JSON, and an assistant's carries everything a ledger needs:
//!
//! ```json
//! { "role": "assistant", "modelID": "mimo-v2.5", "providerID": "...",
//!   "cost": 0, "time": { "created": 1777654926954 },
//!   "tokens": { "total": 10812, "input": 9705, "output": 11,
//!               "reasoning": 72, "cache": { "write": 0, "read": 1024 } } }
//! ```
//!
//! **OpenCode 2 moved both tables.** Messages go to
//! `session_message(id, session_id, type, seq, time_created, time_updated, data)` and sessions to
//! `session_v2`; the upgrade copied the history across under the same ids and stopped writing
//! `message` (on the Mac this was found on, `message` ended 2026-09-19 and every request since was
//! missing). A 2 row's role is the `type` column, not a `role` in `data`, and its model is
//! `"model": { "id": "gpt-6.1-sol", "providerID": "openai" }` rather than `modelID`. Both tables
//! are read (the new one first, then any old row the copy did not carry) and a message id is
//! counted once. A store without the new tables (OpenCode 1, Kilo CLI) reads exactly as before.
//!
//! **More than one place to read.** Beyond `opencode.db`, a release channel writes its own
//! `opencode-<channel>.db` in the same folder, and the first versions kept each message as
//! `storage/message/<session>/<message>.json` before the move to SQLite (the same JSON as `data`,
//! the same ids). All of them are read, and a message id is counted once across all of them, so a
//! message that two stores both hold is one message.
//!
//! **Its own `cost` is ignored.** It is whatever OpenCode's own table said at the time, is zero
//! for a plan it has no rate for, and would put two differently-sourced figures in one total.
//! Everything here is priced from the shared table like the rest of the page.
//!
//! **Reasoning tokens are counted as output**, which is where every price list bills them and
//! where the two CLIs' own counts already put them.
//!
//! Where it lives on Windows: OpenCode resolves its data folder with `xdg-basedir`, which does
//! not special-case Windows, so it is `$XDG_DATA_HOME\opencode` or
//! `%USERPROFILE%\.local\share\opencode`. The `%LOCALAPPDATA%` and `%APPDATA%` spellings are
//! also looked at, for a build that resolves it the Windows way.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::de::{IgnoredAny, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value};

use super::{int, list, push_unique, SpendSource};
use crate::provider::Provider;
use crate::spend::record::AgentUsageRecord;
use crate::spend::sqlite;
use crate::spend::tally::TokenTally;
use crate::spend::transcripts::Sources;

pub struct OpenCode;

/// Keep only usage and identity. OpenCode 2 embeds replay state and tool output in `data`;
/// building a Value for that history dominated scans of multi-gigabyte stores. Unknown fields
/// are validated and skipped without allocating their strings, arrays or objects.
struct UsageMessage(Value);

impl<'de> Deserialize<'de> for UsageMessage {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Fields;
        impl<'de> Visitor<'de> for Fields {
            type Value = UsageMessage;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a message object")
            }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut kept = Map::new();
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "id" | "sessionID" | "role" | "modelID" | "model" | "tokens" | "time" => {
                            kept.insert(key, map.next_value::<Value>()?);
                        }
                        _ => {
                            map.next_value::<IgnoredAny>()?;
                        }
                    }
                }
                Ok(UsageMessage(Value::Object(kept)))
            }
        }
        deserializer.deserialize_map(Fields)
    }
}

fn usage_message(bytes: &[u8]) -> Option<Value> {
    serde_json::from_slice::<UsageMessage>(bytes).ok().map(|m| m.0)
}

/// Borrow SQLite's text instead of copying the entire message before inspecting it.
fn message_data<'a>(row: &'a rusqlite::Row<'_>) -> Option<&'a str> {
    match row.get_ref(2).ok()? {
        rusqlite::types::ValueRef::Text(bytes) | rusqlite::types::ValueRef::Blob(bytes) => {
            std::str::from_utf8(bytes).ok()
        }
        _ => None,
    }
}

// Extract in SQLite too: replay histories never need to cross into Rust. Invalid JSON
// still yields an empty object, so its id is claimed before falling back to an older copy.
const USAGE_DATA: &str = r#"CASE
    WHEN data IS NULL THEN NULL
    WHEN json_valid(data) THEN json_object(
        'role', json_extract(data, '$.role'),
        'modelID', json_extract(data, '$.modelID'),
        'model', json_extract(data, '$.model'),
        'tokens', json_extract(data, '$.tokens'),
        'time', json_extract(data, '$.time'))
    ELSE '{}' END"#;

impl SpendSource for OpenCode {
    fn raw(&self) -> &'static str {
        "openCode"
    }
    fn source_id(&self) -> &'static str {
        "opencode"
    }
    fn display_name(&self) -> &'static str {
        "OpenCode"
    }
    fn card_provider(&self) -> Option<Provider> {
        Some(Provider::OpenCodeGo)
    }
    fn icon(&self) -> Option<&'static str> {
        Some("opencode")
    }
    fn price_vendor(&self) -> Option<&'static str> {
        Some("opencode-go")
    }
    fn env_vars(&self) -> &'static [&'static str] {
        &["XDG_DATA_HOME", "APPDATA", "LOCALAPPDATA", "OPENCODE_DB"]
    }
    fn has_captured_validation(&self) -> bool {
        true
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        store_inputs(sources, "opencode", Some("OPENCODE_DB"))
    }
    fn records(&self, roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        store_records(roots)
    }
}

// MARK: - Where the store is

/// The files and folders an OpenCode-shaped app (`app` is `opencode` or `kilo`) is read from,
/// whether or not they exist: the main database, every channel database beside it, the legacy
/// JSON message and session folders, and a database the app was told to use (`db_override`, an
/// environment variable holding an absolute path).
pub(crate) fn store_inputs(sources: &Sources, app: &str, db_override: Option<&str>) -> Vec<PathBuf> {
    let data_folders = [
        sources.xdg_data_home().join(app),
        // WINDOWS-PATH: unverified (a build that resolves its data folder the Windows way)
        sources.local_app_data().join(app),
        // WINDOWS-PATH: unverified
        sources.app_data().join(app),
    ];
    let channel = format!("{app}-");
    let mut roots = Vec::new();
    for folder in &data_folders {
        push_unique(&mut roots, folder.join(format!("{app}.db")));
        for path in list(folder) {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if name.starts_with(&channel) && name.ends_with(".db") && path.is_file() {
                push_unique(&mut roots, path);
            }
        }
        push_unique(&mut roots, folder.join("storage").join("message"));
        push_unique(&mut roots, folder.join("storage").join("session"));
    }
    if let Some(path) = db_override.and_then(|name| sources.var(name)).filter(|p| p.is_absolute()) {
        push_unique(&mut roots, path);
    }
    roots
}

// MARK: - Reading it

#[derive(Default, Clone)]
struct SessionInfo {
    slug: Option<String>,
    title: Option<String>,
    directory: Option<String>,
}

struct Reader<'a> {
    sessions: &'a HashMap<String, SessionInfo>,
    seen: HashSet<String>,
    records: Vec<AgentUsageRecord>,
}

impl Reader<'_> {
    /// One message. `role` is the `type` column of OpenCode 2's table, None for the old shapes
    /// (whose role is in `data`, and whose rows that predate `total` fold reasoning into output).
    fn message(&mut self, session: String, root: Value, role: Option<String>) {
        let Value::Object(root) = root else { return };
        let totalless_folded = role.is_none();
        let role = role.or_else(|| root.get("role").and_then(Value::as_str).map(str::to_string));
        if role.as_deref() != Some("assistant") {
            return;
        }
        let Some(model) = root
            .get("modelID")
            .and_then(Value::as_str)
            .or_else(|| root.get("model").and_then(|m| m.get("id")).and_then(Value::as_str))
        else {
            return;
        };
        let Some(Value::Object(counts)) = root.get("tokens") else { return };
        let Some(at) = created_at(&root) else { return };

        let cache = counts.get("cache").and_then(Value::as_object);
        let tally = TokenTally {
            input: int(counts.get("input")),
            cache_write: int(cache.and_then(|c| c.get("write"))),
            cache_read: int(cache.and_then(|c| c.get("read"))),
            output: output(counts, totalless_folded),
            ..TokenTally::default()
        };
        if tally.total() <= 0 {
            return;
        }
        let info = self.sessions.get(&session);
        let mut record = AgentUsageRecord::new(at, model, tally).session(&session);
        record.session_name = info.and_then(|s| s.slug.clone());
        record.title = info.and_then(|s| s.title.clone());
        record.project = info.and_then(|s| s.directory.clone());
        self.records.push(record);
    }

    /// A row of a database: the id is claimed first, so a copy is never read twice, even one
    /// whose first copy turned out to carry no usage.
    fn row(&mut self, id: Option<String>, session: Option<String>, data: Option<&str>, role: Option<String>) {
        let (Some(session), Some(data)) = (session, data) else { return };
        if let Some(id) = &id {
            if !self.seen.insert(id.clone()) {
                return;
            }
        }
        let Some(root) = usage_message(data.as_bytes()) else { return };
        self.message(session, root, role);
    }
}

/// `time.created` in milliseconds.
fn created_at(root: &Map<String, Value>) -> Option<DateTime<Utc>> {
    let created = int(root.get("time")?.get("created"));
    if created <= 0 {
        return None;
    }
    DateTime::from_timestamp_millis(created)
}

/// Output with its reasoning, counted once.
///
/// **Two shapes under one name.** Current OpenCode (and Kilo, and MiMo Code, which share its
/// store) reports reasoning **beside** output, and `total` is input, output, reasoning and cache
/// together. Older rows folded reasoning **into** output and left it out of `total`; adding it
/// again counted it twice (632 rows on the Mac this was found on). So:
///
/// - reasoning larger than output cannot be inside it, and is added;
/// - a `total` decides where there is one;
/// - without one, `totalless_folded` does. OpenCode 1's `message` rows that predate `total` all
///   have output at least their reasoning (78 of 78 there, where a fifth of later rows with
///   reasoning beside output do not), so they are the folded shape. OpenCode 2's
///   `session_message` carries no `total` at all and keeps reasoning beside output.
pub(crate) fn output(counts: &Map<String, Value>, totalless_folded: bool) -> i64 {
    let output = int(counts.get("output"));
    let reasoning = int(counts.get("reasoning"));
    if !(reasoning > 0 && output >= reasoning) {
        return output + reasoning;
    }
    let cache = counts.get("cache").and_then(Value::as_object);
    let total = int(counts.get("total"));
    if total <= 0 {
        // Only a row with no total at all is OpenCode 1's old shape; a total written as zero
        // says nothing either way.
        let stated = counts.get("total").is_some_and(|v| !v.is_null());
        return if totalless_folded && !stated { output } else { output + reasoning };
    }
    let folded = total == int(counts.get("input")) + output + int(cache.and_then(|c| c.get("read"))) + int(cache.and_then(|c| c.get("write")));
    if folded {
        output
    } else {
        output + reasoning
    }
}

fn session_info(row: &rusqlite::Row) -> Option<(String, SessionInfo)> {
    let id = sqlite::text(row, 0)?;
    Some((id, SessionInfo { slug: sqlite::text(row, 1), title: sqlite::text(row, 2), directory: sqlite::text(row, 3) }))
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// The JSON files two folders down: `<root>/<a>/<b>.json`.
fn json_files(root: &Path) -> Vec<(String, PathBuf)> {
    let mut found = Vec::new();
    for group in list(root).into_iter().filter(|p| p.is_dir()) {
        let name = file_name(&group);
        for file in list(&group) {
            if file.extension().is_some_and(|e| e == "json") && file.is_file() {
                found.push((name.clone(), file));
            }
        }
    }
    found
}

/// OpenCode 2's `session_v2` first, then `session` for any it lacks, then the legacy session
/// JSON; the first to name a session wins.
fn sessions_in(databases: &[rusqlite::Connection], session_folders: &[&PathBuf]) -> HashMap<String, SessionInfo> {
    let mut sessions: HashMap<String, SessionInfo> = HashMap::new();
    for connection in databases {
        for table in ["session_v2", "session"] {
            sqlite::each(connection, &format!("SELECT id, slug, title, directory FROM {table}"), |row| {
                if let Some((id, info)) = session_info(row) {
                    sessions.entry(id).or_insert(info);
                }
            });
        }
    }
    for folder in session_folders {
        for (_, file) in json_files(folder) {
            let Some(Value::Object(root)) = crate::spend::logio::json(&file) else { continue };
            let text = |key: &str| root.get(key).and_then(Value::as_str).map(str::to_string);
            let Some(id) = text("id") else { continue };
            sessions.entry(id).or_insert(SessionInfo { slug: text("slug"), title: text("title"), directory: text("directory") });
        }
    }
    sessions
}

/// Every assistant message the roots hold, once each by its id.
pub(crate) fn store_records(roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
    let databases: Vec<rusqlite::Connection> = roots.iter().filter(|p| p.is_file()).filter_map(|p| sqlite::open(p)).collect();
    let folders = |name: &str| -> Vec<&PathBuf> { roots.iter().filter(|p| p.is_dir() && file_name(p) == name).collect() };
    let sessions = sessions_in(&databases, &folders("session"));

    let mut reader = Reader { sessions: &sessions, seen: HashSet::new(), records: Vec::new() };
    // The new table of every database first, then the old, then the JSON files, so the newest
    // shape of a message is the one that is read whichever store it was found in first.
    for connection in &databases {
        let sql = format!("SELECT id, session_id, {USAGE_DATA}, type FROM session_message WHERE type = 'assistant'");
        sqlite::each(connection, &sql, |row| {
            reader.row(sqlite::text(row, 0), sqlite::text(row, 1), message_data(row), sqlite::text(row, 3));
        });
    }
    for connection in &databases {
        // A row without an id (a store with no `id` column) cannot have been copied, so it is
        // never a repeat.
        let id = if sqlite::columns(connection, "message").contains("id") { "id" } else { "NULL" };
        sqlite::each(connection, &format!("SELECT {id}, session_id, {USAGE_DATA} FROM message"), |row| {
            reader.row(sqlite::text(row, 0), sqlite::text(row, 1), message_data(row), None);
        });
    }
    for folder in folders("message") {
        for (group, file) in json_files(folder) {
            // The file is named for its message, and nearly all of them were long since moved
            // into a database: a message already counted is not even opened.
            let stem = file.file_stem().map(|s| s.to_string_lossy().into_owned());
            if stem.as_ref().is_some_and(|stem| reader.seen.contains(stem)) {
                continue;
            }
            let Some(root) = std::fs::read(&file).ok().and_then(|bytes| usage_message(&bytes)) else { continue };
            let text = |key: &str| root.get(key).and_then(Value::as_str).map(str::to_string);
            let id = text("id").or(stem);
            if let Some(id) = &id {
                if !reader.seen.insert(id.clone()) {
                    continue;
                }
            }
            let session = text("sessionID").unwrap_or(group);
            reader.message(session, root, None);
        }
    }
    reader.records
}

#[cfg(test)]
pub(crate) mod tests {
    use chrono::TimeZone;
    use serde_json::json;

    use super::*;
    use crate::spend::calendar::Calendar;
    use crate::spend::prices::{ModelPrice, PriceTable};
    use crate::spend::record::build_ledger;
    use crate::spend::sources::{read_agent, SpendAgent};
    use crate::spend::sqlite::tests::database;
    use crate::spend::summary::SpendSummary;

    /// 2026-09-14 00:00 UTC, in milliseconds.
    pub const CREATED: i64 = 1_789_344_000_000;

    pub fn priced() -> PriceTable {
        HashMap::from([("priced".to_string(), ModelPrice::new(1.0, 10.0, Some(0.1), Some(1.0), Some("Priced")))])
    }

    fn ledger_of(agent: SpendAgent, root: &Path, prices: &PriceTable) -> crate::spend::Ledger {
        let cache = tempfile::tempdir().unwrap();
        read_agent(agent, &Sources::new(root), cache.path(), &Calendar::utc(2), prices)
    }

    fn day_tally(ledger: &crate::spend::Ledger) -> TokenTally {
        ledger.days.iter().map(|d| d.tally.clone()).sum()
    }

    fn data_folder(home: &Path, app: &str) -> PathBuf {
        let folder = home.join(".local").join("share").join(app);
        std::fs::create_dir_all(&folder).unwrap();
        folder
    }

    #[test]
    fn an_assistant_messages_counts_become_a_day_and_a_session() {
        let home = tempfile::tempdir().unwrap();
        let assistant = json!({"role":"assistant","modelID":"priced","time":{"created":CREATED},
            "tokens":{"total":0,"input":100,"output":10,"reasoning":5,"cache":{"write":20,"read":300}}});
        // A user's message carries no counts and must not be read as a zero.
        let user = json!({"role":"user","time":{"created":1},"tokens":{}});
        database(
            &data_folder(home.path(), "opencode").join("opencode.db"),
            &[
                "CREATE TABLE session (id TEXT, project_id TEXT, slug TEXT, directory TEXT, title TEXT)",
                "CREATE TABLE message (id TEXT, session_id TEXT, time_created INTEGER, data TEXT)",
                "INSERT INTO session VALUES ('s1','p','slug-one','/Users/me/Code/Pulse','Fix the ring')",
                &format!("INSERT INTO message VALUES ('m1','s1',1,'{assistant}')"),
                &format!("INSERT INTO message VALUES ('m2','s1',2,'{user}')"),
            ],
        );

        let ledger = ledger_of(SpendAgent::OpenCode, home.path(), &priced());
        // Reasoning counts as output, which is where every price list bills it.
        assert_eq!(day_tally(&ledger), TokenTally::new(100, 20, 300, 15));
        // Priced from the shared table, never from the store's own `cost` column.
        let day = ledger.days.iter().find(|d| d.tokens > 0).unwrap();
        assert!((day.cost - (100.0 + 20.0 + 30.0 + 150.0) / 1_000_000.0).abs() < 1e-9);

        let session = &ledger.sessions[0];
        assert_eq!(session.title.as_deref(), Some("Fix the ring"));
        assert_eq!(session.name, "slug-one");
        // The directory's last component, not the whole path.
        assert_eq!(session.project.as_ref().unwrap().name, "Pulse");
    }

    #[test]
    fn opencode_twos_tables_are_read_with_the_copied_history_counted_once() {
        let home = tempfile::tempdir().unwrap();
        // OpenCode 1's shape, as the upgrade left it in both tables.
        let old = json!({"role":"assistant","modelID":"priced","time":{"created":CREATED},
            "tokens":{"input":100,"output":10,"reasoning":0,"cache":{"write":0,"read":0}}});
        // OpenCode 2's: no role, the model an object.
        let new = json!({"time":{"created":CREATED + 1000},"model":{"id":"priced","providerID":"openai"},
            "tokens":{"input":200,"output":20,"reasoning":5,"cache":{"write":0,"read":0}}});
        let user = json!({"time":{"created":1},"tokens":{"input":999}});
        database(
            &data_folder(home.path(), "opencode").join("opencode.db"),
            &[
                "CREATE TABLE session (id TEXT, project_id TEXT, slug TEXT, directory TEXT, title TEXT)",
                "CREATE TABLE message (id TEXT, session_id TEXT, time_created INTEGER, data TEXT)",
                "CREATE TABLE session_v2 (id TEXT, slug TEXT, title TEXT, directory TEXT)",
                "CREATE TABLE session_message (id TEXT, session_id TEXT, type TEXT, seq INTEGER, data TEXT)",
                "INSERT INTO session_v2 VALUES ('s1','slug-one','Fix the ring','/Users/me/Code/Pulse')",
                // Copied by the upgrade: in both tables under one id.
                &format!("INSERT INTO message VALUES ('m1','s1',1,'{old}')"),
                &format!("INSERT INTO session_message VALUES ('m1','s1','assistant',1,'{old}')"),
                // Written since: only the new table.
                &format!("INSERT INTO session_message VALUES ('m2','s1','assistant',2,'{new}')"),
                // A user turn with a count in it is still not a reply.
                &format!("INSERT INTO session_message VALUES ('m3','s1','user',3,'{user}')"),
            ],
        );
        let ledger = ledger_of(SpendAgent::OpenCode, home.path(), &priced());
        assert_eq!(day_tally(&ledger), TokenTally::new(300, 0, 0, 35));
        assert_eq!(ledger.sessions[0].title.as_deref(), Some("Fix the ring"));
    }

    #[test]
    fn channel_databases_and_the_legacy_json_are_read_and_a_message_id_counts_once_across_all() {
        let home = tempfile::tempdir().unwrap();
        let folder = data_folder(home.path(), "opencode");
        let message = |input: i64, at: i64| {
            json!({"role":"assistant","modelID":"priced","time":{"created":at},"tokens":{"input":input,"output":0}})
        };
        let schema = [
            "CREATE TABLE session (id TEXT, project_id TEXT, slug TEXT, directory TEXT, title TEXT)",
            "CREATE TABLE message (id TEXT, session_id TEXT, time_created INTEGER, data TEXT)",
        ];
        let mut main = schema.map(String::from).to_vec();
        main.push(format!("INSERT INTO message VALUES ('m1','s1',1,'{}')", message(100, CREATED)));
        database(&folder.join("opencode.db"), &main.iter().map(String::as_str).collect::<Vec<_>>());
        // A channel's database holds the same message again, and one of its own.
        let mut dev = schema.map(String::from).to_vec();
        dev.push(format!("INSERT INTO message VALUES ('m1','s1',1,'{}')", message(100, CREATED)));
        dev.push(format!("INSERT INTO message VALUES ('m2','s2',2,'{}')", message(7, CREATED)));
        database(&folder.join("opencode-dev.db"), &dev.iter().map(String::as_str).collect::<Vec<_>>());
        // Something that is not a channel database is left alone.
        std::fs::write(folder.join("opencode.db.bak"), "not a database").unwrap();
        std::fs::write(folder.join("other.db"), "not ours").unwrap();

        // The legacy files: m1 again (the same id), and a message nothing else has.
        let storage = folder.join("storage");
        std::fs::create_dir_all(storage.join("message").join("s1")).unwrap();
        std::fs::create_dir_all(storage.join("session").join("proj")).unwrap();
        let mut legacy = message(100, CREATED);
        legacy["id"] = json!("m1");
        std::fs::write(storage.join("message").join("s1").join("m1.json"), legacy.to_string()).unwrap();
        // No id inside: the file name is the id. No sessionID: the folder is the session.
        std::fs::write(storage.join("message").join("s1").join("m3.json"), message(5, CREATED).to_string()).unwrap();
        // Not an assistant message, and a file that is not JSON.
        std::fs::write(storage.join("message").join("s1").join("m4.json"), json!({"role":"user"}).to_string()).unwrap();
        std::fs::write(storage.join("message").join("s1").join("m5.json"), "{broken").unwrap();
        std::fs::write(
            storage.join("session").join("proj").join("s1.json"),
            json!({"id":"s1","slug":"legacy-slug","title":"From the JSON","directory":"/work/Legacy"}).to_string(),
        )
        .unwrap();

        let ledger = ledger_of(SpendAgent::OpenCode, home.path(), &priced());
        assert_eq!(day_tally(&ledger), TokenTally::new(100 + 7 + 5, 0, 0, 0));
        let titles: Vec<_> = ledger.sessions.iter().map(|s| (s.name.clone(), s.title.clone())).collect();
        assert!(titles.contains(&("legacy-slug".to_string(), Some("From the JSON".to_string()))), "{titles:?}");
    }

    #[test]
    fn xdg_data_home_and_an_override_database_move_the_store() {
        let home = tempfile::tempdir().unwrap();
        let xdg = tempfile::tempdir().unwrap();
        let sources = Sources::new(home.path()).with_var("XDG_DATA_HOME", xdg.path()).with_var("OPENCODE_DB", xdg.path().join("custom.db"));
        let inputs = SpendAgent::OpenCode.inputs(&sources);
        assert!(inputs.contains(&xdg.path().join("opencode").join("opencode.db")));
        assert!(inputs.contains(&xdg.path().join("custom.db")));
        // The Windows spellings are named beside it, so a store that appears there is watched.
        assert!(inputs.contains(&home.path().join("AppData").join("Local").join("opencode").join("opencode.db")));
        assert!(inputs.contains(&home.path().join("AppData").join("Roaming").join("opencode").join("storage").join("message")));
        // Without the variable, the XDG default under the home folder.
        let plain = SpendAgent::OpenCode.inputs(&Sources::new(home.path()));
        assert_eq!(plain[0], home.path().join(".local").join("share").join("opencode").join("opencode.db"));
        // A relative override is not a path to read.
        let relative = Sources::new(home.path()).with_var("OPENCODE_DB", "custom.db");
        assert!(!SpendAgent::OpenCode.inputs(&relative).contains(&PathBuf::from("custom.db")));
    }

    #[test]
    fn opencode_is_priced_at_the_plan_vendor_and_kilo_at_its_own() {
        let at = Utc.timestamp_opt(CREATED / 1000, 0).unwrap();
        let record = AgentUsageRecord::new(at, "plan-only", TokenTally::new(1_000_000, 0, 0, 0)).session("s");
        let table: PriceTable = HashMap::from([
            (crate::spend::prices::vendor_key("opencode-go", "plan-only"), ModelPrice::new(2.0, 3.0, None, None, Some("Plan model"))),
        ]);
        let go = build_ledger(std::slice::from_ref(&record), &table, "openCode", SpendAgent::OpenCode.price_vendor(), &Calendar::utc(2), Default::default());
        assert_eq!(go.days[0].cost, 2.0);
        let kilo = build_ledger(&[record], &table, "kiloCLI", SpendAgent::KiloCli.price_vendor(), &Calendar::utc(2), Default::default());
        assert_eq!(kilo.days[0].cost, 0.0);
        assert_eq!(kilo.unpriced_models, ["plan-only"]);
    }

    #[test]
    fn a_store_that_is_not_there_is_not_an_empty_account() {
        let home = tempfile::tempdir().unwrap();
        assert!(ledger_of(SpendAgent::OpenCode, home.path(), &PriceTable::new()).days.is_empty());
        // A database with the wrong shape reads as nothing, not as an error.
        database(&data_folder(home.path(), "opencode").join("opencode.db"), &["CREATE TABLE unrelated (x TEXT)"]);
        assert!(ledger_of(SpendAgent::OpenCode, home.path(), &PriceTable::new()).days.is_empty());
    }

    #[test]
    fn a_sessions_buckets_reach_today_and_only_todays_counts() {
        let home = tempfile::tempdir().unwrap();
        let calendar = Calendar::local();
        let now = Utc::now();
        let today = calendar.start_of_day(now) + chrono::Duration::hours(12);
        let yesterday = calendar.add_days(calendar.start_of_day(now), -1) + chrono::Duration::hours(12);
        let message = |input: i64, at: DateTime<Utc>| {
            json!({"role":"assistant","modelID":"priced","time":{"created":at.timestamp_millis()},"tokens":{"input":input,"output":0}})
        };
        database(
            &data_folder(home.path(), "opencode").join("opencode.db"),
            &[
                "CREATE TABLE session (id TEXT, project_id TEXT, slug TEXT, directory TEXT, title TEXT)",
                "CREATE TABLE message (id TEXT, session_id TEXT, time_created INTEGER, data TEXT)",
                "INSERT INTO session VALUES ('s1','p','slug','/Users/me/Code/Pulse','Fix the ring')",
                &format!("INSERT INTO message VALUES ('m1','s1',1,'{}')", message(100, today)),
                &format!("INSERT INTO message VALUES ('m2','s1',2,'{}')", message(900, yesterday)),
            ],
        );
        let cache = tempfile::tempdir().unwrap();
        let prices: PriceTable = HashMap::from([("priced".to_string(), ModelPrice::new(1_000.0, 10_000.0, Some(100.0), Some(1_000.0), Some("Priced")))]);
        let sources = Sources::new(home.path());
        let unpriced = read_agent(SpendAgent::OpenCode, &sources, cache.path(), &calendar, &PriceTable::new());
        let session = &unpriced.sessions[0];
        assert_eq!(session.slots.len(), 2);
        assert_eq!(session.slots.iter().map(|s| s.unpriced_tokens).sum::<i64>(), session.unpriced_tokens);
        let summary = SpendSummary::of(&HashMap::from([(SpendAgent::OpenCode, unpriced.clone())]), Some(1), now, &calendar);
        assert_eq!(summary.sessions[0].session.unpriced_tokens, 100);
        assert_eq!(summary.sessions[0].session.estimated_cost(), None);
        assert_eq!(summary.projects[0].unpriced_tokens, 100);

        let ledger = read_agent(SpendAgent::OpenCode, &sources, cache.path(), &calendar, &prices);
        let session = &ledger.sessions[0];
        assert_eq!(session.slots.len(), 2);
        assert_eq!(session.slots.iter().map(|s| s.tokens).sum::<i64>(), session.tokens);
        assert!((session.slots.iter().map(|s| s.cost).sum::<f64>() - session.cost).abs() < 1e-9);
        // The day keys and the session buckets are the same work.
        assert_eq!(ledger.days.iter().map(|d| d.tokens).sum::<i64>(), session.tokens);

        let summary = SpendSummary::of(&HashMap::from([(SpendAgent::OpenCode, ledger)]), Some(1), now, &calendar);
        assert_eq!(summary.tokens, 100);
        assert!((summary.cost - 0.1).abs() < 1e-9);
        assert_eq!(summary.projects[0].cost, summary.cost);
        assert_eq!(summary.sessions[0].session.cost, summary.cost);
        assert_eq!(summary.projects[0].tokens, 100);
        assert_eq!(summary.sessions[0].session.tokens, 100);
    }

    fn counts(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    #[test]
    fn reasoning_beside_output_is_added_and_reasoning_inside_it_is_not_counted_twice() {
        // Current rows: reasoning beside output, inside total.
        let beside = counts(json!({"input":10,"output":5,"reasoning":20,"total":135,"cache":{"read":100,"write":0}}));
        assert_eq!(output(&beside, false), 25);
        // Old rows: reasoning already in output, outside total.
        let folded = counts(json!({"input":10,"output":25,"reasoning":20,"total":135,"cache":{"read":100,"write":0}}));
        assert_eq!(output(&folded, false), 25);
        // No total: OpenCode 2 adds it; OpenCode 1's old rows already hold it.
        let bare = counts(json!({"output":25,"reasoning":20}));
        assert_eq!(output(&bare, false), 45);
        assert_eq!(output(&bare, true), 25);
        // Reasoning larger than output cannot be inside it, whatever the total says.
        let larger = counts(json!({"input":10,"output":5,"reasoning":20,"total":115,"cache":{"read":100,"write":0}}));
        assert_eq!(output(&larger, false), 25);
        assert_eq!(output(&counts(json!({"output":5,"reasoning":20})), true), 25);
        // A total written as zero says nothing either way.
        assert_eq!(output(&counts(json!({"output":10,"reasoning":5,"total":0})), true), 15);
    }
    #[test]
    fn replay_bodies_are_skipped_and_usage_matches_the_full_json_shape() {
        let root = json!({
            "id":"m","sessionID":"s","role":"assistant","modelID":"priced",
            "time":{"created":CREATED},"tokens":{"input":10,"output":2},
            "replayState":{"messages":[{"text":"x".repeat(2_000_000)}]},
            "parts":[{"tool":"output","text":"ignored"}]
        });
        let bytes = serde_json::to_vec(&root).unwrap();
        let small = usage_message(&bytes).unwrap();
        assert!(small.get("parts").is_none());
        assert!(small.get("replayState").is_none());
        for key in ["id", "sessionID", "role", "modelID", "time", "tokens"] {
            assert_eq!(small[key], root[key]);
        }
        let sessions = HashMap::new();
        let mut full = Reader { sessions: &sessions, seen: HashSet::new(), records: Vec::new() };
        let mut projected = Reader { sessions: &sessions, seen: HashSet::new(), records: Vec::new() };
        full.message("s".to_string(), root, None);
        projected.message("s".to_string(), small, None);
        assert_eq!(projected.records[0].tally, full.records[0].tally);
        assert_eq!(projected.records[0].timestamp, full.records[0].timestamp);
        assert!(usage_message(br#"{"role":"assistant","ignored":bad}"#).is_none());
        assert!(usage_message(b"[]").is_none());
    }

    #[test]
    fn sql_projection_skips_bad_json_without_aborting_or_reviving_old_duplicates() {
        let home = tempfile::tempdir().unwrap();
        let path = data_folder(home.path(), "opencode").join("opencode.db");
        let good = json!({"model":{"id":"priced"},"time":{"created":CREATED},"tokens":{"input":10,"output":2}});
        let old = json!({"role":"assistant","modelID":"priced","time":{"created":CREATED},"tokens":{"input":999}});
        database(
            &path,
            &[
                "CREATE TABLE session_message (id TEXT, session_id TEXT, data TEXT, type TEXT)",
                "CREATE TABLE message (id TEXT, session_id TEXT, data TEXT)",
                "INSERT INTO session_message VALUES ('bad','s','{broken','assistant')",
                &format!("INSERT INTO session_message VALUES ('good','s','{good}','assistant')"),
                &format!("INSERT INTO message VALUES ('bad','s','{old}')"),
            ],
        );
        let records = store_records(&[path]);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].tally, TokenTally::new(10, 0, 0, 2));
    }
}
