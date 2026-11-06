import ActivityKit
import AppIntents
import Foundation

// Trek's Live Activity, as the app (which starts and updates it) and its widget extension (which
// draws it) both know it. This folder is compiled into both.

/// One activity for the whole phone: the thread most worth a glance, and how many others there are.
nonisolated struct TurnActivityAttributes: ActivityAttributes {
    nonisolated struct ContentState: Codable, Hashable, Sendable {
        nonisolated enum Kind: String, Codable, Sendable {
            /// A turn is running.
            case working
            /// Its turn is over, but what it started is still at it.
            case waiting
            /// It can't go on without the user: an approval, a question, a plan.
            case asking
            /// Nothing is left at work: the words it ends on.
            case finished
        }

        var kind: Kind
        var title: String
        var project: String?
        var agent: String
        /// What it's doing, or what it's asking: "Running cargo test", "Run cargo test?".
        var line: String
        /// When its turn started, to tick from.
        var since: Date?
        /// The thread as shared state knows it (`host:thread`), and the two parts of that, for
        /// the link that opens it.
        var threadID: String
        var hostID: String
        var bareID: String
        /// How many threads wait on the user, and how many are at work, all told.
        var waitingOnYou: Int
        var atWork: Int
        /// Threads whose turn is over with a command of theirs still running.
        var inBackground: Int
        /// The approval to answer from here. Nil for a question or a plan, a sub-agent's request,
        /// one that looks destructive, or a Mac too old to say which request it is.
        var requestID: String?
        /// Why it can't be answered from here: "deletes files".
        var warning: String?
        /// When the app last had this from the Mac: past `staleDate` the card says so.
        var heard: Date

        /// The same to look at (`heard` moves on by itself).
        func looksLike(_ other: ContentState) -> Bool {
            var a = self
            a.heard = other.heard
            return a == other
        }

        /// What else is going on, in a line. Empty when this thread is all there is.
        var others: String {
            var parts: [String] = []
            let waiting = waitingOnYou - (kind == .asking ? 1 : 0)
            if waiting > 0 { parts.append(kind == .asking ? "\(waiting) more waiting on you" : "\(waiting) waiting on you") }
            let working = atWork - (kind == .working || kind == .waiting ? 1 : 0)
            if working > 0 { parts.append(kind == .working || kind == .waiting ? "\(working) more at work" : "\(working) at work") }
            if inBackground > 0 { parts.append("\(inBackground) in the background") }
            return parts.joined(separator: " · ")
        }

        /// Opens its thread (`trek://open`, as a notification's link does).
        var link: URL? {
            var c = URLComponents()
            c.scheme = "trek"
            c.host = "open"
            c.queryItems = [URLQueryItem(name: "thread", value: bareID), URLQueryItem(name: "host", value: hostID)]
            return c.url
        }
    }
}

/// Allow or Deny on the Lock Screen or in the Dynamic Island. It runs in the app, which iOS starts
/// in the background if it has to. A locked phone asks for Face ID first: whoever holds the phone
/// doesn't get to run commands on the Mac.
struct AnswerAskIntent: LiveActivityIntent {
    nonisolated static let title: LocalizedStringResource = "Answer the agent"
    nonisolated static let description = IntentDescription("Allow or deny what an agent is asking to do on your Mac.")
    nonisolated static let isDiscoverable = false
    nonisolated static var authenticationPolicy: IntentAuthenticationPolicy { .requiresAuthentication }

    @Parameter(title: "Thread") var thread: String
    @Parameter(title: "Request") var request: String
    @Parameter(title: "Allow") var allow: Bool

    init() {}

    init(thread: String, request: String, allow: Bool) {
        self.thread = thread
        self.request = request
        self.allow = allow
    }

    @MainActor
    func perform() async throws -> some IntentResult {
        await TurnActivityAnswers.handler?(thread, request, allow)
        return .result()
    }
}

/// What answers `AnswerAskIntent`: the app sets it as it starts; the extension, which only draws
/// the button, never does.
@MainActor
enum TurnActivityAnswers {
    static var handler: ((_ thread: String, _ request: String, _ allow: Bool) async -> Void)?
}
