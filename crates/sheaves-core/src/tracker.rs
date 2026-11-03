//! The application's model: what is on the clock, what happened today, and what
//! Sheaves still owes Harvest.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use jiff::Timestamp;
use uuid::Uuid;

use crate::absence::{Absence, AbsenceResolution};
use crate::budget::ProjectBudget;
use crate::calendar_date::CalendarDate;
use crate::client::{Credentials, HarvestClient, HarvestError};
use crate::clock::Clock;
use crate::credentials::{CredentialError, CredentialStore, Login};
use crate::models::{HarvestCompany, HarvestUser, TimeEntry, seconds_between};
use crate::queue::{Mutation, MutationQueue, Owner};
use crate::snapshot::{CachedSnapshot, SnapshotStore};
use crate::tracked_entry::{
    EntryId, ProjectTargets, TimerTarget, TrackedEntry, group_by_project, timer_targets,
};

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum Connection {
    NeedsCredentials,
    Connecting,
    Online,
    /// Showing cached data; `reason` explains why the last sync failed.
    Offline {
        reason: String,
    },
}

impl Connection {
    pub fn is_configured(&self) -> bool {
        *self != Connection::NeedsCredentials
    }
}

/// What the menu bar should offer: a timer to pause, one to resume, or nothing.
#[derive(Clone, Debug, PartialEq, uniffi::Enum)]
pub enum Activity {
    Idle,
    Running(TrackedEntry),
    /// Stopped, but recently enough that resuming it is the likely next action.
    Recent(TrackedEntry),
}

impl Activity {
    pub fn entry(&self) -> Option<&TrackedEntry> {
        match self {
            Activity::Idle => None,
            Activity::Running(entry) | Activity::Recent(entry) => Some(entry),
        }
    }
}

/// How the machine is powered, as far as polite background traffic cares.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum PowerState {
    PluggedIn,
    Battery,
    /// The user has asked the whole machine to conserve, so ambient network activity
    /// should stop rather than merely slow.
    LowPower,
}

/// Everything a shell draws, as one value. A shell receives a fresh copy each time
/// anything in it changes.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct TrackerState {
    /// Counts changes, so a shell receiving states out of order can keep the newest.
    pub revision: u64,
    pub connection: Connection,
    pub user: Option<HarvestUser>,
    pub company: Option<HarvestCompany>,
    pub targets: Vec<TimerTarget>,
    pub entries: Vec<TrackedEntry>,
    pub recent_targets: Vec<TimerTarget>,
    /// Ranked by what the user has actually logged in Harvest, best first.
    pub frequent_targets: Vec<TimerTarget>,
    pub day: CalendarDate,
    pub today: CalendarDate,
    pub last_synced_at: Option<Timestamp>,
    pub pending_count: u32,
    /// Budgets for the projects that have one this token may read, keyed by project
    /// id. Permanently empty on accounts with none.
    pub budgets: HashMap<i64, ProjectBudget>,
    /// Advances while the tracker ticks, so durations stay live.
    pub now: Timestamp,
    /// The last thing worked on today, whether or not it is still running.
    ///
    /// Held separately from `entries` because the panel can browse other days, and
    /// what the menu bar shows should not change just because the user looked at
    /// last Tuesday.
    pub last_active_today: Option<TrackedEntry>,
}

/// How long a stopped timer stays one click away before the menu bar collapses back
/// to a bare icon.
pub const RECENT_ACTIVITY_WINDOW: f64 = 90.0 * 60.0;

impl TrackerState {
    pub fn running_entry(&self) -> Option<&TrackedEntry> {
        self.entries.iter().find(|entry| entry.is_running)
    }

    pub fn activity(&self) -> Activity {
        if let Some(running) = self.running_entry() {
            return Activity::Running(running.clone());
        }
        match &self.last_active_today {
            Some(recent)
                if !recent.is_running
                    && seconds_between(recent.updated_at, self.now) <= RECENT_ACTIVITY_WINDOW =>
            {
                Activity::Recent(recent.clone())
            }
            _ => Activity::Idle,
        }
    }

    pub fn is_today(&self) -> bool {
        self.day == self.today
    }

    /// The budget to show beside `entry`, or `None` when there is nothing to show.
    pub fn budget_for(&self, entry: &TrackedEntry) -> Option<&ProjectBudget> {
        self.budgets.get(&entry.project.id)
    }

    /// Total hours for the visible day, counting a running timer up to `now`.
    pub fn total_hours(&self) -> f64 {
        self.entries.iter().map(|entry| entry.hours(self.now)).sum()
    }

    /// Targets in the order worth offering them: what was started in this app most
    /// recently, then what the user actually logs in Harvest, then the rest.
    ///
    /// Harvest hands back every task assigned to every project, alphabetically,
    /// which buries the two or three things someone does daily under ones they have
    /// never touched.
    pub fn ordered_targets(&self) -> Vec<TimerTarget> {
        // Only offer pairs still assigned to the user; history outlives assignments.
        let mut available: HashMap<String, &TimerTarget> = HashMap::new();
        for target in &self.targets {
            available.entry(target.id()).or_insert(target);
        }
        let mut seen = HashSet::new();
        self.recent_targets
            .iter()
            .chain(&self.frequent_targets)
            .chain(&self.targets)
            .filter_map(|candidate| available.get(&candidate.id()).copied())
            .filter(|target| seen.insert(target.id()))
            .cloned()
            .collect()
    }

    /// The same ranking, one row per project.
    pub fn ordered_projects(&self) -> Vec<ProjectTargets> {
        group_by_project(&self.ordered_targets())
    }
}

/// How long the visible day may go unchecked before the tracker asks Harvest
/// uninvited, in seconds, or `None` when it should not ask at all.
///
/// Every user action already syncs on the spot; the probe exists to catch changes
/// made elsewhere, the web timesheet or a phone. Its cadence follows what the
/// request costs: someone mid-workday sees outside edits land within a minute, a
/// machine idle for hours checks occasionally, a battery slows both, and Low Power
/// Mode is the user saying to spend nothing, so the probe stops and syncing goes
/// back to being purely event-driven.
pub fn probe_interval(activity: &Activity, power: PowerState) -> Option<f64> {
    let idle = matches!(activity, Activity::Idle);
    match (power, idle) {
        (PowerState::LowPower, _) => None,
        (PowerState::PluggedIn, true) => Some(10.0 * 60.0),
        (PowerState::PluggedIn, false) => Some(60.0),
        (PowerState::Battery, true) => Some(30.0 * 60.0),
        (PowerState::Battery, false) => Some(5.0 * 60.0),
    }
}

