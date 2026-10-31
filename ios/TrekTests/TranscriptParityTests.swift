import Foundation
import Testing
@testable import Trek

private func decode<T: Decodable>(_ type: T.Type, _ json: [String: Any]) throws -> T {
    let d = JSONDecoder()
    d.keyDecodingStrategy = .convertFromSnakeCase
    return try d.decode(T.self, from: JSONSerialization.data(withJSONObject: json))
}

struct MessagePartsTests {
    /// What `orchestrate::consult_prompt` writes after the message.
    static let consulted = """
    Cache the session lookups.

    <trek-consult style="arena" then="report" with="codex/gpt-5.6-sol/high,claude-code/claude-opus-5-5/max" judge="acp:devin/swe-2/medium">
    Before you build this, run a design arena through Trek's `delegate_task` tool:

    1. Ground the problem…

    If `delegate_task` isn't available to you, say so and stop.
    </trek-consult>
    """

    @Test func aConsultComesOffAndIsRead() throws {
        let parts = MessageParts.parse(Self.consulted)
        #expect(parts.said == "Cache the session lookups.")
        let c = try #require(parts.consult)
        #expect(c.style == .arena)
        #expect(!c.implement)
        #expect(c.consultants.map(\.model) == ["gpt-5.6-sol", "claude-opus-5-5"])
        #expect(c.consultants.map(\.effort) == ["high", "max"])
        #expect(c.judge?.agent == "acp:devin")
        #expect(c.agents == ["codex", "claude-code", "acp:devin"])
        let line = c.line { $0.model == "gpt-5.6-sol" ? "Sol" : $0.model == "claude-opus-5-5" ? "Opus 5.5" : "SWE-2" }
        #expect(line == "Design arena: Sol · High, Opus 5.5 · Max · judged by SWE-2 · Medium · then report")
    }

    @Test func advisingWordsAsTheMac() {
        let text = "Fix it.\n\n<trek-consult style=\"advise\" then=\"implement\" with=\"codex/gpt-5.6-sol/xhigh\">\n…\n</trek-consult>"
        let c = MessageParts.parse(text).consult
        #expect(c?.line { _ in "Sol" } == "Consulting Sol · Extra high · advice, then implement")
    }

    @Test func aRestateComesOffUnderAConsult() {
        let text = "Make it survive a restart.\n\n<trek-restate>\nBefore you do anything else, restate…\n</trek-restate>\n\n<trek-consult style=\"discuss\" then=\"implement\" with=\"codex/gpt-5.6-sol/high\">\n…\n</trek-consult>"
        let parts = MessageParts.parse(text)
        #expect(parts.said == "Make it survive a restart.")
        #expect(parts.restate)
        #expect(parts.consult?.style == .discuss)
    }

    @Test func sideBarContextBecomesChips() {
        let text = "Make greet polite\n\n<trek-context>\nThe user attached:\n@src/main.rs\nsrc/util.rs:3-8\n```rust\nfn x() {}\n```\n</trek-context>\n\n<trek-ask>Answer only.</trek-ask>"
        let parts = MessageParts.parse(text)
        #expect(parts.said == "Make greet polite")
        #expect(parts.ask)
        #expect(parts.context == ["main.rs", "util.rs:3–8"])
    }

    @Test func plainTextAndLookalikesStayWhole() {
        #expect(MessageParts.of("Just a message").said == "Just a message")
        // Mentions the tag, but isn't Trek's block at the end.
        let quoted = "What does `<trek-restate>` do?\n\n<trek-restate> is how Trek asks"
        #expect(MessageParts.parse(quoted).said == quoted)
        // A consult with no consultant it can read isn't one.
        let broken = "Hi\n\n<trek-consult style=\"advise\" with=\"nonsense\">\n</trek-consult>"
        #expect(MessageParts.parse(broken).consult == nil)
        #expect(MessageParts.parse(broken).said == broken)
    }
}

