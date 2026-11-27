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

    private func question(_ id: String, seq: Int64, _ state: QuestionState) -> TItem {
        TItem(id: id, seq: seq, body: .question(QuestionRequest(requestId: String(id.dropFirst()), questions: [], state: state, answers: nil)))
    }

    /// A page sent whole carries every request still open: a card kept from before that isn't
    /// among them was answered while the phone wasn't listening, and stops asking; one the
    /// page still has open keeps asking.
    @Test func aWholePageClosesRequestsAnsweredWhileAway() {
        let s = ThreadStore(id: "h:t1", host: "h", summary: nil)
        s.restore(items: [user("i0", seq: 10, "first"), question("rgone", seq: 11, .pending),
                          question("rstill", seq: 12, .pending), user("i1", seq: 13, "next")],
                  seq: 13, more: false)
        #expect(s.pending.map(\.id) == ["rgone", "rstill"])
        s.applyReply(reset: true, seq: 50, items: [user("i1", seq: 40, "next"), user("i2", seq: 41, "later"),
                                                   question("rstill", seq: 42, .pending)], more: false)
        #expect(s.pending.map(\.id) == ["rstill"])
        #expect(s.item("rgone")?.isPendingRequest == false)
        #expect(s.items.map(\.id) == ["i0", "rgone", "rstill", "i1", "i2"])
    }

    /// A reply after the phone's seq adds what's newer and keeps the rest.
    @Test func aReplyAfterTheSeqAppends() {
        let s = ThreadStore(id: "h:t1", host: "h", summary: nil)
        s.restore(items: [user("i0", seq: 10, "first")], seq: 10, more: false)
        s.applyReply(reset: false, seq: 12, items: [user("i1", seq: 12, "the follow-up")], more: false)
        #expect(s.items.map(\.id) == ["i0", "i1"])
        #expect(s.seq == 12)
    }

    // MARK: A transcript sent whole: caught up, or rewritten

    private func tool(_ id: String, seq: Int64, call: String, _ status: ToolStatus = .done) -> TItem {
        TItem(id: id, seq: seq, body: .tool(ToolCall(callId: call, tool: .command, title: "Run", detail: "cargo test", status: status,
                                                    output: "", added: nil, removed: nil)))
    }

    private func said(_ id: String, seq: Int64, _ text: String, at: Int64) -> TItem {
        TItem(id: id, seq: seq, at: at, body: .user(text: text, images: 0, aside: false))
    }

    /// Ten turns, as a Mac numbers them: a message, a tool call and an answer each.
    private func turns(_ range: Range<Int>, seq: Int64 = 100) -> [TItem] {
        range.flatMap { n -> [TItem] in
            [said("i\(n * 3)", seq: seq + Int64(n * 3), "message \(n)", at: Int64(1000 + n)),
             tool("i\(n * 3 + 1)", seq: seq + Int64(n * 3 + 1), call: "call-\(n)"),
             TItem(id: "i\(n * 3 + 2)", seq: seq + Int64(n * 3 + 2), body: .assistant(text: "answer \(n)", streaming: false))]
        }
    }

    /// The Mac also says "read it again" when this phone fell behind on its events. Its last page
    /// then lines up with what's here: the pages scrolled back to stay, and the reader's place
    /// (the list isn't drawn afresh).
    @Test func aResetThatLinesUpKeepsTheEarlierPages() {
        let s = ThreadStore(id: "h:t1", host: "h", summary: nil)
        // The last page, then an earlier one paged in.
        s.applyReply(reset: true, seq: 130, items: turns(6..<10), more: true)
        #expect(s.prepend(turns(2..<6), more: true))
        #expect(s.items.count == 24)
        let generation = s.generation
        let paging = s.paging

        // The phone fell behind: the Mac's last page again, with the two turns it missed, one of
        // the tool calls here finished meanwhile.
        var page = turns(7..<12, seq: 200)
        page[1] = tool("i22", seq: 260, call: "call-7", .failed)
        s.applyReply(reset: true, seq: 300, items: page, more: true)

        #expect(s.generation == generation, "not a rewrite: the reader stays where they are")
        #expect(s.paging == paging, "a page of earlier items on its way is still this transcript's")
        #expect(s.items.first?.id == "i6", "the page scrolled back to is still here")
        #expect(s.items.map(\.id) == turns(2..<12).map(\.id))
        #expect(s.more)
        #expect(s.seq == 300)
        // What changed meanwhile is as the Mac has it now.
        guard case .tool(let call)? = s.item("i22")?.body else { Issue.record("no tool"); return }
        #expect(call.status == .failed)
    }

    /// A rewind: the Mac's transcript is shorter than the one here. Everything is replaced and
    /// the list is drawn afresh, as before.
    @Test func aRewindReplacesEverything() {
        let s = ThreadStore(id: "h:t1", host: "h", summary: nil)
        s.applyReply(reset: true, seq: 130, items: turns(6..<10), more: true)
        s.prepend(turns(2..<6), more: true)
        let generation = s.generation

        // Rewound to before message 8: the last page ends at turn 7.
        s.applyReply(reset: true, seq: 300, items: turns(4..<8), more: true)
        #expect(s.generation == generation + 1)
        #expect(s.items.map(\.id) == turns(4..<8).map(\.id), "what was paged in goes with the rewrite")
    }

    /// After a rewind the Mac numbers the new items like the ones they replaced (`i<n>` is a
    /// position). The same ids with other items behind them is a rewrite, not a catch-up.
    @Test func theSameIdsWithOtherItemsIsARewrite() {
        let s = ThreadStore(id: "h:t1", host: "h", summary: nil)
        s.applyReply(reset: true, seq: 130, items: turns(6..<10), more: true)
        s.prepend(turns(2..<6), more: true)
        let generation = s.generation

        // Rewound to before message 8, and two new turns since: i24… are other items now.
        var page = turns(6..<8, seq: 100)
        page += [said("i24", seq: 400, "another way to ask", at: 5000), tool("i25", seq: 401, call: "call-new-8"),
                 TItem(id: "i26", seq: 402, body: .assistant(text: "a different answer", streaming: false)),
                 said("i27", seq: 403, "and then", at: 5100), tool("i28", seq: 404, call: "call-new-9"),
                 TItem(id: "i29", seq: 405, body: .assistant(text: "done", streaming: false))]
        #expect(page.map(\.id) == turns(6..<10).map(\.id), "the same ids as before")
        s.applyReply(reset: true, seq: 500, items: page, more: true)
        #expect(s.generation == generation + 1)
        #expect(s.items.map(\.id) == page.map(\.id))
        guard case .user(let text, _, _)? = s.item("i24")?.body else { Issue.record("no message"); return }
        #expect(text == "another way to ask")
    }

    @Test func howAPageSentWholeFits() {
        let here = turns(2..<6)
        #expect(ThreadStore.fit(turns(4..<8), onto: here) == .continues(keep: 6))
        #expect(ThreadStore.fit(turns(2..<6), onto: here) == .continues(keep: 0), "nothing new: nothing moves")
        // It starts before everything here and holds all of it.
        #expect(ThreadStore.fit(turns(0..<8), onto: here) == .covers)
        // Shorter than what's here (rewound), nothing in common (far behind), nothing at all.
        #expect(ThreadStore.fit(turns(3..<5), onto: here) == .rewritten)
        #expect(ThreadStore.fit(turns(8..<10), onto: here) == .rewritten)
        #expect(ThreadStore.fit([], onto: here) == .rewritten)
        // An answer still being written has grown: the same item.
        let writing = [TItem(id: "i1", seq: 5, body: .assistant(text: "Let me", streaming: true))]
        let written = [TItem(id: "i1", seq: 9, body: .assistant(text: "Let me look at the test first.", streaming: false))]
        #expect(ThreadStore.fit(written, onto: writing) == .continues(keep: 0))
        // Requests and a turn's changed files sit where each side put them: they don't decide it.
        // The Mac puts open requests last; here one sits where it was asked.
        let ask = question("r3", seq: 20, .pending)
        let mine = [here[0], here[1], ask, here[2]]
        #expect(ThreadStore.fit([here[1], here[2], ask], onto: mine) == .continues(keep: 1))
    }

    /// A page of earlier items asked for before a rewrite holds items of the transcript that
    /// was: it's dropped when it arrives, not put before the new one.
    @Test func aPageFromBeforeARewriteIsDropped() {
        let s = ThreadStore(id: "h:t1", host: "h", summary: nil)
        s.applyReply(reset: true, seq: 130, items: turns(6..<10), more: true)
        // Scrolled to the top: a page is asked for.
        let asked = s.paging
        s.loadingEarlier = true
        // The thread is rewound before the page arrives.
        s.applyReply(reset: true, seq: 300, items: turns(2..<4), more: true)
        #expect(!s.loadingEarlier, "the next scroll to the top asks afresh")

        #expect(!s.prepend(turns(4..<6), more: true, asked: asked))
        #expect(s.items.map(\.id) == turns(2..<4).map(\.id), "the stale page isn't put before it")
        // The page asked for afterwards is the transcript's own.
        #expect(s.prepend(turns(0..<2), more: false, asked: s.paging))
        #expect(s.items.map(\.id) == turns(0..<4).map(\.id))
        #expect(!s.more)
    }

    /// Leaving a thread trims it to its last items: a page still on its way was for before the
    /// old earliest item, and would leave a hole before the new one.
    @Test func aPageFromBeforeATrimIsDropped() {
        let s = ThreadStore(id: "h:t1", host: "h", summary: nil)
        s.applyReply(reset: true, seq: 130, items: turns(2..<10), more: true)
        let asked = s.paging
        s.loadingEarlier = true
        s.trim(to: 6)
        #expect(!s.prepend(turns(0..<2), more: false, asked: asked))
        #expect(s.items.map(\.id) == turns(8..<10).map(\.id))
        #expect(s.more)
    }

    // MARK: Items of a kind this app doesn't know

    private func strange(_ id: String, seq: Int64) -> TItem { TItem(id: id, seq: seq, body: .unknown("hologram")) }

    /// One muted row for a run of them, where there was nothing; the tool groups around it stay
    /// two groups, with the row between saying why.
    @Test func itemsOfAnUnknownKindGetOneRow() {
        let items = [user("i0", seq: 1, "go"), tool("i1", seq: 2, call: "a"), strange("i2", seq: 3), strange("i3", seq: 4),
                     tool("i4", seq: 5, call: "b"), TItem(id: "i5", seq: 6, body: .turnEnd(tookSecs: 3)), strange("i6", seq: 7)]
        let blocks = Block.build(items).blocks
        #expect(blocks.map(\.id) == ["i0", "g-i1", "u-i2", "g-i4", "i5", "u-i6"])
        guard case .unsupported = blocks[2].kind else { Issue.record("\(blocks[2].kind)"); return }
        #expect(blocks[2].count == 2, "one row for the two of them")
        #expect(Block.unsupportedText == "Update Trek on this iPhone to see this")
        // It ends no turn and starts none.
        #expect(Block.turnEnds(blocks, open: false) == ["i5"])
    }

    @Test func anotherUnknownItemJoinsTheRowAlreadyThere() {
        let s = ThreadStore(id: "h:t1", host: "h", summary: nil)
        s.applyReply(reset: true, seq: 2, items: [user("i0", seq: 1, "go"), strange("i1", seq: 2)], more: false)
        #expect(s.blocks.map(\.id) == ["i0", "u-i1"])
        let before = s.blocks[1]
        s.enqueue(strange("i2", seq: 3))
        s.flushQueued()
        #expect(s.blocks.map(\.id) == ["i0", "u-i1"], "still one row")
        #expect(s.blocks[1] != before, "redrawn: it stands for one more")
        s.enqueue(tool("i3", seq: 4, call: "a"))
        s.flushQueued()
        #expect(s.blocks.map(\.id) == ["i0", "u-i1", "g-i3"])
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

    /// A thread whose transcript is all in and empty says so (the list was blank); not while it
    /// loads, has earlier items to fetch, is at work, or once something is in it.
    @Test func anEmptyThreadSaysSo() {
        let store = ThreadStore(id: "h:t", host: "h", summary: row(.idle))
        #expect(!store.nothingYet, "not loaded yet: the spinner's")
        store.applyReply(reset: true, seq: 0, items: [], more: true)
        #expect(!store.nothingYet, "earlier items are still to come")
        store.applyReply(reset: true, seq: 0, items: [], more: false)
        #expect(store.nothingYet)
        store.enqueue(item("i1", 1))
        store.flushQueued()
        #expect(!store.nothingYet)

        let working = ThreadStore(id: "h:t", host: "h", summary: row(.working))
        working.applyReply(reset: true, seq: 0, items: [], more: false)
        #expect(!working.nothingYet, "the working line is under it")
    }
}
