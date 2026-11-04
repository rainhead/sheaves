//! Two machines sharing presence through the running entry, against one simulated
//! Harvest: the scenarios `docs/gnome-port.md` gives for phase 5, which phase 9
//! moved off the tailnet.

mod support;

use std::sync::Arc;

use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde_json::json;

use sheaves_core::absence::{Absence, AbsenceResolution, PresenceReading};
use sheaves_core::client::{Credentials, HarvestClient};
use sheaves_core::clock::{Clock, ManualClock};
use sheaves_core::credentials::{CredentialStore, Login, MemoryCredentialStore};
use sheaves_core::peers::board::{Board, GROUP};
use sheaves_core::peers::{Peers, Presence};
use sheaves_core::queue::MutationQueue;
use sheaves_core::snapshot::SnapshotStore;
use sheaves_core::tracker::TimeTracker;

use support::{FakeHarvest, USER_ID, target};

const THRESHOLD: f64 = 15.0 * 60.0;
const POLL: f64 = 30.0;

/// One of the user's machines: a tracker, and presence that shares through it.
struct Machine {
    tracker: TimeTracker,
    presence: Presence,
    /// When the user last touched this machine's keyboard or mouse.
    last_input: Timestamp,
    /// Every absence this machine reported, the other machine's part consulted.
    absences: Vec<Absence>,
    _directory: tempfile::TempDir,
}

struct Desk {
    clock: Arc<ManualClock>,
    harvest: Arc<FakeHarvest>,
    mac: Machine,
    hostess: Machine,
    /// The Mac's presence as it was before sharing: given the Mac's readings,
    /// reading nothing, for comparison.
    mac_alone: Presence,
    mac_alone_absences: Vec<Absence>,
}

/// The machine the user is at, if any.
#[derive(Clone, Copy, PartialEq)]
enum At {
    Mac,
    Hostess,
    Away,
}

fn clock_at(text: &str) -> Timestamp {
    format!("2026-08-29T{text}:00Z").parse().unwrap()
}

impl Desk {
    /// Both machines launched at eight, on the same account.
    async fn new() -> Self {
        let clock = Arc::new(ManualClock::new(clock_at("08:00"), TimeZone::UTC));
        let harvest = FakeHarvest::new(clock.clone());
        let mac = Self::launch("mac", &clock, &harvest).await;
        let hostess = Self::launch("host", &clock, &harvest).await;
        let mac_alone = Presence::new(
            THRESHOLD,
            clock.now(),
            Arc::new(Peers::new(THRESHOLD, "alone")),
        );
        Self {
            clock,
            harvest,
            mac,
            hostess,
            mac_alone,
            mac_alone_absences: Vec::new(),
        }
    }

    async fn launch(
        machine: &str,
        clock: &Arc<ManualClock>,
        harvest: &Arc<FakeHarvest>,
    ) -> Machine {
        let directory = tempfile::tempdir().unwrap();
        let credentials = Arc::new(MemoryCredentialStore::new(Some(Login {
            credentials: Credentials::new("12345", "token"),
            user_id: Some(USER_ID),
        })));
        let tracker = TimeTracker::new(
            HarvestClient::new(None, harvest.clone(), 0.0),
            credentials as Arc<dyn CredentialStore>,
            SnapshotStore::new(directory.path().join("cache.json")),
            MutationQueue::open(directory.path().join("queue.json"), clock.clone()),
            clock.clone(),
        );
        let peers = Arc::new(Peers::new(THRESHOLD, machine));
        tracker.join(peers.clone());
        tracker.bootstrap().await;
        Machine {
            tracker,
            presence: Presence::new(THRESHOLD, clock.now(), peers),
            last_input: clock.now(),
            absences: Vec::new(),
            _directory: directory,
        }
    }

    fn machine(&mut self, at: At) -> Option<&mut Machine> {
        match at {
            At::Mac => Some(&mut self.mac),
            At::Hostess => Some(&mut self.hostess),
            At::Away => None,
        }
    }

    /// Spends the time until `until` at one machine, or away from both, with both
    /// machines polling every half minute as the apps do.
    async fn spend(&mut self, at: At, until: &str) {
        let until = clock_at(until);
        while self.clock.now() < until {
            self.clock.advance(POLL);
            let now = self.clock.now();
            if let Some(machine) = self.machine(at) {
                machine.last_input = now;
            }
            for machine in [&mut self.mac, &mut self.hostess] {
                let absences = machine.presence.poll(reading(machine, now), now).await;
                machine.absences.extend(absences);
                // Each machine writes its part in the background, as the apps do;
                // this lets that run before the next poll.
                settle().await;
            }
            let alone = self.mac_alone.poll(reading(&self.mac, now), now).await;
            self.mac_alone_absences.extend(alone);
        }
    }
}

