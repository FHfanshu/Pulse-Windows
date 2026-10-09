// Ported from upstream Tests/PulseTests/ServiceStatusTests.swift.
//! Provider status pages: status.openai.com's Codex group, every component status.claude.com
//! shows, and status.deepseek.com's. The fixtures were captured live on 2026-10-04 and trimmed to
//! what is read; the bars below were read off the pages themselves the same day, so the history
//! tests are the page, bar for bar. `openai-status-incident` is written by hand.

use chrono::{DateTime, NaiveDate, Utc};

use super::*;

const OPENAI_OPERATIONAL: &[u8] = include_bytes!("../../tests/fixtures/openai-status-operational.json");
const OPENAI_INCIDENT: &[u8] = include_bytes!("../../tests/fixtures/openai-status-incident.json");
const OPENAI_IMPACTS: &[u8] = include_bytes!("../../tests/fixtures/openai-status-impacts.json");
const CLAUDE_SUMMARY: &[u8] = include_bytes!("../../tests/fixtures/claude-status-summary.json");
const CLAUDE_UPTIME: &[u8] = include_bytes!("../../tests/fixtures/claude-status-uptime.json");
const DEEPSEEK_PAGE: &[u8] = include_bytes!("../../tests/fixtures/deepseek-status-page.html");

fn shanghai() -> Calendar {
    Calendar::with_zone(chrono_tz::Asia::Shanghai, 2)
}

fn at(text: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(text).unwrap().with_timezone(&Utc)
}

fn date(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

/// One character a day: 0 operational, 1 degraded, 2 partial, 3 full, m maintenance, ?
/// unrecognised, - no record.
fn bars(component: &Component) -> String {
    component
        .days
        .iter()
        .map(|day| match day.state {
            None => '-',
            Some(State::Operational) => '0',
            Some(State::Degraded) => '1',
            Some(State::PartialOutage) => '2',
            Some(State::FullOutage) => '3',
            Some(State::Maintenance) => 'm',
            Some(State::Unrecognised) => '?',
        })
        .collect()
}

fn names(components: &[Component]) -> Vec<&str> {
    components.iter().map(|c| c.name.as_str()).collect()
}

// MARK: - status.openai.com

#[test]
fn the_live_page_lists_codexs_four_components_all_operational() {
    let components = incident_io_components(OPENAI_OPERATIONAL, "Codex").unwrap();
    assert_eq!(names(&components), ["Codex Web", "Codex API", "CLI", "VS Code extension"]);
    assert!(components.iter().all(|c| c.state == State::Operational));
}

#[test]
fn affected_components_take_their_stated_state_and_the_rest_are_operational() {
    let components = incident_io_components(OPENAI_INCIDENT, "Codex").unwrap();
    // Hidden rows are dropped; another group's outage is not Codex's.
    assert_eq!(names(&components), ["Codex Web", "Codex API", "CLI", "VS Code extension"]);
    let states: Vec<State> = components.iter().map(|c| c.state).collect();
    assert_eq!(states, [State::Degraded, State::Operational, State::FullOutage, State::Unrecognised]);
}

#[test]
fn each_day_takes_its_worst_impact_the_pages_own_bars_every_one() {
    let calendar = shanghai();
    let now = at("2026-10-04T12:00:00+08:00");
    let days = calendar_days(now, 91, &calendar);
    assert_eq!(days.first().unwrap().start, at("2026-07-05T16:00:00Z"));

    let current = incident_io_components(OPENAI_OPERATIONAL, "Codex").unwrap();
    let components = incident_io_history(OPENAI_IMPACTS, &days, now, current);

    let page = [
        ("Codex Web", "0000000000000010011100000000000000000000000000000000000000011000000000000000000000300010000"),
        ("Codex API", "0000000000000000011100000000000000000000000000100000000000011000000000000000000000300010000"),
        ("CLI", "0000000000001010011100000000000000000000000000000000000000011000000000000000000000300010000"),
        ("VS Code extension", "0000000000000010011100000000000000000000000000000000000000011000000000000000000000300010000"),
    ];
    for component in &components {
        let expected = page.iter().find(|(name, _)| *name == component.name).unwrap().1;
        assert_eq!(bars(component), expected, "{}", component.name);
        assert_eq!(component.uptime, Some(99.95));
        assert!(component.days.iter().all(|d| d.rgb.is_none()));
    }
    assert_eq!(components[0].days.first().unwrap().date, date(2026, 7, 6));
    assert_eq!(components[0].days.last().unwrap().date, date(2026, 10, 4));
}

#[test]
fn days_that_ended_before_the_page_had_data_have_no_record() {
    let calendar = shanghai();
    // Codex API's data starts 2026-03-26T22:18Z; a window over that week.
    let now = at("2026-03-29T12:00:00+08:00");
    let days = calendar_days(now, 6, &calendar);
    let current = vec![Component::new("01KMP3KP5MGE23B80K1EK4S8PV", "Codex API", State::Operational)];

    let components = incident_io_history(OPENAI_IMPACTS, &days, now, current);
    // 24th-26th local ended before 06:18 on the 27th local; the 27th holds it.
    assert_eq!(bars(&components[0]), "---000");
}

#[test]
fn a_missing_group_or_a_feed_that_isnt_one_is_no_reading_not_all_clear() {
    assert!(incident_io_components(OPENAI_OPERATIONAL, "Nonexistent").is_none());
    assert!(incident_io_components(b"{}", "Codex").is_none());
    assert!(incident_io_components(b"<html></html>", "Codex").is_none());
}

#[test]
fn a_history_that_cant_be_read_leaves_the_current_state_standing() {
    let current = incident_io_components(OPENAI_OPERATIONAL, "Codex").unwrap();
    let components = incident_io_history(b"<html></html>", &[], Utc::now(), current.clone());
    assert_eq!(components, current);
}

// MARK: - status.claude.com

#[test]
fn every_component_the_page_shows_in_the_pages_order() {
    let components = statuspage_components(CLAUDE_SUMMARY).unwrap();
    assert_eq!(
        names(&components),
        [
            "claude.ai",
            "Claude Console (platform.claude.com)",
            "Claude API (api.anthropic.com)",
            "Claude Code",
            "Claude Cowork",
            "Claude for Government",
        ]
    );
    assert!(components.iter().all(|c| c.state == State::Operational));
    assert!(statuspage_components(b"{}").is_none());
}

#[test]
fn groups_own_rows_are_left_out_and_a_show_when_degraded_one_while_it_isnt() {
    let summary = br#"{"components": [
      {"id": "b", "name": "Second", "status": "operational", "position": 2},
      {"id": "g", "name": "A group", "status": "operational", "position": 0, "group": true},
      {"id": "a", "name": "First", "status": "operational", "position": 1, "group_id": "g"},
      {"id": "quiet", "name": "Quiet", "status": "operational", "position": 3, "only_show_if_degraded": true},
      {"id": "loud", "name": "Loud", "status": "partial_outage", "position": 4, "only_show_if_degraded": true}
    ]}"#;
    let components = statuspage_components(summary).unwrap();
    assert_eq!(names(&components), ["First", "Second", "Loud"]);
    assert_eq!(components.last().unwrap().state, State::PartialOutage);
}

