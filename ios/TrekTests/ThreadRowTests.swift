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