struct WakeNoteTests {
    @Test func foldsAsTheMac() {
        let one = "[Trek] Sub-agent “Review the cache” (GPT-5.6 Sol, task t-1) finished:\n\nLooks good."
        #expect(WakeNote.isWake(one))
        #expect(WakeNote.summary(one) == "GPT-5.6 Sol reported back on “Review the cache”")
        let failed = "[Trek] Sub-agent “Bump tokio” (Opus 5.5, task t-2) failed: it timed out"
        #expect(WakeNote.summary(failed) == "Opus 5.5 failed on “Bump tokio”")
        #expect(WakeNote.failed(failed))
        let two = one + "\n\n---\n\n" + failed
        #expect(WakeNote.summary(two) == "2 sub-agents reported back")
        let thread = "[Trek] Thread “Speed up the tests” (Sonnet 5.5, thread t-9) finished:\n\nDone.\n\nCall read_thread for its full answer."
        #expect(WakeNote.summary(thread) == "Thread “Speed up the tests” finished")
        #expect(!WakeNote.isWake("A message about [Trek] "))
    }

    @Test func aWakeIsNotTheUsersPrompt() {
        let wake = Block(id: "i2", itemID: "i2", kind: .user(text: "[Trek] Sub-agent “x” (y, task z) finished:\n\nok", images: 0, at: nil, aside: false),
                         version: 2, count: 1)
        let user = Block(id: "i1", itemID: "i1", kind: .user(text: "Go", images: 0, at: nil, aside: false), version: 1, count: 1)
        #expect(user.isPrompt && !wake.isPrompt)
        #expect(wake.isUser)
    }
}

struct TranscriptExtrasTests {
    @Test func newerFieldsAreRead() throws {
        let end = try decode(TItem.self, ["id": "i9", "seq": 9, "kind": "turn_end", "took_secs": 412,
                                          "verified": ["passed": true, "commands": ["./trek-dev check"]]])
        #expect(end.extras.verified == Verification(passed: true, commands: ["./trek-dev check"]))
        let handoff = try decode(TItem.self, ["id": "i3", "seq": 3, "kind": "handoff", "from": "Codex", "to": "Claude Code",
                                              "from_agent": ["key": "codex", "name": "Codex", "logo": "codex"],
                                              "to_agent": ["key": "claude-code", "name": "Claude Code"]])
        #expect(handoff.extras.fromAgent?.logo == "codex")
        #expect(handoff.extras.toAgent?.key == "claude-code")
        let user = try decode(TItem.self, ["id": "i1", "seq": 1, "kind": "user", "text": "@/a/clip.mov", "videos": ["/a/clip.mov"]])
        #expect(user.extras.videos == ["/a/clip.mov"])
        let tool = try decode(TItem.self, ["id": "i4", "seq": 4, "kind": "tool", "call_id": "c", "tool": "other", "title": "watch_video",
                                           "status": "done",
                                           "watch": ["source": "clip.mp4", "video": NSNull(), "duration": NSNull(),
                                                     "frames": [["t": 1.5, "path": "/w/f1.jpg"]]]])
        #expect(tool.extras.watch?.frames == [WatchedFrame(t: 1.5, path: "/w/f1.jpg")])
        #expect(tool.extras.watch?.video == nil)
    }

    @Test func anOlderMacsItemsHaveNone() throws {
        let end = try decode(TItem.self, ["id": "i9", "seq": 9, "kind": "turn_end", "took_secs": 3])
        #expect(end.extras == ItemExtras())
    }

    @Test func extrasSurviveTheCache() throws {
        var item = TItem(id: "i9", seq: 9, body: .turnEnd(tookSecs: 3))
        item.extras.verified = Verification(passed: false, commands: ["make check"])
        item.extras.videos = ["/v.mov"]
        let e = JSONEncoder()
        e.keyEncodingStrategy = .convertToSnakeCase
        let d = JSONDecoder()
        d.keyDecodingStrategy = .convertFromSnakeCase
        let back = try d.decode(TItem.self, from: e.encode(item))
        #expect(back.extras == item.extras)
    }

    @Test func blocksCarryThem() {
        var item = TItem(id: "i9", seq: 9, body: .turnEnd(tookSecs: 3))
        item.extras.verified = Verification(passed: true)
        #expect(Block.single(item)?.extras.verified?.passed == true)
    }

