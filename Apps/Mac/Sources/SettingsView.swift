import AppKit
import SheavesCore
import SwiftUI

struct SettingsView: View {
    @Environment(TimeTracker.self) private var tracker
    @Environment(HotKeyPreference.self) private var hotKeys
    @Environment(LoginItemPreference.self) private var loginItem
    @Environment(PeerPreference.self) private var peers

    @State private var accountID = ""
    @State private var token = ""
    @State private var isConnecting = false
    @State private var errorMessage: String?
    @State private var isConfirmingDisconnect = false
    /// How many queued changes the credentials just tried would orphan, while the
    /// user is being asked what to do about them.
    @State private var unsentElsewhere: Int?
    /// The peers as typed, kept apart from the names so a separator typed last is
    /// not taken away mid-word.
    @State private var peerText: String?

    var body: some View {
        Form {
            if tracker.connection == .needsCredentials {
                connectSection
            } else {
                connectedSection
            }
            shortcutSection
            startupSection
            peersSection
            aboutSection
        }
        .formStyle(.grouped)
        .frame(width: 460)
        .fixedSize(horizontal: false, vertical: true)
        // The user can turn the login item off in System Settings while this window
        // sits open, so re-read it whenever they come back to Sheaves.
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
            loginItem.refresh()
        }
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
            .confirmationDialog(
                "Discard changes made under another account?",
                isPresented: Binding(
                    get: { unsentElsewhere != nil },
                    set: { if !$0 { unsentElsewhere = nil } }
                ),
                titleVisibility: .visible,
                presenting: unsentElsewhere
            ) { _ in
                Button("Discard and Connect", role: .destructive) { connect(discardingUnsentChanges: true) }
                Button("Cancel", role: .cancel) {}
            } message: { count in
                Text("\(count.formatted()) change\(count == 1 ? " has" : "s have") not reached Harvest yet, and \(count == 1 ? "was" : "were") made under a different account or user than this token belongs to. To send \(count == 1 ? "it" : "them"), cancel and connect with a token for that account instead.")
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

    /// Which build is running, so a bug report can say. The core's version comes
    /// from the Rust library itself, which also proves the app is linked to it.
    private var aboutSection: some View {
        Section("About") {
            LabeledContent("Version", value: Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "unknown")
            LabeledContent("Core", value: coreVersion())
        }
    }

    private var peersSection: some View {
        Section {
            TextField(
                "Peers",
                text: Binding(
                    get: { peerText ?? peers.names.joined(separator: ", ") },
                    set: { text in
                        peerText = text
                        peers.names = PeerPreference.names(in: text)
                    }
                ),
                prompt: Text("hostess, peters-macbook-air")
            )
            if !peers.names.isEmpty {
                let addresses = tracker.listeningOn
                LabeledContent(
                    "Answering on",
                    value: addresses.isEmpty ? "No tailnet address" : addresses.joined(separator: ", ")
                )
            }
        } header: {
            Text("Peers")
        } footer: {
            Text("Other machines running Sheaves, by their names on your tailnet. Before asking whether you were away, this Mac asks them whether they saw you meanwhile, and it answers them in turn. Only machines on the tailnet are asked or answered.")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
    }

    private var startupSection: some View {
        Section {
            Toggle(
                "Open Sheaves at login",
                isOn: Binding(get: { loginItem.isEnabled }, set: { loginItem.setEnabled($0) })
            )
            switch loginItem.problem {
            case .needsApproval:
                Label(
                    "macOS is holding this one. Turn Sheaves on under Login Items and it will stick.",
                    systemImage: "exclamationmark.triangle.fill"
                )
                .font(.callout)
                .foregroundStyle(.orange)
                HStack {
                    Spacer()
                    Button("Open Login Items…") { loginItem.openSystemSettings() }
                }
            case .failed(let message):
                Label(message, systemImage: "exclamationmark.triangle.fill")
                    .font(.callout)
                    .foregroundStyle(.orange)
            case nil:
                EmptyView()
            }
        } header: {
            Text("Startup")
        } footer: {
            Text("Sheaves has no Dock icon, so it opens straight into the menu bar with no window. System Settings › General › Login Items lists it alongside everything else that starts this way.")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
    }

    private func connect(discardingUnsentChanges: Bool = false) {
        isConnecting = true
        errorMessage = nil
        Task {
            do {
                try await tracker.connect(
                    HarvestCredentials(accountID: accountID, token: token),
                    discardingUnsentChanges: discardingUnsentChanges
                )
                token = ""
            } catch let error as TimeTracker.UnsentChangesBelongElsewhere {
                unsentElsewhere = error.count
            } catch {
                errorMessage = error.localizedDescription
            }
            isConnecting = false
        }
    }
}
