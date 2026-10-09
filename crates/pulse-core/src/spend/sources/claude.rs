// Ported from upstream Sources/Pulse/Usage/SpendAgent.swift (the Claude Code case).
//! Claude Code: `%USERPROFILE%\.claude\projects\**\*.jsonl`, or `CLAUDE_CONFIG_DIR\projects`.
//!
//! Read as transcripts, line by line, with the per-file cache in `spend::transcripts`; see
//! [`SpendSource::transcript_provider`].

use std::path::PathBuf;

use super::SpendSource;
use crate::provider::Provider;
use crate::spend::transcripts::Sources;

pub struct ClaudeCode;

impl SpendSource for ClaudeCode {
    fn raw(&self) -> &'static str {
        "claudeCode"
    }
    fn source_id(&self) -> &'static str {
        "claude"
    }
    fn display_name(&self) -> &'static str {
        "Claude Code"
    }
    fn has_captured_validation(&self) -> bool {
        true
    }
    fn transcript_provider(&self) -> Option<Provider> {
        Some(Provider::ClaudeCode)
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        sources.roots(Provider::ClaudeCode)
    }
}
