// Ported from upstream Sources/Pulse/Usage/RecapBuild.swift.
//! A calendar month's or year's recap from the Token spend ledgers.
//!
//! Everything is added up by [`SpendSummary::of_range`] over `[start, end)` (totals, models,
//! agents, projects, sessions, hours), so a recap and the Token spend pane cannot count one span
//! two ways. What is worked out here is what the summary does not carry: the previous period,
//! each agent's active days, per-model money, the late-night figures, the cache savings and the
//! persona.
//!
//! Calendar-bounded, to today: a period still running ends at the end of today, and its previous
//! period is cut to the same number of days (October 1-5 against September 1-5). A period that
//! has not started is an empty recap, not an error. `None` only for a month outside 1..=12.

use std::collections::{BTreeSet, HashMap};

use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc};

use super::{AgentShare, Day, ModelShare, Month, Period, Persona, ProjectShare, Recap};
use crate::spend::prices::{ModelPriceLookup, PriceTable};
use crate::spend::summary::SpendSummary;
use crate::spend::tally::TokenTally;
use crate::spend::{Calendar, Ledger, SpendAgent};

/// Money for some work, `None` where there was no work or none of it had a price.
fn cost(cost: f64, tokens: i64, unpriced: i64) -> Option<f64> {
    if tokens > 0 {
        Ledger::shown_cost(cost, tokens, unpriced)
    } else {
        None
    }
}

/// The same work with every cache read billed as fresh input, long-context bands included: what
/// it would have cost with no cache to read from.
fn cache_read_as_input(tally: &TokenTally) -> TokenTally {
    let mut copy = tally.clone();
    copy.input += copy.cache_read;
    copy.cache_read = 0;
    copy.context_bands = tally.context_bands.iter().map(|(band, inner)| (*band, cache_read_as_input(inner))).collect();
    copy
}

/// The quiet that ends a stretch of work.
pub const WORK_BREAK: Duration = Duration::hours(3);
/// A stretch longer than this is a process left running, not a workday.
pub const LONGEST_WORKDAY: Duration = Duration::hours(20);

/// One stretch of work: when it ended, as minutes after the midnight of the day it began (so past
/// 1440 the next morning), and whether it ran over a midnight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Workday {
    pub end_minute: u32,
    pub crosses_midnight: bool,
}

pub fn workdays(instants: &[DateTime<Utc>], calendar: &Calendar) -> Vec<Workday> {
    let mut sorted = instants.to_vec();
    sorted.sort();
    let Some(&first) = sorted.first() else { return Vec::new() };
    let (mut first, mut last) = (first, first);
    let mut result = Vec::new();
    let mut close = |first: DateTime<Utc>, last: DateTime<Utc>| {
        if last - first > LONGEST_WORKDAY {
            return;
        }
        let midnight = calendar.start_of_day(first);
        result.push(Workday {
            end_minute: (last - midnight).num_minutes().max(0) as u32,
            crosses_midnight: calendar.date(first) != calendar.date(last),
        });
    };
    for &instant in &sorted[1..] {
        if instant - last > WORK_BREAK {
            close(first, last);
            first = instant;
        }
        last = instant;
    }
    close(first, last);
    result
}

