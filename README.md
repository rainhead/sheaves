# Sheaves

A native SwiftUI menu bar client for [Harvest](https://www.getharvest.com) time tracking.

The clock you are on is always visible in the menu bar, ⌃⌥⌘T starts a timer from
anywhere, and nothing blocks on the network — Sheaves shows local state immediately
and reconciles with Harvest afterwards.

## Status

Early. The macOS app tracks time; iOS is not built yet, but everything except the
views lives in a platform-neutral package so it can be.

## Running it

Requires Xcode 26 or later.

```sh
brew install xcodegen     # the .xcodeproj is generated, not committed
xcodegen generate
open Sheaves.xcodeproj
```

The project signs ad-hoc so it runs without an Apple Developer account. That
signature changes on every rebuild, so macOS re-asks for Keychain access each time;
setting `DEVELOPMENT_TEAM` in [`project.yml`](project.yml) to your team ID stops that.

Then create a personal access token at
[id.getharvest.com/developers](https://id.getharvest.com/developers) and paste it,
with the account ID shown beside it, into Sheaves' settings. The token is stored in
the Keychain and sent only to `api.harvestapp.com`.

```sh
swift test --package-path Packages/SheavesCore
```

## How it works

Three ideas carry most of the design.

**A day is not a moment.** Harvest's `spent_date` is a date with no time zone, and
round-tripping it through `Date` puts entries on the wrong day for anyone east or
west of wherever the conversion happened. [`CalendarDate`](Packages/SheavesCore/Sources/SheavesCore/Model/CalendarDate.swift)
keeps days as days and only becomes a `Date` against an explicit calendar.

**Local state is the truth until a sync succeeds.** Every action — start, stop,
resume, edit — changes [`TimeTracker`](Packages/SheavesCore/Sources/SheavesCore/Store/TimeTracker.swift)
first and reaches Harvest second, through a persisted
[`MutationQueue`](Packages/SheavesCore/Sources/SheavesCore/Store/MutationQueue.swift).
The queue drains in order and stops at the first failure a retry could fix, so a
dropped connection costs nothing; a change Harvest refuses outright is dropped rather
than left to wedge everything behind it. An entry started offline has no Harvest id
yet, which is why [`TrackedEntry`](Packages/SheavesCore/Sources/SheavesCore/Model/TrackedEntry.swift)
identity is either a server id or a local one, swapped when the create lands.

**The UI never waits to draw.** A JSON
[snapshot](Packages/SheavesCore/Sources/SheavesCore/Persistence/SnapshotStore.swift)
of the last known state is restored at launch, so the popover is populated before the
first request returns.

The global hotkey uses Carbon's `RegisterEventHotKey`
([`GlobalHotKey`](Apps/Mac/Sources/GlobalHotKey.swift)) rather than an `NSEvent`
monitor: it needs no Accessibility permission and can actually consume the key. It is
rebindable in Settings, and says so when another app already owns the combination —
Carbon reports that by failing silently, which is worth surfacing.

## Layout

| Path | What lives there |
| --- | --- |
| [`Packages/SheavesCore`](Packages/SheavesCore) | API client, models, store, persistence. No AppKit, no SwiftUI. |
| [`Apps/Mac`](Apps/Mac) | The menu bar app: popover, quick-entry panel, settings. |
| [`project.yml`](project.yml) | Targets and build settings; the `.xcodeproj` is generated from it. |

## Keyboard

| | |
| --- | --- |
| ⌃⌥⌘T | Quick entry, from any app (rebindable in Settings) |
| ↑ ↓ | Move through matches |
| ⇥ | Jump to the notes field |
| ⏎ | Start the selected timer |
| ⌘← ⌘→ | Previous / next day |

## Licence

MIT. See [LICENSE](LICENSE).
