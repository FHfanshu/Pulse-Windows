// Ported from upstream Sources/Pulse/Usage/BurnRate.swift.
//! How a limit is being spent against its own window: whether you are ahead of an even burn, and
//! whether it will last.
//!
//! Everything here comes from one reading. How much is gone and how far through the window the
//! clock is are both in it already, so the rate is the first divided by the second. No history is
//! kept, nothing is sampled, and there is no reset to detect: a window that has turned over simply
//! reports a small elapsed share again.
//!
//! Two statements, and they are not equally trustworthy. Each is offered only when the evidence
//! carries it:
//! 1. the verdict, "expected to last": an extrapolation, but of the coarsest kind, which survives
//!    bursty usage far better than any number does;
//! 2. when, "in about an hour": a prediction, and the fragile one.

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::model::UsageWindow;

/// Nothing is said about a window barely open. Early on, the elapsed share is so small that
/// dividing by it turns a single burst into a rate that would empty the account before lunch.
pub const MINIMUM_ELAPSED: f64 = 0.03;

/// A prediction further out than this is not shown (seconds): it is the least certain figure in
/// the app and it stops being actionable long before it stops being computable.
pub const HORIZON: f64 = 2.0 * 3600.0;

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reading {
    /// Whether it is on course to run out before the window resets.
    pub exhausts_before_reset: bool,
    /// How long until it runs out, in seconds: only when that is before the reset and inside the
    /// horizon. None far more often than not, on purpose.
    pub time_to_exhaustion: Option<f64>,
}

/// What may be said about a window, or None when nothing may be.
pub fn reading(window: &UsageWindow, now: DateTime<Utc>) -> Option<Reading> {
    // `elapsed_fraction` already refuses a window whose length was chosen to sort by rather than
    // reported, which is exactly the length this would otherwise divide by.
    let elapsed = window.elapsed_fraction(now)?;
    if elapsed < MINIMUM_ELAPSED {
        return None;
    }
    let resets_at = window.resets_at?;

    let used = window.used_fraction.clamp(0.0, 1.0);
    let until_reset = (resets_at - now).num_milliseconds() as f64 / 1000.0;
    if until_reset <= 0.0 {
        return None;
    }

    // The window's own length, from the two figures that describe it.
    let elapsed_seconds = until_reset / (1.0 - elapsed) * elapsed;
    if elapsed_seconds <= 0.0 || !elapsed_seconds.is_finite() {
        return None;
    }

    let per_hour = used / elapsed_seconds * 3600.0;
    if used >= 1.0 || per_hour <= 0.0 {
        return Some(Reading { exhausts_before_reset: used >= 1.0, time_to_exhaustion: (used >= 1.0).then_some(0.0) });
    }

    let until_empty = (1.0 - used) / per_hour * 3600.0;
    let first = until_empty < until_reset;

    Some(Reading {
        exhausts_before_reset: first,
        // Both filters, and they are the difference between a figure and a guess: if the window
        // resets first there is no exhaustion to predict, and past two hours the answer is not
        // actionable.
        time_to_exhaustion: (first && until_empty <= HORIZON).then_some(until_empty),
    })
}

/// "about an hour", "about 40 minutes": rounded, because the precision is not real. "53
/// minutes" claims a minute's accuracy from a figure that moves by a factor of thirteen over one
/// afternoon.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind", content = "value")]
pub enum Approximate {
    UnderFifteenMinutes,
    AboutAnHour,
    AboutMinutes(i64),
    AboutHours(f64),
}

impl Approximate {
    /// The English phrase (upstream's localisation keys).
    pub fn english(&self) -> String {
        match self {
            Approximate::UnderFifteenMinutes => "under 15 minutes".to_string(),
            Approximate::AboutAnHour => "about an hour".to_string(),
            Approximate::AboutMinutes(minutes) => format!("about {minutes} minutes"),
            Approximate::AboutHours(hours) => {
                let text = if hours.fract() == 0.0 { format!("{hours:.0}") } else { format!("{hours:.1}") };
                format!("about {text} hours")
            }
        }
    }
}

