// Ported from upstream Sources/Pulse/Usage/GrokStore.swift.
//! Grok Build's transcripts.
//!
//! `~/.grok/sessions/<directory>/<session id>/updates.jsonl`, where the directory is the working
//! directory **percent-encoded** (`%2FUsers%2Fme%2FCode`, on Windows `C%3A%5CUsers%5Cme%5Ccode`),
//! which is the one agent here that gives its project away in the path without losing anything.
//!
//! One line per event, and the one that counts is the end of a turn:
//!
//! ```json
//! { "timestamp": 1786775595,
//!   "params": { "update": { "sessionUpdate": "turn_completed",
//!     "usage": { "inputTokens": 33379, "outputTokens": 91,
//!                "cachedReadTokens": 22272, "cacheCreationTokens": 0,
//!                "reasoningTokens": 42,
//!                "modelUsage": { "grok-4.6-build": { ... } } } } } }
//! ```
//!
//! **`modelUsage` is what names the model**, and a turn can touch more than one, so the per-model
//! counts are read from it and the flat totals beside it are used only when it is absent.
//!
//! Where it lives on Windows: `GROK_HOME` when set, else `%USERPROFILE%\.grok`; the
//! `%APPDATA%\grok` and `%LOCALAPPDATA%\grok` spellings are also looked at.
//! Not read (upstream lists them as further work): `logs/unified.jsonl` and the sibling
//! `signals.json` reconciliation.

use std::path::PathBuf;

use serde_json::Value;

use super::{int, list, push_unique, SpendSource};
use crate::provider::Provider;
use crate::spend::logio;
use crate::spend::record::AgentUsageRecord;
use crate::spend::tally::TokenTally;
use crate::spend::titles::{text_in, title_from};
use crate::spend::transcripts::Sources;

pub struct Grok;

impl SpendSource for Grok {
    fn raw(&self) -> &'static str {
        "grok"
    }
    fn source_id(&self) -> &'static str {
        "grok"
    }
    fn display_name(&self) -> &'static str {
        "Grok Build"
    }
    fn card_provider(&self) -> Option<Provider> {
        Some(Provider::Grok)
    }
    fn icon(&self) -> Option<&'static str> {
        Some("grok")
    }
    fn env_vars(&self) -> &'static [&'static str] {
        &["GROK_HOME", "APPDATA", "LOCALAPPDATA"]
    }
    fn has_captured_validation(&self) -> bool {
        true
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        let mut roots = Vec::new();
        // WINDOWS-PATH: unverified (`%USERPROFILE%\.grok` is the macOS layout carried over)
        push_unique(&mut roots, sources.var("GROK_HOME").unwrap_or_else(|| sources.home.join(".grok")).join("sessions"));
        // WINDOWS-PATH: unverified
        push_unique(&mut roots, sources.app_data().join("grok").join("sessions"));
        // WINDOWS-PATH: unverified
        push_unique(&mut roots, sources.local_app_data().join("grok").join("sessions"));
        roots
    }
    fn records(&self, roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        let mut records = Vec::new();
        for root in roots {
            for directory in list(root).into_iter().filter(|p| p.is_dir()) {
                // The folder is the working directory, percent-encoded.
                let project = directory.file_name().and_then(|n| percent_decoded(&n.to_string_lossy()));
                for run in list(&directory).into_iter().filter(|p| p.is_dir()) {
                    records.extend(run_records(&run, project.as_deref()));
                }
            }
        }
        records
    }
}

