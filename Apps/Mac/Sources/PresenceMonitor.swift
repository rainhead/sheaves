import AppKit
import CoreAudio
import CoreGraphics
import OSLog
import SheavesCore

/// The signals that say somebody is at this Mac.
///
/// A protocol so the rule about what they mean can be tested without a person
/// sitting at a keyboard, or a call in progress.
@MainActor
protocol PresenceSensor {
    /// Seconds since the last keyboard or mouse event.
    var secondsSinceInput: TimeInterval { get }
    /// Whether any app is capturing from the microphone right now.
    var isMicrophoneInUse: Bool { get }
    /// Whether the screen is locked.
    var isScreenLocked: Bool { get }
}

/// The real thing: the window server's idle clock and the audio HAL.
struct SystemPresenceSensor: PresenceSensor {
    var secondsSinceInput: TimeInterval {
        CGEventSource.secondsSinceLastEventType(
            .hidSystemState,
            eventType: CGEventType(rawValue: ~UInt32(0))!
        )
    }

    /// True while anything on the machine holds the default input device.
    ///
    /// This is the whole reason Sheaves does not interrupt calls. Harvest's own app
    /// watches keyboard and mouse alone, so an hour of Zoom looks exactly like an
    /// hour at lunch. `kAudioDevicePropertyDeviceIsRunningSomewhere` is a property
    /// of the device rather than of the audio, so reading it needs no microphone
    /// entitlement, prompts for no permission, and never sees any content.
    var isMicrophoneInUse: Bool {
        guard let device = Self.defaultInputDevice else { return false }
        var address = AudioObjectPropertyAddress(
            mSelector: kAudioDevicePropertyDeviceIsRunningSomewhere,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain
        )
        var running = UInt32(0)
        var size = UInt32(MemoryLayout<UInt32>.size)
        let status = AudioObjectGetPropertyData(device, &address, 0, nil, &size, &running)
        return status == noErr && running != 0
    }

    /// Read every time rather than cached: plugging in headphones changes it.
    private static var defaultInputDevice: AudioDeviceID? {
        var address = AudioObjectPropertyAddress(
            mSelector: kAudioHardwarePropertyDefaultInputDevice,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain
        )
        var device = AudioDeviceID(0)
        var size = UInt32(MemoryLayout<AudioDeviceID>.size)
        let status = AudioObjectGetPropertyData(
            AudioObjectID(kAudioObjectSystemObject), &address, 0, nil, &size, &device
        )
        return status == noErr && device != 0 ? device : nil
    }

    var isScreenLocked: Bool {
        let session = CGSessionCopyCurrentDictionary() as? [String: Any]
        return session?["CGSSessionScreenIsLocked"] as? Bool ?? false
    }
}

/// Feeds the system's presence signals to an `AbsenceDetector` and reports what
/// comes out.
@MainActor
final class PresenceMonitor {
    private static let log = Logger(subsystem: "com.rainhead.Sheaves", category: "presence")

    /// Half a minute is far finer than the fifteen it takes to count as away, and
    /// costs two property reads.
    static let pollInterval: TimeInterval = 30

    var onAbsence: ((Absence) -> Void)?

    private var detector: AbsenceDetector
    private let sensor: any PresenceSensor
    private var timer: Timer?
    private var workspaceObservers: [any NSObjectProtocol] = []
    private var distributedObservers: [any NSObjectProtocol] = []

    init(
        sensor: any PresenceSensor = SystemPresenceSensor(),
        threshold: TimeInterval = AbsenceDetector.defaultThreshold,
        now: Date = Date()
    ) {
        self.sensor = sensor
        self.detector = AbsenceDetector(threshold: threshold, presentAt: now)
    }

    func start() {
        guard timer == nil else { return }
        let timer = Timer.scheduledTimer(withTimeInterval: Self.pollInterval, repeats: true) { _ in
            Task { @MainActor [weak self] in self?.poll() }
        }
        // The panel and the menu run in tracking modes; without this the poll stops
        // while a menu is open.
        RunLoop.main.add(timer, forMode: .common)
        self.timer = timer

        // Waking and unlocking are not needed to *measure* an absence — the gap in
        // evidence does that on its own — but they are when it should be noticed.
        // Without them, coming back to a machine that slept all night would sit
        // silent for up to another poll.
        let workspace = NSWorkspace.shared.notificationCenter
        workspaceObservers = [NSWorkspace.didWakeNotification, NSWorkspace.screensDidWakeNotification].map { name in
            workspace.addObserver(forName: name, object: nil, queue: .main) { _ in
                Task { @MainActor [weak self] in self?.poll() }
            }
        }
        distributedObservers = [
            DistributedNotificationCenter.default().addObserver(
                forName: .init("com.apple.screenIsUnlocked"), object: nil, queue: .main
            ) { _ in
                Task { @MainActor [weak self] in self?.poll() }
            }
        ]
    }

    func stop() {
        timer?.invalidate()
        timer = nil
        for observer in workspaceObservers {
            NSWorkspace.shared.notificationCenter.removeObserver(observer)
        }
        for observer in distributedObservers {
            DistributedNotificationCenter.default().removeObserver(observer)
        }
        workspaceObservers = []
        distributedObservers = []
    }

    /// Folds one reading of the machine into the detector.
    func poll(now: Date = Date()) {
        // A locked screen outranks everything. A call left running on a locked Mac
        // still holds the microphone, and that is somebody's machine sitting in an
        // empty room, not somebody working.
        guard !sensor.isScreenLocked else { return }

        // The idle clock says how long *since* the last event, which is a better
        // answer than the moment this poll happened to run.
        report(detector.notePresence(at: now.addingTimeInterval(-sensor.secondsSinceInput)))

        if sensor.isMicrophoneInUse {
            report(detector.notePresence(at: now))
        }
    }

    private func report(_ absence: Absence?) {
        guard let absence else { return }
        Self.log.info(
            "absence of \(absence.duration / 60, format: .fixed(precision: 1), privacy: .public) minutes ending \(absence.ended, privacy: .public)"
        )
        onAbsence?(absence)
    }
}
