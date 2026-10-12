import Foundation
import os

// What came with sub-agents, search and the rest of the thread menu (see docs/MOBILE.md): the
// Mac's feature list, the snooze presets, search results, and lists that read item by item.

nonisolated let protocolLog = Logger(subsystem: "dev.trek.TrekMobile", category: "protocol")

/// The optional requests a Mac may serve, as `welcome` lists them. What the Mac doesn't list,
/// the phone doesn't offer (an older Mac simply doesn't show it).
nonisolated enum MacFeature {
    static let search = "search"
    /// Sub-agents' threads open from their parent.
    static let subAgents = "sub_agents"
    static let snooze = "snooze"
    static let markUnread = "mark_unread"
    static let delete = "delete"
    static let regenerateTitle = "regenerate_title"
}

/// The Mac's snooze menu.
nonisolated enum SnoozePreset: String, CaseIterable, Identifiable {
    case hour, threeHours = "three_hours", tomorrow, nextWeek = "next_week"
    var id: String { rawValue }
    var label: String {
        switch self {
        case .hour: "For 1 hour"
        case .threeHours: "For 3 hours"
        case .tomorrow: "Until tomorrow morning"
        case .nextWeek: "Until next week"
        }
    }
}

/// An array read element by element: one that doesn't decode is left out (and logged) instead of
/// the whole message.
nonisolated struct Lossy<Element: Decodable>: Decodable {
    var elements: [Element]

    init(_ elements: [Element]) { self.elements = elements }

    init(from decoder: Decoder) throws {
        var c = try decoder.unkeyedContainer()
        var out: [Element] = []
        while !c.isAtEnd {
            do {
                out.append(try c.decode(Element.self))
            } catch {
                protocolLog.error("Left out a \(String(describing: Element.self), privacy: .public) that didn't decode: \(String(describing: error), privacy: .public)")
                // Past the one that failed.
                _ = try? c.decode(Skipped.self)
            }
        }
        elements = out
    }

    private struct Skipped: Decodable {
        init(from decoder: Decoder) {}
    }
}

/// The reply to `search`: title matches first, then message matches.
nonisolated struct SearchResults: Decodable {
    var query: String
    var results: [SearchResult]

    private enum CodingKeys: String, CodingKey { case query, results }

    init(query: String, results: [SearchResult]) {
        (self.query, self.results) = (query, results)
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        query = try c.decodeIfPresent(String.self, forKey: .query) ?? ""
        results = (try c.decodeIfPresent(Lossy<SearchResult>.self, forKey: .results))?.elements ?? []
    }
}

/// A thread that matched, with its whole row (it may not be in the list).
nonisolated struct SearchResult: Codable, Hashable, Identifiable {
    var thread: ThreadSummary
    /// For a message match: an excerpt, on one line.
    var snippet: String? = nil
    /// The matched words in `snippet`, `[start, end)` in characters.
    var ranges: [[Int]]? = nil
    var id: String { thread.id }
}
