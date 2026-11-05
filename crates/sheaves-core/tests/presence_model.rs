//! Model-based tests of the presence monitor against a simulated day at the desk.
//!
//! The property: polling the machine every half minute, the monitor reports the
//! stretches with no evidence of anyone here that a plain sorted list of all the
//! evidence shows, to within what polling can see. It never reports a stretch that
//! held evidence, such as a call, and it reports every stretch long enough to count.
//!
//! The evidence is what the machine could know: keyboard and mouse input, and the
//! microphone in use while the screen is unlocked. Someone reading at the desk
//! leaves none, so to the machine they are away.

use jiff::{SignedDuration, Timestamp};
use proptest::prelude::*;

use sheaves_core::absence::{Absence, PresenceMonitor, PresenceReading};

const THRESHOLD: f64 = 15.0 * 60.0;
const POLL: f64 = PresenceMonitor::POLL_INTERVAL;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    /// Input every `every` seconds.
    Typing { every: u32 },
    /// Nothing the machine can see: reading, or away.
    Quiet,
    /// The microphone is in use, with no input.
    Call,
    /// The screen is locked; a call may still hold the microphone.
    Locked { call: bool },
    /// The machine sleeps, so nothing polls it.
    Asleep,
}

#[derive(Clone, Copy, Debug)]
struct Segment {
    kind: Kind,
    seconds: u32,
}

fn segment() -> impl Strategy<Value = Segment> {
    let kind = prop_oneof![
        3 => (5..90u32).prop_map(|every| Kind::Typing { every }),
        3 => Just(Kind::Quiet),
        2 => Just(Kind::Call),
        1 => any::<bool>().prop_map(|call| Kind::Locked { call }),
        1 => Just(Kind::Asleep),
    ];
    // Mostly short, sometimes past the threshold.
    let seconds = prop_oneof![3 => 10..600u32, 2 => 600..3600u32];
    (kind, seconds).prop_map(|(kind, seconds)| Segment { kind, seconds })
}

/// The day laid out: when each kind of evidence was there.
struct Timeline {
    inputs: Vec<f64>,
    /// Unlocked microphone use, as closed intervals.
    calls: Vec<(f64, f64)>,
    /// Polls, with the reading each took.
    polls: Vec<(f64, PresenceReading)>,
}

fn lay_out(segments: &[Segment]) -> Timeline {
    let mut inputs = vec![0.0];
    let mut calls = Vec::new();
    let mut spans = Vec::new();
    let mut at = 0.0;
    let mut previous: Option<Kind> = None;
    // The launch counts as presence, and as a look to date a call to, so the day
    // may open with anything: a locked screen, a call, a machine asleep (#43).
    // A closing stretch of typing, so every absence has ended by the last poll.
    let closing = Segment { kind: Kind::Typing { every: 10 }, seconds: 120 };
    for segment in segments.iter().chain(std::iter::once(&closing)) {
        let end = at + f64::from(segment.seconds);
        // Unlocking a screen or waking a machine takes a keypress.
        let still_locked = matches!(previous, Some(Kind::Locked { .. })) && matches!(segment.kind, Kind::Locked { .. });
        if matches!(previous, Some(Kind::Locked { .. } | Kind::Asleep)) && segment.kind != Kind::Asleep && !still_locked {
            inputs.push(at);
        }
        match segment.kind {
            Kind::Locked { .. } if still_locked => {}
            Kind::Typing { every } => {
                let mut t = at;
                while t < end {
                    inputs.push(t);
                    t += f64::from(every);
                }
            }
            Kind::Call => calls.push((at, end)),
            // Locking the screen takes a keypress too.
            Kind::Locked { .. } => {
                inputs.push(at);
            }
            _ => {}
        }
        spans.push((at, end, segment.kind));
        previous = Some(segment.kind);
        at = end;
    }
    inputs.sort_by(f64::total_cmp);
    inputs.dedup();

    let kind_at = |t: f64| {
        spans
            .iter()
            .find(|(start, end, _)| *start <= t && t < *end)
            .map(|(_, _, kind)| *kind)
    };
    let reading = |t: f64, kind: Kind, waking: bool| {
        let last_input = inputs.iter().copied().filter(|input| *input <= t).fold(0.0, f64::max);
        PresenceReading {
            seconds_since_input: t - last_input,
            // The app polls the instant the machine wakes, before any call has
            // had time to start.
            microphone_in_use: !waking && matches!(kind, Kind::Call | Kind::Locked { call: true }),
            screen_locked: matches!(kind, Kind::Locked { .. }),
        }
    };
    let mut polls = Vec::new();
    for window in spans.windows(2) {
        let ((_, _, before), (start, _, kind)) = (window[0], window[1]);
        if before == Kind::Asleep && kind != Kind::Asleep {
            polls.push((start, reading(start, kind, true)));
        }
    }
    let mut t = POLL;
    while t <= at {
        if let Some(kind) = kind_at(t)
            && kind != Kind::Asleep
            && !polls.iter().any(|(woke, _)| *woke == t)
        {
            polls.push((t, reading(t, kind, false)));
        }
        t += POLL;
    }
    polls.sort_by(|a, b| a.0.total_cmp(&b.0));
    Timeline { inputs, calls, polls }
}

