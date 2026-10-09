// Ported from upstream Sources/Pulse/Providers/DeepSeekConsole.swift.
//! DeepSeek's web console, read with the console's own sign-in: the account's usage day by day
//! (tokens of each kind and what was charged, per model) and the balance, for anyone who has not
//! pasted a key.
//!
//! **A second credential beside the key, never instead of it.** The key reads `/user/balance` and
//! nothing else; DeepSeek gives an API key no usage history at all. The console's routes want the
//! console's login token and refuse a key outright (`40003`). That token lives in the browser's
//! `localStorage` under `https://platform.deepseek.com`, key `userToken`, wrapped by the console's
//! storage class:
//!
//! ```json
//! {"value":"<token>","__version":"0"}
//! ```
//!
//! with `"value":null` once signed out. It is kept in its own slot of the key store,
//! `deepSeek#console` (`secret_id`), beside the key's `deepSeek`.
//!
//! Windows: macOS reads the token out of a Chromium browser's LevelDB. Here the sign-in happens in
//! Pulse's own WebView2 window and the token is read from that page's `localStorage`
//! (`src-tauri/src/console_ipc.rs`); this module is everything after the token is in hand. A token
//! the console turns away is not renewed from a browser in the background: the account reads
//! "session expired" and the pane's Read button signs in again.
//!
//! Undocumented, like every console route Pulse reads; the shapes below are the console bundle's
//! own (checked upstream 2026-10-03) and can change.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use tokio::sync::Mutex;

use super::deepseek::{Info, Reply};
use crate::model::AccountKey;
use crate::provider::Provider;
use crate::spend::ledger::{Ledger, LedgerDay, Origin};
use crate::spend::{Calendar, TokenTally};

pub const ORIGIN: &str = "https://platform.deepseek.com";
pub const STORAGE_KEY: &str = "userToken";
/// The key-store slot the token is kept in, beside the key.
pub const SLOT: &str = "console";
/// The console's own "last 30 days": today and the twenty-nine before it.
pub const DAYS: i64 = 30;

/// The id the console token is kept under in the secret store (`deepSeek#console`).
pub fn secret_id() -> String {
    AccountKey { provider: Provider::DeepSeek, slot: SLOT.to_string() }.id()
}

/// What a console read came to.
#[derive(Debug, Clone, PartialEq)]
pub enum Read {
    Answered(Ledger),
    /// The console turned the session away: signed out, or expired.
    SignedOut,
    /// Nothing usable came back.
    Failed,
}

// MARK: - The browser's copy

/// The token out of the console's `localStorage` entry. None for a signed-out console
/// (`"value":null`), an empty one, or anything not shaped like the storage class's wrapper.
pub fn token_from_storage(values: &BTreeMap<String, String>) -> Option<String> {
    let raw = values.get(STORAGE_KEY)?;
    token_from_entry(raw)
}

/// The same, from the entry's own text.
pub fn token_from_entry(raw: &str) -> Option<String> {
    let object: Value = serde_json::from_str(raw).ok()?;
    let token = object.get("value")?.as_str()?.trim();
    (!token.is_empty()).then(|| token.to_string())
}

// MARK: - The replies

/// Every console route answers `{code, msg, data: {biz_code, biz_msg, biz_data}}` with HTTP 200,
/// including a refused token (`40002` missing, `40003` invalid): the status code alone says
/// nothing.
#[derive(Debug, Deserialize)]
pub struct Envelope<Body> {
    pub code: Option<i64>,
    pub data: Option<Inner<Body>>,
}

#[derive(Debug, Deserialize)]
pub struct Inner<Body> {
    pub biz_code: Option<i64>,
    pub biz_data: Option<Body>,
}

/// A number the console may send as a number or as a string: the token counts are added as numbers
/// by the console, the money is handed to a decimal type, which takes either. Absent or unreadable
/// is None, never 0.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Figure(pub Option<f64>);

impl Figure {
    pub fn value(self) -> Option<f64> {
        self.0
    }
}

impl<'de> Deserialize<'de> for Figure {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let read = match Value::deserialize(deserializer)? {
            Value::Number(n) => n.as_f64(),
            Value::String(s) => s.trim().parse::<f64>().ok(),
            _ => None,
        };
        // "nan", "inf" and absurd sizes parse as floats too, and turning one into a count
        // misleads: such a figure is absent.
        Ok(Figure(read.filter(|v| v.is_finite() && v.abs() < 1e15)))
    }
}

