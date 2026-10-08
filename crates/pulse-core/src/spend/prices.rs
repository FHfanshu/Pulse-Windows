// Ported from upstream Sources/Pulse/Usage/ModelPrices.swift and ModelPriceLookup.swift.
//! What one model charges, per million tokens: the models.dev table, kept on disk for a day,
//! and the spelling rules that find a model in it.
//!
//! Missing rates stay `None` rather than falling back to a plausible number: a model Pulse has
//! no price for is counted and left out of the money, so a figure on screen is never part
//! guesswork.

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Model id (or `vendor|id` for a plan vendor) to its rates.
pub type PriceTable = HashMap<String, ModelPrice>;

/// What one model charges, per million tokens.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelPrice {
    pub input: f64,
    pub output: f64,
    #[serde(default)]
    pub cache_read: Option<f64>,
    #[serde(default)]
    pub cache_write: Option<f64>,
    /// How the provider writes the model's name: "GPT-5.6 Sol" rather than `gpt-5.6-sol`.
    #[serde(default)]
    pub name: Option<String>,
    /// The rates a request pays once its context passes a size, lowest threshold first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tiers: Option<Vec<ContextTier>>,
}

/// One long-context tier: a request whose context (fresh input, cache read and cache write
/// together) is over `threshold` tokens is billed at these rates, all of it, not just the part
/// past the line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextTier {
    pub threshold: i64,
    pub input: f64,
    pub output: f64,
    #[serde(default)]
    pub cache_read: Option<f64>,
    #[serde(default)]
    pub cache_write: Option<f64>,
}

impl ContextTier {
    /// The tier as a price of its own; a cache rate it does not state falls back to its own
    /// input rate, as the base rates do.
    pub fn price(&self) -> ModelPrice {
        ModelPrice::new(self.input, self.output, self.cache_read, self.cache_write, None)
    }
}

impl ModelPrice {
    pub fn new(input: f64, output: f64, cache_read: Option<f64>, cache_write: Option<f64>, name: Option<&str>) -> Self {
        Self { input, output, cache_read, cache_write, name: name.map(str::to_string), tiers: None }
    }

    pub fn with_tiers(mut self, tiers: Vec<ContextTier>) -> Self {
        self.tiers = Some(tiers);
        self
    }

    /// The tier a request whose context fell in `band` pays: the highest threshold at or under
    /// the band's floor.
    pub fn tier_for_band(&self, band: i64) -> Option<&ContextTier> {
        self.tiers.as_ref()?.iter().filter(|t| t.threshold <= band).max_by_key(|t| t.threshold)
    }
}

/// Providers whose models Pulse can see usage for, in priority order. `github-copilot` is left
/// out because it re-lists other vendors' models rather than carrying rates of its own; the one
/// real collision is `glm-5.2`, which the order settles.
const PROVIDERS: [&str; 12] = [
    "anthropic", "openai", "xai", "moonshotai", "zhipuai", "minimax", "deepseek", "google", "xiaomi", "alibaba",
    "mistral", "meta",
];

/// Plan vendors an agent can be priced against when no first-party provider publishes the model
/// at all. Stored namespaced (`vendor|id`) so a vendor's price is never found by a lookup that
/// did not ask for that vendor.
const VENDORS: [&str; 3] = ["opencode-go", "kilo", "cline-pass"];

pub const VENDOR_SEPARATOR: char = '|';

pub fn vendor_key(vendor: &str, model: &str) -> String {
    format!("{vendor}{VENDOR_SEPARATOR}{model}")
}

const SOURCE: &str = "https://models.dev/api.json";
const REFRESH_AFTER: Duration = Duration::seconds(24 * 3600);
const RETRY_AFTER: Duration = Duration::seconds(5 * 60);

fn number(value: Option<&Value>) -> Option<f64> {
    value.and_then(Value::as_f64)
}

/// `cost.tiers[]` of `tier.type == "context"`, or `context_over_200k` on its own. models.dev
/// writes a threshold both as `272000` and `272001` ("over 272,000" either way), so the second
/// is read as the first.
pub fn tiers_in(cost: &Value) -> Option<Vec<ContextTier>> {
    fn tier(rates: &Value, over: i64) -> Option<ContextTier> {
        Some(ContextTier {
            threshold: over,
            input: number(rates.get("input"))?,
            output: number(rates.get("output"))?,
            cache_read: number(rates.get("cache_read")),
            cache_write: number(rates.get("cache_write")),
        })
    }

    let mut found = Vec::new();
    for entry in cost.get("tiers").and_then(Value::as_array).into_iter().flatten() {
        let Some(shape) = entry.get("tier") else { continue };
        if shape.get("type").and_then(Value::as_str) != Some("context") {
            continue;
        }
        let Some(size) = shape.get("size").and_then(|s| s.as_i64().or_else(|| s.as_f64().map(|f| f as i64))) else {
            continue;
        };
        if size <= 1 {
            continue;
        }
        let threshold = if (size - 1) % 1_000 == 0 { size - 1 } else { size };
        if let Some(tier) = tier(entry, threshold) {
            found.push(tier);
        }
    }
    if found.is_empty() {
        if let Some(tier) = cost.get("context_over_200k").and_then(|over| tier(over, 200_000)) {
            found.push(tier);
        }
    }
    if found.is_empty() {
        None
    } else {
        found.sort_by_key(|t| t.threshold);
        Some(found)
    }
}

