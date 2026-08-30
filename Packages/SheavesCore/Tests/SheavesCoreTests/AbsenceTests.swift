import Foundation
import Testing
@testable import SheavesCore

@Suite("AbsenceDetector")
struct AbsenceDetectorTests {
    private let start = Date(timeIntervalSince1970: 1_700_000_000)

    private func detector(threshold: TimeInterval = 15 * 60) -> AbsenceDetector {
        AbsenceDetector(threshold: threshold, presentAt: start)
    }

    @Test("says nothing while someone keeps turning up")
    func quietWhilePresent() {
        var detector = detector()
        for minute in 1...60 {
            #expect(detector.notePresence(at: start + Double(minute) * 60) == nil)
        }
    }

    @Test("reports the gap once it reaches the threshold")
    func reportsAbsence() {
        var detector = detector()
        let back = start + 15 * 60
        #expect(detector.notePresence(at: back) == Absence(began: start, ended: back))
    }

    /// A minute under is someone reading at their desk, and asking about it is the
    /// behaviour that makes people turn idle detection off.
    @Test("stays quiet just under the threshold")
    func ignoresShortGaps() {
        var detector = detector()
        #expect(detector.notePresence(at: start + 15 * 60 - 1) == nil)
    }

    /// The absence ends when they came back, not when the poll happened to run. The
    /// HID clock reports how long since the last event, so the caller can hand over
    /// a moment that has already passed.
    @Test("spans from the last presence to the return, not to the poll")
    func measuresToTheReturn() {
        var detector = detector()
        let cameBack = start + 40 * 60
        let noticed = cameBack + 25
        _ = detector.notePresence(at: cameBack)
        #expect(detector.lastPresence == cameBack)

        // The next poll must not report a second absence for the same stretch.
        #expect(detector.notePresence(at: noticed) == nil)
    }

    @Test("only one absence is reported per stretch away")
    func reportsEachAbsenceOnce() {
        var detector = detector()
        #expect(detector.notePresence(at: start + 3600) != nil)
        #expect(detector.notePresence(at: start + 3601) == nil)
        #expect(detector.notePresence(at: start + 2 * 3600) != nil)
    }

    /// A clock that steps backwards — a time-zone change, an NTP correction — must
    /// not manufacture an absence, and must not rewind the record of presence.
    @Test("ignores evidence older than what it already holds")
    func ignoresBackwardsClock() {
        var detector = detector()
        #expect(detector.notePresence(at: start - 3600) == nil)
        #expect(detector.lastPresence == start)
    }
}

@Suite("Absence.trimmedHours")
struct AbsenceTrimTests {
    private let startedAt = Date(timeIntervalSince1970: 1_700_000_000)

    private func running(bankedHours: Double = 0, startedAt: Date) -> TrackedEntry {
        TrackedEntry(
            id: .server(1),
            project: Reference(id: 1, name: "Online Store"),
            task: Reference(id: 2, name: "Programming"),
            spentDate: .today(),
            bankedHours: bankedHours,
            isRunning: true,
            timerStartedAt: startedAt
        )
    }

    @Test("keeps the time worked before they left")
    func trimsToDeparture() throws {
        let entry = running(startedAt: startedAt)
        let absence = Absence(began: startedAt + 1800, ended: startedAt + 5400)
        #expect(try #require(absence.trimmedHours(for: entry)) == 0.5)
    }

    @Test("keeps hours banked before this run")
    func keepsBankedHours() throws {
        let entry = running(bankedHours: 2, startedAt: startedAt)
        let absence = Absence(began: startedAt + 1800, ended: startedAt + 5400)
        #expect(try #require(absence.trimmedHours(for: entry)) == 2.5)
    }

    /// The rule that keeps this machine from speaking for work done elsewhere: a
    /// timer that started after the absence began has never once run while somebody
    /// was at this Mac, so this Mac knows nothing about it. Started on the web, on a
    /// phone, or through the API all look like this from here.
    @Test("refuses a timer that started after the absence began")
    func refusesTimerStartedElsewhere() {
        let entry = running(startedAt: startedAt + 3600)
        let absence = Absence(began: startedAt, ended: startedAt + 7200)
        #expect(absence.trimmedHours(for: entry) == nil)
    }

    @Test("a timer started at the very moment they left is still ours")
    func acceptsTimerStartedAtDeparture() {
        let entry = running(startedAt: startedAt)
        #expect(Absence(began: startedAt, ended: startedAt + 3600).trimmedHours(for: entry) == 0)
    }

    /// Start a timer, walk to the meeting. The click that started it *is* the last
    /// input, and `timerStartedAt` is read a moment later inside the handler it
    /// triggered — so the last presence precedes the start, and without tolerance
    /// the commonest absence of all would be refused as somebody else's timer.
    @Test("a timer started a moment after the last input is still ours")
    func acceptsTimerStartedJustAfterTheLastInput() throws {
        let entry = running(startedAt: startedAt + 0.05)
        let absence = Absence(began: startedAt, ended: startedAt + 3600)
        #expect(try #require(absence.trimmedHours(for: entry)) == 0)
    }

