// Ported from upstream Sources/Pulse/Usage/Readers/ZedReader.swift.
//! Zed's thread database, `threads.db`.
//!
//! Each row's `data` blob is a thread as JSON, or a zstd frame that decodes to the same JSON. A
//! compressed row is decoded with the same `ruzstd` path as DSH (`dsh::jsonl_bytes`) and then parsed
//! exactly as the plain form is; a decoded payload over 32 MiB is refused either way. A row is
//! compressed when its `data_type` says `zstd` **or** its bytes carry the zstd magic, so a mislabelled
//! frame is still read; a `zstd`-labelled row whose bytes are not a frame contributes nothing.
//! A corrupt frame contributes nothing; upstream also reports it as a note, which is not carried.
//! Plain JSON rows (`data_type = 'json'`) are read in full.
//!
//! Only threads whose `model.provider` is `zed.dev` count; a thread driven by an external ACP agent
//! is skipped so the provider behind it is not counted twice. A thread flagged `imported` is another
//! product's, already counted where it came from. The token counts are real reported counters and a
//! thread is cumulative over its life, so there is one aggregate record per thread, keyed `zed:{id}`.
//! `request_token_usage` is summed entry by entry (the entries with work in them);
//! `cumulative_token_usage` is used only when the request entries are empty. There is no reasoning
//! field, so none is derived.
//!
//! Where it lives on Windows: `$XDG_DATA_HOME\zed`, `%APPDATA%\Zed` (the macOS Application Support
//! folder) and `%LOCALAPPDATA%\Zed` are all read. Unverified on a real PC.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde_json::{Map, Value};

use super::{clamped, dsh, flexible, flexible_text, SpendSource};
use crate::spend::logio;
use crate::spend::record::AgentUsageRecord;
use crate::spend::sqlite;
use crate::spend::tally::TokenTally;
use crate::spend::transcripts::Sources;

pub struct Zed;

impl SpendSource for Zed {
    fn raw(&self) -> &'static str {
        "zed"
    }
    fn source_id(&self) -> &'static str {
        "zed"
    }
    fn display_name(&self) -> &'static str {
        "Zed"
    }
    fn env_vars(&self) -> &'static [&'static str] {
        &["XDG_DATA_HOME", "APPDATA", "LOCALAPPDATA"]
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        vec![
            // WINDOWS-PATH: unverified (the XDG layout, in case Zed follows it on Windows)
            sources.xdg_data_home().join("zed").join("threads").join("threads.db"),
            // WINDOWS-PATH: unverified (the macOS Application Support folder, mapped to %APPDATA%)
            sources.app_data().join("Zed").join("threads").join("threads.db"),
            // WINDOWS-PATH: unverified
            sources.local_app_data().join("Zed").join("threads").join("threads.db"),
        ]
    }
    fn records(&self, roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        logio::files(roots, &[], &["threads.db"], &[]).iter().flat_map(|file| read(file)).collect()
    }
}

/// The zstd frame magic, `28 B5 2F FD`.
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];

/// The bound on a plain JSON blob, as upstream has it.
const MAXIMUM_PAYLOAD: usize = 32 * 1024 * 1024;

