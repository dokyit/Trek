import Foundation

/// The demo Mac answering what the Mac's composer and Settings gained (BarsProtocol.swift): a
/// failing pull request's bar on `t-tailscale`, "Continue on Work" on `t-limit`'s limit bar, the
/// Compact chip on `t-worktree`, a switch held for the next message (`-TrekDemoSwitch YES` puts one
/// on `t-release`), the permission rules (this demo Mac lets the phone write them), and files
/// marked viewed.
extension MockHost {
    /// The demo Mac's Settings › Permissions.
    static var rules: [Rule] = [
        Rule(kind: .command, pattern: "rm", action: .deny),
        Rule(kind: .command, pattern: "git push", action: .ask),
        Rule(kind: .edit, pattern: "~/.ssh/**", action: .deny),
        Rule(kind: .fetch, pattern: "docs.rs", action: .allow),
    ]
    /// Files ticked viewed, by where they were ticked.
    static var viewed: [GitTarget: Set<String>] = [:]

    /// What the rows carry for the bars, once the demo threads are there.
    func seedBars() {
        let now = Int64(Date().timeIntervalSince1970 * 1000)
        let m: Int64 = 60_000
        update("t-tailscale", push: false) { t in
            t.checks = FailingChecks(url: "https://github.com/dokyit/trek/pull/212", number: 212, failing: ["test (ios)", "lint"], bar: true)
            t.needs = Needs(kind: .failed, text: "CI failed: test (ios) and 1 more")
        }
        update("t-limit", push: false) { t in t.pause?.otherAccount = OtherAccount(account: "acct-work", label: "Work") }
        update("t-worktree", push: false) { t in
            t.context = ContextUse(used: 128_400, window: 200_000, percent: 64)
            t.compactOffer = CompactOffer(used: 128_400, window: 200_000, lastActive: now - 180 * m, lapsesAt: now - 120 * m,
                                          why: "The prompt cache has expired: it lasts about an hour on a subscription.\nYour next message re-reads all 128k tokens, and they count against your limits.\nCompact first to send less.")
        }
        // A file of `t-tailscale`'s turn read already, as the Mac's Git panel would have ticked it.
        if let first = DemoMac.gitStatus(thread: thread("t-tailscale")).files.first {
            Self.viewed[.thread("t-tailscale")] = [first.path]
        }
        if UserDefaults.standard.bool(forKey: "TrekDemoSwitch") {
            update("t-release", push: false) { t in
                t.pendingSwitch = PendingSwitch(agent: AgentRef(key: "claude-code", name: "Claude Code"), model: "claude-opus-5-5",
                                                modelLabel: "Claude Opus 5.5", effort: "high", to: "Claude Code", from: t.agent.name)
            }
        }
    }

    /// The bars' thread actions; whether it was one.
    func barThreadAction(_ tid: String, _ action: ThreadAction, id: String?) -> Bool {
        switch action {
        case .sendFailedChecks:
            guard let checks = thread(tid)?.checks else { refuse(bar: id, .conflict, "Nothing is failing now"); return true }
            ack(id)
            update(tid) { $0.checks?.sending = true }
            later(1.2) { [weak self] in
                guard let self else { return }
                self.update(tid) { t in t.checks = nil; if t.runState != .failed { t.needs = nil } }
                let names = checks.failing.map { "**\($0)**" }.joined(separator: ", ")
                _ = self.send(.send(threadId: tid, text: "CI failed on #\(checks.number ?? 0): \(names). Fix it, and push again.", mode: .queue, images: []), id: nil)
            }
        case .dismissFailedChecks:
            guard thread(tid)?.checks != nil else { refuse(bar: id, .conflict, "Nothing is failing now"); return true }
            ack(id)
            update(tid) { t in t.checks = nil; if t.runState != .failed { t.needs = nil } }
        case .continueOnAccount(let account):
            guard let t = thread(tid), let other = t.pause?.otherAccount else {
                refuse(bar: id, .conflict, "No other account of its agent has room now")
                return true
            }
            guard other.account == account else { refuse(bar: id, .conflict, "That account has no room now. \(other.label) has."); return true }
            let waiting = t.queued?.first?.text ?? "Carry on."
            ack(id)
            update(tid) { $0.pause = nil; $0.queued = nil; $0.needs = nil }
            _ = send(.send(threadId: tid, text: waiting, mode: nil, images: []), id: nil)
        case .stayOnAgent:
            guard thread(tid)?.pendingSwitch != nil else { refuse(bar: id, .conflict, "No switch waits for its next message"); return true }
            ack(id)
            update(tid) { $0.pendingSwitch = nil }
        default:
            return false
        }
        return true
    }

