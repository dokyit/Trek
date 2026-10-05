import PhotosUI
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
    @State private var effort: String?
    @State private var access: Access?
    @State private var plan = false
    @State private var photos: [PickedPhoto] = []
    @State private var picking = false
    @State private var picked: [PhotosPickerItem] = []
    @State private var sending = false
    @State private var choosingModel = false
    @FocusState private var focused: Bool

    /// `project` and `prompt` start it filled in (`/new` sent from a thread: its project, and what
    /// was typed after the command).
    init(project: String? = nil, prompt: String = "", opened: @escaping (String) -> Void) {
        self.opened = opened
        _projectID = State(initialValue: project.flatMap { $0.isEmpty ? nil : $0 })
        _text = State(initialValue: prompt)
    }

    private var project: ProjectSummary? { model.project(projectID) ?? model.projects.first }
    private var agent: AgentOption? { model.agents.first { $0.key == agentKey } ?? model.agents.first }
    private var modelLabel: String {
        ModelNaming.current(modelID, agent: agent)?.label ?? agent?.name ?? "Agent"
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
            if !photos.isEmpty { PhotoStrip(photos: $photos) }
            TextField("Describe the task", text: $text, axis: .vertical)
                .lineLimit(2...8)
                .font(.system(size: 17))
                .focused($focused)
                .padding(.horizontal, 6)
                .padding(.top, 6)
            HStack(spacing: 8) {
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 8) {
                        photoChip
                        projectChip
                        agentChip
                        accessChip
                        planChip
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
        .photosPicker(isPresented: $picking, selection: $picked, maxSelectionCount: 4, matching: .images)
        .onChange(of: picked) {
            let items = picked
            picked = []
            Task {
                let loaded = await PickedPhoto.load(items)
                withAnimation(.snappy) { photos.append(contentsOf: loaded) }
            }
        }
    }

    private var photoChip: some View {
        Button { picking = true } label: {
            chip { Image(systemName: "photo.on.rectangle").font(.system(size: 14, weight: .medium)) }
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Add photos")
    }

    private var accessChip: some View {
        Menu {
            Picker("Access", selection: Binding(get: { access }, set: { access = $0 })) {
                Text("Default").tag(Access?.none)
                ForEach(Access.allCases.filter { $0 != .fullAccess || model.fullAccessAllowed }) { level in
                    Label(level.label, systemImage: level.icon).tag(Optional(level))
                }
            }
        } label: {
            chip(tint: access?.tint) {
                Image(systemName: (access ?? .autoAcceptEdits).icon).font(.system(size: 13, weight: .medium))
                Text(access?.label ?? "Access")
            }
        }
    }

    private var planChip: some View {
        Button { withAnimation(.snappy(duration: 0.15)) { plan.toggle() } } label: {
            chip(tint: plan ? Trek.plan : nil) {
                Image(systemName: plan ? "list.bullet.clipboard.fill" : "list.bullet.clipboard").font(.system(size: 13, weight: .medium))
                Text("Plan")
            }
        }
        .buttonStyle(.plain)
        .accessibilityLabel(plan ? "Plan first: on" : "Plan first: off")
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

    /// Logo, model and effort, like the thread's chip; opens the same picker.
    private var agentChip: some View {
        Button { choosingModel = true } label: {
            chip { ModelChipLabel(agentKey: agent?.key ?? "", model: modelLabel, effort: effort, logo: 17) }
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Model: \(modelLabel)")
        .sheet(isPresented: $choosingModel) {
            ModelPickerSheet(
                agents: model.agents,
                agentKey: agent?.key ?? "",
                modelID: modelID ?? ModelNaming.defaultModel(agent)?.id,
                effort: effort,
                efforts: { key, id in ModelNaming.current(id, agent: model.agents.first { $0.key == key })?.efforts ?? [] },
                pickModel: { key, id in
                    agentKey = key
                    modelID = id
                },
                pickEffort: { effort = $0 })
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

    /// A chip on a neutral wash, or on a wash of `tint` with a rim of it (access level, plan on).
    private func chip<Content: View>(tint: Color? = nil, @ViewBuilder _ content: () -> Content) -> some View {
        HStack(spacing: 6) { content() }
            .font(.subheadline.weight(.medium))
            .foregroundStyle(tint ?? Trek.foreground)
            .padding(.horizontal, 12)
            .frame(height: 36)
            .background((tint ?? Trek.foreground).opacity(tint == nil ? 0.06 : 0.13), in: Capsule())
            .overlay {
                if let tint { Capsule().strokeBorder(tint.opacity(0.3), lineWidth: 0.75) }
            }
    }

    private func start() {
        guard let project, let agent else { return }
        sending = true
        model.newThread(project: project.id, agent: agent.key, model: modelID ?? ModelNaming.defaultModel(agent)?.id,
                        text: text.trimmingCharacters(in: .whitespacesAndNewlines), worktree: worktree && project.isRepo,
                        effort: effort, access: access, plan: plan, images: photos.compactMap(\.upload)) { tid in
            dismiss()
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.35) { opened(tid) }
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 5) { sending = false }
    }
}
