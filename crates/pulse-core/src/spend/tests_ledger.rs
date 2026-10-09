//! Ported from upstream CacheHitRateTests, CacheWritePricingTests, ContextTierPricingTests,
//! ReplyTimingTests, ModelPrice*Tests, VendorPriceTests and the Claude Code / Codex halves of
//! SessionPricingTests, SessionTitleTests, ProjectIdentityTests, LedgerCacheWriteTests and
//! SpendReadingTests. Transcripts are built by hand in temporary folders; nothing reads the
//! user's own `~/.claude` or `~/.codex`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use chrono::{DateTime, Duration, TimeZone, Utc};
use serde_json::json;

use super::agent::SpendAgent::{ClaudeCode, Codex};
use super::calendar::Calendar;
use super::ledger::{Ledger, LedgerDay, Slot};
use super::model_summary::ModelSpendSummary;
use super::prices::{self, ContextTier, ModelPrice, ModelPrices, PriceCache, PriceTable};
use super::project::UsageProject;
use super::read_ledger_with;
use super::record::{build_ledger, AgentUsageRecord};
use super::summary::SpendSummary;
use super::tally::{ReplyTiming, TokenCost, TokenTally};
use super::transcripts::{parse_claude_code, parse_codex, parse_codex_file, project_of, Sources};
use crate::provider::Provider;

fn local() -> Calendar {
    Calendar::local()
}

fn tally_of(scanned_days: &HashMap<String, HashMap<String, TokenTally>>) -> TokenTally {
    scanned_days.values().flat_map(|m| m.values().cloned()).sum()
}

// MARK: - Cache hit rate

/// Yesterday, so `day(1)` is today: a span is counted back from today.
fn start() -> DateTime<Utc> {
    let cal = local();
    cal.add_days(cal.start_of_day(Utc::now()), -1)
}

fn day(offset: i64, tally: TokenTally, unclassified: i64) -> LedgerDay {
    let mut day = LedgerDay::new(local().add_days(start(), offset), tally.total() + unclassified, 0.0, 0, HashMap::new());
    day.tally = tally;
    day
}

fn ledger_of(days: Vec<LedgerDay>) -> Ledger {
    Ledger { earliest: days.first().map(|d| d.date), days, ..Ledger::empty() }
}

fn near(left: f64, right: f64) {
    assert!((left - right).abs() < 0.0001, "{left} != {right}");
}

#[test]
fn the_rate_reads_over_every_input_token_with_output_left_out() {
    // Claude Code's shape: fresh input, cache written, cache read.
    let rate = ledger_of(vec![
        day(0, TokenTally::new(100, 100, 600, 5_000), 0),
        day(1, TokenTally::new(50, 50, 100, 0), 0),
    ])
    .cache_hit_rate(31, Utc::now())
    .unwrap();
    near(rate, 0.7);
}

#[test]
fn codex_writes_no_cache_and_still_has_a_rate() {
    near(ledger_of(vec![day(0, TokenTally::new(250, 0, 750, 40), 0)]).cache_hit_rate(31, Utc::now()).unwrap(), 0.75);
}

#[test]
fn only_the_span_counts() {
    let rate = ledger_of(vec![day(0, TokenTally::new(1_000, 0, 0, 0), 0), day(1, TokenTally::new(10, 0, 90, 0), 0)])
        .cache_hit_rate(1, Utc::now())
        .unwrap();
    near(rate, 0.9);
}

#[test]
fn a_span_is_calendar_days_not_the_last_records() {
    // Last used ten days ago: the last week is quiet, not the week before the break.
    let stale = ledger_of(vec![day(-10, TokenTally::new(10, 0, 90, 0), 0), day(-9, TokenTally::new(10, 0, 90, 0), 0)]);
    assert_eq!(stale.total(7, Utc::now()).tokens, 0);
    assert_eq!(stale.cache_hit_rate(7, Utc::now()), None);
    assert_eq!(stale.recent(7, Utc::now()).len(), 7);
    assert_eq!(stale.total(31, Utc::now()).tokens, 200);
    // A ledger younger than the span starts at its first day.
    assert_eq!(ledger_of(vec![day(1, TokenTally::new(1, 0, 0, 0), 0)]).recent(31, Utc::now()).len(), 1);
}

#[test]
fn a_model_that_never_named_the_cache_is_left_out() {
    // One model through a compatible endpoint that writes no cache field, one through Anthropic:
    // the rate is the second's alone.
    let silent = TokenTally { input: 900, replies_without_cache_fields: 3, ..TokenTally::default() };
    let cached = TokenTally::new(10, 0, 90, 0);
    let mut mixed = LedgerDay::new(
        day(1, TokenTally::default(), 0).date,
        1_000,
        0.0,
        0,
        HashMap::from([("gateway".to_string(), 900), ("claude".to_string(), 100)]),
    );
    mixed.tally = silent.clone() + cached.clone();
    mixed.model_tallies = HashMap::from([("gateway".to_string(), silent.clone()), ("claude".to_string(), cached)]);
    near(ledger_of(vec![mixed.clone()]).cache_hit_rate(31, Utc::now()).unwrap(), 0.9);
    let names: Vec<_> = ledger_of(vec![mixed]).cache_hit_rates_by_model_in(31, Utc::now(), &local()).into_iter().map(|r| r.name).collect();
    assert_eq!(names, ["claude"]);

    let mut only = day(1, silent.clone(), 0);
    only.model_tallies = HashMap::from([("gateway".to_string(), silent)]);
    assert_eq!(ledger_of(vec![only]).cache_hit_rate(31, Utc::now()), None);
}

#[test]
fn tokens_no_kind_can_claim_withhold_it() {
    assert_eq!(ledger_of(vec![day(0, TokenTally::new(10, 0, 90, 0), 500)]).cache_hit_rate(31, Utc::now()), None);
}

#[test]
fn counts_that_may_be_short_withhold_it() {
    let mut partial = ledger_of(vec![day(0, TokenTally::new(10, 0, 90, 0), 0)]);
    partial.has_partial_counts = true;
    assert_eq!(partial.cache_hit_rate(31, Utc::now()), None);
}

#[test]
fn no_input_is_no_rate() {
    assert_eq!(ledger_of(vec![day(0, TokenTally::new(0, 0, 0, 100), 0)]).cache_hit_rate(31, Utc::now()), None);
    assert_eq!(Ledger::empty().cache_hit_rate(31, Utc::now()), None);
}

#[test]
fn a_store_with_no_cache_column_has_no_rate() {
    let mut ledger = ledger_of(vec![day(0, TokenTally::new(10, 0, 0, 5), 0)]);
    ledger.reports_cache_reads = false;
    assert_eq!(ledger.cache_hit_rate(31, Utc::now()), None);
}

#[test]
fn adding_agents_keeps_their_kinds() {
    let added = Ledger::adding(
        &[ledger_of(vec![day(0, TokenTally::new(10, 0, 30, 0), 0)]), ledger_of(vec![day(0, TokenTally::new(30, 0, 30, 0), 0)])],
        &local(),
    );
    assert_eq!(added.days[0].tally, TokenTally::new(40, 0, 60, 0));
    near(added.cache_hit_rate(31, Utc::now()).unwrap(), 0.6);
}

fn model_day(offset: i64, tallies: &[(&str, TokenTally)], untallied: &[(&str, i64)], unclassified: &[(&str, i64)]) -> LedgerDay {
    let mut models: HashMap<String, i64> = tallies.iter().map(|(k, t)| (k.to_string(), t.total())).collect();
    for (raw, tokens) in untallied.iter().chain(unclassified) {
        *models.entry(raw.to_string()).or_default() += tokens;
    }
    let mut day = LedgerDay::new(local().add_days(start(), offset), models.values().sum(), 0.0, 0, models);
    day.model_tallies = tallies.iter().map(|(k, t)| (k.to_string(), t.clone())).collect();
    day.model_unclassified_tokens = unclassified.iter().map(|(k, v)| (k.to_string(), *v)).collect();
    day
}

#[test]
fn each_model_has_its_own_rate_most_input_first() {
    let rates = ledger_of(vec![
        model_day(0, &[("opus", TokenTally::new(10, 10, 80, 999)), ("haiku", TokenTally::new(30, 0, 10, 0))], &[], &[]),
        model_day(1, &[("opus", TokenTally::new(50, 0, 50, 0))], &[], &[]),
    ])
    .cache_hit_rates_by_model_in(31, Utc::now(), &local());
    assert_eq!(rates.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["opus", "haiku"]);
    assert_eq!(rates.iter().map(|r| r.input_tokens).collect::<Vec<_>>(), [200, 40]);
    near(rates[0].rate, 0.65);
    near(rates[1].rate, 0.25);
}

