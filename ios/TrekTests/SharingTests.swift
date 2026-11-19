import Foundation
import Testing
@testable import Trek

extension AppModel {
    /// A model whose user has already said yes to every kind of recipient, kept apart from the
    /// app's own settings: for tests of what happens after the question.
    func allowSharing() {
        sharingDefaults = UserDefaults(suiteName: "trek-tests-\(UUID().uuidString)")!
        sharingConsent = SharingConsent(version: SharingConsent.currentVersion,
                                        recipients: ["anthropic", "openai", SharingRecipient.otherID, SharingRecipient.localID], at: Date())
    }

    /// A model that has never been asked, whatever the simulator's own settings hold.
    static func neverAsked() -> AppModel {
        let model = AppModel()
        model.sharingDefaults = UserDefaults(suiteName: "trek-tests-\(UUID().uuidString)")!
        model.sharingConsent = nil
        return model
    }
}

private func row(_ id: String, agent: AgentRef) -> ThreadSummary {
    ThreadSummary(id: id, title: "A thread", project: nil, agent: agent, model: nil, modelLabel: nil,
                  runState: .idle, needs: nil, section: .settled, unseen: false, pinned: false, branch: nil, worktree: false,
                  activity: nil, workingSince: nil, updatedAt: 0, additions: 0, deletions: 0)
}

private let claude = AgentRef(key: "claude-code", name: "Claude Code")
private let codex = AgentRef(key: "codex", name: "Codex")

/// A paired computer that records what reaches it, with a Claude Code thread and a Codex one.
private func paired() -> (AppModel, HostLink, RecordingBackend) {
    let backend = RecordingBackend()
    let link = HostLink(paired: .demo, backend: backend)
    link.connection = .connected
    let model = AppModel.neverAsked()
    model.links = [link]
    model.threads = [row("demo:t", agent: claude), row("demo:c", agent: codex)]
    return (model, link, backend)
}

struct SharingConsentTests {
    @Test func theFirstSendAsksAndSendsNothing() throws {
        let (model, _, backend) = paired()
        model.send("Fix the flaky test", to: "demo:t", mode: nil)
        #expect(backend.sent.isEmpty, "nothing leaves before a yes")
        let ask = try #require(model.sharingAsk)
        #expect(ask.recipient == SharingRecipient(agent: "Claude Code", company: "Anthropic"))
        #expect(ask.machine == model.links[0].name)
    }

