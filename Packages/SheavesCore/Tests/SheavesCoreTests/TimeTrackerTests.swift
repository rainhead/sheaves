import Foundation
import Testing
@testable import SheavesCore

@Suite("TimeTracker")
@MainActor
struct TimeTrackerTests {
    /// Builds a tracker wired to scratch files, so tests never touch the real cache.
    private func makeTracker(transport: RoutingTransport) -> (TimeTracker, URL, URL) {
        let snapshotURL = URL.temporaryDirectory.appending(path: "sheaves-snapshot-\(UUID().uuidString).json")
        let queueURL = URL.temporaryDirectory.appending(path: "sheaves-queue-\(UUID().uuidString).json")
        let tracker = TimeTracker(
            client: HarvestClient(credentials: Fixture.credentials, transport: transport, backoffScale: 0),
            keychain: KeychainStore(service: "com.rainhead.Sheaves.tests-\(UUID().uuidString)"),
            snapshots: SnapshotStore(fileURL: snapshotURL),
            queue: MutationQueue(fileURL: queueURL)
        )
        return (tracker, snapshotURL, queueURL)
    }

    private func cleanUp(_ urls: URL...) {
        for url in urls { try? FileManager.default.removeItem(at: url) }
    }

    @Test("loads the day, the account and the available targets")
    func syncPopulatesState() async throws {
        let transport = RoutingTransport.standardAccount()
        let (tracker, snapshot, queue) = makeTracker(transport: transport)
        defer { cleanUp(snapshot, queue) }

        await tracker.sync()

        #expect(tracker.connection == .online)
        #expect(tracker.user?.name == "Kim Allen")
        #expect(tracker.company?.weekStartDay == .monday)
        #expect(tracker.entries.count == 1)
        #expect(tracker.targets.count == 1)
        #expect(tracker.lastSyncedAt != nil)
    }

    /// Project assignments cost a request each sync and change about never; a burst of
    /// start/stop clicks should not re-download them.
    @Test("does not refetch the project list on every sync")
    func cachesAccountData() async throws {
        let transport = RoutingTransport.standardAccount()
        let (tracker, snapshot, queue) = makeTracker(transport: transport)
        defer { cleanUp(snapshot, queue) }

        await tracker.sync()
        await tracker.sync()
        await tracker.sync()

        #expect(await transport.callCount(matching: "project_assignments") == 1)
        #expect(await transport.callCount(matching: "company") == 1)
        // The day itself is re-read every time; that is the point of syncing.
        #expect(await transport.callCount(matching: "time_entries") == 6)
    }

    @Test("shows a started timer before Harvest has confirmed it")
    func startIsOptimistic() async throws {
        let transport = RoutingTransport.standardAccount()
        let (tracker, snapshot, queue) = makeTracker(transport: transport)
        defer { cleanUp(snapshot, queue) }
        await tracker.sync()
        let target = try #require(tracker.targets.first)

        // Nothing answers POST /time_entries here, so the create cannot succeed.
        await tracker.start(target, notes: "spike")

        let running = try #require(tracker.runningEntry)
        #expect(running.task.name == target.task.name)
        #expect(running.notes == "spike")
        #expect(running.isPending)
        #expect(running.id.serverID == nil)
        #expect(tracker.recentTargets.first?.id == target.id)
    }

    /// Harvest allows one running timer per user; the UI must not imply otherwise.
    @Test("never shows two timers running at once")
    func startStopsThePreviousTimer() async throws {
        let running = Fixture.timeEntriesPage([Fixture.runningTimeEntry])
        let transport = RoutingTransport([
            ("users/me/project_assignments", Fixture.projectAssignmentsPage),
            ("users/me", Fixture.currentUser),
            ("company", Fixture.company),
            ("is_running=true", running),
            ("time_entries", running),
        ])
        let (tracker, snapshot, queue) = makeTracker(transport: transport)
        defer { cleanUp(snapshot, queue) }
        await tracker.sync()
        #expect(tracker.runningEntry != nil)
        let target = try #require(tracker.targets.first)

        await tracker.start(target)

        #expect(tracker.entries.filter(\.isRunning).count == 1)
        #expect(tracker.runningEntry?.task.name == target.task.name)
    }

