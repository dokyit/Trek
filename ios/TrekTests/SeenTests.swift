import Foundation
import Testing
@testable import Trek

/// When the Mac is told a thread has been seen: while it's being read, not more often than every
/// couple of seconds, and never for a thread nobody is looking at.
@MainActor
struct SeenPolicyTests {
    @Test func aThreadBeingReadIsMarkedAtOnce() {
        var policy = SeenPolicy()
        #expect(policy.note("mac:t", showing: true, active: true, now: 100) == .send)
    }

    @Test func notWhileTheAppIsAwayNorUnderAnotherThread() {
        var policy = SeenPolicy()
        #expect(policy.note("mac:t", showing: true, active: false, now: 100) == .no, "in the background")
        #expect(policy.note("mac:t", showing: false, active: true, now: 100) == .no, "another thread's screen is over it")
        #expect(policy.note("mac:t", showing: false, active: false, now: 100) == .no)
        // Nothing was sent, so nothing holds the next one back.
        #expect(policy.note("mac:t", showing: true, active: true, now: 100.5) == .send)
    }

    @Test func atMostOneEveryCoupleOfSeconds() {
        var policy = SeenPolicy()
        #expect(policy.note("mac:t", showing: true, active: true, now: 100) == .send)
        // Items keep coming: one wait is set, and the rest ride on it.
        #expect(policy.note("mac:t", showing: true, active: true, now: 100.5) == .wait(1.5))
        #expect(policy.note("mac:t", showing: true, active: true, now: 101) == .no)
        #expect(policy.note("mac:t", showing: true, active: true, now: 101.9) == .no)
        // The wait is over: once, for all of them.
        #expect(policy.due("mac:t", showing: true, active: true, now: 102) == .send)
        #expect(policy.due("mac:t", showing: true, active: true, now: 102) == .no, "only the wait that was set")
        // And the gap starts again from there.
        #expect(policy.note("mac:t", showing: true, active: true, now: 103) == .wait(1))
        // Another thread has its own clock.
        #expect(policy.note("mac:other", showing: true, active: true, now: 103) == .send)
        // Long after: at once again.
        #expect(policy.due("mac:t", showing: true, active: true, now: 104) == .send)
        #expect(policy.note("mac:t", showing: true, active: true, now: 110) == .send)
    }

    @Test func aWaitThatEndsAfterTheThreadWasLeftSendsNothing() {
        var policy = SeenPolicy()
        #expect(policy.note("mac:t", showing: true, active: true, now: 100) == .send)
        #expect(policy.note("mac:t", showing: true, active: true, now: 101) == .wait(1))
        #expect(policy.due("mac:t", showing: false, active: true, now: 102) == .no)
        // Opened again later: nothing is left waiting, it's marked at once.
        #expect(policy.note("mac:t", showing: true, active: true, now: 105) == .send)
    }
}

/// The model's side of it: which things that happen to a thread tell the Mac it's seen.
@MainActor
struct SeenWhileReadingTests {
    /// A model on a clock the test moves, with its timers kept to run by hand.
    private final class Clock {
        var now: TimeInterval = 1000
        var timers: [(at: TimeInterval, work: @MainActor () -> Void)] = []

        /// Moves time on and runs what came due.
        @MainActor func advance(_ seconds: TimeInterval) {
            now += seconds
            let due = timers.filter { $0.at <= now }
            timers.removeAll { $0.at <= now }
            for t in due { t.work() }
        }
    }

    private func row(_ id: String, _ state: RunState, unseen: Bool = false) -> ThreadSummary {
        ThreadSummary(id: id, title: "t", project: nil, agent: AgentRef(key: "codex", name: "Codex"), model: nil, modelLabel: nil,
                      runState: state, needs: nil, section: .inbox, unseen: unseen, pinned: false, branch: nil, worktree: false,
                      activity: nil, workingSince: nil, updatedAt: 0, additions: 0, deletions: 0)
    }

    private func setUp() -> (AppModel, HostLink, RecordingBackend, Clock) {
        let backend = RecordingBackend()
        let link = HostLink(paired: .demo, backend: backend)
        link.connection = .connected
        link.after = { _, _ in }   // no reply timers: the Mac here never answers a subscribe
        let model = AppModel(drafts: DraftStore(url: nil))
        let clock = Clock()
        model.now = { clock.now }
        model.after = { delay, work in clock.timers.append((clock.now + delay, work)) }
        model.links = [link]
        model.handle(.snapshot(Snapshot(threads: [row("t", .working), row("other", .working)], projects: [], agents: [])), from: link)
        return (model, link, backend, clock)
    }

    private func seen(_ backend: RecordingBackend) -> [String] {
        backend.sent.compactMap { if case .markSeen(let tid) = $0.message { tid } else { nil } }
    }

