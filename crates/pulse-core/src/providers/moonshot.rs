// Ported from upstream Providers/Profiled/MoonshotUsageService.swift.
//! Moonshot (Kimi Open Platform): a prepaid balance read with the pasted API key.
//! The key may belong to the international or the China platform; whichever
//! accepted it last is tried first.
//!
//! TEMPLATE: this is the reference shape for every profiled provider port.

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use serde::Deserialize;

use super::profile::{self, Fail};
use crate::model::{AccountKey, ProviderUsage, Unavailability};
use crate::provider::Provider;
use crate::service::{FetchContext, UsageService};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Region {
    International,
    China,
}

impl Region {
    const ALL: [Region; 2] = [Region::International, Region::China];

    fn endpoint(self) -> &'static str {
        match self {
            Region::International => "https://api.moonshot.ai/v1/users/me/balance",
            Region::China => "https://api.moonshot.cn/v1/users/me/balance",
        }
    }

    fn currency(self) -> &'static str {
        match self {
            Region::International => "USD",
            Region::China => "CNY",
        }
    }
}

#[derive(Default)]
pub struct Moonshot {
    /// Which region accepted a key last time, keyed by the key.
    accepted: Mutex<HashMap<String, Region>>,
}

#[async_trait]
impl UsageService for Moonshot {
    fn provider(&self) -> Provider {
        Provider::Moonshot
    }

    async fn fetch(&self, ctx: &FetchContext, account: &AccountKey) -> ProviderUsage {
        let Some(key) = ctx.api_key(account) else {
            return ProviderUsage::unavailable(account.clone(), Unavailability::ApiKeyMissing);
        };
        let key = key.trim().to_string();
        let known = self.accepted.lock().unwrap().get(&key).copied();
        let order: Vec<Region> = match known {
            Some(first) => std::iter::once(first).chain(Region::ALL.into_iter().filter(|r| *r != first)).collect(),
            None => Region::ALL.to_vec(),
        };

        for region in order {
            match profile::get_bearer(ctx, region.endpoint(), &key).await {
                Ok(body) => {
                    self.accepted.lock().unwrap().insert(key.clone(), region);
                    return reading(&body, region, ctx, account)
                        .unwrap_or_else(|reason| ProviderUsage::unavailable(account.clone(), reason));
                }
                // Refused by one platform: it may be the other's key.
                Err(Unavailability::ApiKeyRefused) => continue,
                // Anything else is an answer about the service, not the key.
                Err(reason) => return ProviderUsage::unavailable(account.clone(), reason),
            }
        }
        ProviderUsage::unavailable(account.clone(), Unavailability::ApiKeyRefused)
    }
}

#[derive(Deserialize)]
struct Reply {
    code: Option<i64>,
    status: Option<bool>,
    data: Option<Balance>,
}

#[derive(Deserialize)]
struct Balance {
    available_balance: Option<f64>,
}

fn reading(body: &[u8], region: Region, ctx: &FetchContext, account: &AccountKey) -> Result<ProviderUsage, Fail> {
    let reply: Reply = profile::decode(body)?;
    let (Some(code), Some(status)) = (reply.code, reply.status) else {
        return Err(Unavailability::UnreadableReply);
    };
    if code != 0 || !status {
        return Err(Unavailability::ServerError);
    }
    let available = reply
        .data
        .and_then(|d| d.available_balance)
        .filter(|v| v.is_finite())
        .ok_or(Unavailability::UnreadableReply)?;
    // Kept as reported, below zero included.
    Ok(profile::balance_reading(account, available, region.currency(), ctx))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::profile::test_support::context;

    fn account() -> AccountKey {
        AccountKey::primary(Provider::Moonshot)
    }

    #[test]
    fn reads_the_available_balance_in_the_region_currency() {
        let ctx = context();
        let body = br#"{"code":0,"status":true,"data":{"available_balance":49.58,"voucher_balance":0,"cash_balance":49.58}}"#;
        let usage = reading(body, Region::China, &ctx, &account()).unwrap();
        let credit = usage.credit_remaining.unwrap();
        assert_eq!(credit.amount, 49.58);
        assert_eq!(credit.currency, "CNY");
        assert!(usage.windows.is_empty());
    }

    #[test]
    fn a_failed_code_is_a_server_error_and_garbage_is_unreadable() {
        let ctx = context();
        assert_eq!(reading(br#"{"code":1,"status":false}"#, Region::International, &ctx, &account()).unwrap_err(), Unavailability::ServerError);
        assert_eq!(reading(b"<html>", Region::International, &ctx, &account()).unwrap_err(), Unavailability::UnreadableReply);
    }
}
