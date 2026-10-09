// Ported from upstream Sources/Pulse/Providers/WindowStarter.swift and CommandLocator.swift.
//! Sends one "hi" through a provider's own command-line tool, so a usage window that only starts
//! counting at the first message starts now.
//!
//! **Why this exists.** Claude Code's five-hour window, and Codex's windows, start at the first
//! message after a reset, not at the reset. Somebody who comes back three hours after a reset
//! starts a fresh five hours then, and waits all of it out if they run dry, where a window started
//! at the reset would already be three hours through. `primer` decides when; this only sends.
//!
//! **The cheapest thing that counts, and nothing kept.** The smallest model, no tools, no MCP
//! servers, none of the user's hooks or settings, run in an empty folder of Pulse's own, and told
//! not to save the session, so no transcript is written, nothing appears in the tool's history, and
//! Pulse's own spend figures never see it. Pulse holds no credential for this: each tool uses the
//! login it already has.
//!
//! Windows: `claude` and `codex` are usually npm shims (`claude.cmd`, `codex.cmd`) or native
//! `.exe`s, found on `PATH` and in the usual install folders; a shim is started directly (Rust
//! quotes its arguments for `cmd.exe`) with no console window, and a run that outlives its deadline
//! is killed with its children (`taskkill /T`), because a shim's `node` is a grandchild.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::provider::Provider;
use crate::settings::NetworkProxy;

pub const PROMPT: &str = "hi";

/// A message takes seconds; a tool stuck on a prompt nobody will answer must not hold anything up
/// for longer than this.
pub const DEADLINE: Duration = Duration::from_secs(120);

/// How long Codex's model list is waited for before the account's default is used instead.
const MODEL_LIST_DEADLINE: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Outcome {
    Sent,
    /// The tool is not installed anywhere Pulse looks.
    ToolMissing,
    /// It ran and failed: signed out, offline, a model gone.
    Failed,
    TimedOut,
}

impl Outcome {
    /// The raw value stored in `primerRunOutcomes` (upstream's `rawValue`).
    pub fn raw(self) -> &'static str {
        match self {
            Outcome::Sent => "sent",
            Outcome::ToolMissing => "toolMissing",
            Outcome::Failed => "failed",
            Outcome::TimedOut => "timedOut",
        }
    }

    pub fn from_raw(raw: &str) -> Option<Self> {
        match raw {
            "sent" => Some(Outcome::Sent),
            "toolMissing" => Some(Outcome::ToolMissing),
            "failed" => Some(Outcome::Failed),
            "timedOut" => Some(Outcome::TimedOut),
            _ => None,
        }
    }
}

/// Where the tools are run from: empty, and Pulse's own, so no project's settings or instructions
/// are picked up.
pub fn folder() -> PathBuf {
    crate::paths::data_dir().join("Window starter")
}

/// `claude -p` with Haiku, and with everything that could do more than answer "hi" turned off.
/// `--setting-sources project` loads only the empty folder's settings, which is to say none: the
/// user's own hooks (sounds, notifications, Pulse's own status line) do not run for this.
pub fn claude_arguments() -> Vec<String> {
    [
        "-p",
        PROMPT,
        "--model",
        "haiku",
        "--no-session-persistence",
        "--tools",
        "",
        "--strict-mcp-config",
        "--setting-sources",
        "project",
    ]
    .map(String::from)
    .to_vec()
}

/// `codex exec`, ephemeral, read-only, in Pulse's folder, at the lowest reasoning effort, and with
/// `model` when one cheaper than the default was found (`cheapest_codex_model`).
pub fn codex_arguments(model: Option<&str>, folder: &Path) -> Vec<String> {
    let mut arguments: Vec<String> = ["exec", "--ephemeral", "--skip-git-repo-check", "-s", "read-only", "-C"]
        .map(String::from)
        .to_vec();
    arguments.push(folder.to_string_lossy().into_owned());
    arguments.push("-c".into());
    // Unquoted: Codex reads a value that is not TOML as a string, and a quote would have to
    // survive `cmd.exe` on its way through an npm shim.
    arguments.push("model_reasoning_effort=low".into());
    if let Some(model) = model {
        arguments.push("-m".into());
        arguments.push(model.into());
    }
    arguments.push(PROMPT.into());
    arguments
}