/// `GET /api/v0/usage/by_api_key/amount`: tokens per key, model and bucket.
#[derive(Debug, Deserialize, Default)]
pub struct Amount {
    pub bucket: Option<f64>,
    pub series: Option<Vec<AmountSeries>>,
}

#[derive(Debug, Deserialize)]
pub struct AmountSeries {
    pub model: Option<String>,
    pub buckets: Option<Vec<AmountBucket>>,
}

#[derive(Debug, Deserialize)]
pub struct AmountBucket {
    /// The bucket's start, seconds since 1970.
    pub time: f64,
    pub usage: Option<AmountUsage>,
}

#[derive(Debug, Deserialize)]
pub struct AmountUsage {
    #[serde(rename = "PROMPT_CACHE_HIT_TOKEN")]
    pub cache_hit: Option<Figure>,
    #[serde(rename = "PROMPT_CACHE_MISS_TOKEN")]
    pub cache_miss: Option<Figure>,
    #[serde(rename = "RESPONSE_TOKEN")]
    pub response: Option<Figure>,
    #[serde(rename = "REQUEST")]
    pub requests: Option<Figure>,
}

/// `GET /api/v0/usage/by_api_key/cost`: money per currency, key, model and bucket.
#[derive(Debug, Deserialize, Default)]
pub struct Cost {
    pub data: Option<Vec<Purse>>,
}

#[derive(Debug, Deserialize)]
pub struct Purse {
    pub currency: Option<String>,
    pub series: Option<Vec<CostSeries>>,
}

#[derive(Debug, Deserialize)]
pub struct CostSeries {
    pub model: Option<String>,
    pub buckets: Option<Vec<CostBucket>>,
}

#[derive(Debug, Deserialize)]
pub struct CostBucket {
    pub time: f64,
    pub cost: Option<Figure>,
}

/// `GET /api/v0/users/get_user_summary`: the wallets. Only the balances are decoded; the summary
/// also estimates tokens left, which is a guess of the console's and not repeated here.
#[derive(Debug, Deserialize, Default)]
pub struct Summary {
    /// Money topped up.
    pub normal_wallets: Option<Vec<Wallet>>,
    /// Money granted.
    pub bonus_wallets: Option<Vec<Wallet>>,
}

#[derive(Debug, Deserialize)]
pub struct Wallet {
    pub balance: Option<Figure>,
    pub currency: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    SignedOut,
    Failed,
}

/// The envelope's verdict: its body, or why there is none.
pub fn outcome<Body>(envelope: Envelope<Body>) -> Result<Body, Failure> {
    // 40002 "Missing Token", 40003 "Authorization Failed (invalid token)", and the rest of the
    // 400xx family is the sign-in's business too.
    if let Some(code) = envelope.code.filter(|c| *c != 0) {
        return Err(if (40_000..40_100).contains(&code) { Failure::SignedOut } else { Failure::Failed });
    }
    let Some(inner) = envelope.data else { return Err(Failure::Failed) };
    if inner.biz_code.unwrap_or(0) != 0 {
        return Err(Failure::Failed);
    }
    inner.biz_data.ok_or(Failure::Failed)
}

// MARK: - Asking

async fn get<Body: DeserializeOwned>(
    http: &reqwest::Client,
    path: &str,
    query: &[(String, String)],
    token: &str,
) -> Result<Body, Failure> {
    let request = http
        .get(format!("{ORIGIN}{path}"))
        .query(query)
        .bearer_auth(token)
        .header("Accept", "application/json")
        .timeout(Duration::from_secs(20));
    let reply = request.send().await.map_err(|_| Failure::Failed)?;
    match reply.status().as_u16() {
        200 => {}
        // Only 401. A refused token answers 200 with a 400xx code; a 403 is the WAF in front of the
        // console, which a new sign-in cannot fix.
        401 => return Err(Failure::SignedOut),
        _ => return Err(Failure::Failed),
    }
    let bytes = reply.bytes().await.map_err(|_| Failure::Failed)?;
    let envelope: Envelope<Body> = serde_json::from_slice(&bytes).map_err(|_| Failure::Failed)?;
    outcome(envelope)
}

