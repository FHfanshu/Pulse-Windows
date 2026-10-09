// Ported from upstream Providers/AntigravityUsageService.swift.
//! Antigravity's limits, read from a language server running on this PC.
//!
//! There is no account endpoint and no stored login: Antigravity starts a
//! `language_server` process of its own and serves HTTPS on loopback, and that
//! process alone knows the quota. So these figures exist only while something
//! of Antigravity's is running (`AntigravityNotRunning`).
//!
//! Three things are discovered on every fetch and none is assumed: the process
//! (matched on its install folder, not the Codeium binary name), the port
//! (`--https_server_port 0`, so new each launch) and the per-launch CSRF token
//! from the command line. More than one process can match and most are the wrong
//! one, so every candidate and every port each of them listens on is tried.
//!
//! Windows differences: the process list and command lines come from one
//! `Get-CimInstance Win32_Process` call (hidden window), listening ports from
//! `netstat -ano`, and the install folders are `...\Antigravity\` and
//! `...\Antigravity IDE\` (WINDOWS-PATH: unverified, only those two folder names are
//! matched, never a full path).

use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;

use super::profile;
use crate::model::{AccountKey, ProviderUsage, Unavailability, UsageWindow, WindowKind};
use crate::provider::Provider;
use crate::service::{FetchContext, UsageService};

const QUOTA_METHOD: &str = "exa.language_server_pb.LanguageServerService/RetrieveUserQuotaSummary";
const STATUS_METHOD: &str = "exa.language_server_pb.LanguageServerService/GetUserStatus";
const CSRF_HEADER: &str = "x-codeium-csrf-token";

pub struct Antigravity;

#[async_trait]
impl UsageService for Antigravity {
    fn provider(&self) -> Provider {
        Provider::Antigravity
    }

    async fn fetch(&self, ctx: &FetchContext, account: &AccountKey) -> ProviderUsage {
        let servers = locate_servers().await;
        if servers.is_empty() {
            return ProviderUsage::unavailable(account.clone(), Unavailability::AntigravityNotRunning);
        }

        // An answer with no limits is held, not returned: with two servers up it
        // is what the wrong one says.
        let mut answered_empty = false;
        // Whether anything answered at all. A server that refused the RPC is
        // still Antigravity running.
        let mut something_answered = false;

        for server in &servers {
            // A server listens on several ports and only one speaks this.
            for &port in &server.ports {
                match ask(port, &server.token).await {
                    Ok(windows) if !windows.is_empty() => {
                        // A second call, because the quota reply doesn't name the
                        // plan. Its absence is not worth failing over. No credit
                        // balance: Antigravity reports an allowance, never a balance.
                        let plan = plan(port, &server.token).await;
                        return ProviderUsage::live(account.clone(), windows, ctx.now).with_plan(plan);
                    }
                    Ok(_) => {
                        answered_empty = true;
                        something_answered = true;
                    }
                    Err(Failure::WrongPort) => {}
                    // 401 is the other server saying "not me"; an undecodable 200
                    // is no reason to stop either. Keep looking.
                    Err(Failure::Refused | Failure::Unreadable) => something_answered = true,
                }
            }
        }

        ProviderUsage::unavailable(account.clone(), terminal_reason(answered_empty, something_answered))
    }
}

/// What to say when no candidate produced windows.
fn terminal_reason(answered_empty: bool, something_answered: bool) -> Unavailability {
    if answered_empty {
        Unavailability::NoLimitsReported
    } else if something_answered {
        // Open but unhelpful; a fault the user can only wait out, so not one the
        // alert rules count (unlike `Unreachable` / `UnreadableReply`).
        Unavailability::AntigravityNotAnswering
    } else {
        Unavailability::AntigravityNotRunning
    }
}

// MARK: - Finding it

/// Which Antigravity a language server belongs to, in the order they are asked:
/// the app, then the IDE (both were measured to answer the same payload).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Origin {
    App,
    Ide,
}

impl Origin {
    const ALL: [Origin; 2] = [Origin::App, Origin::Ide];

