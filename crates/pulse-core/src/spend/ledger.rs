// Ported from upstream Sources/Pulse/Usage/UsageLedger.swift (the value types and the pricing pass).
//! A provider's history, worked out from the logs its own CLI leaves on this machine.
//!
//! The providers report limits, not spending, and neither publishes a per-day history, so the
//! only place the day-by-day story exists is the transcripts on disk. That makes this local by
//! nature: work done on another machine is not here, and neither is anything the CLI has since
//! pruned. The money is a translation, not a bill: the figure is what the same tokens would cost
//! at the providers' published API rates.

use std::collections::{BTreeSet, HashMap};

use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use serde::{Deserialize, Serialize};

use super::calendar::Calendar;
use super::prices::{ModelPriceLookup, PriceTable};
use super::project::UsageProject;
use super::tally::{ReplyTiming, TokenCost, TokenTally};

/// Tokens by quarter-hour key (`yyyy-MM-dd HH:mm`, local) and then by model.
pub type Buckets = HashMap<String, HashMap<String, TokenTally>>;

/// Where the figures came from, which decides what may be said about them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Origin {
    /// Scanned from the CLI's session files on this machine.
    #[default]
    LocalTranscripts,
    /// Imported from decoded records (a capture, a dropped log, another tool's store).
    ImportedRecords,
    /// Asked of the provider, so it covers the whole account: one token total per model and no
    /// money behind it.
    ProviderStatistics,
    /// The provider's own log of every request on the account, with the cost it charged.
    ProviderLogs,
}

impl Origin {
    /// Whether a ledger with this origin may appear in the token spend views: records read or
    /// imported here can; a provider's own statistics cannot, because they carry no money.
    pub fn supports_token_spend(self) -> bool {
        matches!(self, Origin::LocalTranscripts | Origin::ImportedRecords)
    }
}

/// One day's work, priced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LedgerDay {
    /// Local midnight.
    pub date: DateTime<Utc>,
    pub tokens: i64,
    pub cost: f64,
    /// Tokens spent on models with no published price. They count towards `tokens` but not
    /// `cost`.
    pub unpriced_tokens: i64,
    /// Tokens by raw model id.
    pub models: HashMap<String, i64>,
    /// The same day split by kind of token: fresh input, cache written, cache read, output.
    pub tally: TokenTally,
    /// The same day split by raw model id, each id with its own tally. Empty for a day read
    /// before the model detail was kept, which is how a per-model breakdown knows its
    /// categories are missing.
    #[serde(skip_serializing_if = "HashMap::is_empty", default)]
    pub model_tallies: HashMap<String, TokenTally>,
    /// The same day by raw model id, each with what its own tokens cost at that model's own
    /// rates. A model absent here has no published price: counted, never priced.
    #[serde(skip_serializing_if = "HashMap::is_empty", default)]
    pub model_costs: HashMap<String, TokenCost>,
    /// Tokens that could not be placed in any of the four kinds, by raw model id. Counted, never
    /// invented into `input`, never priced.
    #[serde(skip_serializing_if = "HashMap::is_empty", default)]
    pub model_unclassified_tokens: HashMap<String, i64>,
}

impl LedgerDay {
    /// A quiet day.
    pub fn quiet(date: DateTime<Utc>) -> Self {
        Self::new(date, 0, 0.0, 0, HashMap::new())
    }

    pub fn new(date: DateTime<Utc>, tokens: i64, cost: f64, unpriced_tokens: i64, models: HashMap<String, i64>) -> Self {
        Self {
            date,
            tokens,
            cost,
            unpriced_tokens,
            models,
            tally: TokenTally::default(),
            model_tallies: HashMap::new(),
            model_costs: HashMap::new(),
            model_unclassified_tokens: HashMap::new(),
        }
    }
}

/// A quarter of an hour's work. Days are what the card shows, but a five-hour limit opens and
/// closes inside one, so the totals are kept at a resolution fine enough to answer "since this
/// window opened".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Slot {
    pub start: DateTime<Utc>,
    pub tokens: i64,
    pub cost: f64,
    #[serde(default)]
    pub unpriced_tokens: i64,
    /// The quarter-hour's tokens by raw model id, where the reader kept them. Empty for a slot
    /// read before the detail was kept.
    #[serde(skip_serializing_if = "HashMap::is_empty", default)]
    pub models: HashMap<String, TokenTally>,
    /// The quarter-hour's timed replies by raw model id.
    #[serde(skip_serializing_if = "HashMap::is_empty", default)]
    pub timings: HashMap<String, ReplyTiming>,
}

