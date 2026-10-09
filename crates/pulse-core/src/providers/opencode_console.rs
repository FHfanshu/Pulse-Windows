// Ported from upstream Sources/Pulse/Providers/OpenCodeConsole.swift.
//! OpenCode's console log of every request on the workspace: the account's own record, across every
//! machine and every client that uses its keys: OpenCode itself, and anything else pointed at the
//! Go endpoint.
//!
//! **Read with the console's session, not the Go key.** The key answers the plan's limits
//! (`opencode_go`); the logs are behind the signed-in console, `GET /console/api/request-logs`.
//! Every console route also wants the workspace in an `x-org-id` header (without it each answers
//! `400 {"_tag":"BadRequest"}`) and `GET /console/api/orgs` lists the session's workspaces, so
//! nothing has to be entered ([`WorkspaceCache`]). The session is signed in once in Settings and
//! kept in its own slot beside the key (`openCodeGo#console`, [`secret_id`]), so neither replaces
//! the other. Undocumented, like the usage route; it can change without notice.
//!
//! **Only what a ledger needs is decoded.** Each entry also carries the city the request came from,
//! the key's id and the request headers; none of it is read, so none of it is ever held.
//!
//! Windows: the sign-in happens in Pulse's own WebView2 window (`src-tauri/src/console_ipc.rs`),
//! whose cookie jar yields the two cookies below; this module is everything after the cookie header
//! is in hand.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::model::AccountKey;
use crate::provider::Provider;
use crate::spend::ledger::{Ledger, LedgerDay, Origin, Slot};
use crate::spend::{Calendar, TokenTally};

pub const HOST: &str = "opencode.ai";
/// The key-store slot the session is kept in, beside the Go key.
pub const SLOT: &str = "console";
/// The console's session and the account login. Stripe's tracking and the language preference come
/// along from the browser and are dropped.
pub const COOKIE_NAMES: [&str; 2] = ["__Host-console_session", "auth"];

/// The console keeps 30 days. A day more is asked for so a month's chart starts on a whole day; the
/// server answers with what it has.
pub const SPAN_SECONDS: i64 = 31 * 86_400;
/// The most the console hands out at once: 100 is answered, 200 is a `400` (measured upstream).
pub const PAGE_SIZE: usize = 100;
/// More than six simultaneous requests slowed the console in local probes. The adaptive pager
/// shares this bound across all time ranges and retries.
pub const CONCURRENT_PAGES: usize = 6;
/// A total budget across the entire scan, not a separate budget per range. A history needing more
/// is marked partial and resumed on the next read.
pub const PAGE_LIMIT: usize = 200;

/// The id the console session is kept under in the secret store (`openCodeGo#console`).
pub fn secret_id() -> String {
    AccountKey { provider: Provider::OpenCodeGo, slot: SLOT.to_string() }.id()
}

/// Keeps only the cookies that sign in; None when the console's session is not among them, which
/// is what "signed out of the console" looks like.
pub fn keep(header: &str) -> Option<String> {
    let pairs: Vec<(&str, &str)> = header
        .split(';')
        .filter_map(|part| {
            let (name, value) = part.trim().split_once('=')?;
            COOKIE_NAMES.contains(&name).then_some((name, value))
        })
        .collect();
    if !pairs.iter().any(|(name, value)| *name == "__Host-console_session" && !value.is_empty()) {
        return None;
    }
    Some(pairs.iter().map(|(n, v)| format!("{n}={v}")).collect::<Vec<_>>().join("; "))
}

/// The cookies the sign-in window's jar yields, as a header kept by [`keep`].
pub fn header_from_cookies(cookies: &BTreeMap<String, String>) -> Option<String> {
    keep(&cookies.iter().map(|(n, v)| format!("{n}={v}")).collect::<Vec<_>>().join("; "))
}

// MARK: - The reply

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    pub items: Vec<Item>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub id: String,
    /// Milliseconds since 1970.
    pub started_at: f64,
    /// `go` for the Go plan; the console also logs other products.
    pub product: Option<String>,
    pub model: Option<String>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub cache_write_tokens: Option<i64>,
    /// What the request cost, in dollars, as OpenCode charged it. None for a request that failed
    /// before anything was spent.
    pub cost: Option<f64>,
}

