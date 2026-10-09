//! The shared path every record source goes through: the whole-ledger cache and the card.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde_json::json;

use super::*;
use crate::spend::prices::ModelPrice;
use crate::spend::{read_card_ledger_with, supports_card};

fn turn(input: i64) -> String {
    json!({"timestamp":1_789_372_800,"params":{"update":{"sessionUpdate":"turn_completed","usage":{"modelUsage":{"priced":{"inputTokens":input,"outputTokens":0}}}}}})
        .to_string()
}

fn grok_log(home: &Path) -> PathBuf {
    let run = home.join(".grok").join("sessions").join("%2Fwork%2FPulse").join("r1");
    std::fs::create_dir_all(&run).unwrap();
    run.join("updates.jsonl")
}

fn read(home: &Path, cache: &Path, prices: &PriceTable) -> Ledger {
    read_agent(SpendAgent::Grok, &Sources::new(home), cache, &Calendar::utc(2), prices)
}

#[test]
fn a_finished_ledger_is_served_from_the_cache_until_a_file_or_the_prices_change() {
    let home = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let log = grok_log(home.path());
    std::fs::write(&log, turn(100)).unwrap();
    let none = PriceTable::new();
    assert_eq!(read(home.path(), cache.path(), &none).all_time().tokens, 100);
    assert!(cache.path().join(format!("agent-{}-grok.json", crate::spend::agent_cache::VERSION)).exists());

    // Same size and modification time, different contents: only the cache still knows the
    // original number, which makes its reuse observable.
    let stamp = std::fs::metadata(&log).unwrap().modified().unwrap();
    std::fs::write(&log, turn(999)).unwrap();
    std::fs::OpenOptions::new().write(true).open(&log).unwrap().set_modified(stamp).unwrap();
    assert_eq!(read(home.path(), cache.path(), &none).all_time().tokens, 100);

    // A file that grew is read again.
    std::fs::write(&log, format!("{}\n{}", turn(999), turn(1))).unwrap();
    assert_eq!(read(home.path(), cache.path(), &none).all_time().tokens, 1_000);

    // A changed price table is read again too, so money follows the table.
    let priced: PriceTable = HashMap::from([("priced".to_string(), ModelPrice::new(1_000_000.0, 0.0, None, None, None))]);
    assert_eq!(read(home.path(), cache.path(), &none).all_time().cost, 0.0);
    assert_eq!(read(home.path(), cache.path(), &priced).all_time().cost, 1_000.0);
    assert_eq!(read(home.path(), cache.path(), &none).all_time().cost, 0.0);
}

/// Not a test: `cargo test -p pulse-core real_stores -- --ignored --nocapture` reads this PC's own
/// stores, read-only, into a throwaway cache folder, and prints the totals. For eyeballing a
/// reader against a real machine; nothing is asserted about what is found.
#[test]
#[ignore]
fn real_stores_on_this_pc() {
    let home = PathBuf::from(std::env::var_os("USERPROFILE").expect("USERPROFILE"));
    let cache = tempfile::tempdir().unwrap();
    let sources = Sources::from_env(&home);
    for agent in SpendAgent::ALL {
        let started = std::time::Instant::now();
        let ledger = read_agent(*agent, &sources, cache.path(), &Calendar::local(), &PriceTable::new());
        println!(
            "{:<12} present={:<5} tokens={:>14} sessions={:>5} days={:>4} ({:.1}s)",
            agent.raw(),
            agent.is_present(&sources),
            ledger.all_time().tokens,
            ledger.sessions.len(),
            ledger.days.len(),
            started.elapsed().as_secs_f64()
        );
    }
}

#[test]
fn a_store_that_appears_later_is_noticed_and_one_that_vanishes_is_an_empty_ledger() {
    let home = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let none = PriceTable::new();
    assert_eq!(read(home.path(), cache.path(), &none).all_time().tokens, 0);
    std::fs::write(grok_log(home.path()), turn(5)).unwrap();
    assert_eq!(read(home.path(), cache.path(), &none).all_time().tokens, 5);
    std::fs::remove_dir_all(home.path().join(".grok")).unwrap();
    assert_eq!(read(home.path(), cache.path(), &none).all_time().tokens, 0);
}

#[test]
fn a_card_adds_up_its_agents_and_a_provider_nothing_borrows_has_none() {
    let home = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    std::fs::write(grok_log(home.path()), turn(42)).unwrap();
    let (sources, calendar, none, now) = (Sources::new(home.path()), Calendar::utc(2), PriceTable::new(), Utc::now());
    let ledger = read_card_ledger_with(Provider::Grok, &sources, cache.path(), &calendar, &none, now).unwrap();
    assert_eq!(ledger.all_time().tokens, 42);
    assert_eq!(ledger.read_at, Some(now));
    assert!(supports_card(Provider::Grok) && supports_card(Provider::Codex) && supports_card(Provider::OpenCodeGo));
    assert!(!supports_card(Provider::Zai));
    assert!(read_card_ledger_with(Provider::Zai, &sources, cache.path(), &calendar, &none, now).is_none());
    // A provider with transcripts of its own reads those, not an agent card.
    assert_eq!(read_card_ledger_with(Provider::Codex, &sources, cache.path(), &calendar, &none, now).unwrap().all_time().tokens, 0);
}
