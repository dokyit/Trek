import SwiftUI
import UIKit

enum SettingsRoute: Hashable {
    case appearance
    case feedback
    case storage
    /// The selected Mac's permission rules (`-TrekTab settings -TrekSheet rules`).
    case rules

    /// Where `-TrekTab` opens Settings.
    static var launched: [SettingsRoute] {
        switch Launch.tab {
        case "appearance": [.appearance]
        case "feedback": [.feedback]
        case "storage": [.storage]
        case "settings" where Launch.sheet == "rules": [.rules]
        default: []
        }
    }
}

/// The Settings tab. At the top, what this iPhone is connected to and how; then the machines,
/// how threads and the composer behave, notifications, approvals and security, what's kept on
/// this iPhone, privacy and feedback, and about the app. Each row says what it does; settings that
/// live on a machine are changed on its own page, and say which machine they belong to.
struct SettingsView: View {
    @Environment(AppModel.self) private var model
    @State private var scanning = false
    @State private var path: [SettingsRoute] = SettingsRoute.launched
    @State private var storage: PhoneStorage.Usage?
    @State private var icon = AppIconChoice.of(UIApplication.shared.alternateIconName)
    /// "Face ID", read once: asking LocalAuthentication on every redraw is wasteful.
    @State private var ownerMethod = DeviceOwner.methodName
    @AppStorage(AppearanceKey.theme) private var theme = ThemeChoice.system.rawValue
    @AppStorage(AppearanceKey.textSize) private var textSize = TextSizeChoice.system.rawValue
    @AppStorage(TurnActivities.settingKey) private var liveActivity = true

    var body: some View {
        @Bindable var model = model
        let update = model.appUpdate
        NavigationStack(path: $path) {
            Form {
                Section {
                    SettingsHeader(icon: icon)
                }

                if let update {
                    Section {
                        UpdateCard(update: update, offersLater: false)
                            .listRowBackground(Color.clear)
                            .listRowInsets(EdgeInsets())
                    }
                }

                machines
                threads
                notifications
                approvals
                thisIPhone
                privacy
                about(update)
            }
            .scrollContentBackground(.hidden)
            .fadesUnderBottomBars(Trek.grouped)
            .background(Trek.grouped)
            .navigationTitle("Settings")
            .navigationDestination(for: SettingsRoute.self) { route in
                switch route {
                case .appearance: AppearanceView()
                case .feedback: FeedbackView()
                case .storage: StorageView()
                case .rules: RulesView(link: model.selected)
                }
            }
            .sheet(isPresented: $scanning) {
                ScannerSheet { link in
                    scanning = false
                    // Same confirmation as a deep link: name, address and fingerprint first.
                    DispatchQueue.main.asyncAfter(deadline: .now() + 0.4) { model.offer(link) }
                }
            }
            // Read again on the way back: Storage may have been cleared, the icon changed.
            .task(id: path.isEmpty) {
                guard path.isEmpty else { return }
                icon = AppIconChoice.of(UIApplication.shared.alternateIconName)
                storage = await PhoneStorage.measure()
            }
        }
    }

    // MARK: Machines

    private var machines: some View {
        Section {
            ForEach(model.links) { link in
                NavigationLink {
                    MachineView(link: link)
                } label: {
                    MachineLabel(link: link)
                }
            }
            Button {
                scanning = true
            } label: {
                SettingsLabel(title: "Add a machine…", symbol: "plus")
            }
        } header: {
            Text(model.links.count > 1 ? "Machines" : "Machine")
        } footer: {
            if !model.links.isEmpty {
                Text("Every machine stays connected at once; their threads share one list. Tap one for its agents, its connection and its own settings.")
                    .font(.ui(.footnote))
            }
        }
    }

    // MARK: Threads and the composer

