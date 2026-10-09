// Ported from upstream Usage/WindowPrimer.swift (the timer half).
//! The window starter's clock: wakes just after a usage window's reset and, if it is still due,
//! sends one "hi" through the provider's own command-line tool (`pulse_core::window_starter`).
//!
//! Everything is **off or empty by default**; it acts only for the providers in
//! `primedProviders` (switched on through the pane's risk confirmation), only for their first
//! account, and only while that account is on the panel.
//!
//! Windows: a monotonic timer does not run while the PC sleeps, so the loop never sleeps longer than
//! a minute and re-reads the wall clock each time; a reset that passed during sleep is started when
//! the PC wakes, if still inside the allowed hours.

use std::collections::{BTreeSet, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use chrono::Utc;
use pulse_core::spend::Calendar;
use pulse_core::window_starter::{starter, Outcome, Primer, PROVIDERS};
use pulse_core::{AccountKey, Provider};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Notify;

use crate::state::{self, AppState};

/// The longest the loop sleeps (see the module note), and how long it waits with nothing to do.
const TICK: Duration = Duration::from_secs(60);
const IDLE: Duration = Duration::from_secs(300);

fn primer() -> &'static Mutex<Primer> {
    static PRIMER: OnceLock<Mutex<Primer>> = OnceLock::new();
    PRIMER.get_or_init(Mutex::default)
}

fn in_flight() -> &'static Mutex<HashSet<Provider>> {
    static FLIGHT: OnceLock<Mutex<HashSet<Provider>>> = OnceLock::new();
    FLIGHT.get_or_init(Mutex::default)
}

fn wake() -> &'static Notify {
    static WAKE: OnceLock<Notify> = OnceLock::new();
    WAKE.get_or_init(Notify::new)
}

/// Settings changed: look again now rather than at the next tick.
pub fn poke() {
    wake().notify_one();
}

pub fn start(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            let delay = tick(&app);
            tokio::select! {
                _ = tokio::time::sleep(delay) => {}
                _ = wake().notified() => {}
            }
        }
    });
}

/// One look: note the coming resets, start whatever is due, and say how long until the next look.
fn tick(app: &AppHandle) -> Duration {
    let state = app.state::<AppState>();
    let settings = state.settings();
    let now = Utc::now();
    let calendar = Calendar::local();
    let hours = settings.primer_hours;

    let starting: BTreeSet<Provider> = PROVIDERS
        .iter()
        .copied()
        .filter(|p| settings.primed_providers.contains(p.raw()) && settings.enabled_accounts.contains(&p.account_id()))
        .collect();

    let mut due: Vec<Provider> = Vec::new();
    let next = {
        let mut primer = primer().lock().unwrap();
        primer.forget_unstarted(&starting);
        for provider in &starting {
            let account = AccountKey::primary(*provider);
            let Some(usage) = state.store.snapshot(&[account.id()]).into_iter().next() else { continue };
            primer.record(&account, &usage.windows, now);
            if !in_flight().lock().unwrap().contains(provider) && primer.take_due(&account, &usage.windows, now, &hours, &calendar) {
                due.push(*provider);
            }
        }
        primer.next_wake(now, &hours, &calendar)
    };

    for provider in due {
        in_flight().lock().unwrap().insert(provider);
        let app = app.clone();
        tauri::async_runtime::spawn(async move { send(&app, provider).await });
    }

    match next {
        Some(next) => next.min(TICK),
        None if starting.is_empty() => IDLE,
        None => TICK,
    }
}

async fn send(app: &AppHandle, provider: Provider) {
    let state = app.state::<AppState>();
    let settings = state.settings();
    let outcome = starter::start(provider, &starter::Environment::from_env(), Some(&settings.network_proxy)).await;
    in_flight().lock().unwrap().remove(&provider);
    record_run(app, provider, outcome);
    // The new reset, so the next start is scheduled from it.
    state.store.refresh(state.settings(), &provider.account_id()).await;
    state::emit_usage(app);
    poke();
}

/// `primerRunTimes` / `primerRunOutcomes`: what the pane's "Last started" row shows.
fn record_run(app: &AppHandle, provider: Provider, outcome: Outcome) {
    let state = app.state::<AppState>();
    let mut settings = (*state.settings()).clone();
    settings.primer_run_outcomes.insert(provider.raw().to_string(), outcome.raw().to_string());
    settings.primer_run_times.insert(provider.raw().to_string(), Utc::now().timestamp_millis() as f64 / 1000.0);
    state.save_settings(settings.clone());
    let _ = app.emit("settings-changed", &settings);
}
