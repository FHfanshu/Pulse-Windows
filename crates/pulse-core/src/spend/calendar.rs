//! A small calendar: local midnights, day arithmetic and week starts, over any
//! chrono time zone, so ledgers cut "today" the way the reader's clock does and
//! tests can pin a fixed zone.

use std::sync::Arc;

use chrono::{DateTime, Datelike, Duration, LocalResult, Months, NaiveDate, NaiveDateTime, TimeZone, Timelike, Utc};

/// What a calendar needs of a zone: its offset at an instant, and the instant(s) a
/// wall-clock time names.
pub trait Zone: Send + Sync {
    fn offset_seconds(&self, utc: &NaiveDateTime) -> i32;
    fn resolve(&self, local: &NaiveDateTime) -> LocalResult<DateTime<Utc>>;
}

impl<T> Zone for T
where
    T: TimeZone + Send + Sync,
    T::Offset: Send + Sync,
{
    fn offset_seconds(&self, utc: &NaiveDateTime) -> i32 {
        use chrono::Offset;
        self.offset_from_utc_datetime(utc).fix().local_minus_utc()
    }

    fn resolve(&self, local: &NaiveDateTime) -> LocalResult<DateTime<Utc>> {
        self.from_local_datetime(local).map(|d| d.with_timezone(&Utc))
    }
}

/// Weekday numbering follows Foundation: 1 is Sunday, 2 is Monday ... 7 is Saturday.
#[derive(Clone)]
pub struct Calendar {
    zone: Arc<dyn Zone>,
    /// 1 = Sunday ... 7 = Saturday.
    pub first_weekday: u32,
}

impl std::fmt::Debug for Calendar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Calendar").field("first_weekday", &self.first_weekday).finish()
    }
}

impl Calendar {
    /// The machine's local zone, weeks starting on Monday.
    pub fn local() -> Self {
        Self::with_zone(chrono::Local, 2)
    }

    pub fn utc(first_weekday: u32) -> Self {
        Self::with_zone(Utc, first_weekday)
    }

    pub fn with_zone<Z>(zone: Z, first_weekday: u32) -> Self
    where
        Z: TimeZone + Send + Sync + 'static,
        Z::Offset: Send + Sync,
    {
        Self { zone: Arc::new(zone), first_weekday: first_weekday.clamp(1, 7) }
    }

    /// The wall-clock time at an instant.
    pub fn to_local(&self, at: DateTime<Utc>) -> NaiveDateTime {
        let naive = at.naive_utc();
        naive + Duration::seconds(i64::from(self.zone.offset_seconds(&naive)))
    }

    /// The instant a wall-clock time names. A repeated time takes its first occurrence; a
    /// skipped one (a spring-forward gap) moves on to the first time that exists.
    pub fn from_local(&self, local: NaiveDateTime) -> DateTime<Utc> {
        let mut probe = local;
        for _ in 0..4 {
            match self.zone.resolve(&probe) {
                LocalResult::Single(at) => return at,
                LocalResult::Ambiguous(first, _) => return first,
                LocalResult::None => probe += Duration::minutes(30),
            }
        }
        DateTime::from_naive_utc_and_offset(local, Utc)
    }

    /// What identifies the zone for cutting days: its offsets at instants across the years and
    /// both halves of each. Two zones that agree on all of them cut the same days, so for a
    /// record of which days were cut, they are the same zone.
    pub fn zone_signature(&self) -> String {
        let mut offsets = Vec::new();
        for year in [2000, 2010, 2020, 2025, 2026, 2027] {
            for month in [1, 4, 7, 10] {
                let at = NaiveDate::from_ymd_opt(year, month, 15).and_then(|d| d.and_hms_opt(12, 0, 0)).expect("a date");
                offsets.push(self.zone.offset_seconds(&at).to_string());
            }
        }
        offsets.join(",")
    }

    pub fn start_of_day(&self, at: DateTime<Utc>) -> DateTime<Utc> {
        self.from_local(self.to_local(at).date().and_hms_opt(0, 0, 0).expect("midnight"))
    }

    pub fn date(&self, at: DateTime<Utc>) -> NaiveDate {
        self.to_local(at).date()
    }

    /// Local midnight of a calendar date.
    pub fn midnight(&self, date: NaiveDate) -> DateTime<Utc> {
        self.from_local(date.and_hms_opt(0, 0, 0).expect("midnight"))
    }

    /// `at` moved by whole days, keeping the wall-clock time where it exists.
    pub fn add_days(&self, at: DateTime<Utc>, days: i64) -> DateTime<Utc> {
        self.from_local(self.to_local(at) + Duration::days(days))
    }

    pub fn add_months(&self, at: DateTime<Utc>, months: i32) -> DateTime<Utc> {
        let local = self.to_local(at);
        let moved = if months >= 0 {
            local.checked_add_months(Months::new(months as u32))
        } else {
            local.checked_sub_months(Months::new(months.unsigned_abs()))
        };
        moved.map(|m| self.from_local(m)).unwrap_or(at)
    }

    pub fn add_years(&self, at: DateTime<Utc>, years: i32) -> DateTime<Utc> {
        self.add_months(at, years * 12)
    }

    pub fn hour(&self, at: DateTime<Utc>) -> u32 {
        self.to_local(at).hour()
    }

    pub fn month(&self, at: DateTime<Utc>) -> u32 {
        self.to_local(at).month()
    }

    /// 1 = Sunday ... 7 = Saturday.
    pub fn weekday(&self, at: DateTime<Utc>) -> u32 {
        self.to_local(at).weekday().num_days_from_sunday() + 1
    }

    /// Midnight of the first day of the week containing `at`.
    pub fn week_start(&self, at: DateTime<Utc>) -> DateTime<Utc> {
        let behind = i64::from((self.weekday(at) + 7 - self.first_weekday) % 7);
        self.start_of_day(self.add_days(self.start_of_day(at), -behind))
    }

    /// Midnight of the first of the month containing `at`.
    pub fn month_start(&self, at: DateTime<Utc>) -> DateTime<Utc> {
        let date = self.date(at);
        self.midnight(NaiveDate::from_ymd_opt(date.year(), date.month(), 1).unwrap_or(date))
    }
}
