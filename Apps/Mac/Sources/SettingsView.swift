import SheavesCore
import SwiftUI

struct SettingsView: View {
    @Environment(TimeTracker.self) private var tracker
    @Environment(HotKeyPreference.self) private var hotKeys

    @State private var accountID = ""
    @State private var token = ""
    @State private var isConnecting = false
    @State private var errorMessage: String?
    @State private var isConfirmingDisconnect = false

    var body: some View {
        Form {
            if tracker.connection == .needsCredentials {
                connectSection
            } else {
                connectedSection
            }
            shortcutSection
        }
        .formStyle(.grouped)
        .frame(width: 460)
        .fixedSize(horizontal: false, vertical: true)
    }

    private var connectSection: some View {
        Section {
            TextField("Account ID", text: $accountID)
                .textContentType(.username)
            SecureField("Personal Access Token", text: $token)

            if let errorMessage {
                Label(errorMessage, systemImage: "exclamationmark.triangle.fill")
                    .font(.callout)
                    .foregroundStyle(.red)
            }

            HStack {
                Link("Create a token…", destination: URL(string: "https://id.getharvest.com/developers")!)
                Spacer()
                Button(isConnecting ? "Connecting…" : "Connect") { connect() }
                    .buttonStyle(.borderedProminent)
                    .disabled(isConnecting || accountID.isEmpty || token.isEmpty)
            }
        } header: {
            Text("Harvest")
        } footer: {
            Text("Create a personal access token at id.getharvest.com/developers. The page shows the token and the account IDs it can reach. Sheaves stores the token in your Keychain and sends it only to api.harvestapp.com.")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
    }

    private var connectedSection: some View {
        Section("Harvest") {
            LabeledContent("Account", value: tracker.company?.name ?? "—")
            LabeledContent("Signed in as", value: tracker.user?.name ?? "—")
            LabeledContent("Projects", value: "\(tracker.targets.count.formatted()) task assignments")
            HStack {
                Spacer()
                Button("Disconnect…", role: .destructive) { isConfirmingDisconnect = true }
            }
            .confirmationDialog(
                "Disconnect from Harvest?",
                isPresented: $isConfirmingDisconnect,
                titleVisibility: .visible
            ) {
                Button("Disconnect", role: .destructive) {
                    Task { await tracker.disconnect() }
                }
                Button("Cancel", role: .cancel) {}
            } message: {
                Text(
                    tracker.pendingCount > 0
                        ? "\(tracker.pendingCount.formatted()) change\(tracker.pendingCount == 1 ? "" : "s") have not reached Harvest yet and will be discarded along with the token."
                        : "The token will be removed from your Keychain. Cached entries are cleared."
                )
            }
        }
    }

    private var shortcutSection: some View {
        @Bindable var hotKeys = hotKeys
        return Section {
            LabeledContent("Quick entry") {
                ShortcutRecorder(shortcut: $hotKeys.shortcut)
            }
            if hotKeys.isRegistered == false {
                Label(
                    "Another app already owns \(hotKeys.shortcut.display). Pick a different combination.",
                    systemImage: "exclamationmark.triangle.fill"
                )
                .font(.callout)
                .foregroundStyle(.orange)
            }
            if hotKeys.shortcut != .quickEntryDefault {
                HStack {
                    Spacer()
                    Button("Reset to ⌃⌥⌘T") { hotKeys.resetToDefault() }
                }
            }
        } header: {
            Text("Shortcut")
        } footer: {
            Text("Opens the quick-entry panel from any app. Sheaves registers it with the window server, so no Accessibility permission is needed.")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
    }

    private func connect() {
        isConnecting = true
        errorMessage = nil
        Task {
            do {
                try await tracker.connect(HarvestCredentials(accountID: accountID, token: token))
                token = ""
            } catch {
                errorMessage = error.localizedDescription
            }
            isConnecting = false
        }
    }
}
