import Foundation

/// A pretend Mac for demo mode: realistic threads across projects and agents, transcripts with
/// every item kind, a thread that keeps working, and answers to everything the phone sends. It
/// replies with the same `ServerMessage`s a real Mac would, so the whole app runs on it.
final class MockHost: Backend {
    var onMessage: ((ServerMessage) -> Void)?
    var onState: ((ConnectionState) -> Void)?

    private var threads: [ThreadSummary] = []
    private var items: [String: [TItem]] = [:]
    private var seq: [String: Int64] = [:]
    private var subscribed: Set<String> = []
    /// Each thread's item counter, for the Mac's ids: `i<n>`, `c<n>` for a turn's changes, `r<n>`
    /// for requests.
    private var counters: [String: Int] = [:]
    /// The seq at which a thread was last rewound (undo, retry, rewind): a subscribe from before
    /// it gets the whole transcript again.
    private var rewritten: [String: Int64] = [:]
    private var timer: Timer?
    private var chatter: Timer?
    private var tick = 0
    private var notes = DemoMac.notes()
    private var settings = DemoMac.settings()
    /// The visualizations thread's answer still being written: its item, and the text to come.
    private var vizStream: (tid: String, id: String, text: String, rest: [String])?
    /// The `t-card` thread's cards, by family (`pr`, `run`, `release`): their item and state.
    private var cardItems: [String: String] = [:]
    private var cardStates: [String: String] = [:]
    /// Its run was re-run: this time it passes.
    private var cardRerun = false
    private let host: HostInfo
    /// What it says it runs on (`machine_info`).
    private let platform: String

    /// The demo's Mac; `-TrekDemoPC` adds a Windows machine beside it (the same threads, on a
    /// machine of another name), for the machine tags and filter.
    init(name: String = "Tobias’s MacBook Pro", id: String = "demo-mac", platform: String = "macos") {
        // `-TrekDemoHostVersion 9.9.9`: the demo Mac runs a newer Trek than the app (Settings says an update is coming).
        host = HostInfo(id: id, name: name, version: UserDefaults.standard.string(forKey: "TrekDemoHostVersion") ?? "0.3.2")
        self.platform = platform
    }

    let projects: [ProjectSummary] = [
        MockHost.project("p-trek", "trek", hue: 24, branch: "main"),
        MockHost.project("p-api", "trek-api", hue: 212, branch: "main"),
        MockHost.project("p-atlas", "atlas-web", hue: 135, branch: "develop"),
        MockHost.project("p-field", "fieldnotes-ios", hue: 275, branch: "main"),
    ]

    var agents: [AgentOption] = [
        AgentOption(key: "claude-code", name: "Claude Code", defaultModel: "claude-opus-5-5", models: [
            ModelOption(id: "claude-opus-5-5", label: "Opus 5.5"), ModelOption(id: "claude-sonnet-5-5", label: "Sonnet 5.5"),
            ModelOption(id: "claude-haiku-4-5", label: "Haiku 4.5")], logo: "claude-code"),
        AgentOption(key: "codex", name: "Codex", defaultModel: "gpt-6-astra", models: [
            ModelOption(id: "gpt-6-astra", label: "GPT-6 Astra"), ModelOption(id: "gpt-5.6-sol", label: "GPT-5.6 Sol"),
            ModelOption(id: "gpt-5.6-luna", label: "GPT-5.6 Luna")], logo: "codex"),
        AgentOption(key: "opencode", name: "OpenCode", defaultModel: "kimi-k3", models: [
            ModelOption(id: "kimi-k3", label: "Kimi K3"), ModelOption(id: "glm-5", label: "GLM 5")], logo: "opencode"),
        AgentOption(key: "acp:cursor", name: "Cursor", defaultModel: "composer-2", models: [
            ModelOption(id: "composer-2", label: "Composer 2")], logo: "cursor"),
    ]

    static func project(_ id: String, _ name: String, hue: Int, branch: String) -> ProjectSummary {
        ProjectSummary(id: id, name: name, hue: hue, monogram: monogram(name), branch: branch, isRepo: true, icon: DemoMac.projectIcon(id))
    }

    /// The desktop's two-letter badge: first letters of the first two words, else two letters.
    static func monogram(_ name: String) -> String {
        let words = name.split { !$0.isLetter && !$0.isNumber }
        switch words.count {
        case 0: return "·"
        case 1: return String(words[0].prefix(2)).uppercased()
        default: return "\(words[0].prefix(1))\(words[1].prefix(1))".uppercased()
        }
    }

    // MARK: Backend

