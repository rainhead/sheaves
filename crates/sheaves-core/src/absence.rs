//! Stretches of time with nobody at the machine, and what they mean for a timer.

use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde::{Deserialize, Serialize};

use crate::calendar_date::CalendarDate;
use crate::models::{offset_by, seconds_between};
use crate::tracked_entry::{TimerTarget, TrackedEntry};

/// A stretch of time with nobody at the machine, noticed when they came back.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct Absence {
    /// The last moment there was evidence of someone here.
    pub began: Timestamp,
    /// When they returned.
    pub ended: Timestamp,
}

impl Absence {
    /// How long they were away, in seconds.
    pub fn duration(&self) -> f64 {
        seconds_between(self.began, self.ended)
    }

    /// A timer is started by a click or a keystroke, and that input *is* the last
    /// evidence of presence, so for a timer started with the mouse `began` lands a
    /// few milliseconds *before* `timer_started_at`, which is read inside the
    /// handler the click triggered. Without a little slack the commonest absence of
    /// all, start a timer and walk to the meeting, would be refused as somebody
    /// else's. Two minutes is far below the gap that makes a timer someone else's.
    pub const START_TOLERANCE: f64 = 120.0;

    /// The hours `entry` should hold if this absence is taken off it, or `None` when
    /// this machine has no business saying anything about that entry.
    ///
    /// The rule that matters is the `None` case. If the timer started *after* the
    /// absence began, then nobody has been at this machine at any point while it
    /// ran, which is what a timer started on the web, the phone or the API looks
    /// like from here. Idle time on this machine is evidence about this machine, and
    /// it says nothing about whether that work was happening somewhere else.
    ///
    /// Harvest's own app applies the same guard ("Rejecting idle time because
    /// current running timer was more recently started than idle time started").
    /// Rejecting is the honest answer: clamping the trim to the timer's start would
    /// invent a stopping point out of evidence nobody has.
    pub fn trimmed_hours(&self, entry: &TrackedEntry) -> Option<f64> {
        if !entry.is_running {
            return None;
        }
        let started_at = entry.timer_started_at?;
        if self.began < offset_by(started_at, -Self::START_TOLERANCE) {
            return None;
        }
        // Never negative: within the tolerance the last input can precede the start.
        Some(entry.banked_hours + seconds_between(started_at, self.began).max(0.0) / 3600.0)
    }

    /// The day the absence itself happened on, which is not always the day of the
    /// timer it interrupted: a timer left running overnight belongs to yesterday,
    /// while this morning's meeting belongs to today.
    pub fn day(&self, zone: &TimeZone) -> CalendarDate {
        CalendarDate::of(self.began, zone)
    }
}

/// Watches for stretches with nobody at the machine.
///
/// It knows nothing about keyboards or microphones, only about the moments
/// something showed that a person was here. That keeps the rule that decides what
/// counts as an absence testable, and keeps it out of the platform code that has to
/// go and ask the window server and the audio system.
///
/// Presence timestamps may be in the past: an idle clock reports *how long since*
/// the last event, which is more precise than the moment we happened to look.
#[derive(Clone, Debug)]
pub struct AbsenceDetector {
    pub threshold: f64,
    last_presence: Timestamp,
}

impl AbsenceDetector {
    /// Long enough that reading at your desk is not mistaken for leaving it.
    pub const DEFAULT_THRESHOLD: f64 = 15.0 * 60.0;

    pub fn new(threshold: f64, present_at: Timestamp) -> Self {
        Self {
            threshold,
            last_presence: present_at,
        }
    }

    pub fn last_presence(&self) -> Timestamp {
        self.last_presence
    }

    /// Records evidence that someone was here at `when`, and reports the absence
    /// that just ended if it was long enough to be worth asking about.
    ///
    /// Sleep, screen lock and a shut lid need no special case: each simply stops the
    /// evidence arriving, and the gap is measured when it resumes. They are worth
    /// observing only so the question can be asked the instant the machine wakes,
    /// rather than at the next poll.
    pub fn note_presence(&mut self, when: Timestamp) -> Option<Absence> {
        // Evidence older than what we already hold says nothing new, and a clock
        // that steps backwards must never manufacture an absence.
        if when <= self.last_presence {
            return None;
        }
        let gap = seconds_between(self.last_presence, when);
        let absence = (gap >= self.threshold).then_some(Absence {
            began: self.last_presence,
            ended: when,
        });
        self.last_presence = when;
        absence
    }
}

