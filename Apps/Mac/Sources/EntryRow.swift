import SheavesCore
import SwiftUI

struct EntryRow: View {
    @Environment(TimeTracker.self) private var tracker

    let entry: TrackedEntry
    var format: HoursFormat

    @State private var isEditingNotes = false
    @State private var draftNotes = ""

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
            Button("Delete", role: .destructive) {
                Task { await tracker.delete(entry) }
            }
            .disabled(entry.isLocked)
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
            Text(
                entry.isRunning
                    ? entry.hours(asOf: tracker.now).formattedHoursWithSeconds()
                    : entry.hours(asOf: tracker.now).formattedHours(format)
            )
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
                .frame(width: 18, height: 18)
        }
        .buttonStyle(.borderless)
        .disabled(entry.isLocked)
        .help(entry.isRunning ? "Stop timer" : "Resume timer")
    }

    private func beginEditingNotes() {
        draftNotes = entry.notes ?? ""
        isEditingNotes = true
    }
}
