import Testing
@testable import Trek

/// Where an answered request's row stands in the transcript as the turn goes on.
@MainActor
struct RequestRowPlaceTests {
    private func said(_ id: String, seq: Int64, _ text: String) -> TItem {
        TItem(id: id, seq: seq, at: nil, body: .assistant(text: text, streaming: false))
    }

    private func question(_ id: String, seq: Int64, _ state: QuestionState) -> TItem {
        TItem(id: id, seq: seq, body: .question(QuestionRequest(requestId: String(id.dropFirst()), questions: [], state: state, answers: nil)))
    }

    /// The row stays where the request was answered: what the agent says next, and the turns
    /// after, come under it. It used to be pushed down by each of them.
    @Test func anAnsweredRequestKeepsItsPlaceAsTheTurnGoesOn() {
        let s = ThreadStore(id: "h:t1", host: "h", summary: nil)
        s.applyReply(reset: true, seq: 10, items: [said("i0", seq: 9, "It needs to run."), question("r1", seq: 10, .pending)], more: false)
        s.enqueue(question("r1", seq: 11, .answered))
        s.enqueue(said("i1", seq: 12, "Done: the migration ran."))
        s.flushQueued()
        #expect(s.items.map(\.id) == ["i0", "r1", "i1"])
        s.enqueue(said("i2", seq: 13, "And the next turn's answer."))
        s.flushQueued()
        #expect(s.items.map(\.id) == ["i0", "r1", "i1", "i2"])
    }

    /// One that belongs before later items still goes there (a turn's changed files, counted
    /// after later turns were already here), request rows or not.
    @Test func anItemTheMacOrdersEarlierStillGoesWhereItBelongs() {
        let s = ThreadStore(id: "h:t1", host: "h", summary: nil)
        s.applyReply(reset: true, seq: 10, items: [said("i0", seq: 8, "one"), question("r1", seq: 9, .answered), said("i2", seq: 10, "three")], more: false)
        s.enqueue(said("i1", seq: 11, "two, late"))
        s.flushQueued()
        #expect(s.items.map(\.id) == ["i0", "i1", "r1", "i2"])
    }
}
