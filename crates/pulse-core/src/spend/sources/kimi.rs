// Ported from upstream Sources/Pulse/Usage/KimiCLIStore.swift.
//! Kimi's CLI, which writes the raw exchange to `wire.jsonl`.
//!
//! ```json
//! { "timestamp": 1789372800,
//!   "message": { "payload": { "token_usage": {
//!     "input_other": 4340, "output": 38,
//!     "input_cache_read": 9216, "input_cache_creation": 0 } } } }
//! ```
//!
//! **`input_other` is fresh input**, as the name says: the cache figures are counted beside it
//! rather than inside it, which is the same arrangement Claude Code uses and the opposite of
//! Codex's.
//!
//! **It names no model, anywhere.** Neither the wire log nor the session state carries one, so
//! its tokens are counted and never costed, the same answer Pulse gives for any model with no
//! published price, arrived at one step earlier. The id below is a placeholder so the buckets
//! have a key, and is deliberately one no price list can match.
//!
//! Where it lives on Windows: `KIMI_SHARE_DIR` when set, else `%USERPROFILE%\.kimi`, then
//! `sessions\**\wire.jsonl`. Kimi Code (`KIMI_CODE_HOME`, else `%USERPROFILE%\.kimi-code`) is
//! looked at the same way. Not read (upstream lists them as further work): Kimi Work's desktop
//! protocol and the model named in `config.json`.

use std::path::PathBuf;

use super::{int, push_unique, SpendSource};
use crate::provider::Provider;
use crate::spend::logio;
use crate::spend::record::AgentUsageRecord;
use crate::spend::tally::TokenTally;
use crate::spend::titles::title_from;
use crate::spend::transcripts::Sources;

/// The placeholder model: no price list can match it.
const MODEL: &str = "kimi (unnamed)";

pub struct KimiCli;

impl SpendSource for KimiCli {
    fn raw(&self) -> &'static str {
        "kimiCLI"
    }
    fn source_id(&self) -> &'static str {
        "kimi"
    }
    fn display_name(&self) -> &'static str {
        "Kimi CLI"
    }
    fn card_provider(&self) -> Option<Provider> {
        Some(Provider::KimiCode)
    }
    fn icon(&self) -> Option<&'static str> {
        Some("kimi")
    }
    fn env_vars(&self) -> &'static [&'static str] {
        &["KIMI_SHARE_DIR", "KIMI_CODE_HOME"]
    }
    fn has_captured_validation(&self) -> bool {
        true
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        let mut roots = Vec::new();
        // WINDOWS-PATH: unverified (`KIMI_SHARE_DIR` is the CLI's own override; the default is
        // the macOS layout carried over)
        push_unique(&mut roots, sources.var("KIMI_SHARE_DIR").unwrap_or_else(|| sources.home.join(".kimi")).join("sessions"));
        // WINDOWS-PATH: unverified
        push_unique(&mut roots, sources.var("KIMI_CODE_HOME").unwrap_or_else(|| sources.home.join(".kimi-code")).join("sessions"));
        roots
    }
    fn records(&self, roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        let mut records = Vec::new();
        for file in logio::files(roots, &[], &["wire.jsonl"], &[]) {
            let folder = file.parent().map(PathBuf::from).unwrap_or_default();
            let id = file.to_string_lossy().into_owned();
            let name = folder.file_name().map(|n| n.to_string_lossy().into_owned());
            // The session's own state file keeps the title beside the wire log. Neither carries a
            // working directory.
            let title = title_beside(&folder);
            for root in logio::json_lines(&file) {
                let Some(usage) = root.get("message").and_then(|m| m.get("payload")).and_then(|p| p.get("token_usage")).filter(|u| u.is_object())
                else {
                    continue;
                };
                let tally = TokenTally {
                    input: int(usage.get("input_other")),
                    cache_write: int(usage.get("input_cache_creation")),
                    cache_read: int(usage.get("input_cache_read")),
                    output: int(usage.get("output")),
                    ..TokenTally::default()
                };
                if tally.total() <= 0 {
                    continue;
                }
                let seconds = match root.get("timestamp") {
                    Some(serde_json::Value::Number(n)) => n.as_f64().unwrap_or(0.0),
                    _ => 0.0,
                };
                if seconds <= 0.0 {
                    continue;
                }
                let Some(at) = logio::timestamp(Some(&serde_json::json!(seconds)), seconds > 10_000_000_000.0) else { continue };
                let mut record = AgentUsageRecord::new(at, MODEL, tally).session(&id);
                record.session_name = name.clone();
                record.title = title.clone();
                records.push(record);
            }
        }
        records
    }
}

