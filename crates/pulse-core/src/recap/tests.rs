// Ported from upstream Tests/PulseTests/RecapTests.swift.
//! The recap's numbers, from synthetic ledgers priced through the production chain
//! (`price_buckets`, the path every reader prices with), on a fixed clock and a UTC calendar.

use std::collections::{BTreeSet, HashMap};

use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};

use super::{build, Day, Period, Persona, Recap};
use crate::spend::ledger::{price_buckets, slot_key, Buckets, Ledger, LedgerDay, Origin, Session, Slot};
use crate::spend::prices::{ContextTier, ModelPrice, PriceTable};
use crate::spend::project::UsageProject;
use crate::spend::tally::TokenTally;
use crate::spend::{Calendar, SpendAgent};

fn calendar() -> Calendar {
    Calendar::utc(2)
}

fn at(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(year, month, day, hour, minute, 0).unwrap()
}

fn noon(year: i32, month: u32, day: u32) -> DateTime<Utc> {
    at(year, month, day, 12, 0)
}

fn midnight(year: i32, month: u32, day: u32) -> DateTime<Utc> {
    at(year, month, day, 0, 0)
}

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

/// Rates in dollars per million tokens, chosen so each kind is visible.
fn prices() -> PriceTable {
    HashMap::from([
        ("claude".to_string(), ModelPrice::new(3.0, 15.0, Some(0.3), Some(3.75), Some("Claude"))),
        ("gpt".to_string(), ModelPrice::new(2.0, 8.0, Some(0.5), None, Some("GPT"))),
        // States no cache rate: reads are billed as input, which saves nothing.
        ("flat".to_string(), ModelPrice::new(1.0, 2.0, None, None, Some("Flat"))),
        (
            "tiered".to_string(),
            ModelPrice::new(3.0, 15.0, Some(0.3), None, Some("Tiered")).with_tiers(vec![ContextTier {
                threshold: 200_000,
                input: 6.0,
                output: 22.5,
                cache_read: Some(0.6),
                cache_write: None,
            }]),
        ),
    ])
}

struct Event {
    at: DateTime<Utc>,
    model: &'static str,
    tally: TokenTally,
}

fn input(tokens: i64) -> TokenTally {
    TokenTally::new(tokens, 0, 0, 0)
}

fn event(at: DateTime<Utc>, tokens: i64) -> Event {
    Event { at, model: "claude", tally: input(tokens) }
}

fn with(at: DateTime<Utc>, model: &'static str, tally: TokenTally) -> Event {
    Event { at, model, tally }
}

/// The production chain: quarter-hour buckets, then a priced ledger.
fn ledger(events: Vec<Event>) -> Ledger {
    let calendar = calendar();
    let mut buckets: Buckets = HashMap::new();
    for event in events {
        let key = slot_key(event.at, &calendar);
        *buckets.entry(key).or_default().entry(event.model.to_string()).or_default() += &event.tally;
    }
    price_buckets(&buckets, &prices(), &calendar, None, &HashMap::new())
}

fn build_with(period: Period, ledgers: Vec<(SpendAgent, Ledger)>, now: DateTime<Utc>) -> Recap {
    build(period, &ledgers.into_iter().collect(), &prices(), now, &calendar()).expect("a period")
}

fn month_of(year: i32, month: u32, ledgers: Vec<(SpendAgent, Ledger)>) -> Recap {
    build_with(Period::Month { year, month }, ledgers, noon(2026, 10, 5))
}

fn october_at(events: Vec<Event>, now: DateTime<Utc>) -> Recap {
    build_with(Period::Month { year: 2026, month: 10 }, vec![(SpendAgent::ClaudeCode, ledger(events))], now)
}

fn october(events: Vec<Event>) -> Recap {
    october_at(events, noon(2026, 10, 5))
}

fn slot(at: DateTime<Utc>, tokens: i64) -> Slot {
    Slot::new(at, tokens, 0.0)
}

fn session(id: &str, project: Option<&str>, from: DateTime<Utc>, to: DateTime<Utc>, slots: Vec<Slot>) -> Session {
    Session {
        id: id.to_string(),
        name: id.to_string(),
        title: None,
        is_review: false,
        project: UsageProject::new(project),
        start: from,
        end: to,
        tokens: slots.iter().map(|s| s.tokens).sum(),
        cost: 0.0,
        unpriced_tokens: 0,
        slots,
        days: Vec::new(),
    }
}

fn near(left: f64, right: f64, tolerance: f64) {
    assert!((left - right).abs() < tolerance, "{left} is not within {tolerance} of {right}");
}

// Bounds.

