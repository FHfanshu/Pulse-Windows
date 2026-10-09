//! Ported from upstream UsageArchiveTests (the `AgentArchive` half): a store that deletes its
//! records keeps them in Token spend, a copy is not counted twice, and the high-water rules.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use chrono::{DateTime, Duration, Utc};

use super::agent_archive::AgentArchive;
use super::agent_cache;
use super::calendar::Calendar;
use super::ledger::{Ledger, Origin};
use super::prices::{ModelPrice, PriceTable};
use super::record::{build_ledger, AgentUsageRecord};
use super::sources::{read_agent, SpendAgent};
use super::tally::TokenTally;
use super::transcripts::Sources;

fn prices() -> PriceTable {
    HashMap::from([("gpt-5".to_string(), ModelPrice::new(1.25, 10.0, Some(0.125), None, Some("GPT-5")))])
}

fn calendar() -> Calendar {
    Calendar::utc(2)
}

fn buddy(id: &str, session: &str, milliseconds: i64, input: i64) -> String {
    format!(
        r#"{{"id":"{id}","type":"message","role":"assistant","timestamp":{milliseconds},"sessionId":"{session}","message":{{"model":"gpt-5","usage":{{"input_tokens":{input},"output_tokens":200}}}}}}"#
    )
}

fn write(path: &Path, line: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, format!("{line}\n")).unwrap();
}

fn workbuddy(home: &Path) -> Ledger {
    let agent = SpendAgent::from_raw("workBuddy").unwrap();
    read_agent(agent, &Sources::new(home), &home.join("cache"), &calendar(), &prices())
}

#[test]
fn a_store_that_deletes_a_session_file_keeps_the_day_money_hours_and_row_and_a_copy_is_counted_once() {
    let home = tempfile::tempdir().unwrap();
    let store = home.path().join(".workbuddy/projects");
    let day = 1_780_000_000_000;
    // `s1` is written twice (a copy of its file in another folder of the store) and the reader
    // folds the copy by message id.
    write(&store.join("a/s1.jsonl"), &buddy("m1", "s1", day, 1_000));
    write(&store.join("b/s1.jsonl"), &buddy("m1", "s1", day, 1_000));
    write(&store.join("a/s2.jsonl"), &buddy("m2", "s2", day + 86_400_000 * 3, 400));

    let before = workbuddy(home.path());
    assert_eq!(before.all_time().tokens, 1_200 + 600);
    assert!(before.all_time().cost > 0.0);

    // One copy gone: nothing was lost, so nothing is added.
    std::fs::remove_file(store.join("a/s1.jsonl")).unwrap();
    let copy_gone = workbuddy(home.path());
    assert_eq!(copy_gone.days, before.days);
    assert_eq!(copy_gone.all_time().tokens, 1_200 + 600);

    // Both gone: the day is kept, priced, in its quarter-hour, with its row.
    std::fs::remove_file(store.join("b/s1.jsonl")).unwrap();
    let deleted = workbuddy(home.path());
    assert_eq!(deleted.days, before.days);
    assert_eq!(deleted.slots, before.slots);
    let ids = |l: &Ledger| l.sessions.iter().map(|s| s.id.clone()).collect::<HashSet<_>>();
    assert_eq!(ids(&deleted), ids(&before));
    assert_eq!(deleted.earliest, before.earliest);

    // Read again, from the cache and the archive alone.
    assert_eq!(workbuddy(home.path()).days, before.days);
}

/// One session's work as a ledger, built by the shared builder.
fn ledger(records: &[AgentUsageRecord]) -> Ledger {
    build_ledger(records, &prices(), "test", None, &calendar(), Origin::LocalTranscripts)
}

fn record(session: &str, id: &str, at: DateTime<Utc>, input: i64) -> AgentUsageRecord {
    let mut record = AgentUsageRecord::new(at, "gpt-5", TokenTally::new(input, 0, 0, 100)).session(session);
    record.deduplication_id = Some(id.to_string());
    record
}

fn start() -> DateTime<Utc> {
    DateTime::from_timestamp(1_780_000_000, 0).unwrap()
}

