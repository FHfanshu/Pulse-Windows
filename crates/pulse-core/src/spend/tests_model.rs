//! Ported from upstream ModelSpendSummaryTests: one model's drill-down, reached the way
//! production reaches it: a reader's quarter-hour buckets through the shared pricing pass, then
//! `ModelSpendSummary::of`. Time and calendar are fixed rather than taken from the machine.
//!
//! The agents are Claude Code and Codex; the upstream tests' OpenCode and Kimi roles are played
//! by them (the summary only cares that they are two agents).

use std::collections::HashMap;

use chrono::{DateTime, TimeZone, Utc};

use super::agent::SpendAgent::{self, ClaudeCode, Codex};
use super::calendar::Calendar;
use super::ledger::{price_buckets, slot_key, Buckets, Ledger, LedgerDay, Origin, Slot};
use super::model_summary::{ModelDay, ModelDayColumn, ModelSpendSummary};
use super::prices::{ModelPrice, PriceTable};
use super::summary::SpendSummary;
use super::tally::{TokenCost, TokenTally};

fn calendar() -> Calendar {
    Calendar::utc(2)
}

fn now() -> DateTime<Utc> {
    Utc.timestamp_opt(1_789_372_800, 0).unwrap()
}

fn today() -> DateTime<Utc> {
    calendar().start_of_day(now())
}

fn at(days_ago: i64, hour: i64) -> DateTime<Utc> {
    calendar().add_days(today(), -days_ago) + chrono::Duration::hours(hour)
}

fn p(input: f64, output: f64, cache_read: Option<f64>, cache_write: Option<f64>, name: &str) -> ModelPrice {
    ModelPrice::new(input, output, cache_read, cache_write, Some(name))
}

/// The same price table a production reader is handed: two raw ids that publish one display name,
/// and one unrelated model.
fn prices() -> PriceTable {
    HashMap::from([
        ("alpha".to_string(), p(1.0, 10.0, Some(0.1), Some(1.0), "Alpha")),
        ("alpha-1".to_string(), p(1.0, 10.0, Some(0.1), Some(1.0), "Alpha")),
        ("alpha-2".to_string(), p(1.0, 10.0, Some(0.1), Some(1.0), "Alpha")),
        ("beta".to_string(), p(2.0, 20.0, Some(0.0), Some(0.0), "Beta")),
    ])
}

/// Rates chosen so a million tokens of each kind costs its rate in dollars. Distinct on purpose:
/// a per-kind figure read from the wrong kind is visible.
fn rate_prices() -> PriceTable {
    HashMap::from([
        ("rates".to_string(), p(1.0, 2.0, Some(3.0), Some(4.0), "Rates")),
        // Cache rates absent, so both fall back to the input rate.
        ("no-cache".to_string(), p(5.0, 6.0, None, None, "No Cache")),
        // A real published zero: a reading of $0, not a missing price.
        ("free".to_string(), p(0.0, 0.0, Some(0.0), Some(0.0), "Free")),
    ])
}

/// Two raw ids, one display name, rates a hundred-fold apart.
fn shared_prices() -> PriceTable {
    HashMap::from([
        ("cheap".to_string(), p(1.0, 0.0, Some(0.0), Some(0.0), "Shared")),
        ("dear".to_string(), p(100.0, 0.0, Some(0.0), Some(0.0), "Shared")),
    ])
}

fn million() -> TokenTally {
    TokenTally::new(1_000_000, 1_000_000, 1_000_000, 1_000_000)
}

type Entry<'a> = (DateTime<Utc>, Vec<(&'a str, TokenTally)>);

fn ledger(entries: &[Entry], prices: &PriceTable) -> Ledger {
    let mut buckets = Buckets::new();
    for (date, models) in entries {
        let key = slot_key(*date, &calendar());
        for (model, tally) in models {
            *buckets.entry(key.clone()).or_default().entry(model.to_string()).or_default() += tally;
        }
    }
    price_buckets(&buckets, prices, &calendar(), None, &HashMap::new())
}

