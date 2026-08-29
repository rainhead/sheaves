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

The project signs ad-hoc, so a fresh checkout builds and runs with no Apple
Developer account. The catch is that an ad-hoc signature changes with every build,
so macOS treats each rebuild as a different app and re-prompts for the Keychain item
holding your token. To stop that, copy
[`Config/Local.example.xcconfig`](Config/Local.example.xcconfig) to
`Config/Local.xcconfig` (git-ignored) and fill in your team ID. A **free** Apple ID
is enough — the $99 Developer Program is for distribution, not for running your own
app on your own Mac.

Then create a personal access token at
[id.getharvest.com/developers](https://id.getharvest.com/developers) and paste it,
with the account ID shown beside it, into Sheaves' settings. The token is stored in
the Keychain and sent only to `api.harvestapp.com`.

```sh
swift test --package-path Packages/SheavesCore              # core logic
xcodebuild -project Sheaves.xcodeproj -scheme Sheaves \
  -destination 'platform=macOS' test                        # app logic
```

The core suite is the bulk of it. The app suite exists because the parts that only
compile against AppKit had no tests at all, and that is where the worst bugs turned
up — a shortcut recorder that trapped on every key press, and a missing main menu
that made ⌘V silently do nothing.

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

**The menu bar is a control, not a readout.** The status item shows a pause button
while a timer runs and a play button for 90 minutes after one stops, so the common
action costs one click and opens nothing; clicking the name opens the popover
instead. That is why [`StatusItemController`](Apps/Mac/Sources/StatusItemController.swift)
is hand-rolled AppKit — SwiftUI's `MenuBarExtra` has exactly one behaviour, which is
to open its content on any click.

**Suggestions are ranked by what you actually do.** Harvest returns every task on
every assigned project, alphabetically, burying the two or three things you do daily.
Sheaves ranks them by 90 days of your own entries, weighted so the score halves every
three weeks: frequency alone pins a finished project to the top for weeks, recency
alone lets one stray entry outrank a habit.

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
| Click ⏸ / ▶ in the menu bar | Pause or resume, without opening anything |
| ↑ ↓ | Move through matches |
| ⇥ | Jump to the notes field |
| ⏎ | Start the selected timer |
| ⌘← ⌘→ | Previous / next day |

## Licence

MIT. See [LICENSE](LICENSE).
