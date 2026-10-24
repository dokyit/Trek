import PhotosUI
import SwiftUI

/// "What are we building?": pick a project, an agent and model, worktree or local; then describe it.
struct NewThreadSheet: View {
    var opened: (String) -> Void
    /// The machine the thread starts on (the picked one when not handed in, e.g. by `/new`).
    @State private var machine: HostLink?
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
    init(project: String? = nil, prompt: String = "", on machine: HostLink? = nil, opened: @escaping (String) -> Void) {
        self.opened = opened
        _machine = State(initialValue: machine)
        _projectID = State(initialValue: project.flatMap { $0.isEmpty ? nil : $0 })
        _text = State(initialValue: prompt)
    }

    private var link: HostLink? { machine ?? model.selected }
    /// This machine's projects (their qualified ids, as the shared list carries them).
    private var linkProjects: [ProjectSummary] {
        guard let link else { return [] }
        return model.projects.filter { link.owns($0.id) }
    }
    private var agents: [AgentOption] { link?.agents ?? [] }
    /// "No project" on this machine: its prefix with nothing after it, which routes the request
    /// to it and reaches it as `""` (a thread of no project, as the Mac's "No project" starts).
    private var noProjectID: String? { link?.qualified("") }
    private var isNoProject: Bool { projectID != nil && projectID == noProjectID }
    private var project: ProjectSummary? {
        isNoProject ? nil : linkProjects.first { $0.id == projectID } ?? linkProjects.first
    }
    private var agent: AgentOption? { agents.first { $0.key == agentKey } ?? agents.first }
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
                        .font(.ui(size: 28, weight: .semibold))
                        .tracking(-0.5)
                    if let host = link?.name {
                        Text("Runs on \(host)").font(.ui(.subheadline)).foregroundStyle(Trek.muted)
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
            projectID = linkProjects.contains(where: { $0.id == projectID }) || isNoProject
                ? projectID : linkProjects.first?.id ?? noProjectID
            agentKey = agents.contains(where: { $0.key == agentKey }) ? agentKey : agents.first?.key
            focused = true
        }
    }

    private var card: some View {
        VStack(alignment: .leading, spacing: 12) {
            if !photos.isEmpty { PhotoStrip(photos: $photos) }
            TextField("Describe the task", text: $text, axis: .vertical)
                .lineLimit(2...8)
                .font(.ui(size: 17))
                .focused($focused)
                .padding(.horizontal, 6)
                .padding(.top, 6)
            HStack(spacing: 8) {
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 8) {
                        photoChip
                        if model.links.count > 1 { machineChip }
                        projectChip
                        agentChip
                        accessChip
                        planChip
                        if !isNoProject, project?.isRepo ?? true { worktreeChip }
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
                ForEach(Access.allCases.filter { $0 != .fullAccess || link?.fullAccessAllowed == true }) { level in
                    Label(level.label, systemImage: level.icon).tag(Optional(level))
                }
            }
        } label: {
            chip {
                Image(systemName: (access ?? .autoAcceptEdits).icon).font(.system(size: 13, weight: .medium))
                    .foregroundStyle(access?.tint ?? Trek.muted)
                Text(access?.label ?? "Access")
            }
        }
    }

    private var planChip: some View {
        Button { withAnimation(.snappy(duration: 0.15)) { plan.toggle() } } label: {
            chip(selected: plan) {
                Image(systemName: "checklist").font(.system(size: 13, weight: .medium))
                    .foregroundStyle(plan ? Trek.plan : Trek.muted)
                Text("Plan")
            }
        }
        .buttonStyle(.plain)
        .accessibilityLabel(plan ? "Plan first: on" : "Plan first: off")
    }

    private var empty: Bool { text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }

    /// Which machine the thread runs on (several paired only).
    private var machineChip: some View {
        Menu {
            Picker("Machine", selection: Binding(get: { link?.id }, set: { id in
                machine = model.links.first { $0.id == id }
                projectID = linkProjects.first?.id ?? noProjectID
                agentKey = agents.first?.key
            })) {
                ForEach(model.links) { l in
                    Text(l.kind.word.map { "\(l.name) · \($0)" } ?? l.name).tag(Optional(l.id))
                }
            }
        } label: {
            chip {
                MachineMark(kind: link?.kind ?? .unknown, size: 13)
                Text(link?.name ?? "Machine")
            }
        }
    }

    private var projectChip: some View {
        Menu {
            Picker("Project", selection: Binding(get: { isNoProject ? noProjectID : project?.id }, set: { projectID = $0 })) {
                ForEach(linkProjects) { p in
                    Text(p.name).tag(Optional(p.id))
                }
                Label("No project", systemImage: "tray").tag(noProjectID)
            }
        } label: {
            chip {
                if let p = project {
                    ProjectBadge(project: p.ref, size: 18)
                    Text(p.name)
                } else if isNoProject {
                    Image(systemName: "tray").font(.system(size: 13, weight: .medium))
                    Text("No project")
                } else {
                    Text("Project")
                }
            }
        }
    }

    /// Logo, model and effort, like the thread's chip; opens the same picker.
    private var agentChip: some View {
        Button { choosingModel = true } label: {
            chip { ModelChipLabel(agentKey: agent?.key ?? "", agentLogo: agent?.logo, model: modelLabel, effort: effort, logo: 17) }
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Model: \(modelLabel)")
        .sheet(isPresented: $choosingModel) {
            ModelPickerSheet(
                agents: agents,
                agentKey: agent?.key ?? "",
                modelID: modelID ?? ModelNaming.defaultModel(agent)?.id,
                effort: effort,
                efforts: { key, id in ModelNaming.current(id, agent: agents.first { $0.key == key })?.efforts ?? [] },
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

    /// A chip on a neutral wash (the Mac's composer pills), a shade darker when `selected`.
    private func chip<Content: View>(selected: Bool = false, @ViewBuilder _ content: () -> Content) -> some View {
        HStack(spacing: 6) { content() }
            .font(.ui(.subheadline).weight(.medium))
            .foregroundStyle(Trek.foreground)
            .padding(.horizontal, 12)
            .frame(height: 36)
            .background(Trek.foreground.opacity(selected ? 0.12 : 0.06), in: Capsule())
    }

    private func start() {
        guard let pid = project?.id ?? (isNoProject ? noProjectID : nil), let agent else { return }
        sending = true
        model.newThread(project: pid, agent: agent.key, model: modelID ?? ModelNaming.defaultModel(agent)?.id,
                        text: text.trimmingCharacters(in: .whitespacesAndNewlines), worktree: worktree && (project?.isRepo ?? false),
                        effort: effort, access: access, plan: plan, images: photos.compactMap(\.upload)) { tid in
            dismiss()
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.35) { opened(tid) }
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 5) { sending = false }
    }
}
