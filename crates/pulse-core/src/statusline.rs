// Ported from upstream App/StatusLineHook.swift.
//! Claude Code's status-line hook.
//!
//! Claude Code pipes a JSON blob (which carries `rate_limits`) to whatever
//! command `statusLine` in `~/.claude/settings.json` names, and shows what that
//! command prints. Registered as `pulse --statusline`, Pulse banks the limits
//! and prints a short line back — or the output of the command that was there
//! before, which it chains to, so an existing status line keeps working.
//!
//! It is a push: figures only arrive while a Claude Code session runs.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{json, Map, Value};

use crate::paths;

pub const MODE_ARGUMENT: &str = "--statusline";

fn settings_file(home: &Path) -> PathBuf {
    home.join(".claude").join("settings.json")
}

fn previous_command_file() -> PathBuf {
    paths::data_dir().join("statusline-previous.txt")
}

/// Entry point for `pulse --statusline`: read stdin, capture, print a line.
pub fn run_as_status_line() {
    let mut input = Vec::new();
    if std::io::stdin().read_to_end(&mut input).is_err() {
        return;
    }
    let Ok(root) = serde_json::from_slice::<Value>(&input) else { return };
    if let Some(limits) = root.get("rate_limits") {
        capture(limits);
    }
    let line = chained_output(&input).unwrap_or_else(|| default_line(&root));
    let mut out = std::io::stdout();
    let _ = out.write_all(line.as_bytes());
    let _ = out.flush();
}

fn capture(limits: &Value) {
    let payload = json!({
        "capturedAt": chrono::Utc::now().timestamp() as f64,
        "rate_limits": limits,
    });
    let target = paths::claude_status_line_capture();
    if let Some(dir) = target.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let temporary = target.with_extension(format!("tmp-{}", std::process::id()));
    if std::fs::write(&temporary, payload.to_string()).is_ok() {
        let _ = std::fs::rename(&temporary, &target);
    }
}

/// The status line that was registered before Pulse, run with the same input.
fn chained_output(input: &[u8]) -> Option<String> {
    let command = std::fs::read_to_string(previous_command_file()).ok()?;
    let command = command.trim();
    if command.is_empty() {
        return None;
    }
    // Claude Code on Windows runs status lines through Git Bash; prefer it so a
    // bash-style command keeps working, and fall back to cmd.
    let mut process = match find_bash() {
        Some(bash) => {
            let mut c = Command::new(bash);
            c.arg("-c").arg(command);
            c
        }
        None => {
            let mut c = Command::new("cmd");
            c.arg("/d").arg("/c").arg(command);
            c
        }
    };
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        process.creation_flags(0x0800_0000);
    }
    let mut child = process.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    child.stdin.take()?.write_all(input).ok()?;
    let output = child.wait_with_output().ok()?;
    String::from_utf8(output.stdout).ok()
}

fn find_bash() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("CLAUDE_CODE_GIT_BASH_PATH").map(PathBuf::from).filter(|p| p.is_file()) {
        return Some(path);
    }
    let program_files = std::env::var_os("ProgramFiles").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Program Files"));
    let candidate = program_files.join("Git").join("bin").join("bash.exe");
    candidate.is_file().then_some(candidate)
}

/// "Sonnet 4.6  ·  5h 12%  ·  7d 40%".
pub fn default_line(root: &Value) -> String {
    let limits = root.get("rate_limits");
    let mut parts: Vec<String> = Vec::new();
    if let Some(model) = root.pointer("/model/display_name").and_then(Value::as_str) {
        parts.push(model.to_string());
    }
    for (key, label) in [("five_hour", "5h"), ("seven_day", "7d")] {
        if let Some(p) = limits.and_then(|l| l.get(key)).and_then(|w| percent(w.get("used_percentage"))) {
            parts.push(format!("{label} {}%", p.round() as i64));
        }
    }
    parts.join("  ·  ")
}

fn percent(value: Option<&Value>) -> Option<f64> {
    value?.as_f64().filter(|p| (0.0..=101.0).contains(p))
}

fn read_settings(home: &Path) -> Option<Map<String, Value>> {
    let bytes = std::fs::read(settings_file(home)).ok()?;
    serde_json::from_slice::<Value>(&bytes).ok()?.as_object().cloned()
}