impl Slot {
    pub fn new(start: DateTime<Utc>, tokens: i64, cost: f64) -> Self {
        Self { start, tokens, cost, unpriced_tokens: 0, models: HashMap::new(), timings: HashMap::new() }
    }
}

/// A reported calendar day's work, independent of whether its hour is known. Used for span
/// filtering, never as an hourly measurement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionDay {
    pub date: DateTime<Utc>,
    pub tokens: i64,
    pub cost: f64,
    #[serde(default)]
    pub unpriced_tokens: i64,
}

/// One transcript: one conversation with the CLI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    /// The file's path, which is unique and stable.
    pub id: String,
    /// What the CLI called it: a uuid for Claude Code, a timestamped rollout name for Codex.
    pub name: String,
    /// What the conversation was called: the title the user set, else the words it opened with.
    pub title: Option<String>,
    /// A session Codex ran itself to review another's planned action.
    #[serde(default)]
    pub is_review: bool,
    pub project: Option<UsageProject>,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub tokens: i64,
    pub cost: f64,
    #[serde(default)]
    pub unpriced_tokens: i64,
    /// The session's own quarter-hour buckets, priced: the same ones the day totals are folded
    /// from. Empty when timing is aggregate, or on an older ledger.
    #[serde(default)]
    pub slots: Vec<Slot>,
    /// Present for normalised records, including aggregate reports.
    #[serde(default)]
    pub days: Vec<SessionDay>,
}

impl Session {
    /// The money to show, or None where none of the tokens had a price.
    pub fn estimated_cost(&self) -> Option<f64> {
        if self.tokens > 0 && self.unpriced_tokens == self.tokens {
            None
        } else {
            Some(self.cost)
        }
    }
}

/// What a span's records say about the prompt cache.
#[derive(Debug, Clone, PartialEq)]
pub enum CacheReading {
    /// The input kinds the rate is measured over: cache reads, and every input token beside
    /// them, from the models that said anything about the cache.
    Measured(TokenTally),
    /// The store (or every model in it) records no cache. Its zero hits are not a cache that
    /// missed, so it has no part in a rate.
    Unrecorded,
    /// Counts that may be missing or cannot be sorted into their kinds: a rate over them would
    /// state part of the work as the whole.
    Unvouched,
}

/// One model's cache hit rate over a span, and how much input it is measured over.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCacheRate {
    pub name: String,
    pub rate: f64,
    pub input_tokens: i64,
}

/// One model's output speed over a span and its average wait for the first token.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSpeed {
    pub name: String,
    pub tokens_per_second: Option<f64>,
    pub first_token_seconds: Option<f64>,
    pub output_tokens: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelShare {
    pub name: String,
    pub tokens: i64,
    pub share: f64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SpanTotal {
    pub tokens: i64,
    pub cost: f64,
    pub unpriced: i64,
}

/// A provider's history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ledger {
    #[serde(default)]
    pub origin: Origin,
    /// What the money is counted in. None is dollars.
    #[serde(default)]
    pub currency: Option<String>,
    /// Some work has only session- or report-level timing, so the hour profile cannot be trusted.
    #[serde(default)]
    pub has_aggregate_timing: bool,
    /// Some of the counts behind this ledger may be missing. Never changes a token number or a
    /// price.
    #[serde(default)]
    pub has_partial_counts: bool,
    /// Whether the records say anything about the prompt cache. False for a store with no cache
    /// column.
    #[serde(default = "yes")]
    pub reports_cache_reads: bool,
    /// Ascending by date, gaps closed so a chart reads as a calendar.
    pub days: Vec<LedgerDay>,
    pub earliest: Option<DateTime<Utc>>,
    /// Models seen in the logs that models.dev has no price for.
    pub unpriced_models: Vec<String>,
    /// How each model id is written by its provider, where models.dev says.
    pub model_names: HashMap<String, String>,
    /// Ascending by start time. Only slots with work in them.
    pub slots: Vec<Slot>,
    /// One per conversation.
    #[serde(default)]
    pub sessions: Vec<Session>,
    /// When the ledger was read, for an "as of" line. Set by the readers' public entry points.
    #[serde(default)]
    pub read_at: Option<DateTime<Utc>>,
}

