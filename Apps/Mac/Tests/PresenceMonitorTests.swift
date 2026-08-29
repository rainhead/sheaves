import Foundation
import SheavesCore
import Testing
@testable import Sheaves

@MainActor
final class FakeSensor: PresenceSensor {
    var secondsSinceInput: TimeInterval = 0
    var isMicrophoneInUse = false
    var isScreenLocked = false
}

@Suite("PresenceMonitor")
@MainActor
struct PresenceMonitorTests {
    private let start = Date(timeIntervalSince1970: 1_700_000_000)

    private func monitor(_ sensor: FakeSensor) -> (PresenceMonitor, Box) {
        let monitor = PresenceMonitor(sensor: sensor, threshold: 15 * 60, now: start)
        let box = Box()
        monitor.onAbsence = { box.absences.append($0) }
        return (monitor, box)
    }

    final class Box {
        var absences: [Absence] = []
    }

    /// Polls every half minute for `minutes`, with the sensor reporting no input at
    /// all since `start` — the shape of somebody sitting still.
    private func run(_ monitor: PresenceMonitor, _ sensor: FakeSensor, minutes: Double) {
        for tick in stride(from: 30.0, through: minutes * 60, by: 30) {
            sensor.secondsSinceInput = tick
            monitor.poll(now: start + tick)
        }
    }

    @Test("says nothing while someone is typing")
    func quietWhileTyping() {
        let sensor = FakeSensor()
        let (monitor, box) = monitor(sensor)
        for tick in stride(from: 30.0, through: 3600, by: 30) {
            sensor.secondsSinceInput = 5
            monitor.poll(now: start + tick)
        }
        #expect(box.absences.isEmpty)
    }

    @Test("reports the absence when they come back")
    func reportsOnReturn() throws {
        let sensor = FakeSensor()
        let (monitor, box) = monitor(sensor)
        run(monitor, sensor, minutes: 20)
        #expect(box.absences.isEmpty)

        // Back at the keyboard, two seconds ago.
        sensor.secondsSinceInput = 2
        monitor.poll(now: start + 20 * 60 + 2)

        let absence = try #require(box.absences.first)
        #expect(absence.began == start)
        #expect(absence.ended == start + 20 * 60)
    }

    /// The reason this exists. Harvest's own app watches keyboard and mouse alone,
    /// so an hour on a call looks exactly like an hour at lunch and it interrupts to
    /// ask. A machine whose microphone is in use has somebody in front of it.
    @Test("a call counts as being here")
    func microphoneCountsAsPresence() {
        let sensor = FakeSensor()
        sensor.isMicrophoneInUse = true
        let (monitor, box) = monitor(sensor)

        run(monitor, sensor, minutes: 90)

        // An absence only ever surfaces when somebody comes back, so this has to
        // come back to mean anything. Without it the assertion holds even with the
        // microphone ignored entirely, which is the whole claim under test.
        sensor.isMicrophoneInUse = false
        sensor.secondsSinceInput = 1
        monitor.poll(now: start + 90 * 60 + 1)

        #expect(box.absences.isEmpty)
    }

    @Test("the absence starts when the call ended, not when the typing did")
    func absenceBeginsAtEndOfCall() throws {
        let sensor = FakeSensor()
        sensor.isMicrophoneInUse = true
        let (monitor, box) = monitor(sensor)

        run(monitor, sensor, minutes: 30)
        let callEnded = start + 30 * 60
        sensor.isMicrophoneInUse = false
        run2(monitor, sensor, from: 30 * 60 + 30, to: 50 * 60)

        sensor.secondsSinceInput = 1
        monitor.poll(now: start + 50 * 60 + 1)

        let absence = try #require(box.absences.first)
        #expect(absence.began == callEnded)
    }

    private func run2(_ monitor: PresenceMonitor, _ sensor: FakeSensor, from: Double, to: Double) {
        for tick in stride(from: from, through: to, by: 30) {
            sensor.secondsSinceInput = tick
            monitor.poll(now: start + tick)
        }
    }

    /// A meeting left open on a locked Mac still holds the microphone. That is a
    /// machine alone in a room, not somebody working.
    @Test("a locked screen outranks the microphone")
    func lockedScreenBeatsMicrophone() throws {
        let sensor = FakeSensor()
        sensor.isMicrophoneInUse = true
        sensor.isScreenLocked = true
        let (monitor, box) = monitor(sensor)

        run(monitor, sensor, minutes: 40)
        #expect(box.absences.isEmpty)

        sensor.isScreenLocked = false
        sensor.secondsSinceInput = 1
        monitor.poll(now: start + 40 * 60 + 1)

        let absence = try #require(box.absences.first)
        #expect(absence.began == start)
    }

    /// Sleeping needs no special case: the evidence simply stops arriving, and the
    /// gap is measured when the user touches the machine again.
    @Test("an absence over sleep is measured when they touch the keyboard")
    func measuresSleep() throws {
        let sensor = FakeSensor()
        let (monitor, box) = monitor(sensor)

        let overnight = 9.0 * 3600
        sensor.secondsSinceInput = 1
        monitor.poll(now: start + overnight)

        let absence = try #require(box.absences.first)
        #expect(absence.began == start)
        #expect(absence.duration == overnight - 1)
    }

    /// A Mac that wakes on its own overnight — Power Nap, a Time Machine run — has
    /// nobody in front of it, and must not be mistaken for somebody coming back.
    @Test("waking without anyone touching it reports nothing")
    func ignoresUnattendedWake() {
        let sensor = FakeSensor()
        let (monitor, box) = monitor(sensor)

        let overnight = 9.0 * 3600
        sensor.secondsSinceInput = overnight
        monitor.poll(now: start + overnight)

        #expect(box.absences.isEmpty)
    }
}
