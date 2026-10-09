// Ported from upstream Sources/Pulse/Usage/SpendAgent.swift (the Codex case).
//! Codex: `%USERPROFILE%\.codex\sessions` and `archived_sessions` (or `CODEX_HOME`).
//!
//! An archived session is still work done. Read as transcripts with the per-file cache in
//! `spend::transcripts`; see [`SpendSource::transcript_provider`].

use std::path::PathBuf;

use super::SpendSource;
use crate::provider::Provider;
use crate::spend::transcripts::Sources;

pub struct Codex;

impl SpendSource for Codex {
    fn raw(&self) -> &'static str {
        "codex"
    }
    fn source_id(&self) -> &'static str {
        "codex"
    }
    fn display_name(&self) -> &'static str {
        "Codex"
    }
    fn has_captured_validation(&self) -> bool {
        true
    }
    fn transcript_provider(&self) -> Option<Provider> {
        Some(Provider::Codex)
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        sources.roots(Provider::Codex)
    }
}
