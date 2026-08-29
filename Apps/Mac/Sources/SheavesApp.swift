import AppKit
import OSLog
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
    let loginItem = LoginItemPreference()

    private static let log = Logger(subsystem: "com.rainhead.Sheaves", category: "hotkey")

    private var statusItem: StatusItemController?
    private var hotKey: GlobalHotKey?

    private lazy var quickEntry = QuickEntryController(
        tracker: tracker,
        openSettings: { [weak self] in self?.showSettings() }
    )
    private lazy var settings = SettingsWindowController(
        tracker: tracker,
        hotKeys: hotKeyPreference,
        loginItem: loginItem
    )

    /// True when the app was launched only to host a unit test bundle.
    private var isHostingTests: Bool {
        ProcessInfo.processInfo.environment["XCTestConfigurationFilePath"] != nil
            || ProcessInfo.processInfo.environment["XCTestBundlePath"] != nil
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        // Tests link against this app, so launching it must not claim a menu bar
        // slot, register a global hotkey or reach for the Keychain.
        guard !isHostingTests else { return }

        installMainMenu()

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

    @objc func showSettings() {
        settings.show()
    }

    /// Installs a main menu even though an accessory app never displays one.
    ///
    /// AppKit routes ⌘X/⌘C/⌘V/⌘A/⌘Z to text fields as *menu key equivalents* —
    /// NSTextField does not implement them itself. A SwiftUI `App` builds this menu
    /// for you; a hand-rolled AppKit entry point does not, and without it pasting a
    /// Harvest token into Settings silently does nothing, which is the very first
    /// thing anyone has to do.
    private func installMainMenu() {
        let mainMenu = NSMenu()

        let appItem = NSMenuItem()
        let appMenu = NSMenu()
        appMenu.addItem(withTitle: "Settings…", action: #selector(showSettings), keyEquivalent: ",")
        appMenu.addItem(.separator())
        appMenu.addItem(
            withTitle: "Quit Sheaves",
            action: #selector(NSApplication.terminate(_:)),
            keyEquivalent: "q"
        )
        appItem.submenu = appMenu
        mainMenu.addItem(appItem)

        let editItem = NSMenuItem()
        let editMenu = NSMenu(title: "Edit")
        editMenu.addItem(withTitle: "Undo", action: Selector(("undo:")), keyEquivalent: "z")
        let redo = NSMenuItem(title: "Redo", action: Selector(("redo:")), keyEquivalent: "z")
        redo.keyEquivalentModifierMask = [.command, .shift]
        editMenu.addItem(redo)
        editMenu.addItem(.separator())
        editMenu.addItem(withTitle: "Cut", action: #selector(NSText.cut(_:)), keyEquivalent: "x")
        editMenu.addItem(withTitle: "Copy", action: #selector(NSText.copy(_:)), keyEquivalent: "c")
        editMenu.addItem(withTitle: "Paste", action: #selector(NSText.paste(_:)), keyEquivalent: "v")
        editMenu.addItem(
            withTitle: "Select All",
            action: #selector(NSText.selectAll(_:)),
            keyEquivalent: "a"
        )
        editItem.submenu = editMenu
        mainMenu.addItem(editItem)

        let windowItem = NSMenuItem()
        let windowMenu = NSMenu(title: "Window")
        windowMenu.addItem(
            withTitle: "Close",
            action: #selector(NSWindow.performClose(_:)),
            keyEquivalent: "w"
        )
        windowItem.submenu = windowMenu
        mainMenu.addItem(windowItem)

        NSApp.mainMenu = mainMenu
    }

    private func registerHotKey(_ shortcut: HotKeyShortcut) {
        // Release the old registration first: Carbon refuses a combination that is
        // still held, including the one being replaced.
        hotKey = nil
        // The Carbon callback is not isolated, so hop back to the main actor.
        hotKey = GlobalHotKey(keyCode: shortcut.keyCode, modifiers: shortcut.carbonModifiers) { [weak self] in
            Task { @MainActor in self?.quickEntry.toggle() }
        }
        let registered = hotKey != nil
        hotKeyPreference.isRegistered = registered
        // Carbon reports a taken combination by failing silently; Settings shows this,
        // but the log is the only trace when nobody is looking at Settings.
        Self.log.info(
            "quick entry hotkey \(shortcut.display, privacy: .public) registered: \(registered, privacy: .public)"
        )
    }
}
