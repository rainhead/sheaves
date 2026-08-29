import Foundation
import Observation
import os

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
    /// Ranked by what the user has actually logged in Harvest, best first.
    public private(set) var frequentTargets: [TimerTarget] = []
    public private(set) var day: CalendarDate = .today()
    public private(set) var lastSyncedAt: Date?
    public private(set) var pendingCount: Int = 0
    /// Advances once a second while a timer runs, so durations stay live.
    public private(set) var now: Date = Date()

    public var runningEntry: TrackedEntry? {
        entries.first(where: \.isRunning)
    }

    /// The last thing worked on today, whether or not it is still running.
    ///
    /// Held separately from `entries` because the popover can browse other days, and
    /// what the menu bar shows should not change just because the user looked at
    /// last Tuesday.
    public private(set) var lastActiveToday: TrackedEntry?

    /// What the menu bar should offer: a timer to pause, one to resume, or nothing.
    public var activity: Activity {
        if let running = runningEntry { return .running(running) }
        guard let recent = lastActiveToday,
              !recent.isRunning,
              now.timeIntervalSince(recent.updatedAt) <= Self.recentActivityWindow
        else { return .idle }
        return .recent(recent)
    }

    public enum Activity: Sendable, Equatable {
        case idle
        case running(TrackedEntry)
        /// Stopped, but recently enough that resuming it is the likely next action.
        case recent(TrackedEntry)

        public var entry: TrackedEntry? {
            switch self {
            case .idle: nil
            case .running(let entry), .recent(let entry): entry
            }
        }
    }

    /// How long a stopped timer stays one click away before the menu bar collapses
    /// back to a bare icon.
    public static let recentActivityWindow: TimeInterval = 90 * 60

    public var isToday: Bool { day == .today() }

    /// Total hours for the visible day, counting a running timer up to `now`.
    public var totalHours: Double {
        entries.reduce(0) { $0 + $1.hours(asOf: now) }
    }

    /// Targets in the order worth offering them: what was started in this app most
    /// recently, then what the user actually logs in Harvest, then the rest.
    ///
    /// Harvest hands back every task assigned to every project, alphabetically, which
    /// buries the two or three things someone does daily under ones they have never
    /// touched.
    public var orderedTargets: [TimerTarget] {
        // Only offer pairs still assigned to the user; history outlives assignments.
        let available = Dictionary(targets.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
        var seen = Set<String>()
        var ordered: [TimerTarget] = []
        for candidate in recentTargets + frequentTargets + targets {
            guard let target = available[candidate.id], seen.insert(target.id).inserted else { continue }
            ordered.append(target)
        }
        return ordered
    }

    public func suggestedTargets(matching query: String) -> [TimerTarget] {
        let trimmed = query.trimmingCharacters(in: .whitespaces)
        guard !trimmed.isEmpty else { return orderedTargets }
        return orderedTargets.filter { $0.searchText.fuzzyMatches(trimmed) }
    }

    // MARK: Collaborators

    private let client: HarvestClient
    private let keychain: KeychainStore
    private let snapshots: SnapshotStore
    private let queue: MutationQueue
    private var ticker: Task<Void, Never>?
    private var syncTask: Task<Void, Never>?
    private var accountDataFetchedAt: Date?
    /// Set once the user navigates away from today, so the app stops following the
    /// calendar. Without this, a menu bar app left running for days keeps logging to
    /// whichever day it happened to launch on.
    private var isPinnedToDay = false
    private let maxRecents = 12
    /// Read with:
    /// `log show --last 10m --predicate 'subsystem == "com.rainhead.Sheaves"'`
    private static let log = Logger(subsystem: "com.rainhead.Sheaves", category: "sync")
    private static let accountDataLifetime: TimeInterval = 600
    /// How far back to look when working out what the user actually works on.
    private static let historyWindowDays = 90

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
        frequentTargets = []
        lastActiveToday = nil
        accountDataFetchedAt = nil
        isPinnedToDay = false
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
        // Chain, do not merely await. Awaiting the in-flight task lets *every*
        // waiter past the guard at once, and each then starts its own performSync —
        // so two drains run concurrently, the queue actor interleaves them at its
        // await points, and the same mutation is sent twice while the next is
        // dropped. Building the next task around the previous one, and publishing
        // it before any suspension, makes the chain genuinely serial.
        let previous = syncTask
        let task = Task {
            await previous?.value
            await self.performSync()
        }
        syncTask = task
        await task.value
        if syncTask == task { syncTask = nil }
    }

    private func performSync() async {
        guard await client.isConfigured else {
            connection = .needsCredentials
            return
        }
        advanceDayIfNeeded()

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
            noteLastActivity()
            try await refreshAccountDataIfStale(userID: user.id)
            self.lastSyncedAt = Date()
            self.connection = .online
            pruneRecents()
            saveSnapshot()
            Self.log.info(
                "sync ok: \(self.entries.count, privacy: .public) entries, \(self.targets.count, privacy: .public) targets on \(self.day.description, privacy: .public)"
            )
        } catch is CancellationError {
            // Shutting down or superseded; the queue is persisted, so nothing is lost.
            return
        } catch let error as HarvestError {
            Self.log.error("sync failed: \(error.localizedDescription, privacy: .public)")
            connection = error.isTransient
                ? .offline(reason: error.localizedDescription)
                : (error == .unauthorized ? .needsCredentials : .offline(reason: error.localizedDescription))
        } catch {
            Self.log.error("sync failed: \(error.localizedDescription, privacy: .public)")
            connection = .offline(reason: error.localizedDescription)
        }
    }

    /// Project assignments and company settings change rarely and cost two requests,
    /// so they are not refetched on every start/stop.
    private func refreshAccountDataIfStale(userID: Int) async throws {
        let isStale = accountDataFetchedAt.map { Date().timeIntervalSince($0) > Self.accountDataLifetime } ?? true
        guard isStale || targets.isEmpty else { return }

        let today = CalendarDate.today()
        async let company = client.company()
        async let assignments = client.projectAssignments()
        async let history = client.timeEntries(
            userID: userID,
            from: today.adding(days: -Self.historyWindowDays),
            to: today
        )
        self.company = try await company
        let fetched = try await assignments
        self.targets = fetched.timerTargets()
        self.frequentTargets = Self.rankByUsage(try await history, asOf: today)
        accountDataFetchedAt = Date()
        Self.log.info(
            "account data: \(fetched.count, privacy: .public) assignments -> \(self.targets.count, privacy: .public) active targets, \(self.frequentTargets.count, privacy: .public) used in the last \(Self.historyWindowDays, privacy: .public) days"
        )
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
        isPinnedToDay = newDay != .today()
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
        noteLastActivity()
        restartClock()
        await enqueue(
            .create(
                local: local,
                target: target,
                spentDate: day,
                notes: notes,
                startedAt: Date(),
                endedAt: nil
            )
        )
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
        noteLastActivity()
        restartClock()

        // A timer that has not reached Harvest yet is resumed by amending its
        // queued create, not by queueing a restart of an entry that does not exist.
        if case .local(let uuid) = entry.id, await queue.amendCreate(local: uuid, endedAt: nil) {
            await queueChanged()
            return
        }
        await enqueue(.restart(entry.id, resumedAt: Date(), bankedHours: entry.bankedHours))
    }

    public func stop(_ entry: TrackedEntry) async {
        guard entry.isRunning else { return }
        let stoppedAt = Date()
        // The total measured here is authoritative; Harvest's own stop timing is not,
        // because the request may not reach it for hours.
        let hours = entry.hours(asOf: stoppedAt)
        mutateLocally(entry.id) {
            $0.bankedHours = hours
            $0.isRunning = false
            $0.timerStartedAt = nil
            $0.isPending = true
        }
        noteLastActivity()
        restartClock()

        if case .local(let uuid) = entry.id,
           await queue.amendCreate(local: uuid, endedAt: stoppedAt) {
            await queueChanged()
            return
        }
        await enqueue(.stop(entry.id, hours: hours))
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
        await queueChanged()
    }

    private func queueChanged() async {
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

    /// Rolls the visible day over at midnight, unless the user is deliberately
    /// looking at another day.
    private func advanceDayIfNeeded() {
        let today = CalendarDate.today()
        guard !isPinnedToDay, day != today else { return }
        day = today
        entries = []
        lastActiveToday = nil
    }

    /// Remembers the most recent thing worked on today, for the menu bar.
    private func noteLastActivity() {
        if let running = runningEntry {
            lastActiveToday = running
            return
        }
        // Only today's list can update this; browsing history must not disturb it.
        guard isToday else { return }
        lastActiveToday = entries
            .filter { !$0.isRunning }
            .max { $0.updatedAt < $1.updatedAt }
    }

    /// Ranks project/task pairs by how much the user really works on them.
    ///
    /// Frequency alone would pin last quarter's project to the top for weeks after it
    /// ended; recency alone would let one stray entry outrank a daily habit. Each
    /// entry contributes a weight that halves every few weeks, so the ordering
    /// follows what someone is working on now without forgetting the whole quarter.
    static func rankByUsage(_ entries: [TimeEntry], asOf today: CalendarDate) -> [TimerTarget] {
        var scores: [String: Double] = [:]
        var targets: [String: TimerTarget] = [:]

        for entry in entries {
            let target = TimerTarget(project: entry.project, task: entry.task, client: entry.client)
            let daysAgo = Double(max(0, today.daysSince(entry.spentDate)))
            scores[target.id, default: 0] += exp(-daysAgo / Double(halfLifeDays))
            targets[target.id] = target
        }

        return scores
            .sorted { lhs, rhs in
                // Ties sort by id so the order never shuffles between syncs.
                lhs.value == rhs.value ? lhs.key < rhs.key : lhs.value > rhs.value
            }
            .compactMap { targets[$0.key] }
    }

    private static let halfLifeDays = 21

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
        let byID = Dictionary(snapshot.targets.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
        recentTargets = snapshot.recentTargetIDs.compactMap { byID[$0] }
        frequentTargets = snapshot.frequentTargetIDs.compactMap { byID[$0] }
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
                frequentTargetIDs: frequentTargets.map(\.id),
                savedAt: lastSyncedAt ?? Date()
            )
        )
    }

    // MARK: - Clock

    private func startTicking() {
        ticker?.cancel()
        ticker = Task { [weak self] in
            while !Task.isCancelled {
                // Durations are shown to the minute, so a five-second tick is enough
                // to turn one over promptly. Idle, `now` only drives the "synced N
                // minutes ago" label, and redrawing the menu bar all day for that is
                // waste. `restartClock` re-picks this the instant a timer changes,
                // so starting one never waits out the idle interval.
                let interval: Duration = self?.runningEntry == nil ? .seconds(30) : .seconds(5)
                try? await Task.sleep(for: interval)
                guard let self else { return }
                self.now = Date()
                if !self.isPinnedToDay, self.day != CalendarDate.today() {
                    Task { await self.sync() }
                }
            }
        }
    }

    /// Advances the clock now and re-picks the tick interval.
    private func restartClock() {
        now = Date()
        startTicking()
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
