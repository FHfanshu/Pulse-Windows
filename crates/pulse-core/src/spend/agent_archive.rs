// Ported from upstream Sources/Pulse/Usage/AgentArchive.swift.
//! The most of each quarter-hour and day Pulse has seen an agent's store hold, so a store that
//! deletes its old records does not take them out of Token spend and the recaps.
//!
//! **Why a high-water mark and not a list of files.** The transcript archive keeps Claude Code's
//! and Codex's transcripts file by file, because their reader counts file by file. The other
//! agents are read from whole stores into one ledger, half of them databases, and nothing says
//! which file a figure came from. What can be said is that a past quarter-hour's work only ever
//! grows while the readers are the same: a record appended, a session written. When a later
//! read holds less, the store has lost records, and the difference is what Pulse keeps. So every
//! stable read raises the marks (per quarter-hour and raw model, per day for what has no
//! quarter-hour, kind by kind) and the ledger shown is the live one plus whatever the marks hold
//! beyond it.
//!
//! **A copy is not counted twice.** A message the store holds in two places is folded to one by
//! the reader, so deleting either leaves the quarter-hour where it was and nothing is added.
//!
//! **What it gets wrong, and which way.** Work added to a past quarter-hour after other work
//! there was deleted (an import of old records, say) is hidden under the mark until the live
//! figure passes it: short, never double. A record whose time a store rewrites to another
//! quarter-hour is counted at both. A session still in the store with part of its work gone
//! keeps its row as read; the lost part is in the days but not in the row.
//!
//! **Only a stable read raises the marks**: a store half-written while it was read, or one the
//! reader could decode only in part, shows the marks over it and changes none of them.
//!
//! **The marks are only comparable under the same readers.** `readings` is the
//! `agent_cache::VERSION` they were taken under; after a bump, marks from before it stand only for
//! a day the new readers see nothing at all on.
//!
//! **Quarter-hours are kept by instant, days by the zone they were cut in.** A quarter-hour's key
//! is its UTC quarter index, so a change of time zone moves nothing. Upstream keys a day by its
//! local date and the zone's name; here a day is keyed by the instant of its local noon, with the
//! zone's offsets (`Calendar::zone_signature`) beside it. After a change of zone each day mark
//! moves to the new zone's day holding that noon, and stands only where the live ledger has
//! nothing on that day or either neighbour.
//!
//! **Its own file, unnumbered** (`archive-agent-<agent>.json`), never superseded with the
//! `agent-<n>-*` cache: that is rebuilt from the stores, this from nothing.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

use super::agent::SpendAgent;
use super::agent_cache;
use super::archive::write_durably;
use super::calendar::Calendar;
use super::ledger::{price_buckets, slot_key, Buckets, Ledger, LedgerDay, Origin, Session, Slot};
use super::prices::{ModelPriceLookup, PriceTable};
use super::tally::TokenTally;

pub const CURRENT_FORMAT: u32 = 1;

const QUARTER: i64 = 15 * 60;

/// Every field optional on the way in, so a field added later does not make the whole archive
/// unreadable, which would drop the history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentArchive {
    pub format: u32,
    /// The `agent_cache::VERSION` the marks were taken under.
    pub readings: u32,
    /// The zone the day marks were cut in (`Calendar::zone_signature`); empty before any.
    pub time_zone: String,
    /// Classified tokens by quarter-hour (its UTC quarter index since 1970) and then raw model.
    pub slots: HashMap<String, HashMap<String, TokenTally>>,
    /// Each quarter-hour's unclassified tokens, which a slot keeps without a model.
    pub slot_extra: HashMap<String, i64>,
    /// Classified tokens of a day and model that sit in no quarter-hour (aggregate records), by
    /// the instant of the day's local noon in Unix seconds.
    pub day_rest: HashMap<String, HashMap<String, TokenTally>>,
    /// Unclassified tokens by day (as `day_rest`) and raw model id.
    pub day_unclassified: HashMap<String, HashMap<String, i64>>,
    pub has_aggregate_timing: bool,
    pub has_partial_counts: bool,
    pub origin: Option<Origin>,
    /// Sessions a stable read had and a later one did not, as they last read. Their money is
    /// kept as it was priced then.
    pub sessions: HashMap<String, Session>,
}

