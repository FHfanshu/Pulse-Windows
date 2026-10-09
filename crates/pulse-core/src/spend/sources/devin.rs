// Ported from upstream Sources/Pulse/Usage/DevinCLIStore.swift.
//! Devin's CLI, which keeps its conversations in SQLite: `devin/cli/sessions.db`.
//!
//! `message_nodes.chat_message` is the message as JSON, and an assistant's carries its own
//! metrics:
//!
//! ```json
//! { "role": "assistant", "metadata": {
//!     "generation_model": "...", "created_at": 0,
//!     "metrics": { "input_tokens": 11486, "output_tokens": 254,
//!                  "cache_read_tokens": 6450, "cache_creation_tokens": null } } }
//! ```
//!
//! **Not the same store as the quota route.** The Devin usage service reads the desktop app's
//! saved plan for the ring; this is the CLI's own transcript database, and the two know nothing
//! about each other.
//!
//! **Shown as "Devin", not "Devin CLI".** The `cli` in the path is Devin's own directory layout,
//! not a product marker: the database is written by Devin Desktop too, which embeds the same core
//! and drives it, so the name does not claim a client the database cannot settle.
//!
//! **One message, two nodes.** The CLI keeps a message's node and a sibling under the same parent
//! with the same `message_id` and the same metrics; read node by node, every reply counted twice.
//! A message is counted once, by its id, or by its request where it has none.
//!
//! Where it lives on Windows: upstream's second-hand notes give a Windows `%APPDATA%` root, so
//! `%APPDATA%\devin\cli\sessions.db` is read, with the `%LOCALAPPDATA%` spelling and the
//! `~/.local/share/devin/cli/sessions.db` layout of the other platforms. Not read (further work
//! upstream): `metadata.num_tokens` attributed to output when `metrics` is absent.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::{int, push_unique, SpendSource};
use crate::spend::record::AgentUsageRecord;
use crate::spend::sqlite;
use crate::spend::tally::TokenTally;
use crate::spend::transcripts::Sources;

pub struct DevinCli;

impl SpendSource for DevinCli {
    fn raw(&self) -> &'static str {
        "devinCLI"
    }
    fn source_id(&self) -> &'static str {
        "devin-cli"
    }
    fn display_name(&self) -> &'static str {
        "Devin"
    }
    fn icon(&self) -> Option<&'static str> {
        Some("devin")
    }
    fn env_vars(&self) -> &'static [&'static str] {
        &["XDG_DATA_HOME", "APPDATA", "LOCALAPPDATA"]
    }
    fn has_captured_validation(&self) -> bool {
        true
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        let tail = |base: PathBuf| base.join("devin").join("cli").join("sessions.db");
        let mut roots = Vec::new();
        // WINDOWS-PATH: unverified
        push_unique(&mut roots, tail(sources.app_data()));
        // WINDOWS-PATH: unverified
        push_unique(&mut roots, tail(sources.local_app_data()));
        // WINDOWS-PATH: unverified (the macOS and Linux layout, in case Windows follows it)
        push_unique(&mut roots, tail(sources.xdg_data_home()));
        roots
    }
    fn records(&self, roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        let mut records = Vec::new();
        // One message is one message in whichever database it is found.
        let mut seen: HashSet<String> = HashSet::new();
        for file in roots.iter().filter(|p| p.is_file()) {
            let Some(connection) = sqlite::open(file) else { continue };

            let mut sessions: HashMap<String, (Option<String>, Option<String>)> = HashMap::new();
            sqlite::each(&connection, "SELECT id, working_directory, title FROM sessions", |row| {
                if let Some(id) = sqlite::text(row, 0) {
                    sessions.insert(id, (sqlite::text(row, 1), sqlite::text(row, 2)));
                }
            });

            sqlite::each(&connection, "SELECT session_id, chat_message, created_at FROM message_nodes", |row| {
                let (Some(session), Some(message)) = (sqlite::text(row, 0), sqlite::text(row, 1)) else { return };
                let Ok(root) = serde_json::from_str::<Value>(&message) else { return };
                let Some(metadata) = root.get("metadata").filter(|m| m.is_object()) else { return };
                let Some(metrics) = metadata.get("metrics").filter(|m| m.is_object()) else { return };

                let tally = TokenTally {
                    input: int(metrics.get("input_tokens")),
                    cache_write: int(metrics.get("cache_creation_tokens")),
                    cache_read: int(metrics.get("cache_read_tokens")),
                    output: int(metrics.get("output_tokens")),
                    ..TokenTally::default()
                };
                if tally.total() <= 0 {
                    return;
                }
                let message_id = root
                    .get("message_id")
                    .and_then(Value::as_str)
                    .or_else(|| metadata.get("request_id").and_then(Value::as_str));
                if let Some(id) = message_id {
                    if !seen.insert(id.to_string()) {
                        return;
                    }
                }
                let seconds = sqlite::integer(row, 2);
                let Some(at) = instant(seconds) else { return };
                let model = metadata.get("generation_model").and_then(Value::as_str).unwrap_or("devin");

                let (directory, title) = sessions.get(&session).cloned().unwrap_or_default();
                let mut record = AgentUsageRecord::new(at, model, tally).session(&session);
                record.session_name = Some(session.clone());
                record.title = title;
                record.project = directory;
                records.push(record);
            });
        }
        records
    }
}