#[test]
fn raw_ids_of_one_model_are_one_line() {
    let mut grouped = ledger_of(vec![model_day(
        0,
        &[("claude-opus-5-20260101", TokenTally::new(10, 0, 30, 0)), ("claude-opus-5", TokenTally::new(10, 0, 50, 0))],
        &[],
        &[],
    )]);
    grouped.model_names = HashMap::from([
        ("claude-opus-5-20260101".to_string(), "Claude Opus 5".to_string()),
        ("claude-opus-5".to_string(), "Claude Opus 5".to_string()),
    ]);
    let rates = grouped.cache_hit_rates_by_model_in(31, Utc::now(), &local());
    assert_eq!(rates.len(), 1);
    assert_eq!(rates[0].name, "Claude Opus 5");
    near(rates[0].rate, 0.8);
}

#[test]
fn a_model_with_any_unvouched_tokens_is_left_out() {
    let rates = ledger_of(vec![
        model_day(0, &[("opus", TokenTally::new(10, 0, 90, 0)), ("sonnet", TokenTally::new(10, 0, 90, 0))], &[], &[]),
        // A day that counted sonnet with no split kept, and gpt with a bare total.
        model_day(1, &[("opus", TokenTally::new(10, 0, 90, 0))], &[("sonnet", 40)], &[]),
        model_day(2, &[("gpt", TokenTally::new(5, 0, 5, 0))], &[], &[("gpt", 20)]),
    ])
    .cache_hit_rates_by_model_in(31, Utc::now(), &local());
    assert_eq!(rates.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["opus"]);
}

#[test]
fn counts_that_may_be_short_list_no_model() {
    let mut partial = ledger_of(vec![model_day(0, &[("opus", TokenTally::new(10, 0, 90, 0))], &[], &[])]);
    partial.has_partial_counts = true;
    assert!(partial.cache_hit_rates_by_model_in(31, Utc::now(), &local()).is_empty());
}

// MARK: - Ledger queries

#[test]
fn today_total_and_top_model() {
    let mut yesterday = day(0, TokenTally::new(10, 0, 0, 0), 0);
    yesterday.models = HashMap::from([("a".to_string(), 10)]);
    yesterday.cost = 1.0;
    let mut today = day(1, TokenTally::new(35, 0, 0, 0), 0);
    today.models = HashMap::from([("a".to_string(), 10), ("b".to_string(), 25)]);
    today.cost = 2.0;
    today.unpriced_tokens = 20;
    let mut ledger = ledger_of(vec![yesterday, today]);
    ledger.model_names = HashMap::from([("b".to_string(), "Model B".to_string())]);
    let now = Utc::now();
    assert_eq!(ledger.today(now).unwrap().tokens, 35);
    let total = ledger.total(1, now);
    assert_eq!((total.tokens, total.cost, total.unpriced), (35, 2.0, 20));
    assert_eq!(ledger.total(2, now).tokens, 45);
    assert_eq!(ledger.all_time().cost, 3.0);
    let (name, share) = ledger.top_model(1, now).unwrap();
    assert_eq!(name, "Model B");
    near(share, 25.0 / 35.0);
    // A quiet recent window falls back to the whole history.
    assert_eq!(ledger.top_model(1, now + Duration::days(30)).unwrap().0, "Model B");
    assert_eq!(ledger.busiest_day_in(2, now, &local()).unwrap().tokens, 35);
    assert_eq!(Ledger::shown_cost(0.0, 10, 10), None);
    assert_eq!(Ledger::shown_cost(0.0, 10, 5), Some(0.0));
    assert_eq!(Ledger::empty().today(now), None);
}

#[test]
fn a_window_takes_the_share_of_the_quarter_hours_at_either_edge() {
    let at = |s: i64| Utc.timestamp_opt(1_790_000_000 + s, 0).unwrap();
    let ledger = Ledger { slots: vec![Slot::new(at(0), 100, 9.0), Slot::new(at(900), 100, 1.0)], ..Ledger::empty() };
    // 100 s into the first quarter to 450 s into the second: 800/900 of 9 and half of 1.
    assert!((ledger.cost_between(at(100), at(1350)) - (8.0 + 0.5)).abs() < 1e-9);
    assert!((ledger.tokens_between(at(100), at(1350)) - (800.0 / 9.0 + 50.0)).abs() < 1e-9);
    assert_eq!(ledger.spend_since(at(900)), (100, 1.0));
}

// MARK: - Tally pricing

fn price() -> ModelPrice {
    ModelPrice::new(5.0, 25.0, Some(0.5), Some(6.25), None)
}

#[test]
fn the_hours_share_is_priced_at_twice_input_the_rest_at_the_five_minute_rate() {
    let tally = TokenTally { cache_write: 1_000_000, cache_write_1h: 800_000, ..TokenTally::default() };
    // 200k at $6.25/M and 800k at $10/M.
    assert!((tally.cost_breakdown(&price()).cache_write - (1.25 + 8.0)).abs() < 1e-6);
    assert_eq!(tally.total(), 1_000_000);
}

#[test]
fn an_hour_share_larger_than_the_writes_is_held_to_them() {
    let tally = TokenTally { cache_write: 100, cache_write_1h: 500, ..TokenTally::default() };
    assert!((tally.cost_breakdown(&price()).cache_write - 100.0 * 10.0 / 1_000_000.0).abs() < 1e-6);
}

#[test]
fn claude_codes_one_hour_share_is_read_from_cache_creation() {
    let line = r#"{"type":"assistant","timestamp":"2026-09-20T10:00:00Z","message":{"id":"a","model":"claude-test","usage":{"input_tokens":3,"cache_creation_input_tokens":1000,"cache_read_input_tokens":0,"output_tokens":5,"cache_creation":{"ephemeral_5m_input_tokens":200,"ephemeral_1h_input_tokens":800}}}}"#;
    let scanned = parse_claude_code(line.as_bytes(), &local());
    assert_eq!(tally_of(&scanned.all_days()), TokenTally { input: 3, cache_write: 1_000, output: 5, cache_write_1h: 800, ..TokenTally::default() });
}

#[test]
fn a_reply_with_no_cache_field_at_all_is_told_apart_from_one_with_zero() {
    let silent = r#"{"type":"assistant","timestamp":"2026-09-20T10:00:00Z","message":{"id":"a","model":"glm","usage":{"input_tokens":30,"output_tokens":5}}}"#;
    let zero = r#"{"type":"assistant","timestamp":"2026-09-20T10:01:00Z","message":{"id":"b","model":"claude-test","usage":{"input_tokens":30,"cache_read_input_tokens":0,"cache_creation_input_tokens":0,"output_tokens":5}}}"#;
    let scanned = parse_claude_code(format!("{silent}\n{zero}").as_bytes(), &local());
    let mut by_model: HashMap<String, TokenTally> = HashMap::new();
    for models in scanned.all_days().values() {
        for (model, tally) in models {
            *by_model.entry(model.clone()).or_default() += tally;
        }
    }
    assert!(by_model["glm"].reports_no_cache());
    assert!(!by_model["claude-test"].reports_no_cache());
}