/// One run's turns: `<run>/updates.jsonl`.
fn run_records(run: &std::path::Path, project: Option<&str>) -> Vec<AgentUsageRecord> {
    let file = run.join("updates.jsonl");
    let name = run.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let id = file.to_string_lossy().into_owned();
    let mut title: Option<String> = None;
    let mut records = Vec::new();

    for root in logio::json_lines(&file) {
        let Some(update) = root.get("params").and_then(|p| p.get("update")).filter(|u| u.is_object()) else { continue };
        let kind = update.get("sessionUpdate").and_then(Value::as_str);

        // The opening prompt, for a row that would otherwise be a uuid.
        if title.is_none() && kind == Some("user_message_chunk") {
            title = text_in(update.get("content")).and_then(|text| title_from(&text));
        }

        let (Some("turn_completed"), Some(usage)) = (kind, update.get("usage").and_then(Value::as_object)) else { continue };
        let Some(at) = logio::timestamp(root.get("timestamp").filter(|v| v.is_number()), false) else { continue };

        // `modelUsage` when it is an object of objects; otherwise the flat figures, as "grok".
        let flat = Value::Object(usage.clone());
        let per_model: Vec<(&str, &Value)> = match usage.get("modelUsage").and_then(Value::as_object) {
            Some(models) if models.values().all(Value::is_object) => {
                let mut sorted: Vec<(&str, &Value)> = models.iter().map(|(k, v)| (k.as_str(), v)).collect();
                sorted.sort_by_key(|(model, _)| *model);
                sorted
            }
            _ => vec![("grok", &flat)],
        };

        for (model, counts) in per_model {
            let tally = TokenTally {
                input: int(counts.get("inputTokens")),
                cache_write: int(counts.get("cacheCreationTokens")),
                cache_read: int(counts.get("cachedReadTokens")),
                output: int(counts.get("outputTokens")) + int(counts.get("reasoningTokens")),
                ..TokenTally::default()
            };
            if tally.total() <= 0 {
                continue;
            }
            let mut record = AgentUsageRecord::new(at, model, tally).session(&id);
            record.session_name = Some(name.clone());
            record.project = project.map(str::to_string);
            records.push(record);
        }
    }
    // The title is found on an early line and belongs to the whole run.
    for record in &mut records {
        record.title = title.clone();
    }
    records
}

