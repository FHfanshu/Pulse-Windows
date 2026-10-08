// Ported from upstream Sources/Pulse/Usage/TokenActivity.swift.
//! The last twelve months of work, one number per calendar day: what the Token spend pane's
//! "Token activity" chart draws as a grid, as weekly bars and as a running total.
//!
//! Independent of the span picker: it always covers the year to today. It is built from the same
//! ledgers the pane already holds, so there is no further read of any store.
//!
//! A day nobody worked is quiet; a day nothing was recorded for carries no number. Days before the
//! first record across the agents counted, and days after today, are `None` in `Week::days`: no
//! bar, no point, no zero. Only days from the first record on can be quiet. The grid still draws
//! the window's days before the first record as placeholder squares (`Week::unrecorded`).
//!
//! The window is the last twelve months in at most 53 week columns. It starts at
//! `today - 1 year + 1 day`, pulled forward when that would need a 54th column, and each column
//! starts on the calendar's own first weekday. The first and last columns are partial.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::Serialize;

use super::agent::SpendAgent;
use super::calendar::Calendar;
use super::ledger::Ledger;

/// One drawn day.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityDay {
    pub date: DateTime<Utc>,
    pub tokens: i64,
}

/// One column of the grid: seven positions from the calendar's first weekday.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Week {
    /// The first day of the column, which may be before the window.
    pub start: DateTime<Utc>,
    /// Exactly seven entries; None where nothing is drawn.
    pub days: Vec<Option<ActivityDay>>,
    /// Exactly seven entries; true where a day is inside the window and not after today but has no
    /// record yet: a placeholder, never a count.
    pub unrecorded: Vec<bool>,
}

impl Week {
    pub fn drawn(&self) -> impl Iterator<Item = &ActivityDay> {
        self.days.iter().flatten()
    }

    pub fn has_data(&self) -> bool {
        self.days.iter().any(Option::is_some)
    }

    pub fn tokens(&self) -> i64 {
        self.drawn().map(|d| d.tokens).sum()
    }

    /// The drawn range, for a label: partial weeks are clipped to it.
    pub fn range(&self) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
        Some((self.drawn().next()?.date, self.drawn().last()?.date))
    }
}

/// One point of the running total. `position` is in week columns, so the three views share one
/// horizontal scale and one set of month labels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Point {
    pub date: DateTime<Utc>,
    pub running: i64,
    pub position: f64,
}

/// A month label under the chart: the column its first day falls in, that day, and where it sits
/// in week columns (`column + weekday row / 7`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Mark {
    pub column: usize,
    pub date: DateTime<Utc>,
    pub position: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenActivity {
    /// The columns, oldest first. Empty when there is no record at all.
    pub weeks: Vec<Week>,
    /// Every drawn day's tokens, added up.
    pub total: i64,
    /// Drawn days with work on them.
    pub active_days: usize,
    /// The three upper bounds of colour steps 1-3 (a fourth step is anything above the last): the
    /// quartiles of the days with work, so one huge day does not flatten the rest of the year
    /// into the palest step.
    pub cuts: Vec<i64>,
    /// Whether a ledger that contributed to the window may be missing counts.
    pub has_partial_counts: bool,
}

impl TokenActivity {
    pub fn is_empty(&self) -> bool {
        self.weeks.is_empty()
    }

    pub fn column_count(&self) -> usize {
        self.weeks.len()
    }

    /// 0 for a quiet day, 1-4 for a day with work, by quartile of the busy days.
    pub fn step(&self, tokens: i64) -> usize {
        if tokens <= 0 {
            return 0;
        }
        1 + self.cuts.iter().filter(|&&cut| tokens > cut).count()
    }

    /// The running total, one point per drawn day.
    pub fn points(&self) -> Vec<Point> {
        let mut running = 0;
        let mut result = Vec::new();
        for (column, week) in self.weeks.iter().enumerate() {
            for (row, day) in week.days.iter().enumerate() {
                let Some(day) = day else { continue };
                running += day.tokens;
                result.push(Point { date: day.date, running, position: column as f64 + (row as f64 + 0.5) / 7.0 });
            }
        }
        result
    }