impl Default for AgentArchive {
    fn default() -> Self {
        Self {
            format: CURRENT_FORMAT,
            readings: agent_cache::VERSION,
            time_zone: String::new(),
            slots: HashMap::new(),
            slot_extra: HashMap::new(),
            day_rest: HashMap::new(),
            day_unclassified: HashMap::new(),
            has_aggregate_timing: false,
            has_partial_counts: false,
            origin: None,
            sessions: HashMap::new(),
        }
    }
}

/// The marks, or one ledger, in working form: quarter-hours by index, days by local date in the
/// calendar at hand.
#[derive(Debug, Clone, Default, PartialEq)]
struct Marks {
    slots: HashMap<i64, HashMap<String, TokenTally>>,
    slot_extra: HashMap<i64, i64>,
    day_rest: HashMap<NaiveDate, HashMap<String, TokenTally>>,
    day_unclassified: HashMap<NaiveDate, HashMap<String, i64>>,
    has_aggregate_timing: bool,
    has_partial_counts: bool,
    origin: Option<Origin>,
}

impl AgentArchive {
    /// Whether an agent's history is kept this way. **Not Devin Desktop**: its records are counted
    /// only where they do not mirror Devin CLI's databases, so rows the CLI deletes would come back
    /// under Desktop while the CLI's marks still held them: counted twice.
    pub fn keeps(agent: SpendAgent) -> bool {
        agent.raw() != "devinDesktop"
    }

    pub fn file_name(agent: SpendAgent) -> String {
        format!("archive-agent-{}.json", agent.raw())
    }

    /// The archive, an empty one when there is none yet, or **None when a file is there and
    /// cannot be read** (damaged, or written by a later build), which must then be neither used
    /// nor overwritten.
    pub fn load(agent: SpendAgent, directory: &Path) -> Option<AgentArchive> {
        let path = directory.join(Self::file_name(agent));
        if !path.exists() {
            return Some(AgentArchive::default());
        }
        let archive: AgentArchive = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
        (archive.format <= CURRENT_FORMAT).then_some(archive)
    }

    pub fn save(&self, agent: SpendAgent, directory: &Path) -> bool {
        let Ok(bytes) = serde_json::to_vec(self) else { return false };
        write_durably(&directory.join(Self::file_name(agent)), &bytes)
    }

    // MARK: - Raising the marks

    /// Raises the marks to a stable read, and to the stable read before it, whose sessions
    /// missing now are kept. Returns whether anything moved.
    pub fn absorb(&mut self, live: &Ledger, previous: Option<&Ledger>, calendar: &Calendar) -> bool {
        let now = Marks::of(live, calendar);
        let mut next = self.aligned(&now.days(calendar), calendar);
        if let Some(previous) = previous {
            next.raise(&Marks::of(previous, calendar));
        }
        next.raise(&now);
        if !live.days.is_empty() {
            next.origin = Some(live.origin);
            next.has_aggregate_timing |= live.has_aggregate_timing;
            next.has_partial_counts |= live.has_partial_counts;
        }

        let mut sessions = self.sessions.clone();
        let live_ids: HashSet<&str> = live.sessions.iter().map(|s| s.id.as_str()).collect();
        for session in previous.map(|p| p.sessions.as_slice()).unwrap_or_default() {
            if !live_ids.contains(session.id.as_str()) {
                sessions.insert(session.id.clone(), session.clone());
            }
        }
        sessions.retain(|id, _| !live_ids.contains(id.as_str()));

        let stored = Self::stored(&next, sessions, calendar);
        if stored == *self {
            return false;
        }
        *self = stored;
        true
    }