fn yes() -> bool {
    true
}

impl Default for Ledger {
    fn default() -> Self {
        Self::empty()
    }
}

impl Ledger {
    pub fn empty() -> Self {
        Self {
            origin: Origin::LocalTranscripts,
            currency: None,
            has_aggregate_timing: false,
            has_partial_counts: false,
            reports_cache_reads: true,
            days: Vec::new(),
            earliest: None,
            unpriced_models: Vec::new(),
            model_names: HashMap::new(),
            slots: Vec::new(),
            sessions: Vec::new(),
            read_at: None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.days.is_empty()
    }

    /// What has gone through since a moment, counting a slot that straddles the boundary in full.
    /// Not the figure a window's worth divides; see `cost_between`.
    pub fn spend_since(&self, start: DateTime<Utc>) -> (i64, f64) {
        self.slots.iter().filter(|s| s.start >= start).fold((0, 0.0), |(t, c), s| (t + s.tokens, c + s.cost))
    }

    /// What went through between two moments, with the quarter-hours at either edge counted for
    /// the share of them inside: the figure a window's worth divides. Work inside a
    /// quarter-hour is taken as even.
    pub fn cost_between(&self, start: DateTime<Utc>, end: DateTime<Utc>) -> f64 {
        self.share(start, end, |slot| slot.cost)
    }

    /// The tokens in the same span, the same way, priced or not.
    pub fn tokens_between(&self, start: DateTime<Utc>, end: DateTime<Utc>) -> f64 {
        self.share(start, end, |slot| slot.tokens as f64)
    }

    fn share(&self, start: DateTime<Utc>, end: DateTime<Utc>, value: impl Fn(&Slot) -> f64) -> f64 {
        const QUARTER: f64 = 15.0 * 60.0;
        self.slots.iter().fold(0.0, |total, slot| {
            let slot_end = slot.start + chrono::Duration::seconds(15 * 60);
            let overlap = (slot_end.min(end) - slot.start.max(start)).num_milliseconds() as f64 / 1000.0;
            if overlap > 0.0 {
                total + value(slot) * (overlap / QUARTER).min(1.0)
            } else {
                total
            }
        })
    }

    /// Today's row, when the last day is today.
    pub fn today_in(&self, now: DateTime<Utc>, calendar: &Calendar) -> Option<&LedgerDay> {
        let last = self.days.last()?;
        (calendar.date(last.date) == calendar.date(now)).then_some(last)
    }

    pub fn today(&self, now: DateTime<Utc>) -> Option<&LedgerDay> {
        self.today_in(now, &Calendar::local())
    }

    /// A span's tokens and money, and how many of the tokens had no price.
    pub fn total_in(&self, over_last: usize, now: DateTime<Utc>, calendar: &Calendar) -> SpanTotal {
        Self::fold(&self.recent_in(over_last, now, calendar))
    }

    pub fn total(&self, over_last: usize, now: DateTime<Utc>) -> SpanTotal {
        self.total_in(over_last, now, &Calendar::local())
    }

    pub fn all_time(&self) -> SpanTotal {
        Self::fold(&self.days.iter().collect::<Vec<_>>())
    }

    fn fold<D: std::borrow::Borrow<LedgerDay>>(days: &[D]) -> SpanTotal {
        days.iter().fold(SpanTotal::default(), |mut total, day| {
            let day = day.borrow();
            total.tokens += day.tokens;
            total.cost += day.cost;
            total.unpriced += day.unpriced_tokens;
            total
        })
    }

    /// The money to show for some work, or None where none of it had a price: an unpriced
    /// model's work cost something, and `$0.00` says it did not.
    pub fn shown_cost(cost: f64, tokens: i64, unpriced: i64) -> Option<f64> {
        if tokens > 0 && unpriced >= tokens {
            None
        } else {
            Some(cost)
        }
    }

    /// The last `count` calendar days, today included, oldest first.
    ///
    /// Not the last `count` entries: `days` runs from the first record to the last and stops
    /// there, so after a fortnight off "the last seven days" would have been the seven before
    /// the break. Days after the last record are filled in as quiet; days before the first are
    /// left off, so a week-old install does not draw a month of empty bars.
    pub fn recent_in(&self, count: usize, now: DateTime<Utc>, calendar: &Calendar) -> Vec<LedgerDay> {
        let today = calendar.start_of_day(now);
        let Some(first) = self.days.first() else { return Vec::new() };
        if count == 0 {
            return Vec::new();
        }
        let start = calendar.add_days(today, -(count as i64 - 1));
        let by_date: HashMap<DateTime<Utc>, &LedgerDay> = self
            .days
            .iter()
            .rev()
            .map(|d| (calendar.start_of_day(d.date), d))
            .collect();
        let mut cursor = calendar.start_of_day(start).max(calendar.start_of_day(first.date));
        let mut span = Vec::new();
        while cursor <= today {
            span.push(by_date.get(&cursor).map(|d| (*d).clone()).unwrap_or_else(|| LedgerDay::quiet(cursor)));
            // Start of day again: a midnight DST start would otherwise leave every later day at
            // 01:00, off its key.
            let next = calendar.start_of_day(calendar.add_days(cursor, 1));
            if next <= cursor {
                break;
            }
            cursor = next;
        }
        span
    }

    pub fn recent(&self, count: usize, now: DateTime<Utc>) -> Vec<LedgerDay> {
        self.recent_in(count, now, &Calendar::local())
    }

    /// How much of the input over a span was served from the prompt cache: cache reads over
    /// every input token (fresh, written to the cache, and read from it). Output is left out.
    ///
    /// Only where every token was sorted into its kind: a source that reports a bare total
    /// leaves tokens no kind can claim, and dividing around them would state a rate for part of
    /// the work as though it were the whole; so would a store that cannot prove its counts are
    /// complete. None then, and None with no input at all.
    pub fn cache_hit_rate_in(&self, over_last: usize, now: DateTime<Utc>, calendar: &Calendar) -> Option<f64> {
        let CacheReading::Measured(tally) = self.cache_reading(&self.recent_in(over_last, now, calendar)) else {
            return None;
        };
        let input = tally.input + tally.cache_write + tally.cache_read;
        (input > 0).then(|| tally.cache_read as f64 / input as f64)
    }

    pub fn cache_hit_rate(&self, over_last: usize, now: DateTime<Utc>) -> Option<f64> {
        self.cache_hit_rate_in(over_last, now, &Calendar::local())
    }

    /// The rule `cache_hit_rate` applies, kept apart so a rate over several ledgers adds up the
    /// same measured tallies instead of working the rule out a second way.
    pub fn cache_reading(&self, span: &[LedgerDay]) -> CacheReading {
        if !self.reports_cache_reads {
            return CacheReading::Unrecorded;
        }
        if self.has_partial_counts {
            return CacheReading::Unvouched;
        }
        let tally: TokenTally = span.iter().map(|d| d.tally.clone()).sum();
        let tokens: i64 = span.iter().map(|d| d.tokens).sum();
        if tally.total() != tokens {
            return CacheReading::Unvouched;
        }

        // A model that said nothing about the cache is left out of the rate, not counted as all
        // misses: its input would sit in the denominator with no hits possible above it. Where
        // every model is such a one, there is no rate. Days read before per-model tallies were
        // kept can only be judged whole.
        let mut by_model: HashMap<&str, TokenTally> = HashMap::new();
        let mut split = true;
        for day in span {
            if day.tokens > 0 && day.model_tallies.is_empty() {
                split = false;
            }
            for (raw, model) in &day.model_tallies {
                *by_model.entry(raw).or_default() += model;
            }
        }
        let measured: TokenTally = if split {
            by_model.into_values().filter(|t| !t.reports_no_cache()).sum()
        } else {
            tally
        };
        if measured.reports_no_cache() {
            return CacheReading::Unrecorded;
        }
        CacheReading::Measured(measured)
    }

    /// `cache_hit_rate` for each model, most input first. Grouped by display name. A model is
    /// left out when any of its tokens in the span cannot be vouched for by kind.
    pub fn cache_hit_rates_by_model_in(&self, over_last: usize, now: DateTime<Utc>, calendar: &Calendar) -> Vec<ModelCacheRate> {
        if self.has_partial_counts || !self.reports_cache_reads {
            return Vec::new();
        }
        let mut tallies: HashMap<String, TokenTally> = HashMap::new();
        let mut unvouched: BTreeSet<String> = BTreeSet::new();
        for day in self.recent_in(over_last, now, calendar) {
            for (raw, tokens) in &day.models {
                if *tokens <= 0 {
                    continue;
                }
                let name = self.model_names.get(raw).unwrap_or(raw).clone();
                match day.model_tallies.get(raw) {
                    Some(tally) if day.model_unclassified_tokens.get(raw).copied().unwrap_or(0) == 0 => {
                        *tallies.entry(name).or_default() += tally;
                    }
                    _ => {
                        unvouched.insert(name);
                    }
                }
            }
        }
        let mut rates: Vec<ModelCacheRate> = tallies
            .into_iter()
            .filter_map(|(name, tally)| {
                let input = tally.input + tally.cache_write + tally.cache_read;
                if unvouched.contains(&name) || tally.reports_no_cache() || input <= 0 {
                    return None;
                }
                Some(ModelCacheRate { rate: tally.cache_read as f64 / input as f64, input_tokens: input, name })
            })
            .collect();
        rates.sort_by(|a, b| b.input_tokens.cmp(&a.input_tokens).then_with(|| a.name.cmp(&b.name)));
        rates
    }

    pub const FEWEST_TIMED_REPLIES: i64 = 5;
    /// How far back a speed is read: the last day, rolling. A month's average says how the model
    /// was, not how it is.
    pub const SPEED_SPAN_SECONDS: i64 = 24 * 3600;

    /// `ReplyTiming` added up per model since a moment, most output first. All output over all
    /// time, not an average of each reply's speed. The first-token wait is the mean over turns.
    /// A quarter-hour straddling `start` is left out.
    pub fn output_speeds_by_model(&self, since: DateTime<Utc>) -> Vec<ModelSpeed> {
        let mut timings: HashMap<String, ReplyTiming> = HashMap::new();
        for slot in self.slots.iter().filter(|s| s.start >= since) {
            for (raw, timing) in &slot.timings {
                let name = self.model_names.get(raw).unwrap_or(raw).clone();
                let entry = timings.entry(name).or_default();
                *entry = *entry + *timing;
            }
        }
        let mut speeds: Vec<ModelSpeed> = timings
            .into_iter()
            .filter_map(|(name, timing)| {
                let speed = (timing.replies >= Self::FEWEST_TIMED_REPLIES && timing.seconds > 0.0)
                    .then(|| timing.output_tokens as f64 / timing.seconds);
                let first = (timing.first_token_turns >= Self::FEWEST_TIMED_REPLIES)
                    .then(|| timing.first_token_seconds / timing.first_token_turns as f64);
                if speed.is_none() && first.is_none() {
                    return None;
                }
                Some(ModelSpeed {
                    name,
                    tokens_per_second: speed,
                    first_token_seconds: first,
                    output_tokens: if speed.is_none() { 0 } else { timing.output_tokens },
                })
            })
            .collect();
        speeds.sort_by(|a, b| b.output_tokens.cmp(&a.output_tokens).then_with(|| a.name.cmp(&b.name)));
        speeds
    }

    /// Each model's share of the tokens over a span, largest first, grouped by display name.
    pub fn model_shares_in(&self, over_last: usize, now: DateTime<Utc>, calendar: &Calendar) -> Vec<ModelShare> {
        let mut totals: HashMap<String, i64> = HashMap::new();
        for day in self.recent_in(over_last, now, calendar) {
            for (raw, tokens) in &day.models {
                if *tokens > 0 {
                    *totals.entry(self.model_names.get(raw).unwrap_or(raw).clone()).or_default() += tokens;
                }
            }
        }
        let overall: i64 = totals.values().sum();
        if overall <= 0 {
            return Vec::new();
        }
        let mut shares: Vec<ModelShare> = totals
            .into_iter()
            .map(|(name, tokens)| ModelShare { name, tokens, share: tokens as f64 / overall as f64 })
            .collect();
        shares.sort_by(|a, b| b.tokens.cmp(&a.tokens).then_with(|| a.name.cmp(&b.name)));
        shares
    }

    /// The heaviest day in a span.
    pub fn busiest_day_in(&self, over_last: usize, now: DateTime<Utc>, calendar: &Calendar) -> Option<LedgerDay> {
        let mut best: Option<LedgerDay> = None;
        for day in self.recent_in(over_last, now, calendar) {
            if best.as_ref().is_none_or(|b| day.tokens > b.tokens) {
                best = Some(day);
            }
        }
        best
    }

    /// The model most of the work went through, and how much of it. Falls back to the whole
    /// history when the recent window is quiet, so the line doesn't vanish after a week off.
    pub fn top_model_in(&self, over_last: usize, now: DateTime<Utc>, calendar: &Calendar) -> Option<(String, f64)> {
        let recent = self.recent_in(over_last, now, calendar);
        let window: Vec<&LedgerDay> = if recent.iter().any(|d| d.tokens > 0) { recent.iter().collect() } else { self.days.iter().collect() };

        let mut totals: HashMap<&str, i64> = HashMap::new();
        for day in window {
            for (model, tokens) in &day.models {
                *totals.entry(model).or_default() += tokens;
            }
        }
        let overall: i64 = totals.values().sum();
        let (leader, tokens) = totals.iter().max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0))).map(|(k, v)| (*k, *v))?;
        if overall <= 0 {
            return None;
        }
        Some((self.model_names.get(leader).cloned().unwrap_or_else(|| leader.to_string()), tokens as f64 / overall as f64))
    }

    pub fn top_model(&self, over_last: usize, now: DateTime<Utc>) -> Option<(String, f64)> {
        self.top_model_in(over_last, now, &Calendar::local())
    }

    /// Several ledgers as one, day by day: tokens, money, token kinds and models per day, the
    /// models' names, and the quarter-hours the value estimate reads. The finer detail (sessions)
    /// is left behind.
    pub fn adding(ledgers: &[Ledger], calendar: &Calendar) -> Ledger {
        let present: Vec<&Ledger> = ledgers.iter().filter(|l| !l.days.is_empty()).collect();
        if present.len() <= 1 {
            return present.first().map(|l| (*l).clone()).unwrap_or_else(Ledger::empty);
        }
        let mut merged: HashMap<DateTime<Utc>, LedgerDay> = HashMap::new();
        for day in present.iter().flat_map(|l| l.days.iter()) {
            let entry = merged.entry(day.date).or_insert_with(|| LedgerDay::quiet(day.date));
            entry.tally += &day.tally;
            entry.tokens += day.tokens;
            entry.cost += day.cost;
            entry.unpriced_tokens += day.unpriced_tokens;
            for (model, tokens) in &day.models {
                *entry.models.entry(model.clone()).or_default() += tokens;
            }
        }
        let (Some(&first), Some(&last)) = (merged.keys().min(), merged.keys().max()) else { return Ledger::empty() };
        let mut days = Vec::new();
        let mut date = first;
        while date <= last {
            days.push(merged.remove(&date).unwrap_or_else(|| LedgerDay::quiet(date)));
            let next = calendar.add_days(date, 1);
            if next <= date {
                break;
            }
            date = next;
        }
        let mut slots: Vec<Slot> = present.iter().flat_map(|l| l.slots.iter().cloned()).collect();
        slots.sort_by_key(|s| s.start);
        let mut model_names: HashMap<String, String> = HashMap::new();
        for ledger in &present {
            for (raw, name) in &ledger.model_names {
                model_names.entry(raw.clone()).or_insert_with(|| name.clone());
            }
        }
        let mut unpriced: Vec<String> = present.iter().flat_map(|l| l.unpriced_models.iter().cloned()).collect::<BTreeSet<_>>().into_iter().collect();
        unpriced.sort();
        let mut ledger = Ledger {
            earliest: present.iter().filter_map(|l| l.earliest).min(),
            unpriced_models: unpriced,
            model_names,
            slots,
            days,
            ..Ledger::empty()
        };
        ledger.origin = if present.iter().any(|l| l.origin == Origin::ImportedRecords) { Origin::ImportedRecords } else { Origin::LocalTranscripts };
        // One agent that cannot vouch for its counts leaves the sum unvouched.
        ledger.has_partial_counts = present.iter().any(|l| l.has_partial_counts);
        ledger
    }
}