fn price_from(model: &Value) -> Option<ModelPrice> {
    let cost = model.get("cost")?;
    let input = number(cost.get("input"))?;
    let output = number(cost.get("output"))?;
    Some(ModelPrice {
        input,
        output,
        cache_read: number(cost.get("cache_read")),
        cache_write: number(cost.get("cache_write")),
        name: model.get("name").and_then(Value::as_str).map(str::to_string),
        tiers: tiers_in(cost),
    })
}

/// The table out of a models.dev `api.json` document: the first-party providers in priority
/// order (the first wins a shared id), then the plan vendors, namespaced, and only for ids the
/// first-party providers did not already price.
pub fn parse_models_dev(root: &Value) -> Option<PriceTable> {
    let mut prices = PriceTable::new();
    for provider in PROVIDERS {
        let Some(models) = root.get(provider).and_then(|p| p.get("models")).and_then(Value::as_object) else {
            continue;
        };
        for (id, model) in models {
            if prices.contains_key(id) {
                continue;
            }
            if let Some(price) = price_from(model) {
                prices.insert(id.clone(), price);
            }
        }
    }
    for vendor in VENDORS {
        let Some(models) = root.get(vendor).and_then(|p| p.get("models")).and_then(Value::as_object) else {
            continue;
        };
        for (id, model) in models {
            if prices.contains_key(id) {
                continue;
            }
            if let Some(price) = price_from(model) {
                prices.insert(vendor_key(vendor, id), price);
            }
        }
    }
    if prices.is_empty() {
        None
    } else {
        Some(prices)
    }
}

// MARK: - Spelling

/// The price for a model id, allowing for the fact that the agents do not all spell one the same
/// way. Aliases, not fuzzy matching: every rule is one product's known habit, written out,
/// because the failure mode of a loose match is a model priced at another model's rate.
pub fn price_for<'a>(model: &str, table: &'a PriceTable, vendor: Option<&str>) -> Option<&'a ModelPrice> {
    if let Some(price) = table.get(model) {
        return Some(price);
    }
    let key = resolve_key(
        model,
        vendor,
        &|id| table.contains_key(id).then(|| id.to_string()),
        &|lowered| table.keys().filter(|k| k.to_lowercase() == lowered).min().cloned(),
    )?;
    table.get(&key)
}

/// Shared by single lookups and the indexed lookup. The order is significant: every first-party
/// spelling precedes every vendor rate. Returns the table key that matched.
pub fn resolve_key(
    model: &str,
    vendor: Option<&str>,
    exact: &dyn Fn(&str) -> Option<String>,
    folded: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    if let Some(key) = exact(model).or_else(|| folded(&model.to_lowercase())) {
        return Some(key);
    }
    let candidates = aliases_for(model);
    for candidate in &candidates {
        if let Some(key) = exact(candidate).or_else(|| folded(&candidate.to_lowercase())) {
            return Some(key);
        }
    }

    // Only now, and only for the vendor asked about: the plan the tokens were bought on is the
    // last word, never the first.
    let vendor = vendor?;
    let namespaced = vendor_key(vendor, model);
    if let Some(key) = exact(&namespaced) {
        return Some(key);
    }
    if let Some(key) = folded(&namespaced.to_lowercase()) {
        return Some(key);
    }
    for candidate in &candidates {
        if let Some(key) = exact(&vendor_key(vendor, candidate)) {
            return Some(key);
        }
    }
    None
}