#[test]
fn claudes_ninety_days_carry_the_pages_colours_and_its_uptime_figures() {
    let current = statuspage_components(CLAUDE_SUMMARY).unwrap();
    let components = statuspage_history(CLAUDE_UPTIME, current);

    // The figures the page printed that day, in its order.
    let uptimes: Vec<Option<f64>> = components.iter().map(|c| c.uptime).collect();
    assert_eq!(uptimes, [99.44, 99.95, 99.52, 99.44, 99.44, 100.0].map(Some));
    assert!(components.iter().all(|c| c.days.len() == 90));

    let code = components.iter().find(|c| c.name == "Claude Code").unwrap();
    assert_eq!(code.days.first().unwrap().date, date(2026, 7, 7));
    assert_eq!(code.days.last().unwrap().date, date(2026, 10, 4));
    assert_eq!(code.days[0].state, Some(State::PartialOutage));
    assert_eq!(code.days[0].rgb, Some(0xE75F36));
    assert_eq!(code.days[1].rgb, Some(0x76AD2A));

    // The state read from the outage seconds agrees with the page's own colour: green exactly on
    // the days with no outage.
    for component in &components {
        for day in &component.days {
            assert_eq!(day.rgb == Some(0x76AD2A), day.state == Some(State::Operational), "{} {}", component.name, day.date);
        }
    }
}