/// The cheapest model in Codex's own list: one named Luna (the fast, efficient tier at the time of
/// writing), else one described as fast, efficient or mini. None keeps the account's default, which
/// still costs next to nothing for one word. Read from the list each time rather than written down
/// here, because OpenAI renames these.
pub fn cheapest_codex_model(reply: &[u8]) -> Option<String> {
    cheapest_codex_model_in(&serde_json::from_slice::<Value>(reply).ok()?)
}

/// The same, from an already decoded reply (`{"data": [...]}` or `{"result": {"data": [...]}}`).
pub fn cheapest_codex_model_in(root: &Value) -> Option<String> {
    let models = root.get("data").or_else(|| root.get("result").and_then(|r| r.get("data")))?.as_array()?;
    let offered: Vec<&Value> = models.iter().filter(|m| m.get("hidden").and_then(Value::as_bool) != Some(true)).collect();
    let id = |model: &Value| -> Option<String> {
        model.get("id").or_else(|| model.get("model")).and_then(Value::as_str).map(str::to_string)
    };
    if let Some(luna) = offered.iter().find(|m| id(m).is_some_and(|i| i.to_lowercase().contains("luna"))) {
        return id(luna);
    }
    let light = offered.iter().find(|model| {
        let description = model.get("description").and_then(Value::as_str).unwrap_or_default().to_lowercase();
        let words = format!("{description} {}", id(model).unwrap_or_default().to_lowercase());
        ["fast", "efficient", "mini"].iter().any(|w| words.contains(w))
    })?;
    id(light)
}

/// The folder Claude Code keeps for a working directory: the path with everything but letters and
/// digits turned into dashes, under `<claude config>/projects`.
pub fn claude_project_folder(directory: &Path, projects_root: &Path) -> PathBuf {
    let name: String = directory
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    projects_root.join(name)
}

/// Claude Code makes a project folder for the starter's directory even when told not to keep the
/// session: empty, but listed with the user's real ones. Removed when it holds no file at all;
/// anything with a file in it is left exactly as it is.
pub fn remove_empty_project(project: &Path) {
    fn holds_a_file(directory: &Path) -> bool {
        let Ok(entries) = std::fs::read_dir(directory) else { return false };
        entries.flatten().any(|entry| match entry.file_type() {
            Ok(kind) if kind.is_dir() => holds_a_file(&entry.path()),
            Ok(_) => true,
            Err(_) => true,
        })
    }
    if project.is_dir() && !holds_a_file(project) {
        let _ = std::fs::remove_dir_all(project);
    }
}

// MARK: - Finding the tools

/// What the locator needs of the machine, so a test needs no disk.
#[derive(Debug, Clone, Default)]
pub struct Environment {
    pub home: PathBuf,
    pub app_data: PathBuf,
    pub local_app_data: PathBuf,
    pub path: Vec<PathBuf>,
    /// The claude config folder (`CLAUDE_CONFIG_DIR`), when set.
    pub claude_config_dir: Option<PathBuf>,
}

impl Environment {
    pub fn from_env() -> Self {
        let var = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from);
        Self {
            home: var("USERPROFILE").unwrap_or_default(),
            app_data: var("APPDATA").unwrap_or_default(),
            local_app_data: var("LOCALAPPDATA").unwrap_or_default(),
            path: std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default(),
            claude_config_dir: var("CLAUDE_CONFIG_DIR"),
        }
    }

    fn claude_projects(&self) -> PathBuf {
        self.claude_config_dir.clone().unwrap_or_else(|| self.home.join(".claude")).join("projects")
    }
}

/// The names a command may have on Windows, best first: a native build, then an npm shim.
const EXTENSIONS: [&str; 3] = ["exe", "cmd", "bat"];

/// Every place a command may be, in order: `PATH`, then `extra` (folders), then where the usual
/// installers put one: the tools' own installers, npm, pnpm, bun, Volta and Scoop.
pub fn candidates(name: &str, environment: &Environment, extra: &[PathBuf]) -> Vec<PathBuf> {
    let usual = [
        environment.home.join(".local").join("bin"),
        environment.app_data.join("npm"),
        environment.local_app_data.join("pnpm"),
        environment.home.join(".bun").join("bin"),
        environment.home.join(".volta").join("bin"),
        environment.home.join(".npm-global"),
        environment.home.join("scoop").join("shims"),
        environment.local_app_data.join("Programs").join(name),
    ];
    environment
        .path
        .iter()
        .chain(extra)
        .chain(usual.iter())
        .filter(|d| !d.as_os_str().is_empty())
        .flat_map(|dir| EXTENSIONS.iter().map(move |ext| dir.join(format!("{name}.{ext}"))))
        .collect()
}