fn title_beside(folder: &std::path::Path) -> Option<String> {
    let state = logio::json(&folder.join("state.json"))?;
    title_from(state.get("custom_title")?.as_str()?)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use chrono::{Duration, Utc};
    use serde_json::json;

    use super::*;
    use crate::spend::calendar::Calendar;
    use crate::spend::prices::{ModelPrice, PriceTable};
    use crate::spend::sources::{read_agent, SpendAgent};
    use crate::spend::summary::SpendSummary;

    fn session_folder(home: &std::path::Path) -> PathBuf {
        let folder = home.join(".kimi").join("sessions").join("hash").join("session");
        std::fs::create_dir_all(&folder).unwrap();
        folder
    }

    fn priced() -> PriceTable {
        HashMap::from([("priced".to_string(), ModelPrice::new(1.0, 10.0, Some(0.1), Some(1.0), Some("Priced")))])
    }

    #[test]
    fn input_other_is_fresh_input_counted_beside_the_cache() {
        let home = tempfile::tempdir().unwrap();
        let folder = session_folder(home.path());
        let line = json!({"timestamp":1_789_372_800,"message":{"payload":{"model":"priced","token_usage":{
            "input_other":100,"output":15,"input_cache_read":300,"input_cache_creation":20}}}});
        std::fs::write(folder.join("wire.jsonl"), line.to_string()).unwrap();
        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::KimiCli, &Sources::new(home.path()), cache.path(), &Calendar::utc(2), &priced());
        let day = ledger.days.iter().find(|d| d.tokens > 0).unwrap();
        assert_eq!(day.tally, TokenTally::new(100, 20, 300, 15));
    }

    #[test]
    fn an_unnamed_model_is_counted_not_costed_and_the_state_file_names_the_session() {
        let home = tempfile::tempdir().unwrap();
        let folder = session_folder(home.path());
        let calendar = Calendar::local();
        let now = Utc::now();
        let today = (calendar.start_of_day(now) + Duration::hours(12)).timestamp();
        let yesterday = (calendar.add_days(calendar.start_of_day(now), -1) + Duration::hours(12)).timestamp();
        let turn = |at: i64, input: i64| json!({"timestamp":at,"message":{"payload":{"token_usage":{"input_other":input,"output":0,"input_cache_read":0,"input_cache_creation":0}}}});
        // A millisecond timestamp is told from seconds, and a line with none, or none counted, is skipped.
        let lines = [
            turn(today, 100),
            turn(yesterday * 1000, 900),
            json!({"message":{"payload":{"token_usage":{"input_other":5}}}}),
            json!({"timestamp":today,"message":{"payload":{"token_usage":{"input_other":0,"output":0}}}}),
            json!({"timestamp":today,"message":{"payload":{}}}),
        ];
        let text: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
        std::fs::write(folder.join("wire.jsonl"), text.join("\n")).unwrap();
        std::fs::write(folder.join("state.json"), json!({"custom_title":"Fix the ring"}).to_string()).unwrap();

        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::KimiCli, &Sources::new(home.path()), cache.path(), &calendar, &priced());
        let session = &ledger.sessions[0];
        assert_eq!(session.slots.len(), 2);
        assert_eq!(session.title.as_deref(), Some("Fix the ring"));
        assert_eq!(session.name, "session");
        // Kimi names no model, so nothing is priceable and the cost is exactly zero.
        assert_eq!(session.cost, 0.0);
        assert_eq!(session.unpriced_tokens, 1_000);
        assert_eq!(session.estimated_cost(), None);
        assert_eq!(ledger.days.iter().map(|d| d.tokens).sum::<i64>(), session.tokens);

        let summary = SpendSummary::of(&HashMap::from([(SpendAgent::KimiCli, ledger)]), Some(1), now, &calendar);
        assert_eq!(summary.tokens, 100);
        // No working directory is stated, so it is in the total and not on the project list.
        assert!(summary.projects.is_empty());
        assert_eq!(summary.sessions[0].session.unpriced_tokens, 100);
        assert_eq!(summary.sessions[0].session.estimated_cost(), None);
    }

    #[test]
    fn the_share_directory_override_moves_the_store_and_a_store_that_is_not_there_is_absent() {
        let home = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let sources = Sources::new(home.path()).with_var("KIMI_SHARE_DIR", elsewhere.path());
        assert_eq!(SpendAgent::KimiCli.inputs(&sources)[0], elsewhere.path().join("sessions"));
        let cache = tempfile::tempdir().unwrap();
        assert!(read_agent(SpendAgent::KimiCli, &Sources::new(home.path()), cache.path(), &Calendar::utc(2), &PriceTable::new()).days.is_empty());
    }
}
