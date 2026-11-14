import Foundation
import Testing
@testable import Trek

/// A sub-agent's row opening it (feature `sub_agent_view`): what a Mac's tool item says of it, a
/// row of its own in the transcript, what goes to the Mac for one of the agent's own, its answer,
/// and the demo Mac's sub-agents growing, stopping and taking messages.
@MainActor
struct SubAgentViewTests {
    /// As `crates/trek-remote/src/protocol/sub_agent_view.rs` writes them.
    static let ownRow = #"{"id":"i4","seq":4,"at":null,"kind":"tool","call_id":"toolu_1","tool":"agent","title":"Agent","detail":"Scout the parser","status":"running","output":"","added":null,"removed":null,"sub_agent":{"agent":{"key":"claude-code","name":"Claude Code","logo":"claude-code"},"label":"Scout the parser","detail":"Reading src/a.rs · 2 steps","state":"running","since":1791000000000,"newer_mac_field":1}}"#
    static let trekRow = #"{"id":"i5","seq":5,"at":null,"kind":"tool","call_id":"trek-task:kid","tool":"other","title":"Sub-agent","detail":"Second opinion","status":"done","output":"Looks fine.","added":null,"removed":null,"sub_agent":{"agent":{"key":"codex","name":"Codex"},"label":"Sol: Second opinion","state":"done","took_secs":42,"thread_id":"kid"}}"#
    static let olderRow = #"{"id":"i6","seq":6,"at":null,"kind":"tool","call_id":"toolu_2","tool":"agent","title":"Agent","detail":"Scout","status":"running","output":"","added":null,"removed":null}"#

    private func item(_ json: String) throws -> TItem {
        try ServerMessage.decoder.decode(TItem.self, from: Data(json.utf8))
    }

    private func call(_ i: TItem) -> ToolCall? {
        if case .tool(let c) = i.body { c } else { nil }
    }

    @Test func aToolItemSaysWhichSubAgentItStandsFor() throws {
        let own = try #require(call(item(Self.ownRow)))
        #expect(own.subAgent?.label == "Scout the parser")
        #expect(own.subAgent?.state == .running)
        #expect(own.subAgent?.since == 1_791_000_000_000)
        #expect(own.subAgent?.threadId == nil)
        #expect(own.subAgent?.agent.logo == "claude-code")
        let trek = try #require(call(item(Self.trekRow)))
        #expect(trek.subAgent?.threadId == "kid")
        #expect(trek.subAgent?.tookSecs == 42)
        #expect(trek.subAgent?.detail == nil)
        // An older Mac's: no sub-agent, the row as before.
        #expect(try call(item(Self.olderRow))?.subAgent == nil)
    }

    /// The transcript cache writes items as the wire has them, and reads them back.
    @Test func itsSubAgentSurvivesTheCache() throws {
        let i = try item(Self.ownRow)
        let e = JSONEncoder()
        e.keyEncodingStrategy = .convertToSnakeCase
        let back = try ServerMessage.decoder.decode(TItem.self, from: e.encode(i))
        #expect(back == i)
    }

    /// A sub-agent's row stands alone, as on the Mac; an older Mac's folds with the other calls.
    @Test func aSubAgentsRowIsARowOfItsOwn() throws {
        let read = TItem(id: "i1", seq: 1, body: .tool(ToolCall(callId: "c1", tool: .read, title: "Read", detail: "a.rs", status: .done,
                                                                output: "", added: nil, removed: nil)))
        let blocks = Block.build([read, try item(Self.ownRow), try item(Self.olderRow), read]).blocks
        #expect(blocks.count == 3)
        guard case .group(let first) = blocks[0].kind, case .subAgent(let c) = blocks[1].kind, case .group(let rest) = blocks[2].kind else {
            Issue.record("\(blocks.map(\.kind))")
            return
        }
        #expect(first.count == 1)
        #expect(c.callId == "toolu_1")
        #expect(rest.count == 2, "the older Mac's row folds with the call after it")
        // It counts as work in its turn.
        #expect(Block.turnEnds([blocks[1]], open: false).isEmpty)
    }

    @Test func aTrekSubAgentsThreadIsThisMachines() throws {
        let link = HostLink(paired: .demo, backend: RecordingBackend())
        let q = AppModel().qualify(try item(Self.trekRow), on: link)
        #expect(call(q)?.subAgent?.threadId == "demo:kid")
    }

