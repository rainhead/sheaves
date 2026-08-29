# Working on Sheaves

A macOS menu bar client for the Harvest time-tracking API. `Packages/SheavesCore`
holds everything but the views so an iOS app can be added later; `Apps/Mac` is the
AppKit and SwiftUI shell.

## Commands

```sh
xcodegen generate                                  # after adding or removing ANY file
swift test --package-path Packages/SheavesCore     # core logic
xcodebuild -project Sheaves.xcodeproj -scheme Sheaves \
  -destination 'platform=macOS' test               # app logic
/usr/bin/log show --last 5m --info \
  --predicate 'subsystem == "com.rainhead.Sheaves"'   # what the app did
```

Signing lives in `Config/Local.xcconfig` (git-ignored, see the example beside it).

## Platform traps that have already cost a day

Each of these produced a bug that shipped past review or a green build that proved
nothing. They are listed with their symptom, because that is how you will meet them.

- **Carbon key-code constants are not ordered.** `kVK_F1` is 122 and `kVK_F12` is
  111, so `case kVK_F1...kVK_F12` builds an invalid range and *traps at runtime* the
  moment the pattern is evaluated — for every key, not only function keys. Look key
  codes up; never match a range of them.
- **An AppKit entry point has no main menu, so ⌘V does nothing.** AppKit routes
  ⌘X/C/V/A/Z to text fields as menu key equivalents. `AppDelegate.installMainMenu`
  exists solely to carry them; an accessory app never shows it. Symptom: pasting
  into a text field silently fails.
- **`MenuBarExtra` cannot be opened programmatically, and `SettingsLink` does not
  work inside its popover** — the link renders as inert text with an I-beam cursor.
  Both are why the status item and its panel are hand-rolled AppKit.
- **`NSPopover` always draws an arrow.** Menu bar extras do not have one, so the
  panel is a borderless `NSPanel`. A titled panel with a hidden title still reserves
  and paints its titlebar strip, which looks like unexplained whitespace.
- **`NSPopover`/`NSPanel` position themselves from their content size at show time,**
  and SwiftUI reports no size until it has laid out. Size first, then position, or
  the panel opens off the top of the screen.
- **The data-protection Keychain needs a team-signed entitlement on macOS.** Asking
  for it in an ad-hoc build fails with `errSecMissingEntitlement` (-34018). macOS
  uses the file Keychain; iOS keeps the data-protection one.
- **`NSScreen.main` is the *key window's* screen,** not the active one. An accessory
  app has no key window when a hotkey fires, so it silently means "primary display".
  Use the screen under `NSEvent.mouseLocation`.
- **Arm window-dismissal watchers a run loop late.** Armed synchronously, the click
  that opened a panel can still be in flight and close it again.

## Testing traps

- **`xcodegen generate` after adding a file.** XcodeGen enumerates at generation
  time, so a new test file is silently absent from the target.
- **"Executed 0 tests" with `TEST SUCCEEDED` is a failure.** It means nothing ran.
- **System Events' `keystroke`/`key code` does not trigger a Carbon global hotkey.**
  Post a real `CGEvent` to `.cghidEventTap` instead. A synthetic click also trips
  global event monitors that a real click in this app would not.
- **Assert the request, not the request count.** Several tests here once passed
  because a *discarded* mutation looked identical to a delivered one. Check the
  method and body.

## Conventions

- Anything a person reads goes through a locale-aware format style, never
  `String(format:)`. The sole exception is `CalendarDate.description`, which is the
  wire format Harvest parses and must stay ASCII.
- Days are `CalendarDate`, never `Date`. Harvest's `spent_date` has no time zone.
- Mutations that affect elapsed time must carry the time the user acted. See the
  README's sync section for why.