    /// Which product a (lowercased, forward-slash) process path belongs to, by
    /// its folder names -- never the executable's name: `language_server` is
    /// Codeium's binary and its other editors ship the same one. A path segment
    /// must equal the product's folder exactly, so `antigravity` does not match
    /// `antigravity ide`; the IDE is tested first because both installs carry an
    /// `extensions/antigravity` folder inside.
    fn of(path: &str) -> Option<Origin> {
        let has = |folder: &str| path.split('/').any(|segment| segment == folder);
        if has("antigravity ide") {
            Some(Origin::Ide)
        } else if has("antigravity") {
            Some(Origin::App)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Candidate {
    pid: u32,
    token: String,
}

struct Server {
    ports: Vec<u16>,
    token: String,
}

/// Every language server on this PC that might be able to answer, best first.
/// One process listing for all of them, one port listing; a process listening
/// on nothing cannot be asked anything and is dropped here.
async fn locate_servers() -> Vec<Server> {
    let Some(listing) = process_listing().await else { return Vec::new() };
    let candidates = parse_processes(&listing);
    if candidates.is_empty() {
        return Vec::new();
    }
    let Some(netstat) = run("netstat", &["-ano"]).await else { return Vec::new() };
    candidates
        .into_iter()
        .filter_map(|c| {
            let ports = listening_ports(&netstat, c.pid);
            (!ports.is_empty()).then_some(Server { ports, token: c.token })
        })
        .collect()
}

/// `pid <TAB> executable path <TAB> command line`, one process per line, for
/// every process whose command line mentions `language_server`.
async fn process_listing() -> Option<String> {
    use base64::Engine;

    const SCRIPT: &str = "[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; \
        Get-CimInstance Win32_Process -Filter \"CommandLine LIKE '%language_server%'\" | \
        ForEach-Object { '{0}`t{1}`t{2}' -f $_.ProcessId, $_.ExecutablePath, $_.CommandLine }";
    // Encoded so no quoting layer between here and PowerShell can mangle it.
    let utf16: Vec<u8> = SCRIPT.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let encoded = base64::engine::general_purpose::STANDARD.encode(utf16);
    run(
        "powershell",
        &["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-EncodedCommand", &encoded],
    )
    .await
}

/// Every matching process's pid and CSRF token, in `Origin` order.
fn parse_processes(listing: &str) -> Vec<Candidate> {
    struct Row {
        pid: u32,
        origin: Origin,
        fields: Vec<String>,
    }

    let rows: Vec<Row> = listing
        .lines()
        .filter_map(|line| {
            let mut parts = line.trim_end_matches('\r').splitn(3, '\t');
            let pid = parts.next()?.trim().parse::<u32>().ok()?;
            let exe = parts.next()?.trim();
            let command = parts.next()?.trim();
            let fields = split_command_line(command);
            // The executable tells the origins apart; the command line if it was unreadable.
            let path = if exe.is_empty() { command } else { exe };
            let path = path.replace('\\', "/").to_lowercase();
            // Has to be Codeium's server binary, inside one of Antigravity's folders.
            if !path.contains("/language_server") {
                return None;
            }
            Some(Row { pid, origin: Origin::of(&path)?, fields })
        })
        .collect();

    Origin::ALL
        .iter()
        .flat_map(|origin| {
            let rows = &rows;
            rows.iter()
                .filter(move |r| r.origin == *origin)
                .filter_map(|r| csrf_token(&r.fields).map(|token| Candidate { pid: r.pid, token }))
        })
        .collect()
}

/// The value after `--csrf_token` (or in `--csrf_token=value`).
fn csrf_token(fields: &[String]) -> Option<String> {
    for (i, field) in fields.iter().enumerate() {
        if field == "--csrf_token" {
            return fields.get(i + 1).cloned();
        }
        if let Some(value) = field.strip_prefix("--csrf_token=") {
            return (!value.is_empty()).then(|| value.to_string());
        }
    }
    None
}

/// Windows-style command-line split: whitespace separates, double quotes group.
fn split_command_line(command: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut started = false;
    for ch in command.chars() {
        match ch {
            '"' => {
                in_quotes = !in_quotes;
                started = true;
            }
            c if c.is_whitespace() && !in_quotes => {
                if started {
                    fields.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            c => {
                current.push(c);
                started = true;
            }
        }
    }
    if started {
        fields.push(current);
    }
    fields
}

/// Every port `pid` is listening on, from `netstat -ano` output. A listening
/// TCP socket is the one whose foreign address is the all-zero wildcard, which
/// holds in every display language (the state column is localized).
fn listening_ports(netstat: &str, pid: u32) -> Vec<u16> {
    let mut ports = Vec::new();
    for line in netstat.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 4 || !fields[0].eq_ignore_ascii_case("tcp") {
            continue;
        }
        let (local, foreign) = (fields[1], fields[2]);
        if !matches!(foreign, "0.0.0.0:0" | "[::]:0" | "*:*") {
            continue;
        }
        if fields.last().and_then(|p| p.parse::<u32>().ok()) != Some(pid) {
            continue;
        }
        if let Some(port) = local.rsplit(':').next().and_then(|p| p.parse::<u16>().ok()) {
            if !ports.contains(&port) {
                ports.push(port);
            }
        }
    }
    ports
}

/// Run a console program with no window and a deadline; its stdout, lossily decoded.
#[cfg(windows)]
async fn run(program: &str, args: &[&str]) -> Option<String> {
    use std::process::Stdio;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .creation_flags(CREATE_NO_WINDOW);
    let output = tokio::time::timeout(Duration::from_secs(20), command.output()).await.ok()?.ok()?;
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(not(windows))]
async fn run(_program: &str, _args: &[&str]) -> Option<String> {
    None
}

// MARK: - Asking it

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Failure {
    /// This port answered, but not with this service -- try the next one.
    WrongPort,
    Refused,
    Unreadable,
}

async fn ask(port: u16, token: &str) -> Result<Vec<UsageWindow>, Failure> {
    let data = post(QUOTA_METHOD, port, token).await?;
    let reply: Reply = serde_json::from_slice(&data).map_err(|_| Failure::Unreadable)?;
    Ok(windows(&reply))
}

/// The plan's name ("Pro", ...). `GetUserStatus` also carries the account's name
/// and email; only the plan name is decoded.
async fn plan(port: u16, token: &str) -> Option<String> {
    let data = post(STATUS_METHOD, port, token).await.ok()?;
    plan_name(&data)
}

fn plan_name(data: &[u8]) -> Option<String> {
    let reply: StatusReply = serde_json::from_slice(data).ok()?;
    let name = reply.user_status?.plan_status?.plan_info?.plan_name?;
    (!name.is_empty()).then_some(name)
}

async fn post(method: &str, port: u16, token: &str) -> Result<Vec<u8>, Failure> {
    // The server signs its own certificate, so verification is off -- for this
    // client only, which can reach nothing but the literal loopback address
    // below, and which never uses the app's proxy.
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .no_proxy()
        .timeout(Duration::from_secs(6))
        .build()
        .map_err(|_| Failure::WrongPort)?;

    let reply = client
        .post(format!("https://127.0.0.1:{port}/{method}"))
        .header("Content-Type", "application/json")
        .header(CSRF_HEADER, token)
        .body("{}")
        .send()
        .await
        .map_err(|_| Failure::WrongPort)?;

    match reply.status().as_u16() {
        200 => reply.bytes().await.map(|b| b.to_vec()).map_err(|_| Failure::WrongPort),
        401 | 403 => Err(Failure::Refused),
        _ => Err(Failure::WrongPort),
    }
}

// MARK: - Reading the reply

#[derive(Debug, Deserialize)]
struct Reply {
    response: Option<ReplyBody>,
}

#[derive(Debug, Deserialize)]
struct ReplyBody {
    groups: Option<Vec<Group>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Group {
    display_name: Option<String>,
    buckets: Option<Vec<Bucket>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Bucket {
    bucket_id: Option<String>,
    window: Option<String>,
    remaining_fraction: Option<f64>,
    reset_time: Option<String>,
}

/// Only the sliver of `GetUserStatus` that is any of Pulse's business.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StatusReply {
    user_status: Option<UserStatus>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserStatus {
    plan_status: Option<PlanStatus>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlanStatus {
    plan_info: Option<PlanInfo>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlanInfo {
    plan_name: Option<String>,
}

/// Parse a `RetrieveUserQuotaSummary` body into windows.
#[cfg(test)]
fn parse_quota(body: &[u8]) -> Result<Vec<UsageWindow>, Unavailability> {
    let reply: Reply = profile::decode(body)?;
    Ok(windows(&reply))
}

fn windows(reply: &Reply) -> Vec<UsageWindow> {
    let groups = reply.response.as_ref().and_then(|r| r.groups.as_deref()).unwrap_or_default();
    groups
        .iter()
        .flat_map(|group| {
            let scope = model_group(group.display_name.as_deref());
            let mut windows: Vec<UsageWindow> = group
                .buckets
                .as_deref()
                .unwrap_or_default()
                .iter()
                .filter_map(|b| window(b, scope.as_deref()))
                .collect();
            // Shortest window first within a group: the one about to bite leads.
            windows.sort_by_key(|w| w.window_seconds);
            windows
        })
        .collect()
}

fn window(bucket: &Bucket, scope: Option<&str>) -> Option<UsageWindow> {
    let id = bucket.bucket_id.as_deref()?;
    let remaining = bucket.remaining_fraction?;
    let (kind, seconds) = length_of(bucket.window.as_deref())?;

    // The only provider that reports what is left rather than what is gone.
    let used = (1.0 - remaining).clamp(0.0, 1.0);

    let mut window = UsageWindow::new(id, kind, used, seconds)
        .with_reset(profile::date(bucket.reset_time.as_deref()))
        .exhausted(remaining <= 0.0);
    window.scope = scope.map(str::to_string);
    Some(window)
}

/// A bucket whose window can't be read is left out rather than guessed at.
/// `5h` and `weekly` are what the server sends today; the numbered forms are
/// there so a new window length is understood rather than dropped.
fn length_of(window: Option<&str>) -> Option<(WindowKind, i64)> {
    let window = window?.to_lowercase();

    match window.as_str() {
        "5h" => return Some((WindowKind::FiveHour, 5 * 3_600)),
        "weekly" => return Some((WindowKind::Weekly, 7 * 86_400)),
        "daily" => return Some((WindowKind::Other(86_400), 86_400)),
        "monthly" => return Some((WindowKind::Other(30 * 86_400), 30 * 86_400)),
        _ => {}
    }

    let unit = window.chars().last()?;
    let count: i64 = window[..window.len() - unit.len_utf8()].parse().ok().filter(|c| *c > 0)?;

    match unit {
        'h' => {
            let seconds = UsageWindow::length(count, 3_600)?;
            Some(if count == 5 { (WindowKind::FiveHour, seconds) } else { (WindowKind::Other(seconds), seconds) })
        }
        'd' => {
            let seconds = UsageWindow::length(count, 86_400)?;
            Some(if count == 7 { (WindowKind::Weekly, seconds) } else { (WindowKind::Other(seconds), seconds) })
        }
        _ => None,
    }
}

/// "Gemini Models" -> "Gemini": the group's name is the limit's scope and the
/// trailing "models" is a word the row can't spare.
fn model_group(name: Option<&str>) -> Option<String> {
    let name = name?.trim_matches(|c| c == ' ' || c == '\t');
    if name.is_empty() {
        return None;
    }
    let words: Vec<&str> = name.split(' ').filter(|w| !w.is_empty()).collect();
    match words.last() {
        Some(last) if words.len() > 1 && last.to_lowercase() == "models" => Some(words[..words.len() - 1].join(" ")),
        _ => Some(name.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::profile::test_support::fixture;
    use super::*;

    fn captured() -> Vec<UsageWindow> {
        parse_quota(&fixture("antigravity-quota.json")).unwrap()
    }

    fn from_json(json: &str) -> Vec<UsageWindow> {
        parse_quota(json.as_bytes()).unwrap()
    }

    #[test]
    fn captured_reply_becomes_four_windows_shortest_first_within_each_group() {
        let windows = captured();
        let ids: Vec<&str> = windows.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(ids, ["gemini-5h", "gemini-weekly", "3p-5h", "3p-weekly"]);
        let kinds: Vec<WindowKind> = windows.iter().map(|w| w.kind).collect();
        assert_eq!(kinds, [WindowKind::FiveHour, WindowKind::Weekly, WindowKind::FiveHour, WindowKind::Weekly]);
    }

    #[test]
    fn group_name_becomes_scope_with_trailing_models_trimmed() {
        let scopes: Vec<Option<String>> = captured().into_iter().map(|w| w.scope).collect();
        let expect = ["Gemini", "Gemini", "Claude and GPT", "Claude and GPT"].map(|s| Some(s.to_string()));
        assert_eq!(scopes, expect);
        assert_eq!(model_group(Some("Models")).as_deref(), Some("Models"));
        assert_eq!(model_group(Some("  ")), None);
        assert_eq!(model_group(None), None);
    }

    #[test]
    fn what_is_left_is_turned_into_what_is_gone_exactly_once() {
        let windows = captured();
        let weekly = windows.iter().find(|w| w.id == "gemini-weekly").unwrap();
        assert!((weekly.used_fraction - 0.0050932).abs() < 0.000_001);
        assert_eq!(weekly.percent_value(false), 1);
        assert!(!weekly.is_exhausted);
        let untouched = windows.iter().find(|w| w.id == "3p-weekly").unwrap();
        assert_eq!(untouched.used_fraction, 0.0);
    }

    #[test]
    fn reset_times_are_read_as_real_dates() {
        let windows = captured();
        let weekly = windows.iter().find(|w| w.id == "gemini-weekly").unwrap();
        let expected = chrono::DateTime::parse_from_rfc3339("2026-09-11T06:11:46Z").unwrap();
        assert_eq!(weekly.resets_at, Some(expected.with_timezone(&chrono::Utc)));
    }

    #[test]
    fn nothing_left_is_spent() {
        let windows = from_json(
            r#"{"response":{"groups":[{"displayName":"Gemini Models","buckets":[
              {"bucketId":"gemini-5h","window":"5h","remainingFraction":0}]}]}}"#,
        );
        assert!(windows[0].is_exhausted);
        assert_eq!(windows[0].percent_value(false), 100);
    }

    #[test]
    fn bucket_whose_window_cannot_be_read_is_left_out() {
        let windows = from_json(
            r#"{"response":{"groups":[{"displayName":"Gemini Models","buckets":[
              {"bucketId":"mystery","window":"fortnightly","remainingFraction":0.5},
              {"bucketId":"nameless","remainingFraction":0.5},
              {"bucketId":"gemini-5h","window":"5h","remainingFraction":0.5}]}]}}"#,
        );
        let ids: Vec<&str> = windows.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(ids, ["gemini-5h"]);
    }

