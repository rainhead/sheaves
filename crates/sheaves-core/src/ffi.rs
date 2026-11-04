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
use std::sync::Arc;

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
use crate::peers::{Peers, Presence, machine_id_in};
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
impl Activity {
    /// The entry the menu bar is about, running or recently stopped.
    pub fn current_entry(&self) -> Option<TrackedEntry> {
        self.entry().cloned()
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

    /// "Client · Project".
    pub fn project_label(&self) -> String {
        self.label()
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
impl TrackerState {
    /// See [`TrackerState::absence_applies`].
    pub fn asks_about(&self, absence: Absence) -> bool {
        self.absence_applies(&absence)
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

/// [`Presence`] for a shell, which keeps the sensors and the timer and hands each
/// reading here.
#[derive(uniffi::Object)]
pub struct PresenceFold {
    presence: Arc<Presence>,
    /// Where reading the other machines' presence runs, which needs a Tokio
    /// runtime. A fold sharing nothing never reads, and runs wherever it is called.
    runtime: Option<tokio::runtime::Handle>,
}

#[uniffi::export]
impl PresenceFold {
    /// A fold sharing nothing, deciding alone.
    #[uniffi::constructor]
    pub fn new(threshold: f64, now: Timestamp) -> Arc<Self> {
        let peers = Arc::new(Peers::new(threshold, "alone"));
        Arc::new(Self {
            presence: Arc::new(Presence::new(threshold, now, peers)),
            runtime: None,
        })
    }

    /// Folds one reading taken at `now`, returning the absences it ended that no
    /// peer saw the user during.
    pub async fn poll(&self, reading: PresenceReading, now: Timestamp) -> Vec<Absence> {
        let presence = self.presence.clone();
        match &self.runtime {
            Some(runtime) => runtime
                .spawn(async move { presence.poll(reading, now).await })
                .await
                .expect("presence polls do not panic"),
            None => presence.poll(reading, now).await,
        }
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
    /// Something the user should know that is not a failure of the last sync, for
    /// the panel to show until it no longer applies.
    pub notice: Option<String>,
}

impl TrackerSnapshot {
    fn of(state: TrackerState, notice: Option<String>) -> Self {
        Self {
            running_entry: state.running_entry().cloned(),
            activity: state.activity(),
            total_hours: state.total_hours(),
            is_today: state.is_today(),
            ordered_projects: state.ordered_projects(),
            notice,
            state,
        }
    }
}

/// How many changes the Swift core left unsent in `pending-mutations.json`, which
/// the Rust core does not read. Zero when the file is missing or empty.
///
/// Read only to warn: the Swift core's encoding is its own, and translating it
/// would be guesswork with someone's hours. The previous version can still send
/// them.
fn swift_core_unsent(directory: &std::path::Path) -> usize {
    let Ok(data) = std::fs::read(directory.join("pending-mutations.json")) else {
        return 0;
    };
    match serde_json::from_slice::<serde_json::Value>(&data) {
        Ok(serde_json::Value::Array(pending)) => pending.len(),
        Ok(value) => value
            .get("pending")
            .and_then(serde_json::Value::as_array)
            .map_or(0, Vec::len),
        Err(_) => 0,
    }
}

fn swift_core_notice(unsent: usize) -> Option<String> {
    (unsent > 0).then(|| {
        let (changes, were) = if unsent == 1 {
            ("change".to_string(), "was")
        } else {
            (format!("{unsent} changes"), "were")
        };
        let count = if unsent == 1 { "1 ".to_string() } else { String::new() };
        format!(
            "{count}{changes} made in an earlier version of Sheaves {were} never sent to Harvest. Open that version until it syncs, then come back to this one."
        )
    })
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
    notice: Option<String>,
    peers: Arc<Peers>,
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
        let notice = if demo {
            None
        } else {
            swift_core_notice(swift_core_unsent(&directory))
        };
        let observed = notice.clone();
        tracker.observe(move |state| {
            observer.state_changed(TrackerSnapshot::of(state, observed.clone()))
        });
        let peers = Arc::new(Peers::new(
            AbsenceDetector::DEFAULT_THRESHOLD,
            machine_id_in(&directory.join("machine.json")),
        ));
        // The demo account is nobody's, and has no running entry to share on.
        if !demo {
            peers.keep_log_in(directory.join("presence.json"), Timestamp::now());
            tracker.join(peers.clone());
        }
        Arc::new(Self {
            runtime,
            tracker,
            notice,
            peers,
        })
    }

    /// A presence fold that shares what it sees through this tracker's running
    /// entry, and reads what the other machines saw before it reports an absence.
    pub fn presence(&self, threshold: f64, now: Timestamp) -> Arc<PresenceFold> {
        Arc::new(PresenceFold {
            presence: Arc::new(Presence::new(threshold, now, self.peers.clone())),
            runtime: Some(self.runtime.handle().clone()),
        })
    }

    pub fn snapshot(&self) -> TrackerSnapshot {
        TrackerSnapshot::of(self.tracker.state(), self.notice.clone())
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

    pub async fn retarget(&self, entry: TrackedEntry, target: TimerTarget) {
        self.run(move |tracker| async move { tracker.retarget(&entry, &target).await })
            .await
    }

    pub async fn log_time(
        &self,
        target: TimerTarget,
        day: CalendarDate,
        hours: f64,
        notes: Option<String>,
    ) {
        self.run(move |tracker| async move {
            tracker
                .log_time(&target, day, hours, notes.as_deref())
                .await
        })
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

#[cfg(test)]
mod tests {
    use super::*;

    mod presence_fold {
        use super::*;

        /// A reading no sensor could honestly give must neither panic nor invent an
        /// absence, and the fold must keep working after it.
        #[tokio::test]
        async fn survives_a_nonsense_reading() {
            let start = Timestamp::from_second(1_700_000_000).unwrap();
            let fold = PresenceFold::new(15.0 * 60.0, start);
            for idle in [f64::INFINITY, f64::NAN, 1e300, -1e300] {
                let reading = PresenceReading {
                    seconds_since_input: idle,
                    ..PresenceReading::default()
                };
                assert!(fold.poll(reading, start).await.is_empty(), "{idle}");
            }
            let later = crate::models::offset_by(start, 3600.0);
            let back = PresenceReading {
                seconds_since_input: 1.0,
                ..PresenceReading::default()
            };
            assert_eq!(fold.poll(back, later).await.len(), 1);
        }
    }

    mod swift_core_queue {
        use super::*;

        #[test]
        fn counts_what_the_swift_core_left_unsent() {
            let directory = tempfile::tempdir().unwrap();
            assert_eq!(swift_core_unsent(directory.path()), 0);

            let file = directory.path().join("pending-mutations.json");
            std::fs::write(&file, r#"{"owner":null,"pending":[],"resolved":[]}"#).unwrap();
            assert_eq!(swift_core_unsent(directory.path()), 0);
            std::fs::write(
                &file,
                r#"{"pending":[{"stop":{}},{"delete":{}}],"resolved":[]}"#,
            )
            .unwrap();
            assert_eq!(swift_core_unsent(directory.path()), 2);
            // The shape from before resolutions were recorded.
            std::fs::write(&file, r#"[{"stop":{}}]"#).unwrap();
            assert_eq!(swift_core_unsent(directory.path()), 1);
        }

        #[test]
        fn says_what_to_do_about_them() {
            assert_eq!(swift_core_notice(0), None);
            let one = swift_core_notice(1).unwrap();
            assert!(one.starts_with("1 change made"), "{one}");
            assert!(swift_core_notice(3).unwrap().starts_with("3 changes made"));
        }
    }
}
