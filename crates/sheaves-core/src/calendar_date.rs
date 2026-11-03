//! Days as Harvest records them: a date with no time zone.

use std::fmt;
use std::str::FromStr;

use jiff::civil::Date;
use jiff::tz::TimeZone;
use jiff::{Timestamp, ToSpan};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A calendar day with no time zone, as Harvest's `spent_date` fields represent it.
///
/// Harvest returns `"2026-08-29"` with no zone. Turning that into an instant means
/// every entry is one day off for somebody, so days stay days here and only become
/// instants at the edges, against an explicit time zone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, uniffi::Record)]
pub struct CalendarDate {
    pub year: i16,
    pub month: i8,
    pub day: i8,
}

impl CalendarDate {
    /// Panics on a day that does not exist; parse text with [`CalendarDate::from_str`].
    pub fn new(year: i16, month: i8, day: i8) -> Self {
        Self::from_civil(Date::new(year, month, day).expect("a real calendar day"))
    }

    fn from_civil(date: Date) -> Self {
        Self {
            year: date.year(),
            month: date.month(),
            day: date.day(),
        }
    }

    fn civil(self) -> Date {
        Date::new(self.year, self.month, self.day).expect("a CalendarDate is always a real day")
    }

    /// The day `instant` falls on in `zone`.
    pub fn of(instant: Timestamp, zone: &TimeZone) -> Self {
        Self::from_civil(instant.to_zoned(zone.clone()).date())
    }

    /// The first instant of this day in `zone`.
    pub fn start_of_day(self, zone: &TimeZone) -> Timestamp {
        self.civil()
            .to_zoned(zone.clone())
            .expect("every day has a start")
            .timestamp()
    }

    pub fn adding_days(self, days: i32) -> Self {
        Self::from_civil(
            self.civil()
                .checked_add(days.days())
                .expect("within jiff's range"),
        )
    }

    /// Whole days from `other` to this date. Negative when `other` is later.
    pub fn days_since(self, other: CalendarDate) -> i32 {
        other
            .civil()
            .until(self.civil())
            .map(|span| span.get_days())
            .unwrap_or(0)
    }

    /// The first day of this date's week, honouring the account's `week_start_day`.
    pub fn start_of_week(self, week_start: Weekday) -> Self {
        let today = self.civil().weekday().to_sunday_zero_offset();
        let first = week_start.sunday_zero_offset();
        let back = (today - first).rem_euclid(7);
        self.adding_days(-(back as i32))
    }
}

/// The wire format Harvest parses, deliberately not locale-aware: ASCII digits and
/// hyphens whatever the reader's region says. Anything shown to a person goes
/// through the shell's own date formatting instead.
impl fmt::Display for CalendarDate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct NotADate;

impl FromStr for CalendarDate {
    type Err = NotADate;

    fn from_str(text: &str) -> Result<Self, NotADate> {
        let parts: Vec<&str> = text.split('-').collect();
        let [year, month, day] = parts.as_slice() else {
            return Err(NotADate);
        };
        let (Ok(year), Ok(month), Ok(day)) = (year.parse(), month.parse(), day.parse()) else {
            return Err(NotADate);
        };
        Date::new(year, month, day)
            .map(Self::from_civil)
            .map_err(|_| NotADate)
    }
}

impl Serialize for CalendarDate {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for CalendarDate {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse()
            .map_err(|_| serde::de::Error::custom(format!("Not an ISO 8601 date: {text}")))
    }
}

/// The account's `week_start_day`. Harvest only ever returns these three.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, uniffi::Enum)]
pub enum Weekday {
    Saturday,
    Sunday,
    Monday,
}

impl Weekday {
    fn sunday_zero_offset(self) -> i8 {
        match self {
            Weekday::Sunday => 0,
            Weekday::Monday => 1,
            Weekday::Saturday => 6,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let date: CalendarDate = "2026-08-29".parse().unwrap();
        assert_eq!((date.year, date.month, date.day), (2026, 8, 29));
        assert_eq!(date.to_string(), "2026-08-29");
    }

    #[test]
    fn padding() {
        assert_eq!(CalendarDate::new(2026, 1, 5).to_string(), "2026-01-05");
    }

    #[test]
    fn rejects_nonsense() {
        assert!("not-a-date".parse::<CalendarDate>().is_err());
        assert!("2026-13-01".parse::<CalendarDate>().is_err());
        assert!("2026-08".parse::<CalendarDate>().is_err());
    }

    #[test]
    fn ordering() {
        let earlier = CalendarDate::new(2026, 8, 29);
        let later = CalendarDate::new(2026, 9, 1);
        assert!(earlier < later);
        assert!(later >= earlier);
    }

    #[test]
    fn adding_days() {
        let end = CalendarDate::new(2026, 8, 31);
        assert_eq!(end.adding_days(1), CalendarDate::new(2026, 9, 1));
        assert_eq!(end.adding_days(-31), CalendarDate::new(2026, 7, 31));
    }

    #[test]
    fn leap_day() {
        assert_eq!(
            CalendarDate::new(2028, 2, 28).adding_days(1),
            CalendarDate::new(2028, 2, 29)
        );
    }

    /// The whole point of the type: a day must not drift when the machine is east or
    /// west of UTC, which is exactly what storing `spent_date` as an instant causes.
    #[test]
    fn stable_across_time_zones() {
        for name in ["Pacific/Kiritimati", "UTC", "Pacific/Midway"] {
            let zone = TimeZone::get(name).unwrap();
            let day = CalendarDate::new(2026, 8, 29);
            assert_eq!(
                CalendarDate::of(day.start_of_day(&zone), &zone),
                day,
                "{name}"
            );
        }
    }

    #[test]
    fn start_of_week() {
        // 2026-08-29 is a Saturday.
        let saturday = CalendarDate::new(2026, 8, 29);
        assert_eq!(
            saturday.start_of_week(Weekday::Monday),
            CalendarDate::new(2026, 8, 24)
        );
        assert_eq!(
            saturday.start_of_week(Weekday::Sunday),
            CalendarDate::new(2026, 8, 23)
        );
        assert_eq!(saturday.start_of_week(Weekday::Saturday), saturday);
    }

    /// From the target ranking suite, which needs it.
    #[test]
    fn days_since() {
        let date = CalendarDate::new(2026, 8, 29);
        assert_eq!(date.days_since(CalendarDate::new(2026, 8, 22)), 7);
        assert_eq!(date.days_since(date), 0);
        assert_eq!(CalendarDate::new(2026, 8, 22).days_since(date), -7);
        // Across a month boundary and a leap day.
        assert_eq!(
            CalendarDate::new(2028, 3, 1).days_since(CalendarDate::new(2028, 2, 28)),
            2
        );
    }
}
