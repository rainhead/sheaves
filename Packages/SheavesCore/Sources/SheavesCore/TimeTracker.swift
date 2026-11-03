import Foundation
import Observation
import os

/// The Rust core's tracker, as SwiftUI observes it.
///
/// Every rule lives in the core (`crates/sheaves-core/src/tracker.rs`). This holds
/// the latest state the core reported and forwards each action, so views read the
/// same properties they did when the core was Swift.
@MainActor
@Observable
public final class TimeTracker {
    public typealias Activity = SheavesCore.Activity

    /// The latest state the core reported.
    public private(set) var snapshot: TrackerSnapshot

    @ObservationIgnored private let core: Tracker

    /// Where the probe learns how the machine is powered. Called on the core's own
    /// threads, so it must not touch the main actor.
    @ObservationIgnored public var powerState: @Sendable () -> PowerState = { .pluggedIn } {
        didSet { core.setPowerProbe(probe: PowerBridge(read: powerState)) }
    }

    /// A tracker on the real Harvest, keeping its files in Application Support and
    /// its token in the Keychain.
    public convenience init(keychain: KeychainStore = KeychainStore()) {
        self.init(keychain: keychain, demo: false)
    }

    /// A tracker on the made-up account the documentation images are drawn from,
    /// which touches neither Harvest nor any file the real app uses.
    public static func demo() -> TimeTracker {
        TimeTracker(
            keychain: KeychainStore(service: "com.rainhead.Sheaves.demo-\(UUID().uuidString)"),
            demo: true
        )
    }

    private init(keychain: KeychainStore, demo: Bool) {
        CoreLog.install()
        let bridge = ObserverBridge()
        let core = Tracker(
            dataDirectory: Self.dataDirectory.path(percentEncoded: false),
            demo: demo,
            credentials: keychain,
            observer: bridge
        )
        self.core = core
        self.snapshot = core.snapshot()
        bridge.tracker = self
    }

    private static var dataDirectory: URL {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
            ?? URL.temporaryDirectory
        return base.appending(path: "Sheaves", directoryHint: .isDirectory)
    }

    /// Keeps the newest state. The core numbers its states, and they can arrive out
    /// of order: a change reported from a background thread can land after the
    /// state an action read back on finishing.
    fileprivate func receive(_ newer: TrackerSnapshot) {
        guard newer.state.revision >= snapshot.state.revision else { return }
        snapshot = newer
    }

    /// Reads the core's state now, so the caller of an action sees its effect when
    /// the action returns.
    private func refresh() {
        receive(core.snapshot())
    }

    // MARK: State

    public var connection: Connection { snapshot.state.connection }
    public var user: HarvestUser? { snapshot.state.user }
    public var company: HarvestCompany? { snapshot.state.company }
    public var targets: [TimerTarget] { snapshot.state.targets }
    public var entries: [TrackedEntry] { snapshot.state.entries }
    public var recentTargets: [TimerTarget] { snapshot.state.recentTargets }
    public var frequentTargets: [TimerTarget] { snapshot.state.frequentTargets }
    public var day: CalendarDate { snapshot.state.day }
    public var lastSyncedAt: Date? { snapshot.state.lastSyncedAt }
    public var pendingCount: Int { Int(snapshot.state.pendingCount) }
    public var budgets: [Int64: ProjectBudget] { snapshot.state.budgets }
    /// Advances while a timer runs, so durations stay live.
    public var now: Date { snapshot.state.now }
    public var lastActiveToday: TrackedEntry? { snapshot.state.lastActiveToday }
    public var runningEntry: TrackedEntry? { snapshot.runningEntry }
    public var activity: Activity { snapshot.activity }
    public var totalHours: Double { snapshot.totalHours }
    public var isToday: Bool { snapshot.isToday }
    public var orderedProjects: [ProjectTargets] { snapshot.orderedProjects }
    /// Something to tell the user that is not a failed sync, such as changes an
    /// earlier version left unsent.
    public var notice: String? { snapshot.notice }

    public func budget(for entry: TrackedEntry) -> ProjectBudget? {
        budget(forProject: entry.project.id)
    }

    public func budget(forProject id: Int64) -> ProjectBudget? {
        budgets[id]
    }

    // MARK: Actions

