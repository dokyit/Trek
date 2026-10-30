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
    /// What was just sent, and when. The keyboard can write it back into the field after it was
    /// cleared: dictation finishing, or a correction or prediction it was holding being committed
    /// as the field lets go of it. Then the field keeps showing the message that went.
    @ObservationIgnored private var sent: (text: String, at: Date)?

    /// How long after a send the keyboard's write-back is told from typing.
    static let echoWindow: TimeInterval = 1.5

    /// A message went: the composer starts empty.
    func cleared(after text: String, at now: Date = .now) {
        sent = (text, now)
        draft = ""
        photos = []
    }

    /// A message put back on purpose (undo, rewind, Edit): not an echo, however soon.
    func put(_ text: String) {
        sent = nil
        draft = text
    }

    /// The draft changed: if that's the keyboard putting back what was just sent, it's emptied
    /// again (and true comes back). Anything else typed ends the watch.
    @discardableResult
    func dropEcho(at now: Date = .now) -> Bool {
        guard let s = sent, !draft.isEmpty else { return false }
        guard now.timeIntervalSince(s.at) < Self.echoWindow, Self.isEcho(draft, of: s.text) else {
            sent = nil
            return false
        }
        draft = ""
        return true
    }

    /// `text` is `sent` written back whole, maybe with its last word corrected: no keystroke
    /// makes most of a message at once.
    nonisolated static func isEcho(_ text: String, of sent: String) -> Bool {
        let a = text.trimmingCharacters(in: .whitespacesAndNewlines)
        let b = sent.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !a.isEmpty, !b.isEmpty else { return false }
        if a == b { return true }
        let shared = a.commonPrefix(with: b).count
        return shared >= 4 && shared * 10 >= b.count * 7 && a.count * 10 >= b.count * 7
    }
}

private struct ThreadScreen: View {
    let store: ThreadStore
    @Environment(AppModel.self) private var model
    @State private var composer = ComposerState()
    @State private var renaming = false
    @State private var newTitle = ""
    @State private var archiving = false
    @State private var showingGit = false
    @State private var showingUsage = false
    @State private var newThread: NewThreadRequest?
    @State private var snoozePicking = false
    @State private var deleteAsk: DeleteAsk?
    @State private var room: CGFloat = 0
    @State private var stripHeight: CGFloat = 0
    @Environment(\.dismiss) private var dismiss

    private var threadID: String { store.id }
    private var thread: ThreadSummary? { store.summary }

    var body: some View {
        let t = thread
        TranscriptList(store: store, composer: composer, working: t?.runState == .working,
                       projectName: t?.project?.name ?? "Project", retryModels: retryModels(t))
            .equatable()
            .environment(\.projectHue, t?.project?.hue)
            .environment(\.liveThreadID, threadID)
            .background(Trek.background)
            .safeAreaBar(edge: .top, spacing: 0) {
                if let t {
                    ThreadStatusStrip(thread: t)
                        .padding(.horizontal, 14)
                        .padding(.vertical, 6)
                        .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { stripHeight = $0 }
                }
            }
            .safeAreaInset(edge: .bottom, spacing: 0) {
                ComposerArea(store: store, composer: composer, room: room - (t == nil ? 0 : stripHeight),
                             newPromptSent: { composer.newPrompt = $0 })
            }
            // Between the title bar and the keyboard (or the screen's foot): what the requests
            // over the composer may take, the strip under the title kept clear.
            .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { room = $0 }
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
            .sheet(isPresented: $showingUsage) { UsageSheet(host: model.link(for: threadID)) }
            .sheet(isPresented: $snoozePicking) { if let t = thread { SnoozePicker(thread: t) } }
            .deleteConfirmation($deleteAsk, model: model) { _ in dismiss() }
            .sheet(item: $newThread) { request in
                NewThreadSheet(project: request.project, prompt: request.prompt, on: request.link) { model.openRequest = $0 }
            }
            .onChange(of: model.newThreadPrompt) {
                // `/new` was sent: the Mac asks for the new-thread sheet, in this project.
                guard let prompt = model.newThreadPrompt else { return }
                model.newThreadPrompt = nil
                newThread = NewThreadRequest(link: prompt.link, project: prompt.projectId, prompt: composer.newPrompt)
                composer.newPrompt = ""
            }
            .onAppear {
                composer.mode = model.followUpMode
                if let draft = model.takeDraft(threadID) { composer.draft = draft }
                model.subscribe(threadID)
                let link = model.link(for: threadID)
                if link?.usage == nil { model.loadUsage(quiet: true, on: link) }
                switch Launch.sheet {
                case "git": showingGit = true
                case "commands": composer.draft = "/"
                case "snooze": snoozePicking = true
                case "delete":
                    // Once the row is in.
                    DispatchQueue.main.asyncAfter(deadline: .now() + 1) { if let t = thread { model.askToDelete(t, into: $deleteAsk) } }
                default: break
                }
            }
            .onDisappear { model.unsubscribe(threadID) }
    }