    /// The marks made comparable with a live ledger read now: day marks moved into this zone, and
    /// marks from other readers cut to the days the live ledger has nothing on. Applied before
    /// raising **and** before showing, so a read too unsettled to raise anything shows no stale
    /// mark either.
    fn aligned(&self, seen: &HashSet<NaiveDate>, calendar: &Calendar) -> Marks {
        let moved = !self.time_zone.is_empty() && self.time_zone != calendar.zone_signature();
        let quiet = |day: NaiveDate| {
            [-1, 0, 1].iter().all(|offset| day.checked_add_signed(chrono::Duration::days(*offset)).is_none_or(|d| !seen.contains(&d)))
        };
        let day_of = |key: &str| -> Option<NaiveDate> {
            let noon = DateTime::<Utc>::from_timestamp(key.parse().ok()?, 0)?;
            let day = calendar.date(noon);
            (!moved || quiet(day)).then_some(day)
        };

        let mut marks = Marks {
            has_aggregate_timing: self.has_aggregate_timing,
            has_partial_counts: self.has_partial_counts,
            origin: self.origin,
            ..Marks::default()
        };
        for (key, models) in &self.slots {
            if let Ok(index) = key.parse::<i64>() {
                marks.slots.insert(index, models.clone());
            }
        }
        for (key, extra) in &self.slot_extra {
            if let Ok(index) = key.parse::<i64>() {
                marks.slot_extra.insert(index, *extra);
            }
        }
        for (key, models) in &self.day_rest {
            let Some(day) = day_of(key) else { continue };
            let into = marks.day_rest.entry(day).or_default();
            for (model, tally) in models {
                let high = into.get(model).map_or_else(|| tally.clone(), |kept| kept.highest(tally));
                into.insert(model.clone(), high);
            }
        }
        for (key, models) in &self.day_unclassified {
            let Some(day) = day_of(key) else { continue };
            let into = marks.day_unclassified.entry(day).or_default();
            for (model, count) in models {
                let high = into.get(model).map_or(*count, |kept| (*kept).max(*count));
                into.insert(model.clone(), high);
            }
        }
        if self.readings != agent_cache::VERSION {
            let unseen = |index: &i64| !seen.contains(&calendar.date(start_of(*index)));
            marks.slots.retain(|index, _| unseen(index));
            marks.slot_extra.retain(|index, _| unseen(index));
            marks.day_rest.retain(|day, _| !seen.contains(day));
            marks.day_unclassified.retain(|day, _| !seen.contains(day));
            // What the old readers flagged is not the new ones' to answer for.
            marks.has_aggregate_timing = false;
            marks.has_partial_counts = false;
        }
        marks
    }

    fn stored(marks: &Marks, sessions: HashMap<String, Session>, calendar: &Calendar) -> AgentArchive {
        let noon = |day: &NaiveDate| {
            calendar.from_local(day.and_hms_opt(12, 0, 0).expect("noon")).timestamp().to_string()
        };
        AgentArchive {
            format: CURRENT_FORMAT,
            readings: agent_cache::VERSION,
            time_zone: calendar.zone_signature(),
            slots: marks.slots.iter().map(|(k, v)| (k.to_string(), v.clone())).collect(),
            slot_extra: marks.slot_extra.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
            day_rest: marks.day_rest.iter().map(|(d, v)| (noon(d), v.clone())).collect(),
            day_unclassified: marks.day_unclassified.iter().map(|(d, v)| (noon(d), v.clone())).collect(),
            has_aggregate_timing: marks.has_aggregate_timing,
            has_partial_counts: marks.has_partial_counts,
            origin: marks.origin,
            sessions,
        }
    }

    // MARK: - Showing what was kept