    public func bootstrap() async { await core.bootstrap(); refresh() }
    public func sync() async { await core.sync(); refresh() }
    public func disconnect() async { await core.disconnect(); refresh() }
    public func selectDay(_ day: CalendarDate) async { await core.selectDay(day: day); refresh() }
    public func goToToday() async { await core.goToToday(); refresh() }
    public func shiftDay(by days: Int) async { await core.shiftDay(days: Int32(days)); refresh() }
    public func start(_ target: TimerTarget, notes: String? = nil) async {
        await core.start(target: target, notes: notes); refresh()
    }
    public func resume(_ entry: TrackedEntry) async { await core.resume(entry: entry); refresh() }
    public func stop(_ entry: TrackedEntry) async { await core.stop(entry: entry); refresh() }
    public func toggle(_ entry: TrackedEntry) async { await core.toggle(entry: entry); refresh() }
    public func stopRunning() async { await core.stopRunning(); refresh() }
    public func updateNotes(_ entry: TrackedEntry, to notes: String) async {
        await core.updateNotes(entry: entry, notes: notes); refresh()
    }
    public func updateHours(_ entry: TrackedEntry, to hours: Double) async {
        await core.updateHours(entry: entry, hours: hours); refresh()
    }
    public func delete(_ entry: TrackedEntry) async { await core.delete(entry: entry); refresh() }
    public func resolve(
        _ absence: Absence,
        on entry: TrackedEntry,
        as resolution: AbsenceResolution
    ) async {
        await core.resolve(absence: absence, entry: entry, resolution: resolution); refresh()
    }

    // MARK: Peers

    /// The other machines to share presence with, by their names on the tailnet,
    /// such as `hostess`. With any named, this Mac also answers them, on its tailnet
    /// addresses only; with none, it neither asks nor answers.
    public func setPeers(_ names: [String]) { core.setPeers(names: names) }

    /// Where this Mac is listening for its peers, such as `100.101.102.103:47147`.
    /// Read when asked: the core looks for new tailnet addresses once a minute.
    public var listeningOn: [String] { core.listeningOn() }

    /// A presence fold that asks this tracker's peers before it reports an absence.
    public func presenceFold(threshold: TimeInterval, now: Date) -> PresenceFold {
        core.presence(threshold: threshold, now: now)
    }

    /// Whether the question about `absence` still applies to the timer running now.
    /// It stops applying once another machine trims the timer, or it is stopped.
    public func stillAsks(about absence: Absence) -> Bool {
        snapshot.state.asksAbout(absence: absence)
    }

    /// Thrown by `connect` when the queue still holds changes made under a different
    /// account or user. Sending them is not an option, so the choice is the user's:
    /// go back to the credentials that made them, or connect again with
    /// `discardingUnsentChanges`.
    public struct UnsentChangesBelongElsewhere: LocalizedError, Equatable {
        public let count: Int

        public var errorDescription: String? {
            "\(count.formatted()) unsent change\(count == 1 ? " was" : "s were") made under a different Harvest account."
        }
    }

    /// Why a connect failed, in words for Settings.
    public struct ConnectionRefused: LocalizedError, Equatable {
        public let message: String
        public var errorDescription: String? { message }
    }

    /// Verifies credentials against Harvest before storing them. A connect that
    /// fails changes nothing.
    public func connect(_ credentials: HarvestCredentials, discardingUnsentChanges: Bool = false) async throws {
        defer { refresh() }
        do {
            try await core.connect(credentials: credentials, discardingUnsentChanges: discardingUnsentChanges)
        } catch ConnectFailure.UnsentChangesBelongElsewhere(let count) {
            throw UnsentChangesBelongElsewhere(count: Int(count))
        } catch ConnectFailure.Failed(let message) {
            throw ConnectionRefused(message: message)
        }
    }
}

/// Carries the core's state reports, which arrive on its own threads, to the main
/// actor in the order they were sent.
private final class ObserverBridge: TrackerObserver, @unchecked Sendable {
    /// Set once, on the main actor, before any report can arrive.
    weak var tracker: TimeTracker?

    func stateChanged(snapshot: TrackerSnapshot) {
        DispatchQueue.main.async { [weak self] in
            MainActor.assumeIsolated { self?.tracker?.receive(snapshot) }
        }
    }
}

private final class PowerBridge: PowerProbe, Sendable {
    let read: @Sendable () -> PowerState

    init(read: @escaping @Sendable () -> PowerState) {
        self.read = read
    }

    func powerState() -> PowerState { read() }
}

/// Sends the core's log lines to the unified log, under the subsystem the app
/// already logs to:
/// `log show --last 10m --predicate 'subsystem == "com.rainhead.Sheaves"'`
private final class CoreLog: LogSink, Sendable {
    private let log = Logger(subsystem: "com.rainhead.Sheaves", category: "core")

    static func install() {
        installLogSink(sink: CoreLog())
    }

    func write(level: LogLevel, target: String, message: String) {
        switch level {
        case .error: log.error("\(target, privacy: .public): \(message, privacy: .public)")
        case .warn: log.warning("\(target, privacy: .public): \(message, privacy: .public)")
        case .info: log.info("\(target, privacy: .public): \(message, privacy: .public)")
        case .debug: log.debug("\(target, privacy: .public): \(message, privacy: .public)")
        }
    }
}