fn current_command(home: &Path) -> Option<String> {
    read_settings(home)?.get("statusLine")?.get("command")?.as_str().map(str::to_string)
}

pub fn is_installed(home: &Path) -> bool {
    current_command(home).is_some_and(|c| c.contains(MODE_ARGUMENT))
}

/// The command line Claude Code should run. Forward slashes work in Git Bash and cmd alike.
pub fn command_for(executable: &Path) -> String {
    format!("\"{}\" {MODE_ARGUMENT}", executable.to_string_lossy().replace('\\', "/"))
}

#[derive(Debug, PartialEq, Eq)]
pub enum InstallError {
    /// settings.json exists but is not JSON Pulse understands; it is never overwritten.
    Unreadable,
    WriteFailed,
}

pub fn install(home: &Path, executable: &Path) -> Result<(), InstallError> {
    let path = settings_file(home);
    let mut settings = if path.exists() { read_settings(home).ok_or(InstallError::Unreadable)? } else { Map::new() };
    if let Some(previous) = settings.get("statusLine").and_then(|s| s.get("command")).and_then(Value::as_str) {
        if !previous.contains(MODE_ARGUMENT) {
            let _ = std::fs::create_dir_all(paths::data_dir());
            let _ = std::fs::write(previous_command_file(), previous);
        }
    }
    settings.insert("statusLine".into(), json!({"type": "command", "command": command_for(executable)}));
    write_settings(&path, &settings)
}

pub fn uninstall(home: &Path) -> Result<(), InstallError> {
    let path = settings_file(home);
    let mut settings = read_settings(home).ok_or(InstallError::Unreadable)?;
    let previous = std::fs::read_to_string(previous_command_file()).unwrap_or_default();
    if previous.trim().is_empty() {
        settings.remove("statusLine");
    } else {
        settings.insert("statusLine".into(), json!({"type": "command", "command": previous.trim()}));
    }
    let _ = std::fs::remove_file(previous_command_file());
    write_settings(&path, &settings)
}

/// Rebuilding or reinstalling moves the executable; point the hook at the current one.
pub fn repair_path_if_needed(home: &Path, executable: &Path) {
    if let Some(command) = current_command(home) {
        if command.contains(MODE_ARGUMENT) && command != command_for(executable) {
            let _ = install(home, executable);
        }
    }
}

fn write_settings(path: &Path, settings: &Map<String, Value>) -> Result<(), InstallError> {
    let backup = path.with_extension("json.pulse-backup");
    if !backup.exists() {
        if let Ok(original) = std::fs::read(path) {
            let _ = std::fs::write(&backup, original);
        }
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let text = serde_json::to_string_pretty(&Value::Object(settings.clone())).map_err(|_| InstallError::WriteFailed)?;
    std::fs::write(path, text).map_err(|_| InstallError::WriteFailed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_home(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pulse-sl-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".claude")).unwrap();
        dir
    }

    #[test]
    fn default_line_names_model_and_limits() {
        let root = json!({"model": {"display_name": "Opus"}, "rate_limits": {"five_hour": {"used_percentage": 12.4}, "seven_day": {"used_percentage": 300}}});
        assert_eq!(default_line(&root), "Opus  ·  5h 12%");
    }

    #[test]
    fn install_keeps_other_settings_and_refuses_unreadable_files() {
        let home = temp_home("install");
        std::fs::write(home.join(".claude/settings.json"), r#"{"model":"opus"}"#).unwrap();
        install(&home, Path::new(r"C:\Apps\Pulse\pulse.exe")).unwrap();
        let written: Value = serde_json::from_slice(&std::fs::read(home.join(".claude/settings.json")).unwrap()).unwrap();
        assert_eq!(written["model"], "opus");
        assert_eq!(written["statusLine"]["command"], "\"C:/Apps/Pulse/pulse.exe\" --statusline");
        assert!(is_installed(&home));
        assert!(home.join(".claude/settings.json.pulse-backup").exists());

        std::fs::write(home.join(".claude/settings.json"), "not json").unwrap();
        assert_eq!(install(&home, Path::new("x")), Err(InstallError::Unreadable));
        let _ = std::fs::remove_dir_all(home);
    }
}
