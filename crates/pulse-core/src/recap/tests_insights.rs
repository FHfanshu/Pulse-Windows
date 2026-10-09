// Ported from upstream Tests/PulseTests/RecapInsightsTests.swift.
//! What the opener, calendar, timetable and payback cards read off a recap: weekday and week
//! totals, the streak chain, the quarters of the day, money per million tokens, and the per-agent
//! strips. All worked out apart from any drawing.

use std::collections::BTreeSet;

use chrono::NaiveDate;

use super::insights::{Mark, RecapInsights};
use super::{AgentShare, Day, Month, Period, Recap};
use crate::spend::SpendAgent;

fn date(month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, month, day).unwrap()
}

fn date_in(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

/// September 2026 from day 1 (a Tuesday) for as many days as `tokens` has.
fn september(tokens: &[i64]) -> Vec<Day> {
    september_priced(tokens, |_| None)
}

fn september_priced(tokens: &[i64], cost: impl Fn(i64) -> Option<f64>) -> Vec<Day> {
    tokens
        .iter()
        .enumerate()
        .map(|(index, count)| Day { date: date(9, index as u32 + 1), tokens: *count, cost: if *count > 0 { cost(*count) } else { None } })
        .collect()
}

#[derive(Default)]
struct Spec {
    year: Option<i32>,
    months: Vec<Month>,
    agents: Vec<AgentShare>,
    hours: Option<Vec<i64>>,
    cost: Option<f64>,
    unpriced_tokens: i64,
}

fn recap_of(days: Vec<Day>, spec: Spec) -> Recap {
    let tokens = days.iter().map(|d| d.tokens).sum();
    Recap {
        period: spec.year.map_or(Period::Month { year: 2026, month: 9 }, Period::Year),
        start: days.first().map_or(date(9, 1), |d| d.date),
        end: date(10, 1),
        is_in_progress: false,
        tokens,
        cost: spec.cost,
        unpriced_tokens: spec.unpriced_tokens,
        previous_tokens: None,
        active_days: days.iter().filter(|d| d.tokens > 0).count(),
        elapsed_days: days.len(),
        sessions: 1,
        months: spec.months,
        hours: spec.hours,
        peak_hour: None,
        late_share: None,
        latest_minute: None,
        late_nights: 0,
        persona: None,
        models: vec![],
        agents: spec.agents,
        projects: vec![],
        cache_hit_rate: None,
        cache_savings: None,
        current_streak: 0,
        longest_streak: 0,
        busiest_day: None,
        currency: "USD".to_string(),
        is_partial: false,
        days,
    }
}

fn recap(days: Vec<Day>) -> Recap {
    recap_of(days, Spec::default())
}

// Weekdays and weekends.

#[test]
fn tokens_by_weekday_monday_first_and_a_weekday_the_period_never_had_is_none_not_zero() {
    // Tue 1, Wed 2, Thu 3: no Monday, Friday, Saturday or Sunday yet.
    let few = recap(september(&[100, 40, 0]));
    let few = RecapInsights::new(&few);
    assert_eq!(few.weekday_tokens(), [None, Some(100), Some(40), Some(0), None, None, None]);
    assert_eq!(few.busiest_weekday(), Some(1));

    // Ten days: Tue Sep 1 through Thu Sep 10, two Tuesdays.
    let ten = recap(september(&[100, 0, 0, 0, 5, 7, 1, 50, 0, 3]));
    let ten = RecapInsights::new(&ten);
    assert_eq!(ten.weekday_tokens(), [Some(1), Some(150), Some(0), Some(3), Some(0), Some(5), Some(7)]);
    assert_eq!(ten.busiest_weekday(), Some(1));
}

#[test]
fn the_busiest_weekday_is_the_earliest_of_a_tie_and_none_when_nothing_was_used() {
    let tie = recap(september(&[10, 10, 0, 0, 0, 0, 0]));
    assert_eq!(RecapInsights::new(&tie).busiest_weekday(), Some(1));
    let none = recap(september(&[0, 0, 0]));
    assert_eq!(RecapInsights::new(&none).busiest_weekday(), None);
}

#[test]
fn weekdays_against_the_weekend_tokens_days_the_period_had_and_days_used() {
    // Sep 1-10: weekend days are 5 and 6.
    let r = recap(september(&[100, 0, 0, 0, 30, 0, 50, 20, 0, 0]));
    let split = RecapInsights::new(&r).work_split().unwrap();
    assert_eq!(split.weekday_tokens, 170);
    assert_eq!(split.weekend_tokens, 30);
    assert_eq!(split.weekday_days, 8);
    assert_eq!(split.weekend_days, 2);
    assert_eq!(split.weekday_active, 3);
    assert_eq!(split.weekend_active, 1);
    assert!((split.weekday_share() - 0.85).abs() < 1e-9);
    assert!((split.weekend_share() - 0.15).abs() < 1e-9);
}

#[test]
fn nothing_to_split_with_no_work() {
    assert!(RecapInsights::new(&recap(september(&[0, 0, 0]))).work_split().is_none());
    assert!(RecapInsights::new(&recap(vec![])).work_split().is_none());
}

#[test]
fn tokens_on_an_average_day_with_work() {
    assert_eq!(RecapInsights::new(&recap(september(&[100, 0, 50]))).tokens_per_active_day(), Some(75));
    assert_eq!(RecapInsights::new(&recap(september(&[0, 0]))).tokens_per_active_day(), None);
}

// Weeks.

#[test]
fn weeks_run_monday_to_sunday_and_are_clipped_to_the_month() {
    let days: Vec<Day> = (1..=30).map(|d| Day { date: date(9, d), tokens: d as i64, cost: None }).collect();
    let r = recap(days);
    let insights = RecapInsights::new(&r);
    let weeks = insights.weeks();
    assert_eq!(weeks.len(), 5);
    assert_eq!(weeks.iter().map(|w| w.first).collect::<Vec<_>>(), [1, 7, 14, 21, 28].map(|d| date(9, d)));
    assert_eq!(weeks.iter().map(|w| w.last).collect::<Vec<_>>(), [6, 13, 20, 27, 30].map(|d| date(9, d)));
    assert_eq!(weeks.iter().map(|w| w.tokens).collect::<Vec<_>>(), vec![21, 70, 119, 168, 87]);
    assert_eq!(insights.busiest_week(), Some(3));
}

#[test]
fn a_quiet_week_is_there_with_nothing_in_it_and_the_busiest_is_the_earliest_of_a_tie() {
    let mut days: Vec<Day> = (1..=14).map(|d| Day { date: date(9, d), tokens: 0, cost: None }).collect();
    days[0].tokens = 10;
    days[7].tokens = 10;
    let r = recap(days);
    let insights = RecapInsights::new(&r);
    assert_eq!(insights.weeks().iter().map(|w| w.tokens).collect::<Vec<_>>(), vec![10, 10, 0]);
    assert_eq!(insights.busiest_week(), Some(0));
}

#[test]
fn a_month_running_only_a_few_days_has_a_short_week_and_nothing_past_today() {
    let r = recap(september(&[5; 8]));
    let weeks = RecapInsights::new(&r).weeks();
    assert_eq!(weeks.iter().map(|w| w.first).collect::<Vec<_>>(), vec![date(9, 1), date(9, 7)]);
    assert_eq!(weeks.iter().map(|w| w.last).collect::<Vec<_>>(), vec![date(9, 6), date(9, 8)]);
}

// The streak.

#[test]
fn the_longest_run_is_the_earliest_of_a_tie_and_none_with_no_work() {
    let r = recap(september(&[1, 0, 1, 1, 0, 1, 1, 0, 1]));
    let run = RecapInsights::new(&r).longest_run().unwrap();
    assert_eq!(run.first, date(9, 3));
    assert_eq!(run.last, date(9, 4));
    assert_eq!(run.length, 2);
    let quiet = recap(september(&[0, 0]));
    assert!(RecapInsights::new(&quiet).longest_run().is_none());
    assert!(RecapInsights::new(&quiet).streak_chain().is_none());
}

#[test]
fn the_chain_is_a_window_of_nine_around_the_run_kept_inside_the_month() {
    // A three-day run in the middle: three days either side.
    let mut tokens = [0i64; 30];
    for day in [25, 26, 27] {
        tokens[day - 1] = 1;
    }
    let r = recap(september(&tokens));
    let middle = RecapInsights::new(&r).streak_chain().unwrap();
    assert_eq!(middle.days.len(), 9);
    assert_eq!(middle.days.first(), Some(&date(9, 22)));
    assert_eq!(middle.days.last(), Some(&date(9, 30)));
    assert_eq!(middle.run(), 3..6);
    assert!(middle.first == date(9, 22) && middle.last == date(9, 30));

    // A run at the very start cannot be centred: the window stays inside.
    let mut early = [0i64; 30];
    early[0] = 1;
    early[1] = 1;
    let r = recap(september(&early));
    let start = RecapInsights::new(&r).streak_chain().unwrap();
    assert_eq!(start.days.first(), Some(&date(9, 1)));
    assert_eq!(start.days.last(), Some(&date(9, 9)));
    assert_eq!(start.run(), 0..2);
}

#[test]
fn a_month_shorter_than_the_window_is_all_of_it_and_a_long_run_shows_its_first_nine_days() {
    let r = recap(september(&[0, 1, 1, 0]));
    let short = RecapInsights::new(&r).streak_chain().unwrap();
    assert_eq!(short.days.len(), 4);
    assert_eq!(short.run(), 1..3);

    let mut tokens = vec![1i64; 20];
    tokens.extend([0, 0]);
    let r = recap(september(&tokens));
    let long = RecapInsights::new(&r).streak_chain().unwrap();
    assert_eq!(long.days.len(), 9);
    assert_eq!(long.run(), 0..9);
    // The label names where the run really ends.
    assert_eq!(long.first, date(9, 1));
    assert_eq!(long.last, date(9, 20));
}

// Time of day.

fn hours(shape: &[(usize, i64)]) -> Vec<i64> {
    let mut hours = vec![0; 24];
    for (hour, tokens) in shape {
        hours[*hour] = *tokens;
    }
    hours
}

fn with_hours(shape: Option<Vec<i64>>) -> Recap {
    recap_of(september(&[1]), Spec { hours: shape, ..Spec::default() })
}

#[test]
fn the_day_starts_at_the_first_hour_from_0500_with_any_work_and_not_at_the_after_midnight_tail() {
    assert_eq!(RecapInsights::new(&with_hours(Some(hours(&[(0, 5), (10, 3), (12, 9)])))).first_hour(), Some(10));
    assert_eq!(RecapInsights::new(&with_hours(Some(hours(&[(5, 1), (10, 3)])))).first_hour(), Some(5));
    assert_eq!(RecapInsights::new(&with_hours(Some(hours(&[(0, 5), (3, 2)])))).first_hour(), None);
    assert_eq!(RecapInsights::new(&with_hours(None)).first_hour(), None);
}

#[test]
fn four_quarters_of_the_day_each_a_share_of_the_hour_shape() {
    let r = with_hours(Some(hours(&[(1, 10), (7, 30), (13, 20), (20, 40)])));
    let insights = RecapInsights::new(&r);
    let quarters = insights.quarters().unwrap();
    assert_eq!(quarters.iter().map(|q| q.from).collect::<Vec<_>>(), vec![0, 6, 12, 18]);
    assert_eq!(quarters.iter().map(|q| q.tokens).collect::<Vec<_>>(), vec![10, 30, 20, 40]);
    assert!((quarters[3].share - 0.4).abs() < 1e-9);
    assert_eq!(insights.leading_quarter(), Some(3));
    assert!(RecapInsights::new(&with_hours(None)).quarters().is_none());
    assert!(RecapInsights::new(&with_hours(Some(hours(&[])))).quarters().is_none());
}

#[test]
fn a_tie_between_quarters_goes_to_the_earlier_one() {
    let r = with_hours(Some(hours(&[(2, 10), (20, 10)])));
    assert_eq!(RecapInsights::new(&r).leading_quarter(), Some(0));
}

// Money.

#[test]
fn dollars_per_million_tokens_count_the_priced_tokens_only() {
    let days = || september(&[1_000_000, 1_000_000]);
    let with = |cost, unpriced| recap_of(days(), Spec { cost, unpriced_tokens: unpriced, ..Spec::default() });
    assert_eq!(RecapInsights::new(&with(Some(10.0), 0)).cost_per_million(), Some(5.0));
    // A quarter unpriced: the cost is for the other three.
    assert_eq!(RecapInsights::new(&with(Some(6.0), 500_000)).cost_per_million(), Some(4.0));
    assert_eq!(RecapInsights::new(&with(None, 0)).cost_per_million(), None);
    assert_eq!(RecapInsights::new(&with(Some(6.0), 2_000_000)).cost_per_million(), None);
    assert_eq!(RecapInsights::new(&with(Some(10.0), 0)).cost_per_active_day(), Some(5.0));
}

#[test]
fn the_dearest_day_is_named_only_when_every_working_day_has_a_price() {
    let priced = september_priced(&[10, 0, 30, 20], |c| Some(c as f64));
    let r = recap_of(priced.clone(), Spec { cost: Some(60.0), ..Spec::default() });
    assert_eq!(RecapInsights::new(&r).costliest_day().unwrap().date, date(9, 3));

    let mut unpriced = priced;
    unpriced[1] = Day { date: date(9, 2), tokens: 99, cost: None };
    let r = recap_of(unpriced, Spec { cost: Some(60.0), ..Spec::default() });
    assert!(RecapInsights::new(&r).costliest_day().is_none());
    assert!(RecapInsights::new(&recap(september(&[10, 20]))).costliest_day().is_none());
}

#[test]
fn the_earliest_of_equal_days_is_the_dearest() {
    let days = september_priced(&[10, 10], |c| Some(c as f64));
    let r = recap_of(days, Spec { cost: Some(20.0), ..Spec::default() });
    assert_eq!(RecapInsights::new(&r).costliest_day().unwrap().date, date(9, 1));
}

#[test]
fn daily_money_is_a_series_only_if_every_working_day_has_a_price_a_quiet_one_is_a_real_zero() {
    let priced = september_priced(&[10, 0, 30], |c| Some(c as f64));
    let r = recap_of(priced.clone(), Spec { cost: Some(40.0), ..Spec::default() });
    assert_eq!(RecapInsights::new(&r).cost_bars(), Some(vec![Some(10.0), Some(0.0), Some(30.0)]));

    let mut partial = priced;
    partial[2] = Day { date: date(9, 3), tokens: 30, cost: None };
    let r = recap_of(partial, Spec { cost: Some(10.0), ..Spec::default() });
    assert_eq!(RecapInsights::new(&r).cost_bars(), None);
    assert_eq!(RecapInsights::new(&recap(september(&[10, 20]))).cost_bars(), None);
}

#[test]
fn a_years_money_is_by_month_and_one_unpriced_month_takes_it_away() {
    let months: Vec<Month> = (1..=12)
        .map(|m| Month { month: m, tokens: if m == 4 { 0 } else { 100 }, cost: if m == 4 { None } else { Some(m as f64) }, active_days: 1 })
        .collect();
    let year = recap_of(september(&[1]), Spec { year: Some(2026), months: months.clone(), cost: Some(70.0), ..Spec::default() });
    let expected: Vec<Option<f64>> = [1.0, 2.0, 3.0, 0.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0].map(Some).to_vec();
    assert_eq!(RecapInsights::new(&year).cost_bars(), Some(expected));

    // A free model's day is priced at zero, and is still a price.
    let free = september_priced(&[10, 20], |c| Some(if c == 10 { 0.0 } else { 5.0 }));
    let r = recap_of(free, Spec { cost: Some(5.0), ..Spec::default() });
    assert_eq!(RecapInsights::new(&r).cost_bars(), Some(vec![Some(0.0), Some(5.0)]));

    let mut broken = months;
    broken[8] = Month { month: 9, tokens: 100, cost: None, active_days: 1 };
    let r = recap_of(september(&[1]), Spec { year: Some(2026), months: broken, cost: Some(70.0), ..Spec::default() });
    assert_eq!(RecapInsights::new(&r).cost_bars(), None);
}

// Who worked when.

fn agent(agent: SpendAgent, on: &[NaiveDate]) -> AgentShare {
    AgentShare {
        agent,
        name: agent.display_name(),
        icon: agent.icon_resource(),
        tokens: 1,
        share: 0.5,
        active_days: on.len(),
        cost: None,
        active_dates: on.iter().copied().collect::<BTreeSet<_>>(),
    }
}

#[test]
fn a_strip_says_used_another_agents_day_a_quiet_weekend_or_a_quiet_weekday() {
    // Sep 1 Tue ... Sep 8 Tue. Work: Tue 1 (this agent), Wed 2 (another), Sat 5 (another); quiet:
    // the rest.
    let days = september(&[5, 5, 0, 0, 5, 0, 0, 0]);
    let claude = agent(SpendAgent::ClaudeCode, &[date(9, 1)]);
    let r = recap_of(days, Spec { agents: vec![claude.clone()], ..Spec::default() });
    let insights = RecapInsights::new(&r);
    assert_eq!(
        insights.day_marks(&claude),
        vec![Mark::Used, Mark::Other, Mark::Quiet, Mark::Quiet, Mark::Other, Mark::QuietWeekend, Mark::Quiet, Mark::Quiet]
    );
    assert_eq!(insights.marks(&claude), insights.day_marks(&claude));
}

#[test]
fn in_a_year_the_strip_has_a_cell_per_month_used_other_or_quiet() {
    let months: Vec<Month> =
        (1..=12).map(|m| Month { month: m, tokens: if m == 3 { 0 } else { 10 }, cost: None, active_days: 1 }).collect();
    let codex = agent(SpendAgent::Codex, &[date_in(2025, 1, 12), date_in(2025, 6, 3)]);
    let r = recap_of(september(&[1]), Spec { year: Some(2025), months, agents: vec![codex.clone()], ..Spec::default() });
    let marks = RecapInsights::new(&r).marks(&codex);
    assert_eq!(marks.len(), 12);
    assert_eq!(marks[0], Mark::Used);
    assert_eq!(marks[2], Mark::Quiet);
    assert_eq!(marks[1], Mark::Other);
    assert_eq!(marks[5], Mark::Used);
}

#[test]
fn in_a_running_year_the_months_to_come_are_neither_quiet_nor_zero() {
    // 2026 read on 10 March: April to December have not begun.
    let months: Vec<Month> = (1..=12)
        .map(|m| Month {
            month: m,
            tokens: if m <= 3 { 10 } else { 0 },
            cost: if m <= 3 { Some(1.0) } else { None },
            active_days: if m <= 3 { 1 } else { 0 },
        })
        .collect();
    let codex = agent(SpendAgent::Codex, &[date(2, 3)]);
    let mut running = recap_of(
        vec![Day { date: date(3, 10), tokens: 5, cost: Some(1.0) }],
        Spec { year: Some(2026), months, agents: vec![codex.clone()], cost: Some(3.0), ..Spec::default() },
    );
    running.start = date(1, 1);
    running.end = date(3, 11);
    running.is_in_progress = true;
    let insights = RecapInsights::new(&running);
    assert!(!insights.is_month_to_come(2));
    assert!(insights.is_month_to_come(3));
    let marks = insights.marks(&codex);
    assert_eq!(marks[1], Mark::Used);
    assert!(marks[3..].iter().all(|m| *m == Mark::ToCome));
    let bars = insights.cost_bars().unwrap();
    assert!(bars[..3].iter().all(|b| *b == Some(1.0)));
    assert!(bars[3..].iter().all(Option::is_none));
}