    @Test("the absence belongs to the day it happened on, not the timer's day")
    func dayComesFromTheAbsence() {
        let calendar = Calendar(identifier: .gregorian)
        let absence = Absence(began: startedAt, ended: startedAt + 3600)
        #expect(absence.day(in: calendar) == CalendarDate(startedAt, in: calendar))
    }

    @Test("refuses an entry that is not running")
    func refusesStoppedEntry() {
        var entry = running(startedAt: startedAt)
        entry.isRunning = false
        entry.timerStartedAt = nil
        #expect(Absence(began: startedAt + 60, ended: startedAt + 3600).trimmedHours(for: entry) == nil)
    }
}

/// The fixture timer starts at 14:30Z with an hour already banked, so an absence
/// from 15:00Z leaves exactly 1.5 hours worked. Fixed instants keep the arithmetic
/// out of the assertions.
@Suite("Resolving an absence")
@MainActor
struct AbsenceResolutionTests {
    private func instant(_ iso: String) -> Date {
        ISO8601DateFormatter().date(from: iso)!
    }

    private var absence: Absence {
        Absence(began: instant("2026-08-29T15:00:00Z"), ended: instant("2026-08-29T15:30:00Z"))
    }

    private var meeting: TimerTarget {
        TimerTarget(
            project: Reference(id: 14308069, name: "Online Store - Phase 1"),
            task: Reference(id: 8083366, name: "Programming"),
            client: Reference(id: 5735776, name: "123 Industries")
        )
    }

    private func runningAccount() -> RoutingTransport {
        let running = Fixture.timeEntriesPage([Fixture.runningTimeEntry])
        return RoutingTransport([
            RoutingTransport.Route(method: "GET", fragment: "users/me/project_assignments", body: Fixture.projectAssignmentsPage),
            RoutingTransport.Route(method: "GET", fragment: "users/me", body: Fixture.currentUser),
            RoutingTransport.Route(method: "GET", fragment: "company", body: Fixture.company),
            RoutingTransport.Route(method: "PATCH", fragment: "/stop", body: Fixture.timeEntry),
            RoutingTransport.Route(method: "PATCH", fragment: "/restart", body: Fixture.runningTimeEntry),
            RoutingTransport.Route(method: "POST", fragment: "time_entries", body: Fixture.timeEntry, status: 201),
            RoutingTransport.Route(method: "PATCH", fragment: "time_entries", body: Fixture.timeEntry),
            RoutingTransport.Route(method: "GET", fragment: "is_running=true", body: running),
            RoutingTransport.Route(method: "GET", fragment: "time_entries", body: running),
        ])
    }

    private func trackerWithRunningTimer(
        _ transport: RoutingTransport
    ) async throws -> (TimeTracker, TrackedEntry, [URL]) {
        let snapshot = URL.temporaryDirectory.appending(path: "sheaves-absence-\(UUID().uuidString).json")
        let queue = URL.temporaryDirectory.appending(path: "sheaves-absence-q-\(UUID().uuidString).json")
        let tracker = TimeTracker(
            client: HarvestClient(credentials: Fixture.credentials, transport: transport, backoffScale: 0),
            keychain: KeychainStore(service: "com.rainhead.Sheaves.tests-\(UUID().uuidString)"),
            snapshots: SnapshotStore(fileURL: snapshot),
            queue: MutationQueue(fileURL: queue)
        )
        await tracker.sync()
        let running = try #require(tracker.runningEntry)
        return (tracker, running, [snapshot, queue])
    }

    private func cleanUp(_ urls: [URL]) {
        for url in urls { try? FileManager.default.removeItem(at: url) }
    }

    @Test("keeping the time sends nothing at all")
    func keepIsSilent() async throws {
        let transport = runningAccount()
        let (tracker, running, files) = try await trackerWithRunningTimer(transport)
        defer { cleanUp(files) }

        await tracker.resolve(absence, on: running, as: .keep)

        #expect(await transport.calls(method: "PATCH", containing: "time_entries").isEmpty)
        #expect(await transport.calls(method: "POST", containing: "time_entries").isEmpty)
    }

    @Test("stopping sends the hours worked, not the hours elapsed")
    func trimAndStopSendsTrimmedHours() async throws {
        let transport = runningAccount()
        let (tracker, running, files) = try await trackerWithRunningTimer(transport)
        defer { cleanUp(files) }

        await tracker.resolve(absence, on: running, as: .trimAndStop)

        #expect(await transport.calls(method: "PATCH", containing: "/stop").count == 1)
        // `/stop` and `/restart` are paths under time_entries too, so pick out the
        // calls that actually carried an hours correction.
        let corrections = await transport.calls(method: "PATCH", containing: "time_entries").compactMap(\.hours)
        #expect(corrections == [1.5])
        #expect(await transport.calls(method: "PATCH", containing: "/restart").isEmpty)
    }