    func start() {
        onState?(.connecting)
        seed()
        seedReach()
        seedLinks()
        seedCards()
        // What the Mac's composer shows for each: efforts per model, each thread's effort and access.
        let efforts = ["low", "medium", "high", "xhigh", "max"]
        agents = (agents + DemoMac.addedAgents).map { a in
            var a = a
            a.models = a.models.map { m in var m = m; m.efforts = efforts; return m }
            return a
        }
        threads = threads.map { t in
            var t = t
            t.effort = t.effort ?? "high"
            t.access = t.access ?? .autoAcceptEdits
            // What the Mac's rows carry besides: context, cost, sub-agents, git.
            DemoMac.details(&t)
            return t
        }
        deliver(.welcome(re: "auth", host: host, features: Self.features), after: 0.15)
        // Sub-agents' threads aren't listed: they come as unlisted rows.
        deliver(.snapshot(Snapshot(threads: threads.filter { $0.parentId == nil }, projects: projects, agents: agents, fullAccess: true,
                                   unlisted: threads.filter { $0.parentId != nil })), after: 0.2)
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.15) { [weak self] in self?.onState?(.connected) }
        timer = Timer.scheduledTimer(withTimeInterval: 3.5, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.advanceWorkingThread() }
        }
        // `-TrekDemoChat <seconds>`: a message into the opened thread that often, each answered, so
        // new answers keep arriving under the reader (the transcript's layout as a long thread grows).
        if Launch.demoChat > 0, let tid = Launch.open {
            var n = 0
            chatter = Timer.scheduledTimer(withTimeInterval: Launch.demoChat, repeats: true) { [weak self] _ in
                MainActor.assumeIsolated {
                    n += 1
                    self?.take(.send(threadId: tid, text: "Step \(n): carry on", mode: nil, images: []), id: nil)
                }
            }
        }
    }

    func stop() {
        timer?.invalidate()
        timer = nil
        chatter?.invalidate()
        chatter = nil
    }

    func wake() {}

    @discardableResult
    func send(_ message: ClientMessage, id: String?) -> Bool {
        take(message, id: id)
        return true
    }

    private func take(_ message: ClientMessage, id: String?) {
        switch message {
        case .pair, .hello:
            break
        case .subscribe(let tid, let afterSeq, let limit):
            subscribed.insert(tid)
            if vizStream?.tid == tid { later(0.8) { [weak self] in self?.streamViz() } }
            let all = items[tid] ?? []
            let current = seq[tid] ?? 0
            if let after = afterSeq, after <= current, after >= (rewritten[tid] ?? 0) {
                // What changed since the phone's copy.
                deliverWire(WireMessage(type: "transcript", re: id, threadId: tid, reset: false, seq: current,
                                        items: all.filter { $0.seq > after }), after: 0.05)
            } else {
                // The last `limit` items, and every request still pending.
                var list = all
                var more = false
                // Turns' changed files (`c…`) come with their ends and don't count.
                if let limit, let cut = Self.cut(all, keeping: limit) {
                    list = all[..<cut].filter(\.isPendingRequest) + all[cut...]
                    more = true
                }
                deliverWire(WireMessage(type: "transcript", re: id, threadId: tid, reset: true, seq: current, items: list, more: more),
                            after: 0.05)
            }
        case .transcriptBefore(let tid, let before, let limit):
            let all = items[tid] ?? []
            guard let i = all.firstIndex(where: { $0.id == before }) else { return refuse(id, .notFound, "That item isn't in this thread any more.") }
            let start = max(0, i - min(limit, 500))
            deliverWire(WireMessage(type: "transcript_page", re: id, threadId: tid, items: Array(all[start..<i]), more: start > 0),
                        after: 0.12)
        case .turnAction(let tid, let itemID, let action, let model, let restore):
            turnAction(tid, itemID, action, model: model, restoreFiles: restore, id: id)
        case .unsubscribe(let tid):
            subscribed.remove(tid)
        case .markSeen(let tid):
            update(tid, push: false) { $0.unseen = false }
        case .ping:
            deliver(.pong(re: id))
        case .machineInfo:
            deliver(.machineInfo(re: id, MachineInfo(name: host.name, platform: platform)))
        case .useReset, .queueAction, .attachment, .backgroundOutput, .stopBackground, .icon, .noteAsset, .addNoteImage, .noteFile:
            takeParity(message, id: id)
        case .cardAction(let tid, let key, let action):
            takeCardAction(tid, key: key, action, id: id)
        case .interrupt(let tid):
            ack(id)
            for item in items[tid] ?? [] {
                if case .tool(var call) = item.body, call.status == .running {
                    call.status = .failed
                    put(tid, TItem(id: item.id, seq: 0, body: .tool(call)), keepID: true)
                }
            }
            append(tid, .error("Interrupted"))
            update(tid) { $0.runState = .idle; $0.activity = nil; $0.workingSince = nil }
            subAgentChanged(tid, to: .stopped)
        case .send(let tid, let text, _, let images):
            // Trek's own commands: `/new` opens the phone's sheet, the rest get the Mac's answer.
            let command = text.hasPrefix("/") ? text.dropFirst().split(separator: " ").first.map(String.init) : nil
            if command == "new" || command == "clear" {
                deliver(.ack(re: id, threadId: nil, open: OpenScreen(screen: "new_thread", projectId: thread(tid)?.project?.id)))
                return
            }
            if let command, let answer = DemoMac.reply(to: command) {
                ack(id)
                append(tid, .user(text: text, images: 0))
                append(tid, .notice(answer))
                return
            }
            ack(id)
            append(tid, .user(text: text, images: images.count))
            update(tid) { $0.runState = .working; $0.needs = nil; $0.workingSince = Self.now; $0.activity = "Thinking"; $0.section = $0.pinned ? .pinned : .working }
            respond(in: tid, to: text)
        case .setPrefs(let tid, let agentKey, let model, let effort, let access, let plan):
            ack(id)
            update(tid) { t in
                if let agentKey, let a = self.agents.first(where: { $0.key == agentKey }) {
                    t.agent = AgentRef(key: a.key, name: a.name)
                    t.model = a.defaultModel
                    t.modelLabel = a.models.first { $0.id == a.defaultModel }?.label
                }
                if let model {
                    t.model = model
                    t.modelLabel = self.agents.flatMap(\.models).first { $0.id == model }?.label
                }
                if let effort { t.effort = effort }
                if let access { t.access = access }
                if let plan { t.plan = plan }
            }
        case .threadAction(let tid, let action):
            if moreThreadAction(tid, action, id: id) { return }
            ack(id)
            update(tid) { t in
                switch action {
                case .pin: t.pinned = true; t.section = .pinned
                case .unpin, .unsettle: t.pinned = false; t.section = .inbox
                case .settle, .archive: t.pinned = false; t.section = .settled
                case .rename(let title): t.title = title
                default: break
                }
            }
        case .newThread(let pid, let agentKey, let model, let text, let worktree, _, _, _, let images):
            let tid = "t-new-\(Int(Date().timeIntervalSince1970))"
            let project = projects.first { $0.id == pid } ?? projects[0]
            let agent = agents.first { $0.key == agentKey } ?? agents[0]
            let modelId = model ?? agent.defaultModel
            let words = text.split(separator: " ").prefix(6).joined(separator: " ")
            let t = ThreadSummary(id: tid, title: words.isEmpty ? "New thread" : words.prefix(1).uppercased() + words.dropFirst(),
                                  project: project.ref, agent: AgentRef(key: agent.key, name: agent.name), model: modelId,
                                  modelLabel: agent.models.first { $0.id == modelId }?.label, runState: .working, needs: nil,
                                  section: .working, unseen: false, pinned: false,
                                  branch: worktree ? "trek/\(words.lowercased().split(separator: " ").prefix(3).joined(separator: "-"))" : project.branch,
                                  worktree: worktree, activity: "Thinking", workingSince: Self.now, updatedAt: Self.nowMs,
                                  additions: 0, deletions: 0)
            threads.insert(t, at: 0)
            items[tid] = []
            deliver(.ack(re: id, threadId: tid))
            deliver(.thread(t))
            append(tid, .user(text: text, images: images.count))
            respond(in: tid, to: text)
        case .answer(let tid, let rid, let response):
            answer(tid, rid, response, id: id)
        case .usage:
            deliver(.usage(re: id, DemoMac.usage()), after: 0.4)
        case .basecamp(let range):
            deliver(.basecamp(re: id, DemoMac.basecamp(range, threads: threads.filter { $0.parentId == nil }, projects: projects)), after: 0.3)
        case .notes:
            deliver(.notes(re: id, notes.sorted { $0.modified > $1.modified }.map(DemoMac.summary)))
        case .note(let nid):
            if let n = notes.first(where: { $0.id == nid }) { deliver(.note(re: id, n)) } else { refuse(id, .notFound, "No such note") }
        case .createNote(let body):
            let n = Note(id: "n-\(UUID().uuidString.prefix(8))", title: DemoMac.title(of: body), body: body, modified: Self.now)
            notes.append(n)
            deliver(.note(re: id, n))
        case .saveNote(let nid, let body, let modified):
            guard let i = notes.firstIndex(where: { $0.id == nid }) else { return refuse(id, .notFound, "No such note") }
            if let modified, modified != notes[i].modified {
                return refuse(id, .conflict, "This note changed on the Mac since you opened it. Open it again to see the change.")
            }
            notes[i].body = body
            notes[i].title = DemoMac.title(of: body)
            notes[i].modified = Self.now
            deliver(.note(re: id, notes[i]))
        case .deleteNote(let nid):
            notes.removeAll { $0.id == nid }
            ack(id)
        case .gitStatus(let target) where gitThread(target)?.project?.id.isEmpty == true,
             .gitBranches(let target) where gitThread(target)?.project?.id.isEmpty == true:
            refuse(id, .badRequest, "This thread has no folder of its own")
        case .gitStatus(let target):
            deliver(.gitStatus(re: id, DemoMac.gitStatus(thread: gitThread(target))), after: 0.2)
        case .gitDiff(_, let path):
            deliver(.gitDiff(re: id, DemoMac.diff(path)), after: 0.2)
        case .gitBranches:
            deliver(.gitBranches(re: id, DemoMac.branches))
        case .gitCommit(_, let message):
            if message.trimmingCharacters(in: .whitespaces).isEmpty { refuse(id, .badRequest, "Write a commit message first.") } else { ack(id, after: 0.6) }
        case .gitPush:
            ack(id, after: 0.8)
        case .gitSwitch(let target, let branch):
            let t = gitThread(target)
            if t?.worktree == true {
                refuse(id, .conflict, "This thread works in a worktree: its branch stays checked out there.")
            } else if t?.runState == .working {
                refuse(id, .conflict, "A thread is working in this folder: switch once it's done.")
            } else {
                if let t { update(t.id) { $0.branch = branch } }
                ack(id, after: 0.3)
            }
        case .worktreeMerge(let tid):
            if thread(tid)?.worktree == true {
                refuse(id, .conflict, "2 files aren't committed yet. Commit or revert them first.")
            } else {
                refuse(id, .badRequest, "This thread has no worktree.")
            }
        case .worktreeRemove(let tid, let deleteBranch, let force):
            guard thread(tid)?.worktree == true else { return refuse(id, .badRequest, "This thread has no worktree.") }
            if !force {
                let branch = thread(tid)?.branch ?? "its branch"
                let lose = deleteBranch ? "\(branch) has 2 commits that main doesn't have." : "\(branch) keeps its 2 commits that main doesn't have."
                return refuse(id, .conflict, "2 uncommitted changes in the worktree would be lost. \(lose)")
            }
            ack(id, after: 0.5)
            update(tid) { $0.worktree = false; $0.base = nil; $0.branch = "main" }
            append(tid, .notice("Its worktree was removed. The thread runs in the project folder now, in a new agent session."))
        case .commands(let tid):
            deliver(.commands(re: id, threadId: tid, DemoMac.commands()))
        case .settings:
            deliver(.settings(re: id, settings))
        case .setSettings(let change):
            if change.defaultAccess == .fullAccess && settings.fullAccess != true {
                return refuse(id, .badRequest, "Full access is locked on this Mac: unlock it in Trek's Permissions settings first.")
            }
            settings = change.applied(to: settings)
            if change.push == true && settings.push.topic.isEmpty || change.newPushTopic {
                settings.push.topic = "trek-\(UUID().uuidString.replacingOccurrences(of: "-", with: "").prefix(32))"
            }
            (settings.push.topicUrl, settings.push.subscribeUrl) = DemoMac.ntfyLinks(server: settings.push.server, topic: settings.push.topic)
            deliver(.settings(re: id, settings))
        case .search(let query, let limit):
            search(query, limit: limit ?? 50, id: id)
        }
    }

    private func thread(_ tid: String) -> ThreadSummary? { threads.first { $0.id == tid } }

    private func gitThread(_ target: GitTarget) -> ThreadSummary? {
        if case .thread(let tid) = target { thread(tid) } else { nil }
    }

    /// Where to cut so `limit` items that aren't changed files are kept; nil when they all fit.
    private static func cut(_ all: [TItem], keeping limit: Int) -> Int? {
        var kept = 0
        var i = all.count
        while i > 0 {
            if !all[i - 1].id.hasPrefix("c") {
                if kept == limit { return i }
                kept += 1
            }
            i -= 1
        }
        return nil
    }

    /// Undo, retry, rewind or fork, as the Mac does them: undo, retry and rewind cut the
    /// transcript back (to before the turn's message) and tell subscribers to load it again.
    private func turnAction(_ tid: String, _ itemID: String, _ action: TurnAction, model: String?, restoreFiles: Bool, id: String?) {
        let all = items[tid] ?? []
        guard let i = all.firstIndex(where: { $0.id == itemID }) else {
            return refuse(id, .notFound, "That message isn't in this thread any more.")
        }
        if action != .fork, thread(tid)?.runState == .working {
            return refuse(id, .badRequest, "Stop the running turn to \(action.rawValue).")
        }
        let isMessage = if case .user = all[i].body { true } else { false }
        switch action {
        case .fork:
            // From a turn: everything up to its end (and what it changed). From a message: what came before it.
            var end = isMessage ? i : i + 1
            if !isMessage, end < all.count, case .changes = all[end].body { end += 1 }
            guard var t = thread(tid) else { return }
            let newID = "t-fork-\(Self.nowMs)"
            t.id = newID
            t.title = "Fork of \(t.title)"
            t.runState = .idle
            t.needs = nil
            t.activity = nil
            t.workingSince = nil
            t.section = .inbox
            t.pinned = false
            t.unseen = false
            t.updatedAt = Self.now
            threads.insert(t, at: 0)
            items[newID] = all.prefix(end).filter { !$0.isPendingRequest }
            seq[newID] = seq[tid]
            counters[newID] = counters[tid]
            if isMessage, case .user(let text, _, _) = all[i].body {
                deliver(.ack(re: id, threadId: newID, text: text))
            } else {
                deliver(.ack(re: id, threadId: newID))
            }
            deliver(.thread(t))
        case .rewind:
            guard case .user(let text, _, _) = all[i].body else { return refuse(id, .badRequest, "Rewind from one of your messages.") }
            cut(tid, at: i)
            deliver(.ack(re: id, threadId: nil, text: text))
            rewound(tid, restoreFiles: restoreFiles)
        case .undo, .retry:
            guard let start = all[...i].lastIndex(where: { if case .user = $0.body { true } else { false } }),
                  case .user(let text, let images, _) = all[start].body else {
                return refuse(id, .badRequest, "This turn didn't start from a message of yours.")
            }
            cut(tid, at: start)
            if action == .undo { deliver(.ack(re: id, threadId: nil, text: text)) } else { ack(id) }
            rewound(tid, restoreFiles: restoreFiles)
            if action == .retry {
                if let model {
                    update(tid) { t in
                        t.model = model
                        t.modelLabel = self.agents.flatMap(\.models).first { $0.id == model }?.label
                    }
                }
                later(0.3) { [weak self] in
                    guard let self else { return }
                    self.append(tid, .user(text: text, images: images))
                    self.update(tid) { $0.runState = .working; $0.needs = nil; $0.workingSince = Self.now; $0.activity = "Thinking" }
                    self.respond(in: tid, to: text)
                }
            }
        }
    }

    /// Drops item `at` and everything after it.
    private func cut(_ tid: String, at i: Int) {
        items[tid] = Array((items[tid] ?? []).prefix(i))
        let n = nextSeq(tid)
        rewritten[tid] = n
        update(tid) { $0.runState = .idle; $0.needs = nil; $0.activity = nil; $0.workingSince = nil; $0.updatedAt = Self.now }
    }

    private func rewound(_ tid: String, restoreFiles: Bool) {
        if restoreFiles { update(tid, push: true) { $0.additions = max(0, $0.additions - 12); $0.deletions = max(0, $0.deletions - 3) } }
        if subscribed.contains(tid) { deliver(.transcriptReset(tid), after: 0.05) }
    }

    private func refuse(_ id: String?, _ code: ErrorCode, _ message: String) {
        deliver(.error(re: id, code: code, message: message), after: 0.2)
    }

    // MARK: Behaviour

    private func answer(_ tid: String, _ rid: String, _ response: AnswerResponse, id: String?) {
        guard let item = items[tid]?.first(where: { $0.isPendingRequest && Self.requestId($0) == rid }) else {
            deliver(.error(re: id, code: .conflict, message: "That request was already answered."))
            return
        }
        ack(id)
        var allowed = true
        switch (item.body, response) {
        case (.approval(var a), .approval(let d)):
            a.state = switch d { case .allow: .allowed; case .allowForSession: .allowedForSession; case .deny: .denied }
            allowed = d != .deny
            put(tid, TItem(id: item.id, seq: 0, body: .approval(a)), keepID: true)
        case (.question(var q), .questions(let answers)):
            q.state = .answered
            q.answers = answers
            put(tid, TItem(id: item.id, seq: 0, body: .question(q)), keepID: true)
        case (.plan(var p), .plan(let approve, let feedback)):
            p.state = approve ? .approved : .rejected
            allowed = approve
            put(tid, TItem(id: item.id, seq: 0, body: .plan(p)), keepID: true)
            if let feedback, !feedback.isEmpty { append(tid, .user(text: feedback, images: 0)) }
        default:
            break
        }
        update(tid) { $0.needs = nil; $0.runState = .working; $0.workingSince = Self.now; $0.activity = allowed ? "Running" : "Thinking" }
        subAgentChanged(tid, to: .running)
        if case .approval(let a) = item.body, allowed {
            let tool = TItem(id: newID(tid, .notice("")), seq: 0, body: .tool(ToolCall(callId: "c\(Self.nowMs)", tool: .command, title: "Run", detail: a.detail, status: .running, output: "", added: nil, removed: nil)))
            put(tid, tool)
            later(2.2) { [weak self] in
                guard let self else { return }
                if case .tool(var c) = tool.body {
                    c.status = .done
                    c.output = "running 48 tests\n........................................\ntest result: ok. 48 passed; 0 failed"
                    self.put(tid, TItem(id: tool.id, seq: 0, body: .tool(c)), keepID: true)
                }
                self.finish(tid, with: "All **48** auth tests pass, including the new `concurrent_refresh_keeps_newest_token` regression test (50 concurrent refreshes, 200 runs, no failures).\n\nThe fix is two lines in `refresh()`: the expiry check now happens *after* taking the lock.", took: 214)
            }
        } else {
            respond(in: tid, to: allowed ? "" : "denied")
        }
    }

    private func respond(in tid: String, to text: String) {
        later(1.0) { [weak self] in
            self?.append(tid, .reasoning(text: "The user wants: \(text.prefix(80)). Let me look at the relevant code first."))
            self?.update(tid) { $0.activity = "Reading files" }
        }
        later(2.0) { [weak self] in
            self?.append(tid, .tool(ToolCall(callId: "c1", tool: .search, title: "Search", detail: "\"\(text.split(separator: " ").first ?? "todo")\" in src/", status: .done, output: "", added: nil, removed: nil)))
            self?.append(tid, .tool(ToolCall(callId: "c2", tool: .read, title: "Read", detail: "src/lib.rs", status: .done, output: "", added: nil, removed: nil)))
        }
        later(3.6) { [weak self] in
            let reply = text == "denied"
                ? "Understood — I won't run that. I'll stop here; tell me how you'd like to proceed."
                : "On it. I've read through `src/lib.rs` and the change is small:\n\n- Keep the public API as it is\n- Move the check into the helper\n- Add a test for the edge case\n\nI'll make the edit now and run the tests."
            self?.finish(tid, with: reply, took: 4)
        }
    }

    private func finish(_ tid: String, with text: String, took: Int) {
        append(tid, .assistant(text: text, streaming: false))
        append(tid, .turnEnd(tookSecs: took))
        update(tid) {
            $0.runState = .idle
            $0.activity = nil
            $0.workingSince = nil
            $0.unseen = !self.subscribed.contains(tid)
            if $0.section == .working { $0.section = .inbox }
        }
    }

    /// The visualizations thread's last answer arrives a few characters at a time, its chart
    /// drawing in as it comes (and stopping part-way with `-TrekVizHold`).
    private func streamViz() {
        guard var s = vizStream, !s.rest.isEmpty else { return }
        if let hold = DemoMac.vizHold, s.text.components(separatedBy: "```trek-viz\n").last?.count ?? 0 >= hold { return }
        s.text += s.rest.removeFirst()
        let done = s.rest.isEmpty
        vizStream = done ? nil : s
        put(s.tid, TItem(id: s.id, seq: 0, body: .assistant(text: s.text, streaming: !done)), keepID: true)
        if done {
            append(s.tid, .turnEnd(tookSecs: 9))
            update(s.tid) { $0.runState = .idle; $0.activity = nil; $0.workingSince = nil }
        } else {
            later(0.09) { [weak self] in self?.streamViz() }
        }
    }

    /// The pinned thread keeps working: a step every few seconds, then it starts over.
    private func advanceWorkingThread() {
        let tid = "t-inbox"
        guard threads.contains(where: { $0.id == tid && $0.runState == .working }) else { return }
        tick += 1
        let steps: [(ToolKind, String, String)] = [
            (.read, "Read", "crates/trek-core/src/store.rs"),
            (.search, "Search", "\"snoozed_until\" in crates/"),
            (.edit, "Edit", "crates/trek-core/src/store.rs"),
            (.command, "Run", "cargo test -p trek-core inbox"),
            (.read, "Read", "crates/trek-app/src/sidebar.rs"),
        ]
        let (kind, title, detail) = steps[tick % steps.count]
        let id = newID(tid, .notice(""))
        put(tid, TItem(id: id, seq: 0, body: .tool(ToolCall(callId: id, tool: kind, title: title, detail: detail, status: kind == .command ? .running : .done, output: "", added: kind == .edit ? 9 : nil, removed: kind == .edit ? 2 : nil))))
        update(tid, push: true) {
            $0.activity = kind == .command ? "Running \(detail)" : "\(title == "Edit" ? "Editing" : title == "Read" ? "Reading" : "Searching") \((detail as NSString).lastPathComponent)"
            if kind == .edit { $0.additions += 9; $0.deletions += 2 }
        }
        if kind == .command {
            later(2.0) { [weak self] in
                self?.put(tid, TItem(id: id, seq: 0, body: .tool(ToolCall(callId: id, tool: kind, title: title, detail: detail, status: .done, output: "test result: ok. 31 passed; 0 failed", added: nil, removed: nil))), keepID: true)
            }
        }
    }

    // MARK: Plumbing

    private static var now: Int64 { Int64(Date().timeIntervalSince1970 * 1000) }
    private static var nowMs: Int64 { now }

    private static func requestId(_ item: TItem) -> String? {
        switch item.body {
        case .approval(let a): a.requestId
        case .question(let q): q.requestId
        case .plan(let p): p.requestId
        default: nil
        }
    }

    func later(_ secs: Double, _ f: @escaping () -> Void) {
        DispatchQueue.main.asyncAfter(deadline: .now() + secs) { f() }
    }

    func deliver(_ m: ServerMessage, after: Double = 0.03) {
        DispatchQueue.main.asyncAfter(deadline: .now() + after) { [weak self] in self?.post(.message(m)) }
    }

    /// What's on its way to the phone, in order. Transcripts go as JSON, encoded (as the Mac
    /// would) and read back (as `TrekClient` does) off the main thread.
    private enum Outgoing {
        case message(ServerMessage)
        case wire(WireMessage)
    }

    private var outbox: [Outgoing] = []
    private var sending = false

    private func post(_ o: Outgoing) {
        outbox.append(o)
        guard !sending else { return }
        sending = true
        Task { await drain() }
    }

    private func drain() async {
        while !outbox.isEmpty {
            switch outbox.removeFirst() {
            case .message(let m):
                onMessage?(m)
            case .wire(let w):
                guard let data = await Self.encode(w) else { continue }
                if let m = await Perf.measureAsync("decode", "\(data.count / 1024) KB", { await ServerMessage.decodeInBackground(data) }) {
                    onMessage?(m)
                }
            }
        }
        sending = false
    }

    @concurrent private nonisolated static func encode(_ w: WireMessage) async -> Data? {
        let encoder = JSONEncoder()
        encoder.keyEncodingStrategy = .convertToSnakeCase
        return try? encoder.encode(w)
    }

    /// A transcript or a page of one, as a real Mac sends it.
    nonisolated struct WireMessage: Encodable, Sendable {
        var type: String
        var re: String?
        var threadId: String
        var reset: Bool? = nil
        var seq: Int64? = nil
        var items: [TItem]
        var more: Bool? = nil
    }

    private func deliverWire(_ message: WireMessage, after: Double) {
        DispatchQueue.main.asyncAfter(deadline: .now() + after) { [weak self] in self?.post(.wire(message)) }
    }

    func ack(_ id: String?, after: Double = 0.03) { deliver(.ack(re: id, threadId: nil), after: after) }

    private func nextSeq(_ tid: String) -> Int64 {
        let n = (seq[tid] ?? 0) + 1
        seq[tid] = n
        return n
    }

    private func append(_ tid: String, _ body: ItemBody) {
        put(tid, TItem(id: newID(tid, body), seq: 0, body: body))
    }

    /// The next item id, as the Mac makes them.
    private func newID(_ tid: String, _ body: ItemBody) -> String {
        let n = (counters[tid] ?? 0) + 1
        counters[tid] = n
        switch body {
        case .approval, .question, .plan: return "r\(n)"
        case .changes: return "c\(n)"
        default: return "i\(n)"
        }
    }

    /// Adds or replaces an item (a fresh seq either way) and tells subscribers.
    private func put(_ tid: String, _ item: TItem, keepID: Bool = false) {
        var item = item
        item.seq = nextSeq(tid)
        if item.at == nil { if case .user = item.body { item.at = Self.now } else if case .turnEnd = item.body { item.at = Self.now } }
        var list = items[tid] ?? []
        if let i = list.firstIndex(where: { $0.id == item.id }) { list[i] = item } else { list.append(item) }
        items[tid] = list
        if subscribed.contains(tid) { deliver(.item(threadId: tid, item: item)) }
        update(tid, push: false) { $0.updatedAt = Self.now }
    }

    func update(_ tid: String, push: Bool = true, _ f: (inout ThreadSummary) -> Void) {
        guard let i = threads.firstIndex(where: { $0.id == tid }) else { return }
        f(&threads[i])
        if push { deliver(threads[i].parentId == nil ? .thread(threads[i]) : .unlisted(threads[i])) }
    }

    // MARK: Sample data

    private func seed() {
        let now = Self.now
        let m: Int64 = 60_000
        let p = Dictionary(uniqueKeysWithValues: projects.map { ($0.id, $0.ref) })
        func t(_ id: String, _ title: String, _ pid: String, _ agent: String, _ model: String, _ state: RunState,
               needs: Needs? = nil, section: ThreadSection, unseen: Bool = false, pinned: Bool = false, branch: String?,
               worktree: Bool = false, activity: String? = nil, since: Int64? = nil, ago: Int64, add: Int = 0, del: Int = 0) -> ThreadSummary {
            let a = agents.first { $0.key == agent }!
            return ThreadSummary(id: id, title: title, project: p[pid], agent: AgentRef(key: a.key, name: a.name), model: model,
                                 modelLabel: a.models.first { $0.id == model }?.label, runState: state, needs: needs,
                                 section: section, unseen: unseen, pinned: pinned, branch: branch, worktree: worktree,
                                 activity: activity, workingSince: since.map { now - $0 * m }, updatedAt: now - ago * m,
                                 additions: add, deletions: del)
        }
        threads = [
            t("t-inbox", "Snoozed threads raise their hand", "p-trek", "claude-code", "claude-opus-5-5", .working,
              section: .pinned, pinned: true, branch: "inbox/raise-hand", worktree: true, activity: "Running cargo test -p trek-core",
              since: 6, ago: 0, add: 86, del: 21),
            t("t-release", "Release notes for 0.3.3", "p-trek", "codex", "gpt-6-astra", .idle, section: .pinned, pinned: true,
              branch: "main", ago: 60 * 26, add: 48, del: 2),
            t("t-flaky", "Fix the flaky session refresh test", "p-api", "claude-code", "claude-opus-5-5", .needsYou,
              needs: Needs(kind: .approval, text: "Run rm -rf target/ && cargo test -p auth"), section: .inbox, unseen: true,
              branch: "fix/auth-flake", worktree: true, ago: 2, add: 43, del: 3),
            t("t-ratelimit", "Rate limit the /login endpoint", "p-api", "codex", "gpt-6-astra", .needsYou,
              needs: Needs(kind: .question, text: "Where should the counters live?"), section: .inbox, unseen: true,
              branch: "feat/login-rate-limit", worktree: true, ago: 9, add: 12, del: 0),
            t("t-settings", "Migrate settings to the v2 schema", "p-atlas", "claude-code", "claude-sonnet-5-5", .needsYou,
              needs: Needs(kind: .plan, text: "Plan ready: 5 steps"), section: .inbox, unseen: true, branch: "develop", ago: 14),
            t("t-vite", "Upgrade to Vite 7", "p-atlas", "opencode", "kimi-k3", .failed,
              needs: Needs(kind: .failed, text: "Build failed: 3 type errors"), section: .inbox, branch: "chore/vite-7",
              worktree: true, ago: 31, add: 120, del: 96),
            t("t-offline", "Offline banner and queued sends", "p-field", "claude-code", "claude-sonnet-5-5", .working,
              section: .working, branch: "feat/offline", worktree: true, activity: "Editing OfflineBanner.swift", since: 3, ago: 0,
              add: 64, del: 8),
            t("t-profile", "Profile the transcript renderer", "p-trek", "codex", "gpt-5.6-sol", .working, section: .working,
              branch: "perf/transcript", worktree: true, activity: "Reading thread_view.rs", since: 12, ago: 1),
            t("t-docs", "Dark mode for the docs site", "p-atlas", "acp:cursor", "composer-2", .working, section: .working,
              branch: "docs/dark", activity: "Running pnpm build", since: 1, ago: 0, add: 31, del: 14),
            t("t-tailscale", "Put the Tailscale address in the pairing QR", "p-trek", "claude-code", "claude-opus-5-5", .idle,
              section: .inbox, unseen: true, branch: "remote/tailscale", worktree: true, ago: 22, add: 54, del: 3),
            t("t-worktree", "Explain the worktree cleanup job", "p-api", "codex", "gpt-6-astra", .idle, section: .inbox,
              branch: "main", ago: 60 * 3),
            t("t-tokio", "Bump tokio and fix deprecations", "p-trek", "opencode", "kimi-k3", .idle, section: .settled,
              branch: "main", ago: 60 * 30, add: 18, del: 22),
            t("t-onboarding", "Onboarding copy pass", "p-field", "acp:cursor", "composer-2", .idle, section: .settled,
              branch: "main", ago: 60 * 24 * 4, add: 40, del: 37),
            // Failed, then settled: history, not something waiting on you.
            t("t-crash", "Trek crashed after using a reset", "p-trek", "codex", "gpt-6-astra", .failed,
              needs: Needs(kind: .failed, text: "Client error: Protocol error (invalid_argument)"), section: .settled,
              branch: "main", ago: 60 * 20),
        ]
        for th in threads { seq[th.id] = 0; items[th.id] = [] }

        var sentAt: [String: Int64] = [:]
        func add(_ tid: String, _ body: ItemBody, at: Int64? = nil) {
            let n = nextSeq(tid)
            var at = at
            // A turn ends its took-seconds after the message that started it.
            if case .user = body { sentAt[tid] = at }
            if at == nil, case .turnEnd(let secs) = body { at = (sentAt[tid] ?? now) + Int64(secs) * 1000 }
            items[tid, default: []].append(TItem(id: newID(tid, body), seq: n, at: at, body: body))
        }
        func tool(_ kind: ToolKind, _ title: String, _ detail: String, _ status: ToolStatus = .done, out: String = "",
                  add: Int? = nil, del: Int? = nil) -> ItemBody {
            .tool(ToolCall(callId: UUID().uuidString, tool: kind, title: title, detail: detail, status: status, output: out, added: add, removed: del))
        }

        // A crash report pasted whole: a message of a megabyte and more, with the underscores and
        // backticks that make Markdown cut it into thousands of runs.
        let report = (0..<14_000).map { i in
            "\(i % 60)   Trek  0x00000001\(String(format: "%08lx", i * 2654435761 % 0xFFFF_FFFF)) __CFRUNLOOP_IS_CALLING_OUT_TO_AN_OBSERVER_CALLBACK_FUNCTION__ + \(i % 97) `crates/trek-app/src/remote/usage.rs:\(i % 400)` _dispatch_main_queue_drain"
        }.joined(separator: "\n")
        add("t-crash", .user(text: "I tried using a reset and clicking to use it. Trek crashed and struggled to reopen.\n-------------------------------------\nTranslated Report\n\n" + report, images: 0), at: now - 21 * 60 * m)
        add("t-crash", .error("Client error: Protocol error (invalid_argument): the request was too large"))

        // Needs approval
        var id = "t-flaky"
        add(id, .user(text: "`session_refresh_keeps_user` fails about 1 in 10 runs on CI. Find out why and fix it.", images: 0), at: now - 9 * m)
        add(id, .reasoning(text: "A flaky test around refresh usually means a race. Let me read the session code and the test."))
        add(id, tool(.read, "Read", "src/auth/session.rs"))
        add(id, tool(.read, "Read", "tests/session_refresh.rs"))
        add(id, tool(.search, "Search", "\"refresh_token(\" in src/"))
        add(id, .reasoning(text: "expires_at is read before the lock is taken."))
        add(id, tool(.command, "Run", "cargo test -p auth session_refresh -- --test-threads=8", out: "test session_refresh_keeps_user ... FAILED\nthread 'session_refresh_keeps_user' panicked at tests/session_refresh.rs:41:5:\nassertion `left == right` failed\n  left: \"tok_2\"\n right: \"tok_3\""))
        add(id, .assistant(text: "Found it. `refresh()` checks the token's expiry **before** taking the lock, so two concurrent refreshes can both see an expired token, and the slower one overwrites the newer token with a stale one:\n\n```rust\nlet expired = self.token.expires_at < now; // read outside the lock\nlet mut guard = self.lock.lock().await;\n```\n\nThe test hits it because it fires two requests in the same tick. I'll move the check under the lock and add a regression test that runs 50 refreshes at once.", streaming: false))
        add(id, .reasoning(text: "Make the edit, then the test."))
        add(id, tool(.edit, "Edit", "src/auth/session.rs", add: 12, del: 3))
        add(id, tool(.edit, "Write", "tests/concurrent_refresh.rs", add: 31, del: 0))
        add(id, .assistant(text: "Both edits are in. The build cache still has artifacts from the old lockfile, so I'd like to clear `target/` and run the auth tests.", streaming: false))
        add(id, .approval(ApprovalRequest(requestId: "r-flaky", title: "Run command", detail: "rm -rf target/ && cargo test -p auth session_refresh -- --test-threads=8 --nocapture 2>&1 | tee /tmp/auth-flaky.log", state: .pending)))

        // Question
        id = "t-ratelimit"
        add(id, .user(text: "Rate limit /login: 5 attempts per minute per IP and per account. Return 429 with Retry-After.", images: 0), at: now - 16 * m)
        add(id, tool(.read, "Read", "src/routes/login.rs"))
        add(id, tool(.read, "Read", "src/state.rs"))
        add(id, tool(.search, "Search", "\"redis\" in Cargo.toml"))
        add(id, .assistant(text: "The handler is easy to wrap with a middleware. One decision first: the service runs **3 replicas** behind the load balancer, so an in-memory counter would let an attacker get 15 tries a minute.", streaming: false))
        add(id, .question(QuestionRequest(requestId: "r-rate", questions: [
            Question(header: "Store", question: "Where should the counters live?", options: [
                QuestionOption(label: "Redis", description: "Shared by all replicas; adds a dependency on the existing cache"),
                QuestionOption(label: "Postgres", description: "No new infra; one UPSERT per attempt"),
                QuestionOption(label: "In memory", description: "Per replica; simplest, but limits multiply")],
                     multi: false, secret: false)], state: .pending, answers: nil)))

        // Plan
        id = "t-settings"
        add(id, .user(text: "Plan the migration of user settings to the v2 schema. Don't change anything yet.", images: 0), at: now - 20 * m)
        add(id, tool(.read, "Read", "src/settings/schema.ts"))
        add(id, tool(.read, "Read", "src/settings/migrate.ts"))
        add(id, tool(.search, "Search", "\"settings.v1\" in src/"))
        add(id, .plan(PlanRequest(requestId: "r-plan", markdown: "## Migrate settings to v2\n\n1. Add `SettingsV2` with the nested `appearance` and `notifications` groups\n2. Write `migrateV1toV2()` with defaults for the 4 new keys\n3. Run it on load, keep a `settings.v1.bak` copy for one release\n4. Update the 11 call sites that read `settings.theme` directly\n5. Tests: round-trip every v1 fixture through the migration", state: .pending)))

        // Failed
        id = "t-vite"
        add(id, .user(text: "Upgrade to Vite 7 and fix whatever breaks.", images: 0), at: now - 40 * m)
        add(id, tool(.edit, "Edit", "package.json", add: 6, del: 6))
        add(id, tool(.command, "Run", "pnpm install"))
        add(id, tool(.edit, "Edit", "vite.config.ts", add: 14, del: 9))
        add(id, tool(.command, "Run", "pnpm build", .failed, out: "src/env.d.ts:3:1 - error TS2304: Cannot find name 'ImportMetaEnv'.\nsrc/main.ts:12:7 - error TS2339\nsrc/router.ts:40:3 - error TS2345\n\nFound 3 errors."))
        add(id, .error("Build failed: 3 type errors in src/env.d.ts, src/main.ts and src/router.ts"))
        add(id, .turnEnd(tookSecs: 188))

        // Working (pinned)
        id = "t-inbox"
        add(id, .user(text: "Snoozed threads should come back to the inbox early when they need the user: an approval, a question, a failure or a finished turn. Make it work and test it.", images: 0), at: now - 6 * m)
        add(id, .reasoning(text: "Section is computed in Thread::own_section. Snoozed wins over needs_you today."))
        add(id, tool(.read, "Read", "crates/trek-core/src/store.rs"))
        add(id, tool(.search, "Search", "\"Section::Snoozed\" in crates/"))
        add(id, tool(.read, "Read", "crates/trek-app/src/sidebar.rs"))
        add(id, .assistant(text: "`own_section` returns **Snoozed** before it checks `needs_you()`, so a snoozed thread with a pending approval stays hidden. I'll check needs-you first and add a `raised_hand` flag so the sidebar can flash the row when it wakes.", streaming: false))
        add(id, tool(.edit, "Edit", "crates/trek-core/src/store.rs", add: 18, del: 4))
        add(id, tool(.edit, "Edit", "crates/trek-app/src/sidebar.rs", add: 22, del: 6))
        add(id, tool(.command, "Run", "cargo test -p trek-core inbox", out: "   Compiling trek-core v0.3.2\n    Finished test profile in 14.2s\n     Running unittests src/lib.rs\ntest result: ok. 29 passed; 0 failed"))

        // Other working threads
        id = "t-offline"
        add(id, .user(text: "Show a banner when the Mac is unreachable and queue sends until it's back.", images: 2), at: now - 3 * m)
        add(id, tool(.read, "Read", "Fieldnotes/Sync/Connection.swift"))
        add(id, tool(.edit, "Write", "Fieldnotes/Views/OfflineBanner.swift", add: 64, del: 0))
        add(id, tool(.edit, "Edit", "Fieldnotes/Views/RootView.swift", .running, add: 8, del: 2))
        id = "t-profile"
        add(id, .user(text: "Scrolling a 3,000-item transcript drops frames. Profile it and tell me where the time goes before changing anything.", images: 0), at: now - 12 * m)
        add(id, tool(.command, "Run", "cargo build --release -p trek-app"))
        add(id, tool(.read, "Read", "crates/trek-app/src/thread_view.rs"))
        id = "t-docs"
        add(id, .user(text: "Add a dark theme to the docs site that follows the system setting.", images: 0), at: now - 1 * m)
        add(id, tool(.edit, "Edit", "docs/.vitepress/theme/style.css", add: 31, del: 14))
        add(id, tool(.command, "Run", "pnpm build", .running))

        // Finished
        id = "t-tailscale"
        add(id, .user(text: "When Tailscale is up, put the tailnet address in the pairing QR instead of the LAN one.", images: 0), at: now - 30 * m)
        add(id, tool(.command, "Run", "tailscale ip -4"))
        add(id, tool(.edit, "Edit", "crates/trek-remote/src/pairing.rs", add: 41, del: 3))
        add(id, tool(.edit, "Write", "crates/trek-remote/tests/pairing.rs", add: 13, del: 0))
        add(id, tool(.command, "Run", "cargo test -p trek-remote", out: "test result: ok. 37 passed; 0 failed"))
        add(id, .assistant(text: "Done. The QR now prefers the **tailnet address** (`100.x` or the MagicDNS name) when `tailscale status` reports the Mac online, and falls back to the LAN address otherwise.\n\n- `advertise_address()` picks the address\n- The QR text shows which network it uses\n- 3 new tests cover both paths and a stopped Tailscale", streaming: false))
        add(id, .turnEnd(tookSecs: 412))

        id = "t-release"
        add(id, .user(text: "Draft release notes for 0.3.3 from the commits since 0.3.2.", images: 0), at: now - 27 * 60 * m)
        add(id, tool(.command, "Run", "git log v0.3.2..HEAD --oneline"))
        add(id, .assistant(text: "## Trek 0.3.3\n\n- **Reports parked by a quit stay parked** through later launches\n- Agent updates wait for background work to finish\n- Notes are told on delivery\n\nWant me to add the commit links?", streaming: false))
        add(id, .turnEnd(tookSecs: 41))

        id = "t-worktree"
        add(id, .user(text: "What does the worktree cleanup job actually delete?", images: 0), at: now - 3 * 60 * m)
        add(id, tool(.read, "Read", "src/jobs/worktree_gc.rs"))
        add(id, .assistant(text: "It removes worktrees whose branch was **merged** into the default branch more than 7 days ago, and never touches one with uncommitted changes.", streaming: false))
        add(id, .turnEnd(tookSecs: 18))

        // The files finished turns changed, after their ends.
        for tid in ["t-tailscale", "t-vite", "t-release"] {
            if let c = DemoMac.changes(for: tid) { add(tid, .changes(c)) }
        }

        for tid in ["t-tokio", "t-onboarding"] {
            add(tid, .user(text: threads.first { $0.id == tid }!.title, images: 0), at: now - 30 * 60 * m)
            add(tid, tool(.command, "Run", "cargo update -p tokio"))
            add(tid, .assistant(text: "Done, and the build is clean.", streaming: false))
            add(tid, .turnEnd(tookSecs: 64))
        }

        // Native visualizations: one of each, one that won't draw, and one being written.
        // (`-TrekVizKind line` makes it `t-viz-line`: the phone keeps each transcript it has seen.)
        id = DemoMac.vizKind.map { "t-viz-\($0)" } ?? "t-viz"
        let viz = DemoMac.vizThread()
        threads.insert(t(id, "Draw this week's numbers", "p-trek", "claude-code", "claude-opus-5-5", viz.streamed == nil ? .idle : .working,
                         section: .inbox, branch: "main", activity: viz.streamed == nil ? nil : "Writing the answer",
                         since: viz.streamed == nil ? nil : 0, ago: 0), at: 2)
        seq[id] = 0
        items[id] = []
        add(id, .user(text: "Show me one of each native visualization, so I can check how they draw on the phone.", images: 0), at: now - 2 * m)
        if let gallery = viz.gallery { add(id, .assistant(text: gallery, streaming: false)) }
        if let invalid = viz.invalid { add(id, .assistant(text: invalid, streaming: false)) }
        add(id, .turnEnd(tookSecs: 38))
        if let streamed = viz.streamed {
            add(id, .user(text: "And one more, drawn as you write it.", images: 0), at: now - 10_000)
            add(id, .assistant(text: streamed.seed, streaming: true))
            vizStream = (id, items[id]!.last!.id, streamed.seed, DemoMac.tokens(streamed.rest))
        }
        // Its stream moves the seqs on each launch: a copy the phone kept from another is redrawn whole.
        rewritten[id] = seq[id]

        // Maths typeset inline and on its own, and a live `trek-html` page.
        id = "t-math"
        threads.insert(t(id, "Bayes’ rule, with numbers", "p-trek", "claude-code", "claude-opus-5-5", .idle, section: .inbox,
                         branch: "main", ago: 1), at: 3)
        seq[id] = 0
        items[id] = []
        add(id, .user(text: DemoMac.mathAsk, images: 0), at: now - 4 * m)
        add(id, .assistant(text: DemoMac.mathAnswer, streaming: false))
        add(id, .turnEnd(tookSecs: 52))

        // A thread in no project: its chat folder has no git (no branch, and git requests are
        // refused, as the Mac does). `-TrekDemoOldMac YES` plays a Mac from before rows said so:
        // a branch anyway, and no `busy_in_background`.
        id = "t-chat"
        var chat = t(id, "What's a good name for the cat?", "", "claude-code", "claude-opus-5-5", .idle, section: .inbox,
                     branch: DemoMac.oldMac ? "main" : nil, ago: 5)
        chat.project = ProjectRef(id: "", name: "No project", hue: 220, monogram: "··")
        threads.insert(chat, at: 4)
        seq[id] = 0
        items[id] = []
        add(id, .user(text: "Our new cat is grey with one white paw. Suggest a few names.", images: 0), at: now - 6 * m)
        add(id, .assistant(text: "A few that suit a grey cat with one white paw: **Sock**, **Pewter**, **Mitt**, **Smoke** and **Earl Grey**.", streaming: false))
        add(id, .turnEnd(tookSecs: 6))

        // An agent asking several things at once: taller than the screen, it scrolls.
        id = "t-quiz"
        threads.insert(t(id, "Set up the release pipeline", "p-api", "claude-code", "claude-opus-5-5", .needsYou,
                         needs: Needs(kind: .question, text: "Which CI should run the release?"), section: .inbox, unseen: true,
                         branch: "ci/release", ago: 1), at: 4)
        seq[id] = 0
        items[id] = []
        add(id, .user(text: "Set up a release pipeline for the API: build, test, tag and publish.", images: 0), at: now - 3 * m)
        add(id, .assistant(text: "A few choices before I write it.", streaming: false))
        add(id, .question(QuestionRequest(requestId: "r-quiz", questions: DemoMac.quiz, state: .pending, answers: nil)))

        // Its turn is over, but its agent's background work isn't: a sub-agent's report is on its
        // way to it. The Mac's sidebar keeps it at work ("Waiting").
        id = "t-waiting"
        threads.insert(t(id, "Audit the dependencies", "p-trek", "claude-code", "claude-opus-5-5", .idle, section: .inbox,
                         branch: "main", ago: 2), at: 4)
        seq[id] = 0
        items[id] = []
        add(id, .user(text: "Audit our dependencies for known advisories; use sub-agents for the big crates.", images: 0), at: now - 8 * m)
        add(id, .assistant(text: "I've started two sub-agents on the largest crates and will report when they're done.", streaming: false))
        add(id, .turnEnd(tookSecs: 40))

        // `-TrekBigThread 3000`: a thread as long as a real day's work, to measure with.
        let big = DemoMac.bigThreadSize
        if big > 0 {
            for (n, tid) in ["t-big", "t-big2"].enumerated() {
                id = tid
                threads.insert(t(id, n == 0 ? "Make the phone fast (\(big) items)" : "Another long day (\(big) items)", "p-trek",
                                 "claude-code", "claude-opus-5-5", .idle, section: .pinned, pinned: true, branch: "perf/phone",
                                 ago: 1 + Int64(n), add: 1240, del: 388), at: 0)
                seq[id] = 0
                items[id] = []
                let made = DemoMac.richThread ? DemoMac.richThread(big, endingAt: now) : DemoMac.bigThread(big, endingAt: now)
                for (body, at) in made { add(id, body, at: at) }
            }
        }

        // `-TrekFixture <path>`: a real thread's items, exported read-only as wire JSON (see
        // `ios/scripts/export-thread.py`), to scroll through as the phone would.
        if let path = DemoMac.fixturePath, let list = DemoMac.fixture(at: path), !list.isEmpty {
            id = "t-fixture"
            threads.insert(t(id, DemoMac.fixtureTitle ?? "Fixture (\(list.count) items)", "p-trek", "claude-code", "claude-opus-5-5",
                             .idle, section: .pinned, pinned: true, branch: "main", ago: 1), at: 0)
            items[id] = list
            seq[id] = list.map(\.seq).max() ?? 0
            counters[id] = list.count
        }
    }
}

