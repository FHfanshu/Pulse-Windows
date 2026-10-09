// Ported from upstream Providers/ZaiUsageService.swift (the "Usage statistics" extension).
//! The account's own token history, from the statistics endpoint the console draws its charts from
//! (`GET {host}/api/monitor/usage/model-usage`).
//!
//! **Better data than a transcript scan, and less of it.** It covers every machine the account is
//! used on rather than this PC alone, but it reports one token total per model per bucket, with no
//! split between input, output and cache, so nothing here can be priced. The ledger it builds says
//! so through `origin`, and the card drops its money column.
//!
//! Measured upstream against `open.bigmodel.cn` on 2026-09-07. The server picks the granularity
//! from the span (hourly up to about a week, daily beyond) and **refuses a span of 90 days** with a
//! 500, so the window asked for has to stay inside what it will answer.

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, NaiveDate, Utc};
use serde::Deserialize;

use super::zai::Storefront;
use crate::history::HistoryRead;
use crate::model::AccountKey;
use crate::service::FetchContext;
use crate::spend::ledger::{Ledger, LedgerDay, Origin};
use crate::spend::Calendar;

pub const HISTORY_DAYS: i64 = 30;

#[derive(Debug, Deserialize)]
pub struct Statistics {
    pub success: Option<bool>,
    pub code: Option<i64>,
    pub data: Option<Payload>,
}

#[derive(Debug, Deserialize, Default)]
pub struct Payload {
    /// Bucket labels, `yyyy-MM-dd HH:mm` when hourly and `yyyy-MM-dd` when daily. The server
    /// chooses; the label's own shape is what says which, so neither is assumed.
    pub x_time: Option<Vec<String>>,
    /// `f64`, not integers: a service that starts reporting `12.5` where it reported `12` would
    /// otherwise fail the whole decode and blank the history, for a figure that is perfectly usable.
    #[serde(rename = "tokensUsage")]
    pub tokens_usage: Option<Vec<f64>>,
    #[serde(rename = "modelDataList")]
    pub model_data_list: Option<Vec<Series>>,
}

#[derive(Debug, Deserialize)]
pub struct Series {
    #[serde(rename = "modelName")]
    pub model_name: Option<String>,
    #[serde(rename = "tokensUsage")]
    pub tokens_usage: Option<Vec<f64>>,
}

/// What a history read found out, which is not the same question as what it found: an empty chart
/// has several causes, and the sentence printed under it is a claim about the reader's account.
/// `Answered` may carry an empty ledger: a real answer about an account with no usage in the window.
pub async fn history(ctx: &FetchContext, account: &AccountKey, shop: Storefront) -> (HistoryRead, Option<Ledger>) {
    let Some(key) = ctx.api_key(account) else { return (HistoryRead::NotConfigured, None) };
    let query = query(Utc::now(), HISTORY_DAYS, &Calendar::local());
    let request = ctx
        .http
        .get(url(shop.host))
        .query(&query)
        .bearer_auth(key.trim())
        .header("Accept", "application/json")
        .timeout(std::time::Duration::from_secs(20));
    let Ok(reply) = request.send().await else { return (HistoryRead::Failed, None) };
    if reply.status().as_u16() != 200 {
        return (HistoryRead::Failed, None);
    }
    let Ok(bytes) = reply.bytes().await else { return (HistoryRead::Failed, None) };
    let Ok(statistics) = serde_json::from_slice::<Statistics>(&bytes) else { return (HistoryRead::Failed, None) };
    let (Some(true), Some(200), Some(payload)) = (statistics.success, statistics.code, statistics.data) else {
        return (HistoryRead::Failed, None);
    };
    // The envelope said success, so this is an answer: no usable rows means an account with
    // nothing spent in the window, an empty ledger rather than a failure to read one.
    (HistoryRead::Answered, Some(ledger(&payload, &Calendar::local()).unwrap_or_else(Ledger::empty)))
}

/// **`host` has no default, deliberately.** Each storefront is its own account with its own key;
/// sending one's bearer token to the other's host is the trap having two providers exists to prevent.
pub fn url(host: &str) -> String {
    format!("{host}/api/monitor/usage/model-usage")
}

/// The span, in the shape the service wants: local wall-clock, no zone, seconds included.
pub fn query(now: DateTime<Utc>, days: i64, calendar: &Calendar) -> Vec<(String, String)> {
    let format = |at: DateTime<Utc>| calendar.to_local(at).format("%Y-%m-%d %H:%M:%S").to_string();
    let start = calendar.start_of_day(calendar.add_days(now, -days));
    vec![("startTime".into(), format(start)), ("endTime".into(), format(now))]
}