#[test]
fn a_month_holds_its_first_and_last_day_and_nothing_from_either_neighbour() {
    let recap = october_at(
        vec![
            event(at(2026, 9, 30, 23, 45), 1_000),
            event(midnight(2026, 10, 1), 100),
            event(at(2026, 10, 31, 23, 45), 100),
            event(midnight(2026, 11, 1), 1_000),
        ],
        midnight(2026, 12, 1),
    );

    assert_eq!(recap.tokens, 200);
    assert_eq!(recap.days.len(), 31);
    assert_eq!(recap.start, date(2026, 10, 1));
    assert_eq!(recap.end, date(2026, 11, 1));
    assert!(!recap.is_in_progress);
    assert_eq!(recap.days.first().unwrap().tokens, 100);
    assert_eq!(recap.days.last().unwrap().tokens, 100);
    // The quarter-hours at either edge are not in the hours, nor in the nights: the neighbours'
    // midnights are not this month's.
    let hours = recap.hours.clone().unwrap();
    assert_eq!(hours.iter().sum::<i64>(), 200);
    assert_eq!(hours[0], 100);
    assert_eq!(hours[23], 100);
    assert_eq!(recap.late_nights, 1);
}

#[test]
fn a_period_running_ends_at_the_end_of_today_and_its_months_are_not_yet_drawn() {
    let recap = october(vec![event(noon(2026, 10, 2), 100), event(at(2026, 10, 5, 9, 0), 50)]);
    assert!(recap.is_in_progress);
    assert_eq!(recap.end, date(2026, 10, 6));
    assert_eq!(recap.elapsed_days, 5);
    assert_eq!(recap.days.len(), 5);
    assert_eq!(recap.active_days, 2);
    assert!(recap.months.is_empty());
}

#[test]
fn a_period_that_has_not_started_is_empty_and_a_month_outside_1_to_12_is_no_period() {
    let future = month_of(2026, 12, vec![(SpendAgent::ClaudeCode, ledger(vec![event(noon(2026, 10, 2), 100)]))]);
    assert!(future.is_empty());
    assert!(future.days.is_empty());
    assert!(!future.is_in_progress);
    assert_eq!(future.previous_tokens, None);
    let none = |month| build(Period::Month { year: 2026, month }, &HashMap::new(), &HashMap::new(), noon(2026, 10, 5), &calendar());
    assert!(none(13).is_none());
    assert!(none(0).is_none());
}

#[test]
fn the_span_form_of_the_summary_cuts_a_session_at_both_edges() {
    use crate::spend::summary::SpendSummary;
    let mut l = ledger(vec![event(at(2026, 10, 31, 23, 45), 100), event(at(2026, 11, 1, 0, 15), 300)]);
    l.sessions = vec![session(
        "s",
        Some("/w/pulse"),
        at(2026, 10, 31, 23, 40),
        at(2026, 11, 1, 0, 20),
        vec![slot(at(2026, 10, 31, 23, 45), 100), slot(at(2026, 11, 1, 0, 15), 300)],
    )];
    let ledgers: HashMap<_, _> = [(SpendAgent::ClaudeCode, l)].into_iter().collect();
    let summary = SpendSummary::of_range(&ledgers, midnight(2026, 10, 1), midnight(2026, 11, 1), noon(2026, 12, 1), &calendar());
    assert_eq!(summary.tokens, 100);
    assert_eq!(summary.sessions.iter().map(|s| s.session.tokens).collect::<Vec<_>>(), vec![100]);
    assert_eq!(summary.projects.iter().map(|p| p.tokens).collect::<Vec<_>>(), vec![100]);
    assert_eq!(summary.days.len(), 31);
    assert_eq!(summary.hours.values().sum::<i64>(), 100);
    // An end not after the start is no days at all.
    let none = SpendSummary::of_range(&ledgers, midnight(2026, 11, 1), midnight(2026, 11, 1), noon(2026, 12, 1), &calendar());
    assert!(none.days.is_empty());
    assert_eq!(none.tokens, 0);
}

// The previous period.

#[test]
fn a_month_in_progress_is_set_against_the_same_days_of_the_month_before() {
    let recap = october(vec![
        event(noon(2026, 10, 1), 100),
        event(noon(2026, 9, 1), 40),
        event(at(2026, 9, 5, 23, 45), 10),
        // Sept 6 is not in "October 1-5 against September 1-5".
        event(noon(2026, 9, 6), 5_000),
    ]);
    assert_eq!(recap.previous_tokens, Some(50));
}

#[test]
fn a_finished_month_is_set_against_the_whole_month_before_clamped_to_its_length() {
    // March has 31 days and February 28: 31 days after Feb 1 is March 4, which is not February's.
    let events = vec![event(noon(2026, 3, 31), 100), event(noon(2026, 2, 28), 20), event(noon(2026, 3, 1), 7_000)];
    let recap = build_with(Period::Month { year: 2026, month: 3 }, vec![(SpendAgent::ClaudeCode, ledger(events))], noon(2026, 4, 10));
    assert_eq!(recap.previous_tokens, Some(20));
}

#[test]
fn januarys_previous_month_is_last_december_to_the_same_date() {
    let recap = build_with(
        Period::Month { year: 2026, month: 1 },
        vec![(
            SpendAgent::ClaudeCode,
            ledger(vec![event(noon(2025, 12, 15), 30), event(noon(2025, 12, 31), 900), event(noon(2026, 1, 3), 10)]),
        )],
        noon(2026, 1, 20),
    );
    assert_eq!(recap.previous_tokens, Some(30));
}

