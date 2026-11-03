//! Harvest responses for tests, copied from the Swift suite's fixtures so both
//! suites check the same JSON.

use crate::calendar_date::CalendarDate;
use crate::client::Credentials;

pub fn credentials() -> Credentials {
    Credentials::new("12345", "sekrit")
}

/// Fixtures date themselves to today. Pinning a literal date would quietly break
/// every test that compares against the tracker's day, one day after it was written.
pub fn today() -> CalendarDate {
    CalendarDate::of(jiff::Timestamp::now(), &jiff::tz::TimeZone::system())
}

pub fn time_entry() -> String {
    r#"{
  "id": 636709355,
  "spent_date": "{today}",
  "user": { "id": 1782959, "name": "Kim Allen" },
  "client": { "id": 5735774, "name": "ABC Corp" },
  "project": { "id": 14307913, "name": "Marketing Website" },
  "task": { "id": 8083365, "name": "Graphic Design" },
  "hours": 2.11,
  "hours_without_timer": 2.11,
  "rounded_hours": 2.25,
  "notes": "Adding CSS styling",
  "is_locked": false,
  "locked_reason": null,
  "is_closed": false,
  "approval_status": "unsubmitted",
  "is_billed": false,
  "timer_started_at": null,
  "started_time": "3:00pm",
  "ended_time": "5:00pm",
  "is_running": false,
  "billable": true,
  "budgeted": true,
  "billable_rate": 100.0,
  "cost_rate": 50.0,
  "created_at": "2026-08-29T22:44:36Z",
  "updated_at": "2026-08-29T22:48:56Z"
}"#
    .replace("{today}", &today().to_string())
}

pub fn running_time_entry() -> String {
    r#"{
  "id": 636708906,
  "spent_date": "{today}",
  "user": { "id": 1782959, "name": "Kim Allen" },
  "client": { "id": 5735776, "name": "123 Industries" },
  "project": { "id": 14308069, "name": "Online Store - Phase 1" },
  "task": { "id": 8083366, "name": "Programming" },
  "hours": 1.5,
  "hours_without_timer": 1.0,
  "notes": null,
  "is_locked": false,
  "locked_reason": null,
  "is_billed": false,
  "timer_started_at": "2026-08-29T14:30:00Z",
  "started_time": "2:30pm",
  "ended_time": null,
  "is_running": true,
  "billable": true,
  "created_at": "2026-08-29T14:30:00Z",
  "updated_at": "2026-08-29T14:30:00Z"
}"#
    .replace("{today}", &today().to_string())
}

pub const PROJECT_ASSIGNMENTS_PAGE: &str = r#"{
  "project_assignments": [
    {
      "id": 130403296,
      "is_project_manager": true,
      "is_active": true,
      "use_default_rates": true,
      "budget": null,
      "created_at": "2026-06-26T21:52:18Z",
      "updated_at": "2026-06-26T21:52:18Z",
      "hourly_rate": 100.0,
      "project": { "id": 14308069, "name": "Online Store - Phase 1", "code": "OS1" },
      "client": { "id": 5735776, "name": "123 Industries" },
      "task_assignments": [
        {
          "id": 155505016,
          "billable": true,
          "is_active": true,
          "created_at": "2026-06-26T21:52:18Z",
          "updated_at": "2026-06-26T21:52:18Z",
          "hourly_rate": 100.0,
          "budget": null,
          "task": { "id": 8083365, "name": "Graphic Design" }
        },
        {
          "id": 155505017,
          "billable": true,
          "is_active": false,
          "created_at": "2026-06-26T21:52:18Z",
          "updated_at": "2026-06-26T21:52:18Z",
          "hourly_rate": 100.0,
          "budget": null,
          "task": { "id": 8083366, "name": "Retired Task" }
        }
      ]
    },
    {
      "id": 130403297,
      "is_project_manager": false,
      "is_active": false,
      "use_default_rates": true,
      "budget": null,
      "created_at": "2026-06-26T21:52:18Z",
      "updated_at": "2026-06-26T21:52:18Z",
      "hourly_rate": 100.0,
      "project": { "id": 14307913, "name": "Archived Project", "code": "" },
      "client": { "id": 5735774, "name": "ABC Corp" },
      "task_assignments": []
    }
  ],
  "per_page": 2000,
  "total_pages": 1,
  "total_entries": 2,
  "next_page": null,
  "previous_page": null,
  "page": 1
}"#;

pub const CLIENTS_PAGE: &str = r#"{
  "clients": [
    { "id": 5735776, "name": "123 Industries", "is_active": true, "currency": "EUR" },
    { "id": 5735774, "name": "ABC Corp", "is_active": true, "currency": "USD" }
  ],
  "per_page": 2000, "total_pages": 1, "total_entries": 2,
  "next_page": null, "previous_page": null, "page": 1
}"#;

