import Foundation

// Wire protocol v1 between the phone and Trek on the Mac. See docs/MOBILE.md: this file mirrors
// crates/trek-remote/src/protocol.rs. Keys are snake_case on the wire (the coders convert), times
// are unix milliseconds, and unknown enum values decode to `.unknown` so a newer Mac doesn't
// break an older phone.

nonisolated let trekProtocolVersion = 1

/// An enum decoded from a string that falls back to a catch-all for values it doesn't know.
nonisolated protocol LenientEnum: RawRepresentable, Codable, Hashable where RawValue == String {
    static var fallback: Self { get }
}

nonisolated extension LenientEnum {
    init(from decoder: Decoder) throws {
        let raw = try decoder.singleValueContainer().decode(String.self)
        self = Self(rawValue: raw) ?? Self.fallback
    }
}

nonisolated enum RunState: String, LenientEnum {
    case idle, working, needsYou = "needs-you", failed
    static var fallback: RunState { .idle }
}

nonisolated enum ThreadSection: String, LenientEnum {
    case pinned, inbox, working, snoozed, settled
    static var fallback: ThreadSection { .inbox }
}

nonisolated enum NeedsKind: String, LenientEnum {
    case approval, question, plan, failed, limit
    static var fallback: NeedsKind { .approval }
}

nonisolated enum ToolKind: String, LenientEnum {
    case command, read, edit, search, web, agent, other
    static var fallback: ToolKind { .other }
}

nonisolated enum ToolStatus: String, LenientEnum {
    case running, done, failed, denied
    static var fallback: ToolStatus { .done }
}

nonisolated enum ApprovalState: String, LenientEnum {
    case pending, allowed, allowedForSession = "allowed_for_session", denied, resolved
    static var fallback: ApprovalState { .resolved }
}

nonisolated enum QuestionState: String, LenientEnum {
    case pending, answered, resolved
    static var fallback: QuestionState { .resolved }
}

nonisolated enum PlanState: String, LenientEnum {
    case pending, approved, rejected, resolved
    static var fallback: PlanState { .resolved }
}

nonisolated enum SendMode: String, Codable, CaseIterable, Identifiable {
    case steer, queue
    var id: String { rawValue }
    var label: String { self == .steer ? "Steer" : "Queue" }
    var help: String {
        self == .steer ? "Goes into the running turn at its next step" : "Waits until the current turn ends"
    }
}

/// How much a thread's agent may do without asking: the Mac's hand-holding levels.
nonisolated enum Access: String, LenientEnum, CaseIterable, Identifiable {
    case supervised, autoAcceptEdits = "auto-accept-edits", auto, fullAccess = "full-access"
    static var fallback: Access { .autoAcceptEdits }
    var id: String { rawValue }
    var label: String {
        switch self {
        case .supervised: "Supervised"
        case .autoAcceptEdits: "Auto-accept edits"
        case .auto: "Auto"
        case .fullAccess: "Full access"
        }
    }
    var help: String {
        switch self {
        case .supervised: "Asks before every edit and command"
        case .autoAcceptEdits: "Applies file edits; asks before commands"
        case .auto: "Works on its own; checks before risky actions"
        case .fullAccess: "No prompts and no sandbox"
        }
    }
    var icon: String {
        switch self {
        case .supervised: "hand.raised"
        case .autoAcceptEdits: "pencil.line"
        case .auto: "wand.and.sparkles"
        case .fullAccess: "exclamationmark.shield"
        }
    }
}

/// Efforts as the Mac names them (`xhigh` → "Extra high").
nonisolated enum Effort {
    static func label(_ raw: String) -> String {
        switch raw {
        case "xhigh": "Extra high"
        default: raw.prefix(1).uppercased() + raw.dropFirst()
        }
    }
}

/// A photo for the agent, as the wire carries it.
nonisolated struct ImageUpload: Hashable {
    var mime: String
    /// Base64.
    var data: String
    var json: [String: Any] { ["mime": mime, "data": data] }
}