    private var threads: some View {
        @Bindable var model = model
        return Section {
            Picker(selection: $model.followUpMode) {
                ForEach(SendMode.allCases) { Text($0.label).tag($0) }
            } label: {
                SettingsLabel(title: "While an agent works", symbol: "paperplane")
            }
            if let link = model.selected {
                NavigationLink {
                    MacSettingsView(machine: link)
                } label: {
                    SettingsValueRow {
                        Text(defaultModel(on: link) ?? "")
                    } label: {
                        SettingsLabel(title: "New threads", subtitle: newThreadsLine(on: link), symbol: "sparkles")
                    }
                }
            }
        } header: {
            Text("Threads")
        } footer: {
            Text("Steer adds your message to the running turn at its next step; Queue holds it until the turn ends. You can switch per message from the composer.")
                .font(.ui(.footnote))
        }
    }

    /// The model new threads start with on `link`: its pick, else its agent's default.
    private func defaultModel(on link: HostLink) -> String? {
        guard let s = link.macSettings else { return nil }
        let agent = link.agents.first { $0.key == s.defaultAgent }
        return ModelNaming.current(s.defaultModel, agent: agent)?.label
    }

    /// "Claude Code · Auto-accept edits", with the machine's name when there are several.
    private func newThreadsLine(on link: HostLink) -> String {
        guard let s = link.macSettings else { return link.waitingWords }
        let agent = link.agents.first { $0.key == s.defaultAgent }?.name ?? s.defaultAgent
        var line = "\(agent) · \(s.defaultAccess.label)"
        if model.links.count > 1 { line += " · on \(MachineTag.short(link.name))" }
        return line
    }

    // MARK: Notifications

    private var notifications: some View {
        Section {
            ForEach(model.links) { link in
                NavigationLink {
                    PushSettingsView(link: link)
                } label: {
                    SettingsValueRow {
                        Text(link.macSettings.map { $0.push.enabled ? "On" : "Off" } ?? "")
                    } label: {
                        SettingsLabel(title: model.links.count > 1 ? "From \(MachineTag.short(link.name))" : "Push notifications",
                                      subtitle: link.macSettings?.push.summary ?? link.waitingWords,
                                      symbol: "bell.badge")
                    }
                }
            }
            if TurnActivities.installed {
                Toggle(isOn: $liveActivity) {
                    SettingsLabel(title: "Live Activity", subtitle: "The thread at work and what waits on you, on the Lock Screen and in the Dynamic Island",
                                  symbol: "platter.filled.top.iphone")
                }
                // The app's tint is the foreground, which draws a switch white on white in Dark.
                .tint(Trek.done)
                .onChange(of: liveActivity) { model.activities.refresh() }
            }
        } header: {
            Text("Notifications")
        } footer: {
            Text(TurnActivities.installed
                 ? "Your Mac tells this iPhone when a thread needs you. iOS soon stops Trek once you leave it, so the Live Activity says when it last heard from your Mac."
                 : "Your Mac tells this iPhone when a thread needs you, through the free ntfy app.")
                .font(.ui(.footnote))
        }
    }

    // MARK: Approvals and security

    private var approvals: some View {
        Section {
            SettingsValueRow {
                Text(sessionAllowed ? "Needs \(ownerMethod)" : "Off on your Mac")
            } label: {
                SettingsLabel(title: "Allow for a session", symbol: ownerMethod == "Touch ID" ? "touchid" : "faceid")
            }
            SettingsValueRow {
                Text("Never")
            } label: {
                SettingsLabel(title: "Requests time out", symbol: "hourglass")
            }
            SettingsValueRow {
                connectionValue
            } label: {
                SettingsLabel(title: "Connection", symbol: "lock")
            }
        } header: {
            Text("Approvals and security")
        } footer: {
            Text("Trek never answers for you: an approval or question waits on the Mac and here until someone decides. Your Mac decides whether this iPhone may allow a request for a whole session.")
                .font(.ui(.footnote))
        }
    }

