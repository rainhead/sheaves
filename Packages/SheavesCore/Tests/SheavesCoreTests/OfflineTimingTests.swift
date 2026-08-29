import Foundation
import Testing
@testable import SheavesCore

/// Harvest's `/stop` banks time up to the moment the request *arrives*, and a create
/// starts its timer on arrival too. Replaying bare commands after a spell offline
/// therefore destroys or invents hours. These tests pin down the fix: the mutation
/// carries the time the user actually acted, and the queue sends explicit hours.
@Suite("Offline timing")
struct OfflineTimingTests {
    private func temporaryFile() -> URL {
        URL.temporaryDirectory.appending(path: "sheaves-timing-\(UUID().uuidString).json")
    }

    private var target: TimerTarget {
        TimerTarget(
            project: Reference(id: 14308069, name: "Online Store - Phase 1"),
            task: Reference(id: 8083366, name: "Programming"),
            client: Reference(id: 5735776, name: "123 Industries")
        )
    }

    private func writableAccount() -> RoutingTransport {
        RoutingTransport([
            RoutingTransport.Route(method: "PATCH", fragment: "/stop", body: Fixture.timeEntry),
            RoutingTransport.Route(method: "PATCH", fragment: "/restart", body: Fixture.runningTimeEntry),
            RoutingTransport.Route(method: "POST", fragment: "time_entries", body: Fixture.timeEntry, status: 201),
            RoutingTransport.Route(method: "PATCH", fragment: "time_entries", body: Fixture.timeEntry),
        ])
    }

    /// Started 2:00, stopped 2:30, delivered at 4:00. Before the fix this created the
    /// entry at 4:00 and stopped it immediately: half an hour of work became zero.
    @Test("records the half hour actually worked, not the moment the request landed")
    func createCarriesMeasuredLength() async throws {
        let file = temporaryFile()
        defer { try? FileManager.default.removeItem(at: file) }
        let transport = writableAccount()
        let client = HarvestClient(credentials: Fixture.credentials, transport: transport, backoffScale: 0)
        let queue = MutationQueue(fileURL: file)

        let startedAt = Date().addingTimeInterval(-2 * 3600)
        await queue.enqueue(
            .create(
                local: UUID(), target: target, spentDate: .today(), notes: nil,
                startedAt: startedAt, endedAt: startedAt.addingTimeInterval(1800)
            )
        )
        let report = await queue.drain(using: client)

        #expect(report.applied == 1)
        let post = try #require(await transport.calls(method: "POST", containing: "time_entries").first)
        #expect(post.hours == 0.5)
        // A finished entry must not be resumed.
        #expect(await transport.calls(method: "PATCH", containing: "/restart").isEmpty)
    }

    /// A timer still running when the queue drains keeps the time it ran offline and
    /// then carries on counting.
    @Test("banks offline time before resuming a timer that is still running")
    func stillRunningCreateBanksThenResumes() async throws {
        let file = temporaryFile()
        defer { try? FileManager.default.removeItem(at: file) }
        let transport = writableAccount()
        let client = HarvestClient(credentials: Fixture.credentials, transport: transport, backoffScale: 0)
        let queue = MutationQueue(fileURL: file)

        await queue.enqueue(
            .create(
                local: UUID(), target: target, spentDate: .today(), notes: nil,
                startedAt: Date().addingTimeInterval(-3600), endedAt: nil
            )
        )
        await queue.drain(using: client)

        let post = try #require(await transport.calls(method: "POST", containing: "time_entries").first)
        let hours = try #require(post.hours)
        #expect(abs(hours - 1.0) < 0.01)
        #expect(await transport.calls(method: "PATCH", containing: "/restart").count == 1)
    }

    /// Running since 9:00, network drops, user stops at 9:30, reconnects at 11:00.
    /// Harvest's own stop would bank two hours; the recorded half hour must win.
    @Test("corrects the total after stopping, so a late stop cannot inflate it")
    func stopCorrectsHours() async throws {
        let file = temporaryFile()
        defer { try? FileManager.default.removeItem(at: file) }
        let transport = writableAccount()
        let client = HarvestClient(credentials: Fixture.credentials, transport: transport, backoffScale: 0)
        let queue = MutationQueue(fileURL: file)

        await queue.enqueue(.stop(.server(636709355), hours: 0.5))
        await queue.drain(using: client)

        #expect(await transport.calls(method: "PATCH", containing: "/stop").count == 1)
        let correction = try #require(
            await transport.calls(method: "PATCH", containing: "time_entries")
                .first { !$0.path.contains("/stop") }
        )
        #expect(correction.hours == 0.5)
    }

    /// Resuming offline must add the time it ran before the request landed, and the
    /// total must be set while the entry is still stopped.
    @Test("banks offline time before resuming, never while the timer is live")
    func restartBanksOfflineTimeFirst() async throws {
        let file = temporaryFile()
        defer { try? FileManager.default.removeItem(at: file) }
        let transport = writableAccount()
        let client = HarvestClient(credentials: Fixture.credentials, transport: transport, backoffScale: 0)
        let queue = MutationQueue(fileURL: file)

        await queue.enqueue(
            .restart(.server(636709355), resumedAt: Date().addingTimeInterval(-1800), bankedHours: 1.0)
        )
        await queue.drain(using: client)

        let patchIndex = try #require(await transport.firstIndex(method: "PATCH", excluding: "/restart"))
        let restartIndex = try #require(await transport.firstIndex(method: "PATCH", containing: "/restart"))
        #expect(patchIndex < restartIndex, "hours must be set before the timer resumes")
        let hours = try #require(await transport.hours(atCall: patchIndex))
        #expect(abs(hours - 1.5) < 0.01)
    }

    /// A timer started and stopped before the queue ever drained should reach Harvest
    /// as one finished entry, not a create followed by a stop.
    @Test("amends a queued create rather than queueing a second mutation")
    func amendsQueuedCreate() async throws {
        let file = temporaryFile()
        defer { try? FileManager.default.removeItem(at: file) }
        let queue = MutationQueue(fileURL: file)
        let local = UUID()

        await queue.enqueue(
            .create(local: local, target: target, spentDate: .today(), notes: nil,
                    startedAt: Date(), endedAt: nil)
        )
        let amended = await queue.amendCreate(local: local, endedAt: Date())

        #expect(amended)
        #expect(await queue.count == 1)
        #expect(await queue.amendCreate(local: UUID(), endedAt: Date()) == false)
    }
}

