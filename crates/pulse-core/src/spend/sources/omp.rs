// Ported from upstream Sources/Pulse/Usage/Readers/PiFamilySessionReader.swift (configuration "omp").
//! Oh My Pi: the Pi transcript format, under its own root.
//!
//! `~/.omp/agent/sessions/**/*.jsonl`, read with the shared Pi parser in [`super::pi`]. Its
//! records fall back to the provider `omp` when a message names none, and fold across sessions
//! like Pi's. `PI_CODING_AGENT_DIR` is deliberately not honoured: Pi reads it too, and watching one
//! tree twice would count it twice.
//!
//! Where it lives on Windows: `%USERPROFILE%\.omp\agent\sessions`. Unverified on a real PC.

use std::path::PathBuf;

use super::pi::{family_records, Family};
use super::SpendSource;
use crate::spend::record::AgentUsageRecord;
use crate::spend::transcripts::Sources;

pub struct Omp;

impl SpendSource for Omp {
    fn raw(&self) -> &'static str {
        "omp"
    }
    fn source_id(&self) -> &'static str {
        "omp"
    }
    fn display_name(&self) -> &'static str {
        "Oh My Pi"
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        // WINDOWS-PATH: unverified (`%USERPROFILE%\.omp` is the macOS layout carried over)
        vec![sources.home.join(".omp").join("agent").join("sessions")]
    }
    fn records(&self, roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        family_records(&Family::OMP, roots)
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
    fn omp_reads_its_own_root_with_the_pi_format_and_a_missing_store_is_empty() {
        let home = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let empty = read_agent(SpendAgent::Omp, &Sources::new(home.path()), cache.path(), &Calendar::utc(2), &prices());
        assert!(empty.days.is_empty());

        write(&home.path().join(".omp/agent/sessions/cwd/session.jsonl"), &pi_file("s", "/w", "m1", None, "gpt-5", [100, 40, 30, 20]));
        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::Omp, &Sources::new(home.path()), cache.path(), &Calendar::utc(2), &prices());
        assert_eq!(ledger.days.iter().map(|d| d.tokens).sum::<i64>(), 190);
    }
}