fn summary(ledgers: Vec<(SpendAgent, Ledger)>, name: &str, over_last: Option<usize>) -> ModelSpendSummary {
    ModelSpendSummary::of(&ledgers.into_iter().collect(), name, over_last, now(), &calendar())
}

fn t(input: i64) -> TokenTally {
    TokenTally::new(input, 0, 0, 0)
}

/// A hand-built day, for the mismatched-input regressions the readers cannot produce.
fn day_record(date: DateTime<Utc>, tokens: i64, model: &str) -> LedgerDay {
    let mut day = LedgerDay::new(date, tokens, 0.0, 0, HashMap::from([(model.to_string(), tokens)]));
    day.tally = t(tokens);
    day.model_tallies = HashMap::from([(model.to_string(), t(tokens))]);
    day
}

fn hand(days: Vec<LedgerDay>, names: &[(&str, &str)], slots: Vec<Slot>) -> Ledger {
    Ledger {
        earliest: days.first().map(|d| d.date),
        days,
        model_names: names.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        slots,
        ..Ledger::empty()
    }
}

fn slot_with(start: DateTime<Utc>, tokens: i64, models: &[(&str, TokenTally)]) -> Slot {
    Slot { models: models.iter().map(|(k, v)| (k.to_string(), v.clone())).collect(), ..Slot::new(start, tokens, 0.0) }
}

#[test]
fn two_models_on_one_day_do_not_cross_use_each_others_categories_or_hours() {
    // The day is one ledger day with both models in it: only the raw id can tell their categories
    // apart, and only the slot can tell their hours.
    let ledger = ledger(&[(at(0, 10), vec![("alpha", t(100))]), (at(0, 11), vec![("beta", TokenTally::new(900, 0, 0, 5))])], &prices());

    let alpha = summary(vec![(ClaudeCode, ledger.clone())], "Alpha", Some(7));
    assert_eq!(alpha.tokens, 100);
    assert_eq!(alpha.tally, Some(t(100)));
    // The hour belongs to the model whose bucket it was, not to the day.
    assert_eq!(alpha.hours, Some(HashMap::from([(10, 100)])));

    let beta = summary(vec![(ClaudeCode, ledger)], "Beta", Some(7));
    assert_eq!(beta.tokens, 905);
    assert_eq!(beta.tally, Some(TokenTally::new(900, 0, 0, 5)));
    assert_eq!(beta.hours, Some(HashMap::from([(11, 905)])));
}

#[test]
fn one_display_name_sums_several_raw_ids_across_several_agents() {
    let first = ledger(&[(at(0, 10), vec![("alpha-1", t(100))])], &prices());
    let second = ledger(&[(at(1, 10), vec![("alpha-2", TokenTally::new(0, 0, 0, 200))])], &prices());
    let ledgers: HashMap<_, _> = HashMap::from([(ClaudeCode, first), (Codex, second)]);

    let result = ModelSpendSummary::of(&ledgers, "Alpha", Some(7), now(), &calendar());
    assert_eq!(result.name, "Alpha");
    assert_eq!(result.tokens, 300);
    assert_eq!(result.tally, Some(TokenTally::new(100, 0, 0, 200)));
    // Heaviest first.
    assert_eq!(result.agents.iter().map(|a| a.agent).collect::<Vec<_>>(), [Codex, ClaudeCode]);
    // Each raw id's categories stay with the day it was spent on.
    assert_eq!(result.days.iter().find(|d| d.tokens == 100).unwrap().tally, Some(t(100)));
    assert_eq!(result.days.iter().find(|d| d.tokens == 200).unwrap().tally, Some(TokenTally::new(0, 0, 0, 200)));

    // The drill-down's total is the combined page's own model row, and the parts add up to it.
    let combined = SpendSummary::of(&ledgers, Some(7), now(), &calendar());
    assert_eq!(combined.models.iter().find(|m| m.name == "Alpha").unwrap().tokens, result.tokens);
    assert_eq!(result.days.iter().map(|d| d.tokens).sum::<i64>(), result.tokens);
    assert_eq!(result.agents.iter().map(|a| a.tokens).sum::<i64>(), result.tokens);
}

