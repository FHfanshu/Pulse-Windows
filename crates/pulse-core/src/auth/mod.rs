// Ported from upstream Sources/Pulse/Auth/ (OAuthLogin, LoopbackCallback, AccountCredentials,
// GitHubDeviceLogin, CursorWebLogin) and Docs/providers/authentication.md.
//! Signing Pulse in to an account of its own, so it can watch more than one subscription.
//!
//! Pulse cannot register an OAuth application with these providers, so it drives the public
//! client the provider's own CLI or editor plugin ships. The consent page names that product, not
//! Pulse, and this is not an official integration. The logins obtained here are Pulse's: they are
//! kept in the secret store (DPAPI `keys.dat`) under `login:<account id>` and renewed from Pulse's
//! own refresh token. Nothing here reads or writes what a CLI stored.
//!
//! Everything that touches the network is split from the parsing and request building next to it,
//! so the unit tests drive canned replies and never open a socket to a provider.

pub mod credentials;
pub mod cursor_web;
pub mod github;
pub mod loopback;
pub mod oauth;
pub mod util;

use crate::model::AccountKey;
use crate::service::FetchContext;

pub use credentials::{AccountCredentials, SecretLogins};
pub use oauth::{DevicePrompt, OAuthLogin};

/// Why a sign-in or a renewal did not produce a login (upstream `OAuthLogin.Failure`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// This provider has no login Pulse can drive.
    Unsupported,
    /// The one port the provider's client accepts is already in use, usually by the CLI's own
    /// sign-in running at the same moment.
    PortBusy(u16),
    Cancelled,
    /// The browser never came back: the sign-in was abandoned or ended on the provider's own
    /// error page.
    TimedOut,
    Refused(String),
    UnreadableReply,
}

impl Failure {
    /// The text for Settings. An upstream English string (the UI passes it through `t()`), except
    /// `Refused`, which carries the provider's own words.
    pub fn message(&self) -> String {
        match self {
            Failure::Unsupported => "This provider can't be signed in to from Pulse.".into(),
            Failure::PortBusy(_) => "Finish or close the sign-in already running, then try again.".into(),
            Failure::Cancelled => "Sign-in was cancelled.".into(),
            Failure::TimedOut => "The browser didn't come back. If it showed an error, try again.".into(),
            Failure::Refused(why) => why.clone(),
            Failure::UnreadableReply => "Couldn't read the reply.".into(),
        }
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for Failure {}

/// The login stored for an added account, as a fetch finds it.
#[derive(Debug, Clone, PartialEq)]
pub enum StoredLogin {
    /// Nothing stored: the caller may fall back to a raw saved key, then report `signedOut`.
    Missing,
    /// A login whose access token is good for at least another minute (renewed if need be).
    Usable(AccountCredentials),
    /// A login is stored but could not be renewed. Reported as `signedOut`, not as a network
    /// error: the remedy is the same and the reader can act on it.
    Expired,
}

/// The login an added account fetches with, renewed first when it is within a minute of expiring
/// (upstream `LiveUsageServices.fetchAdded`).
///
/// A renewal is written only over the login it renews (`SecretLogins::renewed`): not over a
/// longer-lived one, not over another account's login in the same slot, and never into an empty
/// slot, so a renewal still out when the account is removed cannot bring its tokens back.
pub async fn usable_login(ctx: &FetchContext, account: &AccountKey) -> StoredLogin {
    let logins = SecretLogins::new(ctx.secrets.as_ref());
    let Some(stored) = logins.get(account) else { return StoredLogin::Missing };
    if stored.is_fresh(ctx.now) {
        return StoredLogin::Usable(stored);
    }
    match OAuthLogin::refresh(&ctx.http, &stored, account.provider, ctx.now).await {
        Ok(renewed) => {
            logins.renewed(account, &renewed);
            StoredLogin::Usable(renewed)
        }
        Err(_) => StoredLogin::Expired,
    }
}
