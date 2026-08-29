import AppKit
import SheavesCore
import SwiftUI

@main
struct SheavesApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate

    var body: some Scene {
        MenuBarExtra {
            DayView()
                .environment(delegate.tracker)
                .environment(\.openSettingsWindow) { delegate.showSettings() }
        } label: {
            MenuBarLabel(tracker: delegate.tracker)
        }
        .menuBarExtraStyle(.window)
    }
}

/// Owns the model and the things SwiftUI scenes cannot: the global hotkey and the
/// floating quick-entry panel.
@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    let tracker = TimeTracker()
    let hotKeyPreference = HotKeyPreference()

    private lazy var quickEntry = QuickEntryController(
        tracker: tracker,
        openSettings: { [weak self] in self?.showSettings() }
    )
    private lazy var settings = SettingsWindowController(tracker: tracker, hotKeys: hotKeyPreference)
    private var hotKey: GlobalHotKey?

    func showSettings() {
        settings.show()
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
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
