// Ported from upstream Sources/Pulse/Usage/RecapPeriods.swift.
//! Which periods the recap window offers and which one it opens on, the monthly price typed into
//! it, and when "your September recap is ready" is said.
//!
//! Offered: every month and every year from the earliest record on this machine to now, newest
//! first; nothing before it and nothing after today. Opened on: the month (or year) that just
//! finished during its first seven days, the running one after that; where that falls before the
//! earliest record it moves up to it.

use std::collections::HashMap;

use chrono::{Datelike, NaiveDate};
use serde::Serialize;

use super::Period;
use crate::spend::{Calendar, Ledger, SpendAgent};

/// The first days of a month (or year) in which the one just finished is what "the recap" means.
pub const EARLY_DAYS: u32 = 7;

/// `year * 12 + (month - 1)`, so months can be counted and compared.
fn index(date: NaiveDate) -> i32 {
    date.year() * 12 + date.month() as i32 - 1
}

fn month_at(index: i32) -> Period {
    Period::Month { year: index.div_euclid(12), month: index.rem_euclid(12) as u32 + 1 }
}

/// Months with records possible, newest first.
pub fn months(earliest: Option<NaiveDate>, today: NaiveDate) -> Vec<Period> {
    let current = index(today);
    let first = earliest.map_or(current, |e| index(e).min(current));
    (first..=current).rev().map(month_at).collect()
}

/// Years with records possible, newest first.
pub fn years(earliest: Option<NaiveDate>, today: NaiveDate) -> Vec<Period> {
    let current = today.year();
    let first = earliest.map_or(current, |e| e.year().min(current));
    (first..=current).rev().map(Period::Year).collect()
}

/// The month to open on.
pub fn default_month(earliest: Option<NaiveDate>, today: NaiveDate) -> Period {
    let current = index(today);
    let wanted = if today.day() <= EARLY_DAYS { current - 1 } else { current };
    // Nothing known about the first record is not a floor at this month.
    let floor = earliest.map_or(wanted, |e| index(e).min(current));
    month_at(wanted.max(floor))
}

/// The year to open on.
pub fn default_year(earliest: Option<NaiveDate>, today: NaiveDate) -> Period {
    let current = today.year();
    let early = today.month() == 1 && today.day() <= EARLY_DAYS;
    let wanted = if early { current - 1 } else { current };
    let floor = earliest.map_or(wanted, |e| e.year().min(current));
    Period::Year(wanted.max(floor))
}

/// The period of the other kind that goes with this one: the year a month belongs to, and the
/// last month of a year that is offered.
pub fn switched(period: Period, earliest: Option<NaiveDate>, today: NaiveDate) -> Period {
    match period {
        Period::Month { year, .. } => Period::Year(year),
        Period::Year(year) => months(earliest, today)
            .into_iter()
            .find(|p| p.year() == year)
            .unwrap_or_else(|| default_month(earliest, today)),
    }
}

/// No recap is offered before this: a bogus timestamp in some store (an epoch of zero, a clock
/// that was wrong) must not turn into hundreds of empty months in the picker.
pub fn floor() -> NaiveDate {
    NaiveDate::from_ymd_opt(2020, 1, 1).expect("a valid date")
}

/// The earliest day any ledger the Token spend pane may show holds a record for, and not before
/// [`floor`]. A provider's own statistics are not records of this machine's work and do not decide
/// where the recap begins.
pub fn earliest(ledgers: &HashMap<SpendAgent, Ledger>, calendar: &Calendar) -> Option<NaiveDate> {
    let found = ledgers
        .values()
        .filter(|l| l.origin.supports_token_spend())
        .filter_map(|l| l.earliest.or_else(|| l.days.first().map(|d| d.date)))
        .min()?;
    Some(calendar.date(found).max(floor()))
}

/// What the window needs to offer and open on, as period keys.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Offer {
    pub months: Vec<String>,
    pub years: Vec<String>,
    pub default_month: String,
    pub default_year: String,
}

impl Offer {
    pub fn of(earliest: Option<NaiveDate>, today: NaiveDate) -> Offer {
        let keys = |periods: Vec<Period>| periods.iter().map(Period::key).collect();
        Offer {
            months: keys(months(earliest, today)),
            years: keys(years(earliest, today)),
            default_month: default_month(earliest, today).key(),
            default_year: default_year(earliest, today).key(),
        }
    }
}