#[test]
fn colours_are_used_only_when_there_is_one_for_every_day() {
    let one = r##"<rect height="34" width="3" x="0" y="0" fill="#e75f36" role="tab" class="uptime-day component-x day-0" />"##;
    assert_eq!(bar_colours(one), [0xE75F36]);
    assert!(bar_colours(r##"<rect fill="#000000" class="legend" />"##).is_empty());

    let showcase = serde_json::json!({
        "timelines": {"x": {"component": {"startDate": "2026-10-02"}, "days": [
            {"date": "2026-10-01", "outages": {}},
            {"date": "2026-10-02", "outages": {"m": 60}},
            {"date": "2026-10-03", "outages": {"p": 60}}
        ]}},
        "values": [],
        "components": {"x": one}
    });
    let components = statuspage_history(showcase.to_string().as_bytes(), vec![Component::new("x", "X", State::Operational)]);
    let x = &components[0];
    assert_eq!(bars(x), "-32");
    assert!(x.days.iter().all(|d| d.rgb.is_none()));
    assert_eq!(x.uptime, None);
}

// MARK: - status.deepseek.com

#[test]
fn deepseeks_page_every_component_in_its_order_a_section_opened_into_its_own() {
    let components = flashcat(DEEPSEEK_PAGE, Utc::now(), None).unwrap();
    assert_eq!(
        names(&components),
        [
            "DeepSeek V4 Pro API服务(API Service)",
            "DeepSeek V4.1 Flash API服务(API Service)",
            "对话服务(Chatservice)",
            "上传文件服务(File Upload Service)",
            "搜索服务(Search Service)",
        ]
    );
    assert!(components.iter().all(|c| c.state == State::Operational));
    assert!(components.iter().all(|c| c.days.is_empty()));
    // Notified about: the API rows, which is what Pulse's DeepSeek account is.
    let notified: Vec<String> =
        components.iter().filter(|c| StatusPage::DeepSeek.notifies_about(c)).map(|c| c.name.clone()).collect();
    assert_eq!(notified, ["DeepSeek V4 Pro API服务(API Service)", "DeepSeek V4.1 Flash API服务(API Service)"]);
}

#[test]
fn deepseeks_days_are_the_pages_own_bars_every_one_in_local_days() {
    let calendar = shanghai();
    let now = at("2026-10-04T16:00:00+08:00");
    let days = calendar_days(now, 90, &calendar);
    let components = flashcat(DEEPSEEK_PAGE, now, Some(&days)).unwrap();

    // Read off the page in a browser in UTC+8, the same day.
    let page = [
        "000000000000000001000101100120020000001000010000000000000300000000000010100000220000000100",
        "000000000000000001000100200020020010000000010120000000000000000000000020000100220000000100",
        "000000000000000200000000100000000000000000000000000000000000000000000020000100220000000100",
        "---------------000000000000000000000000000000000000000000000000000000000000000000000000000",
        "---------------000000020000000000000003000200000000000000000000000000000000000000000000000",
    ];
    let read: Vec<String> = components.iter().map(bars).collect();
    assert_eq!(read, page);
    let uptimes: Vec<Option<f64>> = components.iter().map(|c| c.uptime).collect();
    assert_eq!(uptimes, [99.92, 99.69, 99.68, 100.0, 99.91].map(Some));
    assert!(components.iter().all(|c| c.days.iter().all(|d| d.rgb.is_none())));
}

#[test]
fn an_open_impact_is_the_state_now_and_a_page_without_the_data_is_no_reading() {
    let flight = concat!(
        "1:[\"$\",\"$L2\",null,{\"initialData\":{\"page\":{\"components\":[",
        "{\"component_id\":\"a\",\"name\":\"X API\",\"section_id\":null,\"status\":null,\"order_id\":1},",
        "{\"component_id\":\"b\",\"name\":\"Y API\",\"section_id\":null,\"status\":\"maintenance\",\"order_id\":2}],",
        "\"sections\":[]}}}]\n",
        "2:[\"$\",\"$L3\",null,{\"initialData\":{\"component_impacts\":[",
        "{\"component_id\":\"a\",\"start_at_seconds\":100,\"status\":\"partial_outage\"},",
        "{\"component_id\":\"a\",\"start_at_seconds\":10,\"end_at_seconds\":20,\"status\":\"full_outage\"}],",
        "\"component_uptimes\":[]}}]\n",
    );
    let literal = serde_json::to_string(flight).unwrap();
    let html = format!("<script>self.__next_f.push([1,{literal}])</script>");

    let now = DateTime::from_timestamp(200, 0).unwrap();
    let components = flashcat(html.as_bytes(), now, None).unwrap();
    let states: Vec<State> = components.iter().map(|c| c.state).collect();
    assert_eq!(states, [State::PartialOutage, State::Maintenance]);

    assert!(flashcat(b"<html></html>", Utc::now(), None).is_none());
}

#[test]
fn each_feed_value_maps_to_its_state() {
    assert_eq!(State::from_feed("operational"), State::Operational);
    assert_eq!(State::from_feed("degraded_performance"), State::Degraded);
    assert_eq!(State::from_feed("partial_outage"), State::PartialOutage);
    assert_eq!(State::from_feed("full_outage"), State::FullOutage);
    assert_eq!(State::from_feed("major_outage"), State::FullOutage);
    assert_eq!(State::from_feed("under_maintenance"), State::Maintenance);
    assert_eq!(State::from_feed("degraded"), State::Degraded);
    assert_eq!(State::from_feed("maintenance"), State::Maintenance);
    assert_eq!(State::from_feed(""), State::Unrecognised);
}

#[test]
fn the_pages_and_their_providers_agree() {
    for page in StatusPage::ALL {
        assert_eq!(StatusPage::for_provider(page.provider()), Some(page));
    }
    assert_eq!(StatusPage::for_provider(Provider::Cursor), None);
    assert_eq!(StatusPage::Claude.host(), "status.claude.com");
}