// MARK: Sub-agents, the thread menu and search

extension MockHost {
    /// What this Mac serves besides the first phones' requests: everything.
    static let features = MacFeature.allCases.map(\.rawValue)

    /// Sub-agents with threads of their own (at work, waiting on an approval, done) and a snoozed
    /// thread.
    private func seedReach() {
        let now = Self.now
        let m: Int64 = 60_000
        func child(_ id: String, of parent: String, _ title: String, agent key: String, model: String, _ state: RunState,
                   needs: Needs? = nil, activity: String? = nil, ago: Int64) -> ThreadSummary? {
            guard let p = thread(parent), let a = agents.first(where: { $0.key == key }) else { return nil }
            var t = ThreadSummary(id: id, title: title, project: p.project, agent: a.ref, model: model,
                                  modelLabel: a.models.first { $0.id == model }?.label, runState: state, needs: needs,
                                  section: state == .working ? .working : .inbox, unseen: false, pinned: false, branch: p.branch,
                                  worktree: p.worktree, activity: activity, workingSince: state == .idle ? nil : now - ago * m,
                                  updatedAt: now - (state == .idle ? ago : 0) * m, additions: 0, deletions: 0)
            t.parentId = parent
            return t
        }
        let kids = [
            child("t-inbox-review", of: "t-inbox", "Review the inbox section rules", agent: "codex", model: "gpt-5.6-sol", .working,
                  activity: "Reading sidebar.rs", ago: 2),
            child("t-profile-bench", of: "t-profile", "Benchmark the scroll path", agent: "claude-code", model: "claude-sonnet-5-5",
                  .needsYou, needs: Needs(kind: .approval, text: "Run cargo bench -p trek-app --bench transcript_scroll"), ago: 4),
            child("t-tailscale-scout", of: "t-tailscale", "Check how pairing.rs picks its address", agent: "codex",
                  model: "gpt-6-astra", .idle, ago: 26),
        ].compactMap { $0 }
        threads += kids
        for k in kids { seq[k.id] = 0; items[k.id] = [] }
        // The parent whose sub-agent waits on the user says so, in the inbox.
        if let i = threads.firstIndex(where: { $0.id == "t-profile" }) {
            threads[i].needs = Needs(kind: .approval, text: "Sub-agent needs you")
            threads[i].needsThread = "t-profile-bench"
            threads[i].section = .inbox
        }
        var snoozed = ThreadSummary(id: "t-glass", title: "Try the Liquid Glass tab bar", project: projects[3].ref,
                                    agent: agents[0].ref, model: "claude-sonnet-5-5", modelLabel: "Sonnet 5.5", runState: .idle,
                                    needs: nil, section: .snoozed, unseen: false, pinned: false, branch: "main", worktree: false,
                                    activity: nil, workingSince: nil, updatedAt: now - 50 * m, additions: 12, deletions: 4)
        snoozed.snoozedUntil = Int64(SnoozePicker.suggested.timeIntervalSince1970 * 1000)
        threads.append(snoozed)
        seq[snoozed.id] = 0
        items[snoozed.id] = []

        func add(_ tid: String, _ body: ItemBody, at: Int64? = nil) {
            let n = nextSeq(tid)
            items[tid, default: []].append(TItem(id: newID(tid, body), seq: n, at: at, body: body))
        }
        func tool(_ kind: ToolKind, _ title: String, _ detail: String, _ status: ToolStatus = .done) -> ItemBody {
            .tool(ToolCall(callId: UUID().uuidString, tool: kind, title: title, detail: detail, status: status, output: "", added: nil, removed: nil))
        }
        add("t-inbox-review", .user(text: "Review how `own_section` orders snoozed, needs-you and settled threads. Advise only: list anything that looks wrong, with file and line.", images: 0), at: now - 2 * m)
        add("t-inbox-review", tool(.read, "Read", "crates/trek-core/src/store.rs"))
        add("t-inbox-review", tool(.search, "Search", "\"own_section(\" in crates/"))
        add("t-inbox-review", tool(.read, "Read", "crates/trek-app/src/sidebar.rs", .running))

        add("t-profile-bench", .user(text: "Benchmark scrolling a 3,000-item transcript before and after caching row heights. Report frames dropped per second.", images: 0), at: now - 4 * m)
        add("t-profile-bench", tool(.read, "Read", "crates/trek-app/benches/transcript_scroll.rs"))
        add("t-profile-bench", .assistant(text: "The bench exists already. I'll run it on the current build first, for the baseline.", streaming: false))
        add("t-profile-bench", .approval(ApprovalRequest(requestId: "r-bench", title: "Run command", detail: "cargo bench -p trek-app --bench transcript_scroll", state: .pending)))

        add("t-tailscale-scout", .user(text: "How does pairing.rs choose the address it puts in the QR code?", images: 0), at: now - 30 * m)
        add("t-tailscale-scout", tool(.read, "Read", "crates/trek-remote/src/pairing.rs"))
        add("t-tailscale-scout", .assistant(text: "`advertise_address()` takes the first private IPv4 address of an interface that's up. It never looks at Tailscale, so a phone on the tailnet gets the LAN address.", streaming: false))
        add("t-tailscale-scout", .turnEnd(tookSecs: 48))

        add("t-glass", .user(text: "Try the Liquid Glass tab bar on the main screen and tell me how it looks with the map behind it.", images: 0), at: now - 52 * m)
        add("t-glass", .assistant(text: "It reads well over the map in light mode. In dark mode the labels lose contrast over the satellite layer; a slightly stronger tint fixes it.", streaming: false))
        add("t-glass", .turnEnd(tookSecs: 95))
    }

