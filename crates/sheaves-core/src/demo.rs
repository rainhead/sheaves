//! A made-up Harvest account, for the documentation images and for a demo instance
//! that can be photographed without touching anyone's timesheet.
//!
//! Deliberately richer than any test fixture: one picture has to carry every state
//! at once, so there is a project with room in its budget, one over its budget, and
//! one that budgets nothing. The names are the ones Harvest's own documentation uses.

use async_trait::async_trait;
use jiff::Timestamp;
use serde_json::{Value, json};

use crate::calendar_date::CalendarDate;
use crate::client::{HarvestError, Request, Response, Transport};
use crate::models::offset_by;

/// Answers every request from the demo account. Anything it does not know, which
/// includes every change, gets an empty object back.
pub struct DemoTransport;

#[async_trait]
impl Transport for DemoTransport {
    async fn send(&self, request: Request) -> Result<Response, HarvestError> {
        let today = CalendarDate::of(Timestamp::now(), &jiff::tz::TimeZone::system());
        let url = request.url.as_str();
        // Longest fragment first: "users/me/project_assignments" also contains
        // "users/me", and "is_running=true" also contains "time_entries".
        let body = if url.contains("users/me/project_assignments") {
            page(assignments(), "project_assignments")
        } else if url.contains("reports/project_budget") {
            page(budgets(), "results")
        } else if url.contains("v2/clients") {
            page(clients(), "clients")
        } else if url.contains("is_running=true") {
            page(json!([running(today)]), "time_entries")
        } else if url.contains("time_entries") {
            page(json!([running(today), stopped(today)]), "time_entries")
        } else if url.contains("company") {
            company()
        } else if url.contains("users/me") {
            json!({ "id": 1, "first_name": "Kim", "last_name": "Allen", "email": "kim@acme.example" })
        } else {
            json!({})
        };
        Ok(Response {
            status: 200,
            headers: Vec::new(),
            body: body.to_string().into_bytes(),
        })
    }
}

fn page(items: Value, key: &str) -> Value {
    let count = items.as_array().map_or(0, Vec::len);
    json!({
        key: items, "per_page": 2000, "total_pages": 1, "total_entries": count,
        "next_page": null, "previous_page": null, "page": 1
    })
}

