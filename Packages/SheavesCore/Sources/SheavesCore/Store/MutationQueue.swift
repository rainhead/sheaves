import Foundation

/// A change the user made locally that Harvest has not accepted yet.
public enum Mutation: Codable, Sendable, Hashable {
    case start(local: UUID, target: TimerTarget, spentDate: CalendarDate, notes: String?)
    case stop(TrackedEntry.ID)
    case restart(TrackedEntry.ID)
    case update(TrackedEntry.ID, notes: String?, hours: Double?)
    case delete(TrackedEntry.ID)

    /// The entry this mutation acts on, so the queue can rewrite local ids once
    /// the create that minted them has landed.
    var subject: TrackedEntry.ID {
        switch self {
        case .start(let local, _, _, _): .local(local)
        case .stop(let id), .restart(let id), .delete(let id): id
        case .update(let id, _, _): id
        }
    }

    func retargeted(to id: TrackedEntry.ID) -> Mutation {
        switch self {
        case .start: self
        case .stop: .stop(id)
        case .restart: .restart(id)
        case .update(_, let notes, let hours): .update(id, notes: notes, hours: hours)
        case .delete: .delete(id)
        }
    }
}

public struct DrainReport: Sendable {
    /// Mutations Harvest accepted.
    public var applied: Int = 0
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
    private let fileURL: URL

    public init(fileURL: URL) {
        self.fileURL = fileURL
        pending = (try? Data(contentsOf: fileURL))
            .flatMap { try? JSONDecoder().decode([Mutation].self, from: $0) } ?? []
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
        var report = DrainReport()

        while let mutation = pending.first {
            do {
                if case .start(let local, _, _, _) = mutation {
                    let entry = try await apply(mutation, using: client)
                    // Later queued changes still point at the placeholder id; repoint them.
                    if let entry {
                        rewrite(local: local, to: .server(entry.id))
                    }
                } else {
                    _ = try await apply(mutation, using: client)
                }
                pending.removeFirst()
                report.applied += 1
                persist()
            } catch let error as HarvestError where error.isTransient {
                report.stoppedWith = error
                break
            } catch let error as HarvestError {
                pending.removeFirst()
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

    private func apply(_ mutation: Mutation, using client: HarvestClient) async throws -> TimeEntry? {
        switch mutation {
        case .start(_, let target, let spentDate, let notes):
            return try await client.startTimeEntry(
                projectID: target.project.id,
                taskID: target.task.id,
                spentDate: spentDate,
                notes: notes
            )
        case .stop(let id):
            guard let serverID = id.serverID else { throw HarvestError.notFound }
            return try await client.stopTimeEntry(id: serverID)
        case .restart(let id):
            guard let serverID = id.serverID else { throw HarvestError.notFound }
            return try await client.restartTimeEntry(id: serverID)
        case .update(let id, let notes, let hours):
            guard let serverID = id.serverID else { throw HarvestError.notFound }
            return try await client.updateTimeEntry(id: serverID, notes: notes, hours: hours)
        case .delete(let id):
            guard let serverID = id.serverID else { throw HarvestError.notFound }
            try await client.deleteTimeEntry(id: serverID)
            return nil
        }
    }

    private func rewrite(local: UUID, to id: TrackedEntry.ID) {
        pending = pending.map { $0.subject == .local(local) ? $0.retargeted(to: id) : $0 }
    }

    private func persist() {
        guard let data = try? JSONEncoder().encode(pending) else { return }
        try? data.write(to: fileURL, options: .atomic)
    }
}
