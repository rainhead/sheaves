import Foundation
import Testing
@testable import SheavesCore

@Suite("TimeTracker")
@MainActor
struct TimeTrackerTests {
    /// Builds a tracker wired to scratch files, so tests never touch the real cache.
    private func makeTracker(transport: RoutingTransport, snapshotURL: URL? = nil) -> (TimeTracker, URL, URL) {
        let snapshotURL = snapshotURL
            ?? URL.temporaryDirectory.appending(path: "sheaves-snapshot-\(UUID().uuidString).json")
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

    @Test("keeps only the budgets it can actually draw")
    func syncKeepsReadableBudgets() async throws {
        // The running-timer account is on the one project the report budgets, which
        // is the case the panel actually draws.
        let transport = RoutingTransport.accountWithRunningTimer()
        let (tracker, snapshot, queue) = makeTracker(transport: transport)
        defer { cleanUp(snapshot, queue) }

        await tracker.sync()

        #expect(tracker.budgets.count == 1)
        let running = try #require(tracker.runningEntry)
        #expect(running.project.id == 14308069)
        #expect(tracker.budget(for: running)?.budgetRemaining == 8)
        // The project with no budget and the one whose figures came back null are
        // both absent, so nothing downstream has to tell them apart.
        #expect(tracker.budgets[14307913] == nil)
        #expect(tracker.budgets[14307915] == nil)
    }

    /// The Reports API allows 100 requests per 15 minutes, not per 15 seconds. A
    /// burst of start/stop clicks must not spend that allowance.
    @Test("does not refetch budgets on every sync")
    func cachesBudgets() async throws {
        let transport = RoutingTransport.standardAccount()
        let (tracker, snapshot, queue) = makeTracker(transport: transport)
        defer { cleanUp(snapshot, queue) }

        await tracker.sync()
        await tracker.sync()
        await tracker.sync()

        #expect(await transport.callCount(matching: "project_budget") == 1)
    }

    /// The absence rule: there is no lesser version of a budget display, so a token
    /// that may not read the report switches the feature off rather than leaving an
    /// empty frame. A refusal is the one settled answer — unlike an account that
    /// merely budgets nothing yet, it is never asked again.
    @Test("stops asking for budgets once Harvest refuses")
    func probesBudgetsOnce() async throws {
        let transport = RoutingTransport.standardAccount(budgetStatus: 403)
        let (tracker, snapshot, queue) = makeTracker(transport: transport)
        defer { cleanUp(snapshot, queue) }

        await tracker.sync()
        await tracker.sync()
        await tracker.sync()

        #expect(await transport.callCount(matching: "project_budget") == 1)
        #expect(tracker.budgets.isEmpty)
        // A refused report is not a broken sync.
        #expect(tracker.connection == .online)
    }

    @Test("shows nothing when the account budgets nothing")
    func accountWithoutBudgets() async throws {
        let transport = RoutingTransport.standardAccount(budgets: Fixture.noProjectBudgetsPage)
        let (tracker, snapshot, queue) = makeTracker(transport: transport)
        defer { cleanUp(snapshot, queue) }

        await tracker.sync()

        #expect(tracker.budgets.isEmpty)
        let entry = try #require(tracker.entries.first)
        #expect(tracker.budget(for: entry) == nil)
        #expect(tracker.connection == .online)
    }

    /// A budget is a decoration. Failing to load one must not tell the user their
    /// time tracking is offline, and must not stop Sheaves trying again.
    @Test("a failed budget report neither breaks nor ends the sync")
    func budgetFailureIsHarmless() async throws {
        let transport = RoutingTransport.standardAccount(budgetStatus: 500)
        let (tracker, snapshot, queue) = makeTracker(transport: transport)
        defer { cleanUp(snapshot, queue) }

        await tracker.sync()
        #expect(tracker.connection == .online)
        #expect(tracker.budgets.isEmpty)
        #expect(tracker.entries.count == 1)

        // Transient, so unlike a refusal it is retried.
        let firstRun = await transport.callCount(matching: "project_budget")
        await tracker.sync()
        #expect(await transport.callCount(matching: "project_budget") > firstRun)
    }

    @Test("draws a budget from the cache before the first reply arrives")
    func restoresBudgetsFromSnapshot() async throws {
        let transport = RoutingTransport.standardAccount()
        let (tracker, snapshot, queue) = makeTracker(transport: transport)
        defer { cleanUp(snapshot, queue) }
        await tracker.sync()

        let (restored, _, secondQueue) = makeTracker(transport: transport, snapshotURL: snapshot)
        defer { cleanUp(secondQueue) }
        await restored.bootstrap()

        #expect(restored.budgets[14308069]?.budgetRemaining == 8)
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
        // Assert the create actually happened. Counting requests would pass even if
        // the mutation had been silently discarded, since every sync issues GETs.
        let creates = await transport.calls(method: "POST", containing: "time_entries")
        #expect(creates.count == 1)
        #expect(creates.first?.projectID == target.project.id)
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
        // The queued start must have been *sent*, not dropped: pendingCount reaching
        // zero is also what discarding it would look like.
        #expect(await reconnected.calls(method: "POST", containing: "time_entries").count == 1)
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
}

@Suite("Cached snapshots")
struct SnapshotStoreTests {
    /// A cache is the thing that should degrade rather than fail. Adding `budgets`
    /// made the synthesized decoder reject every snapshot written before it, which
    /// threw away the entries, the recents and the usage ranking too — on exactly the
    /// launch where the app most wants them.
    @Test("reads a snapshot written before budgets existed")
    func decodesPreBudgetSnapshot() throws {
        let url = URL.temporaryDirectory.appending(path: "sheaves-old-\(UUID().uuidString).json")
        defer { try? FileManager.default.removeItem(at: url) }
        try Data("""
        {
          "targets": [],
          "entries": [],
          "recentTargetIDs": ["1:10"],
          "frequentTargetIDs": [],
          "savedAt": 776000000
        }
        """.utf8).write(to: url)

        let restored = try #require(SnapshotStore(fileURL: url).load())

        #expect(restored.recentTargetIDs == ["1:10"])
        #expect(restored.budgets.isEmpty)
    }

    @Test("round-trips budgets")
    func roundTripsBudgets() throws {
        let url = URL.temporaryDirectory.appending(path: "sheaves-new-\(UUID().uuidString).json")
        defer { try? FileManager.default.removeItem(at: url) }
        let store = SnapshotStore(fileURL: url)

        try store.save(CachedSnapshot(budgets: [
            ProjectBudget(
                projectID: 7, projectName: "P", budgetBy: .project,
                budget: 40, budgetSpent: 32, budgetRemaining: 8
            )
        ]))

        #expect(try #require(store.load()).budgets.first?.budgetRemaining == 8)
    }
}

@Suite("Budget currencies")
@MainActor
struct BudgetCurrencyTests {
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

    /// An account budgeting only in hours never needs a currency, so it must not spend
    /// a request — and a 403 on clients is exactly what a regular user gets.
    @Test("does not ask for clients when no budget is money")
    func skipsCurrencyForHoursBudgets() async throws {
        let transport = RoutingTransport.standardAccount()
        let (tracker, snapshot, queue) = makeTracker(transport: transport)
        defer { cleanUp(snapshot, queue) }

        await tracker.sync()

        // The fixture's only readable budget is measured in hours.
        #expect(tracker.budgets[14308069]?.budgetBy == .project)
        #expect(await transport.callCount(matching: "v2/clients") == 0)
    }

    @Test("labels a monetary budget with its client's currency")
    func joinsCurrency() async throws {
        let transport = RoutingTransport.standardAccount(budgets: Fixture.monetaryBudgetPage)
        let (tracker, snapshot, queue) = makeTracker(transport: transport)
        defer { cleanUp(snapshot, queue) }

        await tracker.sync()

        // Client 5735776 bills in EUR, whatever the machine running this is set to.
        #expect(tracker.budgets[14308069]?.currencyCode == "EUR")
        #expect(await transport.callCount(matching: "v2/clients") == 1)
    }

    /// The permission for clients is nearly the one a monetary budget already needs,
    /// but not exactly, so the refusal has to leave the budget usable.
    @Test("still shows a refused-currency budget, without a symbol")
    func survivesRefusedClients() async throws {
        let transport = RoutingTransport.standardAccount(
            budgets: Fixture.monetaryBudgetPage,
            clients: "{}",
            clientStatus: 403
        )
        let (tracker, snapshot, queue) = makeTracker(transport: transport)
        defer { cleanUp(snapshot, queue) }

        await tracker.sync()

        #expect(tracker.connection == .online)
        let budget = try #require(tracker.budgets[14308069])
        #expect(budget.currencyCode == nil)
        #expect(budget.budgetRemaining == 8)
        #expect(budget.formattedRemaining(.hoursMinutes, locale: Locale(identifier: "en_US")) == "8")

        // A refusal is a settled answer, so it is not asked again.
        await tracker.sync()
        #expect(await transport.callCount(matching: "v2/clients") == 1)
    }
}

@Suite("Reconnecting")
@MainActor
struct ReconnectTests {
    /// A 403 on the budget report is treated as final, which is right for one token and
    /// wrong across two. An expired token drops the connection to `.needsCredentials`
    /// without `disconnect` being called, so a session could reconnect with a better
    /// token and still never ask again.
    @Test("a new token starts the budget probe over")
    func connectResetsRefusals() async throws {
        let snapshotURL = URL.temporaryDirectory.appending(path: "sheaves-\(UUID().uuidString).json")
        let queueURL = URL.temporaryDirectory.appending(path: "sheaves-q-\(UUID().uuidString).json")
        defer { for u in [snapshotURL, queueURL] { try? FileManager.default.removeItem(at: u) } }

        // First token: allowed to track time, refused the budget report.
        let refusing = RoutingTransport.standardAccount(budgetStatus: 403)
        let tracker = TimeTracker(
            client: HarvestClient(credentials: Fixture.credentials, transport: refusing, backoffScale: 0),
            keychain: KeychainStore(service: "com.rainhead.Sheaves.tests-\(UUID().uuidString)"),
            snapshots: SnapshotStore(fileURL: snapshotURL),
            queue: MutationQueue(fileURL: queueURL)
        )
        await tracker.sync()
        await tracker.sync()
        #expect(tracker.budgets.isEmpty)
        #expect(await refusing.callCount(matching: "project_budget") == 1)

        try await tracker.connect(Fixture.credentials)

        // The same scripted account still refuses, but the probe was asked again
        // rather than being written off with the previous token.
        #expect(await refusing.callCount(matching: "project_budget") == 2)
    }

    /// Resetting the probe without clearing what it gates was the worse half of a fix:
    /// a budget refresh only overwrites on success, so a transient failure after
    /// connecting left the previous account's figures on screen under the new token.
    @Test("a new token does not inherit the last account's budgets")
    func connectClearsBudgets() async throws {
        let snapshotURL = URL.temporaryDirectory.appending(path: "sheaves-\(UUID().uuidString).json")
        let queueURL = URL.temporaryDirectory.appending(path: "sheaves-q-\(UUID().uuidString).json")
        defer { for u in [snapshotURL, queueURL] { try? FileManager.default.removeItem(at: u) } }

        // A cache standing in for the previous account, so there is something to inherit.
        try SnapshotStore(fileURL: snapshotURL).save(
            CachedSnapshot(budgets: [
                ProjectBudget(
                    projectID: 14308069, projectName: "Someone else's project",
                    budgetBy: .projectCost, budget: 5000, budgetSpent: 100,
                    budgetRemaining: 4900, currencyCode: "GBP"
                )
            ])
        )

        // New credentials, and a budget report that fails transiently rather than
        // answering — the case where nothing would overwrite what was cached.
        let transport = RoutingTransport.standardAccount(budgetStatus: 500)
        let tracker = TimeTracker(
            client: HarvestClient(credentials: Fixture.credentials, transport: transport, backoffScale: 0),
            keychain: KeychainStore(service: "com.rainhead.Sheaves.tests-\(UUID().uuidString)"),
            snapshots: SnapshotStore(fileURL: snapshotURL),
            queue: MutationQueue(fileURL: queueURL)
        )
        await tracker.bootstrap()
        #expect(tracker.budgets[14308069]?.currencyCode == "GBP", "the cache should have been restored")

        try await tracker.connect(Fixture.credentials)

        #expect(tracker.budgets.isEmpty)
        #expect(tracker.connection == .online)
    }
}

/// The background probe exists to catch changes made outside this app; these pin
/// how its cadence follows power and recent usage rather than the exact numbers,
/// which are tuning.
@Suite("Probe cadence")
struct ProbeCadenceTests {
    private var running: TimeTracker.Activity {
        .running(
            TrackedEntry(
                id: .server(1), project: Reference(id: 1, name: "P"),
                task: Reference(id: 2, name: "T"), spentDate: .today(),
                bankedHours: 0, isRunning: true, timerStartedAt: Date()
            )
        )
    }

    @Test("Low Power Mode stops the probe entirely")
    func lowPowerStopsProbing() {
        #expect(TimeTracker.probeInterval(for: running, on: .lowPower) == nil)
        #expect(TimeTracker.probeInterval(for: .idle, on: .lowPower) == nil)
    }

    @Test("a battery probes less often than the mains, never never")
    func batterySlowsButKeepsProbing() throws {
        let activeMains = try #require(TimeTracker.probeInterval(for: running, on: .pluggedIn))
        let activeBattery = try #require(TimeTracker.probeInterval(for: running, on: .battery))
        let idleMains = try #require(TimeTracker.probeInterval(for: .idle, on: .pluggedIn))
        let idleBattery = try #require(TimeTracker.probeInterval(for: .idle, on: .battery))
        #expect(activeBattery > activeMains)
        #expect(idleBattery > idleMains)
    }

    @Test("working hours probe more often than idle ones")
    func recentUsageQuickensProbing() throws {
        for power in [PowerState.pluggedIn, .battery] {
            let active = try #require(TimeTracker.probeInterval(for: running, on: power))
            let idle = try #require(TimeTracker.probeInterval(for: .idle, on: power))
            #expect(active < idle)
        }
    }

    @Test("a recently stopped timer counts as usage, not idleness")
    func recentTimerCountsAsUsage() throws {
        var entry = TrackedEntry(
            id: .server(1), project: Reference(id: 1, name: "P"),
            task: Reference(id: 2, name: "T"), spentDate: .today(),
            bankedHours: 0.5, isRunning: false
        )
        entry.updatedAt = Date()
        let recent = TimeTracker.Activity.recent(entry)
        let interval = try #require(TimeTracker.probeInterval(for: recent, on: .pluggedIn))
        let idle = try #require(TimeTracker.probeInterval(for: .idle, on: .pluggedIn))
        #expect(interval < idle)
    }
}