/// What to do about a timer that ran while nobody was here.
///
/// The first three are the choices Sheaves offers; the fourth mirrors Harvest's own
/// "Add Idle Time as a New Entry", which exists because the commonest reason a
/// machine sits untouched is that its owner was in a meeting that is itself
/// billable, just not to the project the timer was on.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum AbsenceResolution {
    /// It was working time after all. Bill it.
    Keep,
    /// Take it off, and carry on timing from the moment they came back.
    TrimAndContinue,
    /// Take it off, and stop the timer where they left.
    TrimAndStop,
    /// Take it off the running timer and record it against something else.
    TrimAndLog(TimerTarget),
}

/// One look at the signals that say somebody is at the machine. Each shell reads
/// them its own way; what they mean is decided here.
#[derive(Clone, Copy, Debug, Default, PartialEq, uniffi::Record)]
pub struct PresenceReading {
    /// Seconds since the last keyboard or mouse event.
    pub seconds_since_input: f64,
    /// Whether any app is capturing from the microphone right now. Always false
    /// where the shell has no way to tell.
    pub microphone_in_use: bool,
    pub screen_locked: bool,
}

/// Folds readings of the machine into an [`AbsenceDetector`].
#[derive(Clone, Debug)]
pub struct PresenceMonitor {
    detector: AbsenceDetector,
    /// When the machine was last looked at, so a call found in progress can be
    /// credited to the whole interval it may have been running for.
    last_poll: Option<Timestamp>,
}

impl PresenceMonitor {
    /// Half a minute is far finer than the fifteen it takes to count as away, and
    /// costs two property reads.
    pub const POLL_INTERVAL: f64 = 30.0;

    pub fn new(threshold: f64, now: Timestamp) -> Self {
        Self {
            detector: AbsenceDetector::new(threshold, now),
            last_poll: None,
        }
    }

    pub fn detector(&self) -> &AbsenceDetector {
        &self.detector
    }