#[test]
fn a_previous_period_with_nothing_recorded_is_none_not_zero() {
    let recap = october(vec![event(noon(2026, 10, 2), 100), event(noon(2026, 8, 2), 100)]);
    assert_eq!(recap.previous_tokens, None);
}

// A year.

#[test]
fn a_year_has_twelve_months_the_days_it_has_had_and_last_years_same_days() {
    let l = ledger(vec![
        event(noon(2026, 1, 15), 100),
        event(noon(2026, 3, 1), 200),
        event(noon(2026, 3, 2), 300),
        event(noon(2026, 10, 3), 400),
        event(noon(2025, 1, 1), 50),
        event(noon(2025, 10, 5), 100),
        // The day after the same 278 days.
        event(noon(2025, 10, 6), 9_000),
    ]);
    let recap = build_with(Period::Year(2026), vec![(SpendAgent::ClaudeCode, l)], noon(2026, 10, 5));
    assert_eq!(recap.months.iter().map(|m| m.month).collect::<Vec<_>>(), (1..=12).collect::<Vec<_>>());
    assert_eq!(recap.months.iter().map(|m| m.tokens).collect::<Vec<_>>(), vec![100, 0, 500, 0, 0, 0, 0, 0, 0, 400, 0, 0]);
    assert_eq!(recap.months[2].active_days, 2);
    // Quiet months have no money, not $0.
    assert_eq!(recap.months[1].cost, None);
    assert!(recap.months[2].cost.is_some());
    assert_eq!(recap.days.len(), 278);
    assert_eq!(recap.elapsed_days, 278);
    assert!(recap.is_in_progress);
    assert_eq!(recap.previous_tokens, Some(150));
    assert_eq!(recap.tokens, 1_000);
}

#[test]
fn a_finished_year_runs_to_its_last_day() {
    let recap = build_with(Period::Year(2025), vec![(SpendAgent::ClaudeCode, ledger(vec![event(noon(2025, 12, 31), 10)]))], noon(2026, 10, 5));
    assert_eq!(recap.days.len(), 365);
    assert!(!recap.is_in_progress);
    assert_eq!(recap.end, date(2026, 1, 1));
    assert_eq!(recap.months[11].tokens, 10);
}

// What is missing is none.

#[test]
fn nothing_priced_means_no_money_and_no_cache_saving() {
    let l = ledger(vec![with(noon(2026, 10, 2), "mystery", TokenTally::new(100, 0, 900, 0))]);
    let recap = month_of(2026, 10, vec![(SpendAgent::ClaudeCode, l)]);
    assert_eq!(recap.tokens, 1_000);
    assert_eq!(recap.cost, None);
    assert_eq!(recap.cache_savings, None);
    assert_eq!(recap.days.iter().find(|d| d.tokens > 0).unwrap().cost, None);
    assert_eq!(recap.models.iter().map(|m| m.cost).collect::<Vec<_>>(), vec![None]);
    // The cache hit rate does not need a price.
    assert_eq!(recap.cache_hit_rate, Some(0.9));
}

#[test]
fn a_part_priced_total_is_its_priced_subtotal() {
    let l = ledger(vec![
        with(noon(2026, 10, 2), "claude", input(1_000_000)),
        with(noon(2026, 10, 2), "mystery", input(500)),
    ]);
    let recap = month_of(2026, 10, vec![(SpendAgent::ClaudeCode, l)]);
    assert_eq!(recap.cost, Some(3.0));
    let by_name: HashMap<_, _> = recap.models.iter().map(|m| (m.name.clone(), m.cost)).collect();
    assert_eq!(by_name["Claude"], Some(3.0));
    assert_eq!(by_name["mystery"], None);
}

#[test]
fn aggregate_timing_withholds_every_hour_figure() {
    let mut l = ledger(vec![event(at(2026, 10, 2, 23, 0), 100)]);
    l.has_aggregate_timing = true;
    let recap = month_of(2026, 10, vec![(SpendAgent::ClaudeCode, l)]);
    assert_eq!(recap.tokens, 100);
    assert_eq!(recap.hours, None);
    assert_eq!(recap.peak_hour, None);
    assert_eq!(recap.late_share, None);
    assert_eq!(recap.persona, None);
}

#[test]
fn a_ledger_with_no_quarter_hours_has_no_hour_shape_to_invent() {
    let day = LedgerDay::new(midnight(2026, 10, 2), 100, 1.0, 0, HashMap::from([("claude".to_string(), 100)]));
    let l = Ledger { earliest: Some(day.date), days: vec![day], ..Ledger::empty() };
    let recap = month_of(2026, 10, vec![(SpendAgent::ClaudeCode, l)]);
    assert_eq!(recap.tokens, 100);
    assert_eq!(recap.hours, None);
    assert_eq!(recap.persona, None);
}

