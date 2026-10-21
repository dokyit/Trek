import Foundation
import Observation
import SwiftUI

/// What the phone talks to: a Mac over the network (`TrekClient`) or the in-process `MockHost`.
/// Both speak the same messages, so demo mode exercises the same handling as a real Mac.
protocol Backend: AnyObject {
    var onMessage: ((ServerMessage) -> Void)? { get set }
    var onState: ((ConnectionState) -> Void)? { get set }
    func start()
    /// The app is in front again: check the connection now, and reconnect at once if it's gone.
    func wake()
    /// False when the message couldn't go (no connection to the Mac).
    @discardableResult func send(_ message: ClientMessage, id: String?) -> Bool
    func stop()
    /// The connection doesn't answer: drop it and connect afresh now.
    func reconnect()
}

extension Backend {
    func reconnect() {}
}

enum ConnectionState: Equatable {
    case idle
    case connecting
    case connected
    case offline(String)
    case unauthorized(String)
    /// The Mac answered with a certificate other than the one pinned: maybe an impostor on the
    /// network, maybe Trek was reinstalled. Either way the user pairs again; nothing reconnects.
    case identityChanged(expected: String, seen: String?)

    var label: String {
        switch self {
        case .idle: "Not connected"
        case .connecting: "Connecting…"
        case .connected: "Connected"
        case .offline(let why): why.isEmpty ? "Offline" : why
        case .unauthorized(let why): why
        case .identityChanged: "This Mac's identity changed — pair again"
        }
    }
}

enum AppMode: Equatable {
    case unpaired
    case demo
    case live
}

struct Toast: Identifiable, Equatable {
    let id = UUID()
    var text: String
    var isError: Bool
}

@Observable
final class AppModel {
    /// A thread to bring up, from a notification's `trek://open?thread=…` link.
    var openRequest: String?
    /// A thread to scroll the list to (and unfold whatever hides it), without opening it.
    var revealRequest: String?

    var mode: AppMode = .unpaired
    var connection: ConnectionState = .idle
    /// What the connection is doing, in words ("Looking for … on this Wi-Fi…", "Connected over
    /// Tailscale"), while it's connecting, connected or offline.
    var connectionDetail: String?
    var host: HostInfo?
    var threads: [ThreadSummary] = [] {
        didSet { threadsChanged() }
    }
    var projects: [ProjectSummary] = []
    var agents: [AgentOption] = []
    /// The Mac lets threads run with Full access (unlocked in its Permissions settings).
    var fullAccessAllowed = false
    /// The optional requests the Mac serves (`MacFeature`): what it doesn't list isn't offered.
    var features: Set<String> = []
    /// Rows the list leaves out, by id: sub-agents' threads, and threads opened from a search.
    var unlisted: [String: ThreadSummary] = [:]
    /// Each opened thread's own store (its row, transcript and paging), so an update to one
    /// thread redraws only what shows it. Not observed itself: views observe the stores.
    @ObservationIgnored private var stores: [String: ThreadStore] = [:]
    /// The tab bar's counts, kept as values so views that show them don't redraw on every row update.
    private(set) var workingCount = 0
    private(set) var needsYouCount = 0
    var toast: Toast?
    var pairing = false
    var pairingError: String?
    /// A pairing link (deep link or QR code) waiting for the user to confirm it.
    var pendingLink: PairingLink?
    /// How the live connection is carried, for the Settings indicator.
    var transport: Transport?

    // What the Mac has besides threads, as last read (each `load…` asks again).

    /// Plan usage, as the Mac's Usage popover shows it.
    var usage: Usage?
    var usageLoading = false
    /// Basecamp's recap, by range.
    var basecamp: [BasecampRange: Basecamp] = [:]
    var basecampLoading: Set<BasecampRange> = []
    /// Notes, newest first, and the ones opened whole, by id.
    var notes: [NoteSummary] = []
    var openNotes: [String: Note] = [:]
    /// Git status and branches, by thread or project.
    var gitStatus: [GitTarget: GitStatus] = [:]
    var gitBranches: [GitTarget: GitBranches] = [:]
    /// Git work under way (commit, push, switch, merge, remove), by target: views show a spinner.
    var gitBusy: Set<GitTarget> = []
    /// Each thread's slash commands, as the Mac's `/` picker lists them.
    var commands: [String: [CommandInfo]] = [:]
    /// The Mac's settings the phone may change.
    var macSettings: MacSettings?
    /// The Mac asked the phone to open its new-thread sheet (`/new` sent to a thread), in this
    /// project ("" for none). The view that shows the sheet sets it back to nil.
    var newThreadPrompt: String?

    var followUpMode: SendMode {
        didSet { UserDefaults.standard.set(followUpMode.rawValue, forKey: "followUpMode") }
    }

    private var backend: Backend?
    private var nextRequest = 1
    private var replies: [String: (ServerMessage) -> Void] = [:]
    /// How many items a thread opens with; earlier ones come a page at a time as the user scrolls up.
    static let firstPage = 60
    static let earlierPage = 200
    /// Which Mac the transcript cache is for.
    @ObservationIgnored private var cacheHost: String?
    @ObservationIgnored private var cacheSaves: Set<String> = []