impl Item {
    pub fn date(&self) -> DateTime<Utc> {
        DateTime::from_timestamp_millis(self.started_at.round() as i64).unwrap_or_default()
    }

    /// The four kinds. Input is fresh input: the cache reads are counted beside it, never inside
    /// it: a request reading 294,272 tokens from the cache logs 184 of input.
    pub fn tally(&self) -> TokenTally {
        TokenTally::new(
            self.input_tokens.unwrap_or(0),
            self.cache_write_tokens.unwrap_or(0),
            self.cache_read_tokens.unwrap_or(0),
            self.output_tokens.unwrap_or(0),
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Read {
    Answered(Ledger),
    /// The console turned the session away: signed out, or expired.
    SignedOut,
    /// Nothing usable came back.
    Failed,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PageResult {
    Page(Page),
    SignedOut,
    Failed,
}

pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

// MARK: - Paging

/// Every page from the newest back, asked for one after another with each page's cursor.
///
/// `complete` is false when the walk stopped for any reason but the server's own end (the page
/// limit, a cursor handed back twice, a page that failed after others arrived) so a short history
/// can be said to be short rather than passed off as the month.
pub async fn collect<F, Fut>(retry_delay: Duration, mut fetch: F) -> (Vec<Item>, bool, Option<PageResult>)
where
    F: FnMut(Option<String>) -> Fut,
    Fut: Future<Output = PageResult>,
{
    let mut items = Vec::new();
    let mut cursor: Option<String> = None;
    let mut seen: HashSet<String> = HashSet::new();
    for _ in 0..PAGE_LIMIT {
        let result = retry_page(retry_delay, || fetch(cursor.clone())).await;
        if result == PageResult::SignedOut {
            return (items, false, Some(PageResult::SignedOut));
        }
        let PageResult::Page(page) = result else {
            let outcome = items.is_empty().then_some(result);
            return (items, false, outcome);
        };
        let empty = page.items.is_empty();
        items.extend(page.items);
        let Some(next) = page.next_cursor.filter(|c| !c.is_empty()).filter(|_| !empty) else {
            return (items, true, None);
        };
        if !seen.insert(next.clone()) {
            return (items, false, None);
        }
        cursor = Some(next);
    }
    (items, false, None)
}

/// Retry transient failures twice, but never retry a session the server rejected. Shared by
/// sequential and adaptive walks. (Cancellation is dropping the future, which stops a retry too.)
pub async fn retry_page<F, Fut>(retry_delay: Duration, mut fetch: F) -> PageResult
where
    F: FnMut() -> Fut,
    Fut: Future<Output = PageResult>,
{
    for attempt in 0..=2u32 {
        if attempt > 0 {
            tokio::time::sleep(retry_delay * attempt).await;
        }
        let result = fetch().await;
        if result != PageResult::Failed || attempt == 2 {
            return result;
        }
    }
    PageResult::Failed
}

/// Preserve an item's exact millisecond when a date becomes an inclusive query boundary.
pub fn milliseconds(date: DateTime<Utc>) -> i64 {
    date.timestamp_millis()
}

pub async fn fetch_page(
    http: &reqwest::Client,
    cookie: &str,
    org: &str,
    since: DateTime<Utc>,
    until: Option<DateTime<Utc>>,
    cursor: Option<&str>,
) -> PageResult {
    let mut query = vec![
        ("since".to_string(), milliseconds(since).to_string()),
        ("category".to_string(), "inference".to_string()),
        ("limit".to_string(), PAGE_SIZE.to_string()),
    ];
    if let Some(until) = until {
        query.push(("until".into(), milliseconds(until).to_string()));
    }
    if let Some(cursor) = cursor {
        query.push(("cursor".into(), cursor.to_string()));
    }
    let request = http
        .get(format!("https://{HOST}/console/api/request-logs"))
        .query(&query)
        .header("Cookie", cookie)
        .header("x-org-id", org)
        .header("Accept", "application/json")
        .timeout(Duration::from_secs(20));
    let Ok(reply) = request.send().await else { return PageResult::Failed };
    match reply.status().as_u16() {
        200 => {}
        401 | 403 => return PageResult::SignedOut,
        _ => return PageResult::Failed,
    }
    let Ok(bytes) = reply.bytes().await else { return PageResult::Failed };
    match serde_json::from_slice::<Page>(&bytes) {
        Ok(page) => PageResult::Page(page),
        // A signed-out console answers its sign-in page, not an error code.
        Err(_) => looks_like_a_page(&bytes).then_some(PageResult::SignedOut).unwrap_or(PageResult::Failed),
    }
}

fn looks_like_a_page(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(64)].contains(&b'<')
}

// MARK: - The workspace

/// One of the session's workspaces, as `/console/api/orgs` lists them.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Workspace {
    pub id: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Resolved {
    Workspace(Workspace),
    SignedOut,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceFailure {
    SignedOut,
    Failed,
}

/// The session's workspaces, or why there are none to name.
pub async fn workspaces(http: &reqwest::Client, cookie: &str) -> Result<Vec<Workspace>, WorkspaceFailure> {
    let request = http
        .get(format!("https://{HOST}/console/api/orgs"))
        .header("Cookie", cookie)
        .header("Accept", "application/json")
        .timeout(Duration::from_secs(15));
    let reply = request.send().await.map_err(|_| WorkspaceFailure::Failed)?;
    match reply.status().as_u16() {
        200 => {}
        401 | 403 => return Err(WorkspaceFailure::SignedOut),
        _ => return Err(WorkspaceFailure::Failed),
    }
    let bytes = reply.bytes().await.map_err(|_| WorkspaceFailure::Failed)?;
    match serde_json::from_slice::<Vec<Workspace>>(&bytes) {
        Ok(list) => Ok(list.into_iter().filter(|w| !w.id.is_empty()).collect()),
        Err(_) => Err(if looks_like_a_page(&bytes) { WorkspaceFailure::SignedOut } else { WorkspaceFailure::Failed }),
    }
}

/// Which workspace the console's session reads, asked once per session.
///
/// `/console/api/orgs` lists them. One is the usual case and is used as it is; with several, the
/// first that holds a Go subscription is (the ring is the Go plan's) and the first of all when none
/// answers that it does.
#[derive(Default)]
pub struct WorkspaceCache {
    cached: Mutex<Option<(String, Workspace)>>,
}

impl WorkspaceCache {
    pub fn shared() -> &'static WorkspaceCache {
        static SHARED: OnceLock<WorkspaceCache> = OnceLock::new();
        SHARED.get_or_init(WorkspaceCache::default)
    }

    pub async fn resolve(&self, http: &reqwest::Client, cookie: &str) -> Resolved {
        if let Some((kept, workspace)) = self.cached.lock().unwrap().as_ref() {
            if kept == cookie {
                return Resolved::Workspace(workspace.clone());
            }
        }
        let list = match workspaces(http, cookie).await {
            Err(WorkspaceFailure::SignedOut) => return Resolved::SignedOut,
            Err(WorkspaceFailure::Failed) => return Resolved::Failed,
            Ok(list) => list,
        };
        let Some(first) = list.first().cloned() else { return Resolved::Failed };
        if list.len() == 1 {
            *self.cached.lock().unwrap() = Some((cookie.to_string(), first.clone()));
            return Resolved::Workspace(first);
        }
        for workspace in &list {
            if super::opencode_go::has_go_access(http, cookie, &workspace.id).await {
                *self.cached.lock().unwrap() = Some((cookie.to_string(), workspace.clone()));
                return Resolved::Workspace(workspace.clone());
            }
        }
        // No workspace answered with Go: perhaps none has it, perhaps the console was unreachable
        // for a moment. The first is used for now but **not kept**: a blip at launch must not pin
        // the session to the workspace without the plan until Pulse restarts.
        Resolved::Workspace(first)
    }

    /// Asked again next time: a workspace that stops answering may be gone.
    pub fn forget(&self) {
        *self.cached.lock().unwrap() = None;
    }
}

// MARK: - The ledger

/// The Go plan's requests as a ledger: days from the span ago to today, the gaps closed, with each
/// day's kinds, models and cost, and quarter-hours for the value estimate.
///
/// **Go only.** The console logs every product on the workspace; the ring is the Go plan's, so its
/// card counts the Go plan's requests.
pub fn ledger(items: &[Item], now: DateTime<Utc>, calendar: &Calendar) -> Ledger {
    let start = calendar.start_of_day(now - chrono::Duration::seconds(SPAN_SECONDS));
    let go: Vec<&Item> = items.iter().filter(|i| i.product.as_deref() == Some("go") && i.date() >= start && i.date() <= now).collect();

    #[derive(Default)]
    struct Quarter {
        tokens: i64,
        cost: f64,
        unpriced: i64,
        models: HashMap<String, TokenTally>,
    }
    let mut by_day: HashMap<DateTime<Utc>, Vec<&Item>> = HashMap::new();
    let mut by_slot: BTreeMap<i64, Quarter> = BTreeMap::new();
    let mut unpriced_models: std::collections::BTreeSet<String> = Default::default();
    for item in &go {
        by_day.entry(calendar.start_of_day(item.date())).or_default().push(item);
        let slot = item.date().timestamp().div_euclid(900) * 900;
        let entry = by_slot.entry(slot).or_default();
        let tally = item.tally();
        entry.tokens += tally.total();
        entry.cost += item.cost.unwrap_or(0.0);
        if item.cost.is_none() && tally.total() > 0 {
            entry.unpriced += tally.total();
            if let Some(model) = &item.model {
                unpriced_models.insert(model.clone());
            }
        }
        if let Some(model) = &item.model {
            let kept = entry.models.entry(model.clone()).or_default();
            *kept = kept.clone() + tally;
        }
    }

    let mut days = Vec::new();
    let today = calendar.start_of_day(now);
    let mut date = start;
    while date <= today {
        let items = by_day.get(&date).map(Vec::as_slice).unwrap_or_default();
        let mut models: HashMap<String, i64> = HashMap::new();
        let mut model_tallies: HashMap<String, TokenTally> = HashMap::new();
        let mut tally = TokenTally::default();
        let (mut cost, mut unpriced) = (0.0, 0);
        for item in items {
            let kinds = item.tally();
            tally = tally + kinds.clone();
            cost += item.cost.unwrap_or(0.0);
            if item.cost.is_none() {
                unpriced += kinds.total();
            }
            if let Some(model) = &item.model {
                *models.entry(model.clone()).or_default() += kinds.total();
                let kept = model_tallies.entry(model.clone()).or_default();
                *kept = kept.clone() + kinds;
            }
        }
        let mut day = LedgerDay::new(date, tally.total(), cost, unpriced, models);
        day.tally = tally;
        day.model_tallies = model_tallies;
        days.push(day);
        let next = calendar.add_days(date, 1);
        if next <= date {
            break;
        }
        date = next;
    }

    let mut result = Ledger::empty();
    result.origin = Origin::ProviderLogs;
    result.days = days;
    result.earliest = go.iter().map(|i| i.date()).min();
    result.unpriced_models = unpriced_models.into_iter().collect();
    result.slots = by_slot
        .into_iter()
        .filter_map(|(start, q)| {
            let mut slot = Slot::new(DateTime::from_timestamp(start, 0)?, q.tokens, q.cost);
            slot.unpriced_tokens = q.unpriced;
            slot.models = q.models;
            Some(slot)
        })
        .collect();
    result
}

// MARK: - The shared client

static HTTP: Mutex<Option<reqwest::Client>> = Mutex::new(None);

/// The client the shared history uses, set by the app whenever its network settings change.
pub fn set_http(client: reqwest::Client) {
    *HTTP.lock().unwrap() = Some(client);
}

pub fn http() -> reqwest::Client {
    HTTP.lock().unwrap().clone().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 1, 12, 0, 0).unwrap()
    }

    fn item(id: &str, hours_ago: f64, product: &str, model: &str, tokens: Option<(i64, i64, i64)>, cost: Option<f64>) -> Item {
        let (input, output, cache_read) = tokens.map_or((None, None, None), |(i, o, c)| (Some(i), Some(o), Some(c)));
        Item {
            id: id.into(),
            started_at: (now() - chrono::Duration::milliseconds((hours_ago * 3_600_000.0) as i64)).timestamp_millis() as f64,
            product: Some(product.into()),
            model: Some(model.into()),
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: cache_read,
            cache_write_tokens: input.map(|_| 0),
            cost,
        }
    }

    fn go(id: &str, hours_ago: f64, cost: Option<f64>) -> Item {
        item(id, hours_ago, "go", "deepseek-flash", Some((100, 20, 900)), cost)
    }

    #[test]
    fn only_the_sign_in_cookies_are_kept() {
        let header = "__stripe_mid=x; __Host-console_session=st_1; oc_locale=zh; auth=Fe26.2**abc; __stripe_sid=y";
        assert_eq!(keep(header).as_deref(), Some("__Host-console_session=st_1; auth=Fe26.2**abc"));
        // No console session: nothing worth keeping.
        assert_eq!(keep("auth=Fe26.2**abc; oc_locale=zh"), None);
        assert_eq!(keep("__Host-console_session="), None);
    }

    #[test]
    fn the_session_has_its_own_slot_beside_the_key() {
        assert_eq!(secret_id(), "openCodeGo#console");
        let jar = BTreeMap::from([("auth".to_string(), "a".to_string()), ("__Host-console_session".to_string(), "s".to_string()), ("x".to_string(), "y".to_string())]);
        assert_eq!(header_from_cookies(&jar).as_deref(), Some("__Host-console_session=s; auth=a"));
    }

    #[test]
    fn a_page_decodes_without_the_fields_pulse_does_not_hold() {
        let json = r#"
        {"items":[{"id":"r1","workspaceID":"wrk_X","startedAt":1790850000000,"outcome":"succeeded","product":"go",
          "model":"deepseek-flash","country":"US","city":"Somewhere","serviceAPIKeyID":"key_X",
          "requestHeaders":{"user-agent":"x"},"inputTokens":7305,"outputTokens":420,"reasoningTokens":19,
          "cacheReadTokens":2688,"cacheWriteTokens":0,"cost":0.00135581},
          {"id":"r2","startedAt":1790849000000,"outcome":"failed","product":"go","model":"gpt-6-luna",
          "statusCode":403,"inputTokens":null,"outputTokens":null,"cost":null}],
         "nextCursor":"{\"v\":2,\"after\":{\"id\":\"r2\"}}","until":1790853898774,"retentionDays":30}"#;
        let page: Page = serde_json::from_str(json).unwrap();
        assert_eq!(page.items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["r1", "r2"]);
        assert_eq!(page.items[0].tally(), TokenTally::new(7305, 0, 2688, 420));
        assert_eq!(page.items[1].tally().total(), 0);
        assert!(page.next_cursor.is_some_and(|c| !c.is_empty()));
    }

    #[test]
    fn the_go_plans_requests_become_a_month_priced_as_charged() {
        let calendar = Calendar::utc(2);
        let ledger = ledger(
            &[
                go("a", 1.0, Some(0.002)),
                go("b", 2.0, Some(0.003)),
                // A failed request spends nothing and prices nothing.
                item("c", 3.0, "go", "deepseek-flash", None, None),
                // Another product on the workspace is not the Go plan's.
                item("d", 1.0, "zen", "deepseek-flash", Some((100, 20, 900)), Some(5.0)),
                item("e", 49.0, "go", "glm-5", Some((100, 20, 900)), Some(0.01)),
                // Older than the console keeps, and older than the month.
                go("f", 24.0 * 40.0, Some(1.0)),
            ],
            now(),
            &calendar,
        );
        assert_eq!(ledger.origin, Origin::ProviderLogs);
        assert_eq!(ledger.days.len(), 32);
        let today = ledger.days.last().unwrap();
        assert_eq!(calendar.date(today.date), calendar.date(now()));
        assert_eq!(today.tokens, 2 * 1020);
        assert!((today.cost - 0.005).abs() < 1e-9);
        assert!((ledger.total_in(31, now(), &calendar).cost - 0.015).abs() < 1e-9);
        assert_eq!(ledger.top_model_in(31, now(), &calendar).map(|m| m.0).as_deref(), Some("deepseek-flash"));
        // Every token sorted into its kind: the hit rate can be stated.
        assert!((ledger.cache_hit_rate_in(31, now(), &calendar).unwrap() - 0.9).abs() < 1e-9);
        // Quarter-hours for the value estimate, with the money in them.
        let (_, cost) = ledger.spend_since(now() - chrono::Duration::minutes(90));
        assert!((cost - 0.002).abs() < 1e-9);
    }

    fn page(ids: &[&str], next: Option<&str>) -> PageResult {
        PageResult::Page(Page { items: ids.iter().map(|id| go(id, 1.0, Some(0.002))).collect(), next_cursor: next.map(String::from) })
    }

    #[tokio::test]
    async fn pages_are_followed_to_the_servers_end() {
        let (items, complete, _) = collect(Duration::ZERO, |cursor| async move {
            match cursor.as_deref() {
                None => page(&["1", "2"], Some("c1")),
                Some("c1") => page(&["3"], Some("c2")),
                _ => page(&[], None),
            }
        })
        .await;
        assert_eq!(items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["1", "2", "3"]);
        assert!(complete);
    }

    /// A page the console held past the timeout is asked for again, and the walk is still whole
    /// when it then arrives.
    #[tokio::test]
    async fn a_page_that_fails_once_is_asked_again() {
        let attempts = AtomicUsize::new(0);
        let (items, complete, _) = collect(Duration::ZERO, |cursor| {
            let failing = cursor.is_some() && attempts.fetch_add(1, Ordering::SeqCst) == 0;
            async move {
                match (cursor, failing) {
                    (None, _) => page(&["1"], Some("c1")),
                    (Some(_), true) => PageResult::Failed,
                    (Some(_), false) => page(&["2"], None),
                }
            }
        })
        .await;
        assert_eq!(items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["1", "2"]);
        assert!(complete);
    }

    #[tokio::test]
    async fn a_walk_that_stops_short_says_so() {
        // The same cursor handed back: a loop, not a month.
        let (items, complete, _) = collect(Duration::ZERO, |_| async { page(&["1"], Some("same")) }).await;
        assert_eq!(items.len(), 2);
        assert!(!complete);

        // A page that fails after others arrived keeps what came.
        let (items, complete, outcome) = collect(Duration::ZERO, |cursor| async move {
            if cursor.is_none() { page(&["1"], Some("c1")) } else { PageResult::Failed }
        })
        .await;
        assert_eq!(items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["1"]);
        assert!(!complete);
        assert_eq!(outcome, None);

        // Turned away on the first page: that is the answer.
        let (_, _, outcome) = collect(Duration::ZERO, |_| async { PageResult::SignedOut }).await;
        assert_eq!(outcome, Some(PageResult::SignedOut));

        // Authentication rejection after an answered page is still a rejection.
        let (_, complete, outcome) = collect(Duration::ZERO, |cursor| async move {
            if cursor.is_none() { page(&["1"], Some("next")) } else { PageResult::SignedOut }
        })
        .await;
        assert_eq!(outcome, Some(PageResult::SignedOut));
        assert!(!complete);
    }

    #[test]
    fn the_workspace_list_decodes() {
        let list: Vec<Workspace> = serde_json::from_str(r#"[{"id":"wrk_A","name":"Mine"},{"id":"wrk_B","name":null}]"#).unwrap();
        assert_eq!(list.iter().map(|w| w.id.as_str()).collect::<Vec<_>>(), ["wrk_A", "wrk_B"]);
        assert_eq!(list[0].name.as_deref(), Some("Mine"));
        assert_eq!(list[1].name, None);
    }

    #[test]
    fn a_page_of_html_is_a_signed_out_console() {
        assert!(looks_like_a_page(b"<!doctype html><title>Sign in</title>"));
        assert!(!looks_like_a_page(b"{\"error\":\"nope\"}"));
    }
}