#[test]
fn a_deleted_session_is_kept_once_and_a_refiled_one_is_not() {
    let day = start();
    let first = ledger(&[record("s1", "m1", day, 1_000), record("s2", "m2", day + Duration::hours(1), 1_000)]);
    let mut archive = AgentArchive::default();
    assert!(archive.absorb(&first, None, &calendar()));

    // `s1`'s message now filed under `s2`: the day is whole, and `s1`'s old row would count it
    // twice.
    let refiled = ledger(&[record("s2", "m1", day, 1_000), record("s2", "m2", day + Duration::hours(1), 1_000)]);
    archive.absorb(&refiled, Some(&first), &calendar());
    let shown = archive.merged(&refiled, &prices(), None, &calendar());
    assert_eq!(shown.days, refiled.days);
    assert_eq!(shown.sessions.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), refiled.sessions.iter().map(|s| s.id.as_str()).collect::<Vec<_>>());
    assert_eq!(shown.sessions.len(), 1);

    // `s1` gone with its work: kept, and shown.
    let mut gone = AgentArchive::default();
    gone.absorb(&first, None, &calendar());
    let without = ledger(&[record("s2", "m2", day + Duration::hours(1), 1_000)]);
    gone.absorb(&without, Some(&first), &calendar());
    let kept = gone.merged(&without, &prices(), None, &calendar());
    assert_eq!(kept.days, first.days);
    assert_eq!(kept.slots, first.slots);
    let ids: HashSet<String> = kept.sessions.iter().map(|s| s.id.clone()).collect();
    assert_eq!(ids, first.sessions.iter().map(|s| s.id.clone()).collect());
}

#[test]
fn a_partial_read_lowers_nothing_and_shows_the_marks() {
    let day = start();
    let full = ledger(&[record("s1", "m1", day, 1_000), record("s2", "m2", day + Duration::days(1), 500)]);
    let mut archive = AgentArchive::default();
    archive.absorb(&full, None, &calendar());
    let saved = archive.clone();
    // An unsettled read is never absorbed; it is only shown over the marks.
    let partial = ledger(&[record("s2", "m2", day + Duration::days(1), 500)]);
    let shown = archive.merged(&partial, &prices(), None, &calendar());
    assert_eq!(shown.days, full.days);
    assert_eq!(archive, saved);
}

#[test]
fn marks_from_other_readers_stand_only_where_the_current_ones_see_nothing() {
    let day = start();
    let mut old = AgentArchive::default();
    old.absorb(&ledger(&[record("s1", "m1", day, 9_000), record("s3", "m3", day + Duration::days(2), 1_000)]), None, &calendar());
    old.readings = agent_cache::VERSION - 1;
    let newer = ledger(&[record("s3", "m3", day + Duration::days(2), 10)]);
    old.absorb(&newer, None, &calendar());
    let reread = old.merged(&newer, &prices(), None, &calendar());
    assert_eq!(reread.all_time().tokens, 9_100 + 110);
}

#[test]
fn a_change_of_time_zone_adds_nothing_and_the_archive_round_trips() {
    let day = start();
    let live = ledger(&[record("s1", "m1", day, 1_000), record("s2", "m2", day + Duration::hours(1), 1_000)]);
    let mut archive = AgentArchive::default();
    archive.absorb(&live, None, &calendar());
    // As if the marks had been cut half a world away.
    archive.time_zone = "50400,50400".to_string();
    let shown = archive.merged(&live, &prices(), None, &calendar());
    assert_eq!(shown.days, live.days);
    assert_eq!(shown.slots, live.slots);
    let decoded: AgentArchive = serde_json::from_slice(&serde_json::to_vec(&archive).unwrap()).unwrap();
    assert_eq!(decoded, archive);
}

#[test]
fn aggregate_day_work_and_unclassified_tokens_are_kept_by_day() {
    let day = start();
    let mut aggregate = AgentUsageRecord::new(day, "gpt-5", TokenTally::new(300, 0, 0, 30)).session("agg").aggregate(true);
    aggregate.deduplication_id = Some("agg".into());
    let with_aggregate = ledger(&[aggregate, record("s1", "m1", day, 1_000)]);
    let mut archive = AgentArchive::default();
    archive.absorb(&with_aggregate, None, &calendar());
    let only_event = ledger(&[record("s1", "m1", day, 1_000)]);
    archive.absorb(&only_event, Some(&with_aggregate), &calendar());
    let shown = archive.merged(&only_event, &prices(), None, &calendar());
    assert_eq!(shown.all_time().tokens, with_aggregate.all_time().tokens);
    assert!((shown.all_time().cost - with_aggregate.all_time().cost).abs() < 1e-12);
    assert_eq!(shown.days[0].model_tallies, with_aggregate.days[0].model_tallies);
}