/// Why `connect` did not connect.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum ConnectError {
    #[error(transparent)]
    Harvest(#[from] HarvestError),
    /// The queue still holds changes made under a different account or user.
    /// Sending them is not an option, see [`Owner`], so the choice is the user's: go
    /// back to the credentials that made them, or connect again discarding them.
    #[error("{count} unsent change{} made under a different Harvest account.", if *.count == 1 { " was" } else { "s were" })]
    UnsentChangesBelongElsewhere { count: usize },
    #[error(transparent)]
    Credentials(#[from] CredentialError),
}

/// What the last budget probe learned, and when.
///
/// Only a refusal is final. An account that budgets nothing today may budget
/// something next week, and a menu bar app opened at login can run for weeks, so
/// "none" is a slow question rather than a closed one.
#[derive(Clone, Copy, Debug)]
enum BudgetAvailability {
    Unknown,
    Some { as_of: Timestamp },
    None { as_of: Timestamp },
    Refused,
}

struct State {
    connection: Connection,
    user: Option<HarvestUser>,
    company: Option<HarvestCompany>,
    targets: Vec<TimerTarget>,
    entries: Vec<TrackedEntry>,
    recent_targets: Vec<TimerTarget>,
    frequent_targets: Vec<TimerTarget>,
    day: CalendarDate,
    last_synced_at: Option<Timestamp>,
    pending_count: u32,
    budgets: HashMap<i64, ProjectBudget>,
    now: Timestamp,
    last_active_today: Option<TrackedEntry>,
    /// When the last sync *began*, however it was triggered and however it ended.
    /// The probe paces itself by this rather than `last_synced_at` so a failing
    /// connection is retried on the probe's cadence, not on every tick.
    last_sync_started_at: Option<Timestamp>,
    account_data_fetched_at: Option<Timestamp>,
    budget_availability: BudgetAvailability,
    /// Client currencies by client id, and whether Harvest refused to say. Only an
    /// administrator or a manager who may edit clients can read them, nearly the
    /// same permission a monetary budget already needs.
    currencies: HashMap<i64, String>,
    currencies_refused: bool,
    /// Set once the user navigates away from today, so the tracker stops following
    /// the calendar. Without this, a menu bar app left running for days keeps
    /// logging to whichever day it happened to launch on.
    is_pinned_to_day: bool,
}

const MAX_RECENTS: usize = 12;
const ACCOUNT_DATA_LIFETIME: f64 = 600.0;
/// Budgets come from the Reports API, whose allowance is 100 requests per 15
/// *minutes*. Five minutes keeps the count in single digits per window however hard
/// start and stop are hammered.
const BUDGET_LIFETIME: f64 = 300.0;
/// How long an account with nothing to show is left alone. Long, because the answer
/// rarely changes; finite, because it can.
const EMPTY_BUDGET_LIFETIME: f64 = 3600.0;
/// How far back to look when working out what the user actually works on.
const HISTORY_WINDOW_DAYS: i32 = 90;
const HALF_LIFE_DAYS: f64 = 21.0;

type Observer = Arc<dyn Fn(TrackerState) + Send + Sync>;
type PowerProbe = Arc<dyn Fn() -> PowerState + Send + Sync>;

struct Inner {
    client: HarvestClient,
    credentials: Arc<dyn CredentialStore>,
    snapshots: SnapshotStore,
    queue: MutationQueue,
    clock: Arc<dyn Clock>,
    state: Mutex<State>,
    /// Syncs run one at a time, in the order they were asked for. A burst of clicks
    /// each enqueue a mutation, and tearing down a drain half way through would
    /// report a failure that never happened; two drains at once would send the same
    /// mutation twice.
    sync_gate: tokio::sync::Mutex<()>,
    syncs_in_flight: AtomicUsize,
    /// Set while `connect` is between installing credentials and claiming the queue
    /// for them; no sync may drain in that window.
    is_changing_credentials: AtomicBool,
    observer: Mutex<Option<Observer>>,
    power: Mutex<PowerProbe>,
    ticker: Mutex<Option<tokio::task::JoinHandle<()>>>,
    revision: AtomicU64,
    /// The Harvest user the stored login says the installed credentials belong to,
    /// or `None` when that is not known yet. The queue's owner and the cached user
    /// are checked against this, never the other way round.
    known_user: Mutex<Option<i64>>,
}

/// The application's model.
///
/// Every user action lands in local state first and reaches Harvest afterwards,
/// through the [`MutationQueue`]. That is what makes the menu bar feel instant and
/// what keeps it working on a dropped connection; the cost is that local state is
/// the truth until a sync succeeds, at which point the server's answer replaces it
/// wholesale.
#[derive(Clone)]
pub struct TimeTracker {
    inner: Arc<Inner>,
}

impl TimeTracker {
    pub fn new(
        client: HarvestClient,
        credentials: Arc<dyn CredentialStore>,
        snapshots: SnapshotStore,
        queue: MutationQueue,
        clock: Arc<dyn Clock>,
    ) -> Self {
        let today = clock.today();
        let now = clock.now();
        let pending_count = queue.count() as u32;
        Self {
            inner: Arc::new(Inner {
                client,
                credentials,
                snapshots,
                queue,
                state: Mutex::new(State {
                    connection: Connection::NeedsCredentials,
                    user: None,
                    company: None,
                    targets: Vec::new(),
                    entries: Vec::new(),
                    recent_targets: Vec::new(),
                    frequent_targets: Vec::new(),
                    day: today,
                    last_synced_at: None,
                    pending_count,
                    budgets: HashMap::new(),
                    now,
                    last_active_today: None,
                    last_sync_started_at: None,
                    account_data_fetched_at: None,
                    budget_availability: BudgetAvailability::Unknown,
                    currencies: HashMap::new(),
                    currencies_refused: false,
                    is_pinned_to_day: false,
                }),
                clock,
                sync_gate: tokio::sync::Mutex::new(()),
                syncs_in_flight: AtomicUsize::new(0),
                is_changing_credentials: AtomicBool::new(false),
                observer: Mutex::new(None),
                power: Mutex::new(Arc::new(|| PowerState::PluggedIn)),
                ticker: Mutex::new(None),
                revision: AtomicU64::new(0),
                known_user: Mutex::new(None),
            }),
        }
    }

    /// The whole observable state, as it is now.
    pub fn state(&self) -> TrackerState {
        let state = self.inner.state.lock().unwrap();
        self.inner.snapshot_of(&state)
    }

    /// Calls `observer` with the whole state after every change.
    pub fn observe(&self, observer: impl Fn(TrackerState) + Send + Sync + 'static) {
        *self.inner.observer.lock().unwrap() = Some(Arc::new(observer));
        self.inner.changed();
    }

    /// Where the probe learns how the machine is powered. Until a shell says
    /// otherwise, the machine is treated as plugged in.
    pub fn set_power_source(&self, probe: impl Fn() -> PowerState + Send + Sync + 'static) {
        *self.inner.power.lock().unwrap() = Arc::new(probe);
    }

    // MARK: Lifecycle

    /// Restores cached state, then syncs. Safe to call more than once.
    pub async fn bootstrap(&self) {
        // The login first, so the cache is restored only if it is the login's.
        let login = self
            .inner
            .credentials
            .read()
            .ok()
            .flatten()
            .filter(|login| login.credentials.is_complete());
        *self.inner.known_user.lock().unwrap() = login.as_ref().and_then(|login| login.user_id);
        match login {
            None => {
                self.inner.apply_cached_snapshot(false);
                self.inner
                    .update(|state| state.connection = Connection::NeedsCredentials);
            }
            Some(login) => {
                self.inner.client.set_credentials(Some(login.credentials));
                // A login saved before user ids were recorded vouches for nobody,
                // so Harvest is asked before any cache is believed. Offline, that
                // launch starts without the cache; the login is completed on the
                // first sync that reaches Harvest.
                if login.user_id.is_none()
                    && let Ok(user) = self.inner.client.current_user().await
                {
                    self.learn_user(user.id);
                }
                self.inner.apply_cached_snapshot(true);
                self.sync().await;
                self.start_ticking();
            }
        }
    }

    /// Verifies credentials against Harvest before storing them, so a typo is caught
    /// at the point the user can still see what they pasted.
    ///
    /// Reconnecting is mostly the *same* person after a token expired, and the queue
    /// exists so their offline work survives exactly that, so it is kept whenever
    /// the account and user match whoever queued it. Otherwise nothing is dropped
    /// without `discarding_unsent_changes`: a silent discard is the data loss this
    /// guards against, merely moved earlier.
    ///
    /// A connect that fails changes nothing: whatever credentials and connection
    /// were in place before are in place after.
    pub async fn connect(
        &self,
        credentials: Credentials,
        discarding_unsent_changes: bool,
    ) -> Result<(), ConnectError> {
        // Whitespace pasted around either value is trimmed here, so every shell gets
        // the same rule.
        let credentials = Credentials::new(&credentials.account_id, &credentials.token);
        let inner = &self.inner;
        let previous_connection = inner.state.lock().unwrap().connection.clone();
        inner.update(|state| state.connection = Connection::Connecting);
        // Puts everything back unless the connect succeeds, including when the
        // caller drops this future part way through.
        let mut change = CredentialChange {
            inner,
            previous_connection,
            previous_credentials: None,
            succeeded: false,
        };
        // The client is about to speak for someone new while the queue still speaks
        // for whoever came before. Let a sync already in flight finish under the
        // credentials it started with, and hold off any other until the queue has
        // been claimed: a probe or a click in this window would otherwise deliver
        // the old account's work with the new token.
        inner.is_changing_credentials.store(true, Ordering::SeqCst);
        drop(inner.sync_gate.lock().await);
        change.previous_credentials = Some(inner.client.installed_credentials());
        inner.client.set_credentials(Some(credentials.clone()));

        let result = async {
            let user = inner.client.current_user().await?;
            let owner = Owner {
                account_id: credentials.account_id.clone(),
                user_id: user.id,
            };
            let stranded = inner.queue.stranded_count(&owner);
            if stranded > 0 && !discarding_unsent_changes {
                return Err(ConnectError::UnsentChangesBelongElsewhere { count: stranded });
            }
            // The cache describes whoever was connected before. Gone before the new
            // credentials are saved, a crash between the two cannot leave the next
            // launch restoring the old user beside the new token.
            inner.snapshots.clear();
            // The login records the user it was verified to belong to, in the same
            // write as the token, so nothing can later read one without the other.
            inner.credentials.write(Login {
                credentials: credentials.clone(),
                user_id: Some(user.id),
            })?;
            inner.queue.claim(owner);
            Ok(user)
        }
        .await;

        match result {
            Ok(user) => {
                change.succeeded = true;
                drop(change);
                *inner.known_user.lock().unwrap() = Some(user.id);
                let pending = inner.queue.count() as u32;
                inner.update(|state| {
                    state.pending_count = pending;
                    inner.forget_account_data(state);
                    state.user = Some(user);
                });
                inner.is_changing_credentials.store(false, Ordering::SeqCst);
                self.sync().await;
                self.start_ticking();
                Ok(())
            }
            // `change` puts the old credentials and connection back as it drops.
            Err(error) => Err(error),
        }
    }

    pub async fn disconnect(&self) {
        self.stop_ticking();
        // Wait out a sync already in flight, holding its turn, or its refresh lands
        // after the teardown: the old account's entries back on screen and back in
        // the cache the next launch restores. Syncs queued behind this find no
        // credentials.
        let _turn = self.inner.sync_gate.lock().await;
        *self.inner.known_user.lock().unwrap() = None;
        if let Err(error) = self.inner.credentials.delete() {
            log::error!("could not delete the stored credentials: {error}");
        }
        self.inner.queue.remove_all();
        self.inner.client.set_credentials(None);
        self.inner.update(|state| {
            self.inner.forget_account_data(state);
            state.pending_count = 0;
            state.connection = Connection::NeedsCredentials;
        });
    }

    // MARK: Syncing

    /// Flushes queued changes, then reloads the visible day from Harvest.
    ///
    /// Syncs are serialised rather than cancelled, in the order they were asked for.
    pub async fn sync(&self) {
        // Counted down as the guard drops, so a caller that abandons the sync cannot
        // leave the probe believing one is still running.
        let _in_flight = InFlight::enter(&self.inner.syncs_in_flight);
        let _turn = self.inner.sync_gate.lock().await;
        self.perform_sync().await;
    }

    async fn perform_sync(&self) {
        let inner = &self.inner;
        if inner.is_changing_credentials.load(Ordering::SeqCst) {
            return;
        }
        if !inner.client.is_configured() {
            inner.update(|state| state.connection = Connection::NeedsCredentials);
            return;
        }
        let now = inner.clock.now();
        let today = inner.clock.today();
        inner.update(|state| {
            state.last_sync_started_at = Some(now);
            // Rolls the visible day over at midnight, unless the user is
            // deliberately looking at another day.
            if !state.is_pinned_to_day && state.day != today {
                state.day = today;
                // A running timer belongs in the menu bar whatever its day, and
                // only a refresh that succeeds replaces it: offline, clearing it
                // would leave nothing to stop, and the next start would queue no
                // measured stop for it.
                state.entries.retain(|entry| entry.is_running);
                state.last_active_today = None;
            }
        });

        // `connect` is what normally settles whose queue this is, but it writes the
        // credentials before it claims the queue, and a launch after dying in
        // between arrives here with new credentials and the old owner's work.
        // Checked at the drain so that no route to it can deliver someone else's
        // changes.
        match self.queue_belongs_elsewhere().await {
            Ok(true) => {
                log::error!("queued changes belong to another account or user; not sending them");
                let pending = inner.queue.count() as u32;
                // Settings is where this gets resolved: connecting there offers the
                // choice between going back to their owner and discarding them.
                inner.update(|state| {
                    state.pending_count = pending;
                    state.connection = Connection::NeedsCredentials;
                });
                return;
            }
            Ok(false) => {}
            Err(error) => {
                inner.note_sync_failure(&error);
                return;
            }
        }

        let report = inner.queue.drain(&inner.client).await;
        let pending = inner.queue.count() as u32;
        inner.update(|state| {
            adopt(state, &report.resolved);
            state.pending_count = pending;
        });
        if let Some(blocker) = &report.stopped_with {
            // A rejected token stops the drain too, and asks for new credentials.
            inner.note_sync_failure(blocker);
            return;
        }
        for (mutation, error) in &report.discarded {
            log::error!("dropped refused {}: {error}", mutation.summary());
        }
        // A refused change is dropped rather than retried forever; say so, but carry
        // on: the refresh below will show what Harvest actually holds. Said after the
        // refresh, which would otherwise mark the connection online over it.
        let refused = report.discarded.last().map(|(_, error)| error.to_string());

        match self.refresh().await {
            Err(error) => inner.note_sync_failure(&error),
            Ok(()) => {
                if let Some(reason) = refused {
                    inner.update(|state| state.connection = Connection::Offline { reason });
                }
            }
        }
    }

    async fn refresh(&self) -> Result<(), HarvestError> {
        let inner = &self.inner;
        let user = self.current_user().await?;
        let day = inner.state.lock().unwrap().day;
        let (day_entries, running) = tokio::try_join!(
            inner.client.time_entries(user.id, day, day),
            inner.client.running_time_entry(user.id),
        )?;
        if let Some(credentials) = inner.client.installed_credentials() {
            // A queue from before owners were recorded, one filled before the first
            // sync ever succeeded, or an empty one still naming whoever was connected
            // before, belongs to whoever is connected now.
            inner.queue.claim_if_free(Owner {
                account_id: credentials.account_id,
                user_id: user.id,
            });
        }
        let today = inner.clock.today();
        inner.update(|state| {
            state.entries = merge(&day_entries, running.as_ref());
            note_last_activity(state, today);
        });
        self.refresh_account_data_if_stale(user.id).await?;
        self.refresh_budgets_if_stale().await;
        let now = inner.clock.now();
        inner.update(|state| {
            state.last_synced_at = Some(now);
            state.connection = Connection::Online;
            prune_recents(state);
        });
        inner.save_snapshot();
        let state = self.state();
        log::info!(
            "sync ok: {} entries, {} targets on {}",
            state.entries.len(),
            state.targets.len(),
            state.day
        );
        Ok(())
    }

    /// The signed-in user, asked of Harvest only when nothing on hand says.
    async fn current_user(&self) -> Result<HarvestUser, HarvestError> {
        let known = *self.inner.known_user.lock().unwrap();
        let cached = self.inner.state.lock().unwrap().user.clone();
        if let Some(user) = cached
            && known.is_none_or(|known| known == user.id)
        {
            return Ok(user);
        }
        let fetched = self.inner.client.current_user().await?;
        self.learn_user(fetched.id);
        self.inner
            .update(|state| state.user = Some(fetched.clone()));
        Ok(fetched)
    }

    /// Records who the installed credentials were found to belong to, and saves it
    /// with a stored login that lacks it, as one saved before user ids were recorded
    /// does.
    fn learn_user(&self, user_id: i64) {
        let inner = &self.inner;
        *inner.known_user.lock().unwrap() = Some(user_id);
        let Ok(Some(login)) = inner.credentials.read() else {
            return;
        };
        if login.user_id.is_none()
            && Some(&login.credentials) == inner.client.installed_credentials().as_ref()
        {
            let upgraded = Login {
                user_id: Some(user_id),
                ..login
            };
            if let Err(error) = inner.credentials.write(upgraded) {
                log::error!("could not record the user with the stored login: {error}");
            }
        }
    }

    /// Whether the queue was filled under an account or user other than the one the
    /// installed credentials speak for. The account settles most cases without a
    /// request; only a matching account needs the user.
    async fn queue_belongs_elsewhere(&self) -> Result<bool, HarvestError> {
        let inner = &self.inner;
        let (Some(owner), Some(credentials)) =
            (inner.queue.owner(), inner.client.installed_credentials())
        else {
            return Ok(false);
        };
        if inner.queue.is_empty() {
            return Ok(false);
        }
        if owner.account_id != credentials.account_id {
            return Ok(true);
        }
        // The stored login says whose the credentials are. Only a login saved before
        // that was recorded needs Harvest asked, once; never the cached user, which
        // can come from a snapshot written under other credentials.
        let known = *inner.known_user.lock().unwrap();
        let user_id = match known {
            Some(user_id) => user_id,
            None => {
                let user = inner.client.current_user().await?;
                self.learn_user(user.id);
                user.id
            }
        };
        Ok(owner.user_id != user_id)
    }

    /// Project assignments and company settings change rarely and cost two
    /// requests, so they are not refetched on every start and stop.
    async fn refresh_account_data_if_stale(&self, user_id: i64) -> Result<(), HarvestError> {
        let inner = &self.inner;
        let now = inner.clock.now();
        let is_stale = {
            let state = inner.state.lock().unwrap();
            let expired = state
                .account_data_fetched_at
                .is_none_or(|fetched| seconds_between(fetched, now) > ACCOUNT_DATA_LIFETIME);
            expired || state.targets.is_empty()
        };
        if !is_stale {
            return Ok(());
        }
        let today = inner.clock.today();
        let (company, assignments, history) = tokio::try_join!(
            inner.client.company(),
            inner.client.project_assignments(),
            inner
                .client
                .time_entries(user_id, today.adding_days(-HISTORY_WINDOW_DAYS), today),
        )?;
        let targets = timer_targets(&assignments);
        let frequent = rank_by_usage(&history, today);
        log::info!(
            "account data: {} assignments -> {} active targets, {} used in the last {HISTORY_WINDOW_DAYS} days",
            assignments.len(),
            targets.len(),
            frequent.len()
        );
        inner.update(|state| {
            state.company = Some(company);
            state.targets = targets;
            state.frequent_targets = frequent;
            state.account_data_fetched_at = Some(now);
        });
        Ok(())
    }

    /// Budgets, on a clock of their own.
    ///
    /// Two things set this apart from the rest of the sync. The Reports API allows
    /// 100 requests per 15 minutes rather than per 15 seconds, so budgets refresh on
    /// their own slow timer instead of riding every start and stop. And a budget is
    /// a decoration: an account whose projects are all `budget_by: none`, or a token
    /// without the permission a monetary budget needs, has nothing to show, so an
    /// answer with nothing in it switches the feature off rather than leaving an
    /// empty frame in the panel. Nothing here can fail a sync, which is why it
    /// neither returns an error nor touches the connection.
    async fn refresh_budgets_if_stale(&self) {
        let inner = &self.inner;
        let now = inner.clock.now();
        let availability = inner.state.lock().unwrap().budget_availability;
        match availability {
            BudgetAvailability::Refused => return,
            BudgetAvailability::Some { as_of } if seconds_between(as_of, now) < BUDGET_LIFETIME => {
                return;
            }
            BudgetAvailability::None { as_of }
                if seconds_between(as_of, now) < EMPTY_BUDGET_LIFETIME =>
            {
                return;
            }
            _ => {}
        }

        match inner.client.project_budgets().await {
            Ok(report) => {
                let mut readable: Vec<ProjectBudget> = report
                    .iter()
                    .filter(|budget| budget.is_active && budget.has_readable_budget())
                    .cloned()
                    .collect();
                // Only when there is money to label. An account budgeting purely in
                // hours never needs the currency, so it never spends the request.
                if readable.iter().any(|budget| budget.budget_by.is_monetary()) {
                    self.refresh_currencies().await;
                    let currencies = inner.state.lock().unwrap().currencies.clone();
                    for budget in &mut readable {
                        budget.currency_code =
                            budget.client_id.and_then(|id| currencies.get(&id).cloned());
                    }
                }
                log::info!(
                    "budgets: {} of {} projects have one to show",
                    readable.len(),
                    report.len()
                );
                let as_of = inner.clock.now();
                inner.update(|state| {
                    state.budget_availability = if readable.is_empty() {
                        BudgetAvailability::None { as_of }
                    } else {
                        BudgetAvailability::Some { as_of }
                    };
                    state.budgets = HashMap::new();
                    for budget in readable {
                        state.budgets.entry(budget.project_id).or_insert(budget);
                    }
                });
            }
            Err(HarvestError::Unauthorized | HarvestError::Forbidden) => {
                // The report is readable only by an administrator, or a manager
                // holding the billable-rates permission. A refusal is a settled
                // answer rather than a failure worth retrying: the sync has already
                // proved the token good for everything else.
                log::info!("budgets: this token may not read them; the budget display stays off");
                inner.update(|state| {
                    state.budgets.clear();
                    state.budget_availability = BudgetAvailability::Refused;
                });
            }
            Err(error) => {
                // Anything else is transient. Leave the last known budgets on screen
                // and ask again on the next sync.
                log::error!("budgets: {error}");
            }
        }
    }

    /// The currency each client bills in.
    ///
    /// Harvest reports a currency only on the client record, so labelling a
    /// monetary budget costs a request the budget report itself cannot avoid.
    /// Clients change about never, so this rides the budget refresh rather than
    /// having a clock of its own, and a refusal is permanent: without it a monetary
    /// budget shows a bare number, which is incomplete where a guessed symbol would
    /// be wrong.
    async fn refresh_currencies(&self) {
        let inner = &self.inner;
        if inner.state.lock().unwrap().currencies_refused {
            return;
        }
        match inner.client.clients().await {
            Ok(records) => {
                let mut currencies = HashMap::new();
                for record in records {
                    if let Some(currency) = record.currency {
                        currencies.entry(record.id).or_insert(currency);
                    }
                }
                log::info!("currencies: {} clients name one", currencies.len());
                inner.update(|state| state.currencies = currencies);
            }
            Err(HarvestError::Unauthorized | HarvestError::Forbidden) => {
                log::info!("currencies: this token may not read clients; money shows no symbol");
                inner.update(|state| state.currencies_refused = true);
            }
            Err(error) => log::error!("currencies: {error}"),
        }
    }

    pub async fn select_day(&self, new_day: CalendarDate) {
        let today = self.inner.clock.today();
        let changed = self.inner.update(|state| {
            state.is_pinned_to_day = new_day != today;
            if new_day == state.day {
                return false;
            }
            state.day = new_day;
            // As at midnight: a running timer stays until a refresh replaces it.
            state.entries.retain(|entry| entry.is_running);
            true
        });
        if changed {
            self.sync().await;
        }
    }

    pub async fn go_to_today(&self) {
        self.select_day(self.inner.clock.today()).await;
    }

    pub async fn shift_day(&self, days: i32) {
        let day = self.inner.state.lock().unwrap().day;
        self.select_day(day.adding_days(days)).await;
    }

    // MARK: Timer actions

    /// Starts, or resumes, a timer for `target` on the visible day.
    ///
    /// The dedupe below can only see the visible day's list, which is why "start on
    /// today instead" goes to today *first*: started blind from a past day's view,
    /// it duplicated an entry the user already had today.
    pub async fn start(&self, target: &TimerTarget, notes: Option<&str>) {
        let inner = &self.inner;
        let today = inner.clock.today();
        let existing = {
            let mut state = inner.state.lock().unwrap();
            let day = state.day;
            if let Some(existing) = state
                .entries
                .iter()
                .find(|entry| entry.target().id() == target.id() && entry.spent_date == day)
            {
                Some(existing.clone())
            } else if let Some(recent) = state.last_active_today.clone().filter(|recent| {
                // A failed load leaves the visible list empty, and an empty list
                // cannot dedupe, so on today, fall back on the one entry the tracker
                // always remembers. Without this, "start on today" while offline
                // duplicated an entry that already existed on Harvest.
                day == today
                    && !recent.is_running
                    && recent.spent_date == day
                    && recent.target().id() == target.id()
            }) {
                if !state.entries.iter().any(|entry| entry.id == recent.id) {
                    state.entries.insert(0, recent.clone());
                }
                Some(recent)
            } else {
                None
            }
        };
        if let Some(existing) = existing {
            self.resume(&existing).await;
            return;
        }

        self.stop_running_for_switch();
        let local = Uuid::new_v4();
        let now = inner.clock.now();
        let day = inner.update(|state| {
            let day = state.day;
            state.entries.insert(
                0,
                TrackedEntry {
                    id: EntryId::Local(local),
                    project: target.project.clone(),
                    task: target.task.clone(),
                    client: target.client.clone(),
                    notes: notes.map(str::to_string),
                    spent_date: day,
                    banked_hours: 0.0,
                    is_running: true,
                    timer_started_at: Some(now),
                    is_locked: false,
                    is_billable: true,
                    is_pending: true,
                    updated_at: now,
                },
            );
            note_recent(state, target);
            note_last_activity(state, today);
            day
        });
        self.restart_clock();
        self.enqueue(vec![Mutation::Create {
            local,
            target: target.clone(),
            spent_date: day,
            notes: notes.map(str::to_string),
            started_at: now,
            ended_at: None,
        }])
        .await;
    }

    pub async fn resume(&self, entry: &TrackedEntry) {
        if entry.is_running || entry.is_locked {
            return;
        }
        self.stop_running_for_switch();
        let inner = &self.inner;
        let now = inner.clock.now();
        let today = inner.clock.today();
        inner.update(|state| {
            mutate_locally(state, entry.id, now, |entry| {
                entry.is_running = true;
                entry.timer_started_at = Some(now);
                entry.is_pending = true;
            });
            note_recent(state, &entry.target());
            note_last_activity(state, today);
        });
        self.restart_clock();

        // A timer that has not reached Harvest yet is resumed by amending its queued
        // create, not by queueing a restart of an entry that does not exist.
        if let EntryId::Local(local) = entry.id
            && inner.queue.amend_create(local, None)
        {
            self.queue_changed().await;
            return;
        }
        self.enqueue(vec![Mutation::Restart {
            id: entry.id,
            resumed_at: now,
            banked_hours: entry.banked_hours,
        }])
        .await;
    }

    pub async fn stop(&self, entry: &TrackedEntry) {
        if !entry.is_running {
            return;
        }
        let mutation = self.record_stop(entry);
        let today = self.inner.clock.today();
        self.inner.update(|state| note_last_activity(state, today));
        self.restart_clock();
        match mutation {
            Some(mutation) => self.enqueue(vec![mutation]).await,
            // The stop was folded into a still-queued create.
            None => self.queue_changed().await,
        }
    }

    /// Stops `entry` locally and returns the mutation that records it, or `None`
    /// when a still-queued create absorbed the stop instead.
    ///
    /// The total measured here is authoritative; Harvest's own stop timing is not,
    /// because the request may not reach it for hours. That is also why switching
    /// timers must queue this rather than lean on Harvest stopping the old one
    /// implicitly: the implicit stop banks time up to whenever the *next* request
    /// lands, inflating the old entry by however long the network was away.
    fn record_stop(&self, entry: &TrackedEntry) -> Option<Mutation> {
        let stopped_at = self.inner.clock.now();
        let hours = entry.hours(stopped_at);
        self.inner.update(|state| {
            mutate_locally(state, entry.id, stopped_at, |entry| {
                entry.banked_hours = hours;
                entry.is_running = false;
                entry.timer_started_at = None;
                entry.is_pending = true;
            })
        });
        if let EntryId::Local(local) = entry.id
            && self.inner.queue.amend_create(local, Some(stopped_at))
        {
            return None;
        }
        Some(Mutation::Stop {
            id: entry.id,
            hours,
        })
    }

    /// The switch half of start and resume: whatever is running stops, measured now,
    /// and the recording mutation is queued ahead of whatever follows it.
    fn stop_running_for_switch(&self) {
        let running = self.state().running_entry().cloned();
        if let Some(running) = running
            && let Some(mutation) = self.record_stop(&running)
        {
            self.inner.queue.enqueue(mutation);
        }
    }

    pub async fn toggle(&self, entry: &TrackedEntry) {
        if entry.is_running {
            self.stop(entry).await;
        } else {
            self.resume(entry).await;
        }
    }

    /// Stops whatever is running, wherever it is.
    pub async fn stop_running(&self) {
        let running = self.state().running_entry().cloned();
        if let Some(running) = running {
            self.stop(&running).await;
        }
    }

    pub async fn update_notes(&self, entry: &TrackedEntry, notes: &str) {
        if entry.is_locked {
            return;
        }
        let now = self.inner.clock.now();
        self.inner.update(|state| {
            mutate_locally(state, entry.id, now, |entry| {
                entry.notes = Some(notes.to_string());
                entry.is_pending = true;
            })
        });
        self.enqueue(vec![Mutation::Update {
            id: entry.id,
            notes: Some(notes.to_string()),
            hours: None,
        }])
        .await;
    }

    pub async fn update_hours(&self, entry: &TrackedEntry, hours: f64) {
        if entry.is_locked {
            return;
        }
        let inner = &self.inner;
        let now = inner.clock.now();
        // Typing onto a running timer replaces the total and keeps it counting:
        // getting out of an untracked hour-long meeting and typing "1" must not also
        // stop the clock. Harvest agrees: patching a live entry's hours sets the
        // total and restarts its measurement from the moment the patch lands, which
        // is why the queued mutation carries when the user chose the number.
        if entry.is_running {
            let today = inner.clock.today();
            inner.update(|state| {
                mutate_locally(state, entry.id, now, |entry| {
                    entry.banked_hours = hours;
                    entry.timer_started_at = Some(now);
                    entry.is_pending = true;
                });
                note_last_activity(state, today);
            });
            self.restart_clock();
            self.enqueue(vec![Mutation::Adjust {
                id: entry.id,
                hours,
                as_of: now,
            }])
            .await;
            return;
        }
        inner.update(|state| {
            mutate_locally(state, entry.id, now, |entry| {
                entry.banked_hours = hours;
                entry.is_running = false;
                entry.timer_started_at = None;
                entry.is_pending = true;
            })
        });
        self.enqueue(vec![Mutation::Update {
            id: entry.id,
            notes: None,
            hours: Some(hours),
        }])
        .await;
    }

    pub async fn delete(&self, entry: &TrackedEntry) {
        if entry.is_locked {
            return;
        }
        let today = self.inner.clock.today();
        self.inner.update(|state| {
            state.entries.retain(|candidate| candidate.id != entry.id);
            // The menu bar's memory of today goes too, or "start on today" would
            // resume the deleted entry instead of creating one: the queue would
            // send the delete, then a restart Harvest refuses, and the new timer
            // would never exist.
            if state
                .last_active_today
                .as_ref()
                .is_some_and(|recent| recent.id == entry.id)
            {
                state.last_active_today = None;
                note_last_activity(state, today);
            }
        });
        self.enqueue(vec![Mutation::Delete { id: entry.id }]).await;
    }

    // MARK: Absences

    /// Applies what the user chose about a timer that ran while nobody was here.
    ///
    /// Every outcome is an ordinary mutation carrying explicit hours, which is the
    /// whole reason trimming needs no new machinery: the queue already knows how to
    /// send an honest total for work that finished long before the request lands.
    pub async fn resolve(
        &self,
        absence: &Absence,
        entry: &TrackedEntry,
        resolution: &AbsenceResolution,
    ) {
        // The question can sit on screen for hours, and in that time the entry can
        // be stopped from a phone, edited on the web, or deleted. Answer for the
        // entry as it is now: the captured copy is only a record of what was asked
        // about. An entry that is gone, or no longer running, is no longer ours to
        // trim.
        let Some(entry) = self
            .state()
            .entries
            .iter()
            .find(|candidate| candidate.id == entry.id)
            .cloned()
        else {
            return;
        };
        if entry.is_locked {
            return;
        }
        // `None` means this machine has no standing to speak for the entry.
        let Some(trimmed) = absence.trimmed_hours(&entry) else {
            return;
        };

        match resolution {
            AbsenceResolution::Keep => {}
            AbsenceResolution::TrimAndContinue => {
                self.trim(&entry, trimmed, absence.began, Some(absence.ended))
                    .await;
            }
            AbsenceResolution::TrimAndStop => {
                self.trim(&entry, trimmed, absence.began, None).await;
            }
            AbsenceResolution::TrimAndLog(target) => {
                // Carrying on would bank today's work onto a day that is over, so an
                // entry from an earlier day stops here just as `TrimAndStop` would.
                let resumes_at =
                    (entry.spent_date == self.inner.clock.today()).then_some(absence.ended);
                self.trim(&entry, trimmed, absence.began, resumes_at).await;
                self.log_absence(absence, target).await;
            }
        }
    }

    async fn trim(
        &self,
        entry: &TrackedEntry,
        hours: f64,
        left_at: Timestamp,
        resuming_at: Option<Timestamp>,
    ) {
        let inner = &self.inner;
        let now = inner.clock.now();
        let today = inner.clock.today();
        inner.update(|state| {
            mutate_locally(state, entry.id, now, |entry| {
                entry.banked_hours = hours;
                entry.is_running = resuming_at.is_some();
                entry.timer_started_at = resuming_at;
                entry.is_pending = true;
            });
            note_last_activity(state, today);
        });
        self.restart_clock();

        // A create that has not reached Harvest yet carries its hours in its own
        // start and end, so the trim amends that create rather than correcting an
        // entry Harvest has never heard of.
        let amended = match entry.id {
            EntryId::Local(local) => inner.queue.amend_create(local, Some(left_at)),
            EntryId::Server(_) => false,
        };

        // Harvest's copy of the timer is still running and still counting the
        // absence, so continuing is not one operation but three: stop it, correct
        // the total, start it again from the moment they came back.
        let mut mutations = vec![if amended {
            // The create now ends where they left, but it still reports the hours as
            // the span from its start, which silently includes any pause taken
            // before the queue ever drained. Send the total we measured instead.
            Mutation::Update {
                id: entry.id,
                notes: None,
                hours: Some(hours),
            }
        } else {
            Mutation::Stop {
                id: entry.id,
                hours,
            }
        }];
        if let Some(resumed_at) = resuming_at {
            // Safe for an amended create too: the queue drains in order and rewrites
            // the local id once the create comes back with a real one.
            mutations.push(Mutation::Restart {
                id: entry.id,
                resumed_at,
                banked_hours: hours,
            });
        }
        // Queued together, then sent: enqueueing one at a time syncs in between, so
        // a quit in that window would persist a stop with no restart behind it and
        // leave a timer the user asked to keep running stopped instead.
        self.enqueue(mutations).await;
    }

    /// Records the absence itself against another target: the meeting you were
    /// actually in while the timer sat on something else.
    async fn log_absence(&self, absence: &Absence, target: &TimerTarget) {
        // The absence's own day. A timer left running overnight belongs to
        // yesterday; the meeting that interrupted it this morning does not.
        let spent_date = absence.day(&self.inner.clock.time_zone());
        let local = Uuid::new_v4();
        let now = self.inner.clock.now();
        self.inner.update(|state| {
            if spent_date == state.day {
                state.entries.insert(
                    0,
                    TrackedEntry {
                        id: EntryId::Local(local),
                        project: target.project.clone(),
                        task: target.task.clone(),
                        client: target.client.clone(),
                        notes: None,
                        spent_date,
                        banked_hours: absence.duration() / 3600.0,
                        is_running: false,
                        timer_started_at: None,
                        is_locked: false,
                        is_billable: true,
                        is_pending: true,
                        updated_at: now,
                    },
                );
            }
            note_recent(state, target);
        });
        self.enqueue(vec![Mutation::Create {
            local,
            target: target.clone(),
            spent_date,
            notes: None,
            started_at: absence.began,
            ended_at: Some(absence.ended),
        }])
        .await;
    }

    // MARK: Local bookkeeping

    /// Queues a whole sequence before draining, so a sequence that only makes sense
    /// together is never half persisted.
    async fn enqueue(&self, mutations: Vec<Mutation>) {
        for mutation in mutations {
            self.inner.queue.enqueue(mutation);
        }
        self.queue_changed().await;
    }

    async fn queue_changed(&self) {
        let pending = self.inner.queue.count() as u32;
        self.inner.update(|state| state.pending_count = pending);
        self.inner.save_snapshot();
        self.sync().await;
    }

    // MARK: Clock

    fn start_ticking(&self) {
        let weak = Arc::downgrade(&self.inner);
        let handle = tokio::spawn(tick(weak));
        if let Some(previous) = self.inner.ticker.lock().unwrap().replace(handle) {
            previous.abort();
        }
    }

    fn stop_ticking(&self) {
        if let Some(previous) = self.inner.ticker.lock().unwrap().take() {
            previous.abort();
        }
    }

    /// Advances the clock now and re-picks the tick interval.
    fn restart_clock(&self) {
        let now = self.inner.clock.now();
        self.inner.update(|state| state.now = now);
        if self.inner.ticker.lock().unwrap().is_some() {
            self.start_ticking();
        }
    }
}

/// Counts one sync in flight for as long as it lives.
struct InFlight<'a>(&'a AtomicUsize);

