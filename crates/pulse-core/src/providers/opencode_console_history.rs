// Ported from upstream Sources/Pulse/Providers/OpenCodeConsoleHistory.swift and
// OpenCodeConsolePager.swift.
//! One workspace's compact request log, shared by warm-up, Settings and the card, and the bounded,
//! adaptive walk of the console's newest-first log that fills it.
//!
//! **The pager.** A page tells us both which interval is already covered and the density near its
//! older edge. Split off roughly one more page of time there, leaving the older remainder to
//! another worker. Quiet weeks cost one request, while a busy day can use several workers instead of
//! one long cursor chain.
//!
//! **The history.** Publishes saved/arriving rows while the bounded pager fills the gaps. A failed
//! interval stays pending on disk; meeting one known id cannot prove the other intervals were read.
//! Only complete reads get the short freshness window, including a genuinely empty account.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::task::JoinSet;

use super::opencode_console::{self as console, BoxFuture, Item, Page, PageResult, Read, Resolved, CONCURRENT_PAGES, PAGE_LIMIT};
use crate::spend::ledger::Ledger;
use crate::spend::Calendar;

/// An inclusive time interval of the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Range {
    pub since: DateTime<Utc>,
    pub until: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct PagerResult {
    pub pending: Vec<Range>,
    pub outcome: Option<PageResult>,
}

impl PagerResult {
    pub fn is_complete(&self) -> bool {
        self.pending.is_empty() && self.outcome.is_none()
    }
}

pub type FetchRange = Arc<dyn Fn(Range, Option<String>) -> BoxFuture<PageResult> + Send + Sync>;
pub type Receive = Arc<dyn Fn(Vec<Item>) -> BoxFuture<()> + Send + Sync>;

#[derive(Clone)]
struct Job {
    range: Range,
    cursor: Option<String>,
    seen: HashSet<String>,
}

impl Job {
    fn new(range: Range) -> Self {
        Self { range, cursor: None, seen: HashSet::new() }
    }
}

/// Bounded, adaptive walks of the console's newest-first request log. Injection points exercise the
/// actual scheduler against a synthetic paginated server, including failures and coincident times.
pub async fn read(ranges: Vec<Range>, page_limit: usize, retry_delay: Duration, fetch: FetchRange, receive: Receive) -> PagerResult {
    let mut set: JoinSet<(usize, PageResult)> = JoinSet::new();
    let mut queue: Vec<Job> = ranges.iter().copied().map(Job::new).collect();
    let mut next = 0;
    let mut issued = 0;
    let mut active: HashMap<usize, Job> = HashMap::new();
    let mut result = PagerResult::default();
    let mut saw_items = false;
    let mut failed = false;

    loop {
        while next < queue.len() && active.len() < CONCURRENT_PAGES && issued < page_limit {
            let job = queue[next].clone();
            next += 1;
            let id = issued;
            issued += 1;
            active.insert(id, job.clone());
            let fetch = fetch.clone();
            set.spawn(async move {
                let page = console::retry_page(retry_delay, || fetch(job.range, job.cursor.clone())).await;
                (id, page)
            });
        }
        let Some(Ok((id, reply))) = set.join_next().await else { break };
        let Some(mut job) = active.remove(&id) else { break };
        match reply {
            PageResult::SignedOut => {
                set.abort_all();
                return PagerResult { pending: ranges, outcome: Some(PageResult::SignedOut) };
            }
            PageResult::Failed => {
                result.pending.push(job.range);
                failed = true;
            }
            PageResult::Page(page) => {
                if !page.items.is_empty() {
                    saw_items = true;
                    receive(page.items.clone()).await;
                }
                let Some(cursor) = page.next_cursor.clone().filter(|c| !c.is_empty()).filter(|_| !page.items.is_empty()) else {
                    continue;
                };
                if !job.seen.insert(cursor.clone()) {
                    result.pending.push(job.range);
                    continue;
                }
                match split(&job.range, &page) {
                    Some(parts) => queue.extend(parts.into_iter().map(Job::new)),
                    None => {
                        // Same-millisecond bursts cannot be split by time. Keep the server's opaque
                        // cursor, never fabricate one.
                        job.cursor = Some(cursor);
                        queue.push(job);
                    }
                }
            }
        }
    }
    result.pending.extend(queue.iter().skip(next).map(|j| j.range));
    if failed && !saw_items {
        result.outcome = Some(PageResult::Failed);
    }
    result
}

