// Ported from upstream Settings/AccountHistoryModel.swift, AccountHistoryGroup.swift and the
// figures of Panel/AccountUsageCard.swift.
//! The account pane's "Usage history": what has actually been spent over time, as opposed to how
//! much of the current limit is left.
//!
//! **Upstream records nothing of its own.** The history is read each time the pane opens, from the
//! CLI's own transcripts ([`crate::spend`]'s per-file cache makes the second read cheap), or asked
//! of the provider (Z.ai's statistics, OpenCode Go's request log, DeepSeek's console). There is no
//! recorder to port; this module turns a [`Ledger`] into exactly the figures the card draws, so
//! the UI does no arithmetic of its own.
//!
//! TODO(history-providers): the provider-side histories (Z.ai `model-usage`, OpenCode Go's console
//! request log, DeepSeek's console usage) are not in our Rust provider modules yet, so those
//! accounts answer [`HistoryRead::Unsupported`] and the pane shows nothing for them. Codex's
//! "Limit reset credits" and "Account total" rows need its app server and are not ported either.

use std::path::Path;

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::provider::Provider;
use crate::spend::ledger::{Ledger, Origin, SpanTotal};
use crate::spend::{self, Calendar};

/// "Roughly a month, which is the span most of the figures cover."
pub const SPAN: usize = 31;

/// How the last read of a provider's history went. Kept beside the ledger rather than folded into
/// it: an empty chart has several causes and they must not be said the same way. Telling somebody
/// their account has no usage because the Wi-Fi dropped, beside a ring showing 80%, is the app
/// inventing a reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum HistoryRead {
    /// The source answered, even if the answer is that there is nothing there.
    Answered,
    /// The provider was asked and did not answer.
    Failed,
    /// A credential is missing, so it could not be asked.
    NotConfigured,
    /// The account is switched off, so Pulse has not asked for its history.
    NotAsked,
    /// This build reads no history for this provider.
    Unsupported,
}

/// Whether the provider leaves transcripts on this PC that the ledger reads (upstream
/// `keepsLocalTranscripts`). Not `provides_history`: that is true for both sources.
pub fn keeps_local_transcripts(provider: Provider) -> bool {
    spend::supports(provider)
}

/// Whether a spending history can be shown for this provider at all (upstream `providesHistory`).
/// Today only the transcript readers; see the module's TODO.
pub fn provides_history(provider: Provider) -> bool {
    keeps_local_transcripts(provider)
}

/// Tokens, and the money they cost where any of it had a price.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Figure {
    pub tokens: i64,
    /// `None` where none of the work had a published price: a dash, never a confident `$0.00`.
    pub cost: Option<f64>,
}

impl Figure {
    fn of(total: SpanTotal) -> Self {
        Figure { tokens: total.tokens, cost: Ledger::shown_cost(total.cost, total.tokens, total.unpriced) }
    }
}

/// One bar of the daily chart.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DayBar {
    pub date: DateTime<Utc>,
    pub tokens: i64,
}

/// One model's line in the table: its share of the month's tokens, its cache hit rate over the
/// same month, and its speed and first-token wait over the last day. A figure the records cannot
/// give is `None`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRow {
    pub name: String,
    pub share: Option<f64>,
    pub cache_rate: Option<f64>,
    pub speed: Option<f64>,
    pub first_token: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountHistory {
    pub provider: String,
    pub read: HistoryRead,
    /// What the figures are counted from; decides the footnote and whether money is shown.
    pub origin: Origin,
    /// What the money is counted in; `None` is dollars.
    pub currency: Option<String>,
    /// Whether the ledger has any day at all; without one the card is replaced by the empty state.
    pub has_days: bool,
    pub today: Figure,
    pub last_31_days: Figure,
    pub busiest_day: Figure,
    /// Only a ledger that goes back may call a figure "all time".
    pub all_time: Figure,
    pub last_7_days: Figure,
    /// The last 31 days, oldest first, for the chart (drawn when there is more than one).
    pub days: Vec<DayBar>,
    pub models: Vec<ModelRow>,
    /// The account's own cache rate, which is not the average of the models': each weighs what it
    /// read.
    pub overall_cache_rate: Option<f64>,
    pub has_partial_counts: bool,
    /// Models with no published price, so counted but not costed.
    pub unpriced_models: Vec<String>,
    /// Whether the reader times replies (Claude Code, Codex): decides the table's note.
    pub times_replies: bool,
    /// No model had enough replies in the last 24 hours to time.
    pub speeds_empty: bool,
}

