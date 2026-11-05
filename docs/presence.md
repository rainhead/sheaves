# How Sheaves decides you are here

Idle detection needs an answer to one question: is somebody at this Mac? Getting it
wrong in either direction costs something. Say nobody is here when they are, and the
app interrupts a meeting to ask. Say somebody is here when they are not, and a timer
runs through lunch and bills hours nobody worked.

These are the notes behind
[`PresenceMonitor`](../Apps/Mac/Sources/PresenceMonitor.swift), which reads the
Mac's signals, [`presence.rs`](../crates/sheaves-gnome/src/presence.rs), which reads
GNOME's, and [`absence.rs`](../crates/sheaves-core/src/absence.rs) in the core,
which decides what they mean —
what is measured, what was checked against a real machine, and what is still
guesswork.

## The signals

Presence is keyboard or mouse input **or** the microphone being in use. A locked
screen outranks both: a meeting left open on a locked Mac is a machine alone in a
room, not somebody working.

Sleep, screen lock and a shut lid need no special case. Each one simply stops the
evidence arriving, and the gap is measured when it resumes. Waking and unlocking are
observed only so the question can be asked on waking, rather than up to a poll later.

## Why the microphone

Idle detection that watches only the keyboard and mouse treats an hour of Zoom as an
hour at lunch. Harvest's own client does exactly this — its `IdleTimer` reads HID
idle time and nothing else — which is why it interrupts calls, and why this app
bothers to do anything different.

`kAudioDevicePropertyDeviceIsRunningSomewhere` asks whether a *device* is running,
not what it is carrying. Reading it needs no microphone entitlement, prompts for no
permission, and can never expose any audio. Every device that can record is checked
rather than only the default one, because choosing a headset in Zoom while the system
default stays on the built-in microphone is ordinary.

## What was measured

Against live calls on one Mac, rather than assumed:

| | Microphone in use |
| --- | --- |
| Zoom, unmuted | yes |
| Zoom, muted | yes |
| Zoom, camera on and muted | yes |
| Zoom open, not in a meeting | **no** |
| Google Meet in Firefox, muted | yes |

Two of these carry the design. **Muted still counts**, so sitting silent through a
long meeting is presence — the case that decides whether any of this is worth having.
And **an open Zoom is not presence**, so leaving the app running all day does not
quietly switch idle detection off.

The camera is deliberately not consulted. It would need
`com.apple.security.device.camera`, which lists Sheaves in System Settings forever as
an app that can watch you, to learn something the microphone already knows.

## What it cannot tell you

**Direction.** `kAudioDevicePropertyDeviceIsRunningSomewhere` ignores the scope it is
asked for: on this machine an input-only microphone reports running under the *output*
scope, and an output-only speaker under the *input* scope. There is no
direction-scoped read to prefer, so filtering to devices that can record is as far as
it goes. A duplex device — an audio interface, or the virtual device a meeting app
installs — therefore reads as presence while it is only playing audio.

That failure suppresses a prompt rather than raising a false one. Ignoring duplex
devices instead would miss every call made through an interface, which is the case
this exists for.

**Anything holding a device open.** Krisp, some dictation tools and similar utilities
hold an input device continuously. While one runs, the microphone always reads as
presence and nothing surfaces that.

These are the behaviours of these apps on this Mac today. A client that starts
releasing the microphone on mute would show up as a prompt during a call — the
original complaint, arriving visibly rather than silently.

## Timing

The machine is polled every thirty seconds, far finer than the fifteen minutes it
takes to count as away.

The idle clock reports how long *since* the last event, so presence is dated to that
moment rather than to whenever the poll happened to run. The microphone is read
first, and its evidence dated to the *previous* poll: a microphone found running
could have started at any point since the last look, and dating it to now would let
the gap before it reach the threshold. Joining a call after a quiet stretch of
reading would then open the prompt during the call, which is the one thing this must
never do.

## Other machines

One machine's idle time says nothing about another's. Each instance keeps a log of
when it saw the user over the last day: every moment of evidence above, with gaps
shorter than the threshold joined. The machines share these logs through Harvest,
on the running time entry, so they need no network of their own and nobody has to
name the other machines.

**If you followed a link here from a time entry in Harvest:** the entry's external
reference, which Harvest shows as an icon, is Sheaves writing down when each of your
machines saw you while that timer ran. It reads something like
`1;tq3c0;a7bq:1o-3c;k3x9:0-1o.3c-50`: a version, a starting minute, and for each
machine (named by a random id) the stretches it saw you in, as minutes from that
start, in base 36. It is rewritten about once a minute while you are at a machine
and removed when Sheaves stops the timer. It is on your own entry, so anyone who
can see your timesheet, such as an administrator of the account, can see it too.

Before reporting an absence, a machine reads the running entry, removes every
stretch another machine saw the user in, and reports what is left that is still as
long as the threshold. Answering **Keep** says you were working through the
absence, so the machine that was answered writes it as a stretch of its own, at
once; another machine already asking about the same stretch stops asking when it
next looks at Harvest, within a minute while a timer runs. If Harvest cannot be reached, or the entry links to something
else, the machine decides as it would alone. The rules are in
[`peers.rs`](../crates/sheaves-core/src/peers.rs) and the text in
[`board.rs`](../crates/sheaves-core/src/peers/board.rs).

## On GNOME

Presence is Mutter's idle time and the screensaver's lock state, with
no microphone, read every half minute and again on unlock and on waking
([`presence.rs`](../crates/sheaves-gnome/src/presence.rs)). The rules above apply
unchanged, since the core makes them.

## Whose timer it can speak for

An absence only speaks for a timer that was already running while somebody was here.
If the timer started after the absence began, nobody has been at this Mac at any
point while it ran — which is what a timer started on the web, a phone or the API
looks like from here — so it is left alone.

Harvest applies the same guard. Rejecting is the honest answer: clamping the trim to
the timer's start would invent a stopping point out of evidence nobody has. The one
concession is a couple of minutes of tolerance, because a timer started with the
mouse records its start inside the handler the click triggered, milliseconds after
the click that is itself the last evidence of presence.
