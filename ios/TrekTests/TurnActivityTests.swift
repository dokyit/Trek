import Foundation
import Testing
@testable import Trek

private let now = Date(timeIntervalSince1970: 1_800_000_000)
private let nowMs = Int64(now.timeIntervalSince1970 * 1000)

@MainActor
private func thread(_ id: String, _ state: RunState = .idle, needs: Needs? = nil, ask: RowAsk? = nil, section: ThreadSection = .inbox,
                    since: Int64? = nil, activity: String? = nil, updated: Int64 = nowMs) -> ThreadSummary {
    var t = ThreadSummary(id: "h:\(id)", title: id, project: ProjectRef(id: "p", name: "trek", hue: 24, monogram: "TR"),
                          agent: AgentRef(key: "codex", name: "Codex"), model: nil, modelLabel: nil, runState: state, needs: needs,
                          section: section, unseen: false, pinned: false, branch: nil, worktree: false, activity: activity,
                          workingSince: since, updatedAt: updated, additions: 0, deletions: 0)
    t.ask = ask
    return t
}

@MainActor
private func pick(_ threads: [ThreadSummary]) -> TurnActivityAttributes.ContentState? {
    TurnActivityPick.state(threads: threads, now: now) { ("h", String($0.dropFirst(2))) }
}

@MainActor
struct TurnActivityPickTests {
    @Test func nothingMovingOrBlockedHoldsItUp() {
        #expect(pick([]) == nil)
        #expect(pick([thread("quiet")]) == nil)
        // A failure never settled, a thread paused at a limit: they could stay for days.
        #expect(pick([thread("broke", .failed, needs: Needs(kind: .failed, text: "It broke"))]) == nil)
        #expect(pick([thread("paused", needs: Needs(kind: .limit, text: "Paused"))]) == nil)
        var server = thread("server")
        server.background = ["npm run dev"]
        #expect(pick([server]) == nil, "a dev server may run all day")
        #expect(pick([thread("put away", .working, section: .snoozed, since: nowMs)]) == nil, "snoozed on purpose")
        let old = nowMs - Int64(TurnActivityPick.asksFor * 1000) - 1000
        #expect(pick([thread("old", needs: Needs(kind: .approval, text: "Run it?"), updated: old)]) == nil)
    }

    @Test func whatAsksComesBeforeWhatWorks() {
        let state = pick([thread("working", .working, since: nowMs, activity: "Reading `store.rs`"),
                          thread("asking", .needsYou, needs: Needs(kind: .question, text: "Which CI?"))])
        #expect(state?.kind == .asking)
        #expect(state?.title == "asking")
        #expect(state?.line == "Which CI?")
        #expect(state?.requestID == nil, "a question is answered in the app")
        #expect(state?.others == "1 at work")
    }

