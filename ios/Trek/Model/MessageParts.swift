import Foundation

/// A user message as the Mac's transcript shows it: what the user wrote, without what Trek added
/// for the agent (a consult block, a request to restate it first, the editor side bar's context),
/// and a line for each of those under the bubble. Mirrors `orchestrate::split_consult`,
/// `restate::split_restate` and `ide::ai::context::split`.
nonisolated struct MessageParts: Equatable {
    /// What the user wrote.
    var said: String
    var consult: Consult?
    /// It asked the agent to restate it first ("Asked to restate it first").
    var restate = false
    /// Sent from the editor's side bar in Ask mode.
    var ask = false
    /// The context it carried from the editor, as chips' labels ("main.rs", "Terminal").
    var context: [String] = []

    /// One model asked for advice: `codex/gpt-5.6-sol/high`.
    struct Consultant: Equatable {
        var agent: String
        var model: String
        var effort: String

        init?(key: String) {
            let key = key.trimmingCharacters(in: .whitespaces)
            // Agent keys have no slash; model ids may.
            guard let slash = key.firstIndex(of: "/") else { return nil }
            let rest = key[key.index(after: slash)...]
            guard let last = rest.lastIndex(of: "/") else { return nil }
            agent = String(key[..<slash])
            model = String(rest[..<last])
            effort = String(rest[rest.index(after: last)...]).lowercased()
            guard !agent.isEmpty, !model.isEmpty,
                  ["off", "minimal", "low", "medium", "high", "xhigh", "max"].contains(effort) else { return nil }
        }
    }

    enum Style: Equatable { case advise, discuss, arena }

    struct Consult: Equatable {
        var consultants: [Consultant]
        var style: Style
        /// Then implement (else just report).
        var implement: Bool
        var judge: Consultant?

        /// The agents' keys whose logos lead the line: the consultants, then the judge.
        var agents: [String] { (consultants + (judge.map { [$0] } ?? [])).map(\.agent) }

        /// "Consulting Sol · High, Opus 5.5 · Max · advice, then implement", as the Mac words it.
        /// `name` names a consultant's model (its id when it isn't known).
        func line(name: (Consultant) -> String) -> String {
            let who = consultants.map { "\(name($0)) · \(Effort.label($0.effort))" }.joined(separator: ", ")
            let then = implement ? "then implement" : "then report"
            switch style {
            case .advise: return "Consulting \(who) · \(implement ? "advice, then implement" : "advice only")"
            case .discuss: return "Consulting \(who) · discuss, \(then)"
            case .arena:
                if let judge { return "Design arena: \(who) · judged by \(name(judge)) · \(Effort.label(judge.effort)) · \(then)" }
                return "Design arena: \(who) · \(then)"
            }
        }
    }

    static let consultOpen = "<trek-consult"
    static let consultClose = "</trek-consult>"
    static let restateOpen = "<trek-restate>"
    static let restateClose = "</trek-restate>"
    static let contextOpen = "<trek-context>"
    static let contextClose = "</trek-context>"
    static let askOpen = "<trek-ask>"
    static let askClose = "</trek-ask>"

    private nonisolated(unsafe) static var cache: [String: MessageParts] = [:]
    private static let lock = NSLock()

    /// The parts of `text` (parsed once per text: a transcript redraws its bubbles often).
    static func of(_ text: String) -> MessageParts {
        // Nothing Trek adds: the whole message is what was said (the common case).
        guard text.contains("\n\n<trek-") else { return MessageParts(said: text) }
        lock.lock()
        defer { lock.unlock() }
        if let hit = cache[text] { return hit }
        let parts = parse(text)
        if cache.count > 500 { cache.removeAll() }
        cache[text] = parts
        return parts
    }

    static func parse(_ text: String) -> MessageParts {
        var parts = MessageParts(said: text)
        var rest = Substring(text)
        // In the order the Mac takes them off: the consult, then the restate, then the side bar's.
        if let (before, consult) = splitConsult(rest) {
            rest = before
            parts.consult = consult
        }
        if let before = splitBlock(rest, open: restateOpen, close: restateClose) {
            rest = before
            parts.restate = true
        }
        if let before = splitBlock(rest, open: askOpen, close: askClose) {
            rest = before
            parts.ask = true
        }
        if let range = rest.range(of: "\n\n\(contextOpen)\n", options: .backwards),
           rest.trimmingCharacters(in: .whitespacesAndNewlines).hasSuffix(contextClose) {
            parts.context = contextLabels(rest[range.upperBound...])
            rest = rest[..<range.lowerBound]
        }
        parts.said = String(rest).trimmingTrailingWhitespace()
        return parts
    }

    /// The text before a trailing `\n\n<open>…<close>` block, or nil when it has none.
    private static func splitBlock(_ text: Substring, open: String, close: String) -> Substring? {
        guard let at = text.range(of: "\n\n\(open)", options: .backwards),
              text[at.lowerBound...].trimmingCharacters(in: .whitespacesAndNewlines).hasSuffix(close) else { return nil }
        return Substring(String(text[..<at.lowerBound]).trimmingTrailingWhitespace())
    }

    private static func splitConsult(_ text: Substring) -> (Substring, Consult)? {
        guard let at = text.range(of: "\n\n\(consultOpen)", options: .backwards) else { return nil }
        let block = text[text.index(at.lowerBound, offsetBy: 2)...]
        guard block.trimmingCharacters(in: .whitespacesAndNewlines).hasSuffix(consultClose) else { return nil }
        let head = block.prefix { $0 != "\n" }
        func attr(_ name: String) -> String? {
            guard let start = head.range(of: "\(name)=\"") else { return nil }
            let after = head[start.upperBound...]
            guard let end = after.firstIndex(of: "\"") else { return nil }
            return String(after[..<end])
        }
        let consultants = (attr("with") ?? "").split(separator: ",").compactMap { Consultant(key: String($0)) }
        guard !consultants.isEmpty else { return nil }
        let style: Style = switch attr("style") {
        case "discuss": .discuss
        case "arena": .arena
        default: .advise
        }
        let consult = Consult(consultants: consultants, style: style, implement: attr("then") != "report",
                              judge: style == .arena ? attr("judge").flatMap(Consultant.init(key:)) : nil)
        return (Substring(String(text[..<at.lowerBound]).trimmingTrailingWhitespace()), consult)
    }

    /// The side bar's context block's items, by label: `@path` and `path:a-b` as the file's name
    /// (with its lines), the terminal as "Terminal".
    private static func contextLabels(_ block: Substring) -> [String] {
        var labels: [String] = []
        var fence: String?
        for line in block.split(separator: "\n", omittingEmptySubsequences: false) {
            if let f = fence {
                if line.trimmingCharacters(in: .whitespaces) == f { fence = nil }
                continue
            }
            if line.hasPrefix("``") {
                fence = String(line.prefix { $0 == "`" })
                continue
            }
            if line.hasPrefix(contextClose) { break }
            if line.hasPrefix("@") {
                labels.append(String(line.dropFirst().split(separator: "/").last ?? ""))
            } else if line.hasPrefix("Terminal") {
                labels.append("Terminal")
            } else if let colon = line.lastIndex(of: ":") {
                let ends = line[line.index(after: colon)...].split(separator: "-")
                if ends.count == 2, let a = Int(ends[0]), let b = Int(ends[1]) {
                    let name = line[..<colon].split(separator: "/").last.map(String.init) ?? ""
                    labels.append(a == b ? "\(name):\(a)" : "\(name):\(a)–\(b)")
                }
            }
        }
        return labels
    }
}