struct Entry<'a> {
    id: i64,
    project: (i64, &'a str),
    client: (i64, &'a str),
    task: (i64, &'a str),
    hours: f64,
    notes: Option<&'a str>,
    running: bool,
}

fn entry(today: CalendarDate, entry: Entry<'_>) -> Value {
    // Half an hour ago, so a running timer renders as a plausible duration rather
    // than as however long it has been since this file was written. Harvest's
    // `hours` on a running entry is a snapshot *including* that live half hour;
    // `hours_without_timer` is the banked part alone.
    let started_at = entry
        .running
        .then(|| offset_by(Timestamp::now(), -30.0 * 60.0).to_string());
    json!({
        "id": entry.id, "spent_date": today.to_string(),
        "client": { "id": entry.client.0, "name": entry.client.1 },
        "project": { "id": entry.project.0, "name": entry.project.1 },
        "task": { "id": entry.task.0, "name": entry.task.1 },
        "hours": if entry.running { entry.hours + 0.5 } else { entry.hours },
        "hours_without_timer": entry.hours,
        "notes": entry.notes,
        "is_locked": false, "locked_reason": null, "is_billed": false,
        "timer_started_at": started_at, "started_time": null, "ended_time": null,
        "is_running": entry.running, "billable": true,
        "created_at": format!("{today}T09:00:00Z"),
        "updated_at": format!("{today}T09:0{}:00Z", if entry.running { 1 } else { 0 }),
    })
}

fn running(today: CalendarDate) -> Value {
    entry(
        today,
        Entry {
            id: 900_000_001,
            project: (101, "Online Store - Phase 1"),
            client: (1, "123 Industries"),
            task: (201, "Programming"),
            hours: 1.0,
            notes: Some("Checkout flow"),
            running: true,
        },
    )
}

fn stopped(today: CalendarDate) -> Value {
    entry(
        today,
        Entry {
            id: 900_000_002,
            project: (103, "Marketing Website"),
            client: (2, "ABC Corp"),
            task: (233, "Graphic Design"),
            hours: 2.25,
            notes: None,
            running: false,
        },
    )
}

fn assignment(id: i64, project: (i64, &str), client: (i64, &str), tasks: &[(i64, &str)]) -> Value {
    let tasks: Vec<Value> = tasks
        .iter()
        .map(|(task, name)| {
            json!({
                "id": task + 90000, "billable": true, "is_active": true,
                "created_at": "2026-06-26T21:52:18Z", "updated_at": "2026-06-26T21:52:18Z",
                "hourly_rate": 100.0, "budget": null,
                "task": { "id": task, "name": name }
            })
        })
        .collect();
    json!({
        "id": id, "is_project_manager": true, "is_active": true,
        "use_default_rates": true, "budget": null,
        "created_at": "2026-06-26T21:52:18Z", "updated_at": "2026-06-26T21:52:18Z",
        "hourly_rate": 100.0,
        "project": { "id": project.0, "name": project.1, "code": "" },
        "client": { "id": client.0, "name": client.1 },
        "task_assignments": tasks
    })
}

fn assignments() -> Value {
    json!([
        assignment(
            1,
            (101, "Online Store - Phase 1"),
            (1, "123 Industries"),
            &[
                (201, "Programming"),
                (202, "Design"),
                (203, "Research"),
                (204, "Meetings")
            ],
        ),
        assignment(
            2,
            (102, "Internal Tools"),
            (1, "123 Industries"),
            &[(211, "Programming"), (212, "Design")],
        ),
        assignment(
            3,
            (103, "Marketing Website"),
            (2, "ABC Corp"),
            &[(231, "Copywriting"), (232, "SEO"), (233, "Graphic Design")],
        ),
    ])
}

/// Room in one budget, an overrun in another, and a project that budgets nothing:
/// the three outcomes the panel draws differently.
fn budgets() -> Value {
    json!([
        { "client_id": 1, "client_name": "123 Industries",
          "project_id": 101, "project_name": "Online Store - Phase 1", "project_code": "",
          "budget_is_monthly": false, "budget_by": "project_cost", "is_active": true,
          "budget": 5000.0, "budget_spent": 3993.0, "budget_remaining": 1007.0 },
        { "client_id": 2, "client_name": "ABC Corp",
          "project_id": 103, "project_name": "Marketing Website", "project_code": "",
          "budget_is_monthly": true, "budget_by": "project_cost", "is_active": true,
          "budget": 5000.0, "budget_spent": 5240.0, "budget_remaining": -240.0 },
        { "client_id": 1, "client_name": "123 Industries",
          "project_id": 102, "project_name": "Internal Tools", "project_code": "",
          "budget_is_monthly": false, "budget_by": "none", "is_active": true,
          "budget": null, "budget_spent": null, "budget_remaining": null }
    ])
}

/// Both clients bill in dollars. Without this the budgets render as bare numbers,
/// because Harvest names a currency nowhere else.
fn clients() -> Value {
    json!([
        { "id": 1, "name": "123 Industries", "is_active": true, "currency": "USD" },
        { "id": 2, "name": "ABC Corp", "is_active": true, "currency": "USD" }
    ])
}

fn company() -> Value {
    json!({
        "name": "Acme", "full_domain": "acme.harvestapp.com", "is_active": true,
        "week_start_day": "Monday", "wants_timestamp_timers": false,
        "time_format": "hours_minutes", "weekly_capacity": 126000
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::client::{Credentials, HarvestClient};
    use crate::clock::SystemClock;
    use crate::credentials::MemoryCredentialStore;
    use crate::queue::MutationQueue;
    use crate::snapshot::SnapshotStore;
    use crate::tracker::{Connection, TimeTracker};

    /// The images are drawn from whatever this account syncs to, so it has to sync,
    /// and to every state a picture needs.
    #[tokio::test]
    async fn syncs_to_every_state_the_images_show() {
        let directory = tempfile::tempdir().unwrap();
        let tracker = TimeTracker::new(
            HarvestClient::new(
                Some(Credentials::new("0", "none")),
                Arc::new(DemoTransport),
                0.0,
            ),
            Arc::new(MemoryCredentialStore::default()),
            SnapshotStore::new(directory.path().join("snapshot.json")),
            MutationQueue::open(directory.path().join("queue.json"), Arc::new(SystemClock)),
            Arc::new(SystemClock),
        );

        tracker.sync().await;

        let state = tracker.state();
        assert_eq!(state.connection, Connection::Online);
        assert_eq!(state.entries.len(), 2);
        assert!(state.running_entry().is_some());
        assert_eq!(state.ordered_projects().len(), 3);
        assert!(state.budgets.values().any(|budget| budget.is_over_budget()));
        assert!(
            state
                .budgets
                .values()
                .any(|budget| !budget.is_over_budget())
        );
    }
}
