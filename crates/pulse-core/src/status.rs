// Ported from upstream Providers/ServiceStatus.swift.
//! What a provider's own status page says about its service: each component's state now, the
//! page's own uptime figure, and its last ninety or so days, one bar a day as the page draws them.
//!
//! **Three hosts, three feeds.** status.openai.com is incident.io's, status.claude.com Atlassian
//! Statuspage's, status.deepseek.com Flashcat's. Each is read from what the page itself is drawn
//! from, because no public summary carries the history: OpenAI's Statuspage-compatible summary
//! leaves the Codex group out entirely, and Flashcat offers none without an account key, so
//! DeepSeek's is read out of the page.
//!
//! **Nothing here is Pulse's figure.** The uptime percentage is the page's, passed through and
//! never computed; a missing one is not shown. Claude's bars are the colours the page drew.
//! OpenAI's are the worst impact the feed lists on each local day. A component not listed as
//! affected is operational, because that is how both feeds say it; a page that can't be read is no
//! reading, never "all clear".

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use chrono::{DateTime, Days, NaiveDate, SecondsFormat, TimeZone, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::provider::Provider;
use crate::spend::Calendar;

/// How often a status page is asked: by the outage check while its switch is on, and by a pane
/// while it is open. Status pages are written by people, minutes into an incident; asking more
/// often learns nothing sooner and only loads someone else's server.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(300);

/// Whose status page, and what on it belongs to the provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StatusPage {
    /// status.openai.com, its Codex group.
    OpenAi,
    /// status.claude.com, every component it shows.
    Claude,
    /// status.deepseek.com, every component it shows.
    DeepSeek,
}

impl StatusPage {
    pub const ALL: [StatusPage; 3] = [StatusPage::OpenAi, StatusPage::Claude, StatusPage::DeepSeek];

    pub fn address(self) -> &'static str {
        match self {
            StatusPage::OpenAi => "https://status.openai.com",
            StatusPage::Claude => "https://status.claude.com",
            StatusPage::DeepSeek => "https://status.deepseek.com",
        }
    }

    pub fn host(self) -> &'static str {
        self.address().trim_start_matches("https://")
    }

    /// Who is speaking on the page, for the footnote.
    pub fn company(self) -> &'static str {
        match self {
            StatusPage::OpenAi => "OpenAI",
            StatusPage::Claude => "Anthropic",
            StatusPage::DeepSeek => "DeepSeek",
        }
    }

    /// The provider whose pane shows it, and whose name a notification bears.
    pub fn provider(self) -> Provider {
        match self {
            StatusPage::OpenAi => Provider::Codex,
            StatusPage::Claude => Provider::ClaudeCode,
            StatusPage::DeepSeek => Provider::DeepSeek,
        }
    }

    /// The provider's own status page, where Pulse shows and watches it.
    pub fn for_provider(provider: Provider) -> Option<StatusPage> {
        match provider {
            Provider::Codex => Some(StatusPage::OpenAi),
            Provider::ClaudeCode => Some(StatusPage::Claude),
            Provider::DeepSeek => Some(StatusPage::DeepSeek),
            _ => None,
        }
    }

    /// Whether an outage of this component is worth a notification to someone using the provider.
    /// The pane shows every component on the page; a notification is only for what the provider's
    /// account runs on. Claude's by Statuspage id (Claude Code, and the API it runs on) because
    /// names change and ids don't. DeepSeek's by name: Pulse reads its API account, its API rows
    /// are named after the model, and a new model is a new row; the chat site is not the API.
    /// Codex's page is read as its group already.
    pub fn notifies_about(self, component: &Component) -> bool {
        match self {
            StatusPage::OpenAi => true,
            StatusPage::Claude => ["yyzkbfz2thpt", "k8w3r06qmzrp"].contains(&component.id.as_str()),
            StatusPage::DeepSeek => component.name.contains("API"),
        }
    }

    /// How many days the page draws, ending today.
    pub fn day_count(self) -> usize {
        match self {
            StatusPage::OpenAi => 91,
            StatusPage::Claude | StatusPage::DeepSeek => 90,
        }
    }
}

