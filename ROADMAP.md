# Roadmap

Sheaves is one person's tool, kept public in case it is useful to someone else. The
ordering follows from that: things felt every day first, then the accuracy of the
hours it records, then everything else. Nothing here is a commitment.

## Next

**Nothing claimed.** Budget-in-the-panel is done: the running project's remaining
budget sits under the day header, with a bar for how much of it is gone, and it is
absent on accounts with no budget Sheaves may read. It exists partly to answer a
question — is a budget worth glancing at often enough to earn a second surface? A
few real days with [`BudgetBar`](Apps/Mac/Sources/BudgetBar.swift) decide whether
the widget below is worth its entitlements, or whether editing a day is the better
use of the next stretch.

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

**Editing a day or week.** Inline duration edits, reassigning project and task, and a
week view. Notes and delete already work; the rest was cut from v1 deliberately.

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
