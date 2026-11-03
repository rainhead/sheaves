import Foundation
import Security
import Testing
@testable import SheavesCore

/// These run unsandboxed, so they cover the store's own logic rather than the app's
/// entitlements. The entitlement mistake they follow — asking macOS for the
/// data-protection keychain, which it grants only to team-signed apps — showed up as
/// errSecMissingEntitlement in the real app and could not have been caught here.
@Suite("KeychainStore", .serialized)
struct KeychainStoreTests {
    private func makeStore() -> KeychainStore {
        KeychainStore(service: "com.rainhead.Sheaves.tests.\(UUID().uuidString)")
    }

    @Test("returns nil before anything is stored")
    func readsNothingInitially() throws {
        let store = makeStore()
        defer { try? store.delete() }
        #expect(try store.read() == nil)
    }

    @Test("round-trips credentials")
    func roundTrip() throws {
        let store = makeStore()
        defer { try? store.delete() }
        let credentials = HarvestCredentials(accountID: "12345", token: "sekrit")

        try store.write(login: Login(credentials: credentials, userId: 7))

        #expect(try store.read() == Login(credentials: credentials, userId: 7))
    }

    /// The second write has to update in place; a blind add would fail with
    /// errSecDuplicateItem and leave the old token behind.
    @Test("overwrites an existing token")
    func overwrites() throws {
        let store = makeStore()
        defer { try? store.delete() }

        try store.write(login: Login(credentials: HarvestCredentials(accountID: "1", token: "old"), userId: 1))
        try store.write(login: Login(credentials: HarvestCredentials(accountID: "2", token: "new"), userId: 2))

        let stored = try store.read()
        #expect(stored?.credentials.token == "new")
        #expect(stored?.credentials.accountID == "2")
        #expect(stored?.userId == 2)
    }

    @Test("deletes, and deleting twice is not an error")
    func deletes() throws {
        let store = makeStore()
        try store.write(login: Login(credentials: HarvestCredentials(accountID: "1", token: "t"), userId: nil))

        try store.delete()
        try store.delete()

        #expect(try store.read() == nil)
    }

    /// The switch to the Rust core must not sign anybody out: the item the Swift
    /// core saved, keyed `accountID` and `token`, still reads.
    @Test("reads a token saved before the Rust core")
    func readsTheSwiftCoresItem() throws {
        let service = "com.rainhead.Sheaves.tests.\(UUID().uuidString)"
        let store = KeychainStore(service: service)
        defer { try? store.delete() }
        let saved = Data(#"{"accountID":"12345","token":"sekrit"}"#.utf8)
        let status = SecItemAdd([
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: "harvest-credentials",
            kSecValueData as String: saved,
        ] as CFDictionary, nil)
        try #require(status == errSecSuccess)

        #expect(try store.read() == Login(
            credentials: HarvestCredentials(accountID: "12345", token: "sekrit"), userId: nil
        ))
    }

    @Test("explains a keychain failure in words")
    func describesFailures() {
        let missing = KeychainStore.Failure.status(errSecMissingEntitlement)
        #expect(missing.localizedDescription.contains("development team"))
        // The default `Error` description would have said "error 0", which is what
        // hid this failure the first time.
        #expect(!missing.localizedDescription.contains("error 0"))
        #expect(KeychainStore.Failure.malformedData.localizedDescription.contains("token"))
    }
}
