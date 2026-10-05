import SwiftUI

/// A thread: its transcript, the requests waiting on the user, and the composer.
///
/// Built so each part redraws on its own: the transcript observes only the thread's store (not
/// the app's thread list), its rows are equatable values compared in constant time, and the
/// composer's text lives in its own observable object, so typing redraws only the composer.
struct ThreadView: View {
    var threadID: String
    @Environment(AppModel.self) private var model

    var body: some View {
        // One identity per thread: when a thread replaces another in the path (a fork, a
        // notification), the screen is made anew, subscribes, and starts with its own composer.
        ThreadScreen(store: model.store(threadID))
            .id(threadID)
    }
}

/// What's being written to the thread: read only by the composer area, so a keystroke redraws
/// nothing else.
@Observable
final class ComposerState {
    var draft = ""
    var photos: [PickedPhoto] = []
    var mode: SendMode = .steer
    /// Bumped to bring the keyboard up (after "Edit" put a message back in the composer).
    var focusRequest = 0
    /// What followed `/new` in the message just sent: the new thread's prompt, if the Mac asks
    /// for the sheet.
    @ObservationIgnored var newPrompt = ""
}

private struct ThreadScreen: View {
    let store: ThreadStore
    @Environment(AppModel.self) private var model
    @State private var composer = ComposerState()
    @State private var renaming = false
    @State private var newTitle = ""
    @State private var archiving = false
    @State private var showingGit = false
    @State private var newThread: NewThreadRequest?

    private var threadID: String { store.id }
    private var thread: ThreadSummary? { store.summary }

    var body: some View {
        let t = thread
        TranscriptList(store: store, composer: composer, working: t?.runState == .working,
                       projectName: t?.project?.name ?? "Project", retryModels: retryModels(t))
            .equatable()
            .environment(\.projectHue, t?.project?.hue)
            .background(Trek.background)
            .safeAreaBar(edge: .top, spacing: 0) {
                if let t {
                    ThreadStatusStrip(thread: t)
                        .padding(.horizontal, 14)
                        .padding(.vertical, 6)
                }
            }
            .safeAreaInset(edge: .bottom, spacing: 0) {
                ComposerArea(store: store, composer: composer, newPromptSent: { composer.newPrompt = $0 })
            }
            .navigationTitle(t?.title ?? "Thread")
            .navigationSubtitle(subtitle)
            .navigationBarTitleDisplayMode(.inline)
            .toolbarVisibility(.hidden, for: .tabBar)
            .toolbar { toolbar }
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
            .onChange(of: model.newThreadPrompt) {
                // `/new` was sent: the Mac asks for the new-thread sheet, in this project.
                guard let project = model.newThreadPrompt else { return }
                model.newThreadPrompt = nil
                newThread = NewThreadRequest(project: project, prompt: composer.newPrompt)
                composer.newPrompt = ""
            }
            .onAppear {
                composer.mode = model.followUpMode
                if let draft = model.takeDraft(threadID) { composer.draft = draft }
                model.subscribe(threadID)
                switch Launch.sheet {
                case "git": showingGit = true
                case "commands": composer.draft = "/"
                default: break
                }
            }
            .onDisappear { model.unsubscribe(threadID) }
    }

    /// The models a turn can be retried with: the thread's agent's others.
    private func retryModels(_ t: ThreadSummary?) -> [ModelOption] {
        guard let t else { return [] }
        let agent = model.agents.first { $0.key == t.agent.key }
        let current = ModelNaming.current(t.model, agent: agent)?.id
        return (agent?.models ?? []).filter { m in current.map { !ModelNaming.same($0, m.id) } ?? true }
    }

    private var subtitle: String {
        guard let t = thread else { return "" }
        return [t.project?.name, t.branch].compactMap { $0 }.joined(separator: " · ")
    }

    @ToolbarContentBuilder
    private var toolbar: some ToolbarContent {
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
                if thread?.runState == .working {
                    Button("Stop", systemImage: "stop.fill", role: .destructive) { model.interrupt(threadID) }
                }
                Button("Archive", systemImage: "archivebox", role: .destructive) { archiving = true }
            } label: {
                Image(systemName: "ellipsis")
            }
        }
    }
}

// MARK: Transcript

/// An action under a message or a turn.
enum RowAction {
    case copy(String)
    case copyResponse(end: String)
    case undo(end: String)
    case retry(end: String, model: ModelOption?)
    case fork(item: String)
    case rewind(user: String, edit: Bool)
}