#[test]
fn an_empty_month_is_empty_with_nothing_to_rank_and_nothing_invented() {
    let recap = october(vec![]);
    assert!(recap.is_empty());
    assert_eq!(recap.cost, None);
    assert_eq!(recap.hours, None);
    assert_eq!(recap.persona, None);
    assert_eq!(recap.busiest_day, None);
    assert_eq!(recap.cache_hit_rate, None);
    assert_eq!(recap.cache_savings, None);
    assert!(recap.models.is_empty() && recap.agents.is_empty() && recap.projects.is_empty());
    assert_eq!(recap.days.len(), 5);
    assert_eq!(recap.latest_minute, None);
    assert_eq!(recap.late_nights, 0);
}

#[test]
fn a_providers_own_statistics_are_not_part_of_a_recap() {
    let mut statistics = ledger(vec![event(noon(2026, 10, 2), 9_000)]);
    statistics.origin = Origin::ProviderStatistics;
    let recap = month_of(
        2026,
        10,
        vec![(SpendAgent::ClaudeCode, ledger(vec![event(noon(2026, 10, 2), 100)])), (SpendAgent::Codex, statistics)],
    );
    assert_eq!(recap.tokens, 100);
    assert_eq!(recap.agents.iter().map(|a| a.agent).collect::<Vec<_>>(), vec![SpendAgent::ClaudeCode]);
}

// Time of day.

#[test]
fn hours_the_peak_and_the_late_share_come_from_the_quarter_hours() {
    let recap = october(vec![
        event(at(2026, 10, 2, 22, 30), 300),
        event(at(2026, 10, 2, 22, 45), 100),
        event(at(2026, 10, 3, 3, 0), 100),
        event(at(2026, 10, 3, 14, 0), 500),
    ]);
    let hours = recap.hours.clone().unwrap();
    assert_eq!(hours.len(), 24);
    assert_eq!(hours[22], 400);
    assert_eq!(hours[3], 100);
    assert_eq!(hours[14], 500);
    assert_eq!(recap.peak_hour, Some(14));
    near(recap.late_share.unwrap(), 0.5, 0.0001);
}

#[test]
fn a_tie_for_the_peak_is_the_earlier_hour_every_time() {
    let recap = october(vec![event(at(2026, 10, 2, 16, 0), 100), event(at(2026, 10, 2, 9, 0), 100)]);
    assert_eq!(recap.peak_hour, Some(9));
}

#[test]
fn work_past_midnight_up_to_0500_counts_toward_its_night_once() {
    let mut l = ledger(vec![
        // A night spent across midnight: the session started the evening before.
        event(at(2026, 10, 2, 23, 30), 100),
        event(at(2026, 10, 3, 1, 0), 100),
        event(at(2026, 10, 3, 1, 15), 100),
        // A second night, as late as 04:58.
        event(at(2026, 10, 4, 4, 45), 100),
        // 05:00 is the morning.
        event(at(2026, 10, 5, 5, 0), 100),
    ]);
    l.sessions = vec![
        session(
            "a",
            None,
            at(2026, 10, 2, 23, 20),
            at(2026, 10, 3, 1, 22),
            vec![slot(at(2026, 10, 2, 23, 30), 100), slot(at(2026, 10, 3, 1, 0), 100), slot(at(2026, 10, 3, 1, 15), 100)],
        ),
        session("b", None, at(2026, 10, 4, 4, 40), at(2026, 10, 4, 4, 58), vec![slot(at(2026, 10, 4, 4, 45), 100)]),
    ];
    let recap = month_of(2026, 10, vec![(SpendAgent::ClaudeCode, l.clone())]);
    assert_eq!(recap.late_nights, 2);
    // 04:58, from the session's own last record rather than its quarter-hour.
    assert_eq!(recap.latest_minute, Some(4 * 60 + 58));

    // Without the session's end it is the quarter-hour's start.
    let mut bare = l;
    bare.sessions = vec![];
    assert_eq!(month_of(2026, 10, vec![(SpendAgent::ClaudeCode, bare)]).latest_minute, Some(4 * 60 + 45));
}

#[test]
fn no_work_after_midnight_is_no_late_night_not_a_zero_minute() {
    let recap = october(vec![event(at(2026, 10, 2, 22, 0), 100), event(at(2026, 10, 3, 5, 0), 100)]);
    assert_eq!(recap.late_nights, 0);
    assert_eq!(recap.latest_minute, None);
}

// Persona.

fn shape(fill: &[(usize, i64)]) -> Vec<i64> {
    let mut hours = vec![0; 24];
    for (hour, tokens) in fill {
        hours[*hour] += tokens;
    }
    hours
}