    init() {
        followUpMode = SendMode(rawValue: UserDefaults.standard.string(forKey: "followUpMode") ?? "") ?? .steer
    }

    // MARK: Lifecycle

    func boot(demo forceDemo: Bool) {
        if UserDefaults.standard.bool(forKey: "TrekReset") {
            UserDefaults.standard.set(false, forKey: "demoMode")
            PairedMac.clear()
        }
        if forceDemo {
            attach(MockHost(), mode: .demo)
        } else if UserDefaults.standard.bool(forKey: "demoMode") {
            startDemo()
        } else if let paired = PairedMac.load() {
            startLive(paired)
        } else {
            mode = .unpaired
        }
    }

    func startDemo() {
        UserDefaults.standard.set(true, forKey: "demoMode")
        attach(MockHost(), mode: .demo)
    }

    func startLive(_ paired: PairedMac) {
        UserDefaults.standard.set(false, forKey: "demoMode")
        let transport = paired.transport
        let client = TrekClient(address: paired.address, routes: paired.routes, known: paired.known,
                                auth: .hello(deviceId: Device.id, token: paired.token),
                                transport: transport, hostId: paired.hostId, hostName: paired.hostName)
        Self.keepAddresses(of: client, hostId: paired.hostId)
        attach(client, mode: .live)
        self.transport = transport
    }

    /// The paired Mac's client looks for it on this network (Bonjour, by its id), and the
    /// addresses it answers at are kept for the next launch.
    private static func keepAddresses(of client: TrekClient, hostId: String) {
        client.hostId = hostId
        // Where it answered (dialled first next time), the addresses it answers on now (Wi-Fi,
        // tailnet), and those it answered at before: all dialled together next time.
        client.onRemember = { address, routes, known in
            guard var kept = PairedMac.load(), kept.hostId == hostId else { return }
            kept.address = address
            kept.routes = routes
            kept.known = known
            kept.save()
        }
    }

    /// A `trek://pair` link arrived (deep link or QR code). Nothing happens until the user
    /// confirms it in the sheet: a link alone must never pair the phone.
    func offer(_ link: PairingLink) {
        pairingError = nil
        pendingLink = link
    }

    /// Pairs with a Mac from its QR code or typed details; on success the token and the
    /// certificate fingerprint are kept and the connection stays up as the live one. If pairing
    /// fails, a Mac this phone was already paired with is reconnected.
    func pair(address: String, alternates: [String] = [], code: String, transport: Transport) {
        pendingLink = nil
        pairing = true
        pairingError = nil
        let client = TrekClient(address: address, routes: [address] + alternates,
                                auth: .pair(code: code, deviceId: Device.id, deviceName: Device.name), transport: transport)
        client.onPaired = { [weak self, weak client] token, host, kept in
            // Its id (Bonjour finds it by that on any network), the pinned fingerprint, where it
            // answered and every address it says it answers on (the code's, from a Mac that
            // names none): all there for the first launch after this one.
            PairedMac(address: client?.address ?? address, token: token, hostName: host.name, hostId: host.id, transport: kept,
                      routes: client?.routes ?? [address] + alternates, known: client?.known ?? []).save()
            // This client stays the live one: from now on it's the paired Mac's, as at launch.
            if let client { Self.keepAddresses(of: client, hostId: host.id) }
            UserDefaults.standard.set(false, forKey: "demoMode")
            self?.transport = kept
            self?.pairing = false
            self?.mode = .live
            self?.show("Paired with \(host.name)")
        }
        // The pairing screen (with its progress) stays up until the Mac says yes.
        attach(client, mode: .unpaired)
        self.transport = transport
    }

    /// The app came back to the front.
    func foregrounded() {
        backend?.wake()
    }

    /// The user asked to try now (the connection banner): the Mac is looked for afresh.
    func retryNow() {
        backend?.reconnect()
    }

    /// The connection in words: what it's doing while connecting, connected or offline, else
    /// why it stopped.
    var connectionText: String {
        switch connection {
        case .connecting, .connected, .offline: connectionDetail ?? connection.label
        case .idle, .unauthorized, .identityChanged: connection.label
        }
    }

    func leave() {
        backend?.stop()
        backend = nil
        PairedMac.clear()
        UserDefaults.standard.set(false, forKey: "demoMode")
        reset()
        mode = .unpaired
    }

    private func attach(_ b: Backend, mode: AppMode) {
        backend?.stop()
        reset()
        self.mode = mode
        backend = b
        b.onMessage = { [weak self] in self?.handle($0) }
        (b as? TrekClient)?.onDetail = { [weak self] in self?.connectionDetail = $0 }
        b.onState = { [weak self] state in
            guard let self else { return }
            let was = self.connection
            self.connection = state
            if was == .connected, state != .connected { self.failPending() }
            if state == .connected {
                // Resubscribe after a reconnect, from where we were.
                for s in self.stores.values where s.subscribed { self.resubscribe(s) }
                // What the Mac lets this phone do (allowing for a whole session, Full access).
                self.loadSettings()
                // What screens asked for while the Mac couldn't answer, asked again.
                self.reloadAsked()
            }
            if self.pairing, let why = Self.pairingFailure(state) {
                self.pairing = false
                self.backend?.stop()
                self.backend = nil
                if let previous = PairedMac.load() {
                    // Back to the Mac this phone was paired with before.
                    self.startLive(previous)
                    self.show(why, error: true)
                } else {
                    self.pairingError = why
                    self.mode = .unpaired
                }
            }
        }
        b.start()
    }

