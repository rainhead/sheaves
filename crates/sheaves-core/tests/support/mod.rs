//! A simulated Harvest that several trackers can share, as two machines share the
//! real one.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use jiff::Timestamp;
use serde_json::{Value, json};

use sheaves_core::client::{HarvestError, Method, Request, Response, Transport};
use sheaves_core::clock::{Clock, ManualClock};
use sheaves_core::models::Reference;
use sheaves_core::tracked_entry::TimerTarget;

pub const USER_ID: i64 = 1782959;

/// The one project and task the user works on.
pub fn target() -> TimerTarget {
    TimerTarget {
        project: Reference::new(101, "Online Store"),
        task: Reference::new(201, "Programming"),
        client: Some(Reference::new(1, "123 Industries")),
    }
}

pub fn hours_between(from: Timestamp, to: Timestamp) -> f64 {
    ((to.as_nanosecond() - from.as_nanosecond()) as f64 / 1e9).max(0.0) / 3600.0
}

fn round2(hours: f64) -> f64 {
    (hours * 100.0).round() / 100.0
}

#[derive(Clone, Debug)]
pub struct ServerEntry {
    pub id: i64,
    pub spent_date: String,
    pub notes: Option<String>,
    /// Hours banked, excluding the running stretch.
    pub banked: f64,
    /// When the server started counting.
    pub running_since: Option<Timestamp>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    /// The entry's `external_reference`, as written.
    pub external_reference: Option<Value>,
}

impl ServerEntry {
    pub fn hours(&self, now: Timestamp) -> f64 {
        self.banked
            + self
                .running_since
                .map_or(0.0, |since| hours_between(since, now))
    }

    fn json(&self, now: Timestamp) -> Value {
        json!({
            "id": self.id, "spent_date": self.spent_date,
            "user": { "id": USER_ID, "name": "Kim Allen" },
            "client": { "id": 1, "name": "123 Industries" },
            "project": { "id": 101, "name": "Online Store" },
            "task": { "id": 201, "name": "Programming" },
            "hours": round2(self.hours(now)),
            "hours_without_timer": round2(self.banked),
            "notes": self.notes,
            "is_locked": false, "locked_reason": null, "is_billed": false,
            "is_running": self.running_since.is_some(), "billable": true,
            "timer_started_at": self.running_since.map(|since| since.to_string()),
            "started_time": null, "ended_time": null,
            "created_at": self.created_at.to_string(),
            "updated_at": self.updated_at.to_string(),
            "external_reference": self.external_reference,
        })
    }
}

pub struct FakeHarvest {
    clock: Arc<ManualClock>,
    pub entries: Mutex<Vec<ServerEntry>>,
    next_id: Mutex<i64>,
    /// How many writes of an external reference have arrived.
    reference_writes: Mutex<usize>,
    /// How many times the running timer was asked for.
    running_reads: Mutex<usize>,
}

impl FakeHarvest {
    pub fn new(clock: Arc<ManualClock>) -> Arc<Self> {
        Arc::new(Self {
            clock,
            entries: Mutex::new(Vec::new()),
            next_id: Mutex::new(5000),
            reference_writes: Mutex::new(0),
            running_reads: Mutex::new(0),
        })
    }

    /// Links the only entry elsewhere, as another integration would.
    pub fn set_reference(&self, reference: Value) {
        let mut entries = self.entries.lock().unwrap();
        assert_eq!(entries.len(), 1, "{entries:?}");
        entries[0].external_reference = Some(reference);
    }

    pub fn reference_writes(&self) -> usize {
        *self.reference_writes.lock().unwrap()
    }

    pub fn running_reads(&self) -> usize {
        *self.running_reads.lock().unwrap()
    }

    pub fn only_entry(&self) -> ServerEntry {
        let entries = self.entries.lock().unwrap();
        assert_eq!(entries.len(), 1, "{entries:?}");
        entries[0].clone()
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
                let assignment = json!({
                    "id": 7101, "is_active": true,
                    "project": { "id": 101, "name": "Online Store", "code": "" },
                    "client": { "id": 1, "name": "123 Industries" },
                    "task_assignments": [{ "id": 90201, "billable": true, "is_active": true,
                                           "task": { "id": 201, "name": "Programming" } }]
                });
                (200, Self::page(vec![assignment], "project_assignments"))
            }
            (Method::Get, ["time_entries"]) => {
                let query: HashMap<String, String> =
                    request.url.query_pairs().into_owned().collect();
                let running_only = query.get("is_running").is_some_and(|value| value == "true");
                if running_only {
                    *self.running_reads.lock().unwrap() += 1;
                }
                let from = query.get("from").cloned().unwrap_or_default();
                let to = query
                    .get("to")
                    .cloned()
                    .unwrap_or_else(|| "9999".to_string());
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
            (Method::Get, ["time_entries", id]) => {
                let id: i64 = id.parse().unwrap();
                match entries.iter().find(|entry| entry.id == id) {
                    Some(entry) => (200, entry.json(now)),
                    None => (404, json!({})),
                }
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
                    spent_date: body["spent_date"].as_str().unwrap().to_string(),
                    notes: body
                        .get("notes")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    banked: hours.unwrap_or(0.0),
                    running_since: hours.is_none().then_some(now),
                    created_at: now,
                    updated_at: now,
                    external_reference: None,
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
                if entries
                    .iter()
                    .any(|entry| entry.id == id && entry.running_since.is_some())
                {
                    return (
                        422,
                        json!({ "message": "This time entry is already running." }),
                    );
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
                if let Some(reference) = body.get("external_reference") {
                    *self.reference_writes.lock().unwrap() += 1;
                    // As on a real account: without an http(s) permalink, nothing
                    // is kept.
                    let linked = reference
                        .get("permalink")
                        .and_then(Value::as_str)
                        .is_some_and(|link| link.starts_with("https://"));
                    entry.external_reference = linked.then(|| reference.clone());
                }
                entry.updated_at = now;
                (200, entry.json(now))
            }
            (Method::Delete, ["time_entries", id, "external_reference"]) => {
                let id: i64 = id.parse().unwrap();
                let Some(entry) = entries.iter_mut().find(|entry| entry.id == id) else {
                    return (404, json!({}));
                };
                entry.external_reference = None;
                (200, json!({}))
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

#[async_trait]
impl Transport for FakeHarvest {
    async fn send(&self, request: Request) -> Result<Response, HarvestError> {
        let (status, body) = self.apply(&request);
        Ok(Response {
            status,
            headers: Vec::new(),
            body: body.to_string().into_bytes(),
        })
    }
}
