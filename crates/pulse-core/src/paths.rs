//! Where Pulse keeps its own files: `%APPDATA%\Pulse` (settings, keys, cache,
//! status-line captures). One place, so the app, `--json` and the status-line
//! hook all agree.

use std::path::PathBuf;

fn appdata() -> PathBuf {
    std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

/// Windows difference: a development build keeps its own `%APPDATA%\Pulse Dev`, so it can run beside
/// the installed Pulse without the two writing over each other's settings, placement and caches. It
/// starts as a copy of the installed one's files (`seed_dev_data`), so it has the same accounts.
pub const DEV_DIR: &str = "Pulse Dev";

pub fn data_dir() -> PathBuf {
    appdata().join(if cfg!(debug_assertions) { DEV_DIR } else { "Pulse" })
}

/// The installed Pulse's folder, whichever build is asking.
pub fn release_data_dir() -> PathBuf {
    appdata().join("Pulse")
}

/// The latest blob Claude Code piped to `pulse --statusline`. Read from the installed Pulse's folder in
/// every build: the status line runs the installed `pulse.exe`, which writes it there.
pub fn claude_status_line_capture() -> PathBuf {
    release_data_dir().join("claude-statusline.json")
}

/// A development build's first start: copy the installed Pulse's files (not its folders of caches and
/// browser profiles) into the empty dev folder. Never touches the installed one's files.
pub fn seed_dev_data() {
    if !cfg!(debug_assertions) {
        return;
    }
    seed(&release_data_dir(), &data_dir());
}

fn seed(from: &std::path::Path, to: &std::path::Path) {
    if to.exists() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(from) else { return };
    if std::fs::create_dir_all(to).is_err() {
        return;
    }
    for entry in entries.flatten() {
        if entry.file_type().is_ok_and(|t| t.is_file()) {
            let _ = std::fs::copy(entry.path(), to.join(entry.file_name()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeding_copies_the_files_once_and_leaves_the_source_alone() {
        let root = tempfile::tempdir().unwrap();
        let (from, to) = (root.path().join("Pulse"), root.path().join(DEV_DIR));
        std::fs::create_dir_all(from.join("WebView2-consoles")).unwrap();
        std::fs::write(from.join("settings.json"), "{}").unwrap();
        seed(&from, &to);
        assert_eq!(std::fs::read_to_string(to.join("settings.json")).unwrap(), "{}");
        assert!(!to.join("WebView2-consoles").exists());
        std::fs::write(to.join("settings.json"), "{\"dev\":1}").unwrap();
        std::fs::write(from.join("settings.json"), "{\"new\":1}").unwrap();
        seed(&from, &to);
        assert_eq!(std::fs::read_to_string(to.join("settings.json")).unwrap(), "{\"dev\":1}");
        assert_eq!(std::fs::read_to_string(from.join("settings.json")).unwrap(), "{\"new\":1}");
    }
}
