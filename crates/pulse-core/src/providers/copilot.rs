// Ported from upstream Providers/CopilotUsageService.swift. (Stub: being ported.)
//! Copilot.

use async_trait::async_trait;

use crate::model::{AccountKey, ProviderUsage, Unavailability};
use crate::provider::Provider;
use crate::service::{FetchContext, UsageService};

pub struct Copilot;

#[async_trait]
impl UsageService for Copilot {
    fn provider(&self) -> Provider {
        Provider::Copilot
    }

    async fn fetch(&self, _ctx: &FetchContext, account: &AccountKey) -> ProviderUsage {
        ProviderUsage::unavailable(account.clone(), Unavailability::NotConnected)
    }
}
