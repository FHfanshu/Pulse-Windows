// Ported from upstream Providers/KiroUsageService.swift. (Stub: being ported.)
//! Kiro.

use async_trait::async_trait;

use crate::model::{AccountKey, ProviderUsage, Unavailability};
use crate::provider::Provider;
use crate::service::{FetchContext, UsageService};

pub struct Kiro;

#[async_trait]
impl UsageService for Kiro {
    fn provider(&self) -> Provider {
        Provider::Kiro
    }

    async fn fetch(&self, _ctx: &FetchContext, account: &AccountKey) -> ProviderUsage {
        ProviderUsage::unavailable(account.clone(), Unavailability::NotConnected)
    }
}
