//! Ported from upstream SpendSummaryTests: adding two agents' ledgers together, which is
//! arithmetic until the calendar gets involved. Only Claude Code and Codex exist as agents here,
//! so tests that used a third agent use one of these.

use std::collections::HashMap;

use chrono::{DateTime, TimeZone, Utc};

use super::agent::SpendAgent::{self, ClaudeCode, Codex};
use super::calendar::Calendar;
use super::ledger::{Ledger, LedgerDay, Origin, Session, Slot};
use super::model_summary::ModelSpendSummary;
use super::prices::{ModelPrice, PriceTable};
use super::project::UsageProject;
use super::record::{build_ledger, AgentUsageRecord};
use super::summary::{DayColumn, SpendSummary, SummaryDay};
use super::tally::TokenTally;

fn calendar() -> Calendar {
    Calendar::utc(2)
}

fn today() -> DateTime<Utc> {
    calendar().start_of_day(Utc.timestamp_opt(1_789_372_800, 0).unwrap())
}

fn ago(days: i64) -> DateTime<Utc> {
    calendar().add_days(today(), -days)
}

fn at_hour(date: DateTime<Utc>, hour: i64) -> DateTime<Utc> {
    date + chrono::Duration::hours(hour)
}

fn day(days_ago: i64, tokens: i64, cost: f64) -> LedgerDay {
    LedgerDay::new(ago(days_ago), tokens, cost, 0, HashMap::from([("m".to_string(), tokens)]))
}

fn day_with(days_ago: i64, tokens: i64, cost: f64, models: &[(&str, i64)]) -> LedgerDay {
    LedgerDay::new(ago(days_ago), tokens, cost, 0, models.iter().map(|(k, v)| (k.to_string(), *v)).collect())
}

fn ledger(days: Vec<LedgerDay>) -> Ledger {
    ledger_with(days, Origin::LocalTranscripts, &[])
}

fn ledger_with(days: Vec<LedgerDay>, origin: Origin, names: &[(&str, &str)]) -> Ledger {
    Ledger {
        origin,
        earliest: days.first().map(|d| d.date),
        days,
        model_names: names.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        ..Ledger::empty()
    }
}

fn summary(ledgers: Vec<(SpendAgent, Ledger)>, over_last: Option<usize>) -> SpendSummary {
    SpendSummary::of(&ledgers.into_iter().collect(), over_last, today(), &calendar())
}

fn summary_day(date: DateTime<Utc>, tokens: i64, cost: f64, tally: TokenTally, unclassified: i64) -> SummaryDay {
    SummaryDay { date, tokens, cost, tally, unpriced_tokens: 0, unclassified_tokens: unclassified, has_invalid_categories: false }
}

#[test]
fn a_month_over_a_midnight_dst_start_keeps_every_day_on_its_own_key() {
    // Santiago springs forward at 00:00 on Sunday 6 September 2026: that day starts at 01:00, and
    // the padded series used to stay at 01:00 after it.
    let santiago = Calendar::with_zone(chrono_tz::America::Santiago, 2);
    let date = |month: u32, day: u32| santiago.midnight(chrono::NaiveDate::from_ymd_opt(2026, month, day).unwrap());
    let worked = date(9, 20);
    let ledgers = HashMap::from([(ClaudeCode, ledger(vec![LedgerDay::new(worked, 50, 1.0, 0, HashMap::from([("m".to_string(), 50)]))]))]);
    let summary = SpendSummary::of_range(&ledgers, date(9, 1), date(10, 1), date(10, 5), &santiago);
    assert_eq!(summary.days.len(), 30);
    assert!(summary.days.iter().all(|d| d.date == santiago.start_of_day(d.date)));
    assert_eq!(summary.days.iter().find(|d| d.date == worked).unwrap().tokens, 50);
}

#[test]
fn a_span_is_a_window_on_the_calendar_not_each_ledgers_last_few_rows() {
    // Codex was used today; Claude Code was last used a fortnight ago. Taking seven rows from each
    // would reach back fourteen days for one of them and draw nine bars for a seven-day span.
    let result = summary(
        vec![
            (Codex, ledger((0..7).map(|d| day(d, 100, 1.0)).collect())),
            (ClaudeCode, ledger((10..17).map(|d| day(d, 100, 1.0)).collect())),
        ],
        Some(7),
    );
    assert_eq!(result.days.len(), 7);
    // Only Codex's week is inside the window.
    assert_eq!(result.tokens, 700);
    assert_eq!(result.agents.iter().map(|a| a.agent).collect::<Vec<_>>(), [Codex]);
}

