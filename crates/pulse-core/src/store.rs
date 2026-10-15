// Ported in outline from upstream Usage/UsageStore.swift and UsageCache.swift.
//! Holds the latest reading per account, runs refresh passes, and decides the
//! next one. Rules carried over:
//! - Disabled accounts are never fetched; an empty selection fetches nothing.
//! - A failed fetch keeps the last good reading, shown as stale (the cache).
//! - Generation stamps: a slow, older fetch never overwrites a newer reading.
//! - A provider with nothing fetched yet is not seeded as `loading` forever:
//!   it starts `loading` and resolves on the first pass.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::model::{AccountKey, ProviderUsage, Unavailability, UsageState, UsageWindow};
use crate::spend::elsewhere::ElsewhereWatch;
use crate::refresh::{self, Signals};
use crate::secrets::SecretStore;
use crate::service::{FetchContext, Registry};
use crate::settings::AppSettings;

#[derive(Default)]
struct Entry {
    usage: Option<ProviderUsage>,
    /// Last live reading, kept to stand in when a fetch fails.
    last_good: Option<ProviderUsage>,
    generation: u64,
    refreshing: bool,
    /// What the provider's service itself returned on the latest pass, before a
    /// failure was swapped for the last good figures. Alerts are judged on it.
    raw: Option<ProviderUsage>,
    /// Whether `raw` has landed since the alert engine last took it.
    raw_unseen: bool,
}

/// Windows difference: the longest one account's whole fetch may take. The shared HTTP client
/// times out single requests, but a provider may chain several requests or wait on a local CLI;
/// past this the pass ends as unreachable (the last good figures stay, marked stale).
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(60);

pub struct UsageStore {
    registry: Registry,
    secrets: Arc<dyn SecretStore>,
    cache_path: Option<PathBuf>,
    inner: Mutex<Inner>,
    /// Limits seen being spent where this PC's logs cannot see (`used-elsewhere.json`).
    elsewhere: Arc<ElsewhereWatch>,
    /// Told whenever an account starts or stops refreshing, so the windows follow each account.
    on_change: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    fetch_timeout: Duration,
}

/// Ends an account's refresh however the fetch ends: answered, timed out, cancelled or panicked.
struct Refreshing<'a> {
    store: &'a UsageStore,
    account_id: &'a str,
    generation: u64,
}

impl Drop for Refreshing<'_> {
    fn drop(&mut self) {
        {
            let mut inner = self.store.inner.lock().unwrap_or_else(|e| e.into_inner());
            match inner.entries.get_mut(self.account_id) {
                // A newer pass owns the flag now.
                Some(entry) if entry.generation == self.generation => entry.refreshing = false,
                _ => return,
            }
        }
        self.store.changed();
    }
}

#[derive(Default)]
struct Inner {
    entries: BTreeMap<String, Entry>,
    next_generation: u64,
    signals: Signals,
}

impl UsageStore {
    pub fn new(registry: Registry, secrets: Arc<dyn SecretStore>, cache_path: Option<PathBuf>) -> Self {
        let store = Self {
            registry,
            secrets,
            cache_path,
            inner: Mutex::new(Inner::default()),
            elsewhere: Arc::new(ElsewhereWatch::new(None)),
            on_change: Mutex::new(None),
            fetch_timeout: FETCH_TIMEOUT,
        };
        store.restore_cache();
        store
    }

    /// Called (with no lock held) when an account starts refreshing and when it has finished.
    pub fn set_on_change(&self, on_change: impl Fn() + Send + Sync + 'static) {
        *self.on_change.lock().unwrap() = Some(Arc::new(on_change));
    }

    fn changed(&self) {
        let on_change = self.on_change.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some(on_change) = on_change {
            on_change();
        }
    }

    /// Keeps what the elsewhere watch has seen in `file`, across launches.
    pub fn with_elsewhere_file(mut self, file: PathBuf) -> Self {
        self.elsewhere = Arc::new(ElsewhereWatch::new(Some(file)));
        self
    }

    /// Whether this window's current cycle was seen spent where this PC's logs cannot see
    /// (upstream `UsageStore.usedElsewhere`): its value is not estimated.
    pub fn used_elsewhere(&self, window: &UsageWindow, account: &AccountKey) -> bool {
        self.elsewhere.used_elsewhere(window, account)
    }