impl<'a> InFlight<'a> {
    fn enter(count: &'a AtomicUsize) -> Self {
        count.fetch_add(1, Ordering::SeqCst);
        Self(count)
    }
}

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Undoes a `connect` that does not succeed, however it ends.
struct CredentialChange<'a> {
    inner: &'a Inner,
    previous_connection: Connection,
    /// `None` until the new credentials are installed.
    previous_credentials: Option<Option<Credentials>>,
    succeeded: bool,
}

impl Drop for CredentialChange<'_> {
    fn drop(&mut self) {
        if !self.succeeded {
            // The old credentials go back before syncs are let through again, or one
            // could drain under the token just refused.
            if let Some(previous) = self.previous_credentials.take() {
                self.inner.client.set_credentials(previous);
            }
            let previous_connection = self.previous_connection.clone();
            self.inner
                .update(|state| state.connection = previous_connection);
        }
        self.inner
            .is_changing_credentials
            .store(false, Ordering::SeqCst);
    }
}

/// The ticker: advances `now` so durations stay live, rolls the day over, and syncs
/// when the probe is due. It holds the tracker weakly, so dropping the last
/// `TimeTracker` ends it.
async fn tick(weak: Weak<Inner>) {
    loop {
        // Durations are shown to the minute, so a five-second tick is enough to turn
        // one over promptly. Idle, `now` only drives the "synced N minutes ago"
        // label, and redrawing the menu bar all day for that is waste.
        // `restart_clock` re-picks this the instant a timer changes, so starting one
        // never waits out the idle interval.
        let interval = match weak.upgrade() {
            Some(inner)
                if inner
                    .state
                    .lock()
                    .unwrap()
                    .entries
                    .iter()
                    .any(|e| e.is_running) =>
            {
                5
            }
            Some(_) => 30,
            None => return,
        };
        tokio::time::sleep(Duration::from_secs(interval)).await;
        let Some(inner) = weak.upgrade() else { return };
        let now = inner.clock.now();
        let today = inner.clock.today();
        let rolled_over = inner.update(|state| {
            state.now = now;
            !state.is_pinned_to_day && state.day != today
        });
        if rolled_over || inner.is_probe_due() {
            let tracker = TimeTracker { inner };
            tokio::spawn(async move { tracker.sync().await });
        }
    }
}