fn split(range: &Range, page: &Page) -> Option<Vec<Range>> {
    let dates: Vec<DateTime<Utc>> = page.items.iter().map(Item::date).collect();
    if !dates.iter().all(|d| *d >= range.since && *d <= range.until) {
        return None;
    }
    let (oldest, newest) = (*dates.iter().min()?, *dates.iter().max()?);
    if !(oldest > range.since && oldest < range.until) {
        return None;
    }
    // Millisecond-aligned, inclusive bounds. Keep the oldest timestamp in the next ranges so a page
    // ending halfway through a timestamp loses nothing. The history merges boundary duplicates by
    // request id.
    let width = newest.timestamp_millis() - oldest.timestamp_millis();
    let pivot = DateTime::from_timestamp_millis(oldest.timestamp_millis() - width)?;
    if width < 1000 || !(pivot > range.since && pivot < oldest) {
        return None;
    }
    Some(vec![Range { since: pivot, until: oldest }, Range { since: range.since, until: pivot }])
}

// MARK: - The history

pub const FRESHNESS: Duration = Duration::from_secs(60);
/// How far behind the last read the next one starts (`ranges_to_read`).
pub const TAIL_OVERLAP_SECONDS: i64 = 15 * 60;

pub type Progress = Arc<dyn Fn(Ledger) + Send + Sync>;
pub type Resolve = Arc<dyn Fn(String) -> BoxFuture<Resolved> + Send + Sync>;
pub type FetchPage = Arc<dyn Fn(String, String, Range, Option<String>) -> BoxFuture<PageResult> + Send + Sync>;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Saved {
    org: String,
    items: Vec<Item>,
    complete: bool,
    // Optional only to preserve the original complete-cache format. An old incomplete cache has no
    // proven ranges and must walk the span.
    #[serde(default)]
    pending: Option<Vec<Range>>,
    #[serde(default)]
    checked_through: Option<DateTime<Utc>>,
}

#[derive(Default)]
struct State {
    items: HashMap<String, Item>,
    complete: bool,
    pending: Vec<Range>,
    checked_through: Option<DateTime<Utc>>,
    org: Option<String>,
    visible_cookie: Option<String>,
    observers: HashMap<u64, (String, Progress)>,
    next_observer: u64,
    /// Reads finished so far, and the last one's cookie and answer: a caller that waited on a read
    /// of its own session shares the answer instead of reading again.
    finished: u64,
    last: Option<(String, Read)>,
}

pub struct OpenCodeConsoleHistory {
    file: Option<PathBuf>,
    retry_delay: Duration,
    resolve: Resolve,
    fetch: FetchPage,
    state: Arc<Mutex<State>>,
    /// One read at a time.
    gate: tokio::sync::Mutex<()>,
}

impl OpenCodeConsoleHistory {
    /// Injected routes keep cache, progress and coalescing tests off real accounts.
    pub fn new(file: Option<PathBuf>, retry_delay: Duration, resolve: Resolve, fetch: FetchPage) -> Self {
        Self { file, retry_delay, resolve, fetch, state: Arc::new(Mutex::new(State::default())), gate: tokio::sync::Mutex::new(()) }
    }

    /// The real console, with the client the app last set (`console::set_http`).
    pub fn production(file: PathBuf) -> Self {
        let resolve: Resolve = Arc::new(|cookie| {
            Box::pin(async move { console::WorkspaceCache::shared().resolve(&console::http(), &cookie).await })
        });
        let fetch: FetchPage = Arc::new(|cookie, org, range, cursor| {
            Box::pin(async move { console::fetch_page(&console::http(), &cookie, &org, range.since, Some(range.until), cursor.as_deref()).await })
        });
        Self::new(Some(file), Duration::from_secs(1), resolve, fetch)
    }