/// The monthly subscription price typed into the recap window, as text and as the figure it
/// stands for.
///
/// Strict, and no locale guessing: after trimming whitespace and one leading "$", the text is
/// either plain digits with an optional "." or "," and one or two decimals ("20", "12.5", "12,5"
/// is 12.5), or digits grouped by "," in valid groups of three with an optional ".dd" ("1,200",
/// "1,200.50"). Anything else is refused. Nothing is not zero: empty clears it, and so does 0.
pub mod price {
    /// A month's price in US dollars. Well past anything sold, and low enough that a stray extra
    /// digit is refused rather than drawn on a card.
    pub const MAXIMUM: f64 = 10_000.0;

    #[derive(Debug, Clone, Copy, PartialEq)]
    pub enum Entry {
        None,
        Amount(f64),
        Refused,
    }

    fn all_digits(text: &str) -> bool {
        !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())
    }

    /// Plain digits with an optional separator and one or two decimals.
    fn plain(text: &str) -> Option<String> {
        match text.find(['.', ',']) {
            None => all_digits(text).then(|| text.to_string()),
            Some(at) => {
                let (whole, decimals) = (&text[..at], &text[at + 1..]);
                (all_digits(whole) && all_digits(decimals) && decimals.len() <= 2).then(|| format!("{whole}.{decimals}"))
            }
        }
    }

    /// Digits grouped by commas in threes with an optional ".dd".
    fn grouped(text: &str) -> Option<String> {
        let (integer, decimals) = match text.split_once('.') {
            Some((integer, decimals)) => (integer, Some(decimals)),
            None => (text, None),
        };
        let groups: Vec<&str> = integer.split(',').collect();
        if groups.len() < 2 || groups[0].is_empty() || groups[0].len() > 3 || !groups.iter().all(|g| all_digits(g)) {
            return None;
        }
        if groups[1..].iter().any(|g| g.len() != 3) {
            return None;
        }
        let mut number = groups.concat();
        if let Some(decimals) = decimals {
            if !all_digits(decimals) || decimals.len() > 2 {
                return None;
            }
            number.push('.');
            number.push_str(decimals);
        }
        Some(number)
    }

    pub fn entry(typed: &str) -> Entry {
        let mut text = typed.trim();
        if text.is_empty() {
            return Entry::None;
        }
        if let Some(rest) = text.strip_prefix('$') {
            text = rest.trim();
        }
        let Some(number) = plain(text).or_else(|| grouped(text)) else { return Entry::Refused };
        match number.parse::<f64>() {
            Ok(value) if value.is_finite() && value <= MAXIMUM => {
                if value == 0.0 { Entry::None } else { Entry::Amount(value) }
            }
            _ => Entry::Refused,
        }
    }

    /// What the settings keep: `None` unless it is a positive amount within range.
    pub fn normalized(amount: Option<f64>) -> Option<f64> {
        amount.filter(|a| a.is_finite() && *a > 0.0 && *a <= MAXIMUM)
    }
}

/// When Pulse says "your September recap is ready", and what it says it about.
///
/// Pure: the caller hands over the date, what was already announced and the token count of the
/// month in question, so every rule is decidable from its arguments.
///
/// - Only in the first [`notice::WINDOW_DAYS`] days of a month, and only about the month before.
/// - Once per month: the month announced is remembered.
/// - Only when the month had records, which Pulse has seen. Without a scan nothing is said and
///   nothing is remembered.
pub mod notice {
    use chrono::{Datelike, Months, NaiveDate};

    use super::Period;

    pub const WINDOW_DAYS: u32 = 3;
    /// The notification's identifier, so a click can tell which month it is for.
    pub const IDENTIFIER_PREFIX: &str = "recap-ready-";

    /// The month a notification may be about today, or `None` outside the window.
    pub fn candidate(today: NaiveDate) -> Option<Period> {
        if today.day() > WINDOW_DAYS {
            return None;
        }
        let previous = today.checked_sub_months(Months::new(1))?;
        Some(Period::Month { year: previous.year(), month: previous.month() })
    }

    /// The month to announce now, or `None` for nothing to say.
    pub fn due(today: NaiveDate, announced: Option<Period>, tokens: impl Fn(Period) -> Option<i64>) -> Option<Period> {
        let month = candidate(today).filter(|m| announced != Some(*m))?;
        tokens(month).filter(|t| *t > 0).map(|_| month)
    }

    pub fn identifier(period: Period) -> String {
        format!("{IDENTIFIER_PREFIX}{}", period.key())
    }

    pub fn period_from_identifier(identifier: &str) -> Option<Period> {
        Period::from_key(identifier.strip_prefix(IDENTIFIER_PREFIX)?)
    }
}