pub fn approximate(seconds: f64) -> Approximate {
    let minutes = (seconds / 60.0).round() as i64;
    if minutes < 15 {
        return Approximate::UnderFifteenMinutes;
    }
    // Under an hour in quarters; past that in halves of an hour. The cut is at 68 rather than 75
    // because five quarters is an hour and a quarter: printing "about 75 minutes" between "about
    // an hour" and "about 1.5 hours" reads as a different scale for one step.
    if minutes < 68 {
        let quarters = ((minutes as f64 / 15.0).round() as i64).max(1);
        return if quarters == 4 { Approximate::AboutAnHour } else { Approximate::AboutMinutes(quarters * 15) };
    }
    Approximate::AboutHours((minutes as f64 / 60.0 * 2.0).round() / 2.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::WindowKind;
    use chrono::{Duration, TimeZone};

    fn now() -> DateTime<Utc> {
        Utc.timestamp_opt(1_800_000_000, 0).unwrap()
    }

    /// A five-hour window with `elapsed` of it gone and `used` spent.
    fn window(used: f64, elapsed: f64) -> UsageWindow {
        let length = 5 * 3600;
        let resets = now() + Duration::seconds(((1.0 - elapsed) * length as f64) as i64);
        UsageWindow::new("five", WindowKind::FiveHour, used, length).with_reset(Some(resets))
    }

    #[test]
    fn a_window_barely_open_says_nothing() {
        assert!(reading(&window(0.5, 0.02), now()).is_none());
    }

    #[test]
    fn an_even_burn_lasts_and_a_fast_one_runs_out() {
        let slow = reading(&window(0.2, 0.5), now()).unwrap();
        assert!(!slow.exhausts_before_reset);
        assert_eq!(slow.time_to_exhaustion, None);

        // 80% gone in 40% of the window: empty in a quarter of the window's length.
        let fast = reading(&window(0.8, 0.4), now()).unwrap();
        assert!(fast.exhausts_before_reset);
        let seconds = fast.time_to_exhaustion.unwrap();
        assert!((seconds - 0.2 / (0.8 / (0.4 * 5.0 * 3600.0))).abs() < 5.0, "{seconds}");
    }

    #[test]
    fn an_exhaustion_beyond_the_horizon_keeps_the_verdict_but_not_the_time() {
        // 20% gone in 15% of five hours: empty in three hours, before the reset but past the
        // two-hour horizon.
        let reading = reading(&window(0.2, 0.15), now()).unwrap();
        assert!(reading.exhausts_before_reset);
        assert_eq!(reading.time_to_exhaustion, None);
    }

    #[test]
    fn a_spent_window_is_exhausted_now() {
        let spent = reading(&window(1.0, 0.5), now()).unwrap();
        assert!(spent.exhausts_before_reset);
        assert_eq!(spent.time_to_exhaustion, Some(0.0));
    }

    #[test]
    fn a_window_with_nothing_used_never_runs_out() {
        let idle = reading(&window(0.0, 0.5), now()).unwrap();
        assert!(!idle.exhausts_before_reset);
        assert_eq!(idle.time_to_exhaustion, None);
    }

    #[test]
    fn a_length_that_only_orders_rows_says_nothing() {
        let mut w = window(0.5, 0.5);
        w.reports_length = false;
        assert!(reading(&w, now()).is_none());
    }

    #[test]
    fn durations_are_rounded() {
        assert_eq!(approximate(5.0 * 60.0), Approximate::UnderFifteenMinutes);
        assert_eq!(approximate(20.0 * 60.0), Approximate::AboutMinutes(15));
        assert_eq!(approximate(40.0 * 60.0), Approximate::AboutMinutes(45));
        assert_eq!(approximate(58.0 * 60.0), Approximate::AboutAnHour);
        assert_eq!(approximate(67.0 * 60.0), Approximate::AboutAnHour);
        assert_eq!(approximate(75.0 * 60.0), Approximate::AboutHours(1.5));
        assert_eq!(approximate(120.0 * 60.0), Approximate::AboutHours(2.0));
        assert_eq!(Approximate::AboutHours(1.5).english(), "about 1.5 hours");
        assert_eq!(Approximate::AboutHours(2.0).english(), "about 2 hours");
        assert_eq!(Approximate::AboutMinutes(45).english(), "about 45 minutes");
    }
}
