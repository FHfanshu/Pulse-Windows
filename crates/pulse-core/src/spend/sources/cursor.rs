// Ported from upstream Sources/Pulse/Usage/Readers/CapturedCursorReader.swift (and CapturedCSV.swift).
//! Cursor's usage cache: a dashboard export of `get-filtered-usage-events` (JSON), plus the older CSV
//! shape it replaced, in `cursor-cache/usage*.json` / `usage*.csv` or Pulse's own drop folder.
//!
//! This is **not a native Cursor file**. The cache is written by an export step that needs the user's
//! Cursor authentication; Pulse only reads it. Shape is proved, not assumed from the name: a JSON file
//! is read when its root holds a `usageEventsDisplay` array, and a CSV when its header names the date,
//! model and the counter columns. Anything else is left alone.
//!
//! A session exists only where the export names one (a JSON `conversationId`, or a CSV `Cloud Agent
//! ID`). An event with neither counts its tokens and creates no session row.
//!
//! Equal rows are not merged inside one file, but overlapping exports of one scope are reconciled by
//! content multiset (`{A}` and `{A, B}` give `A` and `B`), with an unconfirmed increment marked
//! partial. A byte-identical file is folded outright. A native `usage.<account>` name declares an
//! account; any other file shares one unknown import scope and is marked partial. No cost is carried:
//! `chargedCents` and `totalCents` are ignored; pricing is the price table's.
//!
//! Where it lives on Windows: `TOKSCALE_CONFIG_DIR` when set, else `%USERPROFILE%\.config\tokscale`,
//! then `\cursor-cache`. `WINDOWS-PATH: unverified`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, TimeZone, Utc};
use serde_json::Value;

use super::{push_unique, reconcile, replay_distinct, SpendSource};
use crate::provider::Provider;
use crate::spend::logio;
use crate::spend::record::AgentUsageRecord;
use crate::spend::tally::TokenTally;
use crate::spend::transcripts::Sources;

pub struct Cursor;

impl SpendSource for Cursor {
    fn raw(&self) -> &'static str {
        "cursor"
    }
    fn source_id(&self) -> &'static str {
        "cursor"
    }
    fn display_name(&self) -> &'static str {
        "Cursor"
    }
    fn card_provider(&self) -> Option<Provider> {
        Some(Provider::Cursor)
    }
    fn icon(&self) -> Option<&'static str> {
        Some("cursor")
    }
    fn env_vars(&self) -> &'static [&'static str] {
        &["TOKSCALE_CONFIG_DIR", "APPDATA"]
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        let mut roots = Vec::new();
        // WINDOWS-PATH: unverified
        let cache = sources.var("TOKSCALE_CONFIG_DIR").unwrap_or_else(|| sources.home.join(".config").join("tokscale"));
        push_unique(&mut roots, cache.join("cursor-cache"));
        push_unique(&mut roots, sources.app_data().join("Pulse").join("UsageImports").join("cursor"));
        roots
    }
    fn records(&self, roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        let files: Vec<PathBuf> = logio::files(roots, &["json", "csv"], &[], &[])
            .into_iter()
            .filter(|file| is_candidate(file))
            .collect();

        // Grouped by scope: a declared account, or the one shared import scope.
        let mut json_by_scope: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
        let mut csv_by_scope: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
        for file in files {
            let scope = scope_of(&file);
            if extension_of(&file) == "json" {
                json_by_scope.entry(scope).or_default().push(file);
            } else {
                csv_by_scope.entry(scope).or_default().push(file);
            }
        }

        let mut scopes: Vec<String> = json_by_scope.keys().chain(csv_by_scope.keys()).cloned().collect();
        scopes.sort();
        scopes.dedup();

        let mut records = Vec::new();
        for scope in scopes {
            let account = scope.strip_prefix("account:").map(str::to_string);
            let json = json_by_scope.get(&scope).cloned().unwrap_or_default();
            let csv = csv_by_scope.get(&scope).cloned().unwrap_or_default();
            records.extend(scope_records(account.as_deref(), &json, &csv));
        }
        records
    }
    fn requires_usage_export(&self) -> bool {
        true
    }
}

