import Foundation

/// The demo Mac's sub-agents as rows in their parents' transcripts (feature `sub_agent_view`):
/// one of Trek's own, which opens its thread, and the agents' own (Claude Code's Task, Codex's
/// agents), whose steps `sub_agent` reads a few more of every time it's asked while they work.
/// Claude Code's stop but take no message; Codex's take both; a finished one shows its answer.
enum DemoSubAgents {
    /// One of an agent's own: what it's told, its steps (`at` seconds after the first ask), its
    /// answer once done.
    struct Own {
        var thread: String
        var call: String
        var link: SubAgentLink
        var prompt: String
        var steps: [Step]
        var answer: String?
        var stops: Bool
        var messages: Bool
    }

    struct Step {
        var at: Double
        var body: ItemBody
    }

    /// What happened to one since the demo started: when it was first asked about, the messages
    /// it was sent (and its replies), and whether it was stopped.
    struct Run {
        var started: Date
        var extra: [Step] = []
        var stoppedAt: Double?
    }

    static var runs: [String: Run] = [:]

    static func tool(_ kind: ToolKind, _ title: String, _ detail: String, _ status: ToolStatus = .done, out: String = "") -> ItemBody {
        .tool(ToolCall(callId: UUID().uuidString, tool: kind, title: title, detail: detail, status: status, output: out,
                       added: nil, removed: nil))
    }

    static let codexNoMessage: String? = nil
    static let claudeNoMessage = "Claude Code doesn't take messages for its sub-agents while they run. Write to the thread, and its agent can pass it on."

    static let own: [Own] = {
        let now = MockHost.now
        return [
            Own(thread: "t-inbox", call: "toolu-scout",
                link: SubAgentLink(agent: DemoMac.claude, label: "Find every place that reads snoozed_until",
                                   detail: "Searching for snoozed_until · 4 steps", state: .running, since: now - 40_000),
                prompt: "Find every place in crates/ that reads `snoozed_until` or decides a thread's section. List each with its file and line, and say which of them would still hide a snoozed thread that needs the user.",
                steps: [
                    Step(at: 0, body: .assistant(text: "I'll search for the field first, then for the functions that pick a section.", streaming: false)),
                    Step(at: 0, body: tool(.search, "Search", "\"snoozed_until\" in crates/", out: "crates/trek-core/src/store.rs:412\ncrates/trek-core/src/store.rs:977\ncrates/trek-app/src/sidebar.rs:1208\ncrates/trek-app/src/remote.rs:2290")),
                    Step(at: 0, body: tool(.read, "Read", "crates/trek-core/src/store.rs", out: "pub fn own_section(&self, now: i64) -> Option<Section> {\n    if self.snoozed_until.is_some_and(|t| t > now) {\n        return Some(Section::Snoozed);\n    }\n    …")),
                    Step(at: 3, body: tool(.read, "Read", "crates/trek-app/src/sidebar.rs")),
                    Step(at: 6, body: .assistant(text: "`own_section` checks the snooze before `needs_you()`: that's the one that hides it. The sidebar and the phone server both call it, so fixing it there covers both.", streaming: false)),
                    Step(at: 9, body: tool(.search, "Search", "\"own_section(\" in crates/", out: "11 matches in 6 files")),
                    Step(at: 13, body: tool(.read, "Read", "crates/trek-app/src/remote/basecamp.rs", .running)),
                ],
                answer: nil, stops: true, messages: false),
            Own(thread: "t-inbox", call: "toolu-copy",
                link: SubAgentLink(agent: DemoMac.claude, label: "Check the sidebar's empty state copy",
                                   detail: "Done · 6 steps", state: .done, tookSecs: 48),
                prompt: "Read the sidebar's empty states and say whether each still reads right once snoozed threads can come back by themselves.",
                steps: [
                    Step(at: 0, body: tool(.search, "Search", "\"Nothing here\" in crates/trek-app/src/")),
                    Step(at: 0, body: tool(.read, "Read", "crates/trek-app/src/sidebar.rs")),
                    Step(at: 0, body: tool(.read, "Read", "crates/trek-app/src/basecamp.rs")),
                    Step(at: 0, body: .assistant(text: "Two of the three empty states still read right.", streaming: false)),
                ],
                answer: "Two of the three empty states read right as they are. **Snoozed** says “Nothing snoozed”, which stays true. The **Inbox**'s “You're all caught up” should become “You're all caught up. Snoozed threads come back here when they need you.”",
                stops: false, messages: false),
            Own(thread: "t-profile", call: "agent-layout",
                link: SubAgentLink(agent: DemoMac.codex, label: "layout timer",
                                   detail: "Running cargo bench -p trek-app · 3 steps", state: .running, since: now - 70_000),
                prompt: "Time how long laying out one transcript row takes, by kind, on the 3,000-item fixture. Change nothing; report the slowest kinds with their median and p95.",
                steps: [
                    Step(at: 0, body: tool(.read, "Read", "crates/trek-app/benches/transcript_scroll.rs")),
                    Step(at: 0, body: tool(.command, "Run command", "cargo bench -p trek-app --bench transcript_scroll -- --list", out: "transcript_scroll/layout_row\ntranscript_scroll/scroll_page")),
                    Step(at: 4, body: tool(.command, "Run command", "cargo bench -p trek-app --bench transcript_scroll layout_row", .running)),
                ],
                answer: nil, stops: true, messages: true),
            Own(thread: "t-ratelimit", call: "agent-counters",
                link: SubAgentLink(agent: DemoMac.codex, label: "counter scout",
                                   detail: "Done · 3 steps", state: .done, tookSecs: 31),
                prompt: "Find where the service keeps counters or caches today, so the rate limiter can live with them.",
                steps: [
                    Step(at: 0, body: tool(.search, "Search", "\"redis\" in src/", out: "src/cache.rs:3\nsrc/state.rs:18")),
                    Step(at: 0, body: tool(.read, "Read", "src/cache.rs")),
                    Step(at: 0, body: .assistant(text: "There's a Redis pool in `AppState` already, used for session caching.", streaming: false)),
                ],
                answer: "Redis is already in: `AppState::cache` holds a pool (src/state.rs:18) used for sessions. A counter keyed by IP and by account with a 60 s expiry fits next to it, with no new dependency.",
                stops: false, messages: false),
        ]
    }()