#[test]
fn an_unreadable_archive_is_neither_used_nor_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let agent = SpendAgent::from_raw("workBuddy").unwrap();
    assert_eq!(AgentArchive::load(agent, dir.path()), Some(AgentArchive::default()));
    std::fs::write(dir.path().join(AgentArchive::file_name(agent)), b"{ later").unwrap();
    assert_eq!(AgentArchive::load(agent, dir.path()), None);
    assert!(!AgentArchive::keeps(SpendAgent::from_raw("devinDesktop").unwrap()));
}


#[test]
fn a_failed_archive_write_keeps_the_previous_cache_for_a_retry() {
    let home = tempfile::tempdir().unwrap();
    let store = home.path().join(".workbuddy/projects");
    let cache_dir = home.path().join("cache");
    let agent = SpendAgent::from_raw("workBuddy").unwrap();
    let day = 1_780_000_000_000;
    write(&store.join("s1.jsonl"), &buddy("m1", "s1", day, 1_000));
    write(&store.join("s2.jsonl"), &buddy("m2", "s2", day + 86_400_000, 400));
    let before = workbuddy(home.path());
    // Model first use after upgrade: a pre-existing cache, but no archive yet.
    std::fs::remove_file(cache_dir.join(AgentArchive::file_name(agent))).unwrap();
    // The archive's temporary output cannot be written; the live cache remains writable.
    let blocker = cache_dir.join(AgentArchive::file_name(agent)).with_extension(format!("tmp-{}", std::process::id()));
    std::fs::create_dir(&blocker).unwrap();
    std::fs::remove_file(store.join("s1.jsonl")).unwrap();
    assert_eq!(workbuddy(home.path()).all_time().tokens, before.all_time().tokens);
    std::fs::remove_dir(&blocker).unwrap();
    assert_eq!(workbuddy(home.path()).all_time().tokens, before.all_time().tokens,
        "A failed archive write must leave the previous cache available for retry");
}

#[test]
fn a_partial_cached_read_raises_no_marks() {
    let home = tempfile::tempdir().unwrap();
    let store = home.path().join(".workbuddy/projects");
    let path = store.join("s1.jsonl");
    let day = 1_780_000_000_000;
    write(&path, &buddy("m1", "s1", day, 1_000));
    let agent = SpendAgent::from_raw("workBuddy").unwrap();
    let mut partial = ledger(&[record("s1", "m1", start(), 1_000)]);
    partial.has_read_limitations = true;
    let roots: Vec<_> = agent.source().inputs(&Sources::new(home.path())).into_iter().filter(|p| p.exists()).collect();
    let cache_dir = home.path().join("cache");
    agent_cache::save(agent, &cache_dir, &agent_cache::Stamp::of(&roots, &prices()), &partial);
    let shown = workbuddy(home.path());
    assert!(shown.has_read_limitations);
    let archive = AgentArchive::load(agent, &cache_dir).unwrap();
    assert_eq!(archive, AgentArchive::default(), "Incomplete cached reads must not create permanent high-water marks");
}


#[test]
fn a_removed_store_root_still_shows_its_archived_history() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join(".workbuddy");
    let store = root.join("projects");
    let path = store.join("s1.jsonl");
    write(&path, &buddy("m1", "s1", 1_780_000_000_000, 1_000));
    let before = workbuddy(home.path());
    assert_eq!(before.all_time().tokens, 1200);
    std::fs::remove_file(&path).unwrap();
    std::fs::remove_dir(&store).unwrap();
    std::fs::remove_dir(&root).unwrap();
    assert_eq!(workbuddy(home.path()).all_time().tokens, before.all_time().tokens,
        "Deleting the last store directory must not hide the durable archive");
}
