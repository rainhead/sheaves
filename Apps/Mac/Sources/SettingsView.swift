import SheavesCore
import SwiftUI

struct SettingsView: View {
    @Environment(TimeTracker.self) private var tracker

    @State private var accountID = ""
    @State private var token = ""
    @State private var isConnecting = false
    @State private var errorMessage: String?

    var body: some View {
        Form {
            if tracker.connection == .needsCredentials {
                connectSection
            } else {
                connectedSection
            }
        }
        .formStyle(.grouped)
        .frame(width: 460)
        .fixedSize(horizontal: false, vertical: true)
    }

    private var connectSection: some View {
        Section {
            TextField("Account ID", text: $accountID)
                .textContentType(.oneTimeCode)
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
            LabeledContent("Projects", value: "\(tracker.targets.count) task assignments")
            LabeledContent("Quick entry") {
                Text("⌃⌥⌘T")
                    .font(.body.monospaced())
            }
            HStack {
                Spacer()
                Button("Disconnect", role: .destructive) {
                    Task { await tracker.disconnect() }
                }
            }
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
