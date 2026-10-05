import SwiftUI

/// "What are we building?": pick a project, an agent and model, worktree or local; then describe it.
struct NewThreadSheet: View {
    var opened: (String) -> Void
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var text = ""
    @State private var projectID: String?
    @State private var agentKey: String?
    @State private var modelID: String?
    @State private var worktree = true
    @State private var sending = false
    @FocusState private var focused: Bool

    private var project: ProjectSummary? { model.project(projectID) ?? model.projects.first }
    private var agent: AgentOption? { model.agents.first { $0.key == agentKey } ?? model.agents.first }
    private var modelLabel: String {
        let id = modelID ?? agent?.defaultModel
        return agent?.models.first { $0.id == id }?.label ?? agent?.name ?? "Agent"
    }

    var body: some View {
        NavigationStack {
            VStack(spacing: 0) {
                Spacer()
                VStack(spacing: 16) {
                    Image("TrekMark")
                        .resizable()
                        .scaledToFit()
                        .frame(width: 54, height: 47)
                        .shadow(color: Color(hex: 0xFF8A3D).opacity(0.45), radius: 18)
                    Text("What are we building?")
                        .font(.system(size: 28, weight: .semibold))
                        .tracking(-0.5)
                    if let host = model.host?.name {
                        Text("Runs on \(host)").font(.subheadline).foregroundStyle(Trek.muted)
                    }
                }
                Spacer()
                card
                    .padding(.horizontal, 14)
                    .padding(.bottom, 10)
            }
            .background(alignment: .top) {
                RidgeBackdrop(height: 520)
            }
            .navigationTitle("New thread")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button { dismiss() } label: { Image(systemName: "xmark") }
                }
            }
        }
        .onAppear {
            projectID = projectID ?? model.projects.first?.id
            agentKey = agentKey ?? model.agents.first?.key
            focused = true
        }
    }

    private var card: some View {
        VStack(alignment: .leading, spacing: 12) {
            TextField("Describe the task", text: $text, axis: .vertical)
                .lineLimit(2...8)
                .font(.system(size: 17))
                .focused($focused)
                .padding(.horizontal, 6)
                .padding(.top, 6)
            HStack(spacing: 8) {
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 8) {
                        projectChip
                        agentChip
                        if project?.isRepo ?? true { worktreeChip }
                    }
                }
                .mask(LinearGradient(stops: [.init(color: .black, location: 0), .init(color: .black, location: 0.88),
                                             .init(color: .clear, location: 1)], startPoint: .leading, endPoint: .trailing))
                Button(action: start) {
                    Image(systemName: "arrow.up")
                        .font(.system(size: 16, weight: .bold))
                        .foregroundStyle(Trek.background)
                        .frame(width: 36, height: 36)
                        .background(Circle().fill(empty ? Trek.muted.opacity(0.35) : Trek.foreground))
                }
                .buttonStyle(.plain)
                .disabled(empty || sending)
                .accessibilityLabel("Start thread")
            }
        }
        .padding(12)
        .glassEffect(.regular, in: RoundedRectangle(cornerRadius: 26, style: .continuous))
    }

    private var empty: Bool { text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }

    private var projectChip: some View {
        Menu {
            Picker("Project", selection: Binding(get: { project?.id }, set: { projectID = $0 })) {
                ForEach(model.projects) { p in
                    Text(p.name).tag(Optional(p.id))
                }
            }
        } label: {
            chip {
                if let p = project {
                    ProjectBadge(project: p.ref, size: 18)
                    Text(p.name)
                } else {
                    Text("Project")
                }
            }
        }
    }

    private var agentChip: some View {
        Menu {
            ForEach(model.agents) { a in
                Menu {
                    ForEach(a.models) { m in
                        Button {
                            agentKey = a.key
                            modelID = m.id
                        } label: {
                            if a.key == agent?.key && (modelID ?? a.defaultModel) == m.id {
                                Label(m.label, systemImage: "checkmark")
                            } else {
                                Text(m.label)
                            }
                        }
                    }
                } label: {
                    Text(a.name)
                }
            }
        } label: {
            chip {
                AgentGlyph(key: agent?.key ?? "", size: 17)
                Text(modelLabel)
            }
        }
    }

    private var worktreeChip: some View {
        Button {
            withAnimation(.snappy(duration: 0.15)) { worktree.toggle() }
        } label: {
            chip {
                Image(systemName: worktree ? "arrow.triangle.branch" : "laptopcomputer")
                    .font(.system(size: 13, weight: .medium))
                Text(worktree ? "New worktree" : "Local")
            }
        }
        .buttonStyle(.plain)
    }

    private func chip<Content: View>(@ViewBuilder _ content: () -> Content) -> some View {
        HStack(spacing: 6) { content() }
            .font(.subheadline.weight(.medium))
            .foregroundStyle(Trek.foreground)
            .padding(.horizontal, 12)
            .frame(height: 36)
            .background(Trek.foreground.opacity(0.06), in: Capsule())
    }

    private func start() {
        guard let project, let agent else { return }
        sending = true
        model.newThread(project: project.id, agent: agent.key, model: modelID ?? agent.defaultModel,
                        text: text.trimmingCharacters(in: .whitespacesAndNewlines), worktree: worktree && project.isRepo) { tid in
            dismiss()
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.35) { opened(tid) }
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 5) { sending = false }
    }
}
