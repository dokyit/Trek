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
    /// A thread to bring up, from a notification's `trek://open?thread=…&host=…` link.
    var openRequest: String?
    /// A thread to scroll the list to (and unfold whatever hides it), without opening it.
    var revealRequest: String?

    var mode: AppMode = .unpaired
    /// Every machine the phone is paired with, connected at once. Thread and project ids that
    /// reach shared state are theirs prefixed (`"host:thread"`), so two machines' rows never mix.
    var links: [HostLink] = []
    /// The machine host-wide surfaces (Notes, Basecamp, Usage, Settings, Search) act on: the one
    /// picked on that screen, else the first connected, else the first.
    var selectedHostId: String? {
        didSet { UserDefaults.standard.set(selectedHostId, forKey: "selectedHost") }
    }
    /// The client pairing right now, before it becomes a link (no host id until it pairs).
    private var pairingClient: TrekClient?

    /// Every host's threads (ids qualified), in each host's own order, links in pairing order.
    var threads: [ThreadSummary] = [] {
        didSet { threadsChanged() }
    }
    /// Every host's projects, qualified ids. `projects(of:)` is a host's own, for its pickers.
    var projects: [ProjectSummary] = []
    /// Rows the list leaves out, by qualified id: sub-agents' threads, threads from a search.
    var unlisted: [String: ThreadSummary] = [:]
    /// Each opened thread's own store (its row, transcript and paging), keyed by qualified id, so
    /// an update to one thread redraws only what shows it. Not observed: views observe the stores.
    @ObservationIgnored private var stores: [String: ThreadStore] = [:]
    /// The tab bar's counts, kept as values so views that show them don't redraw on every row update.
    private(set) var workingCount = 0
    private(set) var needsYouCount = 0
    var toast: Toast?
    var pairing = false
    var pairingError: String?
    /// A pairing link (deep link or QR code) waiting for the user to confirm it.
    var pendingLink: PairingLink?
    /// The host that asked the phone to open its new-thread sheet (`/new` sent to a thread), and
    /// its project there ("" for none). The view that shows the sheet sets it back to nil.
    var newThreadPrompt: NewThreadPrompt?

    /// What `newThreadPrompt` carries: the link that asked, and its project (qualified, "" for none).
    struct NewThreadPrompt: Equatable {
        var link: HostLink
        var projectId: String
    }

    /// Git status and branches, by thread or project (qualified ids).
    var gitStatus: [GitTarget: GitStatus] = [:]
    var gitBranches: [GitTarget: GitBranches] = [:]
    /// Git work under way (commit, push, switch, merge, remove), by target: views show a spinner.
    var gitBusy: Set<GitTarget> = []
    /// Why a target's git couldn't be read, while none of it has been: the Git sheet says it where
    /// its sections would be, rather than spin.
    var gitUnavailable: [GitTarget: GitUnavailable] = [:]
    /// Each thread's slash commands, as its host's `/` picker lists them (qualified ids).
    var commands: [String: [CommandInfo]] = [:]

    var followUpMode: SendMode {
        didSet { UserDefaults.standard.set(followUpMode.rawValue, forKey: "followUpMode") }
    }

    /// How many items a thread opens with; earlier ones come a page at a time as the user scrolls up.
    static let firstPage = 60
    static let earlierPage = 200
    @ObservationIgnored private var cacheSaves: Set<String> = []

    init() {
        followUpMode = SendMode(rawValue: UserDefaults.standard.string(forKey: "followUpMode") ?? "") ?? .steer
        selectedHostId = UserDefaults.standard.string(forKey: "selectedHost")
    }

    // MARK: Links and qualified ids

    /// The link `qid` is qualified with (`"host:…"`), or nil for a bare or unknown id.
    func link(for qid: String) -> HostLink? { links.first { $0.owns(qid) } }
    /// The link by host id.
    func link(id: String) -> HostLink? { links.first { $0.id == id } }
    /// Where host-scoped requests with no id of their own go.
    var selected: HostLink? {
        if let l = links.first(where: { $0.id == selectedHostId }) { return l }
        return links.first { $0.connection == .connected } ?? links.first
    }
    /// The worst connection across the links, for chrome that asks about "the" connection.
    var connection: ConnectionState {
        var worst: ConnectionState = .idle
        for l in links {
            switch (worst, l.connection) {
            case (_, .identityChanged): return l.connection
            case (.identityChanged, _): return worst
            case (_, .unauthorized): worst = l.connection
            case (.unauthorized, _): break
            case (_, .offline): worst = l.connection
            case (.offline, _): break
            case (_, .connecting): worst = .connecting
            case (.connecting, _): break
            case (_, .connected): worst = .connected
            case (.connected, _): break
            default: break
            }
        }
        return worst
    }
    /// `name` on every link's host, one per line (for toasts and errors that span hosts).
    var hostNames: String { links.map(\.name).joined(separator: ", ") }

    /// The selected host, where a caller still speaks of "the" machine.
    var host: HostInfo? { selected?.host }
    /// The selected host's features (what it doesn't list isn't offered). `serves(_:on:)` is the
    /// per-thread answer.
    var features: Set<String> { selected?.features ?? [] }
    /// The selected host's agents (merged across links wouldn't say whose models a key's are).
    var agents: [AgentOption] { selected?.agents ?? links.first?.agents ?? [] }
    /// Whether the selected host lets threads run with Full access.
    var fullAccessAllowed: Bool { selected?.fullAccessAllowed ?? false }
    /// The selected host's carry, for the Settings indicator.
    var transport: Transport? { selected?.transport }
    /// The thread's host's name, for a row's machine label (qualified ids only answer this).
    func hostName(of qid: String) -> String? { link(for: qid)?.name }

    /// `id` qualified with a host: itself when it already is, else the selected host's (for ids
    /// that arrive bare: launch arguments, notifications from before `host` was in the link).
    func qualified(_ id: String) -> String {
        if link(for: id) != nil { return id }
        return selected?.qualified(id) ?? id
    }

    /// `serves(feature)` on the host `qid` belongs to (bare callers get the selected host).
    func serves(_ feature: MacFeature, on qid: String? = nil) -> Bool {
        (qid.flatMap { link(for: $0) } ?? selected)?.features.contains(feature.rawValue) ?? false
    }

    // MARK: Lifecycle

    func boot(demo forceDemo: Bool) {
        if UserDefaults.standard.bool(forKey: "TrekReset") {
            UserDefaults.standard.set(false, forKey: "demoMode")
            PairedMac.clear()
        }
        if forceDemo {
            attach(HostLink(paired: .demo, backend: MockHost()))
            if Launch.demoPC { attach(HostLink(paired: .demoPC, backend: MockHost(name: "T-Book", id: "demo-pc", platform: "windows"))) }
            mode = .demo
        } else if UserDefaults.standard.bool(forKey: "demoMode") {
            startDemo()
        } else {
            for paired in PairedMac.all() { startLive(paired) }
            if links.isEmpty { mode = .unpaired }
        }
    }

    func startDemo() {
        UserDefaults.standard.set(true, forKey: "demoMode")
        leaveAll()
        attach(HostLink(paired: .demo, backend: MockHost()))
        mode = .demo
    }

    /// Leave demo for the real machines, and the machines for demo: the demo link is its own id
    /// and never saved, but it shares the list with no one.
    func endDemo() {
        UserDefaults.standard.set(false, forKey: "demoMode")
        leaveAll()
        for paired in PairedMac.all() { startLive(paired) }
        if links.isEmpty { mode = .unpaired }
    }

    func startLive(_ paired: PairedMac) {
        UserDefaults.standard.set(false, forKey: "demoMode")
        let client = TrekClient(address: paired.address, routes: paired.routes, known: paired.known,
                                auth: .hello(deviceId: Device.id, token: paired.token),
                                transport: paired.transport, hostId: paired.hostId, hostName: paired.hostName)
        Self.keepAddresses(of: client, hostId: paired.hostId)
        attach(HostLink(paired: paired, backend: client))
        mode = .live
    }

    /// The paired Mac's client looks for it on this network (Bonjour, by its id), and the
    /// addresses it answers at are kept for the next launch.
    private static func keepAddresses(of client: TrekClient, hostId: String) {
        client.hostId = hostId
        // Where it answered (dialled first next time), the addresses it answers on now (Wi-Fi,
        // tailnet), and those it answered at before: all dialled together next time.
        client.onRemember = { address, routes, known in
            guard var kept = PairedMac.all().first(where: { $0.hostId == hostId }) else { return }
            kept.address = address
            kept.routes = routes
            kept.known = known
            PairedMac.save(kept)
        }
    }

    /// A `trek://pair` link arrived (deep link or QR code). Nothing happens until the user
    /// confirms it in the sheet: a link alone must never pair the phone.
    func offer(_ link: PairingLink) {
        pairingError = nil
        pendingLink = link
    }

    /// Pairs with a machine from its QR code or typed details; on success the token and the
    /// certificate fingerprint are kept and the connection joins the others as a link of its
    /// own. A failed pairing touches nothing already connected. With the machine's id (a pairing
    /// link carries it), Bonjour looks for it meanwhile, on the Wi-Fi and on a cable to it: Wi-Fi
    /// that keeps devices apart doesn't stop a phone plugged into the Mac from pairing.
    func pair(address: String, alternates: [String] = [], code: String, transport: Transport, hostId: String? = nil) {
        pendingLink = nil
        pairing = true
        pairingError = nil
        let client = TrekClient(address: address, routes: [address] + alternates,
                                auth: .pair(code: code, deviceId: Device.id, deviceName: Device.name), transport: transport,
                                hostId: hostId)
        client.onPaired = { [weak self, weak client] token, host, kept in
            guard let self, let client else { return }
            self.pairing = false
            self.pairingClient = nil
            // Re-pairing a host this phone already knows swaps its record; the link is made fresh.
            if let old = self.link(id: host.id) { self.detach(old) }
            // Pairing from the demo leaves it: its sample machine must not sit beside the real one
            // (it would be the selected machine, and read as a real Mac once the mode is live).
            if self.mode == .demo, let demo = self.link(id: PairedMac.demo.hostId) { self.detach(demo) }
            // Its id (Bonjour finds it by that on any network), the pinned fingerprint, where it
            // answered and every address it says it answers on (the code's, from a Mac that
            // names none): all there for the first launch after this one.
            let paired = PairedMac(address: client.address, token: token, hostName: host.name, hostId: host.id, transport: kept,
                                   routes: client.routes, known: client.known)
            PairedMac.save(paired)
            Self.keepAddresses(of: client, hostId: host.id)
            UserDefaults.standard.set(false, forKey: "demoMode")
            // The client that paired stays the link's: the welcome comes over it next.
            let link = HostLink(paired: paired, backend: client)
            link.host = host
            self.attach(link)
            self.mode = .live
            self.show("Paired with \(host.name)")
        }
        client.onState = { [weak self, weak client] state in
            guard let self, let client else { return }
            if self.pairing, let why = Self.pairingFailure(state) {
                self.pairing = false
                self.pairingClient = nil
                client.stop()
                if self.links.isEmpty {
                    self.pairingError = why
                    self.mode = .unpaired
                } else {
                    self.show(why, error: true)
                }
            }
        }
        // The pairing screen (with its progress) stays up until the machine says yes. Its messages
        // reach the link's handler once it becomes one; until then only the pairing matters.
        client.onMessage = { _ in }
        pairingClient = client
        client.start()
    }

    /// The app came back to the front.
    func foregrounded() {
        for link in links { link.backend.wake() }
    }

    /// The user asked to try a host again now (its connection banner).
    func retryNow(_ link: HostLink) {
        link.backend.reconnect()
    }

    /// Forget one machine: its link closes, its rows leave the list, its pairing is dropped.
    /// Others keep running.
    func leave(_ link: HostLink) {
        detach(link)
        PairedMac.remove(hostId: link.id)
        if links.isEmpty { mode = .unpaired }
    }

    /// Every link down and forgotten (demo included): the state every one of them kept goes too.
    func leaveAll() {
        for link in links { link.backend.stop() }
        links = []
        pairingClient?.stop()
        pairingClient = nil
        reset()
    }

    /// Old name for the one-pairing flow's "unpair": drop the only (or selected) link.
    func leave() {
        if let link = selected { leave(link) }
    }

    /// Close a link and take back out what it put in shared state (its threads, unlisted rows,
    /// stores) — other hosts' rows stay.
    private func detach(_ link: HostLink) {
        link.backend.stop()
        links.removeAll { $0 === link }
        threads = threads.filter { !link.owns($0.id) }
        projects = projects.filter { !link.owns($0.id) }
        unlisted = unlisted.filter { !link.owns($0.key) }
        for s in stores.values where link.owns(s.id) { saveCache(s); s.subscribed = false }
        stores = stores.filter { !link.owns($0.key) }
        commands = commands.filter { !link.owns($0.key) }
        gitStatus = gitStatus.filter { !$0.key.owned(by: link) }
        gitBranches = gitBranches.filter { !$0.key.owned(by: link) }
        gitBusy = gitBusy.filter { !$0.owned(by: link) }
        gitUnavailable = gitUnavailable.filter { !$0.key.owned(by: link) }
        link.failPending()
    }

    private func attach(_ link: HostLink) {
        if let at = links.firstIndex(where: { $0.id == link.id }) {
            links[at].backend.stop()
            links[at] = link
        } else {
            links.append(link)
        }
        let b = link.backend
        b.onMessage = { [weak self, weak link] in guard let link else { return }; self?.handle($0, from: link) }
        (b as? TrekClient)?.onDetail = { [weak link] in link?.connectionDetail = $0 }
        b.onState = { [weak self, weak link] state in
            guard let self, let link else { return }
            let was = link.connection
            link.connection = state
            if was == .connected, state != .connected { link.failPending() }
            if state == .connected {
                // Resubscribe after a reconnect, from where we were.
                for s in self.stores.values where s.subscribed && link.owns(s.id) { self.resubscribe(s, on: link) }
                // What this host lets this phone do (allowing for a whole session, Full access).
                self.loadSettings(on: link)
                // What screens asked for while this host couldn't answer, asked again.
                self.reloadAsked(link)
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
        for link in links { link.failPending() }
        threads = []
        projects = []
        stores = [:]
        unlisted = [:]
        gitStatus = [:]
        gitBranches = [:]
        gitBusy = []
        gitUnavailable = [:]
        commands = [:]
        newThreadPrompt = nil
    }

    // MARK: Incoming

    /// Every id `link`'s messages carry is bare to it: threads, projects, the threads a summary
    /// links to (sub-agents, parents, forks) and rows that open one. All become `link.qualified`
    /// the moment they enter shared state, and `bare` again on the way out (`stripped`).
    func qualify(_ t: ThreadSummary, on link: HostLink) -> ThreadSummary {
        var t = t
        t.id = link.qualified(t.id)
        if let pid = t.project?.id { t.project?.id = link.qualified(pid) }
        t.parentId = t.parentId.map { link.qualified($0) }
        t.startedBy = t.startedBy.map { link.qualified($0) }
        t.continuedFrom = t.continuedFrom.map { link.qualified($0) }
        t.needsThread = t.needsThread.map { link.qualified($0) }
        t.subAgents = t.subAgents?.map { var s = $0; s.threadId = s.threadId.map { link.qualified($0) }; return s }
        return t
    }

    func qualify(_ p: ProjectSummary, on link: HostLink) -> ProjectSummary {
        var p = p
        p.id = link.qualified(p.id)
        return p
    }

    func qualify(_ i: TItem, on link: HostLink) -> TItem {
        var i = i
        if case .threadLink(var l) = i.body {
            l.threadId = link.qualified(l.threadId)
            i.body = .threadLink(l)
        }
        return i
    }

    func handle(_ message: ServerMessage, from link: HostLink) {
        switch message {
        case .paired(_, _, let host, let features, _), .welcome(_, let host, let features, _):
            link.host = host
            link.features = Set(features)
            learnPlatform(of: link)
        case .snapshot(let s):
            let rows = s.threads.map { qualify($0, on: link) }
            let projs = s.projects.map { qualify($0, on: link) }
            withAnimation(.snappy) {
                threads = threads.filter { !link.owns($0.id) } + rows
                projects = projects.filter { !link.owns($0.id) } + projs
            }
            link.projects = s.projects
            link.agents = s.agents
            link.fullAccessAllowed = s.fullAccess ?? false
            // Rows the host no longer lists (a sub-agent deleted while this phone was away) go,
            // unless a thread is open on one. Other hosts' unlisted rows are theirs alone.
            let kept = Set(s.unlisted.map { link.qualified($0.id) })
            unlisted = unlisted.filter { !link.owns($0.key) || kept.contains($0.key) || stores[$0.key] != nil }
            for t in s.unlisted { setUnlisted(qualify(t, on: link)) }
            learnIcons()
        case .thread(let incoming):
            let t = qualify(incoming, on: link)
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
            let qid = link.qualified(id)
            threads.removeAll { $0.id == qid }
            unlisted[qid] = nil
            TranscriptCache.remove(host: link.id, thread: id)
        case .transcript(let re, let tid, let reset, let seq, let items, let more):
            let qid = link.qualified(tid)
            if let s = stores[qid] {
                let rewritten = rewrites.remove(qid) != nil
                if reset, s.loaded {
                    protocolLog.info("transcript \(qid, privacy: .public): reset\(rewritten ? " (rewritten)" : "", privacy: .public), \(s.items.count) items → \(items.count) (re \(re ?? "-", privacy: .public))")
                }
                s.applyReply(reset: reset, seq: seq, items: items.map { qualify($0, on: link) }, more: more, rewritten: rewritten)
                if let start = openStarts.removeValue(forKey: qid) {
                    Perf.report("open \(qid): reply after \(Perf.ms(Perf.now - start)), \(items.count) items\(reset ? "" : " since the cache")")
                    Perf.untilFrame("open \(qid): on screen", from: start)
                }
            }
            link.reply(re, message)
        case .transcriptPage(let re, let tid, let items, let more):
            // Earlier pages carry thread_link rows too: qualify them like the first page's.
            link.reply(re, .transcriptPage(re: re, threadId: tid, items: items.map { qualify($0, on: link) }, more: more))
        case .item(let tid, let item):
            guard let s = stores[link.qualified(tid)], s.subscribed else { return }
            s.enqueue(qualify(item, on: link))
        case .transcriptReset(let tid):
            // Rewound or rewritten on the host: the whole (last page of the) transcript again.
            let qid = link.qualified(tid)
            protocolLog.info("transcript \(qid, privacy: .public): the host reset it")
            rewrites.insert(qid)
            if let s = stores[qid], s.subscribed { request(.subscribe(threadId: qid, afterSeq: nil, limit: Self.firstPage)) }
        case .ack(let re, _, _, _), .pong(let re), .machineInfo(let re, _):
            link.reply(re, message)
        case .usage(let re, _), .basecamp(let re, _), .notes(let re, _), .note(let re, _), .gitStatus(let re, _),
             .gitDiff(let re, _), .gitBranches(let re, _), .commands(let re, _, _), .settings(let re, _),
             .searchResults(let re, _), .attachment(let re, _), .backgroundOutput(let re, _), .icon(let re, _),
             .noteAsset(let re, _), .noteImage(let re, _), .noteFileChunk(let re, _):
            link.reply(re, message)
        case .unlisted(let t):
            setUnlisted(qualify(t, on: link))
        case .error(let re, let code, let text):
            if let re, link.hasReply(re) { link.reply(re, message) } else if code != .unauthorized { show(text, error: true) }
        case .unknown:
            break
        }
    }

    /// Ask a machine what it runs on, once it's connected and answers `machine_info`; kept with
    /// its pairing.
    private func learnPlatform(of link: HostLink) {
        guard link.features.contains(MacFeature.machines.rawValue), link.id != "demo" else { return }
        link.request(.machineInfo) { [weak link] reply in
            guard let link, case .machineInfo(_, let info) = reply, link.paired.platform != info.platform else { return }
            link.paired.platform = info.platform
            PairedMac.save(link.paired)
        }
    }

    /// What the machine of `qid` (a thread or project id) is.
    func machineKind(of qid: String) -> MachineKind {
        MachineKind(platform: link(for: qid)?.paired.platform)
    }

    /// Keeps each opened thread's store in step with its row, and the tab bar's counts.
    ///
    /// Never animated, though the list's change may be (a row moving between sections): an open
    /// thread reads its row (whether it works, its sub-agents) and would animate its whole
    /// transcript with it, its rows sliding and fading over each other while a busy thread's
    /// state flips.
    private func threadsChanged() {
        if !stores.isEmpty {
            let byID = Dictionary(threads.map { ($0.id, $0) }, uniquingKeysWith: { a, _ in a })
            var still = Transaction()
            still.disablesAnimations = true
            withTransaction(still) {
                for (id, s) in stores { s.setSummary(byID[id] ?? unlisted[id]) }
            }
        }
        let working = threads.count(where: \.atWork)
        let needs = threads.count { $0.waitsOnYou && $0.section != .snoozed }
        if working != workingCount { workingCount = working }
        if needs != needsYouCount { needsYouCount = needs }
        learnIcons()
    }

    // MARK: Outgoing

    /// The host a message goes to: the one its thread or project id is qualified with, or the
    /// selected host for asks that name nothing (usage, notes, search).
    func route(_ m: ClientMessage) -> HostLink? {
        func t(_ qid: String) -> HostLink? { link(for: qid) }
        switch m {
        case .subscribe(let tid, _, _), .transcriptBefore(let tid, _, _), .unsubscribe(let tid),
             .send(let tid, _, _, _), .threadAction(let tid, _), .answer(let tid, _, _),
             .interrupt(let tid), .markSeen(let tid), .worktreeMerge(let tid),
             .commands(let tid), .turnAction(let tid, _, _, _, _), .queueAction(let tid, _, _, _, _),
             .attachment(let tid, _, _, _), .backgroundOutput(let tid, _), .stopBackground(let tid, _),
             .cardAction(let tid, _, _):
            return t(tid)
        case .setPrefs(let tid, _, _, _, _, _):
            return t(tid)
        case .worktreeRemove(let tid, _, _):
            return t(tid)
        case .newThread(let pid, _, _, _, _, _, _, _, _):
            return t(pid)
        case .gitStatus(let g), .gitDiff(let g, _), .gitCommit(let g, _), .gitPush(let g),
             .gitBranches(let g), .gitSwitch(let g, _):
            switch g {
            case .thread(let qid), .project(let qid): return t(qid)
            }
        case .noteAsset(let nid, _, _), .addNoteImage(let nid, _, _):
            return t(nid)
        case .icon(_, let pid):
            return pid.flatMap(t) ?? selected
        default:
            return selected
        }
    }

    /// `m` as its destination host expects it: ids it carries back to bare (`link.bare`).
    private func stripped(_ m: ClientMessage, on link: HostLink) -> ClientMessage {
        let b = link.bare
        switch m {
        case .subscribe(let tid, let after, let limit): return .subscribe(threadId: b(tid), afterSeq: after, limit: limit)
        case .transcriptBefore(let tid, let before, let limit): return .transcriptBefore(threadId: b(tid), before: before, limit: limit)
        case .unsubscribe(let tid): return .unsubscribe(threadId: b(tid))
        case .send(let tid, let text, let mode, let images): return .send(threadId: b(tid), text: text, mode: mode, images: images)
        case .newThread(let pid, let agent, let model, let text, let worktree, let effort, let access, let plan, let images):
            return .newThread(projectId: b(pid), agent: agent, model: model, text: text, worktree: worktree,
                              effort: effort, access: access, plan: plan, images: images)
        case .setPrefs(let tid, let a, let m2, let e, let ac, let p2):
            return .setPrefs(threadId: b(tid), agent: a, model: m2, effort: e, access: ac, plan: p2)
        case .threadAction(let tid, let action): return .threadAction(threadId: b(tid), action: action)
        case .answer(let tid, let rid, let r): return .answer(threadId: b(tid), requestId: rid, response: r)
        case .interrupt(let tid): return .interrupt(threadId: b(tid))
        case .markSeen(let tid): return .markSeen(threadId: b(tid))
        case .gitStatus(let g): return .gitStatus(bareTarget(g, on: link))
        case .gitDiff(let g, let path): return .gitDiff(bareTarget(g, on: link), path: path)
        case .gitCommit(let g, let msg): return .gitCommit(bareTarget(g, on: link), message: msg)
        case .gitPush(let g): return .gitPush(bareTarget(g, on: link))
        case .gitBranches(let g): return .gitBranches(bareTarget(g, on: link))
        case .gitSwitch(let g, let br): return .gitSwitch(bareTarget(g, on: link), branch: br)
        case .worktreeMerge(let tid): return .worktreeMerge(threadId: b(tid))
        case .worktreeRemove(let tid, let db, let f): return .worktreeRemove(threadId: b(tid), deleteBranch: db, force: f)
        case .commands(let tid): return .commands(threadId: b(tid))
        case .queueAction(let tid, let ix, let reset, let text, let edit):
            return .queueAction(threadId: b(tid), index: ix, reset: reset, text: text, edit: edit)
        case .attachment(let tid, let iid, let ix, let px): return .attachment(threadId: b(tid), itemId: iid, index: ix, maxPx: px)
        case .backgroundOutput(let tid, let task): return .backgroundOutput(threadId: b(tid), taskId: task)
        case .stopBackground(let tid, let task): return .stopBackground(threadId: b(tid), taskId: task)
        case .cardAction(let tid, let key, let action): return .cardAction(threadId: b(tid), key: key, action: action)
        case .turnAction(let tid, let iid, let a, let mo, let rf):
            return .turnAction(threadId: b(tid), itemId: iid, action: a, model: mo, restoreFiles: rf)
        case .noteAsset(let nid, let path, let px): return .noteAsset(noteId: b(nid), path: path, maxPx: px)
        case .addNoteImage(let nid, let data, let name): return .addNoteImage(noteId: b(nid), data: data, name: name)
        case .icon(let a, let pid): return .icon(agent: a, projectId: pid.map(b))
        default: return m
        }
    }

    private func bareTarget(_ g: GitTarget, on link: HostLink) -> GitTarget {
        switch g {
        case .thread(let qid): return .thread(link.bare(qid))
        case .project(let qid): return .project(link.bare(qid))
        }
    }

    @discardableResult
    func request(_ message: ClientMessage, on explicit: HostLink? = nil, then: ((ServerMessage) -> Void)? = nil) -> String {
        guard let link = explicit ?? route(message) else {
            // No machine to ask: whoever waits for the answer hears so, rather than never.
            then?(.error(re: nil, code: .unknown, message: Self.notConnected))
            return ""
        }
        return link.request(stripped(message, on: link), then: then)
    }

    static let notConnected = "Not connected to that machine."

    /// Failures of an action become a toast; `ok` runs on success.
    func act(_ message: ClientMessage, on link: HostLink? = nil, ok: ((ServerMessage) -> Void)? = nil) {
        request(message, on: link) { [weak self] reply in
            if case .error(_, _, let text) = reply { self?.show(text, error: true) } else { ok?(reply) }
        }
    }

    @ObservationIgnored private var openStarts: [String: Double] = [:]
    /// Threads whose host said it rewrote them (`transcript_reset`): the reply to the subscribe
    /// that follows replaces what's here, where a plain reset keeps the earlier items.
    @ObservationIgnored private var rewrites: Set<String> = []

    /// The thread's store, made on first use (`tid` qualified, as the list carries it).
    func store(_ tid: String) -> ThreadStore {
        if let s = stores[tid] { return s }
        let s = ThreadStore(id: tid, host: link(for: tid)?.id ?? "demo", summary: thread(tid))
        s.onChange = { [weak self, weak s] in if let s { self?.scheduleCacheSave(s) } }
        stores[tid] = s
        return s
    }

    /// The thread's store, if it has one.
    func existingStore(_ tid: String) -> ThreadStore? { stores[tid] }

    /// Opens a thread's transcript: what the cache has shows at once, and the host sends what's
    /// newer (or the last page, when it's the first time).
    func subscribe(_ tid: String) {
        let s = store(tid)
        guard !s.subscribed else { return }
        if Perf.enabled { openStarts[tid] = Perf.navigated ?? Perf.now; Perf.navigated = nil }
        if !s.loaded,
           let cached = Perf.measure("cache load", tid, { TranscriptCache.load(host: s.hostId, thread: s.bareId) }) {
            s.restore(items: cached.items, seq: cached.seq, more: cached.more)
            if let start = openStarts[tid] { Perf.untilFrame("open \(tid): cache on screen (\(cached.items.count) items)", from: start) }
        }
        s.subscribed = true
        resubscribe(s)
        markSeen(tid)
    }

    private func resubscribe(_ s: ThreadStore, on link: HostLink? = nil) {
        protocolLog.info("transcript \(s.id, privacy: .public): subscribe after \(s.loaded ? String(s.seq) : "-", privacy: .public)")
        request(.subscribe(threadId: s.id, afterSeq: s.loaded ? s.seq : nil, limit: Self.firstPage), on: link)
    }

    func unsubscribe(_ tid: String) {
        guard let s = stores[tid], s.subscribed else { return }
        s.subscribed = false
        s.flushQueued()
        saveCache(s)
        s.trim(to: TranscriptCache.keepItems)
        link(for: tid)?.backend.send(.unsubscribe(threadId: s.bareId), id: nil)
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

    private func scheduleCacheSave(_ s: ThreadStore) {
        guard !cacheSaves.contains(s.id) else { return }
        cacheSaves.insert(s.id)
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) { [weak self, weak s] in
            guard let self, let s else { return }
            self.cacheSaves.remove(s.id)
            self.saveCache(s)
        }
    }

    /// Transcript caches sit under their host, so a thread's items are this host's forever
    /// whatever else is paired: same thread id on two machines stays two transcripts.
    private func saveCache(_ s: ThreadStore) {
        guard s.loaded else { return }
        TranscriptCache.save(host: s.hostId, thread: s.bareId, items: s.items, seq: s.seq, more: s.more)
    }

    // MARK: Turn actions

    /// Text for a thread's composer when it opens (a fork from a message).
    @ObservationIgnored private var drafts: [String: String] = [:]

    func takeDraft(_ tid: String) -> String? { drafts.removeValue(forKey: tid) }

    /// Undo, retry or fork a turn (from its `turn_end` item), or rewind or fork from a message
    /// (its `user` item). A fork opens the new thread; a rewind puts the message in `compose`.
    func turnAction(_ tid: String, item: String, _ action: TurnAction, model: String? = nil, restoreFiles: Bool = true,
                    compose: ((String) -> Void)? = nil) {
        let link = route(.turnAction(threadId: tid, itemId: item, action: action, model: model, restoreFiles: restoreFiles))
        act(.turnAction(threadId: tid, itemId: item, action: action, model: model, restoreFiles: restoreFiles)) { [weak self, weak link] reply in
            guard case .ack(_, let newThread, _, let text) = reply, let link else { return }
            switch action {
            case .fork:
                guard let self, let newThread else { self?.show("Forked"); return }
                // Forked from a message: it waits in the new thread's composer.
                let qid = link.qualified(newThread)
                if let text { self.drafts[qid] = text }
                self.openRequest = qid
            case .rewind, .undo:
                if let text { compose?(text) }
            case .retry:
                break
            }
        }
    }

    func markSeen(_ tid: String) {
        if let i = threads.firstIndex(where: { $0.id == tid }), threads[i].unseen { threads[i].unseen = false }
        if let link = link(for: tid) { link.backend.send(.markSeen(threadId: link.bare(tid)), id: nil) }
    }

    func send(_ text: String, to tid: String, mode: SendMode?, images: [ImageUpload] = []) {
        let link = route(.send(threadId: tid, text: text, mode: mode, images: images))
        act(.send(threadId: tid, text: text, mode: mode, images: images)) { [weak self, weak link] reply in
            // `/new`: the host leaves its own screen be and asks the phone to open its sheet.
            guard let self, let link else { return }
            if case .ack(_, _, let open?, _) = reply, open.screen == "new_thread" {
                self.newThreadPrompt = NewThreadPrompt(link: link, projectId: open.projectId.map { link.qualified($0) } ?? "")
            }
        }
    }

    /// Change a thread's agent, model, effort, access or plan mode. The row changes here at once;
    /// the host's own row follows (and puts it back if it refused).
    func setPrefs(_ tid: String, agent: String? = nil, model: String? = nil, effort: String? = nil, access: Access? = nil, plan: Bool? = nil) {
        let before = thread(tid)
        let agents = link(for: tid)?.agents ?? []
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
            // Refused: say why, and put the row back as the host has it.
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

    /// The models `agentKey` offers on `tid`'s host, and the efforts `modelId` takes.
    func models(of agentKey: String, on qid: String? = nil) -> [ModelOption] {
        (qid.flatMap { link(for: $0) }?.agents ?? selected?.agents ?? []).first { $0.key == agentKey }?.models ?? []
    }
    func efforts(agent agentKey: String, model modelId: String?, on qid: String? = nil) -> [String] {
        let agents = qid.flatMap { link(for: $0) }?.agents ?? selected?.agents ?? []
        let models = agents.first { $0.key == agentKey }?.models ?? []
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
        let link = route(.newThread(projectId: project, agent: agent, model: model, text: text, worktree: worktree, effort: effort, access: access, plan: plan, images: images))
        act(.newThread(projectId: project, agent: agent, model: model, text: text, worktree: worktree,
                       effort: effort, access: access, plan: plan, images: images)) { [weak link] reply in
            if case .ack(_, let tid?, _, _) = reply, let link { opened(link.qualified(tid)) }
        }
    }

    /// Pull to refresh. Each host pushes every change, but one missed (a dropped event, a link
    /// that died quietly) stays missed: a link that answers is asked again for what the screens
    /// show, one that doesn't is replaced at once (a new connection brings the list afresh).
    func refresh() async {
        for link in links {
            let alive = await withCheckedContinuation { (c: CheckedContinuation<Bool, Never>) in
                var done = false
                link.request(.ping) { reply in
                    guard !done else { return }
                    done = true
                    if case .pong = reply { c.resume(returning: true) } else { c.resume(returning: false) }
                }
                DispatchQueue.main.asyncAfter(deadline: .now() + 3) { if !done { done = true; c.resume(returning: false) } }
            }
            guard alive else {
                link.backend.reconnect()
                continue
            }
            // From where each open thread is: the host sends what came after.
            for s in stores.values where s.subscribed && link.owns(s.id) { resubscribe(s, on: link) }
            loadUsage(quiet: true, on: link)
            loadSettings(on: link)
        }
    }

    /// What the Notes and Basecamp screens asked of `link`: on a reconnect, asked again where the
    /// answer never came (or, for the notes list, may have changed meanwhile).
    private func reloadAsked(_ link: HostLink) {
        for id in link.notesToSave.keys { saveLeftNote(id, on: link) }
        if link.notesAsked { loadNotes(on: link) }
        for range in link.basecampAsked where link.basecamp[range] == nil && !link.basecampLoading.contains(range) { loadBasecamp(range, on: link) }
    }

    // MARK: Usage and Basecamp

    /// Ask for plan usage (`on`'s asks its agents, at most every 30 seconds). Quiet: in the
    /// background, for the Threads toolbar, so a failure isn't worth a toast.
    func loadUsage(quiet: Bool = false, on link: HostLink? = nil) {
        guard let link = link ?? selected else { return }
        link.usageLoading = true
        link.request(.usage) { [weak self, weak link] reply in
            guard let self, let link else { return }
            link.usageLoading = false
            switch reply {
            case .usage(_, let u):
                link.usage = u
                // Some agent was still answering: ask again shortly for the rest.
                if u.loading == true { DispatchQueue.main.asyncAfter(deadline: .now() + 3) { [weak self, weak link] in self?.loadUsage(quiet: quiet, on: link) } }
            case .error(_, _, let text): if !quiet { self.show(text, error: true) }
            default: break
            }
        }
    }

    func loadBasecamp(_ range: BasecampRange, on link: HostLink? = nil) {
        guard let link = link ?? selected else { return }
        link.basecampAsked.insert(range)
        link.basecampLoading.insert(range)
        link.request(.basecamp(range)) { [weak self, weak link] reply in
            guard let self, let link else { return }
            link.basecampLoading.remove(range)
            switch reply {
            case .basecamp(_, var b):
                // Its review rows open threads and mark them seen: their ids join shared state.
                for i in b.review.indices { b.review[i].threadId = link.qualified(b.review[i].threadId) }
                withAnimation(.snappy) { link.basecamp[range] = b }
            case .error(_, _, let text): self.show(text, error: true)
            default: break
            }
        }
    }

    // MARK: Notes

    func loadNotes(on link: HostLink? = nil) {
        guard let link = link ?? selected else { return }
        link.notesAsked = true
        link.request(.notes) { [weak link] reply in
            if case .notes(_, let list) = reply { withAnimation(.snappy) { link?.notes = list } }
        }
    }

    /// Read a note whole; `done` gets it.
    func openNote(_ id: String, on link: HostLink? = nil, done: ((Note) -> Void)? = nil) {
        guard let link = link ?? selected else { return }
        link.request(.note(id: id)) { [weak self, weak link] reply in
            guard let link else { return }
            switch reply {
            case .note(_, let n):
                link.openNotes[id] = n
                done?(n)
            case .error(_, _, let text): self?.show(text, error: true)
            default: break
            }
        }
    }

    func createNote(_ body: String = "", on link: HostLink? = nil, done: ((Note) -> Void)? = nil) {
        guard let link = link ?? selected else { return }
        link.request(.createNote(body: body)) { [weak self, weak link] reply in
            guard let link else { return }
            switch reply {
            case .note(_, let n):
                link.openNotes[n.id] = n
                self?.loadNotes(on: link)
                done?(n)
            case .error(_, _, let text): self?.show(text, error: true)
            default: break
            }
        }
    }

    /// Save a note's text. If it changed on the host since it was opened, the host refuses
    /// and `conflict` runs (or, without one, the host's message shows as a toast). Any other
    /// failure shows as a toast and `failed` runs.
    func saveNote(_ id: String, body: String, on link: HostLink? = nil, done: ((Note) -> Void)? = nil,
                  conflict: (() -> Void)? = nil, failed: (() -> Void)? = nil) {
        guard let link = link ?? selected else { return }
        let modified = link.openNotes[id]?.modified
        link.notesSaving.insert(id)
        link.request(.saveNote(id: id, body: body, modified: modified)) { [weak self, weak link] reply in
            guard let link else { return }
            link.notesSaving.remove(id)
            switch reply {
            case .note(_, let n):
                link.openNotes[id] = n
                if let i = link.notes.firstIndex(where: { $0.id == id }) {
                    link.notes[i].title = n.title
                    link.notes[i].modified = n.modified
                }
                done?(n)
                self?.saveLeftNote(id, on: link)
            case .error(_, let code, let text):
                if code == .conflict, let conflict { conflict() } else {
                    self?.show(text, error: true)
                    failed?()
                }
                // What its closed editor left would be refused the same way.
                if code == .conflict, link.notesToSave.removeValue(forKey: id) != nil {
                    self?.show("That note changed on \(link.name), so your last changes to it weren't saved.", error: true)
                }
            default: break
            }
        }
    }

    /// A save of this note is out, waiting for the host's answer.
    func savingNote(_ id: String, on link: HostLink? = nil) -> Bool { (link ?? selected)?.notesSaving.contains(id) ?? false }

    /// Save what a note's editor had when it closed: now, after the save out for it, or when the
    /// connection comes back.
    func saveNoteLeft(_ id: String, body: String, on link: HostLink? = nil) {
        guard let link = link ?? selected else { return }
        link.notesToSave[id] = body
        saveLeftNote(id, on: link)
    }

    private func saveLeftNote(_ id: String, on link: HostLink) {
        guard !link.notesSaving.contains(id), let body = link.notesToSave.removeValue(forKey: id), link.openNotes[id]?.body != body else { return }
        saveNote(id, body: body, on: link, failed: { [weak link] in
            if link?.notesToSave[id] == nil { link?.notesToSave[id] = body }
        })
    }

    /// Delete a note: the host keeps it in the notes folder's Deleted.
    func deleteNote(_ id: String, on link: HostLink? = nil) {
        guard let link = link ?? selected else { return }
        let before = link.notes
        withAnimation(.snappy) { link.notes.removeAll { $0.id == id } }
        link.openNotes[id] = nil
        link.notesToSave[id] = nil
        link.request(.deleteNote(id: id)) { [weak self, weak link] reply in
            if case .error(_, _, let text) = reply {
                self?.show(text, error: true)
                link?.notes = before
            }
        }
    }

    // MARK: Git

    func loadGitStatus(_ target: GitTarget) {
        request(.gitStatus(target)) { [weak self] reply in
            switch reply {
            case .gitStatus(_, let s):
                self?.gitUnavailable[target] = nil
                withAnimation(.snappy) { self?.gitStatus[target] = s }
            case .error(_, let code, let text): self?.gitFailed(target, code, text)
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
            case .error(_, let code, let text): self?.gitFailed(target, code, text)
            default: break
            }
        }
    }

    /// Reading `target`'s git failed. Before anything of it came in, the sheet says why in place
    /// of its spinners (a toast would land behind it); once it shows something, a toast says so.
    private func gitFailed(_ target: GitTarget, _ code: ErrorCode, _ text: String) {
        if gitStatus[target] == nil {
            gitUnavailable[target] = GitUnavailable(text: text, refused: code == .badRequest)
        } else {
            show(text, error: true)
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

    func loadSettings(on link: HostLink? = nil) {
        guard let link = link ?? selected else { return }
        link.request(.settings) { [weak link] reply in
            if case .settings(_, let s) = reply { link?.macSettings = s }
        }
    }

    /// Change a host's settings. Shown at once; its answer replaces it (or puts it back,
    /// saying why, when it refuses).
    func changeSettings(_ change: SettingsChange, on link: HostLink? = nil, done: (() -> Void)? = nil) {
        guard let link = link ?? selected else { return }
        let before = link.macSettings
        if let s = link.macSettings { link.macSettings = change.applied(to: s) }
        link.request(.setSettings(change)) { [weak self, weak link] reply in
            switch reply {
            case .settings(_, let s):
                link?.macSettings = s
                done?()
            case .error(_, _, let text):
                self?.show(text, error: true)
                link?.macSettings = before
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
        threads.filter { ($0.section == .working || ($0.section == .inbox && $0.atWork)) && !$0.waitsOnYou }
            .sorted { ($0.workingSince ?? $0.updatedAt) > ($1.workingSince ?? $1.updatedAt) }
    }

    var recent: [ThreadSummary] {
        threads.filter { ($0.section == .inbox && !$0.waitsOnYou && !$0.atWork) || $0.section == .settled }
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
    /// What it said it runs on (`machine_info`: "macos", "windows"), kept so its threads are
    /// marked before it's reached.
    var platform: String?

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

    private enum CodingKeys: String, CodingKey { case address, token, hostName, hostId, transport, routes, known, platform }

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
        platform = try c.decodeIfPresent(String.self, forKey: .platform)
    }

    /// The demo's stand-in pairing (never saved): the demo host answers to "demo".
    static var demo: PairedMac {
        var p = PairedMac(address: "", token: "", hostName: "Demo", hostId: "demo", transport: .plain)
        p.platform = "macos"
        return p
    }
    /// The demo's Windows machine (`-TrekDemoPC`).
    static var demoPC: PairedMac {
        var p = PairedMac(address: "", token: "", hostName: "T-Book", hostId: "demo-pc", transport: .plain)
        p.platform = "windows"
        return p
    }

    /// The Keychain read once: views ask for it as they draw (the Settings tab, the connection
    /// banner), and a Keychain query on every redraw made them slow to appear.
    private static var cached: [PairedMac]?

    /// Every machine this phone is paired with, oldest pairing first.
    static func all() -> [PairedMac] {
        if let cached { return cached }
        var out: [PairedMac] = []
        if let data = Keychain.read("paired-mac") {
            // The record was one pairing until the phone could hold several: a bare object
            // reading as one entry keeps it.
            if let list = try? JSONDecoder().decode([PairedMac].self, from: data) { out = list }
            else if let one = try? JSONDecoder().decode(PairedMac.self, from: data) { out = [one] }
        }
        cached = out
        return out
    }

    /// The first pairing, where a caller still speaks of "the" machine.
    static func load() -> PairedMac? { all().first }

    /// Insert or replace the pairing for `paired.hostId`.
    static func save(_ paired: PairedMac) {
        var all = all()
        if let i = all.firstIndex(where: { $0.hostId == paired.hostId }) { all[i] = paired } else { all.append(paired) }
        persist(all)
    }

    static func remove(hostId: String) {
        persist(all().filter { $0.hostId != hostId })
    }

    static func clear() {
        persist([])
        Keychain.delete("paired-mac")
    }

    private static func persist(_ all: [PairedMac]) {
        cached = all
        if let data = try? JSONEncoder().encode(all) { Keychain.write("paired-mac", data) }
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