    /// The history shared by the app: warm-up, Settings and the card.
    pub fn shared() -> &'static OpenCodeConsoleHistory {
        static SHARED: OnceLock<OpenCodeConsoleHistory> = OnceLock::new();
        SHARED.get_or_init(|| OpenCodeConsoleHistory::production(crate::paths::data_dir().join("opencode-console-log.json")))
    }

    /// The account's ledger, read (or reused) for the session `cookie`. `progress` hears the saved
    /// and arriving rows while the pager fills the gaps.
    pub async fn ledger(&self, cookie: &str, now: DateTime<Utc>, progress: Option<Progress>) -> Read {
        let observer = {
            let mut state = self.state.lock().unwrap();
            progress.map(|receive| {
                let id = state.next_observer;
                state.next_observer += 1;
                state.observers.insert(id, (cookie.to_string(), receive));
                id
            })
        };
        let _remove = ObserverGuard { state: &self.state, id: observer };
        let arrived = self.state.lock().unwrap().finished;

        let guard = match self.gate.try_lock() {
            Ok(guard) => guard,
            Err(_) => {
                // A read is running: hand a joiner what is in hand so far, then wait for it.
                self.publish_to(observer, cookie, now);
                self.gate.lock().await
            }
        };
        {
            let state = self.state.lock().unwrap();
            if state.finished > arrived {
                if let Some((kept, read)) = &state.last {
                    if kept == cookie {
                        return read.clone();
                    }
                }
            }
        }
        let read = self.read(cookie, now).await;
        {
            let mut state = self.state.lock().unwrap();
            state.finished += 1;
            state.last = Some((cookie.to_string(), read.clone()));
        }
        drop(guard);
        read
    }

    fn publish_to(&self, observer: Option<u64>, cookie: &str, now: DateTime<Utc>) {
        let Some(id) = observer else { return };
        let (receive, ledger) = {
            let state = self.state.lock().unwrap();
            if state.visible_cookie.as_deref() != Some(cookie) || (state.items.is_empty() && !state.complete) {
                return;
            }
            let Some((_, receive)) = state.observers.get(&id) else { return };
            (receive.clone(), snapshot(&state, now))
        };
        receive(ledger);
    }

    async fn read(&self, cookie: &str, now: DateTime<Utc>) -> Read {
        self.state.lock().unwrap().visible_cookie = None;
        let workspace = match (self.resolve)(cookie.to_string()).await {
            Resolved::Workspace(workspace) => workspace.id,
            Resolved::SignedOut => return Read::SignedOut,
            Resolved::Failed => return Read::Failed,
        };
        {
            let mut state = self.state.lock().unwrap();
            if state.org.as_deref() != Some(workspace.as_str()) {
                self.restore(&mut state, &workspace);
            }
            state.visible_cookie = Some(cookie.to_string());
        }
        if self.has_rows() {
            publish(&self.state, cookie, now);
        }

        {
            let state = self.state.lock().unwrap();
            if let (true, Some(through)) = (state.complete, state.checked_through) {
                if now >= through && (now - through).to_std().is_ok_and(|age| age < FRESHNESS) {
                    return Read::Answered(snapshot(&state, now));
                }
            }
        }

        let (ranges, was_complete) = {
            let mut state = self.state.lock().unwrap();
            let ranges = ranges_to_read(&state, now);
            let was_complete = state.complete;
            state.complete = false;
            (ranges, was_complete)
        };
        // A read dropped mid-way settled nothing: what was whole stays whole, so the next read
        // takes the tail rather than walking the month again.
        let mut restore = RestoreComplete { state: &self.state, was_complete, armed: true };

        let fetch = self.fetch.clone();
        let (fetch_cookie, fetch_org) = (cookie.to_string(), workspace.clone());
        let fetch_range: FetchRange = Arc::new(move |range, cursor| fetch(fetch_cookie.clone(), fetch_org.clone(), range, cursor));
        let (state, receive_cookie) = (self.state.clone(), cookie.to_string());
        let receive: Receive = Arc::new(move |page| {
            let (state, cookie) = (state.clone(), receive_cookie.clone());
            Box::pin(async move { receive_page(&state, page, &cookie, now) })
        });
        let result = read(ranges, PAGE_LIMIT, self.retry_delay, fetch_range, receive).await;
        restore.armed = false;

        let mut state = self.state.lock().unwrap();
        if result.outcome == Some(PageResult::SignedOut) {
            state.complete = was_complete;
            return Read::SignedOut;
        }
        state.pending = result.pending.clone();
        state.complete = result.is_complete();
        state.checked_through = Some(now);
        let horizon = now - chrono::Duration::seconds(console::SPAN_SECONDS + 86_400);
        state.items.retain(|_, item| item.date() >= horizon);
        self.save(&state);
        if result.outcome == Some(PageResult::Failed) && state.items.is_empty() {
            return Read::Failed;
        }
        Read::Answered(snapshot(&state, now))
    }

    fn has_rows(&self) -> bool {
        let state = self.state.lock().unwrap();
        !state.items.is_empty() || state.complete
    }

    fn restore(&self, state: &mut State, workspace: &str) {
        state.items.clear();
        state.complete = false;
        state.pending.clear();
        state.checked_through = None;
        state.org = Some(workspace.to_string());
        let Some(saved) = self.file.as_ref().and_then(|f| std::fs::read(f).ok()).and_then(|b| serde_json::from_slice::<Saved>(&b).ok()) else {
            return;
        };
        if saved.org != workspace {
            return;
        }
        state.items = saved.items.into_iter().map(|i| (i.id.clone(), i)).collect();
        state.pending = saved.pending.unwrap_or_default();
        state.complete = saved.complete && state.pending.is_empty();
        state.checked_through = saved.checked_through;
    }

    fn save(&self, state: &State) {
        let (Some(file), Some(org)) = (&self.file, &state.org) else { return };
        let saved = Saved {
            org: org.clone(),
            items: state.items.values().cloned().collect(),
            complete: state.complete,
            pending: Some(state.pending.clone()),
            checked_through: state.checked_through,
        };
        let Ok(bytes) = serde_json::to_vec(&saved) else { return };
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = file.with_extension("tmp");
        if std::fs::write(&tmp, bytes).is_ok() {
            let _ = std::fs::rename(&tmp, file);
        }
    }

    /// Forgets what was read, on disk too: for a session that was removed.
    pub fn forget(&self) {
        let mut state = self.state.lock().unwrap();
        *state = State { observers: std::mem::take(&mut state.observers), next_observer: state.next_observer, ..State::default() };
        if let Some(file) = &self.file {
            let _ = std::fs::remove_file(file);
        }
    }
}

