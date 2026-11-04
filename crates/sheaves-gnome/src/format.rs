//! How durations and days read on GNOME. The core hands out numbers and
//! `CalendarDate`s; this turns them into text, through the C locale that GTK sets
//! up from the environment at start.
//!
//! The Mac formats through Foundation's format styles. The C library has no
//! equivalent for durations, so `1:30` keeps ASCII digits in every locale; the
//! decimal form does take the locale's separator, `1,50` in French and German.

use std::ffi::CStr;

use sheaves_core::calendar_date::CalendarDate;
use sheaves_core::hours::HoursFormat;

/// A duration in hours as the account displays it: `1:30` or `1.50`.
pub fn hours(hours: f64, format: HoursFormat) -> String {
    let hours = if hours.is_finite() {
        hours.max(0.0)
    } else {
        0.0
    };
    match format {
        HoursFormat::Decimal => decimal(hours),
        HoursFormat::HoursMinutes => {
            // Round to the minute first, so 0.99999 hours reads 1:00, not 0:60.
            let minutes = (hours * 60.0).round() as u64;
            format!("{}:{:02}", minutes / 60, minutes % 60)
        }
    }
}

/// `value` to two places, with the locale's decimal separator.
fn decimal(value: f64) -> String {
    let mut buffer = [0u8; 64];
    // SAFETY: the format takes one double, the buffer is large enough for any
    // value this is given, and snprintf always terminates what it writes.
    let written = unsafe {
        libc::snprintf(
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            c"%.2f".as_ptr(),
            value,
        )
    };
    if written < 0 {
        return format!("{value:.2}");
    }
    CStr::from_bytes_until_nul(&buffer)
        .map(|text| text.to_string_lossy().into_owned())
        .unwrap_or_else(|_| format!("{value:.2}"))
}

/// The locale's decimal separator, for reading a typed duration back.
pub fn decimal_separator() -> char {
    decimal(1.5).chars().nth(1).unwrap_or('.')
}

/// A day as a heading: "Thursday 29 August", in the locale's words.
pub fn day(day: CalendarDate) -> String {
    let date = gtk::glib::DateTime::from_local(
        day.year.into(),
        day.month.into(),
        day.day.into(),
        12,
        0,
        0.0,
    );
    date.and_then(|date| date.format("%A %e %B"))
        .map(|text| text.split_whitespace().collect::<Vec<_>>().join(" "))
        .unwrap_or_else(|_| day.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hours_and_minutes_round_to_the_minute() {
        assert_eq!(hours(1.5, HoursFormat::HoursMinutes), "1:30");
        assert_eq!(hours(0.99999, HoursFormat::HoursMinutes), "1:00");
        assert_eq!(hours(0.0, HoursFormat::HoursMinutes), "0:00");
        assert_eq!(hours(12.0 + 5.0 / 60.0, HoursFormat::HoursMinutes), "12:05");
    }

    #[test]
    fn nonsense_reads_as_nothing() {
        assert_eq!(hours(-2.0, HoursFormat::HoursMinutes), "0:00");
        assert_eq!(hours(f64::NAN, HoursFormat::HoursMinutes), "0:00");
        assert_eq!(hours(f64::INFINITY, HoursFormat::Decimal), decimal(0.0));
    }

    /// Tests run in the C locale, which writes a full stop.
    #[test]
    fn decimal_has_two_places() {
        assert_eq!(hours(1.5, HoursFormat::Decimal), "1.50");
        assert_eq!(decimal_separator(), '.');
    }
}