/// Pin, settle, archive or rename a thread.
nonisolated enum ThreadAction: Hashable {
    case pin, unpin, settle, unsettle, archive
    case rename(String)
    var json: [String: Any] {
        switch self {
        case .pin: ["kind": "pin"]
        case .unpin: ["kind": "unpin"]
        case .settle: ["kind": "settle"]
        case .unsettle: ["kind": "unsettle"]
        case .archive: ["kind": "archive"]
        case .rename(let title): ["kind": "rename", "title": title]
        }
    }
}

nonisolated enum Decision: String, Codable {
    case allow, allowForSession = "allow_for_session", deny
}

nonisolated struct HostInfo: Codable, Hashable {
    var id: String
    var name: String
    var version: String
}

nonisolated struct ProjectRef: Codable, Hashable {
    var id: String
    var name: String
    var hue: Int
    var monogram: String
}

nonisolated struct AgentRef: Codable, Hashable {
    var key: String
    var name: String
    /// The Mac's logo for it: `claude-code`, `codex`, `opencode`, `gemini`, `openai`… (its
    /// `assets/logos/{dark,light}/<logo>.png`); nil draws a neutral glyph.
    var logo: String? = nil
}

nonisolated struct Needs: Codable, Hashable {
    var kind: NeedsKind
    var text: String
}

nonisolated struct ThreadSummary: Codable, Identifiable, Hashable {
    var id: String
    var title: String
    var project: ProjectRef?
    var agent: AgentRef
    var model: String?
    var modelLabel: String?
    var runState: RunState
    var needs: Needs?
    var section: ThreadSection
    var unseen: Bool
    var pinned: Bool
    var branch: String?
    var worktree: Bool
    var activity: String?
    var workingSince: Int64?
    var updatedAt: Int64
    var additions: Int
    var deletions: Int
    /// Its effort, access and plan mode, as the Mac's composer shows them (absent from older Macs).
    var effort: String? = nil
    var access: Access? = nil
    var plan: Bool? = nil
    /// The effort as the Mac labels it ("Extra high").
    var effortLabel: String? = nil
    /// How full its context window is (the composer's ring), once the thread has been opened.
    var context: ContextUse? = nil
    /// What it cost, as the line under the Mac's composer says it, once the thread has been opened.
    var cost: Cost? = nil
    /// Its sub-agents at work: Trek's, and its agent's own.
    var subAgents: [SubAgent]? = nil
    /// What its agent runs in the background (dev servers, watchers), by title.
    var background: [String]? = nil
    /// In a worktree: the branch it started from and merges into (`branch` is the worktree's).
    var base: String? = nil
    /// Its folder's git state, as last read.
    var git: GitSummary? = nil
}

nonisolated struct ContextUse: Codable, Hashable {
    var used: Int64
    var window: Int64
    /// 0–100.
    var percent: Int
}

nonisolated enum Billing: String, LenientEnum {
    /// A subscription covers it: the figure is what it would cost at API prices.
    case plan
    /// Billed per token.
    case metered
    /// A model on the Mac: nothing is billed.
    case local
    case unknown
    static var fallback: Billing { .unknown }
}

nonisolated struct Cost: Codable, Hashable {
    /// "$1.24", "≈ $1.24 at API prices", "12.3K tokens · price unknown", "202K tokens · free".
    var label: String
    var billing: Billing? = nil
    /// "Claude Max" when billed through a plan.
    var plan: String? = nil
    /// "Included in your Claude Max plan".
    var detail: String? = nil
}

nonisolated enum SubAgentState: String, LenientEnum {
    case running, needsYou = "needs_you", done, failed, stopped
    static var fallback: SubAgentState { .running }
}

nonisolated struct SubAgent: Codable, Hashable {
    /// Whose logo it wears.
    var agent: AgentRef
    /// "Sol" for one Trek runs; nil for the agent's own.
    var model: String? = nil
    var title: String
    var state: SubAgentState
    /// When it started (ms): it's been at work for now minus this.
    var since: Int64? = nil
}

nonisolated struct GitSummary: Codable, Hashable {
    /// Files with changes not committed.
    var changed: Int = 0
    var ahead: Int = 0
    var behind: Int = 0
    var defaultBranch: String? = nil
}

