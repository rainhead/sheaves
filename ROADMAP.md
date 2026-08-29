# Roadmap

Sheaves is one person's tool, kept public in case it is useful to someone else. The
ordering follows from that: things felt every day first, then the accuracy of the
hours it records, then everything else. Nothing here is a commitment.

## Next

**Launch at login.** A menu bar app you have to remember to start is half a menu bar
app. `SMAppService`, plus a toggle in Settings.

**Idle detection.** The one that matters. A timer left running over lunch, or
overnight, quietly bills hours nobody worked — and this app exists to record hours
accurately. Watch idle time (`CGEventSource.secondsSinceLastEventType`), sleep and
wake (`NSWorkspace`), and screen lock; when the user comes back to a timer that ran
through an absence, offer to trim it to when they actually stopped. Mutations already
carry explicit hours, so trimming is a normal edit rather than a special case.

## After that

**Budget, in the panel.** `GET /v2/reports/project_budget` returns `budget`,
`budget_spent` and `budget_remaining` per project in a single call, with `budget_by`
saying whether those are hours or money. Showing the running project's remaining
budget beside its timer is nearly free.

Whether it appears at all is a runtime question, not a styling one. A project with
`budget_by` of `none` has no budget, and a monetary budget is visible only to
administrators and project managers with the billable-rates permission — so for some
accounts the report comes back with nothing usable in it. There is no lesser version
of this feature to fall back to: if there are no budgets to show, the whole thing
should be absent rather than present and empty. Probe once, and let the answer decide
whether the UI exists.

Two other constraints: the Reports API allows only 100 requests per 15 minutes, far
tighter than the 100 per 15 seconds everything else gets, so this wants its own
refresh pace; and budgets can reset monthly (`budget_is_monthly`).

## Maybe, and what each would cost

**A budget widget.** Only if budgets turn out to be visible and worth watching —
the same absence rule applies, and an account with no readable budgets has no reason
to want this at all. Deliberately after budget-in-the-panel, because the data is the
cheap part. A widget is a separate process: it cannot read the app's Keychain item or
its cache, so it needs a Keychain access group and an App Group container, and
`SnapshotStore` has to move out of the app's private container. Both need team-signed
entitlements, which also breaks the ad-hoc path that lets anyone clone and build. Put
the panel version in front of a real day of work first and find out whether a second,
less glanceable surface earns that.

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
