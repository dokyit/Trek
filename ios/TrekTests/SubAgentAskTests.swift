import Foundation
import Testing
@testable import Trek

/// A Mac that keeps what it's sent and answers nothing by itself.
final class RecordingBackend: Backend {
    var onMessage: ((ServerMessage) -> Void)?
    var onState: ((ConnectionState) -> Void)?
    private(set) var sent: [(message: ClientMessage, id: String?)] = []

    func start() {}
    func wake() {}
    func stop() {}

    @discardableResult
    func send(_ message: ClientMessage, id: String?) -> Bool {
        sent.append((message, id))
        return true
    }

    /// The `type` of each message sent, in order.
    var kinds: [String] { sent.compactMap { $0.message.json(id: nil)["type"] as? String } }
}

private func approval(_ id: String, _ state: ApprovalState = .pending) -> TItem {
    TItem(id: id, seq: 1, body: .approval(ApprovalRequest(requestId: "q-\(id)", title: "Run command", detail: "cargo bench", state: state)))
}

/// A sub-agent's request, pinned in its parent's thread and answered there.
@MainActor
struct SubAgentAskTests {
    private let kid = "demo:kid"

    private func ask(kid: String? = "demo:kid", own: [TItem] = [], pending: [TItem] = [], answered: Set<String> = [],
                     loaded: Bool = true, waited: Bool = false, settling: Bool = false) -> SubAgentAsk {
        SubAgentAsk.of(kid: kid, own: own, kidPending: pending, answered: answered, kidLoaded: loaded, waited: waited, settling: settling)
    }

    @Test func itsRequestIsPinnedWhereTheThreadsOwnWouldBe() {
        let r = approval("r3")
        #expect(ask(pending: [r]) == .cards([r]))
        #expect(ask(pending: [r]).pins)
        // No sub-agent waits: nothing of one.
        #expect(ask(kid: nil, pending: [r]) == .none)
        #expect(!ask(kid: nil).pins)
    }

    /// As on the Mac: the thread's own request, or else a sub-agent's.
    @Test func theThreadsOwnRequestsComeFirst() {
        #expect(ask(own: [approval("r1")], pending: [approval("r3")]) == .none)
    }

    /// Nothing is put up only to be swapped a moment later: not the card that opens the thread
    /// while the request is on its way, nor after an answer while the row catches up.
    @Test func nothingFlashesByWhileItLoadsOrSettles() {
        #expect(ask(loaded: false) == .waiting)
        #expect(!ask(loaded: false).pins)
        let r = approval("r3")
        let answered: Set<String> = [SubAgentAsk.key("r3", in: kid)]
        #expect(ask(pending: [r], answered: answered) == .waiting, "answered here, on its way to the Mac")
        #expect(ask(pending: [], settling: true) == .waiting, "closed by the Mac; the row still names it")
        // One of two answered: the other still asks.
        let other = approval("r4")
        #expect(ask(pending: [r, other], answered: answered) == .cards([other]))
    }

    /// A secret is typed on the Mac, and an older Mac may say no more than that it waits: the
    /// card that opens its thread, as before.
    @Test func whatCantBeAnsweredHereOpensItsThread() {
        #expect(ask(pending: []) == .open)
        #expect(ask(loaded: false, waited: true) == .open, "its transcript never came")
        #expect(ask().pins)
    }

    /// Two threads number their requests alike: an answer to the thread's own `r3` doesn't hide
    /// the sub-agent's.
    @Test func answersAreRememberedByThread() {
        let r = approval("r3")
        #expect(SubAgentAsk.key("r3", in: nil) == "r3")
        #expect(SubAgentAsk.key("r3", in: kid) != SubAgentAsk.key("r3", in: "demo:other"))
        #expect(ask(pending: [r], answered: ["r3"]) == .cards([r]))
    }

    /// The parent listens to the sub-agent's transcript for its request, and the sub-agent's own
    /// screen may have it open too: the Mac is told to stop only when both have let go, and the
    /// sub-agent's thread counts as seen only once it has been on screen.
    @Test func aTranscriptStaysOpenUntilEveryReaderLetsGo() {
        let backend = RecordingBackend()
        let model = AppModel()
        model.links = [HostLink(paired: .demo, backend: backend)]
        let holder = SubAgentAsk.holder("demo:parent")

        model.subscribe(kid, as: holder)
        #expect(backend.kinds == ["subscribe"], "read for its request: not marked seen")
        #expect(model.existingStore(kid)?.subscribed == true)

        // Opened from "Open it": on its own screen now.
        model.subscribe(kid)
        #expect(backend.kinds == ["subscribe", "mark_seen"], "one subscription, and seen")
        #expect(model.holders(of: kid) == [holder, AppModel.onScreen])

        // Back to the parent: the screen lets go, the parent still listens.
        model.unsubscribe(kid)
        #expect(!backend.kinds.contains("unsubscribe"))
        #expect(model.existingStore(kid)?.subscribed == true)

        // The parent is left too.
        model.unsubscribe(kid, as: holder)
        #expect(backend.kinds.last == "unsubscribe")
        #expect(model.existingStore(kid)?.subscribed == false)
        #expect(model.holders(of: kid).isEmpty)

        // Letting go twice, or of what was never held, sends nothing more.
        model.unsubscribe(kid, as: holder)
        #expect(backend.kinds.filter { $0 == "unsubscribe" }.count == 1)
    }

    /// The answer goes to the sub-agent's thread, by its id as its Mac knows it.
    @Test func theAnswerGoesToTheSubAgent() throws {
        let backend = RecordingBackend()
        let model = AppModel()
        model.links = [HostLink(paired: .demo, backend: backend)]
        model.answer(kid, requestId: "q-r3", .approval(.deny))
        let sent = try #require(backend.sent.last)
        guard case .answer(let thread, let request, let response) = sent.message else {
            Issue.record("not an answer: \(sent.message)")
            return
        }
        #expect(thread == "kid")
        #expect(request == "q-r3")
        #expect(response.json["decision"] as? String == "deny")
    }
}
