import Foundation
import Testing
@testable import Trek

/// `/side` and the side chat's sheet: what the composer takes for the command, what goes to the
/// Mac, and where the sheet goes with its answer.
@MainActor
struct SideChatTests {
    @Test func slashSideIsTheCommandWithOrWithoutAQuestion() {
        #expect(SideCommand.question("/side") == "")
        #expect(SideCommand.question("  /side  ") == "")
        #expect(SideCommand.question("/side what does retry() do?") == "what does retry() do?")
        #expect(SideCommand.question("/Side   Why twice? ") == "Why twice?")
        #expect(SideCommand.question("/side\nand a second line") == "and a second line")
        // Another command that starts the same, and a message that only mentions it.
        #expect(SideCommand.question("/sidebar") == nil)
        #expect(SideCommand.question("/sides of the box") == nil)
        #expect(SideCommand.question("try /side for that") == nil)
        #expect(SideCommand.question("/new thread") == nil)
        #expect(SideCommand.question("") == nil)
    }

    @Test func thePickerListsItAmongTreksOwn() {
        let mac = DemoMac.commands()
        let listed = SideCommand.adding(to: mac)
        #expect(listed.count == mac.count + 1)
        let at = try? #require(listed.firstIndex { $0.name == "side" })
        // After the last of Trek's own, before the agent's first.
        if let at {
            #expect(listed[at].trek == true)
            #expect(listed[at - 1].trek == true)
            #expect(listed[at + 1].trek != true)
        }
        // Typed, it's found like any other.
        #expect(CommandPicker.matches(listed, "si").first?.name == "side")
        #expect(CommandPicker.matches(listed, "").count == listed.count)
        // The list isn't in yet: the picker waits for the Mac's rather than show one row.
        #expect(SideCommand.adding(to: []).isEmpty)
        // A Mac that lists one of its own keeps it.
        #expect(SideCommand.adding(to: listed) == listed)
    }

