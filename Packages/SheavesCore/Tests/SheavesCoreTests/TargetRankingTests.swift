import Foundation
import Testing
@testable import SheavesCore

@Suite("Ranking targets by real usage")
@MainActor
struct TargetRankingTests {
    private let today = CalendarDate(year: 2026, month: 8, day: 29)

    private func entry(project: String, task: String, daysAgo: Int) -> TimeEntry {
        let spent = today.adding(days: -daysAgo)
        return TimeEntry(
            id: Int64(abs((project + task + spent.description).hashValue % 1_000_000)),
            spentDate: spent,
            user: nil,
            client: Reference(id: 1, name: "A Client"),
            project: Reference(id: abs(project.hashValue % 10_000), name: project),
            task: Reference(id: abs(task.hashValue % 10_000), name: task),
            hours: 1,
            hoursWithoutTimer: 1,
            notes: nil,
            isLocked: false,
            lockedReason: nil,
            isBilled: false,
            isRunning: false,
            billable: true,
            timerStartedAt: nil,
            startedTime: nil,
            endedTime: nil,
            createdAt: Date(),
            updatedAt: Date()
        )
    }

    @Test("ignores pairs the user has never logged")
    func onlyIncludesUsedTargets() {
        let ranked = TimeTracker.rankByUsage(
            [entry(project: "Acme", task: "Programming", daysAgo: 1)],
            asOf: today
        )
        #expect(ranked.count == 1)
        #expect(ranked[0].task.name == "Programming")
    }

    /// The complaint that prompted this: an alphabetical list puts Business
    /// Development above the thing you do every day.
    @Test("puts a daily habit above a single old entry")
    func habitBeatsOneOffs() throws {
        let entries =
            (0..<10).map { entry(project: "Acme", task: "Programming", daysAgo: $0) }
            + [entry(project: "Acme", task: "Business Development", daysAgo: 80)]

        let ranked = TimeTracker.rankByUsage(entries, asOf: today)

        #expect(ranked.first?.task.name == "Programming")
        #expect(ranked.last?.task.name == "Business Development")
    }

    /// Frequency alone would keep a finished project pinned for weeks.
    @Test("prefers current work over a finished project with more hours")
    func recencyBeatsRawFrequency() throws {
        let entries =
            (60..<90).map { entry(project: "Old", task: "Programming", daysAgo: $0) }
            + (0..<5).map { entry(project: "Current", task: "Programming", daysAgo: $0) }

        let ranked = TimeTracker.rankByUsage(entries, asOf: today)

        #expect(ranked.first?.project.name == "Current")
    }

    /// Recency alone would let one stray entry outrank months of real work.
    @Test("does not let a single recent entry outrank sustained work")
    func oneStrayEntryDoesNotWin() throws {
        let entries =
            (0..<20).map { entry(project: "Steady", task: "Programming", daysAgo: $0 + 2) }
            + [entry(project: "Stray", task: "Meeting", daysAgo: 0)]

        let ranked = TimeTracker.rankByUsage(entries, asOf: today)

        #expect(ranked.first?.project.name == "Steady")
    }

    @Test("orders identically on every sync")
    func isStable() {
        let entries = [
            entry(project: "A", task: "One", daysAgo: 3),
            entry(project: "B", task: "Two", daysAgo: 3),
        ]
        let first = TimeTracker.rankByUsage(entries, asOf: today).map(\.id)
        let second = TimeTracker.rankByUsage(entries.reversed(), asOf: today).map(\.id)
        #expect(first == second)
    }

    @Test("offers history first, then everything else, without duplicates")
    func orderedTargetsMergesSources() async throws {
        let transport = RoutingTransport.standardAccount()
        let snapshotURL = URL.temporaryDirectory.appending(path: "s-\(UUID().uuidString).json")
        let queueURL = URL.temporaryDirectory.appending(path: "q-\(UUID().uuidString).json")
        defer {
            try? FileManager.default.removeItem(at: snapshotURL)
            try? FileManager.default.removeItem(at: queueURL)
        }
        let tracker = TimeTracker(
            client: HarvestClient(credentials: Fixture.credentials, transport: transport, backoffScale: 0),
            keychain: KeychainStore(service: "com.rainhead.Sheaves.tests-\(UUID().uuidString)"),
            snapshots: SnapshotStore(fileURL: snapshotURL),
            queue: MutationQueue(fileURL: queueURL)
        )

        await tracker.sync()

        let ordered = tracker.orderedTargets
        #expect(ordered.count == tracker.targets.count)
        #expect(Set(ordered.map(\.id)).count == ordered.count)
    }
}

@Suite("CalendarDate arithmetic")
struct CalendarDateArithmeticTests {
    @Test("counts days between dates")
    func daysSince() {
        let today = CalendarDate(year: 2026, month: 8, day: 29)
        #expect(today.daysSince(CalendarDate(year: 2026, month: 8, day: 22)) == 7)
        #expect(today.daysSince(today) == 0)
        #expect(CalendarDate(year: 2026, month: 8, day: 22).daysSince(today) == -7)
        // Across a month boundary and a leap day.
        #expect(CalendarDate(year: 2028, month: 3, day: 1).daysSince(CalendarDate(year: 2028, month: 2, day: 28)) == 2)
    }
}
