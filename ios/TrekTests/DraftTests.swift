import Foundation
import Testing
@testable import Trek

/// What's typed and not sent: kept per thread and per machine, bounded, and gone after a month.
@MainActor
struct DraftBookTests {
    private let t0 = Date(timeIntervalSince1970: 1_800_000_000)

    @Test func aDraftIsKeptPerThreadAndPerMachine() {
        var book = DraftBook()
        let did1 = book.set("mac:t1", "for the Mac", now: t0)
        #expect(did1)
        book.set("pc:t1", "for the PC", now: t0)
        book.set("mac:t2", "another thread", now: t0)
        #expect(book.text("mac:t1") == "for the Mac")
        #expect(book.text("pc:t1") == "for the PC", "the same thread id on another machine is another draft")
        #expect(book.text("mac:t3") == nil)
        // Saved again unchanged: nothing to write.
        let did2 = book.set("mac:t1", "for the Mac", now: t0.addingTimeInterval(5))
        #expect(!did2)
        // Unpairing a machine takes its drafts, and only its.
        let did3 = book.forget(host: "mac")
        #expect(did3)
        #expect(book.text("mac:t1") == nil && book.text("mac:t2") == nil)
        #expect(book.text("pc:t1") == "for the PC")
    }

    @Test func clearingAndWhitespaceLeaveNoDraft() {
        var book = DraftBook()
        book.set("mac:t1", "half a thought", now: t0)
        let did4 = book.clear("mac:t1")
        #expect(did4)
        #expect(book.text("mac:t1") == nil)
        let did5 = book.clear("mac:t1")
        #expect(!did5)
        // Emptied in the composer: the same as cleared.
        book.set("mac:t1", "half a thought", now: t0)
        book.set("mac:t1", "  \n ", now: t0)
        #expect(book.text("mac:t1") == nil)
        #expect(book.isEmpty)
        // What's typed is kept as typed, its trailing space too.
        book.set("mac:t1", "and then ", now: t0)
        #expect(book.text("mac:t1") == "and then ")
    }

    @Test func onlyTheNewestAreKept() {
        var book = DraftBook()
        for i in 0..<(DraftBook.keep + 5) {
            book.set("mac:t\(i)", "draft \(i)", now: t0.addingTimeInterval(Double(i)))
        }
        #expect(book.entries.count == DraftBook.keep)
        #expect(book.text("mac:t0") == nil && book.text("mac:t4") == nil, "the ones left longest go")
        #expect(book.text("mac:t5") == "draft 5")
        #expect(book.text("mac:t\(DraftBook.keep + 4)") == "draft \(DraftBook.keep + 4)")
        // Touching an old one makes it the newest: it outlives the next ones in.
        book.set("mac:t5", "draft 5, changed", now: t0.addingTimeInterval(1000))
        book.set("mac:new", "one more", now: t0.addingTimeInterval(1001))
        #expect(book.text("mac:t5") == "draft 5, changed")
        #expect(book.text("mac:t6") == nil)
    }

    @Test func aVeryLongDraftIsCut() {
        var book = DraftBook()
        book.set("mac:t1", String(repeating: "a", count: DraftBook.maxLength + 500), now: t0)
        #expect(book.text("mac:t1")?.count == DraftBook.maxLength)
    }

    @Test func aDraftLeftForAMonthGoes() {
        var book = DraftBook()
        book.set("mac:old", "from last month", now: t0)
        book.set("mac:new", "from this week", now: t0.addingTimeInterval(25 * 86400))
        let did6 = book.prune(now: t0.addingTimeInterval(29 * 86400))
        #expect(!did6)
        let did7 = book.prune(now: t0.addingTimeInterval(31 * 86400))
        #expect(did7)
        #expect(book.text("mac:old") == nil)
        #expect(book.text("mac:new") == "from this week")
    }