impl AccountHistory {
    /// The card for a read that produced no ledger.
    pub fn empty(provider: Provider, read: HistoryRead) -> Self {
        Self::of(provider, read, &Ledger::empty(), Utc::now(), &Calendar::local())
    }

    pub fn of(provider: Provider, read: HistoryRead, ledger: &Ledger, now: DateTime<Utc>, calendar: &Calendar) -> Self {
        let recent = ledger.total_in(SPAN, now, calendar);
        let today = ledger
            .today_in(now, calendar)
            .map_or(Figure { tokens: 0, cost: Some(0.0) }, |d| Figure::of(SpanTotal { tokens: d.tokens, cost: d.cost, unpriced: d.unpriced_tokens }));
        let busiest = ledger
            .busiest_day_in(SPAN, now, calendar)
            .map_or(Figure { tokens: 0, cost: Some(0.0) }, |d| Figure::of(SpanTotal { tokens: d.tokens, cost: d.cost, unpriced: d.unpriced_tokens }));

        let speeds = ledger.output_speeds_by_model(now - chrono::Duration::seconds(Ledger::SPEED_SPAN_SECONDS));
        let rates = ledger.cache_hit_rates_by_model_in(SPAN, now, calendar);
        let shares = ledger.model_shares_in(SPAN, now, calendar);
        let rate_of = |name: &str| rates.iter().find(|r| r.name == name).map(|r| r.rate);
        let speed_of = |name: &str| speeds.iter().find(|s| s.name == name);

        // Every model with tokens this month, most first, and any timed in the last day that
        // somehow has none.
        let mut models: Vec<ModelRow> = shares
            .iter()
            .map(|s| ModelRow {
                name: s.name.clone(),
                share: Some(s.share),
                cache_rate: rate_of(&s.name),
                speed: speed_of(&s.name).and_then(|m| m.tokens_per_second),
                first_token: speed_of(&s.name).and_then(|m| m.first_token_seconds),
            })
            .collect();
        for timed in speeds.iter().filter(|s| !shares.iter().any(|m| m.name == s.name)) {
            models.push(ModelRow {
                name: timed.name.clone(),
                share: None,
                cache_rate: rate_of(&timed.name),
                speed: timed.tokens_per_second,
                first_token: timed.first_token_seconds,
            });
        }

        let local = ledger.origin == Origin::LocalTranscripts;
        AccountHistory {
            provider: provider.raw().to_string(),
            read,
            origin: ledger.origin,
            currency: ledger.currency.clone(),
            has_days: !ledger.days.is_empty(),
            today,
            last_31_days: Figure::of(recent),
            busiest_day: busiest,
            all_time: Figure::of(ledger.all_time()),
            last_7_days: Figure::of(ledger.total_in(7, now, calendar)),
            days: ledger.recent_in(SPAN, now, calendar).into_iter().map(|d| DayBar { date: d.date, tokens: d.tokens }).collect(),
            models,
            overall_cache_rate: ledger.cache_hit_rate_in(SPAN, now, calendar),
            has_partial_counts: ledger.has_partial_counts,
            unpriced_models: if local { ledger.unpriced_models.clone() } else { Vec::new() },
            times_replies: matches!(provider, Provider::ClaudeCode | Provider::Codex),
            speeds_empty: speeds.is_empty(),
        }
    }
}

