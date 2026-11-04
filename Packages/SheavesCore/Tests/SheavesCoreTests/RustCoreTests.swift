import Testing
import SheavesCore

@Suite("Rust core")
struct RustCoreTests {
    /// Calling across the boundary at all is the test: the generated Swift checks
    /// the library's interface checksums on first use and traps if the bindings
    /// were generated from a different build of the core.
    @Test("reports its name and version through the bindings")
    func version() {
        #expect(coreVersion().hasPrefix("sheaves-core "))
    }

    /// The whole path a view depends on: an action goes to the core, and its effect
    /// is on the wrapper when the action returns.
    @Test("syncs the demo account through the wrapper")
    @MainActor
    func demoTracker() async {
        let tracker = TimeTracker.demo()
        #expect(tracker.connection == .needsCredentials)

        await tracker.sync()

        #expect(tracker.connection == .online)
        #expect(tracker.entries.count == 2)
        #expect(tracker.runningEntry?.notes == "Checkout flow")
        #expect(tracker.orderedProjects.count == 3)
        #expect(tracker.budgets.values.contains { $0.isOverBudget })
    }

    /// The wrapper reaches the core with the entry and the target it was given.
    /// The demo account keeps no changes, so the refetch that follows the move puts
    /// the entry back where it was; what the move leaves behind is the target at
    /// the head of the recent ones. A start leaves the same, so this cannot tell
    /// the two apart: that the move queues an update carrying the new project and
    /// task is the core's test, in `tracker/tests.rs`.
    @Test("moves an entry to another project and task")
    @MainActor
    func retarget() async throws {
        let tracker = TimeTracker.demo()
        await tracker.sync()
        let entry = try #require(tracker.entries.first { !$0.isRunning })
        let elsewhere = try #require(tracker.orderedProjects.first { $0.id != entry.project.id })
        let target = elsewhere.target(task: try #require(elsewhere.tasks.last))
        #expect(tracker.recentTargets.first != target)

        await tracker.retarget(entry, to: target)

        #expect(tracker.recentTargets.first == target)
        let moved = try #require(tracker.entries.first { $0.id == entry.id })
        #expect(moved.bankedHours == entry.bankedHours)
        #expect(moved.notes == entry.notes)
    }

    /// Hours, target, day and notes each reach the core in their own place: the
    /// labels on the wrapper's arguments are the only thing keeping them apart.
    @Test("logs time on the day on screen without a timer")
    @MainActor
    func logTime() async throws {
        let tracker = TimeTracker.demo()
        await tracker.sync()
        let before = tracker.entries.map(\.id)
        let project = try #require(tracker.orderedProjects.last)
        let target = project.target(task: try #require(project.tasks.first))

        await tracker.logTime(1.5, on: target, day: tracker.day, notes: "Standup")

        let logged = try #require(tracker.entries.first { !before.contains($0.id) })
        #expect(logged.target == target)
        #expect(logged.bankedHours == 1.5)
        #expect(logged.notes == "Standup")
        #expect(!logged.isRunning)
        #expect(logged.spentDate == tracker.day)
    }

    @Test("labels a project the way an entry on it is labelled")
    func projectLabel() {
        let project = ProjectTargets(
            project: Reference(id: 1, name: "Website"),
            client: Reference(id: 2, name: "Acme"),
            tasks: [Reference(id: 3, name: "Design")]
        )
        #expect(project.label == "Acme · Website")
        #expect(project.label == project.target(task: project.tasks[0]).projectLabel)
    }
}
