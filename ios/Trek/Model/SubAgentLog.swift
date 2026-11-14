import Foundation
import Observation

// A sub-agent opened from its row (feature `sub_agent_view`, mirrors
// crates/trek-remote/src/protocol/sub_agent_view.rs). A tool item that stands for one carries a
// `SubAgentLink`: one Trek runs (`delegate_task`) has a thread of its own and opens like any other
// thread; one of the agent's own (Claude Code's Task, Codex's agents) lives in its parent's
// session, and `sub_agent` reads what it has done so far: its prompt, what it said, its calls.
// `sub_agent_action` stops it or sends it a message where its agent takes one; the Mac says which
// (`canStop`, `canMessage`) and, when it can't take a message, why (`noMessage`).

/// On a tool item: the sub-agent it stands for, as its row on the Mac shows it.
nonisolated struct SubAgentLink: Codable, Hashable, Sendable {
    /// Whose logo it wears: the agent running it.
    var agent: AgentRef
    /// "Sol: Review the cache design", or what the agent's own was sent to do.
    var label: String
    /// Where it stands, and what it's doing or why it failed ("Reading src/auth.rs · 4 steps").
    var detail: String? = nil
    var state: SubAgentState
    /// When it started (ms), while it works.
    var since: Int64? = nil
    /// How long it worked, once it's done.
    var tookSecs: Int? = nil
    /// Its thread, for one Trek runs: it opens like any other.
    var threadId: String? = nil

    /// Still at it (working, or waiting on the user).
    var live: Bool { state == .running || state == .needsYou }
}

nonisolated enum SubAgentAction: String, Sendable {
    case stop, message
}

/// The reply to `sub_agent` and `sub_agent_action`: what the agent's own sub-agent has done.
nonisolated struct SubAgentLog: Decodable, Sendable, Equatable {
    var threadId: String
    var callId: String
    var link: SubAgentLink
    /// What it was told to do, when its agent said.
    var prompt: String? = nil
    /// What it said and called, oldest first (ids `s0`, `s1`… stay put while it works).
    var items: [TItem] = []
    /// Earlier ones the Mac let go.
    var more: Int = 0
    /// Its answer, once it's done.
    var answer: String? = nil
    var canStop: Bool = false
    var canMessage: Bool = false
    /// Why there's no message to send it, in the Mac's words.
    var noMessage: String? = nil
    /// Its parent's thread waits on the user (maybe for this one): answered there.
    var asking: Bool = false
    /// The Mac no longer has its steps (its session ended, or Trek restarted).
    var gone: Bool = false

    private enum CodingKeys: String, CodingKey {
        case threadId, callId, link, prompt, items, answer, canStop, canMessage, noMessage, asking, gone
        // Not `more`: transcripts send that as true or false.
        case more = "earlier"
    }

    init(threadId: String, callId: String, link: SubAgentLink, prompt: String? = nil, items: [TItem] = [], more: Int = 0,
         answer: String? = nil, canStop: Bool = false, canMessage: Bool = false, noMessage: String? = nil,
         asking: Bool = false, gone: Bool = false) {
        (self.threadId, self.callId, self.link, self.prompt, self.items, self.more) = (threadId, callId, link, prompt, items, more)
        (self.answer, self.canStop, self.canMessage, self.noMessage, self.asking, self.gone) = (answer, canStop, canMessage, noMessage, asking, gone)
    }

    /// What a Mac leaves out reads as nothing (no items, nothing it can do).
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        threadId = try c.decode(String.self, forKey: .threadId)
        callId = try c.decode(String.self, forKey: .callId)
        link = try c.decode(SubAgentLink.self, forKey: .link)
        prompt = try c.decodeIfPresent(String.self, forKey: .prompt)
        items = (try c.decodeIfPresent(Lossy<TItem>.self, forKey: .items))?.elements ?? []
        more = try c.decodeIfPresent(Int.self, forKey: .more) ?? 0
        answer = try c.decodeIfPresent(String.self, forKey: .answer)
        canStop = try c.decodeIfPresent(Bool.self, forKey: .canStop) ?? false
        canMessage = try c.decodeIfPresent(Bool.self, forKey: .canMessage) ?? false
        noMessage = try c.decodeIfPresent(String.self, forKey: .noMessage)
        asking = try c.decodeIfPresent(Bool.self, forKey: .asking) ?? false
        gone = try c.decodeIfPresent(Bool.self, forKey: .gone) ?? false
    }

    /// Worth asking again: it's still at work, and the Mac still has it.
    var keepsChanging: Bool { link.live && !gone }
}

/// Which sub-agent to show: the agent's own started by call `callId` in thread `threadId`
/// (qualified), with what its row said, to show before the Mac answers.
nonisolated struct SubAgentTarget: Identifiable, Hashable, Sendable {
    var threadId: String
    var callId: String
    var link: SubAgentLink?
    var id: String { "\(threadId)/\(callId)" }
}

