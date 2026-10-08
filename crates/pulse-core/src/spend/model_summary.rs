// Ported from upstream Sources/Pulse/Usage/ModelSpendSummary.swift.
//! One model's usage, answered from the same ledgers the combined page adds up: the drill-down
//! behind one of its rows (and behind one agent's copy of it, since the caller passes only that
//! agent's ledger).
//!
//! Money is an API-rate estimate for this model, never a share of a day. Each raw id is priced at
//! its own published rates and the model's figure is the sum of those. Only the priced subset of
//! tokens is added up: a model whose raw id has no price contributes `unpriced_tokens` and no
//! money, so a subtotal is never mistaken for the whole. A total of exactly zero from a real zero
//! rate is a reading; no price at all is None.
//!
//! Categories and hours are shown only where the ledger's per-model detail reconciles; otherwise
//! they are None. The key is exactly the one the combined list uses
//! (`ledger.model_names[raw] ?? raw`).

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, Utc};
use serde::Serialize;

use super::agent::SpendAgent;
use super::calendar::Calendar;
use super::ledger::Ledger;
use super::tally::{TokenCost, TokenTally};

/// One calendar day of this model's work.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelDay {
    pub date: DateTime<Utc>,
    pub tokens: i64,
    /// Classified tokens where the recorded kinds plus the explicit unclassified remainder
    /// reconcile. None for missing or broken detail.
    pub tally: Option<TokenTally>,
    /// The day's money for the priced subset of its tokens. None where nothing that day could be
    /// priced; a real zero from a zero rate is a value, not None.
    pub cost_breakdown: Option<TokenCost>,
    pub unpriced_tokens: i64,
    pub unclassified_tokens: i64,
}

impl ModelDay {
    pub fn cost(&self) -> Option<f64> {
        self.cost_breakdown.map(|c| c.total())
    }

    pub fn classified_tally(&self) -> Option<&TokenTally> {
        if self.tokens > 0 && self.unclassified_tokens == self.tokens {
            None
        } else {
            self.tally.as_ref()
        }
    }
}

/// One agent's share of this model, over the chosen span.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelAgent {
    pub agent: SpendAgent,
    pub tokens: i64,
    pub cost: Option<f64>,
    pub unpriced_tokens: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSpendSummary {
    pub name: String,
    pub tokens: i64,
    /// The classified subset. Together with `unclassified_tokens` it accounts for the span; None
    /// where contributing detail was missing or broken.
    pub tally: Option<TokenTally>,
    pub unclassified_tokens: i64,
    /// Every day in the span, quiet ones included.
    pub days: Vec<ModelDay>,
    /// Heaviest first, name breaking a tie.
    pub agents: Vec<ModelAgent>,
    /// Tokens by hour of the local day, 0-23. None where the quarter-hour buckets could not be
    /// reconciled with the daily totals, so a model does not inherit another model's, or its
    /// agent's whole, time of day.
    pub hours: Option<HashMap<u32, i64>>,
    /// The model's money by kind, over the priced subset. None where no contributing raw id had a
    /// price, so an unpriced model is not shown as free.
    pub cost_breakdown: Option<TokenCost>,
    /// Tokens in the span with no price behind them. When `cost` is Some and this is more than
    /// zero, the figure is a partial estimate.
    pub unpriced_tokens: i64,
    pub has_aggregate_timing: bool,
    pub has_partial_counts: bool,
}

/// The day table's columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ModelDayColumn {
    Date,
    Fresh,
    CacheRead,
    Output,
    Unclassified,
    Total,
    Cost,
}

impl ModelSpendSummary {
    pub fn cost(&self) -> Option<f64> {
        self.cost_breakdown.map(|c| c.total())
    }

    pub fn is_empty(&self) -> bool {
        self.tokens == 0
    }

    pub fn active_days(&self) -> usize {
        self.days.iter().filter(|d| d.tokens > 0).count()
    }

    pub fn busiest_day(&self) -> Option<&ModelDay> {
        let mut best: Option<&ModelDay> = None;
        for day in &self.days {
            if best.is_none_or(|b| day.tokens > b.tokens) {
                best = Some(day);
            }
        }
        best
    }