impl Inner {
    fn snapshot_of(&self, state: &State) -> TrackerState {
        TrackerState {
            revision: self.revision.load(Ordering::SeqCst),
            connection: state.connection.clone(),
            user: state.user.clone(),
            company: state.company.clone(),
            targets: state.targets.clone(),
            entries: state.entries.clone(),
            recent_targets: state.recent_targets.clone(),
            frequent_targets: state.frequent_targets.clone(),
            day: state.day,
            today: self.clock.today(),
            last_synced_at: state.last_synced_at,
            pending_count: state.pending_count,
            budgets: state.budgets.clone(),
            now: state.now,
            last_active_today: state.last_active_today.clone(),
        }
    }

    /// Applies `change` to the state, then tells the observer.
    fn update<T>(&self, change: impl FnOnce(&mut State) -> T) -> T {
        let result = {
            let mut state = self.state.lock().unwrap();
            let result = change(&mut state);
            // Under the lock, so revisions advance in the order changes were made.
            self.revision.fetch_add(1, Ordering::SeqCst);
            result
        };
        self.changed();
        result
    }

    fn changed(&self) {
        let observer = self.observer.lock().unwrap().clone();
        if let Some(observer) = observer {
            let snapshot = self.snapshot_of(&self.state.lock().unwrap());
            observer(snapshot);
        }
    }

