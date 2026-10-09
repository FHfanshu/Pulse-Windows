// Ported from upstream Sources/Pulse/Usage/Readers/PiFamilySessionReader.swift (configuration "kimchi")
// and Sources/Pulse/Usage/Readers/SessionLogPaths.swift (kimchi).
//! Kimchi: the Pi transcript format with a session-scoped dedup key.
//!
//! `~/.config/kimchi/harness/sessions/**/*.jsonl`, read with the shared Pi parser in [`super::pi`].
//! Unlike Pi, a message is identified only inside its own session: two sessions that reuse one
//! message id are two messages, and a message repeated inside one session folds to one. A message
//! with no id is not folded at all.
//!
//! Override: `KIMCHI_CODING_AGENT_DIR` replaces the base folder (its `sessions` child is read).
//!
//! Where it lives on Windows: `%USERPROFILE%\.config\kimchi\harness\sessions`. Unverified on a
//! real PC.

use std::path::PathBuf;

use super::pi::{family_records, Family};
use super::SpendSource;
use crate::spend::record::AgentUsageRecord;
use crate::spend::transcripts::Sources;

pub struct Kimchi;

impl SpendSource for Kimchi {
    fn raw(&self) -> &'static str {
        "kimchi"
    }
    fn source_id(&self) -> &'static str {
        "kimchi"
    }
    fn display_name(&self) -> &'static str {
        "Kimchi"
    }
    fn env_vars(&self) -> &'static [&'static str] {
        &["KIMCHI_CODING_AGENT_DIR"]
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        match sources.var("KIMCHI_CODING_AGENT_DIR") {
            Some(root) => vec![root.join("sessions")],
            // WINDOWS-PATH: unverified (`%USERPROFILE%\.config` is the macOS layout carried over)
            None => vec![sources.home.join(".config").join("kimchi").join("harness").join("sessions")],
        }
    }
    fn records(&self, roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        family_records(&Family::KIMCHI, roots)
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

    fn total(sources: &Sources) -> i64 {
        let cache = tempfile::tempdir().unwrap();
        read_agent(SpendAgent::Kimchi, sources, cache.path(), &Calendar::utc(2), &prices()).days.iter().map(|d| d.tokens).sum()
    }

    #[test]
    fn the_same_message_id_in_two_sessions_is_two_messages_and_one_repeated_in_a_session_is_one() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join(".config/kimchi/harness/sessions");
        write(&root.join("one.jsonl"), &pi_file("s1", "/w", "shared", None, "gpt-5", [100, 0, 0, 0]));
        write(&root.join("two.jsonl"), &pi_file("s2", "/w", "shared", None, "gpt-5", [100, 0, 0, 0]));
        assert_eq!(total(&Sources::new(home.path())), 200);

        let dup = tempfile::tempdir().unwrap();
        let line = pi_file("s3", "/w", "dup", None, "gpt-5", [100, 0, 0, 0]);
        let second = line.split('\n').nth(1).unwrap();
        write(&dup.path().join("sessions/three.jsonl"), &format!("{line}\n{second}"));
        assert_eq!(total(&Sources::new(home.path()).with_var("KIMCHI_CODING_AGENT_DIR", dup.path())), 100);
    }

    #[test]
    fn a_kimchi_override_moves_the_store_and_the_default_is_absent_when_missing() {
        let home = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        write(&elsewhere.path().join("sessions/a.jsonl"), &pi_file("s", "/w", "m1", None, "gpt-5", [100, 0, 0, 0]));
        assert_eq!(total(&Sources::new(home.path())), 0);
        let sources = Sources::new(home.path()).with_var("KIMCHI_CODING_AGENT_DIR", elsewhere.path());
        assert_eq!(total(&sources), 100);
    }
}
