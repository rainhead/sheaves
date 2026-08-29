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

    /// True while anything on the machine is capturing from any input device.
    ///
    /// This is the whole reason Sheaves does not interrupt calls. Harvest's own app
    /// watches keyboard and mouse alone, so an hour of Zoom looks exactly like an
    /// hour at lunch. `kAudioDevicePropertyDeviceIsRunningSomewhere` is a property
    /// of the device rather than of the audio, so reading it needs no microphone
    /// entitlement, prompts for no permission, and never sees any content.
    ///
    /// Every input device is checked, not only the default one: Zoom, Teams and the
    /// rest let you choose a microphone, and a headset or interface picked there
    /// while the system default stays on the built-in mic would leave the call
    /// invisible — precisely the case this exists to handle.
    var isMicrophoneInUse: Bool {
        Self.inputDevices.contains(where: Self.isCapturing)
    }

    /// Read every time rather than cached: devices come and go with the cable.
    private static var inputDevices: [AudioDeviceID] {
        var address = AudioObjectPropertyAddress(
            mSelector: kAudioHardwarePropertyDevices,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain
        )
        var size = UInt32(0)
        guard AudioObjectGetPropertyDataSize(
            AudioObjectID(kAudioObjectSystemObject), &address, 0, nil, &size
        ) == noErr, size > 0 else { return [] }

        var devices = [AudioDeviceID](repeating: 0, count: Int(size) / MemoryLayout<AudioDeviceID>.size)
        guard AudioObjectGetPropertyData(
            AudioObjectID(kAudioObjectSystemObject), &address, 0, nil, &size, &devices
        ) == noErr else { return [] }

        return devices.filter(hasInput)
    }

    /// A device with no input streams is a pair of speakers, and speakers are not
    /// evidence that anybody is here.
    private static func hasInput(_ device: AudioDeviceID) -> Bool {
        var address = AudioObjectPropertyAddress(
            mSelector: kAudioDevicePropertyStreams,
            mScope: kAudioObjectPropertyScopeInput,
            mElement: kAudioObjectPropertyElementMain
        )
        var size = UInt32(0)
        return AudioObjectGetPropertyDataSize(device, &address, 0, nil, &size) == noErr && size > 0
    }

    /// Whether the device is running — which, on a device that both records and
    /// plays, cannot be narrowed to recording.
    ///
    /// `kAudioDevicePropertyDeviceIsRunningSomewhere` ignores the scope it is asked
    /// for: on this machine an input-only microphone reports running under the
    /// *output* scope too, and an output-only speaker reports it under the input
    /// scope, so there is no direction-scoped read to fall back on. Filtering to
    /// devices that can record is as far as it goes, and a duplex device — a USB
    /// interface, or the virtual device a meeting app installs — therefore reads as
    /// presence while it is only playing audio.
    ///
    /// That failure suppresses a prompt rather than raising a false one. The
    /// alternative, ignoring duplex devices, would miss every call made through an
    /// audio interface, which is the case this feature exists for.
    private static func isCapturing(_ device: AudioDeviceID) -> Bool {
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
    /// When the machine was last looked at, so a call found in progress can be
    /// credited to the whole interval it may have been running for.
    private var lastPoll: Date?
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
        defer { lastPoll = now }

        // The microphone is read first, and its evidence is dated to the previous
        // poll rather than to now.
        //
        // Both matter. A call that began at any point since the last look could have
        // begun at the start of that interval, and dating it to `now` would let the
        // gap before it reach the threshold — so joining a call after a quiet
        // stretch of reading would open the prompt *during the call*, which is the
        // one thing this must never do. Reading it before the idle clock means the
        // presence is banked before any gap is measured.
        if sensor.isMicrophoneInUse {
            report(detector.notePresence(at: lastPoll ?? now))
        }

        // The idle clock says how long *since* the last event, which is a better
        // answer than the moment this poll happened to run.
        report(detector.notePresence(at: now.addingTimeInterval(-sensor.secondsSinceInput)))
    }

    private func report(_ absence: Absence?) {
        guard let absence else { return }
        Self.log.info(
            "absence of \(absence.duration / 60, format: .fixed(precision: 1), privacy: .public) minutes ending \(absence.ended, privacy: .public)"
        )
        onAbsence?(absence)
    }
}
