// Ported from upstream Sources/Pulse/Settings/RecapDeck.swift and RecapScoreData.swift (the rules,
// not the drawing).
//! The cards a [`Recap`] gets, in order, and the facts they share.
//!
//! A card whose data is missing is left out, never drawn empty or with a zero: no agents, no
//! opener; no hour shape, no timetable; no cost, no price or too much unpriced work, no payback.
//! An empty recap gets no deck at all.
//!
//! The reader's monthly price lives in the window and changes live, so this module works out
//! everything that does not depend on it and leaves the one division (`used / paid`) to the card:
//! [`DeckFacts::payback_months`] is `Some` exactly when a payback card could exist given a price.

use chrono::Datelike;
use serde::Serialize;

use super::insights::RecapInsights;
use super::Recap;

/// One card of a recap. Each is 1080 x 1920.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Card {
    /// The dense one-page summary. Stands alone, so it is not numbered.
    Poster,
    /// The month number or the year, and the agents that did the work.
    Opener,
    /// A month: every day of it, one cell each.
    Calendar,
    /// A year: every month of it as a small calendar.
    YearCalendar,
    /// A year: one bar per month.
    Months,
    /// Peak hour, on a dial and as 24 bars.
    Timetable,
    /// What the period cost at API prices against the subscription price.
    Payback,
    /// The closing scorecard.
    Scorecard,
}

/// Cost against what the reader pays.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Payback {
    /// What the period's work would have cost at API prices.
    pub used: f64,
    /// The monthly price the reader typed.
    pub monthly_price: f64,
    /// Months of subscription the period covers: 1 for a month, 12 for a year, prorated by days
    /// for a period still running.
    pub months: f64,
    pub is_to_date: bool,
}

impl Payback {
    /// What the subscription cost over the span: the price times `months`.
    pub fn paid(&self) -> f64 {
        self.monthly_price * self.months
    }

    /// `used / paid`, under 1 when the subscription cost more than the work did.
    pub fn multiple(&self) -> f64 {
        self.used / self.paid()
    }
}

/// The share of tokens without a price at which the payback card is dropped.
pub const MAXIMUM_UNPRICED_SHARE: f64 = 0.01;
/// The cache's saving worth saying: nothing below fifty cents, where "about $0.00" would be the
/// sentence.
pub const MINIMUM_SAVINGS: f64 = 0.5;

/// Months of subscription the period spans: one for a month, twelve for a year. A period still
/// running is prorated by days (the days so far over the days in the month or the year), not by
/// whole months started.
pub fn paid_months(recap: &Recap) -> f64 {
    let whole = if recap.period.is_year() { 12.0 } else { 1.0 };
    if !recap.is_in_progress {
        return whole;
    }
    let total = recap.period.bounds().map_or(0, |(start, end)| (end - start).num_days());
    if total <= 0 {
        return whole;
    }
    whole * (recap.elapsed_days as i64).min(total) as f64 / total as f64
}

/// The months a payback card divides a price by, or `None` when no price could give this recap a
/// payback card: it needs a priced period, and one priced nearly whole (below one percent of its
/// tokens unpriced).
pub fn payback_months(recap: &Recap) -> Option<f64> {
    recap.cost.filter(|c| *c > 0.0)?;
    if recap.tokens <= 0 || recap.unpriced_tokens as f64 / recap.tokens as f64 >= MAXIMUM_UNPRICED_SHARE {
        return None;
    }
    let months = paid_months(recap);
    (months > 0.0).then_some(months)
}

/// A payback needs a priced period, a price, and a period that is priced nearly whole. A price of
/// nothing is not a subscription.
pub fn payback(recap: &Recap, monthly_price: Option<f64>) -> Option<Payback> {
    let used = recap.cost.filter(|c| *c > 0.0)?;
    let price = monthly_price.filter(|p| *p > 0.0)?;
    let months = payback_months(recap)?;
    Some(Payback { used, monthly_price: price, months, is_to_date: recap.is_in_progress })
}

