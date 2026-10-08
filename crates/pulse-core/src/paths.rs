//! Where Pulse keeps its own files: `%APPDATA%\Pulse` (settings, keys, cache,
//! status-line captures). One place, so the app, `--json` and the status-line
//! hook all agree.

use std::path::PathBuf;

pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    base.join("Pulse")
}

/// The latest blob Claude Code piped to `pulse --statusline`.
pub fn claude_status_line_capture() -> PathBuf {
    data_dir().join("claude-statusline.json")
}
