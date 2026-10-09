// Ported from upstream Sources/Pulse/Usage/Readers/CapturedAntigravityReader.swift.
//! The Antigravity **IDE** cache: one JSON object per line, `antigravity-cache/sessions/*.jsonl`,
//! written by pulling usage from a running Antigravity language server.
//!
//! It is not a native local usage log. The cache is produced by an authenticated sync against the
//! language server, and Pulse reads that cache rather than talking to the server itself. It is not
//! `antigravity-cli` (the sibling source), a different product with its own conversations.
//!
//! A `session_meta` line supplies a fallback model for the `usage` lines after it; a line with no
//! model and no fallback is skipped. Placeholder ids (`model_placeholder_…`) are skipped too.
//!
//! Reasoning is not folded into output: this schema does not say whether `reasoning` is already
//! inside `output`, so `output` stays as reported, reasoning is placed nowhere, and the record is
//! marked partial.
//!
//! Where it lives on Windows: `TOKSCALE_CONFIG_DIR` when set, else `%USERPROFILE%\.config\tokscale`,
//! then `\antigravity-cache\sessions`. `WINDOWS-PATH: unverified`.

use std::path::PathBuf;

use serde_json::Value;

use super::{clamped, push_unique, SpendSource};
use crate::provider::Provider;
use crate::spend::logio;
use crate::spend::record::AgentUsageRecord;
use crate::spend::tally::TokenTally;
use crate::spend::transcripts::Sources;

pub struct Antigravity;

impl SpendSource for Antigravity {
    fn raw(&self) -> &'static str {
        "antigravity"
    }
    fn source_id(&self) -> &'static str {
        "antigravity"
    }
    fn display_name(&self) -> &'static str {
        "Antigravity (export)"
    }
    fn card_provider(&self) -> Option<Provider> {
        Some(Provider::Antigravity)
    }
    fn icon(&self) -> Option<&'static str> {
        Some("antigravity")
    }
    fn env_vars(&self) -> &'static [&'static str] {
        &["TOKSCALE_CONFIG_DIR", "APPDATA"]
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        let mut roots = Vec::new();
        // WINDOWS-PATH: unverified
        let cache = sources.var("TOKSCALE_CONFIG_DIR").unwrap_or_else(|| sources.home.join(".config").join("tokscale"));
        push_unique(&mut roots, cache.join("antigravity-cache").join("sessions"));
        push_unique(&mut roots, sources.app_data().join("Pulse").join("UsageImports").join("antigravity"));
        roots
    }
    fn records(&self, roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        let mut records = Vec::new();
        for file in logio::files(roots, &["jsonl"], &[], &[]) {
            let mut fallback_model: Option<String> = None;
            for line in logio::json_lines(&file) {
                let Some(kind) = logio::text(line.get("type")) else { continue };
                if kind == "session_meta" {
                    fallback_model = logio::text(line.get("modelId"));
                    continue;
                }
                if kind != "usage" {
                    continue;
                }
                records.extend(record_of(&line, fallback_model.as_deref()));
            }
        }
        records
    }
}

fn record_of(line: &Value, fallback_model: Option<&str>) -> Option<AgentUsageRecord> {
    let session = logio::text(line.get("sessionId"))?;
    let at = logio::timestamp(line.get("timestamp"), true).filter(|at| at.timestamp() > 0)?;
    let model = logio::text(line.get("modelId")).or_else(|| fallback_model.map(str::to_string))?;
    if model.to_lowercase().starts_with("model_placeholder_") {
        return None;
    }

    // Negative counts clamp to zero; an absent bucket is a real zero in this format.
    let tally = TokenTally::new(
        clamped(line.get("input")),
        clamped(line.get("cacheWrite")),
        clamped(line.get("cacheRead")),
        clamped(line.get("output")),
    );
    if tally.total() <= 0 {
        return None;
    }

    let mut record = AgentUsageRecord::new(at, &model, tally).session(&session);
    // A separate reasoning figure may or may not already be inside `output`: it is neither added
    // nor counted, and the record says it may be incomplete.
    record.is_partial = clamped(line.get("reasoning")) > 0;
    if let Some(response) = logio::text(line.get("responseId")) {
        // The response id is the cache's own identity for the call; a re-sync folds here.
        record.deduplication_id = Some(format!("antigravity:{response}"));
    }
    Some(record)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use chrono::Utc;
    use serde_json::json;

    use super::*;
    use crate::spend::calendar::Calendar;
    use crate::spend::prices::{ModelPrice, PriceTable};
    use crate::spend::sources::{read_agent, SpendAgent};

    fn priced() -> PriceTable {
        HashMap::from([("ag-model".to_string(), ModelPrice::new(1.0, 10.0, Some(0.1), Some(1.0), Some("Ag")))])
    }

    #[test]
    fn a_fallback_model_names_the_rows_after_it_and_reasoning_marks_the_record_partial() {
        let home = tempfile::tempdir().unwrap();
        let folder = home.path().join(".config").join("tokscale").join("antigravity-cache").join("sessions");
        std::fs::create_dir_all(&folder).unwrap();
        let ms = Utc::now().timestamp_millis();
        let lines = [
            json!({"type":"session_meta","modelId":"ag-model"}),
            json!({"type":"usage","sessionId":"s1","timestamp":ms,"input":10,"cacheWrite":2,"cacheRead":3,"output":4,"reasoning":1,"responseId":"resp-1"}),
            // A placeholder model is skipped; a row with no model takes the fallback named above it.
            json!({"type":"usage","sessionId":"s2","timestamp":ms,"modelId":"model_placeholder_m1","input":9,"output":9}),
            json!({"type":"usage","sessionId":"s3","timestamp":ms,"input":9,"output":9}),
            json!({"type":"usage","sessionId":"s4","timestamp":ms,"modelId":"ag-model","input":0,"output":0}),
        ];
        let text: Vec<String> = lines.iter().map(Value::to_string).collect();
        std::fs::write(folder.join("s1.jsonl"), text.join("\n")).unwrap();
        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::Antigravity, &Sources::new(home.path()), cache.path(), &Calendar::utc(2), &priced());
        let day = ledger.days.iter().find(|d| d.tokens > 0).unwrap();
        // s1 (19 tokens) and s3 (18, under the fallback model); the placeholder and the zero row are not counted.
        assert_eq!(day.tally, TokenTally::new(19, 2, 3, 13));
        assert_eq!(ledger.days.iter().map(|d| d.tokens).sum::<i64>(), 37);
    }
}