struct ObserverGuard<'a> {
    state: &'a Mutex<State>,
    id: Option<u64>,
}

impl Drop for ObserverGuard<'_> {
    fn drop(&mut self) {
        if let Some(id) = self.id {
            self.state.lock().unwrap().observers.remove(&id);
        }
    }
}

struct RestoreComplete<'a> {
    state: &'a Mutex<State>,
    was_complete: bool,
    armed: bool,
}

impl Drop for RestoreComplete<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.state.lock().unwrap().complete = self.was_complete;
        }
    }
}

fn ranges_to_read(state: &State, now: DateTime<Utc>) -> Vec<Range> {
    let horizon = now - chrono::Duration::seconds(console::SPAN_SECONDS);
    if !state.complete && state.pending.is_empty() {
        return vec![Range { since: horizon, until: now }];
    }
    let mut ranges: Vec<Range> = state
        .pending
        .iter()
        .filter_map(|range| {
            let (start, end) = (horizon.max(range.since), now.min(range.until));
            (start <= end).then_some(Range { since: start, until: end })
        })
        .collect();
    // Use the last attempted upper bound even on an empty account. The pending intervals describe
    // its holes independently of the new tail.
    let latest = state.checked_through.or_else(|| state.items.values().map(Item::date).max()).unwrap_or(horizon);
    // The log is placed by when a request **started**, and its counts are only known once it ends:
    // a request still running at the last read began before it. Fifteen minutes back covers the
    // longest of them.
    let since = horizon.max(latest.min(now) - chrono::Duration::seconds(TAIL_OVERLAP_SECONDS));
    ranges.insert(0, Range { since, until: now });
    ranges
}

fn snapshot(state: &State, now: DateTime<Utc>) -> Ledger {
    let items: Vec<Item> = state.items.values().cloned().collect();
    let mut ledger = console::ledger(&items, now, &Calendar::local());
    ledger.has_partial_counts = !state.complete;
    ledger
}

fn receive_page(state: &Mutex<State>, page: Vec<Item>, cookie: &str, now: DateTime<Utc>) {
    let changed = {
        let mut state = state.lock().unwrap();
        let mut changed = false;
        for item in page {
            if state.items.get(&item.id) != Some(&item) {
                state.items.insert(item.id.clone(), item);
                changed = true;
            }
        }
        changed
    };
    if changed {
        publish(state, cookie, now);
    }
}

