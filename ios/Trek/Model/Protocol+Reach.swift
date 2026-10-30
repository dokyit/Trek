import Foundation
import os

// What came with sub-agents, search and the rest of the thread menu (see docs/MOBILE.md): the
// Mac's feature list, the snooze presets, search results, and lists that read item by item.

nonisolated let protocolLog = Logger(subsystem: "dev.trek.TrekMobile", category: "protocol")

/// The optional requests a Mac may serve, as `welcome` lists them (`trek_remote::feature`). What
/// the Mac doesn't list, the phone doesn't offer (an older Mac simply doesn't show it).
nonisolated enum MacFeature: String, CaseIterable {
    case search
    /// Sub-agents' threads open from their parent.
    case subAgents = "sub_agents"
    case snooze
    case markUnread = "mark_unread"
    case delete
    case regenerateTitle = "regenerate_title"
    /// Codex's "Use reset" on the Usage card.
    case useReset = "use_reset"
    /// Change or drop a queued follow-up.
    case queueAction = "queue_action"
    /// A message's images.
    case attachment
    /// Background tasks' output, and Stop.
    case background
    /// Added agents' and projects' icons.
    case icon
    /// Basecamp as the Mac draws it now: totals, the chart of turns, the tables by project and by
    /// model, and review rows split into "Needs you" and "Needs review".
    case basecampV2 = "basecamp_v2"
    /// A note's pictures, and adding one from the phone.
    case noteImages = "note_images"
    /// A note's other files (PDFs, videos, anything) as they are, a piece at a time.
    case noteFiles = "note_files"
    /// `machine_info` (and running another Trek's threads, which the phone doesn't).
    case machines
    /// `update_phone`: the Mac builds and installs this app when asked, saying how it goes
    /// (`phone_update`), and its snapshot names the newest Trek it knows of (`release`).
    case updatePhone = "update_phone"
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