#[test]
fn quiet_days_are_in_the_series_so_the_chart_stays_a_calendar() {
    let result = summary(vec![(Codex, ledger(vec![day(0, 100, 1.0), day(6, 50, 0.5)]))], Some(7));
    assert_eq!(result.days.len(), 7);
    assert_eq!(result.active_days(), 2);
    assert_eq!(result.days.iter().filter(|d| d.tokens == 0).count(), 5);
}

#[test]
fn today_is_the_day_in_progress_not_the_last_twenty_four_hours() {
    // Yesterday evening is not part of today, however recent it is.
    let result = summary(vec![(Codex, ledger(vec![day(0, 100, 1.0), day(1, 900, 9.0)]))], Some(1));
    assert_eq!(result.days.len(), 1);
    assert_eq!(result.tokens, 100);
    assert_eq!(result.cost, 1.0);
}

#[test]
fn all_time_runs_from_the_earliest_day_to_today() {
    let result = summary(vec![(Codex, ledger(vec![day(9, 10, 1.0)]))], None);
    assert_eq!(result.days.len(), 10);
    assert_eq!(result.tokens, 10);
}

#[test]
fn a_ledger_that_cannot_be_priced_is_left_out_of_a_priced_total() {
    // Z.ai reports one token total per model and no money. Folding it in would put a figure on the
    // total that half of it cannot carry.
    let result = summary(
        vec![
            (Codex, ledger(vec![day(0, 100, 2.0)])),
            (ClaudeCode, ledger_with(vec![day(0, 900, 0.0)], Origin::ProviderStatistics, &[])),
        ],
        Some(7),
    );
    assert_eq!(result.tokens, 100);
    assert_eq!(result.cost, 2.0);
    assert_eq!(result.agents.iter().map(|a| a.agent).collect::<Vec<_>>(), [Codex]);
}

#[test]
fn an_agent_with_nothing_in_the_span_is_not_a_row() {
    let result = summary(vec![(Codex, ledger(vec![day(0, 100, 1.0)])), (ClaudeCode, ledger(vec![day(0, 0, 0.0)]))], Some(7));
    // A row reading zero is a row that has to be explained.
    assert_eq!(result.agents.iter().map(|a| a.agent).collect::<Vec<_>>(), [Codex]);
}

#[test]
fn agents_rank_by_tokens_heaviest_first() {
    let result = summary(vec![(Codex, ledger(vec![day(0, 100, 1.0)])), (ClaudeCode, ledger(vec![day(0, 400, 9.0)]))], Some(7));
    assert_eq!(result.agents.iter().map(|a| a.agent).collect::<Vec<_>>(), [ClaudeCode, Codex]);
    assert_eq!(result.tokens, 500);
    assert_eq!(result.cost, 10.0);
}

#[test]
fn a_model_used_by_two_agents_is_one_row_naming_both() {
    let result = summary(
        vec![
            (
                Codex,
                ledger_with(vec![day_with(0, 300, 1.0, &[("shared", 200), ("gpt", 100)])], Origin::LocalTranscripts, &[("shared", "Shared Model"), ("gpt", "GPT")]),
            ),
            (ClaudeCode, ledger_with(vec![day_with(0, 100, 1.0, &[("shared", 100)])], Origin::LocalTranscripts, &[("shared", "Shared Model")])),
        ],
        Some(7),
    );
    let shared = &result.models[0];
    assert_eq!(shared.name, "Shared Model");
    assert_eq!(shared.tokens, 300);
    // Both, and in a stable order: a row whose agents shuffle between reads looks like the data
    // moved.
    assert_eq!(shared.agents, [ClaudeCode, Codex]);
    assert!((shared.share - 0.75).abs() < 0.0001);
}