    /// Month labels at each month's first day (the 1st, or the window's first day for the month
    /// the window opens in) through to the running month. A label sits where that day sits, so
    /// months read as evenly spaced as the calendar is: 28 to 31 days is 4 to 4.4 columns. When
    /// two labels are closer than `minimum_gap` columns the earlier one goes, so the newest month
    /// is always named.
    pub fn month_marks(&self, calendar: &Calendar, minimum_gap: f64) -> Vec<Mark> {
        let mut kept: Vec<Mark> = Vec::new();
        let mut previous: Option<u32> = None;
        for (column, week) in self.weeks.iter().enumerate() {
            for row in 0..week.days.len() {
                if week.days[row].is_none() && !week.unrecorded[row] {
                    continue;
                }
                let date = calendar.add_days(week.start, row as i64);
                let month = calendar.month(date);
                if previous != Some(month) {
                    let position = column as f64 + row as f64 / 7.0;
                    if kept.last().is_some_and(|last| position - last.position < minimum_gap) {
                        kept.pop();
                    }
                    kept.push(Mark { column, date, position });
                }
                previous = Some(month);
            }
        }
        kept
    }

    /// The window `of` draws for a given day: the first drawn day and the first column's start.
    pub fn window(today: DateTime<Utc>, calendar: &Calendar) -> (DateTime<Utc>, DateTime<Utc>) {
        let year = calendar.add_years(today, -1);
        let candidate = calendar.start_of_day(calendar.add_days(year, 1));
        let longest = calendar.start_of_day(calendar.add_days(calendar.week_start(today), -52 * 7));
        let start = candidate.max(longest);
        (start, calendar.week_start(start))
    }

