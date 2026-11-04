import SheavesCore
import SwiftUI

/// "Move To", in an entry's context menu: every project a timer can go on, with
/// its tasks beneath it, the entry's own project and task ticked.
///
/// A menu rather than a field in the row, because the Mac edits notes and time in
/// place and a project is not typed: it is picked from a list, which is what a
/// menu already is. A project with one task has nothing to choose beneath it, so
/// it is an item of its own rather than a submenu of one.
struct MoveMenu: View {
    @Environment(TimeTracker.self) private var tracker

    let entry: TrackedEntry

    var body: some View {
        Menu("Move To") {
            ForEach(tracker.orderedProjects) { project in
                if project.tasks.count == 1, let task = project.tasks.first {
                    choice(project.label, target: project.target(task: task))
                } else {
                    Menu {
                        ForEach(project.tasks) { task in
                            choice(task.name, target: project.target(task: task))
                        }
                    } label: {
                        ticked(project.label, when: project.id == entry.project.id)
                    }
                }
            }
        }
    }

    private func choice(_ title: String, target: TimerTarget) -> some View {
        Button {
            Task { await tracker.retarget(entry, to: target) }
        } label: {
            ticked(title, when: target.id == entry.target.id)
        }
    }

    @ViewBuilder
    private func ticked(_ title: String, when isCurrent: Bool) -> some View {
        if isCurrent {
            Label(title, systemImage: "checkmark")
        } else {
            Text(title)
        }
    }
}
