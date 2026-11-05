# Roadmap

Sheaves is one person's tool, kept public in case it is useful to someone else. The
ordering follows from that: things felt every day first, then the accuracy of the
hours it records, then everything else. Nothing here is a commitment.

## Next

**Nothing claimed.** The GNOME app is done ([how it was built](docs/gnome-port.md)),
and so is editing a day. Three questions about absences wait on real use rather
than on code: when exactly an absence begins
([#23](https://github.com/rainhead/sheaves/issues/23)), an absence answered after an
entry made offline got its Harvest id
([#26](https://github.com/rainhead/sheaves/issues/26)), and the two left open in
[phase 5 of the plan](docs/gnome-port.md#phase-5-peers-core-and-mac-a-few-days-opus).

## Maybe, and what each would cost

**A budget widget.** Only if budgets turn out to be worth watching — the same
absence rule applies, and an account with no readable budgets has no reason to want
this at all. The data is already done: `TimeTracker` holds the budgets and the panel
draws them. What is left is the process boundary. A widget cannot read the app's
Keychain item or its cache, so it needs a Keychain access group and an App Group
container, and `SnapshotStore` has to move out of the app's private container. Both
need team-signed entitlements, which also breaks the ad-hoc path that lets anyone
clone and build. That price is the reason to spend a few real days with the panel
version first.

**iOS.** `SheavesCore` is platform-neutral for this, and the views are the only work.
But porting the Mac interface to a phone is the least interesting version. The parts
that would actually earn their place are a Live Activity — the running timer on the
Lock Screen and in the Dynamic Island — and a home screen widget. If iOS happens, it
should be a small app that exists to host those.

**A week view.** Editing a day is done: notes, time, project and task, delete, and
time logged without a timer. Seeing a week at once was cut from v1 and still is.

**What the GNOME app leaves out.** Budgets, which the core already holds and the
window only has to draw. Battery and Low Power Mode, which slow and stop the
probe on the Mac; GNOME's app assumes mains power, and UPower over D-Bus would
tell it otherwise. The microphone, left out because nobody takes calls on the
machine it was built for; PipeWire could say whether a source is running, as Core
Audio does on the Mac. The keyboard in the window: the Mac's panel moves through
entries and projects with the arrows, and the GNOME window does not yet.

**A signed, notarised build.** So it lives in `/Applications` rather than
`DerivedData`. Needs a paid Developer Program membership. Less urgent since team
signing stopped the Keychain re-prompting on every rebuild.

## Not doing yet

**Localisation.** Numbers, dates and durations already go through locale-aware format
styles, but the strings are hardcoded English with ad-hoc pluralisation. Making them
translatable means a string catalog and real plural rules — worth it if anyone who
needs it turns up, not before.

**Text scaling.** Panel widths and the visible-row count are fixed, so large
accessibility text sizes crop rather than expand.

**A package for GNOME.** It is built from the checkout and runs from there, which
is why it installs its own desktop entry and points its login item and shortcut
at whichever binary is running. A `.deb` or a Flatpak would do that properly, and
is worth it once someone other than its one user wants it. A Flatpak would also
have to be let out of its sandbox for the idle monitor.