#[test]
fn a_persona_is_a_plain_majority_of_the_tokens_in_a_band_and_not_half_of_them() {
    // 21:00-04:59.
    assert_eq!(Persona::of(&shape(&[(23, 51), (14, 49)])), Some(Persona::NightOwl));
    assert_eq!(Persona::of(&shape(&[(2, 51), (14, 49)])), Some(Persona::NightOwl));
    // Exactly half is not a majority.
    assert_eq!(Persona::of(&shape(&[(23, 50), (14, 50)])), Some(Persona::AllDay));
    // The band ends are 21:00 in, 20:59 out; 04:59 in, 05:00 out.
    assert_eq!(Persona::of(&shape(&[(21, 60), (12, 40)])), Some(Persona::NightOwl));
    assert_eq!(Persona::of(&shape(&[(20, 60), (12, 40)])), Some(Persona::AllDay));
    assert_eq!(Persona::of(&shape(&[(4, 60), (12, 40)])), Some(Persona::NightOwl));
    assert_eq!(Persona::of(&shape(&[(5, 60), (12, 40)])), Some(Persona::EarlyBird));
    // 05:00-09:59.
    assert_eq!(Persona::of(&shape(&[(9, 51), (14, 49)])), Some(Persona::EarlyBird));
    assert_eq!(Persona::of(&shape(&[(10, 60), (20, 40)])), Some(Persona::DayShift));
    // 10:00-17:59.
    assert_eq!(Persona::of(&shape(&[(17, 51), (20, 49)])), Some(Persona::DayShift));
    assert_eq!(Persona::of(&shape(&[(18, 60), (12, 40)])), Some(Persona::AllDay));
    // Spread out.
    assert_eq!(Persona::of(&shape(&[(7, 30), (13, 30), (19, 20), (23, 20)])), Some(Persona::AllDay));
    // Nothing to read.
    assert_eq!(Persona::of(&[0; 24]), None);
    assert_eq!(Persona::of(&[1, 2, 3]), None);
}

#[test]
fn the_persona_comes_through_the_build() {
    let owl = october(vec![event(at(2026, 10, 2, 23, 0), 100), event(at(2026, 10, 3, 1, 0), 100), event(at(2026, 10, 3, 11, 0), 100)]);
    assert_eq!(owl.persona, Some(Persona::NightOwl));
    let day = october(vec![event(at(2026, 10, 2, 11, 0), 100)]);
    assert_eq!(day.persona, Some(Persona::DayShift));
}

// Cache.

#[test]
fn cache_savings_are_cache_reads_at_the_input_rate_less_what_they_cost() {
    let claude = ledger(vec![
        // 2M reads at $3 - $0.30 per million = $5.40.
        with(noon(2026, 10, 2), "claude", TokenTally::new(1_000, 0, 2_000_000, 0)),
        // No cache rate stated: billed as input, nothing saved.
        with(noon(2026, 10, 2), "flat", TokenTally::new(0, 0, 1_000_000, 0)),
        // No price at all: not guessed.
        with(noon(2026, 10, 2), "mystery", TokenTally::new(0, 0, 5_000_000, 0)),
    ]);
    let codex = ledger(vec![
        // 1M reads at $2 - $0.50 = $1.50.
        with(noon(2026, 10, 3), "gpt", TokenTally::new(10, 0, 1_000_000, 0)),
    ]);
    let recap = month_of(2026, 10, vec![(SpendAgent::ClaudeCode, claude), (SpendAgent::Codex, codex)]);
    near(recap.cache_savings.unwrap(), 6.9, 0.000001);
}

#[test]
fn a_long_context_requests_reads_are_saved_at_its_tiers_rates() {
    // One request over 272K, one under: the first is billed at the tier ($6 input, $0.60 cache
    // read), the second at the base rates.
    let big = TokenTally::new(0, 0, 1_000_000, 0).request(1_000_000);
    let small = TokenTally::new(0, 0, 1_000_000, 0);
    let l = ledger(vec![with(noon(2026, 10, 2), "tiered", big + small)]);
    let charged = l.days.iter().find(|d| d.tokens > 0).unwrap().model_costs["tiered"].cache_read;
    let recap = month_of(2026, 10, vec![(SpendAgent::Codex, l)]);
    // 1M x (6 - 0.6) + 1M x (3 - 0.3) per million.
    near(recap.cache_savings.unwrap(), 8.1, 0.000001);
    // And it is exactly what the ledger charged for those reads.
    near(charged, 0.6 + 0.3, 0.000001);
}

#[test]
fn no_cache_reads_no_savings_figure() {
    let recap = october(vec![event(noon(2026, 10, 2), 1_000)]);
    assert_eq!(recap.cache_savings, None);
    assert_eq!(recap.cache_hit_rate, Some(0.0));
}

#[test]
fn the_hit_rate_is_the_token_spend_rule_reads_over_all_input_output_left_out() {
    let l = ledger(vec![
        with(noon(2026, 10, 2), "claude", TokenTally::new(100, 100, 600, 5_000)),
        with(noon(2026, 10, 3), "claude", TokenTally::new(50, 50, 100, 0)),
    ]);
    let own = l.cache_hit_rate_in(400, noon(2026, 10, 5), &calendar()).unwrap();
    let recap = month_of(2026, 10, vec![(SpendAgent::ClaudeCode, l)]);
    let rate = recap.cache_hit_rate.unwrap();
    near(rate, 0.7, 0.0001);
    near(rate, own, 0.0001);
}