    /// The rows for `tid`'s transcript: Trek's own (a thread of its own) and the agent's own.
    static func rows(in tid: String) -> [ItemBody] {
        var out: [ItemBody] = []
        if tid == "t-inbox" {
            out.append(.tool(ToolCall(callId: "trek-task:t-inbox-review", tool: .other, title: "Sub-agent",
                                      detail: "Review the inbox section rules", status: .running, output: "", added: nil, removed: nil,
                                      subAgent: SubAgentLink(agent: DemoMac.codex, label: "GPT-5.6 Sol: Review the inbox section rules",
                                                             detail: "Running · Read crates/trek-app/src/sidebar.rs", state: .running,
                                                             threadId: "t-inbox-review"))))
        }
        for o in own where o.thread == tid {
            out.append(.tool(ToolCall(callId: o.call, tool: .agent, title: "Agent", detail: o.link.label,
                                      status: o.link.live ? .running : .done, output: o.answer ?? "", added: nil, removed: nil,
                                      subAgent: o.link)))
        }
        return out
    }

    /// What `call` of `tid` has done by now, as a Mac's `sub_agent` says it.
    static func log(_ tid: String, _ call: String, now: Date = Date()) -> SubAgentLog? {
        guard let o = own.first(where: { $0.thread == tid && $0.call == call }) else { return nil }
        let key = "\(tid)/\(call)"
        let run = runs[key] ?? Run(started: now)
        runs[key] = run
        var link = o.link
        let stopped = run.stoppedAt != nil
        let elapsed = run.stoppedAt ?? now.timeIntervalSince(run.started)
        var shown = (o.steps + run.extra).filter { !link.live || $0.at <= elapsed }.sorted { $0.at < $1.at }
        if stopped {
            link.state = .stopped
            link.detail = "Stopped"
            link.tookSecs = link.since.map { Int((now.timeIntervalSince1970 * 1000 - Double($0)) / 1000) }
            // What was running ended with it.
            shown = shown.map { s in
                guard case .tool(var c) = s.body, c.status == .running else { return s }
                c.status = .failed
                return Step(at: s.at, body: .tool(c))
            }
        } else if link.live {
            let calls = shown.count { if case .tool = $0.body { true } else { false } }
            link.detail = "\(shown.last.flatMap(Self.words) ?? "Working") · \(calls) step\(calls == 1 ? "" : "s")"
        }
        let items = shown.enumerated().map { n, s in TItem(id: "s\(n)", seq: Int64(n), body: s.body) }
        return SubAgentLog(threadId: tid, callId: call, link: link, prompt: o.prompt, items: items,
                           answer: link.live ? nil : (stopped ? "Stopped by the user." : o.answer),
                           canStop: link.live && o.stops, canMessage: link.live && o.messages,
                           noMessage: o.messages ? nil : "\(link.agent.name) doesn't take messages for its sub-agents while they run. Write to the thread, and its agent can pass it on.",
                           asking: false, gone: false)
    }

