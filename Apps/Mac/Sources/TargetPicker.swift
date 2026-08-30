import SheavesCore
import SwiftUI

/// The list of projects a timer can be started on.
///
/// Purely presentational: which row is highlighted and what the keyboard does are
/// the host's business, because the popover's arrow keys have to run through the
/// day's entries and these rows as one column. A picker that owned its own selection
/// could only ever be an island in the middle of that.
struct ProjectList: View {
    @Environment(TimeTracker.self) private var tracker

    var projects: [ProjectTargets]
    /// Index of the highlighted row, or nil when the selection is elsewhere.
    var highlighted: Int?
    @Binding var chosenTasks: [Int: Int]
    var maxVisible: Int = 6
    var onStart: (TimerTarget) -> Void

    private var format: HoursFormat { HoursFormat(company: tracker.company) }

    var body: some View {
        if projects.isEmpty {
            Text("No projects yet — sync with Harvest.")
                .font(.callout)
                .foregroundStyle(.secondary)
                .padding(.vertical, 8)
        } else {
            ScrollViewReader { proxy in
                SizedScrollView(maxHeight: CGFloat(maxVisible) * 40) {
                    LazyVStack(alignment: .leading, spacing: 0) {
                        ForEach(Array(projects.enumerated()), id: \.element.id) { index, entry in
                            TargetRow(
                                entry: entry,
                                isSelected: index == highlighted,
                                budget: tracker.budget(forProject: entry.id),
                                format: format,
                                task: binding(for: entry),
                                onStart: { start(entry) }
                            )
                            .id(entry.id)
                        }
                    }
                }
                .onChange(of: highlighted) {
                    guard let highlighted, projects.indices.contains(highlighted) else { return }
                    withAnimation(.easeOut(duration: 0.1)) { proxy.scrollTo(projects[highlighted].id) }
                }
            }
        }
    }

    private func binding(for entry: ProjectTargets) -> Binding<Reference?> {
        Binding(
            get: { entry.task(chosenFrom: chosenTasks) },
            set: { chosenTasks[entry.id] = $0?.id }
        )
    }

    private func start(_ entry: ProjectTargets) {
        guard let task = entry.task(chosenFrom: chosenTasks) else { return }
        onStart(entry.target(task: task))
    }
}

extension ProjectTargets {
    /// The task a row shows: whatever was picked by hand, as long as it is still on
    /// offer, and otherwise the best-ranked one.
    func task(chosenFrom chosen: [Int: Int]) -> Reference? {
        guard let id = chosen[self.id], let match = tasks.first(where: { $0.id == id }) else {
            return tasks.first
        }
        return match
    }
}

/// A `ProjectList` that carries its own keyboard, for the panels that show nothing
/// else selectable — the hotkey panel and the absence prompt. The popover does not
/// use this: its arrow keys span the day's entries too.
struct TargetPicker: View {
    @Environment(TimeTracker.self) private var tracker

    /// What picking a row will do, when that is not obvious from where the list is.
    /// The absence prompt needs it; the hotkey panel does not.
    var title: String?
    var maxVisible: Int = 6
    var onStart: (TimerTarget) -> Void

    @State private var selection = 0
    @State private var chosenTasks: [Int: Int] = [:]
    @FocusState private var isFocused: Bool

    private var projects: [ProjectTargets] { tracker.orderedProjects }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            if let title {
                Text(title)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            ProjectList(
                projects: projects,
                highlighted: min(selection, max(projects.count - 1, 0)),
                chosenTasks: $chosenTasks,
                maxVisible: maxVisible,
                onStart: onStart
            )
        }
        // The focus ring is suppressed because selection is already drawn on the row,
        // and two highlights read as two selections.
        .focusable()
        .focusEffectDisabled()
        .focused($isFocused)
        .onKeyPress(.downArrow) { move(by: 1) }
        .onKeyPress(.upArrow) { move(by: -1) }
        .onKeyPress(.return) { activate() }
        .onAppear { isFocused = true }
    }

    private func move(by offset: Int) -> KeyPress.Result {
        guard !projects.isEmpty else { return .ignored }
        selection = min(max(selection + offset, 0), projects.count - 1)
        return .handled
    }

    private func activate() -> KeyPress.Result {
        guard projects.indices.contains(selection),
              let task = projects[selection].task(chosenFrom: chosenTasks)
        else { return .ignored }
        onStart(projects[selection].target(task: task))
        selection = 0
        return .handled
    }
}