    @Test func theTurnStartedLastIsShownWithoutItsBackticks() throws {
        let state = try #require(pick([thread("first", .working, since: nowMs - 60_000),
                                       thread("last", .working, since: nowMs - 1000, activity: "Reading `store.rs`")]))
        #expect(state.kind == .working)
        #expect(state.title == "last")
        #expect(state.line == "Reading store.rs")
        #expect(state.since == Date(timeIntervalSince1970: Double(nowMs - 1000) / 1000))
        #expect(state.others == "1 more at work")
        #expect(state.hostID == "h" && state.bareID == "last" && state.threadID == "h:last")
        #expect(state.link?.absoluteString == "trek://open?thread=last&host=h")
    }

    @Test func anApprovalCanBeAnsweredFromTheCard() {
        let state = pick([thread("a", .needsYou, needs: Needs(kind: .approval, text: "Run cargo test?"),
                                 ask: RowAsk(requestId: "r1", detail: "cargo test"))])
        #expect(state?.requestID == "r1")
        #expect(state?.warning == nil)
    }

    @Test func butNotOneThatLooksDestructiveAnywhereInIt() {
        // Its first line reads harmless: the row's words alone would have let it through.
        let state = pick([thread("a", .needsYou, needs: Needs(kind: .approval, text: "Run cargo test?"),
                                 ask: RowAsk(requestId: "r1", detail: "cargo test\nrm -rf target"))])
        #expect(state?.requestID == nil)
        #expect(state?.warning == "deletes files")
    }

    @Test func norAnOlderMacsOrASubAgents() {
        #expect(pick([thread("old", .needsYou, needs: Needs(kind: .approval, text: "Run it?"))])?.requestID == nil)
        var parent = thread("parent", .idle, needs: Needs(kind: .approval, text: "Sub-agent needs you"), ask: RowAsk(requestId: "r1"))
        parent.needsThread = "h:kid"
        #expect(pick([parent])?.requestID == nil, "it's answered in the sub-agent's thread")
    }

    @Test func itSaysHowManyMoreWaitAndWhatStillRuns() {
        var server = thread("server")
        server.background = ["npm run dev"]
        let state = pick([thread("a", .needsYou, needs: Needs(kind: .approval, text: "Run it?")),
                          thread("b", .needsYou, needs: Needs(kind: .plan, text: "Plan ready"), updated: nowMs - 1000),
                          thread("c", .working, since: nowMs),
                          server])
        #expect(state?.title == "a", "the newest request")
        #expect(state?.waitingOnYou == 2)
        #expect(state?.others == "1 more waiting on you · 1 at work · 1 in the background")
    }

    @Test func longWordsAreClippedToFitWhatTheCardMayCarry() {
        let long = String(repeating: "word ", count: 200)
        let state = pick([thread("a", .working, since: nowMs, activity: long)])
        #expect((state?.line.count ?? 0) <= 140)
        #expect(state?.line.hasSuffix("…") == true)
    }

    @Test func whenItEndsItSaysSoAndOffersNothing() throws {
        let asking = try #require(pick([thread("a", .needsYou, needs: Needs(kind: .approval, text: "Run it?"),
                                               ask: RowAsk(requestId: "r1", detail: "ls"))]))
        let end = TurnActivityPick.finished(asking, now: now)
        #expect(end.kind == .finished)
        #expect(end.requestID == nil && end.warning == nil)
        #expect(end.others.isEmpty)
    }

    @Test func aStampAloneDoesntChangeHowItLooks() throws {
        var a = try #require(pick([thread("a", .working, since: nowMs)]))
        var b = a
        b.heard = now.addingTimeInterval(30)
        #expect(a.looksLike(b))
        a.line = "Something else"
        #expect(!a.looksLike(b))
    }
}

@MainActor
struct DiffCommentTests {
    @Test func whatChangedInALineIsWhatsLeftOfWhatTheyShare() {
        let parts = DiffText.changedParts("let x = old_value;", "let x = new_value;")
        #expect(parts?.old == 8..<11)
        #expect(parts?.new == 8..<11)
        // Inserted words: nothing of the old line differs.
        let inserted = DiffText.changedParts("foo(a)", "foo(a, b)")
        #expect(inserted?.old == nil)
        #expect(inserted?.new == 5..<8)
        // Nothing in common worth saying so: the whole line is the change.
        #expect(DiffText.changedParts("completely different", "1234567") == nil)
        #expect(DiffText.changedParts(String(repeating: "a", count: 3000), "b") == nil, "too long to compare")
    }

    @Test func aLineReplacedIsMarkedOnBothSidesAndAnAddedOneIsnt() throws {
        let diff = """
        @@ -1,2 +1,3 @@
        -let x = 1;
        +let x = 2;
        +let y = 3;
         done
        @@ -9 +10 @@
        -let z = 1;
        +let z = 2;
        """
        let lines = DiffText.parse(diff)
        // Two added for one removed: which replaced which can't be told.
        #expect(lines.filter { $0.changed != nil }.map(\.text) == ["let z = 1;", "let z = 2;"])
        let last = try #require(lines.last)
        #expect(last.number == 10)
        #expect(last.changed == 8..<9)
        #expect(DiffText.spoken(last) == "Line 10, added: let z = 2;")
    }

