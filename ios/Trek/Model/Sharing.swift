import Foundation
import Speech
import SwiftUI

// Asking before the first message leaves the phone. What's typed, dictated or attached here goes
// to the paired computer and from there to the agent the thread uses, which is usually a cloud AI
// service the user signed in to on that computer. The App Store's rule 5.1.2(i) wants that said,
// and a yes, before it happens: `SharingSheet` says it, `SharingConsent` keeps the yes, and
// `AppModel.request` lets nothing with the user's words in it go without one. The demo's sample
// Mac lives in the app, so nothing sent to it leaves the phone and it's never asked about.

/// Where a message ends up once the paired computer has it.
nonisolated struct SharingRecipient: Equatable, Sendable {
    /// The agent's name as the computer gives it ("Claude Code"); nil when the phone doesn't know
    /// which agent it is.
    var agent: String? = nil
    /// The company whose service that agent sends it to, when its key says so.
    var company: String? = nil
    /// A model server on the computer itself (Ollama, LM Studio, llama.cpp).
    var local = false

    /// What a yes is kept under: one per company, one for the computer's own model servers, and
    /// one for every agent the phone can't place (they all get the same general words).
    var id: String {
        if local { return Self.localID }
        return company.map { $0.lowercased() } ?? Self.otherID
    }

    static let localID = "local"
    static let otherID = "other"

    /// The recipient for an agent by its key on the computer (`AgentId::key` in trek-core:
    /// `claude-code`, `codex`, `acp:<registry id>`, `direct:<provider id>`). Only keys that say
    /// where the words go name a company; `opencode`, `droid` and most ACP agents take whichever
    /// provider the user set up there, which the phone can't see.
    static func of(key: String?, name: String?) -> SharingRecipient {
        let name = name.flatMap { $0.isEmpty ? nil : $0 }
        guard let key, !key.isEmpty else { return SharingRecipient(agent: name) }
        if ["direct:ollama", "direct:lmstudio", "direct:llamacpp"].contains(key) {
            return SharingRecipient(agent: name, local: true)
        }
        let company: String? = switch key {
        case "claude-code", "direct:anthropic": "Anthropic"
        case "codex", "direct:openai": "OpenAI"
        case "acp:gemini", "gemini", "direct:google": "Google"
        case "direct:openrouter": "OpenRouter"
        case "direct:deepseek": "DeepSeek"
        case "direct:xai": "xAI"
        case "direct:mistral": "Mistral"
        case "direct:groq": "Groq"
        default: nil
        }
        return SharingRecipient(agent: name, company: company)
    }

    static func of(_ agent: AgentRef?) -> SharingRecipient { of(key: agent?.key, name: agent?.name) }

    /// How a kept yes names it in Settings.
    static func label(id: String) -> String {
        switch id {
        case localID: "models on your computer"
        case otherID: "other agents"
        case "openai": "OpenAI"
        case "openrouter": "OpenRouter"
        case "deepseek": "DeepSeek"
        case "xai": "xAI"
        default: id.prefix(1).uppercased() + id.dropFirst()
        }
    }
}

/// The yes: which wording it was given to, for which recipients, and when.
nonisolated struct SharingConsent: Codable, Equatable, Sendable {
    /// The wording of `SharingWords`. Raise it when what the sheet says changes in substance (a
    /// new recipient, something else sent): every yes given to an earlier wording stops counting,
    /// and the phone asks again.
    static let currentVersion = 1

    var version: Int
    /// `SharingRecipient.id`s the sheet has named and the user allowed.
    var recipients: [String]
    var at: Date

    func covers(_ recipient: SharingRecipient) -> Bool {
        version == Self.currentVersion && recipients.contains(recipient.id)
    }

    /// Still a yes to today's wording, for someone.
    var stands: Bool { version == Self.currentVersion && !recipients.isEmpty }

    /// This yes with `recipient` added (starting over when it was given to an older wording).
    static func allowing(_ recipient: SharingRecipient, after old: SharingConsent?, at: Date = Date()) -> SharingConsent {
        var kept = old.flatMap { $0.version == currentVersion ? $0.recipients : nil } ?? []
        if !kept.contains(recipient.id) { kept.append(recipient.id) }
        return SharingConsent(version: currentVersion, recipients: kept, at: at)
    }

    private static let key = "sharingConsent"

    static func load(_ defaults: UserDefaults = .standard) -> SharingConsent? {
        defaults.data(forKey: key).flatMap { try? JSONDecoder().decode(SharingConsent.self, from: $0) }
    }

    func save(_ defaults: UserDefaults = .standard) {
        if let data = try? JSONEncoder().encode(self) { defaults.set(data, forKey: Self.key) }
    }

    static func clear(_ defaults: UserDefaults = .standard) {
        defaults.removeObject(forKey: key)
    }

    /// What a request hears back when the user said "Not now".
    static let notSent = "Not sent. Nothing left this iPhone."
    /// The policy the sheet and Settings link to.
    static let policy = URL(string: "https://trekdesktop.com/privacy/")!
}

