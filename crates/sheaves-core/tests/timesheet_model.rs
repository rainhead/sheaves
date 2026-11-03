//! Model-based tests of the tracker and its queue against a simulated Harvest.
//!
//! The property: however the user's actions interleave with a network that drops,
//! fails, or answers late, once the network recovers and the queue drains, Harvest's
//! timesheet says what the user did. Each entry holds the time it was running in
//! the user's own clock, not the time requests happened to arrive, and no entry is
//! duplicated or lost.
//!
//! The oracle is a plain list of entries with banked hours and the moment each
//! started running, which knows nothing of queues, local ids or retries.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use jiff::Timestamp;
use jiff::tz::TimeZone;
use proptest::prelude::*;
use serde_json::{Value, json};

use sheaves_core::calendar_date::CalendarDate;
use sheaves_core::client::{
    Credentials, HarvestClient, HarvestError, Method, Request, Response, Transport,
};
use sheaves_core::clock::{Clock, ManualClock};
use sheaves_core::credentials::{CredentialStore, Login, MemoryCredentialStore};
use sheaves_core::queue::MutationQueue;
use sheaves_core::snapshot::SnapshotStore;
use sheaves_core::tracked_entry::TimerTarget;
use sheaves_core::tracker::TimeTracker;

const USER_ID: i64 = 1782959;
const TARGETS: [(i64, &str, i64, &str); 3] = [
    (101, "Online Store", 201, "Programming"),
    (101, "Online Store", 202, "Design"),
    (103, "Marketing Website", 233, "Graphic Design"),
];

// MARK: A simulated Harvest

#[derive(Clone, Debug)]
struct ServerEntry {
    id: i64,
    project_id: i64,
    task_id: i64,
    spent_date: String,
    notes: Option<String>,
    /// Hours banked, excluding the running stretch.
    banked: f64,
    /// When the server started counting, by its own clock.
    running_since: Option<Timestamp>,
    created_at: Timestamp,
    updated_at: Timestamp,
}

impl ServerEntry {
    fn hours(&self, now: Timestamp) -> f64 {
        self.banked + self.running_since.map_or(0.0, |since| hours_between(since, now))
    }

    fn json(&self, now: Timestamp) -> Value {
        let (project, task) = names(self.project_id, self.task_id);
        json!({
            "id": self.id, "spent_date": self.spent_date,
            "user": { "id": USER_ID, "name": "Kim Allen" },
            "client": { "id": 1, "name": "123 Industries" },
            "project": { "id": self.project_id, "name": project },
            "task": { "id": self.task_id, "name": task },
            "hours": round2(self.hours(now)),
            "hours_without_timer": round2(self.banked),
            "notes": self.notes,
            "is_locked": false, "locked_reason": null, "is_billed": false,
            "is_running": self.running_since.is_some(), "billable": true,
            "timer_started_at": self.running_since.map(|since| since.to_string()),
            "started_time": null, "ended_time": null,
            "created_at": self.created_at.to_string(),
            "updated_at": self.updated_at.to_string(),
        })
    }
}

fn names(project: i64, task: i64) -> (&'static str, &'static str) {
    let (_, project_name, _, task_name) = TARGETS
        .iter()
        .find(|(p, _, t, _)| *p == project && *t == task)
        .copied()
        .unwrap_or((0, "?", 0, "?"));
    (project_name, task_name)
}

/// What happens to one request.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Fate {
    Delivered,
    /// The connection fails before Harvest sees the request.
    LostOnTheWay,
    /// Harvest applies the request, and the reply never arrives.
    LostOnTheWayBack,
    /// Harvest fails without applying the request.
    ServerError,
    /// Harvest applies the request and answers with something unreadable.
    Garbled,
}

struct FakeHarvest {
    clock: Arc<ManualClock>,
    entries: Mutex<Vec<ServerEntry>>,
    next_id: Mutex<i64>,
    /// Fates for the next requests, in order; requests beyond them are delivered.
    fates: Mutex<Vec<Fate>>,
    offline: Mutex<bool>,
}

impl FakeHarvest {
    fn new(clock: Arc<ManualClock>) -> Arc<Self> {
        Arc::new(Self {
            clock,
            entries: Mutex::new(Vec::new()),
            next_id: Mutex::new(5000),
            fates: Mutex::new(Vec::new()),
            offline: Mutex::new(false),
        })
    }

