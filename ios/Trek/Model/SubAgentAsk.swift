import Foundation

/// What a thread pins over its composer for a sub-agent that waits on the user. As on the Mac
/// (`ThreadView::live_footer`), the sub-agent's approval, question or plan shows where the
/// thread's own would be, under a line that names the sub-agent, and is answered there: the
/// answer goes to the sub-agent's thread. The row names the sub-agent (`needs_thread`); its
/// request comes from its own transcript, which the parent listens to while it shows.
nonisolated enum SubAgentAsk: Equatable {
    /// Nothing of a sub-agent's: none waits, or the thread has requests of its own (those come
    /// first, as on the Mac).
    case none
    /// One waits, and what it asks isn't here yet (its transcript is on its way, or its answer
    /// is on its way to the Mac): nothing, rather than a card swapped for another a moment later.
    case waiting
    /// Its requests, answered from here.
    case cards([TItem])
    /// It waits on something that isn't answered from here (a secret, typed on the Mac; a Mac
    /// that says no more than that it waits): the card that opens its thread.
    case open

    /// - Parameters:
    ///   - kid: the sub-agent the row says is waiting (`needsThread`).
    ///   - own: the thread's own requests, answered here or not.
    ///   - kidPending: the sub-agent's requests, as its transcript has them.
    ///   - answered: requests answered here and not yet closed by the Mac (`key`).
    ///   - kidLoaded: its transcript is in.
    ///   - waited: it has been long enough that "nothing yet" won't do.
    ///   - settling: one of its requests was answered here a moment ago, and the row hasn't caught up.
    static func of(kid: String?, own: [TItem], kidPending: [TItem], answered: Set<String>, kidLoaded: Bool, waited: Bool,
                   settling: Bool) -> SubAgentAsk {
        guard let kid, own.isEmpty else { return .none }
        let open = kidPending.filter { !answered.contains(key($0.id, in: kid)) }
        if !open.isEmpty { return .cards(open) }
        // Answered here, every one: the row follows once the Mac has them.
        if !kidPending.isEmpty || settling { return .waiting }
        return kidLoaded || waited ? .open : .waiting
    }

    /// Something of the sub-agent's is pinned.
    var pins: Bool {
        switch self {
        case .cards, .open: true
        case .none, .waiting: false
        }
    }

    /// How a request answered here is remembered until the Mac closes it. With its thread for a
    /// sub-agent's: two threads number their requests alike (`r3` in both).
    static func key(_ item: String, in kid: String?) -> String {
        kid.map { "\($0)/\(item)" } ?? item
    }

    /// Who listens to a sub-agent's transcript on behalf of thread `parent` (`AppModel.subscribe`).
    static func holder(_ parent: String) -> String { "ask:\(parent)" }
}