nonisolated struct ProjectSummary: Codable, Identifiable, Hashable {
    var id: String
    var name: String
    var hue: Int
    var monogram: String
    var branch: String?
    var isRepo: Bool

    var ref: ProjectRef { ProjectRef(id: id, name: name, hue: hue, monogram: monogram) }
}

nonisolated struct ModelOption: Codable, Identifiable, Hashable {
    var id: String
    var label: String
    /// The efforts it takes, lowest first.
    var efforts: [String]? = nil
}

nonisolated struct AgentOption: Codable, Identifiable, Hashable {
    var key: String
    var name: String
    var defaultModel: String?
    var models: [ModelOption]
    /// As `AgentRef.logo`.
    var logo: String? = nil
    var id: String { key }
    var ref: AgentRef { AgentRef(key: key, name: name, logo: logo) }
}

nonisolated struct QuestionOption: Codable, Hashable {
    var label: String
    var description: String
}

nonisolated struct Question: Codable, Hashable {
    var header: String
    var question: String
    var options: [QuestionOption]
    var multi: Bool
    var secret: Bool
}

nonisolated struct QuestionAnswer: Codable, Hashable {
    var question: String
    var answer: String
}

// MARK: Transcript items

nonisolated struct ToolCall: Hashable {
    var callId: String
    var tool: ToolKind
    var title: String
    var detail: String
    var status: ToolStatus
    var output: String
    var added: Int?
    var removed: Int?
}

nonisolated struct ApprovalRequest: Hashable {
    var requestId: String
    var title: String
    var detail: String
    var state: ApprovalState
}

nonisolated struct QuestionRequest: Hashable {
    var requestId: String
    var questions: [Question]
    var state: QuestionState
    var answers: [QuestionAnswer]?
}

nonisolated struct PlanRequest: Hashable {
    var requestId: String
    var markdown: String
    var state: PlanState
}

nonisolated enum ItemBody: Hashable {
    case user(text: String, images: Int)
    case assistant(text: String, streaming: Bool)
    case reasoning(text: String)
    case tool(ToolCall)
    case approval(ApprovalRequest)
    case question(QuestionRequest)
    case plan(PlanRequest)
    case turnEnd(tookSecs: Int)
    case notice(String)
    case error(String)
    case limit(text: String, resetsAt: Int64?)
    case handoff(from: String, to: String)
    /// The files a turn changed, after its `turnEnd`.
    case changes(TurnChanges)
    case unknown(String)
}

nonisolated enum FileStatus: String, LenientEnum {
    case added, modified, deleted, renamed
    /// New and not added to git yet (git status only; a turn's changes say `added`).
    case untracked
    static var fallback: FileStatus { .modified }
}

/// A changed file: in a turn's changes, or in a folder's git status.
nonisolated struct ChangedFile: Codable, Hashable, Identifiable {
    var path: String
    var status: FileStatus
    /// A renamed file's old path.
    var from: String? = nil
    var added: Int = 0
    var removed: Int = 0
    /// No line counts: it isn't text.
    var binary: Bool? = nil
    var id: String { path }
    var name: String { (path as NSString).lastPathComponent }
}

/// What a turn changed, with its totals.
nonisolated struct TurnChanges: Hashable {
    var files: [ChangedFile]
    var added: Int
    var removed: Int
}

