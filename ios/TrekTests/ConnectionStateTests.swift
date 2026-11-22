import SwiftUI
import Testing
import UIKit
@testable import Trek

/// A Mac that can't be reached: nothing sent to it goes.
private final class DeadBackend: Backend {
    var onMessage: ((ServerMessage) -> Void)?
    var onState: ((ConnectionState) -> Void)?
    private(set) var tried = 0

    func start() {}
    func wake() {}
    func stop() {}

    @discardableResult
    func send(_ message: ClientMessage, id: String?) -> Bool {
        tried += 1
        return false
    }
}

/// What the phone does while a Mac can't be reached, or refuses what it's sent: a message that
/// didn't go comes back to the composer, and the screens say which machine is away.
@MainActor
struct ConnectionStateTests {
    private func photo() -> PickedPhoto {
        let image = UIGraphicsImageRenderer(size: CGSize(width: 4, height: 4)).image { _ in }
        return PickedPhoto(image: image)
    }

    // MARK: A message that didn't go

    @Test func aMessageTheMacRefusesComesBackToTheComposer() throws {
        let backend = RecordingBackend()
        let link = HostLink(paired: .demo, backend: backend)
        link.connection = .connected
        let model = AppModel()
        model.allowSharing()   // asked and allowed earlier (SharingTests)
        model.links = [link]
        let composer = ComposerState()
        composer.draft = "Run the tests again"
        composer.photos = [photo()]

        // The order the thread screen uses: the composer clears as the message goes, and a refusal
        // puts it back.
        let held = (text: composer.draft, photos: composer.photos)
        composer.cleared(after: composer.draft)
        #expect(composer.draft.isEmpty && composer.photos.isEmpty)
        model.send("Run the tests again", to: "demo:t", mode: nil, failed: { _ in
            composer.restore(text: held.text, photos: held.photos)
        })
        let asked = try #require(backend.sent.last)
        #expect(composer.draft.isEmpty, "nothing comes back before the Mac has answered")

        model.handle(.error(re: asked.id, code: .hostError, message: "That thread is busy"), from: link)
        #expect(composer.draft == "Run the tests again")
        #expect(composer.photos.count == 1)
    }

    @Test func aMessageToAMacThatCantBeReachedComesBackAtOnce() {
        let backend = DeadBackend()
        let link = HostLink(paired: .demo, backend: backend)
        let model = AppModel()
        model.allowSharing()   // asked and allowed earlier (SharingTests)
        model.links = [link]
        let composer = ComposerState()
        composer.draft = "Add a test"
        let held = composer.draft
        // The refusal comes back inside `send`, before the next line: clearing first keeps it from
        // wiping what was just put back.
        composer.cleared(after: composer.draft)
        model.send("Add a test", to: "demo:t", mode: nil, failed: { _ in composer.restore(text: held, photos: []) })
        #expect(backend.tried == 1)
        #expect(composer.draft == "Add a test")
    }