/// Raw values are what the outage memory writes to disk; don't rename them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum State {
    Operational,
    Degraded,
    PartialOutage,
    FullOutage,
    Maintenance,
    /// A value this build doesn't know. Shown as unrecognised, never as operational.
    Unrecognised,
}

impl State {
    /// What a notification is about: the service is failing. Maintenance is planned, and a value
    /// Pulse can't read is not a witnessed outage.
    pub fn is_outage(self) -> bool {
        matches!(self, State::Degraded | State::PartialOutage | State::FullOutage)
    }

    pub fn from_feed(value: &str) -> State {
        match value {
            "operational" => State::Operational,
            // Flashcat (DeepSeek) says `degraded` and `maintenance`.
            "degraded_performance" | "degraded" => State::Degraded,
            "partial_outage" => State::PartialOutage,
            // incident.io says `full_outage`, Statuspage `major_outage`.
            "full_outage" | "major_outage" => State::FullOutage,
            "under_maintenance" | "maintenance" => State::Maintenance,
            _ => State::Unrecognised,
        }
    }

    /// What the pane and a notification call it: the localization key (upstream's English).
    pub fn title(self) -> &'static str {
        match self {
            State::Operational => "Operational",
            State::Degraded => "Degraded performance",
            State::PartialOutage => "Partial outage",
            State::FullOutage => "Full outage",
            State::Maintenance => "Under maintenance",
            State::Unrecognised => "Unrecognised status",
        }
    }

    /// For picking a day's worst, and telling worse from better.
    pub fn severity(self) -> u8 {
        match self {
            State::Operational => 0,
            State::Maintenance => 1,
            State::Unrecognised => 2,
            State::Degraded => 3,
            State::PartialOutage => 4,
            State::FullOutage => 5,
        }
    }
}

/// One day of a component's history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Day {
    /// The calendar day as the page counts it: this PC's for OpenAI and DeepSeek, the page's own
    /// for Claude.
    pub date: NaiveDate,
    /// The worst the page recorded that day; `None` before it kept any record of the component.
    pub state: Option<State>,
    /// The colour the page drew, where it says: status.claude.com grades a day by how long it was
    /// out, which a state alone can't. 0xRRGGBB.
    pub rgb: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Component {
    pub id: String,
    /// The page's own name ("CLI", "Claude API (api.anthropic.com)"), shown as written, like a
    /// model name.
    pub name: String,
    pub state: State,
    /// Oldest first. Empty when the history couldn't be read; the current state still stands.
    pub days: Vec<Day>,
    /// The page's figure for the period, in percent.
    pub uptime: Option<f64>,
}

