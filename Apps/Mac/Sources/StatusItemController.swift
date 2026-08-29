import AppKit
import SheavesCore
import SwiftUI

/// The menu bar item: a play/pause button on the left, everything else opens the
/// popover.
///
/// This is hand-rolled AppKit rather than SwiftUI's `MenuBarExtra` because a
/// `MenuBarExtra` has exactly one behaviour — any click opens its content — and the
/// point here is that pausing or resuming the current timer should take one click
/// and open nothing. Owning the `NSStatusItem` also means the popover can be shown
/// programmatically, which `MenuBarExtra` cannot do.
@MainActor
final class StatusItemController {
    private let tracker: TimeTracker
    private let openSettings: @MainActor () -> Void
    private let statusItem: NSStatusItem
    private let popover = NSPopover()
    private let content: NSHostingController<AnyView>

    /// Width of the leading icon, and so of the region that toggles the timer.
    private let iconRegionWidth: CGFloat = 24

    init(tracker: TimeTracker, openSettings: @escaping @MainActor () -> Void) {
        self.tracker = tracker
        self.openSettings = openSettings
        statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)

        content = NSHostingController(
            rootView: AnyView(
                DayView()
                    .environment(tracker)
                    .environment(\.openSettingsWindow, openSettings)
            )
        )
        // Let the hosting controller drive the popover's size as SwiftUI's layout
        // settles, rather than the popover keeping whatever size it was created with.
        content.sizingOptions = [.preferredContentSize]

        popover.behavior = .transient
        popover.animates = false
        popover.contentViewController = content

        if let button = statusItem.button {
            button.target = self
            button.action = #selector(handleClick)
            button.imagePosition = .imageLeading
            button.sendAction(on: [.leftMouseUp, .rightMouseUp])
        }

        observeTracker()
    }

    // MARK: - Rendering

    /// Redraws now and again whenever anything the drawing reads changes.
    private func observeTracker() {
        withObservationTracking {
            render()
        } onChange: { [weak self] in
            // onChange fires before the value is applied, so re-read on the next turn.
            Task { @MainActor in self?.observeTracker() }
        }
    }

    private func render() {
        guard let button = statusItem.button else { return }
        let activity = tracker.activity

        button.image = NSImage(
            systemSymbolName: symbolName(for: activity),
            accessibilityDescription: accessibilityLabel(for: activity)
        )
        button.attributedTitle = title(for: activity)
        button.toolTip = toolTip(for: activity)
    }

    private func symbolName(for activity: TimeTracker.Activity) -> String {
        switch activity {
        case .running: "pause.fill"
        case .recent: "play.fill"
        case .idle: "timer"
        }
    }

    /// Monospaced digits, so a ticking total does not jiggle the whole menu bar.
    private func title(for activity: TimeTracker.Activity) -> NSAttributedString {
        guard let entry = activity.entry else { return NSAttributedString(string: "") }
        let hours = entry.hours(asOf: tracker.now).formattedHours(HoursFormat(company: tracker.company))
        return NSAttributedString(
            string: " \(shortened(entry.project.name))  \(hours)",
            attributes: [
                .font: NSFont.monospacedDigitSystemFont(ofSize: NSFont.systemFontSize, weight: .regular)
            ]
        )
    }

    /// Long project names would crowd out everything else in the menu bar.
    private func shortened(_ name: String, limit: Int = 18) -> String {
        guard name.count > limit else { return name }
        return name.prefix(limit - 1).trimmingCharacters(in: .whitespaces) + "…"
    }

    private func toolTip(for activity: TimeTracker.Activity) -> String {
        switch activity {
        case .running(let entry):
            "Pause \(entry.task.name) — click the icon. Click the name for today’s entries."
        case .recent(let entry):
            "Resume \(entry.task.name) — click the icon. Click the name for today’s entries."
        case .idle:
            "Sheaves — click for today’s entries"
        }
    }

    private func accessibilityLabel(for activity: TimeTracker.Activity) -> String {
        switch activity {
        case .running: "Pause timer"
        case .recent: "Resume timer"
        case .idle: "Sheaves"
        }
    }

    // MARK: - Clicks

    @objc private func handleClick() {
        guard let button = statusItem.button else { return }
        let activity = tracker.activity

        // With nothing to toggle there is no second region, so the whole item opens
        // the popover — otherwise the only clickable thing would do nothing.
        guard let entry = activity.entry, isIconClick(on: button) else {
            togglePopover()
            return
        }

        Task { await tracker.toggle(entry) }
    }

    private func isIconClick(on button: NSStatusBarButton) -> Bool {
        guard let event = NSApp.currentEvent, event.type == .leftMouseUp else { return false }
        return button.convert(event.locationInWindow, from: nil).x <= iconRegionWidth
    }

    // MARK: - Popover

    func togglePopover() {
        popover.isShown ? popover.performClose(nil) : showPopover()
    }

    private func showPopover() {
        guard let button = statusItem.button else { return }

        // NSPopover positions itself from its content size at the moment it is shown.
        // SwiftUI does not report a size until it has laid out, so showing first and
        // letting the content grow afterwards left the popover anchored as though it
        // were tiny — and it expanded off the top of the screen. Force a layout and
        // hand over the real size before it picks a position.
        content.view.layoutSubtreeIfNeeded()
        let fitted = content.view.fittingSize
        if fitted.height > 0 {
            popover.contentSize = fitted
        }

        // An accessory app is not active, and an inactive app's popover cannot take
        // key input — which would leave the search field unable to accept a keystroke.
        // Activating first avoids a second reposition after the window becomes key.
        NSApp.activate(ignoringOtherApps: true)
        popover.show(relativeTo: button.bounds, of: button, preferredEdge: .minY)
        popover.contentViewController?.view.window?.makeKey()
        Task { await tracker.sync() }
    }
}