    /// The models a turn can be retried with: the thread's agent's others.
    private func retryModels(_ t: ThreadSummary?) -> [ModelOption] {
        guard let t else { return [] }
        let agent = model.link(for: threadID)?.agents.first { $0.key == t.agent.key }
        let current = ModelNaming.current(t.model, agent: agent)?.id
        return (agent?.models ?? []).filter { m in current.map { !ModelNaming.same($0, m.id) } ?? true }
    }

    private var subtitle: String {
        guard let t = thread else { return "" }
        if t.parentId != nil { return ["Sub-agent", t.modelLabel].compactMap { $0 }.joined(separator: " · ") }
        return [t.project?.name, t.branch].compactMap { $0 }.joined(separator: " · ")
    }

    /// The Git button, for a thread whose folder git serves. A thread in no project works in a
    /// chat folder of its own: the Mac sends it no branch or git state, and an older Mac that
    /// did refuses its git requests (the sheet says so, then the button goes).
    private func offersGit(_ t: ThreadSummary) -> Bool {
        (t.git != nil || t.branch != nil) && model.gitUnavailable[.thread(threadID)]?.refused != true
    }

    @ToolbarContentBuilder
    private var toolbar: some ToolbarContent {
        if let t = thread, offersGit(t) {
            ToolbarItem(placement: .topBarTrailing) {
                Button { showingGit = true } label: { GitButtonLabel(changed: t.git?.changed ?? 0) }
            }
            ToolbarSpacer(.fixed, placement: .topBarTrailing)
        }
        ToolbarItem(placement: .topBarTrailing) {
            ThreadMenu(shown: thread.map { ThreadMenu.Shown($0, model: model) }, thread: thread,
                       usage: { showingUsage = true },
                       pickSnooze: { snoozePicking = true },
                       rename: {
                           newTitle = thread?.title ?? ""
                           renaming = true
                       },
                       archive: { archiving = true },
                       delete: { if let t = thread { model.askToDelete(t, into: $deleteAsk) } })
                .equatable()
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
    @Environment(\.projectHue) private var projectHue
    @Environment(\.liveThreadID) private var liveThreadID
    @Environment(\.trekCalm) private var calm
    @Environment(\.compactRows) private var compact
    @Environment(\.openThread) private var openThread
    @Environment(\.dynamicTypeSize) private var dynamicTypeSize
    @State private var groupOpen: [String: Bool] = [:]
    /// Each changes card's folded folders, by item.
    @State private var folded: [String: Set<String>] = [:]
    @State private var confirming: Confirmation?
    /// Far enough above the latest that the jump-down button is out.
    @State private var awayFromBottom = false
    @State private var scroller = TranscriptScroller()

    nonisolated static func == (a: TranscriptList, b: TranscriptList) -> Bool {
        MainActor.assumeIsolated {
            a.store === b.store && a.working == b.working && a.projectName == b.projectName && a.retryModels == b.retryModels
        }
    }

    var body: some View {
        // The list itself is UIKit (`TranscriptCollection`): it keeps the reader's place as rows
        // are measured, earlier pages land and answers grow. SwiftUI says what the rows are.
        GeometryReader { geo in
            TranscriptCollection(rows: rows, generation: store.generation,
                                 environment: TranscriptEnvironment(model: model, projectHue: projectHue, liveThreadID: liveThreadID,
                                                                    calm: calm, compact: compact, openThread: openThread,
                                                                    dynamicTypeSize: dynamicTypeSize),
                                 scroller: scroller,
                                 loadEarlier: { [model, store] in model.loadEarlier(store.id) },
                                 edges: { e in if e.awayFromBottom != awayFromBottom { awayFromBottom = e.awayFromBottom } },
                                 insets: geo.safeAreaInsets)
            // Under the bars, the composer and the keyboard, as a scroll view is: its insets keep
            // the rows clear of them.
            .ignoresSafeArea(.all, edges: .vertical)
        }
        // Jump back to the latest when the transcript has been left behind.
        .overlay(alignment: .bottomTrailing) {
            Group {
                if awayFromBottom {
                    Button {
                        scroller.scrollToLatest()
                    } label: {
                        Image(systemName: "arrow.down")
                            .font(.system(size: 15, weight: .semibold))
                            .foregroundStyle(Trek.foreground.opacity(0.85))
                            .frame(width: 38, height: 38)
                            .background(Trek.surface, in: Circle())
                            .overlay(Circle().strokeBorder(Trek.border, lineWidth: 0.75))
                            .shadow(color: .black.opacity(0.18), radius: 8, y: 2)
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Jump to the latest")
                    .padding(.trailing, 14)
                    .padding(.bottom, 12)
                    .transition(.scale(scale: 0.6).combined(with: .opacity))
                }
            }
            .animation(.snappy(duration: 0.2), value: awayFromBottom)
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

    /// The list's rows: what's above the transcript, its blocks, and what's under it.
    private var rows: [TranscriptRow] {
        let blocks = store.blocks
        // The latest group, turn and message: open, and with their actions out.
        let lastGroup = blocks.last(where: \.isGroup)?.id
        let ends = Set(Block.turnEnds(blocks, open: working))
        let lastTurn = blocks.last { ends.contains($0.id) }?.id
        let lastUser = blocks.last(where: \.isUser)?.id
        var rows: [TranscriptRow] = []
        rows.reserveCapacity(blocks.count + 3)
        rows.append(.header(store))
        if !store.loaded {
            rows.append(.loading)
        } else if store.more {
            rows.append(.earlier(loading: store.loadingEarlier))
        }
        for block in blocks {
            let open = groupOpen[block.id] ?? ((block.id == lastGroup && working) || Launch.expandAll)
            let row = BlockRow(block: block, threadID: store.id, projectName: projectName,
                               expanded: open,
                               folded: folded[block.id] ?? [],
                               endsTurn: ends.contains(block.id),
                               latest: block.id == lastTurn || block.id == lastUser,
                               busy: working, retryModels: retryModels,
                               toggle: { groupOpen[block.id] = !open },
                               fold: { folded[block.id] = $0 },
                               act: perform)
            // A long answer goes in as parts, a few paragraphs each: laid out a part at a time as
            // the reader comes to it, not several screens at once. Not while it's being written.
            if case .assistant(let text, false) = block.kind, let parts = AnswerPart.split(text, key: block.id, version: block.version) {
                for part in parts {
                    var r = row
                    r.part = part
                    r.endsTurn = row.endsTurn && part.index == parts.count - 1
                    rows.append(.block(r))
                }
            } else {
                rows.append(.block(row))
            }
        }
        rows.append(.footer(store))
        return rows
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
            composer.put(text)
            if c.edit { composer.focusRequest += 1 }
        }
    }
}

/// One row of the transcript's list: what's above the transcript (where this thread came from),
/// the spinner or marker for earlier items, a block, and what's under it (who's working).
enum TranscriptRow: View, Equatable, Identifiable {
    case header(ThreadStore)
    /// The transcript isn't in yet.
    case loading
    case earlier(loading: Bool)
    case block(BlockRow)
    case footer(ThreadStore)

    var id: String {
        switch self {
        case .header: "~header"
        case .loading: "~loading"
        case .earlier: "~earlier"
        case .block(let r): r.part.map { "\(r.block.id)#\($0.index)" } ?? r.block.id
        case .footer: "~footer"
        }
    }

    /// Above the transcript: goes when earlier items have all come in.
    var isMarker: Bool {
        switch self {
        case .loading, .earlier: true
        default: false
        }
    }

    /// The space above the row: a long answer's parts sit as close as its paragraphs do.
    var gapBefore: CGFloat {
        if case .block(let r) = self, let part = r.part, part.index > 0 { return 10 }
        return 18
    }

    /// Not a block: what's around the transcript.
    var isChrome: Bool {
        switch self {
        case .header, .footer: true
        default: false
        }
    }

    nonisolated static func == (a: TranscriptRow, b: TranscriptRow) -> Bool {
        MainActor.assumeIsolated {
            switch (a, b) {
            case (.header(let x), .header(let y)), (.footer(let x), .footer(let y)): x === y
            case (.loading, .loading): true
            case (.earlier(let x), .earlier(let y)): x == y
            case (.block(let x), .block(let y)): x == y
            default: false
            }
        }
    }

    var body: some View {
        switch self {
        case .header(let store): LineageHeader(store: store)
        case .loading: ProgressView().frame(maxWidth: .infinity).padding(.top, 80)
        case .earlier(let loading): EarlierMarker(loading: loading)
        case .block(let row): row.equatable()
        case .footer(let store): WorkingFooter(store: store)
        }
    }

    /// A guess at the row's height before it's drawn: close enough that the scroll bar doesn't
    /// lurch as rows are measured (the reader's place doesn't depend on it).
    func estimatedHeight(width: CGFloat) -> CGFloat {
        let inner = max(120, width - 40)
        // About 8.4 pt a character at the body size.
        let perLine = max(16, Int(inner / 8.4))
        func lines(_ text: some StringProtocol, per: Int) -> CGFloat { CGFloat(max(1, (text.count + per - 1) / per)) }
        switch self {
        case .header, .footer: return 0
        case .loading: return 100
        case .earlier: return 28
        case .block(let row):
            var h: CGFloat
            switch row.block.kind {
            case .user(let text, let images, _, _):
                h = 22 + 22 * min(lines(text.prefix(4000), per: perLine - 8), 40) + 26 + (images > 0 ? 90 : 0)
            case .assistant(let text, _):
                h = Self.markdownHeight(row.part?.text ?? text, perLine: perLine)
            case .group(let items):
                h = row.expanded ? 24 + 30 * CGFloat(min(items.count, ToolGroupView.shownAtFirst)) : 22
            case .resolved: h = 90
            case .turnEnd: h = 24
            case .notice, .handoff: h = 22
            case .error(let text), .limit(let text, _): h = 24 + 20 * lines(text, per: perLine)
            case .threadLink: h = 44
            case .changes(let c): h = 54 + 30 * CGFloat(min(c.files.count, 8))
            }
            if row.endsTurn, !row.block.isTurnEnd { h += 30 }
            return h
        }
    }

    /// Markdown: text lines wrap, code and table lines don't, gaps between blocks.
    private static func markdownHeight(_ text: String, perLine: Int) -> CGFloat {
        var h: CGFloat = 0
        var code = false
        var scanned = 0
        for line in text.split(separator: "\n", omittingEmptySubsequences: false) {
            scanned += line.count
            if scanned > 60_000 { break }
            if line.hasPrefix("```") { code.toggle(); h += 14; continue }
            if code { h += 18; continue }
            if line.isEmpty { h += 10; continue }
            if line.hasPrefix("|") { h += line.contains("---") ? 0 : 32; continue }
            if line.hasPrefix("#") { h += 34; continue }
            h += 24 * CGFloat(max(1, (line.count + perLine - 1) / perLine))
        }
        return max(24, h)
    }
}

/// A stretch of a long answer: whole Markdown blocks (never inside a code fence, a list, a table
/// or display maths), about `target` characters.
struct AnswerPart: Equatable {
    var index: Int
    var text: String

    static func == (a: AnswerPart, b: AnswerPart) -> Bool { a.index == b.index && a.text.utf8.count == b.text.utf8.count }

    /// Answers this long or longer are split.
    static let threshold = 2400
    static let target = 1200

    nonisolated(unsafe) private static var cache: [String: [AnswerPart]] = [:]

    /// The parts of `text` (the answer `key` at `version`), or nil when it's short enough whole.
    static func split(_ text: String, key: String, version: Int64) -> [AnswerPart]? {
        guard text.utf8.count >= threshold else { return nil }
        let k = "\(key)@\(version)"
        if let hit = cache[k] { return hit.count > 1 ? hit : nil }
        let parts = cut(text).enumerated().map { AnswerPart(index: $0.offset, text: $0.element) }
        if cache.count > 4000 { cache.removeAll() }
        cache[k] = parts
        return parts.count > 1 ? parts : nil
    }

    /// Cuts at blank lines outside fences and display maths, where the next line doesn't carry
    /// on a list, a quote, a table or indented text.
    nonisolated static func cut(_ text: String, target: Int = 1200) -> [String] {
        let lines = text.split(separator: "\n", omittingEmptySubsequences: false)
        var parts: [String] = []
        var current: [Substring] = []
        var size = 0
        var fence = false, math = false
        func continues(_ line: Substring) -> Bool {
            let t = line.drop { $0 == " " }
            if line.first == " " || line.first == "\t" || t.hasPrefix(">") || t.hasPrefix("|") { return true }
            if t.hasPrefix("- ") || t.hasPrefix("* ") || t.hasPrefix("+ ") { return true }
            let digits = t.prefix { $0.isNumber }
            return !digits.isEmpty && (t.dropFirst(digits.count).hasPrefix(". ") || t.dropFirst(digits.count).hasPrefix(") "))
        }
        for (i, line) in lines.enumerated() {
            let t = line.trimmingCharacters(in: .whitespaces)
            if t.hasPrefix("```") || t.hasPrefix("~~~") { fence.toggle() }
            if !fence, t == "$$" || (t.hasPrefix("$$") && !(t.count > 2 && t.hasSuffix("$$"))) { math.toggle() }
            if !fence, !math, t.isEmpty, size >= target, i + 1 < lines.count, !continues(lines[i + 1]),
               lines[(i + 1)...].contains(where: { !$0.trimmingCharacters(in: .whitespaces).isEmpty }) {
                parts.append(current.joined(separator: "\n"))
                current = []
                size = 0
                continue
            }
            current.append(line)
            size += line.utf8.count + 1
        }
        if !current.isEmpty { parts.append(current.joined(separator: "\n")) }
        return parts
    }
}

/// At the top while earlier items exist: a spinner while a page is on its way.
struct EarlierMarker: View {
    var loading: Bool

    var body: some View {
        HStack {
            Spacer()
            if loading {
                ProgressView().controlSize(.small)
            } else {
                Text("Earlier messages").font(.ui(.caption)).foregroundStyle(Trek.muted.opacity(0.7))
            }
            Spacer()
        }
        .frame(height: 28)
    }
}

/// Under the transcript while the agent works, or its sub-agents do. Observes the thread's row
/// itself, so its ticking activity doesn't redraw the transcript.
struct WorkingFooter: View {
    let store: ThreadStore

    var body: some View {
        if let t = store.summary {
            let agents = t.subAgents ?? []
            let out = !t.runningSubAgents.isEmpty
            VStack(alignment: .leading, spacing: 12) {
                // While the turn runs, or while what it started is still at it.
                if !agents.isEmpty, t.runState == .working || out {
                    SubAgentsCard(agents: agents)
                        .transition(.opacity.combined(with: .move(edge: .bottom)))
                }
                if t.runState == .working {
                    WorkingLine(threadID: t.id, agentKey: t.agent.logo ?? t.agent.key, since: t.workingSince, activity: t.activity,
                                model: t.modelLabel, subAgents: (t.runningSubAgents.count, agents.count))
                        .padding(.top, 2)
                        .transaction { $0.animation = nil }
                }
            }
            .animation(.snappy(duration: 0.25), value: agents.isEmpty)
        }
    }
}

/// One block of the transcript. Equatable on its values (the block compares by id and seqs), so
/// the list redraws only the rows that changed.
struct BlockRow: View, Equatable {
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
    /// One stretch of a long answer (it's drawn, and measured, a part at a time).
    var part: AnswerPart? = nil

    nonisolated static func == (a: BlockRow, b: BlockRow) -> Bool {
        MainActor.assumeIsolated {
            a.block == b.block && a.part == b.part && a.expanded == b.expanded && a.folded == b.folded && a.endsTurn == b.endsTurn
                && a.latest == b.latest
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
        case .user(let text, let images, let at, let aside):
            VStack(alignment: .trailing, spacing: 4) {
                UserBubble(text: text, images: images, threadID: threadID, itemID: block.itemID)
                MessageActions(item: block.itemID, text: text, at: at, latest: latest, busy: busy, aside: aside, act: act)
            }
        case .assistant(let text, let streaming):
            MarkdownText(text: part?.text ?? text, live: streaming)
                .equatable()
                .textSelection(.enabled)
        case .group(let items):
            ToolGroupView(items: items, expanded: expanded, toggle: toggle)
        case .resolved(let item):
            ResolvedRequestRow(item: item)
        case .turnEnd(let secs, let at):
            TurnActions(end: block.itemID, secs: secs, at: at, latest: latest, busy: busy, models: retryModels, act: act)
        case .notice(let text):
            Label(text, systemImage: "info.circle").font(.ui(.subheadline)).foregroundStyle(Trek.muted)
        case .error(let text):
            Label {
                Text(text)
            } icon: {
                Image(systemName: "exclamationmark.triangle.fill")
            }
            .font(.ui(.subheadline))
            .foregroundStyle(Trek.failed)
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Trek.failed.opacity(0.08), in: RoundedRectangle(cornerRadius: 14, style: .continuous))
        case .limit(let text, let resets):
            Label {
                VStack(alignment: .leading, spacing: 2) {
                    Text(text)
                    if let resets {
                        Text("Resets \(Date(timeIntervalSince1970: Double(resets) / 1000), style: .relative)").font(.ui(.caption))
                    }
                }
            } icon: {
                Image(systemName: "clock.fill")
            }
            .font(.ui(.subheadline))
            .foregroundStyle(Trek.approval)
        case .handoff(let from, let to):
            Label("Handed over from \(from) to \(to)", systemImage: "arrow.left.arrow.right")
                .font(.ui(.subheadline)).foregroundStyle(Trek.muted)
        case .threadLink(let link):
            ThreadLinkRow(link: link)
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
    /// The height between the strip under the title and the keyboard (or the screen's foot).
    var room: CGFloat
    var newPromptSent: (String) -> Void
    @Environment(AppModel.self) private var model
    /// The requests' own height, and that of what's under them (queued, settings, composer).
    @State private var requestsHeight: CGFloat = 0
    @State private var restHeight: CGFloat = 0
    @State private var scroller = RequestScroller()

    private var threadID: String { store.id }

    /// What the requests may take: the room left over the composer, with a strip of the
    /// transcript still showing above them.
    private var requestRoom: CGFloat { max(140, room - restHeight - 64) }

    var body: some View {
        @Bindable var c = composer
        let thread = store.summary
        let working = thread?.runState == .working
        let query = CommandPicker.query(composer.draft)
        let commands = query.map { CommandPicker.matches(model.commands[threadID] ?? [], $0) } ?? []
        let needsKid = thread?.needsThread.flatMap { model.serves(.subAgents, on: threadID) ? $0 : nil }
        VStack(spacing: 10) {
            if needsKid != nil || !store.pending.isEmpty {
                // Requests can be taller than the screen (many questions, each with its options):
                // they scroll on their own, under the strip and over the composer, and the field
                // being typed in comes into view over the keyboard.
                ScrollViewReader { proxy in
                    ScrollView {
                        VStack(spacing: 10) {
                            if let kid = needsKid {
                                SubAgentNeedsCard(threadID: kid, needs: thread?.needs)
                                    .transition(.move(edge: .bottom).combined(with: .opacity))
                            }
                            ForEach(store.pending) { item in
                                PendingRequestCard(item: item, agentName: thread?.agent.name ?? "The agent", threadID: threadID) { response in
                                    if let rid = Self.requestId(item) { model.answer(threadID, requestId: rid, response) }
                                }
                                .transition(.move(edge: .bottom).combined(with: .opacity))
                            }
                        }
                        // Room for the cards' shadows, inside what clips them.
                        .padding(.horizontal, 14)
                        .padding(.top, 12)
                        .padding(.bottom, 6)
                        .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { requestsHeight = $0 }
                    }
                    .frame(height: min(requestsHeight, requestRoom))
                    .scrollBounceBehavior(.basedOnSize)
                    .scrollDismissesKeyboard(.interactively)
                    .padding(.horizontal, -14)
                    .padding(.top, -12)
                    .environment(\.requestScroller, scroller)
                    .onAppear { scroller.proxy = proxy }
                }
                .transition(.move(edge: .bottom).combined(with: .opacity))
            }
            // What stays under the requests, measured for the room it leaves them.
            VStack(spacing: 10) {
                if let queued = thread?.queued, !queued.isEmpty, model.serves(.queueAction, on: threadID) {
                    QueuedStrip(threadID: threadID, queued: queued) { text in
                        composer.put(composer.draft.isEmpty ? text : composer.draft + "\n\n" + text)
                        composer.focusRequest += 1
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
            .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { restHeight = $0 }
        }
        .padding(.horizontal, 14)
        .padding(.bottom, 6)
        .padding(.top, 8)
        .animation(.smooth(duration: 0.3), value: store.pending.map(\.id))
        .animation(.smooth(duration: 0.3), value: thread?.needsThread)
        .animation(.smooth(duration: 0.3), value: thread?.queued)
        .animation(.snappy(duration: 0.2), value: commands.isEmpty)
        .onChange(of: composer.draft) {
            // The keyboard put back the message just sent: it stays gone.
            if composer.dropEcho() { return }
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
        guard model.link(for: threadID)?.connection == .connected else {
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
        composer.cleared(after: composer.draft)
    }
}

/// The Mac asked for the new-thread sheet from a thread (`/new`): on which machine, in which
/// project, with what.
struct NewThreadRequest: Identifiable {
    let id = UUID()
    var link: HostLink
    var project: String
    var prompt: String
}
