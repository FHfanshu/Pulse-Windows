// Ported from upstream Sources/Pulse/Usage/Readers/AntigravityCLIReader.swift (the `antigravity-ide` client).
//! Antigravity IDE's conversations. The IDE writes the same one-SQLite-per-conversation store as the CLI
//! (`antigravity_cli.rs`), into `antigravity/conversations` (the current layout) and
//! `antigravity-ide/conversations` (the other spelling seen beside it), under the same Gemini root.
//!
//! The two clients are never added together: each folder belongs to a different product, so each is its
//! own source. A root that does not exist yields no records.
//!
//! Where it lives on Windows: the same `%GEMINI_CLI_HOME%` or `%USERPROFILE%\.gemini` root, with the IDE
//! folders under it. Unverified on a PC.

use std::path::PathBuf;

use super::antigravity_cli::{conversation_roots, records_in};
use super::SpendSource;
use crate::provider::Provider;
use crate::spend::record::AgentUsageRecord;
use crate::spend::transcripts::Sources;

pub struct AntigravityIde;

impl SpendSource for AntigravityIde {
    fn raw(&self) -> &'static str {
        "antigravityIDE"
    }
    fn source_id(&self) -> &'static str {
        "antigravity-ide"
    }
    fn display_name(&self) -> &'static str {
        "Antigravity IDE"
    }
    fn card_provider(&self) -> Option<Provider> {
        Some(Provider::Antigravity)
    }
    fn icon(&self) -> Option<&'static str> {
        Some("antigravity")
    }
    fn env_vars(&self) -> &'static [&'static str] {
        &["GEMINI_CLI_HOME"]
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        // WINDOWS-PATH: unverified (both IDE folder spellings)
        conversation_roots(sources, &["antigravity", "antigravity-ide"])
    }
    fn records(&self, roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        records_in(roots)
    }
}
