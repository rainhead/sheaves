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
  they saw the user during that gap. Peers are reached only over the tailnet, by
  configured host names.
- **A timer change on one machine is announced to the others**, which sync at
  once rather than waiting for the probe. A stop also re-reads the entry before
  sending hours, so a stop on one machine cannot overwrite a trim made on the
  other.
- **First GNOME targets after the basics: start at login and day editing.** The
  microphone signal is not ported to GNOME, because Peter takes no calls on
  Hostess; the sensor stays optional so it can be added.
- **No packaging.** Build from the checkout and run. Flatpak would sandbox the idle
  monitor and PipeWire for no gain while one person uses it.
- **Order: the Rust core and the Mac switch first, on the Mac; then the core's
  design pass and its property tests; then peers, and GNOME on Hostess.** Switching the Mac app to the Rust core first proves the port against
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

**Peers talk only over the tailnet.** The tailnet is what authenticates and
encrypts the traffic: every device on it is Peter's, and WireGuard carries each
request. So the listener binds only to this machine's Tailscale addresses, which
lie in `100.64.0.0/10` and `fd7a:115c:a1e0::/48`, and refuses a request from any
other source. A machine asks a peer only when the peer's name resolves into those
ranges, and otherwise asks nobody and logs why. Peers are the MagicDNS names in
settings, such as `hostess` and `peters-macbook-air`. When Tailscale is not
running, the machine asks alone, as it does today.

Every message also carries the Harvest user id, and a peer reporting a different
user is ignored. That check stops two people's machines on one tailnet from
mixing their evidence, but it is not authentication: the id is not secret.

Decided 2026-11-02, after review of phase 0: on a shared local network, plain HTTP
would let anyone there read the presence intervals and the user id, and forged
answers repeated at each check could suppress every absence prompt, not just one.
The local network can be added later only with authentication and encryption of
the app's own.

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

Each phase is one pull request, reviewed with the CodeRabbit CLI before pushing
(phase 4 opens one for each bug it finds), and each updates `CLAUDE.md` and the README where it changes a command or a
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

**Decided in phase 0.** The repo pins UniFFI 0.32.2, exactly, in the workspace
`Cargo.toml`, and generates the Swift with UniFFI's own Swift generator, built from
that same pinned version as `crates/uniffi-bindgen` and run through cargo. A
separate generator such as `cargo-swift` was not chosen because it carries its own
UniFFI version, and the generated Swift checks the library's interface checksums
when first called and traps on a mismatch. The core uses UniFFI's proc macros, with
no UDL file; the generator reads the interface from the compiled library.
[`Scripts/build-xcframework.sh`](../Scripts/build-xcframework.sh) builds a static
library for both Mac architectures, joins them with `lipo` because release builds
of the app are universal, and wraps the result and its C header in an XCFramework
at `Packages/SheavesCore/Frameworks`, which SwiftPM requires to sit inside the
package. The generated Swift goes into the `SheavesCore` target itself and compiles
in Swift 6 language mode without changes. Both are git-ignored and rebuilt by the
script; Xcode does not run it, so a change under `crates/` needs the script, or
`make build`, before the app sees it. The compiler is pinned in
`rust-toolchain.toml`.

### Phase 1: port the core with its tests (Mac, several days; Opus)

Port `Packages/SheavesCore` module by module, in dependency order: `CalendarDate`,
the Harvest models, `HoursEdit`, `HarvestClient` with the stub transport,
`MutationQueue`, `TimeTracker`, `AbsenceDetector` with the presence fold,
`ProjectBudget`, target ranking, `SnapshotStore`. Each Swift test gets a Rust test
of the same name; the PR carries a checklist of the roughly 160 Swift tests with
each ticked. Keep the Swift package building until phase 2, so both suites run.
Done when `cargo test` has a counterpart for every Swift test and the testing
traps in `CLAUDE.md` hold: assert the request body, never the request count.

**Decided in phase 1.** The port keeps the Swift design and changes only what Rust
or testing asked for:

