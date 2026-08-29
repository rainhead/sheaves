import SheavesCore
import SwiftUI

/// What the menu bar shows at a glance: the running timer and how long it has run,
/// or a plain icon when nothing is on the clock.
struct MenuBarLabel: View {
    let tracker: TimeTracker

    /// Long project names would crowd out everything else in the menu bar.
    private let maxTitleCharacters = 18

    var body: some View {
        if let running = tracker.runningEntry {
            HStack(spacing: 4) {
                Image(systemName: "timer")
                Text("\(title(for: running))  \(running.hours(asOf: tracker.now).formattedHours(format))")
                    .monospacedDigit()
            }
        } else {
            Image(systemName: "timer")
        }
    }

    private var format: HoursFormat {
        HoursFormat(company: tracker.company)
    }

    private func title(for entry: TrackedEntry) -> String {
        let name = entry.project.name
        guard name.count > maxTitleCharacters else { return name }
        return name.prefix(maxTitleCharacters - 1).trimmingCharacters(in: .whitespaces) + "…"
    }
}
