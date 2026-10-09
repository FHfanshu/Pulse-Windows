// Ported from upstream Sources/Pulse/Usage/WindowPrimer.swift and Providers/WindowStarter.swift.
//! The window starter: sends "hi" after each usage-window reset so the next window starts then.
//! Off by default and behind the pane's risk confirmation; see `docs`' `window-starter.md` upstream.
//!
//! `primer` decides when (pure, tested), `starter` sends (the provider's own command-line tool).
//! The timer that joins them runs in the Tauri app (`src-tauri/src/window_starter_ipc.rs`).

pub mod primer;
pub mod starter;

pub use primer::{eligible, is_due, kept, Primer, GRACE_SECONDS, PROVIDERS};
pub use starter::Outcome;