#[test]
fn agents_with_equal_tokens_are_ordered_by_name() {
    let ledger = ledger(&[(at(0, 10), vec![("alpha", t(100))])], &prices());
    let result = summary(vec![(Codex, ledger.clone()), (ClaudeCode, ledger)], "Alpha", Some(7));
    assert_eq!(result.agents.iter().map(|a| a.agent).collect::<Vec<_>>(), [ClaudeCode, Codex]);
}

#[test]
fn the_span_is_the_calendar_window_quiet_days_included_all_time_starts_at_first_use() {
    // Alpha is used today, six days ago, seven days ago just past the week's cutoff, and nine days
    // ago. Beta appears much earlier, so an all-time window taken from the whole ledger would
    // reach back too far.
    let alpha = ledger(
        &[
            (at(0, 12), vec![("alpha", t(10))]),
            (at(6, 12), vec![("alpha", t(20))]),
            (at(7, 12), vec![("alpha", t(5))]),
            (at(9, 12), vec![("alpha", t(40))]),
        ],
        &prices(),
    );
    let beta = ledger(&[(at(20, 12), vec![("beta", t(999))])], &prices());
    let ledgers = || vec![(ClaudeCode, alpha.clone()), (Codex, beta.clone())];

    let today_only = summary(ledgers(), "Alpha", Some(1));
    assert_eq!(today_only.tokens, 10);
    assert_eq!(today_only.days.len(), 1);

    let week = summary(ledgers(), "Alpha", Some(7));
    // The cutoff is six days back, so the record exactly seven days old is out and the one six
    // days old is in.
    assert_eq!(week.tokens, 30);
    assert_eq!(week.days.len(), 7);
    assert_eq!(week.active_days(), 2);
    assert_eq!(week.days.iter().filter(|d| d.tokens == 0).count(), 5);

    let all = summary(ledgers(), "Alpha", None);
    // From Alpha's first day, not from the earlier Beta day the ledger also holds.
    assert_eq!(all.days.len(), 10);
    assert_eq!(all.days[0].date, calendar().start_of_day(at(9, 0)));
    assert_eq!(all.tokens, 75);
}

#[test]
fn a_model_with_nothing_in_the_span_is_empty_not_a_zero_shaped_series() {
    let ledger = ledger(&[(at(0, 12), vec![("alpha", t(10))])], &prices());
    let beyond = summary(vec![(ClaudeCode, ledger)], "beta", Some(7));
    assert!(beyond.is_empty());
    assert!(beyond.days.is_empty());
    assert_eq!(beyond.tally, None);
    assert_eq!(beyond.hours, None);
}

#[test]
fn a_providers_statistics_are_not_part_of_a_models_priced_answer() {
    let mut ledger = ledger(&[(at(0, 12), vec![("alpha", t(500))])], &prices());
    ledger.origin = Origin::ProviderStatistics;
    assert!(summary(vec![(ClaudeCode, ledger)], "Alpha", Some(7)).is_empty());
}

#[test]
fn an_unpriced_model_still_counts_its_tokens_and_keeps_its_categories() {
    // No price table at all: the model has no display name either, so its raw id is the grouping
    // key.
    let ledger = ledger(&[(at(0, 10), vec![("mystery", TokenTally::new(5, 0, 0, 7))])], &PriceTable::new());
    let result = summary(vec![(ClaudeCode, ledger)], "mystery", Some(7));
    assert_eq!(result.tokens, 12);
    assert_eq!(result.tally, Some(TokenTally::new(5, 0, 0, 7)));
    assert_eq!(result.hours, Some(HashMap::from([(10, 12)])));
}

