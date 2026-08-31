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
    /// Budgets for the projects that have one this token may read, keyed by project
    /// id. Permanently empty on accounts with none — see `refreshBudgetsIfStale`.
    public private(set) var budgets: [Int: ProjectBudget] = [:]
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

    /// How long the visible day may go unchecked before the app asks Harvest
    /// uninvited, or nil when it should not ask at all.
    ///
    /// Every user action already syncs on the spot; the probe exists to catch
    /// changes made elsewhere — the web timesheet, a phone. Its cadence follows
    /// what the request costs: someone mid-workday sees outside edits land within
    /// a minute, a machine idle for hours checks occasionally, a battery slows
    /// both, and Low Power Mode is the user saying to spend nothing — the probe
    /// stops and syncing goes back to being purely event-driven.
    nonisolated static func probeInterval(for activity: Activity, on power: PowerState) -> TimeInterval? {
        switch (power, activity) {
        case (.lowPower, _): nil
        case (.pluggedIn, .idle): 10 * 60
        case (.pluggedIn, _): 60
        case (.battery, .idle): 30 * 60
        case (.battery, _): 5 * 60
        }
    }

    public var isToday: Bool { day == .today() }

    /// The budget to show beside `entry`, or nil when there is nothing to show.
    public func budget(for entry: TrackedEntry) -> ProjectBudget? {
        budget(forProject: entry.project.id)
    }

    public func budget(forProject id: Int) -> ProjectBudget? {
        budgets[id]
    }

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

    /// The same ranking, one row per project.
    public var orderedProjects: [ProjectTargets] {
        orderedTargets.groupedByProject()
    }

    // MARK: Collaborators

    /// Where the probe learns how the machine is powered. The Mac app swaps in an
    /// IOKit-backed answer at launch; the default can only see Low Power Mode, so
    /// it errs on treating a battery as plugged in.
    public var powerState: @MainActor () -> PowerState = {
        ProcessInfo.processInfo.isLowPowerModeEnabled ? .lowPower : .pluggedIn
    }

    private let client: HarvestClient
    private let keychain: KeychainStore
    private let snapshots: SnapshotStore
    private let queue: MutationQueue
    private var ticker: Task<Void, Never>?
    private var syncTask: Task<Void, Never>?
    /// When the last sync *began*, however it was triggered and however it ended.
    /// The probe paces itself by this rather than `lastSyncedAt` so a failing
    /// connection is retried on the probe's cadence, not on every tick.
    private var lastSyncStartedAt: Date?
    private var accountDataFetchedAt: Date?
    private var budgetAvailability: BudgetAvailability = .unknown
    /// Client currencies by client id, and whether Harvest refused to say. Only an
    /// administrator or a manager who may edit clients can read them — nearly the same
    /// permission a monetary budget already needs, so a token that can see money can
    /// usually also see what kind.
    private var currencies: [Int: String] = [:]
    private var currenciesRefused = false

    /// What the last budget probe learned, and when.
    ///
    /// Only a refusal is final. An account that budgets nothing today may budget
    /// something next week, and a menu bar app opened at login can run for weeks —
    /// so "none" is a slow question rather than a closed one.
    private enum BudgetAvailability {
        case unknown
        case some(asOf: Date)
        case none(asOf: Date)
        case refused
    }
    /// Set once the user navigates away from today, so the app stops following the
    /// calendar. Without this, a menu bar app left running for days keeps logging to
    /// whichever day it happened to launch on.
    private var isPinnedToDay = false
    private let maxRecents = 12
    /// Read with:
    /// `log show --last 10m --predicate 'subsystem == "com.rainhead.Sheaves"'`
    private static let log = Logger(subsystem: "com.rainhead.Sheaves", category: "sync")
    private static let accountDataLifetime: TimeInterval = 600
    /// Budgets come from the Reports API, whose allowance is 100 requests per 15
    /// *minutes*. Five minutes keeps the count in single digits per window however
    /// hard start and stop are hammered.
    private static let budgetLifetime: TimeInterval = 300
    /// How long an account with nothing to show is left alone. Long, because the
    /// answer rarely changes; finite, because it can.
    private static let emptyBudgetLifetime: TimeInterval = 3600
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
            forgetAccountData()
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
        await queue.removeAll()
        await client.setCredentials(nil)
        forgetAccountData()
        pendingCount = 0
        connection = .needsCredentials
    }

    /// Everything that describes whichever account was connected a moment ago.
    ///
    /// Shared by `disconnect` and `connect`, because credentials changing means the
    /// same thing in both directions: nothing already on screen is known to describe
    /// the new ones. Clearing only the probe state was worse than clearing none of it
    /// — the panel kept the previous account's budgets, and a transient failure on the
    /// next report left them there, since a budget refresh only overwrites on success.
    /// The cache goes too, or the same stale data returns at the next launch.
    private func forgetAccountData() {
        snapshots.clear()
        user = nil
        company = nil
        targets = []
        entries = []
        recentTargets = []
        frequentTargets = []
        budgets = [:]
        currencies = [:]
        currenciesRefused = false
        lastActiveToday = nil
        accountDataFetchedAt = nil
        budgetAvailability = .unknown
        isPinnedToDay = false
        lastSyncedAt = nil
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
        lastSyncStartedAt = Date()
        advanceDayIfNeeded()

        let report = await queue.drain(using: client)
        adopt(report.resolved)
        pendingCount = await queue.count
        if Task.isCancelled { return }
        if let blocker = report.stoppedWith {
            connection = .offline(reason: blocker.localizedDescription)
            return
        }
        for (mutation, error) in report.discarded {
            Self.log.error(
                "dropped refused mutation \(String(describing: mutation), privacy: .public): \(error.localizedDescription, privacy: .public)"
            )
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
            await refreshBudgetsIfStale()
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

    /// Budgets, on a clock of their own.
    ///
    /// Two things set this apart from the rest of the sync. The Reports API allows
    /// 100 requests per 15 minutes rather than per 15 seconds, so budgets refresh on
    /// their own slow timer instead of riding every start and stop. And a budget is
    /// a decoration: an account whose projects are all `budget_by: none`, or a token
    /// without the permission a monetary budget needs, has nothing to show — so an
    /// answer with nothing in it switches the feature off rather than leaving an
    /// empty frame in the panel, and how long it stays off depends on how settled
    /// the answer was. Nothing here can fail a sync, which is why it neither throws
    /// nor touches `connection`.
    private func refreshBudgetsIfStale() async {
        let now = Date()
        switch budgetAvailability {
        case .refused: return
        case .some(let asOf) where now.timeIntervalSince(asOf) < Self.budgetLifetime: return
        case .none(let asOf) where now.timeIntervalSince(asOf) < Self.emptyBudgetLifetime: return
        default: break
        }

        do {
            let report = try await client.projectBudgets()
            var readable = report.filter { $0.isActive && $0.hasReadableBudget }
            // Only when there is money to label. An account budgeting purely in hours
            // never needs the currency, so it never spends the request.
            if readable.contains(where: { $0.budgetBy.isMonetary }) {
                await refreshCurrencies()
                readable = readable.map { budget in
                    var budget = budget
                    budget.currencyCode = budget.clientID.flatMap { currencies[$0] }
                    return budget
                }
            }
            budgets = Dictionary(readable.map { ($0.projectID, $0) }, uniquingKeysWith: { first, _ in first })
            budgetAvailability = readable.isEmpty ? .none(asOf: Date()) : .some(asOf: Date())
            Self.log.info(
                "budgets: \(readable.count, privacy: .public) of \(report.count, privacy: .public) projects have one to show"
            )
        } catch is CancellationError {
            return
        } catch HarvestError.unauthorized {
            // The report is readable only by an administrator, or a manager holding
            // the billable-rates permission. A refusal is a settled answer rather
            // than a failure worth retrying: the sync above has already proved the
            // token good for everything else.
            budgets = [:]
            budgetAvailability = .refused
            Self.log.info("budgets: this token may not read them; the budget display stays off")
        } catch {
            // Anything else is transient. Leave the last known budgets on screen and
            // ask again on the next sync.
            Self.log.error("budgets: \(error.localizedDescription, privacy: .public)")
        }
    }

    /// The currency each client bills in.
    ///
    /// Harvest reports a currency only on the client record, so labelling a monetary
    /// budget costs a request the budget report itself cannot avoid. Clients change
    /// about never, so this rides the budget refresh rather than having a clock of its
    /// own, and a refusal is permanent: without it a monetary budget shows a bare
    /// number, which is incomplete where a guessed symbol would be wrong.
    private func refreshCurrencies() async {
        guard !currenciesRefused else { return }
        do {
            let records = try await client.clients()
            currencies = Dictionary(
                records.compactMap { record in record.currency.map { (record.id, $0) } },
                uniquingKeysWith: { first, _ in first }
            )
            Self.log.info("currencies: \(self.currencies.count, privacy: .public) clients name one")
        } catch is CancellationError {
            return
        } catch HarvestError.unauthorized {
            currenciesRefused = true
            Self.log.info("currencies: this token may not read clients; money shows no symbol")
        } catch {
            Self.log.error("currencies: \(error.localizedDescription, privacy: .public)")
        }
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
    ///
    /// The dedupe below can only see the visible day's list — which is why "start
    /// on today instead" goes to today *first*: started blind from a past day's
    /// view, it duplicated an entry the user already had today.
    public func start(_ target: TimerTarget, notes: String? = nil) async {
        if let existing = entries.first(where: { $0.target.id == target.id && $0.spentDate == day }) {
            await resume(existing)
            return
        }
        // A failed load leaves the visible list empty, and an empty list cannot
        // dedupe — so on today, fall back on the one entry the tracker always
        // remembers. Without this, "start on today" while offline duplicated an
        // entry that already existed on Harvest.
        if day == .today(), let recent = lastActiveToday,
           !recent.isRunning, recent.spentDate == day, recent.target.id == target.id {
            if !entries.contains(where: { $0.id == recent.id }) {
                entries.insert(recent, at: 0)
            }
            await resume(recent)
            return
        }

        await stopRunningForSwitch()
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
        await stopRunningForSwitch()
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
        let mutation = await recordStop(of: entry)
        noteLastActivity()
        restartClock()
        if let mutation {
            await enqueue(mutation)
        } else {
            // The stop was folded into a still-queued create.
            await queueChanged()
        }
    }

    /// Stops `entry` locally and returns the mutation that records it — nil when a
    /// still-queued create absorbed the stop instead.
    ///
    /// The total measured here is authoritative; Harvest's own stop timing is not,
    /// because the request may not reach it for hours. That is also why switching
    /// timers must queue this rather than lean on Harvest stopping the old one
    /// implicitly: the implicit stop banks time up to whenever the *next* request
    /// lands, inflating the old entry by however long the network was away.
    private func recordStop(of entry: TrackedEntry) async -> Mutation? {
        let stoppedAt = Date()
        let hours = entry.hours(asOf: stoppedAt)
        mutateLocally(entry.id) {
            $0.bankedHours = hours
            $0.isRunning = false
            $0.timerStartedAt = nil
            $0.isPending = true
        }
        if case .local(let uuid) = entry.id,
           await queue.amendCreate(local: uuid, endedAt: stoppedAt) {
            return nil
        }
        return .stop(entry.id, hours: hours)
    }

    /// The switch half of start and resume: whatever is running stops, measured
    /// now, and the recording mutation is queued ahead of whatever follows it.
    private func stopRunningForSwitch() async {
        guard let running = runningEntry else { return }
        if let mutation = await recordStop(of: running) {
            await queue.enqueue(mutation)
        }
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
        // Typing onto a running timer replaces the total and keeps it counting —
        // getting out of an untracked hour-long meeting and typing "1" must not
        // also stop the clock. Harvest agrees: patching a live entry's hours sets
        // the total and restarts its measurement from the moment the patch lands,
        // which is why the queued mutation carries when the user chose the number.
        guard !entry.isRunning else {
            let asOf = Date()
            mutateLocally(entry.id) {
                $0.bankedHours = hours
                $0.timerStartedAt = asOf
                $0.isPending = true
            }
            noteLastActivity()
            restartClock()
            await enqueue(.adjust(entry.id, hours: hours, asOf: asOf))
            return
        }
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

    // MARK: - Absences

    /// Applies what the user chose about a timer that ran while nobody was here.
    ///
    /// Every outcome is an ordinary mutation carrying explicit hours, which is the
    /// whole reason trimming needs no new machinery: the queue already knows how to
    /// send an honest total for work that finished long before the request lands.
    public func resolve(
        _ absence: Absence,
        on entry: TrackedEntry,
        as resolution: AbsenceResolution
    ) async {
        // The question can sit on screen for hours, and in that time the entry can be
        // stopped from a phone, edited on the web, or deleted. Answer for the entry
        // as it is now: the captured copy is only a record of what was asked about.
        // An entry that is gone, or no longer running, is no longer ours to trim.
        guard let entry = entries.first(where: { $0.id == entry.id }) else { return }
        guard !entry.isLocked else { return }
        // Nil means this machine has no standing to speak for the entry; see
        // `Absence.trimmedHours(for:)`.
        guard let trimmed = absence.trimmedHours(for: entry) else { return }

        switch resolution {
        case .keep:
            return
        case .trimAndContinue:
            await trim(entry, to: trimmed, leftAt: absence.began, resumingAt: absence.ended)
        case .trimAndStop:
            await trim(entry, to: trimmed, leftAt: absence.began, resumingAt: nil)
        case .trimAndLog(let target):
            // Carrying on would bank today's work onto a day that is over, so an
            // entry from an earlier day stops here just as `.trimAndStop` would.
            let resumesAt = entry.spentDate == .today() ? absence.ended : nil
            await trim(entry, to: trimmed, leftAt: absence.began, resumingAt: resumesAt)
            await log(absence, against: target)
        }
    }

    private func trim(
        _ entry: TrackedEntry,
        to hours: Double,
        leftAt: Date,
        resumingAt resumedAt: Date?
    ) async {
        mutateLocally(entry.id) {
            $0.bankedHours = hours
            $0.isRunning = resumedAt != nil
            $0.timerStartedAt = resumedAt
            $0.isPending = true
        }
        noteLastActivity()
        restartClock()

        // A create that has not reached Harvest yet carries its hours in its own
        // start and end, so the trim amends that create rather than correcting an
        // entry Harvest has never heard of.
        var amended = false
        if case .local(let uuid) = entry.id {
            amended = await queue.amendCreate(local: uuid, endedAt: leftAt)
        }

        // Harvest's copy of the timer is still running and still counting the
        // absence, so continuing is not one operation but three: stop it, correct
        // the total, start it again from the moment they came back.
        var mutations: [Mutation] = []
        if amended {
            // The create now ends where they left, but it still reports the hours as
            // the span from its start — which silently includes any pause taken
            // before the queue ever drained. Send the total we measured instead.
            mutations.append(.update(entry.id, notes: nil, hours: hours))
        } else {
            mutations.append(.stop(entry.id, hours: hours))
        }
        if let resumedAt {
            // Safe for an amended create too: the queue drains in order and rewrites
            // the local id once the create comes back with a real one.
            mutations.append(.restart(entry.id, resumedAt: resumedAt, bankedHours: hours))
        }
        // Queued together, then sent: enqueueing one at a time syncs in between, so
        // a quit in that window would persist a stop with no restart behind it and
        // leave a timer the user asked to keep running stopped instead.
        await enqueue(mutations)
    }

    /// Records the absence itself against another target — the meeting you were
    /// actually in while the timer sat on something else.
    private func log(_ absence: Absence, against target: TimerTarget) async {
        // The absence's own day. A timer left running overnight belongs to
        // yesterday; the meeting that interrupted it this morning does not.
        let spentDate = absence.day()
        let local = UUID()
        if spentDate == day {
            entries.insert(
                TrackedEntry(
                    id: .local(local),
                    project: target.project,
                    task: target.task,
                    client: target.client,
                    spentDate: spentDate,
                    bankedHours: absence.duration / 3600,
                    isRunning: false,
                    isPending: true
                ),
                at: 0
            )
        }
        noteRecent(target)
        await enqueue([
            .create(
                local: local,
                target: target,
                spentDate: spentDate,
                notes: nil,
                startedAt: absence.began,
                endedAt: absence.ended
            )
        ])
    }

    // MARK: - Local bookkeeping

    private func enqueue(_ mutation: Mutation) async {
        await enqueue([mutation])
    }

    /// Queues a whole sequence before draining, so a sequence that only makes sense
    /// together is never half-persisted.
    private func enqueue(_ mutations: [Mutation]) async {
        for mutation in mutations {
            await queue.enqueue(mutation)
        }
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
        // A local change is an update, and recency decides what the menu bar
        // offers: without this, stopping a long-running timer offline reads as
        // hours-old activity and the resume button vanishes. The next sync brings
        // Harvest's own timestamp, which says the same thing.
        entries[index].updatedAt = Date()
    }

    /// Harvest stops the running timer when another starts; mirror that locally so the
    /// UI never shows two clocks running at once.
    private func noteRecent(_ target: TimerTarget) {
        recentTargets.removeAll { $0.id == target.id }
        recentTargets.insert(target, at: 0)
        if recentTargets.count > maxRecents {
            recentTargets.removeLast(recentTargets.count - maxRecents)
        }
    }

    /// Swaps local placeholder ids for the Harvest ids their creates earned, without
    /// waiting for the refetch — otherwise a stop in that window targets an id the
    /// server has never heard of.
    private func adopt(_ resolved: [UUID: Int64]) {
        guard !resolved.isEmpty else { return }
        for index in entries.indices {
            guard case .local(let uuid) = entries[index].id,
                  let serverID = resolved[uuid]
            else { continue }
            entries[index].id = .server(serverID)
            entries[index].isPending = false
        }
        if case .local(let uuid) = lastActiveToday?.id, let serverID = resolved[uuid] {
            lastActiveToday?.id = .server(serverID)
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
        // Only today's running timer is today's activity: a past day's entry
        // resumed on its own day must not evict what the menu bar knows about
        // today, or stopping it strands a running copy nothing can resume.
        if let running = runningEntry {
            if running.spentDate == .today() {
                lastActiveToday = running
                return
            }
        }
        // Judged by the entry's own date, not the visible day: a timer started "on
        // today instead" lives in a past day's list, and stopping it there must
        // still leave it one click away — not strand a stale running copy here.
        let stopped = entries
            .filter { !$0.isRunning && $0.spentDate == .today() }
            .max { $0.updatedAt < $1.updatedAt }
        if isToday {
            // Today's list is authoritative, even about there being nothing.
            lastActiveToday = stopped
        } else if let stopped {
            // A past day's list can only add what it happens to know about today.
            lastActiveToday = stopped
        }
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
        // Restored so the panel draws a budget immediately, but deliberately without
        // restoring the probe: an empty cache cannot tell "this account has none"
        // from "not asked yet", so the next sync asks again.
        budgets = Dictionary(snapshot.budgets.map { ($0.projectID, $0) }, uniquingKeysWith: { first, _ in first })
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
                budgets: Array(budgets.values),
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
                } else if self.isProbeDue() {
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

    /// Whether the day has gone unverified long enough, given power and recent
    /// usage, to be worth a request nobody asked for.
    private func isProbeDue() -> Bool {
        // Not while one is in flight: syncs chain rather than cancel, so a slow
        // one would bank a queue of ticks to replay back-to-back on recovery.
        guard connection.isConfigured, syncTask == nil else { return false }
        guard let interval = Self.probeInterval(for: activity, on: powerState()) else { return false }
        return Date().timeIntervalSince(lastSyncStartedAt ?? .distantPast) >= interval
    }
}

/// How the machine is powered, as far as polite background traffic cares.
public enum PowerState: Sendable, Equatable {
    case pluggedIn
    case battery
    /// The user has asked the whole machine to conserve, so ambient network
    /// activity should stop rather than merely slow.
    case lowPower
}
