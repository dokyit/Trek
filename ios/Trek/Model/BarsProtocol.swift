import Foundation

// What the Mac's composer and Settings gained, as the wire carries it (see docs/MOBILE.md,
// `crates/trek-remote/src/protocol/{composer_bars,rules,viewed}.rs`): the failing-checks bar,
// "Continue on <account>", the Compact chip, a switch waiting for the next message, the
// permission rules and files marked viewed. Each comes from a Mac that lists its feature; an older
// one sends none of it, and the phone shows none of it.

// MARK: The failing-checks bar (`failed_checks`)

/// A pull request the thread opened has failing checks nobody has dealt with.
nonisolated struct FailingChecks: Codable, Hashable {
    var url: String
    var number: Int? = nil
    var failing: [String] = []
    /// The Mac's bar is up: the thread isn't at work or asking.
    var bar = false
    /// Sending by itself used up its tries.
    var stopped = false
    /// Its log is being read to send.
    var sending = false
    /// The project sends failing checks by itself.
    var auto = false

    init(url: String, number: Int? = nil, failing: [String] = [], bar: Bool = false, stopped: Bool = false,
         sending: Bool = false, auto: Bool = false) {
        (self.url, self.number, self.failing, self.bar, self.stopped, self.sending, self.auto) =
            (url, number, failing, bar, stopped, sending, auto)
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        url = try c.decodeIfPresent(String.self, forKey: .url) ?? ""
        number = try c.decodeIfPresent(Int.self, forKey: .number)
        failing = try c.decodeIfPresent([String].self, forKey: .failing) ?? []
        bar = try c.decodeIfPresent(Bool.self, forKey: .bar) ?? false
        stopped = try c.decodeIfPresent(Bool.self, forKey: .stopped) ?? false
        sending = try c.decodeIfPresent(Bool.self, forKey: .sending) ?? false
        auto = try c.decodeIfPresent(Bool.self, forKey: .auto) ?? false
    }

    /// Trek's tries before it stops sending by itself (`pr_checks::AUTO_TRIES`).
    static let autoTries = 2

    /// The bar's title, as the Mac's.
    var title: String { stopped ? "CI still failing" : "CI failed" }

    /// Beside the title, as the Mac's bar says it: "#128: clippy, test (macos)".
    var detail: String {
        let pr = number.map { "#\($0): " } ?? ""
        let names = failing.joined(separator: ", ")
        return stopped ? "\(pr)\(names). Trek stopped sending it after \(Self.autoTries) tries." : "\(pr)\(names)"
    }
}

/// `set_settings.send_failed_checks`: a project's sending by itself, on or off.
nonisolated struct ProjectSwitch: Hashable {
    var projectId: String
    var on: Bool
    var json: [String: Any] { ["project_id": projectId, "on": on] }
}

// MARK: "Continue on <account>" (`continue_on_account`)

/// Another account of the paused thread's agent with room left.
nonisolated struct OtherAccount: Codable, Hashable {
    /// Its id; nil: the user's normal login.
    var account: String? = nil
    var label: String
}

// MARK: The Compact chip (`compact_offer`)

/// A large idle thread compacting would help once its prompt cache has gone.
nonisolated struct CompactOffer: Codable, Hashable {
    var used: Int64
    var window: Int64
    /// When the thread last did anything (ms): a dismissal holds until it's active again.
    var lastActive: Int64
    /// When its prompt cache goes (ms): the chip shows from then on.
    var lapsesAt: Int64
    /// Why, in the Mac's words.
    var why: String = ""

    init(used: Int64, window: Int64, lastActive: Int64, lapsesAt: Int64, why: String = "") {
        (self.used, self.window, self.lastActive, self.lapsesAt, self.why) = (used, window, lastActive, lapsesAt, why)
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        used = try c.decodeIfPresent(Int64.self, forKey: .used) ?? 0
        window = try c.decodeIfPresent(Int64.self, forKey: .window) ?? 0
        lastActive = try c.decodeIfPresent(Int64.self, forKey: .lastActive) ?? 0
        lapsesAt = try c.decodeIfPresent(Int64.self, forKey: .lapsesAt) ?? 0
        why = try c.decodeIfPresent(String.self, forKey: .why) ?? ""
    }

    /// The cache has gone by `now`.
    func lapsed(at now: Date) -> Bool { Int64(now.timeIntervalSince1970 * 1000) >= lapsesAt }

    /// The chip is up at `now`: the cache has gone, and it wasn't dismissed since the thread was
    /// last active (`dismissed`: the `lastActive` it was dismissed at).
    func shows(at now: Date, dismissed: Int64?) -> Bool { lapsed(at: now) && dismissed != lastActive }
}

