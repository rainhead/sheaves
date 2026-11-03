import Foundation
import Testing
@testable import Sheaves

@Suite("PeerPreference")
@MainActor
struct PeerPreferenceTests {
    private func defaults() -> UserDefaults {
        let name = "PeerPreferenceTests-\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: name)!
        defaults.removePersistentDomain(forName: name)
        return defaults
    }

    @Test("reads names separated by commas, spaces or new lines, once each")
    func readsNames() {
        #expect(PeerPreference.names(in: "hostess, peters-macbook-air\nhostess  ") == ["hostess", "peters-macbook-air"])
        #expect(PeerPreference.names(in: " , ").isEmpty)
    }

    @Test("remembers the names, and says when they change")
    func remembers() {
        let defaults = defaults()
        let preference = PeerPreference(defaults: defaults)
        #expect(preference.names.isEmpty)
        var heard: [[String]] = []
        preference.onChange = { heard.append($0) }

        preference.names = ["hostess"]
        preference.names = ["hostess"]

        #expect(heard == [["hostess"]])
        #expect(PeerPreference(defaults: defaults).names == ["hostess"])
    }
}
