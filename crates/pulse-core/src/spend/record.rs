// Ported from upstream Sources/Pulse/Usage/AgentUsageRecord.swift.
//! The normalised record every non-transcript reader will hand over, and the builder that turns
//! records into the same `Ledger` every other reader makes.
//!
//! One pricing path, not another: the classified kinds are bucketed with the shared `slot_key`
//! and priced by `price_buckets`, and a session's cost and quarter-hours go through
//! `TokenTally::cost_at` and `session_slots` exactly as a transcript's do.

use std::collections::{BTreeSet, HashMap, HashSet};

use chrono::{DateTime, Utc};

use super::calendar::Calendar;
use super::ledger::{price_buckets, session_slots, slot_key, slot_start, Buckets, Ledger, LedgerDay, Origin, Session, SessionDay, Slot};
use super::prices::{ModelPriceLookup, PriceTable};
use super::project::UsageProject;
use super::tally::TokenTally;

/// One incremental piece of work an agent's store recorded.
///
/// Incremental, not cumulative: a parser that reads a running total or a snapshot differences it
/// against the previous reading before handing records over; the builder adds every record up
/// as written and does no product-specific arithmetic.
///
/// A record's tokens have two parts. `tally` is the work split into the four kinds a price list
/// can bill. `unclassified_tokens` is real work the source did not break down (a bare total, or
/// a session total), which is counted and never priced; it is never put into the input bucket,
/// because that would report fresh input nobody measured.
#[derive(Debug, Clone, Default)]
pub struct AgentUsageRecord {
    pub timestamp: DateTime<Utc>,
    pub model: String,
    pub tally: TokenTally,
    pub session_id: Option<String>,
    pub session_name: Option<String>,
    pub title: Option<String>,
    pub project: Option<String>,
    /// The product's own identity for a message, used to fold one message that two roots both
    /// contain down to one.
    pub deduplication_id: Option<String>,
    /// Tokens the caller could not place in any of the four kinds: only the remainder after
    /// subtracting every kind it could identify. A negative value, here or in any kind, makes
    /// the record broken data and the builder skips the record rather than clamping it.
    pub unclassified_tokens: i64,
    /// True when only session- or report-level timing is known: the record lands on its real
    /// calendar day but never in a quarter-hour bucket.
    pub is_aggregate: bool,
    /// True when the source could not prove the report was complete. Never used to invent a
    /// remainder, and changes no price.
    pub is_partial: bool,
}

impl AgentUsageRecord {
    pub fn new(timestamp: DateTime<Utc>, model: &str, tally: TokenTally) -> Self {
        Self { timestamp, model: model.to_string(), tally, ..Self::default() }
    }

    pub fn session(mut self, id: &str) -> Self {
        self.session_id = Some(id.to_string());
        self
    }

    pub fn project(mut self, project: &str) -> Self {
        self.project = Some(project.to_string());
        self
    }

    pub fn unclassified(mut self, tokens: i64) -> Self {
        self.unclassified_tokens = tokens;
        self
    }

    pub fn aggregate(mut self, is_aggregate: bool) -> Self {
        self.is_aggregate = is_aggregate;
        self
    }
}

/// A string that is only whitespace is absent, not a shared key. A non-blank value is returned
/// unchanged, so an explicit identity still folds exactly as written.
fn non_blank(value: &Option<String>) -> Option<&str> {
    value.as_deref().filter(|v| !v.trim().is_empty())
}

/// The checked sum of a record's counts, or None when any is negative or the total would
/// overflow. The only place a count is checked: once a record is accepted its total is part of a
/// running total bounded by `i64::MAX`.
fn checked_total(values: &[i64]) -> Option<i64> {
    let mut total: i64 = 0;
    for &value in values {
        if value < 0 {
            return None;
        }
        total = total.checked_add(value)?;
    }
    Some(total)
}

#[derive(Default)]
struct Running {
    tokens: i64,
    cost: f64,
    unpriced: i64,
    start: Option<DateTime<Utc>>,
    end: Option<DateTime<Utc>>,
    name: Option<String>,
    title: Option<String>,
    project: Option<UsageProject>,
    has_aggregate: bool,
    slots: HashMap<String, (i64, f64, i64)>,
    days: HashMap<DateTime<Utc>, (i64, f64, i64)>,
}

