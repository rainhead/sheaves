import Foundation
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

        try store.write(credentials)

        #expect(try store.read() == credentials)
    }

    /// The second write has to update in place; a blind add would fail with
    /// errSecDuplicateItem and leave the old token behind.
    @Test("overwrites an existing token")
    func overwrites() throws {
        let store = makeStore()
        defer { try? store.delete() }

        try store.write(HarvestCredentials(accountID: "1", token: "old"))
        try store.write(HarvestCredentials(accountID: "2", token: "new"))

        let stored = try store.read()
        #expect(stored?.token == "new")
        #expect(stored?.accountID == "2")
    }

    @Test("deletes, and deleting twice is not an error")
    func deletes() throws {
        let store = makeStore()
        try store.write(HarvestCredentials(accountID: "1", token: "t"))

        try store.delete()
        try store.delete()

        #expect(try store.read() == nil)
    }

    @Test("trims whitespace pasted around a token")
    func trimsPastedWhitespace() throws {
        let store = makeStore()
        defer { try? store.delete() }
        let credentials = HarvestCredentials(accountID: "  12345\n", token: " sekrit ")

        try store.write(credentials)

        #expect(try store.read()?.token == "sekrit")
        #expect(try store.read()?.accountID == "12345")
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