/// One project: its name and chosen task on the first line, its client and whatever
/// budget it has on the second.
///
/// Only the dropdown sits on the right-hand edge. The budget was there too, in a
/// column it had to share with the menu, which sized that column to whichever was
/// wider and left a small grey number stranded under a control. It reads as part of
/// the secondary line instead, on the separator "Client · Project" already uses.
private struct TargetRow: View {
    let entry: ProjectTargets
    let isSelected: Bool
    let budget: ProjectBudget?
    let format: HoursFormat
    @Binding var task: Reference?
    let onStart: () -> Void

    private var selectedText: AnyShapeStyle {
        AnyShapeStyle(Color(nsColor: .alternateSelectedControlTextColor))
    }

    private var secondaryText: AnyShapeStyle {
        isSelected
            ? AnyShapeStyle(Color(nsColor: .alternateSelectedControlTextColor).opacity(0.8))
            : AnyShapeStyle(.secondary)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 1) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(entry.project.name)
                    .font(.callout)
                    .lineLimit(1)
                Spacer(minLength: 8)
                taskControl
            }
            detailLine
                .font(.caption)
                .lineLimit(1)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 4)
        .foregroundStyle(isSelected ? selectedText : AnyShapeStyle(.primary))
        .background(
            isSelected
                ? AnyShapeStyle(Color(nsColor: .selectedContentBackgroundColor))
                : AnyShapeStyle(.clear),
            in: RoundedRectangle(cornerRadius: 5)
        )
        // The row cannot be a Button any more: the task dropdown lives inside it, and
        // a menu nested in a button never gets its own clicks.
        .contentShape(.rect)
        .onTapGesture(perform: onStart)
        // `.contain` rather than `.combine`, so the dropdown stays reachable instead
        // of being flattened into the row's label.
        .accessibilityElement(children: .contain)
        .accessibilityLabel(accessibilityLabel)
        .accessibilityAddTraits(isSelected ? [.isSelected] : [])
        .accessibilityAction(named: "Start timer", onStart)
    }

    private var accessibilityLabel: String {
        var parts = [entry.project.name]
        if let client = entry.client?.name, !client.isEmpty { parts.append(client) }
        if let budget, let remaining = budget.formattedRemaining(format) {
            parts.append(budget.isOverBudget ? "\(remaining) over budget" : "\(remaining) of budget left")
        }
        return parts.joined(separator: ", ")
    }

    /// A project with one task has nothing to choose, so it reads as a label rather
    /// than offering a menu that can only confirm itself.
    @ViewBuilder
    private var taskControl: some View {
        if entry.tasks.count <= 1 {
            Text(task?.name ?? "")
                .font(.callout)
                .lineLimit(1)
        } else {
            Menu {
                ForEach(entry.tasks) { option in
                    Button {
                        task = option
                    } label: {
                        if option.id == task?.id {
                            Label(option.name, systemImage: "checkmark")
                        } else {
                            Text(option.name)
                        }
                    }
                }
            } label: {
                HStack(spacing: 3) {
                    Text(task?.name ?? "")
                        .font(.callout)
                        .lineLimit(1)
                    Image(systemName: "chevron.up.chevron.down")
                        .font(.caption2)
                        .foregroundStyle(secondaryText)
                }
            }
            .menuStyle(.borderlessButton)
            .menuIndicator(.hidden)
            .fixedSize()
            .accessibilityLabel("Task")
            .accessibilityValue(task?.name ?? "")
        }
    }

    /// The client, and the budget after it when there is one. A single flowing line
    /// rather than two aligned cells: these are both quiet facts about the project,
    /// and nothing here needs to line up with anything in the row above.
    private var detailLine: Text {
        let client = Text(entry.client?.name ?? "").foregroundStyle(secondaryText)
        guard let budget, let remaining = budget.formattedRemaining(format) else { return client }

        let figure = Text(budget.isOverBudget ? "\(remaining) over" : "\(remaining) left")
            .monospacedDigit()
            .foregroundStyle(budgetStyle(budget))
        guard entry.client?.name.isEmpty == false else { return figure }
        return client + Text(" · ").foregroundStyle(secondaryText) + figure
    }

    /// Overspend is worth a colour, but not on a selected row: red on the selection
    /// blue is unreadable, and the row is already the thing being pointed at.
    private func budgetStyle(_ budget: ProjectBudget) -> AnyShapeStyle {
        budget.isOverBudget && !isSelected ? AnyShapeStyle(.red) : secondaryText
    }
}
