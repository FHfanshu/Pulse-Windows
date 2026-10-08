//! The seam every provider plugs into, and what it may use while fetching.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::model::{AccountKey, ProviderUsage, Unavailability};
use crate::provider::Provider;
use crate::secrets::SecretStore;
use crate::settings::AppSettings;

/// Everything a fetch may touch. Providers never read settings or secrets any other way.
#[derive(Clone)]
pub struct FetchContext {
    pub http: reqwest::Client,
    pub secrets: Arc<dyn SecretStore>,
    pub settings: Arc<AppSettings>,
    pub now: DateTime<Utc>,
    /// `%USERPROFILE%` — where CLIs keep their dot-folders.
    pub home: PathBuf,
    /// `%APPDATA%` (Roaming).
    pub app_data: PathBuf,
    /// `%LOCALAPPDATA%`.
    pub local_app_data: PathBuf,
}

impl FetchContext {
    pub fn from_env(settings: Arc<AppSettings>, secrets: Arc<dyn SecretStore>) -> Self {
        let env = |k: &str| std::env::var_os(k).map(PathBuf::from).unwrap_or_default();
        Self {
            http: http_client(&settings),
            secrets,
            settings,
            now: Utc::now(),
            home: env("USERPROFILE"),
            app_data: env("APPDATA"),
            local_app_data: env("LOCALAPPDATA"),
        }
    }

    /// The API key the reader pasted for this account, if any.
    pub fn api_key(&self, account: &AccountKey) -> Option<String> {
        self.secrets.get(&account.id()).filter(|k| !k.trim().is_empty())
    }

    pub fn source(&self, provider: Provider) -> Option<&str> {
        self.settings.sources.get(provider.raw()).map(String::as_str)
    }

    pub fn server_address(&self, provider: Provider) -> Option<&str> {
        self.settings.server_addresses.get(provider.raw()).map(String::as_str)
    }
}

pub fn http_client(settings: &AppSettings) -> reqwest::Client {
    let mut builder = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .connect_timeout(Duration::from_secs(10))
        .user_agent(concat!("Pulse/", env!("CARGO_PKG_VERSION"), " (Windows)"));
    let proxy = &settings.network_proxy;
    if proxy.enabled && !proxy.host.is_empty() {
        let scheme = if proxy.scheme.is_empty() { "http" } else { proxy.scheme.as_str() };
        if let Ok(p) = reqwest::Proxy::all(format!("{scheme}://{}:{}", proxy.host, proxy.port)) {
            builder = builder.proxy(p);
        }
    }
    builder.build().unwrap_or_default()
}

#[async_trait]
pub trait UsageService: Send + Sync {
    fn provider(&self) -> Provider;
    async fn fetch(&self, ctx: &FetchContext, account: &AccountKey) -> ProviderUsage;
}

/// Map an HTTP outcome to the shared unavailability cases (upstream `UsageServices` helpers).
pub fn classify_status(status: reqwest::StatusCode) -> Option<Unavailability> {
    match status.as_u16() {
        200..=299 => None,
        401 | 403 => Some(Unavailability::ApiKeyRefused),
        429 => Some(Unavailability::RateLimited),
        500..=599 => Some(Unavailability::ServerError),
        _ => Some(Unavailability::ServerError),
    }
}

pub fn classify_error(error: &reqwest::Error) -> Unavailability {
    if error.is_decode() {
        Unavailability::UnreadableReply
    } else {
        Unavailability::Unreachable
    }
}

/// All services this build ships, keyed by provider.
pub type Registry = BTreeMap<Provider, Arc<dyn UsageService>>;
