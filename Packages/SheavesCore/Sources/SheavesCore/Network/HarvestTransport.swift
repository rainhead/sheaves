import Foundation

/// The seam between the client and the network, so tests can answer requests
/// without a live Harvest account.
public protocol HarvestTransport: Sendable {
    func send(_ request: URLRequest) async throws -> (Data, HTTPURLResponse)
}

public struct URLSessionTransport: HarvestTransport {
    private let session: URLSession

    public init(session: URLSession = .shared) {
        self.session = session
    }

    public func send(_ request: URLRequest) async throws -> (Data, HTTPURLResponse) {
        let (data, response) = try await session.data(for: request)
        guard let http = response as? HTTPURLResponse else {
            throw HarvestError.invalidResponse
        }
        return (data, http)
    }
}
