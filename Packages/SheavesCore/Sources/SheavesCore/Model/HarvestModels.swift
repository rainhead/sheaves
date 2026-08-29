import Foundation

/// The `{ "id": …, "name": … }` stub Harvest embeds wherever it references another record.
public struct Reference: Identifiable, Codable, Sendable, Hashable {
    public let id: Int
    public let name: String

    public init(id: Int, name: String) {
        self.id = id
        self.name = name
    }
}

public struct ProjectReference: Identifiable, Codable, Sendable, Hashable {
    public let id: Int
    public let name: String
    /// Short project code, often empty.
    public let code: String?

    public init(id: Int, name: String, code: String? = nil) {
        self.id = id
        self.name = name
        self.code = code
    }
}

public struct HarvestUser: Identifiable, Codable, Sendable, Hashable {
    public let id: Int
    public let firstName: String
    public let lastName: String
    public let email: String

    public var name: String {
        "\(firstName) \(lastName)".trimmingCharacters(in: .whitespaces)
    }
}

/// Account-wide settings. `wantsTimestampTimers` decides whether this account records
/// durations or start/end times, which changes how a timer is created and stopped.
public struct HarvestCompany: Codable, Sendable, Hashable {
    public let name: String
    public let fullDomain: String
    public let isActive: Bool
    public let weekStartDay: Weekday
    public let wantsTimestampTimers: Bool
    /// `decimal` or `hours_minutes`.
    public let timeFormat: String
    /// Weekly capacity, in seconds.
    public let weeklyCapacity: Int?
}

public struct TimeEntry: Identifiable, Codable, Sendable, Hashable {
    public let id: Int64
    public var spentDate: CalendarDate
    public var user: Reference?
    public var client: Reference?
    public var project: Reference
    public var task: Reference
    /// Total hours recorded, as of when this entry was fetched.
    public var hours: Double
    /// Hours excluding the currently running timer. Use `elapsedHours` for a live total.
    public var hoursWithoutTimer: Double?
    public var notes: String?
    public var isLocked: Bool
    public var lockedReason: String?
    public var isBilled: Bool
    public var isRunning: Bool
    public var billable: Bool
    /// When the running timer was started. Nil unless `isRunning`.
    public var timerStartedAt: Date?
    /// Wall-clock start/end, on `wantsTimestampTimers` accounts. Formatted per account settings.
    public var startedTime: String?
    public var endedTime: String?
    public var createdAt: Date
    public var updatedAt: Date

    /// Hours on the clock right now: a running timer keeps counting between fetches.
    public func elapsedHours(asOf now: Date = Date()) -> Double {
        guard isRunning, let timerStartedAt else { return hours }
        let base = hoursWithoutTimer ?? hours
        return base + max(0, now.timeIntervalSince(timerStartedAt)) / 3600
    }

    /// True when Harvest will reject edits — approved, invoiced, or in a closed timesheet.
    public var isEditable: Bool { !isLocked }
}

public struct ProjectAssignment: Identifiable, Codable, Sendable, Hashable {
    public let id: Int
    public let isActive: Bool
    public let project: ProjectReference
    public let client: Reference
    public let taskAssignments: [TaskAssignment]
}

public struct TaskAssignment: Identifiable, Codable, Sendable, Hashable {
    public let id: Int
    public let billable: Bool
    public let isActive: Bool
    public let task: Reference
}

// MARK: - Pagination

/// A resource Harvest returns in a paginated envelope keyed by `pageKey`.
///
/// `pageKey` is camelCase because the decoder converts from snake_case before
/// keys are matched: `"time_entries"` arrives as `"timeEntries"`.
public protocol PaginatedItem: Decodable, Sendable {
    static var pageKey: String { get }
}

extension TimeEntry: PaginatedItem {
    public static var pageKey: String { "timeEntries" }
}

extension ProjectAssignment: PaginatedItem {
    public static var pageKey: String { "projectAssignments" }
}

struct Page<Item: PaginatedItem>: Decodable, Sendable {
    let items: [Item]
    let page: Int
    let totalPages: Int
    let totalEntries: Int
    let nextPage: Int?

    private struct AnyKey: CodingKey {
        let stringValue: String
        var intValue: Int? { nil }
        init?(stringValue: String) { self.stringValue = stringValue }
        init?(intValue: Int) { nil }
        init(_ literal: String) { self.stringValue = literal }
    }

    init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: AnyKey.self)
        items = try container.decode([Item].self, forKey: AnyKey(Item.pageKey))
        page = try container.decodeIfPresent(Int.self, forKey: AnyKey("page")) ?? 1
        totalPages = try container.decodeIfPresent(Int.self, forKey: AnyKey("totalPages")) ?? 1
        totalEntries = try container.decodeIfPresent(Int.self, forKey: AnyKey("totalEntries")) ?? items.count
        nextPage = try container.decodeIfPresent(Int.self, forKey: AnyKey("nextPage"))
    }
}
