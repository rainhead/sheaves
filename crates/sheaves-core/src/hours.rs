//! Durations as people type them, and the rule for when an edited one is sent.
//!
//! Formatting a duration for display stays in each shell, which has the platform's
//! locale-aware number formatting. Reading one back is here, because the rule that
//! decides whether an edit is sent depends on it and must be the same everywhere.

use serde::{Deserialize, Serialize};

/// How the account prefers to see durations. Harvest reports this as `time_format`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, uniffi::Enum)]
#[serde(rename_all = "snake_case")]
pub enum HoursFormat {
    HoursMinutes,
    Decimal,
}

impl HoursFormat {
    /// The account's format, or hours and minutes when it names none we know.
    pub fn from_time_format(time_format: Option<&str>) -> Self {
        match time_format {
            Some("decimal") => HoursFormat::Decimal,
            _ => HoursFormat::HoursMinutes,
        }
    }
}

/// Reads a duration the way people type them: `1` is an hour, `1:30` and `0:45` are
/// hours and minutes, `1.5` is decimal, written with the locale's
/// `decimal_separator`, so `1,5` where that is a comma. `None` for anything else,
/// including negatives.
///
/// Digits may be in any of the common decimal scripts, because the shells format
/// durations through the locale and an Arabic locale writes `١:٢٤`; what the app
/// displays must read back as the value it displayed.
pub fn parse_hours(text: &str, decimal_separator: char) -> Option<f64> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some((whole, minutes)) = trimmed.split_once(':') {
        // ":45" means three quarters of an hour; "1:5" means 1:05, as on a clock.
        let whole = if whole.is_empty() {
            0
        } else {
            parse_whole(whole)?
        };
        let minutes = parse_whole(minutes)?;
        if minutes >= 60 {
            return None;
        }
        return Some(whole as f64 + minutes as f64 / 60.0);
    }
    let (whole, fraction) = match trimmed.split_once(decimal_separator) {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (trimmed, None),
    };
    let mut ascii = String::new();
    for c in whole.chars() {
        ascii.push(char::from(b'0' + decimal_digit(c)?));
    }
    if let Some(fraction) = fraction {
        ascii.push('.');
        for c in fraction.chars() {
            ascii.push(char::from(b'0' + decimal_digit(c)?));
        }
    }
    if ascii.is_empty() || ascii == "." {
        return None;
    }
    // Enough digits parse as infinity, which JSON cannot hold: queued, it is written
    // as null, and the queue file no longer reads back.
    ascii.parse().ok().filter(|hours: &f64| hours.is_finite())
}

fn parse_whole(text: &str) -> Option<u64> {
    if text.is_empty() {
        return None;
    }
    text.chars().try_fold(0u64, |total, c| {
        total
            .checked_mul(10)?
            .checked_add(u64::from(decimal_digit(c)?))
    })
}

/// The value of a decimal digit in the scripts a locale is likely to format with.
fn decimal_digit(c: char) -> Option<u8> {
    const ZEROS: [u32; 7] = [
        0x0030, // ASCII
        0x0660, // Arabic-Indic
        0x06F0, // Extended Arabic-Indic (Persian, Urdu)
        0x0966, // Devanagari
        0x09E6, // Bengali
        0x0E50, // Thai
        0xFF10, // Fullwidth
    ];
    let code = c as u32;
    ZEROS
        .iter()
        .find(|&&zero| (zero..zero + 10).contains(&code))
        .map(|zero| (code - zero) as u8)
}

/// What a closed duration field should send, if anything.
pub struct HoursEdit;