/// Seconds, or milliseconds where the number is too large to be seconds; None for no time.
fn instant(value: i64) -> Option<DateTime<Utc>> {
    if value <= 0 {
        return None;
    }
    if value > 10_000_000_000 {
        DateTime::from_timestamp_millis(value)
    } else {
        DateTime::from_timestamp(value, 0)
    }
}

#[cfg(test)]
mod tests {
    use chrono::Duration;
    use serde_json::json;

    use super::*;
    use crate::spend::calendar::Calendar;
    use crate::spend::prices::{ModelPrice, PriceTable};
    use crate::spend::sources::{read_agent, SpendAgent};
    use crate::spend::sqlite::tests::database;
    use crate::spend::summary::SpendSummary;

    fn store(home: &std::path::Path) -> PathBuf {
        let folder = home.join("AppData").join("Roaming").join("devin").join("cli");
        std::fs::create_dir_all(&folder).unwrap();
        folder.join("sessions.db")
    }

    const SCHEMA: [&str; 2] = [
        "CREATE TABLE sessions (id TEXT, working_directory TEXT, title TEXT)",
        "CREATE TABLE message_nodes (session_id TEXT, chat_message TEXT, created_at INTEGER)",
    ];

    fn reply(id: Option<&str>, input: i64, output: i64) -> String {
        let mut message = json!({"role":"assistant","metadata":{"generation_model":"priced","metrics":{"input_tokens":input,"output_tokens":output,"cache_read_tokens":6,"cache_creation_tokens":null}}});
        if let Some(id) = id {
            message["message_id"] = json!(id);
        }
        message.to_string()
    }

