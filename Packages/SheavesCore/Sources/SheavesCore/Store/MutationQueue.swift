import Foundation

/// A change the user made locally that Harvest has not accepted yet.
///
/// Every case that affects elapsed time carries the time itself, not just the
/// command. Harvest's `/stop` banks time up to the moment the request *arrives*,
/// and a create starts its timer when it arrives too — so replaying bare commands
/// after a spell offline silently destroys or invents hours. Recording when the
/// user actually acted, and sending explicit hours, keeps the entry honest however
/// late the request lands.
public enum Mutation: Codable, Sendable, Hashable {
    /// `endedAt == nil` means the timer was still running when this was queued; the
    /// entry is created with the elapsed time so far and then resumed.
    case create(
        local: UUID,
        target: TimerTarget,
        spentDate: CalendarDate,
        notes: String?,
        startedAt: Date,
        endedAt: Date?
    )
    /// `hours` is the total measured locally when the user stopped the timer.
    case stop(TrackedEntry.ID, hours: Double)
    /// `bankedHours` is the total before resuming; `resumedAt` is when the user did.
    case restart(TrackedEntry.ID, resumedAt: Date, bankedHours: Double)
    case update(TrackedEntry.ID, notes: String?, hours: Double?)
    /// A new total typed onto a timer still running. Harvest sets the total and
    /// counts on from the moment the patch *arrives*, so `asOf` — when the user
    /// chose the number — is folded in as elapsed time however late that is.
    case adjust(TrackedEntry.ID, hours: Double, asOf: Date)
    case delete(TrackedEntry.ID)

    /// The entry this mutation acts on, so the queue can rewrite local ids once
    /// the create that minted them has landed.
    var subject: TrackedEntry.ID {
        switch self {
        case .create(let local, _, _, _, _, _): .local(local)
        case .stop(let id, _): id
        case .restart(let id, _, _): id
        case .update(let id, _, _): id
        case .adjust(let id, _, _): id
        case .delete(let id): id
        }
    }

    func retargeted(to id: TrackedEntry.ID) -> Mutation {
        switch self {
        case .create: self
        case .stop(_, let hours): .stop(id, hours: hours)
        case .restart(_, let resumedAt, let banked): .restart(id, resumedAt: resumedAt, bankedHours: banked)
        case .update(_, let notes, let hours): .update(id, notes: notes, hours: hours)
        case .adjust(_, let hours, let asOf): .adjust(id, hours: hours, asOf: asOf)
        case .delete: .delete(id)
        }
    }
}

public struct DrainReport: Sendable {
    /// Mutations Harvest accepted.
    public var applied: Int = 0
    /// Local ids that earned a Harvest id during this drain, so the store can adopt
    /// them without waiting for a refetch.
    public var resolved: [UUID: Int64] = [:]
    /// Mutations Harvest refused outright; these were dropped from the queue.
    public var discarded: [(mutation: Mutation, error: HarvestError)] = []
    /// Set when the queue stopped early because the failure looked temporary.
    public var stoppedWith: HarvestError?

    public var isEmpty: Bool { applied == 0 && discarded.isEmpty && stoppedWith == nil }
}