/// The stretches with no evidence at least `threshold` long, from a sorted list of
/// all of it.
fn gaps(timeline: &Timeline, threshold: f64) -> Vec<(f64, f64)> {
    let mut evidence: Vec<(f64, f64)> = timeline.inputs.iter().map(|t| (*t, *t)).collect();
    evidence.push((0.0, 0.0));
    evidence.extend(timeline.calls.iter().copied());
    evidence.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut gaps = Vec::new();
    let mut covered_to = evidence[0].1;
    for (start, end) in evidence.into_iter().skip(1) {
        if start - covered_to >= threshold {
            gaps.push((covered_to, start));
        }
        covered_to = covered_to.max(end);
    }
    gaps
}

fn origin() -> Timestamp {
    "2026-08-31T08:00:00Z".parse().unwrap()
}

fn seconds_of(t: Timestamp) -> f64 {
    t.duration_since(origin()).as_secs_f64()
}

fn run(segments: Vec<Segment>) -> Result<(), TestCaseError> {
    let timeline = lay_out(&segments);
    let mut monitor = PresenceMonitor::new(THRESHOLD, origin());
    let mut reported: Vec<(f64, f64)> = Vec::new();
    for (t, reading) in &timeline.polls {
        let now = origin() + SignedDuration::from_secs_f64(*t);
        for Absence { began, ended } in monitor.poll(*reading, now) {
            reported.push((seconds_of(began), seconds_of(ended)));
        }
    }

    // Polling sees the microphone only at a poll, and input only as of the latest
    // event before a poll, so each edge of an absence can be off by one interval.
    let slack = POLL;

    // Input is seen only as the latest event before an unlocked poll. Nothing reads
    // the machine while it is asleep, so input between the last poll and a sleep
    // goes unseen, and an absence that begins then is dated from the input seen
    // before it, which can be most of the threshold early. The test holds the
    // monitor to the input a poll could see, and the calls a poll could hear.
    let looks: Vec<(f64, PresenceReading)> =
        timeline.polls.iter().copied().filter(|(_, reading)| !reading.screen_locked).collect();
    let mut seen: Vec<f64> = looks.iter().map(|(t, reading)| t - reading.seconds_since_input).collect();
    // The first poll of a locked screen reads the idle clock once, and so sees the
    // keypress that locked it (#47). It counts unless it comes a whole threshold
    // after the evidence before it: then the stillness before the lock and the
    // time away after it are one departure, asked about once, from the last input.
    let mut latest = 0.0_f64;
    let mut was_locked = false;
    for (t, reading) in &timeline.polls {
        if reading.screen_locked {
            let press = t - reading.seconds_since_input;
            if !std::mem::replace(&mut was_locked, true) && press - latest < THRESHOLD {
                seen.push(press);
                latest = latest.max(press);
            }
            continue;
        }
        was_locked = false;
        let evidence = if reading.microphone_in_use { *t } else { t - reading.seconds_since_input };
        latest = latest.max(evidence);
    }
    // A call that begins and ends between two polls is never heard. One that is
    // heard counts from the look before the first poll that heard it, since it may
    // have begun just after that look, to the look before the last: each hearing is
    // dated to the look before it. A locked poll is not a look, so a call heard
    // just after an unlock counts from the last look before the lock, within a poll
    // of the keypress that locked the screen. And an absence after a call begins a
    // poll before the call was last heard (`absence_begins_at_end_of_call` holds
    // that on purpose: it trims more than was missed, never less).
    let heard: Vec<((f64, f64), (f64, f64))> = timeline
        .calls
        .iter()
        .filter_map(|(start, end)| {
            let hearings: Vec<usize> = (0..looks.len())
                .filter(|i| looks[*i].0 >= *start && looks[*i].0 <= *end && looks[*i].1.microphone_in_use)
                .collect();
            let before = |i: usize| if i == 0 { 0.0 } else { looks[i - 1].0 };
            Some(((*start, *end), (before(*hearings.first()?), before(*hearings.last()?))))
        })
        .collect();
    let mut inputs: Vec<f64> =
        timeline.inputs.iter().copied().filter(|input| seen.iter().any(|t| (t - input).abs() < 1e-6)).collect();
    inputs.extend(heard.iter().map(|(_, (from, _))| *from));
    let visible = Timeline {
        inputs,
        calls: heard.iter().map(|(call, _)| *call).collect(),
        polls: Vec::new(),
    };

    // Nothing reported holds evidence beyond what polling blurs.
    for (began, ended) in &reported {
        for input in &visible.inputs {
            prop_assert!(
                !(*input > began + 1e-6 && *input < ended - slack),
                "reported absence {:?} holds input at {}",
                (began, ended), input
            );
        }
        // An absence must not reach into a call, or the prompt opens during it,
        // which must never happen.
        for ((start, end), (from, to)) in &heard {
            prop_assert!(
                *ended <= start + 1e-6 || *began >= to - 1e-6,
                "reported absence {:?} reaches into the call {:?}, heard from {} to {}",
                (began, ended), (start, end), from, to
            );
        }
        // And it is long enough to count, by what polling can see.
        prop_assert!(
            gaps(&visible, THRESHOLD - 2.0 * slack)
                .iter()
                .any(|(start, end)| start < ended && end > began),
            "reported absence {:?} is no gap in the evidence",
            (began, ended)
        );
    }

    // Every stretch long enough to count is reported, with its edges where the
    // evidence puts them.
    for (start, end) in gaps(&visible, THRESHOLD + 2.0 * slack) {
        let found = reported
            .iter()
            .find(|(began, ended)| *began < end && *ended > start + slack);
        let Some((began, ended)) = found else {
            return Err(TestCaseError::fail(format!(
                "the gap {:?} was not reported; reported {:?}; calls {:?}",
                (start, end), reported, timeline.calls
            )));
        };
        // After a call, a poll earlier still, as above.
        let after_call = visible.calls.iter().any(|(_, call_end)| (call_end - start).abs() < 1e-6);
        let early = if after_call { 2.0 * slack } else { slack };
        prop_assert!(
            *began >= start - early - 1e-6 && *began <= start + 1e-6,
            "the gap {:?} was reported as beginning at {}",
            (start, end), began
        );
        prop_assert!(
            *ended >= end - slack - 1e-6 && *ended <= end + slack + 1e-6,
            "the gap {:?} was reported as ending at {}",
            (start, end), ended
        );
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 2000,
        failure_persistence: Some(Box::new(proptest::test_runner::FileFailurePersistence::WithSource("regressions"))),
        ..ProptestConfig::default()
    })]

    #[test]
    fn absences_are_the_gaps_in_the_evidence(segments in prop::collection::vec(segment(), 1..12)) {
        run(segments)?;
    }
}