/// The cards in order. An empty recap has none.
pub fn cards(recap: &Recap, has_payback: bool) -> Vec<Card> {
    if recap.is_empty() {
        return Vec::new();
    }
    let mut cards = vec![Card::Poster];
    if !recap.agents.is_empty() {
        cards.push(Card::Opener);
    }
    let any_day = recap.days.iter().any(|d| d.tokens > 0);
    if recap.period.is_year() {
        if any_day {
            cards.push(Card::YearCalendar);
        }
        if recap.months.len() == 12 && recap.months.iter().any(|m| m.tokens > 0) {
            cards.push(Card::Months);
        }
    } else if any_day {
        cards.push(Card::Calendar);
    }
    if let Some(hours) = &recap.hours {
        if hours.len() == 24 && recap.peak_hour.is_some() && hours.iter().any(|h| *h > 0) {
            cards.push(Card::Timetable);
        }
    }
    if has_payback {
        cards.push(Card::Payback);
    }
    cards.push(Card::Scorecard);
    cards
}

/// Whether some of the money is a floor because part of the work had no published price, where
/// there is money to say it about.
pub fn cost_is_floor(recap: &Recap) -> bool {
    recap.cost.is_some() && recap.unpriced_tokens > 0
}

/// The cache's saving, only from fifty cents.
pub fn cache_savings(recap: &Recap) -> Option<f64> {
    recap.cache_savings.filter(|s| *s >= MINIMUM_SAVINGS)
}

/// Cost per day, or per month for a year, to draw under the poster's total. Empty where fewer
/// than two points carry a price, and where any point with work has none: a line through a zero
/// for a day that was merely unpriced would be a figure made up. A quiet point is a real zero, and
/// a month still to come is not drawn at all.
pub fn cost_series(recap: &Recap) -> Vec<f64> {
    let series: Vec<(i64, Option<f64>)> = if recap.period.is_year() {
        let insights = RecapInsights::new(recap);
        recap
            .months
            .iter()
            .enumerate()
            .filter(|(index, _)| !insights.is_month_to_come(*index))
            .map(|(_, m)| (m.tokens, m.cost))
            .collect()
    } else {
        recap.days.iter().map(|d| (d.tokens, d.cost)).collect()
    };
    if series.iter().any(|(tokens, cost)| *tokens > 0 && cost.is_none()) {
        return Vec::new();
    }
    if series.iter().filter(|(_, cost)| cost.is_some()).count() > 1 {
        series.iter().map(|(_, cost)| cost.unwrap_or(0.0)).collect()
    } else {
        Vec::new()
    }
}

/// The streak a card shows, of the period's own days: the one still going while the period is
/// running and today or yesterday ended a run, else the longest the period had. A past period is
/// never "still going". `None` when no day had work. (days, is_current)
pub fn streak(recap: &Recap) -> Option<(usize, bool)> {
    if recap.longest_streak == 0 {
        return None;
    }
    if recap.is_in_progress && recap.current_streak > 0 {
        return Some((recap.current_streak, true));
    }
    Some((recap.longest_streak, false))
}

/// Sessions over days with work, rounded. `None` when either is none or the average rounds to
/// nothing, so the card says nothing rather than "0".
pub fn sessions_per_active_day(recap: &Recap) -> Option<usize> {
    if recap.active_days == 0 || recap.sessions == 0 {
        return None;
    }
    let average = (recap.sessions as f64 / recap.active_days as f64).round() as usize;
    (average > 0).then_some(average)
}

/// What one day (or, in a year, one month) was: worked, quiet, or not yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Slot {
    /// Some work.
    Active,
    /// Over, and nothing recorded.
    Quiet,
    /// Still to come in a period that is running. Never drawn as a zero.
    Future,
}

/// One bar of the scorecard's day (or month) strip.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScoreBar {
    pub slot: Slot,
    /// The slot's tokens over the period's largest, 0..=1. Zero unless active.
    pub fraction: f64,
    pub is_busiest: bool,
}