    /// Adds one model's work up across every agent, over `over_last` days or all of it when None.
    ///
    /// The cutoff is the calendar's own midnight, the same window `SpendSummary::of` uses.
    /// All-time starts at this model's first day of actual use.
    ///
    /// Money is priced per raw id, per day. A contribution counts only when its own id was priced
    /// and its category detail adds up to the tokens counted; otherwise it is a token, not a
    /// zero. The check is checked: a negative kind or remainder, or a sum that would overflow, is
    /// broken metadata and is neither priced nor crashed on.
    pub fn of(
        ledgers: &HashMap<SpendAgent, Ledger>,
        name: &str,
        over_last: Option<usize>,
        now: DateTime<Utc>,
        calendar: &Calendar,
    ) -> ModelSpendSummary {
        let empty = || ModelSpendSummary { name: name.to_string(), ..ModelSpendSummary::default() };
        let today = calendar.start_of_day(now);
        let cutoff = over_last.map(|span| calendar.start_of_day(calendar.add_days(today, -(span as i64 - 1))));

        let mut day_tokens: BTreeMap<DateTime<Utc>, i64> = BTreeMap::new();
        let mut day_tally: HashMap<DateTime<Utc>, TokenTally> = HashMap::new();
        let mut day_tally_incomplete: HashMap<DateTime<Utc>, bool> = HashMap::new();
        let mut day_cost: HashMap<DateTime<Utc>, TokenCost> = HashMap::new();
        let mut day_unpriced: HashMap<DateTime<Utc>, i64> = HashMap::new();
        let mut day_unclassified: HashMap<DateTime<Utc>, i64> = HashMap::new();
        let mut agent_tokens: HashMap<SpendAgent, i64> = HashMap::new();
        let mut agent_cost: HashMap<SpendAgent, TokenCost> = HashMap::new();
        let mut agent_unpriced: HashMap<SpendAgent, i64> = HashMap::new();
        let mut hour_tokens: HashMap<u32, i64> = HashMap::new();
        let mut total_tally = TokenTally::default();
        let mut total_cost = TokenCost::default();
        let mut total_unpriced = 0;
        let mut total_unclassified = 0;
        let mut tally_complete = true;
        let mut hours_complete = true;
        let mut priced_any = false;
        let mut found = false;
        let mut aggregate = false;
        let mut partial = false;

        for (&agent, ledger) in ledgers {
            // A ledger that cannot be priced has no place here.
            if !ledger.origin.supports_token_spend() {
                continue;
            }

            // Reconciled inside one ledger, one day and one raw id at a time. A global
            // day-total-versus-slot-total check cancels real faults against each other, and
            // would draw an hourly profile nobody measured.
            let mut ledger_day_tokens: HashMap<DateTime<Utc>, HashMap<&str, i64>> = HashMap::new();
            let mut ledger_slot_tokens: HashMap<DateTime<Utc>, HashMap<&str, i64>> = HashMap::new();

            for day in &ledger.days {
                if cutoff.is_some_and(|c| day.date < c) {
                    continue;
                }
                for (raw, &tokens) in &day.models {
                    if tokens <= 0 || ledger.model_names.get(raw).unwrap_or(raw) != name {
                        continue;
                    }
                    found = true;
                    aggregate |= ledger.has_aggregate_timing;
                    partial |= ledger.has_partial_counts;
                    *day_tokens.entry(day.date).or_default() += tokens;
                    *agent_tokens.entry(agent).or_default() += tokens;
                    *ledger_day_tokens.entry(day.date).or_default().entry(raw).or_default() += tokens;

                    // The split must account for every token: classified kinds plus the
                    // explicitly unclassified remainder has to equal the model's total, and every
                    // count has to be a real, non-negative one whose sum fits.
                    let model_tally = day.model_tallies.get(raw);
                    let raw_unclassified = day.model_unclassified_tokens.get(raw).copied();
                    let accounts = model_tally
                        .cloned()
                        .unwrap_or_default()
                        .accounts_for(tokens, raw_unclassified.unwrap_or(0));
                    let unclassified = raw_unclassified.unwrap_or(0);

                    let sums = if accounts {
                        let model = model_tally.cloned().unwrap_or_default();
                        day_tally
                            .get(&day.date)
                            .cloned()
                            .unwrap_or_default()
                            .checked_add(&model)
                            .zip(total_tally.checked_add(&model))
                            .filter(|(d, t)| d.checked_total().is_some() && t.checked_total().is_some())
                    } else {
                        None
                    };
                    if let Some((day_sum, total_sum)) = sums {
                        day_tally.insert(day.date, day_sum);
                        total_tally = total_sum;
                        *day_unclassified.entry(day.date).or_default() += unclassified;
                        total_unclassified += unclassified;
                    } else {
                        tally_complete = false;
                        day_tally_incomplete.insert(day.date, true);
                    }

                    // The money is the classified, priced part only; the unclassified tokens are
                    // counted in `unpriced_tokens` and never costed. A real zero rate is still a
                    // priced part.
                    match day.model_costs.get(raw).filter(|_| accounts) {
                        Some(cost) => {
                            priced_any = true;
                            *day_cost.entry(day.date).or_default() += *cost;
                            total_cost += *cost;
                            *agent_cost.entry(agent).or_default() += *cost;
                            *day_unpriced.entry(day.date).or_default() += unclassified;
                            total_unpriced += unclassified;
                            *agent_unpriced.entry(agent).or_default() += unclassified;
                        }
                        None => {
                            // Counted, never costed, and never patched to a zero.
                            *day_unpriced.entry(day.date).or_default() += tokens;
                            total_unpriced += tokens;
                            *agent_unpriced.entry(agent).or_default() += tokens;
                        }
                    }
                }
            }

            // The time of day survives only in the quarter-hour buckets, and only per model where
            // the reader kept it. A slot with no model detail contributes nothing here, and then
            // the reconciliation below fails, which is the point.
            for slot in &ledger.slots {
                if cutoff.is_some_and(|c| slot.start < c) {
                    continue;
                }
                for (raw, model_tally) in &slot.models {
                    if ledger.model_names.get(raw).unwrap_or(raw) != name {
                        continue;
                    }
                    // A slot whose model tally is broken contributes no hours.
                    let Some(slot_tokens) = model_tally.checked_total().filter(|t| *t > 0) else { continue };
                    let date = calendar.start_of_day(slot.start);
                    *ledger_slot_tokens.entry(date).or_default().entry(raw).or_default() += slot_tokens;
                    *hour_tokens.entry(calendar.hour(slot.start)).or_default() += slot_tokens;
                }
            }

            // Every day of this ledger that carried the model must have slot buckets adding to
            // the same per-raw-id figure. Empty ledgers compare equal and change nothing.
            if ledger_day_tokens != ledger_slot_tokens {
                hours_complete = false;
            }
        }

        if !found {
            return empty();
        }

        let mut summary = empty();
        summary.tokens = day_tokens.values().sum();
        // Every contributing model's split is present and matches its own daily total, and the
        // pieces add up to the total on screen.
        if tally_complete && total_tally.accounts_for(summary.tokens, total_unclassified) {
            summary.tally = Some(total_tally);
            summary.unclassified_tokens = total_unclassified;
        }
        // The hour series is the model's own only where every ledger's buckets reconcile with its
        // days.
        if hours_complete {
            summary.hours = Some(hour_tokens);
        }
        if priced_any {
            summary.cost_breakdown = Some(total_cost);
        }
        summary.unpriced_tokens = total_unpriced;
        summary.has_aggregate_timing = aggregate;
        summary.has_partial_counts = partial;

        summary.agents = agent_tokens
            .into_iter()
            .map(|(agent, tokens)| ModelAgent {
                agent,
                tokens,
                cost: agent_cost.get(&agent).map(TokenCost::total),
                unpriced_tokens: agent_unpriced.get(&agent).copied().unwrap_or(0),
            })
            .collect();
        summary
            .agents
            .sort_by(|a, b| b.tokens.cmp(&a.tokens).then_with(|| a.agent.display_name().cmp(b.agent.display_name())));

        // Every day in the window, quiet ones included: a drill-down should be the same calendar
        // as the row it was opened from.
        let first = cutoff.or_else(|| day_tokens.keys().next().copied()).unwrap_or(today);
        let last = today.max(day_tokens.keys().next_back().copied().unwrap_or(today));
        let mut cursor = first.min(last);
        while cursor <= last {
            let tokens = day_tokens.get(&cursor).copied().unwrap_or(0);
            let tally = if tokens == 0 {
                // A quiet day has nothing missing: its split is an empty one.
                Some(TokenTally::default())
            } else if !day_tally_incomplete.get(&cursor).copied().unwrap_or(false) {
                Some(day_tally.get(&cursor).cloned().unwrap_or_default())
            } else {
                None
            };
            summary.days.push(ModelDay {
                date: cursor,
                tokens,
                cost_breakdown: day_cost.get(&cursor).copied(),
                unpriced_tokens: day_unpriced.get(&cursor).copied().unwrap_or(0),
                unclassified_tokens: if tally.is_some() { day_unclassified.get(&cursor).copied().unwrap_or(0) } else { 0 },
                tally,
            });
            let next = calendar.add_days(cursor, 1);
            if next <= cursor {
                break;
            }
            cursor = next;
        }
        summary
    }