    @Test("banks elapsed time when a timer is stopped")
    func stopBanksElapsedTime() async throws {
        let running = Fixture.timeEntriesPage([Fixture.runningTimeEntry])
        let transport = RoutingTransport([
            ("users/me/project_assignments", Fixture.projectAssignmentsPage),
            ("users/me", Fixture.currentUser),
            ("company", Fixture.company),
            ("is_running=true", running),
            ("time_entries", running),
        ])
        let (tracker, snapshot, queue) = makeTracker(transport: transport)
        defer { cleanUp(snapshot, queue) }
        await tracker.sync()
        let entry = try #require(tracker.runningEntry)

        await tracker.stop(entry)

        let stopped = try #require(tracker.entries.first { $0.id == entry.id })
        #expect(!stopped.isRunning)
        #expect(stopped.timerStartedAt == nil)
        // The fixture banked an hour before the timer was started.
        #expect(stopped.bankedHours >= 1.0)
    }

    @Test("resumes an existing entry rather than creating a duplicate")
    func startResumesMatchingEntry() async throws {
        // The stopped fixture entry is Graphic Design on Marketing Website; the only
        // available target is Graphic Design on Online Store, so they must not match.
        let transport = RoutingTransport.standardAccount()
        let (tracker, snapshot, queue) = makeTracker(transport: transport)
        defer { cleanUp(snapshot, queue) }
        await tracker.sync()
        let existing = try #require(tracker.entries.first)
        let before = tracker.entries.count

        await tracker.start(existing.target)

        #expect(tracker.entries.count == before)
        #expect(tracker.runningEntry?.id == existing.id)
    }

    @Test("keeps working when Harvest is unreachable")
    func staysUsableOffline() async throws {
        let transport = RoutingTransport([])  // every route 404s
        let (tracker, snapshot, queue) = makeTracker(transport: transport)
        defer { cleanUp(snapshot, queue) }

        await tracker.sync()

        if case .online = tracker.connection {
            Issue.record("expected an offline connection state")
        }
    }

    @Test("queues changes it could not deliver")
    func queuesUndeliverableChanges() async throws {
        let transport = RoutingTransport.standardAccount()
        let (tracker, snapshot, queue) = makeTracker(transport: transport)
        defer { cleanUp(snapshot, queue) }
        await tracker.sync()
        let target = try #require(tracker.targets.first)

        // POST /time_entries has no route, so the create is refused and retained.
        await tracker.start(target)

        #expect(tracker.pendingCount >= 1)
    }

    @Test("changing the visible day reloads it")
    func selectDayReloads() async throws {
        let transport = RoutingTransport.standardAccount()
        let (tracker, snapshot, queue) = makeTracker(transport: transport)
        defer { cleanUp(snapshot, queue) }
        await tracker.sync()

        await tracker.shiftDay(by: -1)

        #expect(tracker.day == CalendarDate.today().adding(days: -1))
        #expect(!tracker.isToday)
        await tracker.goToToday()
        #expect(tracker.isToday)
    }

    @Test("totals the visible day")
    func totalsTheDay() async throws {
        let transport = RoutingTransport.standardAccount()
        let (tracker, snapshot, queue) = makeTracker(transport: transport)
        defer { cleanUp(snapshot, queue) }

        await tracker.sync()

        #expect(tracker.totalHours == 2.11)
    }

    @Test("filters targets by fuzzy query")
    func suggestsTargets() async throws {
        let transport = RoutingTransport.standardAccount()
        let (tracker, snapshot, queue) = makeTracker(transport: transport)
        defer { cleanUp(snapshot, queue) }
        await tracker.sync()

        #expect(tracker.suggestedTargets(matching: "").count == 1)
        #expect(tracker.suggestedTargets(matching: "onlgraph").count == 1)
        #expect(tracker.suggestedTargets(matching: "zzz").isEmpty)
    }
}