/// One bar per day of the month (up to the month's last day, the days after today as `Future`),
/// or one per month of the year. Empty where no slot had work.
pub fn score_bars(recap: &Recap) -> Vec<ScoreBar> {
    let quiet = ScoreBar { slot: Slot::Quiet, fraction: 0.0, is_busiest: false };
    let future = ScoreBar { slot: Slot::Future, fraction: 0.0, is_busiest: false };
    let bars: Vec<ScoreBar> = if recap.period.is_year() {
        let maximum = recap.months.iter().map(|m| m.tokens).max().unwrap_or(0);
        let busiest = if maximum > 0 { recap.months.iter().position(|m| m.tokens == maximum) } else { None };
        let insights = RecapInsights::new(recap);
        recap
            .months
            .iter()
            .enumerate()
            .map(|(index, month)| {
                // A month that begins after today has not happened; a past month with nothing
                // is quiet.
                if insights.is_month_to_come(index) {
                    future
                } else if month.tokens > 0 && maximum > 0 {
                    ScoreBar { slot: Slot::Active, fraction: month.tokens as f64 / maximum as f64, is_busiest: Some(index) == busiest }
                } else {
                    quiet
                }
            })
            .collect()
    } else {
        let count = recap.period.bounds().map_or(recap.days.len(), |(start, end)| (end - start).num_days() as usize);
        let maximum = recap.days.iter().map(|d| d.tokens).max().unwrap_or(0);
        let busiest = recap.busiest_day.as_ref().map(|d| d.date);
        (1..=count.max(1))
            .map(|number| match recap.days.iter().find(|d| d.date.day() as usize == number) {
                None => future,
                Some(day) if day.tokens > 0 && maximum > 0 => ScoreBar {
                    slot: Slot::Active,
                    fraction: day.tokens as f64 / maximum as f64,
                    is_busiest: Some(day.date) == busiest,
                },
                Some(_) => quiet,
            })
            .collect()
    };
    if bars.iter().any(|b| b.slot == Slot::Active) { bars } else { Vec::new() }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Tone {
    Lime,
    Ink,
    Grey,
}

/// One tool in the segmented share bar and its legend. `name` is `None` for the grouped rest,
/// which the card calls "Other" in its own language.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScoreAgent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<&'static str>,
    pub share: f64,
    pub tone: Tone,
}

/// The tools as the share bar draws them: the top one lime, the second ink, the third grey, and
/// past three the second's runners-up grouped as the rest. Empty without agents.
pub fn score_agents(recap: &Recap) -> Vec<ScoreAgent> {
    const TONES: [Tone; 3] = [Tone::Lime, Tone::Ink, Tone::Grey];
    let agents = &recap.agents;
    let named = |index: usize| ScoreAgent { name: Some(agents[index].name), share: agents[index].share, tone: TONES[index] };
    if agents.len() <= 3 {
        return (0..agents.len()).map(named).collect();
    }
    let rest: f64 = agents[2..].iter().map(|a| a.share).sum();
    vec![named(0), named(1), ScoreAgent { name: None, share: rest, tone: Tone::Grey }]
}

/// Everything a card needs that does not depend on the reader's price.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeckFacts {
    /// The cards a recap gets with no payback card.
    pub cards: Vec<Card>,
    /// The cards it gets when a price is typed: the same, with the payback before the scorecard.
    pub cards_with_payback: Vec<Card>,
    /// Months to divide the price by; `Some` exactly when a payback is possible given a price.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payback_months: Option<f64>,
    pub cost_is_floor: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_savings: Option<f64>,
    pub cost_series: Vec<f64>,
    /// (days, still going)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub streak: Option<(usize, bool)>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sessions_per_active_day: Option<usize>,
    pub score_bars: Vec<ScoreBar>,
    pub score_agents: Vec<ScoreAgent>,
}

impl DeckFacts {
    pub fn of(recap: &Recap) -> DeckFacts {
        DeckFacts {
            cards: cards(recap, false),
            cards_with_payback: cards(recap, true),
            payback_months: payback_months(recap),
            cost_is_floor: cost_is_floor(recap),
            cache_savings: cache_savings(recap),
            cost_series: cost_series(recap),
            streak: streak(recap),
            sessions_per_active_day: sessions_per_active_day(recap),
            score_bars: score_bars(recap),
            score_agents: score_agents(recap),
        }
    }
}
