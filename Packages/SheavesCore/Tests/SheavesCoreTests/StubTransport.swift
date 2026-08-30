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

    /// Fixtures date themselves to today. Pinning a literal date would quietly break
    /// every test that compares against `TimeTracker.day`, one day after it was written.
    static let today = CalendarDate.today()

    static var timeEntry: String { """
    {
      "id": 636709355,
      "spent_date": "\(today)",
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
    """ }

    static var runningTimeEntry: String { """
    {
      "id": 636708906,
      "spent_date": "\(today)",
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
    """ }

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

    /// One project with two active tasks, for the questions that are about choosing
    /// between tasks rather than between projects.
    static let twoTaskProjectAssignmentsPage = """
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
              "id": 155505018,
              "billable": true,
              "is_active": true,
              "created_at": "2026-06-26T21:52:18Z",
              "updated_at": "2026-06-26T21:52:18Z",
              "hourly_rate": 100.0,
              "budget": null,
              "task": { "id": 8083366, "name": "Programming" }
            }
          ]
        }
      ],
      "per_page": 2000,
      "total_pages": 1,
      "total_entries": 1,
      "next_page": null,
      "previous_page": null,
      "page": 1
    }
    """

    /// Two clients, one billing in a currency that is not the device's.
    static let clientsPage = """
    {
      "clients": [
        { "id": 5735776, "name": "123 Industries", "is_active": true, "currency": "EUR" },
        { "id": 5735774, "name": "ABC Corp", "is_active": true, "currency": "USD" }
      ],
      "per_page": 2000, "total_pages": 1, "total_entries": 2,
      "next_page": null, "previous_page": null, "page": 1
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

    /// The three shapes the report actually returns: a readable hours budget, a
    /// project with no budget at all, and a monetary budget whose figures came back
    /// null because this token may not read them.
    static let projectBudgetPage = """
    {
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
    }
    """

    /// The same project, budgeted in money rather than hours, so the currency join has
    /// something to join to.
    static let monetaryBudgetPage = """
    {
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
    }
    """

    /// An account that budgets nothing — the case where the whole feature should be
    /// absent rather than present and empty.
    static let noProjectBudgetsPage = """
    {
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

/// A transport that answers by HTTP method and URL path rather than call order.
///
/// `TimeTracker` fires several requests concurrently with `async let`, so their
/// arrival order is not fixed and an ordered stub would hand back the wrong bodies.
/// Matching the method matters too: `POST /time_entries` and `GET /time_entries`
/// return quite different shapes, and conflating them once made a test pass for the
/// wrong reason.
actor RoutingTransport: HarvestTransport {
    struct Route {
        var method: String?
        var fragment: String
        var body: String
        var status: Int = 200

        func matches(_ request: URLRequest) -> Bool {
            if let method, method != request.httpMethod { return false }
            return request.url?.absoluteString.contains(fragment) == true
        }
    }

    struct Call: Sendable {
        var method: String
        var path: String
        /// Decoded eagerly to keep the type Sendable; only scalars are asserted on.
        var hours: Double?
        var notes: String?
        var projectID: Int?
        var spentDate: String?
    }

    private let routes: [Route]
    /// When set, every request fails as if the network were gone.
    private var isOffline = false
    private(set) var paths: [String] = []
    /// Method, path and decoded body of every request, so a test can assert that a
    /// POST actually happened and with what — not merely that some request did.
    private(set) var calls: [Call] = []

    init(_ routes: [Route]) {
        self.routes = routes
    }

    func goOffline() { isOffline = true }

    func send(_ request: URLRequest) async throws -> (Data, HTTPURLResponse) {
        let url = request.url!
        paths.append(url.path())
        let body = request.httpBody
            .flatMap { try? JSONSerialization.jsonObject(with: $0) as? [String: Any] } ?? [:]
        calls.append(
            Call(
                method: request.httpMethod ?? "GET",
                path: url.path(),
                hours: body["hours"] as? Double,
                notes: body["notes"] as? String,
                projectID: body["project_id"] as? Int,
                spentDate: body["spent_date"] as? String
            )
        )
        if isOffline { throw URLError(.notConnectedToInternet) }
        guard let route = routes.first(where: { $0.matches(request) }) else {
            throw HarvestError.notFound
        }
        let http = HTTPURLResponse(url: url, statusCode: route.status, httpVersion: "HTTP/1.1", headerFields: nil)!
        return (Data(route.body.utf8), http)
    }

    func callCount(matching fragment: String) -> Int {
        paths.filter { $0.contains(fragment) }.count
    }

    func calls(method: String, containing fragment: String) -> [Call] {
        calls.filter { $0.method == method && $0.path.contains(fragment) }
    }

    func firstIndex(method: String, containing fragment: String) -> Int? {
        calls.firstIndex { $0.method == method && $0.path.contains(fragment) }
    }

    func firstIndex(method: String, excluding fragment: String) -> Int? {
        calls.firstIndex { $0.method == method && !$0.path.contains(fragment) }
    }

    func hours(atCall index: Int) -> Double? {
        calls.indices.contains(index) ? calls[index].hours : nil
    }
}

extension RoutingTransport {
    /// A working account: one project with one active task, plus writable timers.
    static func standardAccount(
        entries: [String] = [Fixture.timeEntry],
        budgets: String = Fixture.projectBudgetPage,
        budgetStatus: Int = 200,
        assignments: String = Fixture.projectAssignmentsPage,
        clients: String = Fixture.clientsPage,
        clientStatus: Int = 200
    ) -> RoutingTransport {
        RoutingTransport([
            Route(method: "GET", fragment: "reports/project_budget", body: budgets, status: budgetStatus),
            Route(method: "GET", fragment: "v2/clients", body: clients, status: clientStatus),
            Route(method: "GET", fragment: "users/me/project_assignments", body: assignments),
            Route(method: "GET", fragment: "users/me", body: Fixture.currentUser),
            Route(method: "GET", fragment: "company", body: Fixture.company),
            Route(method: "PATCH", fragment: "/stop", body: Fixture.timeEntry),
            Route(method: "PATCH", fragment: "/restart", body: Fixture.runningTimeEntry),
            Route(method: "POST", fragment: "time_entries", body: Fixture.runningTimeEntry, status: 201),
            Route(method: "GET", fragment: "is_running=true", body: Fixture.timeEntriesPage([])),
            Route(method: "GET", fragment: "time_entries", body: Fixture.timeEntriesPage(entries)),
        ])
    }

    /// The same account, but with a timer already running.
    static func accountWithRunningTimer() -> RoutingTransport {
        let running = Fixture.timeEntriesPage([Fixture.runningTimeEntry])
        return RoutingTransport([
            Route(method: "GET", fragment: "reports/project_budget", body: Fixture.projectBudgetPage),
            Route(method: "GET", fragment: "v2/clients", body: Fixture.clientsPage),
            Route(method: "GET", fragment: "users/me/project_assignments", body: Fixture.projectAssignmentsPage),
            Route(method: "GET", fragment: "users/me", body: Fixture.currentUser),
            Route(method: "GET", fragment: "company", body: Fixture.company),
            Route(method: "GET", fragment: "is_running=true", body: running),
            Route(method: "GET", fragment: "time_entries", body: running),
        ])
    }
}
