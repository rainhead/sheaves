import AppKit
import Carbon.HIToolbox

/// A key combination, stored the way Carbon wants it and displayed the way the user
/// typed it.
///
/// The label is captured at record time from `charactersIgnoringModifiers` rather than
/// derived from the key code later. That keeps the display honest on non-QWERTY
/// layouts without dragging in `UCKeyTranslate`.
struct HotKeyShortcut: Codable, Sendable, Hashable {
    var keyCode: UInt32
    var carbonModifiers: UInt32
    var label: String

    static let quickEntryDefault = HotKeyShortcut(
        keyCode: UInt32(kVK_ANSI_T),
        carbonModifiers: UInt32(controlKey | optionKey | cmdKey),
        label: "T"
    )

    /// `⌃⌥⌘T`, in the order macOS renders modifiers.
    var display: String {
        var symbols = ""
        if carbonModifiers & UInt32(controlKey) != 0 { symbols += "⌃" }
        if carbonModifiers & UInt32(optionKey) != 0 { symbols += "⌥" }
        if carbonModifiers & UInt32(shiftKey) != 0 { symbols += "⇧" }
        if carbonModifiers & UInt32(cmdKey) != 0 { symbols += "⌘" }
        return symbols + label
    }

    /// Builds a shortcut from a key event, or nil if it could not be a global hotkey.
    ///
    /// At least one of ⌃⌥⌘ is required: registering a bare letter, or a
    /// shift-only combination, would swallow that key in every other app.
    init?(event: NSEvent) {
        let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        var modifiers: UInt32 = 0
        if flags.contains(.command) { modifiers |= UInt32(cmdKey) }
        if flags.contains(.option) { modifiers |= UInt32(optionKey) }
        if flags.contains(.control) { modifiers |= UInt32(controlKey) }
        if flags.contains(.shift) { modifiers |= UInt32(shiftKey) }

        let requiresOneOf = UInt32(cmdKey) | UInt32(optionKey) | UInt32(controlKey)
        guard modifiers & requiresOneOf != 0 else { return nil }

        let typed = event.charactersIgnoringModifiers ?? ""
        guard let label = Self.label(for: event.keyCode, typed: typed) else { return nil }

        self.keyCode = UInt32(event.keyCode)
        self.carbonModifiers = modifiers
        self.label = label
    }

    init(keyCode: UInt32, carbonModifiers: UInt32, label: String) {
        self.keyCode = keyCode
        self.carbonModifiers = carbonModifiers
        self.label = label
    }

    /// Keys with no printable character need a symbol of their own.
    private static func label(for keyCode: UInt16, typed: String) -> String? {
        // Checked by lookup, not by range: Carbon's function-key codes are neither
        // contiguous nor ascending (kVK_F1 is 122, kVK_F12 is 111), so `kVK_F1...kVK_F12`
        // is an invalid range that traps the moment the pattern is evaluated.
        if let number = functionKeyNumber(keyCode) { return "F\(number)" }

        switch Int(keyCode) {
        case kVK_Space: return "␣"
        case kVK_Return, kVK_ANSI_KeypadEnter: return "⏎"
        case kVK_Tab: return "⇥"
        case kVK_Delete: return "⌫"
        case kVK_LeftArrow: return "←"
        case kVK_RightArrow: return "→"
        case kVK_UpArrow: return "↑"
        case kVK_DownArrow: return "↓"
        case kVK_Escape: return nil  // reserved for cancelling the recorder
        default:
            let trimmed = typed.trimmingCharacters(in: .whitespacesAndNewlines).uppercased()
            return trimmed.isEmpty ? nil : trimmed
        }
    }

    private static func functionKeyNumber(_ keyCode: UInt16) -> Int? {
        let order: [Int] = [
            kVK_F1, kVK_F2, kVK_F3, kVK_F4, kVK_F5, kVK_F6, kVK_F7, kVK_F8,
            kVK_F9, kVK_F10, kVK_F11, kVK_F12, kVK_F13, kVK_F14, kVK_F15,
            kVK_F16, kVK_F17, kVK_F18, kVK_F19, kVK_F20,
        ]
        return order.firstIndex(of: Int(keyCode)).map { $0 + 1 }
    }
}

/// The stored quick-entry shortcut, and whether the system accepted it.
///
/// `RegisterEventHotKey` fails when another app already owns the combination, and it
/// fails silently — so the outcome is surfaced here for Settings to show.
@MainActor
@Observable
final class HotKeyPreference {
    private static let defaultsKey = "quickEntryShortcut"

    var shortcut: HotKeyShortcut {
        didSet {
            guard shortcut != oldValue else { return }
            persist()
            onChange?(shortcut)
        }
    }

    /// Nil until a registration has been attempted.
    var isRegistered: Bool?

    /// Set by whoever owns the actual hotkey registration.
    var onChange: ((HotKeyShortcut) -> Void)?

    private let defaults: UserDefaults

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        if let data = defaults.data(forKey: Self.defaultsKey),
           let stored = try? JSONDecoder().decode(HotKeyShortcut.self, from: data) {
            shortcut = stored
        } else {
            shortcut = .quickEntryDefault
        }
    }

    func resetToDefault() {
        shortcut = .quickEntryDefault
    }

    private func persist() {
        guard let data = try? JSONEncoder().encode(shortcut) else { return }
        defaults.set(data, forKey: Self.defaultsKey)
    }
}
