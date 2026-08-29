import Foundation
import Testing
@testable import SheavesCore

@Suite("HarvestClient")
struct HarvestClientTests {
    @Test("sends the three headers Harvest requires")
    func authenticationHeaders() async throws {
        let transport = StubTransport(body: Fixture.currentUser)
        let client = HarvestClient(credentials: Fixture.credentials, transport: transport, backoffScale: 0)

        _ = try await client.currentUser()

        let request = await transport.request(at: 0)
        #expect(request.value(forHTTPHeaderField: "Authorization") == "Bearer sekrit")
        #expect(request.value(forHTTPHeaderField: "Harvest-Account-Id") == "12345")
        // Harvest answers 400 to any request without a User-Agent.
        #expect(request.value(forHTTPHeaderField: "User-Agent") == HarvestClient.userAgent)
        #expect(request.url?.absoluteString == "https://api.harvestapp.com/v2/users/me")
    }

    @Test("refuses to build a request without credentials")
    func requiresCredentials() async {
        let client = HarvestClient(transport: StubTransport(body: "{}"))
        await #expect(throws: HarvestError.notConfigured) {
            _ = try await client.currentUser()
        }
    }

    @Test("decodes the current user")
    func decodesUser() async throws {
        let client = HarvestClient(credentials: Fixture.credentials, transport: StubTransport(body: Fixture.currentUser), backoffScale: 0)
        let user = try await client.currentUser()
        #expect(user.id == 1782959)
        #expect(user.name == "Kim Allen")
    }

    @Test("decodes company settings that drive timer behaviour")
    func decodesCompany() async throws {
        let client = HarvestClient(credentials: Fixture.credentials, transport: StubTransport(body: Fixture.company), backoffScale: 0)
        let company = try await client.company()
        #expect(company.weekStartDay == .monday)
        #expect(company.wantsTimestampTimers == false)
        #expect(HoursFormat(company: company) == .hoursMinutes)
    }

    @Test("decodes a time entry, including its null fields")
    func decodesTimeEntry() async throws {
        let body = Fixture.timeEntriesPage([Fixture.timeEntry, Fixture.runningTimeEntry])
        let client = HarvestClient(credentials: Fixture.credentials, transport: StubTransport(body: body), backoffScale: 0)

        let entries = try await client.timeEntries(
            userID: 1782959,
            from: CalendarDate(year: 2026, month: 8, day: 29),
            to: CalendarDate(year: 2026, month: 8, day: 29)
        )

        #expect(entries.count == 2)
        #expect(entries[0].project.name == "Marketing Website")
        #expect(entries[0].spentDate == CalendarDate(year: 2026, month: 8, day: 29))
        #expect(entries[0].notes == "Adding CSS styling")
        #expect(entries[0].isRunning == false)
        #expect(entries[1].notes == nil)
        #expect(entries[1].isRunning)
        #expect(entries[1].timerStartedAt != nil)
    }

    @Test("scopes time entry queries to one user and one day")
    func scopesQuery() async throws {
        let transport = StubTransport(body: Fixture.timeEntriesPage([]))
        let client = HarvestClient(credentials: Fixture.credentials, transport: transport, backoffScale: 0)

        _ = try await client.timeEntries(
            userID: 1782959,
            from: CalendarDate(year: 2026, month: 8, day: 1),
            to: CalendarDate(year: 2026, month: 8, day: 31)
        )

        let url = try #require(await transport.request(at: 0).url)
        let query = try #require(URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems)
        let values = Dictionary(uniqueKeysWithValues: query.map { ($0.name, $0.value) })
        // A personal access token belonging to an admin would otherwise return the
        // whole team's entries.
        #expect(values["user_id"] == "1782959")
        #expect(values["from"] == "2026-08-01")
        #expect(values["to"] == "2026-08-31")
    }

    @Test("follows pagination until next_page is null")
    func followsPagination() async throws {
        let transport = StubTransport([
            .init(body: Fixture.timeEntriesPage([Fixture.timeEntry], nextPage: 2, page: 1, totalPages: 2)),
            .init(body: Fixture.timeEntriesPage([Fixture.runningTimeEntry], nextPage: nil, page: 2, totalPages: 2)),
        ])
        let client = HarvestClient(credentials: Fixture.credentials, transport: transport, backoffScale: 0)

        let entries = try await client.timeEntries(
            userID: 1782959,
            from: .today(),
            to: .today()
        )

        #expect(entries.count == 2)
        #expect(await transport.requestCount == 2)
        let second = try #require(await transport.request(at: 1).url)
        #expect(second.absoluteString.contains("page=2"))
    }

    @Test("retries when rate limited, honouring Retry-After")
    func retriesOnRateLimit() async throws {
        let transport = StubTransport([
            .init(status: 429, body: "", headers: ["Retry-After": "0"]),
            .init(status: 200, body: Fixture.currentUser),
        ])
        let client = HarvestClient(credentials: Fixture.credentials, transport: transport, backoffScale: 0)

        let user = try await client.currentUser()

        #expect(user.id == 1782959)
        #expect(await transport.requestCount == 2)
    }

    @Test("gives up after repeated rate limiting")
    func givesUpOnPersistentRateLimit() async throws {
        let responses = Array(repeating: StubTransport.Response(status: 429, body: "", headers: ["Retry-After": "0"]), count: 6)
        let transport = StubTransport(responses)
        let client = HarvestClient(credentials: Fixture.credentials, transport: transport, backoffScale: 0)

        await #expect(throws: HarvestError.rateLimited(retryAfter: 0)) {
            _ = try await client.currentUser()
        }
    }

    @Test("maps authentication failures", arguments: [401, 403])
    func mapsUnauthorized(status: Int) async throws {
        let transport = StubTransport(status: status, body: "")
        let client = HarvestClient(credentials: Fixture.credentials, transport: transport, backoffScale: 0)
        await #expect(throws: HarvestError.unauthorized) {
            _ = try await client.currentUser()
        }
    }

    @Test("surfaces Harvest's own message on a rejected change")
    func surfacesRejectionMessage() async throws {
        let transport = StubTransport(status: 422, body: #"{"message": "Time entry is locked."}"#)
        let client = HarvestClient(credentials: Fixture.credentials, transport: transport, backoffScale: 0)

        await #expect(throws: HarvestError.rejected(status: 422, message: "Time entry is locked.")) {
            _ = try await client.stopTimeEntry(id: 1)
        }
    }

    @Test("starts a timer by omitting hours and ended_time")
    func startsTimer() async throws {
        let transport = StubTransport(status: 201, body: Fixture.runningTimeEntry)
        let client = HarvestClient(credentials: Fixture.credentials, transport: transport, backoffScale: 0)

        _ = try await client.startTimeEntry(
            projectID: 14308069,
            taskID: 8083366,
            spentDate: CalendarDate(year: 2026, month: 8, day: 29),
            notes: "spike"
        )

        let request = await transport.request(at: 0)
        #expect(request.httpMethod == "POST")
        let body = try #require(request.httpBody)
        let json = try #require(try JSONSerialization.jsonObject(with: body) as? [String: Any])
        #expect(json["project_id"] as? Int == 14308069)
        #expect(json["task_id"] as? Int == 8083366)
        #expect(json["spent_date"] as? String == "2026-08-29")
        #expect(json["notes"] as? String == "spike")
        // Omitting both is what makes Harvest start the clock, on duration-tracking
        // accounts and timestamp accounts alike.
        #expect(json["hours"] == nil)
        #expect(json["ended_time"] == nil)
    }

    @Test("stops and restarts through the dedicated endpoints")
    func stopAndRestart() async throws {
        let transport = StubTransport([
            .init(body: Fixture.timeEntry),
            .init(body: Fixture.runningTimeEntry),
        ])
        let client = HarvestClient(credentials: Fixture.credentials, transport: transport, backoffScale: 0)

        _ = try await client.stopTimeEntry(id: 636709355)
        _ = try await client.restartTimeEntry(id: 636709355)

        let stop = await transport.request(at: 0)
        #expect(stop.httpMethod == "PATCH")
        #expect(stop.url?.path() == "/v2/time_entries/636709355/stop")
        let restart = await transport.request(at: 1)
        #expect(restart.url?.path() == "/v2/time_entries/636709355/restart")
    }

    @Test("asks Harvest only for the running entry")
    func fetchesRunningEntry() async throws {
        let transport = StubTransport(body: Fixture.timeEntriesPage([Fixture.runningTimeEntry]))
        let client = HarvestClient(credentials: Fixture.credentials, transport: transport, backoffScale: 0)

        let running = try await client.runningTimeEntry(userID: 1782959)

        #expect(running?.id == 636708906)
        let url = try #require(await transport.request(at: 0).url)
        #expect(url.absoluteString.contains("is_running=true"))
    }
}

@Suite("Project assignments")
struct ProjectAssignmentTests {
    @Test("decodes nested projects, clients and tasks")
    func decodes() async throws {
        let client = HarvestClient(
            credentials: Fixture.credentials,
            transport: StubTransport(body: Fixture.projectAssignmentsPage)
        )

        let assignments = try await client.projectAssignments()

        #expect(assignments.count == 2)
        #expect(assignments[0].project.code == "OS1")
        #expect(assignments[0].taskAssignments.count == 2)
    }

    /// Offering an archived project or a retired task would just produce a 422 on start.
    @Test("offers only active project and task pairs as timer targets")
    func filtersInactive() async throws {
        let client = HarvestClient(
            credentials: Fixture.credentials,
            transport: StubTransport(body: Fixture.projectAssignmentsPage)
        )

        let targets = try await client.projectAssignments().timerTargets()

        #expect(targets.count == 1)
        #expect(targets[0].project.name == "Online Store - Phase 1")
        #expect(targets[0].task.name == "Graphic Design")
        #expect(targets[0].projectLabel == "123 Industries · Online Store - Phase 1")
        #expect(targets[0].id == "14308069:8083365")
    }
}