    fn note_sync_failure(&self, error: &HarvestError) {
        log::error!("sync failed: {error}");
        self.update(|state| {
            state.connection = if *error == HarvestError::Unauthorized {
                Connection::NeedsCredentials
            } else {
                Connection::Offline {
                    reason: error.to_string(),
                }
            }
        });
    }

    /// Everything that describes whichever account was connected a moment ago.
    ///
    /// Shared by `disconnect` and `connect`, because credentials changing means the
    /// same thing in both directions: nothing already on screen is known to describe
    /// the new ones. Clearing only the probe state was worse than clearing none of
    /// it: the panel kept the previous account's budgets, and a transient failure on
    /// the next report left them there, since a budget refresh only overwrites on
    /// success. The cache goes too, or the same stale data returns at the next
    /// launch.
    fn forget_account_data(&self, state: &mut State) {
        self.snapshots.clear();
        state.user = None;
        state.company = None;
        state.targets.clear();
        state.entries.clear();
        state.recent_targets.clear();
        state.frequent_targets.clear();
        state.budgets.clear();
        state.currencies.clear();
        state.currencies_refused = false;
        state.last_active_today = None;
        state.account_data_fetched_at = None;
        state.budget_availability = BudgetAvailability::Unknown;
        state.is_pinned_to_day = false;
        state.last_synced_at = None;
    }

