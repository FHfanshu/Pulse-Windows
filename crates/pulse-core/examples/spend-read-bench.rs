//! Read-only local-store timing; prints aggregate counts, never message bodies.
use pulse_core::spend::{
    agent_cache::Stamp,
    sources::{read_agent, SpendAgent},
    transcripts::Sources,
    Calendar, ModelPrices,
};
use std::{path::PathBuf, time::Instant};
fn main() {
    let home = PathBuf::from(std::env::var_os("USERPROFILE").expect("USERPROFILE"));
    let sources = Sources::from_env(home);
    let cache = tempfile::tempdir().unwrap();
    let price_directory = std::env::var_os("PULSE_BENCH_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(pulse_core::paths::data_dir);
    let prices = ModelPrices::cached(&price_directory);
    println!("price entries={}", prices.len());
    for raw in std::env::args().skip(1) {
        let agent = SpendAgent::from_raw(&raw).expect("registered agent id");
        let roots: Vec<_> = agent.inputs(&sources).into_iter().filter(|p| p.exists()).collect();
        let start = Instant::now();
        let _ = Stamp::of(&roots, &prices);
        let stamp = start.elapsed();
        let start = Instant::now();
        let records = agent.source().records(&roots);
        let reading = start.elapsed();
        let tokens: i64 = records.iter().map(|r| r.tally.total() + r.unclassified_tokens).sum();
        drop(records);
        let start = Instant::now();
        let first = read_agent(agent, &sources, cache.path(), &Calendar::local(), &prices);
        let cold = start.elapsed();
        let start = Instant::now();
        let warm = read_agent(agent, &sources, cache.path(), &Calendar::local(), &prices);
        println!(
            "{raw}: stamp={stamp:?} records={reading:?} cold={cold:?} warm={:?} tokens={} sessions={} raw_tokens={tokens} same_tokens={} cache_cost_delta={:e}",
            start.elapsed(),
            first.all_time().tokens,
            first.sessions.len(),
            first.all_time().tokens == warm.all_time().tokens,
            (first.all_time().cost - warm.all_time().cost).abs()
        );
    }
}
