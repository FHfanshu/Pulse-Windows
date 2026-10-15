// Windows difference: match estimates to the login behind the local transcript store, rather
// than assuming that only Pulse's default account can own it.
//! Transcripts do not reliably name their billing account. A current login therefore cannot
//! assign old history: only windows opened after Pulse knows that login became the local one are
//! eligible (see [`LocalAccount::read`]). Tokens are never retained or exposed; billing identities
//! are used only in memory, never sent to the UI or saved in usage caches, and Pulse's own record
//! of local logins keeps only their SHA-256.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

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

    fn digest(&self) -> String {
        let hash = Sha256::digest(format!("{}\n{}", self.user, self.organization).as_bytes());
        hash.iter().map(|b| format!("{b:02x}")).collect()
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

/// Pulse's own record of the local CLI login it has seen per provider, so a window can be
/// attributed to the login that held it from its start. Only a SHA-256 of the identity is kept.
const RECORD: &str = "local-logins.json";
/// How often the "still the same login" time is written back while nothing changes.
const SEEN_REFRESH_MINUTES: i64 = 10;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Seen {
    identity: String,
    /// The earliest time this login is known to have been the local one.
    since: DateTime<Utc>,
    /// The latest time it was still seen. A different login cannot have started before this.
    seen: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalAccount {
    provider: Provider,
    identity: AccountIdentity,
    identity_path: PathBuf,
    since: DateTime<Utc>,
}

impl LocalAccount {
    fn paths(provider: Provider, sources: &Sources) -> Option<(PathBuf, PathBuf)> {
        match provider {
            Provider::ClaudeCode => {
                let dir = sources.claude_config_dir.clone().unwrap_or_else(|| sources.home.join(".claude"));
                // WINDOWS-PATH: unverified; CLI identity metadata, tested with fixtures only.
                let config = sources.claude_config_dir.as_ref().map(|d| d.join(".claude.json"))
                    .unwrap_or_else(|| sources.home.join(".claude.json"));
                Some((config, dir.join(".credentials.json")))
            }
            Provider::Codex => {
                let path = sources.codex_home.clone().unwrap_or_else(|| sources.home.join(".codex")).join("auth.json");
                Some((path.clone(), path))
            }
            _ => None,
        }
    }

    fn identity_at(provider: Provider, path: &Path) -> Option<AccountIdentity> {
        let root: Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
        match provider {
            Provider::ClaudeCode => {
                let account = root.get("oauthAccount")?;
                AccountIdentity::new(account.get("accountUuid")?.as_str()?, account.get("organizationUuid")?.as_str()?)
            }
            Provider::Codex => {
                // API-key stores and external token modes are not subscription histories.
                if root.get("OPENAI_API_KEY").and_then(Value::as_str).is_some_and(|s| !s.is_empty())
                    || root.get("auth_mode").and_then(Value::as_str).is_some_and(|mode| mode != "chatgpt") {
                    return None;
                }
                let tokens = root.get("tokens")?;
                AccountIdentity::codex_token(tokens.get("access_token")?.as_str()?, tokens.get("account_id")?.as_str()?)
            }
            _ => None,
        }
    }

    /// The current local login, and since when it has been the local one. The CLI rewrites its
    /// metadata constantly (Claude Code's `.claude.json` on every session) and renews tokens, so
    /// file times alone would withhold nearly every estimate. Instead the boundary moves only when
    /// the identity changes: then it is the later of the login file's time and the last time the
    /// previous login was seen. The first sighting uses the login file's time.
    pub fn read(provider: Provider, sources: &Sources, record_dir: &Path, now: DateTime<Utc>) -> Option<Self> {
        let (identity_path, login_path) = Self::paths(provider, sources)?;
        let login: DateTime<Utc> = std::fs::metadata(&login_path).ok()?.modified().ok()?.into();
        let identity = Self::identity_at(provider, &identity_path)?;
        let digest = identity.digest();
        let record_path = record_dir.join(RECORD);
        let mut records: BTreeMap<String, Seen> =
            std::fs::read(&record_path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let key = format!("{provider:?}");
        let previous = records.get(&key).cloned();
        let next = match &previous {
            Some(r) if r.identity == digest => Seen { identity: digest, since: r.since.min(login), seen: r.seen.max(now) },
            Some(r) => Seen { identity: digest, since: login.max(r.seen), seen: now },
            None => Seen { identity: digest, since: login, seen: now },
        };
        let since = next.since;
        let stale = previous.as_ref().is_none_or(|r| {
            r.identity != next.identity || r.since != next.since || now - r.seen >= Duration::minutes(SEEN_REFRESH_MINUTES)
        });
        if stale {
            records.insert(key, next);
            if let Ok(bytes) = serde_json::to_vec_pretty(&records) {
                let tmp = record_path.with_extension("tmp");
                if std::fs::write(&tmp, bytes).is_ok() {
                    let _ = std::fs::rename(&tmp, &record_path);
                }
            }
        }
        Some(Self { provider, identity, identity_path, since })
    }

    pub fn matches(&self, identity: Option<&AccountIdentity>) -> bool {
        identity == Some(&self.identity)
    }

    pub fn covers(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> bool {
        from < to && self.since <= from
    }

    pub fn covers_window(&self, window: &UsageWindow, read: DateTime<Utc>) -> bool {
        window.reports_length && window.window_seconds > 0
            && window.resets_at.is_some_and(|reset| {
                reset.checked_sub_signed(Duration::seconds(window.window_seconds))
                    .is_some_and(|opened| self.covers(opened, read) && read < reset)
            })
    }

    /// The same login is still the local one: a switch during a scan must not pair the ledger
    /// with the previous login's percentages.
    pub fn unchanged(&self) -> bool {
        Self::identity_at(self.provider, &self.identity_path).as_ref() == Some(&self.identity)
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

/// Read and check the same store before and after scanning it.
pub fn read_ledger(provider: Provider, identity: Option<&AccountIdentity>, home: &Path, now: DateTime<Utc>) -> Option<(LocalAccount, super::Ledger)> {
    let sources = Sources::from_env(home);
    let dir = crate::paths::data_dir();
    let local = LocalAccount::read(provider, &sources, &dir, now)?;
    if !local.matches(identity) {
        return None;
    }
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
    fn added_and_default_accounts_use_identity_not_slot_and_switches_move_the_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let codex = dir.path().join("custom-codex");
        std::fs::create_dir_all(&codex).unwrap();
        let path = codex.join("auth.json");
        let write = |user: &str, account: &str| std::fs::write(&path, json!({"tokens": {"access_token": token(user, account), "account_id": account}}).to_string()).unwrap();
        write("user-a", "account-a");
        let mut sources = Sources::new(dir.path());
        sources.codex_home = Some(codex);
        let now = Utc::now();
        let local = LocalAccount::read(Provider::Codex, &sources, dir.path(), now).unwrap();
        assert!(local.matches(AccountIdentity::codex_token(&token("user-a", "account-a"), "account-a").as_ref()));
        assert!(!local.matches(None));
        assert!(!local.matches(AccountIdentity::codex_token(&token("user-b", "account-b"), "account-b").as_ref()));
        assert!(!local.covers(now - Duration::days(7), now), "nothing before the login file is attributed");
        let window = UsageWindow::new("five", WindowKind::FiveHour, 0.5, 5 * 3600)
            .with_reset(Some(now + Duration::hours(6)));
        assert!(local.covers_window(&window, now + Duration::hours(2)));
        // A switch is caught mid-scan, and the next read starts the new login no earlier than
        // the last time the previous one was seen.
        let later = now + Duration::hours(3);
        write("user-b", "account-b");
        assert!(!local.unchanged());
        let switched = LocalAccount::read(Provider::Codex, &sources, dir.path(), later).unwrap();
        assert!(!switched.matches(Some(&local.identity)));
        assert!(switched.since >= now);
        let record = std::fs::read_to_string(dir.path().join(RECORD)).unwrap();
        assert!(!record.contains("user-") && !record.contains("account-"), "only a digest is recorded");
    }

    #[test]
    fn metadata_rewrites_and_renewals_keep_the_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("custom-claude");
        std::fs::create_dir_all(&config).unwrap();
        let profile = |starts: u32| json!({"oauthAccount": {"accountUuid": "u", "organizationUuid": "o"}, "numStartups": starts}).to_string();
        std::fs::write(config.join(".claude.json"), profile(1)).unwrap();
        let mut sources = Sources::new(dir.path());
        sources.claude_config_dir = Some(config.clone());
        let now = Utc::now();
        assert!(LocalAccount::read(Provider::ClaudeCode, &sources, dir.path(), now).is_none(), "a login file is required");
        let credentials = config.join(".credentials.json");
        std::fs::write(&credentials, b"fixture").unwrap();
        let first = LocalAccount::read(Provider::ClaudeCode, &sources, dir.path(), now).unwrap();
        assert!(first.matches(AccountIdentity::claude_profile(&json!({"account": {"uuid": "u"}, "organization": {"uuid": "o"}})).as_ref()));
        // Claude Code rewrites its metadata every session, and token renewals rewrite the login.
        std::fs::write(config.join(".claude.json"), profile(2)).unwrap();
        std::fs::write(&credentials, b"renewed").unwrap();
        assert!(first.unchanged());
        let again = LocalAccount::read(Provider::ClaudeCode, &sources, dir.path(), now + Duration::hours(1)).unwrap();
        assert_eq!(again.since, first.since);
    }

    #[test]
    fn matched_accounts_use_their_own_percentage_and_incomplete_or_pre_login_windows_are_withheld() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("login-fixture.json");
        std::fs::write(&path, json!({"tokens": {"access_token": token("u", "o"), "account_id": "o"}}).to_string()).unwrap();
        let now = Utc::now();
        let from = now - Duration::hours(2);
        let identity = AccountIdentity::new("u", "o").unwrap();
        let local = LocalAccount { provider: Provider::Codex, identity, identity_path: path.clone(), since: from - Duration::hours(1) };
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
        // A login that began inside the window cannot take its earlier history.
        let late = LocalAccount { since: from + Duration::minutes(1), ..local.clone() };
        assert!(late.estimate(&window(0.1), &ledger, Some(now), now).is_none());
        // Nor can a switch during the scan.
        std::fs::write(&path, json!({"tokens": {"access_token": token("other", "o"), "account_id": "o"}}).to_string()).unwrap();
        assert!(local.estimate(&window(0.1), &ledger, Some(now), now).is_none());
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
