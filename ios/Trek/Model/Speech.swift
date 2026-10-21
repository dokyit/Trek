import AVFoundation
import Speech
import SwiftUI

// MARK: Heard

/// What dictation has heard so far: the rounds the recognizer has closed by itself, and the one it
/// is still revising (every partial result replaces it wholesale, which is how "it is can" becomes
/// "it is. Can you make the button?").
struct Dictated: Equatable {
    private(set) var settled = ""
    private(set) var live = ""

    /// Everything heard, one space between rounds.
    var text: String {
        [settled, live]
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            .filter { !$0.isEmpty }
            .joined(separator: " ")
    }

    var isEmpty: Bool { text.isEmpty }

    /// A partial result for the round under way.
    mutating func revise(_ partial: String) { live = partial }

    /// The recognizer closed the round (it does that after a pause): keep it and start the next.
    mutating func settle() {
        let round = live.trimmingCharacters(in: .whitespacesAndNewlines)
        live = ""
        guard !round.isEmpty else { return }
        settled = settled.isEmpty ? round : settled + " " + round
    }

    mutating func clear() {
        settled = ""
        live = ""
    }

    /// What the field holds when the user keeps these words: after whatever was typed, with a
    /// space between when both are there (nothing added when the typed text already ends in
    /// whitespace or a newline).
    func merged(into typed: String) -> String {
        let heard = text
        guard !heard.isEmpty else { return typed }
        guard !typed.isEmpty else { return heard }
        return typed.last?.isWhitespace == true ? typed + heard : typed + " " + heard
    }
}

// MARK: Listening

/// Which `SFSpeechAudioBufferRecognitionRequest` the audio tap feeds. A request is one round: when
/// the recognizer closes it the tap stays put and the box is pointed at a fresh request, so the
/// engine never has to stop. Touched from the real-time audio thread and the main actor, so the
/// swap goes through a lock rather than the actor.
/// A weak reference that can cross into the audio thread's closures; read only on the main actor.
nonisolated private final class Weak<T: AnyObject>: @unchecked Sendable {
    weak var value: T?
    init(_ value: T) { self.value = value }
}

nonisolated private final class RequestBox: @unchecked Sendable {
    private let lock = NSLock()
    private var request: SFSpeechAudioBufferRecognitionRequest?

    func feed(_ request: SFSpeechAudioBufferRecognitionRequest) {
        lock.lock()
        self.request = request
        lock.unlock()
    }

    func append(_ buffer: AVAudioPCMBuffer) {
        lock.lock()
        request?.append(buffer)
        lock.unlock()
    }

    func end() {
        lock.lock()
        request?.endAudio()
        request = nil
        lock.unlock()
    }
}

/// Voice input for the composer: the microphone through `AVAudioEngine`, the words from
/// `SFSpeechRecognizer` (on this iPhone when it can), revised live until the user keeps or drops
/// them. Nothing here touches the Mac: dictation works whether or not one is connected.
@MainActor @Observable final class Dictation {
    enum Stage: Equatable { case off, listening }

    private(set) var stage: Stage = .off
    private(set) var heard = Dictated()
    /// Loudness of the last couple of seconds, 0…1, newest last: the composer's waveform.
    private(set) var levels: [Float] = []
    var listening: Bool { stage == .listening }

    private var recognizer: SFSpeechRecognizer?
    private var engine: AVAudioEngine?
    private var request: SFSpeechAudioBufferRecognitionRequest?
    private var task: SFSpeechRecognitionTask?
    private var onFailure: ((String) -> Void)?

    /// The tap keeps its identity across rounds; this box is how it finds the open request.
    private let box = RequestBox()

    /// Levels arrive per audio buffer; the view only wants ~14 bars a second, so the loudest level
    /// since the last bar wins and the rest are dropped.
    private var pendingLevel: Float = 0
    private var lastLevelAt = Date.distantPast
    /// Rounds restarted within a blink of each other: a recognizer that closes instantly every time
    /// is broken, and restarting it forever would spin.
    private var restarts = 0
    private var lastRestartAt = Date.distantPast
    /// Which round is open. A callback from a round already replaced still arrives sometimes (a
    /// benign error landing after its `isFinal`); without this stamp it would open another round on
    /// top of the live one and orphan its task.
    private var generation = 0