#[test]
fn a_models_money_is_each_kind_at_its_own_rate_and_the_parts_sum_to_the_total() {
    let ledger = ledger(&[(at(0, 10), vec![("rates", million())])], &rate_prices());
    let result = summary(vec![(ClaudeCode, ledger)], "Rates", Some(7));

    // $1 input, $4 cache write, $3 cache read, $2 output.
    let expected = TokenCost { input: 1.0, cache_write: 4.0, cache_read: 3.0, output: 2.0 };
    assert_eq!(result.cost_breakdown, Some(expected));
    assert_eq!(result.cost(), Some(10.0));
    assert_eq!(result.unpriced_tokens, 0);

    let day = result.days.iter().find(|d| d.tokens > 0).unwrap();
    assert_eq!(day.cost_breakdown, Some(expected));
    assert_eq!(day.cost(), Some(10.0));
    assert_eq!(result.agents[0].cost, Some(10.0));
    assert_eq!(result.agents[0].unpriced_tokens, 0);

    // Every rollup is the same arithmetic: the day and agent slices add to the model total.
    assert!((result.days.iter().filter_map(ModelDay::cost).sum::<f64>() - result.cost().unwrap()).abs() < 1e-12);
    assert!((result.agents.iter().filter_map(|a| a.cost).sum::<f64>() - result.cost().unwrap()).abs() < 1e-12);
}

#[test]
fn a_missing_cache_rate_falls_back_to_the_input_rate() {
    let ledger = ledger(&[(at(0, 10), vec![("no-cache", TokenTally::new(0, 1_000_000, 1_000_000, 0))])], &rate_prices());
    let result = summary(vec![(ClaudeCode, ledger)], "No Cache", Some(7));
    // Both cache kinds are billed at the $5 input rate.
    assert_eq!(result.cost_breakdown, Some(TokenCost { cache_write: 5.0, cache_read: 5.0, ..TokenCost::default() }));
    assert_eq!(result.cost(), Some(10.0));
}

#[test]
fn two_models_on_one_day_do_not_cross_money() {
    let ledger = ledger(
        &[(at(0, 10), vec![("rates", t(1_000_000))]), (at(0, 11), vec![("no-cache", t(1_000_000))])],
        &rate_prices(),
    );
    let costs = |name| summary(vec![(ClaudeCode, ledger.clone())], name, Some(7)).cost_breakdown;
    assert_eq!(costs("Rates"), Some(TokenCost { input: 1.0, ..TokenCost::default() }));
    assert_eq!(costs("No Cache"), Some(TokenCost { input: 5.0, ..TokenCost::default() }));
    // The day's own money is both, which is what the models must not share.
    assert_eq!(ledger.days.iter().find(|d| d.tokens > 0).unwrap().cost, 6.0);
}

#[test]
fn two_raw_ids_behind_one_name_are_priced_separately_never_merged_onto_one_rate() {
    let ledger = ledger(
        &[(at(0, 10), vec![("cheap", t(1_000_000))]), (at(1, 10), vec![("dear", t(1_000_000))])],
        &shared_prices(),
    );
    let result = summary(vec![(ClaudeCode, ledger)], "Shared", Some(7));
    // $1 + $100, not two million tokens at either rate.
    assert_eq!(result.cost(), Some(101.0));
    assert_eq!(result.cost_breakdown, Some(TokenCost { input: 101.0, ..TokenCost::default() }));
}

#[test]
fn money_adds_across_agents_and_follows_the_span() {
    let cheap = ledger(&[(at(0, 10), vec![("cheap", t(1_000_000))])], &shared_prices());
    let dear = ledger(&[(at(6, 10), vec![("dear", t(1_000_000))])], &shared_prices());
    let ledgers = || vec![(ClaudeCode, cheap.clone()), (Codex, dear.clone())];

    let today_only = summary(ledgers(), "Shared", Some(1));
    assert_eq!(today_only.cost(), Some(1.0));
    assert_eq!(today_only.unpriced_tokens, 0);

    let week = summary(ledgers(), "Shared", Some(7));
    assert_eq!(week.cost(), Some(101.0));
    assert_eq!(week.agents.iter().find(|a| a.agent == Codex).unwrap().cost, Some(100.0));
    assert_eq!(week.agents.iter().find(|a| a.agent == ClaudeCode).unwrap().cost, Some(1.0));
}

