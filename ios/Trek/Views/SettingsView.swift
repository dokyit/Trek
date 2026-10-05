import SwiftUI

struct SettingsView: View {
    @Environment(AppModel.self) private var model
    @State private var confirmUnpair = false

    var body: some View {
        @Bindable var model = model
        NavigationStack {
            Form {
                Section {
                    HStack(spacing: 14) {
                        Image(systemName: "laptopcomputer")
                            .font(.system(size: 22, weight: .medium))
                            .foregroundStyle(Trek.foreground)
                            .frame(width: 46, height: 46)
                            .background(Trek.foreground.opacity(0.07), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
                        VStack(alignment: .leading, spacing: 3) {
                            Text(model.host?.name ?? PairedMac.load()?.hostName ?? "Your Mac").font(.headline)
                            HStack(spacing: 6) {
                                Circle().fill(statusColor).frame(width: 7, height: 7)
                                Text(model.mode == .demo ? "Demo · sample data" : model.connection.label)
                                    .font(.subheadline).foregroundStyle(Trek.muted)
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
                        LabeledContent("Address", value: paired.address)
                        SecurityRow(transport: model.transport ?? paired.transport)
                        if case .tls(let pin) = model.transport ?? paired.transport {
                            LabeledContent("Fingerprint") { Text(pin.short).monospaced() }
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
                }

                PhoneNotificationsSection()

                AppearanceSection()

                Section {
                    LabeledContent("“Allow for session”", value: "Needs \(DeviceOwner.methodName)")
                    LabeledContent("Requests time out", value: "Never")
                } header: {
                    Text("Approvals")
                } footer: {
                    Text("Trek never answers for you. An approval or question waits on the Mac and here until someone decides.")
                }

                Section {
                    LabeledContent("Version", value: ClientMessage.appVersion)
                    LabeledContent("Protocol", value: "v\(trekProtocolVersion)")
                    Link(destination: URL(string: "https://github.com/dokyit/Trek")!) {
                        Label("Trek on GitHub", systemImage: "arrow.up.right.square")
                    }
                } header: {
                    Text("About")
                }
            }
            .scrollContentBackground(.hidden)
            .background(Trek.background)
            .navigationTitle("Settings")
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
            configuration.icon.font(.footnote.weight(.semibold))
            configuration.title.fontWeight(.medium)
        }
    }
}

struct SearchView: View {
    @Environment(AppModel.self) private var model
    @State private var query = ""
    @Environment(\.compactRows) private var compact

    private var results: [ThreadSummary] {
        let q = query.trimmingCharacters(in: .whitespaces).lowercased()
        let sorted = model.threads.sorted { $0.updatedAt > $1.updatedAt }
        guard !q.isEmpty else { return sorted }
        return sorted.filter {
            $0.title.lowercased().contains(q) || ($0.project?.name.lowercased().contains(q) ?? false)
                || $0.agent.name.lowercased().contains(q) || ($0.branch?.lowercased().contains(q) ?? false)
        }
    }

    var body: some View {
        NavigationStack {
            List(results) { t in
                NavigationLink(value: t.id) { ThreadRow(thread: t) }
                    .navigationLinkIndicatorVisibility(.hidden)
                    .listRowSeparator(.hidden)
                    .listRowBackground(Color.clear)
                    .listRowInsets(EdgeInsets(top: compact ? 7 : 9, leading: 20, bottom: compact ? 7 : 9, trailing: 18))
            }
            .listStyle(.plain)
            .scrollContentBackground(.hidden)
            .background(Trek.background)
            .overlay {
                if results.isEmpty {
                    ContentUnavailableView.search(text: query)
                }
            }
            .navigationTitle("Search")
            .searchable(text: $query, prompt: "Threads, projects, agents, branches")
            .navigationDestination(for: String.self) { ThreadView(threadID: $0) }
        }
    }
}
