// Ported from upstream Sources/Pulse/Usage/RecapInsights.swift.
//! What the recap cards read off a [`Recap`] beyond its own fields: weekday and week totals, the
//! shape of a streak, the day split into quarters, money per million tokens, and the strips that
//! show which agent worked on which day.
//!
//! Facts only, like `Recap`, and apart from drawing so they can be tested. A figure that cannot
//! be stood behind is `None`, and the card that would draw it leaves it out: a weekday the period
//! never had is not a zero, a series with one unpriced working day is not a series.

use chrono::{Datelike, Duration, NaiveDate};
use serde::Serialize;

use super::{AgentShare, Day, Recap};

/// Saturday and Sunday (Monday = 0).
pub fn is_weekend(weekday_index: usize) -> bool {
    weekday_index >= 5
}

/// Monday = 0 ... Sunday = 6.
pub fn weekday_index(date: NaiveDate) -> usize {
    date.weekday().num_days_from_monday() as usize
}

/// The days of the week against the weekend: tokens, the days the period has had of each, and
/// how many of them had work.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkSplit {
    pub weekday_tokens: i64,
    pub weekend_tokens: i64,
    pub weekday_days: usize,
    pub weekend_days: usize,
    pub weekday_active: usize,
    pub weekend_active: usize,
}

impl WorkSplit {
    pub fn tokens(&self) -> i64 {
        self.weekday_tokens + self.weekend_tokens
    }

    /// Of all the tokens, 0..=1.
    pub fn weekday_share(&self) -> f64 {
        if self.tokens() > 0 { self.weekday_tokens as f64 / self.tokens() as f64 } else { 0.0 }
    }

    pub fn weekend_share(&self) -> f64 {
        if self.tokens() > 0 { self.weekend_tokens as f64 / self.tokens() as f64 } else { 0.0 }
    }
}

/// A week of the period, Monday to Sunday, clipped to the period: the first and last may be
/// short.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Week {
    pub first: NaiveDate,
    pub last: NaiveDate,
    pub tokens: i64,
}

/// A run of days with work.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    pub first: NaiveDate,
    pub last: NaiveDate,
    pub length: usize,
}

/// The streak drawn as a chain of days: a window of at most nine around the run, kept inside the
/// period, with the run's own days marked (`run_start..run_end` indexes `days`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Chain {
    /// The days drawn, oldest first.
    pub days: Vec<NaiveDate>,
    pub run_start: usize,
    pub run_end: usize,
    /// The dates a label under the chain names: the window's two ends, or the run's own where the
    /// run is longer than the window.
    pub first: NaiveDate,
    pub last: NaiveDate,
}

impl Chain {
    pub const WIDTH: usize = 9;

    pub fn run(&self) -> std::ops::Range<usize> {
        self.run_start..self.run_end
    }
}

/// A quarter of the day and its share of the tokens in the hour shape.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Quarter {
    /// First hour of the quarter: 0, 6, 12 or 18.
    pub from: usize,
    pub tokens: i64,
    pub share: f64,
}

/// What one agent's strip shows for a day (or, in a year, a month).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Mark {
    /// This agent worked.
    Used,
    /// Another agent did, this one did not.
    Other,
    /// Nobody did, and it was a weekend.
    QuietWeekend,
    /// Nobody did, on a weekday (or in a month).
    Quiet,
    /// A month of a running year that has not begun.
    ToCome,
}

pub struct RecapInsights<'a> {
    pub recap: &'a Recap,
}

impl<'a> RecapInsights<'a> {
    pub fn new(recap: &'a Recap) -> Self {
        Self { recap }
    }

    // Weekdays and weekends.

    /// Tokens by day of the week, Monday first. `None` for a weekday the period has not had a
    /// single day of (the first days of a running month), which is unknown rather than quiet.
    pub fn weekday_tokens(&self) -> [Option<i64>; 7] {
        let mut tokens = [0i64; 7];
        let mut seen = [false; 7];
        for day in &self.recap.days {
            let index = weekday_index(day.date);
            tokens[index] += day.tokens;
            seen[index] = true;
        }
        std::array::from_fn(|i| seen[i].then_some(tokens[i]))
    }