/// A wake-up message: what Trek tells the agent when its sub-agents or the threads it started
/// report back (`orchestrate::wake_text`). The transcript folds it to one line, as the Mac does.
nonisolated enum WakeNote {
    static let prefix = "[Trek] "

    static func isWake(_ text: String) -> Bool { text.hasPrefix(prefix) }

    /// "Sol reported back on “Review the cache”" (`orchestrate::wake_summary`).
    static func summary(_ text: String) -> String {
        let n = text.components(separatedBy: prefix).count - 1
        let body: Substring = text.hasPrefix(prefix) ? text.dropFirst(prefix.count) : ""
        func title(_ s: Substring, fallback: String) -> String {
            guard let close = s.firstIndex(of: "”") else { return fallback }
            return String(s[..<close]).trimmingCharacters(in: CharacterSet(charactersIn: "“"))
        }
        if body.hasPrefix("Thread ") {
            let first = body.dropFirst("Thread ".count)
            let how = first.contains(") failed: ") ? "failed"
                : first.contains(") is waiting for the user's approval") ? "needs your approval" : "finished"
            return n <= 1 ? "Thread “\(title(first, fallback: "a thread"))” \(how)" : "\(n) threads and sub-agents reported back"
        }
        let first: Substring = body.hasPrefix("Sub-agent ") ? body.dropFirst("Sub-agent ".count) : ""
        var model = "A sub-agent"
        if let open = first.range(of: "” ("), let comma = first[open.upperBound...].firstIndex(of: ",") {
            model = String(first[open.upperBound..<comma])
        }
        let verb = first.contains(" failed: ") ? "failed on" : first.contains(" was stopped ") ? "was stopped on" : "reported back on"
        return n <= 1 ? "\(model) \(verb) “\(title(first, fallback: "a task"))”" : "\(n) sub-agents reported back"
    }

    /// Any of its reports is a failure.
    static func failed(_ text: String) -> Bool { text.contains(" failed: ") }
}

extension String {
    nonisolated func trimmingTrailingWhitespace() -> String {
        var s = Substring(self)
        while let last = s.last, last.isWhitespace { s.removeLast() }
        return String(s)
    }
}
