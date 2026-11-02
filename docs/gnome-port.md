# Porting Sheaves to GNOME

The plan for making Sheaves run on Hostess, Peter's Ubuntu 26.04 desktop, beside
the Mac app, with the two machines agreeing about when he was away. Decided on
2026-11-02. Each phase below is written as a brief for a separate session, so a
session can pick one up without the conversation that produced this file.

## Decisions

- **Rust for everything portable; Swift only for SwiftUI and Apple integration.**
  `Packages/SheavesCore` is rewritten as a Rust crate and the Mac app calls it
  through [UniFFI](https://mozilla.github.io/uniffi-rs/). The sync rules, the
  mutation queue and the absence rule then exist once, and the Swift tests become
  the specification for the port.
- **On GNOME, the timer lives in the top bar and a click opens a window.** An app
  indicator shows the icon and a label such as "Orcasound 1:30"; the day view,
  quick entry, absence prompt and settings are libadwaita windows. No GNOME Shell
  extension: one would reproduce the Mac popover in the bar, but it needs a
  background service and breaks with each GNOME release.
- **The two machines share presence over the network.** Each instance both serves
  and asks: before a machine prompts about an absence, it asks its peers whether
  they saw the user during that gap. Peers are reached on the local network or
  over the tailnet, by configured host names.
- **A timer change on one machine is announced to the others**, which sync at
  once rather than waiting for the probe. A stop also re-reads the entry before
  sending hours, so a stop on one machine cannot overwrite a trim made on the
  other.
- **First GNOME targets after the basics: start at login and day editing.** The
  microphone signal is not ported to GNOME, because Peter takes no calls on
  Hostess; the sensor stays optional so it can be added.
- **No packaging.** Build from the checkout and run. Flatpak would sandbox the idle
  monitor and PipeWire for no gain while one person uses it.
- **Order: the Rust core and the Mac switch first, on the Mac; then GNOME, on
  Hostess.** Switching the Mac app to the Rust core first proves the port against
  a working, tested interface before any GNOME code depends on it.

## What Hostess has

Checked over SSH on 2026-11-02: GNOME Shell 50.1 on Wayland, Ubuntu's app
indicator extension enabled, libadwaita 1.9, libsecret 0.21, Rust 1.99, PipeWire
tools. On the session bus: `org.gnome.Mutter.IdleMonitor`,
`org.gnome.ScreenSaver`, `org.freedesktop.secrets` and the desktop portal with
`GlobalShortcuts`. No Flatpak. Swift 6.1.3 is installed but nothing in this plan
needs it.

## The repo afterwards

```
Cargo.toml                   workspace
crates/sheaves-core/         models, Harvest client, queue, tracker, presence, peers
crates/sheaves-gnome/        the GNOME app: gtk4-rs, libadwaita, zbus, oo7
Packages/SheavesCore/        thin Swift package: UniFFI bindings, the XCFramework, Keychain
Apps/Mac/                    the menu bar app, as now
Scripts/build-xcframework.sh builds the core for the Mac app and the release
```

## Core design

**State crosses the boundary as snapshots.** The core owns the tracker state. A
shell registers a callback and receives a plain value of the whole observable
state each time it changes. The Mac keeps an `@Observable` wrapper that stores the
latest value, so the views change nothing. GTK does the same with a property.

**The shells implement the platform traits.** Each is a UniFFI callback interface
on the Mac and an ordinary trait object on GNOME:

| Trait | Mac | GNOME |
| --- | --- | --- |
| `CredentialStore` | Keychain, as today | the keyring through the `oo7` crate |
| `PresenceSensor` | `SystemPresenceSensor`, as today | Mutter idle time, screensaver state, no microphone |
| `PowerSource` | `PowerSource`, as today | UPower over D-Bus, or "mains" if it is not worth the code |
| `PeerTransport` | reqwest, inside the core | the same |

**The presence fold moves into the core.** `PresenceMonitor.poll` in
`Apps/Mac/Sources/PresenceMonitor.swift` dates microphone evidence to the previous
poll and input evidence to the idle clock; that reasoning belongs with
`AbsenceDetector`, so GNOME does not reimplement it. Only the sensor stays in each
shell.

**Formatting stays in the shells.** Hours, dates and durations reach the user
through Swift format styles on the Mac and GLib on GNOME. The core hands out
numbers and `CalendarDate` values, which keep their ASCII wire format.

**Crates.** `tokio` for the runtime, `reqwest` with rustls so Linux builds need no
OpenSSL, `serde` for JSON, `jiff` for civil dates and zones, `directories` for the
data folder, `uniffi` with proc macros, `log` with a sink each shell provides. The
GNOME app adds `gtk4`, `libadwaita`, `zbus`, `oo7` and the app indicator binding.

**Persistence.** The snapshot and the queue stay JSON files, in Application
Support on the Mac and `~/.local/share/sheaves` on Linux. The Rust port does not
read the Swift files: switch the Mac with every timer stopped and the queue
drained, and let the first sync rebuild the snapshot.

## Peers: sharing presence between machines

**The case this fixes.** Start a timer on the Mac at nine, work at Hostess from
ten to twelve, come back to the Mac. Today the Mac prompts to trim two hours of
real work. Hostess stays quiet, rightly, because the timer started before its own
absence began. The Mac is wrong only because it lacks Hostess's evidence.

**Each instance keeps a presence log**: intervals during which it had evidence of
the user, for the last 24 hours, built from the same evidence the detector
receives. Consecutive evidence closer together than the threshold merges into one
interval.

**Each instance serves two requests** on a fixed port, over plain HTTP with JSON:

- `GET /presence?since=<time>` returns this machine's intervals overlapping the
  range from `since` to now.
- `POST /touched` with an entry id says this machine just changed a timer. The
  receiver probes Harvest at once.

Every message carries the Harvest user id, and a peer reporting a different user
is ignored. There is no other authentication: on the tailnet it is unnecessary, and
on a shared network the worst a forged message can do is suppress one prompt.
Peers are the host names in settings, such as `hostess` and `peters-macbook-air`,
which MagicDNS and the local network both resolve.

**Merging is done by the detector it already has.** Before a machine reports an
absence, the monitor asks each peer for presence since its own last evidence,
feeds the returned interval boundaries into `AbsenceDetector` in time order, and
only then feeds its own evidence. In the example, the Mac learns of presence from
ten to twelve, so its detector sees a two hour gap from twelve to the return,
which is reported only if it exceeds the threshold. If a peer does not answer
within two seconds, the machine proceeds as today and logs that it asked alone.

**One lunch, two prompts.** When both machines are idle through lunch, both will
prompt. Resolving on one sends `touched`; the other probes, re-evaluates its open
prompt against the entry it now has, and dismisses the prompt if
`Absence.trimmedHours(for:)` no longer applies to the entry.

**A stop re-reads the entry.** `Mutation.stop` carries the time the user acted and
what the local copy believed: banked hours and when the timer started. On delivery
the queue fetches the entry. If the server's timer started when the local copy
thought, the hours go as computed. If the server's timer started later, because
another client trimmed and continued, the hours are recomputed from the server's
banked hours and start time up to the moment the user acted. If the server's timer
is not running at all, the stop is dropped and the local copy adopts the server's
entry. Worked example: timer started at nine, trimmed on Hostess at fourteen to
restart from twelve, stopped on the sleeping Mac at fourteen thirty before it had
probed. Today the Mac sends five and a half hours. With the re-read it sends one
hour banked plus two and a half elapsed since twelve.

## Phases

Each phase is one pull request, reviewed with the CodeRabbit CLI before pushing,
and each updates `CLAUDE.md` and the README where it changes a command or a
concept. The suggested model is a guess at where judgment is needed most: the
queue, the tracker and the peer rules carry the subtle semantics; the GTK phases
are mostly plumbing.

### Phase 0: workspace and the FFI spike (Mac, a day)

Create the Cargo workspace and `crates/sheaves-core` with one exported function.
Write `Scripts/build-xcframework.sh`, make `Packages/SheavesCore` depend on the
built framework and the generated Swift, and have the Mac app display the string
the function returns. Add a Rust toolchain step to `.github/workflows/build.yml`
and run `cargo test` there too. Done when CI is green with the Rust step and the
app shows the string. Decide here, and record in this file, which UniFFI version
and bindings generator the repo pins.

### Phase 1: port the core with its tests (Mac, several days; Opus)

Port `Packages/SheavesCore` module by module, in dependency order: `CalendarDate`,
the Harvest models, `HoursEdit`, `HarvestClient` with the stub transport,
`MutationQueue`, `TimeTracker`, `AbsenceDetector` with the presence fold,
`ProjectBudget`, target ranking, `SnapshotStore`. Each Swift test gets a Rust test
of the same name; the PR carries a checklist of the roughly 160 Swift tests with
each ticked. Keep the Swift package building until phase 2, so both suites run.
Done when `cargo test` has a counterpart for every Swift test and the testing
traps in `CLAUDE.md` hold: assert the request body, never the request count.

### Phase 2: the Mac app on the Rust core (Mac, a few days; Opus)

Generate the bindings, write the `@Observable` wrapper, implement the credential,
presence and power traits in Swift, delete the Swift core, and keep the app test
target green. The release script builds the framework before packaging. Done when
the app suite passes, a full working day shows no regression, and
`Scripts/package-release.sh` produces an app that runs on a clean account.

### Phase 3: peers (core and Mac, a few days; Opus)

Implement the presence log, the two requests, the merge before reporting, the
`touched` announcement after each delivered mutation, the prompt re-evaluation and
the stop re-read, all as specified above. Core tests drive two trackers against a
stub peer transport through three scenarios: the nine, ten, twelve case prompts on
neither; a shared lunch prompts on both and the second prompt withdraws when the
first is answered; a stop after a remote trim sends the recomputed hours. The Mac
gains a peers field in Settings. Done when the scenarios pass and the Mac serves
and answers the requests on the tailnet.

### Phase 4: the GNOME app (Hostess, a few days)

`crates/sheaves-gnome`: a single instance `adw::Application`; an app indicator
with the running timer as its label and a menu to stop or resume; a window with
the day view, start and stop, the ranked target picker and day navigation;
settings with the token stored through `oo7`; an autostart toggle that writes
`~/.config/autostart/sheaves.desktop`. Presence is not wired yet. Done when a
timer started on Hostess appears on the Mac within a minute and the reverse, and
the label in the top bar shows the running timer. Verify in the first hour that
Ubuntu's extension renders the indicator label at all; if it does not, the icon
alone must carry the state, as the Mac's does.

### Phase 5: presence on GNOME (Hostess, a few days; Opus)

Implement `PresenceSensor` from `org.gnome.Mutter.IdleMonitor.GetIdletime` and
`org.gnome.ScreenSaver.GetActive`, polling every thirty seconds and again on
logind's `PrepareForSleep` resume and on the screensaver deactivating. Add the
absence prompt as an `adw::AlertDialog` with the four resolutions. Join the peers.
Done when the nine, ten, twelve scenario, done for real across both machines,
prompts on neither, and a real lunch prompts on both with one answer clearing the
other.

### Phase 6: day editing (core, then GNOME, then Mac)

What the roadmap cut from the first version: change an entry's project and task,
edit a stopped entry's duration in place, and add an entry to any day without
running a timer. Each is a mutation with the same offline rules as the rest. The
week view waits. Done when the three edits work on GNOME and the core tests cover
their queue behaviour; the Mac views follow in their own PR.

### Phase 7: quick entry and the shortcut (Hostess, a day)

A quick entry window that `sheaves-gnome --quick-entry` raises in the running
instance through GApplication's command line handling, bound to a key in GNOME's
custom shortcuts. The GlobalShortcuts portal is available if a binding inside the
app turns out to be wanted. Done when the key opens the window from any app and
starts a timer.

### Phase 8: documents and cleanup

The README's layout table and "How it works", `ROADMAP.md`, this file, and
`docs/presence.md`, which gains the GNOME signals and the peer merge. Done when a
newcomer can build both apps from the README alone.

## Working practice

- GTK work happens on Hostess, over SSH or in a session there. The Rust core is
  portable and its tests run on either machine. A GUI check on Hostess needs
  someone at the desk; a session should say what it could not see.
- The Mac phases need the Swift toolchain, so they stay on the Mac.
- Before each phase, read `CLAUDE.md`: the platform traps there are the reasons
  behind several lines of the Swift code, and the Rust port must keep them.