/// The records of one scope: its JSON lane and its CSV lane, each reconciled, with the JSON lane
/// authoritative where both cover a range.
fn scope_records(account: Option<&str>, json: &[PathBuf], csv: &[PathBuf]) -> Vec<AgentUsageRecord> {
    // A byte-identical file dropped twice is one export; the lanes are deduplicated separately.
    let label = account.unwrap_or("unscoped");
    let json_per_file: Vec<Vec<AgentUsageRecord>> =
        replay_distinct(json).iter().filter_map(|file| json_records(file, label)).collect();
    let csv_per_file: Vec<Vec<AgentUsageRecord>> =
        replay_distinct(csv).iter().filter_map(|file| csv_records(file, label)).collect();

    let (json_records, json_overlap) = reconcile(&json_per_file, signature);
    let (csv_records, csv_overlap) = reconcile(&csv_per_file, signature);
    let mut partial = json_overlap || csv_overlap;

    let mut chosen: Vec<AgentUsageRecord>;
    if !json_records.is_empty() {
        // JSON is authoritative. A CSV row inside its date range is unverifiable overlap and is not
        // added; a row outside it is kept. Either way the account is marked incomplete.
        let first = json_records.iter().map(|r| r.timestamp).min();
        let last = json_records.iter().map(|r| r.timestamp).max();
        chosen = json_records.clone();
        let mut inside = 0;
        for record in csv_records {
            match (first, last) {
                (Some(first), Some(last)) if record.timestamp >= first && record.timestamp <= last => inside += 1,
                _ => chosen.push(record),
            }
        }
        if inside > 0 {
            partial = true;
        }
    } else {
        // No usable JSON: an empty or unreadable file must not suppress a valid CSV.
        chosen = csv_records;
    }

    // A file that declared no account cannot be scoped, so its totals are not claimed complete.
    if account.is_none() {
        partial = true;
    }
    if partial {
        for record in &mut chosen {
            record.is_partial = true;
        }
    }
    chosen
}

/// A row's content identity, used only to reconcile overlapping exports.
fn signature(record: &AgentUsageRecord) -> String {
    format!(
        "{}:{}:{}:{}:{}:{}:{}",
        record.session_id.as_deref().unwrap_or(""),
        record.timestamp.timestamp_millis(),
        record.model,
        record.tally.input,
        record.tally.cache_write,
        record.tally.cache_read,
        record.tally.output,
    )
}

/// `usage.backup*` is a stale copy of the account's cache and would be read as live: excluded.
fn is_candidate(file: &Path) -> bool {
    let stem = stem_of(file).to_lowercase();
    !stem.starts_with("usage.backup")
}

/// `usage.<account>` declares an account, `usage` alone is the active one; any other name is the
/// shared import scope.
fn scope_of(file: &Path) -> String {
    let stem = stem_of(file);
    let parts: Vec<&str> = stem.split('.').collect();
    if parts.first().map(|p| p.to_lowercase()).as_deref() == Some("usage") {
        if parts.len() >= 2 {
            if let Some(account) = slug(parts[1]) {
                return format!("account:{account}");
            }
        }
        return "account:active".to_string();
    }
    "import".to_string()
}

/// An ASCII slug: letters and digits, every other run collapsed to one `-`, lowercased.
fn slug(value: &str) -> Option<String> {
    let mut output = String::new();
    let mut pending_dash = false;
    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            if pending_dash && !output.is_empty() {
                output.push('-');
            }
            pending_dash = false;
            output.push(character);
        } else if !output.is_empty() {
            pending_dash = true;
        }
    }
    (!output.is_empty()).then(|| output.to_lowercase())
}

