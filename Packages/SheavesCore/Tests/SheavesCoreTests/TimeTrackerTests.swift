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
        // Two reads per sync (the day, and whatever is running), plus the one-off
        // history fetch that ranks the target list.
        #expect(await transport.callCount(matching: "time_entries") == 7)
    }

    /// The whole point of the local-first model: the timer shows up even though
    /// Harvest never hears about it.
    @Test("shows a started timer while Harvest is unreachable")
    func startIsOptimistic() async throws {
        let transport = RoutingTransport.standardAccount()
        let (tracker, snapshot, queue) = makeTracker(transport: transport)
        defer { cleanUp(snapshot, queue) }
        await tracker.sync()
        let target = try #require(tracker.targets.first)
        await transport.goOffline()

        await tracker.start(target, notes: "spike")

        let running = try #require(tracker.runningEntry)
        #expect(running.task.name == target.task.name)
        #expect(running.notes == "spike")
        #expect(running.isPending)
        #expect(running.id.serverID == nil)
        #expect(tracker.recentTargets.first?.id == target.id)
        #expect(tracker.pendingCount == 1)
    }

    @Test("adopts Harvest's id once the create lands")
    func startReconcilesWithServer() async throws {
        let transport = RoutingTransport.standardAccount()
        let (tracker, snapshot, queue) = makeTracker(transport: transport)
        defer { cleanUp(snapshot, queue) }
        await tracker.sync()
        let target = try #require(tracker.targets.first)

        await tracker.start(target)

        // Nothing is left owing, and no local placeholder survives the refresh.
        #expect(tracker.pendingCount == 0)
        #expect(tracker.entries.allSatisfy { $0.id.serverID != nil })
        #expect(await transport.callCount(matching: "time_entries") > 0)
    }

    /// Harvest allows one running timer per user; the UI must not imply otherwise.
    @Test("never shows two timers running at once")
    func startStopsThePreviousTimer() async throws {
        let transport = RoutingTransport.accountWithRunningTimer()
        let (tracker, snapshot, queue) = makeTracker(transport: transport)
        defer { cleanUp(snapshot, queue) }
        await tracker.sync()
        #expect(tracker.runningEntry != nil)
        let target = try #require(tracker.targets.first)
        await transport.goOffline()

        await tracker.start(target)

        #expect(tracker.entries.filter(\.isRunning).count == 1)
        #expect(tracker.runningEntry?.task.name == target.task.name)
    }

    @Test("banks elapsed time when a timer is stopped")
    func stopBanksElapsedTime() async throws {
        let transport = RoutingTransport.accountWithRunningTimer()
        let (tracker, snapshot, queue) = makeTracker(transport: transport)
        defer { cleanUp(snapshot, queue) }
        await tracker.sync()
        let entry = try #require(tracker.runningEntry)
        await transport.goOffline()

        await tracker.stop(entry)

        let stopped = try #require(tracker.entries.first { $0.id == entry.id })
        #expect(!stopped.isRunning)
        #expect(stopped.timerStartedAt == nil)
        // The fixture banked an hour before the timer was started.
        #expect(stopped.bankedHours >= 1.0)
    }

    @Test("resumes an existing entry rather than creating a duplicate")
    func startResumesMatchingEntry() async throws {
        let transport = RoutingTransport.standardAccount()
        let (tracker, snapshot, queue) = makeTracker(transport: transport)
        defer { cleanUp(snapshot, queue) }
        await tracker.sync()
        let existing = try #require(tracker.entries.first)
        let before = tracker.entries.count
        await transport.goOffline()

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

    @Test("queues changes it could not deliver, and sends them on reconnect")
    func queuesUndeliverableChanges() async throws {
        let transport = RoutingTransport.standardAccount()
        let (tracker, snapshot, queue) = makeTracker(transport: transport)
        defer { cleanUp(snapshot, queue) }
        await tracker.sync()
        let target = try #require(tracker.targets.first)
        await transport.goOffline()

        await tracker.start(target)
        #expect(tracker.pendingCount == 1)

        // A queue that never drains would make offline support a lie.
        let reconnected = RoutingTransport.standardAccount()
        let recovered = TimeTracker(
            client: HarvestClient(credentials: Fixture.credentials, transport: reconnected, backoffScale: 0),
            keychain: KeychainStore(service: "com.rainhead.Sheaves.tests-\(UUID().uuidString)"),
            snapshots: SnapshotStore(fileURL: snapshot),
            queue: MutationQueue(fileURL: queue)
        )
        await recovered.sync()

        #expect(recovered.pendingCount == 0)
        #expect(await reconnected.callCount(matching: "time_entries") > 0)
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

    @Test("says which field it could not read")
    func reportsDecodingFailures() async throws {
        let transport = RoutingTransport([
            RoutingTransport.Route(method: "GET", fragment: "users/me", body: #"{"id": 1}"#)
        ])
        let (tracker, snapshot, queue) = makeTracker(transport: transport)
        defer { cleanUp(snapshot, queue) }

        await tracker.sync()

        guard case .offline(let reason) = tracker.connection else {
            Issue.record("expected an offline connection state, got \(tracker.connection)")
            return
        }
        #expect(reason.contains("firstName"))
        #expect(reason.contains("users/me"))
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
