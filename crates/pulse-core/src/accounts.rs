// Ported from upstream App/AppSettings+Accounts.swift (addAccount, removeAccount, rename, label)
// and `ProviderSignInModel.label(for:provider:in:)`.
//! Adding and removing the accounts a provider can have more than one of.
//!
//! A provider's first account is just the provider; an added one gets a slot generated once and
//! never reused, so removing one and adding another cannot inherit settings.

use crate::auth::AccountCredentials;
use crate::model::AccountKey;
use crate::provider::Provider;
use crate::settings::{AppSettings, ExtraAccount};

impl AppSettings {
    /// Adds an account Pulse has just signed in to, switched on. The slot is generated here so
    /// it can never collide with one that has been removed.
    pub fn add_account(&mut self, provider: Provider, label: &str) -> AccountKey {
        self.add_account_with_slot(provider, label, uuid::Uuid::new_v4().to_string().to_uppercase())
    }

    pub fn add_account_with_slot(&mut self, provider: Provider, label: &str, slot: String) -> AccountKey {
        let key = AccountKey { provider, slot };
        let id = key.id();
        self.extra_accounts.push(ExtraAccount { id: id.clone(), provider: provider.raw().to_string(), name: label.to_string() });
        self.enabled_accounts.insert(id);
        key
    }

    /// Whether an account exists: a provider's first one always does.
    pub fn has_account(&self, account: &AccountKey) -> bool {
        account.is_primary() || self.extra_accounts.iter().any(|a| a.id == account.id())
    }

    /// Forgets an account, and everything stored against it: a later account must never inherit a
    /// removed one's pinned window, route, colours or any other per-account setting.
    ///
    /// **Add every new per-account store here.**
    pub fn remove_account(&mut self, account: &AccountKey) {
        if account.is_primary() {
            return;
        }
        let id = account.id();
        self.extra_accounts.retain(|a| a.id != id);
        // The set refuses to go empty: deleting the only enabled account would otherwise leave
        // its id behind, naming an account that no longer exists, and the rail would draw
        // nothing. The provider it belonged to takes its place.
        if self.enabled_accounts.len() == 1 && self.enabled_accounts.contains(&id) {
            self.enabled_accounts = [AccountKey::primary(account.provider).id()].into();
        } else {
            self.enabled_accounts.remove(&id);
        }
        self.provider_order.retain(|o| *o != id);
        self.pinned_windows.remove(&id);
        self.sources.remove(&id);
        self.ring_tints.remove(&id);
        self.session_browsers.remove(&id);
        self.server_addresses.remove(&id);
        self.low_balance_alerts.remove(&id);
        self.balance_bases.remove(&id);
        self.balance_budgets.remove(&id);
        self.detailed_cards.remove(&id);
        self.split_accounts.remove(&id);
        if self.tray_account.as_deref() == Some(id.as_str()) {
            self.tray_account = None;
        }
    }

    pub fn rename_account(&mut self, account: &AccountKey, label: &str) {
        let id = account.id();
        if let Some(extra) = self.extra_accounts.iter_mut().find(|a| a.id == id) {
            extra.name = label.to_string();
        }
    }

    /// What to call an account. A provider's first one is just the provider; the rest carry a
    /// label so two subscriptions can be told apart.
    pub fn account_label(&self, account: &AccountKey) -> String {
        let id = account.id();
        self.extra_accounts
            .iter()
            .find(|a| a.id == id)
            .map(|a| a.name.clone())
            .unwrap_or_else(|| account.provider.display_name().to_string())
    }