    /// The heaviest day of the week (Monday = 0), the earliest of a tie; `None` when none had
    /// work.
    pub fn busiest_weekday(&self) -> Option<usize> {
        let tokens = self.weekday_tokens();
        let mut best: Option<usize> = None;
        for index in 0..7 {
            if tokens[index].unwrap_or(0) > best.and_then(|b| tokens[b]).unwrap_or(0) {
                best = Some(index);
            }
        }
        best
    }

    /// `None` where there is nothing to split: no days, or no tokens in them.
    pub fn work_split(&self) -> Option<WorkSplit> {
        if !self.recap.days.iter().any(|d| d.tokens > 0) {
            return None;
        }
        let mut split = WorkSplit {
            weekday_tokens: 0,
            weekend_tokens: 0,
            weekday_days: 0,
            weekend_days: 0,
            weekday_active: 0,
            weekend_active: 0,
        };
        // Days before the first record were not seen, so they are not days the period "had" of
        // either kind.
        for day in self.recap.days.iter().filter(|d| !self.recap.is_before_records(d.date)) {
            let active = usize::from(day.tokens > 0);
            if is_weekend(weekday_index(day.date)) {
                split.weekend_tokens += day.tokens;
                split.weekend_days += 1;
                split.weekend_active += active;
            } else {
                split.weekday_tokens += day.tokens;
                split.weekday_days += 1;
                split.weekday_active += active;
            }
        }
        Some(split)
    }

    /// Tokens on an average day with work, `None` with none.
    pub fn tokens_per_active_day(&self) -> Option<i64> {
        (self.recap.active_days > 0).then(|| self.recap.tokens / self.recap.active_days as i64)
    }

    // Weeks.

    /// The period's weeks, oldest first, from the days it has had.
    pub fn weeks(&self) -> Vec<Week> {
        let mut result: Vec<Week> = Vec::new();
        let mut current_key: Option<NaiveDate> = None;
        for day in &self.recap.days {
            let key = day.date - Duration::days(weekday_index(day.date) as i64);
            match result.last_mut() {
                Some(last) if current_key == Some(key) => {
                    last.last = day.date;
                    last.tokens += day.tokens;
                }
                _ => {
                    result.push(Week { first: day.date, last: day.date, tokens: day.tokens });
                    current_key = Some(key);
                }
            }
        }
        result
    }

    /// The index of the heaviest week, the earliest of a tie; `None` when none had work.
    pub fn busiest_week(&self) -> Option<usize> {
        let weeks = self.weeks();
        let mut best: Option<usize> = None;
        for (index, week) in weeks.iter().enumerate() {
            if week.tokens > best.map_or(0, |b| weeks[b].tokens) {
                best = Some(index);
            }
        }
        best
    }

    // Streak.

    /// The longest run of days with work inside the period, the earliest of a tie. `None` when
    /// no day had work.
    pub fn longest_run(&self) -> Option<Run> {
        let days = &self.recap.days;
        let mut best: Option<Run> = None;
        let mut start: Option<usize> = None;
        for (index, day) in days.iter().enumerate() {
            if day.tokens > 0 {
                let from = *start.get_or_insert(index);
                let length = index - from + 1;
                if length > best.as_ref().map_or(0, |b| b.length) {
                    best = Some(Run { first: days[from].date, last: day.date, length });
                }
            } else {
                start = None;
            }
        }
        best
    }

    pub fn streak_chain(&self) -> Option<Chain> {
        let run = self.longest_run()?;
        let days = &self.recap.days;
        let from = days.iter().position(|d| d.date == run.first)?;
        let width = Chain::WIDTH;
        let count = days.len();
        if run.length >= width {
            return Some(Chain {
                days: days[from..from + width].iter().map(|d| d.date).collect(),
                run_start: 0,
                run_end: width,
                first: run.first,
                last: run.last,
            });
        }
        let window = width.min(count);
        // The run in the middle where it can be, else against the edge.
        let before = (window - run.length) / 2;
        let start = (from as isize - before as isize).min((count - window) as isize).max(0) as usize;
        let chain_days: Vec<NaiveDate> = days[start..start + window].iter().map(|d| d.date).collect();
        let lower = from - start;
        Some(Chain {
            first: chain_days[0],
            last: chain_days[chain_days.len() - 1],
            days: chain_days,
            run_start: lower,
            run_end: lower + run.length,
        })
    }

