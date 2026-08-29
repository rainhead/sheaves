import Foundation

/// Everything the UI needs to draw itself before the first network reply arrives.
public struct CachedSnapshot: Codable, Sendable {
    public var user: HarvestUser?
    public var company: HarvestCompany?
    public var targets: [TimerTarget]
    public var entries: [TrackedEntry]
    /// Most-recently-used target ids, newest first. Drives the palette's default order.
    public var recentTargetIDs: [String]
    public var savedAt: Date

    public init(
        user: HarvestUser? = nil,
        company: HarvestCompany? = nil,
        targets: [TimerTarget] = [],
        entries: [TrackedEntry] = [],
        recentTargetIDs: [String] = [],
        savedAt: Date = Date()
    ) {
        self.user = user
        self.company = company
        self.targets = targets
        self.entries = entries
        self.recentTargetIDs = recentTargetIDs
        self.savedAt = savedAt
    }
}

/// Reads and writes the cache as a single JSON file.
///
/// A whole-file rewrite is fine at this size — a busy year of one person's entries is
/// well under a megabyte — and it keeps the on-disk state impossible to half-update.
public struct SnapshotStore: Sendable {
    private let fileURL: URL

    public init(fileURL: URL) {
        self.fileURL = fileURL
    }

    public init(appName: String = "Sheaves") {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
            ?? URL.temporaryDirectory
        let directory = base.appending(path: appName, directoryHint: .isDirectory)
        try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        self.fileURL = directory.appending(path: "snapshot.json")
    }

    /// Returns nil rather than throwing: a missing or stale-format cache is not an error,
    /// it just means the UI starts empty and waits for the network.
    public func load() -> CachedSnapshot? {
        guard let data = try? Data(contentsOf: fileURL) else { return nil }
        return try? JSONDecoder().decode(CachedSnapshot.self, from: data)
    }

    public func save(_ snapshot: CachedSnapshot) throws {
        let data = try JSONEncoder().encode(snapshot)
        try data.write(to: fileURL, options: .atomic)
    }

    public func clear() {
        try? FileManager.default.removeItem(at: fileURL)
    }
}
