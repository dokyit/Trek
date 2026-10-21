import Foundation
import SwiftUI

/// Sub-agents, the rest of the thread menu (snooze, wake, mark unread, regenerate the title,
/// delete) and search on the Mac. Each is offered only where the Mac lists it (`features`).
extension AppModel {

    /// A row the list leaves out came (or changed): kept by id, and its open thread follows it.
    func setUnlisted(_ t: ThreadSummary) {
        guard unlisted[t.id] != t else { return }
        unlisted[t.id] = t
        if !threads.contains(where: { $0.id == t.id }) { existingStore(t.id)?.setSummary(t) }
    }

    /// Snoozed threads, waking soonest first.
    var snoozed: [ThreadSummary] {
        threads.filter { $0.section == .snoozed }.sorted { ($0.snoozedUntil ?? .max) < ($1.snoozedUntil ?? .max) }
    }

    /// `tid`'s sub-agents with threads of their own, newest first.
    func subAgentThreads(of tid: String) -> [ThreadSummary] {
        unlisted.values.filter { $0.parentId == tid }.sorted { $0.updatedAt > $1.updatedAt }
    }

    // MARK: Thread menu

    func snooze(_ tid: String, _ preset: SnoozePreset) {
        threadAction(tid, .snooze(until: nil, preset: preset), done: preset == .hour || preset == .threeHours
            ? "Snoozed \(preset.label.lowercased())" : "Snoozed \(preset.label.replacingOccurrences(of: "Until ", with: "until "))")
    }

    func snooze(_ tid: String, until date: Date) {
        threadAction(tid, .snooze(until: Int64(date.timeIntervalSince1970 * 1000), preset: nil),
                     done: "Snoozed until \(date.formatted(.dateTime.weekday().hour().minute()))")
    }

    func wake(_ tid: String) {
        threadAction(tid, .wake, done: "Back in the inbox")
    }

    /// Unread again: here at once, and on the Mac.
    func markUnread(_ tid: String) {
        if let i = threads.firstIndex(where: { $0.id == tid }), !threads[i].unseen { threads[i].unseen = true }
        threadAction(tid, .markUnread)
    }

    func regenerateTitle(_ tid: String) {
        threadAction(tid, .regenerateTitle, done: "Writing a new title…")
    }

    /// Delete a thread. The Mac asks first: `confirm` gets its words (what goes with it), and the
    /// thread goes only once the user agrees (`confirmed`); `deleted` runs then.
    func deleteThread(_ tid: String, confirmed: Bool = false, confirm: @escaping (String) -> Void, deleted: @escaping () -> Void) {
        request(.threadAction(threadId: tid, action: .delete(confirmed: confirmed))) { [weak self] reply in
            switch reply {
            case .error(_, .conflict, let text) where !confirmed: confirm(text)
            case .error(_, _, let text): self?.show(text, error: true)
            default:
                self?.show("Deleted")
                deleted()
            }
        }
    }

    // MARK: Search

    /// A host's search for `query` (the selected one's without `link`): `done` gets the threads
    /// that match, qualified (rows that aren't listed are kept, so they open), or nil on failure.
    func search(_ query: String, on link: HostLink? = nil, done: @escaping ([SearchResult]?) -> Void) {
        guard let link = link ?? selected else { return done(nil) }
        link.request(.search(query: query, limit: 60)) { [weak self] reply in
            guard let self else { return done(nil) }
            switch reply {
            case .searchResults(_, let found):
                let results = found.results.map { r in
                    var r = r
                    r.thread = self.qualify(r.thread, on: link)
                    return r
                }
                for r in results where !self.threads.contains(where: { $0.id == r.thread.id }) { self.setUnlisted(r.thread) }
                done(results)
            default: done(nil)
            }
        }
    }
}

/// Opens a thread on top of the one showing (a sub-agent from its parent), in whichever list the
/// user is in.
struct OpenThreadAction {
    var open: (String) -> Void
    func callAsFunction(_ id: String) { open(id) }
}

extension EnvironmentValues {
    @Entry var openThread: OpenThreadAction? = nil
}
