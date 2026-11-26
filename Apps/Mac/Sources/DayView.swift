import SheavesCore
import SwiftUI

/// The menu bar popover: the visible day, what is on the clock, and a place to start
/// something new without reaching for the mouse.
/// A row the arrow keys can land on. The day's entries and the project list are one
/// column on screen, so they are one list to the keyboard; identity is by id rather
/// than by index because starting or stopping a timer reorders the entries under it.
enum PanelRow: Hashable {
    case entry(TrackedEntry.ID)
    case project(Int64)
}

struct DayView: View {
    @Environment(TimeTracker.self) private var tracker
    @Environment(\.openSettingsWindow) private var openSettings

    @State private var selection: PanelRow?
    @State private var chosenTasks: [Int64: Int64] = [:]
    /// Which entry, if any, has its notes field open. Held here so the panel can hand
    /// the keyboard over to it entirely, and so only one row can be editing.
    @State private var editingNotes: TrackedEntry.ID?
    /// The duration field open on an entry, on the same terms as the notes.
    @State private var editingHours: TrackedEntry.ID?
    /// The entry whose play press is waiting on the today-or-then question, so the
    /// keyboard's ⏎ raises the same offer the mouse gets.
    @State private var confirmingResume: TrackedEntry.ID?
    /// The entry ⌫ or the context menu has asked to delete, held here for the same
    /// reason.
    @State private var confirmingDelete: TrackedEntry.ID?
    /// Whether the form for logging time without a timer is open under the header.
    @State private var isLoggingTime = false
    /// What is typed in the project search field under the entries.
    @State private var searchText = ""
    @FocusState private var isSearchFocused: Bool
    private var isEditingField: Bool {
        editingNotes != nil || editingHours != nil || isLoggingTime || isSearchFocused
    }
    @FocusState private var isPanelFocused: Bool

    private var format: HoursFormat { HoursFormat(company: tracker.company) }

    /// The projects the search leaves showing; all of them while it is empty.
    private var projects: [ProjectTargets] { tracker.orderedProjects.matching(searchText) }

    private var rows: [PanelRow] {
        tracker.entries.map { PanelRow.entry($0.id) } + projects.map { PanelRow.project($0.id) }
    }

    /// Falls back to the first project rather than the first entry, so ⏎ on a freshly
    /// opened panel still starts the top-ranked timer instead of stopping a running
    /// one. Recomputed rather than stored, so a selection whose row has gone — an
    /// entry deleted, a project unassigned — lands somewhere real.
    private var effectiveSelection: PanelRow? {
        if let selection, rows.contains(selection) { return selection }
        return projects.first.map { .project($0.id) }
    }

