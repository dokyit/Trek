import SwiftUI

/// Above the composer: what the thread runs with, each a chip that changes it, as the Mac's
/// composer does. The model (or another agent), its effort, how much the agent may do without
/// asking, and plan mode. Full access, when the Mac allows it at all, asks for Face ID first.
struct ThreadSettingsBar: View {
    var thread: ThreadSummary
    @Environment(AppModel.self) private var model
    @State private var confirmingFull = false

    @State private var picking = false

    private var agent: AgentOption? { model.link(for: thread.id)?.agents.first { $0.key == thread.agent.key } }
    private var fullAccessAllowed: Bool { model.link(for: thread.id)?.fullAccessAllowed ?? false }
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
                ModelChipLabel(agentKey: thread.agent.key, agentLogo: thread.agent.logo, model: modelName, effort: thread.effort)
            }
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Model: \(modelName)\(thread.effort.map { ", effort \(Effort.label($0))" } ?? "")")
        .sheet(isPresented: $picking) {
            ModelPickerSheet(
                agents: model.link(for: thread.id)?.agents ?? [],
                agentKey: thread.agent.key,
                modelID: thread.model ?? current?.id,
                effort: thread.effort,
                efforts: { agentKey, modelID in
                    let a = model.link(for: thread.id)?.agents.first { $0.key == agentKey }
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
                        Text(level == .fullAccess && !fullAccessAllowed ? "Locked on your Mac" : level.help)
                    } icon: {
                        Image(systemName: level == access ? "checkmark" : level.icon)
                    }
                }
                .disabled(level == .fullAccess && !fullAccessAllowed)
            }
        } label: {
            chip {
                Image(systemName: access.icon).font(.system(size: 12, weight: .semibold)).foregroundStyle(access.tint ?? Trek.muted)
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
            chip(selected: plan) {
                Image(systemName: "checklist").font(.system(size: 12, weight: .semibold)).foregroundStyle(plan ? Trek.plan : Trek.muted)
                Text("Plan")
            }
        }
        .buttonStyle(.plain)
        .accessibilityLabel(plan ? "Plan mode on" : "Plan mode off")
    }

    /// A pill as the Mac's composer has them: neutral, its icon tinted where it means something,
    /// a shade darker when `selected` (plan mode on).
    private func chip<Content: View>(selected: Bool = false, @ViewBuilder _ content: () -> Content) -> some View {
        HStack(spacing: 5) { content() }
            .font(.ui(.footnote).weight(.medium))
            .foregroundStyle(Trek.foreground)
            .padding(.horizontal, 11)
            .frame(height: 30)
            .glassEffect(selected ? .regular.tint(Trek.foreground.opacity(0.12)).interactive() : .regular.interactive(), in: Capsule())
    }
}
