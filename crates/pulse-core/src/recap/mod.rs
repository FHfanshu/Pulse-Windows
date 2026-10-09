// Ported from upstream Sources/Pulse/Usage/Recap.swift.
//! A month's or a year's recap: what the Token spend readers saw on this machine over a calendar
//! period, gathered for the shareable recap cards.
//!
//! Facts only, no copy: every field is a number, a date, a name the tools themselves use or an
//! enum. Wording, number units and date formats belong to the cards, per language.
//!
//! Missing is `None`, never zero: money where nothing was priced, a cache rate where no store
//! records the cache, a previous period with no records. The card that needs such a figure is
//! left out instead of being drawn with a zero.
//!
//! Days are local calendar dates ([`NaiveDate`]), cut by the [`crate::spend::Calendar`] the recap
//! was built with, so a recap needs no time-zone arithmetic after it is built.

pub mod build;
pub mod deck;
pub mod insights;
pub mod periods;

#[cfg(test)]
pub mod samples;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_deck;
#[cfg(test)]
mod tests_insights;
#[cfg(test)]
mod tests_periods;

use std::collections::BTreeSet;

use chrono::{Months, NaiveDate};
use serde::{Serialize, Serializer};

use crate::spend::SpendAgent;

pub use build::build;

/// Which stretch of the calendar a recap covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Period {
    Month { year: i32, month: u32 },
    Year(i32),
}

impl Period {
    /// "2026-09" for a month, "2026" for a year: the form `--recap` takes, and what is remembered
    /// about which month was announced.
    pub fn key(&self) -> String {
        match self {
            Period::Month { year, month } => format!("{year:04}-{month:02}"),
            Period::Year(year) => format!("{year:04}"),
        }
    }

    /// "2026-09" is a month, "2026" a year. Anything else is no period.
    pub fn from_key(key: &str) -> Option<Period> {
        let digits = |s: &str, len: usize| s.len() == len && s.bytes().all(|b| b.is_ascii_digit());
        let pieces: Vec<&str> = key.split('-').collect();
        match pieces.as_slice() {
            [year] if digits(year, 4) => Some(Period::Year(year.parse().ok()?)),
            [year, month] if digits(year, 4) && digits(month, 2) => {
                let month: u32 = month.parse().ok()?;
                (1..=12).contains(&month).then_some(Period::Month { year: year.parse().ok()?, month })
            }
            _ => None,
        }
    }

    pub fn is_year(&self) -> bool {
        matches!(self, Period::Year(_))
    }

    pub fn year(&self) -> i32 {
        match self {
            Period::Month { year, .. } | Period::Year(year) => *year,
        }
    }

    /// The calendar days the period covers: its first day and the day after its last. `None` for
    /// a month outside 1..=12, which names no span.
    pub fn bounds(&self) -> Option<(NaiveDate, NaiveDate)> {
        match *self {
            Period::Month { year, month } => {
                let start = NaiveDate::from_ymd_opt(year, month, 1)?;
                Some((start, start.checked_add_months(Months::new(1))?))
            }
            Period::Year(year) => {
                let start = NaiveDate::from_ymd_opt(year, 1, 1)?;
                Some((start, start.checked_add_months(Months::new(12))?))
            }
        }
    }

    /// The period before this one: the month before (December of the year before, from
    /// January), or the year before.
    pub fn previous(&self) -> Period {
        match *self {
            Period::Month { year, month } if month > 1 => Period::Month { year, month: month - 1 },
            Period::Month { year, .. } => Period::Month { year: year - 1, month: 12 },
            Period::Year(year) => Period::Year(year - 1),
        }
    }
}

impl Serialize for Period {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.key())
    }
}

/// A rule-based label from where in the day the work fell. Each case is a stated threshold over
/// the hours, not a judgement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Persona {
    /// Most work after 21:00 or before 05:00.
    NightOwl,
    /// Most work between 05:00 and 10:00.
    EarlyBird,
    /// Most work between 10:00 and 18:00.
    DayShift,
    /// No band holds most of it.
    AllDay,
}