nonisolated struct TItem: Identifiable, Hashable, Codable {
    var id: String
    var seq: Int64
    var at: Int64?
    var body: ItemBody

    init(id: String, seq: Int64, at: Int64? = nil, body: ItemBody) {
        self.id = id
        self.seq = seq
        self.at = at
        self.body = body
    }

    /// Every field any item kind can carry; the `kind` picks which ones matter.
    private struct Raw: Codable {
        var id: String
        var seq: Int64
        var at: Int64?
        var kind: String
        var text: String? = nil
        var images: Int? = nil
        var streaming: Bool? = nil
        var callId: String? = nil
        var tool: ToolKind? = nil
        var title: String? = nil
        var detail: String? = nil
        var status: ToolStatus? = nil
        var output: String? = nil
        var added: Int? = nil
        var removed: Int? = nil
        var requestId: String? = nil
        var state: String? = nil
        var questions: [Question]? = nil
        var answers: [QuestionAnswer]? = nil
        var markdown: String? = nil
        var tookSecs: Int? = nil
        var resetsAt: Int64? = nil
        var from: String? = nil
        var to: String? = nil
        var files: [ChangedFile]? = nil
    }

    init(from decoder: Decoder) throws {
        let r = try Raw(from: decoder)
        id = r.id
        seq = r.seq
        at = r.at
        let text = r.text ?? ""
        switch r.kind {
        case "user": body = .user(text: text, images: r.images ?? 0)
        case "assistant": body = .assistant(text: text, streaming: r.streaming ?? false)
        case "reasoning": body = .reasoning(text: text)
        case "tool":
            body = .tool(ToolCall(callId: r.callId ?? r.id, tool: r.tool ?? .other, title: r.title ?? "",
                                  detail: r.detail ?? "", status: r.status ?? .done, output: r.output ?? "",
                                  added: r.added, removed: r.removed))
        case "approval":
            body = .approval(ApprovalRequest(requestId: r.requestId ?? "", title: r.title ?? "", detail: r.detail ?? "",
                                             state: ApprovalState(rawValue: r.state ?? "") ?? .resolved))
        case "question":
            body = .question(QuestionRequest(requestId: r.requestId ?? "", questions: r.questions ?? [],
                                             state: QuestionState(rawValue: r.state ?? "") ?? .resolved,
                                             answers: r.answers))
        case "plan":
            body = .plan(PlanRequest(requestId: r.requestId ?? "", markdown: r.markdown ?? "",
                                     state: PlanState(rawValue: r.state ?? "") ?? .resolved))
        case "turn_end": body = .turnEnd(tookSecs: r.tookSecs ?? 0)
        case "notice": body = .notice(text)
        case "error": body = .error(text)
        case "limit": body = .limit(text: text, resetsAt: r.resetsAt)
        case "handoff": body = .handoff(from: r.from ?? "", to: r.to ?? "")
        case "changes":
            let files = r.files ?? []
            body = .changes(TurnChanges(files: files, added: r.added ?? files.reduce(0) { $0 + $1.added },
                                        removed: r.removed ?? files.reduce(0) { $0 + $1.removed }))
        default: body = .unknown(r.kind)
        }
    }

    /// The item as the wire carries it (with `convertToSnakeCase`), so what `init(from:)` reads
    /// back: for the transcript cache on disk, and for the demo Mac.
    func encode(to encoder: Encoder) throws {
        var r = Raw(id: id, seq: seq, at: at, kind: "")
        switch body {
        case .user(let text, let images): r.kind = "user"; r.text = text; r.images = images
        case .assistant(let text, let streaming): r.kind = "assistant"; r.text = text; r.streaming = streaming
        case .reasoning(let text): r.kind = "reasoning"; r.text = text
        case .tool(let c):
            r.kind = "tool"; r.callId = c.callId; r.tool = c.tool; r.title = c.title; r.detail = c.detail
            r.status = c.status; r.output = c.output; r.added = c.added; r.removed = c.removed
        case .approval(let a): r.kind = "approval"; r.requestId = a.requestId; r.title = a.title; r.detail = a.detail; r.state = a.state.rawValue
        case .question(let q): r.kind = "question"; r.requestId = q.requestId; r.questions = q.questions; r.state = q.state.rawValue; r.answers = q.answers
        case .plan(let p): r.kind = "plan"; r.requestId = p.requestId; r.markdown = p.markdown; r.state = p.state.rawValue
        case .turnEnd(let secs): r.kind = "turn_end"; r.tookSecs = secs
        case .notice(let s): r.kind = "notice"; r.text = s
        case .error(let s): r.kind = "error"; r.text = s
        case .limit(let s, let at): r.kind = "limit"; r.text = s; r.resetsAt = at
        case .handoff(let a, let b): r.kind = "handoff"; r.from = a; r.to = b
        case .changes(let c): r.kind = "changes"; r.files = c.files; r.added = c.added; r.removed = c.removed
        case .unknown(let kind): r.kind = kind
        }
        try r.encode(to: encoder)
    }

    /// A request that waits on the user right now.
    var isPendingRequest: Bool {
        switch body {
        case .approval(let a): a.state == .pending
        case .question(let q): q.state == .pending
        case .plan(let p): p.state == .pending
        default: false
        }
    }
}

