//! What the Mac app sees of the core, through UniFFI.
//!
//! The core's own types cross as they are. This module adds what only a foreign
//! caller needs: the conversions for types UniFFI does not know, an object that runs
//! the tracker on a runtime of its own, and errors in a shape Swift can read.

use std::time::SystemTime;

use jiff::Timestamp;
use uuid::Uuid;

uniffi::custom_type!(Timestamp, SystemTime, {
    remote,
    lower: |instant| SystemTime::from(instant),
    try_lift: |time| Ok(Timestamp::try_from(time)?),
});

uniffi::custom_type!(Uuid, String, {
    remote,
    lower: |id| id.to_string(),
    try_lift: |text| Ok(Uuid::parse_str(&text)?),
});

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::absence::{
    Absence, AbsenceDetector, AbsenceResolution, PresenceMonitor, PresenceReading,
};
use crate::budget::{BudgetFigure, ProjectBudget};
use crate::calendar_date::{CalendarDate, Weekday};
use crate::client::{Credentials, HarvestClient, HttpTransport, Transport};
use crate::clock::{Clock, SystemClock};
use crate::credentials::CredentialStore;
use crate::demo::DemoTransport;
use crate::hours::{HoursEdit, HoursFormat, parse_hours};
use crate::models::{HarvestUser, Reference};
use crate::queue::MutationQueue;
use crate::snapshot::SnapshotStore;
use crate::tracked_entry::{EntryId, ProjectTargets, TimerTarget, TrackedEntry};
use crate::tracker::{Activity, ConnectError, Connection, PowerState, TimeTracker, TrackerState};

// MARK: Methods on records
//
// Named apart from the core's own methods, which Rust callers use and some of which
// take types that do not cross.

#[uniffi::export]
impl TrackedEntry {
    /// Hours on the clock at `now`.
    pub fn hours_at(&self, now: Timestamp) -> f64 {
        self.hours(now)
    }

    pub fn timer_target(&self) -> TimerTarget {
        self.target()
    }
}

#[uniffi::export]
impl EntryId {
    /// Harvest's id, or `None` for an entry Harvest has not heard of yet.
    pub fn harvest_id(&self) -> Option<i64> {
        self.server_id()
    }
}

#[uniffi::export]
impl HarvestUser {
    pub fn display_name(&self) -> String {
        self.name()
    }
}

#[uniffi::export]
impl Connection {
    pub fn has_credentials(&self) -> bool {
        self.is_configured()
    }
}

#[uniffi::export]
impl TimerTarget {
    /// `project:task`, unique per pair.
    pub fn key(&self) -> String {
        self.id()
    }

    /// "Client · Project".
    pub fn label(&self) -> String {
        self.project_label()
    }
}

#[uniffi::export]
impl ProjectTargets {
    pub fn with_task(&self, task: Reference) -> TimerTarget {
        self.target(task)
    }
}

#[uniffi::export]
impl ProjectBudget {
    pub fn is_readable(&self) -> bool {
        self.has_readable_budget()
    }

    pub fn is_overrun(&self) -> bool {
        self.is_over_budget()
    }

    pub fn used(&self) -> f64 {
        self.fraction_used()
    }

    pub fn remaining_figure(&self) -> Option<BudgetFigure> {
        self.remaining()
    }

    pub fn spent_figure(&self) -> Option<BudgetFigure> {
        self.spent()
    }

    pub fn total_figure(&self) -> Option<BudgetFigure> {
        self.total()
    }
}

#[uniffi::export]
impl Absence {
    /// Seconds away.
    pub fn length(&self) -> f64 {
        self.duration()
    }

    /// See [`Absence::trimmed_hours`].
    pub fn trim_for(&self, entry: &TrackedEntry) -> Option<f64> {
        self.trimmed_hours(entry)
    }

    /// The day the absence happened on, in this machine's time zone.
    pub fn local_day(&self) -> CalendarDate {
        self.day(&jiff::tz::TimeZone::system())
    }
}

#[uniffi::export]
impl CalendarDate {
    /// The ISO 8601 text Harvest reads, such as `2026-08-29`.
    pub fn iso(&self) -> String {
        self.to_string()
    }

    pub fn is_before(&self, other: CalendarDate) -> bool {
        *self < other
    }

    pub fn plus_days(&self, days: i32) -> CalendarDate {
        self.adding_days(days)
    }

    pub fn days_after(&self, other: CalendarDate) -> i32 {
        self.days_since(other)
    }

    pub fn first_of_week(&self, week_start: Weekday) -> CalendarDate {
        self.start_of_week(week_start)
    }

    /// The first instant of this day in this machine's time zone.
    pub fn local_start(&self) -> Timestamp {
        self.start_of_day(&jiff::tz::TimeZone::system())
    }
}

/// Today, in this machine's time zone.
#[uniffi::export]
pub fn calendar_today() -> CalendarDate {
    SystemClock.today()
}

/// The day `instant` falls on in this machine's time zone.
#[uniffi::export]
pub fn calendar_day_of(instant: Timestamp) -> CalendarDate {
    CalendarDate::of(instant, &jiff::tz::TimeZone::system())
}