- **Time comes from a `Clock`.** The queue and the tracker read the time and the
  time zone from a `Clock` trait, with `SystemClock` for the apps and `ManualClock`
  for tests that need time to stand still. Instants are `jiff::Timestamp`; days are
  `CalendarDate`, worked out in the clock's zone.
- **Parsing a duration is in the core; formatting one is not.** `parse_hours` takes
  the locale's decimal separator and reads the common decimal scripts, Arabic-Indic
  among them, so a shell's own formatted text reads back. Budgets hand the shell a
  `BudgetFigure`, hours or money with its currency, rather than a string. The Swift
  tests that check formatted strings stay in the Swift suite until phase 2 moves
  them to the Mac app.
- **The shells keep the credentials.** The core asks a `CredentialStore`; the
  Keychain tests stay with the Keychain code in Swift.
- **Concurrency follows the Swift actors.** The tracker's state sits behind one lock
  that is never held across a request. Syncs run one at a time, in the order they
  were asked for. The queue can be changed while a drain waits on the network, as
  the Swift actor allowed, and a second drain started meanwhile returns at once.
- **A shell observes the state as a whole.** `TimeTracker::observe` delivers a
  `TrackerState` value after every change; the helpers the views used, such as
  `activity()` and `ordered_projects()`, are methods on that value.
- **The presence fold is in the core.** `PresenceMonitor::poll` takes a
  `PresenceReading` (seconds since input, microphone, screen lock) and returns the
  absences it ended. The Mac's eight presence tests are ported with it.
- **Files are the core's own.** The snapshot and the queue are snake_case JSON
  written through a temporary file and a rename, in the folder
  `persistence::data_directory()` names.

One finding for phases 3 and 4: the queue rewrites a later mutation's local id when its
create lands, and also remembers which Harvest id each local id earned. Either alone
delivers the stop, so no test fails when the rewrite is removed, in Swift or in
Rust.

### Phase 2: the Mac app on the Rust core (Mac, a few days; Opus)

Generate the bindings, write the `@Observable` wrapper, implement the credential,
presence and power traits in Swift, delete the Swift core, and keep the app test
target green. The release script builds the framework before packaging. Done when
the app suite passes, a full working day shows no regression, and
`Scripts/package-release.sh` produces an app that runs on a clean account.

**Decided in phase 2.** The core's own types cross to Swift as they are, with
UniFFI derives; `crates/sheaves-core/src/ffi.rs` adds only what a foreign caller
needs:

- **A `Tracker` object on a runtime of its own.** Each call is spawned onto that
  runtime and awaited, so a call Swift abandons still finishes in Rust.
- **A `TrackerSnapshot`, with a revision number.** It carries the derived values
  views read, such as the activity and the ordered projects, so no shell computes
  them. The observer's reports reach the main thread asynchronously and can land
  after an action has already read the state back; Swift keeps whichever revision
  is newest.
- **Methods on records, under their own names** (`hours_at`, `trim_for`,
  `local_day`), and a Swift file, `CoreExtensions.swift`, that gives them back the
  names the views used. It renames; it decides nothing.
- **Foreign traits for what the shell knows:** `CredentialStore` (the Keychain),
  `PowerProbe`, `TrackerObserver` and `LogSink`, which sends the core's log lines to
  the unified log under the app's subsystem.
- **The presence fold and the demo account moved to Rust.** The Mac app hands
  `PresenceFold` a reading every poll; the documentation images and the demo
  instance sync against `demo.rs`.
- **Errors are translated at the edge.** `ConnectFailure` keeps the count of
  stranded changes that Settings shows; everything else crosses as a message.
- **TLS is the system's on a Mac** (native-tls, which is Security.framework) and
  rustls elsewhere, and the release profile is tuned for size. The zipped app went
  from 1.4 MB to 7.1 MB; rustls with its crypto library would have made it 10 MB.