    fn next_fate(&self) -> Fate {
        if *self.offline.lock().unwrap() {
            return Fate::LostOnTheWay;
        }
        let mut fates = self.fates.lock().unwrap();
        if fates.is_empty() {
            Fate::Delivered
        } else {
            fates.remove(0)
        }
    }

    fn page(items: Vec<Value>, key: &str) -> Value {
        let count = items.len();
        json!({ key: items, "per_page": 2000, "total_pages": 1, "total_entries": count,
                "next_page": null, "previous_page": null, "page": 1 })
    }

    /// Stops whatever is running, as Harvest does when another timer starts.
    fn stop_running(entries: &mut [ServerEntry], now: Timestamp) {
        for entry in entries.iter_mut() {
            if let Some(since) = entry.running_since.take() {
                entry.banked += hours_between(since, now);
                entry.updated_at = now;
            }
        }
    }

    /// Applies a request to the timesheet, returning the status and body Harvest
    /// would send.
    fn apply(&self, request: &Request) -> (u16, Value) {
        let now = self.clock.now();
        let path = request.url.path().trim_start_matches("/v2/").to_string();
        let body = request.json();
        let mut entries = self.entries.lock().unwrap();
        let segments: Vec<&str> = path.split('/').collect();
        match (request.method, segments.as_slice()) {
            (Method::Get, ["users", "me"]) => (
                200,
                json!({ "id": USER_ID, "first_name": "Kim", "last_name": "Allen", "email": "k@example.com" }),
            ),
            (Method::Get, ["company"]) => (
                200,
                json!({ "name": "Acme", "full_domain": "acme.harvestapp.com", "is_active": true,
                        "week_start_day": "Monday", "wants_timestamp_timers": false,
                        "time_format": "hours_minutes", "weekly_capacity": 126000 }),
            ),
            (Method::Get, ["users", "me", "project_assignments"]) => {
                let assignments: Vec<Value> = [(101, "Online Store"), (103, "Marketing Website")]
                    .iter()
                    .map(|(project, name)| {
                        let tasks: Vec<Value> = TARGETS
                            .iter()
                            .filter(|(p, _, _, _)| p == project)
                            .map(|(_, _, task, task_name)| json!({
                                "id": task + 90000, "billable": true, "is_active": true,
                                "task": { "id": task, "name": task_name }
                            }))
                            .collect();
                        json!({ "id": project + 7000, "is_active": true,
                                "project": { "id": project, "name": name, "code": "" },
                                "client": { "id": 1, "name": "123 Industries" },
                                "task_assignments": tasks })
                    })
                    .collect();
                (200, Self::page(assignments, "project_assignments"))
            }
            (Method::Get, ["time_entries"]) => {
                let query: HashMap<String, String> = request.url.query_pairs().into_owned().collect();
                let running_only = query.get("is_running").is_some_and(|value| value == "true");
                let from = query.get("from").cloned().unwrap_or_default();
                let to = query.get("to").cloned().unwrap_or_else(|| "9999".to_string());
                let items = entries
                    .iter()
                    .filter(|entry| {
                        if running_only {
                            entry.running_since.is_some()
                        } else {
                            entry.spent_date >= from && entry.spent_date <= to
                        }
                    })
                    .map(|entry| entry.json(now))
                    .collect();
                (200, Self::page(items, "time_entries"))
            }
            (Method::Post, ["time_entries"]) => {
                let hours = body.get("hours").and_then(Value::as_f64);
                if hours.is_none() {
                    Self::stop_running(&mut entries, now);
                }
                let mut next = self.next_id.lock().unwrap();
                *next += 1;
                let entry = ServerEntry {
                    id: *next,
                    project_id: body["project_id"].as_i64().unwrap(),
                    task_id: body["task_id"].as_i64().unwrap(),
                    spent_date: body["spent_date"].as_str().unwrap().to_string(),
                    notes: body.get("notes").and_then(Value::as_str).map(str::to_string),
                    banked: hours.unwrap_or(0.0),
                    running_since: hours.is_none().then_some(now),
                    created_at: now,
                    updated_at: now,
                };
                let reply = entry.json(now);
                entries.push(entry);
                (201, reply)
            }
            (Method::Patch, ["time_entries", id, "stop"]) => {
                let id: i64 = id.parse().unwrap();
                let Some(entry) = entries.iter_mut().find(|entry| entry.id == id) else {
                    return (404, json!({}));
                };
                let Some(since) = entry.running_since.take() else {
                    return (422, json!({ "message": "This time entry is not running." }));
                };
                entry.banked += hours_between(since, now);
                entry.updated_at = now;
                (200, entry.json(now))
            }
            (Method::Patch, ["time_entries", id, "restart"]) => {
                let id: i64 = id.parse().unwrap();
                if !entries.iter().any(|entry| entry.id == id) {
                    return (404, json!({}));
                }
                if entries.iter().any(|entry| entry.id == id && entry.running_since.is_some()) {
                    return (422, json!({ "message": "This time entry is already running." }));
                }
                Self::stop_running(&mut entries, now);
                let entry = entries.iter_mut().find(|entry| entry.id == id).unwrap();
                entry.running_since = Some(now);
                entry.updated_at = now;
                (200, entry.json(now))
            }
            (Method::Patch, ["time_entries", id]) => {
                let id: i64 = id.parse().unwrap();
                let Some(entry) = entries.iter_mut().find(|entry| entry.id == id) else {
                    return (404, json!({}));
                };
                if let Some(hours) = body.get("hours").and_then(Value::as_f64) {
                    entry.banked = hours;
                    // A running entry's total is set, and counting restarts now.
                    if entry.running_since.is_some() {
                        entry.running_since = Some(now);
                    }
                }
                if let Some(notes) = body.get("notes").and_then(Value::as_str) {
                    entry.notes = Some(notes.to_string());
                }
                entry.updated_at = now;
                (200, entry.json(now))
            }
            (Method::Delete, ["time_entries", id]) => {
                let id: i64 = id.parse().unwrap();
                let before = entries.len();
                entries.retain(|entry| entry.id != id);
                if entries.len() == before {
                    (404, json!({}))
                } else {
                    (200, json!({}))
                }
            }
            // Budgets and clients: none on this account.
            _ => (404, json!({})),
        }
    }
}

