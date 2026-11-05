//! Formats that must survive a round trip: a `CalendarDate` through its wire text,
//! durations as the shells display them back through `parse_hours`, and the queue
//! and the cache through their files.

use std::sync::Arc;

use jiff::Timestamp;
use jiff::tz::{Offset, TimeZone};
use proptest::prelude::*;
use uuid::Uuid;

use sheaves_core::calendar_date::CalendarDate;
use sheaves_core::clock::SystemClock;
use sheaves_core::hours::parse_hours;
use sheaves_core::models::Reference;
use sheaves_core::queue::{StoppedRun, Mutation, MutationQueue};
use sheaves_core::snapshot::{CachedSnapshot, SnapshotStore};
use sheaves_core::tracked_entry::{EntryId, TimerTarget, TrackedEntry};

fn config() -> ProptestConfig {
    ProptestConfig {
        cases: 1000,
        failure_persistence: Some(Box::new(proptest::test_runner::FileFailurePersistence::WithSource("regressions"))),
        ..ProptestConfig::default()
    }
}

// MARK: Generators

fn date() -> impl Strategy<Value = CalendarDate> {
    (1i16..=9999, 1i8..=12, 1i8..=31).prop_filter_map("a real day", |(year, month, day)| {
        jiff::civil::Date::new(year, month, day).ok().map(|_| CalendarDate::new(year, month, day))
    })
}

fn instant() -> impl Strategy<Value = Timestamp> {
    // 1970 to 2100, to the nanosecond.
    (0i64..4_102_444_800, 0i32..1_000_000_000)
        .prop_map(|(second, nanosecond)| Timestamp::new(second, nanosecond).unwrap())
}

/// Hours as the core computes them: from elapsed time, so any finite non-negative
/// value, not only two-decimal ones.
fn hours() -> impl Strategy<Value = f64> {
    prop_oneof![
        (0u32..100_000).prop_map(|cents| f64::from(cents) / 100.0),
        0.0..1000.0f64,
    ]
}

fn text() -> impl Strategy<Value = String> {
    // Anything a person might type into notes, including what JSON escapes.
    prop_oneof![".{0,40}", "[\"\\\\\n\t\u{0}\u{1F600}\u{202E}]{0,8}"]
}

fn reference() -> impl Strategy<Value = Reference> {
    (any::<i64>(), text()).prop_map(|(id, name)| Reference::new(id, name))
}

fn target() -> impl Strategy<Value = TimerTarget> {
    (reference(), reference(), proptest::option::of(reference()))
        .prop_map(|(project, task, client)| TimerTarget { project, task, client })
}

fn entry_id() -> impl Strategy<Value = EntryId> {
    prop_oneof![
        any::<i64>().prop_map(EntryId::Server),
        any::<u128>().prop_map(|bits| EntryId::Local(Uuid::from_u128(bits))),
    ]
}

fn mutation() -> impl Strategy<Value = Mutation> {
    prop_oneof![
        (any::<u128>(), target(), date(), proptest::option::of(text()), instant(), proptest::option::of(instant()))
            .prop_map(|(local, target, spent_date, notes, started_at, ended_at)| Mutation::Create {
                local: Uuid::from_u128(local),
                target,
                spent_date,
                notes,
                started_at,
                ended_at,
            }),
        (entry_id(), hours(), proptest::option::of((instant(), hours(), instant())))
            .prop_map(|(id, hours, run)| Mutation::Stop {
                id,
                hours,
                run: run.map(|(started_at, banked_hours, stopped_at)| StoppedRun {
                    started_at,
                    banked_hours,
                    stopped_at,
                }),
            }),
        (entry_id(), instant(), hours())
            .prop_map(|(id, resumed_at, banked_hours)| Mutation::Restart { id, resumed_at, banked_hours }),
        (entry_id(), proptest::option::of(text()), proptest::option::of(hours()), proptest::option::of(target()))
            .prop_map(|(id, notes, hours, target)| Mutation::Update { id, notes, hours, target }),
        (entry_id(), hours(), instant()).prop_map(|(id, hours, as_of)| Mutation::Adjust { id, hours, as_of }),
        entry_id().prop_map(|id| Mutation::Delete { id }),
    ]
}

fn tracked_entry() -> impl Strategy<Value = TrackedEntry> {
    (
        (entry_id(), target(), proptest::option::of(text()), date(), hours()),
        (any::<bool>(), proptest::option::of(instant()), any::<bool>(), any::<bool>(), any::<bool>(), instant()),
    )
        .prop_map(
            |(
                (id, target, notes, spent_date, banked_hours),
                (is_running, timer_started_at, is_locked, is_billable, is_pending, updated_at),
            )| TrackedEntry {
                id,
                project: target.project,
                task: target.task,
                client: target.client,
                notes,
                spent_date,
                banked_hours,
                is_running,
                timer_started_at,
                is_locked,
                is_billable,
                is_pending,
                updated_at,
            },
        )
}

/// Digits in one of the scripts a locale formats with.
const ZEROS: [u32; 7] = [0x0030, 0x0660, 0x06F0, 0x0966, 0x09E6, 0x0E50, 0xFF10];

fn in_script(ascii: &str, zero: u32) -> String {
    ascii
        .chars()
        .map(|c| match c.to_digit(10) {
            Some(digit) => char::from_u32(zero + digit).unwrap(),
            None => c,
        })
        .collect()
}

