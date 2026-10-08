//! Ported from upstream AgentUsageRecordTests: decoded incremental records in, one ledger out,
//! through the same pricing path every transcript reader uses. Every date is fixed and computed on
//! a UTC calendar, so the day keys cannot depend on the machine's time zone.

use std::collections::HashMap;

use chrono::{DateTime, TimeZone, Utc};

use super::agent::SpendAgent::{ClaudeCode, Codex};
use super::calendar::Calendar;
use super::ledger::{Ledger, Origin, Session, SessionDay};
use super::model_summary::ModelSpendSummary;
use super::prices::{ModelPrice, PriceTable};
use super::record::{build_ledger, AgentUsageRecord};
use super::summary::SpendSummary;
use super::tally::TokenTally;

pub(crate) fn prices() -> PriceTable {
    HashMap::from([("priced".to_string(), ModelPrice::new(1_000.0, 10_000.0, Some(100.0), Some(1_000.0), Some("Priced")))])
}

pub(crate) fn calendar() -> Calendar {
    Calendar::utc(2)
}

pub(crate) fn now() -> DateTime<Utc> {
    Utc.timestamp_opt(1_789_372_800, 0).unwrap()
}

pub(crate) fn at(days_ago: i64, hour: i64) -> DateTime<Utc> {
    let cal = calendar();
    cal.start_of_day(now()) - chrono::Duration::days(days_ago) + chrono::Duration::hours(hour)
}

pub(crate) fn tally(input: i64) -> TokenTally {
    TokenTally::new(input, 0, 0, 0)
}

pub(crate) fn rec(model: &str, tally: TokenTally, date: DateTime<Utc>) -> AgentUsageRecord {
    AgentUsageRecord::new(date, model, tally)
}

pub(crate) fn build(records: &[AgentUsageRecord], namespace: &str) -> Ledger {
    build_ledger(records, &prices(), namespace, None, &calendar(), Origin::LocalTranscripts)
}

fn slot_sums(session: &Session) -> (i64, f64) {
    session.slots.iter().fold((0, 0.0), |(t, c), s| (t + s.tokens, c + s.cost))
}

fn tokens(ledger: &Ledger) -> i64 {
    ledger.days.iter().map(|d| d.tokens).sum()
}

#[test]
fn incremental_records_roll_into_the_day_keys_and_each_sessions_own_buckets() {
    let mut a = rec("priced", tally(100), at(0, 9)).session("s1");
    a.session_name = Some("one".into());
    let b = rec("priced", tally(50), at(0, 10)).session("s2");
    let c = rec("priced", tally(900), at(1, 11)).session("s1");
    let ledger = build(&[a, b, c], "opencode");

    assert_eq!(ledger.days.len(), 2);
    assert_eq!(tokens(&ledger), 1050);
    assert!((ledger.days.iter().map(|d| d.cost).sum::<f64>() - 1.05).abs() < 1e-9);

    let first = ledger.sessions.iter().find(|s| s.id == "opencode#s1").unwrap();
    assert_eq!(first.tokens, 1000);
    assert!((first.cost - 1.0).abs() < 1e-9);
    assert_eq!(first.slots.len(), 2);
    // The session's buckets must add up to its own totals, or a span would be windowing a
    // different number than the row shows.
    let (slot_tokens, slot_cost) = slot_sums(first);
    assert_eq!(slot_tokens, first.tokens);
    assert!((slot_cost - first.cost).abs() < 1e-9);

    // Two session ids are two sessions, not one.
    assert_eq!(ledger.sessions.len(), 2);
    assert!(ledger.sessions.iter().any(|s| s.id == "opencode#s2"));
}

#[test]
fn only_an_explicit_id_folds_a_message_identical_work_still_counts() {
    let when = at(0, 9);
    let with_id = |id: Option<&str>| {
        let mut r = rec("priced", tally(100), when).session("s1");
        r.deduplication_id = id.map(str::to_string);
        r
    };
    let ledger = build(&[with_id(Some("m1")), with_id(Some("m1")), with_id(Some("m2")), with_id(None), with_id(None)], "a");
    // m1 once, m2 once, the two unidentified twice.
    assert_eq!(tokens(&ledger), 400);
    assert_eq!(ledger.sessions[0].tokens, 400);
}

