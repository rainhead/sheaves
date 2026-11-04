//! Records as Harvest returns them.
//!
//! Field names follow Harvest's snake_case JSON, so these types decode the API's
//! responses and also round-trip through the snapshot file unchanged.

use jiff::Timestamp;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::calendar_date::{CalendarDate, Weekday};

/// The `{ "id": …, "name": … }` stub Harvest embeds wherever it references another
/// record.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, uniffi::Record)]
pub struct Reference {
    pub id: i64,
    pub name: String,
}

impl Reference {
    pub fn new(id: i64, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProjectReference {
    pub id: i64,
    pub name: String,
    /// Short project code, often empty.
    pub code: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, uniffi::Record)]
pub struct HarvestUser {
    pub id: i64,
    pub first_name: String,
    pub last_name: String,
    pub email: String,
}

impl HarvestUser {
    pub fn name(&self) -> String {
        format!("{} {}", self.first_name, self.last_name)
            .trim()
            .to_string()
    }
}

/// Account-wide settings. `wants_timestamp_timers` decides whether this account
/// records durations or start and end times, which changes how a timer is created
/// and stopped.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, uniffi::Record)]
pub struct HarvestCompany {
    pub name: String,
    pub full_domain: String,
    pub is_active: bool,
    pub week_start_day: Weekday,
    pub wants_timestamp_timers: bool,
    /// `decimal` or `hours_minutes`.
    pub time_format: String,
    /// Weekly capacity, in seconds.
    pub weekly_capacity: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TimeEntry {
    pub id: i64,
    pub spent_date: CalendarDate,
    pub user: Option<Reference>,
    pub client: Option<Reference>,
    pub project: Reference,
    pub task: Reference,
    /// Total hours recorded, as of when this entry was fetched.
    pub hours: f64,
    /// Hours excluding the currently running timer. Use `elapsed_hours` for a live
    /// total.
    pub hours_without_timer: Option<f64>,
    pub notes: Option<String>,
    pub is_locked: bool,
    pub locked_reason: Option<String>,
    pub is_billed: bool,
    pub is_running: bool,
    pub billable: bool,
    /// When the running timer was started. `None` unless `is_running`.
    pub timer_started_at: Option<Timestamp>,
    /// Wall-clock start and end, on `wants_timestamp_timers` accounts, formatted per
    /// the account's settings.
    pub started_time: Option<String>,
    pub ended_time: Option<String>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    /// A link to something outside Harvest that this entry belongs to. Sheaves
    /// keeps when each machine saw the user here, on the running entry; see
    /// [`crate::peers::board`].
    #[serde(default)]
    pub external_reference: Option<ExternalReference>,
}

/// Harvest's `external_reference`: meant for an integration's link to an issue or
/// a card, and the only place on an entry that holds text other than its notes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalReference {
    pub id: Option<String>,
    pub group_id: Option<String>,
    /// Must be an http(s) URL, or Harvest stores nothing.
    pub permalink: Option<String>,
}

impl TimeEntry {
    /// Hours on the clock at `now`: a running timer keeps counting between fetches.
    pub fn elapsed_hours(&self, now: Timestamp) -> f64 {
        match (self.is_running, self.timer_started_at) {
            (true, Some(started)) => {
                self.hours_without_timer.unwrap_or(self.hours) + hours_between(started, now)
            }
            _ => self.hours,
        }
    }

    /// False when Harvest will reject edits: approved, invoiced, or in a closed
    /// timesheet.
    pub fn is_editable(&self) -> bool {
        !self.is_locked
    }
}

/// Hours from `from` to `to`, never negative, so a clock that steps backwards
/// cannot take time off an entry.
pub(crate) fn hours_between(from: Timestamp, to: Timestamp) -> f64 {
    seconds_between(from, to).max(0.0) / 3600.0
}

/// Signed seconds from `from` to `to`.
pub(crate) fn seconds_between(from: Timestamp, to: Timestamp) -> f64 {
    (to.as_nanosecond() - from.as_nanosecond()) as f64 / 1e9
}

/// `instant` moved by `seconds`, which may be fractional or negative.
pub(crate) fn offset_by(instant: Timestamp, seconds: f64) -> Timestamp {
    let nanos = instant.as_nanosecond() + (seconds * 1e9).round() as i128;
    Timestamp::from_nanosecond(nanos).expect("within jiff's range")
}

/// A Harvest client, fetched for one field: `currency`.
///
/// Harvest reports a budget's currency nowhere a budget appears: not on the project
/// budget report, not on `company`, and not on the client stub embedded in a project
/// assignment. It exists only here.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ClientRecord {
    pub id: i64,
    pub name: String,
    #[serde(default = "yes")]
    pub is_active: bool,
    /// An ISO 4217 code such as "USD". Optional because Harvest omits it for a
    /// client left on the company default.
    pub currency: Option<String>,
}

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProjectAssignment {
    pub id: i64,
    pub is_active: bool,
    pub project: ProjectReference,
    pub client: Reference,
    pub task_assignments: Vec<TaskAssignment>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TaskAssignment {
    pub id: i64,
    pub billable: bool,
    pub is_active: bool,
    pub task: Reference,
}

// MARK: Pagination

/// A resource Harvest returns in a paginated envelope keyed by `PAGE_KEY`.
pub trait PaginatedItem: DeserializeOwned + Send + 'static {
    const PAGE_KEY: &'static str;
}

