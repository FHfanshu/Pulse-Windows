// Ported from upstream Sources/Pulse/Usage/Readers/VSCodeTaskLogReader.swift (client "kilocode").
//! Kilo Code's VS Code task logs: the same task shape as Roo Code (`roocode.rs`), under the
//! `kilocode.kilo-code` extension id. Its roots and reader are shared with that file.
//!
//! Kilo Code's CLI is `kilo.rs`, which reads a different store; this source is the editor
//! extension only.

use std::path::PathBuf;

use super::roocode::{task_records, task_roots};
use super::SpendSource;
use crate::spend::record::AgentUsageRecord;
use crate::spend::transcripts::Sources;

pub struct KiloCode;

impl SpendSource for KiloCode {
    fn raw(&self) -> &'static str {
        "kiloCode"
    }
    fn source_id(&self) -> &'static str {
        "kilocode"
    }
    fn display_name(&self) -> &'static str {
        "Kilo Code"
    }
    fn icon(&self) -> Option<&'static str> {
        Some("kilocode")
    }
    fn price_vendor(&self) -> Option<&'static str> {
        Some("kilo")
    }
    fn env_vars(&self) -> &'static [&'static str] {
        &["APPDATA"]
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        task_roots("kilocode.kilo-code", sources)
    }
    fn records(&self, roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        task_records(roots)
    }
}
