// Ported from upstream Sources/Pulse/Usage/SpendSummary.swift.
//! Every agent's spending added up, which is a different question from any one agent's.
//!
//! It reuses the ledgers rather than rescanning: adding them up is arithmetic over what is already
//! in memory, not another pass over a few hundred megabytes of transcripts. Only ledgers whose
//! origin supports token spend are in it; a provider's own statistics report one token total per
//! model and no money.

use std::collections::{BTreeSet, HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde::Serialize;

use super::agent::SpendAgent;
use super::calendar::Calendar;
use super::ledger::{Ledger, Session};
use super::project::{ProjectIdentity, UsageProject};
use super::tally::TokenTally;

/// One agent's share of the total.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentShare {
    pub agent: SpendAgent,
    pub tokens: i64,
    pub cost: f64,
    /// Tokens spent on models with no published price. Counted, not costed.
    pub unpriced_tokens: i64,
}

/// One model's share, across every agent that used it. Tokens only, and deliberately: there is no
/// per-model cost in a day's ledger row, and working one out from a day's blended rate would be
/// inventing it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelShare {
    /// The name the provider publishes, where models.dev has one.
    pub name: String,
    pub tokens: i64,
    /// Which agents sent work to it.
    pub agents: Vec<SpendAgent>,
    pub share: f64,
}

/// One day, with every agent's work in it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryDay {
    pub date: DateTime<Utc>,
    pub tokens: i64,
    pub cost: f64,
    /// The day split by kind of token, summed across agents.
    pub tally: TokenTally,
    /// Tokens that day with no price behind them.
    pub unpriced_tokens: i64,
    /// Reported tokens whose source supplied no usable category.
    pub unclassified_tokens: i64,
    pub has_invalid_categories: bool,
}

impl SummaryDay {
    fn quiet(date: DateTime<Utc>) -> Self {
        Self {
            date,
            tokens: 0,
            cost: 0.0,
            tally: TokenTally::default(),
            unpriced_tokens: 0,
            unclassified_tokens: 0,
            has_invalid_categories: false,
        }
    }

    pub fn has_token_breakdown(&self) -> bool {
        !self.has_invalid_categories && self.tally.accounts_for(self.tokens, self.unclassified_tokens)
    }

    pub fn classified_tally(&self) -> Option<&TokenTally> {
        (self.has_token_breakdown() && !(self.tokens > 0 && self.unclassified_tokens == self.tokens)).then_some(&self.tally)
    }
}

/// One transcript, with the agent that wrote it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRow {
    pub agent: SpendAgent,
    pub session: Session,
}

/// A label alone cannot prove that two agents mean the same project, so a label project is
/// scoped to its agent.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectId {
    pub identity: ProjectIdentity,
    pub agent: Option<SpendAgent>,
}

impl ProjectId {
    pub fn new(project: &UsageProject, agent: SpendAgent) -> Self {
        let agent = matches!(project.identity, ProjectIdentity::Label(_)).then_some(agent);
        Self { identity: project.identity.clone(), agent }
    }
}

/// One identified project, across every session in the selected span.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRow {
    pub id: ProjectId,
    pub name: String,
    pub tokens: i64,
    pub cost: f64,
    pub unpriced_tokens: i64,
    pub sessions: usize,
    pub last_used: DateTime<Utc>,
}

impl ProjectRow {
    pub fn estimated_cost(&self) -> Option<f64> {
        (!(self.tokens > 0 && self.unpriced_tokens == self.tokens)).then_some(self.cost)
    }
}

