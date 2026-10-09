//! Pulse core: providers, usage store, cache and refresh. No UI or Tauri dependency,
//! so `pulse --json` can run headless.

pub mod accounts;
pub mod alerts;
pub mod auth;
pub mod codex_signals;
pub mod discovery;
pub mod i18n;
pub mod history;
pub mod mock;
pub mod model;
pub mod outage;
pub mod paths;
pub mod provider;
pub mod providers;
pub mod recap;
pub mod refresh;
pub mod secrets;
pub mod service;
pub mod settings;
pub mod spend;
pub mod status;
pub mod statusline;
pub mod store;
pub mod tray;
pub mod window_starter;

pub use model::*;
pub use provider::Provider;
