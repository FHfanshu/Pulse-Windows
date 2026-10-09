// Ported from upstream Tests/PulseTests/RecapDeckTests.swift and RecapScoreDataTests.swift.
//! Which cards a recap gets: a card whose data is missing is left out. And what the scorecard's
//! small charts are built from: which slot is future, quiet or busiest, and when a figure is left
//! out rather than drawn as a zero.

use std::collections::HashMap;

use chrono::{DateTime, TimeZone, Utc};

use super::deck::{self, Card, Slot, Tone};
use super::samples::{self, MonthSpec};
use super::Recap;
use crate::spend::ledger::{price_buckets, slot_key, Buckets};
use crate::spend::prices::{ModelPrice, PriceTable};
use crate::spend::tally::TokenTally;
use crate::spend::{Calendar, SpendAgent};

const PRICE: Option<f64> = Some(200.0);

fn cards(recap: &Recap, price: Option<f64>) -> Vec<Card> {
    deck::cards(recap, deck::payback(recap, price).is_some())
}

fn month() -> Recap {
    samples::month(MonthSpec::default())
}

fn year() -> Recap {
    samples::year(true, None)
}

#[test]
fn a_full_month_has_all_six_cards_in_order() {
    use Card::*;
    assert_eq!(cards(&month(), PRICE), vec![Poster, Opener, Calendar, Timetable, Payback, Scorecard]);
}

#[test]
fn a_full_year_has_the_years_own_cards() {
    use Card::*;
    assert_eq!(cards(&year(), PRICE), vec![Poster, Opener, YearCalendar, Months, Timetable, Payback, Scorecard]);
}

#[test]
fn no_price_no_payback() {
    assert!(!cards(&month(), None).contains(&Card::Payback));
    assert!(!cards(&month(), Some(0.0)).contains(&Card::Payback));
    assert!(deck::payback(&month(), None).is_none());
}

#[test]
fn no_cost_no_payback_whatever_the_price() {
    let recap = samples::month(MonthSpec { priced: false, ..MonthSpec::default() });
    assert!(!cards(&recap, PRICE).contains(&Card::Payback));
    assert!(deck::payback(&recap, PRICE).is_none());
}

#[test]
fn no_hour_shape_no_timetable() {
    let recap = samples::month(MonthSpec { has_hours: false, ..MonthSpec::default() });
    assert!(!cards(&recap, PRICE).contains(&Card::Timetable));
}

#[test]
fn no_agents_no_opener() {
    let recap = samples::month(MonthSpec { has_agents: false, ..MonthSpec::default() });
    assert!(!cards(&recap, PRICE).contains(&Card::Opener));
}

#[test]
fn a_recap_with_nothing_in_it_has_no_deck() {
    assert!(cards(&samples::empty(), PRICE).is_empty());
}

#[test]
fn the_card_list_without_a_price_and_with_one_differ_only_by_the_payback() {
    let facts = deck::DeckFacts::of(&month());
    assert!(!facts.cards.contains(&Card::Payback));
    assert!(facts.cards_with_payback.contains(&Card::Payback));
    assert_eq!(facts.payback_months, Some(1.0));
    let bare = samples::month(MonthSpec { priced: false, has_hours: false, has_agents: false, ..MonthSpec::default() });
    let facts = deck::DeckFacts::of(&bare);
    assert_eq!(facts.cards, vec![Card::Poster, Card::Calendar, Card::Scorecard]);
    assert_eq!(facts.payback_months, None);
}

#[test]
fn payback_divides_the_estimate_by_the_price() {
    let payback = deck::payback(&month(), PRICE).unwrap();
    assert_eq!(payback.months, 1.0);
    assert_eq!(payback.monthly_price, 200.0);
    assert_eq!(payback.paid(), 200.0);
    assert!(!payback.is_to_date);
    assert!((payback.multiple() - 6.71).abs() < 0.001);
}

#[test]
fn unpriced_work_keeps_the_payback_card_with_the_floor_noted() {
    let with = |share| samples::month(MonthSpec { unpriced_share: share, ..MonthSpec::default() });

    let tenth = with(0.1);
    assert!(deck::payback(&tenth, PRICE).is_some());
    assert!(cards(&tenth, PRICE).contains(&Card::Payback));
    assert!(deck::cost_is_floor(&tenth));

    let sliver = with(0.004);
    assert!(deck::payback(&sliver, PRICE).is_some());
    assert!(deck::cost_is_floor(&sliver));

    assert!(!deck::cost_is_floor(&month()));
    // Nothing priced is no money at all, so there is no floor to speak of.
    let unpriced = samples::month(MonthSpec { priced: false, unpriced_share: 1.0, ..MonthSpec::default() });
    assert!(!deck::cost_is_floor(&unpriced));
}