    private var highlightedProject: Int? {
        guard case .project(let id) = effectiveSelection else { return nil }
        return projects.firstIndex { $0.id == id }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            if tracker.connection == .needsCredentials {
                ConnectPrompt()
            } else {
                header
                if case .offline(let reason) = tracker.connection {
                    FailureBanner(reason: "Offline: \(reason)", retry: { Task { await tracker.sync() } })
                }
                if let notice = tracker.notice {
                    FailureBanner(reason: notice)
                }
                // Anchored to what is on the clock rather than to the visible day:
                // a budget is not a property of a day, and the question it answers
                // is about the thing being worked on now.
                if let active = tracker.activity.entry {
                    BudgetBar(entry: active, format: format)
                }
                if isLoggingTime {
                    Divider()
                    LogTimeForm(dayTitle: dayTitle) {
                        isLoggingTime = false
                        // Back to the panel, so the arrows and ⏎ work again.
                        isPanelFocused = true
                    }
                }
                Divider()
                // Today's entries come first: resuming something already started is
                // the common case, and searching is for the exception.
                entryList
                Divider()
                ProjectSearchField(
                    text: $searchText,
                    isFocused: $isSearchFocused,
                    onMove: moveWithinProjects(by:),
                    onSubmit: startSearchedProject
                )
                ProjectList(
                    projects: projects,
                    query: searchText,
                    highlighted: highlightedProject,
                    chosenTasks: $chosenTasks,
                    onStart: { target in Task { await tracker.start(target) } }
                )
            }
            Divider()
            footer
        }
        .padding(12)
        .frame(width: 380)
        // The ring is suppressed because the selected row already draws itself, and
        // two highlights read as two selections.
        .focusable()
        .focusEffectDisabled()
        .focused($isPanelFocused)
        .onKeyPress(.downArrow) { move(by: 1) }
        .onKeyPress(.upArrow) { move(by: -1) }
        .onKeyPress(.return) { activate() }
        // Not `onKeyPress(.delete)`: that is U+0008, and the Mac's ⌫ sends U+007F,
        // so it never matches. This is the command both delete keys raise.
        .onDeleteCommand(perform: requestDelete)
        // The first match is the one ⏎ starts, as the list changes under the typing.
        .onChange(of: searchText) { selection = projects.first.map { .project($0.id) } }
        .onAppear { isPanelFocused = true }
        // The panel is kept and shown again, so each showing starts with an empty
        // search and the keyboard on the panel. A sheet's window is not a showing.
        .onReceive(NotificationCenter.default.publisher(for: NSWindow.didBecomeKeyNotification)) { note in
            guard let window = note.object as? NSPanel, window.sheetParent == nil else { return }
            searchText = ""
            isPanelFocused = true
        }
    }

    /// The arrows in the search field move through the projects it leaves, and no
    /// further: the entries above are not what is being searched.
    private func moveWithinProjects(by offset: Int) -> KeyPress.Result {
        guard !projects.isEmpty else { return .ignored }
        let current = highlightedProject ?? -1
        selection = .project(projects[min(max(current + offset, 0), projects.count - 1)].id)
        return .handled
    }

    /// ⏎ in the search field starts the highlighted project, or the first match when
    /// the highlight is on an entry, and hands the keyboard back to the panel.
    private func startSearchedProject() {
        let id: Int64?
        if case .project(let selected) = effectiveSelection { id = selected } else { id = projects.first?.id }
        guard let project = projects.first(where: { $0.id == id }),
              let task = project.task(chosenFrom: chosenTasks)
        else { return }
        Task { await tracker.start(project.target(task: task)) }
        searchText = ""
        isPanelFocused = true
    }

    /// Every key here defers to an open notes or duration field. Relying on the text
    /// field to swallow them would leave the arrows moving the selection out from
    /// under somebody in the middle of typing.
    private func move(by offset: Int) -> KeyPress.Result {
        guard !isEditingField else { return .ignored }
        let rows = self.rows
        guard !rows.isEmpty else { return .ignored }
        let current = effectiveSelection.flatMap { rows.firstIndex(of: $0) } ?? 0
        selection = rows[min(max(current + offset, 0), rows.count - 1)]
        return .handled
    }

    /// ⏎ does whatever the selected row's button does: stop or resume an entry, start
    /// a project.
    private func activate() -> KeyPress.Result {
        guard !isEditingField else { return .ignored }
        switch effectiveSelection {
        case .entry(let id):
            // Locked, so the row's own button is disabled: the keyboard must not do
            // what the mouse is forbidden. `resume` already refuses, but `stop` does
            // not, so a locked entry left running was reachable this way alone.
            guard let entry = tracker.entries.first(where: { $0.id == id }), !entry.isLocked
            else { return .ignored }
            // The same fork the play button offers: resuming a past day's entry
            // would charge the old day, so it asks rather than assumes.
            if !entry.isRunning, entry.spentDate != .today() {
                confirmingResume = entry.id
            } else {
                Task { await tracker.toggle(entry) }
            }
        case .project(let id):
            guard let project = projects.first(where: { $0.id == id }),
                  let task = project.task(chosenFrom: chosenTasks)
            else { return .ignored }
            Task { await tracker.start(project.target(task: task)) }
        case nil:
            return .ignored
        }
        return .handled
    }

    /// ⌫ on an entry asks to delete it — the one thing otherwise only a right-click
    /// reached, which nothing on screen admits to. A project row has nothing to
    /// delete, so the key does nothing there.
    private func requestDelete() {
        guard !isEditingField, case .entry(let id) = effectiveSelection,
              let entry = tracker.entries.first(where: { $0.id == id }), !entry.isLocked
        else { return }
        confirmingDelete = entry.id
    }

    /// Arranged as GNOME's header is: + at the start, and the day centred between
    /// its arrows with the day's total beneath it, and on any other day a way back
    /// to today.
    private var header: some View {
        HStack(spacing: 0) {
            // Opens the form rather than toggling it, so a second press does not
            // throw away what was typed.
            Button {
                editingNotes = nil
                editingHours = nil
                isLoggingTime = true
            } label: {
                Image(systemName: "plus")
                    .frame(width: 22, height: 22)
            }
            .buttonStyle(.borderless)
            .help("Log time without a timer")
            .accessibilityLabel("Log time")
            .contentShape(.rect)
            .keyboardShortcut("n")

            Spacer(minLength: 0)

            HStack(spacing: 6) {
                Button {
                    Task { await tracker.shiftDay(by: -1) }
                } label: {
                    Image(systemName: "chevron.left")
                        .frame(width: 22, height: 22)
                }
                .buttonStyle(.borderless)
                .accessibilityLabel("Previous day")
                .contentShape(.rect)
                .keyboardShortcut(.leftArrow, modifiers: .command)

                VStack(spacing: 0) {
                    Text(dayTitle)
                        .font(.headline)
                        .lineLimit(1)
                    HStack(spacing: 6) {
                        Text("\(tracker.totalHours.formattedHours(format)) logged")
                            .monospacedDigit()
                            .foregroundStyle(.secondary)
                        if !tracker.isToday {
                            Button("Back to today") {
                                Task { await tracker.goToToday() }
                            }
                            .buttonStyle(.link)
                        }
                    }
                    .font(.caption)
                    .lineLimit(1)
                }
                // A fixed width, so the arrows stay put as the day's name and total
                // change length: clicking back twice would otherwise miss the
                // second time.
                .frame(width: 170)

                Button {
                    Task { await tracker.shiftDay(by: 1) }
                } label: {
                    Image(systemName: "chevron.right")
                        .frame(width: 22, height: 22)
                }
                .buttonStyle(.borderless)
                .accessibilityLabel("Next day")
                .contentShape(.rect)
                .keyboardShortcut(.rightArrow, modifiers: .command)
            }

            Spacer(minLength: 0)

            // As wide as +, so the day is centred in the panel rather than in what
            // + leaves of it.
            Color.clear.frame(width: 22, height: 22)
        }
    }

    @ViewBuilder
    private var entryList: some View {
        if tracker.entries.isEmpty {
            Text(tracker.isToday ? "Nothing tracked yet today." : "Nothing tracked on this day.")
                .font(.callout)
                .foregroundStyle(.secondary)
                .frame(maxWidth: .infinity, alignment: .center)
                .padding(.vertical, 10)
        } else {
            // The arrow keys can move the selection past the bottom of a long day, so
            // the list has to follow it — as the project list below already does.
            ScrollViewReader { proxy in
                SizedScrollView(maxHeight: 260) {
                    LazyVStack(spacing: 2) {
                        ForEach(tracker.entries) { entry in
                            EntryRow(
                                entry: entry,
                                format: format,
                                isSelected: effectiveSelection == .entry(entry.id),
                                // Opening either field closes the other, wherever it
                                // is: the keyboard defers to "the open field", which
                                // must therefore be singular.
                                isEditingNotes: Binding(
                                    get: { editingNotes == entry.id },
                                    set: {
                                        editingNotes = $0 ? entry.id : nil
                                        if $0 { editingHours = nil }
                                    }
                                ),
                                isEditingHours: Binding(
                                    get: { editingHours == entry.id },
                                    set: {
                                        editingHours = $0 ? entry.id : nil
                                        if $0 { editingNotes = nil }
                                    }
                                ),
                                isConfirmingResume: Binding(
                                    get: { confirmingResume == entry.id },
                                    set: { confirmingResume = $0 ? entry.id : nil }
                                ),
                                isConfirmingDelete: Binding(
                                    get: { confirmingDelete == entry.id },
                                    set: { confirmingDelete = $0 ? entry.id : nil }
                                )
                            )
                            .id(entry.id)
                        }
                    }
                }
                .onChange(of: effectiveSelection) {
                    guard case .entry(let id) = effectiveSelection else { return }
                    withAnimation(.easeOut(duration: 0.1)) { proxy.scrollTo(id) }
                }
            }
        }
    }

    private var footer: some View {
        HStack(spacing: 8) {
            SyncStatusLabel()
            Spacer()
            Button(action: openSettings) {
                Image(systemName: "gearshape")
                    .frame(width: 22, height: 22)
            }
            .buttonStyle(.borderless)
            .help("Settings")
            .accessibilityLabel("Settings")
            .contentShape(.rect)
            .keyboardShortcut(",")

            Button {
                NSApp.terminate(nil)
            } label: {
                Image(systemName: "power")
                    .frame(width: 22, height: 22)
            }
            .buttonStyle(.borderless)
            .help("Quit Sheaves")
            .accessibilityLabel("Quit Sheaves")
            .contentShape(.rect)
            .keyboardShortcut("q")
        }
        .font(.caption)
    }

    private var dayTitle: String {
        if tracker.isToday { return "Today" }
        return tracker.day.startOfDay().formatted(.dateTime.weekday(.wide).month(.abbreviated).day())
    }
}

