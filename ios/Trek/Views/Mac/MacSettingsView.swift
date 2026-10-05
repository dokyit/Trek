import SwiftUI
import UIKit

/// The Mac's settings the phone may change, grouped as the Mac's Settings pages group them:
/// General (new threads, composer, inbox), Permissions, Notifications and Appearance. Each change
/// shows at once and goes to the Mac, which puts it back (saying why) if it refuses. Full access
/// is unlocked, and API keys and the phone server set, on the Mac only.
struct MacSettingsView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Form {
            if let s = model.macSettings {
                MacSettingsForm(s: s)
            } else {
                HStack(spacing: 10) {
                    ProgressView().controlSize(.small)
                    Text("Asking your Mac…").foregroundStyle(Trek.muted)
                }
            }
        }
        .scrollContentBackground(.hidden)
        .background(Trek.background)
        .navigationTitle("Mac settings")
        .navigationSubtitle(model.host?.name ?? "")
        .task { model.loadSettings() }
    }
}

private struct MacSettingsForm: View {
    var s: MacSettings
    @Environment(AppModel.self) private var model

    private func set(_ change: SettingsChange) { model.changeSettings(change) }

    private var agentName: String { model.agents.first { $0.key == s.defaultAgent }?.name ?? s.defaultAgent }
    private var models: [ModelOption] { model.models(of: s.defaultAgent) }
    private var efforts: [String] { model.efforts(agent: s.defaultAgent, model: s.defaultModel) }

    var body: some View {
        Section {
            Picker("Agent", selection: Binding(get: { s.defaultAgent }, set: { set(SettingsChange(defaultAgent: $0)) })) {
                ForEach(model.agents) { a in Text(a.name).tag(a.key) }
                if !model.agents.contains(where: { $0.key == s.defaultAgent }) { Text(agentName).tag(s.defaultAgent) }
            }
            Picker("Model", selection: Binding(get: { s.defaultModel ?? "" }, set: { set(SettingsChange(defaultModel: $0)) })) {
                Text("Agent's default").tag("")
                ForEach(models) { m in Text(m.label).tag(m.id) }
                if let id = s.defaultModel, !models.contains(where: { $0.id == id }) { Text(ModelNaming.display(id)).tag(id) }
            }
            if !efforts.isEmpty {
                Picker("Reasoning effort", selection: Binding(get: { s.defaultEffort }, set: { set(SettingsChange(defaultEffort: $0)) })) {
                    ForEach(efforts, id: \.self) { e in Text(Effort.label(e)).tag(e) }
                    if !efforts.contains(s.defaultEffort) { Text(Effort.label(s.defaultEffort)).tag(s.defaultEffort) }
                }
            }
        } header: {
            Text("New threads")
        } footer: {
            Text("New threads on the Mac and here start with this agent and model. Leave the model on the agent's default to follow whatever it recommends.")
        }

        Section {
            VStack(alignment: .leading, spacing: 10) {
                Text("Messages sent while an agent works")
                Picker("Messages sent while an agent works", selection: Binding(get: { s.followUp }, set: { set(SettingsChange(followUp: $0)) })) {
                    ForEach(SendMode.allCases) { Text($0.label).tag($0) }
                }
                .pickerStyle(.segmented)
                .labelsHidden()
            }
            .padding(.vertical, 4)
        } header: {
            Text("Composer")
        } footer: {
            Text("Steer slips your message in at the agent's next step. Queue holds it until the turn ends. For messages sent on the Mac; this iPhone has its own under Follow-ups.")
        }

        Section {
            Picker("Settle finished threads", selection: Binding(get: { s.autoSettleDays }, set: { set(SettingsChange(autoSettleDays: $0)) })) {
                ForEach(Self.settleChoices, id: \.days) { c in Text(c.label).tag(c.days) }
                if !Self.settleChoices.contains(where: { $0.days == s.autoSettleDays }) {
                    Text("\(s.autoSettleDays) days").tag(s.autoSettleDays)
                }
            }
        } header: {
            Text("Inbox")
        } footer: {
            Text("Read threads leave the inbox after this long. Threads waiting on you never settle on their own.")
        }

        Section {
            NavigationLink {
                AccessPicker(current: s.defaultAccess, unlocked: s.fullAccess == true) { set(SettingsChange(defaultAccess: $0)) }
            } label: {
                LabeledContent("Hand-holding", value: s.defaultAccess.label)
            }
        } header: {
            Text("Permissions")
        } footer: {
            Text(s.fullAccess == true
                 ? "What new threads start with. Change it for any thread from its settings bar."
                 : "What new threads start with. Full access is locked: only your Mac can unlock it, in Settings › Permissions.")
        }

        Section {
            Picker("Alerts", selection: Binding(get: { s.notifications }, set: { set(SettingsChange(notifications: $0)) })) {
                ForEach([NotifyMode.bannerAndSound, .banner, .sound, .off]) { Text($0.label).tag($0) }
            }
        } header: {
            Text("Notifications on the Mac")
        } footer: {
            Text("When a turn finishes or an agent is waiting for your approval.")
        }

        Section {
            Picker("Theme", selection: Binding(get: { s.theme }, set: { set(SettingsChange(theme: $0)) })) {
                ForEach(MacTheme.allCases) { Text($0.label).tag($0) }
            }
            .pickerStyle(.segmented)
            .padding(.vertical, 2)
        } header: {
            Text("Appearance on the Mac")
        } footer: {
            Text("This iPhone's own theme is under Appearance.")
        }
    }