/// Spellings to try for one id, most specific first.
pub fn aliases_for(model: &str) -> Vec<String> {
    let mut candidates: Vec<String> = Vec::new();

    // Grok Build tags its own build of a model: `grok-4.6-build` is xAI's `grok-4.6`.
    if let Some(base) = model.strip_suffix("-build") {
        if model != "grok-build-0.1" {
            candidates.push(base.to_string());
        }
    }

    // Devin's CLI writes the version with dashes and an effort on the end:
    // `gpt-5-6-sol-medium` is OpenAI's `gpt-5.6-sol`.
    for effort in ["-medium", "-high", "-low", "-minimal"] {
        if let Some(base) = model.strip_suffix(effort) {
            candidates.push(base.to_string());
            candidates.push(dotted(base));
        }
    }
    candidates.push(dotted(model));

    // A context window on the end is the same model with more room: `k3-256k` is `kimi-k3`.
    if let Some(base) = strip_context_tag(model) {
        candidates.push(base.to_string());
        candidates.extend(aliases_for(base));
    }

    // Kimi's CLI abbreviates: `k2p6` is `kimi-k2.6`, `k3` is `kimi-k3`.
    if model.starts_with('k') && model.chars().skip(1).all(|c| c.is_numeric() || c == 'p') {
        candidates.push(format!("kimi-{}", model.replace('p', ".")));
    }

    candidates.retain(|c| c != model);
    candidates
}

/// `name-256k` / `name-1m`: the model with a trailing `-<digits><k|m>` removed.
fn strip_context_tag(model: &str) -> Option<&str> {
    let body = model.strip_suffix(['k', 'K', 'm', 'M'])?;
    let digits = body.trim_end_matches(|c: char| c.is_ascii_digit());
    if digits.len() == body.len() {
        return None;
    }
    digits.strip_suffix('-')
}

/// `gpt-5-6-sol` -> `gpt-5.6-sol`: a digit, a dash, a digit is a version number somebody spelled
/// with the wrong separator. Two words joined by a dash are left alone.
fn dotted(model: &str) -> String {
    let chars: Vec<char> = model.chars().collect();
    let mut out = String::with_capacity(model.len());
    for (index, &c) in chars.iter().enumerate() {
        if c == '-' && index > 0 && index + 1 < chars.len() && chars[index - 1].is_numeric() && chars[index + 1].is_numeric()
        {
            out.push('.');
        } else {
            out.push(c);
        }
    }
    out
}

/// One immutable price table, indexed once for a read. Memoises misses too: an unpublished
/// model can occur in thousands of records. No process-global cache can carry a missing or
/// outdated rate into the next price snapshot.
pub struct ModelPriceLookup<'a> {
    table: &'a PriceTable,
    folded: Option<HashMap<String, String>>,
    resolved: HashMap<(String, Option<String>), Option<String>>,
}

impl<'a> ModelPriceLookup<'a> {
    pub fn new(table: &'a PriceTable) -> Self {
        Self { table, folded: None, resolved: HashMap::new() }
    }

    pub fn price(&mut self, model: &str, vendor: Option<&str>) -> Option<&'a ModelPrice> {
        let table = self.table;
        // Exact first-party ids already have a constant-time index.
        if let Some(price) = table.get(model) {
            return Some(price);
        }
        let key = (model.to_string(), vendor.map(str::to_string));
        if let Some(answer) = self.resolved.get(&key) {
            return answer.as_ref().and_then(|k| table.get(k));
        }
        if self.folded.is_none() {
            let mut index: HashMap<String, String> = HashMap::new();
            let mut ids: Vec<&String> = table.keys().collect();
            ids.sort();
            for id in ids {
                index.entry(id.to_lowercase()).or_insert_with(|| id.clone());
            }
            self.folded = Some(index);
        }
        let folded = self.folded.as_ref().expect("index built");
        let answer = resolve_key(
            model,
            vendor,
            &|id| table.contains_key(id).then(|| id.to_string()),
            &|lowered| folded.get(lowered).cloned(),
        );
        let price = answer.as_ref().and_then(|k| table.get(k));
        self.resolved.insert(key, answer);
        price
    }
}

// MARK: - The table on disk, and the download

/// The table with when it was fetched; the file is `model-prices-5.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PriceCache {
    pub fetched_at: DateTime<Utc>,
    pub prices: PriceTable,
}

/// Version 4 includes namespaced plan-vendor rates; version 5 keeps each model's long-context
/// tiers. A version 4 table can be fresh but carries no tiers, so it is only an offline
/// fallback and never suppresses a download on upgrade.
pub const CACHE_FILE: &str = "model-prices-5.json";
const PREVIOUS_CACHE_FILE: &str = "model-prices-4.json";

pub fn read_cache(dir: &Path, allow_previous_version: bool) -> Option<PriceCache> {
    let mut names = vec![CACHE_FILE];
    if allow_previous_version {
        names.push(PREVIOUS_CACHE_FILE);
    }
    for name in names {
        let Ok(bytes) = std::fs::read(dir.join(name)) else { continue };
        if let Ok(cache) = serde_json::from_slice::<PriceCache>(&bytes) {
            return Some(cache);
        }
    }
    None
}

fn write_cache(dir: &Path, cache: &PriceCache) {
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let Ok(bytes) = serde_json::to_vec(cache) else { return };
    write_atomically(&dir.join(CACHE_FILE), &bytes);
}

