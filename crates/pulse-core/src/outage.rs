// Ported from upstream Usage/OutageMemory.swift and UsageAlerts.consider/list (service outages),
// Usage/RecapNotice.swift (the notification's words).
//! "When a service is down": what has been said about a provider's service going down, and the
//! rules for what to say next (`AppSettings::alerts_on_outage`).
//!
//! **The provider's word only.** A component is down when its own status page says degraded,
//! partial or full outage; maintenance is planned, and a value Pulse can't read is not a witnessed
//! outage. A page that can't be read changes nothing here: it is neither an outage nor a recovery.
//!
//! **Once per outage**, like every other alert: a component is announced when it first goes down,
//! again only if it gets worse, and once more when the page calls it operational, and that last
//! only for one that was announced, so a recovery is never news about an outage nobody heard of.
//! An outage already under way when the setting goes on is said at once: silence then a wall is
//! the feature failing.
//!
//! **Only what Pulse is still watching is remembered.** A component the page no longer lists (a
//! Statuspage row shown only while degraded, a renamed one) is forgotten without a word, because
//! its recovery was not seen; and a page nobody is watching (the switch off, the provider off) is
//! forgotten whole. Otherwise an entry outlived its outage: a later outage at the same severity
//! was silent, and switching back on days later said "back to normal" about something that ended
//! unwatched.
//!
//! Its own file (`status-alerts.json`), not a field on the usage memory: that type decodes as a
//! whole, and a key old files lack would have thrown away every limit already warned about.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::alerts::{Notification, Text};
use crate::recap::Period;
use crate::status::{Component, State, StatusPage};

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OutageMemory {
    /// By page (its provider's raw value), then by component id: the state last announced for a
    /// component still down.
    pub announced: BTreeMap<String, BTreeMap<String, State>>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Change {
    /// Down, or worse than when last announced, with the state now.
    pub worse: Vec<Component>,
    /// Announced as down, operational again.
    pub recovered: Vec<Component>,
}

impl Change {
    pub fn is_empty(&self) -> bool {
        self.worse.is_empty() && self.recovered.is_empty()
    }
}

impl OutageMemory {
    pub fn from_json(bytes: &[u8]) -> Self {
        serde_json::from_slice(bytes).unwrap_or_default()
    }

    pub fn to_json(&self) -> Vec<u8> {
        serde_json::to_vec_pretty(self).unwrap_or_default()
    }

    /// What one reading of a page is worth saying, and the record of having said it. `components`
    /// is everything on the page Pulse watches. Pure: no clock, no disk, no notification centre.
    pub fn changes(&mut self, components: &[Component], page: StatusPage) -> Change {
        let key = page.provider().raw().to_string();
        let said = self.announced.get(&key).cloned().unwrap_or_default();
        let mut now: BTreeMap<String, State> = BTreeMap::new();
        let mut change = Change::default();

        for component in components {
            let before = said.get(&component.id).copied();
            if component.state.is_outage() {
                if before.is_none_or(|b| component.state.severity() > b.severity()) {
                    change.worse.push(component.clone());
                }
                // Better but still down is recorded without a word, so getting worse again is
                // news again.
                now.insert(component.id.clone(), component.state);
            } else if component.state == State::Operational {
                if before.is_some() {
                    change.recovered.push(component.clone());
                }
            } else if let Some(before) = before {
                // Maintenance or an unknown value: neither down nor proven back, so what was said
                // stands.
                now.insert(component.id.clone(), before);
            }
        }
        // Anything not on the page any more is dropped here, unannounced.
        if now.is_empty() {
            self.announced.remove(&key);
        } else {
            self.announced.insert(key, now);
        }
        change
    }

    /// Forgets the pages Pulse is no longer watching.
    pub fn keep_only(&mut self, pages: &BTreeSet<StatusPage>) {
        let watched: BTreeSet<&str> = pages.iter().map(|p| p.provider().raw()).collect();
        self.announced.retain(|key, _| watched.contains(key.as_str()));
    }
}

/// The notification for a page's change: one per provider, every component in one sentence
/// ("OpenAI reports CLI (Partial outage) and Codex API (Degraded performance)."), identifier
/// `service-status-<provider>` so a newer word replaces the last. `None` for nothing to say.
pub fn notification(page: StatusPage, change: &Change) -> Option<Notification> {
    if change.is_empty() {
        return None;
    }
    let provider = page.provider();
    // A status, not an event: true whenever it is read.
    let down = if change.worse.is_empty() {
        Text::plain("")
    } else {
        let items = change
            .worse
            .iter()
            .map(|c| Text::key_with("%@ (%@)", vec![Text::plain(c.name.clone()), Text::key(c.state.title())]))
            .collect();
        Text::key_with("%@ reports %@.", vec![Text::plain(page.company()), Text::List(items)])
    };
    let back = if change.recovered.is_empty() {
        Text::plain("")
    } else {
        let items = change.recovered.iter().map(|c| Text::plain(c.name.clone())).collect();
        Text::key_with("Back to normal: %@.", vec![Text::List(items)])
    };
    Some(Notification {
        identifier: format!("service-status-{}", provider.raw()),
        account: provider.raw().to_string(),
        title: Text::plain(provider.display_name()),
        subtitle: Some(Text::key("Service status")),
        body: Text::Sentences(vec![down, back]),
    })
}

/// "Your September recap is ready": identifier `recap-ready-<yyyy>-<mm>`. `month_name` is the
/// month's name in the interface language (the shell has the locale). `None` for a year.
pub fn recap_notification(period: Period, month_name: &str) -> Option<Notification> {
    let Period::Month { .. } = period else { return None };
    Some(Notification {
        identifier: crate::recap::periods::notice::identifier(period),
        account: String::new(),
        title: Text::key("Monthly Recap"),
        subtitle: None,
        body: Text::key_with("Your %@ recap is ready", vec![Text::plain(month_name)]),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::Provider;

    fn component(id: &str, state: State) -> Component {
        Component::new(id, id, state)
    }

    #[test]
    fn an_outage_already_under_way_is_said_at_once_and_once() {
        let mut memory = OutageMemory::default();
        let first = memory.changes(&[component("cli", State::PartialOutage), component("web", State::Operational)], StatusPage::OpenAi);
        assert_eq!(first.worse.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["cli"]);
        assert!(first.recovered.is_empty());

        assert!(memory.changes(&[component("cli", State::PartialOutage)], StatusPage::OpenAi).is_empty());
    }

    #[test]
    fn worse_is_news_again_and_better_but_still_down_is_not() {
        let mut memory = OutageMemory::default();
        memory.changes(&[component("cli", State::Degraded)], StatusPage::OpenAi);

        let worse = memory.changes(&[component("cli", State::FullOutage)], StatusPage::OpenAi);
        assert_eq!(worse.worse.iter().map(|c| c.state).collect::<Vec<_>>(), [State::FullOutage]);
        assert!(memory.changes(&[component("cli", State::Degraded)], StatusPage::OpenAi).is_empty());
        // Recorded at degraded, so a second slide to full is said again.
        let again = memory.changes(&[component("cli", State::FullOutage)], StatusPage::OpenAi);
        assert_eq!(again.worse.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["cli"]);
    }

    #[test]
    fn back_to_normal_only_for_an_outage_that_was_announced() {
        let mut memory = OutageMemory::default();
        assert!(memory.changes(&[component("cli", State::Operational)], StatusPage::OpenAi).is_empty());

        memory.changes(&[component("cli", State::PartialOutage)], StatusPage::OpenAi);
        let back = memory.changes(&[component("cli", State::Operational)], StatusPage::OpenAi);
        assert_eq!(back.recovered.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["cli"]);
        assert!(memory.announced.is_empty());
        assert!(memory.changes(&[component("cli", State::Operational)], StatusPage::OpenAi).is_empty());
    }

    #[test]
    fn maintenance_or_an_unknown_value_neither_raises_nor_clears() {
        let mut memory = OutageMemory::default();
        let quiet = memory.changes(&[component("cli", State::Maintenance), component("web", State::Unrecognised)], StatusPage::OpenAi);
        assert!(quiet.is_empty());
        assert!(memory.announced.is_empty());

        memory.changes(&[component("cli", State::FullOutage)], StatusPage::OpenAi);
        assert!(memory.changes(&[component("cli", State::Maintenance)], StatusPage::OpenAi).is_empty());
        assert!(memory.changes(&[component("cli", State::Unrecognised)], StatusPage::OpenAi).is_empty());
        assert_eq!(memory.announced["codex"]["cli"], State::FullOutage);
    }

    #[test]
    fn a_component_gone_from_the_page_is_forgotten_so_its_next_outage_is_news() {
        let mut memory = OutageMemory::default();
        memory.changes(&[component("quiet", State::Degraded)], StatusPage::Claude);

        // A show-only-when-degraded row recovers by disappearing.
        assert!(memory.changes(&[], StatusPage::Claude).is_empty());
        assert!(memory.announced.is_empty());
        let again = memory.changes(&[component("quiet", State::Degraded)], StatusPage::Claude);
        assert_eq!(again.worse.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["quiet"]);
    }

    #[test]
    fn each_page_is_its_own() {
        let mut memory = OutageMemory::default();
        memory.changes(&[component("cli", State::Degraded)], StatusPage::OpenAi);
        memory.changes(&[component("api", State::Operational)], StatusPage::Claude);
        assert_eq!(memory.announced["codex"]["cli"], State::Degraded);
    }

    #[test]
    fn a_page_nobody_watches_any_more_is_forgotten_so_switching_back_on_says_nothing_stale() {
        let mut memory = OutageMemory::default();
        memory.changes(&[component("cli", State::Degraded)], StatusPage::OpenAi);
        memory.changes(&[component("api", State::Degraded)], StatusPage::DeepSeek);

        memory.keep_only(&BTreeSet::from([StatusPage::DeepSeek]));
        assert_eq!(memory.announced.keys().collect::<Vec<_>>(), ["deepSeek"]);

        memory.keep_only(&BTreeSet::new());
        // Back on after the outage ended unwatched: no "back to normal".
        assert!(memory.changes(&[component("cli", State::Operational)], StatusPage::OpenAi).is_empty());
    }

    #[test]
    fn the_memory_survives_a_round_trip_to_disk() {
        let mut memory = OutageMemory::default();
        memory.changes(&[component("cli", State::Degraded)], StatusPage::OpenAi);
        assert_eq!(OutageMemory::from_json(&memory.to_json()), memory);
        assert_eq!(OutageMemory::from_json(b"not json"), OutageMemory::default());
    }

    #[test]
    fn claude_code_hears_about_claude_code_and_the_api_not_the_rest_of_the_page() {
        let claude: Vec<&str> = ["rwppv331jlwc", "0qbwn08sd68x", "k8w3r06qmzrp", "yyzkbfz2thpt", "bpp5gb3hpjcl", "0scnb50nvy53"]
            .into_iter()
            .filter(|id| StatusPage::Claude.notifies_about(&component(id, State::FullOutage)))
            .collect();
        assert_eq!(claude, ["k8w3r06qmzrp", "yyzkbfz2thpt"]);
        assert!(StatusPage::OpenAi.notifies_about(&component("anything in the Codex group", State::FullOutage)));
    }

    #[test]
    fn the_outage_switch_asks_for_permission_but_leaves_the_usage_rules_asleep() {
        let settings = crate::settings::AppSettings { alerts_on_outage: true, ..Default::default() };
        assert!(crate::alerts::wants_alerts(&settings));
        assert!(!crate::alerts::wants_usage_alerts(&settings));
    }

    #[test]
    fn one_notification_per_provider_names_every_component() {
        let mut memory = OutageMemory::default();
        let change = memory.changes(
            &[component("cli", State::PartialOutage), component("api", State::Degraded)],
            StatusPage::OpenAi,
        );
        let note = notification(StatusPage::OpenAi, &change).unwrap();
        assert_eq!(note.identifier, "service-status-codex");
        assert_eq!(note.title, Text::plain(Provider::Codex.display_name()));
        assert_eq!(note.subtitle, Some(Text::key("Service status")));
        let Text::Sentences(parts) = &note.body else { panic!("a sentence list") };
        let Text::Key { key, args } = &parts[0] else { panic!("the outage sentence") };
        assert_eq!(key, "%@ reports %@.");
        assert_eq!(args[0], Text::plain("OpenAI"));
        let Text::List(items) = &args[1] else { panic!("a list") };
        assert_eq!(items.len(), 2);

        assert!(notification(StatusPage::OpenAi, &Change::default()).is_none());
    }

    #[test]
    fn the_recap_notification_is_about_a_month_and_carries_its_identifier() {
        let month = Period::Month { year: 2026, month: 9 };
        let note = recap_notification(month, "September").unwrap();
        assert_eq!(note.identifier, "recap-ready-2026-09");
        assert_eq!(note.body, Text::key_with("Your %@ recap is ready", vec![Text::plain("September")]));
        assert!(recap_notification(Period::Year(2026), "").is_none());
    }
}