    /// The live ledger with what the marks hold beyond it added in (priced now, at today's
    /// rates) and the kept sessions that the added work can account for. The live ledger itself
    /// when the marks hold nothing more.
    pub fn merged(&self, live: &Ledger, prices: &PriceTable, vendor: Option<&str>, calendar: &Calendar) -> Ledger {
        let now = Marks::of(live, calendar);
        let marks = self.aligned(&now.days(calendar), calendar);

        let mut events = Buckets::new();
        for (index, models) in &marks.slots {
            for (model, high) in models {
                let more = high.beyond(now.slots.get(index).and_then(|m| m.get(model)).unwrap_or(&TokenTally::default()));
                if more.total() > 0 {
                    events.entry(slot_key(start_of(*index), calendar)).or_default().insert(model.clone(), more);
                }
            }
        }
        let mut extras: HashMap<DateTime<Utc>, i64> = HashMap::new();
        for (index, high) in &marks.slot_extra {
            let live_extra = now.slot_extra.get(index).copied().unwrap_or(0);
            if *high > live_extra {
                extras.insert(start_of(*index), high - live_extra);
            }
        }
        // Priced as quarter-hours at the day's midnight, so the shared pricing pass places them on
        // their day; their slots are not used.
        let mut rest = Buckets::new();
        for (day, models) in &marks.day_rest {
            for (model, high) in models {
                let more = high.beyond(now.day_rest.get(day).and_then(|m| m.get(model)).unwrap_or(&TokenTally::default()));
                if more.total() > 0 {
                    rest.entry(format!("{} 00:00", day.format("%Y-%m-%d"))).or_default().insert(model.clone(), more);
                }
            }
        }
        let mut unclassified: HashMap<NaiveDate, HashMap<String, i64>> = HashMap::new();
        for (day, models) in &marks.day_unclassified {
            for (model, high) in models {
                let live_count = now.day_unclassified.get(day).and_then(|m| m.get(model)).copied().unwrap_or(0);
                if *high > live_count {
                    unclassified.entry(*day).or_default().insert(model.clone(), high - live_count);
                }
            }
        }
        if events.is_empty() && extras.is_empty() && rest.is_empty() && unclassified.is_empty() {
            return live.clone();
        }

        let no_timings = HashMap::new();
        let priced_events = price_buckets(&events, prices, calendar, vendor, &no_timings);
        let priced_rest = price_buckets(&rest, prices, calendar, vendor, &no_timings);

        let mut days: HashMap<DateTime<Utc>, LedgerDay> = live.days.iter().map(|d| (d.date, d.clone())).collect();
        let mut room: HashMap<DateTime<Utc>, i64> = HashMap::new();
        for day in priced_events.days.iter().chain(&priced_rest.days).filter(|d| d.tokens > 0) {
            let date = calendar.start_of_day(day.date);
            let combined = combine_day(days.remove(&date), day);
            days.insert(date, combined);
            *room.entry(date).or_default() += day.tokens;
        }
        let mut names = live.model_names.clone();
        for (raw, name) in priced_events.model_names.iter().chain(&priced_rest.model_names) {
            names.entry(raw.clone()).or_insert_with(|| name.clone());
        }
        let mut unpriced: BTreeSet<String> = live.unpriced_models.iter().cloned().collect();
        unpriced.extend(priced_events.unpriced_models.iter().cloned());
        unpriced.extend(priced_rest.unpriced_models.iter().cloned());
        let mut lookup = ModelPriceLookup::new(prices);
        for (day, models) in &unclassified {
            let date = calendar.midnight(*day);
            let count: i64 = models.values().sum();
            let mut added = LedgerDay::new(date, count, 0.0, count, models.clone());
            added.model_unclassified_tokens = models.clone();
            let combined = combine_day(days.remove(&date), &added);
            days.insert(date, combined);
            *room.entry(date).or_default() += count;
            // Named where the price list names it; never priced.
            for model in models.keys() {
                match lookup.price(model, vendor) {
                    Some(price) => {
                        if let Some(name) = &price.name {
                            names.entry(model.clone()).or_insert_with(|| name.clone());
                        }
                    }
                    None => {
                        unpriced.insert(model.clone());
                    }
                }
            }
        }

        let mut slots: HashMap<DateTime<Utc>, Slot> = live.slots.iter().map(|s| (s.start, s.clone())).collect();
        for slot in &priced_events.slots {
            let combined = combine_slot(slots.remove(&slot.start), slot);
            slots.insert(slot.start, combined);
        }
        for (start, extra) in extras {
            let added = Slot { unpriced_tokens: extra, ..Slot::new(start, extra, 0.0) };
            let combined = combine_slot(slots.remove(&start), &added);
            slots.insert(start, combined);
        }

        let mut merged = live.clone();
        merged.days = calendar_days(days, calendar);
        merged.earliest = merged.days.first().map(|d| d.date);
        let mut slots: Vec<Slot> = slots.into_values().collect();
        slots.sort_by_key(|s| s.start);
        merged.slots = slots;
        merged.model_names = names;
        merged.unpriced_models = unpriced.into_iter().collect();
        // Hours are withheld only when what was added has no hour of its own.
        merged.has_aggregate_timing =
            live.has_aggregate_timing || marks.has_aggregate_timing && (!rest.is_empty() || !unclassified.is_empty());
        merged.has_partial_counts = live.has_partial_counts || marks.has_partial_counts;
        if live.days.is_empty() {
            if let Some(origin) = marks.origin {
                merged.origin = origin;
            }
        }
        let mut sessions = live.sessions.clone();
        sessions.extend(self.kept_sessions(room, live, calendar));
        sessions.sort_by_key(|s| std::cmp::Reverse(s.end));
        merged.sessions = sessions;
        merged
    }