fn read(file: &Path) -> Vec<AgentUsageRecord> {
    let Some(connection) = sqlite::open(file) else { return Vec::new() };
    if sqlite::columns(&connection, "threads").is_empty() {
        return Vec::new();
    }
    let sql = sqlite::select(
        &connection,
        "threads",
        &["id", "updated_at", "data_type", "data", "created_at", "folder_paths", "folder_paths_order"],
    );
    let mut records = Vec::new();
    sqlite::each(&connection, &sql, |row| {
        let Some(id) = sqlite::text(row, 0) else { return };
        let Some(blob) = sqlite::blob(row, 3) else { return };
        let data_type = sqlite::text(row, 2).unwrap_or_default();
        let Some(thread) = payload(&data_type, &blob) else { return };
        let Some(thread) = thread.as_object() else { return };

        if thread.get("imported").and_then(Value::as_bool) == Some(true) {
            return;
        }
        let Some(model) = thread.get("model").and_then(Value::as_object) else { return };
        if model.get("provider").and_then(Value::as_str) != Some("zed.dev") {
            return;
        }
        let Some(name) = model.get("model").and_then(Value::as_str).filter(|n| !n.trim().is_empty()) else { return };

        let request = usage_of(thread.get("request_token_usage"));
        let tally = if request.total() > 0 { request } else { single_usage(thread.get("cumulative_token_usage")) };
        if tally.total() <= 0 {
            return;
        }
        let Some(at) = timestamp(row, thread) else { return };

        let mut record = AgentUsageRecord::new(at, name, tally).session(&id).aggregate(true);
        record.project = project(sqlite::text(row, 5).as_deref(), sqlite::text(row, 6).as_deref());
        record.deduplication_id = Some(format!("zed:{id}"));
        records.push(record);
    });
    records
}

/// The thread's JSON. A compressed row (`zstd` by type or by magic) is decoded first and must be a
/// real frame; a corrupt frame, or a payload over the bound, returns None. A plain row must be `json`.
fn payload(data_type: &str, blob: &[u8]) -> Option<Value> {
    if data_type == "zstd" || blob.starts_with(&ZSTD_MAGIC) {
        if !blob.starts_with(&ZSTD_MAGIC) {
            return None;
        }
        let decoded = dsh::jsonl_bytes(blob.to_vec())?;
        if decoded.len() > MAXIMUM_PAYLOAD {
            return None;
        }
        return serde_json::from_slice(&decoded).ok();
    }
    if data_type != "json" || blob.len() > MAXIMUM_PAYLOAD {
        return None;
    }
    serde_json::from_slice(blob).ok()
}

/// `created_at`, else `updated_at`, else the payload's own `updated_at`.
fn timestamp(row: &rusqlite::Row, thread: &Map<String, Value>) -> Option<DateTime<Utc>> {
    sqlite::text(row, 4)
        .and_then(|text| flexible_text(&text))
        .or_else(|| sqlite::text(row, 1).and_then(|text| flexible_text(&text)))
        .or_else(|| flexible(thread.get("updated_at")))
}

/// A `request_token_usage`, an object of usages or an array of them, summed over the entries that
/// carry work.
fn usage_of(value: Option<&Value>) -> TokenTally {
    let mut total = TokenTally::default();
    match value {
        Some(Value::Array(entries)) => entries.iter().for_each(|entry| add(&mut total, &entry_of(entry))),
        Some(Value::Object(map)) => map.values().for_each(|entry| add(&mut total, &entry_of(entry))),
        _ => {}
    }
    total
}

/// A `cumulative_token_usage`: one usage object.
fn single_usage(value: Option<&Value>) -> TokenTally {
    value.map(entry_of).unwrap_or_default()
}

fn add(total: &mut TokenTally, part: &TokenTally) {
    total.input += part.input;
    total.cache_write += part.cache_write;
    total.cache_read += part.cache_read;
    total.output += part.output;
}

/// One usage entry. Values may be numbers or numeric strings, and a negative clamps to zero;
/// `input_tokens` is fresh input and the cache counters are separate. A zero entry is no work.
fn entry_of(value: &Value) -> TokenTally {
    let Some(object) = value.as_object() else { return TokenTally::default() };
    let tally = TokenTally {
        input: clamped(object.get("input_tokens")),
        cache_write: clamped(object.get("cache_creation_input_tokens")),
        cache_read: clamped(object.get("cache_read_input_tokens")),
        output: clamped(object.get("output_tokens")),
        ..TokenTally::default()
    };
    if tally.total() > 0 {
        tally
    } else {
        TokenTally::default()
    }
}