    /// What to call a newly added account.
    ///
    /// The part of the address before the "@", because the card's header is one line at a fixed
    /// width and a whole email address spends all of it. A provider that names nothing gets a
    /// number, which at least counts. Either way it is the user's to change.
    pub fn label_for_new(&self, credentials: &AccountCredentials, provider: Provider) -> String {
        if let Some(name) = credentials.account_name.as_deref().and_then(|n| n.split('@').next()).filter(|n| !n.is_empty()) {
            return name.to_string();
        }
        let existing = self.extra_accounts.iter().filter(|a| a.provider == provider.raw()).count();
        format!("{} {}", provider.display_name(), existing + 2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn credentials(name: Option<&str>) -> AccountCredentials {
        AccountCredentials {
            access_token: "a".into(),
            refresh_token: "r".into(),
            expires_at: Utc::now(),
            account_name: name.map(String::from),
            account_id: None,
        }
    }

    #[test]
    fn a_new_account_is_enabled_and_never_reuses_a_slot() {
        let mut settings = AppSettings::default();
        settings.enabled_accounts = ["codex".to_string()].into();
        let first = settings.add_account(Provider::Codex, "ann");
        let second = settings.add_account(Provider::Codex, "bo");
        assert_ne!(first, second);
        assert!(first.id().starts_with("codex#"));
        assert!(settings.enabled_accounts.contains(&first.id()));
        assert_eq!(settings.extra_accounts.len(), 2);
        assert!(settings.has_account(&first));
        assert_eq!(settings.account_label(&first), "ann");
        assert_eq!(settings.account_label(&AccountKey::primary(Provider::Codex)), "Codex");
        assert_eq!(AccountKey::from_id(&first.id()), Some(first));
    }

    #[test]
    fn labels_use_the_part_before_the_at_sign_or_a_count() {
        let mut settings = AppSettings::default();
        assert_eq!(settings.label_for_new(&credentials(Some("ann@example.com")), Provider::Codex), "ann");
        assert_eq!(settings.label_for_new(&credentials(None), Provider::ClaudeCode), "Claude Code 2");
        settings.add_account(Provider::ClaudeCode, "Work");
        assert_eq!(settings.label_for_new(&credentials(Some("@odd")), Provider::ClaudeCode), "Claude Code 3");
        assert_eq!(settings.label_for_new(&credentials(Some("")), Provider::Codex), "Codex 2");
    }

    #[test]
    fn removing_forgets_every_per_account_setting() {
        let mut settings = AppSettings::default();
        settings.enabled_accounts = ["claudeCode".to_string()].into();
        let key = settings.add_account(Provider::ClaudeCode, "Work");
        let id = key.id();
        settings.provider_order = vec![id.clone(), "claudeCode".into()];
        settings.pinned_windows.insert(id.clone(), "weekly".into());
        settings.sources.insert(id.clone(), "endpoint".into());
        settings.ring_tints.insert(id.clone(), "blue".into());
        settings.balance_budgets.insert(id.clone(), 5.0);
        settings.detailed_cards.insert(id.clone());
        settings.split_accounts.insert(id.clone());
        settings.tray_account = Some(id.clone());

        settings.remove_account(&key);
        assert!(settings.extra_accounts.is_empty());
        assert!(!settings.enabled_accounts.contains(&id));
        assert_eq!(settings.provider_order, vec!["claudeCode".to_string()]);
        assert!(settings.pinned_windows.is_empty() && settings.sources.is_empty() && settings.ring_tints.is_empty());
        assert!(settings.balance_budgets.is_empty() && settings.detailed_cards.is_empty() && settings.split_accounts.is_empty());
        assert_eq!(settings.tray_account, None);
        assert!(!settings.has_account(&key));
    }

    #[test]
    fn removing_the_only_enabled_account_falls_back_to_its_provider() {
        let mut settings = AppSettings::default();
        let key = settings.add_account(Provider::Grok, "x");
        settings.enabled_accounts = [key.id()].into();
        settings.remove_account(&key);
        assert_eq!(settings.enabled_accounts, ["grok".to_string()].into());
    }

    #[test]
    fn the_primary_account_cannot_be_removed() {
        let mut settings = AppSettings::default();
        settings.enabled_accounts = ["codex".to_string()].into();
        settings.remove_account(&AccountKey::primary(Provider::Codex));
        assert!(settings.enabled_accounts.contains("codex"));
    }

    #[test]
    fn rename_changes_only_the_label() {
        let mut settings = AppSettings::default();
        let key = settings.add_account(Provider::Codex, "ann");
        settings.rename_account(&key, "Personal");
        assert_eq!(settings.account_label(&key), "Personal");
        settings.rename_account(&AccountKey::primary(Provider::Codex), "nope");
        assert_eq!(settings.account_label(&AccountKey::primary(Provider::Codex)), "Codex");
    }
}