#[test]
fn a_hit_rate_over_several_agents_adds_the_measured_tallies() {
    let claude = ledger(vec![with(noon(2026, 10, 2), "claude", TokenTally::new(100, 0, 100, 0))]);
    let codex = ledger(vec![with(noon(2026, 10, 2), "gpt", TokenTally::new(100, 0, 700, 0))]);
    let recap = month_of(2026, 10, vec![(SpendAgent::ClaudeCode, claude), (SpendAgent::Codex, codex)]);
    near(recap.cache_hit_rate.unwrap(), 0.8, 0.0001);
    assert_eq!(recap.tokens, 200 + 800);
}

#[test]
fn an_agent_that_records_no_cache_is_left_out_of_the_rate_not_counted_as_misses() {
    let claude = ledger(vec![with(noon(2026, 10, 2), "claude", TokenTally::new(100, 0, 100, 0))]);
    // A store with no cache column: its 1,000 of input is not 1,000 misses.
    let mut keeps_no_cache = ledger(vec![with(noon(2026, 10, 2), "gpt", input(1_000))]);
    keeps_no_cache.reports_cache_reads = false;
    let both = month_of(2026, 10, vec![(SpendAgent::ClaudeCode, claude), (SpendAgent::Codex, keeps_no_cache.clone())]);
    near(both.cache_hit_rate.unwrap(), 0.5, 0.0001);
    assert_eq!(both.tokens, 200 + 1_000);

    let alone = month_of(2026, 10, vec![(SpendAgent::Codex, keeps_no_cache)]);
    assert_eq!(alone.cache_hit_rate, None);
}

#[test]
fn counts_that_may_be_missing_or_no_kind_can_claim_give_no_hit_rate() {
    let events = || vec![with(noon(2026, 10, 2), "claude", TokenTally::new(100, 0, 100, 0))];
    let mut partial = ledger(events());
    partial.has_partial_counts = true;
    let healthy = ledger(events());
    let both = month_of(2026, 10, vec![(SpendAgent::ClaudeCode, partial), (SpendAgent::Codex, healthy)]);
    // One agent's rate cannot stand for the whole.
    assert_eq!(both.cache_hit_rate, None);
    assert!(both.is_partial);

    let mut day = LedgerDay::new(midnight(2026, 10, 2), 700, 0.0, 0, HashMap::from([("m".to_string(), 700)]));
    day.tally = TokenTally::new(100, 0, 100, 0);
    let unclassified = Ledger { earliest: Some(day.date), days: vec![day], ..Ledger::empty() };
    assert_eq!(month_of(2026, 10, vec![(SpendAgent::ClaudeCode, unclassified)]).cache_hit_rate, None);
}

// Who, what, where.

#[test]
fn agents_models_and_projects_rank_heaviest_first_with_shares_of_the_whole() {
    let mut claude = ledger(vec![event(noon(2026, 10, 1), 100), event(noon(2026, 10, 2), 100), event(noon(2026, 10, 3), 100)]);
    claude.sessions = vec![session(
        "c",
        Some("/w/pulse"),
        noon(2026, 10, 1),
        noon(2026, 10, 3),
        vec![slot(noon(2026, 10, 1), 100), slot(noon(2026, 10, 2), 100), slot(noon(2026, 10, 3), 100)],
    )];
    let mut codex = ledger(vec![
        with(noon(2026, 10, 3), "gpt", input(300)),
        with(noon(2026, 10, 4), "gpt", input(300)),
    ]);
    codex.sessions = vec![
        session("x", Some("/w/pulse"), noon(2026, 10, 3), noon(2026, 10, 3), vec![slot(noon(2026, 10, 3), 300)]),
        session("y", Some("/w/other"), noon(2026, 10, 4), noon(2026, 10, 4), vec![slot(noon(2026, 10, 4), 300)]),
    ];
    let recap = month_of(2026, 10, vec![(SpendAgent::ClaudeCode, claude), (SpendAgent::Codex, codex)]);

    assert_eq!(recap.tokens, 900);
    assert_eq!(recap.sessions, 3);

    assert_eq!(recap.agents.iter().map(|a| a.agent).collect::<Vec<_>>(), vec![SpendAgent::Codex, SpendAgent::ClaudeCode]);
    assert_eq!(recap.agents.iter().map(|a| a.tokens).collect::<Vec<_>>(), vec![600, 300]);
    // Active days are the agent's own, not the span's: Codex on two days (one shared with
    // Claude Code), Claude Code on three.
    assert_eq!(recap.agents.iter().map(|a| a.active_days).collect::<Vec<_>>(), vec![2, 3]);
    // And which days, the shared day in both.
    let set = |days: &[u32]| days.iter().map(|d| date(2026, 10, *d)).collect::<BTreeSet<_>>();
    assert_eq!(recap.agents[0].active_dates, set(&[3, 4]));
    assert_eq!(recap.agents[1].active_dates, set(&[1, 2, 3]));
    assert!(recap.agents.iter().all(|a| a.active_dates.len() == a.active_days));
    assert_eq!(recap.active_days, 4);
    near(recap.agents[0].share, 2.0 / 3.0, 0.0001);

    assert_eq!(recap.models.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), vec!["GPT", "Claude"]);
    near(recap.models[0].share, 2.0 / 3.0, 0.0001);
    // 600 tokens at $2 and 300 at $3 per million.
    near(recap.models[0].cost.unwrap(), 0.0012, 1e-9);
    near(recap.models[1].cost.unwrap(), 0.0009, 1e-9);

    // One directory used by two agents is one project.
    assert_eq!(recap.projects.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), vec!["pulse", "other"]);
    assert_eq!(recap.projects.iter().map(|p| p.tokens).collect::<Vec<_>>(), vec![600, 300]);
    assert_eq!(recap.projects.iter().map(|p| p.sessions).collect::<Vec<_>>(), vec![2, 1]);
    near(recap.projects[1].share, 1.0 / 3.0, 0.0001);
}