/// The query every usage route takes: `start`/`end` in seconds and `tz` as a whole number of hours
/// in seconds. A zone off the hour (India's +5:30) is sent as the hour below with the remainder
/// moved into the range: exactly what the console does, so the buckets are its own.
pub fn range(now: DateTime<Utc>, calendar: &Calendar) -> Vec<(String, String)> {
    let today = calendar.start_of_day(now);
    let first = calendar.add_days(today, -(DAYS - 1));
    let end = calendar.add_days(today, 1);
    let offset = (calendar.to_local(now) - now.naive_utc()).num_seconds();
    let hours = offset.div_euclid(3600) * 3600;
    let remainder = offset - hours;
    vec![
        ("start".into(), (first.timestamp() + remainder).to_string()),
        ("end".into(), (end.timestamp() + remainder).to_string()),
        ("tz".into(), hours.to_string()),
    ]
}

// MARK: - The history

/// The last thirty days as a ledger, or why there is none.
pub async fn ledger_for(http: &reqwest::Client, token: &str, currency: Option<&str>, now: DateTime<Utc>) -> Read {
    let calendar = Calendar::local();
    let query = range(now, &calendar);
    let (amount, cost) = tokio::join!(
        get::<Amount>(http, "/api/v0/usage/by_api_key/amount", &query, token),
        get::<Cost>(http, "/api/v0/usage/by_api_key/cost", &query, token),
    );
    match (amount, cost) {
        (Ok(amount), Ok(cost)) => Read::Answered(ledger(&amount, &cost, currency, now, &calendar)),
        (Err(Failure::SignedOut), _) | (_, Err(Failure::SignedOut)) => Read::SignedOut,
        _ => Read::Failed,
    }
}

/// Which currency's money the ledger carries. Yuan and dollars cannot be added, and one is not
/// picked over the other by comparing amounts: the reader's choice for the ring, else the first the
/// reply lists with any money in it, else the first at all.
pub fn currency_of<'a>(cost: &'a Cost, preferring: Option<&str>) -> Option<&'a Purse> {
    let purses: Vec<&Purse> = cost.data.iter().flatten().filter(|p| !p.currency.as_deref().unwrap_or_default().is_empty()).collect();
    let has_money = |purse: &&Purse| {
        purse.series.iter().flatten().any(|s| s.buckets.iter().flatten().any(|b| b.cost.and_then(Figure::value).unwrap_or(0.0) > 0.0))
    };
    // The ring's currency, unless nothing was charged in it and something was in another: zeroes
    // in yuan beside tokens paid in dollars would say the work was free.
    let chosen = preferring.and_then(|code| purses.iter().find(|p| p.currency.as_deref() == Some(code)).copied());
    if let Some(chosen) = chosen.filter(has_money) {
        return Some(chosen);
    }
    purses.iter().find(|p| has_money(p)).copied().or(chosen).or_else(|| purses.first().copied())
}