impl FakeHarvest {
    /// A change made through another client, applied straight to the timesheet.
    fn edit(&self, change: Remote) {
        let now = self.clock.now();
        let mut entries = self.entries.lock().unwrap();
        match change {
            Remote::SetHours { entry, quarter_hours } => {
                if let Some(entry) = entries.get_mut(entry) {
                    entry.banked = f64::from(quarter_hours) / 4.0;
                    if entry.running_since.is_some() {
                        entry.running_since = Some(now);
                    }
                    entry.updated_at = now;
                }
            }
            Remote::SetNotes { entry } => {
                if let Some(entry) = entries.get_mut(entry) {
                    entry.notes = Some(format!("edited at {now}"));
                    entry.updated_at = now;
                }
            }
            Remote::Delete(entry) => {
                if entry < entries.len() {
                    entries.remove(entry);
                }
            }
            Remote::Start(index) => {
                Self::stop_running(&mut entries, now);
                let (project_id, _, task_id, _) = TARGETS[index];
                let mut next = self.next_id.lock().unwrap();
                *next += 1;
                entries.push(ServerEntry {
                    id: *next,
                    project_id,
                    task_id,
                    spent_date: CalendarDate::of(now, &TimeZone::UTC).to_string(),
                    notes: None,
                    banked: 0.0,
                    running_since: Some(now),
                    created_at: now,
                    updated_at: now,
                });
            }
            Remote::Stop => Self::stop_running(&mut entries, now),
        }
    }
}

#[async_trait]
impl Transport for FakeHarvest {
    async fn send(&self, request: Request) -> Result<Response, HarvestError> {
        let fate = self.next_fate();
        let lost = || HarvestError::Network {
            reason: "the connection was lost".to_string(),
        };
        match fate {
            Fate::LostOnTheWay => Err(lost()),
            Fate::ServerError => Ok(response(500, json!({}))),
            Fate::LostOnTheWayBack => {
                self.apply(&request);
                Err(lost())
            }
            Fate::Garbled => {
                let (status, _) = self.apply(&request);
                Ok(response(status, json!({ "unexpected": true })))
            }
            Fate::Delivered => {
                let (status, body) = self.apply(&request);
                Ok(response(status, body))
            }
        }
    }
}

