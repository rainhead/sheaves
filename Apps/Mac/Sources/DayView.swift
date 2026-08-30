import SheavesCore
import SwiftUI

/// The menu bar popover: the visible day, what is on the clock, and a place to start
/// something new without reaching for the mouse.
struct DayView: View {
    @Environment(TimeTracker.self) private var tracker
    @Environment(\.openSettingsWindow) private var openSettings

    private var format: HoursFormat { HoursFormat(company: tracker.company) }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            if tracker.connection == .needsCredentials {
                ConnectPrompt()
            } else {
                header
                if case .offline(let reason) = tracker.connection {
                    FailureBanner(reason: reason)
                }
                // Anchored to what is on the clock rather than to the visible day:
                // a budget is not a property of a day, and the question it answers
                // is about the thing being worked on now.
                if let active = tracker.activity.entry {
                    BudgetBar(entry: active, format: format)
                }
                Divider()
                // Today's entries come first: resuming something already started is
                // the common case, and searching is for the exception.
                entryList
                Divider()
                TargetPicker { target in
                    Task { await tracker.start(target) }
                }
            }
            Divider()
            footer
        }
        .padding(12)
        .frame(width: 380)
    }

    private var header: some View {
        HStack {
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
                    .frame(width: 22, height: 22)
            }
            .buttonStyle(.borderless)
            .accessibilityLabel("Next day")
            .contentShape(.rect)
            .keyboardShortcut(.rightArrow, modifiers: .command)

            Text(tracker.totalHours.formattedHours(format))
                .font(.headline.monospacedDigit())
                .frame(minWidth: 52, alignment: .trailing)
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
            SizedScrollView(maxHeight: 260) {
                LazyVStack(spacing: 2) {
                    ForEach(tracker.entries) { entry in
                        EntryRow(entry: entry, format: format)
                    }
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

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 6) {
            Image(systemName: "exclamationmark.triangle.fill")
                .foregroundStyle(.orange)
            Text(reason)
                .font(.caption)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
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
            return tracker.pendingCount > 0 ? "Offline · \(tracker.pendingCount.formatted()) queued" : "Offline"
        case .online:
            if tracker.pendingCount > 0 { return "Syncing \(tracker.pendingCount.formatted())…" }
            guard let synced = tracker.lastSyncedAt else { return "Synced" }
            // Reading `now` keeps this label live; the relative formatter renders a
            // just-finished sync as "in 0 seconds", which reads like the future.
            let elapsed = tracker.now.timeIntervalSince(synced)
            guard elapsed >= 45 else { return "Synced just now" }
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
