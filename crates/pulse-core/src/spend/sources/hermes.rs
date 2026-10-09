// Ported from upstream Sources/Pulse/Usage/Readers/HermesReader.swift.
//! Hermes Agent's SQLite state: `$HERMES_HOME/state.db`, and one `state.db` per named profile
//! under `profiles/`.
//!
//! ```sql
//! sessions(id, model, billing_provider, started_at, message_count,
//!          input_tokens, output_tokens, cache_read_tokens,
//!          cache_write_tokens, reasoning_tokens, ...)
//! session_model_usage(session_id, model, billing_provider, ...same columns...)
//! ```
//!
//! **A session is a cumulative deployment, not a request.** The per-model `SUM` and the session
//! row both run over the session's whole life, so a rescan of a growing session yields a larger
//! figure under the same identity. Two passes keep that from being counted twice: a session the
//! optional per-model table explains is emitted per model and never re-emitted from the session
//! row; every other session is emitted once from its own totals.
//!
//! **Input is only split when there is no cache to collide with.** The schema does not establish
//! whether `input_tokens` already contains the cache read. With no cache reported the input is
//! fresh input; with a non-zero cache the reported input is carried as unclassified real work and
//! the cache columns are not added at all.
//!
//! **Reasoning is not added to output.** The schema does not establish whether `reasoning_tokens`
//! is already inside `output_tokens`, so the reported output is taken and reasoning is left out.
//!
//! A positive cache or reasoning figure marks the record partial (a known subset).
//!
//! Where it lives on Windows: `%HERMES_HOME%`, else `%USERPROFILE%\.hermes`; the upstream
//! candidates `%LOCALAPPDATA%\hermes\state.db` are also read.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use rusqlite::Connection;

use super::{epoch, list, push_unique, SpendSource};
use crate::spend::logio;
use crate::spend::record::AgentUsageRecord;
use crate::spend::sqlite;
use crate::spend::tally::TokenTally;
use crate::spend::transcripts::Sources;

pub struct Hermes;

impl SpendSource for Hermes {
    fn raw(&self) -> &'static str {
        "hermes"
    }
    fn source_id(&self) -> &'static str {
        "hermes"
    }
    fn display_name(&self) -> &'static str {
        "Hermes"
    }
    fn icon(&self) -> Option<&'static str> {
        Some("hermesagent")
    }
    fn env_vars(&self) -> &'static [&'static str] {
        &["HERMES_HOME", "APPDATA", "LOCALAPPDATA"]
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        // WINDOWS-PATH: unverified (%USERPROFILE%\.hermes, the macOS default under the home folder)
        let root = sources.var("HERMES_HOME").unwrap_or_else(|| sources.home.join(".hermes"));
        let mut roots = vec![root.join("state.db")];
        // `profiles/` itself is watched while it holds no profiles, so a profile created later
        // moves the fingerprint.
        let profiles = root.join("profiles");
        let databases: Vec<PathBuf> = list(&profiles).into_iter().filter(|p| p.is_dir()).map(|p| p.join("state.db")).collect();
        if databases.is_empty() {
            roots.push(profiles);
        } else {
            roots.extend(databases);
        }
        // WINDOWS-PATH: unverified (the upstream candidate under the local app data folder)
        push_unique(&mut roots, sources.local_app_data().join("hermes").join("state.db"));
        roots
    }
    fn records(&self, roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        logio::files(roots, &[], &["state.db"], &[]).iter().flat_map(|file| read(file)).collect()
    }
}

/// The reported counts of one session, or one session and model.
#[derive(Default)]
struct Counts {
    input: i64,
    output: i64,
    /// Read only to mark a record partial; never added to a tally.
    reasoning: i64,
    cache_read: i64,
    cache_write: i64,
}

#[derive(Default)]
struct Session {
    model: Option<String>,
    started_at: Option<DateTime<Utc>>,
    counts: Counts,
}

fn nonblank(text: Option<String>) -> Option<String> {
    text.filter(|t| !t.trim().is_empty())
}

fn read(file: &Path) -> Vec<AgentUsageRecord> {
    let Some(connection) = sqlite::open(file) else { return Vec::new() };
    if sqlite::columns(&connection, "sessions").is_empty() {
        return Vec::new();
    }
    let sessions = sessions(&connection);
    let mut records = per_model(&connection, &sessions);
    let covered: HashSet<String> = records.iter().filter_map(|r| r.session_id.clone()).collect();
    records.extend(session_totals(&sessions, &covered));
    records
}

