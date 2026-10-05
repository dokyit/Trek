import SwiftUI

/// Above the composer: what the thread runs with, each a chip that changes it, as the Mac's
/// composer does. The model (or another agent), its effort, how much the agent may do without
/// asking, and plan mode. Full access, when the Mac allows it at all, asks for Face ID first.
struct ThreadSettingsBar: View {
    var thread: ThreadSummary
    @Environment(AppModel.self) private var model
    @State private var confirmingFull = false

    @State private var picking = false

    private var agent: AgentOption? { model.agents.first { $0.key == thread.agent.key } }
    private var current: ModelOption? { ModelNaming.current(thread.model, agent: agent) }
    private var access: Access { thread.access ?? .autoAcceptEdits }
    private var plan: Bool { thread.plan ?? false }
    /// The model's name: the Mac's label, else the agent's default model, else the agent.
    private var modelName: String {
        if let label = thread.modelLabel, !label.isEmpty, thread.model != nil { return label }
        return current?.label ?? thread.agent.name
    }

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                modelChip
                accessChip
                planChip
            }
            .padding(.horizontal, 2)
        }
        .scrollClipDisabled()
        .onAppear { if Launch.sheet == "model" { picking = true } }
    }

    /// Logo, model and effort, like the Mac's composer pill; opens the model picker.
    private var modelChip: some View {
        Button {
            picking = true
        } label: {
            chip {
                ModelChipLabel(agentKey: thread.agent.key, model: modelName, effort: thread.effort)
            }
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Model: \(modelName)\(thread.effort.map { ", effort \(Effort.label($0))" } ?? "")")
        .sheet(isPresented: $picking) {
            ModelPickerSheet(
                agents: model.agents,
                agentKey: thread.agent.key,
                modelID: thread.model ?? current?.id,
                effort: thread.effort,
                efforts: { agentKey, modelID in
                    let a = model.agents.first { $0.key == agentKey }
                    return ModelNaming.current(modelID, agent: a)?.efforts ?? []
                },
                pickModel: { agentKey, modelID in
                    if agentKey != thread.agent.key {
                        // Another provider: switch the agent and pick its model in one request.
                        model.setPrefs(thread.id, agent: agentKey, model: modelID)
                    } else if modelID != thread.model {
                        model.setPrefs(thread.id, model: modelID)
                    }
                },
                pickEffort: { model.setPrefs(thread.id, effort: $0) })
        }
    }

    private var accessChip: some View {
        Menu {
            ForEach(Access.allCases) { level in
                Button {
                    if level == .fullAccess { confirmingFull = true } else { model.setPrefs(thread.id, access: level) }
                } label: {
                    Label {
                        Text(level.label)
                        Text(level == .fullAccess && !model.fullAccessAllowed ? "Locked on your Mac" : level.help)
                    } icon: {
                        Image(systemName: level == access ? "checkmark" : level.icon)
                    }
                }
                .disabled(level == .fullAccess && !model.fullAccessAllowed)
            }
        } label: {
            chip(tint: access.tint) {
                Image(systemName: access.icon).font(.system(size: 12, weight: .semibold))
                Text(access.label)
                Image(systemName: "chevron.down").font(.system(size: 9, weight: .bold)).foregroundStyle(Trek.muted)
            }
        }
        .accessibilityLabel("Access: \(access.label)")
        .task(id: confirmingFull) {
            guard confirmingFull else { return }
            confirmingFull = false
            switch await DeviceOwner.confirm("Let \(thread.agent.name) run with no prompts and no sandbox") {
            case .success: model.setPrefs(thread.id, access: .fullAccess)
            case .failure(let e): if let m = e.message { model.show(m, error: true) }
            }
        }
    }

    private var planChip: some View {
        Button {
            model.setPrefs(thread.id, plan: !plan)
        } label: {
            chip(tint: plan ? Trek.plan : nil) {
                Image(systemName: plan ? "list.bullet.clipboard.fill" : "list.bullet.clipboard").font(.system(size: 12, weight: .semibold))
                Text("Plan")
            }
        }
        .buttonStyle(.plain)
        .accessibilityLabel(plan ? "Plan mode on" : "Plan mode off")
    }

    private func chip<Content: View>(tint: Color? = nil, @ViewBuilder _ content: () -> Content) -> some View {
        HStack(spacing: 5) { content() }
            .font(.footnote.weight(.medium))
            .foregroundStyle(tint ?? Trek.foreground)
            .padding(.horizontal, 11)
            .frame(height: 30)
            .glassEffect(tint.map { .regular.tint($0.opacity(0.16)).interactive() } ?? .regular.interactive(), in: Capsule())
            .overlay {
                if let tint { Capsule().strokeBorder(tint.opacity(0.35), lineWidth: 0.75) }
            }
    }
}