    /// A sub-agent was answered or stopped: its parent's row says so.
    private func subAgentChanged(_ tid: String, to state: SubAgentState) {
        guard let parent = thread(tid)?.parentId else { return }
        update(parent) { p in
            if p.needsThread == tid {
                p.needs = nil
                p.needsThread = nil
                if p.section == .inbox, p.runState == .working { p.section = .working }
            }
            p.subAgents = p.subAgents?.map { s in
                var s = s
                if s.threadId == tid { s.state = state }
                return s
            }
        }
    }

    /// Snooze, wake, mark unread, regenerate the title and delete; false for the rest.
    private func moreThreadAction(_ tid: String, _ action: ThreadAction, id: String?) -> Bool {
        guard let t = thread(tid) else {
            refuse(id, .notFound, "No such thread")
            return true
        }
        switch action {
        case .snooze(let until, let preset):
            let calendar = Calendar.current
            let morning = { (days: Int) in
                calendar.date(bySettingHour: 9, minute: 0, second: 0, of: calendar.date(byAdding: .day, value: days, to: .now) ?? .now) ?? .now
            }
            let at: Date = switch preset {
            case .hour: .now.addingTimeInterval(3600)
            case .threeHours: .now.addingTimeInterval(3 * 3600)
            case .tomorrow: morning(1)
            case .nextWeek: morning(7)
            case nil: Date(timeIntervalSince1970: Double(until ?? 0) / 1000)
            }
            guard at > .now else {
                refuse(id, .badRequest, "That time has passed")
                return true
            }
            ack(id)
            update(tid) { $0.snoozedUntil = Int64(at.timeIntervalSince1970 * 1000); $0.section = .snoozed; $0.pinned = false }
        case .wake:
            ack(id)
            update(tid) { $0.snoozedUntil = nil; if $0.section == .snoozed { $0.section = .inbox } }
        case .markUnread:
            ack(id)
            update(tid) { $0.unseen = true }
        case .regenerateTitle:
            ack(id)
            later(1.2) { [weak self] in
                self?.update(tid) { $0.title = DemoMac.betterTitle(for: $0.id) ?? $0.title }
            }
        case .delete(let confirmed):
            let kids = threads.filter { $0.parentId == tid }
            guard confirmed else {
                var words = t.worktree
                    ? ["The thread and its transcript are deleted from Trek, and its worktree with them. This can't be undone."]
                    : ["The thread and its transcript are deleted from Trek. This can't be undone."]
                if !kids.isEmpty { words.append(kids.count == 1 ? "Its sub-agent is deleted with it." : "Its \(kids.count) sub-agents are deleted with it.") }
                if t.worktree {
                    words.append("2 uncommitted changes in the worktree would be lost.")
                    words.append("\(t.branch ?? "Its branch") keeps its 2 commits that \(t.base ?? "main") doesn't have.")
                }
                refuse(id, .conflict, words.joined(separator: " "))
                return true
            }
            ack(id, after: 0.3)
            for gone in [tid] + kids.map(\.id) {
                threads.removeAll { $0.id == gone }
                items[gone] = nil
                deliver(.threadRemoved(gone), after: 0.3)
            }
        default:
            return false
        }
        return true
    }

