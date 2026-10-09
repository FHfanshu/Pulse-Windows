// Ported from upstream Sources/Pulse/Usage/SpendAgent.swift (the Claude Code and Codex cases).
//! A coding agent that leaves a record of its work on this machine.
//!
//! Not a `Provider`, deliberately: a provider is something Pulse can draw a ring for; an agent is
//! something that has spent tokens here. The two lists overlap and are not the same.
//!
//! TODO(spend): upstream's catalogue has 54 sources. Only Claude Code and Codex are ported;
//! the rest are not read yet. Still to do, by upstream group:
//! - the other machine-verified readers: OpenCode, Kilo CLI, Grok Build, Kimi CLI, Devin CLI
//! - A (session logs): Pi, Oh My Pi, OmO Native, Kimchi, Prime Agent, Gemini CLI, Qwen Code, Amp,
//!   Droid, OpenClaw
//! - B (editor logs): Roo Code, Kilo Code, Cline, CodeBuddy, WorkBuddy, Cherry Studio,
//!   Command Code, OpenCodeReview, ZCode
//! - C (databases): Hermes, Goose, Zed, Kiro, Crush, Unsloth, Antigravity CLI/IDE, MiMo Code,
//!   Devin Desktop
//! - D (structured logs): Mux, Codebuff, Freebuff, JCode, Augment, Gajae Code, Junie, DeepSeek
//!   Harness, FX, LM Studio, Reasonix
//! - E (exports): Cursor, Antigravity (export), Trae, Warp, Hindsight, MiniMax Code
//! - F: GitHub Copilot
//!
//! Those need the whole-ledger cache (`AgentCache`) and the record readers
//! (`AgentRecordReaders`); `record::build_ledger` is already the shared builder for them.

use serde::{Deserialize, Serialize};

use crate::provider::Provider;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SpendAgent {
    ClaudeCode,
    Codex,
}

impl SpendAgent {
    pub const ALL: [SpendAgent; 2] = [SpendAgent::ClaudeCode, SpendAgent::Codex];

    /// The canonical id the reader families dispatch on.
    pub fn source_id(self) -> &'static str {
        match self {
            SpendAgent::ClaudeCode => "claude",
            SpendAgent::Codex => "codex",
        }
    }

    /// Product names, left untranslated.
    pub fn display_name(self) -> &'static str {
        match self {
            SpendAgent::ClaudeCode => "Claude Code",
            SpendAgent::Codex => "Codex",
        }
    }

    /// The provider whose transcript reader serves this agent.
    pub fn provider(self) -> Provider {
        match self {
            SpendAgent::ClaudeCode => Provider::ClaudeCode,
            SpendAgent::Codex => Provider::Codex,
        }
    }

    pub fn from_provider(provider: Provider) -> Option<SpendAgent> {
        match provider {
            Provider::ClaudeCode => Some(SpendAgent::ClaudeCode),
            Provider::Codex => Some(SpendAgent::Codex),
            _ => None,
        }
    }

    /// The models.dev plan vendor an agent's models fall back to when no first-party provider
    /// publishes them. Neither agent calls a reseller, so neither has one.
    pub fn price_vendor(self) -> Option<&'static str> {
        None
    }

    /// Whether this agent has left a folder of records on this machine: installed, or at least
    /// used once. An agent that is absent entirely is not named anywhere in the pane.
    pub fn is_present(self, sources: &super::transcripts::Sources) -> bool {
        sources.roots(self.provider()).iter().any(|root| root.is_dir())
    }

    /// Whether the store records the prompt cache at all.
    pub fn reports_cache_reads(self) -> bool {
        true
    }

    /// Whether the store is an export or capture rather than a native log.
    pub fn requires_usage_export(self) -> bool {
        false
    }

    /// Read off a real machine with real work in it upstream.
    pub fn has_captured_validation(self) -> bool {
        true
    }
}