    /// The year to `now`, from the ledgers' own day rows.
    pub fn of(ledgers: &HashMap<SpendAgent, Ledger>, now: DateTime<Utc>, calendar: &Calendar) -> TokenActivity {
        let today = calendar.start_of_day(now);
        let (start, grid_start) = Self::window(today, calendar);

        let mut tokens_by_day: HashMap<DateTime<Utc>, i64> = HashMap::new();
        let mut partial = false;
        let mut first: Option<DateTime<Utc>> = None;
        for ledger in ledgers.values() {
            // The same gate the whole pane applies: a provider's own statistics carry no money
            // and are not counted anywhere here.
            if !ledger.origin.supports_token_spend() {
                continue;
            }
            let mut contributes = false;
            for day in ledger.days.iter().filter(|d| d.tokens > 0) {
                let date = calendar.start_of_day(day.date);
                first = Some(first.map_or(date, |f| f.min(date)));
                if date < start || date > today {
                    continue;
                }
                *tokens_by_day.entry(date).or_default() += day.tokens;
                contributes = true;
            }
            if contributes && ledger.has_partial_counts {
                partial = true;
            }
        }
        // Nothing at all, or only work in the future: nothing to draw.
        let Some(first) = first.filter(|f| *f <= today) else { return TokenActivity::default() };
        let first_drawn = first.max(start);

        let mut activity = TokenActivity::default();
        let mut cursor = grid_start;
        while cursor <= today {
            let mut days = Vec::with_capacity(7);
            let mut unrecorded = Vec::with_capacity(7);
            let mut date = cursor;
            for _ in 0..7 {
                if date >= first_drawn && date <= today {
                    days.push(Some(ActivityDay { date, tokens: tokens_by_day.get(&date).copied().unwrap_or(0) }));
                } else {
                    days.push(None);
                }
                unrecorded.push(date >= start && date < first_drawn);
                // Back to the day's start: where DST begins at midnight adding a day lands on
                // 01:00 and would stay there.
                date = calendar.start_of_day(calendar.add_days(date, 1));
            }
            activity.weeks.push(Week { start: cursor, days, unrecorded });
            cursor = date;
        }

        let mut busy: Vec<i64> = activity.weeks.iter().flat_map(|w| w.drawn().map(|d| d.tokens)).filter(|t| *t > 0).collect();
        busy.sort_unstable();
        activity.total = busy.iter().sum();
        activity.active_days = busy.len();
        if !busy.is_empty() {
            activity.cuts = (1..=3)
                .map(|quarter| {
                    let index = (busy.len() * quarter).div_ceil(4).saturating_sub(1);
                    busy[index.min(busy.len() - 1)]
                })
                .collect();
        }
        activity.has_partial_counts = partial;
        activity
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spend::ledger::{LedgerDay, Origin};
    use chrono::{NaiveDate, TimeZone};
    use std::collections::HashMap;

    fn calendar(first_weekday: u32) -> Calendar {
        Calendar::utc(first_weekday)
    }

    fn date(year: i32, month: u32, day: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(year, month, day, 0, 0, 0).unwrap()
    }

    fn hour(year: i32, month: u32, day: u32, hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(year, month, day, hour, 0, 0).unwrap()
    }

    /// Monday 5 October 2026.
    fn now() -> DateTime<Utc> {
        hour(2026, 10, 5, 15)
    }

    fn ledger_with(days: &[(DateTime<Utc>, i64)], origin: Origin, partial: bool) -> Ledger {
        Ledger {
            origin,
            has_partial_counts: partial,
            days: days.iter().map(|(d, t)| LedgerDay::new(*d, *t, 0.0, 0, HashMap::new())).collect(),
            earliest: days.iter().map(|d| d.0).min(),
            ..Ledger::empty()
        }
    }

    fn ledger(days: &[(DateTime<Utc>, i64)]) -> Ledger {
        ledger_with(days, Origin::LocalTranscripts, false)
    }

    fn activity_in(days: &[(DateTime<Utc>, i64)], calendar: &Calendar) -> TokenActivity {
        TokenActivity::of(&HashMap::from([(SpendAgent::ClaudeCode, ledger(days))]), now(), calendar)
    }

    fn activity(days: &[(DateTime<Utc>, i64)]) -> TokenActivity {
        activity_in(days, &calendar(2))
    }

    fn day(activity: &TokenActivity, date: DateTime<Utc>) -> Option<ActivityDay> {
        activity.weeks.iter().flat_map(|w| w.days.iter().flatten()).find(|d| d.date == date).copied()
    }

    #[test]
    fn the_window_is_at_most_53_week_columns_ending_today() {
        for calendar in [calendar(2), calendar(1)] {
            let result = activity_in(&[(date(2024, 1, 1), 5)], &calendar);
            assert_eq!(result.column_count(), 53);
            let drawn: Vec<_> = result.weeks.iter().flat_map(|w| w.drawn()).collect();
            // 6 October 2025 to 5 October 2026: 365 days, today included.
            assert_eq!(drawn.first().unwrap().date, date(2025, 10, 6));
            assert_eq!(drawn.last().unwrap().date, date(2026, 10, 5));
            assert_eq!(drawn.len(), 365);
            assert!(result.weeks.iter().all(|w| w.days.len() == 7));
            assert!(result.weeks.iter().all(|w| calendar.weekday(w.start) == calendar.first_weekday));
        }
    }

    #[test]
    fn a_leap_year_window_never_needs_a_54th_column() {
        for (calendar, today) in
            [(calendar(1), date(2032, 2, 29)), (calendar(2), date(2032, 2, 29)), (calendar(2), date(2028, 2, 29)), (calendar(1), date(2028, 3, 5))]
        {
            let result = TokenActivity::of(&HashMap::from([(SpendAgent::Codex, ledger(&[(date(2020, 1, 1), 1)]))]), today, &calendar);
            assert!(result.column_count() <= 53);
            assert_eq!(result.weeks.iter().flat_map(|w| w.drawn()).last().unwrap().date, today);
        }
    }

    #[test]
    fn work_older_than_the_window_is_not_in_it_but_still_marks_the_first_record() {
        let result = activity(&[(date(2024, 5, 1), 1_000), (date(2026, 3, 3), 7)]);
        assert_eq!(result.total, 7);
        assert_eq!(result.active_days, 1);
        assert_eq!(result.weeks.iter().flat_map(|w| w.drawn()).count(), 365);
    }

    #[test]
    fn with_monday_first_sunday_closes_the_previous_column() {
        let result = activity(&[(date(2026, 10, 4), 50), (date(2026, 10, 5), 100)]);
        let last = result.weeks.last().unwrap();
        let before = &result.weeks[result.weeks.len() - 2];
        assert_eq!(last.days[0].unwrap().tokens, 100);
        assert_eq!(last.tokens(), 100);
        assert_eq!(before.days[6].unwrap().tokens, 50);
        assert_eq!(before.tokens(), 50);
    }

    #[test]
    fn with_sunday_first_sunday_and_monday_share_a_column() {
        let result = activity_in(&[(date(2026, 10, 4), 50), (date(2026, 10, 5), 100)], &calendar(1));
        let last = result.weeks.last().unwrap();
        assert_eq!(last.days[0].unwrap().tokens, 50);
        assert_eq!(last.days[1].unwrap().tokens, 100);
        assert_eq!(last.tokens(), 150);
        assert_eq!(result.total, 150);
    }

    #[test]
    fn a_weeks_range_is_clipped_to_the_drawn_days() {
        let result = activity_in(&[(date(2020, 1, 1), 1), (date(2026, 10, 5), 9)], &calendar(1));
        assert_eq!(result.weeks.last().unwrap().range(), Some((date(2026, 10, 4), date(2026, 10, 5))));
        assert_eq!(result.weeks.first().unwrap().range().unwrap().0, date(2025, 10, 6));
    }

    #[test]
    fn days_before_the_first_record_and_after_today_are_not_drawn_days_between_are_quiet() {
        let first = date(2026, 1, 15);
        let result = activity(&[(first, 40), (date(2026, 1, 17), 60)]);
        assert_eq!(day(&result, date(2026, 1, 14)), None);
        assert_eq!(day(&result, first).unwrap().tokens, 40);
        // Nothing was worked on the 16th and that is a measurement.
        assert_eq!(day(&result, date(2026, 1, 16)), Some(ActivityDay { date: date(2026, 1, 16), tokens: 0 }));
        assert_eq!(day(&result, date(2026, 10, 5)), Some(ActivityDay { date: date(2026, 10, 5), tokens: 0 }));
        assert_eq!(day(&result, date(2026, 10, 6)), None);
        // Columns before the first record are not columns of data.
        let early = result.weeks.first().unwrap();
        assert!(!early.has_data());
        assert_eq!(early.range(), None);
        // In the last column only Monday (today) is drawn.
        assert_eq!(result.weeks.last().unwrap().drawn().count(), 1);
    }

    #[test]
    fn a_midnight_dst_start_does_not_knock_the_days_after_it_off_their_keys() {
        // Cairo springs forward at 00:00 on the last Friday of April: that day starts at 01:00,
        // and stepping by a day used to stay at 01:00.
        let cairo = Calendar::with_zone(chrono_tz::Africa::Cairo, 2);
        let local = |y, m, d, h| cairo.from_local(NaiveDate::from_ymd_opt(y, m, d).unwrap().and_hms_opt(h, 0, 0).unwrap());
        let september = local(2026, 9, 1, 0);
        let now = local(2026, 10, 5, 15);
        let result = TokenActivity::of(&HashMap::from([(SpendAgent::ClaudeCode, ledger(&[(september, 70)]))]), now, &cairo);
        let drawn: Vec<_> = result.weeks.iter().flat_map(|w| w.drawn()).collect();
        assert_eq!(drawn.iter().find(|d| d.date == september).unwrap().tokens, 70);
        assert_eq!(result.total, 70);
        assert_eq!(drawn.last().unwrap().date, cairo.start_of_day(now));
    }

    #[test]
    fn days_in_the_window_before_the_first_record_are_outlines_never_days_with_a_count() {
        let first = date(2026, 1, 15);
        let result = activity(&[(first, 40)]);
        assert_eq!(result.weeks.first().unwrap().unrecorded, vec![true; 7]);
        // The column holding the first record: Mon 12 - Wed 14 outlined, Thu 15 on drawn.
        let column = result.weeks.iter().find(|w| w.days.iter().flatten().any(|d| d.date == first)).unwrap();
        assert_eq!(column.unrecorded, vec![true, true, true, false, false, false, false]);
        // Today's column: today is drawn, the days after it are nothing at all.
        assert_eq!(result.weeks.last().unwrap().unrecorded, vec![false; 7]);
        // A history longer than the window has nothing to outline.
        let long = activity(&[(date(2020, 1, 1), 1)]);
        assert!(long.weeks.iter().all(|w| !w.unrecorded.contains(&true)));
    }

    #[test]
    fn nothing_read_only_future_work_or_only_a_providers_own_statistics_draws_nothing() {
        assert!(TokenActivity::of(&HashMap::new(), now(), &calendar(2)).is_empty());
        assert!(activity(&[(date(2026, 10, 6), 10)]).is_empty());
        let statistics = TokenActivity::of(
            &HashMap::from([(SpendAgent::ClaudeCode, ledger_with(&[(date(2026, 3, 3), 10)], Origin::ProviderStatistics, false))]),
            now(),
            &calendar(2),
        );
        assert!(statistics.is_empty());
        assert_eq!(statistics.total, 0);
        assert!(statistics.points().is_empty());
    }

    #[test]
    fn a_zero_token_row_is_not_a_record() {
        assert!(activity(&[(date(2026, 3, 3), 0)]).is_empty());
    }

    #[test]
    fn agents_add_up_on_a_day_and_the_earliest_record_across_all_of_them_starts_the_drawing() {
        let march = date(2026, 3, 3);
        let result = TokenActivity::of(
            &HashMap::from([
                (SpendAgent::ClaudeCode, ledger(&[(march, 30), (date(2026, 5, 1), 5)])),
                (SpendAgent::Codex, ledger(&[(march, 12), (date(2026, 2, 20), 1)])),
            ]),
            now(),
            &calendar(2),
        );
        assert_eq!(day(&result, march).unwrap().tokens, 42);
        assert_eq!(day(&result, date(2026, 2, 20)).unwrap().tokens, 1);
        assert_eq!(day(&result, date(2026, 2, 19)), None);
        assert_eq!(result.total, 48);
        assert_eq!(result.active_days, 3);
    }

    #[test]
    fn partial_counts_are_flagged_only_when_a_ledger_that_contributed_is_partial() {
        let day = date(2026, 3, 3);
        let partial = TokenActivity::of(
            &HashMap::from([(SpendAgent::Codex, ledger_with(&[(day, 3)], Origin::LocalTranscripts, true))]),
            now(),
            &calendar(2),
        );
        assert!(partial.has_partial_counts);
        // A partial ledger whose work is all outside the window adds nothing to it.
        let outside = TokenActivity::of(
            &HashMap::from([
                (SpendAgent::Codex, ledger_with(&[(date(2023, 1, 1), 3)], Origin::LocalTranscripts, true)),
                (SpendAgent::ClaudeCode, ledger(&[(day, 3)])),
            ]),
            now(),
            &calendar(2),
        );
        assert!(!outside.has_partial_counts);
    }

    #[test]
    fn the_running_total_climbs_from_the_first_day_to_the_windows_total() {
        let result = activity(&[(date(2026, 3, 1), 10), (date(2026, 3, 3), 20), (date(2026, 9, 1), 5)]);
        let points = result.points();
        assert_eq!(points.len(), result.weeks.iter().flat_map(|w| w.drawn()).count());
        assert_eq!(points[0].date, date(2026, 3, 1));
        assert_eq!(points[0].running, 10);
        assert_eq!(points.last().unwrap().date, date(2026, 10, 5));
        assert_eq!(points.last().unwrap().running, 35);
        assert_eq!(points.last().unwrap().running, result.total);
        assert!(points.windows(2).all(|p| p[0].running <= p[1].running && p[0].position < p[1].position));
        assert!(points.iter().all(|p| p.position > 0.0 && p.position < result.column_count() as f64));
        // 1 March 2026 is a Sunday: column row 6 with Monday first.
        let row = (points[0].position - points[0].position.trunc()) * 7.0 - 0.5;
        assert_eq!(row.round() as i64, 6);
    }

    #[test]
    fn steps_are_the_quartiles_of_the_days_with_work_a_quiet_day_is_step_0() {
        let mut days: Vec<_> = (1..=8).map(|d| (date(2026, 4, d), d as i64 * 10)).collect();
        days.push((date(2026, 4, 9), 0));
        let result = activity(&days);
        assert_eq!(result.cuts, vec![20, 40, 60]);
        assert_eq!(result.step(0), 0);
        let steps: Vec<_> = [10, 20, 30, 40, 50, 60, 70, 80].iter().map(|t| result.step(*t)).collect();
        assert_eq!(steps, vec![1, 1, 2, 2, 3, 3, 4, 4]);
    }

    #[test]
    fn one_huge_day_does_not_flatten_the_rest_into_the_palest_step() {
        let mut days: Vec<_> = (1..=7).map(|d| (date(2026, 4, d), d as i64 * 100)).collect();
        days.push((date(2026, 4, 8), 1_000_000_000));
        let result = activity(&days);
        assert_eq!(result.step(1_000_000_000), 4);
        assert!(result.step(700) >= 3);
        assert_eq!(result.step(100), 1);
    }

    #[test]
    fn a_single_active_day_is_the_palest_step() {
        let one = activity(&[(date(2026, 4, 1), 9)]);
        assert_eq!(one.cuts, vec![9, 9, 9]);
        assert_eq!(one.step(9), 1);
        assert_eq!(one.active_days, 1);
    }

    #[test]
    fn month_labels_sit_at_each_months_first_day_evenly_as_the_calendar() {
        let calendar = calendar(2);
        let result = activity(&[(date(2024, 1, 1), 1)]);
        let marks = result.month_marks(&calendar, 3.0);
        let months: Vec<_> = marks.iter().map(|m| calendar.month(m.date)).collect();
        // October 2025 through the running October 2026, which is named even though it starts in
        // the last columns.
        assert_eq!(months, vec![10, 11, 12, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        assert_eq!(marks[0].position, 0.0);
        // Thursday 1 October 2026 is the fourth day of the column before today's.
        assert_eq!(marks.last().unwrap().column, result.weeks.len() - 2);
        assert_eq!(marks.last().unwrap().position, (result.weeks.len() - 2) as f64 + 3.0 / 7.0);
        // Every whole month is 28 to 31 days: 4 to 4 3/7 columns, never 5.
        for pair in marks.windows(3).map(|w| w[2].position - w[1].position) {
            assert!((4.0..=31.0 / 7.0 + 1e-9).contains(&pair), "{pair}");
        }
    }

    #[test]
    fn a_partial_first_month_too_short_to_hold_its_label_is_not_labelled() {
        // Asked on Wednesday 28 October, the window starts on Wednesday 29 October 2025: three
        // days of October before Saturday 1 November.
        let calendar = calendar(2);
        let result = TokenActivity::of(
            &HashMap::from([(SpendAgent::ClaudeCode, ledger(&[(date(2020, 1, 1), 1)]))]),
            hour(2026, 10, 28, 9),
            &calendar,
        );
        let marks = result.month_marks(&calendar, 3.0);
        assert_eq!(calendar.month(marks[0].date), 11);
        assert_eq!(marks[0].position, 5.0 / 7.0);
    }
}