    // Time of day.

    /// The earliest hour from 05:00 on with any work: when the day usually starts, leaving the
    /// after-midnight tail to the night before. `None` where there is no hour shape or nothing
    /// from 05:00 on.
    pub fn first_hour(&self) -> Option<usize> {
        let hours = self.recap.hours.as_ref().filter(|h| h.len() == 24)?;
        (Recap::NIGHT_ENDS_AT_HOUR as usize..24).find(|h| hours[*h] > 0)
    }

    /// The four quarters of the day, midnight first. `None` with no hour shape or no tokens in
    /// it.
    pub fn quarters(&self) -> Option<Vec<Quarter>> {
        let hours = self.recap.hours.as_ref().filter(|h| h.len() == 24)?;
        let total: i64 = hours.iter().sum();
        if total <= 0 {
            return None;
        }
        Some(
            (0..24)
                .step_by(6)
                .map(|from| {
                    let tokens: i64 = hours[from..from + 6].iter().sum();
                    Quarter { from, tokens, share: tokens as f64 / total as f64 }
                })
                .collect(),
        )
    }

    /// The heaviest quarter's index, the earliest of a tie.
    pub fn leading_quarter(&self) -> Option<usize> {
        let quarters = self.quarters()?;
        let mut best = 0;
        for index in 1..quarters.len() {
            if quarters[index].tokens > quarters[best].tokens {
                best = index;
            }
        }
        Some(best)
    }

    // Money.

    /// Dollars per million tokens, over the tokens that had a price: with some unpriced the cost
    /// is a floor, and dividing it by every token would understate the rate.
    pub fn cost_per_million(&self) -> Option<f64> {
        let cost = self.recap.cost?;
        let priced = self.recap.tokens - self.recap.unpriced_tokens;
        (priced > 0).then(|| cost / priced as f64 * 1_000_000.0)
    }

    /// Money on an average day with work.
    pub fn cost_per_active_day(&self) -> Option<f64> {
        let cost = self.recap.cost?;
        (self.recap.active_days > 0).then(|| cost / self.recap.active_days as f64)
    }

    /// The day that cost most among the days with a price, the earliest of a tie; `None` when no
    /// day had one. A day with work and no price at all is left out of the running rather than
    /// taking the tile away: it once did, and one Kimi-only day (0.05% of July) removed it. The
    /// card already says the money is a floor where any work went unpriced.
    pub fn costliest_day(&self) -> Option<Day> {
        let mut best: Option<&Day> = None;
        for day in &self.recap.days {
            if day.cost.unwrap_or(0.0) > best.and_then(|b| b.cost).unwrap_or(0.0) {
                best = Some(day);
            }
        }
        best.cloned()
    }

    /// Money by day (a month) or by month (a year), one entry per day or month: a quiet one a
    /// real zero, and **`None` for one with no figure** (a month still to come or before the first
    /// record, or work with no price at all, which is not a zero). `None` altogether when nothing
    /// had a price.
    pub fn cost_bars(&self) -> Option<Vec<Option<f64>>> {
        let bars: Vec<Option<f64>> = if self.recap.period.is_year() {
            if self.recap.months.len() != 12 {
                return None;
            }
            self.recap
                .months
                .iter()
                .enumerate()
                .map(|(index, month)| {
                    if self.is_month_unrecorded(index) {
                        None
                    } else if month.tokens > 0 {
                        month.cost
                    } else {
                        Some(0.0)
                    }
                })
                .collect()
        } else {
            self.recap.days.iter().map(|d| if d.tokens > 0 { d.cost } else { Some(0.0) }).collect()
        };
        bars.iter().any(|b| b.unwrap_or(0.0) > 0.0).then_some(bars)
    }

    /// Whether month `index` (0 for January) of a running year has not begun: a month to come,
    /// which is not a quiet month and is not drawn as one.
    pub fn is_month_to_come(&self, index: usize) -> bool {
        let recap = self.recap;
        if !recap.is_in_progress || !recap.period.is_year() {
            return false;
        }
        let Some(last) = recap.days.last().map(|d| d.date) else { return false };
        recap.month_starts().get(index).is_some_and(|start| *start > last)
    }