#[test]
fn one_raw_id_priced_in_one_source_and_unknown_only_in_another_is_one_model_row() {
    // The same model id reaches Pulse two ways: one source classified its work, another reported
    // only a bare total. Without the name lookup the unknown-only copy would group under its raw
    // id and the model would split into two rows.
    let prices: PriceTable = HashMap::from([("gpt-5".to_string(), ModelPrice::new(1_000.0, 10_000.0, Some(100.0), Some(1_000.0), Some("GPT-5")))]);
    let at = at_hour(today(), 10);
    let build = |record: AgentUsageRecord, namespace: &str| build_ledger(&[record], &prices, namespace, None, &calendar(), Origin::LocalTranscripts);
    let known = build(AgentUsageRecord::new(at, "gpt-5", TokenTally::new(1_000, 0, 0, 0)), "a");
    let unknown = build(AgentUsageRecord::new(at, "gpt-5", TokenTally::default()).unclassified(500), "b");

    let result = summary(vec![(Codex, known.clone()), (ClaudeCode, unknown.clone())], Some(7));
    assert_eq!(result.tokens, 1_500);
    assert_eq!(result.tally, TokenTally::new(1_000, 0, 0, 0));
    assert_eq!(result.unclassified_tokens, 500);
    assert!(result.has_token_breakdown);
    assert_eq!(result.models.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["GPT-5"]);
    assert_eq!(result.models[0].tokens, 1_500);
    assert_eq!(result.models[0].agents, [ClaudeCode, Codex]);
    // The day carries the unknown-price tokens beside the priced ones.
    let last = result.days.last().unwrap();
    assert_eq!(last.unpriced_tokens, 500);
    assert_eq!(last.unclassified_tokens, 500);
    assert_eq!(last.classified_tally(), Some(&TokenTally::new(1_000, 0, 0, 0)));

    let model = ModelSpendSummary::of(&HashMap::from([(Codex, known), (ClaudeCode, unknown)]), "GPT-5", Some(7), today(), &calendar());
    assert_eq!(model.tokens, 1_500);
    // 1,000 input at $1,000/M for the classified copy.
    assert_eq!(model.cost(), Some(1.0));
    assert_eq!(model.unpriced_tokens, 500);
    assert_eq!(model.tally, Some(TokenTally::new(1_000, 0, 0, 0)));
    assert_eq!(model.unclassified_tokens, 500);
    assert_eq!(model.agents.len(), 2);
}

#[test]
fn broken_category_details_cannot_cancel_across_two_agents() {
    let mut missing = LedgerDay::new(today(), 500, 0.0, 500, HashMap::from([("m".to_string(), 500)]));
    missing.tally = TokenTally::new(100, 0, 0, 0);
    let mut excess = LedgerDay::new(today(), 500, 0.0, 500, HashMap::from([("m".to_string(), 500)]));
    excess.tally = TokenTally::new(900, 0, 0, 0);
    let result = summary(vec![(Codex, ledger(vec![missing])), (ClaudeCode, ledger(vec![excess]))], Some(7));
    assert_eq!(result.tokens, 1_000);
    assert_eq!(result.tally.total(), 1_000);
    assert_eq!(result.unclassified_tokens, 0);
    assert!(!result.has_token_breakdown);
    assert!(!result.days.last().unwrap().has_token_breakdown());
    assert!(result.days.last().unwrap().classified_tally().is_none());
}

#[test]
fn unknown_only_work_sorts_after_measured_input_while_its_unclassified_count_remains_sortable() {
    let unknown = summary_day(today(), 500, 0.0, TokenTally::default(), 500);
    let zero = summary_day(ago(1), 10, 0.0, TokenTally::new(0, 0, 0, 10), 0);
    let mixed = summary_day(ago(2), 150, 0.0, TokenTally::new(100, 0, 0, 0), 50);
    let days = [unknown.clone(), zero.clone(), mixed.clone()];
    assert!(unknown.has_token_breakdown());
    assert!(unknown.classified_tally().is_none());
    let dates = |column, ascending| SpendSummary::sorted(&days, column, ascending, false).iter().map(|d| d.date).collect::<Vec<_>>();
    assert_eq!(dates(DayColumn::Fresh, true), [zero.date, mixed.date, unknown.date]);
    assert_eq!(dates(DayColumn::Fresh, false), [mixed.date, zero.date, unknown.date]);
    assert_eq!(dates(DayColumn::Unclassified, true), [zero.date, mixed.date, unknown.date]);
}

#[test]
fn adjacent_large_unclassified_counts_retain_their_exact_order() {
    let lower: i64 = 9_007_199_254_740_992;
    let newer = summary_day(today(), lower, 0.0, TokenTally::default(), lower);
    let older = summary_day(ago(1), lower + 1, 0.0, TokenTally::default(), lower + 1);
    let sorted = SpendSummary::sorted(&[older, newer], DayColumn::Unclassified, true, false);
    assert_eq!(sorted.iter().map(|d| d.tokens).collect::<Vec<_>>(), [lower, lower + 1]);
}

