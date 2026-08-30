import Foundation
import Testing
@testable import SheavesCore

@Suite("Grouping targets by project")
struct ProjectGroupingTests {
    private func target(_ project: Int, _ projectName: String, _ task: Int, _ taskName: String) -> TimerTarget {
        TimerTarget(
            project: Reference(id: project, name: projectName),
            task: Reference(id: task, name: taskName),
            client: Reference(id: 900 + project, name: "Client \(project)")
        )
    }

    /// The picker shows one row per project, so the pairs have to collapse without
    /// losing the ranking that put them in this order.
    @Test("collapses pairs into one entry per project, in the order they arrived")
    func groupsInRankedOrder() {
        let grouped = [
            target(2, "Second", 20, "Programming"),
            target(1, "First", 10, "Design"),
            target(2, "Second", 21, "QA"),
            target(1, "First", 11, "Research"),
        ].groupedByProject()

        #expect(grouped.map(\.project.name) == ["Second", "First"])
        #expect(grouped[0].tasks.map(\.name) == ["Programming", "QA"])
        #expect(grouped[1].tasks.map(\.name) == ["Design", "Research"])
        #expect(grouped[0].client?.name == "Client 2")
    }

    @Test("does not repeat a task the ranking offered twice")
    func dedupesTasks() {
        let grouped = [
            target(1, "First", 10, "Design"),
            target(1, "First", 10, "Design"),
            target(1, "First", 11, "Research"),
        ].groupedByProject()

        #expect(grouped.count == 1)
        #expect(grouped[0].tasks.map(\.name) == ["Design", "Research"])
    }

    @Test("carries a project id that a budget can be looked up by")
    func identifiedByProject() {
        let grouped = [target(1, "First", 10, "Design")].groupedByProject()
        #expect(grouped[0].id == 1)
        #expect(grouped[0].target(task: Reference(id: 10, name: "Design")).id == "1:10")
    }
}
