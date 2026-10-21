import Foundation
import Observation
import SwiftUI

/// One machine the phone is paired with: its connection and everything it alone serves
/// (threads, notes, basecamp, settings). The phone holds one link per pairing; thread and
/// project ids it reports are prefixed with its host id (`"host:thread"`) wherever they enter
/// shared state, so several hosts' threads can sit in one list without colliding.
@Observable
final class HostLink: Identifiable, Equatable {
    static func == (a: HostLink, b: HostLink) -> Bool { a === b }

    var paired: PairedMac
    let backend: Backend

    var id: String { paired.hostId }
    /// Its name as the user knows it: what it said on connect, else what pairing said.
    var name: String { host?.name ?? paired.hostName }

    var host: HostInfo?
    var connection: ConnectionState = .idle
    /// What the connection is doing, in words, while it's connecting, connected or offline.
    var connectionDetail: String?
    /// The optional requests this host serves (`MacFeature`).
    var features: Set<String> = []
    /// Its projects and agents (bare ids), for the new-thread sheet and this host's pickers.
    var projects: [ProjectSummary] = []
    var agents: [AgentOption] = []
    /// It lets threads run with Full access (unlocked in its Permissions settings).
    var fullAccessAllowed = false

    /// How the phone reaches it (pinned TLS or plain `ws://`), as the client has it now — it can
    /// tighten from a prefix pin to the full fingerprint after pairing.
    var transport: Transport { (backend as? TrekClient)?.transport ?? paired.transport }

    // MARK: Host-scoped surfaces (usage, basecamp, notes, its settings)

    var usage: Usage?
    var usageLoading = false
    var basecamp: [BasecampRange: Basecamp] = [:]
    var basecampLoading: Set<BasecampRange> = []
    @ObservationIgnored var basecampAsked: Set<BasecampRange> = []
    var notes: [NoteSummary] = []
    var openNotes: [String: Note] = [:]
    @ObservationIgnored var notesAsked = false
    @ObservationIgnored var notesSaving: Set<String> = []
    @ObservationIgnored var notesToSave: [String: String] = [:]
    var macSettings: MacSettings?

    // MARK: Requests and replies (request ids are per connection)

    private var nextRequest = 1
    private var replies: [String: (ServerMessage) -> Void] = [:]

    init(paired: PairedMac, backend: Backend) {
        self.paired = paired
        self.backend = backend
    }

    /// The id shared state uses for this host's `bare` thread or project id.
    func qualified(_ bare: String) -> String { "\(id):\(bare)" }

    /// Whether `qid` is this host's (already prefixed), for ids that reach a call site either way.
    func owns(_ qid: String) -> Bool { qid.hasPrefix("\(id):") }

    /// `qid` as this host knows it: the prefix off when present.
    func bare(_ qid: String) -> String { owns(qid) ? String(qid.dropFirst(id.count + 1)) : qid }

    /// Whether it serves `feature` (`AppModel.serves(_:on:)` is the same by qualified id).
    func serves(_ feature: MacFeature) -> Bool { features.contains(feature.rawValue) }

    /// The models `agentKey` offers here, and the efforts `modelId` takes.
    func models(of agentKey: String) -> [ModelOption] { agents.first { $0.key == agentKey }?.models ?? [] }
    func efforts(agent agentKey: String, model modelId: String? = nil) -> [String] {
        let models = models(of: agentKey)
        let id = modelId ?? agents.first { $0.key == agentKey }?.defaultModel
        return models.first { $0.id == id }?.efforts ?? []
    }

    @discardableResult
    func request(_ message: ClientMessage, then: ((ServerMessage) -> Void)? = nil) -> String {
        let id = "\(nextRequest)"
        nextRequest += 1
        if let then { replies[id] = then }
        // Not connected: nothing went, and whoever waits for the answer hears so now.
        if backend.send(message, id: id) != true { reply(id, .error(re: id, code: .unknown, message: AppModel.notConnected)) }
        return id
    }

    /// Failures of an action become a toast; `ok` runs on success.
    func act(_ message: ClientMessage, show: @escaping (String, Bool) -> Void, ok: ((ServerMessage) -> Void)? = nil) {
        request(message) { reply in
            if case .error(_, _, let text) = reply { show(text, true) } else { ok?(reply) }
        }
    }

    func hasReply(_ re: String) -> Bool { replies[re] != nil }

    func reply(_ re: String?, _ message: ServerMessage) {
        guard let re, let f = replies.removeValue(forKey: re) else { return }
        f(message)
    }

    /// The connection went: the answers still awaited aren't coming (a request isn't sent again
    /// by itself, the host may have done it already), so what waits on them stops waiting.
    func failPending(_ why: String = "The connection dropped. Check that it went through before trying again.") {
        let waiting = replies
        replies.removeAll()
        for (id, f) in waiting { f(.error(re: id, code: .unknown, message: why)) }
    }

    /// The connection in words: what it's doing while connecting, connected or offline, else
    /// why it stopped.
    var connectionText: String {
        switch connection {
        case .connecting, .connected, .offline: connectionDetail ?? connection.label
        case .idle, .unauthorized, .identityChanged: connection.label
        }
    }
}
