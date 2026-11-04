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
