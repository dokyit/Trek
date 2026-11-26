import Foundation
import Testing
@testable import Trek

/// A request whose answer never comes fails after a while, instead of leaving whoever asked
/// waiting until the socket drops; and a thread whose transcript didn't come says so.
@MainActor
struct ReplyTimeoutTests {
    /// The link's timers, kept to run by hand.
    private final class Timers {
        var waits: [(delay: TimeInterval, work: @MainActor () -> Void)] = []
        @MainActor func fire() {
            let due = waits
            waits = []
            for w in due { w.work() }
        }
    }

    private func setUp(_ backend: Backend = RecordingBackend(), name: String = "Studio") -> (AppModel, HostLink, Timers) {
        let link = HostLink(paired: PairedMac(address: "10.0.0.2:7420", token: "t", hostName: name, hostId: "studio", transport: .plain),
                            backend: backend)
        link.connection = .connected
        let timers = Timers()
        link.after = { delay, work in timers.waits.append((delay, work)) }
        let model = AppModel(drafts: DraftStore(url: nil))
        model.allowSharing()
        model.links = [link]
        return (model, link, timers)
    }

    @Test func aRequestNeverAnsweredFailsWithAClearError() {
        let (_, link, timers) = setUp()
        var heard: [ServerMessage] = []
        link.request(.notes) { heard.append($0) }
        #expect(heard.isEmpty)
        #expect(timers.waits.map(\.delay) == [ReplyTimeout.read])
        timers.fire()
        guard case .error(_, let code, let text)? = heard.first else { Issue.record("no error"); return }
        #expect(code == .timedOut)
        #expect(text == "Studio didn't answer.")
        #expect(heard.count == 1)
        // The answer arriving after all finds no one waiting, and nothing is heard twice.
        link.reply("1", .notes(re: "1", []))
        #expect(heard.count == 1)
    }

    @Test func anAnswerInTimeIsHeardOnceAndTheWaitDoesNothing() {
        let (_, link, timers) = setUp()
        var heard = 0
        let id = link.request(.ping) { _ in heard += 1 }
        link.reply(id, .pong(re: id))
        #expect(heard == 1)
        timers.fire()
        #expect(heard == 1)
    }

    @Test func nothingWaitsForARequestNobodyListensTo() {
        let (_, link, timers) = setUp()
        link.request(.usage)
        #expect(timers.waits.isEmpty)
    }

