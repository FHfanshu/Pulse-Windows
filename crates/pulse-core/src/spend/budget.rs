// Ported from upstream Sources/Pulse/Usage/BudgetEstimate.swift.
//! What a rate-limit window is worth in money.
//!
//! The providers publish a percentage and nothing else: they will tell you a weekly limit is 3%
//! gone, never what the other 97% is worth. But the two halves of that answer are both here:
//! they report the percentage, and the local logs say what was actually spent in the same
//! stretch of time. Divide one by the other and the window has a price.
//!
//! This is the one number in Pulse that is inferred rather than reported, so it is labelled as an
//! estimate wherever it appears, and withheld rather than guessed when the inputs can't support it:
//! - too little used, and the percentage's own rounding swamps the answer;
//! - logs that start after the window did miss part of the spending;
//! - work done on another machine is invisible here, and pulls the same way. Nothing can detect
//!   that, which is the honest limit of this figure.

use chrono::{DateTime, Duration, Utc};
use serde::Serialize;

use super::ledger::Ledger;
use crate::model::UsageWindow;

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BudgetEstimate {
    /// What the whole window is worth.
    pub full: f64,
    /// What is left of it.
    pub remaining: f64,
    /// What has already gone, by this machine's reckoning.
    pub spent: f64,
}

/// Below this the percentage is too coarse to divide by. The providers report whole numbers, so
/// at p% used the true figure is anywhere in a point around p and the answer moves by up to 1/p,
/// as a sawtooth: the spend climbs while the percentage holds, then the percentage ticks and the
/// figure drops. At 2% that was half the figure, which is noise, not an estimate. At 5% it is a
/// tenth.
pub const MINIMUM_USED: f64 = 0.05;
/// And below this there isn't enough money in play to be worth reporting.
pub const MINIMUM_SPEND: f64 = 0.20;

/// "≈$220": whole dollars from a hundred up, because the figure is an estimate and cents would
/// claim a precision it does not have. Formatted for an English (US) reader; the UI localises.
pub fn approximate(amount: f64) -> String {
    let digits = if amount >= 100.0 { 0 } else { 2 };
    let text = format!("{amount:.digits$}");
    let (whole, fraction) = match text.split_once('.') {
        Some((whole, fraction)) => (whole.to_string(), format!(".{fraction}")),
        None => (text, String::new()),
    };
    let mut grouped = String::new();
    for (index, c) in whole.chars().enumerate() {
        if index > 0 && (whole.len() - index) % 3 == 0 && c != '-' {
            grouped.push(',');
        }
        grouped.push(c);
    }
    format!("≈${grouped}{fraction}")
}

/// Whether a limit's worth is something to estimate at all (upstream
/// `UsageDetailCard.estimatesValue`, which the detailed card applies).
///
/// Not for OpenCode Go: its plan states every limit in money (its docs give the amounts and the
/// console's meters count them in micro-cents), so dividing this PC's spend by a percentage would
/// put a guess beside a figure the provider already publishes, and could disagree with it. Nor for
/// DeepSeek, whose ring is a balance: it is money already, in the account's own currency, not a
/// limit to price.
pub fn estimates_value(provider: crate::provider::Provider) -> bool {
    use crate::provider::Provider;
    provider != Provider::OpenCodeGo && provider != Provider::DeepSeek
}