fn stem_of(file: &Path) -> String {
    file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

fn extension_of(file: &Path) -> String {
    file.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

/// `None` when the file is not a Cursor JSON; `Some` (possibly empty) when it is one.
fn json_records(file: &Path, account: &str) -> Option<Vec<AgentUsageRecord>> {
    let root = logio::json(file)?;
    let events = root.get("usageEventsDisplay")?.as_array()?;

    let mut records = Vec::new();
    for event in events {
        // A blank model names nothing, and no time means the event cannot be placed on a day.
        let Some(model) = logio::text(event.get("model")) else { continue };
        let Some(at) = logio::timestamp(event.get("timestamp"), true).filter(|at| at.timestamp() > 0) else { continue };

        let usage = event.get("tokenUsage").filter(|v| v.is_object());
        let field = |name: &str| usage.and_then(|u| logio::count(u.get(name))).unwrap_or(0);
        let tally = TokenTally::new(field("inputTokens"), field("cacheWriteTokens"), field("cacheReadTokens"), field("outputTokens"));
        if tally.total() <= 0 {
            continue;
        }

        let mut record = AgentUsageRecord::new(at, &model, tally);
        // Only an export-stated conversation is a session; the account keeps two accounts' ids apart.
        if let Some(conversation) = logio::text(event.get("conversationId")) {
            record.session_id = Some(format!("cursor:{account}:{conversation}"));
        }
        records.push(record);
    }
    Some(records)
}

/// `None` when the header is not a Cursor CSV; otherwise the rows it holds.
fn csv_records(file: &Path, account: &str) -> Option<Vec<AgentUsageRecord>> {
    let text = String::from_utf8_lossy(&std::fs::read(file).ok()?).into_owned();
    let rows: Vec<Vec<String>> = csv_rows(&text).into_iter().filter(|row| row.iter().any(|cell| !cell.trim().is_empty())).collect();

    let header_index = rows.iter().position(|row| {
        let names: Vec<String> = row.iter().map(|c| c.trim().to_lowercase()).collect();
        names.iter().any(|n| n == "date") && names.iter().any(|n| n == "model")
    })?;

    let header: Vec<String> = rows[header_index].iter().map(|c| c.trim().to_lowercase()).collect();
    let column = |name: &str| header.iter().position(|h| h == name);
    // Column names, not positions: a v1 export is not read as a v3 one. A header lacking any counter
    // is not this schema.
    let date_column = column("date")?;
    let model_column = column("model")?;
    let input_column = column("input (w/o cache write)")?;
    let cache_write_column = column("input (w/ cache write)")?;
    let cache_read_column = column("cache read")?;
    let output_column = column("output tokens")?;
    // Only a stated cloud agent id is a session.
    let cloud_column = column("cloud agent id");

    let widest = [date_column, model_column, input_column, cache_write_column, cache_read_column, output_column].into_iter().max()?;
    let mut records = Vec::new();
    for row in &rows[header_index + 1..] {
        // A short row is a broken record, not a row of zeroes.
        if row.len() <= widest {
            continue;
        }
        let Some(at) = csv_date(row[date_column].trim()).filter(|at| at.timestamp() > 0) else { continue };
        let Some(model) = logio::text(Some(&Value::String(row[model_column].clone()))) else { continue };

        // The columns are independent buckets: `w/o cache write` is fresh input, `w/ cache write` the
        // cache-write bucket. `Total Tokens` is deliberately not read.
        let tally = TokenTally::new(
            csv_count(&row[input_column]),
            csv_count(&row[cache_write_column]),
            csv_count(&row[cache_read_column]),
            csv_count(&row[output_column]),
        );
        if tally.total() <= 0 {
            continue;
        }

        let mut record = AgentUsageRecord::new(at, &model, tally);
        if let Some(cloud) = cloud_column.and_then(|c| row.get(c)).and_then(|c| logio::text(Some(&Value::String(c.clone())))) {
            record.session_id = Some(format!("cursor:{account}:cloud:{cloud}"));
        }
        // A legacy usage report states a day, not the quarter-hour a request started in.
        record.is_aggregate = true;
        records.push(record);
    }
    Some(records)
}

/// A counter in a CSV cell. A quoted thousands separator is a formatting artefact, dropped first.
fn csv_count(raw: &str) -> i64 {
    logio::count(Some(&Value::String(raw.replace(',', "")))).unwrap_or(0)
}

/// The CSV date-time in its several ISO-ish spellings. A bare `yyyy-MM-dd` is local midnight, so its
/// calendar day is the day the report states. Nothing falls back to the clock.
fn csv_date(raw: &str) -> Option<DateTime<Utc>> {
    if let Some(iso) = logio::timestamp(Some(&Value::String(raw.to_string())), false) {
        return Some(iso);
    }
    if let Ok(with_offset) = DateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S%z") {
        return Some(with_offset.with_timezone(&Utc));
    }
    if let Ok(naive) = NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S") {
        return Local.from_local_datetime(&naive).single().map(|local| local.with_timezone(&Utc));
    }
    let day = NaiveDate::parse_from_str(raw, "%Y-%m-%d").ok()?;
    Local.from_local_datetime(&day.and_hms_opt(0, 0, 0)?).single().map(|local| local.with_timezone(&Utc))
}