extension AppModel {
    /// Opens the sub-agent a row or the at-work card stands for: Trek's own in its thread, the
    /// agent's own in `SubAgentView`. `open` opens a thread where there's a way to (`openThread`).
    /// False when the Mac serves neither.
    @discardableResult
    func open(subAgent link: SubAgentLink?, threadId kid: String?, callId: String?, in tid: String,
              open: ((String) -> Void)? = nil) -> Bool {
        if let kid = kid ?? link?.threadId {
            guard serves(.subAgents, on: tid) else { return false }
            if let open { open(kid) } else { openRequest = kid }
            return true
        }
        guard let callId, serves(.subAgentView, on: tid) else { return false }
        openSubAgent = SubAgentTarget(threadId: tid, callId: callId, link: link)
        return true
    }

    /// Asks the Mac what the sub-agent has done. `done` gets it, or why the Mac couldn't say.
    func fetchSubAgent(_ target: SubAgentTarget, done: @escaping (Result<SubAgentLog, SubAgentFailure>) -> Void) {
        request(.subAgent(threadId: target.threadId, callId: target.callId)) { reply in
            done(Self.subAgentReply(reply))
        }
    }

    /// Stops it, or sends it `text`; `done` gets where it stands after.
    func steer(_ target: SubAgentTarget, _ action: SubAgentAction, text: String? = nil,
               done: @escaping (Result<SubAgentLog, SubAgentFailure>) -> Void) {
        request(.subAgentAction(threadId: target.threadId, callId: target.callId, action: action, text: text)) { reply in
            done(Self.subAgentReply(reply))
        }
    }

    nonisolated static func subAgentReply(_ reply: ServerMessage) -> Result<SubAgentLog, SubAgentFailure> {
        switch reply {
        case .subAgent(_, let log): .success(log)
        case .error(_, _, let text): .failure(SubAgentFailure(message: text))
        default: .failure(SubAgentFailure(message: "That Mac didn't answer about the sub-agent."))
        }
    }
}

/// Why the Mac couldn't show or steer a sub-agent, in its words.
nonisolated struct SubAgentFailure: Error, Equatable, Sendable {
    var message: String
}

/// What `SubAgentView` shows of one sub-agent: asked for again every `every` seconds while it's
/// on screen and the sub-agent works, and once after a stop or a message.
@MainActor
@Observable
final class SubAgentWatch {
    let target: SubAgentTarget
    private(set) var log: SubAgentLog?
    /// The Mac's words when it couldn't say (shown while there's nothing else).
    private(set) var failure: String?
    /// A stop or a message is on its way.
    private(set) var sending = false
    static let every: TimeInterval = 1.5
    @ObservationIgnored private var polling: Task<Void, Never>?

    init(target: SubAgentTarget) { self.target = target }

    /// Its row as last known: the Mac's answer's, else what the row said.
    var link: SubAgentLink? { log?.link ?? target.link }

    /// Asks now, then again while it works, until `stop()`.
    func start(_ model: AppModel) {
        polling?.cancel()
        polling = Task { [weak self, weak model] in
            while !Task.isCancelled {
                guard let self, let model else { return }
                let keeps = await self.fetch(model)
                if !keeps { self.polling = nil; return }
                try? await Task.sleep(for: .seconds(Self.every))
            }
        }
    }

    func stop() {
        polling?.cancel()
        polling = nil
    }

    /// One ask; true while it's worth asking again.
    private func fetch(_ model: AppModel) async -> Bool {
        await withCheckedContinuation { (k: CheckedContinuation<Bool, Never>) in
            model.fetchSubAgent(target) { [weak self] result in
                guard let self else { return k.resume(returning: false) }
                k.resume(returning: self.take(result))
            }
        }
    }

    @discardableResult
    func take(_ result: Result<SubAgentLog, SubAgentFailure>) -> Bool {
        switch result {
        case .success(let log):
            if self.log != log { self.log = log }
            failure = nil
            return log.keepsChanging
        case .failure(let f):
            failure = f.message
            // Asked again later: a Mac that's out of reach a moment may come back.
            return log?.keepsChanging ?? true
        }
    }

    /// Stops it, or sends it `text`; `done` hears the Mac's refusal, if it refused.
    func steer(_ action: SubAgentAction, text: String? = nil, model: AppModel, done: @escaping (String?) -> Void = { _ in }) {
        sending = true
        model.steer(target, action, text: text) { [weak self, weak model] result in
            guard let self else { return }
            self.sending = false
            switch result {
            case .success(let log):
                self.log = log
                done(nil)
                // A message it took: its reply comes in as it works.
                if log.keepsChanging, self.polling == nil, let model { self.start(model) }
            case .failure(let f):
                done(f.message)
            }
        }
    }
}