    /// Some machine lets this iPhone allow a request for the rest of a session (a Mac that
    /// doesn't say takes them, as the approval card has it).
    private var sessionAllowed: Bool {
        model.links.isEmpty || model.links.contains { $0.macSettings?.sessionApprovals != false }
    }

    @ViewBuilder
    private var connectionValue: some View {
        let plain = model.links.count { !$0.transport.isEncrypted }
        if model.mode == .demo {
            Text("Demo, on this iPhone")
        } else if plain == 0 {
            Label("Encrypted · pinned", systemImage: "lock.fill")
                .labelStyle(SettingsLineLabelStyle())
                .foregroundStyle(Trek.done)
        } else {
            Label(model.links.count == 1 ? "Unencrypted" : "\(plain) unencrypted", systemImage: "lock.open.fill")
                .labelStyle(SettingsLineLabelStyle())
                .foregroundStyle(Trek.approval)
        }
    }

    // MARK: This iPhone

    private var thisIPhone: some View {
        Section {
            NavigationLink(value: SettingsRoute.appearance) {
                SettingsValueRow {
                    Text(LookStore.shared.look.name)
                } label: {
                    SettingsLabel(title: "Appearance", subtitle: appearanceLine, symbol: "circle.lefthalf.filled")
                }
            }
            NavigationLink(value: SettingsRoute.storage) {
                SettingsValueRow {
                    Text(storage.map { PhoneStorage.size($0.bytes) } ?? "…")
                } label: {
                    SettingsLabel(title: "Storage", subtitle: storageLine, symbol: "internaldrive")
                }
            }
        } header: {
            Text("This iPhone")
        } footer: {
            Text("Kept on this iPhone only. Your threads and notes stay on your machines.")
                .font(.ui(.footnote))
        }
    }

    /// "Night icon · iPhone's text size", "Paper icon · Larger text".
    private var appearanceLine: String {
        let size = TextSizeChoice(rawValue: textSize) ?? .system
        let mode = (ThemeChoice(rawValue: theme) ?? .system).label
        return "\(mode) · \(icon.label) icon · " + (size == .system ? "iPhone's text size" : "\(size.label) text")
    }

    /// "14 threads and 3 files kept for quick opening".
    private var storageLine: String {
        guard let s = storage else { return "Threads and files kept for quick opening" }
        if s.threads == 0 && s.files == 0 { return "Nothing kept yet" }
        let threads = s.threads == 1 ? "1 thread" : "\(s.threads) threads"
        let files = s.files == 1 ? "1 file" : "\(s.files) files"
        return "\(threads) and \(files) kept for quick opening"
    }

    // MARK: Privacy and feedback

    private var privacy: some View {
        Section {
            NavigationLink(value: SettingsRoute.feedback) {
                SettingsLabel(title: "Send feedback…", subtitle: "A bug, an idea or anything else, to Trek's developer", symbol: "bubble.left")
            }
            Link(destination: URL(string: "https://github.com/dokyit/Trek/blob/main/docs/PRIVACY.md")!) {
                HStack {
                    SettingsLabel(title: "Privacy", subtitle: "What Trek sends, and what it never does", symbol: "hand.raised")
                    Spacer()
                    Image(systemName: "arrow.up.right").font(.ui(.footnote).weight(.semibold)).foregroundStyle(Trek.muted)
                }
            }
        } header: {
            Text("Privacy and feedback")
        } footer: {
            Text("The iPhone app sends no usage data or crash reports. Feedback goes only when you send it.")
                .font(.ui(.footnote))
        }
    }

    // MARK: About

