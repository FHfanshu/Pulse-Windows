//! Fixed sample readings for building the UI before providers are wired.
//! Mirrors the readings in upstream's panel screenshot (Docs/panel.webp).

use chrono::{Duration, Utc};

use crate::model::*;
use crate::provider::Provider;

pub fn sample_usages() -> Vec<ProviderUsage> {
    let now = Utc::now();
    let key = AccountKey::primary;
    let month_end = now + Duration::days(9);

    vec![
        ProviderUsage::live(
            key(Provider::ClaudeCode),
            vec![
                UsageWindow::new("five_hour", WindowKind::FiveHour, 0.06, 18_000)
                    .with_reset(Some(now + Duration::minutes(132))),
                UsageWindow::new("seven_day", WindowKind::Weekly, 0.03, 604_800)
                    .with_reset(Some(now + Duration::days(4))),
            ],
            now,
        )
        .with_plan(Some("Max".into())),
        ProviderUsage::live(
            key(Provider::Codex),
            vec![
                UsageWindow::new("primary", WindowKind::FiveHour, 0.89, 18_000)
                    .with_reset(Some(now + Duration::minutes(47))),
                UsageWindow::new("secondary", WindowKind::Weekly, 0.41, 604_800)
                    .with_reset(Some(now + Duration::days(2))),
            ],
            now,
        )
        .with_plan(Some("Pro".into())),
        ProviderUsage::live(
            key(Provider::Cursor),
            vec![
                {
                    let mut w = UsageWindow::new("cursor", WindowKind::Monthly, 0.02, 2_592_000)
                        .with_scope("Cursor Models")
                        .with_reset(Some(month_end));
                    w.reports_length = false;
                    w
                },
                {
                    let mut w = UsageWindow::new("other", WindowKind::Monthly, 0.0, 2_592_000)
                        .with_scope("Other Models")
                        .with_reset(Some(month_end));
                    w.reports_length = false;
                    w
                },
            ],
            now,
        ),
        ProviderUsage::live(
            key(Provider::Copilot),
            vec![UsageWindow::new("premium", WindowKind::Monthly, 0.22, 2_592_000)
                .with_reset(Some(month_end))],
            now,
        ),
        ProviderUsage::live(
            key(Provider::Gemini),
            vec![UsageWindow::new("daily", WindowKind::Daily, 0.15, 86_400)
                .with_reset(Some(now + Duration::hours(7)))],
            now,
        ),
        ProviderUsage::unavailable(key(Provider::DeepSeek), Unavailability::ApiKeyMissing),
    ]
}