/// A sync failure the user can actually read. The status dot alone is too quiet
/// when the list is empty, because an empty day and a failed load look identical.
struct FailureBanner: View {
    let reason: String
    /// Asks Harvest again. Only a failure that trying again can fix offers it.
    var retry: (() -> Void)?

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 6) {
            Image(systemName: "exclamationmark.triangle.fill")
                .foregroundStyle(.orange)
            Text(reason)
                .font(.caption)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
            if let retry {
                Spacer(minLength: 0)
                Button("Retry", action: retry)
                    .buttonStyle(.link)
                    .font(.caption)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(8)
        .background(.orange.opacity(0.12), in: RoundedRectangle(cornerRadius: 6))
    }
}

/// One line describing whether Harvest and Sheaves agree yet.
struct SyncStatusLabel: View {
    @Environment(TimeTracker.self) private var tracker

    var body: some View {
        HStack(spacing: 5) {
            Circle()
                .fill(color)
                .frame(width: 6, height: 6)
            Text(message)
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .truncationMode(.tail)
        }
        .help(detail)
    }

    private var status: SyncStatus { tracker.syncStatus }

    private var color: Color {
        switch status.statusTone() {
        case .good: .green
        case .working: .yellow
        case .warning: .orange
        case .idle: .secondary
        }
    }