#[test]
fn empty_models_and_empty_tallies_are_skipped_an_unpriced_model_is_counted_not_costed() {
    let when = at(0, 9);
    let ledger = build(
        &[rec("", tally(100), when), rec("priced", TokenTally::default(), when), rec("mystery", tally(10), when)],
        "a",
    );
    assert_eq!(tokens(&ledger), 10);
    assert_eq!(ledger.days.iter().map(|d| d.cost).sum::<f64>(), 0.0);
    assert_eq!(ledger.days[0].unpriced_tokens, 10);
    assert_eq!(ledger.unpriced_models, vec!["mystery".to_string()]);
    // No session was named, so none is invented.
    assert!(ledger.sessions.is_empty());
}

#[test]
fn no_usable_records_is_an_empty_ledger_not_a_zero() {
    let none = build(&[], "a");
    assert!(none.days.is_empty());
    assert!(none.sessions.is_empty());
    assert!(build(&[rec("", tally(5), Utc::now())], "a").days.is_empty());
}

#[test]
fn a_session_keeps_only_the_name_title_and_project_it_was_given() {
    let mut record = rec("priced", tally(10), at(0, 9)).session("id-1").project("Pulse");
    record.session_name = Some("slug".into());
    record.title = Some("Fix the ring".into());
    let ledger = build(&[record], "opencode");
    let session = &ledger.sessions[0];
    assert_eq!(session.id, "opencode#id-1");
    assert_eq!(session.name, "slug");
    assert_eq!(session.title.as_deref(), Some("Fix the ring"));
    assert_eq!(session.project.as_ref().unwrap().name, "Pulse");
}

#[test]
fn a_record_with_no_session_id_counts_toward_totals_and_creates_no_session_row() {
    let ledger = build(&[rec("priced", tally(100), at(0, 9)), rec("priced", tally(900), at(1, 10))], "a");
    assert_eq!(tokens(&ledger), 1000);
    assert!(ledger.sessions.is_empty());
}

#[test]
fn built_records_reconcile_through_the_summaries() {
    let mut first = rec("priced", TokenTally::new(100, 20, 30, 40), at(0, 9)).session("s1");
    first.session_name = Some("one".into());
    let second = rec("priced", tally(900), at(1, 10)).session("s1");
    let ledger = build(&[first, second], "claudeCode");
    let ledgers = HashMap::from([(ClaudeCode, ledger)]);

    let summary = SpendSummary::of(&ledgers, None, now(), &calendar());
    assert_eq!(summary.tokens, 1090);
    // 100*.001 + 20*.001 + 30*.0001 + 40*.01 + 900*.001 = 1.423
    assert!((summary.cost - 1.423).abs() < 1e-9);
    assert_eq!(summary.tally, TokenTally::new(1000, 20, 30, 40));
    assert_eq!(summary.sessions.len(), 1);
    assert_eq!(summary.sessions[0].session.tokens, 1090);
    assert!((summary.sessions[0].session.cost - 1.423).abs() < 1e-9);

    let model = ModelSpendSummary::of(&ledgers, "Priced", None, now(), &calendar());
    assert_eq!(model.tokens, 1090);
    assert_eq!(model.tally, Some(TokenTally::new(1000, 20, 30, 40)));
    assert!((model.cost().unwrap() - 1.423).abs() < 1e-9);
    assert_eq!(model.days.len(), 2);
    assert_eq!(model.hours.unwrap(), HashMap::from([(9, 190), (10, 900)]));

    // A one-day window keeps only today's record and the session's own in-window half: the
    // totals above and the row agree.
    let today = SpendSummary::of(&ledgers, Some(1), now(), &calendar());
    assert_eq!(today.tokens, 190);
    assert_eq!(today.sessions[0].session.tokens, 190);
}

#[test]
fn blank_values_are_absent_not_shared_keys() {
    let when = at(0, 9);
    let blank_id = |id: &str| {
        let mut r = rec("priced", tally(100), when);
        r.deduplication_id = Some(id.to_string());
        r
    };
    let ledger = build(
        &[
            // A blank model names nothing and is skipped.
            rec("", tally(100), when),
            rec("   ", tally(100), when),
            // Blank dedup ids are no ids: the two are not folded together.
            blank_id(""),
            blank_id("   "),
            // A blank session id counts but makes no session.
            rec("priced", tally(100), when).session(""),
        ],
        "a",
    );
    assert_eq!(tokens(&ledger), 300);
    assert!(ledger.sessions.is_empty());
}

#[test]
fn an_explicit_deduplication_id_still_folds_a_blank_one_is_each_its_own_record() {
    let when = at(0, 9);
    let with = |id: &str| {
        let mut r = rec("priced", tally(100), when).session("s1");
        r.deduplication_id = Some(id.to_string());
        r
    };
    let ledger = build(&[with("m1"), with("m1"), with("  "), with("  ")], "a");
    assert_eq!(tokens(&ledger), 300);
}

