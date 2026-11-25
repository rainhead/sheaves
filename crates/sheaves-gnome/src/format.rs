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

/// A day as a heading in the locale's words and order: "Thursday, Nov 26" where
/// the month comes first, as in US English, and "Thursday 26 Nov" where the day
/// does.
pub fn day(day: CalendarDate) -> String {
    day_in_order(day, month_comes_first(&locale_date_format()))
}

fn day_in_order(day: CalendarDate, month_first: bool) -> String {
    let date = gtk::glib::DateTime::from_local(
        day.year.into(),
        day.month.into(),
        day.day.into(),
        12,
        0,
        0.0,
    );
    // The month abbreviated, as the Mac has it: in the window's header the full
    // name made the day too long to sit between its arrows.
    date.and_then(|date| date.format(day_pattern(month_first)))
        .map(|text| text.split_whitespace().collect::<Vec<_>>().join(" "))
        .unwrap_or_else(|_| day.to_string())
}

/// The `strftime` pattern for a day heading. GLib has no skeleton that follows
/// the locale's order, so the order is chosen here.
fn day_pattern(month_first: bool) -> &'static str {
    if month_first { "%A, %b %e" } else { "%A %e %b" }
}

/// The locale's short date pattern, such as `%m/%d/%y` or `%d.%m.%Y`.
fn locale_date_format() -> String {
    // SAFETY: nl_langinfo returns a NUL-terminated string owned by the C
    // library, or an empty one for an item it does not know; it is copied at once.
    unsafe {
        let pattern = libc::nl_langinfo(libc::D_FMT);
        if pattern.is_null() {
            String::new()
        } else {
            CStr::from_ptr(pattern).to_string_lossy().into_owned()
        }
    }
}

/// Whether a short date pattern puts the month before the day. A pattern that
/// names neither reads as month first, as the C locale's does.
fn month_comes_first(pattern: &str) -> bool {
    let mut month = None;
    let mut day = None;
    let mut specifiers = pattern.chars();
    while let Some(character) = specifiers.next() {
        if character != '%' {
            continue;
        }
        // Skip glibc's flags, such as `%-d` and `%_m`.
        let Some(letter) = specifiers.find(|c| !matches!(c, '-' | '_' | '0' | '^' | '#')) else {
            break;
        };
        let place = month.is_none() as u8 + day.is_none() as u8;
        match letter {
            'm' | 'b' | 'B' | 'h' => month = month.or(Some(place)),
            'd' | 'e' => day = day.or(Some(place)),
            // `%D` is `%m/%d/%y`.
            'D' => return true,
            _ => {}
        }
        if month.is_some() && day.is_some() {
            break;
        }
    }
    match (month, day) {
        // Whichever was seen first has the larger count of unseen fields left.
        (Some(m), Some(d)) => m > d,
        (Some(_), None) | (None, None) => true,
        (None, Some(_)) => false,
    }
}

/// How long ago, in words: "3 minutes ago". `seconds` is how long since.
pub fn ago(seconds: i64) -> String {
    let seconds = seconds.max(0);
    let (count, unit) = match seconds {
        0..60 => (seconds, "second"),
        60..3600 => (seconds / 60, "minute"),
        3600..86400 => (seconds / 3600, "hour"),
        _ => (seconds / 86400, "day"),
    };
    format!("{count} {unit}{} ago", if count == 1 { "" } else { "s" })
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

    #[test]
    fn month_first_patterns_put_the_month_first() {
        assert!(month_comes_first("%m/%d/%y"));
        assert!(month_comes_first("%D"));
        assert!(month_comes_first("%Y-%m-%d"));
        assert!(!month_comes_first("%d/%m/%Y"));
        assert!(!month_comes_first("%d.%m.%Y"));
        assert!(!month_comes_first("%-d %b %Y"));
        assert!(month_comes_first("%b %e, %Y"));
        assert!(month_comes_first(""));
    }

    #[test]
    fn the_heading_follows_the_order_it_is_given() {
        let thursday = CalendarDate {
            year: 2026,
            month: 11,
            day: 26,
        };
        assert_eq!(day_pattern(true), "%A, %b %e");
        assert_eq!(day_pattern(false), "%A %e %b");
        // Single-digit days drop the pad that %e leaves, in either order.
        let fifth = CalendarDate {
            year: 2026,
            month: 11,
            day: 5,
        };
        assert!(!day_in_order(fifth, true).contains("  "));
        assert!(!day_in_order(fifth, false).contains("  "));
        // The C locale's names, which is what tests run in.
        assert_eq!(day_in_order(thursday, true), "Thursday, Nov 26");
        assert_eq!(day_in_order(thursday, false), "Thursday 26 Nov");
    }

    #[test]
    fn ago_counts_in_the_largest_whole_unit() {
        assert_eq!(ago(45), "45 seconds ago");
        assert_eq!(ago(60), "1 minute ago");
        assert_eq!(ago(179), "2 minutes ago");
        assert_eq!(ago(3600), "1 hour ago");
        assert_eq!(ago(7300), "2 hours ago");
        assert_eq!(ago(86400 * 3), "3 days ago");
        assert_eq!(ago(-5), "0 seconds ago");
    }
}