    /// Ask for the microphone and for speech recognition, then listen until `keep()` or `drop()`.
    /// `failed` gets one plain sentence whenever it can't start or recognition breaks.
    func start(failed: @escaping (String) -> Void) async {
        guard !listening else { return }
        onFailure = failed

        guard await microphone() else {
            return fail("Trek needs the microphone to hear you. Turn it on in Settings › Trek.")
        }
        guard await speechRecognition() else {
            return fail("Trek needs Speech Recognition to turn what you say into text. Turn it on in Settings › Trek.")
        }
        guard let recognizer = SFSpeechRecognizer(locale: .current) else {
            let language = Locale.current.language.languageCode
                .flatMap { Locale.current.localizedString(forLanguageCode: $0.identifier) } ?? "your language"
            return fail("Speech recognition isn't available for \(language) on this iPhone.")
        }
        guard recognizer.isAvailable else {
            return fail("Speech recognition isn't available right now. Try again in a moment.")
        }

        do {
            let session = AVAudioSession.sharedInstance()
            try session.setCategory(.record, mode: .measurement, options: .duckOthers)
            try session.setActive(true)

            let engine = AVAudioEngine()
            self.engine = engine
            self.recognizer = recognizer
            stage = .listening
            openRound()

            let input = engine.inputNode
            let format = input.outputFormat(forBus: 0)
            let me = Weak(self)
            input.installTap(onBus: 0, bufferSize: 1024, format: format, block: Self.tap(box) { level in
                Task { @MainActor in me.value?.push(level) }
            })
            try engine.start()
        } catch {
            fail("The microphone isn't available right now. Another app may be using it.")
        }
    }

    /// Stop listening and hand back what was heard ("" when nothing was).
    @discardableResult func keep() -> String {
        heard.settle()
        let text = heard.text
        finish()
        return text
    }

    /// Stop listening and throw the words away.
    func drop() { finish() }

    // MARK: rounds

    /// A round is one request feeding off the shared tap. Opening a fresh one when the recognizer
    /// closes the last is what keeps listening going through a pause instead of ending at it.
    private func openRound() {
        guard let recognizer else { return }
        // The outgoing round ends before the next begins. Its cancelled callback is stamped with
        // the old generation, so it can't come back and open a third.
        task?.cancel()
        request?.endAudio()
        generation += 1
        let round = generation
        let request = SFSpeechAudioBufferRecognitionRequest()
        request.shouldReportPartialResults = true
        request.addsPunctuation = true
        request.taskHint = .dictation
        request.requiresOnDeviceRecognition = recognizer.supportsOnDeviceRecognition
        self.request = request
        box.feed(request)
        let me = Weak(self)
        task = recognizer.recognitionTask(with: request, resultHandler: Self.results { text, final, errored, domain, code, description in
            Task { @MainActor in
                me.value?.round(round, text: text, final: final, errored: errored,
                                domain: domain, code: code, description: description)
            }
        })
    }

    // MARK: off the main actor

    /// The permission prompts answer on a queue of their own, so their handlers are made here too.
    nonisolated private static func askMicrophone() async -> Bool {
        await withCheckedContinuation { done in
            AVAudioApplication.requestRecordPermission { done.resume(returning: $0) }
        }
    }

    nonisolated private static func askSpeech() async -> Bool {
        await withCheckedContinuation { done in
            SFSpeechRecognizer.requestAuthorization { done.resume(returning: $0 == .authorized) }
        }
    }

    // The app builds with the main actor as the default isolation, so a closure written inside this
    // class would be the main actor's, and Swift traps when the audio thread or the recognizer's
    // queue calls it. These two are made in `nonisolated` functions, so they belong to no actor.