/// An action that changes the conversation and the files, waiting for the user to confirm it.
private struct Confirmation: Identifiable {
    var action: TurnAction
    var item: String
    var model: ModelOption?
    /// Edit: rewind, then bring up the keyboard.
    var edit = false
    var id: String { "\(action)-\(item)-\(model?.id ?? "")" }

    var title: String {
        switch action {
        case .undo: "Undo this turn?"
        case .retry: model.map { "Retry with \($0.label)?" } ?? "Retry this turn?"
        case .rewind: edit ? "Edit this message?" : "Rewind to this message?"
        case .fork: "Fork from here?"
        }
    }

    var message: String {
        switch action {
        case .undo: "Your message and everything after it go; the message waits in the composer."
        case .retry: "The answer goes, and your message is sent again."
        case .rewind: edit ? "Back to before this message, to change it and send it again." : "Back to before this message; it waits in the composer."
        case .fork: ""
        }
    }

    var verb: String {
        switch action {
        case .undo: "Undo"
        case .retry: "Retry"
        case .rewind: edit ? "Edit" : "Rewind"
        case .fork: "Fork"
        }
    }
}

private struct TranscriptList: View, Equatable {
    let store: ThreadStore
    let composer: ComposerState
    var working: Bool
    var projectName: String
    var retryModels: [ModelOption]

    @Environment(AppModel.self) private var model
    @State private var groupOpen: [String: Bool] = [:]
    /// Each changes card's folded folders, by item.
    @State private var folded: [String: Set<String>] = [:]
    @State private var confirming: Confirmation?
    /// The user has scrolled since the thread opened (before that, the view is settling at the bottom).
    @State private var scrolled = false
    @State private var nearTop = false

    private struct TopEdge: Equatable {
        /// Within a couple of screens of the top.
        var near: Bool
        /// What's loaded doesn't fill the screen.
        var short: Bool
    }

    nonisolated static func == (a: TranscriptList, b: TranscriptList) -> Bool {
        MainActor.assumeIsolated {
            a.store === b.store && a.working == b.working && a.projectName == b.projectName && a.retryModels == b.retryModels
        }
    }

    var body: some View {
        let blocks = store.blocks
        // The latest group, turn and message: open, and with their actions out.
        let lastGroup = blocks.last(where: \.isGroup)?.id
        let ends = Set(Block.turnEnds(blocks, open: working))
        let lastTurn = blocks.last { ends.contains($0.id) }?.id
        let lastUser = blocks.last(where: \.isUser)?.id
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 18) {
                if !store.loaded {
                    ProgressView().frame(maxWidth: .infinity).padding(.top, 80)
                } else if store.more {
                    EarlierMarker(loading: store.loadingEarlier)
                }
                ForEach(blocks) { block in
                    let open = groupOpen[block.id] ?? ((block.id == lastGroup && working) || Launch.expandAll)
                    BlockRow(block: block, threadID: store.id, projectName: projectName,
                             expanded: open,
                             folded: folded[block.id] ?? [],
                             endsTurn: ends.contains(block.id),
                             latest: block.id == lastTurn || block.id == lastUser,
                             busy: working, retryModels: retryModels,
                             toggle: { groupOpen[block.id] = !open },
                             fold: { folded[block.id] = $0 },
                             act: perform)
                        .equatable()
                        .id(block.id)
                }
                WorkingFooter(store: store)
                Color.clear.frame(height: 4).id("end")
            }
            .padding(.horizontal, 20)
            .padding(.top, 12)
            .padding(.bottom, 16)
        }
        // Bottom-anchored: it opens at the latest, stays with it as items arrive, and keeps still
        // when a page of earlier items lands above.
        .defaultScrollAnchor(.bottom)
        .scrollDismissesKeyboard(.interactively)
        // Earlier items load as the user scrolls up towards them (or straight away, while what's
        // loaded doesn't fill the screen); not while the view settles at the bottom.
        .onScrollPhaseChange { _, phase in
            guard phase == .interacting, !scrolled else { return }
            scrolled = true
            if nearTop { model.loadEarlier(store.id) }
        }
        .onScrollGeometryChange(for: TopEdge.self) { geo in
            TopEdge(near: geo.contentOffset.y + geo.contentInsets.top < 1400,
                    short: geo.contentSize.height < geo.containerSize.height)
        } action: { _, edge in
            nearTop = edge.near
            if edge.short || (edge.near && scrolled) { model.loadEarlier(store.id) }
        }
        .confirmationDialog(confirming?.title ?? "", isPresented: Binding(get: { confirming != nil }, set: { if !$0 { confirming = nil } }),
                            titleVisibility: .visible, presenting: confirming) { c in
            Button("\(c.verb) and restore files", role: .destructive) { run(c, restoreFiles: true) }
            Button("\(c.verb), keep files") { run(c, restoreFiles: false) }
            Button("Cancel", role: .cancel) {}
        } message: { c in
            Text(c.message)
        }
    }

    private func perform(_ action: RowAction) {
        switch action {
        case .copy(let text):
            UIPasteboard.general.string = text
            model.show("Copied")
        case .copyResponse(let end):
            let text = store.responseText(endingAt: end)
            guard !text.isEmpty else { return model.show("Nothing to copy in this turn") }
            UIPasteboard.general.string = text
            model.show("Copied the response")
        case .undo(let end):
            confirming = Confirmation(action: .undo, item: end)
        case .retry(let end, let m):
            confirming = Confirmation(action: .retry, item: end, model: m)
        case .rewind(let user, let edit):
            confirming = Confirmation(action: .rewind, item: user, edit: edit)
        case .fork(let item):
            model.turnAction(store.id, item: item, .fork)
        }
    }

    private func run(_ c: Confirmation, restoreFiles: Bool) {
        let composer = composer
        model.turnAction(store.id, item: c.item, c.action, model: c.model?.id, restoreFiles: restoreFiles) { text in
            composer.draft = text
            if c.edit { composer.focusRequest += 1 }
        }
    }
}

