import Foundation

/// When the Mac is told a thread has been seen (`mark_seen`).
///
/// The Mac counts a thread unread when it changed after it was last seen, and a turn that ends
/// counts as a change unless the thread is on the Mac's own screen. Telling it only when a thread
/// opens left a thread being read on the phone unread the moment its turn ended. So it's told
/// again while the thread's screen is the one showing and the app is in front: when its row says
/// unread, when its turn ends, when items come in. At most once every `gap` for a thread (each
/// one is a write on the Mac), with one more at the end of the gap for what happened meanwhile.
/// Never for a thread under another's screen, nor while the app is away: nobody is reading it.
///
/// A plain value: the model asks it what to do and keeps the timer.
nonisolated struct SeenPolicy {
    static let gap: TimeInterval = 2

    enum Verdict: Equatable {
        /// Tell the Mac now.
        case send
        /// Told a moment ago: ask again (`due`) after this long.
        case wait(TimeInterval)
        /// Nothing to do (nobody is looking, or a wait is already out).
        case no
    }

    /// When each thread was last marked seen.
    private var sent: [String: TimeInterval] = [:]
    /// Threads with a wait out.
    private var waiting: Set<String> = []

    /// Something happened in `thread` that the Mac may count as unread. `showing`: its screen is
    /// the one in view. `active`: the app is in front.
    mutating func note(_ thread: String, showing: Bool, active: Bool, now: TimeInterval) -> Verdict {
        guard showing, active else { return .no }
        if let last = sent[thread], now - last < Self.gap {
            guard waiting.insert(thread).inserted else { return .no }
            return .wait(Self.gap - (now - last))
        }
        sent[thread] = now
        return .send
    }

    /// The wait for `thread` is over: tell the Mac, if it's still being read.
    mutating func due(_ thread: String, showing: Bool, active: Bool, now: TimeInterval) -> Verdict {
        guard waiting.remove(thread) != nil, showing, active else { return .no }
        sent[thread] = now
        return .send
    }

    /// The thread is gone: nothing of it is kept.
    mutating func forget(_ thread: String) {
        sent[thread] = nil
        waiting.remove(thread)
    }
}
