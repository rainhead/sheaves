import SheavesCore
import SwiftUI

/// Type-to-filter list of project/task pairs, driven entirely from the keyboard.
///
/// Shared by the menu bar popover and the quick-entry panel so both behave the same:
/// type to narrow, arrows to move, ⇥ for notes, ⏎ to start.
struct TargetPicker: View {
    @Environment(TimeTracker.self) private var tracker

    var placeholder: String = "Start a timer…"
    var maxVisible: Int = 6
    var onStart: (TimerTarget, String?) -> Void

    @State private var query = ""
    @State private var notes = ""
    @State private var selection = 0
    @FocusState private var focus: Field?

    private enum Field: Hashable { case search, notes }

    private var results: [TimerTarget] {
        Array(tracker.suggestedTargets(matching: query).prefix(40))
    }

    private var selected: TimerTarget? {
        results.indices.contains(selection) ? results[selection] : results.first
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            searchField

            if results.isEmpty {
                emptyState
            } else {
                resultList
                notesField
            }
        }
        .onAppear { focus = .search }
        .onChange(of: query) { selection = 0 }
    }

    private var searchField: some View {
        HStack(spacing: 6) {
            Image(systemName: "magnifyingglass")
                .foregroundStyle(.secondary)
            TextField(placeholder, text: $query)
                .textFieldStyle(.plain)
                .accessibilityLabel("Start a timer")
                .accessibilityHint("Type to filter projects and tasks")
                .focused($focus, equals: .search)
                .onSubmit(start)
                .onKeyPress(.downArrow) { move(by: 1) }
                .onKeyPress(.upArrow) { move(by: -1) }
                .onKeyPress(phases: .down) { press in
                    // ⌥⇥ jumps to notes as a convenience; plain Tab must still hand
                    // focus onwards, or the panel becomes a keyboard trap.
                    guard press.key == .tab, press.modifiers.contains(.option) else { return .ignored }
                    focus = .notes
                    return .handled
                }
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 6)
        .background(.quaternary.opacity(0.5), in: RoundedRectangle(cornerRadius: 6))
    }

    private var resultList: some View {
        ScrollViewReader { proxy in
            SizedScrollView(maxHeight: CGFloat(maxVisible) * 36) {
                LazyVStack(alignment: .leading, spacing: 0) {
                    ForEach(Array(results.enumerated()), id: \.element.id) { index, target in
                        Button {
                            selection = index
                            start()
                        } label: {
                            TargetRow(target: target, isSelected: index == selection)
                        }
                        .buttonStyle(.plain)
                        .id(target.id)
                        .accessibilityLabel("\(target.task.name), \(target.projectLabel)")
                        .accessibilityHint("Starts a timer")
                        .accessibilityAddTraits(index == selection ? [.isSelected] : [])
                    }
                }
            }
            .onChange(of: selection) {
                guard let target = selected else { return }
                withAnimation(.easeOut(duration: 0.1)) { proxy.scrollTo(target.id) }
            }
        }
    }

    private var notesField: some View {
        TextField("Notes (optional)", text: $notes)
            .textFieldStyle(.plain)
            .font(.callout)
            .focused($focus, equals: .notes)
            .accessibilityLabel("Notes for the new timer")
            .onSubmit(start)
            .padding(.horizontal, 8)
            .padding(.vertical, 5)
            .background(.quaternary.opacity(0.3), in: RoundedRectangle(cornerRadius: 6))
    }

    private var emptyState: some View {
        Text(tracker.targets.isEmpty ? "No projects yet — sync with Harvest." : "No match for “\(query)”.")
            .font(.callout)
            .foregroundStyle(.secondary)
            .padding(.vertical, 8)
    }

    private func move(by offset: Int) -> KeyPress.Result {
        guard !results.isEmpty else { return .ignored }
        selection = min(max(selection + offset, 0), results.count - 1)
        return .handled
    }

    private func start() {
        guard let selected else { return }
        let trimmed = notes.trimmingCharacters(in: .whitespacesAndNewlines)
        onStart(selected, trimmed.isEmpty ? nil : trimmed)
        query = ""
        notes = ""
        selection = 0
    }
}

private struct TargetRow: View {
    let target: TimerTarget
    let isSelected: Bool

    private var selectedText: AnyShapeStyle {
        AnyShapeStyle(Color(nsColor: .alternateSelectedControlTextColor))
    }

    private var selectedSecondaryText: AnyShapeStyle {
        AnyShapeStyle(Color(nsColor: .alternateSelectedControlTextColor).opacity(0.8))
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 1) {
            Text(target.task.name)
                .font(.callout)
                .lineLimit(1)
            Text(target.projectLabel)
                .font(.caption)
                .foregroundStyle(isSelected ? selectedSecondaryText : AnyShapeStyle(.secondary))
                .lineLimit(1)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 8)
        .padding(.vertical, 4)
        .foregroundStyle(isSelected ? selectedText : AnyShapeStyle(.primary))
        .background(
            isSelected
                ? AnyShapeStyle(Color(nsColor: .selectedContentBackgroundColor))
                : AnyShapeStyle(.clear),
            in: RoundedRectangle(cornerRadius: 5)
        )
    }
}
