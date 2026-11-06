import Foundation
import Testing
@testable import Trek

/// A row as the Mac sends it, with `extra` fields merged in.
private func row(_ extra: [String: Any] = [:]) throws -> ThreadSummary {
    var json: [String: Any] = [
        "id": "t", "title": "Audit the dependencies",
        "project": ["id": "p", "name": "trek", "hue": 24, "monogram": "TR"],
        "agent": ["key": "claude-code", "name": "Claude Code"],
        "run_state": "idle", "section": "inbox", "unseen": false, "pinned": false, "worktree": false,
        "updated_at": 1_791_020_400_000, "additions": 0, "deletions": 0,
    ]
    json.merge(extra) { $1 }
    let d = JSONDecoder()
    d.keyDecodingStrategy = .convertFromSnakeCase
    return try d.decode(ThreadSummary.self, from: JSONSerialization.data(withJSONObject: json))
}

private let runningSubAgent: [String: Any] = ["agent": ["key": "codex", "name": "Codex"], "title": "Review", "state": "running"]
private let doneSubAgent: [String: Any] = ["agent": ["key": "codex", "name": "Codex"], "title": "Review", "state": "done"]

struct BusyInBackgroundTests {
    @Test func theMacSaysSo() throws {
        // Its sub-agents are done, but a report is on its way: only the Mac knows.
        let waiting = try row(["busy_in_background": true, "sub_agents": [doneSubAgent]])
        #expect(waiting.busyInBackground == true)
        #expect(waiting.waitsInBackground && waiting.atWork && waiting.isActive)
        // Sub-agents out, but the Mac says it isn't waiting (paused at a limit, say): it isn't.
        let not = try row(["busy_in_background": false, "sub_agents": [runningSubAgent]])
        #expect(!not.waitsInBackground && !not.atWork)
    }

    @Test func anOlderMacsRowIsWorkedOut() throws {
        let old = try row(["sub_agents": [runningSubAgent]])
        #expect(old.busyInBackground == nil)
        #expect(old.waitsInBackground, "idle, nothing asked, a sub-agent running")
        #expect(try !row(["sub_agents": [doneSubAgent]]).waitsInBackground)
        #expect(try !row(["sub_agents": [runningSubAgent], "needs": ["kind": "approval", "text": "Run it"]]).waitsInBackground)
        #expect(try !row(["sub_agents": [runningSubAgent], "run_state": "working"]).waitsInBackground)
        #expect(try row(["run_state": "working"]).atWork, "working is at work either way")
    }
}

/// A command the agent left running, as the Mac's row carries it.
private let devServer: [String: Any] = [
    "id": "b1", "kind": "shell", "title": "cd \"/Users/sam/web\" && npm run dev", "line": "ready on :5173",
    "since": 1_791_020_000_000, "readable": true, "stoppable": true,
]

@MainActor
struct RunningInBackgroundTests {
    @Test func aCommandStillRunningAfterTheTurnShows() throws {
        // The user's case: the turn is over, nothing waits on a report, a command still runs.
        let t = try row(["busy_in_background": false, "background": [devServer["title"]!], "background_tasks": [devServer]])
        #expect(t.runsInBackground && t.onlyRunsInBackground)
        #expect(!t.atWork, "not \"Waiting\": the Working pill stays for turns")
        #expect(t.isActive, "kept out under its project, folded or not")
        #expect(t.backgroundWork.map(\.id) == ["b1"])
        #expect(t.backgroundWork.first?.since == 1_791_020_000_000)
    }

    @Test func anOlderMacsTitlesStillShow() throws {
        let t = try row(["background": ["pnpm dev", "cargo watch"]])
        #expect(t.runsInBackground)
        #expect(t.backgroundWork.map(\.title) == ["pnpm dev", "cargo watch"])
        #expect(t.backgroundWork.allSatisfy { $0.id.hasPrefix(ThreadSummary.titleOnly) && $0.stoppable == nil })
    }

    @Test func nothingRunningIsNothing() throws {
        let t = try row(["busy_in_background": false])
        #expect(!t.runsInBackground && !t.onlyRunsInBackground && !t.isActive)
        #expect(t.backgroundWork.isEmpty)
    }

    @Test func aWorkingTurnKeepsItsPlace() throws {
        let t = try row(["run_state": "working", "background_tasks": [devServer]])
        #expect(t.runsInBackground && !t.onlyRunsInBackground && t.atWork)
    }

    @Test func itSortsAfterWorkAndBeforeTheQuiet() throws {
        let working = try row(["id": "w", "run_state": "working", "updated_at": 1])
        let running = try row(["id": "r", "background_tasks": [devServer], "updated_at": 2])
        let quiet = try row(["id": "q", "updated_at": 3])
        let sorted = [quiet, running, working].sorted(by: AppModel.activeOrder)
        #expect(sorted.map(\.id) == ["w", "r", "q"])
    }

