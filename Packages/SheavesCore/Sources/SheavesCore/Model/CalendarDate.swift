import Foundation

/// A timezone-free calendar day, as Harvest's `spent_date` fields represent it.
///
/// Harvest returns `"2026-08-29"` with no zone. Round-tripping that through `Date`
/// means every entry is one day off for somebody, so days stay days here and only
/// become `Date` at the edges, against an explicit calendar.
public struct CalendarDate: Hashable, Sendable, Codable, Comparable, CustomStringConvertible {
    public var year: Int
    public var month: Int
    public var day: Int

    public init(year: Int, month: Int, day: Int) {
        self.year = year
        self.month = month
        self.day = day
    }

    /// The day `date` falls on in `calendar` (the user's current calendar by default).
    public init(_ date: Date, in calendar: Calendar = .current) {
        let parts = calendar.dateComponents([.year, .month, .day], from: date)
        self.init(year: parts.year!, month: parts.month!, day: parts.day!)
    }

    public static func today(in calendar: Calendar = .current) -> CalendarDate {
        CalendarDate(Date(), in: calendar)
    }

    /// Midnight at the start of this day.
    public func startOfDay(in calendar: Calendar = .current) -> Date {
        calendar.date(from: DateComponents(year: year, month: month, day: day)) ?? Date()
    }

    public func adding(days: Int, in calendar: Calendar = .current) -> CalendarDate {
        let shifted = calendar.date(byAdding: .day, value: days, to: startOfDay(in: calendar))
        return CalendarDate(shifted ?? startOfDay(in: calendar), in: calendar)
    }

    /// Whole days from `other` to this date. Negative when `other` is later.
    public func daysSince(_ other: CalendarDate, in calendar: Calendar = .current) -> Int {
        calendar.dateComponents(
            [.day],
            from: other.startOfDay(in: calendar),
            to: startOfDay(in: calendar)
        ).day ?? 0
    }

    /// The first day of this date's week, honouring the account's `week_start_day`.
    public func startOfWeek(weekStart: Weekday, in calendar: Calendar = .current) -> CalendarDate {
        var calendar = calendar
        calendar.firstWeekday = weekStart.calendarIndex
        let interval = calendar.dateInterval(of: .weekOfYear, for: startOfDay(in: calendar))
        guard let interval else { return self }
        return CalendarDate(interval.start, in: calendar)
    }

    // MARK: ISO 8601 text

    public var description: String {
        String(format: "%04d-%02d-%02d", year, month, day)
    }

    public init?(iso: String) {
        let parts = iso.split(separator: "-")
        guard parts.count == 3,
              let year = Int(parts[0]), let month = Int(parts[1]), let day = Int(parts[2]),
              (1...12).contains(month), (1...31).contains(day)
        else { return nil }
        self.init(year: year, month: month, day: day)
    }

    public init(from decoder: any Decoder) throws {
        let text = try decoder.singleValueContainer().decode(String.self)
        guard let parsed = CalendarDate(iso: text) else {
            throw DecodingError.dataCorrupted(
                .init(codingPath: decoder.codingPath, debugDescription: "Not an ISO 8601 date: \(text)")
            )
        }
        self = parsed
    }

    public func encode(to encoder: any Encoder) throws {
        var container = encoder.singleValueContainer()
        try container.encode(description)
    }

    public static func < (lhs: CalendarDate, rhs: CalendarDate) -> Bool {
        (lhs.year, lhs.month, lhs.day) < (rhs.year, rhs.month, rhs.day)
    }
}

/// The account's `week_start_day`. Harvest only ever returns these three.
public enum Weekday: String, Sendable, Codable, CaseIterable {
    case saturday = "Saturday"
    case sunday = "Sunday"
    case monday = "Monday"

    /// `Calendar.firstWeekday` uses 1 = Sunday.
    var calendarIndex: Int {
        switch self {
        case .sunday: 1
        case .monday: 2
        case .saturday: 7
        }
    }
}
