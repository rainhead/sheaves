//! How durations and days read on GNOME. The core hands out numbers and
//! `CalendarDate`s; this turns them into text, through the C locale that GTK sets
//! up from the environment at start.
//!
//! The Mac formats through Foundation's format styles. The C library has no
//! equivalent for durations, so `1:30` keeps ASCII digits in every locale; the
//! decimal form does take the locale's separator, `1,50` in French and German.

use std::ffi::CStr;

use sheaves_core::budget::BudgetFigure;
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

/// A budget figure: a duration, or money to the whole unit with the client's
/// currency, or a bare number when the currency is unknown.
pub fn figure(figure: &BudgetFigure, format: HoursFormat) -> String {
    match figure {
        BudgetFigure::Hours(value) => hours(*value, format),
        BudgetFigure::Money { amount, currency } => money(*amount, currency.as_deref()),
    }
}

/// `amount` to the whole unit, grouped as the locale groups digits, with its
/// currency's symbol or else its code on the side the locale puts it:
/// "$1,007" in the US, "1.007 €" in Germany.
fn money(amount: f64, currency: Option<&str>) -> String {
    let amount = if amount.is_finite() { amount } else { 0.0 };
    let number = whole(amount);
    let Some(code) = currency else {
        return number;
    };
    let (symbol_first, spaced) = currency_placement();
    match symbol(code) {
        Some(symbol) => place(symbol, &number, symbol_first, spaced),
        // A code reads as a word, so it always gets a space.
        None => place(code, &number, symbol_first, true),
    }
}

/// `symbol` before or after `number`, with a no-break space between them or none.
fn place(symbol: &str, number: &str, symbol_first: bool, spaced: bool) -> String {
    let space = if spaced { "\u{a0}" } else { "" };
    if symbol_first {
        format!("{symbol}{space}{number}")
    } else {
        format!("{number}{space}{symbol}")
    }
}

/// Whether the locale writes the currency symbol before a positive amount, and
/// whether a space separates them. The C locale leaves both unspecified, and an
/// unspecified side reads as before with no space, as in English.
fn currency_placement() -> (bool, bool) {
    // SAFETY: localeconv returns a pointer to static data that stays valid until
    // the locale changes, which happens only at startup; its fields are read at
    // once and not kept.
    let conventions = unsafe { &*libc::localeconv() };
    // CHAR_MAX means unspecified.
    if conventions.p_cs_precedes as u8 == u8::MAX >> 1 {
        return (true, false);
    }
    (
        conventions.p_cs_precedes != 0,
        conventions.p_sep_by_space == 1,
    )
}

fn symbol(code: &str) -> Option<&'static str> {
    Some(match code {
        "USD" => "$",
        "EUR" => "€",
        "GBP" => "£",
        "JPY" => "¥",
        "CNY" => "CN¥",
        "INR" => "₹",
        "KRW" => "₩",
        "CAD" => "CA$",
        "AUD" => "A$",
        "NZD" => "NZ$",
        _ => return None,
    })
}

/// `value` rounded to a whole number, with the locale's digit grouping.
fn whole(value: f64) -> String {
    let mut buffer = [0u8; 64];
    // SAFETY: the format takes one double, the buffer is large enough for any
    // value this is given, and snprintf always terminates what it writes.
    let written = unsafe {
        libc::snprintf(
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            c"%'.0f".as_ptr(),
            value,
        )
    };
    if written < 0 {
        return format!("{value:.0}");
    }
    CStr::from_bytes_until_nul(&buffer)
        .map(|text| text.to_string_lossy().into_owned())
        .unwrap_or_else(|_| format!("{value:.0}"))
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
    // The month abbreviated, as the Mac has it: in the window's header the full
    // name made the day too long to sit between its arrows.
    date.and_then(|date| date.format("%A %e %b"))
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

    #[test]
    fn the_symbol_goes_where_the_locale_puts_it() {
        assert_eq!(place("$", "1,007", true, false), "$1,007");
        assert_eq!(place("€", "1.007", false, true), "1.007\u{a0}€");
    }

    /// Tests run in the C locale, which does not group digits.
    #[test]
    fn money_has_no_cents() {
        let money = |amount, code: Option<&str>| money(amount, code);
        assert_eq!(money(2400.4, Some("USD")), "$2400");
        assert_eq!(money(1007.0, Some("EUR")), "€1007");
        assert_eq!(money(90.0, Some("CHF")), "CHF\u{a0}90");
        assert_eq!(money(90.0, None), "90");
    }

    /// Tests run in the C locale, which writes a full stop.
    #[test]
    fn decimal_has_two_places() {
        assert_eq!(hours(1.5, HoursFormat::Decimal), "1.50");
        assert_eq!(decimal_separator(), '.');
    }
}
