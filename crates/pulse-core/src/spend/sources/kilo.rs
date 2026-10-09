// Ported from upstream Sources/Pulse/Usage/OpenCodeStore.swift (Kilo CLI is read by the same code).
//! Kilo CLI: `~/.local/share/kilo/kilo.db`, a fork of OpenCode down to the database schema, which
//! is why `opencode.rs` reads it. Its channel databases (`kilo-<channel>.db`) and legacy JSON
//! messages are read the same way. A message with no timestamp of its own is skipped: the
//! database's modification date is not evidence of when it was written.
//!
//! No mark of its own here, and no provider card: Kilo's plan is not one Pulse watches.

use std::path::PathBuf;

use super::opencode::{store_inputs, store_records};
use super::SpendSource;
use crate::spend::record::AgentUsageRecord;
use crate::spend::transcripts::Sources;

pub struct KiloCli;

impl SpendSource for KiloCli {
    fn raw(&self) -> &'static str {
        "kiloCLI"
    }
    fn source_id(&self) -> &'static str {
        "kilo"
    }
    fn display_name(&self) -> &'static str {
        "Kilo CLI"
    }
    fn icon(&self) -> Option<&'static str> {
        Some("kilocode")
    }
    fn price_vendor(&self) -> Option<&'static str> {
        Some("kilo")
    }
    fn env_vars(&self) -> &'static [&'static str] {
        &["XDG_DATA_HOME", "APPDATA", "LOCALAPPDATA"]
    }
    fn has_captured_validation(&self) -> bool {
        true
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        store_inputs(sources, "kilo", None)
    }
    fn records(&self, roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        store_records(roots)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::opencode::tests::{priced, CREATED};
    use crate::spend::calendar::Calendar;
    use crate::spend::sources::{read_agent, SpendAgent};
    use crate::spend::sqlite::tests::database;
    use crate::spend::tally::TokenTally;
    use crate::spend::transcripts::Sources;

    #[test]
    fn kilo_is_read_from_its_own_folder_by_the_same_schema() {
        let home = tempfile::tempdir().unwrap();
        let folder = home.path().join(".local").join("share").join("kilo");
        std::fs::create_dir_all(&folder).unwrap();
        let reply = json!({"role":"assistant","modelID":"priced","time":{"created":CREATED},"tokens":{"input":40,"output":2,"cache":{"read":8,"write":0}}});
        // No time of its own: counted nowhere.
        let undated = json!({"role":"assistant","modelID":"priced","tokens":{"input":999,"output":0}});
        database(
            &folder.join("kilo.db"),
            &[
                "CREATE TABLE session (id TEXT, project_id TEXT, slug TEXT, directory TEXT, title TEXT)",
                "CREATE TABLE message (id TEXT, session_id TEXT, time_created INTEGER, data TEXT)",
                "INSERT INTO session VALUES ('s1','p','kilo-slug','C:\\Users\\me\\code\\Pulse','Kilo work')",
                &format!("INSERT INTO message VALUES ('m1','s1',1,'{reply}')"),
                &format!("INSERT INTO message VALUES ('m2','s1',2,'{undated}')"),
            ],
        );
        // OpenCode's folder is a different store and is not mistaken for Kilo's.
        assert!(!SpendAgent::OpenCode.is_present(&Sources::new(home.path())));
        assert!(SpendAgent::KiloCli.is_present(&Sources::new(home.path())));

        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::KiloCli, &Sources::new(home.path()), cache.path(), &Calendar::utc(2), &priced());
        let tally: TokenTally = ledger.days.iter().map(|d| d.tally.clone()).sum();
        assert_eq!(tally, TokenTally::new(40, 0, 8, 2));
        // A Windows working directory is named by its last folder.
        assert_eq!(ledger.sessions[0].project.as_ref().unwrap().name, "Pulse");
        assert_eq!(ledger.sessions[0].title.as_deref(), Some("Kilo work"));
    }
}