/// Day buckets into ledger days, every day of the thirty present so the chart reads as a calendar.
///
/// Cache-miss input is the ledger's input, cache hits its cache reads, the response its output:
/// DeepSeek writes no cache, it only reads one. **No quarter-hours**: a day bucket does not say when
/// in the day the work ran, so the ledger has no slots and says its timing is aggregate.
pub fn ledger(amount: &Amount, cost: &Cost, preferring: Option<&str>, now: DateTime<Utc>, calendar: &Calendar) -> Ledger {
    // A bucket's middle, not its start: the buckets follow the offset in force *now*, so across a
    // daylight-saving change a day's start sits an hour off local midnight, and an hour early is
    // the day before.
    let half = amount.bucket.filter(|b| *b > 0.0).unwrap_or(86_400.0) / 2.0;
    let day = |time: f64| -> DateTime<Utc> {
        let at = DateTime::from_timestamp_millis(((time + half) * 1000.0) as i64).unwrap_or(now);
        calendar.start_of_day(at)
    };

    let mut tallies: HashMap<DateTime<Utc>, HashMap<String, TokenTally>> = HashMap::new();
    for series in amount.series.iter().flatten() {
        let model = series.model.clone().unwrap_or_default();
        for bucket in series.buckets.iter().flatten() {
            let Some(usage) = &bucket.usage else { continue };
            let figure = |f: Option<Figure>| f.and_then(Figure::value).unwrap_or(0.0) as i64;
            let kinds = TokenTally::new(figure(usage.cache_miss), 0, figure(usage.cache_hit), figure(usage.response));
            if kinds.total() <= 0 {
                continue;
            }
            let entry = tallies.entry(day(bucket.time)).or_default().entry(model.clone()).or_default();
            *entry = entry.clone() + kinds;
        }
    }

    let purse = currency_of(cost, preferring);
    let mut money: HashMap<DateTime<Utc>, f64> = HashMap::new();
    for series in purse.iter().flat_map(|p| p.series.iter().flatten()) {
        for bucket in series.buckets.iter().flatten() {
            let Some(charged) = bucket.cost.and_then(Figure::value).filter(|c| *c != 0.0) else { continue };
            *money.entry(day(bucket.time)).or_default() += charged;
        }
    }

    let today = calendar.start_of_day(now);
    let mut days: Vec<LedgerDay> = Vec::new();
    let mut date = calendar.add_days(today, -(DAYS - 1));
    while date <= today {
        let models = tallies.get(&date).cloned().unwrap_or_default();
        let tally = models.values().fold(TokenTally::default(), |sum, t| sum + t);
        let named: HashMap<String, TokenTally> = models.into_iter().filter(|(name, _)| !name.is_empty()).collect();
        let mut entry = LedgerDay::new(
            date,
            tally.total(),
            money.get(&date).copied().unwrap_or(0.0),
            0,
            named.iter().map(|(name, t)| (name.clone(), t.total())).collect(),
        );
        entry.tally = tally;
        entry.model_tallies = named;
        days.push(entry);
        let next = calendar.add_days(date, 1);
        if next <= date {
            break;
        }
        date = next;
    }

    let earliest = days.iter().find(|d| d.tokens > 0 || d.cost != 0.0).map(|d| d.date);
    let mut result = Ledger::empty();
    // No money came back at all: the tokens alone, and no "$0.00" claiming the work was free.
    result.origin = if purse.is_some() { Origin::ProviderLogs } else { Origin::ProviderStatistics };
    result.currency = purse.and_then(|p| p.currency.clone());
    result.days = days;
    result.earliest = earliest;
    result.has_aggregate_timing = true;
    result
}

// MARK: - The balance

/// The wallets as the key route's reply, so the ring is drawn by exactly the same rule whichever
/// credential answered: topped-up plus granted money per currency. The console states no
/// `is_available`, so nothing here may call the account spent.
pub async fn balance(http: &reqwest::Client, token: &str) -> Result<Reply, Failure> {
    get::<Summary>(http, "/api/v0/users/get_user_summary", &[], token).await.map(|s| reply_from(&s))
}

pub fn reply_from(summary: &Summary) -> Reply {
    let mut order: Vec<String> = Vec::new();
    let mut topped: HashMap<String, f64> = HashMap::new();
    let mut granted: HashMap<String, f64> = HashMap::new();
    for (wallets, is_bonus) in [(&summary.normal_wallets, false), (&summary.bonus_wallets, true)] {
        for wallet in wallets.iter().flatten() {
            let Some(currency) = wallet.currency.clone().filter(|c| !c.is_empty()) else { continue };
            let Some(amount) = wallet.balance.and_then(Figure::value) else { continue };
            if !order.contains(&currency) {
                order.push(currency.clone());
            }
            *(if is_bonus { &mut granted } else { &mut topped }).entry(currency).or_default() += amount;
        }
    }
    let text = |amount: f64| format!("{amount:.2}");
    Reply {
        is_available: None,
        balance_infos: Some(
            order
                .into_iter()
                .map(|currency| Info {
                    total_balance: Some(text(topped.get(&currency).copied().unwrap_or(0.0) + granted.get(&currency).copied().unwrap_or(0.0))),
                    granted_balance: granted.get(&currency).copied().map(text),
                    topped_up_balance: topped.get(&currency).copied().map(text),
                    currency: Some(currency),
                })
                .collect(),
        ),
    }
}

// MARK: - Shared history

/// How long a console answer, of any outcome, is reused: the card, Settings and the warm-up all
/// ask, and two routes a read is cheap enough not to need more than that.
pub const FRESH_FOR: Duration = Duration::from_secs(60);

type Key = (String, Option<String>);

/// One console read at a time, and a fresh answer reused for a minute.
#[derive(Default)]
pub struct History {
    last: Mutex<Option<(Key, std::time::Instant, Read)>>,
}

impl History {
    pub fn shared() -> Arc<Self> {
        static SHARED: std::sync::OnceLock<Arc<History>> = std::sync::OnceLock::new();
        SHARED.get_or_init(|| Arc::new(History::default())).clone()
    }

