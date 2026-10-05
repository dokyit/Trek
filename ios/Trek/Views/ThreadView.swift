import SwiftUI

struct ThreadView: View {
    var threadID: String
    @Environment(AppModel.self) private var model
    @State private var draft = ""
    @State private var photos: [PickedPhoto] = []
    @State private var mode: SendMode = .steer
    @State private var renaming = false
    @State private var newTitle = ""
    @State private var archiving = false
    @State private var groupOpen: [String: Bool] = [:]
    /// Each changes card's folded folders, by item.
    @State private var folded: [String: Set<String>] = [:]
    @State private var showingGit = false
    /// What followed `/new` in the message just sent: the new thread's prompt, if the Mac asks
    /// for the sheet.
    @State private var newPrompt = ""
    @State private var newThread: NewThreadRequest?

    private var thread: ThreadSummary? { model.thread(threadID) }
    private var items: [TItem] { model.transcripts[threadID] ?? [] }

    var body: some View {
        let blocks = Perf.measure("ThreadView.body Block.build", "\(items.count) items") { Block.build(items) }
        let lastGroup = blocks.last { if case .group = $0 { true } else { false } }?.id
        let working = thread?.runState == .working
        let commands = CommandPicker.query(draft).map { CommandPicker.matches(model.commands[threadID] ?? [], $0) } ?? []
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 18) {
                if !model.loadedTranscripts.contains(threadID) {
                    ProgressView().frame(maxWidth: .infinity).padding(.top, 80)
                }
                ForEach(blocks) { block in
                    blockView(block, isLatestGroup: block.id == lastGroup && working)
                        .id(block.id)
                }
                if working, let t = thread {
                    WorkingLine(threadID: t.id, agentKey: t.agent.key, since: t.workingSince, activity: t.activity)
                        .padding(.top, 2)
                        .transaction { $0.animation = nil }
                }
                Color.clear.frame(height: 4).id("end")
            }
            .padding(.horizontal, 20)
            .padding(.top, 12)
            .padding(.bottom, 16)
        }
        .environment(\.projectHue, thread?.project?.hue)
        .defaultScrollAnchor(.bottom)
        .scrollDismissesKeyboard(.interactively)
        .background(Trek.background)
        .safeAreaBar(edge: .top, spacing: 0) {
            if let t = thread {
                ThreadStatusStrip(thread: t)
                    .padding(.horizontal, 14)
                    .padding(.vertical, 6)
            }
        }
        .safeAreaInset(edge: .bottom, spacing: 0) {
            VStack(spacing: 10) {
                ForEach(items.filter(\.isPendingRequest)) { item in
                    PendingRequestCard(item: item, agentName: thread?.agent.name ?? "The agent") { response in
                        if let rid = requestId(item) { model.answer(threadID, requestId: rid, response) }
                    }
                    .transition(.move(edge: .bottom).combined(with: .opacity))
                }
                if !commands.isEmpty {
                    CommandPicker(commands: model.commands[threadID] ?? [], query: CommandPicker.query(draft) ?? "") { c in
                        draft = "/\(c.name) "
                    }
                    .transition(.move(edge: .bottom).combined(with: .opacity))
                } else if let t = thread {
                    ThreadSettingsBar(thread: t)
                }
                Composer(text: $draft.timed("keystroke"), mode: $mode, photos: $photos, placeholder: "Message \(thread?.agent.name ?? "the agent")",
                         working: working, send: send, stop: { model.interrupt(threadID) })
            }
            .padding(.horizontal, 14)
            .padding(.bottom, 6)
            .padding(.top, 8)
            .animation(.smooth(duration: 0.3), value: items.filter(\.isPendingRequest).map(\.id))
            .animation(.snappy(duration: 0.2), value: commands.isEmpty)
        }
        .navigationTitle(thread?.title ?? "Thread")
        .navigationSubtitle(subtitle)
        .navigationBarTitleDisplayMode(.inline)
        .toolbarVisibility(.hidden, for: .tabBar)
        .toolbar {
            if let t = thread, t.git != nil || t.branch != nil {
                ToolbarItem(placement: .topBarTrailing) {
                    Button { showingGit = true } label: { GitButtonLabel(changed: t.git?.changed ?? 0) }
                }
                ToolbarSpacer(.fixed, placement: .topBarTrailing)
            }
            ToolbarItem(placement: .topBarTrailing) {
                Menu {
                    if let t = thread {
                        Section {
                            Label(t.agent.name + (t.modelLabel.map { " · \($0)" } ?? ""), systemImage: "cpu")
                            if let b = t.branch { Label(b, systemImage: "arrow.triangle.branch") }
                            if t.additions + t.deletions > 0 { Label("+\(t.additions) −\(t.deletions)", systemImage: "plus.forwardslash.minus") }
                        }
                    }
                    if let t = thread {
                        Section {
                            if t.pinned {
                                Button("Unpin", systemImage: "pin.slash") { model.threadAction(threadID, .unpin) }
                            } else {
                                Button("Pin", systemImage: "pin") { model.threadAction(threadID, .pin, done: "Pinned") }
                            }
                            if t.section == .settled {
                                Button("Back to the inbox", systemImage: "tray.and.arrow.down") { model.threadAction(threadID, .unsettle) }
                            } else {
                                Button("Settle", systemImage: "checkmark.circle") { model.threadAction(threadID, .settle, done: "Settled") }
                            }
                            Button("Rename…", systemImage: "pencil") {
                                newTitle = t.title
                                renaming = true
                            }
                        }
                    }
                    Button("Copy title", systemImage: "doc.on.doc") { UIPasteboard.general.string = thread?.title }
                    if working {
                        Button("Stop", systemImage: "stop.fill", role: .destructive) { model.interrupt(threadID) }
                    }
                    Button("Archive", systemImage: "archivebox", role: .destructive) { archiving = true }
                } label: {
                    Image(systemName: "ellipsis")
                }
            }
        }
        .alert("Rename thread", isPresented: $renaming) {
            TextField("Title", text: $newTitle)
            Button("Rename") {
                let title = newTitle.trimmingCharacters(in: .whitespacesAndNewlines)
                if !title.isEmpty { model.threadAction(threadID, .rename(title)) }
            }
            Button("Cancel", role: .cancel) {}
        }
        .confirmationDialog("Archive this thread?", isPresented: $archiving, titleVisibility: .visible) {
            Button("Archive", role: .destructive) { model.threadAction(threadID, .archive, done: "Archived") }
        } message: {
            Text("It leaves your threads on the Mac too.")
        }
        .sheet(isPresented: $showingGit) { GitSheet(threadID: threadID) }
        .sheet(item: $newThread) { request in
            NewThreadSheet(project: request.project, prompt: request.prompt) { model.openRequest = $0 }
        }
        .onChange(of: draft) {
            // `/` typed at the start: the picker wants the thread's commands (asked again each
            // time, as the agent's may have changed).
            if draft == "/" || (CommandPicker.query(draft) != nil && model.commands[threadID] == nil) {
                model.loadCommands(threadID)
            }
        }
        .onChange(of: model.newThreadPrompt) {
            // `/new` was sent: the Mac asks for the new-thread sheet, in this project.
            guard let project = model.newThreadPrompt else { return }
            model.newThreadPrompt = nil
            newThread = NewThreadRequest(project: project, prompt: newPrompt)
            newPrompt = ""
        }
        .onAppear {
            mode = model.followUpMode
            model.subscribe(threadID)
            switch Launch.sheet {
            case "git": showingGit = true
            case "commands": draft = "/"
            default: break
            }
        }
        .onDisappear { model.unsubscribe(threadID) }
    }

    private var subtitle: String {
        guard let t = thread else { return "" }
        return [t.project?.name, t.branch].compactMap { $0 }.joined(separator: " · ")
    }

    @ViewBuilder
    private func blockView(_ block: Block, isLatestGroup: Bool) -> some View {
        switch block {
        case .user(_, let text, let images):
            UserBubble(text: text, images: images)
        case .assistant(_, let text):
            MarkdownText(text: text)
                .textSelection(.enabled)
        case .group(let id, let items):
            ToolGroupView(items: items, expanded: Binding(
                get: { groupOpen[id] ?? (isLatestGroup || Launch.expandAll) },
                set: { groupOpen[id] = $0 }))
        case .resolved(let item):
            ResolvedRequestRow(item: item)
        case .turnEnd(_, let secs):
            HStack(spacing: 10) {
                Rectangle().fill(Trek.border).frame(height: 0.5)
                Text("Worked for \(When.duration(secs))").font(.caption).foregroundStyle(Trek.muted.opacity(0.8)).fixedSize()
                Rectangle().fill(Trek.border).frame(height: 0.5)
            }
        case .notice(_, let text):
            Label(text, systemImage: "info.circle").font(.subheadline).foregroundStyle(Trek.muted)
        case .error(_, let text):
            Label {
                Text(text)
            } icon: {
                Image(systemName: "exclamationmark.triangle.fill")
            }
            .font(.subheadline)
            .foregroundStyle(Trek.failed)
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Trek.failed.opacity(0.08), in: RoundedRectangle(cornerRadius: 14, style: .continuous))
        case .limit(_, let text, let resets):
            Label {
                VStack(alignment: .leading, spacing: 2) {
                    Text(text)
                    if let resets {
                        Text("Resets \(Date(timeIntervalSince1970: Double(resets) / 1000), style: .relative)").font(.caption)
                    }
                }
            } icon: {
                Image(systemName: "clock.fill")
            }
            .font(.subheadline)
            .foregroundStyle(Trek.approval)
        case .handoff(_, let from, let to):
            Label("Handed over from \(from) to \(to)", systemImage: "arrow.left.arrow.right")
                .font(.subheadline).foregroundStyle(Trek.muted)
        case .changes(let item, let changes):
            ChangesCard(changes: changes, threadID: threadID, rootName: thread?.project?.name ?? "Project",
                        folded: Binding(get: { folded[item.id] ?? [] }, set: { folded[item.id] = $0 }))
        }
    }

    private func requestId(_ item: TItem) -> String? {
        switch item.body {
        case .approval(let a): a.requestId
        case .question(let q): q.requestId
        case .plan(let p): p.requestId
        default: nil
        }
    }

    private func send() {
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty || !photos.isEmpty else { return }
        let uploads = photos.compactMap(\.upload)
        // `/new` and `/clear` open the new-thread sheet: what follows them is its prompt.
        newPrompt = ""
        for command in ["/new ", "/clear "] where text.hasPrefix(command) {
            newPrompt = String(text.dropFirst(command.count)).trimmingCharacters(in: .whitespaces)
        }
        model.send(text.isEmpty ? "Here's a photo." : text, to: threadID, mode: thread?.runState == .working ? mode : nil, images: uploads)
        draft = ""
        photos = []
    }
}

/// The Mac asked for the new-thread sheet from a thread (`/new`): in which project, with what.
struct NewThreadRequest: Identifiable {
    let id = UUID()
    var project: String
    var prompt: String
}
