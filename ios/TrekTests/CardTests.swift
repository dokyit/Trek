import Foundation
import Testing
@testable import Trek

/// A transcript as the Mac sends it, with `items` in it.
private func transcript(_ items: [[String: Any]]) throws -> [TItem] {
    let json: [String: Any] = ["type": "transcript", "thread_id": "t", "reset": true, "seq": 99, "items": items]
    guard case .transcript(_, _, _, _, let out, _) = try ServerMessage.decode(JSONSerialization.data(withJSONObject: json)) else {
        Issue.record("not a transcript")
        return []
    }
    return out
}

private let prCard: [String: Any] = [
    "id": "i7", "seq": 12, "at": NSNull(), "kind": "card",
    "card": [
        "key": "https://github.com/o/r/pull/128", "kind": "pr", "phase": "checking", "title": "Refresh sessions", "number": 128,
        "url": "https://github.com/o/r/pull/128", "head": "feat/session-refresh", "base": "main", "commits": 3,
        "history": [["title": "Add tests", "oid": "d29f50a", "committed_ms": 1_791_020_000_000]],
        "checks": [["name": "build (macos)", "state": "pass"], ["name": "clippy", "state": "running"]],
        "created_ms": 1_791_019_000_000, "phase_at_ms": 1_791_020_000_000,
    ] as [String: Any],
]

struct CardDecodingTests {
    @Test func aCardItemReads() throws {
        let items = try transcript([prCard])
        #expect(items.count == 1)
        guard case .card(let c) = items[0].body else { Issue.record("\(items[0].body)"); return }
        #expect(c.kind == .pr && c.phase == .checking && c.number == 128)
        #expect(c.head == "feat/session-refresh" && c.commits == 3)
        #expect(c.history.first?.committedMs == 1_791_020_000_000)
        #expect(c.checks.map(\.state) == [.pass, .running])
        #expect(c.phaseAtMs == 1_791_020_000_000 && c.actions.isEmpty)
    }

    @Test func aNewerMacsCardStillReads() throws {
        // A kind, phase, state and button this phone doesn't know, and fields it doesn't have.
        let items = try transcript([[
            "id": "i8", "seq": 13, "kind": "card",
            "card": ["key": "k", "kind": "deploy", "phase": "rolling_out", "checks": [["name": "x", "state": "timed_out"]],
                     "actions": ["merge", "promote"], "preview": ["url": "https://x"]] as [String: Any],
        ]])
        guard case .card(let c) = items.first?.body else { Issue.record("dropped"); return }
        #expect(c.kind == .other && c.phase == .other)
        #expect(c.checks.first?.state == .skipped)
        #expect(c.actions == [.merge])
    }

    @Test func oneThatIsntACardDoesntTakeTheTranscriptWithIt() throws {
        // A card that won't read, and a kind no phone knows yet: the rest of the transcript reads.
        let items = try transcript([
            ["id": "i1", "seq": 1, "kind": "card", "card": "not a card"],
            ["id": "i2", "seq": 2, "kind": "hologram", "beam": ["up": true]],
            ["id": "i3", "seq": 3, "kind": "notice", "text": "Still here"],
        ])
        #expect(items.map(\.id).contains("i3"))
        if let i2 = items.first(where: { $0.id == "i2" }) {
            guard case .unknown("hologram") = i2.body else { Issue.record("\(i2.body)"); return }
        }
    }

    @Test func aCardKeepsInTheCache() throws {
        let item = try #require(try transcript([prCard]).first)
        let e = JSONEncoder()
        e.keyEncodingStrategy = .convertToSnakeCase
        let d = JSONDecoder()
        d.keyDecodingStrategy = .convertFromSnakeCase
        let back = try d.decode(TItem.self, from: e.encode(item))
        #expect(back == item)
    }

    @Test func itsButtonGoesOutAsTheMacReadsIt() throws {
        let o = ClientMessage.cardAction(threadId: "t", key: "run-1", action: .rerun).json(id: "5")
        #expect(o["type"] as? String == "card_action")
        #expect(o["key"] as? String == "run-1" && o["action"] as? String == "rerun" && o["thread_id"] as? String == "t")
        #expect(o["id"] as? String == "5")
    }
}

struct CardWordsTests {
    @Test func aPullRequestSaysWhereItStands() {
        #expect(DemoMac.card("pr-creating").chipParts == ("PR", "Creating"))
        #expect(DemoMac.card("pr-checking").chipParts == ("#128", "Checking"))
        #expect(DemoMac.card("pr-conflicted").chipParts == ("#128", "2 conflicts"))
        #expect(DemoMac.card("pr-resolving").chipParts == ("#128", "Resolving"))
        #expect(DemoMac.card("pr-ready").chipParts == ("#128", "Ready to merge"))
        #expect(DemoMac.card("pr-merging").chipParts == ("#128", "Merging"))
        #expect(DemoMac.card("pr-merged").chipParts == ("#128", "Merged"))
        #expect(DemoMac.card("pr-ready").readyDetail == "No conflicts · build (macos), build (windows) and clippy passed")
        #expect(DemoMac.card("pr-conflicted").conflictsTitle == "2 conflicts with main")
        var cleared = DemoMac.card("pr-resolving")
        cleared.conflicts = []
        #expect(cleared.conflictsTitle == "Conflicts resolved")
    }