    @Test func theFileBringsThemBackAfterARelaunch() throws {
        let url = FileManager.default.temporaryDirectory.appending(path: "trek-drafts-\(UUID().uuidString)").appending(path: "drafts.json")
        defer { try? FileManager.default.removeItem(at: url.deletingLastPathComponent()) }
        var clock = t0
        let first = DraftStore(url: url, now: { clock })
        first.set("mac:t1", "not sent yet")
        first.set(DraftBook.newThread, "an idea for a new thread")
        // The app leaves the front: on disk before iOS can end it.
        first.flush()
        #expect(FileManager.default.fileExists(atPath: url.path))

        let relaunched = DraftStore(url: url, now: { clock })
        #expect(relaunched.text("mac:t1") == "not sent yet")
        #expect(relaunched.text(DraftBook.newThread) == "an idea for a new thread")

        // Sent: gone from the file too.
        relaunched.clear("mac:t1")
        relaunched.flush()
        #expect(DraftStore(url: url, now: { clock }).text("mac:t1") == nil)

        // A month on, what's left has expired when the app starts.
        clock = t0.addingTimeInterval(DraftBook.maxAge + 60)
        let later = DraftStore(url: url, now: { clock })
        #expect(later.text(DraftBook.newThread) == nil)
        later.flush()
        #expect(!FileManager.default.fileExists(atPath: url.path), "no drafts, no file")
    }
}

/// The thread's composer is the model's: what's typed survives leaving the thread and a relaunch,
/// and a message the Mac doesn't take comes back to it, screen or no screen.
@MainActor
struct ComposerDraftTests {
    private func setUp(_ backend: Backend, drafts: DraftStore = DraftStore(url: nil)) -> (AppModel, HostLink) {
        let link = HostLink(paired: .demo, backend: backend)
        link.connection = .connected
        let model = AppModel(drafts: drafts)
        model.allowSharing()   // asked and allowed earlier (SharingTests)
        model.links = [link]
        return (model, link)
    }

    @Test func whatIsTypedIsThereWhenTheThreadOpensAgain() {
        let (model, link) = setUp(RecordingBackend())
        let tid = link.qualified("t1")
        // The thread's screen comes up and something is typed.
        model.subscribe(tid)
        model.composer(tid).draft = "Also check the migration"
        // Back to the list, and into the thread again: the same composer, as it was.
        model.unsubscribe(tid)
        model.subscribe(tid)
        #expect(model.composer(tid).draft == "Also check the migration")
        #expect(model.drafts.text(tid) == "Also check the migration")
        // Another thread has its own.
        #expect(model.composer(link.qualified("t2")).draft.isEmpty)
    }

    @Test func aRelaunchRestoresTheDraft() {
        let url = FileManager.default.temporaryDirectory.appending(path: "trek-drafts-\(UUID().uuidString).json")
        defer { try? FileManager.default.removeItem(at: url) }
        let (model, link) = setUp(RecordingBackend(), drafts: DraftStore(url: url))
        let tid = link.qualified("t1")
        model.composer(tid).draft = "Half a message"
        // iOS takes the app to the background and ends it there.
        model.backgrounded()

        let (again, _) = setUp(RecordingBackend(), drafts: DraftStore(url: url))
        #expect(again.composer(tid).draft == "Half a message")
        // Read back, not the keyboard's echo of a message just sent: it stays.
        #expect(!again.composer(tid).dropEcho())
    }

    @Test func aMessageRefusedAfterTheThreadWasLeftGoesBackToItsDraft() throws {
        let backend = RecordingBackend()
        let (model, link) = setUp(backend)
        let tid = link.qualified("t1")
        model.subscribe(tid)
        model.composer(tid).draft = "Run the tests again"
        // Send, as the thread's screen does: the composer empties as the message goes.
        let held = model.takeComposed(tid)
        #expect(model.composer(tid).draft.isEmpty)
        #expect(model.drafts.text(tid) == "Run the tests again", "kept until the Mac has it")
        model.send("Run the tests again", to: tid, mode: nil, held: held)
        let sent = try #require(backend.sent.last { if case .send = $0.message { true } else { false } })
        // The user goes back to the list before the Mac answers.
        model.unsubscribe(tid)

        model.handle(.error(re: sent.id, code: .hostError, message: "That thread is busy."), from: link)
        #expect(model.composer(tid).draft == "Run the tests again")
        #expect(model.drafts.text(tid) == "Run the tests again")
        #expect(model.toast?.text == "Not sent. Your message is back in the thread's draft. That thread is busy.")
        #expect(model.toast?.isError == true)
    }

