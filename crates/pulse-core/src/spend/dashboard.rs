// Ported from upstream Sources/Pulse/App/MenuDashboard.swift (`MenuAccountDetail.spend`).
//! The four figures the tray dashboard's account tab shows for a provider's local records — Today,
//! Busiest day, Last 31 days, All time — plus the 31-day bars, taken from one ledger so they add up
//! to the same numbers as the Token spend pane.

use chrono::{DateTime, Utc};
use serde::Serialize;

use super::calendar::Calendar;
use super::ledger::{Ledger, LedgerDay, SpanTotal};

/// The span upstream draws: the same one as the detailed card.
pub const SPAN: usize = 31;

/// Tokens and the money to show for them: `None` where none of the work had a price (drawn as a dash).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Figure {
    pub tokens: i64,
    pub cost: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Bar {
    pub date: DateTime<Utc>,
    pub tokens: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardSpend {
    pub today: Figure,
    pub busiest: Figure,
    pub recent: Figure,
    pub all_time: Figure,
    /// The last `SPAN` days, oldest first; empty unless there is more than one day of history
    /// (upstream draws no chart for a single day).
    pub bars: Vec<Bar>,
    pub currency: Option<String>,
}

fn of_day(day: &LedgerDay) -> Figure {
    Figure { tokens: day.tokens, cost: Ledger::shown_cost(day.cost, day.tokens, day.unpriced_tokens) }
}

fn of_total(t: SpanTotal) -> Figure {
    Figure { tokens: t.tokens, cost: Ledger::shown_cost(t.cost, t.tokens, t.unpriced) }
}

impl DashboardSpend {
    /// `None` for a ledger with no days — upstream's "No history yet".
    pub fn of(ledger: &Ledger, now: DateTime<Utc>, calendar: &Calendar) -> Option<Self> {
        if ledger.days.is_empty() {
            return None;
        }
        let recent_days = ledger.recent_in(SPAN, now, calendar);
        // Swift's `max(by: <)` keeps the last of equal days; so does this fold.
        let busiest = recent_days.iter().fold(None::<&LedgerDay>, |best, day| match best {
            Some(b) if day.tokens < b.tokens => Some(b),
            _ => Some(day),
        });
        let zero = Figure { tokens: 0, cost: Some(0.0) };
        Some(Self {
            today: ledger.today_in(now, calendar).map(of_day).unwrap_or_else(|| zero.clone()),
            busiest: busiest.map(of_day).unwrap_or(zero),
            recent: of_total(ledger.total_in(SPAN, now, calendar)),
            all_time: of_total(ledger.all_time()),
            bars: if ledger.days.len() > 1 {
                recent_days.iter().map(|d| Bar { date: d.date, tokens: d.tokens }).collect()
            } else {
                Vec::new()
            },
            currency: ledger.currency.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone};
    use std::collections::HashMap;

    fn ledger(days: &[(i64, f64, i64)], now: DateTime<Utc>) -> Ledger {
        let mut l = Ledger::empty();
        let first = now - Duration::days(days.len() as i64 - 1);
        l.days = days
            .iter()
            .enumerate()
            .map(|(i, (tokens, cost, unpriced))| {
                LedgerDay::new(first + Duration::days(i as i64), *tokens, *cost, *unpriced, HashMap::new())
            })
            .collect();
        l
    }

    fn noon() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 15, 0, 0, 0).unwrap()
    }

    #[test]
    fn empty_ledger_has_no_history() {
        assert_eq!(DashboardSpend::of(&Ledger::empty(), noon(), &Calendar::utc(1)), None);
    }

    #[test]
    fn figures_come_from_one_ledger() {
        let now = noon();
        let l = ledger(&[(100, 1.0, 0), (900, 9.0, 0), (50, 0.5, 0)], now);
        let s = DashboardSpend::of(&l, now, &Calendar::utc(1)).unwrap();
        assert_eq!(s.today.tokens, 50);
        assert_eq!(s.busiest, Figure { tokens: 900, cost: Some(9.0) });
        assert_eq!(s.recent.tokens, 1050);
        assert_eq!(s.all_time.tokens, 1050);
        assert_eq!(s.bars.len(), 3);
    }

    #[test]
    fn unpriced_work_has_no_cost_and_one_day_draws_no_bars() {
        let now = noon();
        let l = ledger(&[(70, 0.0, 70)], now);
        let s = DashboardSpend::of(&l, now, &Calendar::utc(1)).unwrap();
        assert_eq!(s.today.cost, None);
        assert!(s.bars.is_empty());
    }
}