    @Test func notNowSendsNothingAndTheDraftStays() throws {
        let (model, link, backend) = paired()
        let composer = ComposerState()
        composer.draft = "Fix the flaky test"
        // As the thread screen does: asked before the composer is cleared.
        var sent = false
        let may = model.mayShare(with: model.sharingRecipient(thread: "demo:t"), on: link, then: { sent = true })
        #expect(!may)
        model.answerSharing(try #require(model.sharingAsk), allow: false)
        #expect(!sent)
        #expect(backend.sent.isEmpty)
        #expect(composer.draft == "Fix the flaky test")
        #expect(model.sharingAsk == nil)
        #expect(model.sharingConsent == nil, "a no is not remembered: the next send asks again")
        #expect(SharingConsent.load(model.sharingDefaults) == nil)
    }

    /// A send that reached the model without the screen asking first (a chip, a page's button):
    /// "Not now" comes back as a refusal, so whatever cleared itself puts its words back.
    @Test func notNowOnASendInFlightComesBackAsNotSent() throws {
        let (model, _, backend) = paired()
        let composer = ComposerState()
        composer.draft = "Looks good"
        composer.cleared(after: composer.draft)
        model.send("Looks good", to: "demo:t", mode: nil, failed: { _ in composer.restore(text: "Looks good", photos: []) })
        model.answerSharing(try #require(model.sharingAsk), allow: false)
        #expect(backend.sent.isEmpty)
        #expect(composer.draft == "Looks good")
        #expect(model.toast?.text == SharingConsent.notSent)
    }

    @Test func swipingTheSheetAwayIsNotNow() throws {
        let (model, link, backend) = paired()
        var declined = false
        _ = model.mayShare(with: model.sharingRecipient(thread: "demo:t"), on: link, then: {}, declined: { declined = true })
        model.sharingDismissed()
        #expect(declined)
        #expect(backend.sent.isEmpty)
        #expect(model.sharingConsent == nil)
    }

    @Test func allowSendsWhatWaitedAndIsRemembered() throws {
        let (model, _, backend) = paired()
        model.send("Fix the flaky test", to: "demo:t", mode: nil)
        model.answerSharing(try #require(model.sharingAsk), allow: true)
        guard case .send(let thread, let text, _, _, _) = try #require(backend.sent.last).message else {
            Issue.record("not a send")
            return
        }
        #expect(thread == "t")
        #expect(text == "Fix the flaky test")

        // The next one goes without a question, and so does a new launch's.
        model.send("And the other one", to: "demo:t", mode: nil)
        #expect(model.sharingAsk == nil)
        #expect(backend.sent.count == 2)
        let kept = try #require(SharingConsent.load(model.sharingDefaults))
        #expect(kept.version == SharingConsent.currentVersion)
        #expect(kept.recipients == ["anthropic"])
        #expect(kept.covers(.of(claude)))
    }

    @Test func aProviderNotYetNamedIsAskedAbout() throws {
        let (model, _, backend) = paired()
        model.send("One", to: "demo:t", mode: nil)
        model.answerSharing(try #require(model.sharingAsk), allow: true)
        #expect(backend.sent.count == 1)

        // Codex sends to OpenAI, which the first sheet never named.
        model.send("Two", to: "demo:c", mode: nil)
        #expect(backend.sent.count == 1)
        let ask = try #require(model.sharingAsk)
        #expect(ask.recipient.company == "OpenAI")
        model.answerSharing(ask, allow: true)
        #expect(backend.sent.count == 2)
        #expect(model.sharingConsent?.recipients == ["anthropic", "openai"])
    }

    @Test func aNewWordingAsksAgain() throws {
        let (model, _, backend) = paired()
        let old = SharingConsent(version: SharingConsent.currentVersion - 1, recipients: ["anthropic"], at: Date())
        old.save(model.sharingDefaults)
        model.sharingConsent = SharingConsent.load(model.sharingDefaults)
        #expect(model.sharingConsent == old)
        #expect(!old.covers(.of(claude)))
        #expect(!old.stands)

        model.send("Fix it", to: "demo:t", mode: nil)
        #expect(backend.sent.isEmpty)
        model.answerSharing(try #require(model.sharingAsk), allow: true)
        #expect(backend.sent.count == 1)
        // The yes to the old wording isn't carried over.
        #expect(model.sharingConsent?.version == SharingConsent.currentVersion)
        #expect(model.sharingConsent?.recipients == ["anthropic"])
    }

    @Test func theDemoIsNeverAsked() {
        let model = AppModel.neverAsked()
        let link = HostLink(paired: .demo, backend: MockHost())
        model.links = [link]
        model.threads = [row("demo:t", agent: claude)]
        var failed = false
        model.send("Hello", to: "demo:t", mode: nil, failed: { _ in failed = true })
        #expect(model.sharingAsk == nil)
        #expect(!failed)
        #expect(model.mayShare(with: .of(claude), on: link, then: {}))
        #expect(model.sharingConsent == nil, "and nothing is recorded for it")
    }

    @Test func withdrawingAsksAgain() throws {
        let (model, _, backend) = paired()
        model.send("One", to: "demo:t", mode: nil)
        model.answerSharing(try #require(model.sharingAsk), allow: true)
        #expect(model.sharingLine.hasPrefix("Allowed "))
        #expect(model.sharingLine.hasSuffix(": Anthropic"))

        model.withdrawSharing()
        #expect(SharingConsent.load(model.sharingDefaults) == nil)
        #expect(model.sharingLine == "Trek asks before your first message goes to an agent")
        model.send("Two", to: "demo:t", mode: nil)
        #expect(backend.sent.count == 1)
        #expect(model.sharingAsk != nil)
    }

    @Test func aNewThreadAnAnswerAndAFileAreHeldToo() async throws {
        let (model, link, backend) = paired()
        link.agents = [try JSONDecoder().decode(AgentOption.self, from: Data(#"{"key":"codex","name":"Codex","models":[]}"#.utf8))]
        model.newThread(project: "demo:p", agent: "codex", model: nil, text: "Start here", worktree: false) { _ in }
        #expect(backend.sent.isEmpty)
        #expect(model.sharingAsk?.recipient == SharingRecipient(agent: "Codex", company: "OpenAI"))
        model.sharingDismissed()

        model.answer("demo:t", requestId: "q1", .questions([QuestionAnswer(question: "Which branch?", answer: "main")]))
        #expect(backend.sent.isEmpty)
        model.sharingDismissed()

        model.startSideChat(beside: "demo:t", text: "What does retry() do?") { _ in }
        #expect(backend.sent.isEmpty)
        model.sharingDismissed()

        await #expect(throws: UploadError.self) {
            _ = try await model.upload([], on: link) { _, _ in }
        }
        #expect(backend.sent.isEmpty)
    }

    /// Taps that carry none of the user's words go as before: an approval, reading a thread,
    /// looking for a side chat.
    @Test func whatCarriesNoWordsIsNotHeld() {
        let (model, _, backend) = paired()
        model.answer("demo:t", requestId: "q1", .approval(.deny))
        model.findSideChat(of: "demo:t") { _ in }
        model.interrupt("demo:t")
        #expect(model.sharingAsk == nil)
        #expect(backend.kinds == ["answer", "side_chat", "interrupt"])
    }
}

struct SharingWordsTests {
    @Test func aKnownProviderIsNamed() {
        let words = SharingWords(.of(claude), machine: "Studio")
        #expect(words.title == "Send to Claude Code?")
        #expect(words.computer == "What you type, dictate or attach here goes to Studio, your paired computer.")
        #expect(words.agent == "From there Claude Code sends it to Anthropic, a cloud AI service, under the account you use on that computer.")
        #expect(words.trek == "Trek has no servers in between and keeps no copy on servers of its own.")
        #expect(words.dictation == nil)
        #expect(SharingWords(.of(codex), machine: "Studio").agent.contains("sends it to OpenAI"))
    }

    @Test func anAgentThePhoneCantPlaceGetsTheGeneralWords() {
        let open = SharingRecipient.of(key: "opencode", name: "OpenCode")
        #expect(open.company == nil)
        #expect(open.id == SharingRecipient.otherID)
        #expect(SharingWords(open, machine: "Studio").agent
            == "From there it goes to OpenCode, which may send it to a cloud AI service, depending on how it's set up on that computer.")
        let unknown = SharingWords(SharingRecipient(), machine: "Studio")
        #expect(unknown.title == "Send to your agent?")
        #expect(unknown.agent.hasPrefix("From there it goes to the agent you chose there, which may send it to a cloud AI service"))
    }

    @Test func aModelServerOnTheComputerIsSaidToRunThere() {
        let ollama = SharingRecipient.of(key: "direct:ollama", name: "Ollama")
        #expect(ollama.local)
        #expect(ollama.id == SharingRecipient.localID)
        #expect(SharingWords(ollama, machine: "Studio").agent
            == "From there it goes to Ollama, which runs on that computer. It stays there unless you set it up otherwise.")
        // A direct provider with a key is that company's cloud.
        #expect(SharingRecipient.of(key: "direct:anthropic", name: "Anthropic API").id == "anthropic")
    }

    @Test func dictationThatLeavesThePhoneIsSaid() {
        let words = SharingWords(.of(claude), machine: "Studio", dictationOnDevice: false)
        #expect(words.dictation == "Dictation in your language isn't done on this iPhone: Apple turns your speech into text.")
    }

    @Test func thePolicyIsTheWebsites() {
        #expect(SharingConsent.policy.absoluteString == "https://trekdesktop.com/privacy/")
    }
}

struct DistributionTests {
    @Test func theFlagMakesAStoreBuildWhateverElseIsTrue() {
        #expect(Distribution.channel(compiledForStore: true, simulator: false, hasProfile: false) == .store)
        #expect(Distribution.channel(compiledForStore: true, simulator: false, hasProfile: true) == .store)
        #expect(Distribution.channel(compiledForStore: true, simulator: true, hasProfile: false) == .store)
    }

    @Test func aBuildXcodeSignedForThePhoneIsDevelopment() {
        #expect(Distribution.channel(compiledForStore: false, simulator: false, hasProfile: true) == .development)
        #expect(Distribution.channel(compiledForStore: false, simulator: true, hasProfile: false) == .development)
    }

    /// The backstop: an archive made without the flag has no profile once the store has signed it.
    @Test func onAPhoneWithoutAProfileItCameFromTheStore() {
        #expect(Distribution.channel(compiledForStore: false, simulator: false, hasProfile: false) == .store)
    }

    @Test func thisBuildIsDevelopment() {
        // The tests run in a simulator, on a build without TREK_APP_STORE.
        #expect(!Distribution.compiledForStore)
        #expect(Distribution.channel == .development)
        #expect(Distribution.updatesFromMac)
    }

    @Test func aStoreBuildIsOfferedNoUpdateFromAMac() {
        let link = HostLink(paired: .demo, backend: RecordingBackend())
        link.host = HostInfo(id: "demo", name: "Studio", version: "99.0.0")
        link.release = TrekRelease(version: "99.0.1", note: "Faster")
        #expect(AppUpdate.available(in: [link], app: "0.4.57", channel: .development)?.version == "99.0.1")
        #expect(AppUpdate.available(in: [link], app: "0.4.57", channel: .store) == nil)
    }
}
