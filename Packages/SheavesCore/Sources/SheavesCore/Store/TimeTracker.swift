import Foundation
import Observation

/// The application's model: what is on the clock, what happened today, and what
/// Sheaves still owes Harvest.
///
/// Every user action lands in local state first and reaches Harvest afterwards, via
/// `MutationQueue`. That is what makes the menu bar feel instant and what makes it
/// keep working on a dropped connection — the cost is that local state is the truth
/// until a sync succeeds, at which point the server's answer replaces it wholesale.
@MainActor
@Observable
public final class TimeTracker {
    public enum Connection: Sendable, Equatable {
        case needsCredentials
        case connecting
        case online
        /// Showing cached data; `reason` explains why the last sync failed.
        case offline(reason: String)

        public var isConfigured: Bool { self != .needsCredentials }
    }

    // MARK: Observable state

    public private(set) var connection: Connection = .needsCredentials
    public private(set) var user: HarvestUser?
    public private(set) var company: HarvestCompany?
    public private(set) var targets: [TimerTarget] = []
    public private(set) var entries: [TrackedEntry] = []
    public private(set) var recentTargets: [TimerTarget] = []
    public private(set) var day: CalendarDate = .today()
    public private(set) var lastSyncedAt: Date?
    public private(set) var pendingCount: Int = 0
    /// Advances once a second while a timer runs, so durations stay live.
    public private(set) var now: Date = Date()

    public var runningEntry: TrackedEntry? {
        entries.first(where: \.isRunning)
    }

    public var isToday: Bool { day == .today() }

    /// Total hours for the visible day, counting a running timer up to `now`.
    public var totalHours: Double {
        entries.reduce(0) { $0 + $1.hours(asOf: now) }
    }

    /// Targets to offer first in the palette: recents, then everything else.
    public func suggestedTargets(matching query: String) -> [TimerTarget] {
        let ordered = recentTargets + targets.filter { recent in
            !recentTargets.contains(where: { $0.id == recent.id })
        }
        let trimmed = query.trimmingCharacters(in: .whitespaces)
        guard !trimmed.isEmpty else { return ordered }
        return ordered.filter { $0.searchText.fuzzyMatches(trimmed) }
    }

    // MARK: Collaborators

    private let client: HarvestClient
    private let keychain: KeychainStore
    private let snapshots: SnapshotStore
    private let queue: MutationQueue
    private var ticker: Task<Void, Never>?
    private var syncTask: Task<Void, Never>?
    private var accountDataFetchedAt: Date?
    private let maxRecents = 12
    private static let accountDataLifetime: TimeInterval = 600

    public init(
        client: HarvestClient = HarvestClient(),
        keychain: KeychainStore = KeychainStore(),
        snapshots: SnapshotStore = SnapshotStore(),
        queue: MutationQueue = MutationQueue()
    ) {
        self.client = client
        self.keychain = keychain
        self.snapshots = snapshots
        self.queue = queue
    }

    // MARK: - Lifecycle

    /// Restores cached state, then syncs. Safe to call more than once.
    public func bootstrap() async {
        applyCachedSnapshot()

        let credentials = try? keychain.read()
        guard let credentials, credentials.isComplete else {
            connection = .needsCredentials
            return
        }
        await client.setCredentials(credentials)
        await sync()
        startTicking()
    }

    /// Verifies credentials against Harvest before storing them, so a typo is caught
    /// at the point the user can still see what they pasted.
    public func connect(_ credentials: HarvestCredentials) async throws {
        connection = .connecting
        await client.setCredentials(credentials)
        do {
            let user = try await client.currentUser()
            try keychain.write(credentials)
            self.user = user
            await sync()
            startTicking()
        } catch {
            connection = .needsCredentials
            await client.setCredentials(nil)
            throw error
        }
    }

    public func disconnect() async {
        ticker?.cancel()
        syncTask?.cancel()
        try? keychain.delete()
        snapshots.clear()
        await queue.removeAll()
        await client.setCredentials(nil)
        user = nil
        company = nil
        targets = []
        entries = []
        recentTargets = []
        accountDataFetchedAt = nil
        pendingCount = 0
        lastSyncedAt = nil
        connection = .needsCredentials
    }

    // MARK: - Syncing