    private static let settleChoices: [(days: Int, label: String)] = [(0, "Never"), (1, "1 day"), (3, "3 days"), (7, "1 week")]
}

/// The Mac's hand-holding levels, each with what it means, the current one ticked. Full access
/// shows only once the Mac has unlocked it.
private struct AccessPicker: View {
    var current: Access
    var unlocked: Bool
    var pick: (Access) -> Void
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        Form {
            Section {
                ForEach(Access.allCases.filter { $0 != .fullAccess || unlocked || current == .fullAccess }) { a in
                    Button {
                        pick(a)
                        dismiss()
                    } label: {
                        HStack(spacing: 12) {
                            VStack(alignment: .leading, spacing: 3) {
                                Text(a.label).foregroundStyle(Trek.foreground)
                                Text(a.help).font(.footnote).foregroundStyle(Trek.muted)
                            }
                            Spacer()
                            if a == current {
                                Image(systemName: "checkmark").fontWeight(.semibold).foregroundStyle(Trek.foreground)
                            }
                        }
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityAddTraits(a == current ? .isSelected : [])
                }
            } footer: {
                Text("Change it for any thread from its settings bar.")
            }
        }
        .scrollContentBackground(.hidden)
        .background(Trek.background)
        .navigationTitle("Hand-holding")
        .navigationBarTitleDisplayMode(.inline)
    }
}

/// Settings › Notifications: the Mac tells this iPhone when a thread needs the user, through
/// the ntfy app. ntfy's iOS app opens no subscribe link, so the topic is copied and pasted.
struct PhoneNotificationsSection: View {
    @Environment(AppModel.self) private var model
    @State private var server = ""
    @State private var confirmNewTopic = false

    var body: some View {
        if let s = model.macSettings {
            sections(s.push)
        } else {
            Section {
                HStack(spacing: 10) {
                    ProgressView().controlSize(.small)
                    Text("Asking your Mac…").foregroundStyle(Trek.muted)
                }
            } header: {
                Text("Notifications")
            }
        }
    }