    /// The bug: a thread open on the phone turned unread when its turn ended, because the Mac
    /// was told it was seen only when it was opened.
    @Test func aThreadBeingReadIsMarkedSeenAgainWhenItsTurnEnds() {
        let (model, link, backend, clock) = setUp()
        model.subscribe(link.qualified("t"))
        #expect(seen(backend) == ["t"], "once, as it opens")

        // Minutes later its turn ends: the Mac's row says idle, and unread.
        clock.advance(120)
        model.handle(.thread(row("t", .idle, unseen: true)), from: link)
        #expect(seen(backend) == ["t", "t"])
        #expect(model.thread(link.qualified("t"))?.unseen == false, "and it doesn't read as unread here meanwhile")
    }

    @Test func aTurnThatEndsInAThreadNobodyIsReadingStaysUnread() {
        let (model, link, backend, clock) = setUp()
        // Another thread's screen is the one showing.
        model.subscribe(link.qualified("other"))
        clock.advance(10)
        model.handle(.thread(row("t", .idle, unseen: true)), from: link)
        #expect(seen(backend) == ["other"])
        #expect(model.thread(link.qualified("t"))?.unseen == true)

        // Left for the list: nothing is being read.
        model.unsubscribe(link.qualified("other"))
        model.handle(.thread(row("other", .idle, unseen: true)), from: link)
        #expect(seen(backend) == ["other"])
    }

    @Test func notWhileTheAppIsInTheBackgroundAndOnceWhenItComesBack() {
        let (model, link, backend, clock) = setUp()
        model.subscribe(link.qualified("t"))
        clock.advance(30)
        // Home screen. The turn ends while the app is away.
        model.resignedActive()
        model.handle(.thread(row("t", .idle, unseen: true)), from: link)
        #expect(seen(backend) == ["t"], "nobody is reading it")
        #expect(model.thread(link.qualified("t"))?.unseen == true)

        // Back in front, the thread still open: seen now, once.
        clock.advance(300)
        model.foregrounded()
        #expect(seen(backend) == ["t", "t"])
        #expect(model.thread(link.qualified("t"))?.unseen == false)
    }

    @Test func aSideChatOverItsThreadIsTheOneBeingRead() {
        let (model, link, backend, clock) = setUp()
        model.subscribe(link.qualified("t"))
        // Its side chat's sheet comes up over it.
        model.subscribe(link.qualified("other"))
        clock.advance(10)
        model.handle(.thread(row("t", .idle, unseen: true)), from: link)
        #expect(seen(backend) == ["t", "other"], "the thread underneath isn't being read")
        #expect(model.thread(link.qualified("t"))?.unseen == true)
        // The sheet closes: the thread is in view again, and what ended meanwhile is seen.
        model.unsubscribe(link.qualified("other"))
        #expect(model.showing == link.qualified("t"))
        #expect(seen(backend) == ["t", "other", "t"])
        #expect(model.thread(link.qualified("t"))?.unseen == false)
    }

    @Test func itemsOutsideATurnAreCoalesced() {
        let (model, link, backend, clock) = setUp()
        let tid = link.qualified("t")
        model.subscribe(tid)
        model.handle(.thread(row("t", .idle)), from: link)   // the turn is over (and seen)
        let store = model.store(tid)
        store.applyReply(reset: true, seq: 1, items: [], more: false)
        let base = seen(backend).count

        // A burst of items within the gap: one wait, one message at its end.
        clock.advance(0.2)
        for n in 0..<5 {
            store.enqueue(TItem(id: "i\(n)", seq: Int64(10 + n), body: .notice("Background task \(n) finished")))
            store.flushQueued()
            clock.advance(0.1)
        }
        #expect(seen(backend).count == base, "told a moment ago: not again yet")
        #expect(clock.timers.count == 1)
        clock.advance(SeenPolicy.gap)
        #expect(seen(backend).count == base + 1)

        // While a turn runs, its items change nothing about read or unread.
        model.handle(.thread(row("t", .working)), from: link)
        clock.advance(10)
        store.enqueue(TItem(id: "i9", seq: 30, body: .notice("Working")))
        store.flushQueued()
        #expect(seen(backend).count == base + 1)
    }

    @Test func aListSentAfreshWithTheOpenThreadUnreadIsAnswered() {
        let (model, link, backend, clock) = setUp()
        model.subscribe(link.qualified("t"))
        clock.advance(60)
        // The phone missed events: the Mac sends its list again, the open thread unread in it.
        model.handle(.snapshot(Snapshot(threads: [row("t", .idle, unseen: true), row("other", .idle, unseen: true)], projects: [], agents: [])),
                     from: link)
        #expect(seen(backend) == ["t", "t"])
    }
}