    /// Titles first, then messages, as the Mac's search finds them.
    private func search(_ query: String, limit: Int, id: String?) {
        let q = query.trimmingCharacters(in: .whitespaces).lowercased()
        var results: [SearchResult] = []
        for t in threads where t.title.lowercased().contains(q) {
            results.append(SearchResult(thread: t))
        }
        for t in threads where !results.contains(where: { $0.id == t.id }) {
            for item in items[t.id] ?? [] {
                let text: String
                switch item.body {
                case .user(let s, _, _), .assistant(let s, _): text = s
                default: continue
                }
                guard let hit = text.range(of: q, options: [.caseInsensitive, .diacriticInsensitive]) else { continue }
                // A few words either side, on one line.
                let from = text.index(hit.lowerBound, offsetBy: -min(40, text.distance(from: text.startIndex, to: hit.lowerBound)))
                let to = text.index(hit.upperBound, offsetBy: min(60, text.distance(from: hit.upperBound, to: text.endIndex)))
                let lead = from > text.startIndex ? "…" : ""
                let snippet = lead + text[from..<to].replacingOccurrences(of: "\n", with: " ") + (to < text.endIndex ? "…" : "")
                let start = lead.unicodeScalars.count + text[from..<hit.lowerBound].unicodeScalars.count
                results.append(SearchResult(thread: t, snippet: snippet, ranges: [[start, start + text[hit].unicodeScalars.count]]))
                break
            }
        }
        deliver(.searchResults(re: id, SearchResults(query: query, results: Array(results.prefix(limit)))), after: 0.35)
    }
}

