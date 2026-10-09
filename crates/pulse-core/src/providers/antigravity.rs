// Ported from upstream Providers/AntigravityUsageService.swift. (Stub: being ported.)
//! Antigravity.

use async_trait::async_trait;

use crate::model::{AccountKey, ProviderUsage, Unavailability};
use crate::provider::Provider;
use crate::service::{FetchContext, UsageService};

pub struct Antigravity;

#[async_trait]
impl UsageService for Antigravity {
    fn provider(&self) -> Provider {
        Provider::Antigravity
    }

    async fn fetch(&self, _ctx: &FetchContext, account: &AccountKey) -> ProviderUsage {
        ProviderUsage::unavailable(account.clone(), Unavailability::NotConnected)
    }
}
