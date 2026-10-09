// Ported from upstream Sources/Pulse/Usage/Readers/FreebuffUsageReader.swift.
//! Freebuff, which shares Codebuff's `manicode*` trees but persists no usage.
//!
//! A Freebuff chat is one whose `metadata.runState.sessionState.mainAgentState.agentType` starts
//! with `base2-free`; the Codebuff agent types (`base2`, `base2-lite`, `base2-max`, `base2-plan`)
//! are the other product. `codebuff.rs` reads the same trees and claims any chat that carries
//! authoritative usage, so this reader never does.
//!
//! **Nothing is emitted, and that is the whole reader.** The only figures Freebuff leaves are
//! estimates from message character counts (roughly four characters to a token). Pulse does not
//! show an inferred count as if it were reported, and a "0 tokens" from a store that persisted none
//! would be a fabricated zero. So this reader answers with no records. Upstream also marks the
//! agent as not reporting token counts; this port has no such switch, so the pane shows Freebuff
//! with no tokens rather than an estimate.
//!
//! Where it lives on Windows: `FREEBUFF_DATA_DIR` when set, else the Codebuff trees under
//! `%USERPROFILE%\.config`. WINDOWS-PATH: unverified.

use std::path::PathBuf;

use super::{push_unique, SpendSource};
use crate::spend::transcripts::Sources;

pub struct Freebuff;

impl SpendSource for Freebuff {
    fn raw(&self) -> &'static str {
        "freebuff"
    }
    fn source_id(&self) -> &'static str {
        "freebuff"
    }
    fn display_name(&self) -> &'static str {
        "Freebuff"
    }
    fn env_vars(&self) -> &'static [&'static str] {
        &["FREEBUFF_DATA_DIR"]
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        if let Some(dir) = sources.var("FREEBUFF_DATA_DIR") {
            return vec![dir];
        }
        let mut roots = Vec::new();
        // WINDOWS-PATH: unverified (the macOS layout carried over)
        for product in ["manicode", "manicode-dev", "manicode-staging"] {
            push_unique(&mut roots, sources.home.join(".config").join(product));
        }
        roots
    }
    // `records` is the trait default, empty: the store has no reported counters to hand over.
}

/// Whether a chat belongs to Freebuff by its agent type.
#[cfg(test)]
fn is_freebuff(chat: &serde_json::Value) -> bool {
    chat.get("metadata")
        .and_then(|m| m.get("runState"))
        .and_then(|r| r.get("sessionState"))
        .and_then(|s| s.get("mainAgentState"))
        .and_then(|a| a.get("agentType"))
        .and_then(serde_json::Value::as_str)
        .is_some_and(|agent| agent.trim().to_lowercase().starts_with("base2-free"))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::spend::calendar::Calendar;
    use crate::spend::prices::PriceTable;
    use crate::spend::sources::{read_agent, SpendAgent};

    #[test]
    fn a_free_chat_with_counts_on_disk_still_yields_no_record() {
        let home = tempfile::tempdir().unwrap();
        let folder = home.path().join(".config").join("manicode").join("projects").join("Pulse").join("chats").join("2026-09-14T10-20-30.000Z");
        std::fs::create_dir_all(&folder).unwrap();
        let chat = json!([{"id": "m", "variant": "ai", "timestamp": "2026-09-14T10:21:00Z",
            "metadata": {"model": "x", "usage": {"inputTokens": 500}, "runState": {"sessionState": {"mainAgentState": {"agentType": "base2-free-evals"}}}}}]);
        std::fs::write(folder.join("chat-messages.json"), chat.to_string()).unwrap();
        assert!(is_freebuff(&chat[0]));
        assert!(Freebuff.records(&Freebuff.inputs(&Sources::new(home.path()))).is_empty());

        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::Freebuff, &Sources::new(home.path()), cache.path(), &Calendar::utc(2), &PriceTable::new());
        assert!(ledger.days.is_empty());
    }

    #[test]
    fn the_override_moves_the_store_and_is_the_only_root_named() {
        let home = tempfile::tempdir().unwrap();
        let sources = Sources::new(home.path()).with_var("FREEBUFF_DATA_DIR", "/elsewhere/freebuff");
        assert_eq!(Freebuff.inputs(&sources), vec![PathBuf::from("/elsewhere/freebuff")]);
        assert_eq!(Freebuff.inputs(&Sources::new(home.path())).len(), 3);
    }
}