fn response(status: u16, body: Value) -> Response {
    Response {
        status,
        headers: Vec::new(),
        body: body.to_string().into_bytes(),
    }
}

// MARK: The oracle

#[derive(Clone, Debug)]
struct ModelEntry {
    target: usize,
    banked: f64,
    running_since: Option<Timestamp>,
}

impl ModelEntry {
    fn hours(&self, now: Timestamp) -> f64 {
        self.banked + self.running_since.map_or(0.0, |since| hours_between(since, now))
    }
}

#[derive(Default)]
struct Model {
    entries: Vec<ModelEntry>,
}

impl Model {
    fn stop_running(&mut self, now: Timestamp) {
        for entry in &mut self.entries {
            if let Some(since) = entry.running_since.take() {
                entry.banked += hours_between(since, now);
            }
        }
    }

    /// Starting a target already on today's list resumes it.
    fn start(&mut self, target: usize, now: Timestamp) {
        if let Some(index) = self.entries.iter().position(|entry| entry.target == target) {
            if self.entries[index].running_since.is_none() {
                self.stop_running(now);
                self.entries[index].running_since = Some(now);
            }
            return;
        }
        self.stop_running(now);
        self.entries.push(ModelEntry {
            target,
            banked: 0.0,
            running_since: Some(now),
        });
    }
}

// MARK: Actions

#[derive(Clone, Debug)]
enum Action {
    Start(usize),
    StopRunning,
    SetHours { entry: usize, quarter_hours: u8 },
    Delete(usize),
    Wait { minutes: u32 },
    Network(Network),
    Sync,
    /// A sync whose requests meet `fates` in order, after which the network is
    /// back. Lets a lost reply land on a particular request of a drain: the lookup
    /// a retried create makes first, then its POST.
    SyncThrough(Vec<Fate>),
    /// The app quits and is launched again, keeping only what it wrote to disk
    /// and the stored login.
    Relaunch,
    /// Someone changes the timesheet elsewhere: on the web, the phone, another
    /// machine.
    Elsewhere(Remote),
}

#[derive(Clone, Debug)]
enum Remote {
    SetHours { entry: usize, quarter_hours: u8 },
    SetNotes { entry: usize },
    Delete(usize),
    Start(usize),
    Stop,
}

fn remote() -> impl Strategy<Value = Remote> {
    prop_oneof![
        (0..3usize, 1..16u8).prop_map(|(entry, quarter_hours)| Remote::SetHours { entry, quarter_hours }),
        (0..3usize).prop_map(|entry| Remote::SetNotes { entry }),
        (0..3usize).prop_map(Remote::Delete),
        (0..TARGETS.len()).prop_map(Remote::Start),
        Just(Remote::Stop),
    ]
}

#[derive(Clone, Debug)]
enum Network {
    Online,
    Offline,
    Flaky(Vec<Fate>),
}

fn fate() -> impl Strategy<Value = Fate> {
    prop_oneof![
        3 => Just(Fate::Delivered),
        1 => Just(Fate::LostOnTheWay),
        1 => Just(Fate::LostOnTheWayBack),
        1 => Just(Fate::ServerError),
        1 => Just(Fate::Garbled),
    ]
}

fn action() -> impl Strategy<Value = Action> {
    prop_oneof![
        4 => (0..TARGETS.len()).prop_map(Action::Start),
        2 => Just(Action::StopRunning),
        1 => (0..3usize, 1..16u8).prop_map(|(entry, quarter_hours)| Action::SetHours { entry, quarter_hours }),
        1 => (0..3usize).prop_map(Action::Delete),
        4 => (10..120u32).prop_map(|minutes| Action::Wait { minutes }),
        1 => Just(Action::Network(Network::Online)),
        3 => Just(Action::Network(Network::Offline)),
        3 => prop::collection::vec(fate(), 1..4).prop_map(Action::SyncThrough),
        2 => prop::collection::vec(fate(), 1..8).prop_map(|fates| Action::Network(Network::Flaky(fates))),
        // The moments that matter most, made common rather than left to chance: a
        // request saved with its reply lost, as the next or the one after it. In a
        // sync with changes waiting, those are the drain's first requests.
        2 => prop_oneof![
            Just(vec![Fate::LostOnTheWayBack]),
            Just(vec![Fate::Delivered, Fate::LostOnTheWayBack]),
            Just(vec![Fate::Garbled]),
        ].prop_map(|fates| Action::Network(Network::Flaky(fates))),
        1 => Just(Action::Sync),
        2 => Just(Action::Relaunch),
        2 => remote().prop_map(Action::Elsewhere),
    ]
}