// MARK: Server → phone

nonisolated struct Snapshot: Decodable {
    var threads: [ThreadSummary]
    var projects: [ProjectSummary]
    var agents: [AgentOption]
    /// The Mac lets threads run with Full access.
    var fullAccess: Bool? = nil
}

nonisolated enum ErrorCode: String, LenientEnum {
    case badRequest = "bad_request", unauthorized, pairingFailed = "pairing_failed", rateLimited = "rate_limited"
    case unsupportedProtocol = "unsupported_protocol", notFound = "not_found", conflict, hostError = "host_error"
    case unknown
    static var fallback: ErrorCode { .unknown }
}

nonisolated enum ServerMessage {
    case paired(re: String?, token: String, host: HostInfo)
    case welcome(re: String?, host: HostInfo)
    case snapshot(Snapshot)
    case thread(ThreadSummary)
    case threadRemoved(String)
    case transcript(re: String?, threadId: String, reset: Bool, seq: Int64, items: [TItem])
    case item(threadId: String, item: TItem)
    case transcriptReset(String)
    /// `open`: the phone carries on by itself (`/new` sent to a thread opens the new-thread sheet).
    case ack(re: String?, threadId: String?, open: OpenScreen? = nil)
    case pong(re: String?)
    case error(re: String?, code: ErrorCode, message: String)
    case usage(re: String?, Usage)
    case basecamp(re: String?, Basecamp)
    case notes(re: String?, [NoteSummary])
    case note(re: String?, Note)
    case gitStatus(re: String?, GitStatus)
    case gitDiff(re: String?, GitDiff)
    case gitBranches(re: String?, GitBranches)
    case commands(re: String?, threadId: String, [CommandInfo])
    case settings(re: String?, MacSettings)
    case unknown(String)

    private struct Raw: Decodable {
        var type: String
        var re: String?
        var token: String?
        var host: HostInfo?
        var threads: [ThreadSummary]?
        var projects: [ProjectSummary]?
        var agents: [AgentOption]?
        var thread: ThreadSummary?
        var threadId: String?
        var reset: Bool?
        var seq: Int64?
        var items: [TItem]?
        var item: TItem?
        var code: ErrorCode?
        var message: String?
        var fullAccess: Bool?
        var open: OpenScreen?
        var notes: [NoteSummary]?
        var note: Note?
        var commands: [CommandInfo]?
    }

    static let decoder: JSONDecoder = {
        let d = JSONDecoder()
        d.keyDecodingStrategy = .convertFromSnakeCase
        return d
    }()

    static func decode(_ data: Data) throws -> ServerMessage {
        let r = try decoder.decode(Raw.self, from: data)
        switch r.type {
        case "paired":
            guard let token = r.token, let host = r.host else { throw ProtocolError.missing("paired") }
            return .paired(re: r.re, token: token, host: host)
        case "welcome":
            guard let host = r.host else { throw ProtocolError.missing("welcome") }
            return .welcome(re: r.re, host: host)
        case "snapshot":
            return .snapshot(Snapshot(threads: r.threads ?? [], projects: r.projects ?? [], agents: r.agents ?? [],
                                      fullAccess: r.fullAccess))
        case "thread":
            guard let t = r.thread else { throw ProtocolError.missing("thread") }
            return .thread(t)
        case "thread_removed": return .threadRemoved(r.threadId ?? "")
        case "transcript":
            return .transcript(re: r.re, threadId: r.threadId ?? "", reset: r.reset ?? true, seq: r.seq ?? 0, items: r.items ?? [])
        case "item":
            guard let item = r.item, let tid = r.threadId else { throw ProtocolError.missing("item") }
            return .item(threadId: tid, item: item)
        case "transcript_reset": return .transcriptReset(r.threadId ?? "")
        case "ack": return .ack(re: r.re, threadId: r.threadId, open: r.open)
        case "pong": return .pong(re: r.re)
        case "error": return .error(re: r.re, code: r.code ?? .unknown, message: r.message ?? "")
        // Replies that are a value whole: decoded from the message itself.
        case "usage": return .usage(re: r.re, try decoder.decode(Usage.self, from: data))
        case "basecamp": return .basecamp(re: r.re, try decoder.decode(Basecamp.self, from: data))
        case "notes": return .notes(re: r.re, r.notes ?? [])
        case "note":
            guard let note = r.note else { throw ProtocolError.missing("note") }
            return .note(re: r.re, note)
        case "git_status": return .gitStatus(re: r.re, try decoder.decode(GitStatus.self, from: data))
        case "git_diff": return .gitDiff(re: r.re, try decoder.decode(GitDiff.self, from: data))
        case "git_branches": return .gitBranches(re: r.re, try decoder.decode(GitBranches.self, from: data))
        case "commands": return .commands(re: r.re, threadId: r.threadId ?? "", r.commands ?? [])
        case "settings": return .settings(re: r.re, try decoder.decode(MacSettings.self, from: data))
        default: return .unknown(r.type)
        }
    }
}