    /// Why pairing stopped, for the pairing screen; nil while it may still succeed.
    private static func pairingFailure(_ state: ConnectionState) -> String? {
        switch state {
        case .unauthorized(let why): why
        case .identityChanged(let expected, let seen):
            if let seen, seen != expected {
                "That Mac's fingerprint is \(seen), not \(expected). Nothing was sent to it. Check the fingerprint your Mac shows and try again."
            } else {
                "That Mac's certificate doesn't match the pairing code. Nothing was sent to it. Show a new code on your Mac and try again."
            }
        case .offline(let why) where why == TrekClient.unreachable:
            "Couldn't reach that Mac. Check the address, and that this iPhone is on the same network or tailnet."
        case .offline(let why) where why == TrekClient.tailnetUnreachable:
            "Couldn't reach that Mac over Tailscale. Install Tailscale on this iPhone, sign in with the account your Mac uses and turn it on, or pair on the Mac's Wi-Fi instead: Trek on your Mac then gives this iPhone its tailnet address too."
        case .offline(let why): why
        default: nil
        }
    }

    private func reset() {
        // Whoever awaits an answer would otherwise wait for ever (a continuation never resumed).
        failPending("Disconnected from your Mac.")
        threads = []
        projects = []
        agents = []
        stores = [:]
        features = []
        unlisted = [:]
        cacheHost = nil
        replies = [:]
        host = nil
        transport = nil
        connection = .idle
        connectionDetail = nil
        usage = nil
        basecamp = [:]
        basecampLoading = []
        notes = []
        openNotes = [:]
        gitStatus = [:]
        gitBranches = [:]
        gitBusy = []
        commands = [:]
        macSettings = nil
        newThreadPrompt = nil
        notesAsked = false
        basecampAsked = []
        notesSaving = []
        notesToSave = [:]
    }

    // MARK: Incoming

    func handle(_ message: ServerMessage) {
        switch message {
        case .paired(_, _, let host, let features, _), .welcome(_, let host, let features, _):
            self.host = host
            self.features = Set(features)
            cacheHost = host.id
        case .snapshot(let s):
            withAnimation(.snappy) {
                threads = s.threads
                projects = s.projects
                agents = s.agents
                fullAccessAllowed = s.fullAccess ?? false
            }
            // Rows the Mac no longer lists (a sub-agent deleted while this phone was away) go,
            // unless a thread is open on one.
            let kept = Set(s.unlisted.map(\.id))
            unlisted = unlisted.filter { kept.contains($0.key) || stores[$0.key] != nil }
            for t in s.unlisted { setUnlisted(t) }
            learnIcons()
        case .thread(let t):
            if unlisted[t.id] != nil { unlisted[t.id] = nil }
            if let i = threads.firstIndex(where: { $0.id == t.id }) {
                let old = threads[i]
                guard old != t else { break }
                // A working thread's row changes several times a second (its activity): only a
                // move between sections is animated, not every update in place.
                if old.section != t.section || old.runState != t.runState || (old.needs == nil) != (t.needs == nil) {
                    withAnimation(.snappy) { threads[i] = t }
                } else {
                    threads[i] = t
                }
            } else {
                withAnimation(.snappy) { threads.insert(t, at: 0) }
            }
        case .threadRemoved(let id):
            threads.removeAll { $0.id == id }
            unlisted[id] = nil
            if let host = cacheHost ?? currentCacheHost { TranscriptCache.remove(host: host, thread: id) }
        case .transcript(let re, let tid, let reset, let seq, let items, let more):
            if let s = stores[tid] {
                s.applyReply(reset: reset, seq: seq, items: items, more: more)
                if let start = openStarts.removeValue(forKey: tid) {
                    Perf.report("open \(tid): reply after \(Perf.ms(Perf.now - start)), \(items.count) items\(reset ? "" : " since the cache")")
                    Perf.untilFrame("open \(tid): on screen", from: start)
                }
            }
            reply(re, message)
        case .transcriptPage(let re, _, _, _):
            reply(re, message)
        case .item(let tid, let item):
            guard let s = stores[tid], s.subscribed else { return }
            s.enqueue(item)
        case .transcriptReset(let tid):
            // Rewound or rewritten on the Mac: the whole (last page of the) transcript again.
            if let s = stores[tid], s.subscribed { request(.subscribe(threadId: tid, afterSeq: nil, limit: Self.firstPage)) }
        case .ack(let re, _, _, _), .pong(let re):
            reply(re, message)
        case .usage(let re, _), .basecamp(let re, _), .notes(let re, _), .note(let re, _), .gitStatus(let re, _),
             .gitDiff(let re, _), .gitBranches(let re, _), .commands(let re, _, _), .settings(let re, _),
             .searchResults(let re, _), .attachment(let re, _), .backgroundOutput(let re, _), .icon(let re, _),
             .noteAsset(let re, _), .noteImage(let re, _), .noteFileChunk(let re, _):
            reply(re, message)
        case .unlisted(let t):
            setUnlisted(t)
        case .error(let re, let code, let text):
            if let re, replies[re] != nil { reply(re, message) } else if code != .unauthorized { show(text, error: true) }
        case .unknown:
            break
        }
    }

