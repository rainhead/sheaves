import SheavesCore
import SwiftUI

/// Time done with no timer running — a meeting away from the Mac, or yesterday's
/// work recorded today — logged on the day on screen.
///
/// It opens in the panel under the day's header rather than in a window of its own,
/// so the day it will land on is the one in plain view above it. The panel's own
/// keys stand aside while it is open, as they do for a field in a row.
struct LogTimeForm: View {
    @Environment(TimeTracker.self) private var tracker

    /// What the header calls the day on screen: "Today", or the date.
    let dayTitle: String
    let onClose: () -> Void

    @State private var projectID: Int64?
    @State private var taskID: Int64?
    @State private var draftHours = ""
    @State private var notes = ""
    @FocusState private var focus: Field?

    private enum Field { case hours, notes }

    /// `hours` and `notes` start the fields with something typed, for the
    /// documentation images; the panel opens them empty.
    init(dayTitle: String, hours: String = "", notes: String = "", onClose: @escaping () -> Void) {
        self.dayTitle = dayTitle
        self.onClose = onClose
        _draftHours = State(initialValue: hours)
        _notes = State(initialValue: notes)
    }

    private var projects: [ProjectTargets] { tracker.orderedProjects }

    /// The project chosen, or the best-ranked one until something is.
    private var project: ProjectTargets? {
        projects.first { $0.id == projectID } ?? projects.first
    }

    /// The task chosen, as long as it belongs to the project; else its best one.
    private var task: Reference? {
        guard let project else { return nil }
        return project.tasks.first { $0.id == taskID } ?? project.tasks.first
    }

    private var hours: Double? {
        Double.hours(parsing: draftHours).flatMap { $0 > 0 ? $0 : nil }
    }

    /// Something typed that is not a length, which is worth saying while the field
    /// still has it, rather than leaving Log greyed out without a reason.
    private var hoursAreUnreadable: Bool {
        !draftHours.trimmingCharacters(in: .whitespaces).isEmpty && hours == nil
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Log time on \(dayTitle), with no timer")
                .font(.caption)
                .foregroundStyle(.secondary)

            Grid(alignment: .leading, horizontalSpacing: 8, verticalSpacing: 6) {
                GridRow {
                    Text("Project").foregroundStyle(.secondary)
                    projectMenu
                }
                GridRow {
                    Text("Task").foregroundStyle(.secondary)
                    taskMenu
                }
            }
            .font(.callout)

            HStack(spacing: 6) {
                TextField("1:30", text: $draftHours)
                    .textFieldStyle(.roundedBorder)
                    .font(.callout.monospacedDigit())
                    .multilineTextAlignment(.trailing)
                    .frame(width: 58)
                    .focused($focus, equals: .hours)
                    .accessibilityLabel("Time, such as 1:30 or 1.5")
                    .overlay {
                        if hoursAreUnreadable {
                            RoundedRectangle(cornerRadius: 5).stroke(.red.opacity(0.7))
                        }
                    }
                TextField("Notes", text: $notes)
                    .textFieldStyle(.roundedBorder)
                    .font(.callout)
                    .focused($focus, equals: .notes)
            }
            .onSubmit(log)
            .onExitCommand(perform: onClose)

            HStack {
                Spacer()
                Button("Cancel", action: onClose)
                Button("Log", action: log)
                    .buttonStyle(.borderedProminent)
                    .disabled(hours == nil || task == nil)
            }
            .controlSize(.small)
        }
        .onAppear { focus = .hours }
    }

    /// The project's name alone, with its client beneath: "Client · Project" in a
    /// control this narrow was cut off before the project began. The list it opens
    /// has the room, so its items carry both.
    private var projectMenu: some View {
        VStack(alignment: .leading, spacing: 0) {
            Menu {
                ForEach(projects) { option in
                    Button {
                        projectID = option.id
                        taskID = nil
                    } label: {
                        if option.id == project?.id {
                            Label(option.label, systemImage: "checkmark")
                        } else {
                            Text(option.label)
                        }
                    }
                }
            } label: {
                Text(project?.project.name ?? "No projects")
                    .lineLimit(1)
            }
            .menuStyle(.borderlessButton)
            .fixedSize()
            .disabled(projects.isEmpty)
            .accessibilityLabel("Project")
            .accessibilityValue(project?.label ?? "")

            if let client = project?.client?.name, !client.isEmpty {
                Text(client)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
        }
    }

    /// A project with one task has nothing to choose, so it reads as a label, as it
    /// does in the project list below.
    @ViewBuilder
    private var taskMenu: some View {
        if let project, project.tasks.count > 1 {
            Menu {
                ForEach(project.tasks) { option in
                    Button {
                        taskID = option.id
                    } label: {
                        if option.id == task?.id {
                            Label(option.name, systemImage: "checkmark")
                        } else {
                            Text(option.name)
                        }
                    }
                }
            } label: {
                Text(task?.name ?? "")
                    .lineLimit(1)
            }
            .menuStyle(.borderlessButton)
            .fixedSize()
            .accessibilityLabel("Task")
            .accessibilityValue(task?.name ?? "")
        } else {
            Text(task?.name ?? "")
                .lineLimit(1)
        }
    }

    /// On the day on screen when Log is pressed, which is the one named above.
    private func log() {
        guard let hours, let project, let task else { return }
        let target = project.target(task: task)
        let day = tracker.day
        let trimmed = notes.trimmingCharacters(in: .whitespacesAndNewlines)
        Task {
            await tracker.logTime(hours, on: target, day: day, notes: trimmed.isEmpty ? nil : trimmed)
        }
        onClose()
    }
}