/// What `machine`'s sensors read at `now`.
fn reading(machine: &Machine, now: Timestamp) -> PresenceReading {
    PresenceReading {
        seconds_since_input: support::hours_between(machine.last_input, now) * 3600.0,
        ..PresenceReading::default()
    }
}

/// The absences that `machine` would ask about: those it could trim from the timer
/// running now.
fn prompts(machine: &Machine) -> Vec<Absence> {
    let state = machine.tracker.state();
    machine
        .absences
        .iter()
        .filter(|absence| state.absence_applies(absence))
        .copied()
        .collect()
}

/// Lets work the machines spawned, such as writing their part, run to the end.
async fn settle() {
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
}

/// The stretches written on the running entry, by machine.
fn shared(desk: &Desk) -> Option<Board> {
    let reference = desk.harvest.only_entry().external_reference?;
    assert_eq!(reference["group_id"], GROUP);
    Board::decode(reference["id"].as_str()?)
}

/// Start a timer on the Mac at nine, work at Hostess from ten to twelve, come back
/// to the Mac. Alone, the Mac would offer to trim two hours of real work.
#[tokio::test]
async fn work_at_the_other_machine_prompts_on_neither() {
    let mut desk = Desk::new().await;
    desk.spend(At::Mac, "09:00").await;
    desk.mac.tracker.start(&target(), None).await;
    desk.hostess.tracker.sync().await;

    desk.spend(At::Mac, "10:00").await;
    desk.spend(At::Hostess, "12:00").await;
    desk.spend(At::Away, "12:05").await;
    desk.spend(At::Mac, "12:10").await;
    desk.mac.tracker.sync().await;
    desk.hostess.tracker.sync().await;

    assert_eq!(prompts(&desk.mac), []);
    assert_eq!(prompts(&desk.hostess), []);
    // What Hostess wrote is what kept the Mac quiet: ten to twelve.
    let board = shared(&desk).expect("both machines wrote their parts");
    assert_eq!(board.parts["host"].last().unwrap().end, clock_at("12:00"));
    // Alone, the Mac would have asked about ten until five past twelve.
    let state = desk.mac.tracker.state();
    let alone_prompts: Vec<_> = desk
        .mac_alone_absences
        .iter()
        .filter(|absence| state.absence_applies(absence))
        .collect();
    assert_eq!(alone_prompts.len(), 1, "{:?}", desk.mac_alone_absences);
    assert_eq!(alone_prompts[0].began, clock_at("10:00"));
}

/// Both machines sit idle through lunch, so both ask. Answering on one withdraws
/// the question on the other.
#[tokio::test]
async fn a_shared_lunch_prompts_on_both_and_one_answer_clears_the_other() {
    let mut desk = Desk::new().await;
    desk.spend(At::Mac, "09:00").await;
    desk.mac.tracker.start(&target(), None).await;
    desk.hostess.tracker.sync().await;
    desk.spend(At::Mac, "11:00").await;
    desk.spend(At::Hostess, "12:00").await;
    desk.spend(At::Away, "13:00").await;
    desk.spend(At::Mac, "13:01").await;
    desk.spend(At::Hostess, "13:02").await;

    let on_mac = prompts(&desk.mac);
    let on_hostess = prompts(&desk.hostess);
    assert_eq!(on_mac.len(), 1, "{:?}", desk.mac.absences);
    assert_eq!(on_hostess.len(), 1, "{:?}", desk.hostess.absences);
    // Each saw the hour of lunch, not the stretch the other one saw the user in.
    assert_eq!(on_mac[0].began, clock_at("12:00"));
    assert_eq!(on_hostess[0].began, clock_at("12:00"));

    let running = desk
        .hostess
        .tracker
        .state()
        .running_entry()
        .cloned()
        .unwrap();
    desk.hostess
        .tracker
        .resolve(
            &on_hostess[0],
            &running,
            &AbsenceResolution::TrimAndContinue,
        )
        .await;

    // No word passes between the machines: the Mac learns of the trim at its
    // next look at Harvest, which is within a minute while a timer runs.
    assert_eq!(prompts(&desk.mac).len(), 1);
    desk.mac.tracker.sync().await;
    assert_eq!(prompts(&desk.mac), []);
    // Three hours from nine to twelve, and running again from when the user came
    // back, a minute and a half before the restart reached Harvest.
    let entry = desk.harvest.only_entry();
    assert!((entry.banked - 3.025).abs() < 0.01, "{entry:?}");
    assert!(entry.running_since.is_some());
}