/// RFC 4180 rows: quoted fields with `""` as one quote, commas and newlines inside quotes kept, `\r\n`,
/// `\n` and a lone `\r` ending a record, a BOM stripped, and no phantom empty record at the end.
fn csv_rows(text: &str) -> Vec<Vec<String>> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut record: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = text.chars().peekable();

    while let Some(character) = chars.next() {
        if in_quotes {
            if character == '"' {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    in_quotes = false;
                }
            } else {
                field.push(character);
            }
            continue;
        }
        match character {
            '"' => in_quotes = true,
            ',' => record.push(std::mem::take(&mut field)),
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                record.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut record));
            }
            '\n' => {
                record.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut record));
            }
            other => field.push(other),
        }
    }
    if !field.is_empty() || !record.is_empty() {
        record.push(field);
        rows.push(record);
    }
    rows
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::json;

    use super::*;
    use crate::spend::calendar::Calendar;
    use crate::spend::prices::{ModelPrice, PriceTable};
    use crate::spend::sources::{read_agent, SpendAgent};

    fn priced() -> PriceTable {
        HashMap::from([("cursor-model".to_string(), ModelPrice::new(1.0, 10.0, Some(0.1), Some(1.0), Some("Cursor")))])
    }

    fn cache(home: &Path) -> PathBuf {
        let folder = home.join(".config").join("tokscale").join("cursor-cache");
        std::fs::create_dir_all(&folder).unwrap();
        folder
    }

    fn event(conversation: Option<&str>, millis: i64, input: i64, output: i64) -> Value {
        let mut event = json!({"model": "cursor-model", "timestamp": millis.to_string(),
                               "tokenUsage": {"inputTokens": input, "outputTokens": output, "cacheWriteTokens": 0, "cacheReadTokens": 0}});
        if let Some(conversation) = conversation {
            event["conversationId"] = json!(conversation);
        }
        event
    }

    fn export(rows: &[Value]) -> String {
        json!({"usageEventsDisplay": rows}).to_string()
    }

    #[test]
    fn a_declared_account_json_export_reads_its_four_kinds_and_only_a_named_conversation_is_a_session() {
        let home = tempfile::tempdir().unwrap();
        let now = Utc::now().timestamp_millis();
        std::fs::write(
            cache(home.path()).join("usage.work.json"),
            export(&[event(Some("c1"), now, 10, 4), event(None, now, 3, 1), json!({"model": "", "timestamp": now.to_string(), "tokenUsage": {"inputTokens": 9}})]),
        )
        .unwrap();
        let cache_dir = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::Cursor, &Sources::new(home.path()), cache_dir.path(), &Calendar::utc(2), &priced());
        let total: i64 = ledger.days.iter().map(|d| d.tokens).sum();
        assert_eq!(total, 18);
        // A declared account is not partial; only one session exists (the named conversation).
        assert_eq!(ledger.sessions.len(), 1);
        assert!(!ledger.has_partial_counts);
    }

    #[test]
    fn overlapping_exports_of_one_scope_reconcile_by_multiset_and_an_undeclared_file_is_partial() {
        let home = tempfile::tempdir().unwrap();
        let now = Utc::now().timestamp_millis();
        let a = event(Some("c"), now, 10, 0);
        let b = event(Some("c"), now + 60_000, 20, 0);
        // A plain (undeclared) name shares the import scope; {A} and {A, B} give A and B.
        std::fs::write(cache(home.path()).join("export-one.json"), export(std::slice::from_ref(&a))).unwrap();
        std::fs::write(cache(home.path()).join("export-two.json"), export(&[a.clone(), b.clone()])).unwrap();
        let cache_dir = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::Cursor, &Sources::new(home.path()), cache_dir.path(), &Calendar::utc(2), &priced());
        assert_eq!(ledger.days.iter().map(|d| d.tokens).sum::<i64>(), 30);
        assert!(ledger.has_partial_counts);
    }

    #[test]
    fn the_legacy_csv_reads_by_column_name_and_a_quoted_thousands_separator_is_one_number() {
        let home = tempfile::tempdir().unwrap();
        let csv = "Date,Model,Kind,Input (w/o Cache Write),Input (w/ Cache Write),Cache Read,Output Tokens,Total Tokens,Cloud Agent ID\n\
                   2026-03-01,cursor-model,Included,\"1,000\",5,7,2,9999,agent-9\n\
                   2026-03-01,cursor-model,Included,1,0,0,0,0,\n\
                   2026-03-01\n";
        std::fs::write(cache(home.path()).join("usage.csv"), csv).unwrap();
        let cache_dir = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::Cursor, &Sources::new(home.path()), cache_dir.path(), &Calendar::utc(2), &priced());
        // "1,000" + 5 + 7 + 2 = 1014, the row of 1 is 1, and the short row is skipped.
        assert_eq!(ledger.days.iter().map(|d| d.tokens).sum::<i64>(), 1015);
    }

    #[test]
    fn a_missing_store_is_empty_and_the_slug_and_csv_rows_read_upstream_shapes() {
        let home = tempfile::tempdir().unwrap();
        let cache_dir = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::Cursor, &Sources::new(home.path()), cache_dir.path(), &Calendar::utc(2), &priced());
        assert!(ledger.days.is_empty());
        assert_eq!(slug("My Work!").as_deref(), Some("my-work"));
        assert_eq!(scope_of(Path::new("usage.Work.json")), "account:work");
        assert_eq!(scope_of(Path::new("usage.json")), "account:active");
        assert_eq!(scope_of(Path::new("notes.json")), "import");
        assert_eq!(csv_rows("a,\"b,c\"\n\"x\"\"y\"\r\n"), vec![vec!["a".to_string(), "b,c".to_string()], vec!["x\"y".to_string()]]);
    }
}