/// An ordered, persisted list of local changes waiting to reach Harvest.
///
/// Order matters — stopping a timer that a queued create has not yet made real would
/// otherwise fail — so the queue drains strictly in sequence and stops at the first
/// failure that a retry could fix. Failures a retry could *not* fix (a locked entry,
/// a deleted project) drop that one mutation and let the rest through, because
/// otherwise a single bad change would wedge every later one forever.
public actor MutationQueue {
    private var pending: [Mutation] = []
    /// Local ids that have earned a Harvest id, newest last.
    ///
    /// Rewriting only the mutations already queued is not enough: the user can stop a
    /// timer after its create drained but before the store has adopted the real id,
    /// and that stop would target a local id nothing recognises. It would be
    /// discarded as "not found" and the timer would run on Harvest indefinitely.
    private var resolved: [(local: UUID, serverID: Int64)] = []
    private var isDraining = false
    private let fileURL: URL
    /// Enough history to cover any plausible in-flight mutation without growing forever.
    private static let resolvedLimit = 200

    private struct Stored: Codable {
        var pending: [Mutation]
        var resolved: [Resolution]

        struct Resolution: Codable {
            var local: UUID
            var serverID: Int64
        }
    }

    public init(fileURL: URL) {
        self.fileURL = fileURL
        guard let data = try? Data(contentsOf: fileURL) else { return }
        if let stored = try? JSONDecoder().decode(Stored.self, from: data) {
            pending = stored.pending
            resolved = stored.resolved.map { ($0.local, $0.serverID) }
        } else if let legacy = try? JSONDecoder().decode([Mutation].self, from: data) {
            // A queue written before resolutions were recorded.
            pending = legacy
        }
    }

    public init(appName: String = "Sheaves") {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
            ?? URL.temporaryDirectory
        let directory = base.appending(path: appName, directoryHint: .isDirectory)
        try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        self.init(fileURL: directory.appending(path: "pending-mutations.json"))
    }

    public var count: Int { pending.count }
    public var isEmpty: Bool { pending.isEmpty }

    public func enqueue(_ mutation: Mutation) {
        pending.append(mutation)
        persist()
    }

    public func removeAll() {
        pending.removeAll()
        persist()
    }

    /// Sends queued mutations to Harvest in order, stopping at the first retryable failure.
    public func drain(using client: HarvestClient) async -> DrainReport {
        // The loop awaits the network, so an actor lets a second caller in partway
        // through. Two drains would read the same `pending.first`, send it twice,
        // and then each remove one entry — duplicating a mutation and losing its
        // successor.
        guard !isDraining else { return DrainReport() }
        isDraining = true
        defer { isDraining = false }

        var report = DrainReport()

        while let mutation = pending.first {
            do {
                if case .create(let local, _, _, _, _, _) = mutation {
                    // A create can need a second request — a restart, or a stop.
                    // Sending both inside one mutation meant a transient failure of
                    // the second repeated the first: one duplicate entry per retry,
                    // with the original left running. So the create is committed to
                    // the queue the moment the POST succeeds, and the second request
                    // takes its place as an ordinary mutation with ordinary retries.
                    let (entry, followUp) = try await create(mutation, using: client)
                    // Later queued changes still point at the placeholder id; repoint them.
                    remember(local: local, serverID: entry.id)
                    rewrite(local: local, to: .server(entry.id))
                    report.resolved[local] = entry.id
                    // The actor is free while the request is in flight, so the queue
                    // can have been emptied under it — `removeAll` on disconnect.
                    // Touch it only where the drained mutation still stands.
                    if pending.first == mutation {
                        if let followUp {
                            pending[0] = followUp
                        } else {
                            pending.removeFirst()
                        }
                    }
                } else {
                    _ = try await apply(mutation, using: client)
                    if pending.first == mutation { pending.removeFirst() }
                }
                report.applied += 1
                persist()
            } catch let error as HarvestError where error.isTransient {
                report.stoppedWith = error
                break
            } catch let error as HarvestError {
                if pending.first == mutation { pending.removeFirst() }
                report.discarded.append((mutation, error))
                persist()
            } catch is CancellationError {
                // Left queued deliberately; a cancelled drain is not a failed one.
                break
            } catch {
                report.stoppedWith = .invalidResponse
                break
            }
        }

        return report
    }

    /// Harvest keeps hours to two decimals, and its parser refuses the scientific
    /// notation that a tiny Double encodes to — a sub-second elapsed time comes
    /// back as "would create a negative total for the selected project and task".
    /// Every hours figure is therefore rounded to what Harvest can store before
    /// it is sent; two-decimal values always encode as plain decimals.
    static func storableHours(_ hours: Double) -> Double {
        (hours * 100).rounded() / 100
    }

    /// Sends the POST for a queued create and names the mutation that must follow
    /// it, if any. The caller queues that follow-up rather than this method sending
    /// it, so a failure between the two requests cannot repeat the first.
    private func create(
        _ mutation: Mutation,
        using client: HarvestClient
    ) async throws -> (TimeEntry, followUp: Mutation?) {
        guard case .create(_, let target, let spentDate, let notes, let startedAt, let endedAt) = mutation
        else { preconditionFailure("create(_:using:) requires a create mutation") }

        // Measure from when the user started, not from now.
        let finish = endedAt ?? Date()
        let hours = Self.storableHours(max(0, finish.timeIntervalSince(startedAt)) / 3600)
        guard hours > 0 else {
            // Nothing measurable to bank, and an explicit 0 would itself start
            // the timer. Create the entry running — which is what a fresh click
            // wants — and stop it again when the user already had.
            let entry = try await client.createTimeEntry(
                projectID: target.project.id,
                taskID: target.task.id,
                spentDate: spentDate,
                notes: notes,
                hours: nil
            )
            return (entry, endedAt == nil ? nil : .stop(.server(entry.id), hours: 0))
        }
        let entry = try await client.createTimeEntry(
            projectID: target.project.id,
            taskID: target.task.id,
            spentDate: spentDate,
            notes: notes,
            hours: hours
        )
        guard endedAt == nil else { return (entry, nil) }
        // Still running: the entry holds the offline time; the restart lets Harvest
        // count onwards, banking whatever more accrues before it lands.
        return (entry, .restart(.server(entry.id), resumedAt: Date(), bankedHours: hours))
    }

    private func apply(_ mutation: Mutation, using client: HarvestClient) async throws -> TimeEntry? {
        switch mutation {
        case .create:
            preconditionFailure("creates go through create(_:using:), which names their follow-up")

        case .stop(let id, let hours):
            guard let serverID = serverID(for: id) else { throw HarvestError.notFound }
            do {
                _ = try await client.stopTimeEntry(id: serverID)
            } catch HarvestError.rejected {
                // Already stopped server-side — the hours still need correcting.
            }
            return try await client.updateTimeEntry(id: serverID, hours: Self.storableHours(hours))

        case .restart(let id, let resumedAt, let bankedHours):
            guard let serverID = serverID(for: id) else { throw HarvestError.notFound }
            let offline = max(0, Date().timeIntervalSince(resumedAt)) / 3600
            if offline > 1.0 / 3600 {
                // Bank the time it ran offline *before* resuming, so the total is
                // never patched while the timer is live.
                _ = try await client.updateTimeEntry(
                    id: serverID,
                    hours: Self.storableHours(bankedHours + offline)
                )
            }
            return try await client.restartTimeEntry(id: serverID)

        case .update(let id, let notes, let hours):
            guard let serverID = serverID(for: id) else { throw HarvestError.notFound }
            return try await client.updateTimeEntry(
                id: serverID,
                notes: notes,
                hours: hours.map(Self.storableHours)
            )

        case .adjust(let id, let hours, let asOf):
            guard let serverID = serverID(for: id) else { throw HarvestError.notFound }
            let elapsed = max(0, Date().timeIntervalSince(asOf)) / 3600
            return try await client.updateTimeEntry(
                id: serverID,
                hours: Self.storableHours(hours + elapsed)
            )

        case .delete(let id):
            guard let serverID = serverID(for: id) else { throw HarvestError.notFound }
            try await client.deleteTimeEntry(id: serverID)
            return nil
        }
    }

    /// Marks a queued create as finished (or running again) without adding a second
    /// mutation, for a timer started and stopped before the queue ever drained.
    /// Returns false when no such create is pending.
    public func amendCreate(local: UUID, endedAt: Date?) -> Bool {
        let match = pending.firstIndex { mutation in
            if case .create(let id, _, _, _, _, _) = mutation { return id == local }
            return false
        }
        guard let index = match,
              case .create(let id, let target, let date, let notes, let startedAt, _) = pending[index]
        else { return false }

        pending[index] = .create(
            local: id,
            target: target,
            spentDate: date,
            notes: notes,
            startedAt: startedAt,
            endedAt: endedAt
        )
        persist()
        return true
    }

    private func rewrite(local: UUID, to id: TrackedEntry.ID) {
        pending = pending.map { $0.subject == .local(local) ? $0.retargeted(to: id) : $0 }
    }

    private func persist() {
        let stored = Stored(
            pending: pending,
            resolved: resolved.map { Stored.Resolution(local: $0.local, serverID: $0.serverID) }
        )
        guard let data = try? JSONEncoder().encode(stored) else { return }
        try? data.write(to: fileURL, options: .atomic)
    }

    /// The Harvest id a local id earned, if this queue has seen it created.
    private func serverID(for id: TrackedEntry.ID) -> Int64? {
        switch id {
        case .server(let serverID): return serverID
        case .local(let uuid): return resolved.last { $0.local == uuid }?.serverID
        }
    }

    private func remember(local: UUID, serverID: Int64) {
        resolved.removeAll { $0.local == local }
        resolved.append((local, serverID))
        if resolved.count > Self.resolvedLimit {
            resolved.removeFirst(resolved.count - Self.resolvedLimit)
        }
    }
}