/// The day table's columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DayColumn {
    Date,
    /// New input, cache writes included (`TokenTally::fresh`).
    Fresh,
    CacheRead,
    Output,
    Unclassified,
    Total,
    Cost,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpendSummary {
    pub tokens: i64,
    pub cost: f64,
    /// The whole span split by kind of token.
    pub tally: TokenTally,
    pub unclassified_tokens: i64,
    /// Every contributing day's categories and explicit remainder reconcile.
    pub has_token_breakdown: bool,
    pub unpriced_tokens: i64,
    pub agents: Vec<AgentShare>,
    pub models: Vec<ModelShare>,
    pub days: Vec<SummaryDay>,
    /// Newest first.
    pub sessions: Vec<SessionRow>,
    /// Heaviest first.
    pub projects: Vec<ProjectRow>,
    /// Models seen in the logs that models.dev has no price for.
    pub unpriced_models: Vec<String>,
    /// Some contributing ledger had only session- or report-level timing for some of its work,
    /// so the hour figure must not be drawn.
    pub has_aggregate_timing: bool,
    /// Some contributing ledger's counts may be missing: the total is a floor, not a whole.
    pub has_partial_counts: bool,
    /// Tokens by calendar month, newest last.
    pub months: Vec<SummaryDay>,
    /// Tokens by hour of the local day, 0-23, from the quarter-hour buckets.
    pub hours: HashMap<u32, i64>,
    /// The run of days with work on them that is still going, and the longest run there has ever
    /// been: over the whole history, not the span.
    pub current_streak: usize,
    pub longest_streak: usize,
}

impl SpendSummary {
    pub fn is_empty(&self) -> bool {
        self.tokens == 0 && self.cost == 0.0
    }

    /// The heaviest day in the span, across every agent.
    pub fn busiest_day(&self) -> Option<&SummaryDay> {
        let mut best: Option<&SummaryDay> = None;
        for day in &self.days {
            if best.is_none_or(|b| day.tokens > b.tokens) {
                best = Some(day);
            }
        }
        best
    }

    /// Days with anything on them.
    pub fn active_days(&self) -> usize {
        self.days.iter().filter(|d| d.tokens > 0).count()
    }

    pub fn active_hours(&self) -> usize {
        self.hours.values().filter(|v| **v > 0).count()
    }