    /// Keeps each opened thread's store in step with its row, and the tab bar's counts.
    private func threadsChanged() {
        if !stores.isEmpty {
            let byID = Dictionary(threads.map { ($0.id, $0) }, uniquingKeysWith: { a, _ in a })
            for (id, s) in stores { s.setSummary(byID[id] ?? unlisted[id]) }
        }
        let working = threads.count { $0.runState == .working }
        let needs = threads.count { $0.waitsOnYou && $0.section != .snoozed }
        if working != workingCount { workingCount = working }
        if needs != needsYouCount { needsYouCount = needs }
        learnIcons()
    }

    private func reply(_ re: String?, _ message: ServerMessage) {
        guard let re, let f = replies.removeValue(forKey: re) else { return }
        f(message)
    }

    // MARK: Outgoing

    @discardableResult
    func request(_ message: ClientMessage, then: ((ServerMessage) -> Void)? = nil) -> String {
        let id = "\(nextRequest)"
        nextRequest += 1
        if let then { replies[id] = then }
        // Not connected: nothing went, and whoever waits for the answer hears so now.
        if backend?.send(message, id: id) != true { reply(id, .error(re: id, code: .unknown, message: Self.notConnected)) }
        return id
    }

    static let notConnected = "Not connected to your Mac."

    /// The connection went: the answers still awaited aren't coming (a request isn't sent again
    /// by itself, the Mac may have done it already), so what waits on them stops waiting.
    private func failPending(_ why: String = "The connection to your Mac dropped. Check that it went through before trying again.") {
        let waiting = replies
        replies.removeAll()
        for (id, f) in waiting { f(.error(re: id, code: .unknown, message: why)) }
    }

    /// Failures of an action become a toast; `ok` runs on success.
    func act(_ message: ClientMessage, ok: ((ServerMessage) -> Void)? = nil) {
        request(message) { [weak self] reply in
            if case .error(_, _, let text) = reply { self?.show(text, error: true) } else { ok?(reply) }
        }
    }

    @ObservationIgnored private var openStarts: [String: Double] = [:]

    /// The thread's store, made on first use.
    func store(_ tid: String) -> ThreadStore {
        if let s = stores[tid] { return s }
        let s = ThreadStore(id: tid, summary: thread(tid))
        s.onChange = { [weak self, weak s] in if let s { self?.scheduleCacheSave(s) } }
        stores[tid] = s
        return s
    }

    /// The thread's store, if it has one.
    func existingStore(_ tid: String) -> ThreadStore? { stores[tid] }

    /// Opens a thread's transcript: what the cache has shows at once, and the Mac sends what's
    /// newer (or the last page, when it's the first time).
    func subscribe(_ tid: String) {
        let s = store(tid)
        guard !s.subscribed else { return }
        if Perf.enabled { openStarts[tid] = Perf.navigated ?? Perf.now; Perf.navigated = nil }
        if !s.loaded, let host = cacheHost ?? currentCacheHost,
           let cached = Perf.measure("cache load", tid, { TranscriptCache.load(host: host, thread: tid) }) {
            s.restore(items: cached.items, seq: cached.seq, more: cached.more)
            if let start = openStarts[tid] { Perf.untilFrame("open \(tid): cache on screen (\(cached.items.count) items)", from: start) }
        }
        s.subscribed = true
        resubscribe(s)
        markSeen(tid)
    }

    private func resubscribe(_ s: ThreadStore) {
        request(.subscribe(threadId: s.id, afterSeq: s.loaded ? s.seq : nil, limit: Self.firstPage))
    }

    func unsubscribe(_ tid: String) {
        guard let s = stores[tid], s.subscribed else { return }
        s.subscribed = false
        s.flushQueued()
        saveCache(s)
        s.trim(to: TranscriptCache.keepItems)
        backend?.send(.unsubscribe(threadId: tid), id: nil)
    }

    /// The page before the earliest item loaded, when there is one.
    func loadEarlier(_ tid: String) {
        guard let s = stores[tid], s.more, !s.loadingEarlier, let before = s.earliestID else { return }
        s.loadingEarlier = true
        let start = Perf.now
        request(.transcriptBefore(threadId: tid, before: before, limit: Self.earlierPage)) { [weak self, weak s] reply in
            guard let s else { return }
            switch reply {
            case .transcriptPage(_, _, let items, let more):
                s.prepend(items, more: more)
                Perf.untilFrame("earlier page (\(items.count) items) on screen", from: start)
            case .error(_, _, let text):
                s.loadingEarlier = false
                self?.show(text, error: true)
            default: s.loadingEarlier = false
            }
        }
    }

    /// The Mac whose transcripts the cache holds: the one answering, else the one paired with.
    private var currentCacheHost: String? {
        switch mode {
        case .demo: host?.id ?? "demo-mac"
        case .live: host?.id ?? PairedMac.load()?.hostId
        case .unpaired: nil
        }
    }