#[test]
fn the_busiest_day_is_the_busiest_across_agents_not_any_one_of_them() {
    // Neither agent's own heaviest day is the pair's heaviest.
    let result = summary(
        vec![(Codex, ledger(vec![day(0, 60, 1.0), day(1, 50, 1.0)])), (ClaudeCode, ledger(vec![day(1, 50, 1.0)]))],
        Some(7),
    );
    assert_eq!(result.busiest_day().unwrap().tokens, 100);
    assert_eq!(result.busiest_day().unwrap().date, ago(1));
}

#[test]
fn each_day_carries_its_own_split_summed_across_agents() {
    let mut codex = day(0, 300, 1.0);
    codex.tally = TokenTally::new(100, 50, 120, 30);
    let mut claude = day(0, 100, 2.0);
    claude.tally = TokenTally::new(10, 5, 80, 5);
    let result = summary(vec![(Codex, ledger(vec![codex])), (ClaudeCode, ledger(vec![claude]))], Some(7));
    let today = result.days.last().unwrap();
    // Both agents' work on one day is one row, split and all.
    assert_eq!(today.tally.input, 110);
    assert_eq!(today.tally.cache_read, 200);
    assert_eq!(today.tally.total(), 400);
}

#[test]
fn a_days_unknown_price_tokens_roll_up_into_its_day_and_month_rows() {
    let priced = LedgerDay::new(today(), 100, 1.0, 0, HashMap::from([("m".to_string(), 100)]));
    let unpriced = LedgerDay::new(today(), 300, 0.0, 300, HashMap::from([("mystery".to_string(), 300)]));
    let result = summary(vec![(Codex, ledger(vec![priced])), (ClaudeCode, ledger(vec![unpriced]))], Some(7));
    assert_eq!(result.days.last().unwrap().tokens, 400);
    assert_eq!(result.days.last().unwrap().unpriced_tokens, 300);
    assert_eq!(result.months.last().unwrap().unpriced_tokens, 300);
    // A quiet day carries a real zero rather than a missing figure.
    assert_eq!(result.days.first().unwrap().unpriced_tokens, 0);
}

#[test]
fn the_table_sorts_by_any_column_both_ways() {
    let days = [
        summary_day(today(), 10, 9.0, TokenTally::new(1, 0, 9, 0), 0),
        summary_day(ago(1), 900, 1.0, TokenTally::new(800, 0, 100, 0), 0),
    ];
    // Descending is the default, because the first thing anybody looks for in a table like this
    // is the biggest row.
    assert_eq!(SpendSummary::sorted(&days, DayColumn::Cost, false, false)[0].cost, 9.0);
    assert_eq!(SpendSummary::sorted(&days, DayColumn::Total, false, false)[0].tokens, 900);
    assert_eq!(SpendSummary::sorted(&days, DayColumn::Fresh, true, false)[0].tally.fresh(), 1);
    assert_eq!(SpendSummary::sorted(&days, DayColumn::Date, false, false)[0].date, today());
}

#[test]
fn a_day_the_table_shows_blank_sorts_last_whichever_way() {
    let whole = summary_day(today(), 10, 1.0, TokenTally::new(10, 0, 0, 0), 0);
    // 500 tokens, of which only 5 have a kind: its kind cells are blank.
    let partial = summary_day(ago(1), 500, 2.0, TokenTally::new(5, 0, 0, 0), 0);
    for ascending in [true, false] {
        let sorted = SpendSummary::sorted(&[partial.clone(), whole.clone()], DayColumn::Fresh, ascending, false);
        assert_eq!(sorted.last().unwrap().tokens, 500);
    }
}

fn session(name: &str, project: Option<&str>, ended_days_ago: i64, tokens: i64, cost: f64) -> Session {
    Session {
        id: format!("/tmp/{name}.jsonl"),
        name: name.to_string(),
        title: None,
        is_review: false,
        project: UsageProject::new(project),
        start: ago(ended_days_ago),
        end: ago(ended_days_ago),
        tokens,
        cost,
        unpriced_tokens: 0,
        slots: Vec::new(),
        days: Vec::new(),
    }
}