/// Build a recap. See the module note for the rules this adds to the summary's.
///
/// - `hours`: `None` when any contributing store has only session- or report-level timing, and
///   when no quarter-hour work was recorded.
/// - Late nights (`late_nights`, `latest_minute`) are read from **stretches of work**
///   ([`workdays`]): every agent's quarter-hours and sessions' last records, split wherever
///   nothing happened for `WORK_BREAK`. A stretch ends at its last instant, measured from the
///   midnight of the day it began, so an all-nighter that stops at 07:00 finishes at 31:00, later
///   than anything that stopped the same evening, and `latest_minute` is the latest of them
///   (shown on a clock, so 07:00). A stretch that crosses midnight is a late night; one that only
///   *starts* after it (04:30) is an early start, not a night. A stretch longer than
///   `LONGEST_WORKDAY` is not a person's day and is left out. It once took only 00:00-04:59: the
///   all-nighter read 04:45, and someone who always stopped at 23:50 got no figure at all. Work
///   with only day-level timing has no instants: where `hours` is `None`, both are floors.
/// - `cache_hit_rate`: the rule of `Ledger::cache_hit_rate_in`, over every contributing agent:
///   `None` when any of them has counts that may be missing, and a store that records no cache
///   is left out of it.
/// - `cache_savings`: per model and per day, the cache reads billed at the input rate less what
///   they were billed at (long-context tiers included); a model with no price adds nothing, and
///   the figure is `None` where no priced cache read exists.
/// - Streaks are of the period's own days, not the whole history.
pub fn build(
    period: Period,
    ledgers: &HashMap<SpendAgent, Ledger>,
    prices: &PriceTable,
    now: DateTime<Utc>,
    calendar: &Calendar,
) -> Option<Recap> {
    let (start, bounds_end) = period.bounds()?;
    let today = calendar.date(now);
    let tomorrow = today + Duration::days(1);
    let is_in_progress = today >= start && today < bounds_end;
    // Ends at the end of today while running; an unstarted period is empty.
    let end = bounds_end.min(start.max(tomorrow));
    let (start_at, end_at) = (calendar.midnight(start), calendar.midnight(end));

    let summary = SpendSummary::of_range(ledgers, start_at, end_at, now, calendar);
    let elapsed_days = summary.days.len();
    // Only a first record inside the span moves anything: one before the start leaves the whole
    // period observed.
    let first_record = super::periods::earliest(ledgers, calendar);
    let records_begin = first_record.filter(|first| *first > start && *first < end);
    let in_span = |at: DateTime<Utc>| at >= start_at && at < end_at;

    // The previous period. A running period against the same number of days of the one before
    // (capped at its end: March 29 has no February 29); a finished one against the whole of it.
    // Either way the days are kept, and the change is worked out per day, so 31 days are never
    // weighed against 28 nor September against August 1-30. Days before this PC's first record
    // are not part of the span.
    let mut previous_tokens = None;
    let mut previous_days = None;
    if elapsed_days > 0 {
        if let Some((previous_start, previous_end)) = period.previous().bounds() {
            let same = previous_start + Duration::days(elapsed_days as i64);
            let until = if is_in_progress { previous_end.min(same) } else { previous_end };
            let from = previous_start.max(first_record.unwrap_or(previous_start));
            if from < until {
                let before =
                    SpendSummary::of_range(ledgers, calendar.midnight(from), calendar.midnight(until), now, calendar);
                if before.tokens > 0 {
                    previous_tokens = Some(before.tokens);
                    previous_days = Some((until - from).num_days() as usize);
                }
            }
        }
    }

    // Days and months.
    let dated: Vec<(NaiveDate, &crate::spend::summary::SummaryDay)> =
        summary.days.iter().map(|day| (calendar.date(day.date), day)).collect();
    let days: Vec<Day> = dated
        .iter()
        .map(|(date, day)| Day { date: *date, tokens: day.tokens, cost: cost(day.cost, day.tokens, day.unpriced_tokens) })
        .collect();

    let mut months = Vec::new();
    if period.is_year() {
        for month in 1..=12u32 {
            let in_month: Vec<_> = dated.iter().filter(|(date, _)| date.month() == month).collect();
            let tokens: i64 = in_month.iter().map(|(_, d)| d.tokens).sum();
            let money: f64 = in_month.iter().map(|(_, d)| d.cost).sum();
            let unpriced: i64 = in_month.iter().map(|(_, d)| d.unpriced_tokens).sum();
            months.push(Month {
                month,
                tokens,
                cost: cost(money, tokens, unpriced),
                active_days: in_month.iter().filter(|(_, d)| d.tokens > 0).count(),
            });
        }
    }

    // The earliest of the heaviest days: a tie is not a later day's.
    let mut busiest: Option<&Day> = None;
    for day in &days {
        if day.tokens > busiest.map_or(0, |b| b.tokens) {
            busiest = Some(day);
        }
    }
    let busiest_day = busiest.cloned();

    // Time of day.
    let mut hours: Option<Vec<i64>> = None;
    if !summary.has_aggregate_timing && summary.hours.values().sum::<i64>() > 0 {
        hours = Some((0..24u32).map(|hour| summary.hours.get(&hour).copied().unwrap_or(0)).collect());
    }
    let hour_total: i64 = hours.as_ref().map_or(0, |h| h.iter().sum());
    let (mut peak_hour, mut late_share) = (None, None);
    if let Some(hours) = hours.as_ref().filter(|_| hour_total > 0) {
        // The earliest hour among equals, so a tie reads the same twice.
        let mut best = 0;
        for hour in 1..24 {
            if hours[hour] > hours[best] {
                best = hour;
            }
        }
        peak_hour = Some(best as u32);
        late_share = Some(Recap::LATE_HOURS.iter().map(|h| hours[*h]).sum::<i64>() as f64 / hour_total as f64);
    }

    // Per ledger: late nights, cache, per-model money.
    let mut lookup = ModelPriceLookup::new(prices);
    let mut instants: Vec<DateTime<Utc>> = Vec::new();
    let mut cache_tally = TokenTally::default();
    let mut cache_unvouched = false;
    let mut savings = 0.0;
    let mut priced_cache_reads = 0i64;
    let mut model_money: HashMap<String, f64> = HashMap::new();

    for share in &summary.agents {
        let agent = share.agent;
        let Some(ledger) = ledgers.get(&agent) else { continue };

        // A quarter-hour's start, and a session's own last record, are the instants of work the
        // stretches below are read from.
        instants.extend(ledger.slots.iter().filter(|s| s.tokens > 0 && in_span(s.start)).map(|s| s.start));
        for session in ledger.sessions.iter().filter(|s| !s.slots.is_empty() && in_span(s.end)) {
            instants.push(session.end);
        }

        let span: Vec<_> = ledger.days.iter().filter(|d| in_span(d.date)).cloned().collect();

        if agent.reports_cache_reads() {
            match ledger.cache_reading(&span) {
                crate::spend::ledger::CacheReading::Measured(tally) => cache_tally += &tally,
                crate::spend::ledger::CacheReading::Unrecorded => {}
                crate::spend::ledger::CacheReading::Unvouched => cache_unvouched = true,
            }
        }

        for day in &span {
            for (raw, costs) in &day.model_costs {
                *model_money.entry(ledger.model_names.get(raw).unwrap_or(raw).clone()).or_default() += costs.total();
            }
            if !agent.reports_cache_reads() {
                continue;
            }
            for (raw, tally) in day.model_tallies.iter().filter(|(_, t)| t.cache_read > 0) {
                let Some(price) = lookup.price(raw, agent.price_vendor()) else { continue };
                savings += cache_read_as_input(tally).cost_at(price) - tally.cost_at(price);
                priced_cache_reads += tally.cache_read;
            }
        }
    }

    let workdays = workdays(&instants, calendar);
    let latest_minute = workdays.iter().map(|w| w.end_minute).max();
    let late_nights = workdays.iter().filter(|w| w.crosses_midnight).count();

    let cache_input = cache_tally.input + cache_tally.cache_write + cache_tally.cache_read;
    let cache_hit_rate =
        (!cache_unvouched && cache_input > 0).then(|| cache_tally.cache_read as f64 / cache_input as f64);

    // Shares.
    let tokens = summary.tokens;
    let share_of = |part: i64| if tokens > 0 { (part as f64 / tokens as f64).min(1.0) } else { 0.0 };

    let agents: Vec<AgentShare> = summary
        .agents
        .iter()
        .map(|row| {
            // The agent's own days, from its own ledger: the same days the summary adds up.
            let active_dates: BTreeSet<NaiveDate> = ledgers
                .get(&row.agent)
                .map(|ledger| {
                    ledger
                        .days
                        .iter()
                        .filter(|d| d.tokens > 0 && d.date >= start_at && d.date < end_at)
                        .map(|d| calendar.date(d.date))
                        .collect()
                })
                .unwrap_or_default();
            AgentShare {
                agent: row.agent,
                name: row.agent.display_name(),
                icon: row.agent.icon_resource(),
                tokens: row.tokens,
                share: share_of(row.tokens),
                active_days: active_dates.len(),
                cost: cost(row.cost, row.tokens, row.unpriced_tokens),
                active_dates,
            }
        })
        .collect();

    let (current_streak, longest_streak) = Recap::streaks(&days, is_in_progress);
    let persona = hours.as_deref().and_then(Persona::of);

    Some(Recap {
        period,
        start,
        end,
        is_in_progress,
        tokens,
        cost: cost(summary.cost, tokens, summary.unpriced_tokens),
        unpriced_tokens: summary.unpriced_tokens,
        previous_tokens,
        active_days: summary.active_days(),
        elapsed_days,
        records_begin,
        previous_days,
        sessions: summary.sessions.len(),
        days,
        months,
        hours,
        peak_hour,
        late_share,
        latest_minute,
        late_nights,
        persona,
        models: summary
            .models
            .iter()
            .map(|model| ModelShare {
                name: model.name.clone(),
                tokens: model.tokens,
                share: share_of(model.tokens),
                cost: model_money.get(&model.name).copied(),
            })
            .collect(),
        agents,
        projects: summary
            .projects
            .iter()
            .filter(|p| p.tokens > 0)
            .map(|project| ProjectShare {
                name: project.name.clone(),
                tokens: project.tokens,
                share: share_of(project.tokens),
                sessions: project.sessions,
            })
            .collect(),
        cache_hit_rate,
        cache_savings: (priced_cache_reads > 0).then_some(savings),
        current_streak,
        longest_streak,
        busiest_day,
        currency: "USD".to_string(),
        is_partial: summary.has_partial_counts,
    })
}
