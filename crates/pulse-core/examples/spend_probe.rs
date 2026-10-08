use pulse_core::spend::*;
use pulse_core::Provider;

fn main() {
    let home = std::path::PathBuf::from(std::env::var("USERPROFILE").unwrap());
    let cache = std::env::temp_dir().join("spend-probe-cache");
    let sources = transcripts::Sources::from_env(&home);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let prices = rt.block_on(ModelPrices::new(std::env::temp_dir().join("spend-probe-prices")).prices());
    println!("prices: {}", prices.len());
    for p in [Provider::ClaudeCode, Provider::Codex] {
        let t = std::time::Instant::now();
        let l = read_ledger_with(p, &sources, &cache, &Calendar::local(), &prices, chrono::Utc::now()).unwrap();
        let tot = l.all_time();
        println!(
            "{:?}: {:?} days={} sessions={} tokens={} cost={:.2} unpriced={} in {:?}",
            p,
            l.earliest,
            l.days.len(),
            l.sessions.len(),
            tot.tokens,
            tot.cost,
            tot.unpriced,
            t.elapsed()
        );
        println!("  hit rate 31d: {:?} top: {:?}", l.cache_hit_rate(31, chrono::Utc::now()), l.top_model(31, chrono::Utc::now()));
        let pc = prompt_cache(p, &home, chrono::Utc::now());
        println!("  prompt cache live={} lapsed={:?}", pc.live.len(), pc.last_lapsed.map(|l| l.expires_at));
    }
}