    @Test func aMessageTheMacTookStaysGone() throws {
        let backend = RecordingBackend()
        let link = HostLink(paired: .demo, backend: backend)
        let model = AppModel()
        model.allowSharing()   // asked and allowed earlier (SharingTests)
        model.links = [link]
        let composer = ComposerState()
        composer.draft = "Looks good"
        composer.cleared(after: composer.draft)
        var took = false
        model.send("Looks good", to: "demo:t", mode: nil, failed: { _ in composer.restore(text: "Looks good", photos: []) }, sent: { took = true })
        model.handle(.ack(re: try #require(backend.sent.last?.id), threadId: nil), from: link)
        #expect(took)
        #expect(composer.draft.isEmpty)
    }

    @Test func whatWasTypedMeanwhileStaysAfterTheMessageThatCameBack() {
        let composer = ComposerState()
        composer.draft = "second thought"
        composer.restore(text: "the first message", photos: [])
        #expect(composer.draft == "the first message\nsecond thought")
        // Nothing typed: just the message.
        let empty = ComposerState()
        empty.restore(text: "the first message", photos: [])
        #expect(empty.draft == "the first message")
        // The keyboard's echo watch doesn't swallow it.
        #expect(!empty.dropEcho())
        #expect(empty.draft == "the first message")
    }

    // MARK: Which machine is away

    @Test func theNotConnectedWordsNameTheMachine() {
        let link = HostLink(paired: .demo, backend: DeadBackend())
        #expect(AppModel.notConnected(link) == "\(link.name) isn't connected.")
        #expect(AppModel.notConnected(nil) == AppModel.notConnected)
    }

    @Test func aMachineThatCantBeReachedIsListedAsAway() {
        let up = HostLink(paired: .demo, backend: DeadBackend())
        up.connection = .connected
        let down = HostLink(paired: PairedMac(address: "10.0.0.2:7420", token: "t", hostName: "Studio", hostId: "studio", transport: .plain),
                            backend: DeadBackend())
        down.connection = .offline("")
        let starting = HostLink(paired: PairedMac(address: "10.0.0.3:7420", token: "t", hostName: "Laptop", hostId: "laptop", transport: .plain),
                                backend: DeadBackend())
        starting.connection = .idle

        let model = AppModel()
        model.links = [up, down, starting]
        model.mode = .live
        #expect(model.unreachable.map(\.id) == [down.id], "connected and not-yet-started machines aren't away")
        #expect(NewThreadAccessory.away(model.unreachable) == "Can't reach Studio")
        down.connection = .unauthorized("Gone")
        #expect(model.unreachable.map(\.id) == [down.id])
        up.connection = .offline("")
        #expect(NewThreadAccessory.away(model.unreachable) == "2 machines offline")
        // The demo's sample machine is never away.
        model.mode = .demo
        #expect(model.unreachable.isEmpty)
    }

    // MARK: Pairing again

    @Test func pairingAgainKeepsTheMachineUntilTheNewPairingReplacesIt() {
        let link = HostLink(paired: .demo, backend: DeadBackend())
        link.connection = .unauthorized("This Mac no longer knows this iPhone. Pair again to carry on.")
        let model = AppModel()
        model.links = [link]
        model.mode = .live
        model.pairingError = "Couldn't reach that Mac."
        model.pairAgain(link)
        #expect(model.pairingAgain === link)
        #expect(model.links.count == 1 && model.links.first === link)
        #expect(model.mode == .live, "the first-run screen doesn't come up")
        #expect(model.pairingError == nil)
    }

    // MARK: Leaving the demo

    /// Settings › Pair with a Mac in the demo: the next launch opens on pairing, not back in the
    /// demo (it left `demoMode` set, and a relaunch before pairing dropped the user back in).
    @Test func pairWithAMacLeavesTheDemoForGood() throws {
        let defaults = UserDefaults.standard
        let was = defaults.object(forKey: "demoMode")
        defer { defaults.set(was, forKey: "demoMode") }

        let model = AppModel()
        model.startDemo()
        #expect(defaults.bool(forKey: "demoMode") && model.mode == .demo)
        model.endDemo()
        #expect(!defaults.bool(forKey: "demoMode"))
        #expect(model.mode != .demo)
        #expect(model.link(id: PairedMac.demo.hostId) == nil)

        // Leaving the last machine by hand (the old way out) forgets the demo too.
        model.startDemo()
        model.leave(try #require(model.link(id: PairedMac.demo.hostId)))
        #expect(!defaults.bool(forKey: "demoMode"))
        #expect(model.mode == .unpaired)
    }

    // MARK: Waiting on a machine

    /// What a page says while what it asked for hasn't come: a spinner only while the machine can
    /// answer ("Asking your Mac…" spun for ever while it was offline).
    @Test func aPageWaitingOnAnOfflineMachineDoesntSpin() {
        let link = HostLink(paired: PairedMac(address: "10.0.0.2:7420", token: "t", hostName: "Studio", hostId: "studio", transport: .plain),
                            backend: DeadBackend())
        link.connection = .connected
        #expect(link.waiting("Asking your Mac…") == ("Asking your Mac…", true))
        link.connection = .offline("")
        let away = link.waiting("Reading the trail…")
        #expect(away.text == "Shown once \(MachineTag.short("Studio")) is connected" && !away.spins)
        link.connection = .connecting
        #expect(!link.waiting("Asking your agents…").spins)
    }

    // MARK: Local Network off

    /// With Local Network off for Trek, the words send the user to iOS Settings and the banner
    /// offers Open Settings; while Tailscale is still being tried, or for anything else, it doesn't.
    @Test func localNetworkOffOffersSettings() {
        let wifi = NetworkPath(kind: .wifi)
        let alone = ReachStatus.searching(ReachFacts(mac: "Studio", path: wifi, localDenied: true))
        #expect(alone.opensSettings && !alone.trying)
        #expect(alone.text.contains("Local Network"))
        let tailnetDown = ReachStatus.searching(ReachFacts(mac: "Studio", path: wifi, localDenied: true, hasTailnet: true, tailnetFailed: true))
        #expect(tailnetDown.opensSettings && !tailnetDown.trying)
        let tryingTailnet = ReachStatus.searching(ReachFacts(mac: "Studio", path: wifi, localDenied: true, hasTailnet: true))
        #expect(!tryingTailnet.opensSettings && tryingTailnet.trying)
        let isolated = ReachStatus.searching(ReachFacts(mac: "Studio", path: wifi, hasLocal: true, localFailed: true, elapsed: 30))
        #expect(!isolated.opensSettings)
    }

    // MARK: Notes

    @Test func noNotesIsTheMacsWordNotSilence() throws {
        let backend = RecordingBackend()
        let link = HostLink(paired: .demo, backend: backend)
        let model = AppModel()
        model.links = [link]
        #expect(!link.notesLoaded && !link.notesFailed)
        model.loadNotes(on: link)
        model.handle(.error(re: try #require(backend.sent.last?.id), code: .unknown, message: "Notes aren't available"), from: link)
        #expect(link.notesFailed && !link.notesLoaded)
        model.loadNotes(on: link)
        model.handle(.notes(re: try #require(backend.sent.last?.id), []), from: link)
        #expect(link.notesLoaded && !link.notesFailed)
        #expect(link.notes.isEmpty)
    }
}
