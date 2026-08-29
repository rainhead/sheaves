import Foundation
import Security

/// Stores the Harvest personal access token in the Keychain.
///
/// The token is a bearer credential for the whole Harvest account, so it never
/// touches UserDefaults or the app's container — only the data-protection Keychain,
/// which keeps the macOS and iOS behaviour the same.
public struct KeychainStore: Sendable {
    public enum Failure: Error, Sendable, Equatable {
        case status(OSStatus)
        case malformedData
    }

    private let service: String
    private let account: String

    public init(service: String = "com.rainhead.Sheaves", account: String = "harvest-credentials") {
        self.service = service
        self.account = account
    }

    private var baseQuery: [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecUseDataProtectionKeychain as String: true,
        ]
    }

    public func read() throws -> HarvestCredentials? {
        var query = baseQuery
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne

        var item: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &item)
        switch status {
        case errSecSuccess:
            guard let data = item as? Data,
                  let credentials = try? JSONDecoder().decode(HarvestCredentials.self, from: data)
            else { throw Failure.malformedData }
            return credentials
        case errSecItemNotFound:
            return nil
        default:
            throw Failure.status(status)
        }
    }

    public func write(_ credentials: HarvestCredentials) throws {
        let data = try JSONEncoder().encode(credentials)
        let attributes: [String: Any] = [kSecValueData as String: data]

        let updateStatus = SecItemUpdate(baseQuery as CFDictionary, attributes as CFDictionary)
        if updateStatus == errSecSuccess { return }
        guard updateStatus == errSecItemNotFound else { throw Failure.status(updateStatus) }

        var insert = baseQuery
        insert[kSecValueData as String] = data
        let addStatus = SecItemAdd(insert as CFDictionary, nil)
        guard addStatus == errSecSuccess else { throw Failure.status(addStatus) }
    }

    public func delete() throws {
        let status = SecItemDelete(baseQuery as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else {
            throw Failure.status(status)
        }
    }
}