    private func scheduleCacheSave(_ s: ThreadStore) {
        guard !cacheSaves.contains(s.id) else { return }
        cacheSaves.insert(s.id)
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) { [weak self, weak s] in
            guard let self, let s else { return }
            self.cacheSaves.remove(s.id)
            self.saveCache(s)
        }
    }

    private func saveCache(_ s: ThreadStore) {
        guard s.loaded, let host = cacheHost ?? currentCacheHost else { return }
        TranscriptCache.save(host: host, thread: s.id, items: s.items, seq: s.seq, more: s.more)
    }

    // MARK: Turn actions

    /// Text for a thread's composer when it opens (a fork from a message).
    @ObservationIgnored private var drafts: [String: String] = [:]

    func takeDraft(_ tid: String) -> String? { drafts.removeValue(forKey: tid) }

    /// Undo, retry or fork a turn (from its `turn_end` item), or rewind or fork from a message
    /// (its `user` item). A fork opens the new thread; a rewind puts the message in `compose`.
    func turnAction(_ tid: String, item: String, _ action: TurnAction, model: String? = nil, restoreFiles: Bool = true,
                    compose: ((String) -> Void)? = nil) {
        act(.turnAction(threadId: tid, itemId: item, action: action, model: model, restoreFiles: restoreFiles)) { [weak self] reply in
            guard case .ack(_, let newThread, _, let text) = reply else { return }
            switch action {
            case .fork:
                guard let self, let newThread else { self?.show("Forked"); return }
                // Forked from a message: it waits in the new thread's composer.
                if let text { self.drafts[newThread] = text }
                self.openRequest = newThread
            case .rewind, .undo:
                if let text { compose?(text) }
            case .retry:
                break
            }
        }
    }

    func markSeen(_ tid: String) {
        if let i = threads.firstIndex(where: { $0.id == tid }), threads[i].unseen { threads[i].unseen = false }
        backend?.send(.markSeen(threadId: tid), id: nil)
    }

    func send(_ text: String, to tid: String, mode: SendMode?, images: [ImageUpload] = []) {
        act(.send(threadId: tid, text: text, mode: mode, images: images)) { [weak self] reply in
            // `/new`: the Mac leaves its own screen be and asks the phone to open its sheet.
            if case .ack(_, _, let open?, _) = reply, open.screen == "new_thread" { self?.newThreadPrompt = open.projectId ?? "" }
        }
    }

    /// Change a thread's agent, model, effort, access or plan mode. The row changes here at once;
    /// the Mac's own row follows (and puts it back if it refused).
    func setPrefs(_ tid: String, agent: String? = nil, model: String? = nil, effort: String? = nil, access: Access? = nil, plan: Bool? = nil) {
        let before = thread(tid)
        if let i = threads.firstIndex(where: { $0.id == tid }) {
            if let agent, let a = agents.first(where: { $0.key == agent }) {
                threads[i].agent = AgentRef(key: a.key, name: a.name)
                threads[i].model = a.defaultModel
                threads[i].modelLabel = a.models.first { $0.id == a.defaultModel }?.label
            }
            if let model {
                threads[i].model = model
                threads[i].modelLabel = agents.first { $0.key == threads[i].agent.key }?.models.first { $0.id == model }?.label ?? ModelNaming.display(model)
            }
            if let effort { threads[i].effort = effort }
            if let access { threads[i].access = access }
            if let plan { threads[i].plan = plan }
        }
        request(.setPrefs(threadId: tid, agent: agent, model: model, effort: effort, access: access, plan: plan)) { [weak self] reply in
            // Refused: say why, and put the row back as the Mac has it.
            guard case .error(_, _, let text) = reply, let self else { return }
            self.show(text, error: true)
            if let before, let i = self.threads.firstIndex(where: { $0.id == tid }) { self.threads[i] = before }
        }
    }

    func threadAction(_ tid: String, _ action: ThreadAction, done: String? = nil) {
        act(.threadAction(threadId: tid, action: action)) { [weak self] _ in
            if let done { self?.show(done) }
        }
    }

    /// The models `agentKey` offers, and the efforts `modelId` takes.
    func models(of agentKey: String) -> [ModelOption] { agents.first { $0.key == agentKey }?.models ?? [] }
    func efforts(agent agentKey: String, model modelId: String?) -> [String] {
        let models = models(of: agentKey)
        let id = modelId ?? agents.first { $0.key == agentKey }?.defaultModel
        return models.first { $0.id == id }?.efforts ?? []
    }

    func interrupt(_ tid: String) {
        act(.interrupt(threadId: tid))
    }

    func answer(_ tid: String, requestId: String, _ response: AnswerResponse) {
        act(.answer(threadId: tid, requestId: requestId, response: response))
    }

    func newThread(project: String, agent: String, model: String?, text: String, worktree: Bool,
                   effort: String? = nil, access: Access? = nil, plan: Bool = false, images: [ImageUpload] = [],
                   opened: @escaping (String) -> Void) {
        act(.newThread(projectId: project, agent: agent, model: model, text: text, worktree: worktree,
                       effort: effort, access: access, plan: plan, images: images)) { reply in
            if case .ack(_, let tid?, _, _) = reply { opened(tid) }
        }
    }

    /// Pull to refresh. The Mac pushes every change, but one missed (a dropped event, a link
    /// that died quietly) stays missed: a link that answers is asked again for what the screens
    /// show, one that doesn't is replaced at once (a new connection brings the list afresh).
    func refresh() async {
        let alive = await withCheckedContinuation { (c: CheckedContinuation<Bool, Never>) in
            var done = false
            request(.ping) { reply in
                guard !done else { return }
                done = true
                if case .pong = reply { c.resume(returning: true) } else { c.resume(returning: false) }
            }
            DispatchQueue.main.asyncAfter(deadline: .now() + 3) { if !done { done = true; c.resume(returning: false) } }
        }
        guard alive else {
            backend?.reconnect()
            return
        }
        // From where each open thread is: the Mac sends what came after.
        for s in stores.values where s.subscribed { resubscribe(s) }
        loadUsage(quiet: true)
        loadSettings()
    }

    /// What the Notes and Basecamp screens asked for: on a reconnect, asked again where the
    /// answer never came (or, for the notes list, may have changed meanwhile).
    @ObservationIgnored private var notesAsked = false
    @ObservationIgnored private var basecampAsked: Set<BasecampRange> = []

    private func reloadAsked() {
        for id in notesToSave.keys { saveLeftNote(id) }
        if notesAsked { loadNotes() }
        for range in basecampAsked where basecamp[range] == nil && !basecampLoading.contains(range) { loadBasecamp(range) }
    }

    // MARK: Usage and Basecamp

    /// Ask for plan usage (the Mac asks its agents, at most every 30 seconds). Quiet: in the
    /// background, for the Threads toolbar, so a failure isn't worth a toast.
    func loadUsage(quiet: Bool = false) {
        usageLoading = true
        request(.usage) { [weak self] reply in
            guard let self else { return }
            self.usageLoading = false
            switch reply {
            case .usage(_, let u):
                self.usage = u
                // Some agent was still answering: ask again shortly for the rest.
                if u.loading == true { DispatchQueue.main.asyncAfter(deadline: .now() + 3) { [weak self] in self?.loadUsage(quiet: quiet) } }
            case .error(_, _, let text): if !quiet { self.show(text, error: true) }
            default: break
            }
        }
    }

    func loadBasecamp(_ range: BasecampRange) {
        basecampAsked.insert(range)
        basecampLoading.insert(range)
        request(.basecamp(range)) { [weak self] reply in
            guard let self else { return }
            self.basecampLoading.remove(range)
            switch reply {
            case .basecamp(_, let b): withAnimation(.snappy) { self.basecamp[range] = b }
            case .error(_, _, let text): self.show(text, error: true)
            default: break
            }
        }
    }

    // MARK: Notes

    func loadNotes() {
        notesAsked = true
        request(.notes) { [weak self] reply in
            if case .notes(_, let list) = reply { withAnimation(.snappy) { self?.notes = list } }
        }
    }

    /// Read a note whole; `done` gets it.
    func openNote(_ id: String, done: ((Note) -> Void)? = nil) {
        request(.note(id: id)) { [weak self] reply in
            switch reply {
            case .note(_, let n):
                self?.openNotes[id] = n
                done?(n)
            case .error(_, _, let text): self?.show(text, error: true)
            default: break
            }
        }
    }

    func createNote(_ body: String = "", done: ((Note) -> Void)? = nil) {
        request(.createNote(body: body)) { [weak self] reply in
            switch reply {
            case .note(_, let n):
                self?.openNotes[n.id] = n
                self?.loadNotes()
                done?(n)
            case .error(_, _, let text): self?.show(text, error: true)
            default: break
            }
        }
    }

    /// Save a note's text. If it changed on the Mac since it was opened, the Mac refuses
    /// and `conflict` runs (or, without one, the Mac's message shows as a toast). Any other
    /// failure shows as a toast and `failed` runs.
    func saveNote(_ id: String, body: String, done: ((Note) -> Void)? = nil, conflict: (() -> Void)? = nil,
                  failed: (() -> Void)? = nil) {
        let modified = openNotes[id]?.modified
        notesSaving.insert(id)
        request(.saveNote(id: id, body: body, modified: modified)) { [weak self] reply in
            self?.notesSaving.remove(id)
            switch reply {
            case .note(_, let n):
                self?.openNotes[id] = n
                if let i = self?.notes.firstIndex(where: { $0.id == id }) {
                    self?.notes[i].title = n.title
                    self?.notes[i].modified = n.modified
                }
                done?(n)
                self?.saveLeftNote(id)
            case .error(_, let code, let text):
                if code == .conflict, let conflict { conflict() } else {
                    self?.show(text, error: true)
                    failed?()
                }
                // What its closed editor left would be refused the same way.
                if code == .conflict, self?.notesToSave.removeValue(forKey: id) != nil {
                    self?.show("That note changed on your Mac, so your last changes to it weren't saved.", error: true)
                }
            default: break
            }
        }
    }

    /// Notes with a save out, and text a closed editor left to save (sent after the save out
    /// answers, as two out at once would carry the same version and the Mac would refuse the
    /// second; kept, and sent again on a reconnect, when it can't go).
    @ObservationIgnored private var notesSaving: Set<String> = []
    @ObservationIgnored private var notesToSave: [String: String] = [:]

    /// A save of this note is out, waiting for the Mac's answer.
    func savingNote(_ id: String) -> Bool { notesSaving.contains(id) }

    /// Save what a note's editor had when it closed: now, after the save out for it, or when the
    /// connection comes back.
    func saveNoteLeft(_ id: String, body: String) {
        notesToSave[id] = body
        saveLeftNote(id)
    }

    private func saveLeftNote(_ id: String) {
        guard !notesSaving.contains(id), let body = notesToSave.removeValue(forKey: id), openNotes[id]?.body != body else { return }
        saveNote(id, body: body, failed: { [weak self] in
            if self?.notesToSave[id] == nil { self?.notesToSave[id] = body }
        })
    }

    /// Delete a note: the Mac keeps it in the notes folder's Deleted.
    func deleteNote(_ id: String) {
        let before = notes
        withAnimation(.snappy) { notes.removeAll { $0.id == id } }
        openNotes[id] = nil
        notesToSave[id] = nil
        request(.deleteNote(id: id)) { [weak self] reply in
            if case .error(_, _, let text) = reply {
                self?.show(text, error: true)
                self?.notes = before
            }
        }
    }

    // MARK: Git

    func loadGitStatus(_ target: GitTarget) {
        request(.gitStatus(target)) { [weak self] reply in
            switch reply {
            case .gitStatus(_, let s): withAnimation(.snappy) { self?.gitStatus[target] = s }
            case .error(_, _, let text): self?.show(text, error: true)
            default: break
            }
        }
    }

    /// One changed file's diff; `done` gets it.
    func gitDiff(_ target: GitTarget, path: String, done: @escaping (GitDiff) -> Void) {
        request(.gitDiff(target, path: path)) { [weak self] reply in
            switch reply {
            case .gitDiff(_, let d): done(d)
            case .error(_, _, let text): self?.show(text, error: true)
            default: break
            }
        }
    }

    func loadBranches(_ target: GitTarget) {
        request(.gitBranches(target)) { [weak self] reply in
            switch reply {
            case .gitBranches(_, let b): self?.gitBranches[target] = b
            case .error(_, _, let text): self?.show(text, error: true)
            default: break
            }
        }
    }

    /// Commit every change (in a worktree thread, the worktree's).
    func commit(_ target: GitTarget, message: String) {
        git(target, .gitCommit(target, message: message), done: "Committed")
    }

    func push(_ target: GitTarget) {
        git(target, .gitPush(target), done: "Pushed")
    }

    /// Check out another branch (refused in a worktree thread, or while a thread works there).
    func switchBranch(_ target: GitTarget, to branch: String) {
        git(target, .gitSwitch(target, branch: branch), done: "On \(branch)") { [weak self] in self?.loadBranches(target) }
    }

    /// Merge a worktree thread's branch into its base on the Mac.
    func mergeWorktree(_ tid: String) {
        git(.thread(tid), .worktreeMerge(threadId: tid), done: "Merged")
    }

    /// Remove a worktree thread's worktree. When something would be lost the Mac says what and
    /// nothing happens: `confirm` gets its words; call again with `force` once the user agrees.
    func removeWorktree(_ tid: String, deleteBranch: Bool = false, force: Bool = false, confirm: @escaping (String) -> Void) {
        let target = GitTarget.thread(tid)
        gitBusy.insert(target)
        request(.worktreeRemove(threadId: tid, deleteBranch: deleteBranch, force: force)) { [weak self] reply in
            guard let self else { return }
            self.gitBusy.remove(target)
            switch reply {
            case .error(_, .conflict, let text) where !force: confirm(text)
            case .error(_, _, let text): self.show(text, error: true)
            default:
                self.show("Removed the worktree")
                self.loadGitStatus(target)
            }
        }
    }

    private func git(_ target: GitTarget, _ message: ClientMessage, done: String, then: (() -> Void)? = nil) {
        gitBusy.insert(target)
        request(message) { [weak self] reply in
            guard let self else { return }
            self.gitBusy.remove(target)
            if case .error(_, _, let text) = reply { self.show(text, error: true) } else { self.show(done); then?() }
            self.loadGitStatus(target)
        }
    }

    // MARK: Commands and settings

    func loadCommands(_ tid: String) {
        request(.commands(threadId: tid)) { [weak self] reply in
            if case .commands(_, _, let list) = reply { self?.commands[tid] = list }
        }
    }

    func loadSettings() {
        request(.settings) { [weak self] reply in
            if case .settings(_, let s) = reply { self?.macSettings = s }
        }
    }

    /// Change the Mac's settings. Shown at once; the Mac's answer replaces it (or puts it back,
    /// saying why, when it refuses).
    func changeSettings(_ change: SettingsChange, done: (() -> Void)? = nil) {
        let before = macSettings
        if let s = macSettings { macSettings = change.applied(to: s) }
        request(.setSettings(change)) { [weak self] reply in
            switch reply {
            case .settings(_, let s):
                self?.macSettings = s
                done?()
            case .error(_, _, let text):
                self?.show(text, error: true)
                self?.macSettings = before
            default: break
            }
        }
    }

    func show(_ text: String, error: Bool = false) {
        withAnimation(.snappy) { toast = Toast(text: text, isError: error) }
        let id = toast?.id
        DispatchQueue.main.asyncAfter(deadline: .now() + 3) { [weak self] in
            if self?.toast?.id == id { withAnimation(.snappy) { self?.toast = nil } }
        }
    }

    // MARK: Derived

    func thread(_ id: String) -> ThreadSummary? { threads.first { $0.id == id } ?? unlisted[id] }

    func project(_ id: String?) -> ProjectSummary? { projects.first { $0.id == id } }

    var pinned: [ThreadSummary] { threads.filter { $0.section == .pinned } }

    var needsYou: [ThreadSummary] {
        threads.filter { $0.section == .inbox && $0.waitsOnYou }.sorted { $0.updatedAt > $1.updatedAt }
    }

    var working: [ThreadSummary] {
        threads.filter { ($0.section == .working || ($0.section == .inbox && $0.runState == .working)) && !$0.waitsOnYou }
            .sorted { ($0.workingSince ?? $0.updatedAt) > ($1.workingSince ?? $1.updatedAt) }
    }

    var recent: [ThreadSummary] {
        threads.filter { ($0.section == .inbox && !$0.waitsOnYou && $0.runState != .working) || $0.section == .settled }
            .sorted { $0.updatedAt > $1.updatedAt }
    }

}

