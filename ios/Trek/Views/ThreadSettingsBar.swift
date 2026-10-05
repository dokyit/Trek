import SwiftUI

/// Above the composer: what the thread runs with, each a chip that changes it, as the Mac's
/// composer does. The model (or another agent), its effort, how much the agent may do without
/// asking, and plan mode. Full access, when the Mac allows it at all, asks for Face ID first.
struct ThreadSettingsBar: View {
    var thread: ThreadSummary
    @Environment(AppModel.self) private var model
    @State private var confirmingFull = false

    private var efforts: [String] { model.efforts(agent: thread.agent.key, model: thread.model) }
    private var access: Access { thread.access ?? .autoAcceptEdits }
    private var plan: Bool { thread.plan ?? false }

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                modelChip
                if !efforts.isEmpty { effortChip }
                accessChip
                planChip
            }
            .padding(.horizontal, 2)
        }
        .scrollClipDisabled()
    }

    private var modelChip: some View {
        Menu {
            Section(thread.agent.name) {
                ForEach(model.models(of: thread.agent.key)) { m in
                    Button {
                        model.setPrefs(thread.id, model: m.id)
                    } label: {
                        if m.id == thread.model { Label(m.label, systemImage: "checkmark") } else { Text(m.label) }
                    }
                }
            }
            let others = model.agents.filter { $0.key != thread.agent.key }
            if !others.isEmpty {
                Menu("Switch agent") {
                    ForEach(others) { a in
                        Button(a.name) { model.setPrefs(thread.id, agent: a.key) }
                    }
                }
            }
        } label: {
            chip {
                AgentGlyph(key: thread.agent.key, size: 15)
                Text(thread.modelLabel ?? thread.model ?? thread.agent.name).lineLimit(1)
                Image(systemName: "chevron.down").font(.system(size: 9, weight: .bold)).foregroundStyle(Trek.muted)
            }
        }
        .accessibilityLabel("Model: \(thread.modelLabel ?? thread.agent.name)")
    }

    private var effortChip: some View {
        Menu {
            Picker("Effort", selection: Binding(get: { thread.effort ?? "" }, set: { model.setPrefs(thread.id, effort: $0) })) {
                ForEach(efforts, id: \.self) { e in Text(Effort.label(e)).tag(e) }
            }
        } label: {
            chip {
                Image(systemName: "gauge.with.dots.needle.67percent").font(.system(size: 12, weight: .medium))
                Text(thread.effort.map(Effort.label) ?? "Effort")
                Image(systemName: "chevron.down").font(.system(size: 9, weight: .bold)).foregroundStyle(Trek.muted)
            }
        }
        .accessibilityLabel("Effort: \(thread.effort.map(Effort.label) ?? "default")")
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
            chip(tint: access == .fullAccess ? Trek.failed : nil) {
                Image(systemName: access.icon).font(.system(size: 12, weight: .medium))
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
                Image(systemName: "list.bullet.clipboard").font(.system(size: 12, weight: .medium))
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
            .glassEffect(tint.map { .regular.tint($0.opacity(0.18)).interactive() } ?? .regular.interactive(), in: Capsule())
    }
}