    /// The hour with the most tokens in it; None where nothing was recorded.
    pub fn peak_hour(&self) -> Option<u32> {
        // The earliest hour among equals.
        self.hours.iter().filter(|(_, v)| **v > 0).max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0))).map(|(h, _)| *h)
    }

    pub fn project_name(&self, row: &SessionRow) -> Option<String> {
        let project = row.session.project.as_ref()?;
        let id = ProjectId::new(project, row.agent);
        Some(self.projects.iter().find(|p| p.id == id).map_or_else(|| project.name.clone(), |p| p.name.clone()))
    }

    /// Run lengths over the days with work. Today is not over: a run that reached yesterday is
    /// still current before anything has been done today.
    pub fn streaks(worked: &HashSet<DateTime<Utc>>, today: DateTime<Utc>, calendar: &Calendar) -> (usize, usize) {
        let mut current = 0;
        let mut cursor = if worked.contains(&today) { today } else { calendar.start_of_day(calendar.add_days(today, -1)) };
        while worked.contains(&cursor) {
            current += 1;
            cursor = calendar.start_of_day(calendar.add_days(cursor, -1));
        }
        let mut sorted: Vec<_> = worked.iter().copied().collect();
        sorted.sort();
        let (mut longest, mut run) = (0, 0);
        let mut last: Option<DateTime<Utc>> = None;
        for day in sorted {
            let follows = last.is_some_and(|l| calendar.start_of_day(calendar.add_days(l, 1)) == day);
            run = if follows { run + 1 } else { 1 };
            longest = longest.max(run);
            last = Some(day);
        }
        (current, longest)
    }

    /// The day rows, in the order a column asks for. What the table shows blank sorts last,
    /// either way: a day whose kinds do not add up to its total shows no kinds, and a day nothing
    /// in which had a price shows no money.
    pub fn sorted(days: &[SummaryDay], column: DayColumn, ascending: bool, cache_unreported: bool) -> Vec<SummaryDay> {
        fn ranked<T: PartialOrd>(days: &[SummaryDay], ascending: bool, value: impl Fn(&SummaryDay) -> Option<T>) -> Vec<SummaryDay> {
            let mut out = days.to_vec();
            out.sort_by(|lhs, rhs| {
                let by_date = || if ascending { lhs.date.cmp(&rhs.date) } else { rhs.date.cmp(&lhs.date) };
                match (value(lhs), value(rhs)) {
                    (None, None) => by_date(),
                    (None, _) => std::cmp::Ordering::Greater,
                    (_, None) => std::cmp::Ordering::Less,
                    (Some(left), Some(right)) => {
                        if left == right {
                            by_date()
                        } else if (left < right) == ascending {
                            std::cmp::Ordering::Less
                        } else {
                            std::cmp::Ordering::Greater
                        }
                    }
                }
            });
            out
        }
        match column {
            DayColumn::Date => ranked(days, ascending, |d| Some(d.date)),
            DayColumn::Fresh => ranked(days, ascending, |d| d.classified_tally().map(TokenTally::fresh)),
            DayColumn::CacheRead => {
                ranked(days, ascending, |d| d.classified_tally().map(|t| t.cache_read).filter(|r| !(cache_unreported && *r == 0)))
            }
            DayColumn::Output => ranked(days, ascending, |d| d.classified_tally().map(|t| t.output)),
            DayColumn::Unclassified => ranked(days, ascending, |d| d.has_token_breakdown().then_some(d.unclassified_tokens)),
            DayColumn::Total => ranked(days, ascending, |d| Some(d.tokens)),
            DayColumn::Cost => ranked(days, ascending, |d| (!(d.tokens > 0 && d.unpriced_tokens == d.tokens)).then_some(d.cost)),
        }
    }

    /// Adds the ledgers up over the last `span` calendar days, or over everything when `span` is
    /// None.
    ///
    /// The span is a window on the calendar, not each ledger's last N entries: an agent last used
    /// a fortnight ago would contribute its own last seven recorded days, and "the last 7 days"
    /// then drew nine bars. The series is padded out to the whole window so a day nobody worked
    /// is a gap in a calendar rather than a missing column.
    pub fn of(ledgers: &HashMap<SpendAgent, Ledger>, over_last: Option<usize>, now: DateTime<Utc>, calendar: &Calendar) -> SpendSummary {
        let today = calendar.start_of_day(now);
        let cutoff = over_last.map(|span| calendar.start_of_day(calendar.add_days(today, -(span as i64 - 1))));
        Self::summarize(ledgers, cutoff, None, today, calendar)
    }

    /// The calendar days from `start` up to, not including, `end`: both local midnights. A
    /// month's or a year's recap is such a span. Days, quarter-hours and a session's own buckets
    /// outside `[start, end)` are not in it, and the padded series runs from `start` to the day
    /// before `end`. Streaks are still the whole history, as of `now`'s day.
    pub fn of_range(
        ledgers: &HashMap<SpendAgent, Ledger>,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        now: DateTime<Utc>,
        calendar: &Calendar,
    ) -> SpendSummary {
        Self::summarize(ledgers, Some(start), Some(end), calendar.start_of_day(now), calendar)
    }

    /// The part of a session that falls inside the span, or None where none does.
    ///
    /// This is the whole reason a session carries its own buckets: a session whose work straddles
    /// the cutoff contributes only its in-span quarter-hours, so a project's money and the span's
    /// own total are the same sum. Calendar-day buckets preserve the span when a report has no
    /// exact hour, and are used only when quarter-hour buckets are unavailable. A session with
    /// neither is one read before the ledger kept them: it falls back to being counted whole
    /// when it ended inside the span.
    fn window(session: &Session, lower: Option<DateTime<Utc>>, upper: Option<DateTime<Utc>>) -> Option<(i64, f64, i64, DateTime<Utc>)> {
        if lower.is_none() && upper.is_none() {
            return Some((session.tokens, session.cost, session.unpriced_tokens, session.end));
        }
        let inside = |date: DateTime<Utc>| lower.is_none_or(|l| date >= l) && upper.is_none_or(|u| date < u);

        if session.slots.is_empty() {
            if !session.days.is_empty() {
                let days: Vec<_> = session.days.iter().filter(|d| inside(d.date)).collect();
                let last = days.iter().map(|d| d.date).max()?;
                return Some((
                    days.iter().map(|d| d.tokens).sum(),
                    days.iter().map(|d| d.cost).sum(),
                    days.iter().map(|d| d.unpriced_tokens).sum(),
                    last,
                ));
            }
            return inside(session.end).then_some((session.tokens, session.cost, session.unpriced_tokens, session.end));
        }

        let (mut tokens, mut cost, mut unpriced) = (0, 0.0, 0);
        let mut last = lower.unwrap_or(DateTime::<Utc>::MIN_UTC);
        let mut found = false;
        for slot in session.slots.iter().filter(|s| inside(s.start)) {
            found = true;
            tokens += slot.tokens;
            cost += slot.cost;
            unpriced += slot.unpriced_tokens;
            last = last.max(slot.start);
        }
        found.then_some((tokens, cost, unpriced, last))
    }

    fn summarize(
        ledgers: &HashMap<SpendAgent, Ledger>,
        cutoff: Option<DateTime<Utc>>,
        upper: Option<DateTime<Utc>>,
        today: DateTime<Utc>,
        calendar: &Calendar,
    ) -> SpendSummary {
        let mut summary = SpendSummary::default();
        let inside = |date: DateTime<Utc>| cutoff.is_none_or(|c| date >= c) && upper.is_none_or(|u| date < u);

        let mut day_tokens: HashMap<DateTime<Utc>, i64> = HashMap::new();
        let mut day_cost: HashMap<DateTime<Utc>, f64> = HashMap::new();
        let mut day_tally: HashMap<DateTime<Utc>, TokenTally> = HashMap::new();
        let mut day_unpriced: HashMap<DateTime<Utc>, i64> = HashMap::new();
        let mut day_unclassified: HashMap<DateTime<Utc>, i64> = HashMap::new();
        let mut invalid_category_days: HashSet<DateTime<Utc>> = HashSet::new();
        let mut categories_complete = true;
        let mut hour_tokens: HashMap<u32, i64> = HashMap::new();
        let mut tally = TokenTally::default();
        let mut model_tokens: HashMap<String, i64> = HashMap::new();
        let mut model_agents: HashMap<String, BTreeSet<SpendAgent>> = HashMap::new();
        let mut unpriced: BTreeSet<String> = BTreeSet::new();
        #[derive(Default)]
        struct ProjectTotals {
            tokens: i64,
            cost: f64,
            unpriced: i64,
            sessions: usize,
            last_used: Option<DateTime<Utc>>,
            metadata: Option<UsageProject>,
        }
        let mut projects: HashMap<ProjectId, ProjectTotals> = HashMap::new();
        let mut has_aggregate = false;
        let mut has_partial = false;
        let mut worked: HashSet<DateTime<Utc>> = HashSet::new();

        let mut agents: Vec<(&SpendAgent, &Ledger)> = ledgers.iter().collect();
        agents.sort_by_key(|(agent, _)| **agent);
        for (&agent, ledger) in agents {
            // A ledger that cannot be priced has no place in a combined cost.
            if !ledger.origin.supports_token_spend() {
                continue;
            }
            for day in ledger.days.iter().filter(|d| d.tokens > 0) {
                worked.insert(calendar.start_of_day(day.date));
            }

            let window: Vec<_> = ledger.days.iter().filter(|d| inside(d.date)).collect();
            if window.is_empty() {
                continue;
            }

            let (mut agent_tokens, mut agent_cost, mut agent_unpriced) = (0, 0.0, 0);
            for day in &window {
                agent_tokens += day.tokens;
                agent_cost += day.cost;
                agent_unpriced += day.unpriced_tokens;
                tally += &day.tally;

                // Only the source's explicit remainder, never total minus known kinds. Check each
                // day before combining different agents.
                let mut unknown: Option<i64> = Some(0);
                for value in day.model_unclassified_tokens.values() {
                    unknown = unknown.and_then(|total| if *value < 0 { None } else { total.checked_add(*value) });
                }
                match unknown {
                    Some(unknown) => {
                        if !day.tally.accounts_for(day.tokens, unknown) {
                            categories_complete = false;
                            invalid_category_days.insert(day.date);
                        }
                        *day_unclassified.entry(day.date).or_default() += unknown;
                        summary.unclassified_tokens += unknown;
                    }
                    None => {
                        categories_complete = false;
                        invalid_category_days.insert(day.date);
                    }
                }

                *day_tokens.entry(day.date).or_default() += day.tokens;
                *day_cost.entry(day.date).or_default() += day.cost;
                *day_unpriced.entry(day.date).or_default() += day.unpriced_tokens;
                *day_tally.entry(day.date).or_default() += &day.tally;

                for (model, tokens) in &day.models {
                    let name = ledger.model_names.get(model).unwrap_or(model).clone();
                    *model_tokens.entry(name.clone()).or_default() += tokens;
                    model_agents.entry(name).or_default().insert(agent);
                }
            }

            // The time of day, which only the quarter-hour buckets carry. Bucketed by the hour
            // their start falls in: a bucket never straddles one.
            for slot in ledger.slots.iter().filter(|s| inside(s.start) && s.tokens > 0) {
                *hour_tokens.entry(calendar.hour(slot.start)).or_default() += slot.tokens;
            }

            // A session is counted by the part of it that falls in the span, not by when it
            // ended. A conversation that ran past midnight, or was resumed over days, used to be
            // added whole to whichever day it finished on. Its own quarter-hour buckets are what
            // make the window exact, and they are priced, so the money is a sum rather than a
            // proportion guessed from the total.
            for session in &ledger.sessions {
                let Some((tokens, cost, unpriced_tokens, last)) = Self::window(session, cutoff, upper) else { continue };

                // The row carries the span's portion, so the list and the totals above it are
                // the same arithmetic. Its start and end stay the conversation's own.
                let mut row = session.clone();
                row.tokens = tokens;
                row.cost = cost;
                row.unpriced_tokens = unpriced_tokens;
                summary.sessions.push(SessionRow { agent, session: row });

                let Some(metadata) = &session.project else { continue };
                let totals = projects.entry(ProjectId::new(metadata, agent)).or_default();
                totals.metadata = Some(metadata.clone());
                totals.tokens += tokens;
                totals.cost += cost;
                totals.unpriced += unpriced_tokens;
                totals.sessions += 1;
                totals.last_used = Some(totals.last_used.map_or(last, |l| l.max(last)));
            }

            if agent_tokens <= 0 && agent_cost <= 0.0 {
                continue;
            }

            // A ledger that contributed and had aggregate timing taints the hour figure for the
            // whole scope; a ledger that contributed nothing does not. The same rule marks the
            // total partial.
            has_aggregate |= ledger.has_aggregate_timing;
            has_partial |= ledger.has_partial_counts;

            summary.agents.push(AgentShare { agent, tokens: agent_tokens, cost: agent_cost, unpriced_tokens: agent_unpriced });
            summary.tokens += agent_tokens;
            summary.cost += agent_cost;
            summary.unpriced_tokens += agent_unpriced;
            unpriced.extend(ledger.unpriced_models.iter().cloned());
        }

        // Heaviest first: the list is read to find where the work went.
        summary.agents.sort_by(|a, b| b.tokens.cmp(&a.tokens));

        let overall: i64 = model_tokens.values().sum();
        summary.models = model_tokens
            .into_iter()
            .map(|(name, tokens)| {
                let mut agents: Vec<SpendAgent> = model_agents.remove(&name).unwrap_or_default().into_iter().collect();
                // Sorted so a row's agents do not shuffle between reads.
                agents.sort_by_key(|a| a.display_name());
                ModelShare { share: if overall > 0 { tokens as f64 / overall as f64 } else { 0.0 }, name, tokens, agents }
            })
            .collect();
        summary.models.sort_by(|a, b| b.tokens.cmp(&a.tokens).then_with(|| a.name.cmp(&b.name)));

        (summary.current_streak, summary.longest_streak) = Self::streaks(&worked, today, calendar);

        // Every day in the window, including the empty ones: a chart whose bars are only the
        // days with work compresses a quiet fortnight into nothing and reads as a busy one.
        let first = cutoff.or_else(|| day_tokens.keys().min().copied()).unwrap_or(today);
        let last = match upper {
            Some(upper) => calendar.add_days(upper, -1),
            None => today.max(day_tokens.keys().max().copied().unwrap_or(today)),
        };
        // A bounded span starts where it was asked to; an `end` not after `start` leaves no days
        // rather than one before it.
        let mut cursor = if upper.is_none() { first.min(last) } else { first };
        while cursor <= last {
            summary.days.push(SummaryDay {
                date: cursor,
                tokens: day_tokens.get(&cursor).copied().unwrap_or(0),
                cost: day_cost.get(&cursor).copied().unwrap_or(0.0),
                tally: day_tally.get(&cursor).cloned().unwrap_or_default(),
                unpriced_tokens: day_unpriced.get(&cursor).copied().unwrap_or(0),
                unclassified_tokens: day_unclassified.get(&cursor).copied().unwrap_or(0),
                has_invalid_categories: invalid_category_days.contains(&cursor),
            });
            // Start of day again: where DST begins at midnight adding a day lands on 01:00 and
            // every later day would miss its key.
            let next = calendar.start_of_day(calendar.add_days(cursor, 1));
            if next <= cursor {
                break;
            }
            cursor = next;
        }

        // Months are rolled up from the padded day series, so a month with no work in it is
        // still a row rather than a hole in the sequence.
        let mut months: HashMap<DateTime<Utc>, SummaryDay> = HashMap::new();
        for day in &summary.days {
            let month = calendar.month_start(day.date);
            let entry = months.entry(month).or_insert_with(|| SummaryDay::quiet(month));
            entry.tokens += day.tokens;
            entry.cost += day.cost;
            entry.unpriced_tokens += day.unpriced_tokens;
        }
        summary.months = months.into_values().collect();
        summary.months.sort_by_key(|m| m.date);

        summary.sessions.sort_by(|a, b| b.session.end.cmp(&a.session.end).then_with(|| a.session.id.cmp(&b.session.id)));
        let known: HashSet<UsageProject> = projects.values().filter_map(|p| p.metadata.clone()).collect();
        let names = UsageProject::display_names(&known);
        let mut name_counts: HashMap<&str, usize> = HashMap::new();
        for project in projects.values().filter_map(|p| p.metadata.as_ref()) {
            *name_counts.entry(project.name.as_str()).or_default() += 1;
        }
        summary.projects = projects
            .iter()
            .filter_map(|(id, totals)| {
                let metadata = totals.metadata.as_ref()?;
                let mut name = names.get(metadata).cloned().unwrap_or_else(|| metadata.name.clone());
                if let Some(agent) = id.agent {
                    let count = name_counts.get(name.as_str()).copied().unwrap_or(0);
                    if count > usize::from(metadata.name == name) {
                        name.push_str(" · ");
                        name.push_str(agent.display_name());
                    }
                }
                Some(ProjectRow {
                    id: id.clone(),
                    name,
                    tokens: totals.tokens,
                    cost: totals.cost,
                    unpriced_tokens: totals.unpriced,
                    sessions: totals.sessions,
                    last_used: totals.last_used.unwrap_or(DateTime::<Utc>::MIN_UTC),
                })
            })
            .collect();
        summary.projects.sort_by(|a, b| b.tokens.cmp(&a.tokens).then_with(|| a.name.cmp(&b.name)));

        summary.has_token_breakdown = categories_complete && tally.accounts_for(summary.tokens, summary.unclassified_tokens);
        summary.tally = tally;
        summary.hours = hour_tokens;
        summary.unpriced_models = unpriced.into_iter().collect();
        summary.has_aggregate_timing = has_aggregate;
        summary.has_partial_counts = has_partial;
        summary
    }
}