    /// The kept sessions whose work the added days can hold, oldest first.
    ///
    /// **A session is shown only if the tokens it lost are still lost.** When the store folded a
    /// deleted session's messages into another one that is still there, the days have nothing
    /// added for them, and showing the old row as well would count a project's work twice.
    fn kept_sessions(&self, mut room: HashMap<DateTime<Utc>, i64>, live: &Ledger, calendar: &Calendar) -> Vec<Session> {
        let live_ids: HashSet<&str> = live.sessions.iter().map(|s| s.id.as_str()).collect();
        let mut ordered: Vec<&Session> = self.sessions.values().filter(|s| !live_ids.contains(s.id.as_str())).collect();
        ordered.sort_by(|a, b| a.start.cmp(&b.start).then_with(|| a.id.cmp(&b.id)));
        let mut kept = Vec::new();
        for session in ordered {
            let mut by_day: HashMap<DateTime<Utc>, i64> = HashMap::new();
            if !session.days.is_empty() {
                for day in &session.days {
                    *by_day.entry(calendar.start_of_day(day.date)).or_default() += day.tokens;
                }
            } else if !session.slots.is_empty() {
                for slot in &session.slots {
                    *by_day.entry(calendar.start_of_day(slot.start)).or_default() += slot.tokens;
                }
            } else {
                by_day.insert(calendar.start_of_day(session.start), session.tokens);
            }
            if !by_day.iter().all(|(day, tokens)| room.get(day).copied().unwrap_or(0) >= *tokens) {
                continue;
            }
            for (day, tokens) in by_day {
                *room.entry(day).or_default() -= tokens;
            }
            kept.push(session.clone());
        }
        kept
    }
}

impl Marks {
    /// One ledger in the archive's terms.
    fn of(ledger: &Ledger, calendar: &Calendar) -> Marks {
        let mut marks = Marks::default();
        let mut in_slots: HashMap<NaiveDate, HashMap<String, TokenTally>> = HashMap::new();
        for slot in &ledger.slots {
            let index = slot.start.timestamp().div_euclid(QUARTER);
            let day = calendar.date(slot.start);
            let mut classified = 0;
            for (model, tally) in &slot.models {
                *marks.slots.entry(index).or_default().entry(model.clone()).or_default() += tally;
                *in_slots.entry(day).or_default().entry(model.clone()).or_default() += tally;
                classified += tally.total();
            }
            if slot.tokens > classified {
                *marks.slot_extra.entry(index).or_default() += slot.tokens - classified;
            }
        }
        for day in &ledger.days {
            let date = calendar.date(day.date);
            for (model, tally) in &day.model_tallies {
                let rest = tally.beyond(in_slots.get(&date).and_then(|m| m.get(model)).unwrap_or(&TokenTally::default()));
                if rest.total() > 0 {
                    marks.day_rest.entry(date).or_default().insert(model.clone(), rest);
                }
            }
            for (model, count) in &day.model_unclassified_tokens {
                if *count > 0 {
                    marks.day_unclassified.entry(date).or_default().insert(model.clone(), *count);
                }
            }
        }
        marks
    }

