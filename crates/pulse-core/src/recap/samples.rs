// Ported from upstream Sources/Pulse/Settings/RecapSamples.swift.
//! Made-up recaps for tests and for looking at the cards. Not a code path the app ships: the
//! numbers are the design mockup's (840 million tokens in September 2026, $1,342, 27 of 30 days,
//! 412 sessions, peak at 23:00), worked out so they agree with one another, and a year to go with
//! them.
//!
//! Only the agents this port reads exist (Claude Code and Codex), so the sample lineup has two.

use std::collections::BTreeSet;

use chrono::{Datelike, NaiveDate, Weekday};

use super::{AgentShare, Day, ModelShare, Month, Period, Persona, ProjectShare, Recap};
use crate::spend::SpendAgent;

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).expect("a sample date")
}

fn days_in(year: i32, month: u32) -> u32 {
    Period::Month { year, month }.bounds().map(|(s, e)| (e - s).num_days() as u32).unwrap_or(30)
}

/// Tokens by hour of day, a night owl's: 62% of it from 21:00 to 04:59.
const HOUR_WEIGHTS: [f64; 24] = [
    180.0, 115.0, 51.0, 20.0, 8.0, 2.0, 2.0, 4.0, 10.0, 25.0, 40.0, 55.0, 50.0, 48.0, 60.0, 66.0, 62.0, 58.0, 50.0, 64.0, 78.0,
    225.0, 246.0, 256.0,
];

fn hours(total: i64) -> Vec<i64> {
    let weight: f64 = HOUR_WEIGHTS.iter().sum();
    HOUR_WEIGHTS.iter().map(|w| (w / weight * total as f64) as i64).collect()
}

const SAMPLE_AGENTS: [(SpendAgent, f64, usize); 2] = [(SpendAgent::ClaudeCode, 0.58, 27), (SpendAgent::Codex, 0.29, 21)];

/// The agents' shares, each with its own days: the first worked on every day with any work, the
/// others on an evenly spread few of them, so what the agents did on a day is never more than the
/// day had.
fn agent_shares(total: i64, days: &[Day], scale: usize) -> Vec<AgentShare> {
    let active: Vec<NaiveDate> = days.iter().filter(|d| d.tokens > 0).map(|d| d.date).collect();
    SAMPLE_AGENTS
        .iter()
        .enumerate()
        .map(|(index, (agent, share, base))| {
            let count = (base * scale).min(active.len());
            let dates: BTreeSet<NaiveDate> = if index == 0 {
                active.iter().copied().collect()
            } else {
                (0..count).map(|k| active[(k * active.len() + index) / count % active.len()]).collect()
            };
            AgentShare {
                agent: *agent,
                name: agent.display_name(),
                icon: agent.icon_resource(),
                tokens: (share * total as f64) as i64,
                share: *share,
                active_days: dates.len(),
                cost: None,
                active_dates: dates,
            }
        })
        .collect()
}

const SAMPLE_MODELS: [(&str, f64, f64); 5] = [
    ("Claude Opus", 0.41, 812.0),
    ("GPT-5.6 Sol", 0.23, 268.0),
    ("Claude Sonnet", 0.17, 154.0),
    ("GPT-5.6 mini", 0.11, 61.0),
    ("GLM-5", 0.08, 47.0),
];

const SAMPLE_PROJECTS: [(&str, f64, usize); 3] = [("pulse", 0.38, 168), ("agent-lab", 0.21, 94), ("notes-site", 0.14, 61)];

/// A tiny deterministic generator, so the sample year is the same every run.
struct Lehmer(u64);

impl Lehmer {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 33) as f64 / (1u64 << 31) as f64
    }
}

#[derive(Clone)]
pub struct MonthSpec {
    pub month_number: u32,
    pub priced: bool,
    pub has_hours: bool,
    pub has_agents: bool,
    pub has_cache: bool,
    pub persona: Option<Persona>,
    pub in_progress: bool,
    pub unpriced_share: f64,
    pub cache_savings: f64,
    pub through_day: Option<u32>,
}

impl Default for MonthSpec {
    fn default() -> Self {
        Self {
            month_number: 9,
            priced: true,
            has_hours: true,
            has_agents: true,
            has_cache: true,
            persona: Some(Persona::NightOwl),
            in_progress: false,
            unpriced_share: 0.0,
            cache_savings: 410.0,
            through_day: None,
        }
    }
}