impl HoursEdit {
    /// The hours to send, or `None` to send nothing.
    ///
    /// `as_opened` is the text the field was showing when it opened, and a field
    /// still showing it has asked for nothing. That matters most while a timer runs:
    /// updating a live entry's hours restarts its measurement from the moment the
    /// change lands, so echoing the opening value back would round the total to the
    /// displayed minute and throw away however long the field sat open, for a user
    /// who only clicked the duration and then clicked elsewhere. A stopped entry must
    /// not be marked pending over a number nobody changed either.
    ///
    /// An edited running entry always sends. Its total is moving, so there is
    /// nothing to compare it against: banked hours exclude the segment still being
    /// measured.
    pub fn commit(
        draft: &str,
        as_opened: &str,
        banked: f64,
        is_running: bool,
        decimal_separator: char,
    ) -> Option<f64> {
        if draft == as_opened {
            return None;
        }
        let hours = parse_hours(draft, decimal_separator)?;
        if !is_running && hours == banked {
            return None;
        }
        Some(hours)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod hours_edit {
        use super::*;

        fn commit(draft: &str, as_opened: &str, banked: f64, is_running: bool) -> Option<f64> {
            HoursEdit::commit(draft, as_opened, banked, is_running, '.')
        }

        /// The regression this exists for. A running entry's field opens showing the
        /// total rounded to the minute; committing that back sets the total *and*
        /// restarts the clock, so a user who clicked the duration and then clicked
        /// the notes would silently lose the seconds since.
        #[test]
        fn untouched_running() {
            assert_eq!(commit("1:30", "1:30", 1.2, true), None);
        }

        #[test]
        fn untouched_stopped() {
            assert_eq!(commit("1:30", "1:30", 1.5, false), None);
        }

        #[test]
        fn edited_running() {
            assert_eq!(commit("2", "1:30", 1.2, true), Some(2.0));
        }

        /// Banked hours exclude the segment still being measured, so they are not
        /// the question. Typing the banked figure onto a running entry is a real
        /// request.
        #[test]
        fn edited_running_to_banked() {
            assert_eq!(commit("1:12", "1:30", 1.2, true), Some(1.2));
        }

        #[test]
        fn edited_stopped() {
            assert_eq!(commit("2:15", "1:30", 1.5, false), Some(2.25));
        }

        /// Retyping a stopped entry's own total in another notation is a change to
        /// the text and no change at all to Harvest.
        #[test]
        fn retyped_stopped() {
            assert_eq!(commit("1.5", "1:30", 1.5, false), None);
        }

        #[test]
        fn unparseable() {
            assert_eq!(commit("soon", "1:30", 1.5, false), None);
        }

        #[test]
        fn locale_decimal() {
            assert_eq!(
                HoursEdit::commit("1,75", "1:30", 1.5, false, ','),
                Some(1.75)
            );
        }
    }

    mod hours_parsing {
        use super::*;

        #[test]
        fn reads_common_forms() {
            assert_eq!(parse_hours("1", '.'), Some(1.0));
            assert_eq!(parse_hours("1:30", '.'), Some(1.5));
            assert_eq!(parse_hours(":45", '.'), Some(0.75));
            assert_eq!(parse_hours("1:5", '.'), Some(1.0 + 5.0 / 60.0));
            assert_eq!(parse_hours("0:00", '.'), Some(0.0));
            assert_eq!(parse_hours(" 2 ", '.'), Some(2.0));
            assert_eq!(parse_hours("1.5", '.'), Some(1.5));
        }

        #[test]
        fn respects_locale() {
            // French and German both write a comma.
            assert_eq!(parse_hours("1,5", ','), Some(1.5));
        }

        #[test]
        fn rejects_junk() {
            for junk in ["", "   ", "abc", "-1", "1:75", "1:-5"] {
                assert_eq!(parse_hours(junk, '.'), None, "{junk:?}");
            }
        }

        /// Found by phase 4's property tests. Pasted onto a running timer, this
        /// was queued as infinite hours, written to the queue file as null, and at
        /// the next launch the file was set aside with every unsent change in it.
        #[test]
        fn refuses_a_number_too_large_to_hold() {
            assert_eq!(parse_hours(&"9".repeat(400), '.'), None);
            assert_eq!(
                parse_hours(&format!("1.{}", "0".repeat(400)), '.'),
                Some(1.0)
            );
        }

        /// The edit field prefills with the formatted duration, so whatever the app
        /// displays must read back as the value it displayed. The formatting half of
        /// the Swift test stays in the shell; these are the strings it produces for
        /// en_US, fr_FR and ar_EG.
        #[test]
        fn round_trips_display() {
            let shown = [
                ("0:00", '.', 0.0),
                ("0:15", '.', 0.25),
                ("1:24", '.', 1.4),
                ("10:30", '.', 10.5),
                ("1.40", '.', 1.4),
                ("1,40", ',', 1.4),
                ("١:٢٤", '٫', 1.4),
                ("١٠:٣٠", '٫', 10.5),
                ("١٫٤٠", '٫', 1.4),
            ];
            for (text, separator, hours) in shown {
                assert_eq!(parse_hours(text, separator), Some(hours), "{text}");
            }
        }
    }
}
