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

    /// The project chosen, looked up by id and never by position: the list
    /// re-ranks under the open form whenever a timer starts or a sync lands, and
    /// the time must go where the form showed it would. One a sync has since taken
    /// away is nothing, so Log waits for another choice.
    private var project: ProjectTargets? {
        projectID.flatMap { id in projects.first { $0.id == id } }
    }

    /// The task chosen, on the same terms.
    private var task: Reference? {
        guard let project, let taskID else { return nil }
        return project.tasks.first { $0.id == taskID }
    }

    /// Chooses the best-ranked project and its best task, once: when the form
    /// opens, or when the projects first arrive if it opened before they had.
    private func chooseDefault() {
        guard projectID == nil, let first = projects.first else { return }
        choose(first)
    }

    /// Choosing a project chooses its best task at that moment, for the same
    /// reason the project is held by id.
    private func choose(_ project: ProjectTargets) {
        projectID = project.id
        taskID = project.tasks.first?.id
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

            // The rows GNOME's form has, in its order. The column style lines the
            // labels up on one edge and the controls on another, each row on its
            // first line of text.
            Form {
                LabeledContent("Project") { projectMenu }
                LabeledContent("Task") { taskMenu }
                // What the field takes is said beneath it, as a Mac form does, not
                // as a placeholder: an example of a time inside a time field reads as
                // a time already typed, and in a field this narrow it was cut off.
                LabeledContent("Time") {
                    VStack(alignment: .leading, spacing: 3) {
                        TextField("Time", text: $draftHours)
                            .labelsHidden()
                            .font(.callout.monospacedDigit())
                            .frame(width: 64)
                            .focused($focus, equals: .hours)
                            .accessibilityHint("Such as 1:30 or 1.5")
                            .overlay {
                                if hoursAreUnreadable {
                                    RoundedRectangle(cornerRadius: 5).stroke(.red.opacity(0.7))
                                }
                            }
                        Text("Such as 1:30 or 1.5")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }
                // Written as an entry's notes are in the list below: "Add notes" by
                // its bubble, with no label of its own. A field drawn that way rather
                // than a button that opens one, so Tab still reaches it from Time.
                LabeledContent {
                    HStack(spacing: 4) {
                        Image(systemName: "text.bubble")
                            .foregroundStyle(.tertiary)
                        TextField("Notes", text: $notes, prompt: Text("Add notes"))
                            .labelsHidden()
                            .textFieldStyle(.plain)
                            .focused($focus, equals: .notes)
                    }
                    .padding(.vertical, 3)
                    .padding(.horizontal, 4)
                    .overlay {
                        if focus == .notes {
                            RoundedRectangle(cornerRadius: 5).stroke(.separator)
                        }
                    }
                    .padding(.horizontal, -4)
                } label: {
                    EmptyView()
                }
            }
            .formStyle(.columns)
            .textFieldStyle(.roundedBorder)
            .font(.callout)
            .onSubmit(log)

            HStack {
                Spacer()
                Button("Cancel", action: onClose)
                Button("Log", action: log)
                    .buttonStyle(.borderedProminent)
                    .disabled(hours == nil || task == nil)
            }
            .controlSize(.small)
        }
        // On the whole form rather than its fields, so Escape closes the form even
        // when a menu or nothing at all has the focus; otherwise it reaches the
        // panel, which hides itself with the form still open inside it.
        .onExitCommand(perform: onClose)
        .onChange(of: projects) { chooseDefault() }
        .onAppear {
            chooseDefault()
            focus = .hours
        }
    }

    /// "Client · Project", in the menu and its items alike: the form's column has
    /// room for both, which the narrow control it replaced did not.
    private var projectMenu: some View {
        Menu {
            ForEach(projects) { option in
                Button {
                    choose(option)
                } label: {
                    if option.id == project?.id {
                        Label(option.label, systemImage: "checkmark")
                    } else {
                        Text(option.label)
                    }
                }
            }
        } label: {
            Text(project?.label ?? (projects.isEmpty ? "No projects" : "Choose a project"))
                .lineLimit(1)
        }
        .menuStyle(.button)
        // Its own height but not its own width, so a long name is cut short
        // rather than pushing the form past the panel's edge.
        .fixedSize(horizontal: false, vertical: true)
        .disabled(projects.isEmpty)
        .accessibilityLabel("Project")
        .accessibilityValue(project?.label ?? "")
    }

    /// A project with one task has nothing to choose, so it reads as a label, as it
    /// does in the project list below — unless the task chosen has gone, which
    /// leaves something to choose after all.
    @ViewBuilder
    private var taskMenu: some View {
        if let project, project.tasks.count > 1 || (task == nil && !project.tasks.isEmpty) {
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
                Text(task?.name ?? "Choose a task")
                    .lineLimit(1)
            }
            .menuStyle(.button)
            // Its own height but not its own width, so a long name is cut short
            // rather than pushing the form past the panel's edge.
            .fixedSize(horizontal: false, vertical: true)
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
