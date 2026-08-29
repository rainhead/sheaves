import Foundation

/// A Harvest personal access token plus the account it addresses.
///
/// One token can reach several Harvest and Forecast accounts, so the account id
/// travels with it — Harvest requires both on every request.
/// Create one at https://id.getharvest.com/developers
public struct HarvestCredentials: Sendable, Hashable, Codable {
    public var accountID: String
    public var token: String

    public init(accountID: String, token: String) {
        self.accountID = accountID.trimmingCharacters(in: .whitespacesAndNewlines)
        self.token = token.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    public var isComplete: Bool { !accountID.isEmpty && !token.isEmpty }
}