// MARK: A switch waiting for the next message (`switch_preview`)

/// The move to another agent the thread's next message makes, as the Mac's bar says it.
nonisolated struct PendingSwitch: Codable, Hashable {
    var agent: AgentRef
    var model: String? = nil
    var modelLabel: String? = nil
    var effort: String? = nil
    /// Where it goes: "Codex", "Codex · Work", "the Work account".
    var to: String
    /// What it's on now, for "Stay on …".
    var from: String

    /// What the bar says, by how much room it has and whether a turn would be stopped (the
    /// Mac's `switch_bar::sentence`).
    static func sentence(to: String, running: Bool, compact: Bool) -> (lead: String, rest: String?) {
        switch (compact, running) {
        case (true, false): ("Next message switches to \(to)", nil)
        case (true, true): ("Next message stops this and switches to \(to)", nil)
        case (false, false): ("Your next message switches to \(to).", "The conversation so far goes with it.")
        case (false, true): ("Your next message switches to \(to).", "It stops this turn, and the conversation so far goes with it.")
        }
    }
}

/// The reply to `switch_preview`: what the next message carries to the new agent.
nonisolated struct SwitchPreview: Decodable, Hashable {
    var threadId: String? = nil
    var text: String = ""
    var tokens: Int64 = 0
    var budget: Int64 = 0
    var leftOut: String? = nil

    private enum CodingKeys: String, CodingKey { case threadId, text, tokens, budget, leftOut }

    init(threadId: String? = nil, text: String, tokens: Int64, budget: Int64, leftOut: String? = nil) {
        (self.threadId, self.text, self.tokens, self.budget, self.leftOut) = (threadId, text, tokens, budget, leftOut)
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        threadId = try c.decodeIfPresent(String.self, forKey: .threadId)
        text = try c.decodeIfPresent(String.self, forKey: .text) ?? ""
        tokens = try c.decodeIfPresent(Int64.self, forKey: .tokens) ?? 0
        budget = try c.decodeIfPresent(Int64.self, forKey: .budget) ?? 0
        leftOut = try c.decodeIfPresent(String.self, forKey: .leftOut)
    }
}

// MARK: Permission rules (`rules`)

/// What a rule looks at.
nonisolated enum RuleKind: String, LenientEnum, CaseIterable, Identifiable {
    case command, edit, fetch, tool
    static var fallback: RuleKind { .tool }
    var id: String { rawValue }
    /// As the Mac's Settings › Permissions names it.
    var label: String {
        switch self {
        case .command: "Command"
        case .edit: "File edit"
        case .fetch: "Web fetch"
        case .tool: "Tool"
        }
    }
    var placeholder: String {
        switch self {
        case .command: "A command, like rm or git push"
        case .edit: "A file or folder, like ~/.ssh/** or *.lock"
        case .fetch: "An address, like example.com"
        case .tool: "A tool's name, like WebSearch or mcp__github__*"
        }
    }
    var testPlaceholder: String {
        switch self {
        case .command: "A command to try, like rm -rf build; echo done"
        case .edit: "A path to try, like src/main.rs"
        case .fetch: "An address to try, like https://example.com/x"
        case .tool: "A tool's name to try, like mcp__github__create_issue"
        }
    }
}

/// What a rule says.
nonisolated enum RuleVerdict: String, LenientEnum, CaseIterable, Identifiable {
    case deny, ask, allow
    static var fallback: RuleVerdict { .ask }
    var id: String { rawValue }
    var label: String { rawValue.prefix(1).uppercased() + rawValue.dropFirst() }
}

/// One of the user's rules.
nonisolated struct Rule: Codable, Hashable {
    var kind: RuleKind
    var pattern: String
    var action: RuleVerdict

    /// "Deny command “rm”", as the Mac shows a rule.
    var shown: String { "\(action.label) \(kind.label.lowercased()) “\(pattern)”" }
    var json: [String: Any] { ["kind": kind.rawValue, "pattern": pattern, "action": action.rawValue] }
}

/// The rule an approval's "Always allow…" would add.
nonisolated struct RuleDraft: Codable, Hashable {
    var kind: RuleKind
    var pattern: String
}