    /// The local days with any work.
    fn days(&self, calendar: &Calendar) -> HashSet<NaiveDate> {
        let mut days: HashSet<NaiveDate> = self.day_rest.keys().chain(self.day_unclassified.keys()).copied().collect();
        days.extend(self.slots.keys().chain(self.slot_extra.keys()).map(|index| calendar.date(start_of(*index))));
        days
    }

    fn raise(&mut self, other: &Marks) {
        for (index, models) in &other.slots {
            let into = self.slots.entry(*index).or_default();
            for (model, tally) in models {
                let high = into.get(model).map_or_else(|| tally.clone(), |kept| kept.highest(tally));
                into.insert(model.clone(), high);
            }
        }
        for (index, extra) in &other.slot_extra {
            let into = self.slot_extra.entry(*index).or_default();
            *into = (*into).max(*extra);
        }
        for (day, models) in &other.day_rest {
            let into = self.day_rest.entry(*day).or_default();
            for (model, tally) in models {
                let high = into.get(model).map_or_else(|| tally.clone(), |kept| kept.highest(tally));
                into.insert(model.clone(), high);
            }
        }
        for (day, models) in &other.day_unclassified {
            let into = self.day_unclassified.entry(*day).or_default();
            for (model, count) in models {
                let high = into.get(model).map_or(*count, |kept| (*kept).max(*count));
                into.insert(model.clone(), high);
            }
        }
    }
}

fn start_of(index: i64) -> DateTime<Utc> {
    DateTime::<Utc>::from_timestamp(index * QUARTER, 0).expect("a quarter-hour")
}

fn add_maps<V: Clone + std::ops::AddAssign>(base: &mut HashMap<String, V>, added: &HashMap<String, V>) {
    for (key, value) in added {
        match base.get_mut(key) {
            Some(kept) => *kept += value.clone(),
            None => {
                base.insert(key.clone(), value.clone());
            }
        }
    }
}

fn combine_day(base: Option<LedgerDay>, added: &LedgerDay) -> LedgerDay {
    let Some(mut base) = base else { return added.clone() };
    base.tokens += added.tokens;
    base.cost += added.cost;
    base.unpriced_tokens += added.unpriced_tokens;
    add_maps(&mut base.models, &added.models);
    base.tally += &added.tally;
    add_maps(&mut base.model_tallies, &added.model_tallies);
    add_maps(&mut base.model_costs, &added.model_costs);
    add_maps(&mut base.model_unclassified_tokens, &added.model_unclassified_tokens);
    base
}

fn combine_slot(base: Option<Slot>, added: &Slot) -> Slot {
    let Some(mut base) = base else { return added.clone() };
    base.tokens += added.tokens;
    base.cost += added.cost;
    base.unpriced_tokens += added.unpriced_tokens;
    add_maps(&mut base.models, &added.models);
    base
}

/// Ascending, with the quiet days between filled in, as every ledger's days are.
fn calendar_days(mut days: HashMap<DateTime<Utc>, LedgerDay>, calendar: &Calendar) -> Vec<LedgerDay> {
    let (Some(&first), Some(&last)) = (days.keys().min(), days.keys().max()) else { return Vec::new() };
    let mut filled = Vec::new();
    let mut cursor = first;
    while cursor <= last {
        filled.push(days.remove(&cursor).unwrap_or_else(|| LedgerDay::quiet(cursor)));
        let next = calendar.start_of_day(calendar.add_days(cursor, 1));
        if next <= cursor {
            break;
        }
        cursor = next;
    }
    filled
}
