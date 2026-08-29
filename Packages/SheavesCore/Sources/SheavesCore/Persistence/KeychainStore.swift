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
        var query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
        #if os(iOS)
        // iOS has only the data-protection keychain. Asking for it on macOS as well
        // would be tidier, but macOS grants it only to apps signed with a keychain
        // access group — which needs a real team — so an ad-hoc build would be
        // refused outright. macOS therefore uses the file-based keychain.
        query[kSecUseDataProtectionKeychain as String] = true
        query[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlock
        #endif
        return query
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

extension KeychainStore.Failure: LocalizedError {
    public var errorDescription: String? {
        switch self {
        case .malformedData:
            return "The saved Harvest credentials could not be read. Disconnect and paste the token again."
        case .status(let status):
            let detail = SecCopyErrorMessageString(status, nil) as String? ?? "Keychain error"
            switch status {
            case errSecMissingEntitlement:
                return "macOS refused Keychain access. Sheaves needs to be signed with a development team — set DEVELOPMENT_TEAM in project.yml. (\(status))"
            case errSecUserCanceled, errSecAuthFailed:
                return "Keychain access was denied. Allow Sheaves to use the login keychain and try again. (\(status))"
            default:
                return "\(detail) (\(status))"
            }
        }
    }
}