// MARK: The property

fn hours_between(from: Timestamp, to: Timestamp) -> f64 {
    ((to.as_nanosecond() - from.as_nanosecond()) as f64 / 1e9).max(0.0) / 3600.0
}

fn round2(hours: f64) -> f64 {
    (hours * 100.0).round() / 100.0
}

fn target(index: usize) -> TimerTarget {
    let (project, project_name, task, task_name) = TARGETS[index];
    TimerTarget {
        project: sheaves_core::models::Reference::new(project, project_name),
        task: sheaves_core::models::Reference::new(task, task_name),
        client: Some(sheaves_core::models::Reference::new(1, "123 Industries")),
    }
}

/// Launches the app as a shell does: a tracker over the files in `directory` and
/// the stored login, bootstrapped.
async fn launch(
    harvest: &Arc<FakeHarvest>,
    clock: &Arc<ManualClock>,
    directory: &std::path::Path,
    credentials: &Arc<MemoryCredentialStore>,
) -> TimeTracker {
    let tracker = TimeTracker::new(
        HarvestClient::new(None, harvest.clone(), 0.0),
        credentials.clone() as Arc<dyn CredentialStore>,
        SnapshotStore::new(directory.join("cache.json")),
        MutationQueue::open(directory.join("queue.json"), clock.clone()),
        clock.clone(),
    );
    tracker.bootstrap().await;
    tracker
}

