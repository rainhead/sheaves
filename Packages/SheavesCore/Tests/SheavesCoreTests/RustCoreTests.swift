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
}