// MARK: - Slot keys

/// The quarter-hour a moment falls in, as the key the buckets are held under (`yyyy-MM-dd HH:mm`,
/// local). Shared with the readers that take their counts from a record store rather than a
/// transcript, so every agent's day is cut the same way.
pub fn slot_key(at: DateTime<Utc>, calendar: &Calendar) -> String {
    let quarter = 15 * 60;
    let floored = at.timestamp().div_euclid(quarter) * quarter;
    let moment = DateTime::<Utc>::from_timestamp(floored, 0).unwrap_or(at);
    calendar.to_local(moment).format("%Y-%m-%d %H:%M").to_string()
}

/// The instant a slot key names.
pub fn slot_start(key: &str, calendar: &Calendar) -> Option<DateTime<Utc>> {
    NaiveDateTime::parse_from_str(key, "%Y-%m-%d %H:%M").ok().map(|n| calendar.from_local(n))
}

fn slot_date(key: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(key.get(..10)?, "%Y-%m-%d").ok()
}

/// Turns a reader's running `slot_key` totals into the sorted, priced buckets a session carries.
pub fn session_slots(totals: &HashMap<String, (i64, f64, i64)>, calendar: &Calendar) -> Vec<Slot> {
    let mut slots: Vec<Slot> = totals
        .iter()
        .filter_map(|(key, (tokens, cost, unpriced))| {
            slot_start(key, calendar).map(|start| Slot { unpriced_tokens: *unpriced, ..Slot::new(start, *tokens, *cost) })
        })
        .collect();
    slots.sort_by_key(|s| s.start);
    slots
}