#[test]
fn a_model_with_no_price_anywhere_has_no_money_to_show() {
    let ledger = ledger(&[(at(0, 10), vec![("mystery", TokenTally::new(5, 0, 0, 7))])], &PriceTable::new());
    let result = summary(vec![(ClaudeCode, ledger)], "mystery", Some(7));
    assert_eq!(result.cost(), None);
    assert_eq!(result.cost_breakdown, None);
    assert_eq!(result.unpriced_tokens, 12);
    let day = result.days.iter().find(|d| d.tokens > 0).unwrap();
    assert_eq!(day.cost(), None);
    assert_eq!(day.unpriced_tokens, 12);
    assert_eq!(result.agents[0].cost, None);
    assert_eq!(result.agents[0].unpriced_tokens, 12);
}

#[test]
fn a_part_priced_model_shows_a_subtotal_and_the_unpriced_remainder() {
    // A priced raw id and an unpriced one behind one display name: the case the UI labels as a
    // partial estimate.
    let mut day = LedgerDay::new(today(), 3_000_000, 1.0, 2_000_000, HashMap::from([("known".to_string(), 1_000_000), ("unknown".to_string(), 2_000_000)]));
    day.tally = t(3_000_000);
    day.model_tallies = HashMap::from([("known".to_string(), t(1_000_000)), ("unknown".to_string(), t(2_000_000))]);
    day.model_costs = HashMap::from([("known".to_string(), TokenCost { input: 1.0, ..TokenCost::default() })]);
    let mut ledger = hand(vec![day], &[("known", "Mix"), ("unknown", "Mix")], vec![]);
    ledger.unpriced_models = vec!["unknown".into()];
    let result = summary(vec![(ClaudeCode, ledger)], "Mix", Some(7));

    assert_eq!(result.cost(), Some(1.0));
    assert_eq!(result.unpriced_tokens, 2_000_000);
    let day = result.days.iter().find(|d| d.tokens > 0).unwrap();
    assert_eq!(day.cost(), Some(1.0));
    assert_eq!(day.unpriced_tokens, 2_000_000);
    assert_eq!(result.agents[0].cost, Some(1.0));
    assert_eq!(result.agents[0].unpriced_tokens, 2_000_000);
}

#[test]
fn a_real_zero_rate_is_a_reading_of_zero_not_a_missing_price() {
    let ledger = ledger(&[(at(0, 10), vec![("free", t(1_000_000))])], &rate_prices());
    let result = summary(vec![(ClaudeCode, ledger)], "Free", Some(7));
    assert_eq!(result.cost(), Some(0.0));
    assert_eq!(result.cost_breakdown, Some(TokenCost::default()));
    assert_eq!(result.unpriced_tokens, 0);
    assert_eq!(result.days.iter().find(|d| d.tokens > 0).unwrap().cost(), Some(0.0));
    assert_eq!(result.agents[0].cost, Some(0.0));
}

#[test]
fn missing_or_incomplete_money_metadata_is_counted_never_invented() {
    // Money was never saved (an old cache): counted, unpriced, not $0.
    let mut day = LedgerDay::new(today(), 1_000_000, 0.0, 0, HashMap::from([("alpha".to_string(), 1_000_000)]));
    day.tally = t(1_000_000);
    day.model_tallies = HashMap::from([("alpha".to_string(), t(1_000_000))]);
    let no_money = hand(vec![day], &[("alpha", "Alpha")], vec![]);
    let old = summary(vec![(ClaudeCode, no_money)], "Alpha", Some(7));
    assert_eq!(old.tokens, 1_000_000);
    assert_eq!(old.cost(), None);
    assert_eq!(old.unpriced_tokens, 1_000_000);

    // Money present but the category detail does not match the tokens: the split cannot be
    // trusted, so the money is not shown either.
    let mut day = LedgerDay::new(today(), 500, 0.0, 0, HashMap::from([("alpha".to_string(), 500)]));
    day.tally = t(100);
    day.model_tallies = HashMap::from([("alpha".to_string(), t(100))]);
    day.model_costs = HashMap::from([("alpha".to_string(), TokenCost { input: 1.0, ..TokenCost::default() })]);
    let mismatched = hand(vec![day], &[("alpha", "Alpha")], vec![]);
    let incomplete = summary(vec![(ClaudeCode, mismatched)], "Alpha", Some(7));
    assert_eq!(incomplete.tokens, 500);
    assert_eq!(incomplete.cost(), None);
    assert_eq!(incomplete.unpriced_tokens, 500);
}