/// `observed_at` is when the percentage was read. The spend is counted up to that moment, not to
/// now: the percentage does not move between readings while the logs do, and counting what was
/// spent since the last reading against the old percentage set the figure high by however much
/// work the refresh interval held.
pub fn estimate(
    window: &UsageWindow,
    ledger: &Ledger,
    observed_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Option<BudgetEstimate> {
    if window.window_seconds <= 0
        // A length that only orders the rows is not a period. Kimi's weekly allowance states a
        // reset and no length; seven days back from the reset is a guess at when its spending
        // began.
        || !window.reports_length
        // Account-wide windows only. A limit scoped to one model is spent by that model alone,
        // but the logs' spending for the period is everything together.
        || window.scope.is_some()
        || window.used_fraction < MINIMUM_USED
    {
        return None;
    }
    let resets = window.resets_at?;

    let opened = resets - Duration::seconds(window.window_seconds);
    // Read inside this window, or the percentage is another window's.
    let read = observed_at.unwrap_or(now).min(now);
    if !(opened < read && read < resets) {
        return None;
    }

    // Logs that begin after the window did would only show part of the spending, and the
    // shortfall lands straight in the answer.
    let first_logged = ledger.slots.first()?.start;
    if first_logged > opened {
        return None;
    }

    let spent = ledger.cost_between(opened, read);
    if spent < MINIMUM_SPEND {
        return None;
    }

    let full = spent / window.used_fraction;
    Some(BudgetEstimate { full, remaining: (full - spent).max(0.0), spent })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::WindowKind;
    use crate::spend::ledger::Slot;
    use chrono::TimeZone;

    fn at(seconds: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(seconds, 0).unwrap()
    }

    /// 100 s into a quarter-hour.
    fn opened() -> DateTime<Utc> {
        at(1_790_000_100)
    }
    const QUARTER: i64 = 15 * 60;

    fn ledger(slots: &[(i64, f64)]) -> Ledger {
        Ledger {
            slots: slots.iter().map(|(offset, cost)| Slot::new(at(1_790_000_000 + offset), 1, *cost)).collect(),
            ..Ledger::empty()
        }
    }

    fn window(used: f64) -> UsageWindow {
        UsageWindow::new("test.five", WindowKind::FiveHour, used, 5 * 3600).with_reset(Some(opened() + Duration::seconds(5 * 3600)))
    }

    #[test]
    fn the_quarter_hour_a_window_opened_in_counts_for_its_share_inside_the_window() {
        // 900 s quarter, window opened 100 s in: 800/900 of it is inside.
        let ledger = ledger(&[(0, 9.0), (QUARTER, 1.0)]);
        let read = opened() + Duration::seconds(3600);
        let estimate = estimate(&window(0.1), &ledger, Some(read), read).unwrap();
        assert!((estimate.spent - 9.0).abs() < 1e-9);
        assert!((estimate.full - 90.0).abs() < 1e-9);
    }

    #[test]
    fn work_after_the_percentage_was_read_is_not_set_against_it() {
        let ledger = ledger(&[(0, 0.0), (QUARTER, 5.0), (4 * QUARTER, 50.0)]);
        let read = opened() + Duration::seconds(2 * QUARTER);
        let estimate = estimate(&window(0.1), &ledger, Some(read), read + Duration::seconds(3600)).unwrap();
        assert!((estimate.spent - 5.0).abs() < 1e-9);
    }

    #[test]
    fn too_little_used_or_a_reading_from_before_the_window_gives_no_estimate() {
        let ledger = ledger(&[(0, 1.0), (QUARTER, 5.0)]);
        let later = opened() + Duration::seconds(3600);
        assert!(estimate(&window(0.04), &ledger, Some(later), later).is_none());
        assert!(estimate(&window(0.05), &ledger, Some(later), later).is_some());
        assert!(estimate(&window(0.1), &ledger, Some(opened() - Duration::seconds(60)), later).is_none());
    }

    #[test]
    fn a_window_whose_length_only_orders_the_rows_gives_no_estimate() {
        let ledger = ledger(&[(0, 1.0), (QUARTER, 5.0)]);
        let later = opened() + Duration::seconds(3600);
        let mut window = window(0.5);
        assert!(estimate(&window, &ledger, Some(later), later).is_some());
        window.reports_length = false;
        assert!(estimate(&window, &ledger, Some(later), later).is_none());
    }

    #[test]
    fn logs_that_start_after_the_window_opened_and_scoped_windows_are_withheld() {
        let later = opened() + Duration::seconds(3600);
        assert!(estimate(&window(0.5), &ledger(&[(QUARTER, 5.0)]), Some(later), later).is_none());
        let scoped = window(0.5).with_scope("opus");
        assert!(estimate(&scoped, &ledger(&[(0, 5.0)]), Some(later), later).is_none());
        // Below the spend floor.
        assert!(estimate(&window(0.5), &ledger(&[(0, 0.1)]), Some(later), later).is_none());
    }

    #[test]
    fn approximate_drops_cents_from_a_hundred_up() {
        assert_eq!(approximate(220.4), "≈$220");
        assert_eq!(approximate(12.5), "≈$12.50");
        assert_eq!(approximate(1234.0), "≈$1,234");
        assert_eq!(approximate(99.994), "≈$99.99");
    }
}