/// The timer started at nine on the Mac. At two, Hostess trims the absence from ten
/// to twelve and carries on; at half past two the user stops the timer on the Mac,
/// which has been asleep and has not heard. Today the Mac would send five and a
/// half hours; it sends one hour, plus two and a half since twelve.
#[tokio::test]
async fn a_stop_after_a_trim_elsewhere_sends_what_harvest_implies() {
    let mut desk = Desk::new().await;
    desk.spend(At::Mac, "09:00").await;
    desk.mac.tracker.start(&target(), None).await;
    desk.hostess.tracker.sync().await;

    desk.clock.set(clock_at("14:00"));
    let running = desk
        .hostess
        .tracker
        .state()
        .running_entry()
        .cloned()
        .unwrap();
    let away = Absence {
        began: clock_at("10:00"),
        ended: clock_at("12:00"),
    };
    desk.hostess
        .tracker
        .resolve(&away, &running, &AbsenceResolution::TrimAndContinue)
        .await;

    desk.clock.set(clock_at("14:30"));
    // The Mac still believes the timer has run since nine.
    let believed = desk.mac.tracker.state().running_entry().cloned().unwrap();
    assert_eq!(believed.timer_started_at, Some(clock_at("09:00")));
    desk.mac.tracker.stop_running().await;

    let entry = desk.harvest.only_entry();
    assert!(entry.running_since.is_none());
    assert!((entry.banked - 3.5).abs() < 0.02, "{entry:?}");
}

/// A stop removes what was shared: the stretches were about the timer while it
/// ran, and a finished entry carries nothing.
#[tokio::test]
async fn a_stop_removes_what_was_shared() {
    let mut desk = Desk::new().await;
    desk.spend(At::Mac, "09:00").await;
    desk.mac.tracker.start(&target(), None).await;
    desk.hostess.tracker.sync().await;
    desk.spend(At::Hostess, "10:00").await;
    let board = shared(&desk).expect("Hostess wrote its part");
    // Only since the timer started at nine: the launch at eight is about no
    // question this timer could raise.
    let hostess = &board.parts["host"];
    assert!(hostess[0].start >= clock_at("09:00"), "{hostess:?}");

    desk.mac.tracker.stop_running().await;
    assert_eq!(desk.harvest.only_entry().external_reference, None);
}

/// An entry linked elsewhere, to an issue tracker say, is left as it is, and the
/// machines decide alone, as they did before sharing.
#[tokio::test]
async fn someone_elses_reference_is_left_alone() {
    let mut desk = Desk::new().await;
    desk.spend(At::Mac, "09:00").await;
    desk.mac.tracker.start(&target(), None).await;
    desk.hostess.tracker.sync().await;
    let theirs = json!({
        "id": "PROJ-12", "group_id": "PROJ",
        "permalink": "https://tracker.example/PROJ-12",
    });
    desk.harvest.set_reference(theirs.clone());

    desk.spend(At::Mac, "10:00").await;
    desk.spend(At::Hostess, "12:00").await;
    desk.spend(At::Mac, "12:10").await;

    assert_eq!(desk.harvest.only_entry().external_reference, Some(theirs));
    assert_eq!(prompts(&desk.mac).len(), 1, "{:?}", desk.mac.absences);

    // And the stop leaves it too.
    desk.mac.tracker.stop_running().await;
    assert!(desk.harvest.only_entry().external_reference.is_some());
}

/// A minute between writes, and none while nobody is at the machine.
#[tokio::test]
async fn writes_about_once_a_minute_while_it_sees_the_user() {
    let mut desk = Desk::new().await;
    desk.spend(At::Mac, "09:00").await;
    desk.mac.tracker.start(&target(), None).await;
    desk.hostess.tracker.sync().await;
    let before = desk.harvest.reference_writes();
    desk.spend(At::Mac, "09:10").await;
    let busy = desk.harvest.reference_writes() - before;
    assert!((9..=11).contains(&busy), "{busy} writes in ten minutes");

    let before = desk.harvest.reference_writes();
    desk.spend(At::Away, "09:20").await;
    let idle = desk.harvest.reference_writes() - before;
    assert!(idle <= 1, "{idle} writes with nobody there");
}

/// With no timer running, there is nothing to share, and no reason to ask
/// Harvest once a minute whether there is.
#[tokio::test]
async fn asks_nothing_while_no_timer_runs() {
    let mut desk = Desk::new().await;
    let before = desk.harvest.running_reads();
    desk.spend(At::Mac, "08:30").await;
    assert_eq!(desk.harvest.running_reads(), before);
}