    @Test func anIndentIsCountedInColumnsAndKeptFromUsingUpTheLine() {
        #expect(DiffText.indent(of: "let x").columns == 0)
        #expect(DiffText.indent(of: "    let x").characters == 4)
        #expect(DiffText.indent(of: "\t\tlet x").columns == 8)
        #expect(DiffText.indent(of: "\t\tlet x").characters == 2)
        #expect(DiffText.indent(of: String(repeating: " ", count: 80) + "x").columns == 24)
        #expect(DiffText.indent(of: String(repeating: " ", count: 80) + "x").characters == 80, "all of it is left out of the text")
    }

    @Test func commentsKeepToTheOrderOfTheFileAndGoWhenEmptied() throws {
        let c = DiffComments()
        let lines = DiffText.parse("@@ -1 +1,2 @@\n-old\n+new\n+more")
        c.compose(path: "a.rs", line: lines[3])
        var second = try #require(c.composing)
        second.text = "And here"
        c.save(second)
        c.compose(path: "a.rs", line: lines[1])
        var first = try #require(c.composing)
        #expect(first.removed)
        #expect(first.text.isEmpty)
        first.text = "Here"
        c.save(first)
        #expect(c.list.map(\.text) == ["Here", "And here"])
        // Taken up again, it has what was written; emptied, it's gone.
        c.compose(path: "a.rs", line: lines[1])
        #expect(c.composing?.text == "Here")
        var emptied = try #require(c.composing)
        emptied.text = "  "
        c.save(emptied)
        #expect(c.list.map(\.text) == ["And here"])
    }

    @Test func theMessageSaysWhereEachIsAndTheLineItsAbout() {
        let list = [
            DiffComments.Comment(path: "src/a.rs", row: 1, number: 41, removed: true, quote: "    old();", text: "Why did this go?"),
            DiffComments.Comment(path: "src/a.rs", row: 2, number: 42, removed: false, quote: "new();", text: " Take the lock first. \n"),
        ]
        #expect(DiffComments.message(list) == """
        Comments on the diff:

        `src/a.rs:41` (a line it removes)
        > old();
        Why did this go?

        `src/a.rs:42`
        > new();
        Take the lock first.
        """)
    }
}

@MainActor
struct LaunchAndRenderTests {
    @Test func aNewModelHasntBootedSoNothingDrawsThePairingScreenYet() {
        let model = AppModel()
        #expect(!model.booted)
        #expect(model.mode == .unpaired, "it can't know yet: the Keychain is read as it boots")
    }

    @Test func comparingAnAnswerDoesntNeedTheMainActor() async {
        // SwiftUI compares rows on its render thread at times: a main-actor `==` traps there.
        nonisolated(unsafe) let pair = (MarkdownText(text: "x"), MarkdownText(text: "x"))
        nonisolated(unsafe) let other = MarkdownText(text: "y")
        let results = await Task.detached { (pair.0 == pair.1, pair.0 == other) }.value
        #expect(results.0)
        #expect(!results.1)
    }
}

/// The clock the owner has seen wrong: times come from the Mac as instants, and read in this
/// iPhone's own time zone.
@MainActor
struct ClockTests {
    @Test func aMessageFromAnHourAgoSaysThatHourHere() throws {
        let now = Date(timeIntervalSince1970: 1_800_000_000)
        let then = now.addingTimeInterval(-3600)
        let words = When.clock(Int64(then.timeIntervalSince1970 * 1000), now: now)
        let same = Calendar.current.isDate(then, inSameDayAs: now)
        let f = DateFormatter()
        f.setLocalizedDateFormatFromTemplate(same ? "jmm" : "EEEjmm")
        #expect(words == f.string(from: then))
    }

    @Test func aMacAheadOfThisPhoneDoesntGoNegative() {
        let now = Date(timeIntervalSince1970: 1_800_000_000)
        let ahead = Int64(now.timeIntervalSince1970 * 1000) + 90_000
        #expect(When.short(ahead, now: now) == "now")
        #expect(When.short(Int64(now.timeIntervalSince1970 * 1000) - 120_000, now: now) == "2m")
        #expect(When.duration(3661) == "1h 1m")
    }
}