    /// Whether month `index` of a year ended before this PC's first record: nothing was seen in
    /// it, so it is not a quiet month either.
    pub fn is_month_before_records(&self, index: usize) -> bool {
        let recap = self.recap;
        let (true, Some(begin)) = (recap.period.is_year(), recap.records_begin) else { return false };
        recap
            .month_starts()
            .get(index)
            .and_then(|start| start.checked_add_months(chrono::Months::new(1)))
            .is_some_and(|next| next <= begin)
    }

    /// A month with nothing to show: still to come, or before the records.
    pub fn is_month_unrecorded(&self, index: usize) -> bool {
        self.is_month_to_come(index) || self.is_month_before_records(index)
    }

    /// Per month, whether it has nothing to show (the months card draws such a month as one to
    /// come).
    pub fn months_to_come(&self) -> Vec<bool> {
        (0..self.recap.months.len()).map(|i| self.is_month_unrecorded(i)).collect()
    }

    // Who worked when.

    /// One mark per day of the period for an agent.
    pub fn day_marks(&self, agent: &AgentShare) -> Vec<Mark> {
        self.recap
            .days
            .iter()
            .map(|day| {
                if agent.active_dates.contains(&day.date) {
                    Mark::Used
                } else if day.tokens > 0 {
                    Mark::Other
                } else if is_weekend(weekday_index(day.date)) {
                    Mark::QuietWeekend
                } else {
                    Mark::Quiet
                }
            })
            .collect()
    }

    /// One mark per month of a year for an agent: used when it had work in the month at all.
    /// Empty for a month recap.
    pub fn month_marks(&self, agent: &AgentShare) -> Vec<Mark> {
        let used: std::collections::BTreeSet<u32> = agent.active_dates.iter().map(|d| d.month()).collect();
        self.recap
            .months
            .iter()
            .enumerate()
            .map(|(index, month)| {
                if self.is_month_unrecorded(index) {
                    Mark::ToCome
                } else if used.contains(&month.month) {
                    Mark::Used
                } else if month.tokens > 0 {
                    Mark::Other
                } else {
                    Mark::Quiet
                }
            })
            .collect()
    }

    /// The marks the opener draws: days for a month, months for a year.
    pub fn marks(&self, agent: &AgentShare) -> Vec<Mark> {
        if self.recap.period.is_year() { self.month_marks(agent) } else { self.day_marks(agent) }
    }
}

/// Everything above in one serialisable value, for the cards.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Insights {
    pub weekday_tokens: [Option<i64>; 7],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub busiest_weekday: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub work_split: Option<WorkSplit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens_per_active_day: Option<i64>,
    pub weeks: Vec<Week>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub busiest_week: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub longest_run: Option<Run>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub streak_chain: Option<Chain>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_hour: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quarters: Option<Vec<Quarter>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub leading_quarter: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_per_million: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_per_active_day: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub costliest_day: Option<Day>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_bars: Option<Vec<Option<f64>>>,
    pub months_to_come: Vec<bool>,
    /// One strip per agent, in the recap's agent order.
    pub agent_marks: Vec<Vec<Mark>>,
}

impl Insights {
    pub fn of(recap: &Recap) -> Insights {
        let insights = RecapInsights::new(recap);
        Insights {
            weekday_tokens: insights.weekday_tokens(),
            busiest_weekday: insights.busiest_weekday(),
            work_split: insights.work_split(),
            tokens_per_active_day: insights.tokens_per_active_day(),
            weeks: insights.weeks(),
            busiest_week: insights.busiest_week(),
            longest_run: insights.longest_run(),
            streak_chain: insights.streak_chain(),
            first_hour: insights.first_hour(),
            quarters: insights.quarters(),
            leading_quarter: insights.leading_quarter(),
            cost_per_million: insights.cost_per_million(),
            cost_per_active_day: insights.cost_per_active_day(),
            costliest_day: insights.costliest_day(),
            cost_bars: insights.cost_bars(),
            months_to_come: insights.months_to_come(),
            agent_marks: recap.agents.iter().map(|a| insights.marks(a)).collect(),
        }
    }
}