/// The reply to `rules` and `rule_action`.
nonisolated struct RulesList: Decodable, Hashable {
    var rules: [Rule] = []
    /// This phone may change them (the Mac's Settings › Phone).
    var editable = false

    private enum CodingKeys: String, CodingKey { case rules, editable }

    init(rules: [Rule] = [], editable: Bool = false) { (self.rules, self.editable) = (rules, editable) }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        rules = (try c.decodeIfPresent(Lossy<Rule>.self, forKey: .rules))?.elements ?? []
        editable = try c.decodeIfPresent(Bool.self, forKey: .editable) ?? false
    }
}

/// `rule_action`.
nonisolated enum RuleChange: Hashable {
    case add(Rule)
    /// `rule` as shown: if the list changed meanwhile, the Mac refuses.
    case remove(index: Int, Rule)
    var json: [String: Any] {
        switch self {
        case .add(let rule): ["action": "add", "rule": rule.json]
        case .remove(let index, let rule): ["action": "remove", "index": index, "rule": rule.json]
        }
    }
}

nonisolated enum RuleOutcome: String, LenientEnum {
    case refused, asks, runs, unknown
    static var fallback: RuleOutcome { .unknown }
    var label: String { self == .unknown ? "?" : rawValue.prefix(1).uppercased() + rawValue.dropFirst() }
}

nonisolated struct LevelOutcome: Decodable, Hashable {
    var level: Access
    var outcome: RuleOutcome
}

/// The reply to `rule_test`: which rule decides the call, and what happens at each level.
nonisolated struct RuleTest: Decodable, Hashable {
    var kind: RuleKind = .command
    var text: String = ""
    var decidedBy: String? = nil
    var rule: Int? = nil
    var levels: [LevelOutcome] = []

    private enum CodingKeys: String, CodingKey { case kind, text, decidedBy, rule, levels }

    init(kind: RuleKind, text: String, decidedBy: String? = nil, rule: Int? = nil, levels: [LevelOutcome] = []) {
        (self.kind, self.text, self.decidedBy, self.rule, self.levels) = (kind, text, decidedBy, rule, levels)
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        kind = try c.decodeIfPresent(RuleKind.self, forKey: .kind) ?? .command
        text = try c.decodeIfPresent(String.self, forKey: .text) ?? ""
        decidedBy = try c.decodeIfPresent(String.self, forKey: .decidedBy)
        rule = try c.decodeIfPresent(Int.self, forKey: .rule)
        levels = (try c.decodeIfPresent(Lossy<LevelOutcome>.self, forKey: .levels))?.elements ?? []
    }

    /// What it says when no rule decides.
    var said: String { decidedBy ?? "No rule decides this: the thread's level does." }
}

// MARK: Files marked viewed (`diff_viewed`)

extension GitStatus {
    /// `path` is marked viewed, and hasn't changed since.
    func isViewed(_ path: String) -> Bool { viewed?.contains(path) ?? false }

    /// "· 1 of 3 viewed", as the Mac's file list heads it; nil with none viewed.
    static func viewedLabel(_ viewed: Int, of total: Int) -> String? {
        viewed == 0 ? nil : "\(viewed) of \(total) viewed"
    }
}

// MARK: Phone → Mac

nonisolated extension ClientMessage {
    /// The new requests' JSON (`json(id:)` hands them here).
    var barsJSON: [String: Any] {
        switch self {
        case .holdSwitch(let threadId, let agent, let model):
            var o: [String: Any] = ["type": "set_prefs", "thread_id": threadId, "agent": agent, "hold": true]
            if let model { o["model"] = model }
            return o
        case .switchPreview(let threadId):
            return ["type": "switch_preview", "thread_id": threadId]
        case .rules:
            return ["type": "rules"]
        case .ruleAction(let change):
            return change.json.merging(["type": "rule_action"]) { $1 }
        case .ruleTest(let kind, let text):
            return ["type": "rule_test", "kind": kind.rawValue, "text": text]
        case .alwaysAllow(let threadId, let requestId, let pattern):
            return ["type": "always_allow", "thread_id": threadId, "request_id": requestId, "pattern": pattern]
        case .setViewed(let target, let path, let viewed):
            return target.json.merging(["type": "set_viewed", "path": path, "viewed": viewed]) { $1 }
        default:
            return [:]
        }
    }
}