#[test]
fn without_per_model_detail_the_totals_stay_and_categories_and_hours_go_missing() {
    let real = ledger(&[(at(0, 10), vec![("alpha", t(100))])], &prices());
    // The shape a cache written before the detail existed decodes to: the day's model totals are
    // there, the per-model split, slot models and money are not.
    let mut stripped = real.clone();
    for day in &mut stripped.days {
        day.model_tallies.clear();
        day.model_costs.clear();
    }
    for slot in &mut stripped.slots {
        slot.models.clear();
    }
    let result = summary(vec![(ClaudeCode, stripped)], "Alpha", Some(7));
    // The token question is still answered...
    assert_eq!(result.tokens, 100);
    // ...but the split, the hours and the money were never saved, so they are None rather than a
    // total read as one kind.
    assert_eq!(result.tally, None);
    assert_eq!(result.hours, None);
    assert_eq!(result.cost(), None);
    assert_eq!(result.unpriced_tokens, 100);
}

#[test]
fn hours_are_withheld_when_the_slot_buckets_do_not_reconcile_with_the_days() {
    // Both models kept their daily detail, but only `beta` kept slot detail: an hour series built
    // from this would file `beta`'s time of day under `alpha` too.
    let mut day = LedgerDay::new(today(), 300, 0.0, 0, HashMap::from([("alpha".to_string(), 100), ("beta".to_string(), 200)]));
    day.tally = t(300);
    day.model_tallies = HashMap::from([("alpha".to_string(), t(100)), ("beta".to_string(), t(200))]);
    let ledger = hand(vec![day], &[("alpha", "Alpha"), ("beta", "Beta")], vec![slot_with(at(0, 10), 300, &[("beta", t(200))])]);
    let alpha = summary(vec![(ClaudeCode, ledger)], "Alpha", Some(7));
    // Its own daily split is intact...
    assert_eq!(alpha.tokens, 100);
    assert_eq!(alpha.tally, Some(t(100)));
    // ...but its hours cannot be told from the agent's, so they are None.
    assert_eq!(alpha.hours, None);
}

#[test]
fn one_agents_overcount_cannot_cancel_another_agents_missing_detail() {
    // A total-only check passes here: the day figures add to 200 and the slot figures add to 200.
    // But A's slots double its own day and B's slots are missing entirely.
    let slot = at(0, 10);
    let overcounted = hand(vec![day_record(today(), 100, "alpha")], &[("alpha", "Alpha")], vec![slot_with(slot, 200, &[("alpha", t(200))])]);
    let missing = hand(vec![day_record(today(), 100, "alpha")], &[("alpha", "Alpha")], vec![slot_with(slot, 100, &[])]);
    let result = summary(vec![(ClaudeCode, overcounted), (Codex, missing)], "Alpha", Some(7));
    assert_eq!(result.tokens, 200);
    assert_eq!(result.tally, Some(t(200)));
    assert_eq!(result.hours, None);
}

#[test]
fn tokens_swapped_between_two_days_of_one_agent_withhold_the_hours() {
    // Day totals add to 200 and slot totals add to 200, so a total-only check passes, but neither
    // day's buckets match that day's own total.
    let yesterday = calendar().add_days(today(), -1);
    let ledger = hand(
        vec![day_record(today(), 100, "alpha"), day_record(yesterday, 100, "alpha")],
        &[("alpha", "Alpha")],
        vec![slot_with(at(0, 10), 50, &[("alpha", t(50))]), slot_with(at(1, 10), 150, &[("alpha", t(150))])],
    );
    let result = summary(vec![(ClaudeCode, ledger)], "Alpha", Some(7));
    assert_eq!(result.tokens, 200);
    assert_eq!(result.tally, Some(t(200)));
    assert_eq!(result.hours, None);
}