impl Component {
    pub fn new(id: impl Into<String>, name: impl Into<String>, state: State) -> Self {
        Self { id: id.into(), name: name.into(), state, days: Vec::new(), uptime: None }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceStatus {
    pub page: StatusPage,
    pub components: Vec<Component>,
}

// ---------------------------------------------------------------------------
// Days
// ---------------------------------------------------------------------------

/// One local calendar day: its date and the instants it starts and ends at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DayRange {
    pub date: NaiveDate,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

/// The last `count` calendar days, oldest first, today last.
pub fn calendar_days(now: DateTime<Utc>, count: usize, calendar: &Calendar) -> Vec<DayRange> {
    let today = calendar.date(now);
    (0..count)
        .rev()
        .filter_map(|back| {
            let date = today.checked_sub_days(Days::new(back as u64))?;
            let next = date.checked_add_days(Days::new(1))?;
            Some(DayRange { date, start: calendar.midnight(date), end: calendar.midnight(next) })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Reading the feeds
// ---------------------------------------------------------------------------

const OPENAI_FEED: &str = "https://status.openai.com/proxy/status.openai.com";
const OPENAI_GROUP: &str = "Codex";
const CLAUDE_SUMMARY: &str = "https://status.claude.com/api/v2/summary.json";
const CLAUDE_UPTIME: &str = "https://status.claude.com/uptime_showcase";
/// The page itself: Flashcat publishes no feed without an account key, and renders its data into
/// the page.
const DEEPSEEK_PAGE: &str = "https://status.deepseek.com/";

async fn fetch(http: &reqwest::Client, url: &str, accept: &str) -> Option<Vec<u8>> {
    let response = http
        .get(url)
        .header(reqwest::header::ACCEPT, accept)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .ok()?;
    if response.status().as_u16() != 200 {
        return None;
    }
    response.bytes().await.ok().map(|b| b.to_vec())
}

async fn fetch_json(http: &reqwest::Client, url: &str) -> Option<Vec<u8>> {
    fetch(http, url, "application/json").await
}

/// The page's components with their history, or `None` when its current state couldn't be read.
pub async fn read(page: StatusPage, http: &reqwest::Client, now: DateTime<Utc>, calendar: &Calendar) -> Option<ServiceStatus> {
    let components = match page {
        StatusPage::OpenAi => read_openai(http, now, calendar).await?,
        StatusPage::Claude => read_claude(http).await?,
        StatusPage::DeepSeek => {
            let html = fetch(http, DEEPSEEK_PAGE, "text/html").await?;
            let days = calendar_days(now, page.day_count(), calendar);
            flashcat(&html, now, Some(&days))?
        }
    };
    Some(ServiceStatus { page, components })
}

/// The state now and nothing else: one request, for the outage check. `None` when it couldn't be
/// read.
pub async fn current(page: StatusPage, http: &reqwest::Client, now: DateTime<Utc>) -> Option<Vec<Component>> {
    match page {
        StatusPage::OpenAi => incident_io_components(&fetch_json(http, OPENAI_FEED).await?, OPENAI_GROUP),
        StatusPage::Claude => statuspage_components(&fetch_json(http, CLAUDE_SUMMARY).await?),
        StatusPage::DeepSeek => flashcat(&fetch(http, DEEPSEEK_PAGE, "text/html").await?, now, None),
    }
}

async fn read_openai(http: &reqwest::Client, now: DateTime<Utc>, calendar: &Calendar) -> Option<Vec<Component>> {
    let days = calendar_days(now, StatusPage::OpenAi.day_count(), calendar);
    let (first, last) = (days.first()?, days.last()?);
    let stamp = |at: DateTime<Utc>| at.to_rfc3339_opts(SecondsFormat::Millis, true);
    let history = format!("{OPENAI_FEED}/component_impacts?start_at={}&end_at={}", stamp(first.start), stamp(last.end));

    let (summary, impacts) = tokio::join!(fetch_json(http, OPENAI_FEED), fetch_json(http, &history));
    let mut components = incident_io_components(&summary?, OPENAI_GROUP)?;
    if let Some(impacts) = impacts {
        components = incident_io_history(&impacts, &days, now, components);
    }
    Some(components)
}

/// The summary first, because it says which components there are; then their history in one
/// request (the page asks up to 60 at a time).
async fn read_claude(http: &reqwest::Client) -> Option<Vec<Component>> {
    let mut components = statuspage_components(&fetch_json(http, CLAUDE_SUMMARY).await?)?;
    let ids = components.iter().map(|c| c.id.as_str()).collect::<Vec<_>>().join(",");
    if let Some(showcase) = fetch_json(http, &format!("{CLAUDE_UPTIME}?components={ids}")).await {
        components = statuspage_history(&showcase, components);
    }
    Some(components)
}

// MARK: - JSON helpers

/// A list read entry by entry: one the page shapes differently, a null name on some unrelated
/// component, is skipped rather than taking every other component, and so the whole pane and every
/// alert, with it. A missing list is empty.
fn lenient<T>(list: Option<&Value>, read: impl Fn(&Value) -> Option<T>) -> Vec<T> {
    list.and_then(Value::as_array).map(|items| items.iter().filter_map(read).collect()).unwrap_or_default()
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key)?.as_str().map(str::to_string)
}

fn timestamp(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text).ok().map(|d| d.with_timezone(&Utc))
}

fn seconds(value: f64) -> Option<DateTime<Utc>> {
    Utc.timestamp_millis_opt((value * 1000.0) as i64).single()
}

// MARK: - Shared history model (incident.io and Flashcat)

/// One stretch of a component not being operational; no end while it lasts.
#[derive(Debug, Clone)]
pub struct Impact {
    pub component_id: String,
    pub start: DateTime<Utc>,
    pub end: Option<DateTime<Utc>>,
    pub state: State,
}

/// Each component's days and uptime from its impacts: a day takes the worst impact that overlaps
/// it, and a day that ended before the page had data for the component has none.
pub fn applying_history(
    impacts: &[Impact],
    since: &HashMap<String, DateTime<Utc>>,
    uptimes: &HashMap<String, f64>,
    days: &[DayRange],
    now: DateTime<Utc>,
    components: Vec<Component>,
) -> Vec<Component> {
    components
        .into_iter()
        .map(|mut component| {
            let spans: Vec<(DateTime<Utc>, DateTime<Utc>, State)> = impacts
                .iter()
                .filter(|i| i.component_id == component.id)
                .filter_map(|i| {
                    let end = i.end.unwrap_or(now);
                    (end > i.start).then_some((i.start, end, i.state))
                })
                .collect();
            let started = since.get(&component.id).copied();

            component.days = days
                .iter()
                .map(|day| {
                    if started.is_some_and(|s| day.end <= s) {
                        return Day { date: day.date, state: None, rgb: None };
                    }
                    let worst = spans
                        .iter()
                        .filter(|(start, end, _)| *start < day.end && *end > day.start)
                        .map(|(_, _, state)| *state)
                        .max_by_key(|s| s.severity());
                    Day { date: day.date, state: Some(worst.unwrap_or(State::Operational)), rgb: None }
                })
                .collect();
            component.uptime = uptimes.get(&component.id).copied();
            component
        })
        .collect()
}

// MARK: - incident.io (status.openai.com)

/// The named group's visible components, each with its state now. `None` when the feed doesn't
/// parse or has no such group, or the group shows nothing.
pub fn incident_io_components(data: &[u8], group_name: &str) -> Option<Vec<Component>> {
    let feed: Value = serde_json::from_slice(data).ok()?;
    let summary = feed.get("summary")?;
    let structure = summary.get("structure")?;
    let group = lenient(structure.get("items"), |item| {
        let group = item.get("group")?;
        let name = text(group, "name")?;
        let components = lenient(group.get("components"), |c| {
            Some((text(c, "component_id")?, text(c, "name")?, c.get("hidden").and_then(Value::as_bool)))
        });
        Some((name, components))
    })
    .into_iter()
    .find(|(name, _)| name == group_name)?
    .1;

    let mut affected: HashMap<String, String> = HashMap::new();
    for entry in lenient(summary.get("affected_components"), |a| Some((text(a, "component_id")?, text(a, "status")?))) {
        affected.entry(entry.0).or_insert(entry.1);
    }
    let components: Vec<Component> = group
        .into_iter()
        .filter(|(_, _, hidden)| *hidden != Some(true))
        .map(|(id, name, _)| {
            let state = affected.get(&id).map_or(State::Operational, |s| State::from_feed(s));
            Component::new(id, name, state)
        })
        .collect();
    (!components.is_empty()).then_some(components)
}

/// Each component's days and uptime from the impacts feed: a day takes the worst impact that
/// overlaps it, and a day that ended before the page had data for the component has none.
pub fn incident_io_history(data: &[u8], days: &[DayRange], now: DateTime<Utc>, components: Vec<Component>) -> Vec<Component> {
    let Ok(feed) = serde_json::from_slice::<Value>(data) else { return components };
    if !feed.is_object() {
        return components;
    }
    let impacts = lenient(feed.get("component_impacts"), |i| {
        Some(Impact {
            component_id: text(i, "component_id")?,
            state: State::from_feed(&text(i, "status")?),
            start: timestamp(&text(i, "start_at")?)?,
            end: text(i, "end_at").as_deref().and_then(timestamp),
        })
    });
    let mut since = HashMap::new();
    let mut uptimes = HashMap::new();
    // A group's own figure comes in this list too, with a `status_page_component_group_id` and no
    // `component_id`: requiring one on every entry once lost the whole history.
    for record in lenient(feed.get("component_uptimes"), |u| {
        let uptime = match u.get("uptime")? {
            Value::String(s) => s.trim().parse::<f64>().ok(),
            Value::Number(n) => n.as_f64(),
            _ => None,
        };
        Some((text(u, "component_id"), uptime, text(u, "data_available_since").as_deref().and_then(timestamp)))
    }) {
        let (Some(id), uptime, available) = record else { continue };
        match available {
            Some(at) => since.insert(id.clone(), at),
            None => since.remove(&id),
        };
        match uptime {
            Some(value) => uptimes.insert(id, value),
            None => uptimes.remove(&id),
        };
    }
    applying_history(&impacts, &since, &uptimes, days, now, components)
}

// MARK: - Flashcat (status.deepseek.com)

/// Every component the page shows, in its order, with its state now and, given `days`, its days
/// and uptime. `None` when the page's data can't be found, or it shows nothing.
///
/// **Read out of the page**, because Flashcat's API needs an account key. The page is a Next.js
/// app: its data travels in `self.__next_f.push([1,"..."])` chunks, which joined are lines of
/// `id:JSON`. Two carry an `initialData` object, the layout (`page`), and the impacts and
/// uptimes, and nothing else is read. A redesign that moves them is no reading, never "all clear".
pub fn flashcat(html: &[u8], now: DateTime<Utc>, days: Option<&[DayRange]>) -> Option<Vec<Component>> {
    let feed = flashcat_initial_data(&String::from_utf8_lossy(html))?;
    let page = feed.get("page").filter(|p| p.is_object())?;

    struct Placed {
        id: String,
        name: String,
        section: Option<String>,
        status: Option<String>,
        order: Option<i64>,
        hidden: bool,
    }
    let placed = lenient(page.get("components"), |c| {
        Some(Placed {
            id: text(c, "component_id")?,
            name: text(c, "name")?,
            section: text(c, "section_id"),
            status: text(c, "status"),
            order: c.get("order_id").and_then(Value::as_i64),
            hidden: c.get("hide_all").and_then(Value::as_bool) == Some(true),
        })
    });
    let sections = lenient(page.get("sections"), |s| {
        Some((text(s, "section_id")?, s.get("order_id").and_then(Value::as_i64), s.get("hide_all").and_then(Value::as_bool) == Some(true)))
    });

    let impacts = lenient(feed.get("component_impacts"), |i| {
        Some(Impact {
            component_id: text(i, "component_id")?,
            state: State::from_feed(&text(i, "status")?),
            start: seconds(i.get("start_at_seconds")?.as_f64()?)?,
            end: i.get("end_at_seconds").and_then(Value::as_f64).and_then(seconds),
        })
    });

    // Top level: components outside any section, and sections, by order; a section opens into its
    // own components, by order. The section's own row is an aggregate and isn't drawn, as with
    // OpenAI's groups.
    let visible: Vec<&Placed> = placed.iter().filter(|c| !c.hidden).collect();
    let order = |value: Option<i64>| value.unwrap_or(i64::MAX);
    let mut groups: Vec<(i64, Vec<&Placed>)> = visible
        .iter()
        .filter(|c| c.section.as_deref().unwrap_or("").is_empty())
        .map(|c| (order(c.order), vec![*c]))
        .collect();
    for (section_id, section_order, hidden) in &sections {
        if *hidden {
            continue;
        }
        let mut members: Vec<&Placed> = visible.iter().filter(|c| c.section.as_deref() == Some(section_id)).copied().collect();
        members.sort_by_key(|c| order(c.order));
        groups.push((order(*section_order), members));
    }
    groups.sort_by_key(|(o, _)| *o);

    let components: Vec<Component> = groups
        .into_iter()
        .flat_map(|(_, members)| members)
        .map(|c| {
            // The page's own word when it gives one; otherwise whatever impact is still open;
            // otherwise operational.
            let open = impacts
                .iter()
                .filter(|i| i.component_id == c.id && i.start <= now && i.end.is_none_or(|e| e > now))
                .map(|i| i.state)
                .max_by_key(|s| s.severity());
            let stated = c.status.as_deref().filter(|s| !s.is_empty()).map(State::from_feed);
            Component::new(c.id.clone(), c.name.clone(), stated.or(open).unwrap_or(State::Operational))
        })
        .collect();
    if components.is_empty() {
        return None;
    }
    let Some(days) = days else { return Some(components) };

    let mut since = HashMap::new();
    let mut uptimes = HashMap::new();
    for record in lenient(feed.get("component_uptimes"), |u| {
        Some((text(u, "component_id"), u.get("uptime").and_then(Value::as_f64), u.get("available_since_seconds").and_then(Value::as_f64)))
    }) {
        let (Some(id), uptime, available) = record else { continue };
        match available.and_then(seconds) {
            Some(at) => since.insert(id.clone(), at),
            None => since.remove(&id),
        };
        match uptime {
            Some(value) => uptimes.insert(id, value),
            None => uptimes.remove(&id),
        };
    }
    Some(applying_history(&impacts, &since, &uptimes, days, now, components))
}

/// The page's `initialData` objects, merged into one JSON object; `None` without the layout.
pub fn flashcat_initial_data(html: &str) -> Option<Value> {
    let push = Regex::new(r#"self\.__next_f\.push\(\[1,("(?:[^"\\]|\\.)*")\]\)"#).ok()?;
    let flight: String = push
        .captures_iter(html)
        .filter_map(|c| serde_json::from_str::<String>(c.get(1)?.as_str()).ok())
        .collect();

    let mut merged = serde_json::Map::new();
    for line in flight.split('\n').filter(|l| l.contains("\"initialData\"")) {
        let Some((_, rest)) = line.split_once(':') else { continue };
        let Ok(record) = serde_json::from_str::<Value>(rest) else { continue };
        collect_initial_data(&record, &mut merged);
    }
    merged.contains_key("page").then_some(Value::Object(merged))
}

fn collect_initial_data(node: &Value, merged: &mut serde_json::Map<String, Value>) {
    match node {
        Value::Object(object) => {
            if let Some(Value::Object(data)) = object.get("initialData") {
                for (key, value) in data {
                    merged.entry(key.clone()).or_insert_with(|| value.clone());
                }
            }
            object.values().for_each(|v| collect_initial_data(v, merged));
        }
        Value::Array(items) => items.iter().for_each(|v| collect_initial_data(v, merged)),
        _ => {}
    }
}

// MARK: - Statuspage (status.claude.com)

/// Every component the page shows, in its order, each with its state now: groups' own rows left
/// out, and one marked to show only when degraded left out while it isn't. `None` when the page
/// shows none.
pub fn statuspage_components(data: &[u8]) -> Option<Vec<Component>> {
    let summary: Value = serde_json::from_slice(data).ok()?;
    let mut listed = lenient(summary.get("components"), |c| {
        Some((
            text(c, "id")?,
            text(c, "name")?,
            text(c, "status")?,
            c.get("position").and_then(Value::as_i64),
            c.get("group").and_then(Value::as_bool) == Some(true),
            c.get("only_show_if_degraded").and_then(Value::as_bool) == Some(true),
        ))
    });
    listed.retain(|(_, _, status, _, group, only_degraded)| !group && (!only_degraded || status != "operational"));
    listed.sort_by_key(|(_, _, _, position, _, _)| position.unwrap_or(i64::MAX));
    let components: Vec<Component> =
        listed.into_iter().map(|(id, name, status, ..)| Component::new(id, name, State::from_feed(&status))).collect();
    (!components.is_empty()).then_some(components)
}

/// Each component's days, colours and uptime from the uptime showcase, the feed status.claude.com
/// draws its bars from.
pub fn statuspage_history(data: &[u8], components: Vec<Component>) -> Vec<Component> {
    let Ok(showcase) = serde_json::from_slice::<Value>(data) else { return components };
    let Some(timelines) = showcase.get("timelines").and_then(Value::as_object) else { return components };
    let bars = showcase.get("components").and_then(Value::as_object);
    let values = showcase.get("values");

    components
        .into_iter()
        .map(|mut component| {
            let Some(timeline) = timelines.get(&component.id).filter(|t| t.get("days").is_some_and(Value::is_array)) else {
                return component;
            };
            let started = timeline.get("component").and_then(|c| text(c, "startDate"));

            let mut days: Vec<Day> = lenient(timeline.get("days"), |day| {
                let label = text(day, "date")?;
                let parts: Vec<u32> = label.split('-').filter_map(|p| p.parse().ok()).collect();
                let [year, month, date] = parts[..] else { return None };
                let date = NaiveDate::from_ymd_opt(year as i32, month, date)?;
                // Same-format dates compare as text.
                if started.as_deref().is_some_and(|s| label.as_str() < s) {
                    return Some(Day { date, state: None, rgb: None });
                }
                let outage = |key: &str| day.get("outages").and_then(|o| o.get(key)).and_then(Value::as_i64).unwrap_or(0);
                let state = if outage("m") > 0 {
                    State::FullOutage
                } else if outage("p") > 0 {
                    State::PartialOutage
                } else {
                    State::Operational
                };
                Some(Day { date, state: Some(state), rgb: None })
            });
            // The page's own colours, only when there is one for every day: a partial list can't
            // be lined up with the days safely.
            let colours = bars.and_then(|b| b.get(&component.id)).and_then(Value::as_str).map(bar_colours).unwrap_or_default();
            if colours.len() == days.len() {
                for (day, rgb) in days.iter_mut().zip(colours) {
                    day.rgb = Some(rgb);
                }
            }
            component.days = days;
            component.uptime = values
                .and_then(Value::as_array)
                .and_then(|list| list.iter().find(|v| text(v, "component").as_deref() == Some(&component.id)))
                .and_then(|v| v.get("ninety"))
                .and_then(Value::as_f64);
            component
        })
        .collect()
}

/// The `fill` of each `uptime-day` bar in the page's SVG, in order.
pub fn bar_colours(html: &str) -> Vec<u32> {
    let (Ok(tag), Ok(fill)) = (Regex::new(r"<rect\b[^>]*>"), Regex::new(r##"fill="#([0-9a-fA-F]{6})""##)) else {
        return Vec::new();
    };
    tag.find_iter(html)
        .filter_map(|rect| {
            let rect = rect.as_str();
            if !rect.contains("uptime-day") {
                return None;
            }
            u32::from_str_radix(fill.captures(rect)?.get(1)?.as_str(), 16).ok()
        })
        .collect()
}

// ---------------------------------------------------------------------------
// A small cache in front of the pages
// ---------------------------------------------------------------------------

/// A page read is kept this long, so reopening a pane (or a second window) does not ask someone
/// else's server again within the minute.
const FRESH_FOR: Duration = Duration::from_secs(60);
/// A failed read is kept for less, so a pane that opens a moment later tries again.
const FAILED_FOR: Duration = Duration::from_secs(15);

/// Full reads of the status pages, kept briefly.
#[derive(Default)]
pub struct StatusCache {
    kept: Mutex<BTreeMap<StatusPage, (Instant, Option<ServiceStatus>)>>,
}

impl StatusCache {
    fn lookup(&self, page: StatusPage) -> Option<Option<ServiceStatus>> {
        let kept = self.kept.lock().ok()?;
        let (at, status) = kept.get(&page)?;
        let fresh = if status.is_some() { FRESH_FOR } else { FAILED_FOR };
        (at.elapsed() < fresh).then(|| status.clone())
    }

    /// The page's status, from the cache when a read is recent, else read now. `force` skips the
    /// cache (a Refresh button).
    pub async fn read(&self, page: StatusPage, http: &reqwest::Client, force: bool) -> Option<ServiceStatus> {
        if !force {
            if let Some(kept) = self.lookup(page) {
                return kept;
            }
        }
        let status = read(page, http, Utc::now(), &Calendar::local()).await;
        if let Ok(mut kept) = self.kept.lock() {
            kept.insert(page, (Instant::now(), status.clone()));
        }
        status
    }
}

#[cfg(test)]
mod tests;