#[test]
fn a_session_is_in_the_span_by_when_it_ended() {
    let mut ledger = ledger(vec![day(0, 10, 1.0)]);
    ledger.sessions = vec![
        session("recent", Some("Pulse"), 1, 100, 1.0),
        // A conversation that finished a fortnight ago is not in a week.
        session("old", Some("Pulse"), 14, 900, 9.0),
    ];
    let result = summary(vec![(Codex, ledger)], Some(7));
    assert_eq!(result.sessions.iter().map(|s| s.session.name.as_str()).collect::<Vec<_>>(), ["recent"]);
}

#[test]
fn projects_roll_up_their_sessions_sessions_without_one_are_not_a_bucket() {
    let mut ledger = ledger(vec![day(0, 10, 1.0)]);
    ledger.sessions = vec![
        session("a", Some("Pulse"), 0, 100, 1.0),
        session("b", Some("Pulse"), 2, 300, 3.0),
        // Codex keeps no directory. Putting these in an "other" row would make the largest
        // project on the list a name for "unknown".
        session("c", None, 0, 900, 9.0),
    ];
    let result = summary(vec![(Codex, ledger)], Some(7));
    assert_eq!(result.projects.len(), 1);
    let project = &result.projects[0];
    assert_eq!(project.tokens, 400);
    assert_eq!(project.sessions, 2);
    // The most recent of its sessions, not the first one seen.
    assert_eq!(project.last_used, today());
    // Every session is still a row, project or no project.
    assert_eq!(result.sessions.len(), 3);
}

#[test]
fn a_streak_is_the_run_ending_at_the_most_recent_day() {
    // Worked today and the two days before; a gap; then four days.
    let worked = [0, 1, 2, 4, 5, 6, 7];
    let result = summary(vec![(Codex, ledger(worked.iter().map(|d| day(*d, 10, 1.0)).collect()))], Some(10));
    assert_eq!(result.current_streak, 3);
    assert_eq!(result.longest_streak, 4);
    assert_eq!(result.active_days(), 7);
}

#[test]
fn today_is_not_over_a_run_that_reached_yesterday_is_still_current_a_whole_day_off_ends_it() {
    let yesterday = summary(vec![(Codex, ledger((1..5).map(|d| day(d, 10, 1.0)).collect()))], Some(10));
    assert_eq!(yesterday.current_streak, 4);
    assert_eq!(yesterday.longest_streak, 4);

    let broken = summary(vec![(Codex, ledger((2..6).map(|d| day(d, 10, 1.0)).collect()))], Some(10));
    assert_eq!(broken.current_streak, 0);
    assert_eq!(broken.longest_streak, 4);
}

#[test]
fn streaks_count_the_whole_history_whatever_the_span() {
    let result = summary(vec![(Codex, ledger((0..20).map(|d| day(d, 10, 1.0)).collect()))], Some(1));
    assert_eq!(result.current_streak, 20);
    assert_eq!(result.longest_streak, 20);
    // What is added up is still the span's.
    assert_eq!(result.tokens, 10);
}

#[test]
fn the_peak_hour_comes_from_the_quarter_hour_buckets_not_the_days() {
    let afternoon = at_hour(today(), 16) + chrono::Duration::minutes(30);
    let morning = at_hour(today(), 9);
    let mut ledger = ledger(vec![day(0, 400, 1.0)]);
    ledger.slots = vec![Slot::new(morning, 100, 0.25), Slot::new(afternoon, 300, 0.75)];
    let result = summary(vec![(Codex, ledger)], Some(7));
    // A ledger day has already thrown the time of day away, so this can only come from the buckets.
    assert_eq!(result.peak_hour(), Some(16));
    assert_eq!(result.hours[&9], 100);
}

#[test]
fn nothing_to_add_up_is_empty_not_zero_shaped() {
    assert!(summary(vec![], Some(7)).is_empty());
    assert!(summary(vec![(Codex, Ledger::empty())], Some(7)).is_empty());
}