    /// The microphone tap, run on the real-time audio thread: feeds the open round and reports the
    /// buffer's loudness (0…1).
    nonisolated private static func tap(_ box: RequestBox, level: @escaping @Sendable (Float) -> Void) -> AVAudioNodeTapBlock {
        { buffer, _ in
            box.append(buffer)
            guard let channel = buffer.floatChannelData?[0] else { return }
            let frames = Int(buffer.frameLength)
            var sum: Float = 0
            for i in 0..<frames { sum += channel[i] * channel[i] }
            let db = 20 * log10(max(sqrt(sum / Float(max(frames, 1))), 1e-7))
            level(max(0, min(1, (db + 50) / 50)))
        }
    }

    /// The recognizer's callback, run on its own queue. `SFSpeechRecognitionResult` and the error
    /// aren't Sendable, so only their plain values go on.
    nonisolated private static func results(
        _ deliver: @escaping @Sendable (String?, Bool, Bool, String, Int, String?) -> Void
    ) -> (SFSpeechRecognitionResult?, (any Error)?) -> Void {
        { result, error in
            let nsError = error as NSError?
            deliver(result?.bestTranscription.formattedString, result?.isFinal ?? false, error != nil,
                    nsError?.domain ?? "", nsError?.code ?? 0, nsError?.localizedDescription)
        }
    }

    /// One callback's worth of a round, on the main actor: a partial result revises the live words;
    /// a closed round (`isFinal`, or the recognizer ending it on a benign no-speech error) settles
    /// them and hands the tap a fresh request. Only a real failure stops listening.
    private func round(_ round: Int, text: String?, final: Bool, errored: Bool,
                       domain: String, code: Int, description: String?) {
        guard listening, round == generation else { return }
        if let text, !text.isEmpty { heard.revise(text) }
        guard final || errored else { return }

        heard.settle()
        guard !errored || benign(domain: domain, code: code) else {
            fail(description ?? "Speech recognition isn't available right now. Try again in a moment.")
            return
        }
        let now = Date()
        restarts = now.timeIntervalSince(lastRestartAt) < 0.3 ? restarts + 1 : 0
        lastRestartAt = now
        guard restarts <= 10 else {
            fail("Speech recognition stopped working. Tap the microphone to try again.")
            return
        }
        openRound()
    }

    /// Errors the recognizer raises when there's simply nothing to transcribe, and the cancellation
    /// a stopped round raises itself: none of them mean listening has to end.
    private func benign(domain: String, code: Int) -> Bool {
        if code == NSURLErrorCancelled { return true }
        return domain == "kAFAssistantErrorDomain" && [203, 216, 301, 1107, 1110].contains(code)
    }

    // MARK: meters and permissions

    /// A bar for the strip, at most every ~70 ms; older than 64 bars have scrolled off the left.
    private func push(_ level: Float) {
        guard listening else { return }
        pendingLevel = max(pendingLevel, level)
        let now = Date()
        guard now.timeIntervalSince(lastLevelAt) >= 0.07 else { return }
        lastLevelAt = now
        levels.append(pendingLevel)
        pendingLevel = 0
        if levels.count > 64 { levels.removeFirst(levels.count - 64) }
    }

    private func microphone() async -> Bool {
        switch AVAudioApplication.shared.recordPermission {
        case .granted: return true
        case .denied: return false
        default:
            return await Self.askMicrophone()
        }
    }

    private func speechRecognition() async -> Bool {
        switch SFSpeechRecognizer.authorizationStatus() {
        case .authorized: return true
        case .denied, .restricted: return false
        case .notDetermined:
            return await Self.askSpeech()
        @unknown default: return false
        }
    }

    /// Everything stops: tap off, request ended, task cancelled, engine down, session released (a
    /// session that won't deactivate isn't worth a toast), and the heard words and bars cleared.
    private func finish() {
        engine?.inputNode.removeTap(onBus: 0)
        box.end()
        task?.cancel()
        engine?.stop()
        try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
        recognizer = nil
        engine = nil
        request = nil
        task = nil
        heard.clear()
        levels = []
        pendingLevel = 0
        restarts = 0
        stage = .off
    }

    private func fail(_ reason: String) {
        let report = onFailure
        onFailure = nil
        finish()
        report?(reason)
    }
}
