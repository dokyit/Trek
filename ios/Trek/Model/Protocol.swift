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
}

nonisolated struct AgentOption: Codable, Identifiable, Hashable {
    var key: String
    var name: String
    var defaultModel: String?
    var models: [ModelOption]
    var id: String { key }
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
    case unknown(String)
}

nonisolated struct TItem: Identifiable, Hashable, Decodable {
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
    private struct Raw: Decodable {
        var id: String
        var seq: Int64
        var at: Int64?
        var kind: String
        var text: String?
        var images: Int?
        var streaming: Bool?
        var callId: String?
        var tool: ToolKind?
        var title: String?
        var detail: String?
        var status: ToolStatus?
        var output: String?
        var added: Int?
        var removed: Int?
        var requestId: String?
        var state: String?
        var questions: [Question]?
        var answers: [QuestionAnswer]?
        var markdown: String?
        var tookSecs: Int?
        var resetsAt: Int64?
        var from: String?
        var to: String?
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
        default: body = .unknown(r.kind)
        }
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
    case ack(re: String?, threadId: String?)
    case pong(re: String?)
    case error(re: String?, code: ErrorCode, message: String)
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
            return .snapshot(Snapshot(threads: r.threads ?? [], projects: r.projects ?? [], agents: r.agents ?? []))
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
        case "ack": return .ack(re: r.re, threadId: r.threadId)
        case "pong": return .pong(re: r.re)
        case "error": return .error(re: r.re, code: r.code ?? .unknown, message: r.message ?? "")
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
    case send(threadId: String, text: String, mode: SendMode?)
    case newThread(projectId: String, agent: String, model: String?, text: String, worktree: Bool)
    case answer(threadId: String, requestId: String, response: AnswerResponse)
    case interrupt(threadId: String)
    case markSeen(threadId: String)
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
        case .send(let threadId, let text, let mode):
            o = ["type": "send", "thread_id": threadId, "text": text, "mode": mode?.rawValue ?? NSNull()]
        case .newThread(let projectId, let agent, let model, let text, let worktree):
            o = ["type": "new_thread", "project_id": projectId, "agent": agent, "model": model ?? NSNull(),
                 "text": text, "worktree": worktree]
        case .answer(let threadId, let requestId, let response):
            o = ["type": "answer", "thread_id": threadId, "request_id": requestId, "response": response.json]
        case .interrupt(let threadId):
            o = ["type": "interrupt", "thread_id": threadId]
        case .markSeen(let threadId):
            o = ["type": "mark_seen", "thread_id": threadId]
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