// MARK: Links between threads, and a note with a picture

extension MockHost {
    /// A thread whose agent started two threads and then handed its work to a fresh one, those
    /// threads (each with its row back), and a note with a picture in it.
    /// A thread whose agent opened a pull request, then watched CI and cut a release: each a live
    /// card that moves on by itself and waits where the user has a button to press (Resolve
    /// conflicts, Merge, Re-run). `-TrekDemoCard <state>` holds one card in one state instead.
    private func seedCards() {
        let now = Self.now
        let m: Int64 = 60_000
        let id = "t-card"
        let a = agents.first { $0.key == "claude-code" }!
        let row = ThreadSummary(id: id, title: "Ship the session refresh fix", project: projects.first { $0.id == "p-trek" }?.ref,
                                agent: a.ref, model: "claude-opus-5-5", modelLabel: a.models.first { $0.id == "claude-opus-5-5" }?.label,
                                runState: .idle, needs: nil, section: .inbox, unseen: false, pinned: false, branch: "feat/session-refresh",
                                worktree: true, activity: nil, workingSince: nil, updatedAt: now, additions: 128, deletions: 36)
        threads.insert(row, at: min(1, threads.count))
        // As a real Mac numbers them (from when it first served the thread, in µs): a launch
        // holding another state isn't taken for the copy the phone kept from the last one.
        seq[id] = now * 1000
        // And what the phone kept from an earlier launch goes whole (its cards may be others).
        rewritten[id] = now * 1000
        items[id] = []
        let ask = ItemBody.user(text: "Open a PR for the session refresh fix, watch its checks, merge it when it's green, then cut a nightly.",
                                images: 0)
        put(id, TItem(id: newID(id, ask), seq: 0, at: now - 6 * m, body: ask))
        append(id, .assistant(text: "Pushing the branch and opening the pull request.", streaming: false))
        if let held = DemoMac.heldCard {
            showCard(held, then: false)
        } else {
            showCard("pr-creating", then: true)
        }
    }

