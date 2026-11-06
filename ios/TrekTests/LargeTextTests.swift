import SwiftUI
import Testing
import UIKit
@testable import Trek

/// At the largest accessibility size nothing runs off the screen, and a row's title keeps its room.
/// Measured, as `ThreadsHeaderLayoutTests` does: these were seen broken in the simulator.
@MainActor
struct LargeTextTests {
    static let widths: [CGFloat] = [375, 402, 440]

    private func size<V: View>(_ view: V, width: CGFloat, type: DynamicTypeSize = .accessibility5) -> CGSize {
        let model = AppModel()
        let host = UIHostingController(rootView: view.environment(model).dynamicTypeSize(type))
        return host.sizeThatFits(in: CGSize(width: width, height: .greatestFiniteMagnitude))
    }

    private func thread(_ title: String, state: RunState = .working) -> ThreadSummary {
        ThreadSummary(id: "h:t", title: title, project: ProjectRef(id: "p", name: "trek", hue: 24, monogram: "TR"),
                      agent: AgentRef(key: "codex", name: "Codex"), model: nil, modelLabel: "GPT-6 Astra", runState: state, needs: nil,
                      section: .working, unseen: false, pinned: false, branch: "main", worktree: false, activity: "Reading store.rs",
                      workingSince: 1_800_000_000_000, updatedAt: 1_800_000_000_000, additions: 12, deletions: 3)
    }

    @Test func aPlansButtonsStayInTheScreen() {
        // Side by side they ran off both sides, and the composer under them went along.
        let plan = PlanRequest(requestId: "r", markdown: "Migrate settings to v2", state: .pending)
        for width in Self.widths {
            let card = PlanCard(request: plan, agentName: "Claude Code") { _ in }
            #expect(size(card, width: width).width <= width + 0.5, "at \(Int(width)) pt")
            #expect(size(card, width: width, type: .large).width <= width + 0.5, "the usual size at \(Int(width)) pt")
        }
    }

    @Test func anApprovalsButtonsStayInTheScreen() {
        let ask = ApprovalRequest(requestId: "r", title: "Run command", detail: "cargo test -p auth", state: .pending)
        for width in Self.widths {
            let card = ApprovalCard(request: ask, agentName: "Claude Code", threadID: "h:t") { _ in }
            #expect(size(card, width: width).width <= width + 0.5, "at \(Int(width)) pt")
        }
    }

    @Test func aRowsTitleKeepsItsRoom() {
        // One short title and one long: beside "Working 6m 12s" a title got no room at all, so a row
        // grew no taller for three lines of it. On lines of their own, it takes them.
        for width in Self.widths {
            let short = size(ThreadRow(thread: thread("Fix")), width: width)
            let long = size(ThreadRow(thread: thread("Snoozed threads raise their hand when something needs the user")), width: width)
            #expect(long.height > short.height + 30, "at \(Int(width)) pt: \(short.height) to \(long.height)")
            #expect(long.width <= width + 0.5, "at \(Int(width)) pt")
        }
        // At the usual sizes it's still one line, cut off.
        let short = size(ThreadRow(thread: thread("Fix")), width: 402, type: .large)
        let long = size(ThreadRow(thread: thread("Snoozed threads raise their hand when something needs the user")), width: 402, type: .large)
        #expect(short.height == long.height)
    }

    @Test func aChangedFileKeepsItsName() {
        let file = ChangedFile(path: "crates/trek-core/src/store_with_a_long_name.rs", status: .modified)
        for width in Self.widths {
            #expect(size(ChangedFileRow(file: file), width: width).width <= width + 0.5, "at \(Int(width)) pt")
        }
        // Its lines go under its name rather than beside it.
        #expect(size(ChangedFileRow(file: file), width: 402).height > size(ChangedFileRow(file: file), width: 402, type: .xxLarge).height * 2)
    }

    @Test func aTurnsButtonsStayInTheScreen() {
        for width in Self.widths {
            let turn = TurnActions(end: "i1", secs: 412, at: 1_800_000_000_000, latest: true, busy: false, models: []) { _ in }
            #expect(size(turn, width: width - 40).width <= width - 40 + 0.5, "at \(Int(width)) pt")
            let message = MessageActions(item: "i1", text: "Hi", at: 1_800_000_000_000, latest: true, busy: false) { _ in }
            #expect(size(message, width: width - 40).width <= width - 40 + 8.5, "at \(Int(width)) pt (it hangs 8 pt out on purpose)")
        }
    }

    @Test func theChipsStopGrowingWhereTheirCapsulesDo() {
        let context = ContextUse(used: 61_000, window: 200_000, percent: 31)
        #expect(size(ContextCostLabel(context: context, cost: nil), width: 402).height == 30)
        // And say the same as at xxLarge, however large the text.
        #expect(size(ContextCostLabel(context: context, cost: nil), width: 402) == size(ContextCostLabel(context: context, cost: nil), width: 402, type: .xxLarge))
    }

    @Test func fourPhotosFitTheBubble() {
        // The bubble: the row's width (402, less 20 each side) less its 48 pt of air and 15 each side.
        let room = 402 - 40 - 48 - 30.0
        for count in 1...6 {
            let tiles = min(count, 4)
            let width = CGFloat(tiles) * MessageImages.side(count: count) + CGFloat(tiles - 1) * 6
            #expect(width <= room, "\(count) photos: \(width) of \(room)")
        }
    }