    private func about(_ update: AppUpdate.Available?) -> some View {
        Section {
            LabeledContent("Version", value: ClientMessage.appBuild.map { "\(ClientMessage.appVersion) (\($0))" } ?? ClientMessage.appVersion)
            if let update {
                LabeledContent("Update available", value: update.version)
            }
            if let expiry = AppSigning.expiry {
                LabeledContent("Signed until") {
                    Text(expiry.formatted(.dateTime.weekday(.abbreviated).day().month(.abbreviated)))
                        .foregroundStyle(expiry.timeIntervalSinceNow < AppSigning.soon ? Trek.approval : Trek.muted)
                }
            }
            LabeledContent("Protocol", value: "v\(trekProtocolVersion)")
            Link(destination: URL(string: "https://github.com/dokyit/Trek")!) {
                HStack {
                    Text("Trek on GitHub").foregroundStyle(Trek.foreground)
                    Spacer()
                    Image(systemName: "arrow.up.right").font(.ui(.footnote).weight(.semibold)).foregroundStyle(Trek.muted)
                }
            }
        } header: {
            Text("About")
        } footer: {
            VStack(alignment: .leading, spacing: 8) {
                if let update, !update.canUpdate {
                    Text("Update available — your Mac will install it over Wi-Fi.").font(.ui(.footnote))
                }
                if AppSigning.expiry != nil {
                    Text("A free signing lasts 7 days. Your Mac signs Trek again when two days are left, whenever it can reach this iPhone.")
                        .font(.ui(.footnote))
                }
                Text("Trek is not affiliated with or endorsed by the makers of the agents it connects. Agent and provider names and logos are trademarks of their respective owners, shown only to identify the agent a thread uses.")
                    .font(.ui(.footnote))
            }
        }
    }
}

/// A machine's row in the Machines list: its badge, name, whether it's connected, and what it
/// runs (Trek's version, its agents and projects).
private struct MachineLabel: View {
    var link: HostLink
    @Environment(AppModel.self) private var model

    var body: some View {
        HStack(spacing: 14) {
            MachineMark(kind: link.kind, size: 20)
                .frame(width: 42, height: 42)
                .background(link.kind.tint.opacity(0.12), in: RoundedRectangle(cornerRadius: 11, style: .continuous))
            VStack(alignment: .leading, spacing: 3) {
                Text(link.name).font(.ui(.headline))
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    Circle().fill(MachineStatus.color(link, demo: model.mode == .demo)).frame(width: 7, height: 7)
                    Text(model.mode == .demo ? "Demo · sample data" : link.connectionText)
                        .font(.ui(.subheadline)).foregroundStyle(Trek.muted)
                        .fixedSize(horizontal: false, vertical: true)
                }
                if let detail {
                    Text(detail).font(.ui(.footnote)).foregroundStyle(Trek.muted)
                }
            }
            .alignmentGuide(.listRowSeparatorLeading) { $0[.leading] }
        }
        .padding(.vertical, 2)
    }

    /// "Trek 0.4.42 · 4 agents · 6 projects": nil until it has said.
    private var detail: String? {
        var parts: [String] = []
        if let host = link.host { parts.append("Trek \(host.version)") }
        if !link.agents.isEmpty { parts.append(link.agents.count == 1 ? "1 agent" : "\(link.agents.count) agents") }
        if !link.projects.isEmpty { parts.append(link.projects.count == 1 ? "1 project" : "\(link.projects.count) projects") }
        return parts.isEmpty ? nil : parts.joined(separator: " · ")
    }
}

/// One machine: what it is and how it's doing, its agents and their plans, how the phone reaches
/// it (its addresses and the connection's security), its Trek settings, what it tells this
/// iPhone, and Unpair.
struct MachineView: View {
    var link: HostLink
    @Environment(AppModel.self) private var model
    @State private var confirmUnpair = false

    private var paired: PairedMac { link.paired }
    private var routes: [String] {
        ([paired.address] + paired.routes).filter { !$0.isEmpty }.reduce(into: [String]()) { if !$0.contains($1) { $0.append($1) } }
    }
    /// "Mac", "PC": how its own words name it.
    private var word: String { link.kind == .mac || link.kind == .unknown ? "Mac" : "PC" }

