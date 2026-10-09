//! The Notifications pane's backend: which accounts can be warned about a low balance, and the
//! "your September recap is ready" notification (upstream `RecapNotice`, `RecapNoticeRule`).
//!
//! The recap rules are `pulse_core::recap::periods::notice` (pure, tested). This is the clock, the
//! read and the toast:
//!
//! - first check 90 seconds after launch, then every half hour; a check outside the first three
//!   days of a month returns before it reads anything;
//! - it works only while Token spend reading is on, and only when the switch is on;
//! - the month announced is remembered (`recap_announced_month`) **before** the toast is handed to
//!   the system, so a relaunch, a second check or switching the setting off and on says nothing
//!   more;
//! - only when the month had tokens. Windows deviation: upstream reads only the scan its
//!   `SpendWarmer` already keeps and never starts one of its own. This port has no warmer, so the
//!   check reads the ledgers itself, but only inside those three days and with the per-file
//!   transcript cache, so the second read costs a directory listing.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use chrono::Utc;
use pulse_core::recap::periods::notice;
use pulse_core::recap::Period;
use pulse_core::spend::{self, summary::SpendSummary, Calendar, Ledger, SpendAgent};
use pulse_core::Provider;
use tauri::{AppHandle, Emitter, Manager};

use crate::state::AppState;

/// Out of the way of launch.
const LAUNCH_DELAY: Duration = Duration::from_secs(90);
/// Half an hour is a fine grain for "the first three days of a month".
const CHECK_INTERVAL: Duration = Duration::from_secs(30 * 60);

/// Whether the account's provider reports a prepaid balance a "Warn below" line can be set on.
/// The pane offers the row only when this is true.
#[tauri::command]
pub fn reports_spendable_balance(provider: String) -> bool {
    Provider::from_raw(&provider).is_some_and(pulse_core::alerts::reports_spendable_balance)
}

/// Start the recap clock. Call once from `setup`.
pub fn start(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(LAUNCH_DELAY).await;
        loop {
            check_recap(&app).await;
            tokio::time::sleep(CHECK_INTERVAL).await;
        }
    });
}

/// Switching it on in the first days of a month says so at once.
pub fn check_recap_now(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move { check_recap(&app).await });
}

fn home() -> PathBuf {
    std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_default()
}

/// Decides now, and posts at most one notification.
async fn check_recap(app: &AppHandle) {
    let state = app.state::<AppState>();
    let settings = state.settings();
    let calendar = Calendar::local();
    let today = calendar.date(Utc::now());
    let announced = settings.recap_announced_month.as_deref().and_then(Period::from_key);
    if !settings.alerts_on_recap
        || !settings.reads_token_spend
        || notice::candidate(today).is_none_or(|month| announced == Some(month))
    {
        return;
    }

    // The month's summary is arithmetic over every day of every ledger: off the async threads.
    let due = tauri::async_runtime::spawn_blocking(move || {
        let now = Utc::now();
        let ledgers: HashMap<SpendAgent, Ledger> = spend::read_present_ledgers(&home(), now);
        notice::due(today, announced, |period| {
            let (first, after) = period.bounds()?;
            let summary = SpendSummary::of_range(&ledgers, calendar.midnight(first), calendar.midnight(after), now, &calendar);
            Some(summary.tokens)
        })
    })
    .await
    .ok()
    .flatten();
    let Some(month) = due else { return };

    // Read again after the await: a second check (the switch going on while the timer fires) may
    // have announced this month meanwhile.
    let current = state.settings();
    if !current.alerts_on_recap || current.recap_announced_month.as_deref() == Some(month.key().as_str()) {
        return;
    }
    // Remembered before it is posted, and whether or not the system will show it: a month is
    // announced once.
    let mut updated = (*current).clone();
    updated.recap_announced_month = Some(month.key());
    state.save_settings(updated.clone());
    let _ = app.emit("settings-changed", &updated);

    let Period::Month { month: number, .. } = month else { return };
    let name = crate::notify::month_name(number, crate::notify::language(&updated));
    if let Some(note) = pulse_core::outage::recap_notification(month, &name) {
        crate::notify::post(app, &updated, &note);
    }
}