    /// A step in the words the row's detail line uses.
    private static func words(_ s: Step) -> String? {
        guard case .tool(let c) = s.body else { return nil }
        return c.title == "Run command" ? "Running \(c.detail.split(separator: " ").prefix(3).joined(separator: " "))" : "\(c.title) \((c.detail as NSString).lastPathComponent)"
    }

    /// `sub_agent_action`: where it stands after, or the Mac's refusal.
    static func act(_ tid: String, _ call: String, _ action: SubAgentAction, text: String?, now: Date = Date()) -> Result<SubAgentLog, SubAgentFailure> {
        guard let o = own.first(where: { $0.thread == tid && $0.call == call }), let before = log(tid, call, now: now) else {
            return .failure(SubAgentFailure(message: "No such sub-agent"))
        }
        guard before.link.live else { return .failure(SubAgentFailure(message: "It has already finished.")) }
        let key = "\(tid)/\(call)"
        let elapsed = now.timeIntervalSince(runs[key]?.started ?? now)
        switch action {
        case .stop:
            guard o.stops else { return .failure(SubAgentFailure(message: "\(o.link.agent.name) doesn't let Trek stop its sub-agents.")) }
            runs[key]?.stoppedAt = elapsed
        case .message:
            guard o.messages else { return .failure(SubAgentFailure(message: before.noMessage ?? "It takes no messages.")) }
            let said = (text ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            guard !said.isEmpty else { return .failure(SubAgentFailure(message: "Write something to send.")) }
            runs[key]?.extra.append(Step(at: elapsed, body: .user(text: said, images: 0)))
            runs[key]?.extra.append(Step(at: elapsed + 1.5, body: .assistant(text: "Got it: I'll take that into account and say so in my report.", streaming: false)))
        }
        return log(tid, call, now: now).map { .success($0) } ?? .failure(SubAgentFailure(message: "No such sub-agent"))
    }
}

extension MockHost {
    /// `sub_agent` and `sub_agent_action`, as a Mac answers them.
    func takeSubAgent(_ message: ClientMessage, id: String?) {
        switch message {
        case .subAgent(let tid, let call):
            if let log = DemoSubAgents.log(tid, call) {
                deliver(.subAgent(re: id, log), after: 0.12)
            } else {
                deliver(.error(re: id, code: .notFound, message: "No such sub-agent"), after: 0.12)
            }
        case .subAgentAction(let tid, let call, let action, let text):
            switch DemoSubAgents.act(tid, call, action, text: text) {
            case .success(let log):
                deliver(.subAgent(re: id, log), after: 0.25)
                // Its row and its parent's at-work card say so too.
                if action == .stop { subAgentStopped(tid, call: call, log.link) }
            case .failure(let f):
                deliver(.error(re: id, code: .badRequest, message: f.message), after: 0.2)
            }
        default:
            break
        }
    }
}
