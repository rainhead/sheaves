#if DEBUG
import Foundation
import SheavesCore

/// The account the documentation images are rendered from — and the one a
/// demo instance runs on, so a real screenshot can show the status item and
/// panel without touching anyone's timesheet.
///
/// Deliberately richer than any test fixture: one picture has to carry every state
/// at once, so there is a project with room in its budget, one over its budget, and
/// one that budgets nothing.
struct DemoAccount: HarvestTransport {
    func send(_ request: URLRequest) async throws -> (Data, HTTPURLResponse) {
        let url = request.url!
        let body = Self.routes.first { url.absoluteString.contains($0.0) }?.1 ?? "{}"
        let response = HTTPURLResponse(url: url, statusCode: 200, httpVersion: "HTTP/1.1", headerFields: nil)!
        return (Data(body.utf8), response)
    }

    // Longest fragment first: "users/me/project_assignments" also contains "users/me",
    // and "is_running=true" also contains "time_entries".
    private static var routes: [(String, String)] {
        [
            ("users/me/project_assignments", assignments),
            ("reports/project_budget", budgets),
            ("v2/clients", clients),
            ("is_running=true", page(#"[\#(running)]"#, key: "time_entries")),
            ("time_entries", page(#"[\#(running),\#(stopped)]"#, key: "time_entries")),
            ("company", company),
            ("users/me", currentUser),
        ]
    }

    private static var today: String { CalendarDate.today().description }

    private static func page(_ items: String, key: String) -> String {
        """
        { "\(key)": \(items), "per_page": 2000, "total_pages": 1,
          "total_entries": 1, "next_page": null, "previous_page": null, "page": 1 }
        """
    }

    private static func entry(
        id: Int, project: Int, projectName: String, client: Int, clientName: String,
        task: Int, taskName: String, hours: Double, notes: String?, running: Bool
    ) -> String {
        // Half an hour ago, so a running timer renders as a plausible duration rather
        // than as however long it has been since this file was written.
        let startedAt = running
            ? "\"\(Date().addingTimeInterval(-30 * 60).formatted(.iso8601))\""
            : "null"
        return """
        {
          "id": \(id), "spent_date": "\(today)",
          "client": { "id": \(client), "name": "\(clientName)" },
          "project": { "id": \(project), "name": "\(projectName)" },
          "task": { "id": \(task), "name": "\(taskName)" },
          "hours": \(hours), "hours_without_timer": \(hours),
          "notes": \(notes.map { "\"\($0)\"" } ?? "null"),
          "is_locked": false, "locked_reason": null, "is_billed": false,
          "timer_started_at": \(startedAt), "started_time": null, "ended_time": null,
          "is_running": \(running), "billable": true,
          "created_at": "\(today)T09:00:00Z", "updated_at": "\(today)T09:0\(running ? 1 : 0):00Z"
        }
        """
    }

    private static var running: String {
        entry(
            id: 900_000_001, project: 101, projectName: "Orcasound", client: 1, clientName: "Beam Reach",
            task: 201, taskName: "Programming", hours: 1.0, notes: "Hydrophone ingest", running: true
        )
    }

    private static var stopped: String {
        entry(
            id: 900_000_002, project: 103, projectName: "Beeline", client: 2,
            clientName: "Oregon State University Extension",
            task: 233, taskName: "Analysis", hours: 2.25, notes: nil, running: false
        )
    }

    private static func assignment(
        id: Int, project: Int, name: String, client: Int, clientName: String, tasks: [(Int, String)]
    ) -> String {
        let taskJSON = tasks.map {
            """
            { "id": \($0.0 + 90000), "billable": true, "is_active": true,
              "created_at": "2026-06-26T21:52:18Z", "updated_at": "2026-06-26T21:52:18Z",
              "hourly_rate": 100.0, "budget": null,
              "task": { "id": \($0.0), "name": "\($0.1)" } }
            """
        }.joined(separator: ",")
        return """
        {
          "id": \(id), "is_project_manager": true, "is_active": true,
          "use_default_rates": true, "budget": null,
          "created_at": "2026-06-26T21:52:18Z", "updated_at": "2026-06-26T21:52:18Z",
          "hourly_rate": 100.0,
          "project": { "id": \(project), "name": "\(name)", "code": "" },
          "client": { "id": \(client), "name": "\(clientName)" },
          "task_assignments": [\(taskJSON)]
        }
        """
    }

    private static var assignments: String {
        page(
            [
                assignment(
                    id: 1, project: 101, name: "Orcasound", client: 1, clientName: "Beam Reach",
                    tasks: [(201, "Programming"), (202, "Design"), (203, "Research"), (204, "Meetings")]
                ),
                assignment(
                    id: 2, project: 102, name: "SalishSea.io", client: 1, clientName: "Beam Reach",
                    tasks: [(211, "Programming"), (212, "Design")]
                ),
                assignment(
                    id: 3, project: 103, name: "Beeline", client: 2,
                    clientName: "Oregon State University Extension",
                    tasks: [(231, "Programming"), (232, "Field Work"), (233, "Analysis")]
                ),
            ].joined(separator: ",").enclosedInArray,
            key: "project_assignments"
        )
    }

    /// Room in one budget, an overrun in another, and a project that budgets nothing:
    /// the three outcomes the panel draws differently.
    private static var budgets: String {
        page(
            """
            [
              { "client_id": 1, "client_name": "Beam Reach",
                "project_id": 101, "project_name": "Orcasound", "project_code": "",
                "budget_is_monthly": false, "budget_by": "project_cost", "is_active": true,
                "budget": 5000.0, "budget_spent": 3993.0, "budget_remaining": 1007.0 },
              { "client_id": 2, "client_name": "Oregon State University Extension",
                "project_id": 103, "project_name": "Beeline", "project_code": "",
                "budget_is_monthly": true, "budget_by": "project_cost", "is_active": true,
                "budget": 5000.0, "budget_spent": 5240.0, "budget_remaining": -240.0 },
              { "client_id": 1, "client_name": "Beam Reach",
                "project_id": 102, "project_name": "SalishSea.io", "project_code": "",
                "budget_is_monthly": false, "budget_by": "none", "is_active": true,
                "budget": null, "budget_spent": null, "budget_remaining": null }
            ]
            """,
            key: "results"
        )
    }

    /// Both clients bill in dollars. Without this the budgets render as bare numbers,
    /// because Harvest names a currency nowhere else.
    private static var clients: String {
        page(
            """
            [
              { "id": 1, "name": "Beam Reach", "is_active": true, "currency": "USD" },
              { "id": 2, "name": "Oregon State University Extension",
                "is_active": true, "currency": "USD" }
            ]
            """,
            key: "clients"
        )
    }

    private static let company = """
    { "name": "Beam Reach", "full_domain": "beamreach.harvestapp.com", "is_active": true,
      "week_start_day": "Monday", "wants_timestamp_timers": false,
      "time_format": "hours_minutes", "weekly_capacity": 126000 }
    """

    private static let currentUser = """
    { "id": 1, "first_name": "Peter", "last_name": "Abrahamsen",
      "email": "peter@example.com" }
    """
}

private extension String {
    /// `page` wraps its argument in an object, so the argument has to be the array.
    var enclosedInArray: String { "[\(self)]" }
}
#endif