    /// Whether the day has gone unverified long enough, given power and recent
    /// usage, to be worth a request nobody asked for.
    fn is_probe_due(&self) -> bool {
        // Not while one is in flight: syncs queue rather than cancel, so a slow one
        // would bank a queue of ticks to replay back to back on recovery.
        if self.syncs_in_flight.load(Ordering::SeqCst) > 0 {
            return false;
        }
        let power = (self.power.lock().unwrap().clone())();
        let (activity, configured, last_started) = {
            let state = self.state.lock().unwrap();
            let snapshot = self.snapshot_of(&state);
            (
                snapshot.activity(),
                state.connection.is_configured(),
                state.last_sync_started_at,
            )
        };
        if !configured {
            return false;
        }
        let Some(interval) = probe_interval(&activity, power) else {
            return false;
        };
        last_started.is_none_or(|started| seconds_between(started, self.clock.now()) >= interval)
    }

    /// Restores the cache, if it belongs to the login's user. `has_login` says a
    /// login is stored: one whose user is not known yet vouches for no cache.
    fn apply_cached_snapshot(&self, has_login: bool) {
        let Some(snapshot) = self.snapshots.load() else {
            return;
        };
        // A cache written for anyone but the login's user describes someone else's
        // timesheet, however it came to be left behind. With credentials whose user
        // is not known, no cache can be vouched for.
        let vouched = match *self.known_user.lock().unwrap() {
            Some(known) => snapshot.user.as_ref().map(|user| user.id) == Some(known),
            None => !has_login,
        };
        if !vouched {
            log::info!("the cache is not the stored login's; starting without it");
            self.snapshots.clear();
            return;
        }
        self.update(|state| {
            let by_id: HashMap<String, &TimerTarget> = snapshot
                .targets
                .iter()
                .map(|target| (target.id(), target))
                .collect();
            state.recent_targets = snapshot
                .recent_target_ids
                .iter()
                .filter_map(|id| by_id.get(id).map(|target| (*target).clone()))
                .collect();
            state.frequent_targets = snapshot
                .frequent_target_ids
                .iter()
                .filter_map(|id| by_id.get(id).map(|target| (*target).clone()))
                .collect();
            state.user = snapshot.user.clone();
            state.company = snapshot.company.clone();
            state.targets = snapshot.targets.clone();
            let day = state.day;
            state.entries = snapshot
                .entries
                .iter()
                .filter(|entry| entry.spent_date == day || entry.is_running)
                .cloned()
                .collect();
            // Restored so the panel draws a budget immediately, but deliberately
            // without restoring the probe: an empty cache cannot tell "this account
            // has none" from "not asked yet", so the next sync asks again.
            state.budgets = snapshot
                .budgets
                .iter()
                .map(|budget| (budget.project_id, budget.clone()))
                .collect();
            state.last_synced_at = snapshot.saved_at;
        });
    }

