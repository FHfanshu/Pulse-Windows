// Ported from upstream Sources/Pulse/Usage/Readers/CapturedWarpReader.swift.
//! Warp's synced account snapshot, `warp-cache/usage*.json`.
//!
//! **This reader returns no records, ever, and that is the point.** A Warp snapshot carries a
//! request count and money (`requestsUsed`, `spendCents`) and **no tokens of any kind**. A request
//! is not a token, and dividing the spend by a price would put a number on the page that nobody
//! measured. The source is still named here so the pane can say that Warp reports requests and
//! cost but no tokens, rather than showing a silent zero.
//!
//! Where it lives on Windows: `TOKSCALE_CONFIG_DIR` when set, else `%USERPROFILE%\.config\tokscale`
//! (the upstream `~/.config/tokscale` tree). `WINDOWS-PATH: unverified`. Pulse's own drop folder is
//! `%APPDATA%\Pulse\UsageImports\warp`.

use std::path::PathBuf;

use super::{push_unique, SpendSource};
use crate::provider::Provider;
use crate::spend::record::AgentUsageRecord;
use crate::spend::transcripts::Sources;

pub struct Warp;

impl SpendSource for Warp {
    fn raw(&self) -> &'static str {
        "warp"
    }
    fn source_id(&self) -> &'static str {
        "warp"
    }
    fn display_name(&self) -> &'static str {
        "Warp"
    }
    fn card_provider(&self) -> Option<Provider> {
        Some(Provider::Warp)
    }
    fn env_vars(&self) -> &'static [&'static str] {
        &["TOKSCALE_CONFIG_DIR", "APPDATA"]
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        let mut roots = Vec::new();
        // WINDOWS-PATH: unverified
        let cache = sources.var("TOKSCALE_CONFIG_DIR").unwrap_or_else(|| sources.home.join(".config").join("tokscale"));
        push_unique(&mut roots, cache.join("warp-cache"));
        push_unique(&mut roots, sources.app_data().join("Pulse").join("UsageImports").join("warp"));
        roots
    }
    fn records(&self, _roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        // A snapshot with no token fields: nothing to count, and nothing to invent.
        Vec::new()
    }
    fn requires_usage_export(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::spend::calendar::Calendar;
    use crate::spend::prices::PriceTable;
    use crate::spend::sources::{read_agent, SpendAgent};

    #[test]
    fn a_warp_snapshot_is_named_but_never_counted() {
        let home = tempfile::tempdir().unwrap();
        let folder = home.path().join(".config").join("tokscale").join("warp-cache");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("usage.json"), r#"{"requestsUsed":12,"spendCents":340}"#).unwrap();
        let cache = tempfile::tempdir().unwrap();
        let prices: PriceTable = HashMap::new();
        let ledger = read_agent(SpendAgent::Warp, &Sources::new(home.path()), cache.path(), &Calendar::utc(2), &prices);
        assert!(ledger.days.iter().all(|d| d.tokens == 0));
        assert_eq!(SpendAgent::Warp.source().inputs(&Sources::new(home.path())).len(), 2);
    }
}
