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
    private let panel: MenuBarPanel
    private let content: NSHostingController<AnyView>
    private var dismissObservers: [any NSObjectProtocol] = []
    private var globalClickMonitor: Any?
    /// True while this app is tracking a menu of its own — the picker's task
    /// dropdown, for one.
    ///
    /// The watchers below cannot tell our menu from anyone else's: HIToolbox
    /// broadcasts menu tracking system-wide, which is the whole reason it is
    /// observed, and the broadcast our dropdown causes is indistinguishable from
    /// Safari's. NSMenu posts locally when it begins and ends, and that is the
    /// signal that says the menu is ours.
    private var isTrackingOwnMenu = false
    /// Clicking the status item makes it key, which can close the panel a moment
    /// before the click action runs — without this the item would reopen what the
    /// same click just dismissed.
    private var lastHiddenAt = Date.distantPast

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
                    .background(.regularMaterial)
                    .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
            )
        )
        content.sizingOptions = [.preferredContentSize]
        panel = MenuBarPanel(contentViewController: content)

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
            accessibilityDescription: nil
        )
        button.attributedTitle = title(for: activity)
        button.toolTip = toolTip(for: activity)
        // Spoken, not read as digits: "0:19" becomes "zero nineteen" otherwise. The
        // label describes state rather than promising an action, because an
        // accessibility press cannot hit the icon region and so opens the panel.
        button.setAccessibilityLabel(accessibilityLabel(for: activity))
        button.setAccessibilityValue(spokenState(for: activity))
    }

    private func accessibilityLabel(for activity: TimeTracker.Activity) -> String {
        guard let entry = activity.entry else { return "Sheaves, nothing tracked" }
        return "Sheaves, \(entry.task.name), \(entry.target.projectLabel)"
    }

    private func spokenState(for activity: TimeTracker.Activity) -> String {
        guard let entry = activity.entry else { return "no timer running" }
        let total = Int((entry.hours(asOf: tracker.now) * 60).rounded())
        let duration = total < 60
            ? "\(total.formatted()) minute\(total == 1 ? "" : "s")"
            : "\(( total / 60).formatted()) hour\(total / 60 == 1 ? "" : "s") \((total % 60).formatted()) minute\(total % 60 == 1 ? "" : "s")"
        return activity.entry.map { _ in
            "\(duration), \(entry.isRunning ? "timer running" : "paused")"
        } ?? duration
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

    // MARK: - Clicks

    @objc private func handleClick() {
        guard let button = statusItem.button else { return }
        let activity = tracker.activity

        if NSApp.currentEvent?.type == .rightMouseUp {
            showMenu(for: activity)
            return
        }

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

    /// The conventional menu bar extra right-click menu — and the only route to
    /// pause, resume or quit that does not require a mouse aimed at a 24-point strip.
    private func showMenu(for activity: TimeTracker.Activity) {
        let menu = NSMenu()

        if let entry = activity.entry {
            let title = entry.isRunning ? "Pause \(entry.task.name)" : "Resume \(entry.task.name)"
            let item = NSMenuItem(title: title, action: #selector(toggleTimer), keyEquivalent: "")
            item.target = self
            menu.addItem(item)
            menu.addItem(.separator())
        }

        let open = NSMenuItem(title: "Open Sheaves", action: #selector(openPanel), keyEquivalent: "")
        open.target = self
        menu.addItem(open)

        let settings = NSMenuItem(title: "Settings…", action: #selector(openSettingsItem), keyEquivalent: ",")
        settings.target = self
        menu.addItem(settings)

        menu.addItem(.separator())
        menu.addItem(
            NSMenuItem(title: "Quit Sheaves", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        )

        // Attaching the menu makes the next click open it; clearing it afterwards
        // keeps left-click on the button's own action.
        statusItem.menu = menu
        statusItem.button?.performClick(nil)
        statusItem.menu = nil
    }

    @objc private func toggleTimer() {
        guard let entry = tracker.activity.entry else { return }
        Task { await tracker.toggle(entry) }
    }

    @objc private func openPanel() {
        showPanel()
    }

    @objc private func openSettingsItem() {
        openSettings()
    }

    // MARK: - Panel

    func togglePopover() {
        panel.isVisible ? hidePanel() : showPanel()
    }

    private func showPanel() {
        guard Date().timeIntervalSince(lastHiddenAt) > 0.2 else { return }
        guard let button = statusItem.button, let buttonWindow = button.window else { return }

        // Size before positioning: the panel hangs from the menu bar downwards, so
        // its origin depends on how tall the content turns out to be.
        content.view.layoutSubtreeIfNeeded()
        let fitted = content.view.fittingSize
        if fitted.width > 0, fitted.height > 0 {
            panel.setContentSize(fitted)
        }

        let anchor = buttonWindow.convertToScreen(button.convert(button.bounds, to: nil))
        panel.setFrameOrigin(origin(below: anchor))

        // An accessory app is not active, and an inactive app's window cannot take
        // key input — the search field would swallow every keystroke.
        NSApp.activate(ignoringOtherApps: true)
        panel.makeKeyAndOrderFront(nil)
        // Arm dismissal on the next turn of the run loop. Armed synchronously, the
        // very click that opened the panel can still be in flight and close it again.
        Task { @MainActor in
            guard self.panel.isVisible else { return }
            self.installDismissWatchers()
        }
        Task { await tracker.sync() }
    }

    func hidePanel() {
        guard panel.isVisible else { return }
        panel.orderOut(nil)
        lastHiddenAt = Date()
        removeDismissWatchers()
    }

    /// Menu bar panels sit just under the bar, centred on their item, and never hang
    /// off the edge of the screen.
    private func origin(below anchor: NSRect) -> NSPoint {
        let screen = statusItem.button?.window?.screen ?? NSScreen.main
        let visible = screen?.visibleFrame ?? .zero
        let size = panel.frame.size
        let margin: CGFloat = 8

        let x = min(
            max(anchor.midX - size.width / 2, visible.minX + margin),
            visible.maxX - size.width - margin
        )
        return NSPoint(x: x, y: anchor.minY - size.height - 2)
    }

    /// Menu bar panels close as soon as attention moves anywhere else.
    ///
    /// Losing key focus is not enough on its own: another app's menu bar menu opens
    /// inside its own event loop, and this panel never resigns key while that
    /// happens. HIToolbox broadcasts menu tracking system-wide, which covers those;
    /// a global click monitor covers menu bar extras that are panels rather than
    /// menus, since those produce an ordinary mouse-down in another process.
    ///
    /// Both are indiscriminate, and this panel now contains a menu of its own, so
    /// `isTrackingOwnMenu` is what keeps opening the task dropdown from reading as
    /// attention moving away.
    private func installDismissWatchers() {
        guard dismissObservers.isEmpty else { return }
        // A sheet on the panel — a delete confirmation, the resume-on-today offer —
        // makes the panel resign key to its own dialog, which must not read as
        // attention moving away: hiding the panel would take the question with it.
        let close: @Sendable (Notification) -> Void = { [weak self] _ in
            MainActor.assumeIsolated {
                guard let self, !self.isTrackingOwnMenu, self.panel.attachedSheet == nil
                else { return }
                self.hidePanel()
            }
        }

        dismissObservers.append(
            NotificationCenter.default.addObserver(
                forName: NSMenu.didBeginTrackingNotification, object: nil, queue: .main
            ) { [weak self] _ in
                MainActor.assumeIsolated { self?.isTrackingOwnMenu = true }
            }
        )
        dismissObservers.append(
            NotificationCenter.default.addObserver(
                forName: NSMenu.didEndTrackingNotification, object: nil, queue: .main
            ) { [weak self] _ in
                MainActor.assumeIsolated { self?.endMenuTracking() }
            }
        )

        dismissObservers.append(
            NotificationCenter.default.addObserver(
                forName: NSWindow.didResignKeyNotification, object: panel, queue: .main, using: close
            )
        )
        dismissObservers.append(
            DistributedNotificationCenter.default().addObserver(
                forName: NSNotification.Name("com.apple.HIToolbox.beginMenuTrackingNotification"),
                object: nil,
                queue: .main,
                using: close
            )
        )

        // Global monitors see only other applications' events, so clicking inside
        // this panel or on the status item does not trip it.
        globalClickMonitor = NSEvent.addGlobalMonitorForEvents(
            matching: [.leftMouseDown, .rightMouseDown, .otherMouseDown]
        ) { [weak self] _ in
            Task { @MainActor in
                guard let self, !self.isTrackingOwnMenu, self.panel.attachedSheet == nil
                else { return }
                self.hidePanel()
            }
        }
    }

    /// Stops treating a menu as ours, a run loop turn late: the system-wide broadcast
    /// for the menu that just closed can still be in flight behind the local
    /// notification that closed it, and it would land on a cleared flag and shut the
    /// panel that the menu belongs to.
    private func endMenuTracking() {
        Task { @MainActor [weak self] in
            self?.isTrackingOwnMenu = false
        }
    }

    private func removeDismissWatchers() {
        for observer in dismissObservers {
            NotificationCenter.default.removeObserver(observer)
            DistributedNotificationCenter.default().removeObserver(observer)
        }
        dismissObservers.removeAll()
        if let globalClickMonitor {
            NSEvent.removeMonitor(globalClickMonitor)
            self.globalClickMonitor = nil
        }
        // Never leave this set: a stuck flag is a panel that will not close.
        isTrackingOwnMenu = false
    }
}

/// A borderless panel that looks like every other menu bar extra.
///
/// `NSPopover` was the obvious choice and the wrong one: it always draws an arrow
/// pointing at its anchor, and nothing else in the menu bar has one.
private final class MenuBarPanel: NSPanel {
    init(contentViewController: NSViewController) {
        super.init(
            contentRect: NSRect(x: 0, y: 0, width: 400, height: 400),
            styleMask: [.borderless, .nonactivatingPanel, .fullSizeContentView],
            backing: .buffered,
            defer: false
        )
        self.contentViewController = contentViewController
        isOpaque = false
        backgroundColor = .clear
        hasShadow = true
        level = .popUpMenu
        isMovable = false
        isReleasedWhenClosed = false
        animationBehavior = .utilityWindow
        collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
    }

    // Borderless panels refuse key by default, which would leave the search field
    // unable to take a keystroke.
    override var canBecomeKey: Bool { true }

    override func cancelOperation(_ sender: Any?) {
        orderOut(nil)
    }
}