    @ViewBuilder
    private func sections(_ push: PushSettings) -> some View {
        Section {
            Toggle(isOn: Binding(get: { push.enabled }, set: { model.changeSettings(SettingsChange(push: $0)) })) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Tell my phone when a thread needs me")
                    Text("Also when one finishes or fails, through the free ntfy app")
                        .font(.footnote)
                        .foregroundStyle(Trek.muted)
                }
            }
            .tint(Trek.done)
            if push.enabled {
                Picker("Send them", selection: Binding(get: { push.when }, set: { model.changeSettings(SettingsChange(pushWhen: $0)) })) {
                    ForEach(PushWhen.allCases) { Text($0.label).tag($0) }
                }
            }
        } header: {
            Text("Notifications")
        } footer: {
            if push.enabled {
                Text("Away means no keyboard or mouse on your Mac for two minutes, or its screen locked. Tapping a notification opens the thread here.")
            }
        }

        if push.enabled, !push.topic.isEmpty {
            Section {
                VStack(alignment: .leading, spacing: 6) {
                    Text("Topic").font(.footnote).foregroundStyle(Trek.muted)
                    Text(push.topic)
                        .font(.system(.subheadline, design: .monospaced))
                        .textSelection(.enabled)
                        .lineLimit(1)
                        .minimumScaleFactor(0.6)
                        .padding(.horizontal, 10)
                        .padding(.vertical, 7)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .background(Trek.foreground.opacity(0.06), in: RoundedRectangle(cornerRadius: 9, style: .continuous))
                }
                .padding(.vertical, 2)
                Button("Copy topic", systemImage: "doc.on.doc") {
                    UIPasteboard.general.string = push.topic
                    model.show("Topic copied")
                }
                Button("Send a test", systemImage: "bell.badge") {
                    model.changeSettings(SettingsChange(pushTest: true)) {
                        model.show("Sent. It should arrive through ntfy in a moment.")
                    }
                }
                VStack(alignment: .leading, spacing: 8) {
                    step(1, "Copy the topic.")
                    step(2, "Open ntfy and tap +.")
                    step(3, "Paste the topic and tap Subscribe.")
                    if !Self.isDefault(push.server) {
                        step(4, "Turn on “Use another server” and enter \(push.server).")
                    }
                }
                .padding(.vertical, 4)
            } header: {
                Text("Set up ntfy")
            } footer: {
                Text("Keep the topic to yourself: anyone who has it can read what's sent.")
            }

            Section {
                TextField("Server", text: $server, prompt: Text("https://ntfy.sh"))
                    .keyboardType(.URL)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .submitLabel(.done)
                    .onSubmit {
                        let value = server.trimmingCharacters(in: .whitespaces)
                        if !value.isEmpty, value != push.server { model.changeSettings(SettingsChange(pushServer: value)) } else { server = push.server }
                    }
                Button("New topic", systemImage: "arrow.triangle.2.circlepath", role: .destructive) { confirmNewTopic = true }
            } header: {
                Text("ntfy server")
            } footer: {
                Text("The free ntfy.sh, or your own server.")
            }
            .onAppear { server = push.server }
            .onChange(of: push.server) { server = push.server }
            .confirmationDialog("Make a new topic?", isPresented: $confirmNewTopic, titleVisibility: .visible) {
                Button("New topic", role: .destructive) { model.changeSettings(SettingsChange(newPushTopic: true)) }
            } message: {
                Text("The old topic stops getting notifications. Subscribe to the new one in ntfy.")
            }
        }
    }

    private func step(_ n: Int, _ text: String) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            Text("\(n)")
                .font(.caption.weight(.bold).monospacedDigit())
                .foregroundStyle(Trek.foreground)
                .frame(width: 20, height: 20)
                .background(Trek.foreground.opacity(0.08), in: Circle())
            Text(text).font(.subheadline)
        }
    }

    private static func isDefault(_ server: String) -> Bool {
        let s = server.lowercased().trimmingCharacters(in: CharacterSet(charactersIn: "/ "))
        return s == "https://ntfy.sh" || s == "ntfy.sh"
    }
}