/// Read the account's history. Blocking (it scans transcripts): call it from `spawn_blocking`.
///
/// `enabled` and `primary` are the account's: history is per provider, read from that CLI's
/// transcripts, which do not say which account was signed in at the time, so an added account
/// (or one switched off) is never asked.
pub fn read(provider: Provider, enabled: bool, primary: bool, home: &Path, now: DateTime<Utc>) -> AccountHistory {
    if !enabled || !primary {
        return AccountHistory::empty(provider, HistoryRead::NotAsked);
    }
    if !provides_history(provider) {
        return AccountHistory::empty(provider, HistoryRead::Unsupported);
    }
    match spend::read_ledger(provider, home, now) {
        // Reading this PC's own files always answers, even when the answer is that there is
        // nothing there.
        Ok(ledger) => AccountHistory::of(provider, HistoryRead::Answered, &ledger, now, &Calendar::local()),
        Err(_) => AccountHistory::empty(provider, HistoryRead::Unsupported),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use chrono::TimeZone;

    use super::*;
    use crate::spend::ledger::LedgerDay;

    fn calendar() -> Calendar {
        Calendar::utc(2)
    }

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 9, 12, 0, 0).unwrap()
    }

    fn day(days_ago: i64, tokens: i64, cost: f64, unpriced: i64, models: &[(&str, i64)]) -> LedgerDay {
        let date = calendar().start_of_day(now()) - chrono::Duration::days(days_ago);
        let models: HashMap<String, i64> = models.iter().map(|(m, t)| (m.to_string(), *t)).collect();
        LedgerDay::new(date, tokens, cost, unpriced, models)
    }

    fn ledger(days: Vec<LedgerDay>) -> Ledger {
        Ledger { days, ..Ledger::empty() }
    }

    #[test]
    fn the_figures_add_up_over_their_spans() {
        let ledger = ledger(vec![
            day(40, 1_000, 1.0, 0, &[("a", 1_000)]),
            day(10, 4_000, 4.0, 0, &[("a", 3_000), ("b", 1_000)]),
            day(2, 2_000, 2.0, 0, &[("a", 2_000)]),
            day(0, 500, 0.5, 0, &[("b", 500)]),
        ]);
        let history = AccountHistory::of(Provider::ClaudeCode, HistoryRead::Answered, &ledger, now(), &calendar());

        assert!(history.has_days);
        assert_eq!(history.today, Figure { tokens: 500, cost: Some(0.5) });
        assert_eq!(history.last_7_days.tokens, 2_500);
        assert_eq!(history.last_31_days.tokens, 6_500);
        assert_eq!(history.all_time.tokens, 7_500);
        assert_eq!(history.busiest_day.tokens, 4_000);
        // 31 calendar days up to today, oldest first, gaps closed.
        assert_eq!(history.days.len(), 31);
        assert_eq!(history.days.last().unwrap().tokens, 500);
    }

    #[test]
    fn models_are_listed_by_share_largest_first() {
        let ledger = ledger(vec![day(1, 4_000, 4.0, 0, &[("a", 3_000), ("b", 1_000)])]);
        let history = AccountHistory::of(Provider::Codex, HistoryRead::Answered, &ledger, now(), &calendar());
        let names: Vec<&str> = history.models.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["a", "b"]);
        assert_eq!(history.models[0].share, Some(0.75));
        assert!(history.times_replies);
        assert!(history.speeds_empty);
    }

    #[test]
    fn work_with_no_price_shows_no_money_not_a_zero() {
        let ledger = ledger(vec![day(0, 1_000, 0.0, 1_000, &[("mystery", 1_000)])]);
        let history = AccountHistory::of(Provider::ClaudeCode, HistoryRead::Answered, &ledger, now(), &calendar());
        assert_eq!(history.today, Figure { tokens: 1_000, cost: None });
        assert_eq!(history.last_31_days.cost, None);
    }

    #[test]
    fn an_empty_ledger_has_no_days_and_says_what_was_read() {
        let history = AccountHistory::empty(Provider::Codex, HistoryRead::NotAsked);
        assert!(!history.has_days);
        assert_eq!(history.read, HistoryRead::NotAsked);
        assert!(history.models.is_empty());
        assert!(history.days.is_empty());
    }

    #[test]
    fn only_a_primary_account_that_is_switched_on_is_asked() {
        let home = Path::new("C:/nowhere-pulse-test");
        let off = read(Provider::ClaudeCode, false, true, home, now());
        assert_eq!(off.read, HistoryRead::NotAsked);
        let added = read(Provider::ClaudeCode, true, false, home, now());
        assert_eq!(added.read, HistoryRead::NotAsked);
        // A provider whose history is asked of the provider itself is not read here yet.
        let cursor = read(Provider::Cursor, true, true, home, now());
        assert_eq!(cursor.read, HistoryRead::Unsupported);
    }

    #[test]
    fn unpriced_models_are_named_only_for_local_transcripts() {
        let mut local = ledger(vec![day(0, 10, 0.0, 10, &[("x", 10)])]);
        local.unpriced_models = vec!["x".into()];
        let history = AccountHistory::of(Provider::ClaudeCode, HistoryRead::Answered, &local, now(), &calendar());
        assert_eq!(history.unpriced_models, ["x"]);

        local.origin = Origin::ProviderStatistics;
        let history = AccountHistory::of(Provider::Zai, HistoryRead::Answered, &local, now(), &calendar());
        assert!(history.unpriced_models.is_empty());
    }
}