    #[test]
    fn window_length_not_seen_before_is_understood() {
        let windows = from_json(
            r#"{"response":{"groups":[{"displayName":"Gemini Models","buckets":[
              {"bucketId":"a","window":"3h","remainingFraction":1},
              {"bucketId":"b","window":"7d","remainingFraction":1},
              {"bucketId":"c","window":"30d","remainingFraction":1}]}]}}"#,
        );
        let ids: Vec<&str> = windows.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "c"]);
        let kinds: Vec<WindowKind> = windows.iter().map(|w| w.kind).collect();
        assert_eq!(kinds, [WindowKind::Other(3 * 3_600), WindowKind::Weekly, WindowKind::Other(30 * 86_400)]);
        assert_eq!(length_of(Some("0h")), None);
        assert_eq!(length_of(Some("-2d")), None);
        assert_eq!(length_of(Some("5H")), Some((WindowKind::FiveHour, 18_000)));
        assert_eq!(length_of(Some("daily")), Some((WindowKind::Other(86_400), 86_400)));
        assert_eq!(length_of(Some("99999999999999999d")), None);
    }

    #[test]
    fn empty_reply_is_no_windows() {
        assert!(from_json("{}").is_empty());
        assert!(from_json(r#"{"response":{"groups":[]}}"#).is_empty());
        assert_eq!(parse_quota(b"nope"), Err(Unavailability::UnreadableReply));
    }

    #[test]
    fn plan_name_is_the_only_thing_read_from_the_status_reply() {
        let body = br#"{"userStatus":{"name":"A","email":"a@b.c","planStatus":{"planInfo":{"planName":"Pro"}}}}"#;
        assert_eq!(plan_name(body).as_deref(), Some("Pro"));
        assert_eq!(plan_name(br#"{"userStatus":{"planStatus":{"planInfo":{"planName":""}}}}"#), None);
        assert_eq!(plan_name(b"{}"), None);
        assert_eq!(plan_name(b"junk"), None);
    }

    #[test]
    fn nothing_answering_is_told_apart_from_something_unhelpful() {
        assert_eq!(terminal_reason(true, true), Unavailability::NoLimitsReported);
        assert_eq!(terminal_reason(false, true), Unavailability::AntigravityNotAnswering);
        assert_eq!(terminal_reason(false, false), Unavailability::AntigravityNotRunning);
    }

    const LISTING: &str = "\
4120\tC:\\Users\\me\\AppData\\Local\\Programs\\Antigravity IDE\\resources\\app\\extensions\\antigravity\\bin\\language_server_windows_x64.exe\t\"C:\\Users\\me\\AppData\\Local\\Programs\\Antigravity IDE\\resources\\app\\extensions\\antigravity\\bin\\language_server_windows_x64.exe\" --https_server_port 0 --csrf_token ide-token-1\r
4121\tC:\\Users\\me\\AppData\\Local\\Programs\\Antigravity IDE\\resources\\app\\extensions\\antigravity\\bin\\language_server_windows_x64.exe\t\"C:\\Users\\me\\AppData\\Local\\Programs\\Antigravity IDE\\resources\\app\\extensions\\antigravity\\bin\\language_server_windows_x64.exe\" --csrf_token=ide-token-2
3300\tC:\\Users\\me\\AppData\\Local\\Programs\\Antigravity\\resources\\app\\extensions\\antigravity\\bin\\language_server_windows_x64.exe\tC:\\Users\\me\\AppData\\Local\\Programs\\Antigravity\\resources\\app\\extensions\\antigravity\\bin\\language_server_windows_x64.exe --https_server_port 0 --csrf_token app-token
5000\tC:\\Tools\\Windsurf\\language_server_windows_x64.exe\tlanguage_server_windows_x64.exe --csrf_token other
6000\tC:\\Users\\me\\AppData\\Local\\Programs\\Antigravity\\Antigravity.exe\t\"Antigravity.exe\" --note language_server
7000\tC:\\Users\\me\\AppData\\Local\\Programs\\Antigravity\\resources\\app\\extensions\\antigravity\\bin\\language_server_windows_x64.exe\tlanguage_server_windows_x64.exe --https_server_port 0
garbage line
";

    #[test]
    fn processes_are_ordered_app_first_and_matched_on_install_folder() {
        let found = parse_processes(LISTING);
        assert_eq!(
            found,
            [
                Candidate { pid: 3300, token: "app-token".into() },
                Candidate { pid: 4120, token: "ide-token-1".into() },
                Candidate { pid: 4121, token: "ide-token-2".into() },
            ]
        );
    }

    #[test]
    fn command_line_is_used_when_the_executable_path_is_unreadable() {
        let line = "42\t\t\"C:\\Program Files\\Antigravity\\bin\\language_server.exe\" --csrf_token tok";
        assert_eq!(parse_processes(line), [Candidate { pid: 42, token: "tok".into() }]);
    }

    #[test]
    fn command_line_splitting_honours_quotes() {
        assert_eq!(
            split_command_line(r#""C:\a b\x.exe"  --csrf_token   t "" end"#),
            ["C:\\a b\\x.exe", "--csrf_token", "t", "", "end"]
        );
        assert!(split_command_line("   ").is_empty());
        assert_eq!(csrf_token(&["--csrf_token".to_string()]), None);
    }

    const NETSTAT: &str = "
Active Connections

  Proto  Local Address          Foreign Address        State           PID
  TCP    0.0.0.0:135            0.0.0.0:0              LISTENING       1008
  TCP    127.0.0.1:50123        0.0.0.0:0              LISTENING       3300
  TCP    127.0.0.1:50124        0.0.0.0:0              LISTENING       3300
  TCP    127.0.0.1:50124        0.0.0.0:0              LISTENING       3300
  TCP    127.0.0.1:50200        127.0.0.1:50123        ESTABLISHED     3300
  TCP    127.0.0.1:50300        0.0.0.0:0              LISTEN-LOCALIZED 4120
  TCP    [::1]:50301            [::]:0                 LISTENING       4120
  TCP    [::]:50302             [::]:0                 LISTENING       33000
  UDP    127.0.0.1:50400        *:*                                    3300
";

    #[test]
    fn listening_ports_come_from_the_wildcard_foreign_address_for_one_pid() {
        assert_eq!(listening_ports(NETSTAT, 3300), [50123, 50124]);
        assert_eq!(listening_ports(NETSTAT, 4120), [50300, 50301]);
        assert_eq!(listening_ports(NETSTAT, 33000), [50302]);
        assert!(listening_ports(NETSTAT, 9).is_empty());
    }
}