    /// Uploads and media must not be cut off by the wait for an ordinary read.
    @Test func transfersAndSlowWorkGetLonger() {
        #expect(ReplyTimeout.seconds(for: .subscribe(threadId: "t", afterSeq: nil, limit: 60)) == ReplyTimeout.read)
        #expect(ReplyTimeout.seconds(for: .transcriptBefore(threadId: "t", before: "i9", limit: 200)) == ReplyTimeout.read)
        #expect(ReplyTimeout.seconds(for: .uploadPart(uploadId: "u", name: "clip.mov", total: 10, offset: 0, data: "AAAA")) == ReplyTimeout.slow)
        #expect(ReplyTimeout.seconds(for: .media(threadId: "t", path: "/clip.mov", maxPx: nil, offset: 0, length: 65536)) == ReplyTimeout.slow)
        #expect(ReplyTimeout.seconds(for: .attachment(threadId: "t", itemId: "i1", index: 0, maxPx: 480)) == ReplyTimeout.slow)
        #expect(ReplyTimeout.seconds(for: .noteFile(path: "a.pdf", offset: 0, length: 65536)) == ReplyTimeout.slow)
        #expect(ReplyTimeout.seconds(for: .gitPush(.thread("t"))) == ReplyTimeout.slow)
        // A message: longer than a read (the Mac runs a connection's actions in turn), and as
        // long as a transfer when it carries pictures or names files.
        #expect(ReplyTimeout.seconds(for: .send(threadId: "t", text: "hi", mode: nil, images: [])) == ReplyTimeout.action)
        #expect(ReplyTimeout.seconds(for: .send(threadId: "t", text: "hi", mode: nil, images: [ImageUpload(mime: "image/png", data: "iVBORw0K")]))
            == ReplyTimeout.slow)
        #expect(ReplyTimeout.seconds(for: .send(threadId: "t", text: "hi", mode: nil, images: [], files: ["u1"])) == ReplyTimeout.slow)
        #expect(ReplyTimeout.seconds(for: .answer(threadId: "t", requestId: "r1", response: .approval(.allow))) == ReplyTimeout.action)
        #expect(ReplyTimeout.read < ReplyTimeout.action && ReplyTimeout.action < ReplyTimeout.slow)
        // Nothing is awaited for these.
        #expect(ReplyTimeout.seconds(for: .markSeen(threadId: "t")) == nil)
        #expect(ReplyTimeout.seconds(for: .unsubscribe(threadId: "t")) == nil)
        // Something done says it may have gone through; something read doesn't.
        #expect(ReplyTimeout.words(for: .send(threadId: "t", text: "hi", mode: nil, images: []), machine: "Studio")
            == "Studio didn't answer. Check that it went through before trying again.")
    }

    @Test func aDroppedConnectionAndAMacThatCantBeReachedSayWhich() {
        let (_, link, _) = setUp()
        var code: ErrorCode?
        link.request(.notes) { if case .error(_, let c, _) = $0 { code = c } }
        link.failPending()
        #expect(code == .dropped)
        #expect(ErrorCode.dropped.unsure && ErrorCode.timedOut.unsure && !ErrorCode.notSent.unsure && !ErrorCode.hostError.unsure)
        // The wire never carries the phone's own codes: one it doesn't know reads as unknown.
        #expect(ErrorCode(rawValue: "host_error") == .hostError)
        #expect(!ErrorCode.hostError.isLocal && ErrorCode.timedOut.isLocal)
    }

    /// A reply this app can't read used to be left out, and its request waited for ever.
    @Test func anAnswerThatCantBeReadFailsItsRequest() {
        // `usage` whole is the value: one with a field of the wrong type doesn't decode.
        let bad = Data(#"{"type":"usage","re":"7","providers":"none"}"#.utf8)
        #expect((try? ServerMessage.decode(bad)) == nil)
        guard case .error(let re, let code, _)? = ServerMessage.unreadable(bad) else { Issue.record("no error"); return }
        #expect(re == "7")
        #expect(code == .unreadable)
        // A push that doesn't decode answers nothing: left out, as before.
        #expect(ServerMessage.unreadable(Data(#"{"type":"item","thread_id":"t"}"#.utf8)) == nil)
        #expect(ServerMessage.unreadable(Data("not json".utf8)) == nil)
    }

    // MARK: A thread that couldn't be loaded

    @Test func aThreadWhoseTranscriptNeverComesSaysSoAndTryAgainLoadsIt() throws {
        let backend = RecordingBackend()
        let (model, link, timers) = setUp(backend)
        let tid = link.qualified("t-\(UUID().uuidString)")
        model.subscribe(tid)
        let store = model.store(tid)
        #expect(!store.loaded && store.failure == nil, "spinning")

        // Twenty seconds on, nothing has come.
        timers.fire()
        #expect(store.failure == "Studio didn't answer.")
        #expect(model.toast == nil, "said in the thread, not in a toast over it")

        // Try again: asked afresh, spinning meanwhile, and this time the Mac answers.
        let asks = backend.kinds.count { $0 == "subscribe" }
        model.retryLoad(tid)
        #expect(store.failure == nil)
        #expect(backend.kinds.count { $0 == "subscribe" } == asks + 1)
        let again = try #require(backend.sent.last?.id)
        model.handle(.transcript(re: again, threadId: link.bare(tid), reset: true, seq: 3,
                                 items: [TItem(id: "i0", seq: 3, body: .notice("Here it is"))], more: false), from: link)
        #expect(store.loaded && store.failure == nil)
        timers.fire()
        #expect(store.failure == nil, "the wait for an answer that came does nothing")
    }

    @Test func aTranscriptTheMacRefusesSaysWhy() throws {
        let backend = RecordingBackend()
        let (model, link, _) = setUp(backend)
        let tid = link.qualified("t-\(UUID().uuidString)")
        model.subscribe(tid)
        let asked = try #require(backend.sent.first { if case .subscribe = $0.message { true } else { false } })
        model.handle(.error(re: asked.id, code: .hostError, message: "Couldn't read that thread's transcript."), from: link)
        #expect(model.store(tid).failure == "Couldn't read that thread's transcript.")
        #expect(model.toast == nil)
    }

    /// The answer that comes after the wait ran out is still the transcript: it loads.
    @Test func aLateTranscriptStillLoads() throws {
        let backend = RecordingBackend()
        let (model, link, timers) = setUp(backend)
        let tid = link.qualified("t-\(UUID().uuidString)")
        model.subscribe(tid)
        let asked = try #require(backend.sent.first { if case .subscribe = $0.message { true } else { false } })
        timers.fire()
        #expect(model.store(tid).failure != nil)
        model.handle(.transcript(re: asked.id, threadId: link.bare(tid), reset: true, seq: 1, items: [], more: false), from: link)
        #expect(model.store(tid).loaded)
        #expect(model.store(tid).failure == nil)
    }

    /// Not connected, or the connection went meanwhile: that's the connection banner's to say,
    /// and the new connection asks again.
    @Test func aMacThatCantBeReachedIsNotALoadFailure() {
        let backend = RecordingBackend()
        let (model, link, _) = setUp(backend)
        let tid = link.qualified("t-\(UUID().uuidString)")
        model.subscribe(tid)
        link.failPending()
        #expect(model.store(tid).failure == nil)
    }

    /// The demo Mac not answering a thread's first ask, as `-TrekDemoNoAnswer` makes it.
    @Test func theDemoMacCanLeaveAThreadUnanswered() async throws {
        let mac = MockHost()
        mac.unanswered = ["t-inbox"]
        let link = HostLink(paired: .demo, backend: mac)
        let timers = Timers()
        link.after = { delay, work in timers.waits.append((delay, work)) }
        let model = AppModel(drafts: DraftStore(url: nil))
        model.links = [link]
        mac.onMessage = { [weak model, weak link] in if let model, let link { model.handle($0, from: link) } }
        mac.onState = { [weak link] in link?.connection = $0 }
        mac.start()
        defer { mac.stop() }
        TranscriptCache.removeAll(host: link.id)
        for _ in 0..<100 where model.threads.isEmpty { try await Task.sleep(for: .milliseconds(50)) }
        let tid = link.qualified("t-inbox")
        #expect(model.thread(tid) != nil)

        model.subscribe(tid)
        let store = model.store(tid)
        try await Task.sleep(for: .milliseconds(400))
        #expect(!store.loaded, "the demo Mac said nothing")
        timers.fire()
        #expect(store.failure != nil)

        model.retryLoad(tid)
        for _ in 0..<100 where !store.loaded { try await Task.sleep(for: .milliseconds(50)) }
        #expect(store.loaded && store.failure == nil)
        #expect(!store.items.isEmpty)

        // It says the phone fell behind (nothing rewritten): the transcript is read again and
        // lines up, so the list isn't drawn afresh.
        let generation = store.generation
        let asks = store.seq
        mac.fallBehind("t-inbox")
        try await Task.sleep(for: .milliseconds(600))
        #expect(store.generation == generation)
        #expect(store.seq >= asks)
        model.unsubscribe(tid)
    }
}