/// September 2026, finished (or another month of 2026, with the same days repeated or cut to its
/// length; `through_day` is a month still running: only the days so far, so the rest are to
/// come).
pub fn month(spec: MonthSpec) -> Recap {
    // The 1st is a Tuesday; days 4, 10 and 11 are quiet.
    const WEIGHTS: [f64; 30] = [
        55.0, 70.0, 40.0, 0.0, 35.0, 90.0, 80.0, 62.0, 66.0, 0.0, 0.0, 74.0, 100.0, 88.0, 60.0, 45.0, 120.0, 96.0, 60.0, 72.0,
        84.0, 64.0, 78.0, 92.0, 86.0, 58.0, 66.0, 70.0, 82.0, 66.0,
    ];
    let tokens_per_weight = 428_800.0;
    let cost_per_token = 1342.0 / 840_019_200.0;
    let month_length = days_in(2026, spec.month_number);
    let length = spec.through_day.unwrap_or(month_length).min(month_length);
    let days: Vec<Day> = (0..length)
        .map(|index| {
            let tokens = (WEIGHTS[index as usize % WEIGHTS.len()] * tokens_per_weight) as i64;
            Day {
                date: date(2026, spec.month_number, index + 1),
                tokens,
                cost: (spec.priced && tokens > 0).then_some(tokens as f64 * cost_per_token),
            }
        })
        .collect();
    let total: i64 = days.iter().map(|d| d.tokens).sum();
    let mut busiest: Option<&Day> = None;
    for day in &days {
        if day.tokens > busiest.map_or(0, |b| b.tokens) {
            busiest = Some(day);
        }
    }
    let (current, longest) = Recap::streaks(&days, spec.in_progress);
    let end = match spec.through_day {
        Some(day) => date(2026, spec.month_number, day + 1),
        None => Period::Month { year: 2026, month: spec.month_number }.bounds().expect("bounds").1,
    };
    Recap {
        period: Period::Month { year: 2026, month: spec.month_number },
        start: date(2026, spec.month_number, 1),
        end,
        is_in_progress: spec.in_progress,
        tokens: total,
        cost: spec.priced.then(|| if spec.through_day.is_none() { 1342.0 } else { total as f64 * cost_per_token }),
        unpriced_tokens: (spec.unpriced_share * total as f64) as i64,
        previous_tokens: Some((total as f64 / 1.38) as i64),
        active_days: days.iter().filter(|d| d.tokens > 0).count(),
        elapsed_days: length as usize,
        sessions: if spec.through_day.is_none() { 412 } else { 14 * length as usize },
        months: Vec::new(),
        hours: spec.has_hours.then(|| hours(total)),
        peak_hour: spec.has_hours.then_some(23),
        late_share: spec.has_hours.then_some(0.62),
        latest_minute: Some(134),
        late_nights: 11,
        persona: spec.persona,
        models: SAMPLE_MODELS
            .iter()
            .map(|(name, share, cost)| ModelShare {
                name: name.to_string(),
                tokens: (share * total as f64) as i64,
                share: *share,
                cost: spec.priced.then_some(*cost),
            })
            .collect(),
        agents: if spec.has_agents { agent_shares(total, &days, 1) } else { Vec::new() },
        projects: SAMPLE_PROJECTS
            .iter()
            .map(|(name, share, sessions)| ProjectShare {
                name: name.to_string(),
                tokens: (share * total as f64) as i64,
                share: *share,
                sessions: *sessions,
            })
            .collect(),
        cache_hit_rate: spec.has_cache.then_some(0.91),
        cache_savings: (spec.has_cache && spec.priced).then_some(spec.cache_savings),
        current_streak: current,
        longest_streak: longest,
        busiest_day: busiest.cloned(),
        currency: "USD".to_string(),
        is_partial: false,
        days,
    }
}