/// At the top while earlier items exist: a spinner while a page is on its way.
private struct EarlierMarker: View {
    var loading: Bool

    var body: some View {
        HStack {
            Spacer()
            if loading {
                ProgressView().controlSize(.small)
            } else {
                Text("Earlier messages").font(.caption).foregroundStyle(Trek.muted.opacity(0.7))
            }
            Spacer()
        }
        .frame(height: 28)
    }
}

/// Under the transcript while the agent works. Observes the thread's row itself, so its ticking
/// activity doesn't redraw the transcript.
private struct WorkingFooter: View {
    let store: ThreadStore

    var body: some View {
        if let t = store.summary, t.runState == .working {
            WorkingLine(threadID: t.id, agentKey: t.agent.key, since: t.workingSince, activity: t.activity)
                .padding(.top, 2)
                .transaction { $0.animation = nil }
        }
    }
}

/// One block of the transcript. Equatable on its values (the block compares by id and seqs), so
/// the list redraws only the rows that changed.
private struct BlockRow: View, Equatable {
    var block: Block
    var threadID: String
    var projectName: String
    var expanded: Bool
    var folded: Set<String>
    /// An error, limit or notice a turn stopped with: the turn's actions go under it.
    var endsTurn: Bool
    /// The latest turn or message: its actions are out.
    var latest: Bool
    /// A turn is running: the actions that need the Mac idle are off.
    var busy: Bool
    var retryModels: [ModelOption]
    var toggle: () -> Void
    var fold: (Set<String>) -> Void
    var act: (RowAction) -> Void

    nonisolated static func == (a: BlockRow, b: BlockRow) -> Bool {
        MainActor.assumeIsolated {
            a.block == b.block && a.expanded == b.expanded && a.folded == b.folded && a.endsTurn == b.endsTurn && a.latest == b.latest
                && a.busy == b.busy && a.projectName == b.projectName && a.retryModels == b.retryModels
        }
    }

    var body: some View {
        if endsTurn, !block.isTurnEnd {
            VStack(alignment: .leading, spacing: 6) {
                content
                TurnActions(end: block.itemID, secs: nil, at: nil, latest: latest, busy: busy, models: retryModels, act: act)
            }
        } else {
            content
        }
    }