/// Turns buckets into days, slots and money.
///
/// Free of reader state, so the readers that take their counts from a record store can price
/// them exactly as the transcripts are priced. Two ways of turning tokens into dollars in one
/// app is two figures that eventually disagree.
pub fn price_buckets(
    buckets: &Buckets,
    prices: &PriceTable,
    calendar: &Calendar,
    vendor: Option<&str>,
    timings: &HashMap<String, HashMap<String, ReplyTiming>>,
) -> Ledger {
    if buckets.is_empty() {
        return Ledger::empty();
    }

    let mut unpriced: BTreeSet<String> = BTreeSet::new();
    let mut names: HashMap<String, String> = HashMap::new();
    let mut slots: Vec<Slot> = Vec::with_capacity(buckets.len());
    let mut by_date: HashMap<DateTime<Utc>, LedgerDay> = HashMap::new();
    let mut midnights: HashMap<NaiveDate, DateTime<Utc>> = HashMap::new();
    let mut lookup = ModelPriceLookup::new(prices);

    for (key, models) in buckets {
        let Some(start) = slot_start(key, calendar) else { continue };
        let day_date = match slot_date(key) {
            Some(date) => *midnights.entry(date).or_insert_with(|| calendar.midnight(date)),
            None => calendar.start_of_day(start),
        };
        let day = by_date.entry(day_date).or_insert_with(|| LedgerDay::quiet(day_date));

        let mut tokens = 0;
        let mut cost = 0.0;
        let mut unpriced_tokens = 0;

        for (model, tally) in models {
            let total = tally.total();
            tokens += total;
            *day.models.entry(model.clone()).or_default() += total;
            // Per model, so a drill-down can show one model's own split rather than the whole
            // day's.
            *day.model_tallies.entry(model.clone()).or_default() += tally;
            day.tally += tally;

            if let Some(price) = lookup.price(model, vendor) {
                let money = tally.cost_breakdown(price);
                cost += money.total();
                *day.model_costs.entry(model.clone()).or_default() += money;
                if let Some(name) = &price.name {
                    names.insert(model.clone(), name.clone());
                }
            } else {
                unpriced.insert(model.clone());
                unpriced_tokens += total;
            }
        }

        slots.push(Slot {
            start,
            tokens,
            cost,
            unpriced_tokens,
            models: models.clone(),
            timings: timings.get(key).cloned().unwrap_or_default(),
        });
        day.tokens += tokens;
        day.cost += cost;
        day.unpriced_tokens += unpriced_tokens;
    }

    let (Some(&earliest), Some(&latest)) = (by_date.keys().min(), by_date.keys().max()) else { return Ledger::empty() };

    // Fill the quiet days back in. Without them the bars would sit shoulder to shoulder and a
    // fortnight off would look like a weekend.
    let mut days = Vec::new();
    let mut cursor = earliest;
    while cursor <= latest {
        days.push(by_date.remove(&cursor).unwrap_or_else(|| LedgerDay::quiet(cursor)));
        let next = calendar.start_of_day(calendar.add_days(cursor, 1));
        if next <= cursor {
            break;
        }
        cursor = next;
    }

    slots.sort_by_key(|s| s.start);
    Ledger {
        days,
        earliest: Some(earliest),
        unpriced_models: unpriced.into_iter().collect(),
        model_names: names,
        slots,
        ..Ledger::empty()
    }
}