    /// Flushes queued changes, then reloads the visible day from Harvest.
    ///
    /// Syncs are serialised rather than cancelled: a burst of clicks each enqueue a
    /// mutation, and tearing down a drain half way through would report a failure
    /// that never happened.
    public func sync() async {
        if let inFlight = syncTask {
            await inFlight.value
        }
        let task = Task { await performSync() }
        syncTask = task
        await task.value
        if syncTask == task { syncTask = nil }
    }

    private func performSync() async {
        guard await client.isConfigured else {
            connection = .needsCredentials
            return
        }

        let report = await queue.drain(using: client)
        pendingCount = await queue.count
        if Task.isCancelled { return }
        if let blocker = report.stoppedWith {
            connection = .offline(reason: blocker.localizedDescription)
            return
        }
        if let (_, error) = report.discarded.last {
            // A refused change is dropped rather than retried forever; say so, but
            // carry on — the refresh below will show what Harvest actually holds.
            connection = .offline(reason: error.localizedDescription)
        }

        do {
            let user: HarvestUser
            if let known = self.user {
                user = known
            } else {
                user = try await client.currentUser()
            }
            async let dayEntries = client.timeEntries(userID: user.id, from: day, to: day)
            async let running = client.runningTimeEntry(userID: user.id)

            self.user = user
            self.entries = try await merge(dayEntries: dayEntries, running: running)
            try await refreshAccountDataIfStale()
            self.lastSyncedAt = Date()
            self.connection = .online
            pruneRecents()
            saveSnapshot()
        } catch is CancellationError {
            // Shutting down or superseded; the queue is persisted, so nothing is lost.
            return
        } catch let error as HarvestError {
            connection = error.isTransient
                ? .offline(reason: error.localizedDescription)
                : (error == .unauthorized ? .needsCredentials : .offline(reason: error.localizedDescription))
        } catch {
            connection = .offline(reason: error.localizedDescription)
        }
    }

    /// Project assignments and company settings change rarely and cost two requests,
    /// so they are not refetched on every start/stop.
    private func refreshAccountDataIfStale() async throws {
        let isStale = accountDataFetchedAt.map { Date().timeIntervalSince($0) > Self.accountDataLifetime } ?? true
        guard isStale || targets.isEmpty else { return }

        async let company = client.company()
        async let assignments = client.projectAssignments()
        self.company = try await company
        self.targets = try await assignments.timerTargets()
        accountDataFetchedAt = Date()
    }

    /// A timer started on another day still belongs in the menu bar, so it is folded
    /// into the visible day's list even when it is not from that day.
    private func merge(dayEntries: [TimeEntry], running: TimeEntry?) -> [TrackedEntry] {
        var tracked = dayEntries.map { TrackedEntry($0) }
        if let running, !tracked.contains(where: { $0.id == .server(running.id) }) {
            tracked.insert(TrackedEntry(running), at: 0)
        }
        return tracked.sorted { lhs, rhs in
            if lhs.isRunning != rhs.isRunning { return lhs.isRunning }
            return lhs.updatedAt > rhs.updatedAt
        }
    }

    public func selectDay(_ newDay: CalendarDate) async {
        guard newDay != day else { return }
        day = newDay
        entries = []
        await sync()
    }

    public func goToToday() async { await selectDay(.today()) }

    public func shiftDay(by days: Int) async {
        await selectDay(day.adding(days: days))
    }

    // MARK: - Timer actions

    /// Starts (or resumes) a timer for `target` on the visible day.
    public func start(_ target: TimerTarget, notes: String? = nil) async {
        if let existing = entries.first(where: { $0.target.id == target.id && $0.spentDate == day }) {
            await resume(existing)
            return
        }

        stopRunningLocally()
        let local = UUID()
        entries.insert(
            TrackedEntry(
                id: .local(local),
                project: target.project,
                task: target.task,
                client: target.client,
                notes: notes,
                spentDate: day,
                bankedHours: 0,
                isRunning: true,
                timerStartedAt: Date(),
                isPending: true
            ),
            at: 0
        )
        noteRecent(target)
        await enqueue(.start(local: local, target: target, spentDate: day, notes: notes))
    }

    public func resume(_ entry: TrackedEntry) async {
        guard !entry.isRunning, entry.isLocked == false else { return }
        stopRunningLocally()
        mutateLocally(entry.id) {
            $0.isRunning = true
            $0.timerStartedAt = Date()
            $0.isPending = true
        }
        noteRecent(entry.target)
        await enqueue(.restart(entry.id))
    }