nonisolated enum ProtocolError: Error {
    case missing(String)
}

// MARK: Phone → server

nonisolated enum AnswerResponse {
    case approval(Decision)
    case questions([QuestionAnswer])
    case plan(approve: Bool, feedback: String?)

    var json: [String: Any] {
        switch self {
        case .approval(let d): ["kind": "approval", "decision": d.rawValue]
        case .questions(let qa):
            ["kind": "questions", "answers": qa.map { ["question": $0.question, "answer": $0.answer] }]
        case .plan(let approve, let feedback):
            ["kind": "plan", "approve": approve, "feedback": feedback ?? NSNull()]
        }
    }
}

nonisolated enum ClientMessage {
    case pair(code: String, deviceId: String, deviceName: String)
    case hello(deviceId: String, token: String)
    case subscribe(threadId: String, afterSeq: Int64?)
    case unsubscribe(threadId: String)
    case send(threadId: String, text: String, mode: SendMode?, images: [ImageUpload])
    case newThread(projectId: String, agent: String, model: String?, text: String, worktree: Bool,
                   effort: String?, access: Access?, plan: Bool, images: [ImageUpload])
    case setPrefs(threadId: String, agent: String?, model: String?, effort: String?, access: Access?, plan: Bool?)
    case threadAction(threadId: String, action: ThreadAction)
    case answer(threadId: String, requestId: String, response: AnswerResponse)
    case interrupt(threadId: String)
    case markSeen(threadId: String)
    case usage
    case basecamp(BasecampRange)
    case notes
    case note(id: String)
    case createNote(body: String)
    /// `modified`: the note's `modified` as the phone read it (the Mac refuses if it changed since).
    case saveNote(id: String, body: String, modified: Int64?)
    case deleteNote(id: String)
    case gitStatus(GitTarget)
    case gitDiff(GitTarget, path: String)
    case gitCommit(GitTarget, message: String)
    case gitPush(GitTarget)
    case gitBranches(GitTarget)
    case gitSwitch(GitTarget, branch: String)
    case worktreeMerge(threadId: String)
    /// Without `force` the Mac refuses (`conflict`, saying what) when work would be lost.
    case worktreeRemove(threadId: String, deleteBranch: Bool, force: Bool)
    case commands(threadId: String)
    case settings
    case setSettings(SettingsChange)
    case ping

    static let appVersion = Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "0.1.0"

    /// The JSON object for this message; `id` correlates the Mac's reply (`re`).
    func json(id: String?) -> [String: Any] {
        var o: [String: Any]
        switch self {
        case .pair(let code, let deviceId, let deviceName):
            o = ["type": "pair", "protocol": trekProtocolVersion, "code": code, "device_id": deviceId,
                 "device_name": deviceName, "app_version": Self.appVersion]
        case .hello(let deviceId, let token):
            o = ["type": "hello", "protocol": trekProtocolVersion, "device_id": deviceId, "token": token,
                 "app_version": Self.appVersion]
        case .subscribe(let threadId, let afterSeq):
            o = ["type": "subscribe", "thread_id": threadId, "after_seq": afterSeq.map { $0 as Any } ?? NSNull()]
        case .unsubscribe(let threadId):
            o = ["type": "unsubscribe", "thread_id": threadId]
        case .send(let threadId, let text, let mode, let images):
            o = ["type": "send", "thread_id": threadId, "text": text, "mode": mode?.rawValue ?? NSNull()]
            if !images.isEmpty { o["images"] = images.map(\.json) }
        case .newThread(let projectId, let agent, let model, let text, let worktree, let effort, let access, let plan, let images):
            o = ["type": "new_thread", "project_id": projectId, "agent": agent, "model": model ?? NSNull(),
                 "text": text, "worktree": worktree]
            if let effort { o["effort"] = effort }
            if let access { o["access"] = access.rawValue }
            if plan { o["plan"] = true }
            if !images.isEmpty { o["images"] = images.map(\.json) }
        case .setPrefs(let threadId, let agent, let model, let effort, let access, let plan):
            o = ["type": "set_prefs", "thread_id": threadId]
            if let agent { o["agent"] = agent }
            if let model { o["model"] = model }
            if let effort { o["effort"] = effort }
            if let access { o["access"] = access.rawValue }
            if let plan { o["plan"] = plan }
        case .threadAction(let threadId, let action):
            o = ["type": "thread_action", "thread_id": threadId, "action": action.json]
        case .answer(let threadId, let requestId, let response):
            o = ["type": "answer", "thread_id": threadId, "request_id": requestId, "response": response.json]
        case .interrupt(let threadId):
            o = ["type": "interrupt", "thread_id": threadId]
        case .markSeen(let threadId):
            o = ["type": "mark_seen", "thread_id": threadId]
        case .usage:
            o = ["type": "usage"]
        case .basecamp(let range):
            o = ["type": "basecamp", "range": range.rawValue]
        case .notes:
            o = ["type": "notes"]
        case .note(let id):
            o = ["type": "note", "note_id": id]
        case .createNote(let body):
            o = ["type": "create_note", "body": body]
        case .saveNote(let id, let body, let modified):
            o = ["type": "save_note", "note_id": id, "body": body]
            if let modified { o["modified"] = modified }
        case .deleteNote(let id):
            o = ["type": "delete_note", "note_id": id]
        case .gitStatus(let target):
            o = target.json.merging(["type": "git_status"]) { $1 }
        case .gitDiff(let target, let path):
            o = target.json.merging(["type": "git_diff", "path": path]) { $1 }
        case .gitCommit(let target, let message):
            o = target.json.merging(["type": "git_commit", "message": message]) { $1 }
        case .gitPush(let target):
            o = target.json.merging(["type": "git_push"]) { $1 }
        case .gitBranches(let target):
            o = target.json.merging(["type": "git_branches"]) { $1 }
        case .gitSwitch(let target, let branch):
            o = target.json.merging(["type": "git_switch", "branch": branch]) { $1 }
        case .worktreeMerge(let threadId):
            o = ["type": "worktree_merge", "thread_id": threadId]
        case .worktreeRemove(let threadId, let deleteBranch, let force):
            o = ["type": "worktree_remove", "thread_id": threadId]
            if deleteBranch { o["delete_branch"] = true }
            if force { o["force"] = true }
        case .commands(let threadId):
            o = ["type": "commands", "thread_id": threadId]
        case .settings:
            o = ["type": "settings"]
        case .setSettings(let change):
            o = change.json.merging(["type": "set_settings"]) { $1 }
        case .ping:
            o = ["type": "ping"]
        }
        if let id { o["id"] = id }
        return o
    }

    func encoded(id: String?) throws -> Data {
        try JSONSerialization.data(withJSONObject: json(id: id), options: [.sortedKeys])
    }
}
