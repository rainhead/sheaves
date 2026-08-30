import Foundation

/// A time entry as Sheaves shows it, which is not quite what Harvest returns.
///
/// An entry started while offline has no Harvest id yet, so identity is either a
/// server id or a locally minted one. Everything downstream — the menu bar, the
/// day list, the mutation queue — can then treat the two alike, and the local id
/// is swapped for the real one once the create lands.
public struct TrackedEntry: Identifiable, Sendable, Hashable, Codable {
    public enum ID: Hashable, Sendable, Codable {
        case server(Int64)
        case local(UUID)

        public var serverID: Int64? {
            if case .server(let id) = self { return id }
            return nil
        }
    }

    public var id: ID
    public var project: Reference
    public var task: Reference
    public var client: Reference?
    public var notes: String?
    public var spentDate: CalendarDate
    /// Hours banked before the current run. A running entry adds live elapsed time on top.
    public var bankedHours: Double
    public var isRunning: Bool
    public var timerStartedAt: Date?
    public var isLocked: Bool
    public var isBillable: Bool
    /// True while a local change has not yet been accepted by Harvest.
    public var isPending: Bool
    public var updatedAt: Date

    public init(
        id: ID,
        project: Reference,
        task: Reference,
        client: Reference? = nil,
        notes: String? = nil,
        spentDate: CalendarDate,
        bankedHours: Double,
        isRunning: Bool,
        timerStartedAt: Date? = nil,
        isLocked: Bool = false,
        isBillable: Bool = true,
        isPending: Bool = false,
        updatedAt: Date = Date()
    ) {
        self.id = id
        self.project = project
        self.task = task
        self.client = client
        self.notes = notes
        self.spentDate = spentDate
        self.bankedHours = bankedHours
        self.isRunning = isRunning
        self.timerStartedAt = timerStartedAt
        self.isLocked = isLocked
        self.isBillable = isBillable
        self.isPending = isPending
        self.updatedAt = updatedAt
    }

    public init(_ entry: TimeEntry, isPending: Bool = false) {
        self.init(
            id: .server(entry.id),
            project: entry.project,
            task: entry.task,
            client: entry.client,
            notes: entry.notes,
            spentDate: entry.spentDate,
            bankedHours: entry.hoursWithoutTimer ?? (entry.isRunning ? 0 : entry.hours),
            isRunning: entry.isRunning,
            timerStartedAt: entry.timerStartedAt,
            isLocked: entry.isLocked,
            isBillable: entry.billable,
            isPending: isPending,
            updatedAt: entry.updatedAt
        )
    }

    /// Hours on the clock at `now` — banked time plus whatever the running timer has added.
    public func hours(asOf now: Date = Date()) -> Double {
        guard isRunning, let timerStartedAt else { return bankedHours }
        return bankedHours + max(0, now.timeIntervalSince(timerStartedAt)) / 3600
    }

    public var target: TimerTarget {
        TimerTarget(project: project, task: task, client: client)
    }
}

/// A project/task pair a timer can be started against — what the palette searches
/// and what "resume" resumes.
public struct TimerTarget: Identifiable, Sendable, Hashable, Codable {
    public var project: Reference
    public var task: Reference
    public var client: Reference?

    public init(project: Reference, task: Reference, client: Reference? = nil) {
        self.project = project
        self.task = task
        self.client = client
    }

    public var id: String { "\(project.id):\(task.id)" }

    /// "Client · Project" — the project alone is often ambiguous across clients.
    public var projectLabel: String {
        guard let client, !client.name.isEmpty else { return project.name }
        return "\(client.name) · \(project.name)"
    }
}

/// A project a timer can be started against, with the tasks available on it.
///
/// The picker lists projects rather than project/task pairs. A project with six
/// tasks was six rows carrying the same second line, which buried how few projects
/// there really are; the task is a choice made within the row instead.
public struct ProjectTargets: Identifiable, Sendable, Hashable {
    public var project: Reference
    public var client: Reference?
    /// Best first, by the same ranking that decides the order of the projects.
    public var tasks: [Reference]

    public init(project: Reference, client: Reference? = nil, tasks: [Reference]) {
        self.project = project
        self.client = client
        self.tasks = tasks
    }

    public var id: Int { project.id }

    /// The pair to start, given whichever task the row is showing.
    public func target(task: Reference) -> TimerTarget {
        TimerTarget(project: project, task: task, client: client)
    }
}

public extension Sequence where Element == TimerTarget {
    /// Collapses ranked pairs into one entry per project.
    ///
    /// Order is taken from the sequence on both axes: a project appears where its
    /// best-ranked task put it, and its tasks keep the order they were ranked in. So
    /// filtering pairs by a query and grouping afterwards leaves the task that
    /// matched at the head of its project's list, which is what lets typing a task
    /// name still choose that task.
    func groupedByProject() -> [ProjectTargets] {
        var order: [Int] = []
        var grouped: [Int: ProjectTargets] = [:]
        for target in self {
            let id = target.project.id
            if grouped[id] == nil {
                order.append(id)
                grouped[id] = ProjectTargets(project: target.project, client: target.client, tasks: [])
            }
            if !grouped[id]!.tasks.contains(where: { $0.id == target.task.id }) {
                grouped[id]!.tasks.append(target.task)
            }
        }
        return order.compactMap { grouped[$0] }
    }
}

public extension Sequence where Element == ProjectAssignment {
    /// Every active project/task pair the user may log against.
    func timerTargets() -> [TimerTarget] {
        flatMap { assignment -> [TimerTarget] in
            guard assignment.isActive else { return [] }
            let project = Reference(id: assignment.project.id, name: assignment.project.name)
            return assignment.taskAssignments
                .filter(\.isActive)
                .map { TimerTarget(project: project, task: $0.task, client: assignment.client) }
        }
    }
}