    @Test func oneLineOfACommand() {
        #expect(BackgroundLine.oneLine("npm   run dev\n# second line") == "npm run dev")
    }
}

struct ComposerEchoTests {
    @Test func sendingEmptiesTheComposer() {
        let c = ComposerState()
        c.draft = "Also check the snooze picker"
        c.cleared(after: c.draft)
        #expect(c.draft.isEmpty && c.photos.isEmpty)
    }

    @Test func theKeyboardWritingTheMessageBackIsUndone() {
        let c = ComposerState()
        let sent = Date()
        c.draft = "Also check the snooze picker"
        c.cleared(after: c.draft, at: sent)
        // Dictation finishing, or a held correction committed, puts it all back at once.
        c.draft = "Also check the snooze picker"
        #expect(c.dropEcho(at: sent.addingTimeInterval(0.2)))
        #expect(c.draft.isEmpty)
        // With its last word corrected.
        c.draft = "Also check the snooze pickers"
        #expect(c.dropEcho(at: sent.addingTimeInterval(0.4)))
        #expect(c.draft.isEmpty)
    }

    @Test func whatsTypedNextStays() {
        let c = ComposerState()
        let sent = Date()
        c.draft = "Also check the snooze picker"
        c.cleared(after: c.draft, at: sent)
        c.draft = "A"
        #expect(!c.dropEcho(at: sent.addingTimeInterval(0.3)))
        #expect(c.draft == "A")
        // Typing ended the watch: even the same words later are the user's.
        c.draft = "Also check the snooze picker"
        #expect(!c.dropEcho(at: sent.addingTimeInterval(0.5)))
    }

    @Test func afterAMomentItsTheUsers() {
        let c = ComposerState()
        let sent = Date()
        c.draft = "Send it again"
        c.cleared(after: c.draft, at: sent)
        c.draft = "Send it again"
        #expect(!c.dropEcho(at: sent.addingTimeInterval(ComposerState.echoWindow + 0.1)))
        #expect(c.draft == "Send it again")
    }

    @Test func echoes() {
        #expect(ComposerState.isEcho("Fix the tests", of: "Fix the tests\n"))
        #expect(ComposerState.isEcho("Rename the helper functoin", of: "Rename the helper function"))
        #expect(!ComposerState.isEcho("Fix", of: "Fix the flaky session refresh test"))
        #expect(!ComposerState.isEcho("Something else", of: "Fix the tests"))
    }
}

/// What the Mac adds to a row for the phone: an older Mac's has none of it.
@MainActor
struct AskAndCutShortTests {
    @Test func anOlderMacsRowHasNeither() throws {
        let t = try row(["needs": ["kind": "approval", "text": "Run it"]])
        #expect(t.ask == nil && t.cutShort == nil)
        #expect(try StatusLook.of(row()) == nil, "an idle row with nothing to say has no pill")
    }

    @Test func aRowSaysWhichRequestAndWhatItWouldDo() throws {
        let t = try row(["needs": ["kind": "approval", "text": "Run cargo test?"],
                         "ask": ["request_id": "r1", "detail": "cargo test\nrm -rf target"]])
        #expect(t.ask?.requestId == "r1")
        #expect(t.ask?.detail.contains("rm -rf") == true)
    }

    @Test func aTurnCutOffIsKeptInView() throws {
        let t = try row(["cut_short": ["state": "offered", "how": "Reopens its Codex session", "waiting": false]])
        #expect(t.cutShort?.state == .offered)
        #expect(t.isActive, "not folded away with the quiet threads")
        #expect(StatusLook.of(t)?.label == "Interrupted")
        let failed = try row(["cut_short": ["state": "failed", "how": "h", "why": "It stopped"]])
        #expect(failed.cutShort?.state == .failed && failed.cutShort?.why == "It stopped")
        let unknown = try row(["cut_short": ["state": "something new"]])
        #expect(unknown.cutShort?.state == .offered, "a newer Mac's state still offers")
    }

    @Test func theBarSaysWhatTheMacsBarSays() throws {
        let thread = try row()
        let bar = { (cut: CutShort) in CutShortBar(thread: thread, cut: cut) }
        #expect(bar(CutShort(state: .offered, how: "Reopens it")).title == "Interrupted")
        #expect(bar(CutShort(state: .offered, how: "Reopens it")).detail == "Reopens it")
        let waiting = CutShort(state: .offered, how: "Reopens it", waiting: true)
        #expect(bar(waiting).title == "Waiting for you when Trek closed")
        #expect(bar(waiting).detail == "Its question went with Trek. Ask again to bring it back.")
        #expect(bar(CutShort(state: .offered, waiting: true, asked: "Run make?")).detail == "Run make?")
        #expect(bar(CutShort(state: .failed, why: "No session")).title == "Couldn't resume")
        #expect(bar(CutShort(state: .failed, why: "No session")).detail == "No session")
    }
}