fn publish(state: &Mutex<State>, cookie: &str, now: DateTime<Utc>) {
    let (listeners, ledger) = {
        let state = state.lock().unwrap();
        let listeners: Vec<Progress> = state.observers.values().filter(|(c, _)| c == cookie).map(|(_, p)| p.clone()).collect();
        if listeners.is_empty() {
            return;
        }
        (listeners, snapshot(&state, now))
    };
    for listener in listeners {
        listener(ledger.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::opencode_console::PAGE_SIZE;
    use chrono::TimeZone;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Notify;

    fn base() -> DateTime<Utc> {
        Utc.timestamp_opt(1_790_856_000, 0).unwrap()
    }

    fn full_range() -> Range {
        Range { since: base() - chrono::Duration::seconds(console::SPAN_SECONDS), until: base() }
    }

    fn row(id: String, started_ms: f64) -> Item {
        Item {
            id,
            started_at: started_ms,
            product: Some("go".into()),
            model: Some("test".into()),
            input_tokens: Some(100),
            output_tokens: Some(20),
            cache_read_tokens: Some(900),
            cache_write_tokens: Some(0),
            cost: Some(0.002),
        }
    }

    fn rows(count: usize, spacing: f64, newest: DateTime<Utc>) -> Vec<Item> {
        (0..count).map(|i| row(format!("request-{i}"), newest.timestamp_millis() as f64 - i as f64 * spacing * 1000.0)).collect()
    }

    /// An original synthetic server: inclusive time bounds, opaque cursors and descending
    /// timestamps, with the same 100-row page size as the console.
    struct Server {
        rows: Vec<Item>,
        requests: Mutex<Vec<(Range, Option<String>)>>,
        active: AtomicUsize,
        peak: AtomicUsize,
        fail_until: Mutex<Option<DateTime<Utc>>>,
        /// Requests after this many wait for `gate`.
        hold_after: usize,
        gate: Option<Arc<Gate>>,
        arrived: Notify,
    }

    #[derive(Default)]
    struct Gate {
        opened: Mutex<bool>,
        notify: Notify,
    }

    impl Gate {
        async fn wait(&self) {
            loop {
                let notified = self.notify.notified();
                if *self.opened.lock().unwrap() {
                    return;
                }
                notified.await;
            }
        }
        fn open(&self) {
            *self.opened.lock().unwrap() = true;
            self.notify.notify_waiters();
        }
    }

    impl Server {
        fn new(mut rows: Vec<Item>) -> Arc<Self> {
            Self::gated(rows.drain(..).collect(), None, usize::MAX)
        }

        fn gated(mut rows: Vec<Item>, gate: Option<Arc<Gate>>, hold_after: usize) -> Arc<Self> {
            rows.sort_by(|a, b| b.started_at.partial_cmp(&a.started_at).unwrap().then_with(|| a.id.cmp(&b.id)));
            Arc::new(Self {
                rows,
                requests: Mutex::new(Vec::new()),
                active: AtomicUsize::new(0),
                peak: AtomicUsize::new(0),
                fail_until: Mutex::new(None),
                hold_after,
                gate,
                arrived: Notify::new(),
            })
        }

        fn count(&self) -> usize {
            self.requests.lock().unwrap().len()
        }

        async fn wait_for_requests(&self, count: usize) {
            loop {
                let notified = self.arrived.notified();
                if self.count() >= count {
                    return;
                }
                notified.await;
            }
        }

        async fn fetch(self: &Arc<Self>, range: Range, cursor: Option<String>) -> PageResult {
            let seen = {
                let mut requests = self.requests.lock().unwrap();
                requests.push((range, cursor.clone()));
                requests.len()
            };
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(active, Ordering::SeqCst);
            self.arrived.notify_waiters();
            if seen > self.hold_after {
                if let Some(gate) = &self.gate {
                    gate.wait().await;
                }
            }
            // Yield so independent requests can overlap; tests assert the bound, never a
            // machine-dependent elapsed time.
            tokio::time::sleep(Duration::from_millis(1)).await;
            self.active.fetch_sub(1, Ordering::SeqCst);
            if self.fail_until.lock().unwrap().is_some_and(|until| range.until <= until) {
                return PageResult::Failed;
            }
            let (since, until) = (console::milliseconds(range.since) as f64, console::milliseconds(range.until) as f64);
            let matched: Vec<&Item> = self.rows.iter().filter(|r| r.started_at >= since && r.started_at <= until).collect();
            let offset = cursor.and_then(|c| c.parse::<usize>().ok()).unwrap_or(0);
            let page: Vec<Item> = matched.iter().skip(offset).take(PAGE_SIZE).map(|r| (*r).clone()).collect();
            let next = offset + page.len();
            PageResult::Page(Page { items: page, next_cursor: (next < matched.len()).then(|| next.to_string()) })
        }
    }

    #[derive(Default)]
    struct Received {
        items: Mutex<HashMap<String, Item>>,
        pages: AtomicUsize,
    }

    fn fetcher(server: &Arc<Server>) -> FetchRange {
        let server = server.clone();
        Arc::new(move |range, cursor| {
            let server = server.clone();
            Box::pin(async move { server.fetch(range, cursor).await })
        })
    }

    fn receiver(received: &Arc<Received>) -> Receive {
        let received = received.clone();
        Arc::new(move |page| {
            let received = received.clone();
            Box::pin(async move {
                received.pages.fetch_add(1, Ordering::SeqCst);
                let mut items = received.items.lock().unwrap();
                for item in page {
                    items.insert(item.id.clone(), item);
                }
            })
        })
    }

    async fn run(server: &Arc<Server>, received: &Arc<Received>, ranges: Vec<Range>, limit: usize) -> PagerResult {
        read(ranges, limit, Duration::ZERO, fetcher(server), receiver(received)).await
    }

    fn tokens(ledger: &Ledger) -> i64 {
        ledger.all_time().tokens
    }

    // MARK: Paging

    #[tokio::test]
    async fn quiet_histories_cost_one_request_instead_of_31_daily_requests() {
        for count in [0, 3, 99] {
            let server = Server::new(rows(count, 60.0, base()));
            let received = Arc::new(Received::default());
            let result = run(&server, &received, vec![full_range()], PAGE_LIMIT).await;
            assert!(result.is_complete());
            assert_eq!(server.count(), 1);
            assert_eq!(received.items.lock().unwrap().len(), count);
        }
    }

    #[tokio::test]
    async fn a_busy_day_shares_workers_with_exact_totals_across_inclusive_boundaries() {
        let all = rows(2_400, 5.0, base() - chrono::Duration::days(10));
        let server = Server::new(all.clone());
        let received = Arc::new(Received::default());
        let result = run(&server, &received, vec![full_range()], PAGE_LIMIT).await;
        assert!(result.is_complete());
        assert_eq!(received.items.lock().unwrap().len(), all.len());
        assert!(server.peak.load(Ordering::SeqCst) > 1);
        assert!(server.peak.load(Ordering::SeqCst) <= CONCURRENT_PAGES);
        // The former daily walk needs 31 initial pages plus 23 continuations.
        assert!(server.count() < 54, "{} requests", server.count());
        let items: Vec<Item> = received.items.lock().unwrap().values().cloned().collect();
        let ledger = console::ledger(&items, base(), &Calendar::utc(2));
        assert_eq!(tokens(&ledger), all.len() as i64 * 1_020);
        assert!((ledger.all_time().cost - all.len() as f64 * 0.002).abs() < 1e-6);
    }

    #[tokio::test]
    async fn more_than_a_page_at_the_same_timestamp_follows_cursors_without_dropping_requests() {
        let server = Server::new(rows(350, 0.0, base()));
        let received = Arc::new(Received::default());
        let result = run(&server, &received, vec![full_range()], PAGE_LIMIT).await;
        assert!(result.is_complete());
        assert_eq!(received.items.lock().unwrap().len(), 350);
        assert_eq!(server.count(), 4);
        let cursors: Vec<String> = server.requests.lock().unwrap().iter().filter_map(|(_, c)| c.clone()).collect();
        assert_eq!(cursors, ["100", "200", "300"]);
    }

    #[tokio::test]
    async fn a_global_page_budget_leaves_resumable_intervals_never_a_falsely_complete_scan() {
        let all = rows(1_000, 60.0, base());
        let server = Server::new(all.clone());
        let received = Arc::new(Received::default());
        let first = run(&server, &received, vec![full_range()], 3).await;
        assert!(!first.is_complete());
        assert_eq!(server.count(), 3);
        let second = run(&server, &received, first.pending, PAGE_LIMIT).await;
        assert!(second.is_complete());
        assert_eq!(received.items.lock().unwrap().len(), all.len());
    }

    #[tokio::test]
    async fn splitting_through_a_shared_millisecond_cannot_round_its_remaining_requests_out_of_the_query() {
        // The first page ends inside 150 requests at one millisecond.
        let boundary = DateTime::from_timestamp_millis(2_147_483_648_002).unwrap();
        let older: Vec<Item> = (0..150).map(|i| row(format!("edge-{i}"), 2_147_483_648_002.0)).collect();
        let mut all = rows(99, 0.1, boundary + chrono::Duration::seconds(100));
        all.extend(older);
        let server = Server::new(all.clone());
        let received = Arc::new(Received::default());
        let range = Range { since: boundary - chrono::Duration::days(1), until: boundary + chrono::Duration::seconds(200) };
        let result = run(&server, &received, vec![range], PAGE_LIMIT).await;
        assert!(result.is_complete());
        assert_eq!(received.items.lock().unwrap().len(), all.len());
    }

    #[tokio::test]
    async fn adaptive_cursor_fallback_preserves_authentication_failure_and_rejects_cursor_loops() {
        let one = |ids: usize, next: &'static str| {
            PageResult::Page(Page { items: rows(ids, 1.0, base()), next_cursor: Some(next.to_string()) })
        };
        let received = Arc::new(Received::default());
        let signed_out: FetchRange = Arc::new(move |_, cursor| {
            Box::pin(async move { if cursor.is_none() { one(1, "next") } else { PageResult::SignedOut } })
        });
        let rejected = read(vec![full_range()], PAGE_LIMIT, Duration::ZERO, signed_out, receiver(&received)).await;
        assert_eq!(rejected.outcome, Some(PageResult::SignedOut));
        assert!(!rejected.is_complete());
        assert_eq!(received.pages.load(Ordering::SeqCst), 1);

        let looping = Arc::new(Received::default());
        let same: FetchRange = Arc::new(move |_, _| Box::pin(async move { one(1, "same") }));
        let result = read(vec![full_range()], PAGE_LIMIT, Duration::ZERO, same, receiver(&looping)).await;
        assert!(!result.is_complete());
        assert_eq!(result.pending, vec![full_range()]);
        assert_eq!(looping.pages.load(Ordering::SeqCst), 2);
    }

    // MARK: History

    fn history(server: &Arc<Server>, file: Option<PathBuf>) -> OpenCodeConsoleHistory {
        let resolve: Resolve = Arc::new(|cookie| {
            Box::pin(async move { Resolved::Workspace(console::Workspace { id: cookie, name: None }) })
        });
        let fetch = {
            let server = server.clone();
            Arc::new(move |_cookie: String, _org: String, range: Range, cursor: Option<String>| -> BoxFuture<PageResult> {
                let server = server.clone();
                Box::pin(async move { server.fetch(range, cursor).await })
            }) as FetchPage
        };
        OpenCodeConsoleHistory::new(file, Duration::ZERO, resolve, fetch)
    }

    fn answered(read: Read) -> Ledger {
        match read {
            Read::Answered(ledger) => ledger,
            other => panic!("expected an answer, got {other:?}"),
        }
    }

    fn tail() -> chrono::Duration {
        chrono::Duration::seconds(TAIL_OVERLAP_SECONDS)
    }

    #[tokio::test]
    async fn complete_caches_including_empty_accounts_skip_repeated_reads_and_survive_restart() {
        for count in [0usize, 5] {
            let root = tempfile::tempdir().unwrap();
            let file = root.path().join("history.json");
            let server = Server::new(rows(count, 60.0, base()));
            let reader = history(&server, Some(file.clone()));
            let now = base();
            reader.ledger("org", now, None).await;
            assert_eq!(server.count(), 1);
            let second = answered(reader.ledger("org", now + chrono::Duration::seconds(10), None).await);
            assert_eq!(tokens(&second), count as i64 * 1_020);
            assert!(!second.has_partial_counts);
            assert_eq!(server.count(), 1);
            history(&server, Some(file)).ledger("org", now + chrono::Duration::seconds(20), None).await;
            assert_eq!(server.count(), 1);
            reader.ledger("org", now + chrono::Duration::seconds(120), None).await;
            let requests = server.requests.lock().unwrap().clone();
            assert_eq!(requests.len(), 2);
            assert_eq!(requests.last().unwrap().0.since, now - tail());
        }
    }

    #[tokio::test]
    async fn a_saved_partial_scan_retries_its_holes_overlap_with_known_ids_cannot_hide_missing_history() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("history.json");
        let now = base();
        let all = rows(250, 3600.0, now);
        let server = Server::new(all.clone());
        let boundary = now - chrono::Duration::hours(198);
        *server.fail_until.lock().unwrap() = Some(boundary);
        let first = answered(history(&server, Some(file.clone())).ledger("org", now, None).await);
        assert!(first.has_partial_counts);
        assert!(tokens(&first) > 0);
        assert!(tokens(&first) < all.len() as i64 * 1_020);
        let before = server.count();
        let reader = history(&server, Some(file));
        let second = answered(reader.ledger("org", now + chrono::Duration::seconds(120), None).await);
        assert!(second.has_partial_counts);
        let retries: Vec<(Range, Option<String>)> = server.requests.lock().unwrap().iter().skip(before).cloned().collect();
        assert!(retries.iter().all(|(r, _)| r.until <= boundary || r.since >= now + chrono::Duration::seconds(120) - tail() - chrono::Duration::seconds(120)));
        *server.fail_until.lock().unwrap() = None;
        let last = answered(reader.ledger("org", now + chrono::Duration::seconds(240), None).await);
        assert!(!last.has_partial_counts);
        assert_eq!(tokens(&last), all.len() as i64 * 1_020);
    }

    #[tokio::test]
    async fn the_original_complete_disk_cache_shape_upgrades_with_only_an_incremental_request() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("history.json");
        let all = rows(5, 60.0, base() - chrono::Duration::seconds(600));
        std::fs::write(&file, serde_json::to_vec(&serde_json::json!({"org": "org", "complete": true, "items": all})).unwrap()).unwrap();
        let server = Server::new(all.clone());
        let result = answered(history(&server, Some(file)).ledger("org", base(), None).await);
        assert!(!result.has_partial_counts);
        assert_eq!(tokens(&result), 5 * 1_020);
        assert_eq!(server.count(), 1);
        assert_eq!(server.requests.lock().unwrap()[0].0.since, all[0].date() - tail());
    }

    #[tokio::test]
    async fn a_cache_from_a_different_workspace_never_appears_in_progress_or_totals() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("history.json");
        history(&Server::new(rows(5, 60.0, base())), Some(file.clone())).ledger("first", base(), None).await;
        let heard = Arc::new(Mutex::new(Vec::<Ledger>::new()));
        let sink = heard.clone();
        let server = Server::new(Vec::new());
        let read = history(&server, Some(file))
            .ledger("other", base(), Some(Arc::new(move |l| sink.lock().unwrap().push(l))))
            .await;
        assert_eq!(tokens(&answered(read)), 0);
        assert!(heard.lock().unwrap().is_empty());
        assert_eq!(server.requests.lock().unwrap()[0].0, full_range());
    }

    #[tokio::test]
    async fn settings_joining_a_warmup_gets_its_current_partial_snapshot_before_the_remaining_pages() {
        let gate = Arc::new(Gate::default());
        let server = Server::gated(rows(300, 60.0, base()), Some(gate.clone()), 1);
        let reader = Arc::new(history(&server, None));
        let warmup = tokio::spawn({
            let reader = reader.clone();
            async move { reader.ledger("org", base(), None).await }
        });
        server.wait_for_requests(2).await;
        let heard = Arc::new(Mutex::new(Vec::<Ledger>::new()));
        let notify = Arc::new(Notify::new());
        let (sink, ping) = (heard.clone(), notify.clone());
        let pane = tokio::spawn({
            let reader = reader.clone();
            async move {
                reader
                    .ledger("org", base(), Some(Arc::new(move |l| {
                        sink.lock().unwrap().push(l);
                        ping.notify_one();
                    })))
                    .await
            }
        });
        notify.notified().await;
        {
            let heard = heard.lock().unwrap();
            assert!(heard[0].has_partial_counts);
            assert_eq!(tokens(&heard[0]), 100 * 1_020);
        }
        gate.open();
        let (a, b) = (answered(warmup.await.unwrap()), answered(pane.await.unwrap()));
        assert_eq!(a, b);
        assert!(!b.has_partial_counts);
        assert_eq!(tokens(&b), 300 * 1_020);
    }

    #[tokio::test]
    async fn saved_figures_arrive_before_a_slow_incremental_request_finishes() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("history.json");
        let all = rows(5, 60.0, base());
        history(&Server::new(all.clone()), Some(file.clone())).ledger("org", base(), None).await;
        let gate = Arc::new(Gate::default());
        let server = Server::gated(all, Some(gate.clone()), 0);
        let reader = history(&server, Some(file));
        let heard = Arc::new(Mutex::new(Vec::<Ledger>::new()));
        let sink = heard.clone();
        let read = reader.ledger("org", base() + chrono::Duration::seconds(120), Some(Arc::new(move |l| sink.lock().unwrap().push(l))));
        let release = async {
            server.wait_for_requests(1).await;
            assert_eq!(tokens(&heard.lock().unwrap()[0]), 5 * 1_020);
            gate.open();
        };
        let (_, ()) = tokio::join!(read, release);
    }

    #[tokio::test]
    async fn a_different_session_cannot_join_another_workspaces_in_flight_result() {
        let gate = Arc::new(Gate::default());
        let server = Server::gated(rows(1, 60.0, base()), Some(gate.clone()), 0);
        let resolve: Resolve = Arc::new(|cookie| Box::pin(async move { Resolved::Workspace(console::Workspace { id: cookie, name: None }) }));
        let fetch = {
            let server = server.clone();
            Arc::new(move |_c: String, org: String, range: Range, cursor: Option<String>| -> BoxFuture<PageResult> {
                let server = server.clone();
                Box::pin(async move {
                    if org == "other" {
                        PageResult::Page(Page { items: vec![], next_cursor: None })
                    } else {
                        server.fetch(range, cursor).await
                    }
                })
            }) as FetchPage
        };
        let reader = Arc::new(OpenCodeConsoleHistory::new(None, Duration::ZERO, resolve, fetch));
        let first = tokio::spawn({
            let reader = reader.clone();
            async move { reader.ledger("first", base(), None).await }
        });
        server.wait_for_requests(1).await;
        let other = tokio::spawn({
            let reader = reader.clone();
            async move { reader.ledger("other", base(), None).await }
        });
        gate.open();
        assert_eq!(tokens(&answered(first.await.unwrap())), 1_020);
        assert_eq!(tokens(&answered(other.await.unwrap())), 0);
    }

    #[tokio::test]
    async fn a_signed_out_console_is_the_answer_and_keeps_what_was_whole_whole() {
        let server = Server::new(rows(3, 60.0, base()));
        let resolve: Resolve = Arc::new(|cookie| Box::pin(async move { Resolved::Workspace(console::Workspace { id: cookie, name: None }) }));
        let fetch: FetchPage = Arc::new(|_, _, _, _| Box::pin(async { PageResult::SignedOut }));
        let reader = OpenCodeConsoleHistory::new(None, Duration::ZERO, resolve, fetch);
        assert_eq!(reader.ledger("org", base(), None).await, Read::SignedOut);
        // And an unresolvable session is a failure or a sign-out of its own.
        let gone: Resolve = Arc::new(|_| Box::pin(async { Resolved::SignedOut }));
        let reader = OpenCodeConsoleHistory::new(None, Duration::ZERO, gone, Arc::new(|_, _, _, _| Box::pin(async { PageResult::Failed })));
        assert_eq!(reader.ledger("org", base(), None).await, Read::SignedOut);
        drop(server);
    }
}
