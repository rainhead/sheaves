import AppKit
import Carbon.HIToolbox

/// A system-wide hotkey, registered through Carbon's `RegisterEventHotKey`.
///
/// Carbon is the only API that catches a key combination while another app is
/// frontmost without asking the user for Accessibility access — `NSEvent`'s global
/// monitors need that permission and still cannot swallow the event.
@MainActor
final class GlobalHotKey {
    // Carbon's opaque pointer is not Sendable, and `deinit` is not actor-isolated;
    // the pointer is only ever touched from the main actor and by `deinit`, which
    // by definition runs when nothing else holds this object.
    private nonisolated(unsafe) var reference: EventHotKeyRef?
    private let identifier: UInt32

    /// - Parameters:
    ///   - keyCode: A virtual key code, e.g. `kVK_ANSI_T`.
    ///   - modifiers: Carbon modifier mask, e.g. `cmdKey | optionKey | controlKey`.
    init?(keyCode: UInt32, modifiers: UInt32, action: @escaping @Sendable () -> Void) {
        identifier = HotKeyRegistry.shared.register(action)
        HotKeyRegistry.shared.installHandlerIfNeeded()

        let hotKeyID = EventHotKeyID(signature: HotKeyRegistry.signature, id: identifier)
        var reference: EventHotKeyRef?
        let status = RegisterEventHotKey(
            keyCode,
            modifiers,
            hotKeyID,
            GetEventDispatcherTarget(),
            0,
            &reference
        )
        guard status == noErr, let reference else {
            HotKeyRegistry.shared.unregister(identifier)
            return nil
        }
        self.reference = reference
    }

    deinit {
        if let reference {
            UnregisterEventHotKey(reference)
        }
        HotKeyRegistry.shared.unregister(identifier)
    }
}

/// Bridges Carbon's C callback back to Swift closures.
///
/// The callback fires on the main thread but is not visible to the compiler as
/// isolated, so the registry guards its table with a lock and hops explicitly.
private final class HotKeyRegistry: @unchecked Sendable {
    static let shared = HotKeyRegistry()
    static let signature: OSType = 0x53_48_56_53  // 'SHVS'

    private let lock = NSLock()
    private var actions: [UInt32: @Sendable () -> Void] = [:]
    private var nextID: UInt32 = 1
    private var handler: EventHandlerRef?

    func register(_ action: @escaping @Sendable () -> Void) -> UInt32 {
        lock.lock()
        defer { lock.unlock() }
        let id = nextID
        nextID += 1
        actions[id] = action
        return id
    }

    func unregister(_ id: UInt32) {
        lock.lock()
        defer { lock.unlock() }
        actions[id] = nil
    }

    func fire(_ id: UInt32) {
        lock.lock()
        let action = actions[id]
        lock.unlock()
        guard let action else { return }
        DispatchQueue.main.async { action() }
    }

    func installHandlerIfNeeded() {
        lock.lock()
        defer { lock.unlock() }
        guard handler == nil else { return }

        var eventType = EventTypeSpec(
            eventClass: OSType(kEventClassKeyboard),
            eventKind: UInt32(kEventHotKeyPressed)
        )
        InstallEventHandler(
            GetEventDispatcherTarget(),
            { _, event, _ -> OSStatus in
                var hotKeyID = EventHotKeyID()
                let status = GetEventParameter(
                    event,
                    EventParamName(kEventParamDirectObject),
                    EventParamType(typeEventHotKeyID),
                    nil,
                    MemoryLayout<EventHotKeyID>.size,
                    nil,
                    &hotKeyID
                )
                guard status == noErr else { return status }
                HotKeyRegistry.shared.fire(hotKeyID.id)
                return noErr
            },
            1,
            &eventType,
            nil,
            &handler
        )
    }
}