fn running(period: super::Period, now: DateTime<Utc>) -> Recap {
    let calendar = Calendar::utc(2);
    let prices: PriceTable = HashMap::from([("claude".to_string(), ModelPrice::new(3.0, 15.0, Some(0.3), Some(3.75), Some("Claude")))]);
    let mut buckets: Buckets = HashMap::new();
    buckets.entry(slot_key(now, &calendar)).or_default().insert("claude".to_string(), TokenTally::new(1_000, 0, 0, 0));
    // A record a year back as well, so the records reach before the period and these test the
    // proration by days gone, not `records_begin`.
    let year_back = now - chrono::Duration::days(366);
    buckets.entry(slot_key(year_back, &calendar)).or_default().insert("claude".to_string(), TokenTally::new(1, 0, 0, 0));
    let ledger = price_buckets(&buckets, &prices, &calendar, None, &HashMap::new());
    super::build(period, &[(SpendAgent::ClaudeCode, ledger)].into_iter().collect(), &prices, now, &calendar).unwrap()
}

fn utc(year: i32, month: u32, day: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(year, month, day, 12, 0, 0).unwrap()
}

#[test]
fn a_month_still_running_is_charged_by_the_days_gone_not_for_the_whole_month() {
    // 10 days of 31.
    let recap = running(super::Period::Month { year: 2026, month: 10 }, utc(2026, 10, 10));
    assert!(recap.is_in_progress);
    let payback = deck::payback(&recap, Some(31.0)).unwrap();
    assert!(payback.is_to_date);
    assert!((payback.months - 10.0 / 31.0).abs() < 1e-9);
    assert!((payback.paid() - 10.0).abs() < 1e-9);
    assert_eq!(payback.monthly_price, 31.0);
}

#[test]
fn a_year_still_running_is_charged_twelve_months_by_the_days_gone_over_the_days_in_the_year() {
    // 1 March 2026: 31 + 28 + 1 = 60 days of 365.
    let recap = running(super::Period::Year(2026), utc(2026, 3, 1));
    assert_eq!(recap.elapsed_days, 60);
    let payback = deck::payback(&recap, Some(365.0)).unwrap();
    assert!((payback.months - 12.0 * 60.0 / 365.0).abs() < 1e-9);
    assert!((payback.paid() - 12.0 * 60.0).abs() < 1e-9);
    // A leap year has 366.
    let leap = running(super::Period::Year(2028), utc(2028, 1, 31));
    assert!((deck::paid_months(&leap) - 12.0 * 31.0 / 366.0).abs() < 1e-9);
}

#[test]
fn a_finished_period_is_charged_whole() {
    assert_eq!(deck::paid_months(&month()), 1.0);
    assert_eq!(deck::paid_months(&year()), 12.0);
}

#[test]
fn a_cache_saving_under_fifty_cents_is_not_worth_a_sentence() {
    let saving = |spec: MonthSpec| deck::cache_savings(&samples::month(spec));
    assert_eq!(saving(MonthSpec::default()), Some(410.0));
    assert_eq!(saving(MonthSpec { cache_savings: 0.5, ..MonthSpec::default() }), Some(0.5));
    assert_eq!(saving(MonthSpec { cache_savings: 0.49, ..MonthSpec::default() }), None);
    assert_eq!(saving(MonthSpec { cache_savings: 0.0, ..MonthSpec::default() }), None);
    assert_eq!(saving(MonthSpec { has_cache: false, ..MonthSpec::default() }), None);
}

#[test]
fn the_posters_cost_line_is_drawn_only_when_every_day_with_work_has_a_price() {
    let full = month();
    let series = deck::cost_series(&full);
    assert_eq!(series.len(), full.days.len());
    // Quiet days are zero, not missing.
    let quiet: Vec<_> = full.days.iter().zip(&series).filter(|(d, _)| d.tokens == 0).collect();
    assert!(!quiet.is_empty() && quiet.iter().all(|(_, c)| **c == 0.0));
    assert!(deck::cost_series(&samples::month(MonthSpec { priced: false, ..MonthSpec::default() })).is_empty());
    assert_eq!(deck::cost_series(&year()).len(), 12);
}

#[test]
fn a_past_period_never_says_its_streak_is_still_going_a_running_one_does() {
    let past = month();
    let (days, is_current) = deck::streak(&past).unwrap();
    assert!(!is_current);
    assert_eq!(days, past.longest_streak);
    assert!(days <= past.elapsed_days);

    let running = samples::month(MonthSpec { in_progress: true, ..MonthSpec::default() });
    let (_, current) = deck::streak(&running).unwrap();
    assert_eq!(current, running.current_streak > 0);
}

#[test]
fn a_years_payback_is_against_twelve_months_of_the_price() {
    let payback = deck::payback(&year(), PRICE).unwrap();
    assert_eq!(payback.months, 12.0);
    assert_eq!(payback.paid(), 2400.0);
}

// Scorecard data.