#[test]
fn a_model_whose_totals_do_not_match_its_categories_gets_no_tally() {
    // The day says 500 tokens but the only split kept adds to 100: reading it would understate
    // the model.
    let mut day = LedgerDay::new(today(), 500, 0.0, 0, HashMap::from([("alpha".to_string(), 500)]));
    day.tally = t(100);
    day.model_tallies = HashMap::from([("alpha".to_string(), t(100))]);
    let result = summary(vec![(ClaudeCode, hand(vec![day], &[("alpha", "Alpha")], vec![]))], "Alpha", Some(7));
    assert_eq!(result.tokens, 500);
    assert_eq!(result.tally, None);
}

#[test]
fn negative_or_overflowing_metadata_is_counted_unpriced_never_priced_or_crashed_on() {
    // The kinds plus the remainder do not fit. Reading a plain total at all would overflow.
    let overflowing_tally = TokenTally::new(i64::MAX, 0, 0, 1);
    let mut day = LedgerDay::new(today(), 500, 0.0, 0, HashMap::from([("alpha".to_string(), 500)]));
    day.tally = overflowing_tally.clone();
    day.model_tallies = HashMap::from([("alpha".to_string(), overflowing_tally)]);
    day.model_costs = HashMap::from([("alpha".to_string(), TokenCost { input: 1.0, ..TokenCost::default() })]);
    let overflow = summary(vec![(ClaudeCode, hand(vec![day], &[("alpha", "Alpha")], vec![]))], "Alpha", Some(7));
    assert_eq!(overflow.tokens, 500);
    assert_eq!(overflow.cost(), None);
    assert_eq!(overflow.tally, None);
    assert_eq!(overflow.unpriced_tokens, 500);

    // A negative remainder must not be clamped to zero, which would let the split pass and the
    // money through. It is broken data instead.
    let mut day = LedgerDay::new(today(), 500, 0.0, 0, HashMap::from([("alpha".to_string(), 500)]));
    day.tally = t(500);
    day.model_tallies = HashMap::from([("alpha".to_string(), t(500))]);
    day.model_costs = HashMap::from([("alpha".to_string(), TokenCost { input: 1.0, ..TokenCost::default() })]);
    day.model_unclassified_tokens = HashMap::from([("alpha".to_string(), -100)]);
    let negative = summary(vec![(ClaudeCode, hand(vec![day], &[("alpha", "Alpha")], vec![]))], "Alpha", Some(7));
    assert_eq!(negative.tokens, 500);
    assert_eq!(negative.cost(), None);
    assert_eq!(negative.tally, None);
    assert_eq!(negative.unpriced_tokens, 500);
}

// MARK: - Day sorting

fn md(offset: i64, tokens: i64, tally: Option<TokenTally>) -> ModelDay {
    ModelDay {
        date: calendar().add_days(today(), offset),
        tokens,
        tally,
        cost_breakdown: None,
        unpriced_tokens: 0,
        unclassified_tokens: 0,
    }
}

fn cost_day(offset: i64, cost: f64) -> ModelDay {
    ModelDay { cost_breakdown: Some(TokenCost { input: cost, ..TokenCost::default() }), ..md(offset, 1, Some(t(1))) }
}

fn dates(days: &[ModelDay], column: ModelDayColumn, ascending: bool) -> Vec<DateTime<Utc>> {
    ModelSpendSummary::sorted(days, column, ascending, false).iter().map(|d| d.date).collect()
}

#[test]
fn a_missing_category_sorts_last_in_both_directions_a_real_zero_sorts_as_zero() {
    // Oldest has a real 0 input, the middle two are tied at 5, and the newest has no split at all.
    let zero = md(0, 10, Some(t(0)));
    let tied_older = md(1, 20, Some(t(5)));
    let tied_newer = md(2, 30, Some(t(5)));
    let missing = md(3, 40, None);
    let days = [zero.clone(), tied_older.clone(), tied_newer.clone(), missing.clone()];
    // 0 first, then the two 5s oldest-first, then the missing one.
    assert_eq!(dates(&days, ModelDayColumn::Fresh, true), [zero.date, tied_older.date, tied_newer.date, missing.date]);
    // The two 5s newest-first, then the real 0, then the missing one last again.
    assert_eq!(dates(&days, ModelDayColumn::Fresh, false), [tied_newer.date, tied_older.date, zero.date, missing.date]);
}

