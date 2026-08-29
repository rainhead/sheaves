import SheavesCore
import SwiftUI

struct EntryRow: View {
    @Environment(TimeTracker.self) private var tracker

    let entry: TrackedEntry
    var format: HoursFormat

    @State private var isEditingNotes = false
    @State private var draftNotes = ""
    @State private var isConfirmingDelete = false

    var body: some View {
        HStack(alignment: .center, spacing: 8) {
            VStack(alignment: .leading, spacing: 1) {
                Text(entry.task.name)
                    .font(.callout)
                    .fontWeight(entry.isRunning ? .semibold : .regular)
                    .lineLimit(1)
                Text(entry.target.projectLabel)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                notes
            }
            .frame(maxWidth: .infinity, alignment: .leading)

            duration
            toggleButton
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 6)
        .background(
            entry.isRunning ? AnyShapeStyle(.tint.opacity(0.12)) : AnyShapeStyle(.clear),
            in: RoundedRectangle(cornerRadius: 6)
        )
        .contextMenu {
            Button("Edit Notes…") { beginEditingNotes() }
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
            Task { await tracker.toggle(entry) }
        }
        .accessibilityAction(named: "Edit notes") { beginEditingNotes() }
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
        case (0, let m): return "\(m) minute\(m == 1 ? "" : "s")"
        case (let h, 0): return "\(h) hour\(h == 1 ? "" : "s")"
        case (let h, let m): return "\(h) hour\(h == 1 ? "" : "s") \(m) minute\(m == 1 ? "" : "s")"
        }
    }

    @ViewBuilder
    private var notes: some View {
        if isEditingNotes {
            TextField("Notes", text: $draftNotes)
                .textFieldStyle(.roundedBorder)
                .font(.caption)
                .onSubmit {
                    isEditingNotes = false
                    Task { await tracker.updateNotes(entry, to: draftNotes) }
                }
                .onExitCommand { isEditingNotes = false }
        } else if let text = entry.notes, !text.isEmpty {
            Text(text)
                .font(.caption)
                .foregroundStyle(.secondary)
                .lineLimit(2)
                .onTapGesture(count: 2) { beginEditingNotes() }
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
            Text(entry.hours(asOf: tracker.now).formattedHours(format))
                .font(.callout.monospacedDigit())
                .fontWeight(entry.isRunning ? .semibold : .regular)
        }
    }

    private var toggleButton: some View {
        Button {
            Task { await tracker.toggle(entry) }
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
        draftNotes = entry.notes ?? ""
        isEditingNotes = true
    }
}