extension ThreadSummary {
    /// Waits on the user as the Mac's badge counts it (`needs_you`): a request, or a failure not
    /// settled yet. A settled failure is history, as on the Mac (whose row still says "Failed"):
    /// counted, it raised a hand on the tab bar that no row under "Needs you" answered for. Only a
    /// failure can be settled with `needs`: a thread that asks for something is in the inbox.
    var waitsOnYou: Bool { section != .settled && (needs != nil || runState == .needsYou || runState == .failed) }
}

// MARK: Pairing storage

struct PairedMac: Codable {
    var address: String
    var token: String
    var hostName: String
    var hostId: String
    /// Pinned TLS (the full fingerprint) or plain `ws://` (chosen by the user when pairing).
    var transport: Transport
    /// Every address the Mac answers on, as it last said (its Wi-Fi and its tailnet). All of
    /// them are dialled together with `address`, where it answered last.
    var routes: [String] = []
    /// Addresses it answered at before (the Wi-Fi it was paired on, another it was on since),
    /// newest first: dialled too, and they tell its Wi-Fi from another.
    var known: [String] = []

    init(address: String, token: String, hostName: String, hostId: String, transport: Transport, routes: [String] = [],
         known: [String] = []) {
        self.address = address
        self.token = token
        self.hostName = hostName
        self.hostId = hostId
        self.transport = transport
        self.routes = routes
        self.known = known
    }