    /// The core decides which state this is; the words and the relative time are
    /// the Mac's. Reading the status reads `now`, which keeps the label live.
    private var message: String {
        switch status {
        case .notConnected: "Not connected"
        case .connecting: "Connecting…"
        case .offline(let queued):
            queued > 0 ? "Offline · \(queued.formatted()) queued" : "Offline"
        case .syncing(let pending): "Syncing \(pending.formatted())…"
        case .syncedJustNow: "Synced just now"
        case .synced(let at):
            at.map { "Synced \($0.formatted(.relative(presentation: .numeric)))" } ?? "Synced"
        }
    }

    private var detail: String {
        if case .offline(let reason) = tracker.connection { return reason }
        return "Sheaves keeps working offline and sends changes when Harvest is reachable."
    }
}

/// Shown before any credentials exist, so the popover is never just empty.
struct ConnectPrompt: View {
    @Environment(\.openSettingsWindow) private var openSettings

    var body: some View {
        VStack(spacing: 8) {
            Image(systemName: "clock.badge.questionmark")
                .font(.largeTitle)
                .foregroundStyle(.secondary)
            Text("Connect to Harvest")
                .font(.headline)
            Text("Sheaves needs a personal access token before it can track anything.")
                .font(.caption)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
            Button("Open Settings…", action: openSettings)
                .buttonStyle(.borderedProminent)
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 12)
    }
}