/// With two machines paired, an agent's icon is asked of a machine that has the agent.
@MainActor
struct IconHostTests {
    private func machine(_ id: String, agents: [AgentOption]) -> (HostLink, RecordingBackend) {
        let backend = RecordingBackend()
        let link = HostLink(paired: PairedMac(address: "10.0.0.2:7420", token: "t", hostName: id, hostId: id, transport: .plain), backend: backend)
        link.connection = .connected
        link.after = { _, _ in }
        link.agents = agents
        return (link, backend)
    }

    @Test func anAgentOnlyTheOtherMachineHasIsAskedOfThatMachine() {
        let (mac, macSent) = machine("mac", agents: [AgentOption(key: "codex", name: "Codex", defaultModel: nil, models: [])])
        let (pc, pcSent) = machine("pc", agents: [AgentOption(key: "acp:goose", name: "Goose", defaultModel: nil, models: [], icon: "h-goose")])
        let model = AppModel(drafts: DraftStore(url: nil))
        model.links = [mac, pc]
        model.selectedHostId = "mac"
        defer { UserDefaults.standard.removeObject(forKey: "selectedHost") }
        #expect(model.selected === mac)

        #expect(model.iconHost(agent: "acp:goose", hash: "h-goose") === pc)
        // The store's asker is set once per process, by whichever model came first: this test's own.
        IconStore.shared.fetch = nil
        defer { IconStore.shared.fetch = nil }
        model.learnIcons()
        var answered = false
        IconStore.shared.fetch?("h-goose", "acp:goose", nil) { _ in answered = true }
        #expect(pcSent.kinds == ["icon"], "the machine that has the agent")
        #expect(macSent.kinds.isEmpty, "not the picked machine, which doesn't know it")
        #expect(!answered)
    }

    @Test func thePickedMachineIsAskedWhenItHasTheAgentToo() {
        let goose = AgentOption(key: "acp:goose", name: "Goose", defaultModel: nil, models: [], icon: "h-goose")
        let (mac, _) = machine("mac", agents: [goose])
        let (pc, _) = machine("pc", agents: [goose])
        let model = AppModel(drafts: DraftStore(url: nil))
        model.links = [mac, pc]
        model.selectedHostId = "pc"
        defer { UserDefaults.standard.removeObject(forKey: "selectedHost") }
        #expect(model.iconHost(agent: "acp:goose", hash: "h-goose") === pc)
        // One that isn't connected is passed over for one that is.
        model.selectedHostId = "none"
        pc.connection = .offline("")
        #expect(model.iconHost(agent: "acp:goose", hash: "h-goose") === mac)
        // An agent nobody lists: no machine to prefer (the picked one is asked, as before).
        #expect(model.iconHost(agent: "acp:unknown", hash: "h-x") == nil)
    }

    @Test func aThreadsAgentSaysWhichMachineHasIt() {
        let (mac, _) = machine("mac", agents: [])
        let (pc, _) = machine("pc", agents: [])
        let model = AppModel(drafts: DraftStore(url: nil))
        model.links = [mac, pc]
        model.selectedHostId = "mac"
        defer { UserDefaults.standard.removeObject(forKey: "selectedHost") }
        var agent = AgentRef(key: "acp:goose", name: "Goose")
        agent.icon = "h-goose"
        model.threads = [ThreadSummary(id: pc.qualified("t"), title: "t", project: nil, agent: agent, model: nil, modelLabel: nil,
                                       runState: .idle, needs: nil, section: .inbox, unseen: false, pinned: false, branch: nil,
                                       worktree: false, activity: nil, workingSince: nil, updatedAt: 0, additions: 0, deletions: 0)]
        #expect(model.iconHost(agent: "acp:goose", hash: "h-goose") === pc)
    }
}