    /// The day rows in the order a column asks for.
    ///
    /// A missing category or price is not a zero: a day whose split was never recorded has no
    /// category columns; a day that priced nothing has no cost. So a missing cell sorts last
    /// whichever way the column is turned, a known zero sorts where zero belongs, and equal
    /// values keep a deterministic order broken by date. Token counts are compared as integers
    /// and money as floats.
    pub fn sorted(days: &[ModelDay], column: ModelDayColumn, ascending: bool, cache_unreported: bool) -> Vec<ModelDay> {
        fn ranked<T: PartialOrd>(days: &[ModelDay], ascending: bool, value: impl Fn(&ModelDay) -> Option<T>) -> Vec<ModelDay> {
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
            ModelDayColumn::Date => ranked(days, ascending, |d| Some(d.date)),
            ModelDayColumn::Fresh => ranked(days, ascending, |d| d.classified_tally().map(TokenTally::fresh)),
            ModelDayColumn::CacheRead => ranked(days, ascending, |d| {
                d.classified_tally().map(|t| t.cache_read).filter(|r| !(cache_unreported && *r == 0))
            }),
            ModelDayColumn::Output => ranked(days, ascending, |d| d.classified_tally().map(|t| t.output)),
            ModelDayColumn::Unclassified => ranked(days, ascending, |d| d.tally.as_ref().map(|_| d.unclassified_tokens)),
            ModelDayColumn::Total => ranked(days, ascending, |d| Some(d.tokens)),
            ModelDayColumn::Cost => ranked(days, ascending, ModelDay::cost),
        }
    }
}