#[test]
fn the_busiest_day_is_the_earliest_of_the_heaviest_and_streaks_are_the_periods_own() {
    let recap = october(vec![
        event(noon(2026, 9, 29), 10),
        event(noon(2026, 9, 30), 10),
        event(noon(2026, 10, 1), 10),
        event(noon(2026, 10, 2), 400),
        event(noon(2026, 10, 3), 400),
        event(noon(2026, 10, 5), 10),
    ]);
    assert_eq!(recap.busiest_day.as_ref().map(|d| d.date), Some(date(2026, 10, 2)));
    assert_eq!(recap.busiest_day.as_ref().map(|d| d.tokens), Some(400));
    // 29 Sep is September's: the run inside October is the 1st to the 3rd. The 5th is a new run
    // that is still going (today), one day.
    assert_eq!(recap.longest_streak, 3);
    assert_eq!(recap.current_streak, 1);
    assert_eq!(recap.currency, "USD");
    assert!(!recap.is_partial);
}

#[test]
fn a_past_months_streaks_stop_at_the_months_edges_and_its_current_one_is_how_it_ended() {
    let mut events: Vec<Event> =
        [1, 2, 3, 10, 11, 12, 13, 14, 28, 29, 30].iter().map(|d| event(noon(2026, 9, *d), 10)).collect();
    // October's own run must not reach back, nor add to September.
    events.extend((1..=4).map(|d| event(noon(2026, 10, d), 10)));
    events.extend([event(noon(2026, 8, 30), 10), event(noon(2026, 8, 31), 10)]);
    let recap = month_of(2026, 9, vec![(SpendAgent::ClaudeCode, ledger(events))]);
    assert!(!recap.is_in_progress);
    assert_eq!(recap.longest_streak, 5);
    // The run that ends on 30 September (28, 29, 30), not October's.
    assert_eq!(recap.current_streak, 3);
    assert!(recap.longest_streak <= recap.elapsed_days);
}

#[test]
fn a_past_month_that_ended_on_a_quiet_day_has_no_current_run_and_a_running_months_grace_is_today_only() {
    let quiet_end = month_of(2026, 9, vec![(SpendAgent::ClaudeCode, ledger(vec![event(noon(2026, 9, 20), 10), event(noon(2026, 9, 21), 10)]))]);
    assert_eq!(quiet_end.longest_streak, 2);
    assert_eq!(quiet_end.current_streak, 0);

    // Today (5 Oct) has had nothing yet: yesterday's run is still current.
    let before = october((2..=4).map(|d| event(noon(2026, 10, d), 10)).collect());
    assert_eq!(before.current_streak, 3);
    // Two quiet days end it.
    let lapsed = october((1..=3).map(|d| event(noon(2026, 10, d), 10)).collect());
    assert_eq!(lapsed.current_streak, 0);
    assert_eq!(lapsed.longest_streak, 3);
    // The first of the month with nothing yet: yesterday is not in the period.
    let first = october_at(vec![event(noon(2026, 9, 30), 10)], noon(2026, 10, 1));
    assert_eq!(first.tokens, 0);
    assert_eq!(first.current_streak, 0);
}

#[test]
fn the_streak_helper_counts_runs_of_days_with_tokens_with_todays_grace_only_while_running() {
    let days = |tokens: &[i64]| -> Vec<Day> {
        tokens
            .iter()
            .enumerate()
            .map(|(i, t)| Day { date: date(2026, 1, 1) + Duration::days(i as i64), tokens: *t, cost: None })
            .collect()
    };
    assert_eq!(Recap::streaks(&[], true), (0, 0));
    let shape = days(&[1, 1, 0, 1, 1, 1, 0]);
    assert_eq!(Recap::streaks(&shape, false), (0, 3));
    assert_eq!(Recap::streaks(&shape, true), (3, 3));
    assert_eq!(Recap::streaks(&days(&[0, 0, 5, 5]), true), (2, 2));
}