// MARK: Peers

/// The peer merge, against a plain scan of the timeline.
///
/// The property: whatever order peers' intervals arrive in, and however they
/// overlap, the absences left are exactly the stretches of the original absence
/// that no interval touches, kept when they are as long as the threshold. An
/// interval that is a single moment still splits a gap: someone was seen then.
///
/// The model works in whole minutes and looks at every half minute: a gap from `a`
/// to `b` is a run of moments, from `a` plus a half to `b` less a half, that no
/// interval contains, bounded by moments that one does or by the absence's ends.
mod merge {
    use super::*;
    use sheaves_core::peers::{Interval, unwitnessed};

    const THRESHOLD_MINUTES: i64 = 15;

    fn minute(m: i64) -> Timestamp {
        origin() + SignedDuration::from_mins(m)
    }

    fn model(began: i64, ended: i64, seen: &[(i64, i64)]) -> Vec<(i64, i64)> {
        let covered = |halves: i64| {
            seen.iter()
                .any(|&(start, end)| 2 * start <= halves && halves <= 2 * end)
        };
        let mut gaps = Vec::new();
        let mut run_start: Option<i64> = None;
        for halves in (2 * began + 1)..(2 * ended) {
            match (covered(halves), run_start) {
                (false, None) => run_start = Some(halves),
                (true, Some(first)) => {
                    gaps.push(((first - 1) / 2, halves / 2));
                    run_start = None;
                }
                _ => {}
            }
        }
        if let Some(first) = run_start {
            gaps.push(((first - 1) / 2, ended));
        }
        gaps.retain(|&(a, b)| b - a >= THRESHOLD_MINUTES);
        gaps
    }

    fn case() -> impl Strategy<Value = (i64, i64, Vec<(i64, i64)>, Vec<usize>)> {
        (0..120i64, 1..300i64).prop_flat_map(|(began, length)| {
            let ended = began + length;
            let interval = (began - 30..ended + 30, 0..90i64).prop_map(|(start, length)| (start, start + length));
            prop::collection::vec(interval, 0..6).prop_flat_map(move |seen| {
                let order = Just((0..seen.len()).collect::<Vec<_>>()).prop_shuffle();
                (Just(began), Just(ended), Just(seen), order)
            })
        })
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: 2000,
            failure_persistence: Some(Box::new(proptest::test_runner::FileFailurePersistence::WithSource("regressions"))),
            ..ProptestConfig::default()
        })]

        #[test]
        fn what_no_peer_saw_is_what_is_left((began, ended, seen, order) in case()) {
            let absence = Absence { began: minute(began), ended: minute(ended) };
            let intervals: Vec<Interval> = order
                .iter()
                .map(|&i| Interval { start: minute(seen[i].0), end: minute(seen[i].1) })
                .collect();
            let merged: Vec<(i64, i64)> = unwitnessed(absence, &intervals, THRESHOLD)
                .iter()
                .map(|gap| (
                    (gap.began - origin()).get_seconds() / 60,
                    (gap.ended - origin()).get_seconds() / 60,
                ))
                .collect();
            prop_assert_eq!(merged, model(began, ended, &seen));
        }
    }
}
