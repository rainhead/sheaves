import AppKit
import OSLog
import ServiceManagement

/// The part of `SMAppService` this app uses.
///
/// It exists so the preference can be tested: the real thing writes to launchd and
/// would leave a login item behind on whichever machine ran the suite.
@MainActor
protocol LoginItemService {
    var status: SMAppService.Status { get }
    func register() throws
    func unregister() throws
    func openSystemSettings()
}

/// Registers the running app itself, which is all a single-bundle menu bar app needs.
struct MainAppLoginItem: LoginItemService {
    var status: SMAppService.Status { SMAppService.mainApp.status }
    func register() throws { try SMAppService.mainApp.register() }
    func unregister() throws { try SMAppService.mainApp.unregister() }
    func openSystemSettings() { SMAppService.openSystemSettingsLoginItems() }
}

/// Why a login item is not on when the user asked for it to be.
enum LoginItemProblem: Equatable {
    /// macOS is holding the registration: the user turned Sheaves off under Login
    /// Items, and nothing the app can call will turn it back on. Only System
    /// Settings can, so that is what we point at.
    case needsApproval
    case failed(String)
}

/// Whether Sheaves opens at login, kept in step with what macOS actually thinks.
///
/// There is no stored preference here — `SMAppService` *is* the state, and the user
/// can change it from System Settings behind the app's back. Everything reads
/// `status`; `refresh()` picks up a change made elsewhere.
@MainActor
@Observable
final class LoginItemPreference {
    private static let log = Logger(subsystem: "com.rainhead.Sheaves", category: "loginitem")

    private(set) var status: SMAppService.Status
    private(set) var problem: LoginItemProblem?

    var isEnabled: Bool { status == .enabled }

    private let service: any LoginItemService

    init(service: any LoginItemService = MainAppLoginItem()) {
        self.service = service
        self.status = service.status
    }

    /// Re-reads the system's answer, discarding any complaint about an older attempt.
    func refresh() {
        status = service.status
        problem = nil
    }

    func setEnabled(_ enabled: Bool) {
        problem = nil
        var thrown: (any Error)?
        do {
            if enabled {
                try service.register()
            } else {
                try service.unregister()
            }
        } catch {
            thrown = error
        }

        // Ask the system rather than assume: `register()` can return without error
        // and still leave the item waiting on the user, and `status` is the only
        // thing that distinguishes the two.
        status = service.status
        if enabled && status != .enabled {
            problem = status == .requiresApproval
                ? .needsApproval
                : .failed(thrown?.localizedDescription ?? "macOS did not enable it.")
        } else if let thrown {
            problem = .failed(thrown.localizedDescription)
        }

        Self.log.info(
            """
            launch at login set to \(enabled, privacy: .public): \
            status \(self.status.rawValue, privacy: .public)\
            \(thrown.map { " error \($0.localizedDescription)" } ?? "", privacy: .public)
            """
        )
    }

    func openSystemSettings() {
        service.openSystemSettings()
    }
}