    @Test func aRunCountsItsJobs() {
        #expect(DemoMac.card("run-running").chipParts == ("CI", "2 of 3 passed"))
        #expect(DemoMac.card("run-passed").chipParts == ("CI", "Passed"))
        let failed = DemoMac.card("run-failed")
        #expect(failed.chipParts == ("CI", "Test (Windows) failed"))
        #expect(failed.failureTitle == "Test (Windows) failed")
        #expect(failed.failureDetail == "2 of 214 tests failed in trek-remote", "the error the Mac found says more")
        var plain = failed
        plain.step = "Failed: Test (Windows)"
        #expect(plain.failureDetail == "The other jobs passed")
    }

    @Test func aReleaseBuildsThenPublishes() {
        #expect(DemoMac.card("release-building").chipParts == ("v0.4.28", "Building 2 of 3"))
        #expect(DemoMac.card("release-publishing").chipParts == ("v0.4.28", "Publishing"))
        #expect(DemoMac.card("release-published").chipParts == ("v0.4.28", "Published"))
        #expect(DemoMac.card("release-failed").chipParts == ("v0.4.28", "Windows build failed"))
        #expect(DemoMac.card("release-building").platforms == "Mac + Windows")
        #expect(DemoMac.card("release-published").platformsForSub == "Mac and Windows")
        #expect(DemoMac.card("release-failed").failureDetail == "1 of 3 jobs passed")
    }

    @Test func timesAreToldAsTheMacTellsThem() {
        #expect(RemoteCard.duration(45_000) == "45s")
        #expect(RemoteCard.duration(21 * 60_000 + 5_000) == "21m 05s")
        #expect(RemoteCard.duration(64 * 60_000) == "1h 4m")
        let now: Int64 = 1_791_020_000_000
        var run = DemoMac.card("run-running", now: now)
        #expect(run.tookMs(now: now) == 21 * 60_000 + 5_000)
        #expect(run.stages[2].timeLabel(now: now) == "21m", "a running job counts whole minutes")
        #expect(run.stages[0].timeLabel(now: now) == "4m 10s")
        run.startedMs = 0
        #expect(run.tookMs(now: now) == 0, "nothing before it starts")
    }
}

struct CardTransitionTests {
    private func blocks(_ bodies: [ItemBody]) -> [Block] {
        Block.build(bodies.enumerated().map { TItem(id: "i\($0.offset)", seq: Int64($0.offset + 1), body: $0.element) }).blocks
    }

    @Test func theChipFollowsTheNewestCard() {
        let now = DemoMac.now
        let b = blocks([.card(DemoMac.card("pr-merged", now: now - 60_000)), .assistant(text: "CI next", streaming: false),
                        .card(DemoMac.card("run-running", now: now))])
        #expect(CardChip.card(in: b, now: now)?.card.kind == .run)
        #expect(CardChip.card(in: blocks([.notice("nothing")]), now: now) == nil)
    }

    @Test func aGoodFinishLingersThenGoes() {
        let now = DemoMac.now
        let b = blocks([.card(DemoMac.card("pr-merged", now: now))])
        #expect(CardChip.card(in: b, now: now + 3_000) != nil)
        #expect(CardChip.card(in: b, now: now + 4_000) == nil)
    }

    @Test func aFailureStaysUntilTheNextMessage() {
        let now = DemoMac.now
        let failed = ItemBody.card(DemoMac.card("run-failed", now: now))
        #expect(CardChip.card(in: blocks([failed]), now: now + 60_000) != nil)
        #expect(CardChip.card(in: blocks([failed, .user(text: "Re-run it", images: 0)]), now: now) == nil)
        #expect(CardChip.card(in: blocks([failed, .user(text: "/usage", images: 0, aside: true)]), now: now) != nil,
                "a command Trek answered isn't a message to the agent")
    }

    @Test func anUpdateRedrawsTheCardItReplaces() {
        let a = Block.single(TItem(id: "i4", seq: 10, body: .card(DemoMac.card("pr-ready"))))
        let b = Block.single(TItem(id: "i4", seq: 11, body: .card(DemoMac.card("pr-merging"))))
        #expect(a != nil && b != nil && a != b && a?.id == b?.id)
    }

    @Test func theDemosButtonsMoveItOn() {
        #expect(DemoMac.cardAfter(.merge, on: "pr-ready") == "pr-merging")
        #expect(DemoMac.cardAfter(.resolve, on: "pr-conflicted") == "pr-resolving")
        #expect(DemoMac.cardAfter(.rerun, on: "run-failed") == "run-running")
        #expect(DemoMac.cardAfter(.merge, on: "pr-checking") == nil, "no Merge while it checks")
        for s in DemoMac.cardStates {
            let c = DemoMac.card(s)
            #expect(c.actions.allSatisfy { DemoMac.cardAfter($0, on: s) != nil }, "\(s) offers only what it does")
        }
    }
}