    @Test func rowsSayPauseAndSharedFolder() throws {
        let json: [String: Any] = [
            "id": "t", "title": "T", "project": ["id": "p", "name": "trek", "hue": 24, "monogram": "TR"],
            "agent": ["key": "claude-code", "name": "Claude Code"], "run_state": "idle", "section": "inbox", "unseen": false,
            "pinned": false, "worktree": false, "updated_at": 1, "additions": 0, "deletions": 0,
            "shares_folder": true, "pause": ["resets_at": 1_800_000_000_000, "resume": true, "wrapped": false],
        ]
        let t = try decode(ThreadSummary.self, json)
        #expect(t.sharesFolder == true)
        #expect(t.pause == LimitPause(resetsAt: 1_800_000_000_000, resume: true, wrapped: false))
    }
}

struct TranscriptRequestsTests {
    @Test func limitAndCompactActionsAreTagged() {
        #expect(ThreadAction.resumeAtReset.json["kind"] as? String == "resume_at_reset")
        #expect(ThreadAction.cancelResume.json["kind"] as? String == "cancel_resume")
        #expect(ThreadAction.tryAgain.json["kind"] as? String == "try_again")
        #expect(ThreadAction.snoozeUntilReset.json["kind"] as? String == "snooze_until_reset")
        #expect(ThreadAction.compact.json["kind"] as? String == "compact")
    }

    @Test func requestsAreShapedAsTheMacReadsThem() {
        let media = ClientMessage.media(threadId: "t", path: "docs/a.png", maxPx: 1024, offset: 0, length: nil).json(id: "1")
        #expect(media["type"] as? String == "media")
        #expect(media["max_px"] as? Int == 1024)
        #expect(media["length"] == nil)
        let preview = ClientMessage.turnPreview(threadId: "t", itemId: "i4", action: .undo).json(id: nil)
        #expect(preview["action"] as? String == "undo")
        let send = ClientMessage.sendConsult(threadId: "t", text: "Go", mode: nil, images: [],
                                             consult: ConsultRequest(with: ["codex/gpt-5.6-sol/high"], style: .discuss, implement: false)).json(id: nil)
        #expect(send["type"] as? String == "send")
        let consult = send["consult"] as? [String: Any]
        #expect(consult?["style"] as? String == "discuss")
        #expect(consult?["then"] as? String == "report")
        #expect(consult?["with"] as? [String] == ["codex/gpt-5.6-sol/high"])
    }

    @Test func repliesAreRead() throws {
        let card = try ServerMessage.decode(JSONSerialization.data(withJSONObject: [
            "type": "context_card", "re": "1", "thread_id": "t", "used": 120_000, "window": 200_000, "reserved": 6_666,
            "parts": [["kind": "system", "label": "System", "tokens": 21_000, "detail": [["label": "Prompt", "value": NSNull()]], "note": NSNull()],
                      ["kind": "messages", "label": "Messages", "tokens": 99_000, "detail": [], "note": NSNull()],
                      ["kind": "free", "label": "Free", "tokens": 73_334, "detail": [], "note": NSNull()],
                      ["kind": "reserved", "label": "Reserved", "tokens": 6_666, "detail": [], "note": NSNull()]],
            "compact": "ready",
        ]))
        guard case .contextCard(_, let c) = card else { Issue.record("not a context card"); return }
        #expect(c.filled.map(\.kind) == ["system", "messages"])
        #expect(c.free == 73_334 && c.held == 6_666)
        let preview = try ServerMessage.decode(JSONSerialization.data(withJSONObject: [
            "type": "turn_preview", "re": "2", "thread_id": "t", "item_id": "i4", "recap": true,
            "files": [["path": "a.rs", "change": "bring_back"]], "unavailable": NSNull(),
        ]))
        guard case .turnPreview(_, let p) = preview else { Issue.record("not a preview"); return }
        #expect(p.recap)
        #expect(p.files?.first?.verb == "bring back")
        let media = try ServerMessage.decode(JSONSerialization.data(withJSONObject: [
            "type": "media", "re": "3", "thread_id": "t", "path": "a.mp4", "mime": "video/mp4", "offset": 0, "total": 10, "bytes_b64": "AAAA",
        ]))
        guard case .media(_, let m) = media else { Issue.record("not media"); return }
        #expect(m.asNoteChunk.total == 10 && m.width == nil)
    }
}

