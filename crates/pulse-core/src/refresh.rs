// Ported from upstream Usage/AdaptiveRefresh.swift.
//! How long to wait before the next pass. One-shot: the wait is recomputed after
//! every pass, and every signal can only lengthen it.

use chrono::{DateTime, Utc};

pub const FLOOR_SECONDS: i64 = 120;
pub const CEILING_SECONDS: i64 = 1800;
pub const UNWATCHED_CEILING_SECONDS: i64 = 300;

#[derive(Debug, Clone, Default)]
pub struct Signals {
    pub last_agent_activity: Option<DateTime<Utc>>,
    pub last_change: Option<DateTime<Utc>>,
    pub last_looked: Option<DateTime<Utc>>,
    pub is_panel_visible: bool,
    /// Battery saver / metered connection.
    pub is_constrained: bool,
}

pub fn interval(signals: &Signals, is_watched: bool, now: DateTime<Utc>) -> i64 {
    if signals.is_constrained || !signals.is_panel_visible {
        return CEILING_SECONDS;
    }
    let cap = if is_watched { CEILING_SECONDS } else { UNWATCHED_CEILING_SECONDS };
    let quiet = [signals.last_agent_activity, signals.last_change, signals.last_looked]
        .into_iter()
        .flatten()
        .map(|t| (now - t).num_seconds())
        .filter(|age| *age >= 0)
        .min();
    let Some(quiet) = quiet else { return cap };
    let wait = match quiet {
        q if q < 5 * 60 => FLOOR_SECONDS,
        q if q < 60 * 60 => 300,
        q if q < 4 * 60 * 60 => 900,
        _ => CEILING_SECONDS,
    };
    wait.min(cap)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn recent_activity_hits_the_floor_and_hidden_panel_the_ceiling() {
        let now = Utc::now();
        let mut s = Signals { is_panel_visible: true, last_change: Some(now - Duration::minutes(1)), ..Default::default() };
        assert_eq!(interval(&s, true, now), FLOOR_SECONDS);
        s.is_panel_visible = false;
        assert_eq!(interval(&s, true, now), CEILING_SECONDS);
    }

    #[test]
    fn no_signals_waits_the_cap() {
        let s = Signals { is_panel_visible: true, ..Default::default() };
        assert_eq!(interval(&s, false, Utc::now()), UNWATCHED_CEILING_SECONDS);
    }
}