    fn save_snapshot(&self) {
        let snapshot = {
            let state = self.state.lock().unwrap();
            CachedSnapshot {
                user: state.user.clone(),
                company: state.company.clone(),
                targets: state.targets.clone(),
                entries: state.entries.clone(),
                recent_target_ids: state.recent_targets.iter().map(TimerTarget::id).collect(),
                frequent_target_ids: state.frequent_targets.iter().map(TimerTarget::id).collect(),
                budgets: state.budgets.values().cloned().collect(),
                saved_at: Some(state.last_synced_at.unwrap_or_else(|| self.clock.now())),
            }
        };
        if let Err(error) = self.snapshots.save(&snapshot) {
            log::error!("could not save the snapshot: {error}");
        }
    }
}

/// A timer started on another day still belongs in the menu bar, so it is folded
/// into the visible day's list even when it is not from that day.
fn merge(day_entries: &[TimeEntry], running: Option<&TimeEntry>) -> Vec<TrackedEntry> {
    let mut tracked: Vec<TrackedEntry> =
        day_entries.iter().map(TrackedEntry::from_harvest).collect();
    if let Some(running) = running
        && !tracked
            .iter()
            .any(|entry| entry.id == EntryId::Server(running.id))
    {
        tracked.insert(0, TrackedEntry::from_harvest(running));
    }
    tracked.sort_by(|lhs, rhs| {
        rhs.is_running
            .cmp(&lhs.is_running)
            .then(rhs.updated_at.cmp(&lhs.updated_at))
    });
    tracked
}

