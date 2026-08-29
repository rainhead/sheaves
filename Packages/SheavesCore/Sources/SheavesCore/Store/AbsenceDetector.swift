import Foundation

/// A stretch of time with nobody at the machine, noticed when they came back.
public struct Absence: Equatable, Sendable, Codable {
    /// The last moment there was evidence of someone here.
    public let began: Date
    /// When they returned.
    public let ended: Date

    public init(began: Date, ended: Date) {
        self.began = began
        self.ended = ended
    }

    public var duration: TimeInterval { ended.timeIntervalSince(began) }
}

/// Watches for stretches with nobody at the machine.
///
/// It knows nothing about keyboards or microphones — only about the moments
/// something showed that a person was here. That keeps the rule that decides what
/// counts as an absence testable, and keeps it out of the platform code that has to
/// go and ask the window server and CoreAudio.
///
/// Presence timestamps may be in the past: the HID idle clock reports *how long
/// since* the last event, which is more precise than the moment we happened to look.
public struct AbsenceDetector: Sendable {
    /// Long enough that reading at your desk is not mistaken for leaving it.
    public static let defaultThreshold: TimeInterval = 15 * 60

    public var threshold: TimeInterval
    public private(set) var lastPresence: Date

    public init(threshold: TimeInterval = AbsenceDetector.defaultThreshold, presentAt now: Date) {
        self.threshold = threshold
        self.lastPresence = now
    }

    /// Records evidence that someone was here at `when`, and reports the absence
    /// that just ended if it was long enough to be worth asking about.
    ///
    /// Sleep, screen lock and a shut lid need no special case: each simply stops the
    /// evidence arriving, and the gap is measured when it resumes. They are worth
    /// observing only so the question can be asked the instant the machine wakes,
    /// rather than at the next poll.
    @discardableResult
    public mutating func notePresence(at when: Date) -> Absence? {
        // Evidence older than what we already hold says nothing new, and a clock that
        // steps backwards must never manufacture an absence.
        guard when > lastPresence else { return nil }

        let gap = when.timeIntervalSince(lastPresence)
        let absence = gap >= threshold ? Absence(began: lastPresence, ended: when) : nil
        lastPresence = when
        return absence
    }
}

public extension Absence {
    /// The hours `entry` should hold if this absence is taken off it, or nil when
    /// this machine has no business saying anything about that entry.
    ///
    /// The rule that matters is the nil case. If the timer started *after* the
    /// absence began, then nobody has been at this Mac at any point while it ran —
    /// which is what a timer started on the web, the phone or the API looks like
    /// from here. Idle time on this machine is evidence about this machine, and it
    /// says nothing about whether that work was happening somewhere else.
    ///
    /// Harvest's own app applies the same guard ("Rejecting idle time because
    /// current running timer was more recently started than idle time started").
    /// Rejecting is the honest answer: clamping the trim to the timer's start would
    /// invent a stopping point out of evidence nobody has.
    func trimmedHours(for entry: TrackedEntry) -> Double? {
        guard entry.isRunning, let startedAt = entry.timerStartedAt else { return nil }
        guard began >= startedAt else { return nil }
        return entry.bankedHours + began.timeIntervalSince(startedAt) / 3600
    }
}

/// What to do about a timer that ran while nobody was here.
///
/// The first three are the choices Sheaves offers; the fourth mirrors Harvest's
/// own "Add Idle Time as a New Entry", which exists because the commonest reason a
/// machine sits untouched is that its owner was in a meeting that is itself
/// billable — just not to the project the timer was on.
public enum AbsenceResolution: Sendable, Equatable {
    /// It was working time after all. Bill it.
    case keep
    /// Take it off, and carry on timing from the moment they came back.
    case trimAndContinue
    /// Take it off, and stop the timer where they left.
    case trimAndStop
    /// Take it off the running timer and record it against something else.
    case trimAndLog(TimerTarget)
}