    /// The `t-card` thread's card of `state`'s family shows `state` (a new card when the family
    /// is new), and (`then`) moves on by itself when the demo says it does.
    private func showCard(_ state: String, then: Bool) {
        let tid = "t-card"
        let family = String(state.prefix { $0 != "-" })
        if cardItems[family] == nil {
            switch family {
            case "run": append(tid, .assistant(text: "Merged #128. CI is running on main.", streaming: false))
            case "release": append(tid, .assistant(text: "CI is green. Cutting a nightly.", streaming: false))
            default: break
            }
        }
        let body = ItemBody.card(DemoMac.card(state))
        let iid = cardItems[family] ?? newID(tid, body)
        cardItems[family] = iid
        cardStates[family] = state
        put(tid, TItem(id: iid, seq: 0, body: body))
        guard then else { return }
        var next = DemoMac.nextCard(after: state)
        if state == "run-running", cardRerun { next = ("run-passed", 4) }
        guard let (after, secs) = next else { return }
        later(secs) { [weak self] in
            // Moved on meanwhile (a button was pressed): that one goes on from there.
            guard let self, self.cardStates[family] == state else { return }
            self.showCard(after, then: true)
        }
    }

    /// A card's button pressed on the phone.
    func takeCardAction(_ tid: String, key: String, _ action: CardAction, id: String?) {
        let family = cardStates.first { DemoMac.card($0.value).key == key }?.key
        guard tid == "t-card", let family, let state = cardStates[family] else {
            return deliver(.error(re: id, code: .notFound, message: "That card isn't in the thread any more"))
        }
        guard let next = DemoMac.cardAfter(action, on: state) else {
            return deliver(.error(re: id, code: .conflict, message: "It can't do that right now"))
        }
        ack(id)
        if action == .rerun, family == "run" { cardRerun = true }
        later(0.4) { [weak self] in self?.showCard(next, then: DemoMac.heldCard == nil) }
    }