#[test]
fn blank_session_metadata_is_none_and_the_id_is_the_names_fallback() {
    let mut record = rec("priced", tally(10), at(0, 9)).session("id-1").project(" ");
    record.session_name = Some("  ".into());
    record.title = Some("\n".into());
    let ledger = build(&[record], "a");
    let session = &ledger.sessions[0];
    assert_eq!(session.name, "id-1");
    assert_eq!(session.title, None);
    assert_eq!(session.project, None);
}

#[test]
fn a_bare_total_is_counted_never_invented_into_a_kind_and_never_priced() {
    let ledger = build(&[rec("priced", TokenTally::default(), at(0, 9)).unclassified(500).session("s1")], "a");
    let day = ledger.days.iter().find(|d| d.tokens > 0).unwrap();
    assert_eq!(day.tokens, 500);
    assert_eq!(day.unpriced_tokens, 500);
    assert_eq!(day.models["priced"], 500);
    assert_eq!(day.model_unclassified_tokens["priced"], 500);
    // Never placed into a kind, and never costed even though the model has a published price.
    assert!(!day.model_tallies.contains_key("priced"));
    assert_eq!(day.tally.total(), 0);
    assert_eq!(day.cost, 0.0);
    assert!(!day.model_costs.contains_key("priced"));

    let session = &ledger.sessions[0];
    assert_eq!(session.tokens, 500);
    assert_eq!(session.cost, 0.0);
    // An event record's unknown tokens still reach their hour bucket...
    assert_eq!(ledger.slots[0].tokens, 500);

    // ...and the drill-down preserves the explicit unclassified count, without displaying
    // measured input/output zeroes or money. It is reached by the published name, which the
    // unknown-only id now takes even though its tokens were never priced.
    assert_eq!(ledger.model_names["priced"], "Priced");
    let model = ModelSpendSummary::of(&HashMap::from([(Codex, ledger)]), "Priced", Some(7), now(), &calendar());
    assert_eq!(model.tokens, 500);
    assert_eq!(model.tally, Some(TokenTally::default()));
    assert_eq!(model.unclassified_tokens, 500);
    assert_eq!(model.days.last().unwrap().unclassified_tokens, 500);
    assert!(model.days.last().unwrap().classified_tally().is_none());
    assert_eq!(model.cost(), None);
    assert_eq!(model.unpriced_tokens, 500);
}

#[test]
fn a_mixed_model_keeps_its_classified_cost_and_counts_the_remainder_unpriced() {
    let ledger = build(
        &[
            rec("priced", tally(100), at(0, 9)).session("s1"),
            rec("priced", TokenTally::default(), at(0, 9)).unclassified(50).session("s1"),
        ],
        "a",
    );
    let day = ledger.days.iter().find(|d| d.tokens > 0).unwrap();
    assert_eq!(day.tokens, 150);
    assert_eq!(day.models["priced"], 150);
    assert_eq!(day.model_tallies["priced"], tally(100));
    assert_eq!(day.model_unclassified_tokens["priced"], 50);
    // 100 input at $1000/M.
    assert!((day.model_costs["priced"].total() - 0.1).abs() < 1e-12);

    let model = ModelSpendSummary::of(&HashMap::from([(Codex, ledger)]), "Priced", Some(7), now(), &calendar());
    assert_eq!(model.tokens, 150);
    assert!((model.cost().unwrap() - 0.1).abs() < 1e-12);
    assert_eq!(model.unpriced_tokens, 50);
    // The classified subset and the explicit remainder both survive.
    assert_eq!(model.tally, Some(tally(100)));
    assert_eq!(model.unclassified_tokens, 50);
    assert_eq!(model.days.last().unwrap().classified_tally(), Some(&tally(100)));
    assert_eq!(model.days.last().unwrap().unclassified_tokens, 50);
}

#[test]
fn a_negative_or_overflowing_record_is_skipped_whole_the_rest_of_the_run_is_kept() {
    let when = at(0, 9);
    let ledger = build(
        &[
            // A legitimate record that must survive.
            rec("priced", tally(10), when),
            // A record whose four kinds do not fit: it must not saturate the whole ledger.
            rec("priced", TokenTally::new(i64::MAX, 0, 0, 1), when),
            // A negative unclassified remainder: it must not be clamped to zero and counted.
            rec("priced", tally(5), when).unclassified(-3),
        ],
        "a",
    );
    // The two hostile records are gone, and the valid ten is exactly what is left.
    assert_eq!(tokens(&ledger), 10);
    assert!((ledger.days.iter().map(|d| d.cost).sum::<f64>() - 0.01).abs() < 1e-12);
    assert!(ledger.days.iter().all(|d| d.tokens != i64::MAX));
}

