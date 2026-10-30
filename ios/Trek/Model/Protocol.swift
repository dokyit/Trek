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

nonisolated enum SendMode: String, LenientEnum, CaseIterable, Identifiable {
    case steer, queue
    static var fallback: SendMode { .steer }
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

/// Pin, settle, archive or rename a thread; snooze, wake, mark unread, regenerate its title or
/// delete it (where the Mac lists those features).
nonisolated enum ThreadAction: Hashable {
    case pin, unpin, settle, unsettle, archive
    case rename(String)
    /// Until `until` (ms), or one of the Mac's presets.
    case snooze(until: Int64?, preset: SnoozePreset?)
    case wake, markUnread, regenerateTitle
    /// Unconfirmed, the Mac answers `conflict` with what its confirmation says.
    case delete(confirmed: Bool)
    var json: [String: Any] {
        switch self {
        case .pin: ["kind": "pin"]
        case .unpin: ["kind": "unpin"]
        case .settle: ["kind": "settle"]
        case .unsettle: ["kind": "unsettle"]
        case .archive: ["kind": "archive"]
        case .rename(let title): ["kind": "rename", "title": title]
        case .snooze(let until, let preset):
            { () -> [String: Any] in
                var o: [String: Any] = ["kind": "snooze"]
                if let until { o["until"] = until }
                if let preset { o["preset"] = preset.rawValue }
                return o
            }()
        case .wake: ["kind": "wake"]
        case .markUnread: ["kind": "mark_unread"]
        case .regenerateTitle: ["kind": "regenerate_title"]
        case .delete(let confirmed): confirmed ? ["kind": "delete", "confirmed": true] : ["kind": "delete"]
        }
    }
}

/// What `turn_action` does to a turn.
nonisolated enum TurnAction: String, Hashable {
    /// Take back the turn: the message and everything after it.
    case undo
    /// Take it back and send the message again (with `model`, another model).
    case retry
    /// A new thread with the conversation up to here.
    case fork
    /// Back to before a message; the ack carries its text, to edit and send again.
    case rewind
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
    /// The icon chosen for it on the Mac, by hash (`IconStore` fetches it); nil: the monogram.
    var icon: String? = nil
}

nonisolated struct AgentRef: Codable, Hashable {
    var key: String
    var name: String
    /// The Mac's logo for it: `claude-code`, `codex`, `opencode`, `gemini`, `openai`… (its
    /// `assets/logos/{dark,light}/<logo>.png`); nil draws a neutral glyph.
    var logo: String? = nil
    /// An added agent's own icon, by hash (`IconStore` fetches it).
    var icon: String? = nil
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
    /// A sub-agent's: the thread that started it.
    var parentId: String? = nil
    /// Started by another thread's agent (`start_thread`): that thread.
    var startedBy: String? = nil
    /// Took another thread's work over (`handoff`): that thread.
    var continuedFrom: String? = nil
    /// Snoozed: when it wakes (ms).
    var snoozedUntil: Int64? = nil
    /// `needs` is a sub-agent's: its thread, where it's answered.
    var needsThread: String? = nil
    /// Follow-ups waiting to go, in order (`queue_action` changes them).
    var queued: [QueuedMessage]? = nil
    /// `background` in full: each task's last line, and whether it can be stopped.
    var backgroundTasks: [BackgroundTask]? = nil
    /// Its turn is over but it still waits on work it started: the Mac's sidebar keeps it at work
    /// ("Waiting"). Nil from older Macs (`waitsInBackground` works it out then).
    var busyInBackground: Bool? = nil
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
    /// "92% cached": how much of its prompts the prompt cache served, and in words.
    var cached: String? = nil
    var cachedDetail: String? = nil
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
    /// Its thread, for one Trek runs: it opens like any other.
    var threadId: String? = nil
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
    var icon: String? = nil

    var ref: ProjectRef { ProjectRef(id: id, name: name, hue: hue, monogram: monogram, icon: icon) }
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
    var icon: String? = nil
    var id: String { key }
    var ref: AgentRef { AgentRef(key: key, name: name, logo: logo, icon: icon) }

    private enum CodingKeys: String, CodingKey { case key, name, defaultModel, models, logo, icon }

    /// An agent whose models the Mac left out still reads (with none).
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        key = try c.decode(String.self, forKey: .key)
        name = try c.decode(String.self, forKey: .name)
        defaultModel = try c.decodeIfPresent(String.self, forKey: .defaultModel)
        models = (try c.decodeIfPresent(Lossy<ModelOption>.self, forKey: .models))?.elements ?? []
        logo = try c.decodeIfPresent(String.self, forKey: .logo)
        icon = try c.decodeIfPresent(String.self, forKey: .icon)
    }

    init(key: String, name: String, defaultModel: String?, models: [ModelOption], logo: String? = nil, icon: String? = nil) {
        (self.key, self.name, self.defaultModel, self.models, self.logo, self.icon) = (key, name, defaultModel, models, logo, icon)
    }
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
    /// `aside`: not a prompt (a command Trek answered, a typed answer): nothing to rewind to.
    case user(text: String, images: Int, aside: Bool = false)
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
    /// A row that opens another thread: one this thread's agent started or handed over to, or
    /// the one that started it or that it continues.
    case threadLink(ThreadLink)
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
        var aside: Bool? = nil
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
        var link: ThreadLinkKind? = nil
        var threadId: String? = nil
        var agent: AgentRef? = nil
        var open: Bool? = nil
    }

    init(from decoder: Decoder) throws {
        let r = try Raw(from: decoder)
        id = r.id
        seq = r.seq
        at = r.at
        let text = r.text ?? ""
        switch r.kind {
        case "user": body = .user(text: text, images: r.images ?? 0, aside: r.aside ?? false)
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
        case "thread_link":
            body = .threadLink(ThreadLink(link: r.link ?? .startedBy, threadId: r.threadId ?? "", title: r.title ?? "",
                                          agent: r.agent, state: r.state.flatMap(LinkedThreadState.init(rawValue:)),
                                          open: r.open ?? true))
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
        case .user(let text, let images, let aside): r.kind = "user"; r.text = text; r.images = images; r.aside = aside ? true : nil
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
        case .threadLink(let l):
            r.kind = "thread_link"; r.link = l.link; r.threadId = l.threadId; r.title = l.title; r.agent = l.agent
            r.state = l.state?.rawValue; r.open = l.open
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
    /// Threads the list leaves out that the phone may open (sub-agents, ones open from a search).
    var unlisted: [ThreadSummary] = []
}

nonisolated enum ErrorCode: String, LenientEnum {
    case badRequest = "bad_request", unauthorized, pairingFailed = "pairing_failed", rateLimited = "rate_limited"
    case unsupportedProtocol = "unsupported_protocol", notFound = "not_found", conflict, hostError = "host_error"
    case unknown
    static var fallback: ErrorCode { .unknown }
}

nonisolated enum ServerMessage {
    /// `features`: the optional requests the Mac serves (`MacFeature`). `routes`: every
    /// `host:port` it answers on (Wi-Fi, tailnet), to try in turn; none from older Macs.
    case paired(re: String?, token: String, host: HostInfo, features: [String] = [], routes: [String] = [])
    case welcome(re: String?, host: HostInfo, features: [String] = [], routes: [String] = [])
    case snapshot(Snapshot)
    case thread(ThreadSummary)
    case threadRemoved(String)
    /// `more`: items before these exist (load them with `transcriptBefore`).
    case transcript(re: String?, threadId: String, reset: Bool, seq: Int64, items: [TItem], more: Bool = false)
    /// The items just before the one asked for, in order.
    case transcriptPage(re: String?, threadId: String, items: [TItem], more: Bool)
    case item(threadId: String, item: TItem)
    case transcriptReset(String)
    /// `open`: the phone carries on by itself (`/new` sent to a thread opens the new-thread sheet).
    /// `text`: a rewound message, for the composer.
    case ack(re: String?, threadId: String?, open: OpenScreen? = nil, text: String? = nil)
    case pong(re: String?)
    /// What the machine is (`machine_info`): its platform marks its threads here.
    case machineInfo(re: String?, MachineInfo)
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
    /// A thread the list leaves out (a sub-agent's, one open from a search).
    case unlisted(ThreadSummary)
    case searchResults(re: String?, SearchResults)
    case attachment(re: String?, Attachment)
    case backgroundOutput(re: String?, BackgroundOutput)
    case icon(re: String?, IconReply)
    case noteAsset(re: String?, NoteAsset)
    case noteImage(re: String?, NoteImage)
    case noteFileChunk(re: String?, NoteFileChunk)
    case unknown(String)

    private struct Raw: Decodable {
        var type: String
        var re: String?
        var token: String?
        var host: HostInfo?
        // Lists read item by item: one that doesn't decode is left out, not the message.
        var threads: Lossy<ThreadSummary>?
        var projects: Lossy<ProjectSummary>?
        var agents: Lossy<AgentOption>?
        var unlisted: Lossy<ThreadSummary>?
        var features: [String]?
        var routes: [String]?
        var thread: ThreadSummary?
        var threadId: String?
        var reset: Bool?
        var seq: Int64?
        var items: Lossy<TItem>?
        var item: TItem?
        var code: ErrorCode?
        var message: String?
        var fullAccess: Bool?
        var open: OpenScreen?
        var notes: [NoteSummary]?
        var note: Note?
        var commands: [CommandInfo]?
        var text: String?
        var more: Bool?
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
            return .paired(re: r.re, token: token, host: host, features: r.features ?? [], routes: r.routes ?? [])
        case "welcome":
            guard let host = r.host else { throw ProtocolError.missing("welcome") }
            return .welcome(re: r.re, host: host, features: r.features ?? [], routes: r.routes ?? [])
        case "snapshot":
            return .snapshot(Snapshot(threads: r.threads?.elements ?? [], projects: r.projects?.elements ?? [],
                                      agents: r.agents?.elements ?? [], fullAccess: r.fullAccess,
                                      unlisted: r.unlisted?.elements ?? []))
        case "thread":
            guard let t = r.thread else { throw ProtocolError.missing("thread") }
            return .thread(t)
        case "thread_removed": return .threadRemoved(r.threadId ?? "")
        case "transcript":
            return .transcript(re: r.re, threadId: r.threadId ?? "", reset: r.reset ?? true, seq: r.seq ?? 0,
                               items: r.items?.elements ?? [], more: r.more ?? false)
        case "transcript_page":
            return .transcriptPage(re: r.re, threadId: r.threadId ?? "", items: r.items?.elements ?? [], more: r.more ?? false)
        case "item":
            guard let item = r.item, let tid = r.threadId else { throw ProtocolError.missing("item") }
            return .item(threadId: tid, item: item)
        case "transcript_reset": return .transcriptReset(r.threadId ?? "")
        case "ack": return .ack(re: r.re, threadId: r.threadId, open: r.open, text: r.text)
        case "pong": return .pong(re: r.re)
        case "machine_info": return .machineInfo(re: r.re, try decoder.decode(MachineInfo.self, from: data))
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
        case "unlisted":
            guard let t = r.thread else { throw ProtocolError.missing("unlisted") }
            return .unlisted(t)
        case "search_results": return .searchResults(re: r.re, try decoder.decode(SearchResults.self, from: data))
        case "attachment": return .attachment(re: r.re, try decoder.decode(Attachment.self, from: data))
        case "background_output": return .backgroundOutput(re: r.re, try decoder.decode(BackgroundOutput.self, from: data))
        case "icon": return .icon(re: r.re, try decoder.decode(IconReply.self, from: data))
        case "note_asset": return .noteAsset(re: r.re, try decoder.decode(NoteAsset.self, from: data))
        case "note_image": return .noteImage(re: r.re, try decoder.decode(NoteImage.self, from: data))
        case "note_file_chunk": return .noteFileChunk(re: r.re, try decoder.decode(NoteFileChunk.self, from: data))
        default: return .unknown(r.type)
        }
    }
}

