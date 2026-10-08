// Ported from upstream Tests/PulseTests/RecapWindowTests.swift (the periods, the price and the
// notice rule; the window's own state is the UI's).

use std::collections::HashMap;

use chrono::{DateTime, NaiveDate, TimeZone, Utc};

use super::periods::{self, notice, price};
use super::Period;
use crate::spend::ledger::Origin;
use crate::spend::{Calendar, Ledger, SpendAgent};

fn d(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

fn month(year: i32, month: u32) -> Period {
    Period::Month { year, month }
}

// Which periods.

#[test]
fn months_run_from_the_earliest_record_to_now_newest_first() {
    let months = periods::months(Some(d(2026, 7, 20)), d(2026, 10, 5));
    assert_eq!(months, vec![month(2026, 10), month(2026, 9), month(2026, 8), month(2026, 7)]);
}

#[test]
fn months_cross_a_year_boundary_without_skipping() {
    let months = periods::months(Some(d(2025, 11, 3)), d(2026, 2, 9));
    assert_eq!(months, vec![month(2026, 2), month(2026, 1), month(2025, 12), month(2025, 11)]);
}

#[test]
fn with_no_record_known_only_this_month_is_offered_and_a_future_record_offers_nothing_later() {
    let now = d(2026, 10, 5);
    assert_eq!(periods::months(None, now), vec![month(2026, 10)]);
    assert_eq!(periods::months(Some(d(2027, 3, 1)), now), vec![month(2026, 10)]);
    assert_eq!(periods::years(None, now), vec![Period::Year(2026)]);
}

#[test]
fn years_run_from_the_earliest_records_year_to_this_one() {
    let years = periods::years(Some(d(2024, 12, 31)), d(2026, 10, 5));
    assert_eq!(years, vec![Period::Year(2026), Period::Year(2025), Period::Year(2024)]);
}

// Which one it opens on.

#[test]
fn the_first_seven_days_open_on_the_month_that_just_ended_after_that_the_running_one() {
    let open = |day| periods::default_month(Some(d(2026, 1, 10)), d(2026, 10, day));
    assert_eq!(open(1), month(2026, 9));
    assert_eq!(open(7), month(2026, 9));
    assert_eq!(open(8), month(2026, 10));
    assert_eq!(open(31), month(2026, 10));
}

#[test]
fn in_january_the_month_that_just_ended_is_december_of_the_year_before() {
    assert_eq!(periods::default_month(Some(d(2025, 3, 1)), d(2026, 1, 4)), month(2025, 12));
}

#[test]
fn the_default_never_falls_before_the_earliest_record() {
    // Records begin on October 1; asked on the 3rd, September is not offered.
    assert_eq!(periods::default_month(Some(d(2026, 10, 1)), d(2026, 10, 3)), month(2026, 10));
    // And with nothing known it is simply the rule.
    assert_eq!(periods::default_month(None, d(2026, 10, 3)), month(2026, 9));
    assert_eq!(periods::default_month(None, d(2026, 10, 9)), month(2026, 10));
}

#[test]
fn the_year_opens_on_the_one_that_just_ended_during_the_first_week_of_january_only() {
    let open = |m, day| periods::default_year(Some(d(2024, 5, 1)), d(2026, m, day));
    assert_eq!(open(1, 3), Period::Year(2025));
    assert_eq!(open(1, 7), Period::Year(2025));
    assert_eq!(open(1, 8), Period::Year(2026));
    assert_eq!(open(10, 3), Period::Year(2026));
}

#[test]
fn switching_kind_keeps_the_year_and_lands_on_the_last_offered_month_of_it() {
    let (now, earliest) = (d(2026, 10, 5), Some(d(2025, 3, 1)));
    assert_eq!(periods::switched(month(2025, 6), earliest, now), Period::Year(2025));
    assert_eq!(periods::switched(Period::Year(2025), earliest, now), month(2025, 12));
    // The running year ends on the running month.
    assert_eq!(periods::switched(Period::Year(2026), earliest, now), month(2026, 10));
}

#[test]
fn a_periods_key_round_trips() {
    assert_eq!(month(2026, 9).key(), "2026-09");
    assert_eq!(Period::from_key("2026-09"), Some(month(2026, 9)));
    assert_eq!(Period::Year(2026).key(), "2026");
    assert_eq!(Period::from_key("2026"), Some(Period::Year(2026)));
    assert_eq!(Period::from_key("soon"), None);
}

#[test]
fn the_offer_names_every_period_as_a_key() {
    let offer = periods::Offer::of(Some(d(2025, 11, 3)), d(2026, 1, 4));
    assert_eq!(offer.months, vec!["2026-01", "2025-12", "2025-11"]);
    assert_eq!(offer.years, vec!["2026", "2025"]);
    assert_eq!(offer.default_month, "2025-12");
    assert_eq!(offer.default_year, "2025");
}

// The price.

#[test]
fn nothing_typed_is_no_price_and_so_is_zero() {
    for text in ["", "   ", "0", "0.00"] {
        assert_eq!(price::entry(text), price::Entry::None, "{text:?}");
    }
}

#[test]
fn an_amount_is_read_the_way_it_was_typed_with_or_without_the_dollar_sign() {
    use price::Entry::Amount;
    assert_eq!(price::entry("200"), Amount(200.0));
    assert_eq!(price::entry("$20"), Amount(20.0));
    assert_eq!(price::entry("$ 20"), Amount(20.0));
    assert_eq!(price::entry(" 17.5 "), Amount(17.5));
    assert_eq!(price::entry("17.50"), Amount(17.5));
    // A comma and one or two digits is a decimal comma, in any language.
    assert_eq!(price::entry("12,5"), Amount(12.5));
    assert_eq!(price::entry("12,50"), Amount(12.5));
    // A comma and three digits is a thousands separator, with or without cents.
    assert_eq!(price::entry("1,200"), Amount(1200.0));
    assert_eq!(price::entry("1,200.50"), Amount(1200.5));
    assert_eq!(price::entry("$1,200"), Amount(1200.0));
}

#[test]
fn words_exponents_units_signs_odd_grouping_and_a_third_decimal_are_refused() {
    for text in [
        "abc", "free", "$", "1e3", "20 USD", "-5", "+5", ">10000", "1.2.3", "1,2,3", "12.345", "12,345,67", "1,20,000", ",5", ".5", "5.",
        "1 000", "٣٤", "10001", "$$5",
    ] {
        assert_eq!(price::entry(text), price::Entry::Refused, "{text} should be refused");
    }
    assert_eq!(price::entry("10000"), price::Entry::Amount(10_000.0));
}

#[test]
fn the_kept_price_is_none_unless_it_is_a_positive_amount_within_range() {
    assert_eq!(price::normalized(None), None);
    assert_eq!(price::normalized(Some(0.0)), None);
    assert_eq!(price::normalized(Some(-1.0)), None);
    assert_eq!(price::normalized(Some(f64::INFINITY)), None);
    assert_eq!(price::normalized(Some(f64::NAN)), None);
    assert_eq!(price::normalized(Some(20_000.0)), None);
    assert_eq!(price::normalized(Some(20.0)), Some(20.0));
}

// Where records begin.

#[test]
fn statistics_are_not_records_of_this_machine_and_a_bogus_date_cannot_offer_hundreds_of_months() {
    let calendar = Calendar::utc(2);
    let ledger = |origin, earliest: DateTime<Utc>| Ledger { origin, earliest: Some(earliest), ..Ledger::empty() };
    let local = ledger(Origin::LocalTranscripts, Utc.with_ymd_and_hms(2026, 3, 4, 12, 0, 0).unwrap());
    let statistics = ledger(Origin::ProviderStatistics, Utc.with_ymd_and_hms(2023, 1, 1, 12, 0, 0).unwrap());
    let both: HashMap<_, _> = [(SpendAgent::ClaudeCode, local), (SpendAgent::Codex, statistics.clone())].into_iter().collect();
    assert_eq!(periods::earliest(&both, &calendar), Some(d(2026, 3, 4)));
    let only_statistics: HashMap<_, _> = [(SpendAgent::Codex, statistics)].into_iter().collect();
    assert_eq!(periods::earliest(&only_statistics, &calendar), None);

    let bogus: HashMap<_, _> =
        [(SpendAgent::ClaudeCode, ledger(Origin::LocalTranscripts, Utc.timestamp_opt(0, 0).unwrap()))].into_iter().collect();
    let found = periods::earliest(&bogus, &calendar);
    assert_eq!(found, Some(periods::floor()));
    assert_eq!(periods::months(found, d(2026, 10, 5)).len(), 6 * 12 + 10);
}

// The notification.

const SEPTEMBER: Period = Period::Month { year: 2026, month: 9 };

#[test]
fn on_the_1st_with_records_last_month_and_nothing_announced_september_is_announced() {
    assert_eq!(notice::due(d(2026, 10, 1), None, |_| Some(1_000)), Some(SEPTEMBER));
}

#[test]
fn once_per_month_the_one_already_announced_is_not_announced_again() {
    for day in 1..=3 {
        assert_eq!(notice::due(d(2026, 10, day), Some(SEPTEMBER), |_| Some(1_000)), None, "day {day}");
    }
    // The next month is news again.
    assert_eq!(notice::due(d(2026, 11, 2), Some(SEPTEMBER), |_| Some(1_000)), Some(month(2026, 10)));
}

#[test]
fn only_for_a_month_that_had_records() {
    let now = d(2026, 10, 1);
    assert_eq!(notice::due(now, None, |_| Some(0)), None);
    // Nothing read yet is not "no records": nothing is said either way.
    assert_eq!(notice::due(now, None, |_| None), None);
}

#[test]
fn only_in_the_first_days_of_a_month_and_only_about_the_month_before() {
    let due = |m, day| notice::due(d(2026, m, day), None, |_| Some(1));
    assert_eq!(due(10, 3), Some(SEPTEMBER));
    assert_eq!(due(10, 4), None);
    assert_eq!(due(10, 15), None);
    assert_eq!(due(1, 2), Some(month(2025, 12)));
}

#[test]
fn the_tokens_are_asked_about_the_month_announced_and_only_inside_the_window() {
    let asked = std::cell::RefCell::new(Vec::new());
    let _ = notice::due(d(2026, 10, 2), None, |p| {
        asked.borrow_mut().push(p);
        Some(1)
    });
    assert_eq!(*asked.borrow(), vec![SEPTEMBER]);
    asked.borrow_mut().clear();
    let _ = notice::due(d(2026, 10, 20), None, |p| {
        asked.borrow_mut().push(p);
        Some(1)
    });
    assert!(asked.borrow().is_empty(), "outside the window nothing is even counted");
}

#[test]
fn a_notifications_identifier_names_its_month_so_a_click_can_open_it() {
    let identifier = notice::identifier(SEPTEMBER);
    assert_eq!(identifier, "recap-ready-2026-09");
    assert_eq!(notice::period_from_identifier(&identifier), Some(SEPTEMBER));
    assert_eq!(notice::period_from_identifier("acct|window|spent"), None);
    assert_eq!(notice::period_from_identifier("recap-ready-never"), None);
}