#[uniffi::export]
pub fn hours_format_of(time_format: Option<String>) -> HoursFormat {
    HoursFormat::from_time_format(time_format.as_deref())
}

/// See [`parse_hours`]. `decimal_separator` is the locale's, and only its first
/// character counts.
#[uniffi::export]
pub fn parse_duration(text: String, decimal_separator: String) -> Option<f64> {
    parse_hours(&text, decimal_separator.chars().next().unwrap_or('.'))
}

/// See [`HoursEdit::commit`].
#[uniffi::export]
pub fn hours_edit_commit(
    draft: String,
    as_opened: String,
    banked: f64,
    is_running: bool,
    decimal_separator: String,
) -> Option<f64> {
    HoursEdit::commit(
        &draft,
        &as_opened,
        banked,
        is_running,
        decimal_separator.chars().next().unwrap_or('.'),
    )
}

#[uniffi::export]
pub fn default_absence_threshold() -> f64 {
    AbsenceDetector::DEFAULT_THRESHOLD
}

#[uniffi::export]
pub fn presence_poll_interval() -> f64 {
    PresenceMonitor::POLL_INTERVAL
}

// MARK: Presence

/// [`PresenceMonitor`] for a shell, which keeps the sensors and the timer and hands
/// each reading here.
#[derive(uniffi::Object)]
pub struct PresenceFold {
    monitor: Mutex<PresenceMonitor>,
}

#[uniffi::export]
impl PresenceFold {
    #[uniffi::constructor]
    pub fn new(threshold: f64, now: Timestamp) -> Arc<Self> {
        Arc::new(Self {
            monitor: Mutex::new(PresenceMonitor::new(threshold, now)),
        })
    }

    /// Folds one reading taken at `now`, returning any absence it ended.
    pub fn poll(&self, reading: PresenceReading, now: Timestamp) -> Vec<Absence> {
        self.monitor.lock().unwrap().poll(reading, now)
    }
}

// MARK: The tracker

/// Everything a shell draws, with the values derived from it worked out here so no
/// shell reimplements them.
#[derive(Clone, Debug, uniffi::Record)]
pub struct TrackerSnapshot {
    pub state: TrackerState,
    pub running_entry: Option<TrackedEntry>,
    pub activity: Activity,
    pub total_hours: f64,
    pub is_today: bool,
    pub ordered_projects: Vec<ProjectTargets>,
}

impl From<TrackerState> for TrackerSnapshot {
    fn from(state: TrackerState) -> Self {
        Self {
            running_entry: state.running_entry().cloned(),
            activity: state.activity(),
            total_hours: state.total_hours(),
            is_today: state.is_today(),
            ordered_projects: state.ordered_projects(),
            state,
        }
    }
}

/// Told about every change to the tracker's state. Called on a background thread.
#[uniffi::export(with_foreign)]
pub trait TrackerObserver: Send + Sync {
    fn state_changed(&self, snapshot: TrackerSnapshot);
}

/// How the machine is powered, asked each time the probe considers a sync.
#[uniffi::export(with_foreign)]
pub trait PowerProbe: Send + Sync {
    fn power_state(&self) -> PowerState;
}

/// Why `connect` did not connect, in the shape Swift reads.
#[derive(Clone, Debug, PartialEq, thiserror::Error, uniffi::Error)]
pub enum ConnectFailure {
    /// Changes made under a different account or user are still queued. See
    /// [`ConnectError::UnsentChangesBelongElsewhere`].
    #[error("{count} unsent change{} made under a different Harvest account.", if *.count == 1 { " was" } else { "s were" })]
    UnsentChangesBelongElsewhere { count: u32 },
    /// Anything else, described for a person.
    #[error("{message}")]
    Failed { message: String },
}

impl From<ConnectError> for ConnectFailure {
    fn from(error: ConnectError) -> Self {
        match error {
            ConnectError::UnsentChangesBelongElsewhere { count } => {
                ConnectFailure::UnsentChangesBelongElsewhere {
                    count: count as u32,
                }
            }
            other => ConnectFailure::Failed {
                message: other.to_string(),
            },
        }
    }
}

/// The tracker as a shell holds it.
///
/// It runs on a runtime of its own, and every call is spawned onto it, so a call
/// the shell abandons still finishes here rather than stopping part way.
#[derive(uniffi::Object)]
pub struct Tracker {
    runtime: tokio::runtime::Runtime,
    tracker: TimeTracker,
}

impl Tracker {
    /// Runs `work` on the tracker's runtime and waits for it.
    async fn run<T, F>(&self, work: impl FnOnce(TimeTracker) -> F) -> T
    where
        F: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        let future = work(self.tracker.clone());
        self.runtime
            .spawn(future)
            .await
            .expect("tracker calls do not panic")
    }
}