    @Test func refusedWithTheThreadStillOpenTheToastIsTheMacsWords() throws {
        let backend = RecordingBackend()
        let (model, link) = setUp(backend)
        let tid = link.qualified("t1")
        model.subscribe(tid)
        model.composer(tid).draft = "Run the tests again"
        let held = model.takeComposed(tid)
        model.send("Run the tests again", to: tid, mode: nil, held: held)
        // Something typed while it was on its way stays, after the message that came back.
        model.composer(tid).draft = "and the linter"
        let sent = try #require(backend.sent.last { if case .send = $0.message { true } else { false } })
        model.handle(.error(re: sent.id, code: .hostError, message: "That thread is busy."), from: link)
        #expect(model.composer(tid).draft == "Run the tests again\nand the linter")
        #expect(model.drafts.text(tid) == "Run the tests again\nand the linter")
        #expect(model.toast?.text == "That thread is busy.", "the words are back in the composer in view")
    }

    @Test func theDraftIsDroppedOnceTheMacTakesTheMessage() throws {
        let backend = RecordingBackend()
        let (model, link) = setUp(backend)
        let tid = link.qualified("t1")
        model.subscribe(tid)
        model.composer(tid).draft = "Looks good, merge it"
        let held = model.takeComposed(tid)
        var took = false
        model.send("Looks good, merge it", to: tid, mode: nil, held: held, sent: { took = true })
        #expect(model.drafts.text(tid) == "Looks good, merge it")
        let sent = try #require(backend.sent.last { if case .send = $0.message { true } else { false } })
        model.handle(.ack(re: sent.id, threadId: nil), from: link)
        #expect(took)
        #expect(model.drafts.text(tid) == nil)
        #expect(model.composer(tid).draft.isEmpty)
    }

    @Test func aDroppedConnectionSaysItMayHaveGone() throws {
        let backend = RecordingBackend()
        let (model, link) = setUp(backend)
        let tid = link.qualified("t1")
        model.composer(tid).draft = "Ship it"
        let held = model.takeComposed(tid)
        model.send("Ship it", to: tid, mode: nil, held: held)
        // No screen is up for the thread, and the connection goes before the ack.
        link.failPending()
        #expect(model.composer(tid).draft == "Ship it")
        let toast = try #require(model.toast?.text)
        #expect(toast.hasSuffix(SendFailure.backInDraft))
        #expect(!toast.hasPrefix("Not sent"), "it may have gone: the toast says to check")
        #expect(toast.contains("Check that it went through"))
    }