    /// Any outcome counts as fresh, not only an answer: a refused token asked again at every
    /// settings change would cost two requests each time. Callers that arrive while a read runs
    /// wait on it and share its answer.
    pub async fn ledger(&self, http: &reqwest::Client, token: &str, currency: Option<&str>) -> Read {
        let key: Key = (token.to_string(), currency.map(str::to_string));
        let mut last = self.last.lock().await;
        if let Some((kept, at, read)) = last.as_ref() {
            if *kept == key && at.elapsed() < FRESH_FOR {
                return read.clone();
            }
        }
        let read = ledger_for(http, token, currency, Utc::now()).await;
        *last = Some((key, std::time::Instant::now(), read.clone()));
        read
    }

    pub async fn forget(&self) {
        *self.last.lock().await = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{FixedOffset, TimeZone};
    use serde_json::json;

    fn calendar(seconds: i32) -> Calendar {
        Calendar::with_zone(FixedOffset::east_opt(seconds).unwrap(), 2)
    }

    /// 2026-10-03 15:00 at +08:00.
    fn now() -> DateTime<Utc> {
        Utc.timestamp_opt(1_791_010_800, 0).unwrap()
    }

    fn today(c: &Calendar) -> DateTime<Utc> {
        c.start_of_day(now())
    }

    fn day(c: &Calendar, offset: i64) -> i64 {
        c.add_days(today(c), offset).timestamp()
    }

    fn decode<T: DeserializeOwned>(json: &str) -> T {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn the_token_is_the_storage_wrappers_value_signed_out_or_empty_is_none() {
        let entry = |raw: &str| BTreeMap::from([("userToken".to_string(), raw.to_string())]);
        assert_eq!(token_from_storage(&entry(r#"{"value":"abc123","__version":"0"}"#)).as_deref(), Some("abc123"));
        assert_eq!(token_from_storage(&entry(r#"{"value":null,"__version":"0"}"#)), None);
        assert_eq!(token_from_storage(&entry(r#"{"value":"  ","__version":"0"}"#)), None);
        assert_eq!(token_from_storage(&entry("abc123")), None);
        assert_eq!(token_from_storage(&BTreeMap::new()), None);
    }

    #[test]
    fn the_token_has_its_own_slot_beside_the_key() {
        assert_eq!(secret_id(), "deepSeek#console");
        assert_eq!(AccountKey::from_id(&secret_id()).unwrap().slot, "console");
    }

    #[test]
    fn a_refused_token_answers_http_200_with_a_400xx_code_and_that_is_signed_out() {
        type E = Envelope<Summary>;
        let verdict = |json: &str| outcome(decode::<E>(json)).err();
        assert_eq!(verdict(r#"{"code":40002,"msg":"Missing Token","data":null}"#), Some(Failure::SignedOut));
        assert_eq!(verdict(r#"{"code":40003,"msg":"Authorization Failed (invalid token)","data":null}"#), Some(Failure::SignedOut));
        assert_eq!(verdict(r#"{"code":50000,"msg":"x","data":null}"#), Some(Failure::Failed));
        assert_eq!(verdict(r#"{"code":0,"data":{"biz_code":1,"biz_data":null}}"#), Some(Failure::Failed));
        assert_eq!(verdict(r#"{"code":0,"data":{"biz_code":0,"biz_data":{"normal_wallets":[],"bonus_wallets":[]}}}"#), None);
    }

    #[test]
    fn the_range_is_the_consoles_last_30_days_whole_local_days_tz_in_whole_hours() {
        let c = calendar(8 * 3600);
        let values: HashMap<String, String> = range(now(), &c).into_iter().collect();
        assert_eq!(values["start"], day(&c, -29).to_string());
        assert_eq!(values["end"], day(&c, 1).to_string());
        assert_eq!(values["tz"], "28800");

        // India: the hour below, the half hour moved into the range.
        let india = calendar(5 * 3600 + 1800);
        let off: HashMap<String, String> = range(now(), &india).into_iter().collect();
        assert_eq!(off["tz"], "18000");
        assert_eq!(off["start"], (day(&india, -29) + 1800).to_string());

        // West of Greenwich: the hour below as well (-3:30 is -4h with half an hour added back).
        let newfoundland = calendar(-(3 * 3600 + 1800));
        let west: HashMap<String, String> = range(now(), &newfoundland).into_iter().collect();
        assert_eq!(west["tz"], (-4 * 3600).to_string());
        assert_eq!(west["start"], (day(&newfoundland, -29) + 1800).to_string());
    }

    #[test]
    fn day_buckets_become_thirty_ledger_days_with_kinds_models_and_the_charged_money() {
        let c = calendar(8 * 3600);
        let amount: Envelope<Amount> = decode(&format!(
            r#"{{"code":0,"data":{{"biz_code":0,"biz_data":{{"start":{start},"end":{end},"bucket":86400,
              "models":["deepseek-chat","deepseek-reasoner"],
              "series":[
                {{"api_key":{{"tracking_id":"k1","name":"a","sensitive_id":"sk-1***","valid":true}},"model":"deepseek-chat",
                 "buckets":[{{"time":{d0},"usage":{{"PROMPT_CACHE_HIT_TOKEN":800,"PROMPT_CACHE_MISS_TOKEN":150,"RESPONSE_TOKEN":50,"REQUEST":3}}}},
                            {{"time":{d2},"usage":{{"PROMPT_CACHE_HIT_TOKEN":0,"PROMPT_CACHE_MISS_TOKEN":0,"RESPONSE_TOKEN":0,"REQUEST":0}}}}]}},
                {{"api_key":{{"tracking_id":"k2","name":"b","sensitive_id":"sk-2***","valid":true}},"model":"deepseek-chat",
                 "buckets":[{{"time":{d0},"usage":{{"PROMPT_CACHE_HIT_TOKEN":"100","PROMPT_CACHE_MISS_TOKEN":"0","RESPONSE_TOKEN":"0","REQUEST":"1"}}}}]}},
                {{"api_key":{{"tracking_id":"k1","name":"a","sensitive_id":"sk-1***","valid":true}},"model":"deepseek-reasoner",
                 "buckets":[{{"time":{d1},"usage":{{"PROMPT_CACHE_HIT_TOKEN":0,"PROMPT_CACHE_MISS_TOKEN":400,"RESPONSE_TOKEN":600,"REQUEST":2}}}}]}}
              ]}}}}}}"#,
            start = day(&c, -29),
            end = day(&c, 1),
            d0 = day(&c, 0),
            d1 = day(&c, -1),
            d2 = day(&c, -2),
        ));
        let cost: Envelope<Cost> = decode(&format!(
            r#"{{"code":0,"data":{{"biz_code":0,"biz_data":{{"start":0,"end":0,"bucket":86400,"models":[],
              "data":[
                {{"currency":"USD","series":[]}},
                {{"currency":"CNY","series":[
                  {{"api_key":{{"tracking_id":"k1"}},"model":"deepseek-chat","buckets":[{{"time":{d0},"cost":"0.0125"}}]}},
                  {{"api_key":{{"tracking_id":"k1"}},"model":"deepseek-reasoner","buckets":[{{"time":{d1},"cost":"1.5"}}]}}
                ]}}
              ]}}}}}}"#,
            d0 = day(&c, 0),
            d1 = day(&c, -1),
        ));
        let ledger = ledger(&outcome(amount).unwrap(), &outcome(cost).unwrap(), None, now(), &c);
        assert_eq!(ledger.origin, Origin::ProviderLogs);
        assert_eq!(ledger.currency.as_deref(), Some("CNY"));
        assert!(ledger.slots.is_empty());
        assert!(ledger.has_aggregate_timing);
        assert_eq!(ledger.days.len(), 30);
        assert_eq!(ledger.days[0].date, c.add_days(today(&c), -29));

        let last = ledger.days.last().unwrap();
        assert_eq!(last.date, today(&c));
        // Two keys on one model, added; a string count read as a number.
        assert_eq!(last.tally, TokenTally::new(150, 0, 900, 50));
        assert_eq!(last.models, HashMap::from([("deepseek-chat".to_string(), 1_100)]));
        assert!((last.cost - 0.0125).abs() < 1e-9);

        let yesterday = &ledger.days[28];
        assert_eq!(yesterday.tally, TokenTally::new(400, 0, 0, 600));
        assert!((yesterday.cost - 1.5).abs() < 1e-9);
        assert_eq!(ledger.days[27].tokens, 0);
        assert_eq!(ledger.earliest, Some(yesterday.date));
    }

    #[test]
    fn the_money_follows_the_readers_currency_else_the_first_with_money_in_it() {
        let cost: Cost = decode(
            r#"{"data":[{"currency":"USD","series":[{"model":"m","buckets":[{"time":0,"cost":"0"}]}]},
                        {"currency":"CNY","series":[{"model":"m","buckets":[{"time":0,"cost":"2"}]}]}]}"#,
        );
        let code = |c: Option<&Purse>| c.and_then(|p| p.currency.clone());
        assert_eq!(code(currency_of(&cost, None)).as_deref(), Some("CNY"));
        // Nothing was charged in dollars: zeroes there would call the work free.
        assert_eq!(code(currency_of(&cost, Some("USD"))).as_deref(), Some("CNY"));
        assert_eq!(code(currency_of(&cost, Some("EUR"))).as_deref(), Some("CNY"));
        assert!(currency_of(&Cost { data: Some(vec![]) }, None).is_none());

        let both: Cost = decode(
            r#"{"data":[{"currency":"CNY","series":[{"model":"m","buckets":[{"time":0,"cost":"2"}]}]},
                        {"currency":"USD","series":[{"model":"m","buckets":[{"time":0,"cost":"1"}]}]}]}"#,
        );
        assert_eq!(code(currency_of(&both, Some("USD"))).as_deref(), Some("USD"));
    }

    #[test]
    fn no_money_in_the_reply_is_tokens_only_never_a_zero_bill_bad_numbers_are_absent() {
        let c = calendar(8 * 3600);
        let amount: Amount = decode(&format!(
            r#"{{"bucket":86400,"series":[{{"model":"m","buckets":[
              {{"time":{d0},"usage":{{"PROMPT_CACHE_HIT_TOKEN":"nan","PROMPT_CACHE_MISS_TOKEN":"1e30","RESPONSE_TOKEN":"inf"}}}},
              {{"time":{d1},"usage":{{"PROMPT_CACHE_HIT_TOKEN":0,"PROMPT_CACHE_MISS_TOKEN":10,"RESPONSE_TOKEN":5}}}}]}}]}}"#,
            d0 = day(&c, 0),
            d1 = day(&c, -1),
        ));
        let ledger = ledger(&amount, &Cost { data: Some(vec![]) }, None, now(), &c);
        assert_eq!(ledger.origin, Origin::ProviderStatistics);
        assert_eq!(ledger.currency, None);
        assert_eq!(ledger.days.last().unwrap().tokens, 0);
        assert_eq!(ledger.days[28].tokens, 15);
    }

    #[test]
    fn the_wallets_become_the_key_routes_reply_topped_up_plus_granted_per_currency() {
        let summary: Summary = decode(
            r#"{"current_token":0,"monthly_usage":0,"total_usage":0,
                "normal_wallets":[{"balance":"8.15","currency":"CNY","token_estimation":"1"}],
                "bonus_wallets":[{"balance":"5.97","currency":"CNY","token_estimation":"1"},{"balance":"1","currency":"USD"}],
                "total_costs":[{"currency":"CNY","amount":"132.97"}]}"#,
        );
        let reply = reply_from(&summary);
        assert_eq!(reply.is_available, None);
        let infos = reply.balance_infos.as_ref().unwrap();
        assert_eq!(infos.len(), 2);
        assert_eq!(infos[0].currency.as_deref(), Some("CNY"));
        assert_eq!(infos[0].total_balance.as_deref(), Some("14.12"));
        assert_eq!(infos[0].topped_up_balance.as_deref(), Some("8.15"));
        assert_eq!(infos[0].granted_balance.as_deref(), Some("5.97"));
        assert_eq!(infos[1].currency.as_deref(), Some("USD"));
        assert_eq!(infos[1].total_balance.as_deref(), Some("1.00"));
        assert_eq!(infos[1].topped_up_balance, None);

        // A wallet with no readable balance is left out, not read as zero.
        let blank = reply_from(&decode(r#"{"normal_wallets":[{"balance":null,"currency":"CNY"}],"bonus_wallets":[]}"#));
        assert!(blank.balance_infos.unwrap().is_empty());
    }

    #[test]
    fn a_figure_is_a_number_or_a_numeric_string_and_nothing_else() {
        let figure = |v: Value| serde_json::from_value::<Figure>(v).unwrap().value();
        assert_eq!(figure(json!(3)), Some(3.0));
        assert_eq!(figure(json!(" 2.5 ")), Some(2.5));
        assert_eq!(figure(json!(true)), None);
        assert_eq!(figure(json!(null)), None);
        assert_eq!(figure(json!("1e30")), None);
    }
}