/// What the sheet says, in order. Kept apart from the view so the words can be read (and tested)
/// as sentences. Changing what they promise means raising `SharingConsent.currentVersion`.
nonisolated struct SharingWords: Equatable, Sendable {
    var title: String
    /// To the paired computer.
    var computer: String
    /// On to the agent, and whose service that is.
    var agent: String
    /// Trek's own part.
    var trek: String
    /// Only when this iPhone can't recognize the user's language itself.
    var dictation: String?

    init(_ recipient: SharingRecipient, machine: String, dictationOnDevice: Bool = true) {
        let agent = recipient.agent
        title = agent.map { "Send to \($0)?" } ?? "Send to your agent?"
        computer = "What you type, dictate or attach here goes to \(machine), your paired computer."
        let who = agent ?? "the agent you chose there"
        if recipient.local {
            self.agent = "From there it goes to \(who), which runs on that computer. It stays there unless you set it up otherwise."
        } else if let company = recipient.company {
            self.agent = "From there \(who) sends it to \(company), a cloud AI service, under the account you use on that computer."
        } else {
            self.agent = "From there it goes to \(who), which may send it to a cloud AI service, depending on how it's set up on that computer."
        }
        trek = "Trek has no servers in between and keeps no copy on servers of its own."
        dictation = dictationOnDevice ? nil
            : "Dictation in your language isn't done on this iPhone: Apple turns your speech into text."
    }

    /// Whether this iPhone turns speech into text itself for the user's language. When it can't,
    /// `Dictation` lets Apple's servers do it (`requiresOnDeviceRecognition` is only set where
    /// the recognizer supports it), and the sheet says so.
    @MainActor static var dictationOnDevice: Bool {
        SFSpeechRecognizer(locale: .current)?.supportsOnDeviceRecognition ?? true
    }
}

/// The question on screen: who it's about, and what waits on the answer.
struct SharingAsk: Identifiable {
    let id = UUID()
    var recipient: SharingRecipient
    /// The paired computer's name.
    var machine: String
    /// "Allow": what was held back goes now.
    var allow: () -> Void = {}
    /// "Not now", or the sheet swiped away: nothing went.
    var declined: () -> Void = {}
}

extension ClientMessage {
    /// The thread (or, for a new one, the agent) that gets the user's own words or files with
    /// this message; nil for everything that carries none (taps, reads, settings).
    enum SharedWith: Equatable {
        case thread(String)
        case agent(key: String, project: String)
    }

    var sharedWith: SharedWith? {
        switch self {
        case .send(let tid, _, _, _, _), .sendConsult(let tid, _, _, _, _, _):
            return .thread(tid)
        case .newThread(let pid, let agent, _, _, _, _, _, _, _, _):
            return .agent(key: agent, project: pid)
        case .sideChat(let tid, let text, let images):
            return text != nil || !images.isEmpty ? .thread(tid) : nil
        case .answer(let tid, _, let response):
            switch response {
            case .approval: return nil
            case .questions: return .thread(tid)
            case .plan(_, let feedback): return (feedback ?? "").isEmpty ? nil : .thread(tid)
            }
        case .queueAction(let tid, _, _, _, let edit):
            return edit != nil ? .thread(tid) : nil
        case .subAgentAction(let tid, _, _, let text):
            return (text ?? "").isEmpty ? nil : .thread(tid)
        default:
            return nil
        }
    }
}

extension AppModel {
    /// Who gets what's sent to `tid` (qualified).
    func sharingRecipient(thread tid: String) -> SharingRecipient {
        SharingRecipient.of(thread(tid)?.agent)
    }

    /// Who gets what's sent with `message`, or nil when it carries nothing of the user's.
    func sharingRecipient(of message: ClientMessage, on link: HostLink?) -> SharingRecipient? {
        switch message.sharedWith {
        case .thread(let tid)?:
            return sharingRecipient(thread: tid)
        case .agent(let key, _)?:
            return SharingRecipient.of(key: key, name: link?.agents.first { $0.key == key }?.name)
        case nil:
            return nil
        }
    }

    /// Whether `link` is the demo's sample machine: it runs inside the app, so what's sent to it
    /// goes nowhere.
    static func staysOnPhone(_ link: HostLink?) -> Bool { link?.backend is MockHost }

    /// Whether something for `recipient` may go to `link` without asking (again).
    func sharingAllowed(_ recipient: SharingRecipient, on link: HostLink?) -> Bool {
        Self.staysOnPhone(link) || sharingConsent?.covers(recipient) == true
    }

    /// True when what's about to be sent may go. Otherwise the sheet asks, and nothing has gone:
    /// `allow` runs after a yes (send it then), `declined` after "Not now".
    func mayShare(with recipient: SharingRecipient, on link: HostLink?,
                  then allow: @escaping () -> Void, declined: @escaping () -> Void = {}) -> Bool {
        if sharingAllowed(recipient, on: link) { return true }
        // One question at a time: whatever waited on an earlier one didn't go.
        if let old = sharingAsk {
            sharingAsk = nil
            old.declined()
        }
        sharingAsk = SharingAsk(recipient: recipient, machine: link?.name ?? "your computer", allow: allow, declined: declined)
        return false
    }

    /// The sheet's two buttons.
    func answerSharing(_ ask: SharingAsk, allow: Bool) {
        guard sharingAsk?.id == ask.id else { return }
        sharingAsk = nil
        guard allow else { return ask.declined() }
        let consent = SharingConsent.allowing(ask.recipient, after: sharingConsent)
        consent.save(sharingDefaults)
        sharingConsent = consent
        ask.allow()
    }

    /// The sheet went away without an answer (swiped down): that's "Not now".
    func sharingDismissed() {
        guard let ask = sharingAsk else { return }
        sharingAsk = nil
        ask.declined()
    }

    /// Settings › Privacy: the yes taken back. The next message asks again.
    func withdrawSharing() {
        SharingConsent.clear(sharingDefaults)
        sharingConsent = nil
    }

    /// Settings' line about it: "Allowed 19 Nov 2026: Anthropic, OpenAI".
    var sharingLine: String {
        guard let consent = sharingConsent, consent.stands else {
            return "Trek asks before your first message goes to an agent"
        }
        let who = consent.recipients.map(SharingRecipient.label(id:)).joined(separator: ", ")
        return "Allowed \(consent.at.formatted(.dateTime.day().month(.abbreviated).year())): \(who)"
    }
}
