import Foundation
import Testing
@testable import SheavesCore

@Suite("MutationQueue")
struct MutationQueueTests {
    private func temporaryFile() -> URL {
        URL.temporaryDirectory.appending(path: "sheaves-queue-\(UUID().uuidString).json")
    }

    private var target: TimerTarget {
        TimerTarget(
            project: Reference(id: 14308069, name: "Online Store - Phase 1"),
            task: Reference(id: 8083366, name: "Programming"),
            client: Reference(id: 5735776, name: "123 Industries")
        )
    }

    @Test("drains in order and empties itself")
    func drainsInOrder() async throws {
        let file = temporaryFile()
        defer { try? FileManager.default.removeItem(at: file) }
        let queue = MutationQueue(fileURL: file)
        let transport = StubTransport([
            .init(body: Fixture.timeEntry),
            .init(body: Fixture.timeEntry),
            .init(body: Fixture.timeEntry),
        ])
        let client = HarvestClient(credentials: Fixture.credentials, transport: transport, backoffScale: 0)

        await queue.enqueue(.update(.server(636709355), notes: "first", hours: nil))
        await queue.enqueue(.stop(.server(636709355), hours: 1))
        let report = await queue.drain(using: client)

        #expect(report.applied == 2)
        #expect(await queue.isEmpty)
        #expect(await transport.request(at: 0).url?.path() == "/v2/time_entries/636709355")
        #expect(await transport.request(at: 1).url?.path() == "/v2/time_entries/636709355/stop")
    }

    /// Starting a timer offline and stopping it before reconnecting: the stop refers
    /// to an entry Harvest has never seen, so the queue has to repoint it once the
    /// create returns a real id.
    @Test("repoints later mutations at the id a queued create earns")
    func rewritesLocalIdentifiers() async throws {
        let file = temporaryFile()
        defer { try? FileManager.default.removeItem(at: file) }
        let queue = MutationQueue(fileURL: file)
        let transport = StubTransport([
            .init(status: 201, body: Fixture.runningTimeEntry),  // create
            .init(body: Fixture.timeEntry),                      // stop
            .init(body: Fixture.timeEntry),                      // hours correction
        ])
        let client = HarvestClient(credentials: Fixture.credentials, transport: transport, backoffScale: 0)

        let local = UUID()
        await queue.enqueue(
            .create(local: local, target: target, spentDate: .today(), notes: nil,
                    startedAt: Date(), endedAt: Date())
        )
        await queue.enqueue(.stop(.local(local), hours: 0.5))
        let report = await queue.drain(using: client)

        #expect(report.applied == 2)
        // 636708906 is the id the stubbed create returned.
        #expect(await transport.request(at: 1).url?.path() == "/v2/time_entries/636708906/stop")
    }

    /// A dropped connection must not lose the change; it stays queued for next time.
    @Test("stops at a retryable failure and keeps the rest queued")
    func haltsOnTransientFailure() async throws {
        let file = temporaryFile()
        defer { try? FileManager.default.removeItem(at: file) }
        let queue = MutationQueue(fileURL: file)
        let transport = StubTransport([
            .init(status: 500, body: ""),
            .init(status: 500, body: ""),
            .init(status: 500, body: ""),
            .init(status: 500, body: ""),
        ])
        let client = HarvestClient(credentials: Fixture.credentials, transport: transport, backoffScale: 0)

        await queue.enqueue(.stop(.server(1), hours: 1))
        await queue.enqueue(.stop(.server(2), hours: 1))
        let report = await queue.drain(using: client)

        #expect(report.applied == 0)
        #expect(report.stoppedWith?.isTransient == true)
        #expect(await queue.count == 2)
    }

    /// A change Harvest will never accept — a locked entry, a deleted project — would
    /// otherwise wedge every later change behind it forever.
    @Test("discards a refused mutation and lets the queue continue")
    func discardsPermanentFailure() async throws {
        let file = temporaryFile()
        defer { try? FileManager.default.removeItem(at: file) }
        let queue = MutationQueue(fileURL: file)
        let transport = StubTransport([
            .init(status: 422, body: #"{"message": "Time entry is locked."}"#),
            .init(body: Fixture.timeEntry),
            .init(body: Fixture.timeEntry),
        ])
        let client = HarvestClient(credentials: Fixture.credentials, transport: transport, backoffScale: 0)

        await queue.enqueue(.update(.server(1), notes: "nope", hours: nil))
        await queue.enqueue(.stop(.server(2), hours: 1))
        let report = await queue.drain(using: client)

        #expect(report.applied == 1)
        #expect(report.discarded.count == 1)
        #expect(await queue.isEmpty)
    }

    @Test("survives being reloaded from disk")
    func persistsAcrossLaunches() async throws {
        let file = temporaryFile()
        defer { try? FileManager.default.removeItem(at: file) }

        let first = MutationQueue(fileURL: file)
        await first.enqueue(.stop(.server(636709355), hours: 1))
        #expect(await first.count == 1)

        let reopened = MutationQueue(fileURL: file)
        #expect(await reopened.count == 1)

        await reopened.removeAll()
        #expect(await MutationQueue(fileURL: file).isEmpty)
    }
}

@Suite("TrackedEntry")
struct TrackedEntryTests {
    private func entry(banked: Double, running: Bool, startedAt: Date?) -> TrackedEntry {
        TrackedEntry(
            id: .server(1),
            project: Reference(id: 1, name: "Project"),
            task: Reference(id: 2, name: "Task"),
            spentDate: .today(),
            bankedHours: banked,
            isRunning: running,
            timerStartedAt: startedAt
        )
    }

    @Test("counts a running timer forward from when it started")
    func countsRunningTime() {
        let started = Date()
        let tracked = entry(banked: 1.0, running: true, startedAt: started)
        #expect(tracked.hours(asOf: started.addingTimeInterval(1800)) == 1.5)
    }

    @Test("reports only banked hours when stopped")
    func stoppedEntry() {
        let tracked = entry(banked: 2.25, running: false, startedAt: nil)
        #expect(tracked.hours(asOf: Date().addingTimeInterval(3600)) == 2.25)
    }

    /// Clock changes can put `now` behind `timerStartedAt`; a negative duration in the
    /// menu bar would be worse than a stalled one.
    @Test("never reports a negative duration")
    func clampsBackwardsClock() {
        let started = Date()
        let tracked = entry(banked: 1.0, running: true, startedAt: started)
        #expect(tracked.hours(asOf: started.addingTimeInterval(-600)) == 1.0)
    }

    /// Harvest's `hours` on a running entry is a snapshot; `hours_without_timer` is the
    /// part that is safe to add live elapsed time to.
    @Test("banks the timer-free hours when converting a running Harvest entry")
    func convertsRunningEntry() throws {
        let data = Data(Fixture.runningTimeEntry.utf8)
        let entry = try HarvestClient.decoder.decode(TimeEntry.self, from: data)
        let tracked = TrackedEntry(entry)

        #expect(tracked.isRunning)
        #expect(tracked.bankedHours == 1.0)
        #expect(tracked.id == .server(636708906))
    }

    @Test("keeps the recorded hours when converting a stopped entry")
    func convertsStoppedEntry() throws {
        let data = Data(Fixture.timeEntry.utf8)
        let entry = try HarvestClient.decoder.decode(TimeEntry.self, from: data)
        let tracked = TrackedEntry(entry)

        #expect(!tracked.isRunning)
        #expect(tracked.bankedHours == 2.11)
        #expect(tracked.hours() == 2.11)
    }
}