    private enum CodingKeys: String, CodingKey { case address, token, hostName, hostId, transport, routes, known }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        address = try c.decode(String.self, forKey: .address)
        token = try c.decode(String.self, forKey: .token)
        hostName = try c.decode(String.self, forKey: .hostName)
        hostId = try c.decode(String.self, forKey: .hostId)
        // Records from before TLS were paired over plain ws:// and stay that way (Settings says
        // "Unencrypted") until paired again.
        transport = try c.decodeIfPresent(Transport.self, forKey: .transport) ?? .plain
        routes = try c.decodeIfPresent([String].self, forKey: .routes) ?? []
        known = try c.decodeIfPresent([String].self, forKey: .known) ?? []
    }

    /// The Keychain read once: views ask for it as they draw (the Settings tab, the connection
    /// banner), and a Keychain query on every redraw made them slow to appear.
    private static var cached: PairedMac??

    static func load() -> PairedMac? {
        if let cached { return cached }
        let paired = Keychain.read("paired-mac").flatMap { try? JSONDecoder().decode(PairedMac.self, from: $0) }
        cached = .some(paired)
        return paired
    }

    func save() {
        if let data = try? JSONEncoder().encode(self) { Keychain.write("paired-mac", data) }
        Self.cached = .some(self)
    }

    static func clear() {
        Keychain.delete("paired-mac")
        cached = .some(nil)
    }
}

enum Device {
    /// This phone's id for the Mac, kept in the Keychain so it survives reinstalls.
    static let id: String = {
        if let d = Keychain.read("device-id"), let s = String(data: d, encoding: .utf8) { return s }
        let s = UUID().uuidString
        Keychain.write("device-id", Data(s.utf8))
        return s
    }()

    static var name: String {
        UIDevice.current.name
    }
}

enum Keychain {
    private static func query(_ key: String) -> [String: Any] {
        [kSecClass as String: kSecClassGenericPassword,
         kSecAttrService as String: "dev.trek.TrekMobile",
         kSecAttrAccount as String: key]
    }

    static func read(_ key: String) -> Data? {
        var q = query(key)
        q[kSecReturnData as String] = true
        q[kSecMatchLimit as String] = kSecMatchLimitOne
        var out: AnyObject?
        return SecItemCopyMatching(q as CFDictionary, &out) == errSecSuccess ? out as? Data : nil
    }

    static func write(_ key: String, _ data: Data) {
        delete(key)
        var q = query(key)
        q[kSecValueData as String] = data
        q[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        SecItemAdd(q as CFDictionary, nil)
    }

    static func delete(_ key: String) {
        SecItemDelete(query(key) as CFDictionary)
    }
}