fn run(actions: Vec<Action>) -> Result<(), TestCaseError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async move {
        let start: Timestamp = "2026-08-29T08:00:00Z".parse().unwrap();
        let clock = Arc::new(ManualClock::new(start, TimeZone::UTC));
        let harvest = FakeHarvest::new(clock.clone());
        let directory = tempfile::tempdir().unwrap();
        let credentials = Arc::new(MemoryCredentialStore::new(Some(Login {
            credentials: Credentials::new("12345", "token"),
            user_id: Some(USER_ID),
        })));
        let mut tracker = launch(&harvest, &clock, directory.path(), &credentials).await;
        let today = CalendarDate::of(start, &TimeZone::UTC);
        let mut model = Model::default();
        // The model knows only what this user did here; once the timesheet is
        // changed elsewhere, only the comparison with a fresh launch holds.
        let mut edited_elsewhere = false;

        for action in actions {
            // The day stays the same: past four in the afternoon, only waits that
            // keep it so are taken.
            let now = clock.now();
            match action {
                Action::Start(index) => {
                    tracker.start(&target(index), None).await;
                    model.start(index, now);
                }
                Action::StopRunning => {
                    tracker.stop_running().await;
                    model.stop_running(now);
                }
                Action::SetHours { entry, quarter_hours } => {
                    let hours = f64::from(quarter_hours) / 4.0;
                    let state = tracker.state();
                    let Some(local) = state.entries.get(entry).cloned() else { continue };
                    let Some(index) = TARGETS
                        .iter()
                        .position(|(p, _, t, _)| *p == local.project.id && *t == local.task.id)
                    else {
                        continue;
                    };
                    let Some(modelled) = model.entries.iter_mut().find(|e| e.target == index) else {
                        continue;
                    };
                    tracker.update_hours(&local, hours).await;
                    modelled.banked = hours;
                    if modelled.running_since.is_some() {
                        modelled.running_since = Some(now);
                    }
                }
                Action::Delete(entry) => {
                    let state = tracker.state();
                    let Some(local) = state.entries.get(entry).cloned() else { continue };
                    let Some(index) = TARGETS
                        .iter()
                        .position(|(p, _, t, _)| *p == local.project.id && *t == local.task.id)
                    else {
                        continue;
                    };
                    tracker.delete(&local).await;
                    model.entries.retain(|e| e.target != index);
                }
                Action::Wait { minutes } => {
                    let seconds = f64::from(minutes) * 60.0;
                    let end_of_day: Timestamp = "2026-08-29T23:00:00Z".parse().unwrap();
                    if crate::hours_between(now, end_of_day) * 3600.0 > seconds {
                        clock.advance(seconds);
                    }
                }
                Action::Network(Network::Online) => {
                    *harvest.offline.lock().unwrap() = false;
                    harvest.fates.lock().unwrap().clear();
                }
                Action::Network(Network::Offline) => *harvest.offline.lock().unwrap() = true,
                Action::Network(Network::Flaky(fates)) => {
                    *harvest.offline.lock().unwrap() = false;
                    *harvest.fates.lock().unwrap() = fates;
                }
                Action::Sync => tracker.sync().await,
                Action::SyncThrough(fates) => {
                    *harvest.offline.lock().unwrap() = false;
                    *harvest.fates.lock().unwrap() = fates;
                    tracker.sync().await;
                    harvest.fates.lock().unwrap().clear();
                }
                Action::Elsewhere(change) => {
                    edited_elsewhere = true;
                    harvest.edit(change);
                }
                Action::Relaunch => {
                    // The old process is gone before the new one reads its files.
                    drop(tracker);
                    tracker = launch(&harvest, &clock, directory.path(), &credentials).await;
                }
            }
        }

        // The network recovers, and the queue is given every chance to drain.
        *harvest.offline.lock().unwrap() = false;
        harvest.fates.lock().unwrap().clear();
        for _ in 0..6 {
            tracker.sync().await;
            if tracker.state().pending_count == 0 {
                break;
            }
        }
        prop_assert_eq!(tracker.state().pending_count, 0, "the queue drains once the network is back");

        // Whatever happened, the app now shows what a fresh launch, reading Harvest
        // alone, would show.
        let shown = tracker.state();
        let fresh_directory = tempfile::tempdir().unwrap();
        let fresh = launch(&harvest, &clock, fresh_directory.path(), &credentials).await.state();
        let now = clock.now();
        let describe = |entries: &[sheaves_core::tracked_entry::TrackedEntry]| {
            let mut described: Vec<(String, i64, i64, Option<String>, i64, bool)> = entries
                .iter()
                .filter(|entry| entry.spent_date == today)
                .map(|entry| {
                    (
                        format!("{:?}", entry.id),
                        entry.project.id,
                        entry.task.id,
                        entry.notes.clone().filter(|notes| !notes.is_empty()),
                        (entry.hours(now) * 100.0).round() as i64,
                        entry.is_running,
                    )
                })
                .collect();
            described.sort();
            described
        };
        prop_assert_eq!(describe(&shown.entries), describe(&fresh.entries), "the app and a fresh launch disagree");
        if edited_elsewhere {
            return Ok(());
        }

        let server = harvest.entries.lock().unwrap().clone();
        let today = today.to_string();
        for (index, _) in TARGETS.iter().enumerate() {
            let (project, _, task, _) = TARGETS[index];
            let on_server: Vec<&ServerEntry> = server
                .iter()
                .filter(|entry| entry.project_id == project && entry.task_id == task && entry.spent_date == today)
                .collect();
            let modelled = model.entries.iter().find(|entry| entry.target == index);
            match modelled {
                None => prop_assert!(
                    on_server.is_empty(),
                    "target {} was deleted or never started, but Harvest holds {:?}",
                    index,
                    on_server
                ),
                Some(modelled) => {
                    prop_assert_eq!(
                        on_server.len(),
                        1,
                        "target {} should be one entry, Harvest holds {:?}",
                        index,
                        on_server
                    );
                    let held = on_server[0].hours(now);
                    let worked = modelled.hours(now);
                    prop_assert!(
                        (held - worked).abs() <= 0.05,
                        "target {}: Harvest holds {:.3} h, the user worked {:.3} h",
                        index,
                        held,
                        worked
                    );
                    prop_assert_eq!(
                        on_server[0].running_since.is_some(),
                        modelled.running_since.is_some(),
                        "target {} running on Harvest and for the user should agree",
                        index
                    );
                }
            }
        }
        Ok(())
    })
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        failure_persistence: Some(Box::new(proptest::test_runner::FileFailurePersistence::WithSource("regressions"))),
        ..ProptestConfig::default()
    })]

    #[test]
    fn harvest_ends_up_holding_what_the_user_did(actions in prop::collection::vec(action(), 1..30)) {
        run(actions)?;
    }
}
