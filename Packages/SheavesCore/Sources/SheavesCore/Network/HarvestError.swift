import Foundation

public enum HarvestError: Error, Sendable, Equatable {
    /// No credentials have been supplied yet.
    case notConfigured
    /// 401/403 — the token or account id is wrong, or the token was revoked.
    case unauthorized
    /// 429 — Harvest allows 100 requests per 15 seconds.
    case rateLimited(retryAfter: TimeInterval)
    case notFound
    /// 422 and friends; `message` is Harvest's own explanation where it gave one.
    case rejected(status: Int, message: String)
    case server(status: Int)
    /// The response was not HTTP at all.
    case invalidResponse
    /// The response was well-formed HTTP that did not match the expected shape.
    /// `detail` names the field and path, which is the whole point of the case.
    case decoding(endpoint: String, detail: String)
}

extension HarvestError: LocalizedError {
    public var errorDescription: String? {
        switch self {
        case .notConfigured:
            "Sheaves isn’t connected to Harvest yet."
        case .unauthorized:
            "Harvest rejected the token. Check the token and account ID."
        case .rateLimited(let retryAfter):
            "Harvest is rate limiting requests. Retrying in \(Int(retryAfter.rounded()).formatted())s."
        case .notFound:
            "That record no longer exists in Harvest."
        case .rejected(_, let message):
            message.isEmpty ? "Harvest rejected the change." : message
        case .server(let status):
            "Harvest returned an error (\(status)). Try again shortly."
        case .invalidResponse:
            "Harvest returned something Sheaves couldn’t read."
        case .decoding(let endpoint, let detail):
            "Couldn’t read Harvest’s \(endpoint) response: \(detail)"
        }
    }

    /// Whether retrying the identical request could plausibly succeed.
    public var isTransient: Bool {
        switch self {
        case .rateLimited, .server, .invalidResponse: true
        // Retrying cannot change a response shape, and for a queued write the change
        // already landed — only reading the reply failed.
        case .notConfigured, .unauthorized, .notFound, .rejected, .decoding: false
        }
    }
}