impl Persona {
    /// The band of the day most of the work fell in, from tokens by local hour (24 entries, 0 =
    /// midnight). Each case is a plain majority (more than half of the tokens), so at most one
    /// can hold, checked in this order: night 21:00-04:59, early 05:00-09:59, day 10:00-17:59,
    /// else all day (18:00-20:59 is a band of its own that cannot win alone, on purpose).
    ///
    /// `None` where there is no shape to read: not 24 hours, or no tokens in them.
    pub fn of(hours: &[i64]) -> Option<Persona> {
        if hours.len() != 24 {
            return None;
        }
        let total: i64 = hours.iter().sum();
        if total <= 0 {
            return None;
        }
        let share = |range: std::ops::Range<usize>| hours[range].iter().sum::<i64>() as f64 / total as f64;
        if share(21..24) + share(0..5) > 0.5 {
            return Some(Persona::NightOwl);
        }
        if share(5..10) > 0.5 {
            return Some(Persona::EarlyBird);
        }
        if share(10..18) > 0.5 {
            return Some(Persona::DayShift);
        }
        Some(Persona::AllDay)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Day {
    pub date: NaiveDate,
    pub tokens: i64,
    /// Estimated at API prices; `None` when none of the day's work had a price.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
}

/// Year recaps only: one entry per calendar month, January first.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Month {
    /// 1..=12.
    pub month: u32,
    pub tokens: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
    pub active_days: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelShare {
    /// The provider's own display name ("Claude Opus 4.6"), else the raw id.
    pub name: String,
    pub tokens: i64,
    /// Of the recap's tokens, 0..=1.
    pub share: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentShare {
    pub agent: SpendAgent,
    pub name: &'static str,
    /// The stem of the agent's mark in the bundled icon set, where it has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<&'static str>,
    pub tokens: i64,
    pub share: f64,
    /// Calendar days in the period with any of this agent's work.
    pub active_days: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
    /// Which days those were, all inside the period: the opener's per-agent strip is drawn from
    /// them.
    pub active_dates: BTreeSet<NaiveDate>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectShare {
    /// The directory's short name, as the Token spend pane shows it.
    pub name: String,
    pub tokens: i64,
    pub share: f64,
    pub sessions: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Recap {
    pub period: Period,
    /// The first day of the period, and the day it ends before. A period still running ends
    /// after today.
    pub start: NaiveDate,
    pub end: NaiveDate,
    /// Whether the period is still running: figures are to date.
    pub is_in_progress: bool,

    pub tokens: i64,
    /// Estimated at published API prices; `None` when nothing was priced.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
    /// Tokens of the period with no published price behind them. Above zero, `cost` is the
    /// priced part only, a floor.
    pub unpriced_tokens: i64,
    /// The same span of the previous period, for "up 38% on August". `None` when nothing was
    /// recorded then.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_tokens: Option<i64>,

    /// Calendar days with any work, and the days the period has had so far.
    pub active_days: usize,
    pub elapsed_days: usize,
    pub sessions: usize,

    /// Every calendar day of the period up to today, quiet ones included.
    pub days: Vec<Day>,
    /// Year recaps: twelve entries. Month recaps: empty.
    pub months: Vec<Month>,

    /// Tokens by local hour, 24 entries, 0 = midnight. `None` when any store behind the period
    /// has only session- or report-level timing, so an hour shape would be invented.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hours: Option<Vec<i64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peak_hour: Option<u32>,
    /// Share of the period's tokens in 21:00-04:59.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub late_share: Option<f64>,
    /// The latest any stretch of work ended, as minutes after the midnight of the day it began:
    /// past 1440 when it ran into the next morning (`build::workdays`). `None` with no timed work.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_minute: Option<u32>,
    /// Stretches of work that ran over a midnight.
    pub late_nights: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub persona: Option<Persona>,

    /// Heaviest first.
    pub models: Vec<ModelShare>,
    pub agents: Vec<AgentShare>,
    pub projects: Vec<ProjectShare>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_hit_rate: Option<f64>,
    /// What the cache reads would have cost at each model's input rate, less what they did cost
    /// at its cache-read rate. `None` when unpriced.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_savings: Option<f64>,

    /// Streaks belong to the period: counted over `days` and nothing outside it. A past month's
    /// card must not carry October's streak.
    pub current_streak: usize,
    pub longest_streak: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub busiest_day: Option<Day>,

    /// ISO currency of `cost`; the readers price in US dollars.
    pub currency: String,
    /// Whether some store behind it may be missing counts: the total is a floor.
    pub is_partial: bool,

    /// The first day this PC has any record for, when that falls inside the period after its
    /// first day; `None` when the records reach back to the start or before it.
    ///
    /// **Before it Pulse saw nothing, which is not a quiet day.** A year whose records began in
    /// May drew January to April as zeros and counted them in every denominator ("active 88 of
    /// 282 days", the workday split, the plan price prorated from January 1). `observed_days` is
    /// the count those use, and a month entirely before it is drawn as unrecorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub records_begin: Option<NaiveDate>,

    /// The days `previous_tokens` was counted over: as many as this period has had while it runs
    /// (fewer where the period before is shorter), all of the period before once this one is
    /// over, and none from before the first record. `None` where `previous_tokens` is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_days: Option<usize>,
}

impl Recap {
    /// The period's days Pulse could have seen work on: from `records_begin` (or the start) up to
    /// today or the end. `elapsed_days` without a later first record; the "same period"
    /// comparison keeps `elapsed_days`.
    pub fn observed_days(&self) -> usize {
        match self.records_begin {
            None => self.elapsed_days,
            Some(begin) => self.days.iter().filter(|d| d.date >= begin).count(),
        }
    }

    /// Whether a day falls before the first record this PC holds.
    pub fn is_before_records(&self, date: NaiveDate) -> bool {
        self.records_begin.is_some_and(|begin| date < begin)
    }

    /// The tokens-by-hour band that counts as late: 21:00 through 04:59.
    pub const LATE_HOURS: [usize; 8] = [21, 22, 23, 0, 1, 2, 3, 4];
    /// Work before this local hour, after midnight, belongs to the night before.
    pub const NIGHT_ENDS_AT_HOUR: u32 = 5;

    pub fn is_empty(&self) -> bool {
        self.tokens == 0
    }

    /// The first day of each month the period covers: twelve for a year, one for a month.
    pub fn month_starts(&self) -> Vec<NaiveDate> {
        let count = if self.period.is_year() { 12 } else { 1 };
        (0..count).filter_map(|offset| self.start.checked_add_months(Months::new(offset))).collect()
    }

    /// The streaks of `days` (oldest first, one entry per calendar day, quiet ones included) as
    /// (current, longest). A running period's last day is today, and a quiet today does not
    /// break the run until it has passed.
    pub fn streaks(days: &[Day], is_in_progress: bool) -> (usize, usize) {
        let (mut longest, mut run) = (0, 0);
        for day in days {
            run = if day.tokens > 0 { run + 1 } else { 0 };
            longest = longest.max(run);
        }
        let mut current = 0;
        let mut index = days.len() as isize - 1;
        if is_in_progress && index >= 0 && days[index as usize].tokens == 0 {
            index -= 1;
        }
        while index >= 0 && days[index as usize].tokens > 0 {
            current += 1;
            index -= 1;
        }
        (current, longest)
    }
}

/// A recap with everything the cards read off it: the insights, and the deck facts that do not
/// depend on the reader's price. This is what the window is sent.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    #[serde(flatten)]
    pub recap: Recap,
    /// `Recap::observed_days`: the denominator for "active N of M days" and the like.
    pub observed_days: usize,
    pub insights: insights::Insights,
    pub deck: deck::DeckFacts,
}

impl Report {
    pub fn of(recap: Recap) -> Report {
        let insights = insights::Insights::of(&recap);
        let deck = deck::DeckFacts::of(&recap);
        Report { observed_days: recap.observed_days(), recap, insights, deck }
    }
}