/// Adds records up over a namespace.
///
/// `namespace` prefixes every session id so the same session id written by two agents cannot
/// collide. Records with an explicit `deduplication_id` are counted once globally, which folds a
/// message present in more than one of an agent's roots; a record with none is kept every time.
/// Records are never deduplicated by equal timestamp, model or tally: two identical requests are
/// two requests.
///
/// A record's total is its four kinds plus `unclassified_tokens`, added with checked
/// arithmetic, and it is kept when that is positive. A record whose kinds or remainder are
/// negative, or whose total does not fit, is broken and is skipped whole. Unclassified tokens
/// count in the day, the model, the session and the unpriced figure, and are listed per raw id so
/// a summary can tell them from a broken split; they are never turned into a kind and never
/// costed. An empty model is skipped, and a record with no `session_id` creates no session row.
pub fn build_ledger(
    records: &[AgentUsageRecord],
    prices: &PriceTable,
    namespace: &str,
    vendor: Option<&str>,
    calendar: &Calendar,
    origin: Origin,
) -> Ledger {
    // Classified kinds of every record: what the shared pricing pass turns into days, names and
    // money.
    let mut known_buckets = Buckets::new();
    // Classified kinds of non-aggregate records only: the only ones that may become slots.
    let mut event_buckets = Buckets::new();
    let mut event_unknown: HashMap<String, HashMap<String, i64>> = HashMap::new();
    let mut day_unknown: HashMap<DateTime<Utc>, HashMap<String, i64>> = HashMap::new();

    let mut seen: HashSet<&str> = HashSet::new();
    let mut sessions: HashMap<String, Running> = HashMap::new();
    let mut has_aggregate_timing = false;
    // Set only by an accepted record, so a record that was skipped whole cannot make the ledger
    // look partial.
    let mut has_partial_counts = false;
    let mut accepted_models: BTreeSet<&str> = BTreeSet::new();
    // The running total of every accepted record. Checked, so a hostile count cannot carry it
    // past the maximum.
    let mut accepted_total: i64 = 0;
    let mut lookup = ModelPriceLookup::new(prices);
    // Many replies share a quarter-hour. Resolving its local slot and midnight for every
    // reply repeatedly calls the Windows time-zone APIs during large OpenCode scans.
    // Cache by the UTC quarter that slot_key rounds by, preserving its DST resolution.
    let mut quarters: HashMap<i64, (String, DateTime<Utc>)> = HashMap::new();

    for record in records {
        if record.model.trim().is_empty() {
            continue;
        }
        let model = record.model.as_str();

        // Every count is a real, non-negative one that fits. A negative kind, a negative
        // remainder or an overflowing record total is broken data: the record is skipped whole
        // and the rest of the run is kept.
        let tally = &record.tally;
        let Some(known) = checked_total(&[tally.input, tally.cache_write, tally.cache_read, tally.output]) else { continue };
        let Some(total) = checked_total(&[known, record.unclassified_tokens]) else { continue };
        if total <= 0 {
            continue;
        }

        // Only an explicit identity folds a message; everything else is a separate request even
        // when it looks identical. A blank id is no identity.
        if let Some(id) = non_blank(&record.deduplication_id) {
            if !seen.insert(id) {
                continue;
            }
        }

        // A record that would push the ledger's own total past what an integer can hold is not
        // added, so no later group sum can overflow.
        let Some(running_total) = checked_total(&[accepted_total, total]) else { continue };
        accepted_total = running_total;
        accepted_models.insert(model);

        let extra = record.unclassified_tokens;
        let quarter = record.timestamp.timestamp().div_euclid(15 * 60);
        let (key, day) = match quarters.entry(quarter) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                let key = slot_key(record.timestamp, calendar);
                let Some(start) = slot_start(&key, calendar) else { continue };
                entry.insert((key, calendar.start_of_day(start)))
            }
        };
        let key = key.clone();
        let day = *day;
        if known > 0 {
            add_tally(&mut known_buckets, &key, model, tally);
            if !record.is_aggregate {
                add_tally(&mut event_buckets, &key, model, tally);
            }
        }
        if extra > 0 {
            *day_unknown.entry(day).or_default().entry(model.to_string()).or_default() += extra;
            if !record.is_aggregate {
                *event_unknown.entry(key.clone()).or_default().entry(model.to_string()).or_default() += extra;
            }
        }

        if record.is_aggregate {
            has_aggregate_timing = true;
        }
        if record.is_partial {
            has_partial_counts = true;
        }

        // A blank session id names no session, but the record's tokens still count toward the
        // totals and the models.
        let Some(session_id) = non_blank(&record.session_id) else { continue };

        let price = lookup.price(model, vendor);
        let money = price.map_or(0.0, |p| record.tally.cost_at(p));
        let unpriced = extra + if price.is_none() { known } else { 0 };
        let name = non_blank(&record.session_name).map(str::to_string);
        let title = non_blank(&record.title).map(str::to_string);
        let project = UsageProject::new(record.project.as_deref());

        let running = sessions.entry(session_id.to_string()).or_default();
        running.tokens += total;
        running.cost += money;
        running.unpriced += unpriced;
        running.start = Some(running.start.map_or(record.timestamp, |s| s.min(record.timestamp)));
        running.end = Some(running.end.map_or(record.timestamp, |e| e.max(record.timestamp)));
        if running.name.is_none() {
            running.name = name;
        }
        if running.title.is_none() {
            running.title = title;
        }
        if running.project.is_none() {
            running.project = project;
        }
        let by_day = running.days.entry(day).or_insert((0, 0.0, 0));
        by_day.0 += total;
        by_day.1 += money;
        by_day.2 += unpriced;
        if record.is_aggregate {
            running.has_aggregate = true;
        } else {
            let slot = running.slots.entry(key).or_insert((0, 0.0, 0));
            slot.0 += total;
            slot.1 += money;
            slot.2 += unpriced;
        }
    }

    // Nothing written at all is empty, not a zero-shaped ledger. Unclassified-only records still
    // count, so both maps are checked.
    if known_buckets.is_empty() && day_unknown.is_empty() {
        return Ledger::empty();
    }

    let mut ledger = price_buckets(&known_buckets, prices, calendar, vendor, &HashMap::new());

    // A raw id seen only as unclassified tokens still has a published name. It never reaches the
    // pricing pass, so without this its display name is lost and the same model splits into two
    // rows. Looking up the name is not pricing it: an unknown-only id keeps no money, and an id
    // with no rate at all is named as unpriced.
    let mut unpriced: BTreeSet<String> = ledger.unpriced_models.iter().cloned().collect();
    for model in &accepted_models {
        match lookup.price(model, vendor) {
            Some(price) => {
                if let Some(name) = &price.name {
                    ledger.model_names.insert((*model).to_string(), name.clone());
                }
            }
            None => {
                unpriced.insert((*model).to_string());
            }
        }
    }
    ledger.unpriced_models = unpriced.into_iter().collect();

    // Slots: events only, with their unclassified tokens added to the bucket they landed in,
    // never to a model's known tally, so a model's hours stop reconciling and go nil rather than
    // borrowing the count.
    let mut slot_by_start: HashMap<DateTime<Utc>, Slot> = HashMap::new();
    for slot in price_buckets(&event_buckets, prices, calendar, vendor, &HashMap::new()).slots {
        slot_by_start.insert(slot.start, slot);
    }
    for (key, models) in &event_unknown {
        let Some(start) = slot_start(key, calendar) else { continue };
        let extra: i64 = models.values().sum();
        let slot = slot_by_start.entry(start).or_insert_with(|| Slot::new(start, 0, 0.0));
        slot.tokens += extra;
        slot.unpriced_tokens += extra;
    }
    let mut slots: Vec<Slot> = slot_by_start.into_values().collect();
    slots.sort_by_key(|s| s.start);
    ledger.slots = slots;

    // Days: the classified rollup, then unclassified tokens layered on. A day that only has
    // unclassified tokens is created rather than dropped.
    let mut day_by_date: HashMap<DateTime<Utc>, LedgerDay> = ledger.days.drain(..).map(|d| (d.date, d)).collect();
    for (date, models) in &day_unknown {
        let base = day_by_date.entry(*date).or_insert_with(|| LedgerDay::quiet(*date));
        for (model, count) in models {
            *base.models.entry(model.clone()).or_default() += count;
            *base.model_unclassified_tokens.entry(model.clone()).or_default() += count;
            base.tokens += count;
            base.unpriced_tokens += count;
        }
    }

    let (Some(&first), Some(&last)) = (day_by_date.keys().min(), day_by_date.keys().max()) else { return Ledger::empty() };
    let mut days = Vec::new();
    let mut cursor = first;
    while cursor <= last {
        days.push(day_by_date.remove(&cursor).unwrap_or_else(|| LedgerDay::quiet(cursor)));
        let next = calendar.add_days(cursor, 1);
        if next <= cursor {
            break;
        }
        cursor = next;
    }

    ledger.days = days;
    ledger.earliest = Some(first);
    ledger.has_aggregate_timing = has_aggregate_timing;
    ledger.has_partial_counts = has_partial_counts;
    ledger.origin = origin;
    let mut rows: Vec<Session> = sessions
        .into_iter()
        .filter_map(|(id, running)| {
            let (start, end) = (running.start?, running.end?);
            let mut session_days: Vec<SessionDay> = running
                .days
                .iter()
                .map(|(date, (tokens, cost, unpriced))| SessionDay { date: *date, tokens: *tokens, cost: *cost, unpriced_tokens: *unpriced })
                .collect();
            session_days.sort_by_key(|d| d.date);
            Some(Session {
                // Prefixed, so the same id two agents both wrote is still two sessions.
                id: format!("{namespace}#{id}"),
                // The id is an identity, not invented metadata.
                name: running.name.unwrap_or(id),
                title: running.title,
                is_review: false,
                project: running.project,
                start,
                end,
                tokens: running.tokens,
                cost: running.cost,
                unpriced_tokens: running.unpriced,
                // Aggregate timing withholds hours, not known dates. Day buckets keep a resumed
                // session inside the selected span.
                slots: if running.has_aggregate { Vec::new() } else { session_slots(&running.slots, calendar) },
                days: session_days,
            })
        })
        .collect();
    rows.sort_by(|a, b| b.end.cmp(&a.end).then_with(|| a.id.cmp(&b.id)));
    ledger.sessions = rows;
    ledger
}

fn add_tally(buckets: &mut Buckets, key: &str, model: &str, tally: &TokenTally) {
    *buckets.entry(key.to_string()).or_default().entry(model.to_string()).or_default() += tally;
}