/// The first candidate that is a file.
pub fn locate(name: &str, environment: &Environment, extra: &[PathBuf]) -> Option<PathBuf> {
    candidates(name, environment, extra).into_iter().find(|p| p.is_file())
}

/// Claude Code, including the folder its own installer used to use (`~/.claude/local`).
pub fn locate_claude(environment: &Environment) -> Option<PathBuf> {
    locate("claude", environment, &[environment.home.join(".claude").join("local")])
}

pub fn locate_codex(environment: &Environment) -> Option<PathBuf> {
    locate("codex", environment, &[])
}

// MARK: - Running

/// A helper's command: the folder the tool was found in leads `PATH` (an npm shim finds its
/// `node` there), the chosen proxy is passed on, and no console window opens.
fn command_for(binary: &Path, arguments: &[String], directory: &Path, proxy: Option<&NetworkProxy>) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(binary);
    command.args(arguments).current_dir(directory);
    if let Some(folder) = binary.parent() {
        let mut paths = vec![folder.to_path_buf()];
        if let Some(existing) = std::env::var_os("PATH") {
            paths.extend(std::env::split_paths(&existing).filter(|p| p.as_path() != folder));
        }
        if let Ok(joined) = std::env::join_paths(paths) {
            command.env("PATH", joined);
        }
    }
    if let Some(proxy) = proxy.filter(|p| p.enabled && !p.host.is_empty()) {
        let scheme = if proxy.scheme.is_empty() { "http" } else { proxy.scheme.as_str() };
        let address = format!("{scheme}://{}:{}", proxy.host, proxy.port);
        for name in ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY"] {
            command.env(name, &address);
        }
    }
    #[cfg(windows)]
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    command
}

/// Stops a run and everything it started: a shim's `node` is a grandchild that
/// `Child::kill` alone would leave behind.
async fn kill_tree(child: &mut tokio::process::Child) {
    #[cfg(windows)]
    if let Some(pid) = child.id() {
        let mut taskkill = tokio::process::Command::new("taskkill");
        taskkill.args(["/PID", &pid.to_string(), "/T", "/F"]).stdout(Stdio::null()).stderr(Stdio::null());
        taskkill.creation_flags(0x0800_0000);
        let _ = taskkill.status().await;
    }
    let _ = child.kill().await;
}

/// Runs the tool to the end or the deadline. Output is not kept: only whether it succeeded matters.
pub async fn run(binary: &Path, arguments: &[String], directory: &Path, proxy: Option<&NetworkProxy>, deadline: Duration) -> Outcome {
    let mut command = command_for(binary, arguments, directory, proxy);
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).kill_on_drop(true);
    let Ok(mut child) = command.spawn() else { return Outcome::ToolMissing };
    match tokio::time::timeout(deadline, child.wait()).await {
        Ok(Ok(status)) if status.success() => Outcome::Sent,
        Ok(_) => Outcome::Failed,
        Err(_) => {
            kill_tree(&mut child).await;
            Outcome::TimedOut
        }
    }
}

/// Codex's own model list over its app server (`model/list`), or None when it cannot be had.
async fn codex_models(codex: &Path, directory: &Path, proxy: Option<&NetworkProxy>) -> Option<Value> {
    let mut command = command_for(codex, &["app-server".to_string()], directory, proxy);
    command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
    let mut child = command.spawn().ok()?;
    let mut stdin = child.stdin.take()?;
    let mut lines = BufReader::new(child.stdout.take()?).lines();
    let messages = [
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
               "params": {"clientInfo": {"name": "Pulse", "title": "Pulse", "version": env!("CARGO_PKG_VERSION")}}}),
        json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "model/list", "params": {}}),
    ];
    for message in &messages {
        let mut line = message.to_string();
        line.push('\n');
        stdin.write_all(line.as_bytes()).await.ok()?;
    }
    stdin.flush().await.ok()?;
    let read = async {
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(message) = serde_json::from_str::<Value>(&line) else { continue };
            if message.get("id").and_then(Value::as_i64) != Some(2) {
                continue;
            }
            return message.get("result").cloned();
        }
        None
    };
    let result = tokio::time::timeout(MODEL_LIST_DEADLINE, read).await.ok().flatten();
    drop(stdin);
    kill_tree(&mut child).await;
    result
}