fn sessions(connection: &Connection) -> HashMap<String, Session> {
    let sql = sqlite::select(
        connection,
        "sessions",
        &["id", "model", "started_at", "input_tokens", "output_tokens", "cache_read_tokens", "cache_write_tokens", "reasoning_tokens"],
    );
    let mut rows = HashMap::new();
    sqlite::each(connection, &sql, |row| {
        let Some(id) = sqlite::text(row, 0) else { return };
        let session = Session {
            model: sqlite::text(row, 1),
            started_at: sqlite::number(row, 2).and_then(epoch),
            counts: Counts {
                input: sqlite::count(row, 3).unwrap_or(0),
                output: sqlite::count(row, 4).unwrap_or(0),
                cache_read: sqlite::count(row, 5).unwrap_or(0),
                cache_write: sqlite::count(row, 6).unwrap_or(0),
                reasoning: sqlite::count(row, 7).unwrap_or(0),
            },
        };
        rows.insert(id, session);
    });
    rows
}

/// Pass one: `session_model_usage` grouped by session, model and provider, each column summed. A
/// group with any tokens, in a session with a start time, is one record.
fn per_model(connection: &Connection, sessions: &HashMap<String, Session>) -> Vec<AgentUsageRecord> {
    if sqlite::columns(connection, "session_model_usage").is_empty() {
        return Vec::new();
    }
    let sql = sqlite::select(
        connection,
        "session_model_usage",
        &["session_id", "model", "billing_provider", "input_tokens", "output_tokens", "cache_read_tokens", "cache_write_tokens", "reasoning_tokens"],
    );
    let mut groups: HashMap<(String, String, Option<String>), Counts> = HashMap::new();
    sqlite::each(connection, &sql, |row| {
        let (Some(session), Some(model)) = (sqlite::text(row, 0), nonblank(sqlite::text(row, 1))) else { return };
        let sums = groups.entry((session, model, sqlite::text(row, 2))).or_default();
        sums.input += sqlite::count(row, 3).unwrap_or(0);
        sums.output += sqlite::count(row, 4).unwrap_or(0);
        sums.cache_read += sqlite::count(row, 5).unwrap_or(0);
        sums.cache_write += sqlite::count(row, 6).unwrap_or(0);
        sums.reasoning += sqlite::count(row, 7).unwrap_or(0);
    });
    groups
        .into_iter()
        .filter_map(|((session, model, provider), sums)| {
            let total = sums.input + sums.output + sums.cache_read + sums.cache_write + sums.reasoning;
            if total <= 0 {
                return None;
            }
            let started = sessions.get(&session)?.started_at?;
            // A NULL provider stays a distinct key from an empty string.
            let provider = provider.unwrap_or_else(|| "<null>".to_string());
            record(started, &model, &sums, &session, format!("hermes:{session}:{model}:{provider}"))
        })
        .collect()
}

/// Pass two: a session's own row, for a session the per-model table did not cover.
fn session_totals(sessions: &HashMap<String, Session>, covered: &HashSet<String>) -> Vec<AgentUsageRecord> {
    sessions
        .iter()
        .filter(|(id, _)| !covered.contains(*id))
        .filter_map(|(id, session)| {
            // No stated start or no stated model: nothing to bucket the tokens under.
            let started = session.started_at?;
            let model = nonblank(session.model.clone())?;
            record(started, &model, &session.counts, id, id.clone())
        })
        .collect()
}

