import Testing
@testable import Trek

/// The transcript store's bookkeeping: what the cache saves must hang together.
@MainActor
struct ThreadStoreTests {
    private func user(_ id: String, seq: Int64, _ text: String) -> TItem {
        TItem(id: id, seq: seq, at: nil, body: .user(text: text, images: 0, aside: false))
    }

    /// A live item moves `seq` only once it's among the items: a cache written in between
    /// (the app going to the background) would otherwise ask the Mac for what's newer than a
    /// message it never kept, and that message never came back.
    @Test func seqMovesOnlyWithTheItemsKept() {
        let s = ThreadStore(id: "h:t1", host: "h", summary: nil)
        s.applyReply(reset: true, seq: 10, items: [user("i0", seq: 10, "first")], more: false)
        #expect(s.seq == 10)
        s.enqueue(user("i1", seq: 11, "the follow-up"))
        #expect(s.seq == 10, "not before the item is in")
        #expect(s.items.map(\.id) == ["i0"])
        s.flushQueued()
        #expect(s.seq == 11)
        #expect(s.items.map(\.id) == ["i0", "i1"])
        // Applied in one pass with the others that arrived: the highest of them.
        s.enqueue(user("i2", seq: 12, "and"))
        s.enqueue(user("i3", seq: 14, "another"))
        s.flushQueued()
        #expect(s.seq == 14)
        #expect(s.items.count == 4)
    }

    /// A reply after the phone's seq adds what's newer and keeps the rest.
    @Test func aReplyAfterTheSeqAppends() {
        let s = ThreadStore(id: "h:t1", host: "h", summary: nil)
        s.restore(items: [user("i0", seq: 10, "first")], seq: 10, more: false)
        s.applyReply(reset: false, seq: 12, items: [user("i1", seq: 12, "the follow-up")], more: false)
        #expect(s.items.map(\.id) == ["i0", "i1"])
        #expect(s.seq == 12)
    }
}

/// The working line under the transcript: it goes with the turn's last rows, not before them.
@MainActor
struct WorkingLineTests {
    private func row(_ state: RunState) -> ThreadSummary {
        ThreadSummary(id: "h:t", title: "t", project: nil, agent: AgentRef(key: "codex", name: "Codex"), model: nil, modelLabel: nil,
                      runState: state, needs: nil, section: .inbox, unseen: false, pinned: false, branch: nil, worktree: false,
                      activity: nil, workingSince: nil, updatedAt: 0, additions: 0, deletions: 0)
    }

    private func item(_ id: String, _ seq: Int64) -> TItem { TItem(id: id, seq: seq, body: .turnEnd(tookSecs: 3)) }

    @Test func itWaitsForTheTurnsLastRows() {
        // As a Mac sends them: the row says the turn is over, then the items come.
        let store = ThreadStore(id: "h:t", host: "h", summary: row(.working))
        #expect(store.showsWorking && !store.footerEmpty)
        store.setSummary(row(.idle))
        #expect(store.showsWorking, "the row alone doesn't take it away")
        store.enqueue(item("i1", 1))
        store.flushQueued()
        #expect(!store.showsWorking, "gone with the rows, in the same pass")
        #expect(store.footerEmpty)
        #expect(store.blocks.count == 1)
    }

    @Test func aTurnEndingWithNoMoreRowsDoesntKeepItUp() async throws {
        let store = ThreadStore(id: "h:t", host: "h", summary: row(.working))
        store.setSummary(row(.idle))
        try await Task.sleep(for: .seconds(ThreadStore.turnEndHold + 0.3))
        #expect(!store.showsWorking)
    }

    @Test func aNewTurnStartingMeanwhileKeepsIt() async throws {
        let store = ThreadStore(id: "h:t", host: "h", summary: row(.working))
        store.setSummary(row(.idle))
        store.setSummary(row(.working))
        try await Task.sleep(for: .seconds(ThreadStore.turnEndHold + 0.3))
        #expect(store.showsWorking, "the first hold's timer doesn't end the new turn's line")
        store.enqueue(item("i1", 1))
        store.flushQueued()
        #expect(store.showsWorking, "rows of a running turn leave it")
    }

    @Test func aThreadOpenedIdleHasNothingUnderIt() {
        let store = ThreadStore(id: "h:t", host: "h", summary: row(.idle))
        #expect(!store.showsWorking && store.footerEmpty)
        #expect(ThreadStore(id: "h:t", host: "h", summary: nil).footerEmpty)
    }
}