/// Writes through a sibling temp file so a reader never sees half a cache.
pub(crate) fn write_atomically(path: &Path, bytes: &[u8]) {
    let temp = path.with_extension(format!("tmp-{}", std::process::id()));
    if std::fs::write(&temp, bytes).is_ok() && std::fs::rename(&temp, path).is_err() {
        let _ = std::fs::remove_file(&temp);
    }
}

type DownloadFuture = Pin<Box<dyn Future<Output = Option<PriceTable>> + Send>>;

struct Inner {
    dir: PathBuf,
    now: Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>,
    download: Arc<dyn Fn() -> DownloadFuture + Send + Sync>,
    state: Mutex<PriceState>,
    /// Serialises refreshes: concurrent readers share one download.
    gate: tokio::sync::Mutex<()>,
}

#[derive(Default)]
struct PriceState {
    cached: Option<PriceCache>,
    /// The successful fetch's expiry, or a short retry delay after failure. Retrying does not
    /// change the snapshot's original `fetched_at`.
    next_fetch_at: Option<DateTime<Utc>>,
}

/// The price table: valid for 24 hours, refreshed on the next read after that, kept on disk so
/// the settings pane works offline. A failed download keeps the previous table and its
/// timestamp, and may be retried after five minutes.
#[derive(Clone)]
pub struct ModelPrices {
    inner: Arc<Inner>,
}

impl ModelPrices {
    /// The real thing: models.dev, in `dir`.
    pub fn new(dir: PathBuf) -> Self {
        Self::with(dir, Arc::new(Utc::now), Arc::new(|| Box::pin(download_models_dev())))
    }

    /// Isolated cache, clock and network boundaries for lifecycle tests.
    pub fn with(
        dir: PathBuf,
        now: Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>,
        download: Arc<dyn Fn() -> DownloadFuture + Send + Sync>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner { dir, now, download, state: Mutex::new(PriceState::default()), gate: tokio::sync::Mutex::new(()) }),
        }
    }

    /// The process-wide table, kept under Pulse's data folder.
    pub fn shared() -> &'static ModelPrices {
        static SHARED: std::sync::OnceLock<ModelPrices> = std::sync::OnceLock::new();
        SHARED.get_or_init(|| ModelPrices::new(crate::paths::data_dir()))
    }

    /// The table as it stands, fetching a replacement first when it has expired. A download a
    /// reader started runs to the end even if that reader is dropped, because another may need it.
    pub async fn prices(&self) -> PriceTable {
        let inner = self.inner.clone();
        tokio::spawn(async move { inner.prices().await }).await.unwrap_or_default()
    }

    /// Whatever table is on disk, however old, without touching the network.
    pub fn cached(dir: &Path) -> PriceTable {
        read_cache(dir, true).map(|c| c.prices).unwrap_or_default()
    }
}

impl Inner {
    async fn prices(&self) -> PriceTable {
        let _turn = self.gate.lock().await;
        let at = (self.now)();
        {
            let mut state = self.state.lock().expect("price state");
            if let Some(next) = state.next_fetch_at {
                if at < next {
                    return state.cached.as_ref().map(|c| c.prices.clone()).unwrap_or_default();
                }
            }
            if state.cached.is_none() {
                if let Some(saved) = read_cache(&self.dir, false) {
                    let expires = saved.fetched_at + REFRESH_AFTER;
                    state.cached = Some(saved.clone());
                    if at < expires {
                        state.next_fetch_at = Some(expires);
                        return saved.prices;
                    }
                } else {
                    // A pre-vendor table is useful offline, but even a recent one must not
                    // suppress the upgrade download.
                    state.cached = read_cache(&self.dir, true);
                }
            }
        }

        let fetched = (self.download)().await;
        let mut state = self.state.lock().expect("price state");
        match fetched {
            Some(prices) => {
                let snapshot = PriceCache { fetched_at: (self.now)(), prices };
                state.next_fetch_at = Some(snapshot.fetched_at + REFRESH_AFTER);
                write_cache(&self.dir, &snapshot);
                state.cached = Some(snapshot);
            }
            None => {
                // Keep the last table without renewing its age or writing it back as a new
                // download. Repeated reads offline are bounded.
                state.next_fetch_at = Some((self.now)() + RETRY_AFTER);
            }
        }
        state.cached.as_ref().map(|c| c.prices.clone()).unwrap_or_default()
    }
}

async fn download_models_dev() -> Option<PriceTable> {
    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(30)).build().ok()?;
    let response = client.get(SOURCE).send().await.ok()?;
    if response.status().as_u16() != 200 {
        return None;
    }
    let root: Value = response.json().await.ok()?;
    parse_models_dev(&root)
}