#[test]
fn tokens_with_no_published_price_are_carried_so_the_money_can_be_called_a_floor() {
    let mixed = month_of(
        2026,
        10,
        vec![(
            SpendAgent::ClaudeCode,
            ledger(vec![with(noon(2026, 10, 2), "claude", input(1_000_000)), with(noon(2026, 10, 2), "mystery", input(500))]),
        )],
    );
    assert_eq!(mixed.unpriced_tokens, 500);
    assert_eq!(mixed.cost, Some(3.0));

    let none = month_of(2026, 10, vec![(SpendAgent::ClaudeCode, ledger(vec![with(noon(2026, 10, 2), "claude", input(1_000))]))]);
    assert_eq!(none.unpriced_tokens, 0);

    let all_unpriced = month_of(2026, 10, vec![(SpendAgent::ClaudeCode, ledger(vec![with(noon(2026, 10, 2), "mystery", input(1_000))]))]);
    assert_eq!(all_unpriced.unpriced_tokens, 1_000);
    assert_eq!(all_unpriced.cost, None);
}

#[test]
fn one_unpriced_day_takes_the_posters_cost_line_away_rather_than_drawing_a_zero_for_it() {
    let series = |mystery_day: Option<u32>| {
        let mut events = vec![event(noon(2026, 10, 1), 1_000_000), event(noon(2026, 10, 3), 1_000_000)];
        if let Some(day) = mystery_day {
            events.push(with(noon(2026, 10, day), "mystery", input(500)));
        }
        let recap = month_of(2026, 10, vec![(SpendAgent::ClaudeCode, ledger(events))]);
        super::deck::cost_series(&recap)
    };
    // October 2nd is quiet: a real zero between priced days.
    assert_eq!(series(None), vec![3.0, 0.0, 3.0, 0.0, 0.0]);
    // On the 4th there was work with no price: no line.
    assert!(series(Some(4)).is_empty());
}

// The command and the report.

#[test]
fn a_period_key_is_a_month_a_year_or_nothing() {
    assert_eq!(Period::from_key("2026-09"), Some(Period::Month { year: 2026, month: 9 }));
    assert_eq!(Period::from_key("2026"), Some(Period::Year(2026)));
    assert_eq!(Period::from_key("2026-13"), None);
    assert_eq!(Period::from_key("2026-9"), None);
    assert_eq!(Period::from_key("26"), None);
    assert_eq!(Period::from_key("2026-09-01"), None);
    assert_eq!(Period::from_key("september"), None);
    assert_eq!(Period::Month { year: 2026, month: 9 }.key(), "2026-09");
    assert_eq!(Period::Year(2026).key(), "2026");
}

#[test]
fn the_serialised_recap_leaves_a_missing_figure_out_rather_than_printing_a_zero() {
    let recap = build_with(Period::Month { year: 2026, month: 10 }, vec![], noon(2026, 10, 5));
    let json = serde_json::to_value(&recap).unwrap();
    assert_eq!(json["period"], "2026-10");
    assert_eq!(json["start"], "2026-10-01");
    assert_eq!(json["end"], "2026-10-06");
    assert_eq!(json["tokens"], 0);
    for absent in ["cost", "hours", "persona", "cacheHitRate", "previousTokens"] {
        assert!(json.get(absent).is_none(), "{absent} should be absent");
    }

    let full = october(vec![event(at(2026, 10, 2, 23, 0), 1_000_000)]);
    let json = serde_json::to_value(&full).unwrap();
    assert_eq!(json["persona"], "nightOwl");
    assert_eq!(json["cost"], 3.0);
    assert_eq!(json["hours"].as_array().unwrap().len(), 24);
    // Each agent carries the days it worked, as dates.
    assert_eq!(json["agents"][0]["activeDates"], serde_json::json!(["2026-10-02"]));
    assert_eq!(json["agents"][0]["activeDays"], 1);
}

#[test]
fn a_year_whose_records_begin_in_may_counts_from_may_not_january() {
    let recap = build_with(
        Period::Year(2026),
        vec![(SpendAgent::ClaudeCode, ledger(vec![event(noon(2026, 5, 14), 100), event(noon(2026, 9, 2), 300)]))],
        noon(2026, 10, 9),
    );
    assert_eq!(recap.records_begin, Some(date(2026, 5, 14)));
    // January 1 to October 9 has been 282 days; May 14 to October 9, 149.
    assert_eq!(recap.elapsed_days, 282);
    assert_eq!(recap.observed_days(), 149);
    assert!(recap.is_before_records(date(2026, 5, 13)));
    assert!(!recap.is_before_records(date(2026, 5, 14)));

    let insights = super::insights::RecapInsights::new(&recap);
    assert!((0..4).all(|i| insights.is_month_before_records(i)));
    assert!(!insights.is_month_before_records(4), "May holds the first record");
    assert!(insights.is_month_unrecorded(11), "December is still to come");
    let split = insights.work_split().unwrap();
    assert_eq!(split.weekday_days + split.weekend_days, 149);

    // The plan price runs from the first record too.
    assert!((super::deck::paid_months(&recap) - 12.0 * 149.0 / 365.0).abs() < 1e-9);
}

#[test]
fn records_reaching_back_before_the_period_leave_it_whole() {
    let recap = october(vec![event(noon(2026, 9, 20), 100), event(noon(2026, 10, 2), 100)]);
    assert_eq!(recap.records_begin, None);
    assert_eq!(recap.observed_days(), recap.elapsed_days);
}
