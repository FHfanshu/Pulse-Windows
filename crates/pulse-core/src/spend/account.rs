// Windows difference: match estimates to the login behind the local transcript store, rather
// than assuming that only Pulse's default account can own it.
//! Transcripts do not reliably name their billing account. A current login therefore cannot
//! assign old history: only windows opened after both the login and its identity metadata were
//! last written are eligible. A renewal may temporarily withhold an estimate too; guessing an
//! older boundary would also accept an account switch. Tokens are never retained or exposed;
//! billing identities are used only in memory, never sent to the UI or saved in usage caches.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};
use serde_json::Value;

use crate::model::UsageWindow;
use crate::provider::Provider;

use super::transcripts::Sources;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountIdentity {
    user: String,
    organization: String,
}

impl AccountIdentity {
    fn new(user: &str, organization: &str) -> Option<Self> {
        if user.trim().is_empty() || organization.trim().is_empty() {
            return None;
        }
        Some(Self { user: user.into(), organization: organization.into() })
    }

    pub fn claude_profile(root: &Value) -> Option<Self> {
        Self::new(root.get("account")?.get("uuid")?.as_str()?, root.get("organization")?.get("uuid")?.as_str()?)
    }

    pub fn codex_token(token: &str, account: &str) -> Option<Self> {
        let claims = crate::auth::util::jwt_claims(token)?;
        let auth = claims.get("https://api.openai.com/auth")?;
        // The workspace alone is shared by different members. Both ids must agree, including
        // the workspace used in the usage request's ChatGPT-Account-Id header.
        if auth.get("chatgpt_account_id")?.as_str()? != account {
            return None;
        }
        Self::new(auth.get("chatgpt_user_id")?.as_str()?, account)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Stamp {
    path: PathBuf,
    modified: DateTime<Utc>,
    length: u64,
}

impl Stamp {
    fn read(path: PathBuf) -> Option<Self> {
        let metadata = std::fs::metadata(&path).ok()?;
        Some(Self { path, modified: metadata.modified().ok()?.into(), length: metadata.len() })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalAccount {
    identity: AccountIdentity,
    stamps: Vec<Stamp>,
}

impl LocalAccount {
    pub fn read(provider: Provider, sources: &Sources) -> Option<Self> {
        let (identity_path, login_path) = match provider {
            Provider::ClaudeCode => {
                let dir = sources.claude_config_dir.clone().unwrap_or_else(|| sources.home.join(".claude"));
                // WINDOWS-PATH: unverified; CLI identity metadata, tested with fixtures only.
                let config = sources.claude_config_dir.as_ref().map(|d| d.join(".claude.json"))
                    .unwrap_or_else(|| sources.home.join(".claude.json"));
                (config, dir.join(".credentials.json"))
            }
            Provider::Codex => {
                let path = sources.codex_home.clone().unwrap_or_else(|| sources.home.join(".codex")).join("auth.json");
                (path.clone(), path)
            }
            _ => return None,
        };
        let stamps = vec![Stamp::read(identity_path.clone())?, Stamp::read(login_path)?];
        let root: Value = serde_json::from_slice(&std::fs::read(&identity_path).ok()?).ok()?;
        let identity = match provider {
            Provider::ClaudeCode => {
                let account = root.get("oauthAccount")?;
                AccountIdentity::new(account.get("accountUuid")?.as_str()?, account.get("organizationUuid")?.as_str()?)?
            }
            Provider::Codex => {
                // API-key stores and external token modes are not subscription histories.
                if root.get("OPENAI_API_KEY").and_then(Value::as_str).is_some_and(|s| !s.is_empty())
                    || root.get("auth_mode").and_then(Value::as_str).is_some_and(|mode| mode != "chatgpt") {
                    return None;
                }
                let tokens = root.get("tokens")?;
                AccountIdentity::codex_token(tokens.get("access_token")?.as_str()?, tokens.get("account_id")?.as_str()?)?
            }
            _ => return None,
        };
        let local = Self { identity, stamps };
        local.unchanged().then_some(local)
    }

    pub fn matches(&self, identity: Option<&AccountIdentity>) -> bool {
        identity == Some(&self.identity)
    }

    pub fn covers(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> bool {
        from < to && self.stamps.iter().all(|s| s.modified <= from)
    }

    pub fn covers_window(&self, window: &UsageWindow, read: DateTime<Utc>) -> bool {
        window.reports_length && window.window_seconds > 0
            && window.resets_at.is_some_and(|reset| {
                reset.checked_sub_signed(Duration::seconds(window.window_seconds))
                    .is_some_and(|opened| self.covers(opened, read) && read < reset)
            })
    }

    pub fn unchanged(&self) -> bool {
        self.stamps.iter().all(|stamp| Stamp::read(stamp.path.clone()).as_ref() == Some(stamp))
    }

    pub fn estimate(&self, window: &UsageWindow, ledger: &super::Ledger, observed_at: Option<DateTime<Utc>>, now: DateTime<Utc>) -> Option<super::budget::BudgetEstimate> {
        let read = observed_at?.min(now);
        if !self.covers_window(window, read) || !self.unchanged() || ledger.has_read_limitations || ledger.has_partial_counts {
            return None;
        }
        let opened = window.resets_at?.checked_sub_signed(Duration::seconds(window.window_seconds))?;
        if ledger.slots.iter().any(|slot| slot.start < read && slot.start + Duration::minutes(15) > opened && slot.unpriced_tokens > 0) {
            return None;
        }
        super::budget::estimate(window, ledger, observed_at, now)
    }
}

/// Read and check the same store before and after scanning it. Switching logins during a scan
/// must not return a ledger paired with the previous login's percentages.
pub fn read_ledger(provider: Provider, identity: Option<&AccountIdentity>, home: &Path, now: DateTime<Utc>) -> Option<(LocalAccount, super::Ledger)> {
    let sources = Sources::from_env(home);
    let local = LocalAccount::read(provider, &sources)?;
    if !local.matches(identity) {
        return None;
    }
    let dir = crate::paths::data_dir();
    let prices = super::ModelPrices::cached(&dir);
    let ledger = super::read_ledger_with(provider, &sources, &dir, &super::Calendar::local(), &prices, now).ok()?;
    if ledger.has_read_limitations || ledger.has_partial_counts || !local.unchanged() {
        return None;
    }
    Some((local, ledger))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::util::base64url;
    use crate::model::WindowKind;
    use serde_json::json;

    fn token(user: &str, account: &str) -> String {
        let claims = json!({"https://api.openai.com/auth": {"chatgpt_user_id": user, "chatgpt_account_id": account}});
        format!("e30.{}.fixture", base64url(claims.to_string().as_bytes()))
    }

    #[test]
    fn identities_require_both_user_and_subscription_and_never_match_names() {
        let one = AccountIdentity::claude_profile(&json!({"account": {"uuid": "a"}, "organization": {"uuid": "o"}})).unwrap();
        let another = AccountIdentity::claude_profile(&json!({"account": {"uuid": "b"}, "organization": {"uuid": "o"}})).unwrap();
        assert_ne!(one, another);
        assert!(AccountIdentity::claude_profile(&json!({"account": {"email": "same"}, "organization": {"uuid": "o"}})).is_none());
        assert!(AccountIdentity::new("", "o").is_none());
        assert!(AccountIdentity::codex_token(&token("u", "o"), "other").is_none());
        assert_ne!(AccountIdentity::codex_token(&token("u", "o"), "o"), AccountIdentity::codex_token(&token("other", "o"), "o"));
    }

    #[test]
    fn added_and_default_accounts_use_identity_not_slot_and_switches_invalidate_history() {
        let dir = tempfile::tempdir().unwrap();
        let codex = dir.path().join("custom-codex");
        std::fs::create_dir_all(&codex).unwrap();
        let path = codex.join("auth.json");
        let write = |user: &str, account: &str| std::fs::write(&path, json!({"tokens": {"access_token": token(user, account), "account_id": account}}).to_string()).unwrap();
        write("user-a", "account-a");
        let mut sources = Sources::new(dir.path());
        sources.codex_home = Some(codex);
        let local = LocalAccount::read(Provider::Codex, &sources).unwrap();
        assert!(local.matches(AccountIdentity::codex_token(&token("user-a", "account-a"), "account-a").as_ref()));
        assert!(!local.matches(None));
        assert!(!local.matches(AccountIdentity::codex_token(&token("user-b", "account-b"), "account-b").as_ref()));
        let now = Utc::now();
        assert!(!local.covers(now - Duration::days(7), now));
        let window = UsageWindow::new("five", WindowKind::FiveHour, 0.5, 5 * 3600)
            .with_reset(Some(now + Duration::hours(6)));
        assert!(local.covers_window(&window, now + Duration::hours(2)));
        write("user-b", "account-b-longer");
        assert!(!local.unchanged());
        assert!(!LocalAccount::read(Provider::Codex, &sources).unwrap().matches(Some(&local.identity)));
    }

    #[test]
    fn claude_overrides_require_identity_and_login_metadata_and_changes_abort() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("custom-claude");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(config.join(".claude.json"), json!({"oauthAccount": {"accountUuid": "u", "organizationUuid": "o"}}).to_string()).unwrap();
        let mut sources = Sources::new(dir.path());
        sources.claude_config_dir = Some(config.clone());
        assert!(LocalAccount::read(Provider::ClaudeCode, &sources).is_none());
        std::fs::write(config.join(".credentials.json"), b"fixture").unwrap();
        let local = LocalAccount::read(Provider::ClaudeCode, &sources).unwrap();
        assert!(local.matches(AccountIdentity::claude_profile(&json!({"account": {"uuid": "u"}, "organization": {"uuid": "o"}})).as_ref()));
        std::fs::remove_file(config.join(".credentials.json")).unwrap();
        assert!(!local.unchanged());
    }

    #[test]
    fn matched_accounts_use_their_own_percentage_and_incomplete_or_pre_switch_windows_are_withheld() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("login-fixture");
        std::fs::write(&path, b"fixture").unwrap();
        let now = Utc::now();
        let from = now - Duration::hours(2);
        std::fs::File::options().write(true).open(&path).unwrap().set_times(
            std::fs::FileTimes::new().set_modified((from - Duration::hours(1)).into()),
        ).unwrap();
        let local = LocalAccount { identity: AccountIdentity::new("u", "o").unwrap(), stamps: vec![Stamp::read(path.clone()).unwrap()] };
        let ledger = super::super::Ledger {
            slots: vec![super::super::Slot::new(from - Duration::minutes(15), 0, 0.0), super::super::Slot::new(from + Duration::minutes(15), 1_000, 5.0)],
            ..super::super::Ledger::empty()
        };
        let window = |fraction| UsageWindow::new("five", WindowKind::FiveHour, fraction, 5 * 3600).with_reset(Some(from + Duration::hours(5)));
        assert_eq!(local.estimate(&window(0.1), &ledger, Some(now), now).unwrap().full, 50.0);
        assert_eq!(local.estimate(&window(0.5), &ledger, Some(now), now).unwrap().full, 10.0);
        assert!(local.estimate(&window(0.1), &ledger, None, now).is_none());
        let mut incomplete = ledger.clone();
        incomplete.has_read_limitations = true;
        assert!(local.estimate(&window(0.1), &incomplete, Some(now), now).is_none());
        incomplete.has_read_limitations = false;
        incomplete.slots[1].unpriced_tokens = 100;
        assert!(local.estimate(&window(0.1), &incomplete, Some(now), now).is_none());
        // Renewals and account switches both change the proof boundary. Neither can lend
        // pre-change history to the new reading, even if the file still exists.
        std::fs::write(&path, b"new-login-fixture").unwrap();
        assert!(local.estimate(&window(0.1), &ledger, Some(now), now).is_none());
        let renewed = LocalAccount { identity: local.identity, stamps: vec![Stamp::read(path).unwrap()] };
        assert!(renewed.estimate(&window(0.1), &ledger, Some(now), now).is_none());
    }

    #[test]
    fn billing_identity_is_never_serialized_or_restored_from_the_usage_cache() {
        let mut usage = crate::model::ProviderUsage::live(crate::model::AccountKey::primary(Provider::Codex), vec![], Utc::now());
        usage.spend_identity = AccountIdentity::new("private-user-fixture", "private-org-fixture");
        let json = serde_json::to_string(&usage).unwrap();
        assert!(!json.contains("private-") && !json.contains("spendIdentity"));
        let restored: crate::model::ProviderUsage = serde_json::from_str(&json).unwrap();
        assert!(restored.spend_identity.is_none());
    }
}