    /// Readings for the given account ids, in that order. Accounts never fetched read `loading`.
    pub fn snapshot(&self, accounts: &[String]) -> Vec<ProviderUsage> {
        let inner = self.inner.lock().unwrap();
        let now = Utc::now();
        accounts
            .iter()
            .filter_map(|id| {
                let key = AccountKey::from_id(id)?;
                let entry = inner.entries.get(id);
                let usage = entry.and_then(|e| e.usage.clone()).and_then(|u| match u.state {
                    UsageState::Unavailable(_) => Some(u),
                    _ => u.current(now),
                });
                Some(usage.unwrap_or_else(|| ProviderUsage::unavailable(key, Unavailability::Loading)))
            })
            .collect()
    }

    pub fn refreshing(&self) -> Vec<String> {
        let inner = self.inner.lock().unwrap();
        inner.entries.iter().filter(|(_, e)| e.refreshing).map(|(k, _)| k.clone()).collect()
    }

    /// Passes that landed since the last call, as `(shown, raw)`: what the panel
    /// displays and what the provider actually returned. Each is handed out once,
    /// so a failure is counted once per pass however many callers ask.
    pub fn take_observations(&self) -> Vec<(ProviderUsage, ProviderUsage)> {
        let mut inner = self.inner.lock().unwrap();
        inner
            .entries
            .values_mut()
            .filter_map(|e| {
                if !std::mem::take(&mut e.raw_unseen) {
                    return None;
                }
                Some((e.usage.clone()?, e.raw.clone()?))
            })
            .collect()
    }

    /// Live readings as they stand, each its own raw: for re-judging what is already
    /// in hand after a notification setting changes. Not a pass, so no failure is counted.
    pub fn live_readings(&self) -> Vec<ProviderUsage> {
        let inner = self.inner.lock().unwrap();
        inner
            .entries
            .values()
            .filter_map(|e| e.usage.clone())
            .filter(|u| u.state == UsageState::Live)
            .collect()
    }

    pub fn note_looked(&self) {
        self.inner.lock().unwrap().signals.last_looked = Some(Utc::now());
    }

    pub fn set_panel_visible(&self, visible: bool) {
        self.inner.lock().unwrap().signals.is_panel_visible = visible;
    }

    pub fn set_agent_activity(&self, last_write: Option<DateTime<Utc>>) {
        self.inner.lock().unwrap().signals.last_agent_activity = last_write;
    }

    /// Seconds until the next pass should run.
    pub fn next_interval(&self, settings: &AppSettings) -> i64 {
        match settings.refresh_interval {
            crate::settings::RefreshInterval::Fixed(minutes) => (minutes.max(1) as i64) * 60,
            crate::settings::RefreshInterval::Adaptive => {
                let signals = self.inner.lock().unwrap().signals.clone();
                refresh::interval(&signals, true, Utc::now())
            }
        }
    }

    /// Fetch one account. Returns true when its reading changed.
    pub async fn refresh(&self, settings: Arc<AppSettings>, account_id: &str) -> bool {
        if !settings.enabled_accounts.contains(account_id) {
            return false;
        }
        let Some(key) = AccountKey::from_id(account_id) else { return false };
        let generation = {
            let mut inner = self.inner.lock().unwrap();
            inner.next_generation += 1;
            let generation = inner.next_generation;
            let entry = inner.entries.entry(account_id.to_string()).or_default();
            entry.generation = generation;
            entry.refreshing = true;
            generation
        };
        let _refreshing = Refreshing { store: self, account_id, generation };
        self.changed();

        let fetched = match self.registry.get(&key.provider) {
            Some(service) => {
                let ctx = FetchContext::from_env(settings.clone(), self.secrets.clone());
                match tokio::time::timeout(self.fetch_timeout, service.fetch(&ctx, &key)).await {
                    Ok(fetched) => fetched,
                    Err(_) => {
                        tracing::warn!(account = account_id, "usage fetch timed out after {:?}", self.fetch_timeout);
                        ProviderUsage::unavailable(key.clone(), Unavailability::Unreachable)
                    }
                }
            }
            None => ProviderUsage::unavailable(key.clone(), Unavailability::NoLimitsReported),
        };

        // Only with Token spend on: the watch compares the readings with what this PC's own
        // records say was spent, and reads nothing otherwise.
        let checks = if settings.reads_token_spend {
            self.elsewhere.observe(&fetched, &key, Utc::now())
        } else {
            Vec::new()
        };
        if !checks.is_empty() {
            let watch = self.elsewhere.clone();
            let identity = fetched.spend_identity.clone();
            let primary = key.is_primary();
            // Blocking work: the transcripts are read for the span each check names.
            tokio::task::spawn_blocking(move || {
                let home = std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_default();
                for check in checks {
                    let ledger = if primary {
                        // The default account is the local CLI login itself, as upstream.
                        let Ok(ledger) = crate::spend::read_ledger(check.provider, &home, Utc::now()) else { continue };
                        ledger
                    } else {
                        // Windows difference: another account's local work is not evidence about
                        // this account. Unknown ownership must not persist an elsewhere mark either.
                        let Some((local, ledger)) = crate::spend::account::read_ledger(check.provider, identity.as_ref(), &home, Utc::now()) else { continue };
                        if !local.covers(check.from, check.to) || !local.unchanged() {
                            continue;
                        }
                        ledger
                    };
                    watch.resolve(&check, ledger.cost_between(check.from, check.to), ledger.tokens_between(check.from, check.to));
                }
            });
        }

        let changed = {
            let mut inner = self.inner.lock().unwrap();
            let entry = inner.entries.entry(account_id.to_string()).or_default();
            if entry.generation != generation {
                // A newer pass started after this one; its answer wins.
                return false;
            }
            entry.raw = Some(fetched.clone());
            entry.raw_unseen = true;
            let shown = merge(fetched, entry.last_good.as_ref(), Utc::now());
            if shown.state == UsageState::Live {
                entry.last_good = Some(shown.clone());
            }
            let changed = entry.usage.as_ref().map(|u| &u.windows) != Some(&shown.windows)
                || entry.usage.as_ref().map(|u| u.state) != Some(shown.state);
            entry.usage = Some(shown);
            if changed {
                inner.signals.last_change = Some(Utc::now());
            }
            changed
        };
        if changed {
            self.save_cache();
        }
        changed
    }