    #[test]
    fn a_message_kept_as_two_sibling_nodes_is_counted_once() {
        let home = tempfile::tempdir().unwrap();
        let at: i64 = 1_789_372_800;
        let mut sql: Vec<String> = SCHEMA.iter().map(|s| s.to_string()).collect();
        sql.push("INSERT INTO sessions VALUES ('s1','C:\\Users\\me\\code\\Pulse','Fix the ring')".into());
        sql.push(format!("INSERT INTO message_nodes VALUES ('s1','{}',{at})", reply(Some("m1"), 100, 10)));
        sql.push(format!("INSERT INTO message_nodes VALUES ('s1','{}',{at})", reply(Some("m1"), 100, 10)));
        sql.push(format!("INSERT INTO message_nodes VALUES ('s1','{}',{at})", reply(Some("m2"), 50, 5)));
        // No message id and no request id: kept every time, like any request with no identity.
        sql.push(format!("INSERT INTO message_nodes VALUES ('s1','{}',{})", reply(None, 1, 0), at * 1000));
        // Not metered, and not dated: neither is usage.
        sql.push(format!("INSERT INTO message_nodes VALUES ('s1','{}',{at})", json!({"role":"user"})));
        sql.push(format!("INSERT INTO message_nodes VALUES ('s1','{}',0)", reply(Some("m9"), 777, 0)));
        database(&store(home.path()), &sql.iter().map(String::as_str).collect::<Vec<_>>());

        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::DevinCli, &Sources::new(home.path()), cache.path(), &Calendar::utc(2), &PriceTable::new());
        // (110 + 6) + (55 + 6) + (1 + 6)
        assert_eq!(ledger.days.iter().map(|d| d.tokens).sum::<i64>(), 116 + 61 + 7);
        let session = &ledger.sessions[0];
        assert_eq!(session.title.as_deref(), Some("Fix the ring"));
        assert_eq!(session.name, "s1");
        assert_eq!(session.project.as_ref().unwrap().name, "Pulse");
    }

    #[test]
    fn a_sessions_buckets_reach_today_and_only_todays_counts() {
        let home = tempfile::tempdir().unwrap();
        let calendar = Calendar::local();
        let now = Utc::now();
        let today = (calendar.start_of_day(now) + Duration::hours(12)).timestamp();
        let yesterday = (calendar.add_days(calendar.start_of_day(now), -1) + Duration::hours(12)).timestamp();
        let plain = |input: i64| json!({"role":"assistant","metadata":{"generation_model":"priced","metrics":{"input_tokens":input,"output_tokens":0}}}).to_string();
        let mut sql: Vec<String> = SCHEMA.iter().map(|s| s.to_string()).collect();
        sql.push("INSERT INTO sessions VALUES ('s1','/Users/me/Code/Pulse','Fix the ring')".into());
        sql.push(format!("INSERT INTO message_nodes VALUES ('s1','{}',{today})", plain(100)));
        sql.push(format!("INSERT INTO message_nodes VALUES ('s1','{}',{yesterday})", plain(900)));
        database(&store(home.path()), &sql.iter().map(String::as_str).collect::<Vec<_>>());
        let cache = tempfile::tempdir().unwrap();
        let sources = Sources::new(home.path());

        let unpriced = read_agent(SpendAgent::DevinCli, &sources, cache.path(), &calendar, &PriceTable::new());
        let session = &unpriced.sessions[0];
        assert_eq!(session.slots.len(), 2);
        assert_eq!(session.slots.iter().map(|s| s.unpriced_tokens).sum::<i64>(), session.unpriced_tokens);
        let summary = SpendSummary::of(&std::collections::HashMap::from([(SpendAgent::DevinCli, unpriced)]), Some(1), now, &calendar);
        assert_eq!(summary.sessions[0].session.unpriced_tokens, 100);
        assert_eq!(summary.sessions[0].session.estimated_cost(), None);
        assert_eq!(summary.projects[0].unpriced_tokens, 100);

        let prices: PriceTable = std::collections::HashMap::from([("priced".to_string(), ModelPrice::new(1_000.0, 10_000.0, Some(100.0), Some(1_000.0), Some("Priced")))]);
        let ledger = read_agent(SpendAgent::DevinCli, &sources, cache.path(), &calendar, &prices);
        let session = &ledger.sessions[0];
        assert_eq!(session.slots.iter().map(|s| s.tokens).sum::<i64>(), session.tokens);
        assert_eq!(ledger.days.iter().map(|d| d.tokens).sum::<i64>(), session.tokens);
        let summary = SpendSummary::of(&std::collections::HashMap::from([(SpendAgent::DevinCli, ledger)]), Some(1), now, &calendar);
        assert_eq!(summary.tokens, 100);
        assert!((summary.cost - 0.1).abs() < 1e-9);
        assert_eq!(summary.projects[0].cost, summary.cost);
        assert_eq!(summary.sessions[0].session.tokens, 100);
    }

    #[test]
    fn a_store_that_is_not_there_is_not_an_empty_account() {
        let home = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        assert!(read_agent(SpendAgent::DevinCli, &Sources::new(home.path()), cache.path(), &Calendar::utc(2), &PriceTable::new()).days.is_empty());
        assert!(!SpendAgent::DevinCli.is_present(&Sources::new(home.path())));
    }
}