@Suite("Retry-After")
struct RetryAfterTests {
    private func response(_ headers: [String: String]) -> HTTPURLResponse {
        HTTPURLResponse(
            url: URL(string: "https://api.harvestapp.com/v2/users/me")!,
            statusCode: 429,
            httpVersion: "HTTP/1.1",
            headerFields: headers
        )!
    }

    /// The retry test itself runs with the backoff scaled to zero, so it cannot tell
    /// an honoured header from an ignored one. This checks the parsing directly.
    @Test("reads the header Harvest sends")
    func readsHeader() {
        #expect(HarvestClient.retryAfter(from: response(["Retry-After": "15"])) == 15)
        #expect(HarvestClient.retryAfter(from: response(["Retry-After": "0"])) == 0)
    }

    @Test("falls back to Harvest's own window when the header is missing or junk")
    func fallsBack() {
        #expect(HarvestClient.retryAfter(from: response([:])) == 15)
        #expect(HarvestClient.retryAfter(from: response(["Retry-After": "soon"])) == 15)
    }
}

@Suite("Local id handoff")
struct LocalIdentifierHandoffTests {
    private func temporaryFile() -> URL {
        URL.temporaryDirectory.appending(path: "sheaves-ids-\(UUID().uuidString).json")
    }

    private var target: TimerTarget {
        TimerTarget(
            project: Reference(id: 14308069, name: "Online Store - Phase 1"),
            task: Reference(id: 8083366, name: "Programming")
        )
    }

    private func writableAccount() -> RoutingTransport {
        RoutingTransport([
            RoutingTransport.Route(method: "PATCH", fragment: "/stop", body: Fixture.timeEntry),
            RoutingTransport.Route(method: "PATCH", fragment: "/restart", body: Fixture.runningTimeEntry),
            RoutingTransport.Route(method: "POST", fragment: "time_entries", body: Fixture.runningTimeEntry, status: 201),
            RoutingTransport.Route(method: "PATCH", fragment: "time_entries", body: Fixture.timeEntry),
        ])
    }

    /// Stopping a timer after its create drained, but before the store adopted the
    /// real id, used to be discarded as "not found" — leaving the timer running on
    /// Harvest forever.
    @Test("honours a stop queued against an already-created local id")
    func resolvesLocalIdAfterCreate() async throws {
        let file = temporaryFile()
        defer { try? FileManager.default.removeItem(at: file) }
        let transport = writableAccount()
        let client = HarvestClient(credentials: Fixture.credentials, transport: transport, backoffScale: 0)
        let queue = MutationQueue(fileURL: file)
        let local = UUID()

        await queue.enqueue(
            .create(local: local, target: target, spentDate: .today(), notes: nil,
                    startedAt: Date(), endedAt: Date())
        )
        let first = await queue.drain(using: client)
        #expect(first.resolved[local] == 636708906)

        // Queued only now — after the create drained, so no rewrite could have run.
        await queue.enqueue(.stop(.local(local), hours: 0.25))
        let second = await queue.drain(using: client)

        #expect(second.applied == 1)
        #expect(second.discarded.isEmpty)
        #expect(await transport.calls(method: "PATCH", containing: "636708906/stop").count == 1)
    }

    @Test("remembers resolutions across a relaunch")
    func resolutionsSurviveRelaunch() async throws {
        let file = temporaryFile()
        defer { try? FileManager.default.removeItem(at: file) }
        let transport = writableAccount()
        let client = HarvestClient(credentials: Fixture.credentials, transport: transport, backoffScale: 0)
        let local = UUID()

        let first = MutationQueue(fileURL: file)
        await first.enqueue(
            .create(local: local, target: target, spentDate: .today(), notes: nil,
                    startedAt: Date(), endedAt: Date())
        )
        _ = await first.drain(using: client)

        let reopened = MutationQueue(fileURL: file)
        await reopened.enqueue(.stop(.local(local), hours: 0.25))
        let report = await reopened.drain(using: client)

        #expect(report.applied == 1)
        #expect(report.discarded.isEmpty)
    }
}
