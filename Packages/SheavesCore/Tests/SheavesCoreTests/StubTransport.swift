import Foundation
@testable import SheavesCore

/// A scripted `HarvestTransport`: hands back queued responses in order and records
/// what was asked for, so tests can assert on headers, paths and query strings.
actor StubTransport: HarvestTransport {
    struct Response {
        var status: Int = 200
        var body: String = "{}"
        var headers: [String: String] = [:]
    }

    private var scripted: [Response]
    private(set) var requests: [URLRequest] = []

    init(_ scripted: [Response]) {
        self.scripted = scripted
    }

    init(status: Int = 200, body: String) {
        self.scripted = [Response(status: status, body: body)]
    }

    func send(_ request: URLRequest) async throws -> (Data, HTTPURLResponse) {
        requests.append(request)
        guard !scripted.isEmpty else {
            throw HarvestError.invalidResponse
        }
        let response = scripted.removeFirst()
        let http = HTTPURLResponse(
            url: request.url!,
            statusCode: response.status,
            httpVersion: "HTTP/1.1",
            headerFields: response.headers
        )!
        return (Data(response.body.utf8), http)
    }

    var requestCount: Int { requests.count }

    func request(at index: Int) -> URLRequest { requests[index] }
}

enum Fixture {
    static let credentials = HarvestCredentials(accountID: "12345", token: "sekrit")

    static let timeEntry = """
    {
      "id": 636709355,
      "spent_date": "2026-08-29",
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
    }
    """

    static let runningTimeEntry = """
    {
      "id": 636708906,
      "spent_date": "2026-08-29",
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
    }
    """

    static func timeEntriesPage(_ entries: [String], nextPage: Int? = nil, page: Int = 1, totalPages: Int = 1) -> String {
        """
        {
          "time_entries": [\(entries.joined(separator: ","))],
          "per_page": 2000,
          "total_pages": \(totalPages),
          "total_entries": \(entries.count),
          "next_page": \(nextPage.map(String.init) ?? "null"),
          "previous_page": null,
          "page": \(page)
        }
        """
    }

    static let projectAssignmentsPage = """
    {
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
    }
    """

    static let company = """
    {
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
    }
    """

    static let currentUser = """
    {
      "id": 1782959,
      "first_name": "Kim",
      "last_name": "Allen",
      "email": "kim@acme.example",
      "telephone": "",
      "timezone": "Eastern Time (US & Canada)",
      "is_active": true,
      "created_at": "2026-06-26T20:41:00Z",
      "updated_at": "2026-06-26T21:23:36Z"
    }
    """
}