    @Test func photosWithNoWordsAreCounted() {
        #expect(PickedPhoto.caption(0) == "Here's a photo.")
        #expect(PickedPhoto.caption(1) == "Here's a photo.")
        #expect(PickedPhoto.caption(4) == "Here are 4 photos.")
    }
}

@MainActor
struct SidewaysAndPartsTests {
    @Test func theFadeShowsOnlyWhereThereIsMore() {
        // It all fits: nothing to hint at, with a point's slack.
        var edges = SidewaysEdges(contentWidth: 300, viewWidth: 300, offset: 0)
        #expect(!edges.before && !edges.after)
        edges = SidewaysEdges(contentWidth: 300.5, viewWidth: 300, offset: 0)
        #expect(!edges.before && !edges.after)
        // Runs off the side: more after, none before.
        edges = SidewaysEdges(contentWidth: 800, viewWidth: 300, offset: 0)
        #expect(!edges.before && edges.after)
        // Part-way along: both. At the end: only before.
        edges = SidewaysEdges(contentWidth: 800, viewWidth: 300, offset: 200)
        #expect(edges.before && edges.after)
        edges = SidewaysEdges(contentWidth: 800, viewWidth: 300, offset: 500)
        #expect(edges.before && !edges.after)
    }

    @Test func anAnswersPartsAddUpToWhatItMeasuredWhole() {
        let whole: CGFloat = 1000
        let shares = TranscriptRow.shareOut(whole, guesses: [200, 300, 500], gaps: 20)
        #expect(abs(shares.reduce(0, +) + 20 - whole) < 0.001, "so nothing under it moves as it changes hands")
        #expect(abs(shares[2] / shares[0] - 2.5) < 0.001, "in proportion to the guesses")
        // Nothing to go on: the guesses stand.
        #expect(TranscriptRow.shareOut(0, guesses: [10, 20], gaps: 10) == [10, 20])
        #expect(TranscriptRow.shareOut(100, guesses: [0, 0], gaps: 10) == [0, 0])
    }

    @Test func goingThroughTheFilesKeepsTheCardsOrder() {
        // The list the diff steps through is the order the card shows: folder by folder.
        let files = [ChangedFile(path: "b/two.rs", status: .modified), ChangedFile(path: "top.rs", status: .modified),
                     ChangedFile(path: "a/one.rs", status: .modified)]
        #expect(ChangesCard.folders(files).flatMap(\.files).map(\.path) == ["top.rs", "a/one.rs", "b/two.rs"])
    }
}

/// The Interrupted bar answers at once, and doesn't go on saying what the Mac refused.
@MainActor
struct CutShortBarTests {
    /// Takes what's sent; the test answers for the Mac.
    private final class FakeMac: Backend {
        var onMessage: ((ServerMessage) -> Void)?
        var onState: ((ConnectionState) -> Void)?
        var sent: [String] = []
        func start() {}
        func wake() {}
        func stop() {}
        @discardableResult func send(_ message: ClientMessage, id: String?) -> Bool {
            if let id { sent.append(id) }
            return true
        }
    }

    private func setUp(_ state: CutShortState = .offered) -> (AppModel, HostLink, FakeMac) {
        let mac = FakeMac()
        let link = HostLink(paired: .demo, backend: mac)
        let model = AppModel()
        model.links = [link]
        var row = ThreadSummary(id: link.qualified("t"), title: "t", project: nil, agent: AgentRef(key: "codex", name: "Codex"), model: nil,
                                modelLabel: nil, runState: .idle, needs: nil, section: .inbox, unseen: false, pinned: false, branch: nil,
                                worktree: false, activity: nil, workingSince: nil, updatedAt: 0, additions: 0, deletions: 0)
        row.cutShort = CutShort(state: state, how: "Reopens its Codex session")
        model.threads = [row]
        return (model, link, mac)
    }

    @Test func resumeSaysSoAtOnceAndGoesOnce() {
        let (model, link, mac) = setUp()
        let id = link.qualified("t")
        model.resumeCutShort(id)
        #expect(model.thread(id)?.cutShort?.state == .resuming)
        #expect(mac.sent.count == 1)
        // A second tap before the Mac answers: not a second resume.
        model.resumeCutShort(id)
        #expect(mac.sent.count == 1)
        link.reply(mac.sent[0], .ack(re: mac.sent[0], threadId: nil))
        #expect(model.thread(id)?.cutShort?.state == .resuming, "until the Mac's row says otherwise")
    }

    @Test func aRefusedResumePutsTheOfferBack() {
        let (model, link, mac) = setUp(.failed)
        let id = link.qualified("t")
        model.resumeCutShort(id)
        #expect(model.thread(id)?.cutShort?.state == .resuming)
        link.reply(mac.sent[0], .error(re: mac.sent[0], code: .conflict, message: "There's nothing to resume"))
        #expect(model.thread(id)?.cutShort?.state == .failed, "Retry is back")
        #expect(model.toast?.text == "There's nothing to resume")
    }

    @Test func dismissGoesAtOnceAndComesBackIfRefused() {
        let (model, link, mac) = setUp()
        let id = link.qualified("t")
        model.dismissCutShort(id)
        #expect(model.thread(id)?.cutShort == nil)
        link.reply(mac.sent[0], .error(re: mac.sent[0], code: .unknown, message: "No"))
        #expect(model.thread(id)?.cutShort?.state == .offered)
    }
}