    @ViewBuilder
    private var content: some View {
        switch block.kind {
        case .user(let text, let images, let at):
            VStack(alignment: .trailing, spacing: 4) {
                UserBubble(text: text, images: images)
                MessageActions(item: block.itemID, text: text, at: at, latest: latest, busy: busy, act: act)
            }
        case .assistant(let text):
            MarkdownText(text: text)
                .equatable()
                .textSelection(.enabled)
        case .group(let items):
            ToolGroupView(items: items, expanded: expanded, toggle: toggle)
        case .resolved(let item):
            ResolvedRequestRow(item: item)
        case .turnEnd(let secs, let at):
            TurnActions(end: block.itemID, secs: secs, at: at, latest: latest, busy: busy, models: retryModels, act: act)
        case .notice(let text):
            Label(text, systemImage: "info.circle").font(.subheadline).foregroundStyle(Trek.muted)
        case .error(let text):
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
        case .limit(let text, let resets):
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
        case .handoff(let from, let to):
            Label("Handed over from \(from) to \(to)", systemImage: "arrow.left.arrow.right")
                .font(.subheadline).foregroundStyle(Trek.muted)
        case .changes(let changes):
            ChangesCard(changes: changes, threadID: threadID, rootName: projectName,
                        folded: Binding(get: { folded }, set: { fold($0) }))
        }
    }
}

// MARK: Composer and pending requests

/// Above the keyboard: requests waiting on the user, the `/` command picker or the thread's
/// settings, and the composer. The only view that reads the draft.
private struct ComposerArea: View {
    let store: ThreadStore
    let composer: ComposerState
    var newPromptSent: (String) -> Void
    @Environment(AppModel.self) private var model

    private var threadID: String { store.id }

    var body: some View {
        @Bindable var c = composer
        let thread = store.summary
        let working = thread?.runState == .working
        let query = CommandPicker.query(composer.draft)
        let commands = query.map { CommandPicker.matches(model.commands[threadID] ?? [], $0) } ?? []
        VStack(spacing: 10) {
            ForEach(store.pending) { item in
                PendingRequestCard(item: item, agentName: thread?.agent.name ?? "The agent") { response in
                    if let rid = Self.requestId(item) { model.answer(threadID, requestId: rid, response) }
                }
                .transition(.move(edge: .bottom).combined(with: .opacity))
            }
            if !commands.isEmpty {
                CommandPicker(commands: model.commands[threadID] ?? [], query: query ?? "") { cmd in
                    composer.draft = "/\(cmd.name) "
                }
                .transition(.move(edge: .bottom).combined(with: .opacity))
            } else if let thread {
                ThreadSettingsBar(thread: thread)
            }
            Composer(text: $c.draft.timed("keystroke"), mode: $c.mode, photos: $c.photos,
                     placeholder: "Message \(thread?.agent.name ?? "the agent")",
                     working: working, focusRequest: composer.focusRequest, send: send, stop: { model.interrupt(threadID) })
        }
        .padding(.horizontal, 14)
        .padding(.bottom, 6)
        .padding(.top, 8)
        .animation(.smooth(duration: 0.3), value: store.pending.map(\.id))
        .animation(.snappy(duration: 0.2), value: commands.isEmpty)
        .onChange(of: composer.draft) {
            // `/` typed at the start: the picker wants the thread's commands (asked again each
            // time, as the agent's may have changed).
            let draft = composer.draft
            if draft == "/" || (CommandPicker.query(draft) != nil && model.commands[threadID] == nil) {
                model.loadCommands(threadID)
            }
        }
    }

    private static func requestId(_ item: TItem) -> String? {
        switch item.body {
        case .approval(let a): a.requestId
        case .question(let q): q.requestId
        case .plan(let p): p.requestId
        default: nil
        }
    }

    private func send() {
        let text = composer.draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty || !composer.photos.isEmpty else { return }
        // Nothing can go while the Mac is out of reach: what's typed stays in the composer.
        guard model.connection == .connected else {
            model.show(AppModel.notConnected, error: true)
            return
        }
        let uploads = composer.photos.compactMap(\.upload)
        // `/new` and `/clear` open the new-thread sheet: what follows them is its prompt.
        var prompt = ""
        for command in ["/new ", "/clear "] where text.hasPrefix(command) {
            prompt = String(text.dropFirst(command.count)).trimmingCharacters(in: .whitespaces)
        }
        newPromptSent(prompt)
        model.send(text.isEmpty ? "Here's a photo." : text, to: threadID,
                   mode: store.summary?.runState == .working ? composer.mode : nil, images: uploads)
        composer.draft = ""
        composer.photos = []
    }
}

/// The Mac asked for the new-thread sheet from a thread (`/new`): in which project, with what.
struct NewThreadRequest: Identifiable {
    let id = UUID()
    var project: String
    var prompt: String
}