impl PaginatedItem for TimeEntry {
    const PAGE_KEY: &'static str = "time_entries";
}

impl PaginatedItem for ProjectAssignment {
    const PAGE_KEY: &'static str = "project_assignments";
}

impl PaginatedItem for ClientRecord {
    const PAGE_KEY: &'static str = "clients";
}

/// One page of a paginated response.
#[derive(Debug)]
pub struct Page<Item> {
    pub items: Vec<Item>,
    pub page: i64,
    pub total_pages: i64,
    pub total_entries: i64,
    pub next_page: Option<i64>,
    /// Harvest's own URL for the next page.
    ///
    /// Preferred over `next_page`, and the reason this exists: on a cursor-paginated
    /// response Harvest returns null for `page`, `next_page` and `previous_page` on
    /// every page but the first and last, so walking page numbers stops after one
    /// page and silently loses the rest. Harvest's documentation says to follow
    /// these links rather than build pagination URLs.
    pub next_link: Option<url::Url>,
}

#[derive(Deserialize)]
struct Envelope {
    page: Option<i64>,
    total_pages: Option<i64>,
    total_entries: Option<i64>,
    next_page: Option<i64>,
    links: Option<Links>,
}

#[derive(Deserialize)]
struct Links {
    next: Option<String>,
}

impl<Item: PaginatedItem> Page<Item> {
    /// Decodes a page, naming the field that failed, such as
    /// `time_entries[0].spent_date`, when one does.
    pub(crate) fn decode(data: &[u8]) -> Result<Self, String> {
        let mut object: serde_json::Map<String, serde_json::Value> = decode_described(data)?;
        let items = object
            .remove(Item::PAGE_KEY)
            .ok_or_else(|| format!("missing field ‘{}’ at the top level", Item::PAGE_KEY))?;
        let items: Vec<Item> = serde_path_to_error::deserialize(items).map_err(|error| {
            describe(
                error.path().to_string(),
                error.inner(),
                Some(Item::PAGE_KEY),
            )
        })?;
        let envelope: Envelope = serde_json::from_value(serde_json::Value::Object(object))
            .map_err(|error| error.to_string())?;
        Ok(Page {
            page: envelope.page.unwrap_or(1),
            total_pages: envelope.total_pages.unwrap_or(1),
            total_entries: envelope.total_entries.unwrap_or(items.len() as i64),
            next_page: envelope.next_page,
            next_link: envelope
                .links
                .and_then(|links| links.next)
                .and_then(|next| url::Url::parse(&next).ok()),
            items,
        })
    }
}

/// Decodes `data`, describing a failure by the path to the field that caused it.
/// serde's own message omits the path, which is the useful part.
pub(crate) fn decode_described<T: DeserializeOwned>(data: &[u8]) -> Result<T, String> {
    let mut deserializer = serde_json::Deserializer::from_slice(data);
    serde_path_to_error::deserialize(&mut deserializer)
        .map_err(|error| describe(error.path().to_string(), error.inner(), None))
}

fn describe(path: String, error: &serde_json::Error, prefix: Option<&str>) -> String {
    let path = match (prefix, path.as_str()) {
        (Some(prefix), ".") => prefix.to_string(),
        (Some(prefix), path) if path.starts_with('[') => format!("{prefix}{path}"),
        (Some(prefix), path) => format!("{prefix}.{path}"),
        (None, ".") => "the top level".to_string(),
        (None, path) => path.to_string(),
    };
    // serde_json appends " at line L column C", which says nothing to a reader.
    let message = error.to_string();
    let message = message
        .split(" at line ")
        .next()
        .unwrap_or(&message)
        .to_string();
    format!("{message} at {path}")
}