    public func stop(_ entry: TrackedEntry) async {
        guard entry.isRunning else { return }
        mutateLocally(entry.id) {
            $0.bankedHours = $0.hours(asOf: Date())
            $0.isRunning = false
            $0.timerStartedAt = nil
            $0.isPending = true
        }
        await enqueue(.stop(entry.id))
    }

    public func toggle(_ entry: TrackedEntry) async {
        entry.isRunning ? await stop(entry) : await resume(entry)
    }

    /// Stops whatever is running, wherever it is.
    public func stopRunning() async {
        guard let running = runningEntry else { return }
        await stop(running)
    }

    public func updateNotes(_ entry: TrackedEntry, to notes: String) async {
        guard !entry.isLocked else { return }
        mutateLocally(entry.id) {
            $0.notes = notes
            $0.isPending = true
        }
        await enqueue(.update(entry.id, notes: notes, hours: nil))
    }

    public func updateHours(_ entry: TrackedEntry, to hours: Double) async {
        guard !entry.isLocked else { return }
        mutateLocally(entry.id) {
            $0.bankedHours = hours
            $0.isRunning = false
            $0.timerStartedAt = nil
            $0.isPending = true
        }
        await enqueue(.update(entry.id, notes: nil, hours: hours))
    }

    public func delete(_ entry: TrackedEntry) async {
        guard !entry.isLocked else { return }
        entries.removeAll { $0.id == entry.id }
        await enqueue(.delete(entry.id))
    }

    // MARK: - Local bookkeeping

    private func enqueue(_ mutation: Mutation) async {
        await queue.enqueue(mutation)
        pendingCount = await queue.count
        saveSnapshot()
        await sync()
    }

    private func mutateLocally(_ id: TrackedEntry.ID, _ change: (inout TrackedEntry) -> Void) {
        guard let index = entries.firstIndex(where: { $0.id == id }) else { return }
        change(&entries[index])
    }

    /// Harvest stops the running timer when another starts; mirror that locally so the
    /// UI never shows two clocks running at once.
    private func stopRunningLocally() {
        guard let index = entries.firstIndex(where: \.isRunning) else { return }
        entries[index].bankedHours = entries[index].hours(asOf: Date())
        entries[index].isRunning = false
        entries[index].timerStartedAt = nil
    }

    private func noteRecent(_ target: TimerTarget) {
        recentTargets.removeAll { $0.id == target.id }
        recentTargets.insert(target, at: 0)
        if recentTargets.count > maxRecents {
            recentTargets.removeLast(recentTargets.count - maxRecents)
        }
    }

    /// Drops recents for projects the user is no longer assigned to.
    private func pruneRecents() {
        guard !targets.isEmpty else { return }
        let available = Set(targets.map(\.id))
        recentTargets.removeAll { !available.contains($0.id) }
    }

    // MARK: - Cache

    private func applyCachedSnapshot() {
        guard let snapshot = snapshots.load() else { return }
        user = snapshot.user
        company = snapshot.company
        targets = snapshot.targets
        recentTargets = snapshot.recentTargetIDs.compactMap { id in
            snapshot.targets.first { $0.id == id }
        }
        entries = snapshot.entries.filter { $0.spentDate == day || $0.isRunning }
        lastSyncedAt = snapshot.savedAt
    }

    private func saveSnapshot() {
        try? snapshots.save(
            CachedSnapshot(
                user: user,
                company: company,
                targets: targets,
                entries: entries,
                recentTargetIDs: recentTargets.map(\.id),
                savedAt: lastSyncedAt ?? Date()
            )
        )
    }

    // MARK: - Clock

    private func startTicking() {
        ticker?.cancel()
        ticker = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(1))
                guard let self else { return }
                self.now = Date()
            }
        }
    }
}

extension String {
    /// Subsequence match, the way a command palette filters: "acdes" finds "Acme / Design".
    func fuzzyMatches(_ query: String) -> Bool {
        var remaining = Substring(query.lowercased())
        for character in lowercased() where character == remaining.first {
            remaining = remaining.dropFirst()
            if remaining.isEmpty { return true }
        }
        return remaining.isEmpty
    }
}
