import AppKit
import SheavesCore
import SwiftUI

/// The Spotlight-style panel the global hotkey summons.
///
/// A floating panel rather than the menu bar popover, because SwiftUI's
/// `MenuBarExtra` cannot be opened programmatically — and a centred panel is a
/// better target for a hotkey anyway.
@MainActor
final class QuickEntryController {
    private let tracker: TimeTracker
    private let openSettings: @MainActor () -> Void
    private var panel: NSPanel?
    private var content: NSHostingController<AnyView>?

    init(tracker: TimeTracker, openSettings: @escaping @MainActor () -> Void) {
        self.tracker = tracker
        self.openSettings = openSettings
    }

    func toggle() {
        if panel?.isVisible == true {
            hide()
        } else {
            show()
        }
    }

    func show() {
        let panel = panel ?? makePanel()
        self.panel = panel

        // Size before positioning: the panel is centred, so where it goes depends on
        // how tall the content turns out to be.
        if let content {
            content.view.layoutSubtreeIfNeeded()
            let fitted = content.view.fittingSize
            if fitted.width > 0, fitted.height > 0 {
                panel.setContentSize(fitted)
            }
        }
        centre(panel)
        NSApp.activate(ignoringOtherApps: true)
        panel.makeKeyAndOrderFront(nil)
        Task { await tracker.sync() }
    }

    func hide() {
        panel?.orderOut(nil)
    }

    private func makePanel() -> NSPanel {
        let panel = FloatingPanel(
            contentRect: NSRect(x: 0, y: 0, width: 520, height: 200),
            // Borderless, not titled: a titled window reserves and paints a titlebar
            // strip even with the title hidden and the bar transparent, which showed
            // up as unexplained whitespace above the content. This also matches the
            // menu bar panel, which had no business looking like a different app.
            styleMask: [.borderless, .nonactivatingPanel, .fullSizeContentView],
            backing: .buffered,
            defer: false
        )
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = true
        panel.isMovableByWindowBackground = true
        panel.level = .floating
        panel.hidesOnDeactivate = true
        panel.isReleasedWhenClosed = false
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]

        let root = QuickEntryView(onDismiss: { [weak self] in self?.hide() })
            .environment(tracker)
            .environment(\.openSettingsWindow) { [weak self] in
                self?.hide()
                self?.openSettings()
            }
            .background(.regularMaterial)
            .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))

        let hosting = NSHostingController(rootView: AnyView(root))
        hosting.sizingOptions = [.preferredContentSize]
        panel.contentViewController = hosting
        content = hosting
        return panel
    }

    private func centre(_ panel: NSPanel) {
        // Not NSScreen.main: that is the screen holding the *key window*, and this
        // app has none when the hotkey fires from another app — so it falls back to
        // the primary display and the panel opens on the wrong monitor. The pointer
        // is the best available signal for where the user is working.
        let pointer = NSEvent.mouseLocation
        let target = NSScreen.screens.first { $0.frame.contains(pointer) }
            ?? NSScreen.main
        guard let screen = target?.visibleFrame else { return }
        let size = panel.frame.size
        panel.setFrameOrigin(
            NSPoint(
                x: screen.midX - size.width / 2,
                // Slightly above centre reads better than dead centre, as Spotlight does.
                y: screen.midY - size.height / 2 + screen.height * 0.1
            )
        )
    }
}

/// A panel has to opt in to becoming key, or the search field never takes keystrokes.
private final class FloatingPanel: NSPanel {
    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { true }

    override func cancelOperation(_ sender: Any?) {
        orderOut(nil)
    }
}

struct QuickEntryView: View {
    @Environment(TimeTracker.self) private var tracker
    var onDismiss: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            header

            if tracker.connection == .needsCredentials {
                ConnectPrompt()
            } else {
                TargetPicker(placeholder: "Start a timer…", maxVisible: 5) { target in
                    Task { await tracker.start(target) }
                    onDismiss()
                }
            }

            Divider()
            SyncStatusLabel()
                .font(.caption)
        }
        .padding(16)
        .frame(width: 520, alignment: .topLeading)
        .fixedSize(horizontal: false, vertical: true)
        .onExitCommand(perform: onDismiss)
    }

    @ViewBuilder
    private var header: some View {
        if let running = tracker.runningEntry {
            HStack(spacing: 8) {
                Image(systemName: "timer")
                    .foregroundStyle(.tint)
                VStack(alignment: .leading, spacing: 0) {
                    Text(running.task.name).font(.callout).fontWeight(.semibold)
                    Text(running.target.projectLabel).font(.caption).foregroundStyle(.secondary)
                }
                Spacer()
                Text(running.hours(asOf: tracker.now).formattedHours(HoursFormat(company: tracker.company)))
                    .font(.callout.monospacedDigit())
                Button("Stop") {
                    Task { await tracker.stopRunning() }
                    onDismiss()
                }
            }
            .padding(.bottom, 2)
        } else {
            Text("Nothing running · \(tracker.totalHours.formattedHours(HoursFormat(company: tracker.company))) today")
                .font(.callout)
                .foregroundStyle(.secondary)
        }
    }
}