    func takeBars(_ message: ClientMessage, id: String?) {
        switch message {
        case .holdSwitch(let tid, let key, let model):
            guard let a = agents.first(where: { $0.key == key }), thread(tid) != nil else {
                return refuse(bar: id, .notFound, "That agent isn't set up on this Mac")
            }
            let modelID = model ?? a.defaultModel
            ack(id)
            update(tid) { t in
                t.pendingSwitch = PendingSwitch(agent: a.ref, model: modelID, modelLabel: a.models.first { $0.id == modelID }?.label,
                                                effort: t.effort, to: a.name, from: t.agent.name)
            }
        case .switchPreview(let tid):
            guard let t = thread(tid), let p = t.pendingSwitch else { return refuse(bar: id, .notFound, "No switch waits for its next message") }
            let text = """
            You're picking up a conversation from \(p.from). The user moved it to you: carry on from here.

            What they asked: “\(t.title)”. \(p.from) read the code around it and made the first changes; \(t.additions) lines added and \(t.deletions) removed so far, not committed yet.

            Where it was: the last turn ended with the change in place and its tests passing. Nothing waits on an answer.

            The full transcript is on the Mac if you need what was left out: search it rather than guess.
            """
            deliver(.switchPreview(re: id, SwitchPreview(threadId: tid, text: text, tokens: 3_100, budget: 24_000,
                                                          leftOut: "Left out: 6 tool outputs and 2 long answers.")), after: 0.2)
        case .rules:
            deliver(.rules(re: id, RulesList(rules: Self.rules, editable: true)))
        case .ruleAction(let change):
            switch change {
            case .add(let rule):
                if !Self.rules.contains(rule) { Self.rules.append(rule) }
            case .remove(let index, let rule):
                guard Self.rules.indices.contains(index), Self.rules[index] == rule else {
                    return refuse(bar: id, .conflict, "Your rules have changed since. Look at them again.")
                }
                Self.rules.remove(at: index)
            }
            deliver(.rules(re: id, RulesList(rules: Self.rules, editable: true)))
        case .ruleTest(let kind, let text):
            deliver(.ruleTest(re: id, Self.test(kind, text)))
        case .alwaysAllow(let tid, let rid, let pattern):
            let rule = Rule(kind: .command, pattern: pattern, action: .allow)
            if !Self.rules.contains(rule) { Self.rules.append(rule) }
            ack(id)
            _ = send(.answer(threadId: tid, requestId: rid, response: .approval(.allow)), id: nil)
        case .setViewed(let target, let path, let viewed):
            var marked = Self.viewed[target] ?? []
            if viewed { marked.insert(path) } else { marked.remove(path) }
            Self.viewed[target] = marked
            let t: ThreadSummary? = if case .thread(let tid) = target { thread(tid) } else { nil }
            deliver(.gitStatus(re: id, viewedStatus(DemoMac.gitStatus(thread: t), target)), after: 0.15)
        default:
            break
        }
    }

    /// `status` with the files ticked viewed there.
    func viewedStatus(_ status: GitStatus, _ target: GitTarget) -> GitStatus {
        var s = status
        let marked = Self.viewed[target] ?? []
        s.viewed = s.files.map(\.path).filter(marked.contains)
        return s
    }

    /// What the demo's rules say about a call: the first rule of its kind it falls under (deny,
    /// then ask, then allow), as the Mac's tester words it.
    static func test(_ kind: RuleKind, _ text: String) -> RuleTest {
        let parts = kind == .command
            ? text.components(separatedBy: CharacterSet(charactersIn: ";|&")).map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
            : [text]
        let order: [RuleVerdict] = [.deny, .ask, .allow]
        var found: (Int, Rule, String)?
        for verdict in order where found == nil {
            for (i, r) in rules.enumerated() where r.kind == kind && r.action == verdict {
                if let part = parts.first(where: { $0 == r.pattern || $0.hasPrefix(r.pattern + " ") || $0.contains(r.pattern) }) {
                    found = (i, r, part)
                    break
                }
            }
        }
        let levels: [Access] = [.supervised, .autoAcceptEdits, .auto, .fullAccess]
        guard let (i, rule, part) = found else {
            let asks: (Access) -> Bool = { level in kind == .command ? level == .supervised || level == .autoAcceptEdits : level == .supervised }
            return RuleTest(kind: kind, text: text, levels: levels.map { LevelOutcome(level: $0, outcome: asks($0) ? .asks : .runs) })
        }
        let outcome: RuleOutcome = switch rule.action {
        case .deny: .refused
        case .ask: .asks
        case .allow: .runs
        }
        let why = part == text ? "\(rule.shown) decides." : "\(rule.shown) decides, because this line runs “\(part)”."
        return RuleTest(kind: kind, text: text, decidedBy: why, rule: i, levels: levels.map { LevelOutcome(level: $0, outcome: outcome) })
    }

    private func refuse(bar id: String?, _ code: ErrorCode, _ message: String) {
        deliver(.error(re: id, code: code, message: message))
    }
}