- **The Rust files have new names**, `cache.json` and `queue.json`, beside the Swift
  core's `snapshot.json` and `pending-mutations.json`, which the Rust never reads or
  overwrites. The Keychain item keeps its format, so the token saved by the Swift
  core still reads.

### Phase 3: one record of the connection, and ids for queued changes (core; Opus)

Phase 1's eight review rounds kept finding bugs, most of them in the Swift core
too, and nearly all came from two design weaknesses. Each fix there closed one
case; this phase removes the causes. Nothing here changes what a user sees.

- **Who is connected is kept in three places,** written at different moments: the
  stored credentials, the queue's owner, and the cached user in the snapshot. Four
  findings were crash windows between those writes, such as a launch after dying
  between saving a colleague's token and claiming the queue. Keep one durable
  record of the account and user, written once a connect is verified, and check
  the queue and the cache against it. See [#14](https://github.com/rainhead/sheaves/issues/14).
- **The queue recognises the change it sent by comparing whole values,** so
  anything that amends a queued change breaks "is the head still the one I sent".
  The in-flight amend, the reopened create, and the queue's two ways of mapping a
  local id to Harvest's (rewriting later changes, and remembering the mapping) all
  come from this. Give each queued change an id when it is enqueued, and compare
  ids. Two gaps then become fixable: a create retried after a 5xx or a timeout,
  which Harvest may already have saved, and a stop or restart whose request landed
  but whose reply did not decode. Harvest has no idempotency key; its
  `external_reference` field on a time entry might carry the change's id, so a
  retry could find its entry exactly. Check whether a personal token may set it.
  See [#15](https://github.com/rainhead/sheaves/issues/15).

Done when each finding listed in the two issues has a test that fails without the
fix, and the property tests of phase 4 have the ids to work with.

**Decided in phase 3.**

- **The stored login is the record of who is connected.** The credential store
  holds a `Login`: the credentials and the Harvest user they were verified to
  belong to, saved in one write when a connect succeeds. The queue's owner and the
  cached user are checked against it, never the reverse, so the owner check needs
  no request and a cache left by someone else is discarded at launch. A login saved
  before user ids were recorded asks Harvest once and is completed.
- **Each queued change has an id,** minted when it is enqueued, and the drain
  recognises the change it sent by that id. Rewriting later changes' local ids is
  gone; the remembered mapping from local ids to Harvest's is the one way a change
  learns its entry.
- **A create is marked as attempted before it is sent.** A retry after an
  ambiguous failure looks for the entry first, matching project, task, day and
  notes on an entry created since the attempt, with five minutes' allowance for the
  two clocks, and not already taken by another create. A failed lookup stops the
  drain without discarding anything.
- **Harvest's `external_reference` was not used.** It would make the match exact,
  but if a personal token may not set it, every create would be refused, and that
  can only be checked against a real account. The match above is the fallback until
  someone checks.
- **A write whose reply will not decode counts as landed.** A create whose reply
  will not decode waits for the lookup.

### Phase 4: a property-testing pass over the core (either machine; Opus)

Search the Rust core for correctness bugs with `proptest`, following the
[proptest-praxis skill](https://github.com/inanna-malick/agent-skills/blob/main/skills/proptest-praxis/SKILL.md).
It runs before peers, because peers add ordering of their own on top of the
queue and the tracker, and phase 1's reviews showed those two are where the subtle
bugs are. The ported tests check the cases someone thought of. This pass generates long,
random sequences of operations and compares the core with a simple model of what
it should do, which finds the cases nobody thought of. The core is a good subject:
its implementation is involved, but most of what it promises can be stated simply.

Candidate targets, each with the simple statement that serves as the model:

- **The mutation queue.** Any sequence of actions, delivered with failures,
  retries, restarts from the queue file and a token for another account at any
  point, leaves Harvest as if each action had arrived at the moment the user took
  it. The model is a list of timestamped actions applied in order.
- **The time tracker.** After any mix of local actions and edits made elsewhere,
  a sync shows the same entries as rebuilding from Harvest alone, except for
  changes still in the queue. An entry started offline keeps one identity when its
  Harvest id arrives.
- **The absence detector.** The absence reported is the gap a plain sorted list of
  every presence interval shows, whatever order the evidence arrives in. Phase 5
  adds the peer merge to this target, and a stop after a trim on the other machine
  sending the hours the server's own entry implies.
- **Formats that must survive a round trip.** `CalendarDate` through its wire
  format, the snapshot and the queue through their JSON files, and `HoursEdit`
  between typed text and hours.

The skill's deliverables differ from the other phases'. Its property tests live on
a discovery branch that is not merged, and each bug it confirms becomes its own PR
from `main`, holding a deterministic regression test and the smallest fix. Done
when every target above has a property test that a deliberately introduced defect
makes fail, and each failure the tests found is fixed or written up as an open
question.

**Found in phase 4.** The property tests are on the branch `gnome-4-property-tests`,
in `crates/sheaves-core/tests`, and stay unmerged. There are three:

- **`timesheet_model.rs`** runs the tracker and its queue against a simulated
  Harvest through random user actions, interleaved with a network that drops
  requests, fails, or saves a request and loses the reply, with relaunches of
  the app from its files, and with edits made elsewhere (hours, notes, deletes,
  timers started on the web). Once the network recovers, the queue must drain,
  Harvest must hold what the user did, and the app must show what a fresh launch
  reading Harvest alone shows. It covers the queue and tracker targets except a
  token for another account, which it does not generate; the tracker's unit tests
  for connecting cover that.
- **`presence_model.rs`** lays out a day at the desk (typing, reading, calls, a
  locked screen, sleep), polls it as the Mac app does, and checks the absences
  reported against the gaps in the evidence a poll could see. The order in which
  evidence arrives only varies once peers exist, so phase 5 extends this test.
- **`round_trips.rs`** covers `CalendarDate` text, durations as the shells display
  them, and the queue and cache files.

Each was checked against planted defects, among them a queue file that forgets a
create was attempted, a queue that retries a change to an entry deleted
elsewhere, a halved absence threshold, an ignored microphone, and a field the
cache file drops. Each planted defect made its test fail.

Two bugs were found and fixed: a deleted entry could be resumed
([#18](https://github.com/rainhead/sheaves/pull/18)), and four hundred digits
typed as a duration read as infinite hours, which made the queue file unreadable
([#22](https://github.com/rainhead/sheaves/pull/22)). Three questions about when
an absence begins are open in [#23](https://github.com/rainhead/sheaves/issues/23).
One finding was written up and not changed: serde_json reads a float back up to one
unit in the last place away from what it wrote, because its `float_roundtrip`
feature is off. That is harmless here, because every hours figure is rounded to
two decimals before it is sent.

### Phase 5: peers (core and Mac, a few days; Opus)

Implement the presence log, the two requests, the merge before reporting, the
`touched` announcement after each delivered mutation, the prompt re-evaluation and
the stop re-read, all as specified above. Core tests drive two trackers against a
stub peer transport through three scenarios: the nine, ten, twelve case prompts on
neither; a shared lunch prompts on both and the second prompt withdraws when the
first is answered; a stop after a remote trim sends the recomputed hours. A fourth
test sends a request from an address outside the tailnet ranges and checks that it
is refused. The Mac gains a peers field in Settings, and the sandboxed app needs
the `com.apple.security.network.server` entitlement to listen. Done when the
scenarios pass and the Mac serves and answers the requests on the tailnet.

**Decided in phase 5.** The peers live in
[`crates/sheaves-core/src/peers.rs`](../crates/sheaves-core/src/peers.rs), so the
GNOME app gets them whole:

- **One object per machine, `Peers`,** holds the presence log, the names to ask,
  and whose machine it is. `Presence` wraps the presence monitor around it: each
  poll's evidence goes into the log, and each absence the monitor finds is checked
  with the peers before it is reported. The tracker `join`s the same `Peers`, so it
  announces what it delivered and syncs when told. On the Mac,
  `Tracker::presence` hands out a fold joined to the tracker's peers.
- **The merge subtracts what peers saw rather than feeding a detector.** Feeding
  interval boundaries into `AbsenceDetector`, as this file first said, would
  measure the inside of a peer's interval as a gap. `unwitnessed` removes every
  covered stretch from the absence and keeps what is left over the threshold, so
  half an hour seen at Hostess in the middle of a morning leaves two absences, each
  asked about in turn.
- **The log is in memory.** A relaunch counts as presence anyway, so there is
  nothing to lose. It reaches back a day.
- **Port 47147, HTTP/1.1 through hyper.** reqwest already brings hyper, so the
  server adds little. The listener looks for this machine's tailnet addresses once a
  minute, so Tailscale started after the app is found. It listens only while at
  least one peer is named. `POST /touched` carries a list of entry ids, since one
  drain can change several.
- **A stop carries its run:** when the timer started, the hours banked before, and
  when the user stopped it. Delivery reads the entry back first. Not running: the
  stop is dropped. Running since the same start with the same banked hours: the hours
  measured here go. Running since some other start: Harvest's banked hours plus the
  time from that start to the stop. Restarted more than two minutes after the stop:
  dropped, since the later action wins. Two cases needed care. A stop made while a
  change to the same entry is still queued here carries no run, because this
  machine's own restart landing late would look like another client's. And the
  hours decided on are written into the queued stop with the attempt, so a stop
  whose `/stop` landed but whose correction failed still sends them on retry.
- **A prompt withdraws itself** when, on any change of state, the running timer no
  longer ran through the absence (`TrackerState::absence_applies`). The Mac's
  prompt watches the tracker for that.

Checked on the machines, 2026-11-03: the core built and served on Hostess, and
answered from the Mac over the tailnet with presence for the right user, 403 for
another, and the change notice; nothing answered on Hostess's LAN address. From the
Mac, the core's client found Hostess by name and read its log. The Mac's own
listener bound to both tailnet addresses but answered nothing, because the macOS
application firewall silently holds incoming connections to programs it does not
know (a built-in `nc` on the same address answered). The app is ad-hoc signed, so
it will need allowing under System Settings › Network › Firewall › Options, and may
need it again after each build. Whether the sandboxed app then answers is unchecked.

Open, for Peter: answering **Keep** on one machine changes nothing in Harvest, so
the other machine's prompt for the same lunch stays open and asks again. Withdrawing
it would need the answer itself shared between peers. And if both prompts are
answered with a trim within the few seconds before the notice arrives, the second
machine's stop is dropped as stale but its restart still goes, which patches the
running timer's hours to its own trim.

### Phase 6: the GNOME app (Hostess, a few days)

`crates/sheaves-gnome`: a single instance `adw::Application`; an app indicator
with the running timer as its label and a menu to stop or resume; a window with
the day view, start and stop, the ranked target picker and day navigation;
settings with the token stored through `oo7`; an autostart toggle that writes
`~/.config/autostart/sheaves.desktop`. Presence is not wired yet. Done when a
timer started on Hostess appears on the Mac within a minute and the reverse, and
the label in the top bar shows the running timer. Verify in the first hour that
Ubuntu's extension renders the indicator label at all; if it does not, the icon
alone must carry the state, as the Mac's does.

**Decided in phase 6.** The app is
[`crates/sheaves-gnome`](../crates/sheaves-gnome), a workspace member left out of
the default members so `cargo test` on a Mac does not try to build GTK; CI builds
it on Ubuntu 26.04, the release Hostess runs.

- **It uses the Mac's tracker object.** `ffi::Tracker` is plain Rust, so the GNOME
  app calls it directly and gets the same runtime, sync and peers wiring. GTK keeps
  the main thread; the core's state reports reach it over a channel, and each action
  is awaited on GLib's executor.
- **The top bar item is written on zbus.** The label beside the icon is the Ayatana
  extension's `XAyatanaLabel` property, which no Rust tray crate offers (`ksni`
  0.3.7 has no way to add it), so `indicator.rs` implements the item and the small
  part of `com.canonical.dbusmenu` a flat menu needs. Checked at the desk on
  2026-11-03: Ubuntu's extension draws the label. A click opens the window, a
  middle click stops or resumes, and the right-click menu offers Stop or Resume,
  Open Sheaves and Quit. The item registers again when GNOME Shell restarts.
- **The token is in the keyring through oo7,** stored with its user id as JSON. The
  core asks for credentials synchronously from inside its own runtime, so each
  keyring call runs on a thread of its own.
- **One instance per session,** through GApplication. Closing the window hides it;
  `--background` starts with no window, and the login item passes it. The login
  item is a desktop entry in `~/.config/autostart` running the binary where it is.
- **Durations are formatted through the C locale:** the decimal form takes the
  locale's separator; `1:30` keeps ASCII digits, since the C library has no
  duration formatting. Days are formatted by GLib.
- **`SHEAVES_RENDER=<folder>`** in a debug build draws the window from the demo
  account to `gnome-window.png`, as the Mac's documentation images do.
- **Not in this phase:** the power source (the tracker assumes mains power, which
  Hostess is), editing notes or hours (phase 8), presence and its prompt, and the
  peers field (phase 7).

Checked at the desk, 2026-11-03: a timer started on Hostess appeared on the Mac,
and stopping it on the Mac changed Hostess's top bar to the play icon within a
minute. "Within a minute" holds while either machine has a timer running, because
the probe then runs every minute. From an idle machine it does not: the Mac picked
up Hostess's new timer at its next idle probe, two minutes later, and that probe
can be ten minutes away. The change notices of phase 5 close that gap once the
GNOME app joins its peers in phase 7 and the Mac's firewall lets Sheaves answer.

### Phase 7: presence on GNOME (Hostess, a few days; Opus)

Implement `PresenceSensor` from `org.gnome.Mutter.IdleMonitor.GetIdletime` and
`org.gnome.ScreenSaver.GetActive`, polling every thirty seconds and again on
logind's `PrepareForSleep` resume and on the screensaver deactivating. Add the
absence prompt as an `adw::AlertDialog` with the four resolutions. Join the peers.
Done when the nine, ten, twelve scenario, done for real across both machines,
prompts on neither, and a real lunch prompts on both with one answer clearing the
other.

### Phase 8: day editing (core, then GNOME, then Mac)

What the roadmap cut from the first version: change an entry's project and task,
edit a stopped entry's duration in place, and add an entry to any day without
running a timer. Each is a mutation with the same offline rules as the rest. The
week view waits. Done when the three edits work on GNOME and the core tests cover
their queue behaviour; the Mac views follow in their own PR.

### Phase 9: quick entry and the shortcut (Hostess, a day)

A quick entry window that `sheaves-gnome --quick-entry` raises in the running
instance through GApplication's command line handling, bound to a key in GNOME's
custom shortcuts. The GlobalShortcuts portal is available if a binding inside the
app turns out to be wanted. Done when the key opens the window from any app and
starts a timer.

### Phase 10: documents and cleanup

The README's layout table and "How it works", `ROADMAP.md`, this file, and
`docs/presence.md`, which gains the GNOME signals and the peer merge. Done when a
newcomer can build both apps from the README alone.

## Working practice

- Phases merge to `main` one at a time, each when it is done, and each branches
  from `main`. One agent works on the port at a time, so no phase waits on another's
  review and a PR stack would add nothing.
- GTK work happens on Hostess, over SSH or in a session there. The Rust core is
  portable and its tests run on either machine. A GUI check on Hostess needs
  someone at the desk; a session should say what it could not see.
- The Mac phases need the Swift toolchain, so they stay on the Mac.
- Before each phase, read `CLAUDE.md`: the platform traps there are the reasons
  behind several lines of the Swift code, and the Rust port must keep them.