#[uniffi::export]
impl Tracker {
    /// A tracker keeping its files in `data_directory`, which it creates.
    ///
    /// With `demo`, it talks to the made-up account in [`crate::demo`] instead of
    /// Harvest, and keeps its files in a temporary folder of its own.
    #[uniffi::constructor]
    pub fn new(
        data_directory: String,
        demo: bool,
        credentials: Arc<dyn CredentialStore>,
        observer: Arc<dyn TrackerObserver>,
    ) -> Arc<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("sheaves-core")
            .enable_all()
            .build()
            .expect("a runtime can start");
        let directory = if demo {
            std::env::temp_dir().join(format!("sheaves-demo-{}", Uuid::new_v4()))
        } else {
            PathBuf::from(data_directory)
        };
        let (transport, installed): (Arc<dyn Transport>, _) = if demo {
            (Arc::new(DemoTransport), Some(Credentials::new("0", "demo")))
        } else {
            (Arc::new(HttpTransport::new()), None)
        };
        let clock: Arc<dyn Clock> = Arc::new(SystemClock);
        let tracker = TimeTracker::new(
            HarvestClient::new(installed, transport, 1.0),
            credentials,
            // Named apart from the Swift core's files in the same folder, which this
            // never reads or overwrites.
            SnapshotStore::new(directory.join("cache.json")),
            MutationQueue::open(directory.join("queue.json"), clock.clone()),
            clock,
        );
        tracker.observe(move |state| observer.state_changed(state.into()));
        Arc::new(Self { runtime, tracker })
    }

    pub fn snapshot(&self) -> TrackerSnapshot {
        self.tracker.state().into()
    }

    pub fn set_power_probe(&self, probe: Arc<dyn PowerProbe>) {
        self.tracker.set_power_source(move || probe.power_state());
    }

    pub async fn bootstrap(&self) {
        self.run(|tracker| async move { tracker.bootstrap().await })
            .await
    }

    pub async fn connect(
        &self,
        credentials: Credentials,
        discarding_unsent_changes: bool,
    ) -> Result<(), ConnectFailure> {
        self.run(move |tracker| async move {
            tracker
                .connect(credentials, discarding_unsent_changes)
                .await
                .map_err(ConnectFailure::from)
        })
        .await
    }

    pub async fn disconnect(&self) {
        self.run(|tracker| async move { tracker.disconnect().await })
            .await
    }

    pub async fn sync(&self) {
        self.run(|tracker| async move { tracker.sync().await })
            .await
    }

    pub async fn select_day(&self, day: CalendarDate) {
        self.run(move |tracker| async move { tracker.select_day(day).await })
            .await
    }

    pub async fn go_to_today(&self) {
        self.run(|tracker| async move { tracker.go_to_today().await })
            .await
    }

    pub async fn shift_day(&self, days: i32) {
        self.run(move |tracker| async move { tracker.shift_day(days).await })
            .await
    }

    pub async fn start(&self, target: TimerTarget, notes: Option<String>) {
        self.run(move |tracker| async move { tracker.start(&target, notes.as_deref()).await })
            .await
    }

    pub async fn resume(&self, entry: TrackedEntry) {
        self.run(move |tracker| async move { tracker.resume(&entry).await })
            .await
    }

    pub async fn stop(&self, entry: TrackedEntry) {
        self.run(move |tracker| async move { tracker.stop(&entry).await })
            .await
    }

    pub async fn toggle(&self, entry: TrackedEntry) {
        self.run(move |tracker| async move { tracker.toggle(&entry).await })
            .await
    }

    pub async fn stop_running(&self) {
        self.run(|tracker| async move { tracker.stop_running().await })
            .await
    }

    pub async fn update_notes(&self, entry: TrackedEntry, notes: String) {
        self.run(move |tracker| async move { tracker.update_notes(&entry, &notes).await })
            .await
    }

    pub async fn update_hours(&self, entry: TrackedEntry, hours: f64) {
        self.run(move |tracker| async move { tracker.update_hours(&entry, hours).await })
            .await
    }

    pub async fn delete(&self, entry: TrackedEntry) {
        self.run(move |tracker| async move { tracker.delete(&entry).await })
            .await
    }

    pub async fn resolve(
        &self,
        absence: Absence,
        entry: TrackedEntry,
        resolution: AbsenceResolution,
    ) {
        self.run(move |tracker| async move { tracker.resolve(&absence, &entry, &resolution).await })
            .await
    }
}

// MARK: Logging

#[derive(Clone, Copy, Debug, uniffi::Enum)]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
}

/// Where the core's log lines go: the unified log on a Mac, the journal on GNOME.
#[uniffi::export(with_foreign)]
pub trait LogSink: Send + Sync {
    fn write(&self, level: LogLevel, target: String, message: String);
}

struct ForeignLogger(Arc<dyn LogSink>);

impl log::Log for ForeignLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &log::Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let level = match record.level() {
            log::Level::Error => LogLevel::Error,
            log::Level::Warn => LogLevel::Warn,
            log::Level::Info => LogLevel::Info,
            log::Level::Debug | log::Level::Trace => LogLevel::Debug,
        };
        self.0.write(
            level,
            record.target().to_string(),
            record.args().to_string(),
        );
    }

    fn flush(&self) {}
}

/// Sends the core's log lines to `sink`. Only the first call takes effect.
#[uniffi::export]
pub fn install_log_sink(sink: Arc<dyn LogSink>) {
    if log::set_boxed_logger(Box::new(ForeignLogger(sink))).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
}