#[test]
fn a_session_resumed_past_midnight_is_counted_by_the_day_not_by_when_it_ended() {
    // The bug: one session with 900 tokens / $9 yesterday and 100 / $1 today put the whole 1000 /
    // $10 on the project while the Today total read 100 / $1.
    let late_yesterday = at_hour(ago(1), 23) + chrono::Duration::minutes(30);
    let noon_today = at_hour(today(), 12);
    let mut ledger = ledger(vec![day(1, 900, 9.0), day(0, 100, 1.0)]);
    ledger.sessions = vec![Session {
        title: Some("Long".into()),
        start: late_yesterday,
        end: noon_today,
        slots: vec![Slot::new(late_yesterday, 900, 9.0), Slot::new(noon_today, 100, 1.0)],
        ..session("long", Some("Pulse"), 0, 1000, 10.0)
    }];

    let today_only = summary(vec![(Codex, ledger.clone())], Some(1));
    assert_eq!(today_only.tokens, 100);
    assert_eq!(today_only.cost, 1.0);
    assert_eq!(today_only.projects[0].tokens, 100);
    assert!((today_only.projects[0].cost - 1.0).abs() < 1e-9);
    // The row carries the span's portion, so the list and the totals above it are the same
    // arithmetic.
    assert_eq!(today_only.sessions[0].session.tokens, 100);

    // The whole session is back once the span reaches before it.
    let week = summary(vec![(Codex, ledger)], Some(7));
    assert_eq!(week.projects[0].tokens, 1000);
    assert!((week.projects[0].cost - 10.0).abs() < 1e-9);
}

#[test]
fn a_session_resumed_over_days_contributes_only_the_days_in_the_window() {
    let slots: Vec<Slot> = (0..3).map(|offset| Slot::new(at_hour(ago(offset), 10), 100, 1.0)).collect();
    let mut ledger = ledger((0..3).map(|d| day(d, 100, 1.0)).collect());
    ledger.sessions = vec![Session {
        start: slots[2].start,
        end: slots[0].start,
        slots: slots.clone(),
        ..session("resumed", Some("Pulse"), 0, 300, 3.0)
    }];

    let week = summary(vec![(Codex, ledger.clone())], Some(7));
    assert_eq!(week.projects[0].tokens, 300);
    assert_eq!(week.sessions.len(), 1);

    let today_only = summary(vec![(Codex, ledger)], Some(1));
    assert_eq!(today_only.tokens, 100);
    assert_eq!(today_only.projects[0].tokens, 100);
    assert_eq!(today_only.projects[0].sessions, 1);
    assert_eq!(today_only.sessions.len(), 1);
}

#[test]
fn the_project_total_is_the_sum_of_the_in_span_session_buckets() {
    // Two sessions in one project, one straddling the cutoff. Nothing is prorated: the project's
    // money is the sum of the buckets, the same number the day rows add up to.
    let yesterday_late = at_hour(ago(1), 23);
    let morning = at_hour(today(), 9);
    let afternoon = at_hour(today(), 15);
    let mut ledger = ledger(vec![day(1, 500, 5.0), day(0, 300, 3.0)]);
    ledger.sessions = vec![
        Session {
            start: yesterday_late,
            end: afternoon,
            slots: vec![Slot::new(yesterday_late, 500, 5.0), Slot::new(afternoon, 100, 1.0)],
            ..session("a", Some("Pulse"), 0, 600, 6.0)
        },
        Session { start: morning, end: morning, slots: vec![Slot::new(morning, 200, 2.0)], ..session("b", Some("Pulse"), 0, 200, 2.0) },
    ];
    let result = summary(vec![(Codex, ledger)], Some(1));
    assert_eq!(result.projects.iter().map(|p| p.tokens).sum::<i64>(), result.tokens);
    assert_eq!(result.projects[0].cost, result.cost);
    assert_eq!(result.projects[0].tokens, 300);
}

#[test]
fn a_calendar_range_cuts_days_slots_and_sessions_and_pads_from_its_start() {
    let first = ago(5);
    let end = ago(2);
    let mut ledger = ledger((0..8).map(|d| day(d, 10, 1.0)).collect());
    ledger.slots = (0..8).map(|d| Slot::new(at_hour(ago(d), 9), 10, 1.0)).collect();
    let ledgers = HashMap::from([(Codex, ledger)]);
    let result = SpendSummary::of_range(&ledgers, first, end, today(), &calendar());
    assert_eq!(result.days.len(), 3);
    assert_eq!(result.days[0].date, first);
    assert_eq!(result.tokens, 30);
    assert_eq!(result.hours[&9], 30);
    // An end not after the start is no days at all; streaks stay whole-history.
    let none = SpendSummary::of_range(&ledgers, end, first, today(), &calendar());
    assert!(none.days.is_empty());
    assert_eq!(none.tokens, 0);
    assert_eq!(none.current_streak, 8);
}