/// `2026-08-31 14:00` and `2026-08-31` both land on the same day.
pub fn day(label: &str, calendar: &Calendar) -> Option<DateTime<Utc>> {
    let text: String = label.chars().take(10).collect();
    NaiveDate::parse_from_str(&text, "%Y-%m-%d").ok().map(|date| calendar.midnight(date))
}

/// Buckets into days, because the card is a day-by-day chart and the server may have answered in
/// hours. None when the payload carries no usable rows.
pub fn ledger(payload: &Payload, calendar: &Calendar) -> Option<Ledger> {
    let labels = payload.x_time.as_ref().filter(|l| !l.is_empty())?;
    let totals = payload.tokens_usage.clone().unwrap_or_default();
    let mut by_day: BTreeMap<DateTime<Utc>, i64> = BTreeMap::new();
    let mut models_by_day: HashMap<DateTime<Utc>, HashMap<String, i64>> = HashMap::new();

    for (index, label) in labels.iter().enumerate() {
        let Some(day) = day(label, calendar) else { continue };
        let mut from_models = 0i64;
        for series in payload.model_data_list.iter().flatten() {
            let (Some(name), Some(counts)) = (&series.model_name, &series.tokens_usage) else { continue };
            let Some(count) = counts.get(index).filter(|c| **c > 0.0) else { continue };
            let tokens = count.round() as i64;
            *models_by_day.entry(day).or_default().entry(name.clone()).or_default() += tokens;
            from_models += tokens;
        }
        // **The total series is preferred, and not required.** A reply whose `tokensUsage` is
        // missing or short while `modelDataList` is populated used to throw the whole history away,
        // having already collected the per-model counts that would have answered.
        let total = totals.get(index).map_or(from_models, |t| t.round() as i64);
        if total > 0 {
            *by_day.entry(day).or_default() += total;
        }
    }

    // **Every day between the first and the last, including the empty ones.** The chart draws one
    // equal-width bar per element and no date axis, so a ledger of only the busy days reads as a
    // calendar and isn't one.
    let (first, last) = (*by_day.keys().next()?, *by_day.keys().next_back()?);
    let mut days = Vec::new();
    let mut cursor = first;
    while cursor <= last {
        let tokens = by_day.get(&cursor).copied().unwrap_or(0);
        // No cost, and no pretending: these tokens are counted and cannot be priced from what the
        // service reports.
        days.push(LedgerDay::new(cursor, tokens, 0.0, tokens, models_by_day.remove(&cursor).unwrap_or_default()));
        let next = calendar.add_days(cursor, 1);
        if next <= cursor {
            break;
        }
        cursor = next;
    }

    let mut result = Ledger::empty();
    result.origin = Origin::ProviderStatistics;
    result.earliest = days.first().map(|d| d.date);
    result.days = days;
    // Rate-window spend is a transcript thing: it needs the moment work happened, and a day bucket
    // cannot answer it.
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::profile::test_support::fixture;
    use chrono::TimeZone;

    fn calendar() -> Calendar {
        Calendar::utc(2)
    }

    fn payload(name: &str) -> Payload {
        serde_json::from_slice::<Statistics>(&fixture(name)).unwrap().data.unwrap()
    }

    fn inline(json: &str) -> Payload {
        serde_json::from_str::<Statistics>(json).unwrap().data.unwrap()
    }

    fn day_of(text: &str) -> DateTime<Utc> {
        day(text, &calendar()).unwrap()
    }

    #[test]
    fn hourly_buckets_are_folded_into_days() {
        let ledger = ledger(&payload("glm-model-usage.json"), &calendar()).unwrap();
        assert_eq!(ledger.days.iter().map(|d| d.date).collect::<Vec<_>>(), ["2026-09-05", "2026-09-06", "2026-09-07"].map(day_of));
        assert_eq!(ledger.days.iter().map(|d| d.tokens).collect::<Vec<_>>(), [1600, 5000, 900]);
    }

    #[test]
    fn a_daily_answer_needs_no_folding() {
        let c = calendar();
        assert_eq!(day("2026-09-06", &c), Some(day_of("2026-09-06")));
        assert_eq!(day("2026-09-06 01:00", &c), Some(day_of("2026-09-06")));
        assert_eq!(day("nonsense", &c), None);
    }

    #[test]
    fn each_storefront_is_asked_its_own_host() {
        assert!(url("https://api.z.ai").starts_with("https://api.z.ai/"));
        assert!(url("https://open.bigmodel.cn").starts_with("https://open.bigmodel.cn/"));
        assert!(url(Storefront::ZAI.host).ends_with("/api/monitor/usage/model-usage"));
    }

    #[test]
    fn quiet_days_between_busy_ones_are_kept_so_the_chart_is_a_calendar() {
        let p = inline(r#"{"code":200,"success":true,"data":{"x_time":["2026-09-01","2026-09-05"],"tokensUsage":[100,200],"modelDataList":[]}}"#);
        let ledger = ledger(&p, &calendar()).unwrap();
        assert_eq!(ledger.days.iter().map(|d| d.tokens).collect::<Vec<_>>(), [100, 0, 0, 0, 200]);
    }

    #[test]
    fn a_missing_totals_series_falls_back_to_the_per_model_counts() {
        let p = inline(r#"{"code":200,"success":true,"data":{"x_time":["2026-09-06"],"modelDataList":[{"modelName":"glm-4.6","tokensUsage":[750]}]}}"#);
        let ledger = ledger(&p, &calendar()).unwrap();
        assert_eq!(ledger.days.iter().map(|d| d.tokens).collect::<Vec<_>>(), [750]);
        assert_eq!(ledger.days[0].models.get("glm-4.6"), Some(&750));
    }

    #[test]
    fn a_fractional_count_does_not_blank_the_whole_history() {
        let p = inline(r#"{"code":200,"success":true,"data":{"x_time":["2026-09-06"],"tokensUsage":[12.5],"modelDataList":[]}}"#);
        assert_eq!(ledger(&p, &calendar()).unwrap().days[0].tokens, 13);
    }

    #[test]
    fn per_model_totals_are_kept_summed_across_the_days_hours() {
        let ledger = ledger(&payload("glm-model-usage.json"), &calendar()).unwrap();
        let fifth = ledger.days.iter().find(|d| d.date == day_of("2026-09-05")).unwrap();
        assert_eq!(fifth.models.get("glm-4.6"), Some(&1400));
        assert_eq!(fifth.models.get("glm-4-flash"), Some(&200));
    }

    #[test]
    fn every_token_is_unpriced_and_the_ledger_says_where_it_came_from() {
        let ledger = ledger(&payload("glm-model-usage.json"), &calendar()).unwrap();
        // One token total per model, with no split between input, output and cache: no price list
        // can turn it into money, and the card must not print a zero as though it were a cost.
        assert_eq!(ledger.origin, Origin::ProviderStatistics);
        assert!(ledger.days.iter().all(|d| d.cost == 0.0 && d.unpriced_tokens == d.tokens));
        assert!(ledger.slots.is_empty());
    }

    #[test]
    fn an_account_with_no_usage_yet_is_no_history_not_a_history_of_zeroes() {
        let empty = inline(r#"{"code":200,"success":true,"data":{"x_time":[],"tokensUsage":[],"modelDataList":[]}}"#);
        assert!(ledger(&empty, &calendar()).is_none());
        let all_zero = inline(r#"{"code":200,"success":true,"data":{"x_time":["2026-09-06"],"tokensUsage":[0],"modelDataList":[]}}"#);
        assert!(ledger(&all_zero, &calendar()).is_none());
    }

    #[test]
    fn the_span_is_local_wall_clock_without_a_zone_and_inside_what_the_server_answers() {
        let now = Utc.timestamp_opt(1_788_768_000, 0).unwrap();
        let items: HashMap<String, String> = query(now, HISTORY_DAYS, &calendar()).into_iter().collect();
        let mut names: Vec<&String> = items.keys().collect();
        names.sort();
        assert_eq!(names, ["endTime", "startTime"]);
        // 90 days comes back a 500, so the window has to stay inside what the service will answer.
        assert!(HISTORY_DAYS <= 30);
        // No zone offset and no `T`: the service wants plain local time.
        let start = &items["startTime"];
        assert!(start.contains(' ') && !start.contains('T') && !start.contains('+'));
        assert!(start.ends_with("00:00:00"));
    }

    #[tokio::test]
    async fn no_key_is_not_a_failure_to_reach_the_service() {
        let ctx = crate::providers::profile::test_support::context();
        let (read, ledger) = history(&ctx, &AccountKey::primary(crate::provider::Provider::Zai), Storefront::ZAI).await;
        assert_eq!(read, HistoryRead::NotConfigured);
        assert!(ledger.is_none());
    }
}