    /// Refresh every enabled account concurrently.
    pub async fn refresh_all(self: &Arc<Self>, settings: Arc<AppSettings>) -> bool {
        let ids = settings.ordered_enabled_accounts();
        let tasks: Vec<_> = ids
            .into_iter()
            .map(|id| {
                let store = self.clone();
                let settings = settings.clone();
                tokio::spawn(async move { store.refresh(settings, &id).await })
            })
            .collect();
        let mut any = false;
        for t in tasks {
            match t.await {
                Ok(changed) => any |= changed,
                // The account's own refresh flag was cleared as the task unwound.
                Err(error) => tracing::error!("usage refresh task failed: {error}"),
            }
        }
        any
    }

    fn restore_cache(&self) {
        let Some(path) = &self.cache_path else { return };
        let Ok(bytes) = std::fs::read(path) else { return };
        let Ok(saved) = serde_json::from_slice::<Vec<ProviderUsage>>(&bytes) else { return };
        let now = Utc::now();
        let mut inner = self.inner.lock().unwrap();
        for mut usage in saved {
            let Some(current) = usage.current(now) else { continue };
            usage = current;
            usage.state = UsageState::Stale;
            usage.is_cached = true;
            let entry = inner.entries.entry(usage.account.id()).or_default();
            entry.last_good = Some(usage.clone());
            entry.usage = Some(usage);
        }
    }

    fn save_cache(&self) {
        let Some(path) = &self.cache_path else { return };
        let saved: Vec<ProviderUsage> = {
            let inner = self.inner.lock().unwrap();
            inner.entries.values().filter_map(|e| e.last_good.clone()).collect()
        };
        if let Ok(bytes) = serde_json::to_vec(&saved) {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(path, bytes);
        }
    }
}