    /// Folds one reading taken at `now` into the detector, and returns any absence
    /// it ended.
    pub fn poll(&mut self, reading: PresenceReading, now: Timestamp) -> Vec<Absence> {
        // A locked screen outranks everything. A call left running on a locked
        // machine still holds the microphone, and that is somebody's machine sitting
        // in an empty room, not somebody working.
        if reading.screen_locked {
            return Vec::new();
        }
        let mut absences = Vec::new();

        // The microphone is read first, and its evidence is dated to the previous
        // poll rather than to now.
        //
        // Both matter. A call that began at any point since the last look could have
        // begun at the start of that interval, and dating it to `now` would let the
        // gap before it reach the threshold, so joining a call after a quiet stretch
        // of reading would open the prompt *during the call*, which is the one thing
        // this must never do. Reading it before the idle clock means the presence is
        // banked before any gap is measured.
        if reading.microphone_in_use {
            absences.extend(self.detector.note_presence(self.last_poll.unwrap_or(now)));
        }

        // The idle clock says how long *since* the last event, which is a better
        // answer than the moment this poll happened to run.
        absences.extend(
            self.detector
                .note_presence(offset_by(now, -reading.seconds_since_input)),
        );
        self.last_poll = Some(now);
        absences
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calendar_date::CalendarDate;
    use crate::models::Reference;
    use crate::tracked_entry::EntryId;

    fn at(seconds: f64) -> Timestamp {
        offset_by(Timestamp::from_second(1_700_000_000).unwrap(), seconds)
    }

    mod absence_detector {
        use super::*;

        fn detector() -> AbsenceDetector {
            AbsenceDetector::new(15.0 * 60.0, at(0.0))
        }

        #[test]
        fn quiet_while_present() {
            let mut detector = detector();
            for minute in 1..=60 {
                assert_eq!(detector.note_presence(at(f64::from(minute) * 60.0)), None);
            }
        }

        #[test]
        fn reports_absence() {
            let mut detector = detector();
            let back = at(15.0 * 60.0);
            assert_eq!(
                detector.note_presence(back),
                Some(Absence {
                    began: at(0.0),
                    ended: back
                })
            );
        }

        /// A minute under is someone reading at their desk, and asking about it is
        /// the behaviour that makes people turn idle detection off.
        #[test]
        fn ignores_short_gaps() {
            assert_eq!(detector().note_presence(at(15.0 * 60.0 - 1.0)), None);
        }

        /// The absence ends when they came back, not when the poll happened to run.
        #[test]
        fn measures_to_the_return() {
            let mut detector = detector();
            let came_back = at(40.0 * 60.0);
            detector.note_presence(came_back);
            assert_eq!(detector.last_presence(), came_back);
            // The next poll must not report a second absence for the same stretch.
            assert_eq!(detector.note_presence(offset_by(came_back, 25.0)), None);
        }

        #[test]
        fn reports_each_absence_once() {
            let mut detector = detector();
            assert!(detector.note_presence(at(3600.0)).is_some());
            assert!(detector.note_presence(at(3601.0)).is_none());
            assert!(detector.note_presence(at(7200.0)).is_some());
        }

        /// A clock that steps backwards, from a time-zone change or an NTP
        /// correction, must not manufacture an absence, and must not rewind the
        /// record of presence.
        #[test]
        fn ignores_backwards_clock() {
            let mut detector = detector();
            assert_eq!(detector.note_presence(at(-3600.0)), None);
            assert_eq!(detector.last_presence(), at(0.0));
        }
    }

    mod absence_trim {
        use super::*;

        fn running(banked_hours: f64, started_at: Timestamp) -> TrackedEntry {
            TrackedEntry {
                id: EntryId::Server(1),
                project: Reference::new(1, "Online Store"),
                task: Reference::new(2, "Programming"),
                client: None,
                notes: None,
                spent_date: CalendarDate::new(2026, 8, 29),
                banked_hours,
                is_running: true,
                timer_started_at: Some(started_at),
                is_locked: false,
                is_billable: true,
                is_pending: false,
                updated_at: started_at,
            }
        }

        fn absence(began: f64, ended: f64) -> Absence {
            Absence {
                began: at(began),
                ended: at(ended),
            }
        }

        #[test]
        fn trims_to_departure() {
            assert_eq!(
                absence(1800.0, 5400.0).trimmed_hours(&running(0.0, at(0.0))),
                Some(0.5)
            );
        }

        #[test]
        fn keeps_banked_hours() {
            assert_eq!(
                absence(1800.0, 5400.0).trimmed_hours(&running(2.0, at(0.0))),
                Some(2.5)
            );
        }

        /// The rule that keeps this machine from speaking for work done elsewhere: a
        /// timer that started after the absence began has never once run while
        /// somebody was at this machine, so this machine knows nothing about it.
        #[test]
        fn refuses_timer_started_elsewhere() {
            assert_eq!(
                absence(0.0, 7200.0).trimmed_hours(&running(0.0, at(3600.0))),
                None
            );
        }

        #[test]
        fn accepts_timer_started_at_departure() {
            assert_eq!(
                absence(0.0, 3600.0).trimmed_hours(&running(0.0, at(0.0))),
                Some(0.0)
            );
        }

        /// Start a timer, walk to the meeting. The click that started it *is* the
        /// last input, and `timer_started_at` is read a moment later, so the last
        /// presence precedes the start; without tolerance the commonest absence of
        /// all would be refused as somebody else's timer.
        #[test]
        fn accepts_timer_started_just_after_the_last_input() {
            assert_eq!(
                absence(0.0, 3600.0).trimmed_hours(&running(0.0, at(0.05))),
                Some(0.0)
            );
        }

        #[test]
        fn day_comes_from_the_absence() {
            let zone = TimeZone::get("America/Los_Angeles").unwrap();
            assert_eq!(
                absence(0.0, 3600.0).day(&zone),
                CalendarDate::of(at(0.0), &zone)
            );
        }

        #[test]
        fn refuses_stopped_entry() {
            let mut entry = running(0.0, at(0.0));
            entry.is_running = false;
            entry.timer_started_at = None;
            assert_eq!(absence(60.0, 3600.0).trimmed_hours(&entry), None);
        }
    }

    mod presence_monitor {
        use super::*;

        struct Run {
            monitor: PresenceMonitor,
            reading: PresenceReading,
            absences: Vec<Absence>,
        }

        impl Run {
            fn new() -> Self {
                Self {
                    monitor: PresenceMonitor::new(15.0 * 60.0, at(0.0)),
                    reading: PresenceReading::default(),
                    absences: Vec::new(),
                }
            }

            fn poll(&mut self, seconds: f64) {
                let absences = self.monitor.poll(self.reading, at(seconds));
                self.absences.extend(absences);
            }

            /// Polls every half minute from `from` through `to` seconds, with no
            /// input since the start: the shape of somebody sitting still.
            fn still(&mut self, from: f64, to: f64) {
                let mut tick = from;
                while tick <= to {
                    self.reading.seconds_since_input = tick;
                    self.poll(tick);
                    tick += 30.0;
                }
            }

            /// Back at the keyboard, `ago` seconds before `seconds`.
            fn return_at(&mut self, seconds: f64, ago: f64) {
                self.reading.seconds_since_input = ago;
                self.poll(seconds);
            }
        }

        #[test]
        fn quiet_while_typing() {
            let mut run = Run::new();
            let mut tick = 30.0;
            while tick <= 3600.0 {
                run.reading.seconds_since_input = 5.0;
                run.poll(tick);
                tick += 30.0;
            }
            assert!(run.absences.is_empty());
        }

        #[test]
        fn reports_on_return() {
            let mut run = Run::new();
            run.still(30.0, 20.0 * 60.0);
            assert!(run.absences.is_empty());
            run.return_at(20.0 * 60.0 + 2.0, 2.0);
            assert_eq!(
                run.absences,
                [Absence {
                    began: at(0.0),
                    ended: at(20.0 * 60.0)
                }]
            );
        }

        /// The reason this exists. Harvest's own app watches keyboard and mouse
        /// alone, so an hour on a call looks exactly like an hour at lunch and it
        /// interrupts to ask. A machine whose microphone is in use has somebody in
        /// front of it.
        #[test]
        fn microphone_counts_as_presence() {
            let mut run = Run::new();
            run.reading.microphone_in_use = true;
            run.still(30.0, 90.0 * 60.0);
            // An absence only ever surfaces when somebody comes back, so this has to
            // come back to mean anything.
            run.reading.microphone_in_use = false;
            run.return_at(90.0 * 60.0 + 1.0, 1.0);
            assert!(run.absences.is_empty());
        }

        #[test]
        fn absence_begins_at_end_of_call() {
            let mut run = Run::new();
            run.reading.microphone_in_use = true;
            run.still(30.0, 30.0 * 60.0);
            let call_ended = 30.0 * 60.0;
            run.reading.microphone_in_use = false;
            run.still(30.0 * 60.0 + 30.0, 50.0 * 60.0);
            run.return_at(50.0 * 60.0 + 1.0, 1.0);

            // One poll interval earlier than the call actually ended: a microphone
            // found running could have started at any point since the last look, so
            // its evidence is dated to that look. Erring this way trims slightly
            // more than was missed, never less.
            assert_eq!(
                run.absences[0].began,
                at(call_ended - PresenceMonitor::POLL_INTERVAL)
            );
        }

        /// Joining a call after a quiet stretch of reading must not open the prompt
        /// *during* the call.
        #[test]
        fn call_joined_just_before_the_threshold_is_presence() {
            let mut run = Run::new();
            run.still(30.0, 14.5 * 60.0);
            assert!(run.absences.is_empty());
            run.reading.microphone_in_use = true;
            run.still(14.5 * 60.0 + 30.0, 40.0 * 60.0);
            assert!(run.absences.is_empty());
        }

        /// A meeting left open on a locked machine still holds the microphone. That
        /// is a machine alone in a room, not somebody working.
        #[test]
        fn locked_screen_beats_microphone() {
            let mut run = Run::new();
            run.reading.microphone_in_use = true;
            run.reading.screen_locked = true;
            run.still(30.0, 40.0 * 60.0);
            assert!(run.absences.is_empty());
            run.reading.screen_locked = false;
            run.return_at(40.0 * 60.0 + 1.0, 1.0);
            assert_eq!(run.absences[0].began, at(0.0));
        }

        /// Sleeping needs no special case: the evidence simply stops arriving, and
        /// the gap is measured when the user touches the machine again.
        #[test]
        fn measures_sleep() {
            let mut run = Run::new();
            let overnight = 9.0 * 3600.0;
            run.return_at(overnight, 1.0);
            assert_eq!(run.absences[0].began, at(0.0));
            assert_eq!(run.absences[0].duration(), overnight - 1.0);
        }

        /// A machine that wakes on its own overnight, for Power Nap or a backup, has
        /// nobody in front of it, and must not be mistaken for somebody coming back.
        #[test]
        fn ignores_unattended_wake() {
            let mut run = Run::new();
            let overnight = 9.0 * 3600.0;
            run.return_at(overnight, overnight);
            assert!(run.absences.is_empty());
        }
    }
}
