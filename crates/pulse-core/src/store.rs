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

use chrono::{DateTime, Utc};

use crate::model::{AccountKey, ProviderUsage, Unavailability, UsageState};
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
}

pub struct UsageStore {
    registry: Registry,
    secrets: Arc<dyn SecretStore>,
    cache_path: Option<PathBuf>,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    entries: BTreeMap<String, Entry>,
    next_generation: u64,
    signals: Signals,
}

impl UsageStore {
    pub fn new(registry: Registry, secrets: Arc<dyn SecretStore>, cache_path: Option<PathBuf>) -> Self {
        let store = Self { registry, secrets, cache_path, inner: Mutex::new(Inner::default()) };
        store.restore_cache();
        store
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

    pub fn note_looked(&self) {
        self.inner.lock().unwrap().signals.last_looked = Some(Utc::now());
    }

    pub fn set_panel_visible(&self, visible: bool) {
        self.inner.lock().unwrap().signals.is_panel_visible = visible;
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

        let fetched = match self.registry.get(&key.provider) {
            Some(service) => {
                let ctx = FetchContext::from_env(settings.clone(), self.secrets.clone());
                service.fetch(&ctx, &key).await
            }
            None => ProviderUsage::unavailable(key.clone(), Unavailability::NoLimitsReported),
        };

        let changed = {
            let mut inner = self.inner.lock().unwrap();
            let entry = inner.entries.entry(account_id.to_string()).or_default();
            if entry.generation != generation {
                // A newer pass started after this one; its answer wins.
                return false;
            }
            entry.refreshing = false;
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
            any |= t.await.unwrap_or(false);
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
}
