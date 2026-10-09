// Ported from upstream App/AppSettings.swift and its topic files (bot-mark settings omitted).
//! Every stored preference, in one serde struct persisted as `settings.json`.
//! Field names match upstream's property names so the two can be compared.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum PanelSize {
    Small,
    #[default]
    Standard,
    Large,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum RailSpacing {
    Compact,
    #[default]
    Standard,
    Roomy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum WindowClockDirection {
    #[default]
    Elapsed,
    Remaining,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum TrayStyle {
    #[default]
    Figure,
    Ring,
    Split,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum AppLanguage {
    #[default]
    System,
    En,
    #[serde(rename = "zh-Hans")]
    ZhHans,
    #[serde(rename = "zh-Hant")]
    ZhHant,
    Ja,
    Ko,
}

/// Upstream `RefreshInterval`: adaptive (one-shot timer, 2–30 min) or fixed minutes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", tag = "type", content = "minutes")]
pub enum RefreshInterval {
    #[default]
    Adaptive,
    Fixed(u32),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct NetworkProxy {
    pub enabled: bool,
    pub scheme: String,
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtraAccount {
    pub id: String,
    pub provider: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GlobalShortcut {
    /// Accelerator in Tauri's syntax, e.g. "Ctrl+Alt+P".
    pub accelerator: String,
}

/// Hours of the day the window starter may act in, by this PC's clock (upstream `PrimerHours`;
/// the logic is in `window_starter::primer`). 0-23; equal start and end means all day.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrimerHours {
    pub start: u8,
    pub end: u8,
}

impl Default for PrimerHours {
    fn default() -> Self {
        Self { start: 7, end: 23 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppSettings {
    // Application
    pub hides_tray_icon: bool,
    pub tray_account: Option<String>,
    pub shows_usage_in_tray: bool,
    pub tray_style: TrayStyle,
    /// The tray icon's left click opens the usage dashboard popup (upstream `showsMenuDashboard`).
    pub shows_menu_dashboard: bool,
    pub open_settings_shortcut: Option<GlobalShortcut>,
    pub toggle_panel_shortcut: Option<GlobalShortcut>,
    pub language: AppLanguage,
    pub launch_at_login: bool,

    // Panel
    pub is_panel_visible: bool,
    pub hides_in_full_screen: bool,
    pub follows_active_display: bool,
    pub panel_size: PanelSize,
    pub rail_spacing: RailSpacing,
    pub top_rail_shows_percentages: bool,
    pub side_rail_shows_percentages: bool,
    pub label_above_ring: bool,
    pub free_across_figures_beside: bool,
    pub uses_round_ends: bool,
    pub uses_glass: bool,
    pub glass_transparency: f64,
    /// A light solid surface with dark content, instead of black (upstream #74). Ignored while
    /// `uses_glass` is on, and kept rather than cleared so turning glass off brings it back.
    pub uses_light_panel: bool,
    pub auto_collapse: bool,
    pub detailed_cards: BTreeSet<String>,

    // Rings
    pub shows_window_clock: bool,
    pub window_clock_direction: WindowClockDirection,
    pub shows_forecast: bool,
    pub shows_remaining: bool,
    /// Percent, 60–90.
    pub warning_threshold: u8,
    pub dock_shows_alert_color: bool,
    pub shows_second_ring: bool,
    pub animates_ring_activity: bool,
    pub split_accounts: BTreeSet<String>,
    pub pinned_windows: BTreeMap<String, String>,
    pub ring_tints: BTreeMap<String, String>,

    // Accounts
    pub enabled_accounts: BTreeSet<String>,
    pub extra_accounts: Vec<ExtraAccount>,
    pub provider_order: Vec<String>,
    pub offered_providers: BTreeSet<String>,

    // Providers
    pub sources: BTreeMap<String, String>,
    pub server_addresses: BTreeMap<String, String>,
    pub balance_bases: BTreeMap<String, String>,
    pub balance_budgets: BTreeMap<String, f64>,
    pub session_browsers: BTreeMap<String, String>,
    pub refresh_interval: RefreshInterval,
    pub network_proxy: NetworkProxy,

    // Token spend
    pub reads_token_spend: bool,

    // Recap
    /// The reader's monthly subscription price in US dollars, for the payback card. `None` and 0
    /// both mean no price.
    pub recap_monthly_price: Option<f64>,
    /// Replace project names on the cards with "Project 1", "Project 2"...
    pub recap_hides_projects: bool,
    /// The month ("2026-09") whose "recap is ready" notification was already sent.
    pub recap_announced_month: Option<String>,

    // Window starter (`window_starter`): off or empty by default, switched on only through the
    // pane's risk confirmation. Provider raw values ("claudeCode", "codex") throughout.
    pub primed_providers: BTreeSet<String>,
    pub primer_hours: PrimerHours,
    /// The last attempt's outcome per provider (`WindowStarter::Outcome` raw value).
    pub primer_run_outcomes: BTreeMap<String, String>,
    /// The last attempt's time per provider, seconds since 1970.
    pub primer_run_times: BTreeMap<String, f64>,

    // Notifications (all off by default)
    pub low_balance_alerts: BTreeMap<String, f64>,
    pub alert_threshold: Option<u8>,
    pub alerts_on_reset: bool,
    pub alerts_on_failure: bool,
    /// "When a service is down": the provider's own status page says its service is failing.
    pub alerts_on_outage: bool,
    /// "When a recap is ready": last month's recap, in the first days of the month.
    pub alerts_on_recap: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            hides_tray_icon: false,
            tray_account: None,
            shows_usage_in_tray: false,
            tray_style: TrayStyle::Figure,
            shows_menu_dashboard: false,
            open_settings_shortcut: None,
            toggle_panel_shortcut: None,
            language: AppLanguage::System,
            launch_at_login: true,
            is_panel_visible: true,
            hides_in_full_screen: true,
            follows_active_display: false,
            panel_size: PanelSize::Standard,
            rail_spacing: RailSpacing::Standard,
            top_rail_shows_percentages: false,
            side_rail_shows_percentages: true,
            label_above_ring: false,
            free_across_figures_beside: false,
            uses_round_ends: false,
            uses_glass: false,
            glass_transparency: 0.5,
            uses_light_panel: false,
            auto_collapse: false,
            detailed_cards: BTreeSet::new(),
            shows_window_clock: false,
            window_clock_direction: WindowClockDirection::Elapsed,
            shows_forecast: false,
            shows_remaining: false,
            warning_threshold: 75,
            dock_shows_alert_color: true,
            shows_second_ring: false,
            animates_ring_activity: true,
            split_accounts: BTreeSet::new(),
            pinned_windows: BTreeMap::new(),
            ring_tints: BTreeMap::new(),
            enabled_accounts: BTreeSet::new(),
            extra_accounts: Vec::new(),
            provider_order: Vec::new(),
            offered_providers: BTreeSet::new(),
            sources: BTreeMap::new(),
            server_addresses: BTreeMap::new(),
            balance_bases: BTreeMap::new(),
            balance_budgets: BTreeMap::new(),
            session_browsers: BTreeMap::new(),
            refresh_interval: RefreshInterval::Adaptive,
            network_proxy: NetworkProxy::default(),
            reads_token_spend: false,
            recap_monthly_price: None,
            recap_hides_projects: false,
            recap_announced_month: None,
            primed_providers: BTreeSet::new(),
            primer_hours: PrimerHours::default(),
            primer_run_outcomes: BTreeMap::new(),
            primer_run_times: BTreeMap::new(),
            low_balance_alerts: BTreeMap::new(),
            alert_threshold: None,
            alerts_on_reset: false,
            alerts_on_failure: false,
            alerts_on_outage: false,
            alerts_on_recap: false,
        }
    }
}

impl AppSettings {
    /// Upstream `needsProviderSelection`: nothing chosen yet, so no rail and no fetching.
    pub fn needs_provider_selection(&self) -> bool {
        self.enabled_accounts.is_empty()
    }

    /// Enabled accounts in display order: the stored order first (unknown ids
    /// dropped), then anything it does not mention, by name.
    pub fn ordered_enabled_accounts(&self) -> Vec<String> {
        let mut out: Vec<String> =
            self.provider_order.iter().filter(|id| self.enabled_accounts.contains(*id)).cloned().collect();
        let mut rest: Vec<String> =
            self.enabled_accounts.iter().filter(|id| !out.contains(id)).cloned().collect();
        rest.sort();
        out.extend(rest);
        out
    }

    /// Sanitize values a hand-edited file could carry.
    pub fn normalized(mut self) -> Self {
        self.warning_threshold = self.warning_threshold.clamp(60, 90);
        if !self.glass_transparency.is_finite() {
            self.glass_transparency = 0.5;
        }
        self.glass_transparency = self.glass_transparency.clamp(0.0, 1.0);
        // Hours are 0-23, as upstream's restore insists; anything else falls back to the default.
        if self.primer_hours.start > 23 || self.primer_hours.end > 23 {
            self.primer_hours = PrimerHours::default();
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_fields_take_defaults() {
        let s: AppSettings = serde_json::from_str(r#"{"panelSize":"large"}"#).unwrap();
        assert_eq!(s.panel_size, PanelSize::Large);
        assert!(s.side_rail_shows_percentages);
        assert_eq!(s.warning_threshold, 75);
    }

    #[test]
    fn the_window_starter_is_off_by_default_and_hours_stay_in_a_day() {
        let s = AppSettings::default();
        assert!(s.primed_providers.is_empty());
        assert_eq!(s.primer_hours, PrimerHours { start: 7, end: 23 });
        let s: AppSettings = serde_json::from_str(r#"{"primedProviders":["codex"],"primerHours":{"start":30,"end":2}}"#).unwrap();
        assert!(s.primed_providers.contains("codex"));
        assert_eq!(s.normalized().primer_hours, PrimerHours::default());
    }

    #[test]
    fn every_notification_starts_off_and_an_old_file_without_the_new_ones_loads() {
        let s = AppSettings::default();
        assert!(!s.alerts_on_outage && !s.alerts_on_recap && !s.alerts_on_reset && !s.alerts_on_failure);
        assert!(s.alert_threshold.is_none() && s.low_balance_alerts.is_empty());
        let old: AppSettings = serde_json::from_str(r#"{"alertsOnFailure":true}"#).unwrap();
        assert!(old.alerts_on_failure && !old.alerts_on_outage && !old.alerts_on_recap);
    }

    #[test]
    fn order_keeps_stored_then_appends_by_name() {
        let mut s = AppSettings::default();
        s.enabled_accounts = ["codex", "claudeCode", "cursor"].map(String::from).into();
        s.provider_order = vec!["cursor".into(), "gone".into()];
        assert_eq!(s.ordered_enabled_accounts(), vec!["cursor", "claudeCode", "codex"]);
    }
}
