import AppKit
import SheavesCore

/// An AppKit entry point rather than a SwiftUI `App`.
///
/// Everything with a window is now owned directly — the status item, its popover,
/// the quick-entry panel, the settings window — because each needed behaviour
/// SwiftUI scenes do not offer: a menu bar button that acts on one click, and
/// windows that can be opened programmatically from a global hotkey. A SwiftUI
/// `App` would contribute only a scene that is never used.
@main
@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    static func main() {
        let app = NSApplication.shared
        let delegate = AppDelegate()
        app.delegate = delegate
        // NSApplication holds its delegate weakly.
        withExtendedLifetime(delegate) {
            app.run()
        }
    }

    let tracker = TimeTracker()
    let hotKeyPreference = HotKeyPreference()

    private var statusItem: StatusItemController?
    private var hotKey: GlobalHotKey?

    private lazy var quickEntry = QuickEntryController(
        tracker: tracker,
        openSettings: { [weak self] in self?.showSettings() }
    )
    private lazy var settings = SettingsWindowController(tracker: tracker, hotKeys: hotKeyPreference)

    func applicationDidFinishLaunching(_ notification: Notification) {
        statusItem = StatusItemController(
            tracker: tracker,
            openSettings: { [weak self] in self?.showSettings() }
        )

        Task {
            await tracker.bootstrap()
            // Nothing in the menu bar is usable without a token, and the popover is
            // easy to miss on a first launch, so ask for one straight away.
            if tracker.connection == .needsCredentials {
                showSettings()
            }
        }

        hotKeyPreference.onChange = { [weak self] shortcut in
            self?.registerHotKey(shortcut)
        }
        registerHotKey(hotKeyPreference.shortcut)
    }

    func showSettings() {
        settings.show()
    }

    private func registerHotKey(_ shortcut: HotKeyShortcut) {
        // Release the old registration first: Carbon refuses a combination that is
        // still held, including the one being replaced.
        hotKey = nil
        // The Carbon callback is not isolated, so hop back to the main actor.
        hotKey = GlobalHotKey(keyCode: shortcut.keyCode, modifiers: shortcut.carbonModifiers) { [weak self] in
            Task { @MainActor in self?.quickEntry.toggle() }
        }
        hotKeyPreference.isRegistered = hotKey != nil
    }
}