#[test]
fn an_unknown_only_raw_id_takes_its_published_name_but_never_its_price() {
    let ledger = build(&[rec("priced", TokenTally::default(), at(0, 9)).unclassified(500)], "a");
    // The name is a grouping, not a price: the model is still unknown-only for money, and it is
    // not called "no published price" either.
    assert_eq!(ledger.model_names["priced"], "Priced");
    assert!(ledger.unpriced_models.is_empty());
    let day = ledger.days.iter().find(|d| d.tokens > 0).unwrap();
    assert_eq!(day.cost, 0.0);
    assert!(!day.model_costs.contains_key("priced"));
}

#[test]
fn an_unknown_only_raw_id_with_no_rate_at_all_is_named_as_unpriced() {
    let ledger = build(&[rec("mystery", TokenTally::default(), at(0, 9)).unclassified(500)], "a");
    assert!(!ledger.model_names.contains_key("mystery"));
    assert_eq!(ledger.unpriced_models, vec!["mystery".to_string()]);
}

#[test]
fn broken_data_without_unclassified_metadata_is_not_mistaken_for_a_legitimate_partial() {
    // The day accounts 500 but only 100 is classified and nothing says the rest is unclassified,
    // so the split is broken and nothing is priced.
    let mut day = super::ledger::LedgerDay::new(calendar().start_of_day(at(0, 9)), 500, 0.0, 0, HashMap::from([("alpha".to_string(), 500)]));
    day.tally = tally(100);
    day.model_tallies = HashMap::from([("alpha".to_string(), tally(100))]);
    let broken = Ledger { days: vec![day], model_names: HashMap::from([("alpha".to_string(), "Alpha".to_string())]), ..Ledger::empty() };
    let summary = ModelSpendSummary::of(&HashMap::from([(Codex, broken)]), "Alpha", Some(7), now(), &calendar());
    assert_eq!(summary.tokens, 500);
    assert_eq!(summary.tally, None);
    assert_eq!(summary.cost(), None);
    assert_eq!(summary.unpriced_tokens, 500);
}

#[test]
fn imported_records_enter_the_spend_summaries_provider_statistics_do_not() {
    let imported = build_ledger(&[rec("priced", tally(100), at(0, 9))], &prices(), "a", None, &calendar(), Origin::ImportedRecords);
    assert_eq!(imported.origin, Origin::ImportedRecords);
    assert!(imported.origin.supports_token_spend());

    let combined = SpendSummary::of(&HashMap::from([(Codex, imported.clone())]), Some(7), now(), &calendar());
    assert_eq!(combined.tokens, 100);
    assert!((combined.cost - 0.1).abs() < 1e-12);
    assert!(!ModelSpendSummary::of(&HashMap::from([(Codex, imported.clone())]), "Priced", Some(7), now(), &calendar()).is_empty());

    let mut statistics = imported;
    statistics.origin = Origin::ProviderStatistics;
    assert!(!statistics.origin.supports_token_spend());
    assert!(SpendSummary::of(&HashMap::from([(Codex, statistics.clone())]), Some(7), now(), &calendar()).is_empty());
    assert!(ModelSpendSummary::of(&HashMap::from([(Codex, statistics)]), "Priced", Some(7), now(), &calendar()).is_empty());
}

#[test]
fn origin_is_a_serialised_string() {
    assert_eq!(serde_json::to_string(&Origin::ImportedRecords).unwrap(), "\"importedRecords\"");
    assert_eq!(serde_json::from_str::<Origin>("\"importedRecords\"").unwrap(), Origin::ImportedRecords);
}

