import SheavesCore
import SwiftUI

struct EntryRow: View {
    @Environment(TimeTracker.self) private var tracker

    let entry: TrackedEntry
    var format: HoursFormat
    var isSelected: Bool = false
    /// Held by the panel rather than the row, so that only one row can be editing at
    /// a time and so the panel knows to leave the keyboard alone while one is.
    @Binding var isEditingNotes: Bool
    /// Same arrangement for the duration, which is a field the way the notes are.
    @Binding var isEditingHours: Bool
    /// Play on a past day's entry asks first — resuming it would charge the old
    /// day. The panel holds this too, so the keyboard's ⏎ can raise the same offer.
    @Binding var isConfirmingResume: Bool
    @State private var draftNotes = ""
    @State private var draftHours = ""
    @State private var isConfirmingDelete = false
    @State private var isHovering = false
    @FocusState private var isNotesFocused: Bool
    @FocusState private var isHoursFocused: Bool

    var body: some View {
        HStack(alignment: .center, spacing: 8) {
            VStack(alignment: .leading, spacing: 1) {
                Text(entry.task.name)
                    .font(.callout)
                    .fontWeight(entry.isRunning ? .semibold : .regular)
                    .lineLimit(1)
                Text(entry.target.projectLabel)
                    .font(.caption)
                    .foregroundStyle(secondaryText)
                    .lineLimit(1)
                notes
            }
            .frame(maxWidth: .infinity, alignment: .leading)

            duration
            toggleButton
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 6)
        .foregroundStyle(isSelected ? selectedText : AnyShapeStyle(.primary))
        .background(background, in: RoundedRectangle(cornerRadius: 6))
        // Without a shape, hover only registers over drawn text — a stopped row has
        // a clear background, so most of it was not hoverable and "Add notes"
        // appeared only when the pointer crossed the two lines of the label.
        .contentShape(.rect)
        .onHover { isHovering = $0 }
        .contextMenu {
            Button(entry.notes?.isEmpty == false ? "Edit Notes…" : "Add Notes…") { beginEditingNotes() }
                .disabled(entry.isLocked)
            Button("Edit Time…") { beginEditingHours() }
                .disabled(entry.isLocked)
            Button("Delete…", role: .destructive) { isConfirmingDelete = true }
                .disabled(entry.isLocked)
        }
        // Without this the row reads as disconnected fragments: task, project,
        // notes, two status icons and a duration, with the icons explained only by
        // a mouse-only tooltip.
        .accessibilityElement(children: .combine)
        .accessibilityLabel(accessibilityLabel)
        .accessibilityAction(named: entry.isRunning ? "Stop timer" : "Resume timer") {
            requestToggle()
        }
        .accessibilityAction(named: entry.notes?.isEmpty == false ? "Edit notes" : "Add notes") {
            beginEditingNotes()
        }
        .accessibilityAction(named: "Edit time") { beginEditingHours() }
        .confirmationDialog(
            "Delete this time entry?",
            isPresented: $isConfirmingDelete,
            titleVisibility: .visible
        ) {
            Button("Delete", role: .destructive) {
                Task { await tracker.delete(entry) }
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("\(entry.hours(asOf: tracker.now).formattedHours(format)) on \(entry.task.name) will be removed from Harvest. This cannot be undone.")
        }
        // Play on a past entry usually means "do that work now", not "backfill an
        // old day" — so it asks, the way Harvest's own app does. Both choices are
        // spelled out because neither is a cancel: one starts a fresh timer today
        // with the same project, task and notes; the other genuinely reopens the
        // old day.
        .confirmationDialog(
            "This entry is from \(entryDayLabel).",
            isPresented: $isConfirmingResume,
            titleVisibility: .visible
        ) {
            Button("Start on Today") {
                Task { await tracker.start(entry.target, notes: entry.notes, on: .today()) }
            }
            Button("Resume on \(entryDayLabel)") {
                Task { await tracker.resume(entry) }
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Starting on Today leaves \(entryDayLabel)’s time as it is.")
        }
    }

    private var entryDayLabel: String {
        entry.spentDate.startOfDay().formatted(.dateTime.weekday(.wide).month(.abbreviated).day())
    }

    /// What play or ⏎ should do: toggle, unless resuming would quietly charge a
    /// past day — then raise the offer instead.
    private func requestToggle() {
        if !entry.isRunning, entry.spentDate != .today() {
            isConfirmingResume = true
        } else {
            Task { await tracker.toggle(entry) }
        }
    }

    private var selectedText: AnyShapeStyle {
        AnyShapeStyle(Color(nsColor: .alternateSelectedControlTextColor))
    }

    private var secondaryText: AnyShapeStyle {
        isSelected
            ? AnyShapeStyle(Color(nsColor: .alternateSelectedControlTextColor).opacity(0.8))
            : AnyShapeStyle(.secondary)
    }

    /// Selection outranks the running tint: a running row still says so with its
    /// weight and its stop button, where a selected row has only this to say it.
    private var background: AnyShapeStyle {
        if isSelected { return AnyShapeStyle(Color(nsColor: .selectedContentBackgroundColor)) }
        return entry.isRunning ? AnyShapeStyle(.tint.opacity(0.12)) : AnyShapeStyle(.clear)
    }

    /// Spoken as one sentence, including what the status icons mean.
    private var accessibilityLabel: String {
        var parts = [entry.task.name, entry.target.projectLabel]
        if let notes = entry.notes, !notes.isEmpty { parts.append(notes) }
        parts.append(spokenDuration)
        if entry.isRunning { parts.append("timer running") }
        if entry.isPending { parts.append("waiting to sync") }
        if entry.isLocked { parts.append("locked, approved or invoiced") }
        return parts.joined(separator: ", ")
    }

    /// "1:14" is read as "one fourteen"; say what it means.
    private var spokenDuration: String {
        let total = Int((entry.hours(asOf: tracker.now) * 60).rounded())
        let hours = total / 60
        let minutes = total % 60
        switch (hours, minutes) {
        case (0, let m): return "\(m.formatted()) minute\(m == 1 ? "" : "s")"
        case (let h, 0): return "\(h.formatted()) hour\(h == 1 ? "" : "s")"
        case (let h, let m): return "\(h.formatted()) hour\(h == 1 ? "" : "s") \(m.formatted()) minute\(m == 1 ? "" : "s")"
        }
    }

    /// Notes are written here rather than when a timer starts, so this is the only
    /// way to add them — it cannot be a double-click on text that is not there.
    @ViewBuilder
    private var notes: some View {
        if isEditingNotes {
            TextField("Notes", text: $draftNotes)
                .textFieldStyle(.roundedBorder)
                .font(.caption)
                .focused($isNotesFocused)
                .onSubmit(commitNotes)
                .onExitCommand { isEditingNotes = false }
                .onAppear { isNotesFocused = true }
        } else if let text = entry.notes, !text.isEmpty {
            Button(action: beginEditingNotes) {
                Text(text)
                    .font(.caption)
                    .foregroundStyle(secondaryText)
                    .lineLimit(2)
                    .multilineTextAlignment(.leading)
            }
            .buttonStyle(.plain)
            .disabled(entry.isLocked)
            .help("Edit notes")
        } else if !entry.isLocked {
            Button(action: beginEditingNotes) {
                Label("Add notes", systemImage: "text.bubble")
                    .font(.caption)
                    .foregroundStyle(.tertiary)
            }
            .buttonStyle(.plain)
            .help("Add notes")
            // Quiet until pointed at, so a day of entries is not a wall of prompts.
            // Opacity rather than a conditional view: the row keeps its height, so
            // the list does not shift under the pointer.
            .opacity(isHovering ? 1 : 0)
            .animation(.easeOut(duration: 0.12), value: isHovering)
        }
    }

    private var duration: some View {
        HStack(spacing: 4) {
            if entry.isPending {
                // A local change Harvest has not acknowledged yet.
                Image(systemName: "arrow.trianglehead.2.clockwise.rotate.90")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                    .help("Waiting to sync")
            }
            if entry.isLocked {
                Image(systemName: "lock.fill")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                    .help("Approved or invoiced — Harvest won’t accept changes")
            }
            if isEditingHours {
                TextField("0:00", text: $draftHours)
                    .textFieldStyle(.roundedBorder)
                    .font(.callout.monospacedDigit())
                    .multilineTextAlignment(.trailing)
                    .frame(width: 58)
                    .focused($isHoursFocused)
                    .onSubmit(commitHours)
                    .onExitCommand { isEditingHours = false }
                    .onAppear {
                        // Normally prefilled by `beginEditingHours`; also here, so a
                        // render that opens the field directly (the documentation
                        // images) shows what a user would see.
                        if draftHours.isEmpty {
                            draftHours = entry.hours(asOf: tracker.now).formattedHours(format)
                        }
                        isHoursFocused = true
                    }
            } else {
                // A field pretending to be a label, exactly like the notes above:
                // out of a meeting nobody timed, the fix is to click the duration
                // and type "1".
                Button(action: beginEditingHours) {
                    Text(entry.hours(asOf: tracker.now).formattedHours(format))
                        .font(.callout.monospacedDigit())
                        .fontWeight(entry.isRunning ? .semibold : .regular)
                }
                .buttonStyle(.plain)
                .disabled(entry.isLocked)
                .help("Edit time")
            }
        }
    }

    private var toggleButton: some View {
        Button {
            requestToggle()
        } label: {
            Image(systemName: entry.isRunning ? "stop.fill" : "play.fill")
                .font(.caption)
                .frame(width: 26, height: 26)
        }
        .buttonStyle(.borderless)
        .disabled(entry.isLocked)
        .help(entry.isRunning ? "Stop timer" : "Resume timer")
        .accessibilityLabel(entry.isRunning ? "Stop timer" : "Resume timer")
        .contentShape(.rect)
    }

    private func beginEditingNotes() {
        guard !entry.isLocked else { return }
        draftNotes = entry.notes ?? ""
        isEditingNotes = true
    }

    private func beginEditingHours() {
        guard !entry.isLocked else { return }
        draftHours = entry.hours(asOf: tracker.now).formattedHours(format)
        isEditingHours = true
    }

    private func commitHours() {
        isEditingHours = false
        guard let hours = Double.hours(parsing: draftHours) else { return }
        // A stopped entry left as it was should not be marked pending over nothing;
        // a running one always takes the new value, because its total is moving.
        guard entry.isRunning || hours != entry.bankedHours else { return }
        Task { await tracker.updateHours(entry, to: hours) }
    }

    private func commitNotes() {
        isEditingNotes = false
        let trimmed = draftNotes.trimmingCharacters(in: .whitespacesAndNewlines)
        guard trimmed != (entry.notes ?? "") else { return }
        Task { await tracker.updateNotes(entry, to: trimmed) }
    }
}