/// What to show after a fetch: the fetch itself when it says something, else the
/// last good reading marked stale — except for answers that are not failures.
fn merge(fetched: ProviderUsage, last_good: Option<&ProviderUsage>, now: DateTime<Utc>) -> ProviderUsage {
    let UsageState::Unavailable(reason) = fetched.state else { return fetched };
    let is_failure = matches!(
        reason,
        Unavailability::Unreachable
            | Unavailability::UnreadableReply
            | Unavailability::RateLimited
            | Unavailability::ServerError
    );
    if !is_failure {
        return fetched;
    }
    match last_good.and_then(|g| g.current(now)) {
        Some(mut stale) => {
            stale.state = UsageState::Stale;
            stale.is_cached = true;
            stale
        }
        None => fetched,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{UsageWindow, WindowKind};
    use crate::provider::Provider;

    #[test]
    fn transient_failure_shows_last_good_as_stale() {
        let now = Utc::now();
        let key = AccountKey::primary(Provider::Codex);
        let good = ProviderUsage::live(key.clone(), vec![UsageWindow::new("w", WindowKind::Weekly, 0.3, 604_800)], now);
        let failed = ProviderUsage::unavailable(key.clone(), Unavailability::Unreachable);
        let shown = merge(failed, Some(&good), now);
        assert_eq!(shown.state, UsageState::Stale);
        assert_eq!(shown.windows.len(), 1);

        let refused = ProviderUsage::unavailable(key, Unavailability::ApiKeyRefused);
        assert_eq!(merge(refused, Some(&good), now).state, UsageState::Unavailable(Unavailability::ApiKeyRefused));
    }

    struct Scripted(Mutex<Vec<ProviderUsage>>);

    #[async_trait::async_trait]
    impl crate::service::UsageService for Scripted {
        fn provider(&self) -> Provider {
            Provider::Codex
        }
        async fn fetch(&self, _: &FetchContext, _: &AccountKey) -> ProviderUsage {
            self.0.lock().unwrap().remove(0)
        }
    }

    #[tokio::test]
    async fn each_pass_is_handed_to_alerts_once_with_the_raw_answer() {
        let key = AccountKey::primary(Provider::Codex);
        let good = ProviderUsage::live(key.clone(), vec![UsageWindow::new("w", WindowKind::Weekly, 0.3, 604_800)], Utc::now());
        let down = ProviderUsage::unavailable(key.clone(), Unavailability::Unreachable);
        let service: Arc<dyn crate::service::UsageService> = Arc::new(Scripted(Mutex::new(vec![good, down])));
        let registry: Registry = [(Provider::Codex, service)].into();
        let store = UsageStore::new(registry, Arc::new(crate::secrets::MemorySecrets::default()), None);
        let mut settings = AppSettings::default();
        settings.enabled_accounts.insert(key.id());
        let settings = Arc::new(settings);

        assert!(store.take_observations().is_empty());
        store.refresh(settings.clone(), &key.id()).await;
        let first = store.take_observations();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].1.state, UsageState::Live);
        assert!(store.take_observations().is_empty(), "handed out once");
        assert_eq!(store.live_readings().len(), 1);

        // The panel keeps the last good figures, marked stale; the raw answer is the failure.
        store.refresh(settings, &key.id()).await;
        let second = store.take_observations();
        assert_eq!(second[0].0.state, UsageState::Stale);
        assert_eq!(second[0].1.state, UsageState::Unavailable(Unavailability::Unreachable));
        assert!(store.live_readings().is_empty());
    }

    /// Never answers, or panics, as a hung or broken provider would.
    struct Broken(bool);

    #[async_trait::async_trait]
    impl crate::service::UsageService for Broken {
        fn provider(&self) -> Provider {
            Provider::Codex
        }
        async fn fetch(&self, _: &FetchContext, _: &AccountKey) -> ProviderUsage {
            if self.0 {
                panic!("provider failed");
            }
            std::future::pending().await
        }
    }

    /// What `refreshing()` read each time the store said something changed.
    type Seen = Arc<Mutex<Vec<Vec<String>>>>;

    fn broken_store(panics: bool) -> (Arc<UsageStore>, Arc<AppSettings>, String, Seen) {
        let key = AccountKey::primary(Provider::Codex);
        let service: Arc<dyn crate::service::UsageService> = Arc::new(Broken(panics));
        let registry: Registry = [(Provider::Codex, service)].into();
        let mut store = UsageStore::new(registry, Arc::new(crate::secrets::MemorySecrets::default()), None);
        store.fetch_timeout = Duration::from_millis(50);
        let store = Arc::new(store);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (watch, record) = (Arc::downgrade(&store), seen.clone());
        store.set_on_change(move || {
            if let Some(store) = watch.upgrade() {
                record.lock().unwrap().push(store.refreshing());
            }
        });
        let mut settings = AppSettings::default();
        settings.enabled_accounts.insert(key.id());
        (store, Arc::new(settings), key.id(), seen)
    }

    #[tokio::test]
    async fn a_hung_fetch_times_out_and_reports_start_and_end() {
        let (store, settings, id, seen) = broken_store(false);
        store.refresh_all(settings).await;
        assert!(store.refreshing().is_empty());
        assert_eq!(*seen.lock().unwrap(), vec![vec![id.clone()], vec![]], "true, then false");
        let shown = store.snapshot(&[id]);
        assert_eq!(shown[0].state, UsageState::Unavailable(Unavailability::Unreachable));
    }

    #[tokio::test]
    async fn a_panicking_fetch_still_ends_its_refresh() {
        let (store, settings, id, seen) = broken_store(true);
        assert!(!store.refresh_all(settings).await);
        assert!(store.refreshing().is_empty(), "the flag does not outlive the task");
        assert_eq!(seen.lock().unwrap().last(), Some(&Vec::new()));
        assert_eq!(seen.lock().unwrap().first(), Some(&vec![id]));
    }
}