#[test]
fn a_finished_month_has_a_bar_per_day_quiet_days_grey_one_busiest_none_to_come() {
    let bars = deck::score_bars(&month());
    assert_eq!(bars.len(), 30);
    assert!(!bars.iter().any(|b| b.slot == Slot::Future));
    // Days 4, 10 and 11 are quiet in the sample.
    assert!(bars[3].slot == Slot::Quiet && bars[9].slot == Slot::Quiet && bars[10].slot == Slot::Quiet);
    assert_eq!(bars.iter().filter(|b| b.is_busiest).count(), 1);
    assert_eq!(bars.iter().filter(|b| b.slot == Slot::Active).count(), 27);
    // The busiest day is the 17th, the fullest bar.
    assert!(bars[16].is_busiest && bars[16].fraction == 1.0);
    assert!(bars.iter().all(|b| if b.slot == Slot::Active { b.fraction > 0.0 } else { b.fraction == 0.0 }));
}

#[test]
fn a_month_still_running_leaves_the_days_to_come_as_future_not_as_quiet() {
    let recap = samples::month(MonthSpec { in_progress: true, through_day: Some(12), ..MonthSpec::default() });
    let bars = deck::score_bars(&recap);
    assert_eq!(bars.len(), 30);
    assert!(bars[11].slot != Slot::Future);
    assert!(bars[12..].iter().all(|b| b.slot == Slot::Future));
}

#[test]
fn a_year_has_twelve_bars_the_months_to_come_future_the_busiest_month_ink() {
    let bars = deck::score_bars(&year());
    assert_eq!(bars.len(), 12);
    assert_eq!(bars.iter().filter(|b| b.is_busiest).count(), 1);
    // September is the sample's busiest month.
    assert!(bars[8].is_busiest);

    // A year still running has days only through July: the rest are months to come.
    let mut running = samples::year(true, Some(7));
    running.is_in_progress = true;
    let bars = deck::score_bars(&running);
    assert!(bars[..7].iter().all(|b| b.slot == Slot::Active));
    assert!(bars[7..].iter().all(|b| b.slot == Slot::Future));
}

#[test]
fn a_running_years_cost_line_stops_at_the_last_month_it_has_had() {
    let mut running = samples::year(true, Some(7));
    running.is_in_progress = true;
    assert_eq!(deck::cost_series(&running).len(), 7);
    assert_eq!(deck::cost_series(&year()).len(), 12);
}

#[test]
fn no_tokens_in_any_slot_no_strip() {
    assert!(deck::score_bars(&samples::empty()).is_empty());
}

#[test]
fn sessions_per_active_day_is_rounded_and_absent_where_it_would_be_zero() {
    // 412 sessions over 27 days.
    assert_eq!(deck::sessions_per_active_day(&month()), Some(15));
    assert_eq!(deck::sessions_per_active_day(&samples::empty()), None);
}

#[test]
fn tools_three_as_they_are_more_as_two_and_the_rest_grouped() {
    let two = deck::score_agents(&month());
    assert_eq!(two.len(), 2);
    assert_eq!(two.iter().map(|a| a.tone).collect::<Vec<_>>(), vec![Tone::Lime, Tone::Ink]);

    // Five tools (the sample here has the two this port reads): the first two and the rest.
    let mut five = month();
    let template = five.agents[1].clone();
    for share in [0.08, 0.04, 0.01] {
        five.agents.push(super::AgentShare { share, ..template.clone() });
    }
    let agents = deck::score_agents(&five);
    assert_eq!(agents.len(), 3);
    assert_eq!(agents.iter().map(|a| a.tone).collect::<Vec<_>>(), vec![Tone::Lime, Tone::Ink, Tone::Grey]);
    assert!(agents[2].name.is_none());
    assert!((agents[2].share - 0.13).abs() < 1e-9);
    assert!(deck::score_agents(&samples::month(MonthSpec { has_agents: false, ..MonthSpec::default() })).is_empty());
}

/// Writes every sample's report as JSON, for looking at the cards without the app:
/// `PULSE_RECAP_SAMPLES=<dir> cargo test -p pulse-core dump_samples -- --ignored`.
#[test]
#[ignore]
fn dump_samples() {
    let Some(dir) = std::env::var_os("PULSE_RECAP_SAMPLES") else { return };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).unwrap();
    let write = |name: &str, recap: Recap| {
        let report = super::Report::of(recap);
        std::fs::write(dir.join(format!("{name}.json")), serde_json::to_string_pretty(&report).unwrap()).unwrap();
    };
    write("month", month());
    write("year", year());
    write("month-running", samples::month(MonthSpec { in_progress: true, through_day: Some(12), ..MonthSpec::default() }));
    write("month-unpriced", samples::month(MonthSpec { priced: false, ..MonthSpec::default() }));
    write("month-six-weeks", samples::month(MonthSpec { month_number: 8, ..MonthSpec::default() }));
    write("year-running", {
        let mut running = samples::year(true, Some(7));
        running.is_in_progress = true;
        running
    });
    write("empty", samples::empty());
}