/// One aggregate record from the reported columns. With a cache reported, the input may already
/// hold it, so the input is carried as unclassified and the cache is not added beside it.
fn record(at: DateTime<Utc>, model: &str, counts: &Counts, session: &str, dedup: String) -> Option<AgentUsageRecord> {
    let has_cache = counts.cache_read > 0 || counts.cache_write > 0;
    let tally = if has_cache {
        TokenTally { output: counts.output, ..TokenTally::default() }
    } else {
        TokenTally { input: counts.input, output: counts.output, ..TokenTally::default() }
    };
    let unclassified = if has_cache { counts.input } else { 0 };
    if tally.total() + unclassified <= 0 {
        return None;
    }
    let mut record = AgentUsageRecord::new(at, model, tally).session(session).unclassified(unclassified).aggregate(true);
    record.deduplication_id = Some(dedup);
    record.is_partial = has_cache || counts.reasoning > 0;
    Some(record)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spend::calendar::Calendar;
    use crate::spend::prices::PriceTable;
    use crate::spend::sources::{read_agent, SpendAgent};
    use crate::spend::sqlite::tests::database;

    const AT: i64 = 1_789_372_800;

    fn store(home: &Path) -> PathBuf {
        let folder = home.join(".hermes");
        std::fs::create_dir_all(&folder).unwrap();
        folder.join("state.db")
    }

    fn schema() -> Vec<String> {
        vec![
            "CREATE TABLE sessions (id TEXT, model TEXT, billing_provider TEXT, started_at REAL, input_tokens INTEGER, output_tokens INTEGER, cache_read_tokens INTEGER, cache_write_tokens INTEGER, reasoning_tokens INTEGER)".into(),
            "CREATE TABLE session_model_usage (session_id TEXT, model TEXT, billing_provider TEXT, input_tokens INTEGER, output_tokens INTEGER, cache_read_tokens INTEGER, cache_write_tokens INTEGER, reasoning_tokens INTEGER)".into(),
        ]
    }

    fn run(home: &Path) -> Vec<AgentUsageRecord> {
        let roots = Hermes.inputs(&Sources::new(home));
        Hermes.records(&roots)
    }

    #[test]
    fn a_session_without_cache_is_fresh_input_and_a_cached_one_is_unclassified() {
        let home = tempfile::tempdir().unwrap();
        let mut sql = schema();
        sql.push(format!("INSERT INTO sessions VALUES ('plain','m-a','p',{AT},100,20,0,0,5)"));
        sql.push(format!("INSERT INTO sessions VALUES ('cached','m-b','p',{AT},500,30,400,0,0)"));
        database(&store(home.path()), &sql.iter().map(String::as_str).collect::<Vec<_>>());

        let records = run(home.path());
        let plain = records.iter().find(|r| r.session_id.as_deref() == Some("plain")).unwrap();
        // Reasoning is not added to output and marks the record partial.
        assert_eq!(plain.tally, TokenTally::new(100, 0, 0, 20));
        assert_eq!(plain.unclassified_tokens, 0);
        assert!(plain.is_partial && plain.is_aggregate);

        let cached = records.iter().find(|r| r.session_id.as_deref() == Some("cached")).unwrap();
        assert_eq!(cached.tally, TokenTally::new(0, 0, 0, 30));
        assert_eq!(cached.unclassified_tokens, 500);
        assert!(cached.is_partial);
    }

    #[test]
    fn a_session_with_a_model_table_is_counted_per_model_and_not_again_from_its_row() {
        let home = tempfile::tempdir().unwrap();
        let mut sql = schema();
        sql.push(format!("INSERT INTO sessions VALUES ('s','m-a','p',{AT},999,999,0,0,0)"));
        sql.push("INSERT INTO session_model_usage VALUES ('s','m-a','p',10,1,0,0,0)".into());
        sql.push("INSERT INTO session_model_usage VALUES ('s','m-a','p',20,2,0,0,0)".into());
        sql.push("INSERT INTO session_model_usage VALUES ('s','m-b','p',5,0,0,0,0)".into());
        // A row with no time is dropped whole, and a group with no tokens is nothing.
        sql.push("INSERT INTO sessions VALUES ('undated','m-a','p',NULL,7,7,0,0,0)".into());
        sql.push("INSERT INTO session_model_usage VALUES ('undated','m-a','p',7,7,0,0,0)".into());
        database(&store(home.path()), &sql.iter().map(String::as_str).collect::<Vec<_>>());

        let records = run(home.path());
        let mut kinds: Vec<(String, i64)> = records.iter().map(|r| (r.model.clone(), r.tally.total())).collect();
        kinds.sort();
        assert_eq!(kinds, [("m-a".to_string(), 33), ("m-b".to_string(), 5)]);
    }

    #[test]
    fn a_session_row_is_read_only_when_the_model_table_does_not_cover_it() {
        let home = tempfile::tempdir().unwrap();
        let mut sql = schema();
        sql.push(format!("INSERT INTO sessions VALUES ('solo','m-z','p',{},11,4,0,0,0)", AT * 1000));
        database(&store(home.path()), &sql.iter().map(String::as_str).collect::<Vec<_>>());
        let records = run(home.path());
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].tally, TokenTally::new(11, 0, 0, 4));
        assert_eq!(records[0].timestamp.timestamp(), AT, "a millisecond epoch is read as milliseconds");
    }

    #[test]
    fn a_missing_store_is_an_empty_ledger() {
        let home = tempfile::tempdir().unwrap();
        assert!(run(home.path()).is_empty());
        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::Hermes, &Sources::new(home.path()), cache.path(), &Calendar::utc(2), &PriceTable::new());
        assert!(ledger.days.is_empty());
    }

    #[test]
    fn an_environment_override_moves_the_store() {
        let home = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let mut sql = schema();
        sql.push(format!("INSERT INTO sessions VALUES ('s','m-a','p',{AT},3,1,0,0,0)"));
        database(&elsewhere.path().join("state.db"), &sql.iter().map(String::as_str).collect::<Vec<_>>());
        let sources = Sources::new(home.path()).with_var("HERMES_HOME", elsewhere.path());
        let records = Hermes.records(&Hermes.inputs(&sources));
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].tally.total(), 4);
    }

    #[test]
    fn a_profile_database_is_read_too() {
        let home = tempfile::tempdir().unwrap();
        let profile = home.path().join(".hermes").join("profiles").join("work");
        std::fs::create_dir_all(&profile).unwrap();
        let mut sql = schema();
        sql.push(format!("INSERT INTO sessions VALUES ('w','m-a','p',{AT},8,2,0,0,0)"));
        database(&profile.join("state.db"), &sql.iter().map(String::as_str).collect::<Vec<_>>());
        assert_eq!(run(home.path()).iter().map(|r| r.tally.total()).sum::<i64>(), 10);
    }
}
