import ServiceManagement
import Testing
@testable import Sheaves

/// A stand-in for `SMAppService.mainApp`.
///
/// The real one writes to launchd, so a suite that used it would leave a login item
/// behind on whichever machine ran the tests — and would pass or fail depending on
/// what that machine had already been told.
@MainActor
final class FakeLoginItemService: LoginItemService {
    /// What each call leaves the system in, recorded the way the app reads it back.
    enum Call: Equatable {
        case register
        case unregister
        case openSystemSettings
    }

    var status: SMAppService.Status
    /// The status the system reports once `register()` has been called, which is not
    /// always the one that was asked for.
    var statusAfterRegister: SMAppService.Status = .enabled
    var registerError: (any Error)?
    var unregisterError: (any Error)?
    private(set) var calls: [Call] = []

    init(status: SMAppService.Status = .notRegistered) {
        self.status = status
    }

    func register() throws {
        calls.append(.register)
        if let registerError {
            status = statusAfterRegister
            throw registerError
        }
        status = statusAfterRegister
    }

    func unregister() throws {
        calls.append(.unregister)
        if let unregisterError { throw unregisterError }
        status = .notRegistered
    }

    func openSystemSettings() {
        calls.append(.openSystemSettings)
    }
}

private struct Denied: LocalizedError {
    var errorDescription: String? { "Operation not permitted"  }
}

@Suite("LoginItemPreference")
@MainActor
struct LoginItemPreferenceTests {
    @Test("reads the system's answer rather than a stored preference")
    func startsFromTheSystem() {
        #expect(LoginItemPreference(service: FakeLoginItemService(status: .enabled)).isEnabled)
        #expect(LoginItemPreference(service: FakeLoginItemService(status: .notRegistered)).isEnabled == false)
        #expect(LoginItemPreference(service: FakeLoginItemService(status: .requiresApproval)).isEnabled == false)
    }

    @Test("turning it on registers the app")
    func enables() {
        let service = FakeLoginItemService()
        let preference = LoginItemPreference(service: service)

        preference.setEnabled(true)

        #expect(service.calls == [.register])
        #expect(preference.isEnabled)
        #expect(preference.problem == nil)
    }

    @Test("turning it off unregisters the app")
    func disables() {
        let service = FakeLoginItemService(status: .enabled)
        let preference = LoginItemPreference(service: service)

        preference.setEnabled(false)

        #expect(service.calls == [.unregister])
        #expect(preference.isEnabled == false)
        #expect(preference.problem == nil)
    }

    /// The user turned Sheaves off under Login Items. `register()` cannot undo that,
    /// so the switch has to stay off and say who can.
    @Test("a registration the user has denied asks them to clear it")
    func reportsApproval() {
        let service = FakeLoginItemService(status: .requiresApproval)
        service.registerError = Denied()
        service.statusAfterRegister = .requiresApproval
        let preference = LoginItemPreference(service: service)

        preference.setEnabled(true)

        #expect(preference.isEnabled == false)
        #expect(preference.problem == .needsApproval)
    }

    /// `register()` can return without complaint and still not enable the item; the
    /// status is the only thing that knows.
    @Test("a silent failure to enable is still a failure")
    func reportsSilentFailure() {
        let service = FakeLoginItemService()
        service.statusAfterRegister = .notFound
        let preference = LoginItemPreference(service: service)

        preference.setEnabled(true)

        #expect(preference.isEnabled == false)
        #expect(preference.problem != nil)
        #expect(preference.problem != .needsApproval)
    }

    @Test("a thrown error is shown as macOS worded it")
    func reportsThrownError() {
        let service = FakeLoginItemService()
        service.registerError = Denied()
        service.statusAfterRegister = .notRegistered
        let preference = LoginItemPreference(service: service)

        preference.setEnabled(true)

        #expect(preference.problem == .failed("Operation not permitted"))
    }

    @Test("a failure to turn it off is reported too")
    func reportsUnregisterFailure() {
        let service = FakeLoginItemService(status: .enabled)
        service.unregisterError = Denied()
        let preference = LoginItemPreference(service: service)

        preference.setEnabled(false)

        #expect(preference.problem == .failed("Operation not permitted"))
        #expect(preference.isEnabled)
    }

    /// System Settings can change this behind the app's back.
    @Test("picks up a change made outside the app")
    func refreshesFromTheSystem() {
        let service = FakeLoginItemService(status: .enabled)
        let preference = LoginItemPreference(service: service)
        #expect(preference.isEnabled)

        service.status = .requiresApproval
        preference.refresh()

        #expect(preference.isEnabled == false)
        #expect(preference.problem == nil)
    }

    @Test("a refresh clears a complaint about an earlier attempt")
    func refreshClearsProblem() {
        let service = FakeLoginItemService(status: .requiresApproval)
        service.statusAfterRegister = .requiresApproval
        let preference = LoginItemPreference(service: service)
        preference.setEnabled(true)
        #expect(preference.problem == .needsApproval)

        service.status = .enabled
        preference.refresh()

        #expect(preference.problem == nil)
        #expect(preference.isEnabled)
    }

    @Test("hands the user off to Login Items")
    func opensSystemSettings() {
        let service = FakeLoginItemService()
        LoginItemPreference(service: service).openSystemSettings()
        #expect(service.calls == [.openSystemSettings])
    }
}
