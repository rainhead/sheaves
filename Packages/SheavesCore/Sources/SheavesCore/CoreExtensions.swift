import Foundation

// Swift spellings for the Rust core's types, so the views read as they did when the
// core was Swift. Every rule behind these lives in `crates/sheaves-core`; this file
// only renames, and adds the conformances SwiftUI wants.

public typealias HarvestCredentials = Credentials

public extension Credentials {
    /// The core trims whitespace pasted around either value when it connects.
    init(accountID: String, token: String) {
        self.init(accountId: accountID, token: token)
    }

    var accountID: String { accountId }
}

extension TrackedEntry: Identifiable {}

public extension TrackedEntry {
    typealias ID = EntryId

    /// Hours on the clock at `now`: banked time plus whatever the running timer has
    /// added.
    func hours(asOf now: Date = Date()) -> Double {
        hoursAt(now: now)
    }

    var target: TimerTarget { timerTarget() }
}

public extension EntryId {
    var serverID: Int64? { harvestId() }
}

extension TimerTarget: Identifiable {
    public var id: String { key() }
}

public extension TimerTarget {
    /// "Client · Project": the project alone is often ambiguous across clients.
    var projectLabel: String { label() }
}

extension ProjectTargets: Identifiable {
    public var id: Int64 { project.id }
}

public extension ProjectTargets {
    func target(task: Reference) -> TimerTarget { withTask(task: task) }

    /// "Client · Project", the way an entry names its project.
    var label: String { projectLabel() }
}

extension Reference: Identifiable {}

public extension ProjectBudget {
    var projectID: Int64 { projectId }
    var hasReadableBudget: Bool { isReadable() }
    var isOverBudget: Bool { isOverrun() }
    /// How much of the budget is used, 0…1, clamped so an overrun draws a full bar.
    var fractionUsed: Double { used() }
}

public extension HarvestUser {
    var name: String { displayName() }
}

public extension Absence {
    /// Seconds away.
    var duration: TimeInterval { length() }

    /// The hours `entry` should hold with this absence taken off, or nil when this
    /// machine has no standing to say.
    func trimmedHours(for entry: TrackedEntry) -> Double? { trimFor(entry: entry) }

    /// The day the absence happened on, in this machine's time zone.
    func day() -> CalendarDate { localDay() }
}

extension CalendarDate: CustomStringConvertible, Comparable {
    /// The ISO 8601 text Harvest reads. Not locale-aware, deliberately.
    public var description: String { iso() }

    public static func < (lhs: CalendarDate, rhs: CalendarDate) -> Bool {
        lhs.isBefore(other: rhs)
    }
}

public extension CalendarDate {
    static func today() -> CalendarDate { calendarToday() }

    /// The day `date` falls on in this machine's time zone.
    init(_ date: Date) { self = calendarDayOf(instant: date) }

    func adding(days: Int) -> CalendarDate { plusDays(days: Int32(days)) }

    func daysSince(_ other: CalendarDate) -> Int { Int(daysAfter(other: other)) }

    /// Midnight at the start of this day, in this machine's time zone.
    func startOfDay() -> Date { localStart() }
}

public extension Connection {
    var isConfigured: Bool { hasCredentials() }
}

public extension Activity {
    var entry: TrackedEntry? { currentEntry() }
}

public extension HoursFormat {
    init(company: HarvestCompany?) {
        self = hoursFormatOf(timeFormat: company?.timeFormat)
    }
}

public enum HoursEdit {
    /// The hours a closed duration field should send, or nil to send nothing. The
    /// rule is the core's; see `HoursEdit::commit` there.
    public static func commit(
        draft: String,
        asOpened: String,
        banked: Double,
        isRunning: Bool,
        locale: Locale = .autoupdatingCurrent
    ) -> Double? {
        hoursEditCommit(
            draft: draft,
            asOpened: asOpened,
            banked: banked,
            isRunning: isRunning,
            decimalSeparator: locale.decimalSeparator ?? "."
        )
    }
}

public extension Double {
    /// Reads a duration the way people type them: `1`, `1:30`, `:45`, `1.5`, with
    /// the locale's decimal separator. The parser is the core's.
    static func hours(parsing text: String, locale: Locale = .autoupdatingCurrent) -> Double? {
        parseDuration(text: text, decimalSeparator: locale.decimalSeparator ?? ".")
    }
}