    /// Harvest's own copy of the timer is still running and still counting the
    /// absence, so continuing has to stop it, correct the total, and start it again.
    /// Leaving out the stop would let Harvest keep the hours nobody worked.
    @Test("continuing stops, corrects the total, then restarts")
    func trimAndContinueStopsCorrectsRestarts() async throws {
        let transport = runningAccount()
        let (tracker, running, files) = try await trackerWithRunningTimer(transport)
        defer { cleanUp(files) }

        await tracker.resolve(absence, on: running, as: .trimAndContinue)

        #expect(await transport.calls(method: "PATCH", containing: "/stop").count == 1)
        // The first correction is the trimmed total. A second follows here only
        // because the fixture's return time is hours in the past, so the queue banks
        // the stretch since; in life the two are seconds apart.
        let corrections = await transport.calls(method: "PATCH", containing: "time_entries").compactMap(\.hours)
        #expect(corrections.first == 1.5)
        #expect(await transport.calls(method: "PATCH", containing: "/restart").count == 1)
    }

    @Test("logging the absence elsewhere creates an entry for exactly that stretch")
    func trimAndLogCreatesTheIdleEntry() async throws {
        let transport = runningAccount()
        let (tracker, running, files) = try await trackerWithRunningTimer(transport)
        defer { cleanUp(files) }

        await tracker.resolve(absence, on: running, as: .trimAndLog(meeting))

        let created = try #require(await transport.calls(method: "POST", containing: "time_entries").first)
        #expect(created.hours == 0.5)
        #expect(created.projectID == meeting.project.id)
        // The absence's own day. Taking it from the interrupted timer would put this
        // morning's meeting on yesterday's timesheet whenever a timer ran overnight.
        #expect(created.spentDate == absence.day().description)
        // The timer it was taken from keeps running.
        #expect(await transport.calls(method: "PATCH", containing: "/restart").count == 1)
    }

    /// A timer this machine has never seen running while anyone was here — started
    /// on the web or a phone — must be left alone entirely.
    @Test("a timer started after the absence began is left untouched")
    func refusesTimerStartedElsewhere() async throws {
        let transport = runningAccount()
        let (tracker, running, files) = try await trackerWithRunningTimer(transport)
        defer { cleanUp(files) }

        let earlier = Absence(
            began: instant("2026-08-29T14:00:00Z"),
            ended: instant("2026-08-29T15:30:00Z")
        )
        await tracker.resolve(earlier, on: running, as: .trimAndStop)

        #expect(await transport.calls(method: "PATCH", containing: "/stop").isEmpty)
        #expect(await transport.calls(method: "PATCH", containing: "time_entries").isEmpty)
        #expect(await transport.calls(method: "POST", containing: "time_entries").isEmpty)
    }
}

/// A create still sitting in the queue reports its hours as the span from its start,
/// which silently swallows any pause taken before the queue drained. Trimming one
/// therefore sends the measured total explicitly rather than trusting that span.
@Suite("Trimming a create that never reached Harvest")
struct AbsenceAmendedCreateTests {
    private func temporaryFile() -> URL {
        URL.temporaryDirectory.appending(path: "sheaves-amend-\(UUID().uuidString).json")
    }

    @Test("sends the hours measured, not the span the create covers")
    func correctsTheSpan() async throws {
        let file = temporaryFile()
        defer { try? FileManager.default.removeItem(at: file) }
        let transport = RoutingTransport([
            RoutingTransport.Route(method: "POST", fragment: "time_entries", body: Fixture.timeEntry, status: 201),
            RoutingTransport.Route(method: "PATCH", fragment: "time_entries", body: Fixture.timeEntry),
        ])
        let client = HarvestClient(credentials: Fixture.credentials, transport: transport, backoffScale: 0)
        let queue = MutationQueue(fileURL: file)
        let local = UUID()
        let target = TimerTarget(
            project: Reference(id: 14308069, name: "Online Store - Phase 1"),
            task: Reference(id: 8083366, name: "Programming")
        )

        // Worked 09:00–09:30, paused, back at 10:00, away from 10:20: fifty minutes.
        let start = Date(timeIntervalSince1970: 1_700_000_000)
        await queue.enqueue(
            .create(
                local: local, target: target, spentDate: .today(), notes: nil,
                startedAt: start, endedAt: nil
            )
        )
        #expect(await queue.amendCreate(local: local, endedAt: start + 80 * 60))
        await queue.enqueue(.update(.local(local), notes: nil, hours: 50.0 / 60))

        let report = await queue.drain(using: client)
        #expect(report.applied == 2)

        // The create still reports the whole span, pause included, rounded to the
        // two decimals Harvest keeps…
        let created = try #require(await transport.calls(method: "POST", containing: "time_entries").first)
        #expect(created.hours == 1.33)
        // …so the total that stands has to be the one that was measured.
        let corrections = await transport.calls(method: "PATCH", containing: "time_entries").compactMap(\.hours)
        #expect(corrections == [0.83])
    }
}