/// 2025, finished: 6.76 billion tokens, September the busiest month. `through_month` is a year
/// still running.
pub fn year(priced: bool, through_month: Option<u32>) -> Recap {
    let month_totals: Vec<f64> =
        [310.0, 280.0, 420.0, 390.0, 520.0, 610.0, 580.0, 640.0, 840.0, 770.0, 710.0, 690.0].iter().map(|m| m * 1_000_000.0).collect();
    let cost_per_token = 9820.0 / month_totals.iter().sum::<f64>();
    let mut random = Lehmer(2025);
    let mut days: Vec<Day> = Vec::new();
    let mut months: Vec<Month> = Vec::new();
    for month in 1..=12u32 {
        // A year still running has nothing in the months to come.
        if through_month.is_some_and(|through| month > through) {
            months.push(Month { month, tokens: 0, cost: None, active_days: 0 });
            continue;
        }
        let count = days_in(2025, month);
        let weights: Vec<f64> = (1..=count)
            .map(|day| {
                let weekday = date(2025, month, day).weekday();
                let weekend = weekday == Weekday::Sun || weekday == Weekday::Sat;
                let quiet = random.next() < if weekend { 0.28 } else { 0.07 };
                if quiet { 0.0 } else { 0.35 + 0.65 * random.next() }
            })
            .collect();
        let scale = month_totals[month as usize - 1] / weights.iter().sum::<f64>();
        let month_days: Vec<Day> = weights
            .iter()
            .enumerate()
            .map(|(index, weight)| {
                let tokens = (weight * scale) as i64;
                Day {
                    date: date(2025, month, index as u32 + 1),
                    tokens,
                    cost: (priced && tokens > 0).then_some(tokens as f64 * cost_per_token),
                }
            })
            .collect();
        let tokens: i64 = month_days.iter().map(|d| d.tokens).sum();
        months.push(Month {
            month,
            tokens,
            cost: priced.then_some(tokens as f64 * cost_per_token),
            active_days: month_days.iter().filter(|d| d.tokens > 0).count(),
        });
        days.extend(month_days);
    }
    let total: i64 = days.iter().map(|d| d.tokens).sum();
    let (current, longest) = Recap::streaks(&days, false);
    let mut busiest: Option<&Day> = None;
    for day in &days {
        if day.tokens > busiest.map_or(0, |b| b.tokens) {
            busiest = Some(day);
        }
    }
    Recap {
        period: Period::Year(2025),
        start: date(2025, 1, 1),
        end: date(2026, 1, 1),
        is_in_progress: through_month.is_some(),
        tokens: total,
        cost: priced.then_some(9820.0),
        unpriced_tokens: 0,
        previous_tokens: Some((total as f64 / 2.4) as i64),
        active_days: days.iter().filter(|d| d.tokens > 0).count(),
        elapsed_days: days.len(),
        sessions: 3214,
        months,
        hours: Some(hours(total)),
        peak_hour: Some(23),
        late_share: Some(0.62),
        latest_minute: Some(207),
        late_nights: 96,
        persona: Some(Persona::NightOwl),
        models: SAMPLE_MODELS
            .iter()
            .map(|(name, share, cost)| ModelShare {
                name: name.to_string(),
                tokens: (share * total as f64) as i64,
                share: *share,
                cost: priced.then_some(cost * 7.3),
            })
            .collect(),
        agents: agent_shares(total, &days, 10),
        projects: SAMPLE_PROJECTS
            .iter()
            .map(|(name, share, sessions)| ProjectShare {
                name: name.to_string(),
                tokens: (share * total as f64) as i64,
                share: *share,
                sessions: sessions * 7,
            })
            .collect(),
        cache_hit_rate: Some(0.89),
        cache_savings: priced.then_some(3120.0),
        current_streak: current,
        longest_streak: longest,
        busiest_day: busiest.cloned(),
        currency: "USD".to_string(),
        is_partial: false,
        days,
    }
}

/// A month in which nothing was recorded.
pub fn empty() -> Recap {
    Recap {
        period: Period::Month { year: 2026, month: 9 },
        start: date(2026, 9, 1),
        end: date(2026, 10, 1),
        is_in_progress: false,
        tokens: 0,
        cost: None,
        unpriced_tokens: 0,
        previous_tokens: None,
        active_days: 0,
        elapsed_days: 30,
        sessions: 0,
        days: Vec::new(),
        months: Vec::new(),
        hours: None,
        peak_hour: None,
        late_share: None,
        latest_minute: None,
        late_nights: 0,
        persona: None,
        models: Vec::new(),
        agents: Vec::new(),
        projects: Vec::new(),
        cache_hit_rate: None,
        cache_savings: None,
        current_streak: 0,
        longest_streak: 0,
        busiest_day: None,
        currency: "USD".to_string(),
        is_partial: false,
    }
}