    @Test func whatGoesToTheMac() {
        let find = ClientMessage.sideChat(threadId: "t", text: nil, images: []).json(id: "4")
        #expect(find as NSDictionary == ["type": "side_chat", "thread_id": "t", "id": "4"] as NSDictionary)
        let start = ClientMessage.sideChat(threadId: "t", text: "What does retry() do?",
                                           images: [ImageUpload(mime: "image/png", data: "iVBORw0K")]).json(id: "5")
        #expect(start as NSDictionary == [
            "type": "side_chat", "thread_id": "t", "id": "5", "text": "What does retry() do?",
            "images": [["mime": "image/png", "data": "iVBORw0K"]],
        ] as NSDictionary)
        #expect(MacFeature.sideChat.rawValue == "side_chat")
    }

    /// The same lines the Mac's protocol tests round-trip.
    @Test func theMacsAnswerIsRead() throws {
        let found = #"{"type":"side_chat","re":"5","thread_id":"t","side":{"id":"s","title":"What does retry() do?","project":{"id":"p","name":"trek","hue":24,"monogram":"TR"},"agent":{"key":"codex","name":"Codex"},"model":null,"model_label":null,"run_state":"working","needs":null,"section":"settled","unseen":false,"pinned":false,"branch":null,"worktree":false,"activity":null,"working_since":null,"updated_at":1791000000000,"additions":0,"deletions":0}}"#
        guard case .sideChat(let re, let reply) = try ServerMessage.decode(Data(found.utf8)) else {
            Issue.record("not a side chat")
            return
        }
        #expect(re == "5")
        #expect(reply.threadId == "t")
        #expect(reply.side?.id == "s")
        #expect(reply.side?.runState == .working)
        guard case .sideChat(_, let none) = try ServerMessage.decode(Data(#"{"type":"side_chat","re":"4","thread_id":"t"}"#.utf8)) else {
            Issue.record("not a side chat")
            return
        }
        #expect(none.side == nil)
    }

    private func row(_ id: String, _ title: String) -> ThreadSummary {
        ThreadSummary(id: id, title: title, project: nil, agent: AgentRef(key: "codex", name: "Codex"), model: nil, modelLabel: nil,
                      runState: .idle, needs: nil, section: .settled, unseen: false, pinned: false, branch: nil, worktree: false,
                      activity: nil, workingSince: nil, updatedAt: 0, additions: 0, deletions: 0)
    }

    /// The Mac is asked by the thread's own id; the side chat's row comes back under this Mac's
    /// name, in no list, and is the one the sheet opens on next time.
    @Test func aThreadsSideChatIsFoundAndRemembered() throws {
        let backend = RecordingBackend()
        let link = HostLink(paired: .demo, backend: backend)
        let model = AppModel()
        model.links = [link]
        var got: Result<String?, SideChatFailure>?
        model.findSideChat(of: "demo:t") { got = $0 }
        let asked = try #require(backend.sent.last)
        guard case .sideChat(let thread, let text, let images) = asked.message else {
            Issue.record("not side_chat: \(asked.message)")
            return
        }
        #expect(thread == "t")
        #expect(text == nil)
        #expect(images.isEmpty)

        model.handle(.sideChat(re: asked.id, SideChatReply(threadId: "t", side: row("s", "Where is the QR drawn?"))), from: link)
        #expect(got == .success("demo:s"))
        #expect(model.thread("demo:s")?.title == "Where is the QR drawn?")
        #expect(!model.threads.contains { $0.id == "demo:s" }, "no list holds it")
        #expect(model.sideChats["demo:t"] == "demo:s")

        // Gone on the Mac since: found no more, and forgotten here.
        model.findSideChat(of: "demo:t") { got = $0 }
        model.handle(.sideChat(re: backend.sent.last?.id, SideChatReply(threadId: "t", side: nil)), from: link)
        #expect(got == .success(nil))
        #expect(model.sideChats["demo:t"] == nil)
    }

    @Test func startingOneSendsItsFirstMessageAndARefusalIsSaidInTheMacsWords() throws {
        let backend = RecordingBackend()
        let link = HostLink(paired: .demo, backend: backend)
        let model = AppModel()
        model.links = [link]
        var got: Result<String?, SideChatFailure>?
        model.startSideChat(beside: "demo:t", text: "What does retry() do?") { got = $0 }
        let asked = try #require(backend.sent.last)
        guard case .sideChat(let thread, let text, _) = asked.message else {
            Issue.record("not side_chat: \(asked.message)")
            return
        }
        #expect(thread == "t")
        #expect(text == "What does retry() do?")
        model.handle(.error(re: asked.id, code: .notFound, message: "Codex isn't set up on this Mac"), from: link)
        #expect(got == .failure(SideChatFailure(message: "Codex isn't set up on this Mac")))
        #expect(model.sideChats["demo:t"] == nil)
    }

    /// Where the sheet goes once the Mac has said which side chat the thread has.
    @Test func theSheetFollowsWhatTheMacFinds() {
        #expect(SideChatPhase.finding.found("demo:s", fresh: false) == .chat("demo:s"))
        #expect(SideChatPhase.finding.found(nil, fresh: false) == .draft)
        // Shown at once from memory, and still the one.
        #expect(SideChatPhase.chat("demo:s").found("demo:s", fresh: false) == .chat("demo:s"))
        // A newer one was started on the Mac; or the one shown is gone.
        #expect(SideChatPhase.chat("demo:s").found("demo:s2", fresh: false) == .chat("demo:s2"))
        #expect(SideChatPhase.chat("demo:s").found(nil, fresh: false) == .draft)
        // "New side chat" was tapped meanwhile: the draft stays.
        #expect(SideChatPhase.draft.found("demo:s", fresh: true) == .draft)
        #expect(SideChatPhase.draft.found("demo:s", fresh: false) == .chat("demo:s"))
    }

    @Test func aMacThatCantBeAskedLeavesWhatsOnScreen() {
        #expect(SideChatPhase.finding.failed("Not connected to that machine.") == .unavailable("Not connected to that machine."))
        #expect(SideChatPhase.chat("demo:s").failed("x") == .chat("demo:s"))
        #expect(SideChatPhase.draft.failed("x") == .draft)
    }

    @Test func theDraftSaysWhatASideChatIs() {
        let said = SideChatDraftWords.blurb(agent: "Codex", project: "atlas")
        #expect(said.contains("of its own in atlas, so"))
        // A hyphenated name isn't broken over two lines at its hyphen.
        #expect(SideChatDraftWords.blurb(agent: "Codex", project: "trek-api").contains("trek\u{2060}-\u{2060}api"))
        #expect(said.contains("Codex carries on in the thread"))
        // A thread in no project: it names no place.
        #expect(SideChatDraftWords.blurb(agent: "Codex", project: nil).hasPrefix("A side chat has a session of its own, so Codex"))
    }
}