#[test]
fn a_tally_saved_before_the_hour_share_existed_still_decodes() {
    let tally: TokenTally = serde_json::from_str(r#"{"input":1,"cacheWrite":2,"cacheRead":3,"output":4}"#).unwrap();
    assert_eq!(tally, TokenTally::new(1, 2, 3, 4));
}

// MARK: - Long-context tiers

fn tiered() -> ModelPrice {
    // gpt-5.5's shape on models.dev.
    ModelPrice::new(5.0, 30.0, Some(0.5), None, None)
        .with_tiers(vec![ContextTier { threshold: 272_000, input: 10.0, output: 45.0, cache_read: Some(1.0), cache_write: None }])
}

#[test]
fn models_dev_tiers_are_read_with_272001_meaning_over_272000() {
    let cost = json!({
        "input": 5, "output": 30,
        "tiers": [{"input": 10, "output": 45, "cache_read": 1, "tier": {"type": "context", "size": 272_001}}],
    });
    let tier = prices::tiers_in(&cost).unwrap().remove(0);
    assert_eq!(tier.threshold, 272_000);
    assert!(tier.input == 10.0 && tier.cache_read == Some(1.0));
    // Where only `context_over_200k` is stated, it is a 200K tier.
    let over = prices::tiers_in(&json!({"context_over_200k": {"input": 6, "output": 22.5}}));
    assert_eq!(over.unwrap()[0].threshold, 200_000);
    assert!(prices::tiers_in(&json!({"input": 1, "output": 2})).is_none());
}

#[test]
fn a_request_over_the_threshold_is_priced_at_the_tier_whole_one_under_it_at_the_base() {
    let short = TokenTally::new(1_000_000, 0, 0, 0).request(100_000);
    let long = TokenTally::new(1_000_000, 0, 0, 0).request(300_000);
    assert!((short.cost_at(&tiered()) - 5.0).abs() < 1e-9);
    assert!((long.cost_at(&tiered()) - 10.0).abs() < 1e-9);
    // Added up in one quarter-hour, each still pays its own rate.
    assert!(((short.clone() + long.clone()).cost_at(&tiered()) - 15.0).abs() < 1e-9);
    // A model with no tier pays its base rate for both.
    let flat = ModelPrice::new(5.0, 30.0, Some(0.5), None, None);
    assert!(((short + long).cost_at(&flat) - 10.0).abs() < 1e-9);
}

#[test]
fn a_request_between_two_kept_sizes_is_not_priced_at_a_tier_it_may_not_have_reached() {
    // 262,144 sits between the 256K and 272K bands: a 260K request is in the 256K band and is not
    // moved to a 262,144 tier.
    let odd = ModelPrice::new(1.0, 1.0, None, None, None)
        .with_tiers(vec![ContextTier { threshold: 262_144, input: 9.0, output: 9.0, cache_read: None, cache_write: None }]);
    assert!((TokenTally::new(1_000_000, 0, 0, 0).request(260_000).cost_at(&odd) - 1.0).abs() < 1e-9);
}

#[test]
fn codex_a_reading_whose_request_input_passed_272k_lands_in_that_band() {
    let lines = [
        r#"{"timestamp":"2026-01-02T09:00:00Z","type":"turn_context","payload":{"model":"gpt-5.5"}}"#,
        r#"{"timestamp":"2026-01-02T09:00:00Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":300000,"cached_input_tokens":0,"output_tokens":10,"total_tokens":300010},"last_token_usage":{"input_tokens":300000,"cached_input_tokens":0,"output_tokens":10,"total_tokens":300010}}}}"#,
    ];
    let tally = tally_of(&parse_codex(lines.join("\n").as_bytes(), &local()).all_days());
    assert_eq!(tally.context_bands.keys().copied().collect::<Vec<_>>(), [272_000]);
    assert_eq!(tally.context_bands[&272_000].total(), 300_010);
}

#[test]
fn only_what_is_not_zero_is_written_and_it_reads_back_whole() {
    let tally = TokenTally::new(3, 0, 0, 4).request(210_000);
    let text = serde_json::to_string(&tally).unwrap();
    assert!(!text.contains("cacheRead"));
    assert_eq!(serde_json::from_str::<TokenTally>(&text).unwrap(), tally);
}

// MARK: - Reply timing

fn claude_line(kind: &str, time: &str, id: Option<&str>, output: i64) -> String {
    match (kind, id) {
        ("assistant", Some(id)) => format!(
            r#"{{"type":"assistant","message":{{"id":"{id}","model":"claude-test","usage":{{"input_tokens":10,"cache_read_input_tokens":90,"output_tokens":{output}}}}},"timestamp":"{time}"}}"#
        ),
        _ => format!(
            r#"{{"type":"user","message":{{"role":"user","content":[{{"type":"tool_result","content":"{{\"type\":\"user\",\"timestamp\":\"2000-01-01T00:00:00Z\"}}"}}]}},"timestamp":"{time}"}}"#
        ),
    }
}

#[test]
fn a_reply_over_several_lines_counts_its_last_lines_output_timed_from_the_request_to_its_last_line() {
    let lines = [
        claude_line("user", "2026-09-20T10:00:00.000Z", None, 0),
        claude_line("assistant", "2026-09-20T10:00:04.000Z", Some("a"), 12),
        claude_line("assistant", "2026-09-20T10:00:10.000Z", Some("a"), 500),
        // A tool result between two lines of the next reply's request does not start the clock
        // before this reply ended.
        claude_line("user", "2026-09-20T10:00:09.000Z", None, 0),
        claude_line("assistant", "2026-09-20T10:00:20.000Z", Some("b"), 300),
        // Too short to stand for a speed, still counted as tokens.
        claude_line("user", "2026-09-20T10:00:21.000Z", None, 0),
        claude_line("assistant", "2026-09-20T10:00:23.000Z", Some("c"), 40),
    ];
    let scanned = parse_claude_code(lines.join("\n").as_bytes(), &local());
    let output: i64 = scanned.all_days().values().flat_map(|m| m.values()).map(|t| t.output).sum();
    assert_eq!(output, 500 + 300 + 40);
    let timing = scanned.all_timings().values().flat_map(|m| m.values().copied()).fold(ReplyTiming::default(), |a, b| a + b);
    assert_eq!(timing.replies, 2);
    assert_eq!(timing.output_tokens, 800);
    assert!((timing.seconds - 20.0).abs() < 0.001);
    assert_eq!(timing.first_token_turns, 0);
}

fn temp_home() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

fn write(root: &Path, relative: &str, text: &str) -> PathBuf {
    let file = root.join(relative);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, text).unwrap();
    file
}

fn set_modified(file: &Path, when: DateTime<Utc>) {
    let handle = std::fs::OpenOptions::new().write(true).open(file).unwrap();
    handle.set_modified(SystemTime::from(when)).unwrap();
}

fn read(home: &Path, cache: &Path, provider: Provider, prices: &PriceTable) -> Ledger {
    read_ledger_with(provider, &Sources::new(home), cache, &local(), prices, Utc::now()).unwrap()
}

#[test]
fn a_reply_copied_into_a_resumed_sessions_transcript_is_counted_once_for_the_original() {
    let root = temp_home();
    let history = [claude_line("user", "2026-09-20T10:00:00.000Z", None, 0), claude_line("assistant", "2026-09-20T10:00:10.000Z", Some("a"), 500)];
    // The original, then a resumed session: the same history, then its own reply.
    write(root.path(), ".claude/projects/-Users-me-Code-Pulse/original.jsonl", &history.join("\n"));
    let resumed = [
        history[0].clone(),
        history[1].clone(),
        claude_line("user", "2026-09-21T09:00:00.000Z", None, 0),
        claude_line("assistant", "2026-09-21T09:00:05.000Z", Some("b"), 300),
    ];
    write(root.path(), ".claude/projects/-Users-me-Code-Pulse/resumed.jsonl", &resumed.join("\n"));

    let cache = root.path().join("cache");
    let ledger = read(root.path(), &cache, ClaudeCode.provider().unwrap(), &PriceTable::new());
    assert_eq!(ledger.days.iter().map(|d| d.tally.output).sum::<i64>(), 500 + 300);
    let sessions: HashMap<_, _> = ledger.sessions.iter().map(|s| (s.name.clone(), s.tokens)).collect();
    assert_eq!(sessions["original"], 10 + 90 + 500);
    assert_eq!(sessions["resumed"], 10 + 90 + 300);
}

