import SwiftUI

enum SettingsRoute: Hashable {
    case appearance
}

struct SettingsView: View {
    @Environment(AppModel.self) private var model
    @State private var confirmUnpair = false
    @State private var path: [SettingsRoute] = Launch.tab == "appearance" ? [.appearance] : []
    @AppStorage(AppearanceKey.theme) private var theme = ThemeChoice.system.rawValue

    var body: some View {
        @Bindable var model = model
        NavigationStack(path: $path) {
            Form {
                Section {
                    HStack(spacing: 14) {
                        Image(systemName: "laptopcomputer")
                            .font(.system(size: 22, weight: .medium))
                            .foregroundStyle(Trek.foreground)
                            .frame(width: 46, height: 46)
                            .background(Trek.foreground.opacity(0.07), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
                        VStack(alignment: .leading, spacing: 3) {
                            Text(model.host?.name ?? PairedMac.load()?.hostName ?? "Your Mac").font(.ui(.headline))
                            HStack(spacing: 6) {
                                Circle().fill(statusColor).frame(width: 7, height: 7)
                                Text(model.mode == .demo ? "Demo · sample data" : model.connection.label)
                                    .font(.ui(.subheadline)).foregroundStyle(Trek.muted)
                            }
                        }
                    }
                    .padding(.vertical, 4)
                    NavigationLink {
                        MacSettingsView()
                    } label: {
                        Label("Mac settings", systemImage: "slider.horizontal.3")
                    }
                    if model.mode == .live, let paired = PairedMac.load() {
                        // Each way to the Mac, the one in use ticked: Wi-Fi at home, the tailnet
                        // anywhere Tailscale is on.
                        ForEach(([paired.address] + paired.routes).reduce(into: [String]()) { if !$0.contains($1) { $0.append($1) } }, id: \.self) { route in
                            LabeledContent(TrekClient.isTailnet(route) ? "Tailscale" : "Wi-Fi") {
                                HStack(spacing: 6) {
                                    Text(route).font(.code(.subheadline))
                                    if route == paired.address, model.connection == .connected {
                                        Image(systemName: "checkmark").font(.ui(.caption).weight(.semibold)).foregroundStyle(Trek.done)
                                    }
                                }
                            }
                        }
                        SecurityRow(transport: model.transport ?? paired.transport)
                        if case .tls(let pin) = model.transport ?? paired.transport {
                            LabeledContent("Fingerprint") { Text(pin.short).font(.code(.body)) }
                        }
                    }
                    if let host = model.host {
                        LabeledContent("Trek", value: host.version)
                    }
                    if model.mode == .demo {
                        Button("Pair with a Mac") { model.leave() }
                    } else {
                        Button("Unpair this iPhone", role: .destructive) { confirmUnpair = true }
                    }
                } header: {
                    Text("Mac")
                }

                Section {
                    Picker("While an agent works", selection: $model.followUpMode) {
                        ForEach(SendMode.allCases) { Text($0.label).tag($0) }
                    }
                } header: {
                    Text("Follow-ups")
                } footer: {
                    Text("Steer adds your message to the running turn at its next step. Queue waits until the turn ends. You can switch per message from the composer.")
                        .font(.ui(.footnote))
                }

                PhoneNotificationsSection()

                Section {
                    NavigationLink(value: SettingsRoute.appearance) {
                        LabeledContent {
                            Text((ThemeChoice(rawValue: theme) ?? .system).label)
                        } label: {
                            Label("Appearance", systemImage: "circle.lefthalf.filled")
                        }
                    }
                } footer: {
                    Text("Theme, app icon, text size, background art and motion, kept on this iPhone.")
                        .font(.ui(.footnote))
                }

                Section {
                    LabeledContent("“Allow for session”", value: "Needs \(DeviceOwner.methodName)")
                    LabeledContent("Requests time out", value: "Never")
                } header: {
                    Text("Approvals")
                } footer: {
                    Text("Trek never answers for you. An approval or question waits on the Mac and here until someone decides.")
                        .font(.ui(.footnote))
                }

                Section {
                    LabeledContent("Version", value: ClientMessage.appVersion)
                    LabeledContent("Protocol", value: "v\(trekProtocolVersion)")
                    Link(destination: URL(string: "https://github.com/dokyit/Trek")!) {
                        Label("Trek on GitHub", systemImage: "arrow.up.right.square")
                    }
                } header: {
                    Text("About")
                } footer: {
                    Text("Trek is not affiliated with or endorsed by the makers of the agents it connects. Agent and provider names and logos are trademarks of their respective owners, shown only to identify the agent a thread uses.")
                        .font(.ui(.footnote))
                }
            }
            .scrollContentBackground(.hidden)
            .background(Trek.grouped)
            .navigationTitle("Settings")
            .navigationDestination(for: SettingsRoute.self) { route in
                switch route {
                case .appearance: AppearanceView()
                }
            }
            .task { model.loadSettings() }
            .onChange(of: model.connection) { if model.connection == .connected { model.loadSettings() } }
            .confirmationDialog("Unpair this iPhone?", isPresented: $confirmUnpair, titleVisibility: .visible) {
                Button("Unpair", role: .destructive) { model.leave() }
            } message: {
                Text("You'll need a new code from the Mac to connect again. Revoke the device on the Mac too to be sure.")
            }
        }
    }

    private var statusColor: Color {
        if model.mode == .demo { return Trek.plan }
        switch model.connection {
        case .connected: return Trek.done
        case .connecting: return Trek.approval
        default: return Trek.failed
        }
    }
}

/// "Encrypted · pinned" with a lock, or "Unencrypted" in amber for a plain `ws://` Mac.
struct SecurityRow: View {
    var transport: Transport

    var body: some View {
        LabeledContent("Connection") {
            if transport.isEncrypted {
                Label("Encrypted · pinned", systemImage: "lock.fill")
                    .foregroundStyle(Trek.done)
            } else {
                Label("Unencrypted", systemImage: "lock.open.fill")
                    .foregroundStyle(Trek.approval)
            }
        }
        .labelStyle(SecurityLabelStyle())
        .accessibilityElement(children: .combine)
    }
}

private struct SecurityLabelStyle: LabelStyle {
    func makeBody(configuration: Configuration) -> some View {
        HStack(spacing: 5) {
            configuration.icon.font(.ui(.footnote).weight(.semibold))
            configuration.title.fontWeight(.medium)
        }
    }
}
