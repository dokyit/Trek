import SwiftUI

enum SettingsRoute: Hashable {
    case appearance
}

struct SettingsView: View {
    @Environment(AppModel.self) private var model
    @State private var scanning = false
    @State private var path: [SettingsRoute] = Launch.tab == "appearance" ? [.appearance] : []
    @AppStorage(AppearanceKey.theme) private var theme = ThemeChoice.system.rawValue

    var body: some View {
        @Bindable var model = model
        NavigationStack(path: $path) {
            Form {
                Section {
                    ForEach(model.links) { link in
                        NavigationLink {
                            MachineView(link: link)
                        } label: {
                            MachineLabel(link: link)
                        }
                    }
                    Button("Add a machine…", systemImage: "plus") { scanning = true }
                } header: {
                    Text(model.links.count > 1 ? "Machines" : "Mac")
                } footer: {
                    if !model.links.isEmpty {
                        Text("Every machine stays connected at once; their threads share one list.")
                            .font(.ui(.footnote))
                    }
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
            .sheet(isPresented: $scanning) {
                ScannerSheet { link in
                    scanning = false
                    // Same confirmation as a deep link: name, address and fingerprint first.
                    DispatchQueue.main.asyncAfter(deadline: .now() + 0.4) { model.offer(link) }
                }
            }
        }
    }
}

/// A machine's row in the Machines list: its badge, name, and whether it's connected.
private struct MachineLabel: View {
    var link: HostLink
    @Environment(AppModel.self) private var model

    var body: some View {
        HStack(spacing: 14) {
            Image(systemName: "laptopcomputer")
                .font(.system(size: 20, weight: .medium))
                .foregroundStyle(Trek.foreground)
                .frame(width: 40, height: 40)
                .background(Trek.foreground.opacity(0.07), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
            VStack(alignment: .leading, spacing: 3) {
                Text(link.name).font(.ui(.headline))
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    Circle().fill(statusColor).frame(width: 7, height: 7)
                    Text(model.mode == .demo ? "Demo · sample data" : link.connectionText)
                        .font(.ui(.subheadline)).foregroundStyle(Trek.muted)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
        }
        .padding(.vertical, 2)
    }

    private var statusColor: Color {
        if model.mode == .demo { return Trek.plan }
        switch link.connection {
        case .connected: return Trek.done
        case .connecting: return Trek.approval
        default: return Trek.failed
        }
    }
}

/// One machine: how the phone reaches it (its addresses and the connection's security), its
/// Trek version, its settings, what it tells this iPhone, and Unpair.
struct MachineView: View {
    var link: HostLink
    @Environment(AppModel.self) private var model
    @State private var confirmUnpair = false

    private var paired: PairedMac { link.paired }
    private var routes: [String] {
        ([paired.address] + paired.routes).filter { !$0.isEmpty }.reduce(into: [String]()) { if !$0.contains($1) { $0.append($1) } }
    }

    var body: some View {
        Form {
            Section {
                if model.mode == .live {
                    switch link.connection {
                    case .offline:
                        Button("Try now") { model.retryNow(link) }
                    case .unauthorized, .identityChanged:
                        Button("Pair again") { model.leave(link) }
                    default: EmptyView()
                    }
                    // Each way to the Mac, the one in use ticked: Wi-Fi at home, the tailnet
                    // anywhere Tailscale is on.
                    ForEach(routes, id: \.self) { route in
                        LabeledContent(Self.routeLabel(route)) {
                            HStack(spacing: 6) {
                                Text(route).font(.code(.subheadline))
                                if route == paired.address, link.connection == .connected {
                                    Image(systemName: "checkmark").font(.ui(.caption).weight(.semibold)).foregroundStyle(Trek.done)
                                }
                            }
                        }
                    }
                    SecurityRow(transport: link.transport)
                    if case .tls(let pin) = link.transport {
                        LabeledContent("Fingerprint") { Text(pin.short).font(.code(.body)) }
                    }
                }
                if let host = link.host {
                    LabeledContent("Trek", value: host.version)
                }
                NavigationLink {
                    MacSettingsView(machine: link)
                } label: {
                    Label("Mac settings", systemImage: "slider.horizontal.3")
                }
            }

            PhoneNotificationsSection(machine: link)

            Section {
                if model.mode == .demo {
                    Button("Pair with a Mac") { model.leave(link) }
                } else {
                    Button("Unpair this iPhone", role: .destructive) { confirmUnpair = true }
                }
            }
        }
        .scrollContentBackground(.hidden)
        .background(Trek.grouped)
        .navigationTitle(link.name)
        .task { model.loadSettings(on: link) }
        .onChange(of: link.connection) { if link.connection == .connected { model.loadSettings(on: link) } }
        .confirmationDialog("Unpair this iPhone?", isPresented: $confirmUnpair, titleVisibility: .visible) {
            Button("Unpair", role: .destructive) { model.leave(link) }
        } message: {
            Text("You'll need a new code from \(link.name) to connect again. Revoke the device on it too to be sure. Other machines stay paired.")
        }
    }

    /// Which way to the Mac an address is.
    private static func routeLabel(_ address: String) -> String {
        switch Address.kind(address) {
        case .local: "Wi-Fi"
        case .tailnet: "Tailscale"
        case .other: "Address"
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