/// `%2FUsers%2Fme` as `/Users/me`; None when the escapes are not valid UTF-8.
fn percent_decoded(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let pair = std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok());
            if let Some(byte) = pair {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).ok()
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

    fn priced() -> PriceTable {
        HashMap::from([("priced".to_string(), ModelPrice::new(1.0, 10.0, Some(0.1), Some(1.0), Some("Priced")))])
    }

    fn run_folder(home: &std::path::Path, encoded_directory: &str, run: &str) -> PathBuf {
        let folder = home.join(".grok").join("sessions").join(encoded_directory).join(run);
        std::fs::create_dir_all(&folder).unwrap();
        folder
    }

    fn write_lines(file: PathBuf, lines: &[Value]) {
        let text: Vec<String> = lines.iter().map(Value::to_string).collect();
        std::fs::write(file, text.join("\n")).unwrap();
    }

    #[test]
    fn a_turns_usage_is_read_per_model_and_the_folder_names_the_project() {
        let home = tempfile::tempdir().unwrap();
        // The folder is the working directory, percent-encoded.
        let run = run_folder(home.path(), "%2FUsers%2Fme%2FCode%2FPulse", "01a01492");
        write_lines(
            run.join("updates.jsonl"),
            &[
                json!({"timestamp":1_789_372_800,"params":{"update":{"sessionUpdate":"user_message_chunk","content":"Fix the ring"}}}),
                json!({"timestamp":1_789_372_800,"params":{"update":{"sessionUpdate":"turn_completed","usage":{
                    "inputTokens":999,"outputTokens":999,"modelUsage":{"priced":{"inputTokens":100,"outputTokens":10,
                    "reasoningTokens":5,"cachedReadTokens":300,"cacheCreationTokens":20}}}}}}),
            ],
        );
        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::Grok, &Sources::new(home.path()), cache.path(), &Calendar::utc(2), &priced());
        let day = ledger.days.iter().find(|d| d.tokens > 0).unwrap();
        // `modelUsage` wins over the flat totals beside it: reading both would count the turn twice.
        assert_eq!(day.tally, TokenTally::new(100, 20, 300, 15));
        let session = &ledger.sessions[0];
        assert_eq!(session.project.as_ref().unwrap().name, "Pulse");
        assert_eq!(session.title.as_deref(), Some("Fix the ring"));
        assert_eq!(session.name, "01a01492");
    }

    #[test]
    fn flat_totals_stand_in_when_there_is_no_model_usage_and_a_windows_directory_decodes() {
        let home = tempfile::tempdir().unwrap();
        let run = run_folder(home.path(), "C%3A%5CUsers%5Cme%5Ccode%5CPulse", "r1");
        write_lines(
            run.join("updates.jsonl"),
            &[json!({"timestamp":1_789_372_800.5,"params":{"update":{"sessionUpdate":"turn_completed","usage":{"inputTokens":50,"outputTokens":4,"reasoningTokens":1}}}}),
              // Not a turn's end, and a turn with no time: neither counts.
              json!({"timestamp":1_789_372_900,"params":{"update":{"sessionUpdate":"agent_message_chunk","usage":{"inputTokens":777}}}}),
              json!({"params":{"update":{"sessionUpdate":"turn_completed","usage":{"inputTokens":888}}}})],
        );
        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::Grok, &Sources::new(home.path()), cache.path(), &Calendar::utc(2), &PriceTable::new());
        assert_eq!(ledger.days.iter().map(|d| d.tokens).sum::<i64>(), 55);
        // The model is "grok", which has no price: counted, never costed.
        assert_eq!(ledger.unpriced_models, ["grok"]);
        assert_eq!(ledger.sessions[0].project.as_ref().unwrap().name, "Pulse");
        assert_eq!(percent_decoded("a%20b%zz%"), Some("a b%zz%".to_string()));
    }

    #[test]
    fn grok_home_moves_the_store() {
        let home = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let sources = Sources::new(home.path()).with_var("GROK_HOME", elsewhere.path());
        let inputs = SpendAgent::Grok.inputs(&sources);
        assert_eq!(inputs[0], elsewhere.path().join("sessions"));
        assert!(!inputs.contains(&home.path().join(".grok").join("sessions")));
    }

    #[test]
    fn a_sessions_buckets_reach_today_and_only_todays_counts() {
        let home = tempfile::tempdir().unwrap();
        let calendar = Calendar::local();
        let now = Utc::now();
        let today = (calendar.start_of_day(now) + Duration::hours(12)).timestamp();
        let yesterday = (calendar.add_days(calendar.start_of_day(now), -1) + Duration::hours(12)).timestamp();
        let run = run_folder(home.path(), "%2FUsers%2Fme%2FCode%2FPulse", "01a0");
        let turn = |at: i64, input: i64| json!({"timestamp":at,"params":{"update":{"sessionUpdate":"turn_completed","usage":{"modelUsage":{"priced":{"inputTokens":input,"outputTokens":0}}}}}});
        write_lines(run.join("updates.jsonl"), &[turn(today, 100), turn(yesterday, 900)]);
        let cache = tempfile::tempdir().unwrap();
        let sources = Sources::new(home.path());

        let unpriced = read_agent(SpendAgent::Grok, &sources, cache.path(), &calendar, &PriceTable::new());
        let session = &unpriced.sessions[0];
        assert_eq!(session.slots.len(), 2);
        assert_eq!(session.slots.iter().map(|s| s.unpriced_tokens).sum::<i64>(), session.unpriced_tokens);
        let summary = SpendSummary::of(&HashMap::from([(SpendAgent::Grok, unpriced)]), Some(1), now, &calendar);
        assert_eq!(summary.sessions[0].session.unpriced_tokens, 100);
        assert_eq!(summary.sessions[0].session.estimated_cost(), None);
        assert_eq!(summary.projects[0].unpriced_tokens, 100);

        let prices: PriceTable = HashMap::from([("priced".to_string(), ModelPrice::new(1_000.0, 10_000.0, Some(100.0), Some(1_000.0), Some("Priced")))]);
        let ledger = read_agent(SpendAgent::Grok, &sources, cache.path(), &calendar, &prices);
        let session = &ledger.sessions[0];
        assert_eq!(session.slots.iter().map(|s| s.tokens).sum::<i64>(), session.tokens);
        assert_eq!(ledger.days.iter().map(|d| d.tokens).sum::<i64>(), session.tokens);
        let summary = SpendSummary::of(&HashMap::from([(SpendAgent::Grok, ledger)]), Some(1), now, &calendar);
        assert_eq!(summary.tokens, 100);
        assert!((summary.cost - 0.1).abs() < 1e-9);
        assert_eq!(summary.projects[0].tokens, 100);
        assert_eq!(summary.sessions[0].session.tokens, 100);
    }

    #[test]
    fn a_store_that_is_not_there_is_not_an_empty_account() {
        let home = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        assert!(read_agent(SpendAgent::Grok, &Sources::new(home.path()), cache.path(), &Calendar::utc(2), &PriceTable::new()).days.is_empty());
    }
}
