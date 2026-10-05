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
    private var timer: Timer?
    private var tick = 0
    private let host = HostInfo(id: "demo-mac", name: "Tobias’s MacBook Pro", version: "0.3.2")

    let projects: [ProjectSummary] = [
        MockHost.project("p-trek", "trek", hue: 24, branch: "main"),
        MockHost.project("p-api", "trek-api", hue: 212, branch: "main"),
        MockHost.project("p-atlas", "atlas-web", hue: 135, branch: "develop"),
        MockHost.project("p-field", "fieldnotes-ios", hue: 275, branch: "main"),
    ]

    let agents: [AgentOption] = [
        AgentOption(key: "claude-code", name: "Claude Code", defaultModel: "claude-opus-5-5", models: [
            ModelOption(id: "claude-opus-5-5", label: "Opus 5.5"), ModelOption(id: "claude-sonnet-5-5", label: "Sonnet 5.5"),
            ModelOption(id: "claude-haiku-4-5", label: "Haiku 4.5")]),
        AgentOption(key: "codex", name: "Codex", defaultModel: "gpt-6-astra", models: [
            ModelOption(id: "gpt-6-astra", label: "GPT-6 Astra"), ModelOption(id: "gpt-5.6-sol", label: "GPT-5.6 Sol"),
            ModelOption(id: "gpt-5.6-luna", label: "GPT-5.6 Luna")]),
        AgentOption(key: "opencode", name: "OpenCode", defaultModel: "kimi-k3", models: [
            ModelOption(id: "kimi-k3", label: "Kimi K3"), ModelOption(id: "glm-5", label: "GLM 5")]),
        AgentOption(key: "acp:cursor", name: "Cursor", defaultModel: "composer-2", models: [
            ModelOption(id: "composer-2", label: "Composer 2")]),
        AgentOption(key: "acp:gemini", name: "Gemini CLI", defaultModel: "gemini-3-pro", models: [
            ModelOption(id: "gemini-3-pro", label: "Gemini 3 Pro")]),
    ]

    static func project(_ id: String, _ name: String, hue: Int, branch: String) -> ProjectSummary {
        ProjectSummary(id: id, name: name, hue: hue, monogram: monogram(name), branch: branch, isRepo: true)
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
        deliver(.welcome(re: "auth", host: host), after: 0.15)
        deliver(.snapshot(Snapshot(threads: threads, projects: projects, agents: agents)), after: 0.2)
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.15) { [weak self] in self?.onState?(.connected) }
        timer = Timer.scheduledTimer(withTimeInterval: 3.5, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.advanceWorkingThread() }
        }
    }

    func stop() {
        timer?.invalidate()
        timer = nil
    }

    func send(_ message: ClientMessage, id: String?) {
        switch message {
        case .pair, .hello:
            break
        case .subscribe(let tid, _):
            subscribed.insert(tid)
            deliver(.transcript(re: id, threadId: tid, reset: true, seq: seq[tid] ?? 0, items: items[tid] ?? []), after: 0.05)
        case .unsubscribe(let tid):
            subscribed.remove(tid)
        case .markSeen(let tid):
            update(tid, push: false) { $0.unseen = false }
        case .ping:
            deliver(.pong(re: id))
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
        case .send(let tid, let text, _):
            ack(id)
            append(tid, .user(text: text, images: 0))
            update(tid) { $0.runState = .working; $0.needs = nil; $0.workingSince = Self.now; $0.activity = "Thinking"; $0.section = $0.pinned ? .pinned : .working }
            respond(in: tid, to: text)
        case .newThread(let pid, let agentKey, let model, let text, let worktree):
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
            append(tid, .user(text: text, images: 0))
            respond(in: tid, to: text)
        case .answer(let tid, let rid, let response):
            answer(tid, rid, response, id: id)
        }
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
        if case .approval(let a) = item.body, allowed {
            let tool = TItem(id: "\(tid)-run-\(Self.nowMs)", seq: 0, body: .tool(ToolCall(callId: "c\(Self.nowMs)", tool: .command, title: "Run", detail: a.detail, status: .running, output: "", added: nil, removed: nil)))
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
        let id = "\(tid)-live-\(tick)"
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

    private func later(_ secs: Double, _ f: @escaping () -> Void) {
        DispatchQueue.main.asyncAfter(deadline: .now() + secs) { f() }
    }

    private func deliver(_ m: ServerMessage, after: Double = 0.03) {
        DispatchQueue.main.asyncAfter(deadline: .now() + after) { [weak self] in self?.onMessage?(m) }
    }

    private func ack(_ id: String?) { deliver(.ack(re: id, threadId: nil)) }

    private func nextSeq(_ tid: String) -> Int64 {
        let n = (seq[tid] ?? 0) + 1
        seq[tid] = n
        return n
    }

    private func append(_ tid: String, _ body: ItemBody) {
        put(tid, TItem(id: "\(tid)-\(UUID().uuidString.prefix(8))", seq: 0, body: body))
    }

    /// Adds or replaces an item (a fresh seq either way) and tells subscribers.
    private func put(_ tid: String, _ item: TItem, keepID: Bool = false) {
        var item = item
        item.seq = nextSeq(tid)
        if item.at == nil, case .user = item.body { item.at = Self.now }
        var list = items[tid] ?? []
        if let i = list.firstIndex(where: { $0.id == item.id }) { list[i] = item } else { list.append(item) }
        items[tid] = list
        if subscribed.contains(tid) { deliver(.item(threadId: tid, item: item)) }
        update(tid, push: false) { $0.updatedAt = Self.now }
    }

    private func update(_ tid: String, push: Bool = true, _ f: (inout ThreadSummary) -> Void) {
        guard let i = threads.firstIndex(where: { $0.id == tid }) else { return }
        f(&threads[i])
        if push { deliver(.thread(threads[i])) }
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
            t("t-onboarding", "Onboarding copy pass", "p-field", "acp:gemini", "gemini-3-pro", .idle, section: .settled,
              branch: "main", ago: 60 * 24 * 4, add: 40, del: 37),
        ]
        for th in threads { seq[th.id] = 0; items[th.id] = [] }

        func add(_ tid: String, _ body: ItemBody, at: Int64? = nil) {
            let n = nextSeq(tid)
            items[tid, default: []].append(TItem(id: "\(tid)-\(n)", seq: n, at: at, body: body))
        }
        func tool(_ kind: ToolKind, _ title: String, _ detail: String, _ status: ToolStatus = .done, out: String = "",
                  add: Int? = nil, del: Int? = nil) -> ItemBody {
            .tool(ToolCall(callId: UUID().uuidString, tool: kind, title: title, detail: detail, status: status, output: out, added: add, removed: del))
        }

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
        add(id, .user(text: "Show a banner when the Mac is unreachable and queue sends until it's back.", images: 0), at: now - 3 * m)
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

        for tid in ["t-tokio", "t-onboarding"] {
            add(tid, .user(text: threads.first { $0.id == tid }!.title, images: 0), at: now - 30 * 60 * m)
            add(tid, tool(.command, "Run", "cargo update -p tokio"))
            add(tid, .assistant(text: "Done, and the build is clean.", streaming: false))
            add(tid, .turnEnd(tookSecs: 64))
        }
    }
}
