import Foundation
import Observation

/// The other machines this Mac shares presence with, by their names on the
/// tailnet, such as `hostess`.
///
/// Before asking about an absence, this Mac asks them whether they saw the user
/// meanwhile; with any named, it also answers them. The rules are in the core
/// (`crates/sheaves-core/src/peers.rs`); this only remembers the names.
@MainActor
@Observable
final class PeerPreference {
    private static let defaultsKey = "peers"

    var names: [String] {
        didSet {
            guard names != oldValue else { return }
            defaults.set(names, forKey: Self.defaultsKey)
            onChange?(names)
        }
    }

    /// Set by whoever hands the names to the tracker.
    @ObservationIgnored var onChange: (([String]) -> Void)?

    @ObservationIgnored private let defaults: UserDefaults

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        names = defaults.stringArray(forKey: Self.defaultsKey) ?? []
    }

    /// The names in what was typed, separated by commas, spaces or new lines.
    static func names(in text: String) -> [String] {
        var names: [String] = []
        for name in text.split(whereSeparator: { $0 == "," || $0.isWhitespace }).map(String.init)
        where !names.contains(name) {
            names.append(name)
        }
        return names
    }
}