/// Starts `provider`'s windows with one message. Never runs for any other provider.
pub async fn start(provider: Provider, environment: &Environment, proxy: Option<&NetworkProxy>) -> Outcome {
    let directory = folder();
    let _ = std::fs::create_dir_all(&directory);
    match provider {
        Provider::ClaudeCode => {
            let Some(claude) = locate_claude(environment) else { return Outcome::ToolMissing };
            let outcome = run(&claude, &claude_arguments(), &directory, proxy, DEADLINE).await;
            remove_empty_project(&claude_project_folder(&directory, &environment.claude_projects()));
            outcome
        }
        Provider::Codex => {
            let Some(codex) = locate_codex(environment) else { return Outcome::ToolMissing };
            let model = codex_models(&codex, &directory, proxy).await.as_ref().and_then(cheapest_codex_model_in);
            run(&codex, &codex_arguments(model.as_deref(), &directory), &directory, proxy, DEADLINE).await
        }
        _ => Outcome::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(arguments: &[String], flag: &str, value: &str) -> bool {
        arguments.windows(2).any(|w| w[0] == flag && w[1] == value)
    }

    #[test]
    fn claude_is_asked_with_haiku_no_tools_no_mcp_no_user_settings_nothing_saved() {
        let arguments = claude_arguments();
        assert_eq!(&arguments[..2], ["-p", "hi"]);
        assert!(arguments.contains(&"--no-session-persistence".to_string()));
        assert!(pair(&arguments, "--model", "haiku"));
        assert!(pair(&arguments, "--tools", ""));
        assert!(pair(&arguments, "--setting-sources", "project"));
        assert!(arguments.contains(&"--strict-mcp-config".to_string()));
    }

    #[test]
    fn codex_is_asked_ephemerally_read_only_at_low_effort_with_the_model_when_one_was_found() {
        let folder = Path::new("C:/data/starter");
        let arguments = codex_arguments(Some("gpt-5.6-luna"), folder);
        assert_eq!(arguments.first().map(String::as_str), Some("exec"));
        assert_eq!(arguments.last().map(String::as_str), Some("hi"));
        assert!(arguments.contains(&"--ephemeral".to_string()));
        assert!(pair(&arguments, "-m", "gpt-5.6-luna"));
        assert!(pair(&arguments, "-s", "read-only"));
        assert!(pair(&arguments, "-C", "C:/data/starter"));
        assert!(pair(&arguments, "-c", "model_reasoning_effort=low"));
        assert!(!codex_arguments(None, folder).contains(&"-m".to_string()));
    }

    #[test]
    fn the_cheapest_codex_model_luna_by_name_else_one_described_as_fast_else_none() {
        let reply = |models: Value| serde_json::to_vec(&json!({ "data": models })).unwrap();
        let current = reply(json!([
            {"id": "gpt-5.6-sol", "description": "Older coding model for complex work."},
            {"id": "gpt-5.6-luna", "description": "Older fast and efficient model."},
            {"id": "gpt-5.6-terra", "description": "Older balanced model."},
        ]));
        assert_eq!(cheapest_codex_model(&current).as_deref(), Some("gpt-5.6-luna"));
        let renamed = reply(json!([
            {"id": "gpt-7-big", "description": "Frontier model."},
            {"id": "gpt-7-swift", "description": "Fast and cheap."},
        ]));
        assert_eq!(cheapest_codex_model(&renamed).as_deref(), Some("gpt-7-swift"));
        let hidden = reply(json!([{"id": "gpt-5.6-luna", "hidden": true, "description": "fast"}]));
        assert_eq!(cheapest_codex_model(&hidden), None);
        assert_eq!(cheapest_codex_model(&reply(json!([{"id": "only", "description": "A model."}]))), None);
        assert_eq!(cheapest_codex_model(b"nope"), None);
        // The app server wraps the list in a result, and may name the model "model".
        let wrapped = serde_json::to_vec(&json!({"result": {"data": [{"model": "gpt-9-mini"}]}})).unwrap();
        assert_eq!(cheapest_codex_model(&wrapped).as_deref(), Some("gpt-9-mini"));
    }

    #[test]
    fn a_tool_is_looked_for_on_path_then_its_own_installers_folder_then_the_usual_places() {
        let environment = Environment {
            home: PathBuf::from("C:/Users/me"),
            app_data: PathBuf::from("C:/Users/me/AppData/Roaming"),
            local_app_data: PathBuf::from("C:/Users/me/AppData/Local"),
            path: vec![PathBuf::from("C:/tools")],
            claude_config_dir: None,
        };
        let listed = candidates("claude", &environment, &[environment.home.join(".claude").join("local")]);
        let names: Vec<String> = listed.iter().map(|p| p.to_string_lossy().replace('\\', "/")).collect();
        assert_eq!(names[0], "C:/tools/claude.exe");
        assert_eq!(names[1], "C:/tools/claude.cmd");
        assert_eq!(names[3], "C:/Users/me/.claude/local/claude.exe");
        assert!(names.contains(&"C:/Users/me/.local/bin/claude.exe".to_string()));
        assert!(names.contains(&"C:/Users/me/AppData/Roaming/npm/claude.cmd".to_string()));
        assert!(names.contains(&"C:/Users/me/AppData/Local/Programs/claude/claude.exe".to_string()));
        // PATH leads: nothing from the usual places precedes it.
        let first_usual = names.iter().position(|n| n.contains(".claude/local")).unwrap();
        assert!(names[..first_usual].iter().all(|n| n.starts_with("C:/tools/")));
    }

    #[test]
    fn locating_finds_the_first_file_that_exists() {
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("claude.cmd"), "@echo off").unwrap();
        let environment = Environment { path: vec![bin.clone()], ..Environment::default() };
        assert_eq!(locate_claude(&environment), Some(bin.join("claude.cmd")));
        assert_eq!(locate_codex(&environment), None);
    }

    #[test]
    fn claudes_project_folder_for_the_starters_directory_is_named_the_way_claude_names_it() {
        let directory = Path::new("C:\\Users\\me\\AppData\\Roaming\\Pulse\\Window starter");
        let root = Path::new("C:/Users/me/.claude/projects");
        assert_eq!(
            claude_project_folder(directory, root),
            root.join("C--Users-me-AppData-Roaming-Pulse-Window-starter")
        );
    }

    #[test]
    fn an_empty_project_folder_is_removed_and_one_with_a_file_in_it_is_left_alone() {
        let root = tempfile::tempdir().unwrap();
        let empty = root.path().join("empty");
        std::fs::create_dir_all(empty.join("nested")).unwrap();
        remove_empty_project(&empty);
        assert!(!empty.exists());

        let kept = root.path().join("kept");
        std::fs::create_dir_all(kept.join("nested")).unwrap();
        std::fs::write(kept.join("nested").join("session.jsonl"), "{}").unwrap();
        remove_empty_project(&kept);
        assert!(kept.join("nested").join("session.jsonl").exists());

        // Not there at all is fine.
        remove_empty_project(&root.path().join("missing"));
    }

    #[test]
    fn outcomes_round_trip_through_their_stored_names() {
        for outcome in [Outcome::Sent, Outcome::ToolMissing, Outcome::Failed, Outcome::TimedOut] {
            assert_eq!(Outcome::from_raw(outcome.raw()), Some(outcome));
        }
        assert_eq!(Outcome::from_raw("nope"), None);
    }

    #[cfg(windows)]
    mod running {
        use super::*;

        fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
            let path = dir.join(name);
            std::fs::write(&path, format!("@echo off\r\n{body}\r\n")).unwrap();
            path
        }

        #[tokio::test]
        async fn a_tool_that_exits_cleanly_has_sent_one_that_fails_has_not_and_a_missing_one_is_missing() {
            let root = tempfile::tempdir().unwrap();
            let ok = script(root.path(), "ok.cmd", "exit /b 0");
            let bad = script(root.path(), "bad.cmd", "exit /b 3");
            let arguments = ["-p".to_string(), "hi".to_string()];
            assert_eq!(run(&ok, &arguments, root.path(), None, Duration::from_secs(20)).await, Outcome::Sent);
            assert_eq!(run(&bad, &arguments, root.path(), None, Duration::from_secs(20)).await, Outcome::Failed);
            let missing = root.path().join("missing.exe");
            assert_eq!(run(&missing, &arguments, root.path(), None, Duration::from_secs(20)).await, Outcome::ToolMissing);
        }

        #[tokio::test]
        async fn a_tool_that_does_not_answer_in_time_is_stopped_and_timed_out() {
            let root = tempfile::tempdir().unwrap();
            let slow = script(root.path(), "slow.cmd", "ping -n 30 127.0.0.1 >nul");
            let started = std::time::Instant::now();
            assert_eq!(run(&slow, &[], root.path(), None, Duration::from_millis(500)).await, Outcome::TimedOut);
            assert!(started.elapsed() < Duration::from_secs(15));
        }
    }
}
