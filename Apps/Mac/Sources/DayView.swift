import SheavesCore
import SwiftUI

/// The menu bar popover: the visible day, what is on the clock, and a place to start
/// something new without reaching for the mouse.
struct DayView: View {
    @Environment(TimeTracker.self) private var tracker

    private var format: HoursFormat { HoursFormat(company: tracker.company) }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            if tracker.connection == .needsCredentials {
                ConnectPrompt()
            } else {
                header
                Divider()
                TargetPicker { target, notes in
                    Task { await tracker.start(target, notes: notes) }
                }
                Divider()
                entryList
            }
            Divider()
            footer
        }
        .padding(12)
        .frame(width: 380)
        .task { await tracker.sync() }
    }

    private var header: some View {
        HStack {
            Button {
                Task { await tracker.shiftDay(by: -1) }
            } label: {
                Image(systemName: "chevron.left")
            }
            .buttonStyle(.borderless)
            .keyboardShortcut(.leftArrow, modifiers: .command)

            VStack(spacing: 0) {
                Text(dayTitle)
                    .font(.headline)
                if !tracker.isToday {
                    Button("Back to today") {
                        Task { await tracker.goToToday() }
                    }
                    .buttonStyle(.link)
                    .font(.caption)
                }
            }
            .frame(maxWidth: .infinity)

            Button {
                Task { await tracker.shiftDay(by: 1) }
            } label: {
                Image(systemName: "chevron.right")
            }
            .buttonStyle(.borderless)
            .keyboardShortcut(.rightArrow, modifiers: .command)

            Text(tracker.totalHours.formattedHours(format))
                .font(.headline.monospacedDigit())
                .frame(minWidth: 52, alignment: .trailing)
        }
    }

    @ViewBuilder
    private var entryList: some View {
        if tracker.entries.isEmpty {
            Text("Nothing tracked \(tracker.isToday ? "yet today" : "on this day").")
                .font(.callout)
                .foregroundStyle(.secondary)
                .frame(maxWidth: .infinity, alignment: .center)
                .padding(.vertical, 12)
        } else {
            ScrollView {
                LazyVStack(spacing: 2) {
                    ForEach(tracker.entries) { entry in
                        EntryRow(entry: entry, format: format)
                    }
                }
            }
            .frame(maxHeight: 260)
        }
    }

    private var footer: some View {
        HStack(spacing: 8) {
            SyncStatusLabel()
            Spacer()
            SettingsLink {
                Image(systemName: "gearshape")
            }
            .buttonStyle(.borderless)
            .help("Settings")

            Button {
                NSApp.terminate(nil)
            } label: {
                Image(systemName: "power")
            }
            .buttonStyle(.borderless)
            .help("Quit Sheaves")
            .keyboardShortcut("q")
        }
        .font(.caption)
    }

    private var dayTitle: String {
        if tracker.isToday { return "Today" }
        return tracker.day.startOfDay().formatted(.dateTime.weekday(.wide).month(.abbreviated).day())
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

    private var color: Color {
        switch tracker.connection {
        case .online: tracker.pendingCount > 0 ? .yellow : .green
        case .connecting: .yellow
        case .offline: .orange
        case .needsCredentials: .secondary
        }
    }

    private var message: String {
        switch tracker.connection {
        case .needsCredentials:
            return "Not connected"
        case .connecting:
            return "Connecting…"
        case .offline:
            return tracker.pendingCount > 0 ? "Offline · \(tracker.pendingCount) queued" : "Offline"
        case .online:
            if tracker.pendingCount > 0 { return "Syncing \(tracker.pendingCount)…" }
            guard let synced = tracker.lastSyncedAt else { return "Synced" }
            return "Synced \(synced.formatted(.relative(presentation: .numeric)))"
        }
    }

    private var detail: String {
        if case .offline(let reason) = tracker.connection { return reason }
        return "Sheaves keeps working offline and sends changes when Harvest is reachable."
    }
}

/// Shown before any credentials exist, so the popover is never just empty.
struct ConnectPrompt: View {
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
            SettingsLink {
                Text("Open Settings…")
            }
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 12)
    }
}
