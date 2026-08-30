import Foundation

/// A typed client for the Harvest V2 API.
///
/// Harvest allows 100 requests per 15 seconds and answers 429 with `Retry-After`;
/// the client honours that header and retries rather than surfacing the failure,
/// so callers only see a `rateLimited` error once retries are exhausted.
public actor HarvestClient {
    public static let baseURL = URL(string: "https://api.harvestapp.com/v2/")!
    /// Harvest returns 400 unless a request identifies its client.
    public static let userAgent = "Sheaves (https://github.com/rainhead/sheaves)"

    private let transport: any HarvestTransport
    private var credentials: HarvestCredentials?
    private let maxRetries = 3
    /// Scales the wait between retries. Tests set it to zero; nothing else should.
    private let backoffScale: Double

    public init(
        credentials: HarvestCredentials? = nil,
        transport: any HarvestTransport = URLSessionTransport(),
        backoffScale: Double = 1
    ) {
        self.credentials = credentials
        self.transport = transport
        self.backoffScale = backoffScale
    }

    public func setCredentials(_ credentials: HarvestCredentials?) {
        self.credentials = credentials
    }

    public var isConfigured: Bool { credentials?.isComplete == true }

    // MARK: - Account

    public func currentUser() async throws -> HarvestUser {
        try await get("users/me")
    }

    public func company() async throws -> HarvestCompany {
        try await get("company")
    }

    /// Every project/task pair the current user is allowed to log time against.
    public func projectAssignments() async throws -> [ProjectAssignment] {
        try await getAllPages("users/me/project_assignments")
    }

    // MARK: - Time entries

    public func timeEntries(
        userID: Int,
        from: CalendarDate,
        to: CalendarDate
    ) async throws -> [TimeEntry] {
        try await getAllPages("time_entries", query: [
            "user_id": String(userID),
            "from": from.description,
            "to": to.description,
        ])
    }

    /// The user's running timer, if any. Harvest permits at most one.
    public func runningTimeEntry(userID: Int) async throws -> TimeEntry? {
        let page: Page<TimeEntry> = try await get("time_entries", query: [
            "user_id": String(userID),
            "is_running": "true",
        ])
        return page.items.first
    }

    /// Creates a time entry.
    ///
    /// Omitting `hours` starts the timer running: that is what an absent `hours`
    /// means on duration-tracking accounts, and an absent `ended_time` on timestamp
    /// accounts, so one body covers both. Passing `hours` creates a stopped entry of
    /// exactly that length, which is how work done offline is recorded accurately
    /// rather than being measured from whenever the request happens to arrive.
    public func createTimeEntry(
        projectID: Int,
        taskID: Int,
        spentDate: CalendarDate = .today(),
        notes: String? = nil,
        hours: Double? = nil
    ) async throws -> TimeEntry {
        try await send(
            "time_entries",
            method: "POST",
            body: CreateTimeEntryPayload(
                projectId: projectID,
                taskId: taskID,
                spentDate: spentDate,
                notes: notes,
                hours: hours
            )
        )
    }

    public func stopTimeEntry(id: Int64) async throws -> TimeEntry {
        try await send("time_entries/\(id)/stop", method: "PATCH", body: EmptyBody())
    }

    public func restartTimeEntry(id: Int64) async throws -> TimeEntry {
        try await send("time_entries/\(id)/restart", method: "PATCH", body: EmptyBody())
    }

    public func updateTimeEntry(
        id: Int64,
        notes: String? = nil,
        hours: Double? = nil,
        projectID: Int? = nil,
        taskID: Int? = nil,
        spentDate: CalendarDate? = nil
    ) async throws -> TimeEntry {
        try await send(
            "time_entries/\(id)",
            method: "PATCH",
            body: UpdateTimeEntryPayload(
                projectId: projectID,
                taskId: taskID,
                spentDate: spentDate,
                notes: notes,
                hours: hours
            )
        )
    }

    public func deleteTimeEntry(id: Int64) async throws {
        _ = try await perform(request(path: "time_entries/\(id)", method: "DELETE"))
    }

    /// Clients, for their currencies.
    ///
    /// Readable only by an administrator or a manager who may edit clients; anyone
    /// else gets 403. That is nearly the same permission a monetary budget already
    /// requires, so a token that can see money can usually also see what kind.
    public func clients() async throws -> [ClientRecord] {
        try await getAllPages("clients")
    }

    // MARK: - Reports

    /// Every project's budget, spend and remainder in one request.
    ///
    /// This is the Reports API, which allows 100 requests per 15 *minutes* — sixty
    /// times tighter than the 100 per 15 seconds the rest of Harvest gives. The
    /// client does not pace this; the caller must.
    ///
    /// The answer can be empty of anything useful: a project with `budget_by` of
    /// `none` has no budget, and a monetary budget is readable only by an
    /// administrator or by a manager with the billable-rates permission. Callers
    /// should ask `hasReadableBudget` rather than assume a row carries figures.
    public func projectBudgets() async throws -> [ProjectBudget] {
        try await getAllPages("reports/project_budget")
    }

    // MARK: - Request plumbing

    private func get<T: Decodable & Sendable>(_ path: String, query: [String: String] = [:]) async throws -> T {
        let data = try await perform(request(path: path, query: query))
        return try Self.decode(T.self, from: data, endpoint: path)
    }

    /// Follows Harvest's own `links.next` rather than counting pages.
    ///
    /// Harvest's documentation is explicit that pagination URLs should be followed
    /// rather than constructed, and the reason bites here: a cursor-paginated response
    /// returns null for `page`, `next_page` and `previous_page` on every page but the
    /// first and last. Walking page numbers therefore stops after one page and drops
    /// the rest without any error to notice. Page numbers remain the fallback, for a
    /// response that carries no links.
    private func getAllPages<Item: PaginatedItem>(
        _ path: String,
        query: [String: String] = [:]
    ) async throws -> [Item] {
        var query = query
        query["per_page"] = "2000"
        query["page"] = "1"

        var collected: [Item] = []
        var next: URL? = try Self.url(path: path, query: query)
        // A malformed or looping `links.next` would otherwise fetch forever.
        var visited: Set<URL> = []

        while let url = next, visited.insert(url).inserted {
            let data = try await perform(request(url: url))
            let envelope = try Self.decode(Page<Item>.self, from: data, endpoint: path)
            collected.append(contentsOf: envelope.items)

            if let link = envelope.nextLink {
                next = link
            } else if let page = envelope.nextPage {
                var query = query
                query["page"] = String(page)
                next = try Self.url(path: path, query: query)
            } else {
                next = nil
            }
        }
        return collected
    }

    private func send<Body: Encodable & Sendable, T: Decodable & Sendable>(
        _ path: String,
        method: String,
        body: Body
    ) async throws -> T {
        var request = try request(path: path, method: method)
        request.httpBody = try Self.encoder.encode(body)
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        let data = try await perform(request)
        return try Self.decode(T.self, from: data, endpoint: path)
    }

    static func url(path: String, query: [String: String] = [:]) throws -> URL {
        guard var components = URLComponents(
            url: baseURL.appending(path: path),
            resolvingAgainstBaseURL: false
        ) else { throw HarvestError.invalidResponse }
        if !query.isEmpty {
            components.queryItems = query
                .sorted { $0.key < $1.key }
                .map { URLQueryItem(name: $0.key, value: $0.value) }
        }
        guard let url = components.url else { throw HarvestError.invalidResponse }
        return url
    }

    private func request(path: String, method: String = "GET", query: [String: String] = [:]) throws -> URLRequest {
        try request(url: Self.url(path: path, query: query), method: method)
    }

    /// Headers for a URL Harvest handed us, which is how pagination is followed.
    private func request(url: URL, method: String = "GET") throws -> URLRequest {
        guard let credentials, credentials.isComplete else { throw HarvestError.notConfigured }
        var request = URLRequest(url: url)
        request.httpMethod = method
        request.setValue("Bearer \(credentials.token)", forHTTPHeaderField: "Authorization")
        request.setValue(credentials.accountID, forHTTPHeaderField: "Harvest-Account-Id")
        request.setValue(Self.userAgent, forHTTPHeaderField: "User-Agent")
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        return request
    }

    private func perform(_ request: URLRequest) async throws -> Data {
        var attempt = 0
        while true {
            let (data, response) = try await transport.send(request)
            switch response.statusCode {
            case 200..<300:
                return data
            case 401, 403:
                throw HarvestError.unauthorized
            case 404:
                throw HarvestError.notFound
            case 429:
                let retryAfter = Self.retryAfter(from: response)
                guard attempt < maxRetries else { throw HarvestError.rateLimited(retryAfter: retryAfter) }
                attempt += 1
                try await Task.sleep(for: .seconds(retryAfter * backoffScale))
            case 500...:
                // Never retry a POST: the server may have committed the change
                // before failing, and a second create means a duplicate time entry.
                // Let the caller decide, with the request left in the queue.
                guard request.httpMethod != "POST" else {
                    throw HarvestError.server(status: response.statusCode)
                }
                guard attempt < maxRetries else { throw HarvestError.server(status: response.statusCode) }
                attempt += 1
                try await Task.sleep(for: .seconds(Double(attempt) * backoffScale))
            default:
                throw HarvestError.rejected(
                    status: response.statusCode,
                    message: Self.errorMessage(from: data)
                )
            }
        }
    }

    static func retryAfter(from response: HTTPURLResponse) -> TimeInterval {
        let header = response.value(forHTTPHeaderField: "Retry-After")
        return header.flatMap(TimeInterval.init) ?? 15
    }

    /// Harvest reports validation failures as `{"message": "..."}` or a field-keyed map.
    private static func errorMessage(from data: Data) -> String {
        guard let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return "" }
        if let message = object["message"] as? String { return message }
        return object
            .map { key, value in "\(key): \(value)" }
            .sorted()
            .joined(separator: "; ")
    }

    // MARK: - Coding

    static let decoder: JSONDecoder = {
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        decoder.dateDecodingStrategy = .custom { decoder in
            let text = try decoder.singleValueContainer().decode(String.self)
            // Harvest sends whole seconds, but tolerate fractional in case that changes.
            if let date = try? Date(text, strategy: .iso8601) { return date }
            if let date = try? Date(text, strategy: .iso8601.year().month().day()
                .dateTimeSeparator(.standard).time(includingFractionalSeconds: true)
                .timeZone(separator: .omitted)) { return date }
            throw DecodingError.dataCorrupted(
                .init(codingPath: decoder.codingPath, debugDescription: "Not an ISO 8601 timestamp: \(text)")
            )
        }
        return decoder
    }()

    static let encoder: JSONEncoder = {
        let encoder = JSONEncoder()
        encoder.keyEncodingStrategy = .convertToSnakeCase
        return encoder
    }()



    private static func decode<T: Decodable>(_ type: T.Type, from data: Data, endpoint: String) throws -> T {
        do {
            return try decoder.decode(type, from: data)
        } catch {
            throw HarvestError.decoding(endpoint: endpoint, detail: describe(error))
        }
    }

    /// Turns a `DecodingError` into something that names the offending field.
    /// The default description is a wall of text that omits the path.
    private static func describe(_ error: any Error) -> String {
        func path(_ context: DecodingError.Context) -> String {
            let path = context.codingPath.map(\.stringValue).joined(separator: ".")
            return path.isEmpty ? "the top level" : path
        }
        switch error {
        case let DecodingError.keyNotFound(key, context):
            return "missing field ‘\(key.stringValue)’ at \(path(context))"
        case let DecodingError.typeMismatch(type, context):
            return "expected \(type) at \(path(context))"
        case let DecodingError.valueNotFound(type, context):
            return "unexpected null \(type) at \(path(context))"
        case let DecodingError.dataCorrupted(context):
            return "\(context.debugDescription) at \(path(context))"
        default:
            return String(describing: error)
        }
    }
}

// MARK: - Request bodies

private struct EmptyBody: Encodable, Sendable {}

private struct CreateTimeEntryPayload: Encodable, Sendable {
    let projectId: Int
    let taskId: Int
    let spentDate: CalendarDate
    var notes: String?
    var hours: Double?
}

private struct UpdateTimeEntryPayload: Encodable, Sendable {
    var projectId: Int?
    var taskId: Int?
    var spentDate: CalendarDate?
    var notes: String?
    var hours: Double?
}