#[test]
fn codex_pairs_a_late_count_with_the_reply_before_the_request_that_preceded_it_and_keeps_its_first_token_wait() {
    fn line(time: &str, payload: &str, kind: &str) -> String {
        format!(r#"{{"timestamp":"{time}","type":"{kind}","payload":{payload}}}"#)
    }
    let lines = [
        line("2026-09-20T10:00:00.000Z", r#"{"model":"gpt-test"}"#, "turn_context"),
        line("2026-09-20T10:00:00.000Z", r#"{"type":"message","role":"user","content":[{"type":"input_text","text":"hi"}]}"#, "response_item"),
        line("2026-09-20T10:00:03.000Z", r#"{"type":"reasoning"}"#, "response_item"),
        line("2026-09-20T10:00:05.000Z", r#"{"type":"function_call","name":"shell"}"#, "response_item"),
        // The tool runs, its output goes back, and only then the count.
        line("2026-09-20T10:00:30.000Z", r#"{"type":"function_call_output","output":"x"}"#, "response_item"),
        line("2026-09-20T10:00:30.001Z", r#"{"type":"token_count","info":{"total_token_usage":{"input_tokens":1000,"cached_input_tokens":900,"output_tokens":400}}}"#, "event_msg"),
        line("2026-09-20T10:00:34.000Z", r#"{"type":"message","role":"assistant","content":[]}"#, "response_item"),
        line("2026-09-20T10:00:34.001Z", r#"{"type":"token_count","info":{"total_token_usage":{"input_tokens":2000,"cached_input_tokens":1900,"output_tokens":600}}}"#, "event_msg"),
        line("2026-09-20T10:00:34.002Z", r#"{"type":"task_complete","duration_ms":34000,"time_to_first_token_ms":2500}"#, "event_msg"),
    ];
    let root = temp_home();
    let file = write(root.path(), "rollout.jsonl", &lines.join("\n"));
    let scanned = parse_codex_file(&file, &local());
    let timing = scanned.timings.values().flat_map(|m| m.values().copied()).fold(ReplyTiming::default(), |a, b| a + b);
    // 400 over 0:00 -> 0:05, 200 over 0:30 -> 0:34.
    assert_eq!(timing.replies, 2);
    assert_eq!(timing.output_tokens, 600);
    assert!((timing.seconds - 9.0).abs() < 0.001);
    assert_eq!(timing.first_token_turns, 1);
    assert_eq!(timing.first_token_seconds, 2.5);
}

#[test]
fn speeds_and_waits_need_five_timed_replies_or_turns_per_model_within_the_last_day() {
    let now = Utc::now();
    let slot = |ago: i64, timings: Vec<(&str, ReplyTiming)>| Slot {
        timings: timings.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
        ..Slot::new(now - Duration::seconds(ago), 1, 0.0)
    };
    let timing = |output, seconds, replies| ReplyTiming { output_tokens: output, seconds, replies, ..ReplyTiming::default() };
    let ledger = Ledger {
        model_names: HashMap::from([("fast".to_string(), "Fast".to_string())]),
        slots: vec![
            // Two days old: a speed for a server that has moved on.
            slot(2 * 86_400, vec![("fast", timing(100_000, 10.0, 50))]),
            slot(
                3_600,
                vec![
                    ("fast", timing(1_000, 10.0, 5)),
                    ("rare", timing(5_000, 10.0, 4)),
                    ("codex", ReplyTiming { first_token_seconds: 15.0, first_token_turns: 5, ..timing(600, 20.0, 6) }),
                ],
            ),
        ],
        ..Ledger::empty()
    };
    let speeds = ledger.output_speeds_by_model(now - Duration::seconds(Ledger::SPEED_SPAN_SECONDS));
    assert_eq!(speeds.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["Fast", "codex"]);
    assert_eq!(speeds[0].tokens_per_second, Some(100.0));
    assert_eq!(speeds[0].first_token_seconds, None);
    assert_eq!(speeds[1].tokens_per_second, Some(30.0));
    assert_eq!(speeds[1].first_token_seconds, Some(3.0));
    assert!(ledger.output_speeds_by_model(now).is_empty());
}

#[test]
fn a_wait_or_reply_outside_the_plausible_range_is_not_timed() {
    assert!(ReplyTiming::reply(99, 1.0).is_none());
    assert!(ReplyTiming::reply(500, 0.0).is_none());
    assert!(ReplyTiming::reply(500, ReplyTiming::LONGEST + 1.0).is_none());
    assert!(ReplyTiming::reply(500, 5.0).is_some());
    assert!(ReplyTiming::first_token(0.0).is_none());
    assert!(ReplyTiming::first_token(ReplyTiming::LONGEST_FIRST_TOKEN + 1.0).is_none());
}

// MARK: - Session titles

#[test]
fn the_folder_name_is_only_the_fallback_for_a_missing_directory() {
    // Claude Code replaces every separator with a dash, so the folder name cannot be turned back
    // into a path: the stated `cwd` is preferred wherever a transcript has one.
    let claude = "/Users/me/.claude/projects/-Users-me-Code-Pulse/abc.jsonl";
    assert_eq!(project_of(claude, ClaudeCode.provider().unwrap()).unwrap().name, "Pulse");
    let windows = r"C:\Users\me\.claude\projects\D--code-Pulse\abc.jsonl";
    assert_eq!(project_of(windows, ClaudeCode.provider().unwrap()).unwrap().name, "Pulse");
    assert!(project_of("/Users/me/.codex/sessions/2026/09/14/rollout-x.jsonl", Provider::Codex).is_none());
}

const OPENING: &str = r#"{"type":"user","cwd":"/Users/me/Code/Pulse","message":{"role":"user","content":"fix the ring"}}"#;

fn parse(lines: &[&str]) -> super::transcripts::Scanned {
    parse_claude_code(lines.join("\n").as_bytes(), &local())
}

#[test]
fn a_title_set_after_the_opening_prompt_is_not_ignored() {
    // Claude Code writes `customTitle` on its own line when a conversation is renamed, long after
    // `cwd` and the opening prompt were read.
    let scanned = parse(&[OPENING, r#"{"type":"custom-title","customTitle":"Renamed by hand"}"#]);
    assert_eq!(scanned.title.as_deref(), Some("Renamed by hand"));
    assert_eq!(scanned.cwd.as_deref(), Some("/Users/me/Code/Pulse"));
}

#[test]
fn the_last_valid_custom_title_wins() {
    let scanned = parse(&[OPENING, r#"{"customTitle":"First name"}"#, r#"{"customTitle":"Second name"}"#]);
    assert_eq!(scanned.title.as_deref(), Some("Second name"));
}

#[test]
fn an_unreadable_custom_title_leaves_the_valid_one_alone() {
    let scanned = parse(&[OPENING, r#"{"customTitle":"A real name"}"#, r#"{"customTitle":""}"#]);
    assert_eq!(scanned.title.as_deref(), Some("A real name"));
}

#[test]
fn a_custom_title_still_outranks_an_opening_prompt_that_comes_after_it() {
    let scanned = parse(&[r#"{"type":"custom-title","customTitle":"Chosen first"}"#, OPENING]);
    assert_eq!(scanned.title.as_deref(), Some("Chosen first"));
}

#[test]
fn reading_a_custom_title_does_not_change_the_token_counts() {
    let scanned = parse(&[
        OPENING,
        r#"{"type":"assistant","timestamp":"2026-09-14T10:00:00Z","message":{"id":"m1","model":"claude-sonnet-4-5","usage":{"input_tokens":100,"output_tokens":10}}}"#,
        r#"{"customTitle":"Renamed"}"#,
    ]);
    assert_eq!(scanned.title.as_deref(), Some("Renamed"));
    let tally = tally_of(&scanned.all_days());
    assert_eq!((tally.input, tally.output, tally.total()), (100, 10, 110));
}

#[test]
fn a_synthetic_reply_and_an_idless_reply_are_handled() {
    let scanned = parse(&[
        r#"{"type":"assistant","timestamp":"2026-09-14T10:00:00Z","message":{"id":"m1","model":"<synthetic>","usage":{"input_tokens":100,"output_tokens":10}}}"#,
        r#"{"type":"assistant","timestamp":"2026-09-14T10:00:00Z","message":{"model":"m","usage":{"input_tokens":7,"output_tokens":1}}}"#,
        r#"{"type":"assistant","timestamp":"2026-09-14T10:00:00Z","message":{"model":"m","usage":{"input_tokens":7,"output_tokens":1}}}"#,
    ]);
    // The placeholder counts nothing; a reply with no id has nothing to be de-duplicated by.
    assert_eq!(tally_of(&scanned.all_days()).total(), 16);
}

// MARK: - Transcripts through the ledger

fn test_prices() -> PriceTable {
    HashMap::from([
        ("paid".to_string(), ModelPrice::new(1_000.0, 1_000.0, None, None, None)),
        ("free".to_string(), ModelPrice::new(0.0, 0.0, None, None, None)),
    ])
}

#[test]
fn claude_and_codex_sessions_retain_unpriced_buckets() {
    for provider in [ClaudeCode, Codex] {
        let root = temp_home();
        let (folder, lines) = if provider == ClaudeCode {
            (
                ".claude/projects/fixture",
                vec![
                    r#"{"type":"assistant","timestamp":"2026-09-19T10:00:00Z","cwd":"/work/api","message":{"id":"m1","model":"unknown","usage":{"input_tokens":90}}}"#,
                    r#"{"type":"assistant","timestamp":"2026-09-20T10:00:00Z","cwd":"/work/api","message":{"id":"m2","model":"paid","usage":{"input_tokens":10}}}"#,
                ],
            )
        } else {
            (
                ".codex/sessions",
                vec![
                    r#"{"payload":{"cwd":"/work/api","model":"unknown"}}"#,
                    r#"{"timestamp":"2026-09-19T10:00:00Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":90,"output_tokens":0}}}}"#,
                    r#"{"payload":{"model":"paid"}}"#,
                    r#"{"timestamp":"2026-09-20T10:00:00Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"output_tokens":0}}}}"#,
                ],
            )
        };
        write(root.path(), &format!("{folder}/session.jsonl"), &lines.join("\n"));
        let calendar = Calendar::utc(2);
        let cache = root.path().join("cache");
        let ledger =
            read_ledger_with(provider.provider().unwrap(), &Sources::new(root.path()), &cache, &calendar, &test_prices(), Utc::now()).unwrap();
        let session = &ledger.sessions[0];
        assert_eq!(session.tokens, 100);
        assert_eq!(session.unpriced_tokens, 90);
        assert_eq!(session.slots.iter().map(|s| s.unpriced_tokens).sum::<i64>(), 90);

        // Only the part of the session inside the span is counted: today's, the priced half.
        let now = Utc.with_ymd_and_hms(2026, 9, 20, 12, 0, 0).unwrap();
        let today = SpendSummary::of(&HashMap::from([(provider, ledger)]), Some(1), now, &calendar);
        assert_eq!(today.sessions[0].session.unpriced_tokens, 0);
        assert_eq!(today.sessions[0].session.estimated_cost(), Some(0.01));
        assert_eq!(today.projects[0].unpriced_tokens, 0);
    }
}

#[test]
fn unchanged_rescans_reprice_without_rewriting_edits_and_deletions_persist() {
    let home = temp_home();
    let cache = home.path().join("cache");
    let row = |id: &str| {
        format!(r#"{{"type":"assistant","timestamp":"2026-09-20T10:00:00Z","message":{{"id":"{id}","model":"priced","usage":{{"input_tokens":100,"output_tokens":10}}}}}}"#)
    };
    let log = write(home.path(), ".claude/projects/fixture/s.jsonl", &format!("{}\n", row("one")));

    let first = read(home.path(), &cache, ClaudeCode.provider().unwrap(), &PriceTable::new());
    assert_eq!(first.all_time().tokens, 110);
    assert_eq!(first.all_time().cost, 0.0);
    let saved = cache.join("ledger-9-claudeCode.json");
    // An old timestamp makes a write observable without sleeps or timing thresholds.
    let sentinel = Utc.timestamp_opt(1_000_000, 0).unwrap();
    set_modified(&saved, sentinel);
    let prices = HashMap::from([("priced".to_string(), ModelPrice::new(1.0, 2.0, None, None, None))]);
    let repriced = read(home.path(), &cache, ClaudeCode.provider().unwrap(), &prices);
    assert_eq!(repriced.all_time().tokens, 110);
    assert!((repriced.all_time().cost - 0.00012).abs() < 1e-8);
    let modified = |path: &Path| DateTime::<Utc>::from(std::fs::metadata(path).unwrap().modified().unwrap());
    assert_eq!(modified(&saved), sentinel);

    std::fs::write(&log, format!("{}\n{}\n", row("one"), row("two"))).unwrap();
    let changed = read(home.path(), &cache, ClaudeCode.provider().unwrap(), &prices);
    assert_eq!(changed.all_time().tokens, 220);
    assert_ne!(modified(&saved), sentinel);
    let restored = read(home.path(), &cache, ClaudeCode.provider().unwrap(), &prices);
    assert_eq!(restored.all_time().tokens, 220);

    std::fs::remove_file(&log).unwrap();
    let removed = read(home.path(), &cache, ClaudeCode.provider().unwrap(), &prices);
    assert_eq!(removed.all_time().tokens, 0);
    let object: serde_json::Value = serde_json::from_slice(&std::fs::read(&saved).unwrap()).unwrap();
    assert!(object["files"].as_object().unwrap().is_empty());
}

#[test]
fn unchanged_files_are_served_from_the_cache_not_reparsed() {
    let home = temp_home();
    let cache = home.path().join("cache");
    let row = r#"{"type":"assistant","timestamp":"2026-09-20T10:00:00Z","message":{"id":"one","model":"m","usage":{"input_tokens":100,"output_tokens":10}}}"#;
    let log = write(home.path(), ".claude/projects/fixture/s.jsonl", row);
    assert_eq!(read(home.path(), &cache, ClaudeCode.provider().unwrap(), &PriceTable::new()).all_time().tokens, 110);
    // Same size, same modification time, different contents: only the cache still knows the
    // original numbers, so this makes its reuse observable.
    let stamp = std::fs::metadata(&log).unwrap().modified().unwrap();
    std::fs::write(&log, row.replace("100", "999")).unwrap();
    std::fs::OpenOptions::new().write(true).open(&log).unwrap().set_modified(stamp).unwrap();
    assert_eq!(read(home.path(), &cache, ClaudeCode.provider().unwrap(), &PriceTable::new()).all_time().tokens, 110);
}

#[test]
fn the_streamed_codex_parser_keeps_cumulative_deltas() {
    let root = temp_home();
    let count = r#"{"timestamp":"2026-01-02T09:00:00Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":10}}}}"#;
    let file = write(root.path(), ".codex/sessions/session.jsonl", &format!("{}\n{count}\n{count}", r#"{"payload":{"cwd":"/work/project","model":"gpt-5"}}"#));
    let scanned = parse_codex_file(&file, &local());
    assert_eq!(scanned.cwd.as_deref(), Some("/work/project"));
    assert_eq!(tally_of(&scanned.all_days()), TokenTally::new(80, 0, 20, 10));
    let cache = root.path().join("cache");
    assert_eq!(read(root.path(), &cache, Codex.provider().unwrap(), &PriceTable::new()).all_time().tokens, 110);
    assert!(cache.join("ledger-9-codex.json").exists());
}

fn codex_count(time: &str, input: i64, output: i64, last: (i64, i64)) -> String {
    format!(
        r#"{{"timestamp":"2026-01-02T{time}Z","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":{input},"cached_input_tokens":0,"output_tokens":{output},"total_tokens":{}}},"last_token_usage":{{"input_tokens":{},"cached_input_tokens":0,"output_tokens":{},"total_tokens":{}}}}}}}}}"#,
        input + output,
        last.0,
        last.1,
        last.0 + last.1
    )
}

fn codex_turn(id: &str) -> String {
    format!(r#"{{"timestamp":"2026-01-02T09:10:00Z","type":"event_msg","payload":{{"type":"task_started","turn_id":"{id}"}}}}"#)
}

const CODEX_MODEL: &str = r#"{"timestamp":"2026-01-02T09:00:00Z","type":"turn_context","payload":{"model":"gpt-5"}}"#;

/// A Codex Desktop sub-agent's rollout: it opens on the parent's running total, replays the
/// parent's last reading in a `rollout-N` turn, then does 550 tokens of its own work.
fn codex_fork(replay_turn: &str) -> String {
    [
        r#"{"timestamp":"2026-01-02T09:10:00Z","type":"session_meta","payload":{"id":"child","forked_from_id":"parent"}}"#.to_string(),
        CODEX_MODEL.to_string(),
        codex_count("09:10:00", 1_000, 100, (0, 0)),
        codex_turn(replay_turn),
        codex_count("09:10:00", 2_000, 200, (1_000, 100)),
        codex_turn("019f8f37-eb69-7e62-8fa8-e3d19fec5ade"),
        codex_count("09:10:00", 2_500, 250, (500, 50)),
    ]
    .join("\n")
}

#[test]
fn a_codex_fork_counts_only_its_own_work_archived_or_not_with_or_without_its_parent_on_disk() {
    let root = temp_home();
    let parent = [
        r#"{"timestamp":"2026-01-02T09:00:00Z","type":"session_meta","payload":{"id":"parent"}}"#.to_string(),
        CODEX_MODEL.to_string(),
        codex_count("09:00:00", 1_000, 100, (1_000, 100)),
        codex_count("09:05:00", 2_000, 200, (1_000, 100)),
    ]
    .join("\n");
    write(root.path(), ".codex/sessions/2026/01/02/rollout-parent.jsonl", &parent);
    // Archived, and its replayed reading in an ordinary turn: the copy is known by the running
    // total it repeats.
    write(root.path(), ".codex/archived_sessions/rollout-child.jsonl", &codex_fork("019f8f06-dfd5-7cd2-a871-b31a926f92b7"));

    let cache = root.path().join("cache");
    assert_eq!(read(root.path(), &cache, Codex.provider().unwrap(), &PriceTable::new()).all_time().tokens, 2_200 + 550);

    // The parent gone, the replayed turn is still known by its `rollout-` id, and the opening
    // total is still the parent's.
    let alone = write(root.path(), "alone.jsonl", &codex_fork("rollout-4"));
    assert_eq!(tally_of(&parse_codex_file(&alone, &local()).all_days()).total(), 550);
}

#[test]
fn a_fork_is_ordered_after_every_session_that_is_not_one() {
    // Both files name the same running total; whichever is a fork must lose, whatever the names.
    let root = temp_home();
    let parent = [
        r#"{"timestamp":"2026-01-02T09:00:00Z","type":"session_meta","payload":{"id":"parent"}}"#.to_string(),
        CODEX_MODEL.to_string(),
        codex_count("09:00:00", 1_000, 100, (1_000, 100)),
    ]
    .join("\n");
    let fork = [
        r#"{"timestamp":"2026-01-02T08:00:00Z","type":"session_meta","payload":{"id":"child","forked_from_id":"parent"}}"#.to_string(),
        CODEX_MODEL.to_string(),
        codex_count("08:00:00", 1_000, 100, (1_000, 100)),
    ]
    .join("\n");
    write(root.path(), ".codex/sessions/a-fork.jsonl", &fork);
    write(root.path(), ".codex/sessions/z-parent.jsonl", &parent);
    let ledger = read(root.path(), &root.path().join("cache"), Codex.provider().unwrap(), &PriceTable::new());
    assert_eq!(ledger.all_time().tokens, 1_100);
}

#[test]
fn environment_overrides_move_the_transcript_roots() {
    let home = temp_home();
    let elsewhere = temp_home();
    write(elsewhere.path(), "projects/p/s.jsonl", r#"{"type":"assistant","timestamp":"2026-09-20T10:00:00Z","message":{"id":"x","model":"m","usage":{"input_tokens":5}}}"#);
    write(
        elsewhere.path(),
        "sessions/r.jsonl",
        &format!("{}\n{}", r#"{"payload":{"model":"gpt-5"}}"#, r#"{"timestamp":"2026-01-02T09:00:00Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":7}}}}"#),
    );
    let mut sources = Sources::new(home.path());
    let cache = home.path().join("cache");
    let tokens = |sources: &Sources, provider| {
        read_ledger_with(provider, sources, &cache, &local(), &PriceTable::new(), Utc::now()).unwrap().all_time().tokens
    };
    assert_eq!(tokens(&sources, ClaudeCode.provider().unwrap()), 0);
    sources.claude_config_dir = Some(elsewhere.path().to_path_buf());
    sources.codex_home = Some(elsewhere.path().to_path_buf());
    assert_eq!(tokens(&sources, ClaudeCode.provider().unwrap()), 5);
    assert_eq!(tokens(&sources, Codex.provider().unwrap()), 7);
    assert!(matches!(
        read_ledger_with(Provider::Cursor, &sources, &cache, &local(), &PriceTable::new(), Utc::now()),
        Err(super::SpendError::Unsupported(_))
    ));
}

// MARK: - Projects

#[test]
fn same_name_directories_stay_separate_after_the_per_file_cache_reloads() {
    for provider in [ClaudeCode, Codex] {
        let root = temp_home();
        let when = Utc.with_ymd_and_hms(2026, 9, 20, 10, 0, 0).unwrap();
        for (index, cwd) in ["/work/client-a/api", "/work/client-b/api", "/work/client-a/api/"].into_iter().enumerate() {
            let (path, rows) = if provider == ClaudeCode {
                (
                    format!(".claude/projects/fixture/s{index}.jsonl"),
                    vec![json!({"type": "assistant", "timestamp": "2026-09-20T10:00:00Z", "cwd": cwd,
                        "message": {"id": format!("m{index}"), "model": "unknown", "usage": {"input_tokens": 100, "output_tokens": 10}}})],
                )
            } else {
                (
                    format!(".codex/sessions/s{index}.jsonl"),
                    vec![
                        json!({"payload": {"cwd": cwd, "model": "unknown"}}),
                        json!({"timestamp": "2026-09-20T10:00:00Z", "payload": {"type": "token_count",
                            "info": {"total_token_usage": {"input_tokens": 100, "output_tokens": 10}}}}),
                    ],
                )
            };
            let text: Vec<String> = rows.iter().map(ToString::to_string).collect();
            let file = write(root.path(), &path, &text.join("\n"));
            set_modified(&file, when);
        }
        let cache = root.path().join("cache");
        for pass in 0..2 {
            // A new read each time: the second pass restores the per-file cache.
            let ledger = read(root.path(), &cache, provider.provider().unwrap(), &PriceTable::new());
            let summary = SpendSummary::of(&HashMap::from([(provider, ledger)]), None, when, &local());
            assert_eq!(summary.projects.len(), 2);
            let counts: HashMap<_, _> = summary.projects.iter().map(|p| (p.name.clone(), p.tokens)).collect();
            assert_eq!(counts, HashMap::from([("client-a/api".to_string(), 220), ("client-b/api".to_string(), 110)]));
            assert_eq!(summary.projects.iter().map(|p| p.sessions).sum::<usize>(), 3);
            assert!(summary.sessions.iter().all(|s| summary.project_name(s).is_some_and(|n| n.contains("/api"))));
            if pass == 0 {
                // Same size and mtime: only the per-file cache still knows the original
                // directory. This makes its reuse observable.
                let log = root.path().join(if provider == ClaudeCode { ".claude/projects/fixture/s0.jsonl" } else { ".codex/sessions/s0.jsonl" });
                let contents = std::fs::read_to_string(&log).unwrap();
                std::fs::write(&log, contents.replace("client-a", "client-z")).unwrap();
                set_modified(&log, when);
            }
        }
    }
}

#[test]
fn explicit_paths_merge_across_agents_labels_do_not_claim_a_directory() {
    let when = Utc.with_ymd_and_hms(2026, 9, 20, 10, 0, 0).unwrap();
    let ledger = |project: &str, namespace: &str| {
        build_ledger(
            &[AgentUsageRecord::new(when, "unknown", TokenTally::new(10, 0, 0, 0)).session("s").project(project)],
            &PriceTable::new(),
            namespace,
            None,
            &local(),
            super::ledger::Origin::LocalTranscripts,
        )
    };
    let merged = SpendSummary::of(
        &HashMap::from([(ClaudeCode, ledger("/work/api", "claude")), (Codex, ledger("/work/api/", "codex"))]),
        None,
        when,
        &local(),
    );
    assert_eq!(merged.projects.len(), 1);
    assert_eq!(merged.projects[0].name, "api");
    assert_eq!(merged.projects[0].tokens, 20);
    let labels = SpendSummary::of(
        &HashMap::from([(ClaudeCode, ledger("api", "claude")), (Codex, ledger("api", "codex"))]),
        None,
        when,
        &local(),
    );
    assert_eq!(labels.projects.len(), 2);
    assert_eq!(labels.projects.iter().map(|p| p.name.clone()).collect::<std::collections::HashSet<_>>().len(), 2);
}

#[test]
fn claude_fallback_folders_retain_identity_without_guessing_a_cwd() {
    let first = project_of("/logs/-work-client-a-api/a.jsonl", Provider::ClaudeCode).unwrap();
    let second = project_of("/logs/-work-client-b-api/b.jsonl", Provider::ClaudeCode).unwrap();
    assert_eq!(first.name, "api");
    assert_eq!(first.path(), None);
    assert_ne!(first.identity, second.identity);
    assert_ne!(Some(&first), UsageProject::new(Some("/work/client-a/api")).as_ref());
    assert!(UsageProject::new(Some(" \n")).is_none());
    assert_ne!(UsageProject::new(Some("/Work/API")), UsageProject::new(Some("/work/api")));
}

#[test]
fn directory_suffixes_grow_until_nested_names_are_distinguishable() {
    let a = UsageProject::new(Some("/client-a/service/api")).unwrap();
    let b = UsageProject::new(Some("/client-b/service/api")).unwrap();
    assert_eq!(UsageProject::display_name(&a, &[&a, &b]), "client-a/service/api");
    assert_eq!(UsageProject::display_name(&b, &[&a, &b]), "client-b/service/api");
    assert_eq!(UsageProject::display_name(&a, &[&a]), "api");
}

#[test]
fn a_remote_workspace_uri_is_named_by_its_last_folder_and_kept_whole_as_its_identity() {
    let one = UsageProject::new(Some("vscode-remote://ssh-remote%2Bhost-a/home/me/proj")).unwrap();
    let two = UsageProject::new(Some("vscode-remote://ssh-remote%2Bhost-b/home/me/proj")).unwrap();
    assert_eq!(one.name, "proj");
    assert_eq!(one.identity, super::project::ProjectIdentity::Label("vscode-remote://ssh-remote%2Bhost-a/home/me/proj".into()));
    assert_ne!(one, two);
}

#[test]
fn an_agent_worktree_is_its_repositorys_project_read_fresh_or_from_an_old_cache() {
    let worktree = UsageProject::new(Some("/Users/me/Pulse/.claude/worktrees/agent-a4734cf379be4c74a/Sources")).unwrap();
    assert_eq!(Some(&worktree), UsageProject::new(Some("/Users/me/Pulse")).as_ref());
    assert_eq!(worktree.name, "Pulse");
    // Not a worktree: a folder that only looks like one is left alone.
    assert_eq!(UsageProject::new(Some("/.claude/worktrees/x")).unwrap().name, "x");
    assert_eq!(UsageProject::new(Some("/Users/me/claude/worktrees/x")).unwrap().name, "x");

    let cached = r#"{"identity":{"directory":"/Users/me/Pulse/.claude/worktrees/agent-x"},"name":"agent-x"}"#;
    let decoded: UsageProject = serde_json::from_str(cached).unwrap();
    assert_eq!(Some(decoded), UsageProject::new(Some("/Users/me/Pulse")));
}

#[test]
fn windows_directories_are_projects_and_a_scratch_folder_is_none() {
    let project = UsageProject::new(Some(r"D:\code\Pulse")).unwrap();
    assert_eq!(project.name, "Pulse");
    assert_eq!(project.path(), Some("D:/code/Pulse"));
    assert_eq!(UsageProject::new(Some(r"D:\code\Pulse\")), UsageProject::new(Some("D:/code/Pulse")));
    assert!(UsageProject::new(Some(r"C:\Users\me\Documents\Codex\2026-09-01\fix-the-ring")).is_none());
    assert!(UsageProject::new(Some("/Users/me/Documents/Codex/2026-09-01/fix-the-ring")).is_none());
    // Only a date-shaped component with a slug under it is scratch.
    assert!(UsageProject::new(Some("/Users/me/Documents/Codex/2026-09-01")).is_some());
    assert!(UsageProject::new(Some("/x/deepseek-harness/default-workspace")).is_none());
}

// MARK: - Model prices

fn price_with(input: f64, name: &str) -> ModelPrice {
    ModelPrice::new(input, input, None, None, Some(name))
}

fn alias_table() -> PriceTable {
    HashMap::from([
        ("grok-4.6".to_string(), price_with(1.0, "Grok 4.6")),
        ("grok-build-0.1".to_string(), price_with(9.0, "Grok Build")),
        ("kimi-k3".to_string(), price_with(2.0, "Kimi K3")),
        ("kimi-k2.6".to_string(), price_with(3.0, "Kimi K2.6")),
        ("gpt-5.6-sol".to_string(), price_with(4.0, "GPT-5.6 Sol")),
        ("MiniMax-M3".to_string(), price_with(5.0, "MiniMax-M3")),
    ])
}

fn name(id: &str) -> Option<String> {
    prices::price_for(id, &alias_table(), None).and_then(|p| p.name.clone())
}

#[test]
fn grok_build_tags_its_own_build_of_a_model() {
    assert_eq!(name("grok-4.6-build").as_deref(), Some("Grok 4.6"));
    // Its own model really is called that, and must not be stripped into something else.
    assert_eq!(name("grok-build-0.1").as_deref(), Some("Grok Build"));
}

#[test]
fn a_context_window_on_the_end_is_the_same_model_with_more_room() {
    assert_eq!(name("kimi-k3-256k").as_deref(), Some("Kimi K3"));
    // And it composes with the abbreviation below.
    assert_eq!(name("k3-256k").as_deref(), Some("Kimi K3"));
    assert_eq!(name("k3-1m").as_deref(), Some("Kimi K3"));
}

#[test]
fn kimis_cli_abbreviates_the_version() {
    assert_eq!(name("k3").as_deref(), Some("Kimi K3"));
    assert_eq!(name("k2p6").as_deref(), Some("Kimi K2.6"));
}

#[test]
fn devin_writes_the_version_with_dashes_and_an_effort_on_the_end() {
    assert_eq!(name("gpt-5-6-sol-medium").as_deref(), Some("GPT-5.6 Sol"));
    assert_eq!(name("gpt-5-6-sol").as_deref(), Some("GPT-5.6 Sol"));
    // A dash between two words is not a version separator.
    assert!(!prices::aliases_for("gpt-sol").contains(&"gpt.sol".to_string()));
}

#[test]
fn case_is_the_only_difference_for_minimax() {
    assert_eq!(name("minimax-m3").as_deref(), Some("MiniMax-M3"));
    assert_eq!(name("MINIMAX-M3").as_deref(), Some("MiniMax-M3"));
}

#[test]
fn a_model_nobody_publishes_a_price_for_stays_unpriced() {
    // The failure mode of a loose match is a model billed at another model's rate.
    assert_eq!(name("swe-2-high"), None);
    assert_eq!(name("codex-auto-review"), None);
    assert_eq!(name(""), None);
}

#[test]
fn repeated_exact_folded_alias_and_vendor_lookups_preserve_pricing_precedence() {
    let maker = ModelPrice::new(2.0, 4.0, Some(1.0), Some(3.0), Some("Maker"));
    let plan = ModelPrice::new(8.0, 9.0, None, None, Some("Plan"));
    let table: PriceTable = HashMap::from([
        ("gpt-5.6-sol".to_string(), maker.clone()),
        ("MiniMax-M3".to_string(), maker.clone()),
        ("opencode-go|gpt-5-6-sol-medium".to_string(), plan.clone()),
        ("opencode-go|plan-only".to_string(), plan.clone()),
        ("kilo|plan-only".to_string(), maker.clone()),
        ("opencode-go|kimi-k3".to_string(), plan.clone()),
    ]);
    let mut lookup = prices::ModelPriceLookup::new(&table);
    for _ in 0..3 {
        assert_eq!(lookup.price("gpt-5.6-sol", None), Some(&maker));
        assert_eq!(lookup.price("gpt-5-6-sol-medium", Some("opencode-go")), Some(&maker));
        assert_eq!(lookup.price("MINIMAX-M3", None), Some(&maker));
        assert_eq!(lookup.price("plan-only", None), None);
        assert_eq!(lookup.price("plan-only", Some("opencode-go")), Some(&plan));
        assert_eq!(lookup.price("PLAN-ONLY", Some("opencode-go")), Some(&plan));
        assert_eq!(lookup.price("plan-only", Some("kilo")), Some(&maker));
        assert_eq!(lookup.price("k3-256k", Some("opencode-go")), Some(&plan));
        assert_eq!(lookup.price("unpublished", Some("opencode-go")), None);
    }
}

#[test]
fn new_price_snapshots_cannot_inherit_a_hit_or_miss_from_an_older_table() {
    let maker = ModelPrice::new(2.0, 4.0, Some(1.0), Some(3.0), Some("Maker"));
    let plan = ModelPrice::new(8.0, 9.0, None, None, Some("Plan"));
    let old_table: PriceTable = HashMap::from([("priced".to_string(), maker.clone())]);
    let mut old = prices::ModelPriceLookup::new(&old_table);
    assert_eq!(old.price("priced", None), Some(&maker));
    assert_eq!(old.price("new", None), None);
    let next_table: PriceTable = HashMap::from([("priced".to_string(), plan.clone()), ("new".to_string(), maker.clone())]);
    let mut next = prices::ModelPriceLookup::new(&next_table);
    assert_eq!(next.price("priced", None), Some(&plan));
    assert_eq!(next.price("new", None), Some(&maker));
    let empty = PriceTable::new();
    assert_eq!(prices::ModelPriceLookup::new(&empty).price("priced", None), None);
    assert_eq!(old.price("priced", None), Some(&maker));
}

fn vendor_table() -> PriceTable {
    HashMap::from([
        ("deepseek-v4-flash".to_string(), ModelPrice::new(0.15, 0.6, Some(0.003), None, Some("DeepSeek V4 Flash"))),
        (prices::vendor_key("opencode-go", "deepseek-v4.1-flash"), ModelPrice::new(0.15, 0.6, Some(0.003), None, Some("DeepSeek V4.1 Flash"))),
        (prices::vendor_key("opencode-go", "deepseek-v4-flash"), ModelPrice::new(99.0, 99.0, None, None, Some("Wrong"))),
    ])
}

#[test]
fn a_model_only_the_plan_vendor_publishes_is_priced_by_that_vendor() {
    let table = vendor_table();
    let price = prices::price_for("deepseek-v4.1-flash", &table, Some("opencode-go")).unwrap();
    assert_eq!((price.input, price.output), (0.15, 0.6));
}

#[test]
fn without_a_vendor_the_same_model_stays_unpriced() {
    assert!(prices::price_for("deepseek-v4.1-flash", &vendor_table(), None).is_none());
}

#[test]
fn a_first_party_price_always_wins_over_the_plan_vendors() {
    let table = vendor_table();
    let price = prices::price_for("deepseek-v4-flash", &table, Some("opencode-go")).unwrap();
    assert_eq!(price.input, 0.15, "the vendor's 99 must not be reachable");
}

#[test]
fn a_vendor_that_sells_nothing_for_the_model_is_still_none() {
    assert!(prices::price_for("no-such-model", &vendor_table(), Some("opencode-go")).is_none());
}

#[test]
fn neither_transcript_agent_names_a_plan_vendor() {
    assert_eq!(ClaudeCode.price_vendor(), None);
    assert_eq!(Codex.price_vendor(), None);
}

#[test]
fn plan_prices_survive_the_record_day_model_and_session_pricing_chain() {
    let at = Utc.timestamp_opt(1_789_560_000, 0).unwrap();
    let tally = TokenTally::new(1_000_000, 1_000_000, 1_000_000, 1_000_000);
    let table: PriceTable = HashMap::from([(prices::vendor_key("cline-pass", "plan-only"), ModelPrice::new(2.0, 3.0, Some(0.5), Some(1.0), Some("Plan model")))]);
    let records = [AgentUsageRecord::new(at, "plan-only", tally.clone()).session("s")];
    let ledger = build_ledger(&records, &table, "cline", Some("cline-pass"), &local(), super::ledger::Origin::LocalTranscripts);
    let ledgers = HashMap::from([(ClaudeCode, ledger.clone())]);
    let summary = SpendSummary::of(&ledgers, Some(1), at, &local());
    let detail = ModelSpendSummary::of(&ledgers, "Plan model", Some(1), at, &local());
    assert_eq!(summary.cost, 6.5);
    assert_eq!(summary.unpriced_tokens, 0);
    assert!(ledger.unpriced_models.is_empty());
    assert_eq!(ledger.slots[0].cost, 6.5);
    assert_eq!(ledger.sessions[0].cost, 6.5);
    assert_eq!(ledger.sessions[0].slots[0].cost, 6.5);
    assert_eq!(detail.cost_breakdown, Some(TokenCost { input: 2.0, cache_write: 1.0, cache_read: 0.5, output: 3.0 }));

    let without = build_ledger(&[AgentUsageRecord::new(at, "plan-only", tally.clone())], &table, "pi", None, &local(), super::ledger::Origin::LocalTranscripts);
    assert_eq!(without.days[0].unpriced_tokens, tally.total());
}

// MARK: - The price table on disk

const START: i64 = 1_800_000_000;
const DAY: i64 = 86_400;
const RETRY: i64 = 300;

fn table(input: f64) -> PriceTable {
    HashMap::from([("probe-model".to_string(), ModelPrice::new(input, 2.0, None, None, Some("Probe")))])
}

fn save(prices: &PriceTable, at: i64, root: &Path, version: u32) {
    let cache = PriceCache { fetched_at: Utc.timestamp_opt(at, 0).unwrap(), prices: prices.clone() };
    std::fs::write(root.join(format!("model-prices-{version}.json")), serde_json::to_vec(&cache).unwrap()).unwrap();
}

struct Clock(Arc<Mutex<i64>>);

impl Clock {
    fn new() -> Self {
        Clock(Arc::new(Mutex::new(START)))
    }
    fn now(&self) -> DateTime<Utc> {
        Utc.timestamp_opt(*self.0.lock().unwrap(), 0).unwrap()
    }
    fn advance(&self, seconds: i64) {
        *self.0.lock().unwrap() += seconds;
    }
}

struct Downloads {
    replies: Vec<Option<PriceTable>>,
    calls: AtomicUsize,
}

impl Downloads {
    fn new(replies: Vec<Option<PriceTable>>) -> Arc<Self> {
        Arc::new(Downloads { replies, calls: AtomicUsize::new(0) })
    }
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

fn prices_with(root: &Path, clock: &Clock, downloads: &Arc<Downloads>) -> ModelPrices {
    let now = clock.0.clone();
    let downloads = downloads.clone();
    ModelPrices::with(
        root.to_path_buf(),
        Arc::new(move || Utc.timestamp_opt(*now.lock().unwrap(), 0).unwrap()),
        Arc::new(move || {
            let downloads = downloads.clone();
            Box::pin(async move {
                let index = downloads.calls.fetch_add(1, Ordering::SeqCst);
                downloads.replies.get(index).cloned().flatten()
            })
        }),
    )
}

#[tokio::test]
async fn a_disk_table_expires_at_its_original_fetch_time_including_after_loading_into_memory() {
    let root = tempfile::tempdir().unwrap();
    let clock = Clock::new();
    let (old, new) = (table(1.0), table(9.0));
    save(&old, START - 23 * 3_600, root.path(), 5);
    let downloads = Downloads::new(vec![Some(new.clone())]);
    let prices = prices_with(root.path(), &clock, &downloads);

    assert_eq!(prices.prices().await, old);
    clock.advance(3_599);
    assert_eq!(prices.prices().await, old);
    assert_eq!(downloads.calls(), 0);
    clock.advance(1);
    assert_eq!(prices.prices().await, new);
    assert_eq!(downloads.calls(), 1);
    let saved = prices::read_cache(root.path(), false).unwrap();
    assert_eq!(saved.fetched_at, clock.now());
    assert_eq!(saved.prices, new);

    // Relaunching reuses the download without granting it another day.
    let relaunched = prices_with(root.path(), &clock, &downloads);
    assert_eq!(relaunched.prices().await, new);
    assert_eq!(downloads.calls(), 1);
}

#[tokio::test]
async fn a_downloaded_table_also_expires_while_the_same_instance_stays_alive() {
    let root = tempfile::tempdir().unwrap();
    let clock = Clock::new();
    let (old, new) = (table(1.0), table(9.0));
    let downloads = Downloads::new(vec![Some(old.clone()), Some(new.clone())]);
    let prices = prices_with(root.path(), &clock, &downloads);
    assert_eq!(prices.prices().await, old);
    clock.advance(DAY - 1);
    assert_eq!(prices.prices().await, old);
    assert_eq!(downloads.calls(), 1);
    clock.advance(1);
    assert_eq!(prices.prices().await, new);
    assert_eq!(downloads.calls(), 2);
}

#[tokio::test]
async fn offline_fallback_can_recover_without_a_relaunch_including_a_fresh_table_without_tiers() {
    for version in [4, 5] {
        let root = tempfile::tempdir().unwrap();
        let clock = Clock::new();
        let (old, new) = (table(1.0), table(9.0));
        let original = if version == 4 { START } else { START - 3 * DAY };
        save(&old, original, root.path(), version);
        let downloads = Downloads::new(vec![None, Some(new.clone())]);
        let prices = prices_with(root.path(), &clock, &downloads);
        assert_eq!(prices.prices().await, old);
        assert_eq!(downloads.calls(), 1);
        assert_eq!(prices::read_cache(root.path(), true).unwrap().fetched_at.timestamp(), original);
        if version == 4 {
            assert!(prices::read_cache(root.path(), false).is_none());
        }
        clock.advance(RETRY - 1);
        assert_eq!(prices.prices().await, old);
        assert_eq!(downloads.calls(), 1);
        clock.advance(1);
        assert_eq!(prices.prices().await, new);
        assert_eq!(downloads.calls(), 2);
        let saved = prices::read_cache(root.path(), false).unwrap();
        assert_eq!(saved.fetched_at, clock.now());
        assert_eq!(saved.prices, new);
    }
}

#[tokio::test]
async fn an_offline_first_run_retries_without_writing_an_empty_price_table() {
    let root = tempfile::tempdir().unwrap();
    let clock = Clock::new();
    let new = table(9.0);
    let downloads = Downloads::new(vec![None, Some(new.clone())]);
    let prices = prices_with(root.path(), &clock, &downloads);
    assert!(prices.prices().await.is_empty());
    assert!(prices::read_cache(root.path(), false).is_none());
    clock.advance(RETRY - 1);
    assert!(prices.prices().await.is_empty());
    assert_eq!(downloads.calls(), 1);
    clock.advance(1);
    assert_eq!(prices.prices().await, new);
    assert_eq!(downloads.calls(), 2);
}

#[tokio::test]
async fn a_failed_refresh_keeps_the_last_download_and_its_timestamp() {
    let root = tempfile::tempdir().unwrap();
    let clock = Clock::new();
    let (old, new) = (table(1.0), table(9.0));
    let downloads = Downloads::new(vec![Some(old.clone()), None, None, Some(new.clone())]);
    let prices = prices_with(root.path(), &clock, &downloads);
    assert_eq!(prices.prices().await, old);
    clock.advance(DAY);
    assert_eq!(prices.prices().await, old);
    assert_eq!(downloads.calls(), 2);
    clock.advance(RETRY);
    assert_eq!(prices.prices().await, old);
    assert_eq!(downloads.calls(), 3);
    assert_eq!(prices::read_cache(root.path(), false).unwrap().fetched_at.timestamp(), START);
    clock.advance(RETRY);
    assert_eq!(prices.prices().await, new);
    assert_eq!(downloads.calls(), 4);
}

#[tokio::test]
async fn concurrent_readers_share_a_refresh_and_date_the_result_when_it_arrives() {
    let root = tempfile::tempdir().unwrap();
    let clock = Clock::new();
    let (old, new) = (table(1.0), table(9.0));
    save(&old, START - DAY, root.path(), 5);
    let downloads = Downloads::new(vec![Some(new.clone())]);
    let prices = prices_with(root.path(), &clock, &downloads);
    let first = tokio::spawn({
        let prices = prices.clone();
        async move { prices.prices().await }
    });
    let readers: Vec<_> = (0..12)
        .map(|_| {
            let prices = prices.clone();
            tokio::spawn(async move { prices.prices().await })
        })
        .collect();
    assert_eq!(first.await.unwrap(), new);
    for reader in readers {
        assert_eq!(reader.await.unwrap(), new);
    }
    assert_eq!(downloads.calls(), 1);
    assert_eq!(prices::read_cache(root.path(), false).unwrap().fetched_at, clock.now());
}

#[test]
fn a_price_cache_without_tiers_is_only_an_offline_fallback_after_upgrade() {
    let root = tempfile::tempdir().unwrap();
    let table = vendor_table();
    let previous = PriceCache { fetched_at: Utc::now(), prices: HashMap::from([("direct".to_string(), table["deepseek-v4-flash"].clone())]) };
    std::fs::write(root.path().join("model-prices-4.json"), serde_json::to_vec(&previous).unwrap()).unwrap();
    assert!(prices::read_cache(root.path(), false).is_none());
    assert_eq!(prices::read_cache(root.path(), true).unwrap().prices, previous.prices);
    let current = PriceCache { fetched_at: Utc::now(), prices: table };
    std::fs::write(root.path().join("model-prices-5.json"), serde_json::to_vec(&current).unwrap()).unwrap();
    assert_eq!(prices::read_cache(root.path(), false).unwrap().prices, current.prices);
    assert_eq!(prices::read_cache(root.path(), true).unwrap().prices, current.prices);
}

#[test]
fn models_dev_documents_become_a_namespaced_table() {
    let document = json!({
        "anthropic": {"models": {
            "claude-x": {"name": "Claude X", "cost": {"input": 3, "output": 15, "cache_read": 0.3, "cache_write": 3.75}},
            "no-cost": {"name": "No cost"}
        }},
        "openai": {"models": {"claude-x": {"name": "Shadow", "cost": {"input": 1, "output": 1}}, "gpt-y": {"cost": {"input": 5, "output": 30}}}},
        "opencode-go": {"models": {"claude-x": {"cost": {"input": 7, "output": 7}}, "plan-only": {"name": "Plan", "cost": {"input": 2, "output": 3}}}},
    });
    let table = prices::parse_models_dev(&document).unwrap();
    // The first provider in the list wins a shared id; the plan vendor's copy of a first-party id
    // is not kept.
    assert_eq!(table["claude-x"].name.as_deref(), Some("Claude X"));
    assert_eq!(table["claude-x"].cache_write, Some(3.75));
    assert!(!table.contains_key("no-cost"));
    assert_eq!(table["gpt-y"].input, 5.0);
    assert!(!table.contains_key("opencode-go|claude-x"));
    assert_eq!(table["opencode-go|plan-only"].output, 3.0);
    assert!(prices::parse_models_dev(&json!({})).is_none());
}