/// The workspace's directory: `folder_paths` is newline-separated, and `folder_paths_order` names
/// the original index of the first one.
fn project(paths: Option<&str>, order: Option<&str>) -> Option<String> {
    let list: Vec<&str> = paths?.split('\n').map(str::trim).filter(|p| !p.is_empty()).collect();
    let first = *list.first()?;
    let index = order
        .and_then(|order| order.split(',').next())
        .and_then(|first| first.trim().parse::<usize>().ok());
    Some(index.and_then(|i| list.get(i).copied()).unwrap_or(first).to_string())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::spend::calendar::Calendar;
    use crate::spend::prices::PriceTable;
    use crate::spend::sources::{read_agent, SpendAgent};
    use crate::spend::sqlite::tests::database;

    fn store(home: &Path) -> PathBuf {
        let folder = home.join("AppData").join("Roaming").join("Zed").join("threads");
        std::fs::create_dir_all(&folder).unwrap();
        folder.join("threads.db")
    }

    const SCHEMA: &str = "CREATE TABLE threads (id TEXT, summary TEXT, updated_at TEXT, data_type TEXT, data BLOB, created_at TEXT, folder_paths TEXT, folder_paths_order TEXT)";

    fn thread(provider: &str, extra: Value) -> String {
        let mut value = json!({
            "model": {"provider": provider, "model": "claude-x"},
            "request_token_usage": {
                "a": {"input_tokens": 100, "output_tokens": 10, "cache_read_input_tokens": 5, "cache_creation_input_tokens": 2},
                "b": {"input_tokens": 0, "output_tokens": 0},
                "c": {"input_tokens": "50", "output_tokens": -4}
            },
            "cumulative_token_usage": {"input_tokens": 999, "output_tokens": 999}
        });
        if let (Value::Object(base), Value::Object(more)) = (&mut value, extra) {
            base.extend(more);
        }
        value.to_string()
    }

    /// Inserts one row of plain JSON with the given data type and text.
    fn row(id: &str, data_type: &str, data: &str, created: &str, folders: &str, order: &str) -> String {
        format!("INSERT INTO threads VALUES ('{id}','s','2026-09-14T09:00:00Z','{data_type}',CAST('{}' AS BLOB),'{created}','{folders}','{order}')", data.replace('\'', "''"))
    }

    fn run(home: &Path) -> Vec<AgentUsageRecord> {
        Zed.records(&Zed.inputs(&Sources::new(home)))
    }

    #[test]
    fn request_usage_is_summed_over_entries_with_work_and_a_cumulative_is_the_fallback() {
        let home = tempfile::tempdir().unwrap();
        let no_requests = thread("zed.dev", json!({"request_token_usage": {}}));
        let sql = [
            SCHEMA.to_string(),
            row("t1", "json", &thread("zed.dev", json!({})), "2026-09-14T10:00:00Z", "C:\\Users\\me\\code\\Pulse\n/other", "1,0"),
            row("t2", "json", &no_requests, "2026-09-14T11:00:00Z", "", ""),
        ];
        database(&store(home.path()), &sql.iter().map(String::as_str).collect::<Vec<_>>());

        let records = run(home.path());
        let t1 = records.iter().find(|r| r.session_id.as_deref() == Some("t1")).unwrap();
        // 100 + 5 + 2 + 10 from "a", and 50 from "c" (a numeric string); "b" has no work and "c"'s negative output clamps.
        assert_eq!(t1.tally, TokenTally::new(150, 2, 5, 10));
        assert_eq!(t1.project.as_deref(), Some("/other"), "folder_paths_order names index 1");
        assert!(t1.is_aggregate);
        let t2 = records.iter().find(|r| r.session_id.as_deref() == Some("t2")).unwrap();
        assert_eq!(t2.tally, TokenTally::new(999, 0, 0, 999));
        assert_eq!(t2.deduplication_id.as_deref(), Some("zed:t2"));
    }

    #[test]
    fn other_providers_imported_threads_and_compressed_rows_are_skipped() {
        let home = tempfile::tempdir().unwrap();
        let mut zstd = ZSTD_MAGIC.to_vec();
        zstd.extend_from_slice(b"not decoded");
        let sql = [
            SCHEMA.to_string(),
            row("external", "json", &thread("anthropic", json!({})), "2026-09-14T10:00:00Z", "", ""),
            row("imported", "json", &thread("zed.dev", json!({"imported": true})), "2026-09-14T10:00:00Z", "", ""),
            row("labelled", "zstd", "{}", "2026-09-14T10:00:00Z", "", ""),
            format!("INSERT INTO threads VALUES ('magic','s','2026-09-14T09:00:00Z',NULL,X'{}','2026-09-14T10:00:00Z','','')", zstd.iter().map(|b| format!("{b:02X}")).collect::<String>()),
        ];
        database(&store(home.path()), &sql.iter().map(String::as_str).collect::<Vec<_>>());
        assert!(run(home.path()).is_empty());
    }

    /// A zstd frame holding `data` as one raw block (the frame shape the DSH tests build too).
    fn zstd_frame(data: &[u8]) -> Vec<u8> {
        let mut padded = data.to_vec();
        while padded.len() < 256 {
            padded.push(b'\n');
        }
        let len = padded.len();
        let mut out = vec![0x28, 0xB5, 0x2F, 0xFD, 0x60];
        out.extend(((len - 256) as u16).to_le_bytes());
        out.extend(&(((len as u32) << 3) | 1).to_le_bytes()[..3]);
        out.extend(padded);
        out
    }

    #[test]
    fn a_compressed_thread_is_decoded_like_a_plain_one_and_a_corrupt_frame_contributes_nothing() {
        let home = tempfile::tempdir().unwrap();
        let body = thread("zed.dev", json!({}));
        let hex = |bytes: &[u8]| bytes.iter().map(|b| format!("{b:02X}")).collect::<String>();
        let mut corrupt = ZSTD_MAGIC.to_vec();
        corrupt.extend_from_slice(b"not a frame at all");
        let sql = [
            SCHEMA.to_string(),
            format!("INSERT INTO threads VALUES ('packed','s','2026-09-14T09:00:00Z','zstd',X'{}','2026-09-14T10:00:00Z','','')", hex(&zstd_frame(body.as_bytes()))),
            // A mislabelled frame (no `zstd` label) is still read; a corrupt one is not.
            format!("INSERT INTO threads VALUES ('mislabelled','s','2026-09-14T09:00:00Z','json',X'{}','2026-09-14T10:00:00Z','','')", hex(&zstd_frame(body.as_bytes()))),
            format!("INSERT INTO threads VALUES ('broken','s','2026-09-14T09:00:00Z','zstd',X'{}','2026-09-14T10:00:00Z','','')", hex(&corrupt)),
            // Labelled zstd but not a frame: nothing to decode, nothing read.
            row("unframed", "zstd", &body, "2026-09-14T10:00:00Z", "", ""),
        ];
        database(&store(home.path()), &sql.iter().map(String::as_str).collect::<Vec<_>>());

        let mut ids: Vec<String> = run(home.path()).iter().filter_map(|r| r.session_id.clone()).collect();
        ids.sort();
        assert_eq!(ids, vec!["mislabelled".to_string(), "packed".to_string()]);
        let packed = run(home.path()).into_iter().find(|r| r.session_id.as_deref() == Some("packed")).unwrap();
        assert_eq!(packed.tally, TokenTally::new(150, 2, 5, 10));
    }

    #[test]
    fn a_missing_store_is_an_empty_ledger() {
        let home = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::Zed, &Sources::new(home.path()), cache.path(), &Calendar::utc(2), &PriceTable::new());
        assert!(ledger.days.is_empty());
    }
}