#[test]
fn the_total_column_is_never_missing_so_a_quiet_day_takes_its_real_place() {
    let quiet = md(0, 0, None);
    let busy_older = md(1, 100, None);
    let busy_newer = md(2, 100, None);
    let days = [quiet.clone(), busy_older.clone(), busy_newer.clone()];
    assert_eq!(dates(&days, ModelDayColumn::Total, true), [quiet.date, busy_older.date, busy_newer.date]);
    // Equal totals keep a deterministic order, most recent first.
    assert_eq!(dates(&days, ModelDayColumn::Total, false), [busy_newer.date, busy_older.date, quiet.date]);
}

#[test]
fn unknown_only_tokens_sort_as_missing_input_and_as_a_recorded_unclassified_count() {
    let unknown = ModelDay { unclassified_tokens: 500, ..md(0, 500, Some(TokenTally::default())) };
    let zero = md(-1, 10, Some(TokenTally::new(0, 0, 0, 10)));
    let mixed = ModelDay { unclassified_tokens: 50, ..md(-2, 150, Some(t(100))) };
    let days = [unknown.clone(), zero.clone(), mixed.clone()];
    assert_eq!(dates(&days, ModelDayColumn::Fresh, true), [zero.date, mixed.date, unknown.date]);
    assert_eq!(dates(&days, ModelDayColumn::Fresh, false), [mixed.date, zero.date, unknown.date]);
    assert_eq!(dates(&days, ModelDayColumn::Unclassified, true), [zero.date, mixed.date, unknown.date]);
}

#[test]
fn the_date_column_has_no_missing_values_and_follows_the_arrow() {
    let oldest = md(0, 1, Some(t(1)));
    let middle = md(1, 2, Some(t(2)));
    let newest = md(2, 3, Some(t(3)));
    let days = [middle.clone(), newest.clone(), oldest.clone()];
    assert_eq!(dates(&days, ModelDayColumn::Date, true), [oldest.date, middle.date, newest.date]);
    assert_eq!(dates(&days, ModelDayColumn::Date, false), [newest.date, middle.date, oldest.date]);
}

#[test]
fn the_cost_column_keeps_fractional_money_sorts_a_real_zero_and_holds_none_last() {
    let missing = md(0, 10, None); // never priced
    let zero = cost_day(1, 0.0); // a real $0
    let small = cost_day(2, 0.001); // sub-cent, not truncated
    let large = cost_day(3, 100.5);
    let days = [missing.clone(), large.clone(), zero.clone(), small.clone()];
    assert_eq!(dates(&days, ModelDayColumn::Cost, true), [zero.date, small.date, large.date, missing.date]);
    assert_eq!(dates(&days, ModelDayColumn::Cost, false), [large.date, small.date, zero.date, missing.date]);
}

#[test]
fn large_token_counts_sort_by_exact_value_not_by_a_widened_double() {
    // Two adjacent integers above 2^53, which a double cannot tell apart. The larger is the older
    // day, so a comparison that ties and falls back to the date breaks the order in one direction
    // and hides the other.
    let low: i64 = 9_007_199_254_740_992;
    let high = low + 1;
    let days = [md(0, low, Some(t(low))), md(1, high, Some(t(high)))];
    let tokens = |column, ascending| ModelSpendSummary::sorted(&days, column, ascending, false).iter().map(|d| d.tokens).collect::<Vec<_>>();
    assert_eq!(tokens(ModelDayColumn::Total, true), [low, high]);
    assert_eq!(tokens(ModelDayColumn::Total, false), [high, low]);
    assert_eq!(tokens(ModelDayColumn::Fresh, true), [low, high]);
    assert_eq!(tokens(ModelDayColumn::Fresh, false), [high, low]);
}
