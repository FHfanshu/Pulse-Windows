// Ported from upstream Auth/AccountCredentials.swift (`accounts.dat`, AES-GCM) — here one JSON
// entry per account in the DPAPI secret store.
//! The login Pulse holds for one account it signed in to itself.
//!
//! This is the point where Pulse stops only borrowing credentials: an added account has no CLI
//! behind it, so Pulse keeps the tokens and renews them. Stored as JSON under
//! `login:<account id>`; the account id's own key (`ctx.api_key`) stays the place a pasted raw
//! token lives, which the providers still fall back to.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::model::AccountKey;
use crate::secrets::SecretStore;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountCredentials {
    pub access_token: String,
    pub refresh_token: String,
    /// When the access token stops being accepted. Renewal happens before this, not after a
    /// request has already been refused.
    pub expires_at: DateTime<Utc>,
    /// What the provider called the account when it was added; seeds the label so two
    /// subscriptions are not both called "Codex".
    #[serde(default)]
    pub account_name: Option<String>,
    /// Codex's usage endpoint wants the account named in a header of its own, and the token is
    /// the only place it appears.
    #[serde(default)]
    pub account_id: Option<String>,
}

impl AccountCredentials {
    /// A minute's headroom: a token that expires while the request is in flight comes back
    /// refused, and the retry costs more than renewing early.
    pub fn is_fresh(&self, now: DateTime<Utc>) -> bool {
        self.expires_at - now > Duration::seconds(60)
    }
}

/// The stored key for an account's login.
pub fn secret_key(account: &AccountKey) -> String {
    format!("login:{}", account.id())
}

/// Reads and writes logins in a [`SecretStore`].
pub struct SecretLogins<'a> {
    store: &'a dyn SecretStore,
}

impl<'a> SecretLogins<'a> {
    pub fn new(store: &'a dyn SecretStore) -> Self {
        Self { store }
    }

    pub fn get(&self, account: &AccountKey) -> Option<AccountCredentials> {
        serde_json::from_str(&self.store.get(&secret_key(account))?).ok()
    }

    /// Stores a login, or forgets one when `None` is passed. Sign-out is a decision, not a race,
    /// so this is unconditional.
    pub fn set(&self, account: &AccountKey, credentials: Option<&AccountCredentials>) -> bool {
        match credentials {
            Some(credentials) => {
                let Ok(json) = serde_json::to_string(credentials) else { return false };
                self.store.set(&secret_key(account), Some(&json));
                self.store.get(&secret_key(account)).as_deref() == Some(json.as_str())
            }
            None => {
                self.store.set(&secret_key(account), None);
                true
            }
        }
    }

    /// Stores a renewal, unless what is already there outlives it.
    ///
    /// **A renewal is not an ordinary write.** Two passes can be renewing the same account at
    /// once, and the abandoned one answers last; written plainly, that puts the older login back
    /// over the newer, and a provider that rotates refresh tokens refuses it. The write is a
    /// compare-and-replace against the exact text that was read, so it also **never brings a
    /// login back**: removing an account forgets its login, and a renewal still out at that
    /// moment finds an empty slot and writes nothing.
    pub fn renewed(&self, account: &AccountKey, credentials: &AccountCredentials) -> bool {
        let key = secret_key(account);
        let Some(held) = self.store.get(&key) else { return false };
        let Ok(existing) = serde_json::from_str::<AccountCredentials>(&held) else { return false };
        if !accepts_renewal(credentials, Some(&existing)) {
            return false;
        }
        let Ok(json) = serde_json::to_string(credentials) else { return false };
        self.store.replace(&key, &held, &json)
    }
}

/// The rule a renewal is written by, apart from the store so it can be tested without one.
pub fn accepts_renewal(credentials: &AccountCredentials, existing: Option<&AccountCredentials>) -> bool {
    let Some(existing) = existing else { return false };
    if existing.expires_at >= credentials.expires_at {
        return false;
    }
    if let (Some(held), Some(renewing)) = (&existing.account_id, &credentials.account_id) {
        if held != renewing {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::Provider;
    use crate::secrets::MemorySecrets;

    fn login(minutes: i64, id: Option<&str>) -> AccountCredentials {
        AccountCredentials {
            access_token: format!("access-{minutes}"),
            refresh_token: "refresh".into(),
            expires_at: Utc::now() + Duration::minutes(minutes),
            account_name: Some("me@example.com".into()),
            account_id: id.map(String::from),
        }
    }

    fn account() -> AccountKey {
        AccountKey { provider: Provider::Codex, slot: "abc".into() }
    }

    #[test]
    fn freshness_leaves_a_minute_of_headroom() {
        let now = Utc::now();
        assert!(login(5, None).is_fresh(now));
        let mut near = login(0, None);
        near.expires_at = now + Duration::seconds(59);
        assert!(!near.is_fresh(now));
    }

    #[test]
    fn round_trips_under_its_own_key_and_forgets() {
        let secrets = MemorySecrets::default();
        let logins = SecretLogins::new(&secrets);
        let held = login(10, Some("acct"));
        assert!(logins.set(&account(), Some(&held)));
        assert_eq!(logins.get(&account()), Some(held));
        // Stored beside, not over, the raw key of the same account.
        assert!(secrets.get("codex#abc").is_none());
        assert!(secrets.get("login:codex#abc").is_some());
        assert!(logins.set(&account(), None));
        assert!(logins.get(&account()).is_none());
    }

    #[test]
    fn json_uses_camel_case_and_tolerates_missing_optionals() {
        let text = r#"{"accessToken":"a","refreshToken":"r","expiresAt":"2030-01-01T00:00:00Z"}"#;
        let parsed: AccountCredentials = serde_json::from_str(text).unwrap();
        assert_eq!(parsed.access_token, "a");
        assert!(parsed.account_name.is_none() && parsed.account_id.is_none());
    }

    #[test]
    fn renewal_only_replaces_a_shorter_lived_login() {
        let secrets = MemorySecrets::default();
        let logins = SecretLogins::new(&secrets);
        logins.set(&account(), Some(&login(10, Some("acct"))));

        // Older than what is held: refused.
        assert!(!logins.renewed(&account(), &login(5, Some("acct"))));
        // Another account's login in the same slot: refused.
        assert!(!logins.renewed(&account(), &login(60, Some("other"))));
        // Longer-lived and the same account: accepted.
        let newer = login(60, Some("acct"));
        assert!(logins.renewed(&account(), &newer));
        assert_eq!(logins.get(&account()), Some(newer));
    }

    #[test]
    fn a_renewal_never_fills_an_empty_slot() {
        let secrets = MemorySecrets::default();
        let logins = SecretLogins::new(&secrets);
        assert!(!logins.renewed(&account(), &login(60, None)));
        assert!(logins.get(&account()).is_none());
        // Forgotten while a renewal was out.
        logins.set(&account(), Some(&login(10, None)));
        logins.set(&account(), None);
        assert!(!logins.renewed(&account(), &login(60, None)));
        assert!(logins.get(&account()).is_none());
    }
}