    var body: some View {
        Form {
            Section {
                MachineSummary(link: link)
                    .listRowBackground(Color.clear)
                    .listRowInsets(EdgeInsets(top: 4, leading: 0, bottom: 4, trailing: 0))
            }

            agents

            if model.mode == .live {
                Section {
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
                } header: {
                    Text("Connection")
                } footer: {
                    Text("The phone tries each way to the \(word) at once; the one in use is ticked.")
                        .font(.ui(.footnote))
                }
            }

            Section {
                NavigationLink {
                    MacSettingsView(machine: link)
                } label: {
                    SettingsLabel(title: "Trek settings", subtitle: settingsLine, symbol: "slider.horizontal.3")
                }
            } footer: {
                Text("New threads, the composer, the inbox, hand-holding, and the \(word)'s own alerts and theme. Each change goes to the \(word) at once.")
                    .font(.ui(.footnote))
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
        .navigationBarTitleDisplayMode(.inline)
        .task {
            model.loadSettings(on: link)
            // Plans and what's left of them, for the agents' rows; asked quietly, once a visit.
            if link.usage == nil { model.loadUsage(quiet: true, on: link) }
        }
        .onChange(of: link.connection) { if link.connection == .connected { model.loadSettings(on: link) } }
        .confirmationDialog("Unpair this iPhone?", isPresented: $confirmUnpair, titleVisibility: .visible) {
            Button("Unpair", role: .destructive) { model.leave(link) }
        } message: {
            Text("You'll need a new code from \(link.name) to connect again. Revoke the device on it too to be sure. Other machines stay paired.")
        }
    }

    // MARK: Agents

    @ViewBuilder
    private var agents: some View {
        if !link.agents.isEmpty {
            Section {
                ForEach(link.agents) { agent in
                    AgentRow(agent: agent, link: link)
                }
                NavigationLink {
                    UsageView(host: link)
                } label: {
                    SettingsLabel(title: "Plan usage", subtitle: "Each plan's limits, and when they reset", symbol: "gauge.with.dots.needle.33percent")
                }
            } header: {
                Text("Agents")
            } footer: {
                Text("The agents set up on \(link.name), with the plan each is signed in with. Sign-ins stay on the \(word).")
                    .font(.ui(.footnote))
            }
        }
    }

    /// "Claude Code · Opus 5.5 · Auto-accept edits": what new threads start with here.
    private var settingsLine: String {
        guard let s = link.macSettings else { return link.waitingWords }
        let agent = link.agents.first { $0.key == s.defaultAgent }
        let name = agent?.name ?? s.defaultAgent
        let model = ModelNaming.current(s.defaultModel, agent: agent)?.label
        return ([name, model, s.defaultAccess.label].compactMap { $0 }).joined(separator: " · ")
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

/// The top of a machine's page: its mark, name, connection and Trek version, then its threads in
/// four figures.
private struct MachineSummary: View {
    var link: HostLink
    @Environment(AppModel.self) private var model

    private var threads: [ThreadSummary] { model.threads.filter { link.owns($0.id) } }

    var body: some View {
        VStack(spacing: 14) {
            VStack(spacing: 6) {
                MachineMark(kind: link.kind, size: 32)
                    .frame(width: 72, height: 72)
                    .background(Trek.elevatedGrouped, in: RoundedRectangle(cornerRadius: 18, style: .continuous))
                    .padding(.bottom, 4)
                Text(link.name)
                    .font(.ui(.title2).weight(.semibold))
                    .multilineTextAlignment(.center)
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    Circle().fill(MachineStatus.color(link, demo: model.mode == .demo)).frame(width: 7, height: 7)
                    Text(model.mode == .demo ? "Demo · sample data" : link.connectionText)
                        .font(.ui(.subheadline)).foregroundStyle(Trek.muted)
                        .multilineTextAlignment(.center)
                }
                if let line = versionLine {
                    Text(line).font(.ui(.footnote)).foregroundStyle(Trek.muted)
                }
            }
            .frame(maxWidth: .infinity)
            .accessibilityElement(children: .combine)

            HStack(spacing: 0) {
                figure(threads.count, threads.count == 1 ? "thread" : "threads")
                figure(threads.count { $0.atWork }, "working")
                figure(threads.count { $0.waitsOnYou && $0.section != .snoozed }, "need you")
                figure(link.projects.count, link.projects.count == 1 ? "project" : "projects")
            }
            .padding(.vertical, 12)
            .background(Trek.elevatedGrouped, in: RoundedRectangle(cornerRadius: 22, style: .continuous))
        }
    }

    /// "Trek 0.4.42 · macOS".
    private var versionLine: String? {
        let parts = [link.host.map { "Trek \($0.version)" }, link.kind == .mac ? "macOS" : link.kind.word].compactMap { $0 }
        return parts.isEmpty ? nil : parts.joined(separator: " · ")
    }

    private func figure(_ n: Int, _ label: String) -> some View {
        VStack(spacing: 2) {
            Text("\(n)").font(.ui(.title3).weight(.semibold).monospacedDigit()).foregroundStyle(Trek.foreground)
            Text(label).font(.ui(.caption).weight(.medium)).foregroundStyle(Trek.muted).lineLimit(1).minimumScaleFactor(0.8)
        }
        .frame(maxWidth: .infinity)
        .accessibilityElement(children: .combine)
    }
}

/// An agent on a machine: its logo and name ("Default" for the one new threads start with), the
/// plan it's signed in with and its default model, and what's left of its tightest limit.
private struct AgentRow: View {
    var agent: AgentOption
    var link: HostLink

    private var plan: ProviderUsage? { link.usage?.providers.first { $0.agent.key == agent.key } }
    private var isDefault: Bool { link.macSettings?.defaultAgent == agent.key }

    var body: some View {
        SettingsValueRow {
            if let left = left {
                Text(left).foregroundStyle(leftColor)
            }
        } label: {
            HStack(spacing: 12) {
                AgentGlyph(agent.ref, size: 30)
                VStack(alignment: .leading, spacing: 2) {
                    HStack(spacing: 6) {
                        Text(agent.name).foregroundStyle(Trek.foreground)
                        if isDefault {
                            Text("Default")
                                .font(.ui(.caption2).weight(.semibold))
                                .foregroundStyle(Trek.muted)
                                .padding(.horizontal, 6)
                                .padding(.vertical, 2)
                                .background(Trek.foreground.opacity(0.07), in: Capsule())
                        }
                    }
                    if let line {
                        Text(line).font(.ui(.footnote)).foregroundStyle(Trek.muted)
                    }
                }
                .alignmentGuide(.listRowSeparatorLeading) { $0[.leading] }
            }
        }
        .accessibilityElement(children: .combine)
    }

    /// "Claude Max · Opus 5.5".
    private var line: String? {
        let model = ModelNaming.defaultModel(agent)?.label
        let parts = [plan?.plan, model].compactMap { $0 }
        return parts.isEmpty ? nil : parts.joined(separator: " · ")
    }

    /// "58% left" on its tightest limit; nil when its plan has none (or hasn't said).
    private var left: String? {
        guard let t = plan?.tightest else { return nil }
        return "\(Int(max(0, 100 - t.percent).rounded()))% left"
    }

    /// Amber from 80% used, red from 95%, as the Mac's Usage button.
    private var leftColor: Color {
        let used = plan?.tightest?.percent ?? 0
        return used >= 95 ? Trek.failed : used >= 80 ? Trek.approval : Trek.muted
    }
}

extension Trek {
    /// A row's own fill in a grouped list (the system's, so it matches the rows around it).
    static let elevatedGrouped = Color(uiColor: .secondarySystemGroupedBackground)
}

extension HostLink {
    /// What a row says while this machine's settings haven't come: asked for, or waiting for the
    /// machine to be reachable (a row that said "Asking…" for an offline Mac would wait for ever).
    var waitingWords: String {
        connection == .connected ? "Asking \(name)…" : "Shown once \(MachineTag.short(name)) is connected"
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
