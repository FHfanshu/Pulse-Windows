// Ported from upstream Sources/Pulse/Usage/Readers/PiFamilySessionReader.swift (configuration "senpi")
// and Sources/Pulse/Usage/Readers/SessionLogPaths.swift (senpi).
//! OmO Native (Senpi): the Pi transcript format, plus its OmO task children.
//!
//! `~/.senpi/agent/sessions/**/*.jsonl`, read with the shared Pi parser in [`super::pi`]. Two
//! differences from Pi: `session_info.name` is the conversation's human title, and the OmO task
//! children live outside the sessions tree, under `<cwd>/.omo/senpi-task/children` for every
//! working directory a session header in the sessions tree names. Those folders are resolved in
//! `inputs`, so the cache watches exactly what is parsed.
//!
//! Overrides: `SENPI_CODING_AGENT_DIR` moves the base folder (its `sessions` child is read), and
//! `SENPI_CODING_AGENT_SESSION_DIR` adds a second sessions folder.
//!
//! Where it lives on Windows: `%USERPROFILE%\.senpi\agent\sessions`. Unverified on a real PC.

use std::path::PathBuf;

use super::pi::{family_records, push_unique_path, senpi_children, Family};
use super::SpendSource;
use crate::spend::record::AgentUsageRecord;
use crate::spend::transcripts::Sources;

pub struct Senpi;

impl SpendSource for Senpi {
    fn raw(&self) -> &'static str {
        "senpi"
    }
    fn source_id(&self) -> &'static str {
        "senpi"
    }
    fn display_name(&self) -> &'static str {
        "OmO Native"
    }
    fn env_vars(&self) -> &'static [&'static str] {
        &["SENPI_CODING_AGENT_DIR", "SENPI_CODING_AGENT_SESSION_DIR"]
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        // WINDOWS-PATH: unverified (`%USERPROFILE%\.senpi` is the macOS layout carried over)
        let base = sources.var("SENPI_CODING_AGENT_DIR").unwrap_or_else(|| sources.home.join(".senpi").join("agent"));
        let mut roots = Vec::new();
        push_unique_path(&mut roots, base.join("sessions"));
        if let Some(state) = sources.var("SENPI_CODING_AGENT_SESSION_DIR") {
            push_unique_path(&mut roots, state);
        }
        for child in senpi_children(&roots) {
            push_unique_path(&mut roots, child);
        }
        roots
    }
    fn records(&self, roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        family_records(&Family::SENPI, roots)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::spend::calendar::Calendar;
    use crate::spend::prices::{ModelPrice, PriceTable};
    use crate::spend::sources::pi::tests::{pi_file, write};
    use crate::spend::sources::{read_agent, SpendAgent};
    use crate::spend::transcripts::Sources;

    fn prices() -> PriceTable {
        HashMap::from([("gpt-5".to_string(), ModelPrice::new(1.0, 10.0, Some(0.1), Some(1.0), Some("GPT-5")))])
    }

    #[test]
    fn senpi_reads_the_omo_task_children_named_by_a_session_header() {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        // A JSON string: forward slashes keep a Windows path from breaking the escape rules.
        let cwd = project.path().to_string_lossy().replace('\\', "/");
        write(
            &home.path().join(".senpi/agent/sessions/parent.jsonl"),
            &pi_file("parent", &cwd, "p1", None, "gpt-5", [10, 1, 0, 0]),
        );
        write(
            &project.path().join(".omo/senpi-task/children/child.jsonl"),
            &pi_file("child", &cwd, "c1", Some("child-1"), "gpt-5", [20, 2, 0, 0]),
        );

        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::Senpi, &Sources::new(home.path()), cache.path(), &Calendar::utc(2), &prices());
        assert_eq!(ledger.days.iter().map(|d| d.tokens).sum::<i64>(), 33);
        assert_eq!(ledger.sessions.len(), 2);
    }

    #[test]
    fn a_senpi_base_override_moves_the_store() {
        let home = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        write(&home.path().join(".senpi/agent/sessions/a.jsonl"), &pi_file("s", "/w", "m1", None, "gpt-5", [100, 0, 0, 0]));
        let sources = Sources::new(home.path()).with_var("SENPI_CODING_AGENT_DIR", elsewhere.path());
        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::Senpi, &sources, cache.path(), &Calendar::utc(2), &prices());
        assert!(ledger.days.is_empty());
    }
}