pub const COMPANY: &str = r#"{
  "base_uri": "https://acme.harvestapp.com",
  "full_domain": "acme.harvestapp.com",
  "name": "Acme",
  "is_active": true,
  "week_start_day": "Monday",
  "wants_timestamp_timers": false,
  "time_format": "hours_minutes",
  "date_format": "%m/%d/%Y",
  "plan_type": "sponsored",
  "clock": "12h",
  "decimal_symbol": ".",
  "thousands_separator": ",",
  "color_scheme": "orange",
  "weekly_capacity": 126000,
  "expense_feature": true,
  "invoice_feature": true,
  "estimate_feature": true,
  "approval_feature": true
}"#;

pub const PROJECT_BUDGET_PAGE: &str = r#"{
  "results": [
    {
      "client_id": 5735776,
      "client_name": "123 Industries",
      "project_id": 14308069,
      "project_name": "Online Store - Phase 1",
      "project_code": "OS1",
      "budget_is_monthly": false,
      "budget_by": "project",
      "is_active": true,
      "budget": 40.0,
      "budget_spent": 32.0,
      "budget_remaining": 8.0
    },
    {
      "client_id": 5735774,
      "client_name": "ABC Corp",
      "project_id": 14307913,
      "project_name": "Marketing Website",
      "project_code": "",
      "budget_is_monthly": false,
      "budget_by": "none",
      "is_active": true,
      "budget": null,
      "budget_spent": null,
      "budget_remaining": null
    },
    {
      "client_id": 5735774,
      "client_name": "ABC Corp",
      "project_id": 14307915,
      "project_name": "Retainer",
      "project_code": "",
      "budget_is_monthly": true,
      "budget_by": "task_fees",
      "is_active": true,
      "budget": null,
      "budget_spent": null,
      "budget_remaining": null
    }
  ],
  "per_page": 2000,
  "total_pages": 1,
  "total_entries": 3,
  "next_page": null,
  "previous_page": null,
  "page": 1
}"#;

pub const MONETARY_BUDGET_PAGE: &str = r#"{
  "results": [
    {
      "client_id": 5735776,
      "client_name": "123 Industries",
      "project_id": 14308069,
      "project_name": "Online Store - Phase 1",
      "project_code": "OS1",
      "budget_is_monthly": false,
      "budget_by": "project_cost",
      "is_active": true,
      "budget": 40.0,
      "budget_spent": 32.0,
      "budget_remaining": 8.0
    }
  ],
  "per_page": 2000, "total_pages": 1, "total_entries": 1,
  "next_page": null, "previous_page": null, "page": 1
}"#;

pub const NO_PROJECT_BUDGETS_PAGE: &str = r#"{
  "results": [
    {
      "client_id": 5735776,
      "client_name": "123 Industries",
      "project_id": 14308069,
      "project_name": "Online Store - Phase 1",
      "project_code": "OS1",
      "budget_is_monthly": false,
      "budget_by": "none",
      "is_active": true,
      "budget": null,
      "budget_spent": null,
      "budget_remaining": null
    }
  ],
  "per_page": 2000,
  "total_pages": 1,
  "total_entries": 1,
  "next_page": null,
  "previous_page": null,
  "page": 1
}"#;

pub const CURRENT_USER: &str = r#"{
  "id": 1782959,
  "first_name": "Kim",
  "last_name": "Allen",
  "email": "kim@acme.example",
  "telephone": "",
  "timezone": "Eastern Time (US & Canada)",
  "is_active": true,
  "created_at": "2026-06-26T20:41:00Z",
  "updated_at": "2026-06-26T21:23:36Z"
}"#;

pub fn time_entries_page(
    entries: &[String],
    next_page: Option<i64>,
    page: i64,
    total_pages: i64,
    next_link: Option<&str>,
) -> String {
    let next_page = next_page.map_or("null".to_string(), |page| page.to_string());
    let next_link = next_link.map_or("null".to_string(), |link| format!("\"{link}\""));
    format!(
        r#"{{
  "time_entries": [{entries}],
  "per_page": 2000,
  "total_pages": {total_pages},
  "total_entries": {count},
  "next_page": {next_page},
  "previous_page": null,
  "page": {page},
  "links": {{ "next": {next_link} }}
}}"#,
        entries = entries.join(","),
        count = entries.len(),
    )
}

/// A single page holding `entries`.
pub fn entries_page(entries: &[String]) -> String {
    time_entries_page(entries, None, 1, 1, None)
}