    private func seedLinks() {
        let now = Self.now
        let m: Int64 = 60_000
        let atlas = projects[2].ref
        func row(_ id: String, _ title: String, agent key: String, model: String, _ state: RunState, needs: Needs? = nil,
                 activity: String? = nil, ago: Int64) -> ThreadSummary {
            let a = agents.first { $0.key == key }!
            return ThreadSummary(id: id, title: title, project: atlas, agent: a.ref, model: model,
                                 modelLabel: a.models.first { $0.id == model }?.label, runState: state, needs: needs,
                                 section: needs != nil ? .inbox : state == .working ? .working : .inbox, unseen: false, pinned: false,
                                 branch: "develop", worktree: false, activity: activity,
                                 workingSince: state == .working ? now - ago * m : nil, updatedAt: now - ago * m, additions: 0, deletions: 0)
        }
        let lead = "t-split"
        var docs = row("t-split-docs", "Document the v2 settings schema", agent: "codex", model: "gpt-6-astra", .working,
                       activity: "Editing docs/settings.md", ago: 3)
        docs.startedBy = lead
        var tests = row("t-split-tests", "Migration tests for old settings files", agent: "claude-code", model: "claude-sonnet-5-5",
                        .needsYou, needs: Needs(kind: .approval, text: "Run pnpm test settings"), ago: 5)
        tests.startedBy = lead
        var next = row("t-split-next", "Finish the settings migration", agent: "claude-code", model: "claude-opus-5-5", .idle, ago: 1)
        next.continuedFrom = lead
        let first = row(lead, "Split up the settings migration", agent: "claude-code", model: "claude-opus-5-5", .idle, ago: 2)
        threads.insert(contentsOf: [first, next, docs, tests], at: 4)
        for t in [first, next, docs, tests] { seq[t.id] = 0; items[t.id] = [] }

        func add(_ tid: String, _ body: ItemBody, at: Int64? = nil) {
            let n = nextSeq(tid)
            items[tid, default: []].append(TItem(id: newID(tid, body), seq: n, at: at, body: body))
        }
        func link(_ kind: ThreadLinkKind, _ t: ThreadSummary, _ state: LinkedThreadState? = nil) -> ItemBody {
            .threadLink(ThreadLink(link: kind, threadId: t.id, title: t.title, agent: t.agent, state: state))
        }
        add(lead, .user(text: "The settings migration is too big for one go. Split it: docs and tests in threads of their own, and carry on with the code.", images: 0), at: now - 40 * m)
        add(lead, .assistant(text: "I'll start a thread for the schema docs and one for the migration tests, so they run alongside, then do the loader myself.", streaming: false))
        add(lead, link(.started, docs, .working))
        add(lead, link(.started, tests, .working))
        add(lead, .assistant(text: "Both are running. The loader now reads v1 files and writes v2; my context is nearly full, so I'm handing the rest to a fresh thread with a brief.", streaming: false))
        add(lead, link(.continuedIn, next, .working))
        add(lead, .turnEnd(tookSecs: 214))

        add(next.id, link(.continuedFrom, first))
        add(next.id, .user(text: "You're taking over from another thread. Goal: migrate settings to v2. Done: the loader reads v1 and writes v2. Next: drop the v1 writer, then run the suite.", images: 0), at: now - 6 * m)
        add(next.id, .assistant(text: "The v1 writer is gone and the suite passes. The docs and tests threads are still yours to check.", streaming: false))
        add(next.id, .turnEnd(tookSecs: 260))

        for kid in [docs, tests] {
            add(kid.id, link(.startedBy, first))
            add(kid.id, .user(text: kid.id == docs.id ? "Write docs/settings.md for the v2 schema: every key, its type and default." : "Add tests that load every v1 settings file in fixtures/ and check the v2 result.", images: 0), at: now - 30 * m)
        }
        add(docs.id, .tool(ToolCall(callId: "c-docs", tool: .edit, title: "Edit", detail: "docs/settings.md", status: .running, output: "", added: nil, removed: nil)))
        add(tests.id, .approval(ApprovalRequest(requestId: "r-split", title: "Run command", detail: "pnpm test settings", state: .pending)))

        notes.append(Note(id: "n-sketch", title: "Sync flow", body: "# Sync flow\n\nFrom the whiteboard after standup:\n\n![Whiteboard](\(DemoMac.whiteboard))\n\n- [ ] Queue sends while offline\n- [x] Retry with backoff", modified: now - 12 * m))
    }
}