    @Test func theToastsWords() {
        #expect(SendFailure.toast(reason: "Codex isn't signed in.", code: .hostError, away: false) == "Codex isn't signed in.")
        #expect(SendFailure.toast(reason: "Codex isn't signed in.", code: .hostError, away: true)
            == "Not sent. Your message is back in the thread's draft. Codex isn't signed in.")
        #expect(SendFailure.toast(reason: "", code: .notSent, away: true) == "Not sent. Your message is back in the thread's draft.")
        #expect(SendFailure.toast(reason: "Studio didn't answer. Check that it went through before trying again.", code: .timedOut, away: true)
            == "Studio didn't answer. Check that it went through before trying again. Your message is back in the thread's draft.")
    }

    @Test func aThreadRemovedOnTheMacTakesItsDraft() {
        let (model, link) = setUp(RecordingBackend())
        let tid = link.qualified("t1")
        model.composer(tid).draft = "Never sent"
        model.composer(link.qualified("t2")).draft = "Another thread's"
        model.handle(.threadRemoved("t1"), from: link)
        #expect(model.drafts.text(tid) == nil)
        #expect(model.composer(tid).draft.isEmpty)
        #expect(model.drafts.text(link.qualified("t2")) == "Another thread's")
    }

    @Test func aForkedMessageWaitsInTheNewThreadsComposer() throws {
        let backend = RecordingBackend()
        let (model, link) = setUp(backend)
        model.turnAction(link.qualified("t1"), item: "i4", .fork)
        let asked = try #require(backend.sent.last)
        model.handle(.ack(re: asked.id, threadId: "t9", open: nil, text: "Try it with a cache"), from: link)
        #expect(model.openRequest == link.qualified("t9"))
        #expect(model.composer(link.qualified("t9")).draft == "Try it with a cache")
    }
}

/// The new-thread sheet starts from the machine's own defaults.
@MainActor
struct NewThreadStartTests {
    private let agents = [
        AgentOption(key: "claude-code", name: "Claude Code", defaultModel: "claude-opus-5-5", models: [
            ModelOption(id: "claude-opus-5-5", label: "Opus 5.5", efforts: ["low", "high"]),
            ModelOption(id: "claude-sonnet-5-5", label: "Sonnet 5.5", efforts: ["low", "high", "max"]),
        ]),
        AgentOption(key: "codex", name: "Codex", defaultModel: "gpt-6-astra", models: [
            ModelOption(id: "gpt-6-astra", label: "GPT-6 Astra", efforts: ["medium", "xhigh"]),
            ModelOption(id: "gpt-5.6-sol", label: "GPT-5.6 Sol", efforts: ["medium", "xhigh"]),
        ]),
    ]

    private func settings(agent: String, model: String?, effort: String) -> MacSettings {
        var s = DemoMac.settings()
        s.defaultAgent = agent
        s.defaultModel = model
        s.defaultEffort = effort
        return s
    }

    @Test func itStartsFromTheMachinesDefaultAgentAndModel() {
        let start = NewThreadStart.choice(agents: agents, settings: settings(agent: "codex", model: "gpt-5.6-sol", effort: "xhigh"))
        #expect(start == NewThreadStart.Choice(agent: "codex", model: "gpt-5.6-sol", effort: "xhigh"))
    }

    @Test func withoutSettingsItIsTheFirstAgentAsBefore() {
        #expect(NewThreadStart.choice(agents: agents, settings: nil) == NewThreadStart.Choice(agent: "claude-code"))
        #expect(NewThreadStart.choice(agents: [], settings: nil) == NewThreadStart.Choice())
    }

    @Test func aDefaultTheMachineNoLongerOffersFallsBack() {
        // An agent it doesn't list (removed since): the first one.
        #expect(NewThreadStart.choice(agents: agents, settings: settings(agent: "gemini", model: "gemini-4", effort: "high"))
            == NewThreadStart.Choice(agent: "claude-code"))
        // A model the agent doesn't list: the agent's own default; an effort the model doesn't take: the machine's.
        #expect(NewThreadStart.choice(agents: agents, settings: settings(agent: "codex", model: "gpt-4", effort: "max"))
            == NewThreadStart.Choice(agent: "codex"))
        // No default model: the agent's own, with the default effort when it takes it.
        #expect(NewThreadStart.choice(agents: agents, settings: settings(agent: "codex", model: nil, effort: "medium"))
            == NewThreadStart.Choice(agent: "codex", model: nil, effort: "medium"))
        // A dated snapshot of a model it lists is that model.
        #expect(NewThreadStart.choice(agents: agents, settings: settings(agent: "claude-code", model: "claude-sonnet-5-5-20261001", effort: "max"))
            == NewThreadStart.Choice(agent: "claude-code", model: "claude-sonnet-5-5", effort: "max"))
    }
}