#[test]
fn an_aggregate_record_keeps_its_day_including_in_its_session_but_no_hour_bucket() {
    let ledger = build(&[rec("priced", tally(100), at(0, 9)).unclassified(400).aggregate(true).session("s1")], "a");
    assert!(ledger.has_aggregate_timing);
    let day = ledger.days.iter().find(|d| d.tokens > 0).unwrap();
    assert_eq!(day.tokens, 500);
    assert_eq!(day.models["priced"], 500);
    assert_eq!(day.model_tallies["priced"], tally(100));
    assert_eq!(day.model_unclassified_tokens["priced"], 400);
    // No quarter-hour bucket was fabricated for the aggregate work.
    assert!(ledger.slots.is_empty());

    let session = &ledger.sessions[0];
    assert_eq!(session.tokens, 500);
    // Known calendar dates survive without inventing an hour series.
    assert!(session.slots.is_empty());
    assert_eq!(session.days, vec![SessionDay { date: day.date, tokens: 500, cost: session.cost, unpriced_tokens: 400 }]);
    assert!((session.cost - 0.1).abs() < 1e-12);

    let ledgers = HashMap::from([(Codex, ledger)]);
    let combined = SpendSummary::of(&ledgers, Some(7), now(), &calendar());
    assert_eq!(combined.tokens, 500);
    assert!(combined.has_aggregate_timing);

    let model = ModelSpendSummary::of(&ledgers, "Priced", Some(7), now(), &calendar());
    assert_eq!(model.tokens, 500);
    assert!(model.has_aggregate_timing);
    assert!(model.hours.is_none());
    assert!((model.cost().unwrap() - 0.1).abs() < 1e-12);
    assert_eq!(model.unpriced_tokens, 400);
    assert_eq!(model.tally, Some(tally(100)));
    assert_eq!(model.unclassified_tokens, 400);
    assert_eq!(combined.unclassified_tokens, 400);
    assert!(combined.has_token_breakdown);
}

#[test]
fn aggregate_and_mixed_timing_sessions_window_by_their_known_days() {
    for only_aggregate in [true, false] {
        let ledger = build(
            &[
                rec("priced", tally(900), at(1, 9)).aggregate(true).session("s").project("Pulse"),
                rec("priced", tally(100), at(0, 9)).unclassified(50).aggregate(only_aggregate).session("s").project("Pulse"),
            ],
            "a",
        );
        assert!(ledger.sessions[0].slots.is_empty());
        assert_eq!(ledger.sessions[0].days.len(), 2);
        let ledgers = HashMap::from([(Codex, ledger)]);
        let today = SpendSummary::of(&ledgers, Some(1), now(), &calendar());
        assert_eq!(today.tokens, 150);
        assert_eq!(today.sessions[0].session.tokens, today.tokens);
        assert_eq!(today.projects[0].tokens, today.tokens);
        assert_eq!(today.sessions[0].session.cost, today.cost);
        assert_eq!(today.projects[0].cost, today.cost);
        assert!(today.has_aggregate_timing);
        let all = SpendSummary::of(&ledgers, None, now(), &calendar());
        assert_eq!(all.sessions[0].session.tokens, 1050);
    }
}

#[test]
fn a_partial_record_marks_the_ledger_and_both_summaries_without_changing_a_number() {
    let mut record = rec("priced", tally(100), at(0, 9)).session("s1");
    record.is_partial = true;
    let ledger = build(&[record], "a");
    assert!(ledger.has_partial_counts);
    // The reported count is kept exactly; the flag invents no remainder.
    assert_eq!(tokens(&ledger), 100);
    assert!((ledger.days.iter().map(|d| d.cost).sum::<f64>() - 0.1).abs() < 1e-12);

    let ledgers = HashMap::from([(Codex, ledger)]);
    let combined = SpendSummary::of(&ledgers, Some(7), now(), &calendar());
    assert!(combined.has_partial_counts);
    assert_eq!(combined.tokens, 100);
    let model = ModelSpendSummary::of(&ledgers, "Priced", Some(7), now(), &calendar());
    assert!(model.has_partial_counts);
    assert_eq!(model.tokens, 100);
    assert!((model.cost().unwrap() - 0.1).abs() < 1e-12);
}

#[test]
fn a_skipped_partial_record_makes_nothing_partial_and_produces_no_usage() {
    // An empty model and a zero-total record are skipped whole, so a flag on them must not
    // manufacture a partial ledger or any usage.
    let mut empty_model = rec("", tally(100), at(0, 9));
    empty_model.is_partial = true;
    let none = build(&[empty_model], "a");
    assert!(!none.has_partial_counts);
    assert!(none.days.is_empty());

    let mut zero = rec("priced", TokenTally::default(), at(0, 9));
    zero.is_partial = true;
    let zero = build(&[zero], "a");
    assert!(!zero.has_partial_counts);
    let ledgers = HashMap::from([(Codex, zero)]);
    assert!(!SpendSummary::of(&ledgers, Some(7), now(), &calendar()).has_partial_counts);
    assert!(!ModelSpendSummary::of(&ledgers, "Priced", Some(7), now(), &calendar()).has_partial_counts);
}
