import AppKit
import Carbon.HIToolbox
import Testing
@testable import Sheaves

/// The recorder crashed on every key press before these existed: Carbon's function
/// key codes are neither contiguous nor ascending, so `case kVK_F1...kVK_F12` built
/// an invalid range and trapped as soon as the pattern was evaluated. Nothing caught
/// it because the app had no tests at all.
@Suite("HotKeyShortcut")
struct HotKeyShortcutTests {
    private func event(
        keyCode: Int,
        modifiers: NSEvent.ModifierFlags,
        characters: String = "t"
    ) -> NSEvent {
        NSEvent.keyEvent(
            with: .keyDown,
            location: .zero,
            modifierFlags: modifiers,
            timestamp: 0,
            windowNumber: 0,
            context: nil,
            characters: characters,
            charactersIgnoringModifiers: characters,
            isARepeat: false,
            keyCode: UInt16(keyCode)
        )!
    }

    /// The regression test for the crash: every key code a keyboard can report,
    /// with modifiers held, must be handled without trapping.
    @Test("survives every key code")
    func handlesEveryKeyCode() {
        for code in 0...255 {
            _ = HotKeyShortcut(event: event(keyCode: code, modifiers: [.command, .option, .control]))
            _ = HotKeyShortcut(event: event(keyCode: code, modifiers: [.command], characters: ""))
        }
    }

    @Test("labels function keys by number, not by range order")
    func labelsFunctionKeys() throws {
        let f1 = try #require(HotKeyShortcut(event: event(keyCode: kVK_F1, modifiers: [.command])))
        let f12 = try #require(HotKeyShortcut(event: event(keyCode: kVK_F12, modifiers: [.command])))
        let f20 = try #require(HotKeyShortcut(event: event(keyCode: kVK_F20, modifiers: [.command])))
        #expect(f1.label == "F1")
        #expect(f12.label == "F12")
        #expect(f20.label == "F20")
    }

    @Test("names keys that print nothing")
    func labelsSpecialKeys() throws {
        let cases: [(Int, String)] = [
            (kVK_Space, "␣"), (kVK_Return, "⏎"), (kVK_Tab, "⇥"),
            (kVK_Delete, "⌫"), (kVK_LeftArrow, "←"), (kVK_UpArrow, "↑"),
        ]
        for (code, expected) in cases {
            let shortcut = try #require(
                HotKeyShortcut(event: event(keyCode: code, modifiers: [.command], characters: ""))
            )
            #expect(shortcut.label == expected)
        }
    }

    /// Registering a bare letter would swallow that key in every other app.
    @Test("requires a modifier that is not shift alone")
    func requiresRealModifier() {
        #expect(HotKeyShortcut(event: event(keyCode: kVK_ANSI_T, modifiers: [])) == nil)
        #expect(HotKeyShortcut(event: event(keyCode: kVK_ANSI_T, modifiers: [.shift])) == nil)
        #expect(HotKeyShortcut(event: event(keyCode: kVK_ANSI_T, modifiers: [.command])) != nil)
        #expect(HotKeyShortcut(event: event(keyCode: kVK_ANSI_T, modifiers: [.control])) != nil)
        #expect(HotKeyShortcut(event: event(keyCode: kVK_ANSI_T, modifiers: [.option])) != nil)
    }

    /// Escape cancels recording, so it can never become a shortcut.
    @Test("refuses escape")
    func refusesEscape() {
        #expect(HotKeyShortcut(event: event(keyCode: kVK_Escape, modifiers: [.command], characters: "")) == nil)
    }

    @Test("renders modifiers in the order macOS writes them")
    func rendersDisplayString() throws {
        let shortcut = try #require(
            HotKeyShortcut(
                event: event(keyCode: kVK_ANSI_T, modifiers: [.command, .option, .control, .shift])
            )
        )
        #expect(shortcut.display == "⌃⌥⇧⌘T")
        #expect(HotKeyShortcut.quickEntryDefault.display == "⌃⌥⌘T")
    }

    /// The label comes from what the user typed, so a non-QWERTY layout shows the key
    /// they actually pressed rather than the one at that position on a US keyboard.
    @Test("labels from the character typed, uppercased")
    func labelsFromTypedCharacter() throws {
        let shortcut = try #require(
            HotKeyShortcut(event: event(keyCode: kVK_ANSI_Z, modifiers: [.command], characters: "w"))
        )
        #expect(shortcut.label == "W")
    }

    @Test("round-trips through its stored form")
    func roundTripsCoding() throws {
        let original = HotKeyShortcut.quickEntryDefault
        let data = try JSONEncoder().encode(original)
        #expect(try JSONDecoder().decode(HotKeyShortcut.self, from: data) == original)
    }
}

@Suite("HotKeyPreference")
@MainActor
struct HotKeyPreferenceTests {
    private func scratchDefaults() -> UserDefaults {
        let suite = "com.rainhead.Sheaves.tests.\(UUID().uuidString)"
        return UserDefaults(suiteName: suite)!
    }

    @Test("starts on the documented default")
    func defaultsToQuickEntry() {
        #expect(HotKeyPreference(defaults: scratchDefaults()).shortcut == .quickEntryDefault)
    }

    @Test("remembers a new shortcut and tells its owner to re-register")
    func persistsAndNotifies() throws {
        let defaults = scratchDefaults()
        let preference = HotKeyPreference(defaults: defaults)
        var registered: [HotKeyShortcut] = []
        preference.onChange = { registered.append($0) }

        let f5 = HotKeyShortcut(keyCode: UInt32(kVK_F5), carbonModifiers: UInt32(cmdKey), label: "F5")
        preference.shortcut = f5

        #expect(registered == [f5])
        #expect(HotKeyPreference(defaults: defaults).shortcut == f5)

        preference.resetToDefault()
        #expect(preference.shortcut == .quickEntryDefault)
    }

    /// Re-selecting the same combination should not tear down a working registration.
    @Test("ignores a change to the same shortcut")
    func ignoresNoOpChange() {
        let preference = HotKeyPreference(defaults: scratchDefaults())
        var changes = 0
        preference.onChange = { _ in changes += 1 }
        preference.shortcut = .quickEntryDefault
        #expect(changes == 0)
    }
}