struct AnswerMediaTests {
    @Test func picturesAndVideosOnTheirOwnLines() {
        #expect(MediaLink.images("![after](docs/shot.png)")?.map(\.path) == ["docs/shot.png"])
        #expect(MediaLink.images("![a](a.png) ![b](https://x.dev/b)")?.count == 2)
        #expect(MediaLink.images("See ![a](a.png)") == nil, "text around it: a paragraph")
        #expect(MediaLink.images("![a](notes.txt)") == nil, "not a picture")
        #expect(MediaLink.video("[demo](clip.mp4)")?.label == "demo")
        #expect(MediaLink.video("[clip.mov](file:///Users/me/clip.mov)")?.path == "/Users/me/clip.mov")
        #expect(MediaLink.video("[demo](https://x.dev/v.webm)")?.web != nil)
        #expect(MediaLink.video("[page](https://x.dev/)") == nil)
        #expect(MediaLink.video("the [demo](clip.mp4) is here") == nil)
    }

    @Test func markdownMakesThemBlocks() {
        let blocks = Markdown.parse("Here:\n\n![shot](a.png)\n\n[clip](b.mov)\n\nDone.")
        #expect(blocks.count == 4)
        if case .images(let p) = blocks[1] { #expect(p.first?.link == "a.png") } else { Issue.record("no picture") }
        if case .video(let v) = blocks[2] { #expect(v.link == "b.mov") } else { Issue.record("no video") }
    }

    @Test func codeFencesNameTheirLanguage() {
        #expect(CodeBlockView.label("rust") == "rust")
        #expect(CodeBlockView.label("Rust ignore") == "rust")
        #expect(CodeBlockView.label("{.swift}") == "swift")
        #expect(CodeBlockView.label("rust,no_run") == "rust")
        #expect(CodeBlockView.label("") == nil)
    }
}

struct ComposerChipsTests {
    private func thread(branch: String?, changed: Int, ahead: Int, state: RunState = .idle) -> ThreadSummary {
        var t = ThreadSummary(id: "t", title: "T", project: nil, agent: AgentRef(key: "claude-code", name: "Claude Code"), model: nil,
                              modelLabel: nil, runState: state, needs: nil, section: .inbox, unseen: false, pinned: false, branch: branch,
                              worktree: false, activity: nil, workingSince: nil, updatedAt: 0, additions: 0, deletions: 0)
        t.git = GitSummary(changed: changed, ahead: ahead, behind: 0, defaultBranch: "main")
        return t
    }

    @Test func createPRIsOfferedAsOnTheMac() {
        #expect(CreatePR.offered(thread(branch: "feat/x", changed: 2, ahead: 0), blocks: []))
        #expect(CreatePR.offered(thread(branch: "feat/x", changed: 0, ahead: 1), blocks: []))
        #expect(!CreatePR.offered(thread(branch: "main", changed: 2, ahead: 1), blocks: []), "on the default branch")
        #expect(!CreatePR.offered(thread(branch: "feat/x", changed: 0, ahead: 0), blocks: []), "nothing on it")
        #expect(!CreatePR.offered(thread(branch: "feat/x", changed: 2, ahead: 0, state: .working), blocks: []), "while it works")
        let pr = Block(id: "c1", itemID: "c1", kind: .card(RemoteCard()), version: 1, count: 1)
        #expect(!CreatePR.offered(thread(branch: "feat/x", changed: 2, ahead: 0), blocks: [pr]), "a PR is out")
    }

    @Test func theLimitBarSaysWhen() {
        let now = Date(timeIntervalSince1970: 1_000)
        #expect(LimitBar.detail(LimitPause(resetsAt: nil), now: now) == "Reset time unknown")
        let at = Int64(1_000 + 2 * 3600) * 1000
        #expect(LimitBar.detail(LimitPause(resetsAt: at, resume: true), now: now).hasPrefix("Resumes at "))
        #expect(LimitBar.detail(LimitPause(resetsAt: at), now: now).hasPrefix("Resets "))
        #expect(LimitBar.detail(LimitPause(resetsAt: at), now: now).contains("(in 2h"))
    }
}