fn mutate_locally(
    state: &mut State,
    id: EntryId,
    now: Timestamp,
    change: impl FnOnce(&mut TrackedEntry),
) {
    let Some(entry) = state.entries.iter_mut().find(|entry| entry.id == id) else {
        return;
    };
    change(entry);
    // A local change is an update, and recency decides what the menu bar offers:
    // without this, stopping a long-running timer offline reads as hours-old
    // activity and the resume button vanishes. The next sync brings Harvest's own
    // timestamp, which says the same thing.
    entry.updated_at = now;
}

fn note_recent(state: &mut State, target: &TimerTarget) {
    state
        .recent_targets
        .retain(|recent| recent.id() != target.id());
    state.recent_targets.insert(0, target.clone());
    state.recent_targets.truncate(MAX_RECENTS);
}

/// Swaps local placeholder ids for the Harvest ids their creates earned, without
/// waiting for the refetch: otherwise a stop in that window targets an id the server
/// has never heard of.
fn adopt(state: &mut State, resolved: &HashMap<Uuid, i64>) {
    if resolved.is_empty() {
        return;
    }
    for entry in &mut state.entries {
        if let EntryId::Local(local) = entry.id
            && let Some(&server_id) = resolved.get(&local)
        {
            entry.id = EntryId::Server(server_id);
            entry.is_pending = false;
        }
    }
    if let Some(recent) = &mut state.last_active_today
        && let EntryId::Local(local) = recent.id
        && let Some(&server_id) = resolved.get(&local)
    {
        recent.id = EntryId::Server(server_id);
    }
}

/// Remembers the most recent thing worked on today, for the menu bar.
fn note_last_activity(state: &mut State, today: CalendarDate) {
    // Only today's running timer is today's activity: a past day's entry resumed on
    // its own day must not evict what the menu bar knows about today, or stopping it
    // strands a running copy nothing can resume.
    if let Some(running) = state.entries.iter().find(|entry| entry.is_running)
        && running.spent_date == today
    {
        state.last_active_today = Some(running.clone());
        return;
    }
    // Judged by the entry's own date, not the visible day: a timer started "on today
    // instead" lives in a past day's list, and stopping it there must still leave it
    // one click away, not strand a stale running copy here.
    let stopped = state
        .entries
        .iter()
        .filter(|entry| !entry.is_running && entry.spent_date == today)
        .max_by_key(|entry| entry.updated_at)
        .cloned();
    if state.day == today {
        // Today's list is authoritative, even about there being nothing.
        state.last_active_today = stopped;
    } else if stopped.is_some() {
        // A past day's list can only add what it happens to know about today.
        state.last_active_today = stopped;
    }
}

/// Drops recents for projects the user is no longer assigned to.
fn prune_recents(state: &mut State) {
    if state.targets.is_empty() {
        return;
    }
    let available: HashSet<String> = state.targets.iter().map(TimerTarget::id).collect();
    state
        .recent_targets
        .retain(|recent| available.contains(&recent.id()));
}

/// Ranks project and task pairs by how much the user really works on them.
///
/// Frequency alone would pin last quarter's project to the top for weeks after it
/// ended; recency alone would let one stray entry outrank a daily habit. Each entry
/// contributes a weight that falls off with its age, so the ordering follows what
/// someone is working on now without forgetting the whole quarter.
pub fn rank_by_usage(entries: &[TimeEntry], today: CalendarDate) -> Vec<TimerTarget> {
    let mut scores: HashMap<String, f64> = HashMap::new();
    let mut targets: HashMap<String, TimerTarget> = HashMap::new();
    for entry in entries {
        let target = TimerTarget {
            project: entry.project.clone(),
            task: entry.task.clone(),
            client: entry.client.clone(),
        };
        let days_ago = f64::from(today.days_since(entry.spent_date).max(0));
        *scores.entry(target.id()).or_default() += (-days_ago / HALF_LIFE_DAYS).exp();
        targets.insert(target.id(), target);
    }
    let mut ranked: Vec<(String, f64)> = scores.into_iter().collect();
    // Ties sort by id so the order never shuffles between syncs.
    ranked.sort_by(|(lhs_id, lhs), (rhs_id, rhs)| {
        rhs.partial_cmp(lhs)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| lhs_id.cmp(rhs_id))
    });
    ranked
        .into_iter()
        .filter_map(|(id, _)| targets.remove(&id))
        .collect()
}

#[cfg(test)]
mod tests;