    @Test func whatGoesToTheMac() {
        let read = ClientMessage.subAgent(threadId: "t", callId: "toolu_1").json(id: "3")
        #expect(read as NSDictionary == ["type": "sub_agent", "thread_id": "t", "call_id": "toolu_1", "id": "3"] as NSDictionary)
        let stop = ClientMessage.subAgentAction(threadId: "t", callId: "toolu_1", action: .stop, text: nil).json(id: "4")
        #expect(stop as NSDictionary == ["type": "sub_agent_action", "thread_id": "t", "call_id": "toolu_1", "action": "stop", "id": "4"] as NSDictionary)
        let say = ClientMessage.subAgentAction(threadId: "t", callId: "c", action: .message, text: "Skip the tests").json(id: nil)
        #expect(say["action"] as? String == "message")
        #expect(say["text"] as? String == "Skip the tests")
        #expect(MacFeature.subAgentView.rawValue == "sub_agent_view")
    }

    @Test func theMacsAnswerIsRead() throws {
        let full = #"{"type":"sub_agent","re":"7","thread_id":"t","call_id":"toolu_1","link":{"agent":{"key":"claude-code","name":"Claude Code"},"label":"Scout","detail":"Reading a.rs","state":"running"},"prompt":"Scout the parser.","items":[{"id":"s0","seq":0,"at":null,"kind":"assistant","text":"Looking.","streaming":false},{"id":"s1","seq":1,"at":null,"kind":"tool","call_id":"t1","tool":"read","title":"Read","detail":"a.rs","status":"done","output":"fn a()","added":null,"removed":null},{"id":"s2","seq":2,"kind":"from_the_future"}],"earlier":3,"can_stop":true,"can_message":false,"no_message":"Claude Code doesn't take messages for its sub-agents while they run.","asking":true}"#
        guard case .subAgent(let re, let log) = try ServerMessage.decode(Data(full.utf8)) else {
            Issue.record("not a sub_agent")
            return
        }
        #expect(re == "7")
        #expect(log.callId == "toolu_1")
        #expect(log.prompt == "Scout the parser.")
        #expect(log.items.count == 3)
        #expect(log.items.map(\.id) == ["s0", "s1", "s2"])
        #expect(log.more == 3)
        #expect(log.canStop && !log.canMessage && log.asking && !log.gone)
        #expect(log.noMessage?.hasPrefix("Claude Code") == true)
        #expect(log.keepsChanging)
        // What a Mac leaves out: nothing it can do, nothing more to ask for once it's done.
        let bare = #"{"type":"sub_agent","thread_id":"t","call_id":"c","link":{"agent":{"key":"codex","name":"Codex"},"label":"x","state":"done"},"gone":true}"#
        guard case .subAgent(_, let done) = try ServerMessage.decode(Data(bare.utf8)) else {
            Issue.record("not a sub_agent")
            return
        }
        #expect(done.items.isEmpty && !done.canStop && !done.canMessage && done.gone && !done.keepsChanging)
        // A transcript's `more` still reads as it did.
        guard case .transcript(_, _, _, _, _, let more) = try ServerMessage.decode(Data(#"{"type":"transcript","thread_id":"t","reset":true,"seq":1,"items":[],"more":true}"#.utf8)) else {
            Issue.record("not a transcript")
            return
        }
        #expect(more)
    }

    /// A row opens Trek's own in its thread, the agent's own over the thread, and nothing on a
    /// Mac that serves neither.
    @Test func aRowOpensWhatTheMacServes() throws {
        let backend = RecordingBackend()
        let link = HostLink(paired: .demo, backend: backend)
        let model = AppModel()
        model.links = [link]
        let own = try #require(call(item(Self.ownRow))?.subAgent)
        #expect(!model.open(subAgent: own, threadId: nil, callId: "toolu_1", in: "demo:t"), "an older Mac")
        #expect(model.openSubAgent == nil)
        link.features = [MacFeature.subAgents.rawValue, MacFeature.subAgentView.rawValue]
        #expect(model.open(subAgent: own, threadId: nil, callId: "toolu_1", in: "demo:t"))
        #expect(model.openSubAgent == SubAgentTarget(threadId: "demo:t", callId: "toolu_1", link: own))
        model.openSubAgent = nil
        var opened: String?
        let trek = try #require(call(model.qualify(item(Self.trekRow), on: link))?.subAgent)
        #expect(model.open(subAgent: trek, threadId: nil, callId: "trek-task:kid", in: "demo:t", open: { opened = $0 }))
        #expect(opened == "demo:kid")
        #expect(model.openSubAgent == nil)

        // Read and stopped by the parent's id, bare on the wire.
        var got: Result<SubAgentLog, SubAgentFailure>?
        model.fetchSubAgent(SubAgentTarget(threadId: "demo:t", callId: "toolu_1")) { got = $0 }
        let asked = try #require(backend.sent.last)
        guard case .subAgent(let tid, let cid) = asked.message else { Issue.record("\(asked.message)"); return }
        #expect(tid == "t" && cid == "toolu_1")
        model.handle(.error(re: asked.id, code: .badRequest, message: "It has already finished."), from: link)
        #expect(got == .failure(SubAgentFailure(message: "It has already finished.")))
    }

    // MARK: The demo Mac

    @Test func theDemoMacsSubAgentGrowsWhileItWorksThenStops() throws {
        DemoSubAgents.runs = [:]
        let start = Date()
        let first = try #require(DemoSubAgents.log("t-inbox", "toolu-scout", now: start))
        #expect(first.link.state == .running)
        #expect(first.canStop && !first.canMessage)
        #expect(first.noMessage?.contains("Claude Code") == true)
        #expect(first.prompt?.isEmpty == false)
        let later = try #require(DemoSubAgents.log("t-inbox", "toolu-scout", now: start.addingTimeInterval(20)))
        #expect(later.items.count > first.items.count, "more steps as it works")
        #expect(Set(later.items.map(\.id)).isSuperset(of: first.items.map(\.id)), "ids stay put")
        // Claude Code takes no message for it: the Mac refuses, in plain words.
        guard case .failure(let refused) = DemoSubAgents.act("t-inbox", "toolu-scout", .message, text: "hi", now: start.addingTimeInterval(21)) else {
            Issue.record("took a message")
            return
        }
        #expect(refused.message.contains("doesn't take messages"))
        guard case .success(let stopped) = DemoSubAgents.act("t-inbox", "toolu-scout", .stop, text: nil, now: start.addingTimeInterval(22)) else {
            Issue.record("didn't stop")
            return
        }
        #expect(stopped.link.state == .stopped)
        #expect(!stopped.canStop && !stopped.keepsChanging)
        #expect(!stopped.items.contains { if case .tool(let c) = $0.body { c.status == .running } else { false } })
        // Asked again later, it's as it was when it stopped.
        #expect(DemoSubAgents.log("t-inbox", "toolu-scout", now: start.addingTimeInterval(60))?.items.count == stopped.items.count)
    }

    @Test func aCodexSubAgentTakesAMessageAndADoneOneShowsItsAnswer() throws {
        DemoSubAgents.runs = [:]
        let start = Date()
        _ = DemoSubAgents.log("t-profile", "agent-layout", now: start)
        guard case .success(let told) = DemoSubAgents.act("t-profile", "agent-layout", .message, text: "Only the code rows", now: start.addingTimeInterval(5)) else {
            Issue.record("refused")
            return
        }
        #expect(told.canMessage)
        #expect(told.items.contains { if case .user(let t, _, _) = $0.body { t == "Only the code rows" } else { false } })
        let reply = try #require(DemoSubAgents.log("t-profile", "agent-layout", now: start.addingTimeInterval(8)))
        #expect(reply.items.last.map { if case .assistant = $0.body { true } else { false } } == true, "it answers")
        let done = try #require(DemoSubAgents.log("t-ratelimit", "agent-counters", now: start))
        #expect(done.link.state == .done)
        #expect(done.answer?.contains("Redis") == true)
        #expect(!done.canStop && !done.canMessage && !done.keepsChanging)
        // The demo threads have the rows: Trek's own opens its thread.
        let rows = DemoSubAgents.rows(in: "t-inbox").compactMap { if case .tool(let c) = $0 { c.subAgent } else { nil } }
        #expect(rows.count == 3)
        #expect(rows.first?.threadId == "t-inbox-review")
        #expect(MockHost.features.contains("sub_agent_view"))
    }
}
