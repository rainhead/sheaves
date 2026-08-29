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
        } label: {
            MenuBarLabel(tracker: delegate.tracker)
        }
        .menuBarExtraStyle(.window)

        Settings {
            SettingsView()
                .environment(delegate.tracker)
        }
    }
}

/// Owns the model and the things SwiftUI scenes cannot: the global hotkey and the
/// floating quick-entry panel.
@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    let tracker = TimeTracker()

    private lazy var quickEntry = QuickEntryController(tracker: tracker)
    private var hotKey: GlobalHotKey?

    func applicationDidFinishLaunching(_ notification: Notification) {
        Task { await tracker.bootstrap() }

        // The Carbon callback is not isolated, so hop back to the main actor.
        hotKey = GlobalHotKey.quickEntryDefault { [weak self] in
            Task { @MainActor in self?.quickEntry.toggle() }
        }
    }
}