/// The value's debug text, with every decimal number cut to twelve significant
/// digits. The files are read with serde_json's default float parser, which can
/// land one unit in the last place away from what was written (its
/// `float_roundtrip` feature is off). Hours are rounded to two decimals before
/// anything is sent, so that is written up rather than held against the files
/// here. Debug text, not JSON, so a field the files drop still shows.
fn close<T: std::fmt::Debug>(value: &T) -> String {
    let text = format!("{value:?}");
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    let mut previous = ' ';
    while let Some(c) = chars.next() {
        if c.is_ascii_digit() && !previous.is_alphanumeric() && previous != '_' {
            let mut number = String::from(c);
            while let Some(&next) = chars.peek() {
                if next.is_ascii_digit() || next == '.' || next == 'e' || (next == '-' && number.ends_with('e')) {
                    number.push(next);
                    chars.next();
                } else {
                    break;
                }
            }
            match (number.contains('.'), number.parse::<f64>()) {
                (true, Ok(float)) => out.push_str(&format!("{float:.11e}")),
                _ => out.push_str(&number),
            }
            previous = number.chars().last().unwrap();
        } else {
            out.push(c);
            previous = c;
        }
    }
    out
}

// MARK: Properties

proptest! {
    #![proptest_config(config())]

    #[test]
    fn a_date_reads_back_from_its_text(date in date()) {
        let text = date.to_string();
        prop_assert_eq!(text.len(), 10, "ISO text is ten ASCII characters: {}", text);
        prop_assert_eq!(text.parse::<CalendarDate>(), Ok(date));
        let json = serde_json::to_string(&date).unwrap();
        prop_assert_eq!(serde_json::from_str::<CalendarDate>(&json).unwrap(), date);
    }

    #[test]
    fn the_day_of_an_instant_holds_it(instant in instant(), offset_hours in -12i8..=14) {
        let zone = TimeZone::fixed(Offset::constant(offset_hours));
        let day = CalendarDate::of(instant, &zone);
        let start = day.start_of_day(&zone);
        let next = day.adding_days(1).start_of_day(&zone);
        prop_assert!(start <= instant && instant < next, "{} is not on {}", instant, day);
        prop_assert_eq!(day.to_string().parse::<CalendarDate>(), Ok(day));
    }

    /// What a shell displays in hours and minutes reads back as that value.
    #[test]
    fn hours_and_minutes_read_back(whole in 0u32..10_000, minutes in 0u32..60, zero in prop::sample::select(ZEROS.to_vec())) {
        let text = in_script(&format!("{whole}:{minutes:02}"), zero);
        let parsed = parse_hours(&text, '.');
        let expected = f64::from(whole) + f64::from(minutes) / 60.0;
        prop_assert!(parsed.is_some_and(|hours| (hours - expected).abs() < 1e-9), "{:?} read as {:?}", text, parsed);
    }

    /// What a shell displays as decimal hours, with the locale's separator, reads
    /// back as that value.
    #[test]
    fn decimal_hours_read_back(
        cents in 0u32..1_000_000,
        separator in prop::sample::select(vec!['.', ',', '\u{066B}']),
        zero in prop::sample::select(ZEROS.to_vec()),
    ) {
        let ascii = format!("{}.{:02}", cents / 100, cents % 100);
        let text = in_script(&ascii.replace('.', &separator.to_string()), zero);
        let parsed = parse_hours(&text, separator);
        let expected = f64::from(cents) / 100.0;
        prop_assert!(parsed.is_some_and(|hours| (hours - expected).abs() < 1e-9), "{:?} read as {:?}", text, parsed);
    }

    /// Whatever is typed, a duration read from it is a number of hours Harvest could
    /// hold, or nothing.
    #[test]
    fn typed_hours_are_finite_or_refused(text in prop_oneof!["[0-9.,:٠-٩ ]{0,400}", "[0-9]{0,400}", "[0-9]{0,400}\\.[0-9]{0,400}"]) {
        if let Some(hours) = parse_hours(&text, '.') {
            prop_assert!(hours.is_finite() && hours >= 0.0, "{:?} read as {}", text, hours);
        }
    }

    #[test]
    fn the_queue_reads_back_from_its_file(mutations in prop::collection::vec(mutation(), 0..8)) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("queue.json");
        let queue = MutationQueue::open(&path, Arc::new(SystemClock));
        for mutation in &mutations {
            queue.enqueue(mutation.clone());
        }
        drop(queue);
        let reopened = MutationQueue::open(&path, Arc::new(SystemClock));
        prop_assert_eq!(close(&reopened.pending()), close(&mutations));
    }

    #[test]
    fn the_cache_reads_back_from_its_file(
        entries in prop::collection::vec(tracked_entry(), 0..6),
        targets in prop::collection::vec(target(), 0..4),
        saved_at in proptest::option::of(instant()),
    ) {
        let directory = tempfile::tempdir().unwrap();
        let store = SnapshotStore::new(directory.path().join("cache.json"));
        let snapshot = CachedSnapshot { entries, targets, saved_at, ..CachedSnapshot::default() };
        store.save(&snapshot).unwrap();
        prop_assert_eq!(close(&store.load()), close(&Some(snapshot)));
    }
}