nonisolated extension ServerMessage {
    /// Decodes off the main actor: a long transcript takes a while to read, and the UI keeps going.
    /// A message that doesn't decode is logged and left out.
    @concurrent static func decodeInBackground(_ data: Data) async -> ServerMessage? {
        do {
            return try decode(data)
        } catch {
            protocolLog.error("Left out a message from the Mac that didn't decode: \(String(describing: error), privacy: .public)")
            return nil
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
    /// `limit`: on a reset, only the last `limit` items (and every pending request).
    case subscribe(threadId: String, afterSeq: Int64?, limit: Int? = nil)
    /// The `limit` items just before item `before` (the Mac caps it at 500).
    case transcriptBefore(threadId: String, before: String, limit: Int)
    /// Undo, retry, fork or rewind a turn, from its `turn_end` item (or, rewinding and forking from
    /// a message, its `user` item). `model`: retry with another model.
    case turnAction(threadId: String, itemId: String, action: TurnAction, model: String? = nil, restoreFiles: Bool = true)
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
    /// What the machine is: its name and platform (a Mac that lists `machines` answers).
    case machineInfo
    /// Threads whose title or messages match, as the Mac's own search finds them.
    case search(query: String, limit: Int?)
    // Parity with the Mac (Parity.swift); a Mac lists the ones it answers in `features`.
    case useReset(creditId: String)
    /// `text`: the message as shown, so one that went out meanwhile isn't touched.
    case queueAction(threadId: String, index: Int, reset: Bool, text: String, edit: String?)
    case attachment(threadId: String, itemId: String, index: Int, maxPx: Int?)
    case backgroundOutput(threadId: String, taskId: String)
    case stopBackground(threadId: String, taskId: String)
    case icon(agent: String?, projectId: String?)
    /// One of a note's pictures (`attachments/…`), fitting `maxPx` a side.
    case noteAsset(noteId: String, path: String, maxPx: Int?)
    /// Keep a photo (a base64 JPEG) with the notes; the Mac answers with its link.
    case addNoteImage(noteId: String, data: String, name: String?)
    /// `length` bytes from `offset` of a file kept with the notes (`attachments/…`), as it is.
    case noteFile(path: String, offset: Int64, length: Int)

    static let appVersion = Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "0.1.0"
    /// The build (the number of commits it was built from, `install-device.sh`).
    static let appBuild = Bundle.main.infoDictionary?["CFBundleVersion"] as? String

    /// The JSON object for this message; `id` correlates the Mac's reply (`re`).
    func json(id: String?) -> [String: Any] {
        var o: [String: Any]
        switch self {
        case .pair(let code, let deviceId, let deviceName):
            o = ["type": "pair", "protocol": trekProtocolVersion, "code": code, "device_id": deviceId,
                 "device_name": deviceName, "app_version": Self.appVersion, "platform": "ios"]
        case .hello(let deviceId, let token):
            o = ["type": "hello", "protocol": trekProtocolVersion, "device_id": deviceId, "token": token,
                 "app_version": Self.appVersion, "platform": "ios"]
        case .subscribe(let threadId, let afterSeq, let limit):
            o = ["type": "subscribe", "thread_id": threadId, "after_seq": afterSeq.map { $0 as Any } ?? NSNull()]
            if let limit { o["limit"] = limit }
        case .transcriptBefore(let threadId, let before, let limit):
            o = ["type": "transcript_before", "thread_id": threadId, "before": before, "limit": limit]
        case .turnAction(let threadId, let itemId, let action, let model, let restoreFiles):
            o = ["type": "turn_action", "thread_id": threadId, "item_id": itemId, "action": action.rawValue]
            if let model { o["model"] = model }
            if action != .fork { o["restore_files"] = restoreFiles }
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
        case .machineInfo:
            o = ["type": "machine_info"]
        case .search(let query, let limit):
            o = ["type": "search", "query": query]
            if let limit { o["limit"] = limit }
        case .useReset, .queueAction, .attachment, .backgroundOutput, .stopBackground, .icon, .noteAsset, .addNoteImage, .noteFile:
            o = parityJSON
        }
        if let id { o["id"] = id }
        return o
    }

    func encoded(id: String?) throws -> Data {
        try JSONSerialization.data(withJSONObject: json(id: id), options: [.sortedKeys])
    }
}

/// A machine's answer to `machine_info`.
nonisolated struct MachineInfo: Codable, Sendable, Equatable {
    var name: String
    /// "macos", "windows", "linux".
    var platform: String
}
