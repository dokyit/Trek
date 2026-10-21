import CryptoKit
import Foundation
import Network
import Security

/// How the phone talks to a Mac: TLS pinned to the certificate fingerprint it paired with (the
/// default), or plain `ws://` for Macs that don't serve TLS, only after the user agreed to it.
nonisolated enum Transport: Equatable, Codable {
    /// `wss://`, accepting only a certificate whose SHA-256 (of its DER) matches the pin.
    case tls(CertificatePin)
    /// `ws://`, unencrypted. Only ever chosen by the user through the "Unencrypted connection" warning.
    case plain

    var isEncrypted: Bool { if case .tls = self { true } else { false } }
}

nonisolated enum CertificatePin: Equatable, Codable {
    /// The full fingerprint, 64 lowercase hex characters (from the QR code, or seen while pairing).
    case full(String)
    /// The first 16 hex characters the Mac shows beside the pairing code, typed by the user. Only
    /// used to pair; the full fingerprint seen then is what's kept. (64 bits: no one makes a
    /// certificate to match them while a code lasts.)
    case prefix(String)

    func matches(_ fingerprint: String) -> Bool {
        switch self {
        case .full(let fp): fp == fingerprint
        case .prefix(let p): p.count == Fingerprint.typedLength && fingerprint.hasPrefix(p)
        }
    }

    /// `ABCD-1234-EF56-7890`, as the Mac shows it.
    var short: String {
        switch self {
        case .full(let fp), .prefix(let fp): Fingerprint.short(fp)
        }
    }
}

nonisolated enum Fingerprint {
    /// SHA-256 of a certificate's DER encoding, as 64 lowercase hex characters.
    static func of(_ der: Data) -> String {
        SHA256.hash(data: der).map { String(format: "%02x", $0) }.joined()
    }

    /// How many hex characters the Mac shows to type.
    static let typedLength = 16

    /// `ABCD-1234-EF56-7890`: the first 16 hex characters, uppercased and grouped in fours.
    static func short(_ fp: String) -> String {
        let head = Array(String(fp.prefix(typedLength)).uppercased())
        return stride(from: 0, to: head.count, by: 4).map { String(head[$0..<min($0 + 4, head.count)]) }.joined(separator: "-")
    }

    /// A full fingerprint from a link, or nil if it isn't 64 hex characters.
    static func full(_ s: String) -> String? {
        let fp = s.lowercased()
        return fp.count == 64 && fp.allSatisfy(\.isHexDigit) ? fp : nil
    }

    /// The short form as typed ("abcd 1234 ef56 7890", "ABCD-1234-…") → 16 lowercase hex
    /// characters, or nil.
    static func typedPrefix(_ s: String) -> String? {
        let hex = s.lowercased().filter { !$0.isWhitespace && $0 != "-" }
        return hex.count == typedLength && hex.allSatisfy(\.isHexDigit) ? hex : nil
    }
}

/// Checks the Mac's certificate against the pin during the TLS handshake, and says when the
/// WebSocket opened. There's no CA here: the certificate is self-signed, so its fingerprint is
/// the whole of the trust decision. It runs before any byte of ours goes out, so a wrong server
/// never sees the pairing code or the token. Without a pin (plain `ws://`) it only says "open".
nonisolated final class PinningDelegate: NSObject, URLSessionWebSocketDelegate, @unchecked Sendable {
    private let lock = NSLock()
    private let pin: CertificatePin?
    private let opened: @Sendable () -> Void
    private var _seen: String?
    private var _rejected: String?

    init(pin: CertificatePin?, opened: @escaping @Sendable () -> Void = {}) {
        self.pin = pin
        self.opened = opened
    }

    /// The fingerprint of the last certificate accepted.
    var seen: String? { lock.withLock { _seen } }
    /// The fingerprint of a certificate refused for not matching the pin (nil if none was).
    var rejected: String? { lock.withLock { _rejected } }

    func urlSession(_ session: URLSession, didReceive challenge: URLAuthenticationChallenge,
                    completionHandler: @escaping (URLSession.AuthChallengeDisposition, URLCredential?) -> Void) {
        guard pin != nil, challenge.protectionSpace.authenticationMethod == NSURLAuthenticationMethodServerTrust,
              let trust = challenge.protectionSpace.serverTrust else {
            completionHandler(.performDefaultHandling, nil)
            return
        }
        if check(trust) {
            completionHandler(.useCredential, URLCredential(trust: trust))
        } else {
            completionHandler(.cancelAuthenticationChallenge, nil)
        }
    }

    /// The pin's verdict on the certificate a server offered, noted (accepted or refused). Also
    /// what `NetworkLink`'s TLS asks.
    func check(_ trust: SecTrust) -> Bool {
        guard let pin else { return true }
        guard let leaf = (SecTrustCopyCertificateChain(trust) as? [SecCertificate])?.first else {
            lock.withLock { _rejected = "" }
            return false
        }
        let fp = Fingerprint.of(SecCertificateCopyData(leaf) as Data)
        if pin.matches(fp) {
            lock.withLock { _seen = fp; _rejected = nil }
            return true
        }
        lock.withLock { _rejected = fp }
        return false
    }

    func urlSession(_ session: URLSession, webSocketTask: URLSessionWebSocketTask, didOpenWithProtocol protocol: String?) {
        opened()
    }
}

/// One dial to one of the Mac's addresses: a WebSocket that opens once TLS accepted the pinned
/// certificate and the upgrade went through, then carries messages until it closes.
protocol MacLink: AnyObject {
    var address: String { get }
    var onOpen: (() -> Void)? { get set }
    var onMessage: ((ServerMessage) -> Void)? { get set }
    /// Once, when it ends by itself (not after `close`).
    var onClose: ((LinkFailure) -> Void)? { get set }
    /// The fingerprint of the certificate TLS accepted (nil for plain `ws://`).
    var fingerprint: String? { get }
    /// The phone's interface it runs over, once open, when it can tell (USB, Wi-Fi…).
    var via: Via? { get }
    func start(timeout: TimeInterval)
    func send(_ text: String, done: @escaping @MainActor (Bool) -> Void)
    func close()
}

extension MacLink {
    var via: Via? { nil }
}

/// The clock the client reads and schedules on (a script replaces it to run time by hand).
class Timeline {
    func now() -> TimeInterval { Date().timeIntervalSinceReferenceDate }
    func after(_ delay: TimeInterval, _ work: @escaping @MainActor () -> Void) {
        DispatchQueue.main.asyncAfter(deadline: .now() + delay) { work() }
    }
}

/// The live connection to Trek on a Mac: a WebSocket speaking protocol v1 (docs/MOBILE.md), over
/// TLS pinned to the Mac's certificate unless the user chose an unencrypted connection.
///
/// Finding the Mac: every way to it is dialled at once (`Candidates`: what Bonjour finds on this
/// network under the paired Mac's id, where it answered last, the routes it named, addresses it
/// answered at before, its tailnet address), the local ones a moment ahead off the tailnet; the
/// first to open with the pinned certificate authenticates and the rest are closed. A new search
/// starts at once when the network changes, the app comes to the front, the user asks or a working
/// connection drops; while nothing answers it goes round again after 1, 2, 4, 8, then every 10 s,
/// and Bonjour stays open so a Mac that appears is dialled the moment it's seen. A refused token
/// or code, or a certificate that doesn't match the pin, stops it.
final class TrekClient: Backend {
    enum Auth {
        case pair(code: String, deviceId: String, deviceName: String)
        case hello(deviceId: String, token: String)
    }

    var onMessage: ((ServerMessage) -> Void)?
    var onState: ((ConnectionState) -> Void)?
    /// What's going on, in words ("Looking for … on this Wi-Fi…", "Connected over Tailscale"),
    /// for the Threads list and Settings.
    var onDetail: ((String) -> Void)?
    /// Called once when pairing succeeds, with the device token to keep and the transport to keep
    /// using (for TLS, pinned to the full fingerprint seen while pairing).
    var onPaired: ((String, HostInfo, Transport) -> Void)?
    /// What to keep for the next launch, when it changed: where the Mac answered (dialled first
    /// then), the routes it names, and other addresses it answered at before.
    var onRemember: ((_ address: String, _ routes: [String], _ known: [String]) -> Void)?

    /// Where the Mac answered last: the first dialled.
    private(set) var address: String
    /// Every address the Mac says it answers on (its Wi-Fi and its tailnet), as it last said.
    private(set) var routes: [String]
    /// Addresses the Mac answered at before (another network it was on), newest first.
    private(set) var known: [String]
    /// The paired Mac's lasting id, which its Bonjour record carries: with it the Mac is looked
    /// for on this network. Set when pairing succeeds, for the client that paired stays the live one.
    var hostId: String? {
        didSet {
            guard hostId != oldValue else { return }
            finder?.cancel()
            finder = nil
            if !stopped, !authenticated { startFinder() }
        }
    }
    /// The Mac's name, for the words ("Looking for Toby's MacBook Air on this Wi-Fi…").
    var hostName: String?
    private var auth: Auth
    private(set) var transport: Transport

    // What the client runs on, replaced by a script that drives it without a network.
    var makeLink: (String, Transport) -> MacLink = {
        // A URL can't name a Bonjour service, or carry a link-local address's interface.
        Address.needsInterface($0) ? NetworkLink(address: $0, transport: $1) : SocketLink(address: $0, transport: $1)
    }
    var makeFinder: (String) -> MacBrowsing = { MacFinder(hostId: $0) }
    var paths: PathWatching = PathWatcher()
    var timeline = Timeline()

    private(set) var stopped = false
    private(set) var authenticated = false
    /// The link that won the round: authenticating, then the connection.
    private(set) var live: MacLink?
    /// This round's dials still under way, by address.
    private var links: [String: MacLink] = [:]
    private(set) var round: DialRound?
    private var backoff = Backoff()
    private var heartbeat: Heartbeat?
    private var finder: MacBrowsing?
    /// Addresses Bonjour found the Mac at on this network (and its service on each interface it
    /// was seen on), newest first.
    private(set) var found: [String] = []
    /// The interface each of `found` was found on.
    private(set) var foundVia: [String: Via] = [:]
    /// When the round under way began: a sighting just after it (the round's own resolve) is
    /// what it's dialling already, not news.
    private var roundStarted: TimeInterval = 0
    private(set) var path: NetworkPath?
    /// iOS doesn't let Trek look on the local network.
    private var denied = false
    private var search = Search()
    /// Bumped to call off what's scheduled for a round (its head starts, the retry after it).
    private var epoch = 0
    /// Bumped to call off the heartbeat.
    private var beat = 0
    private(set) var state: ConnectionState = .idle
    private(set) var detail: String?
    /// What the connection runs over (USB, Wi-Fi, Tailscale…), while connected.
    private(set) var connectedOver: LinkKind?

    /// What this search (since the connection went, the network changed, the app came to the
    /// front or the user asked) has learnt.
    private struct Search {
        var started: TimeInterval = 0
        var seen = false
        /// Addresses dialled that failed.
        var failed: Set<String> = []
    }

    /// How long a dial may take to open.
    static let dialTimeout: TimeInterval = 8
    /// How long the Mac may take to answer `hello` or `pair` on a socket that opened.
    static let authTimeout: TimeInterval = 8
    /// How long a ping sent to check the connection (the app came back, the network changed)
    /// may go unanswered before the connection is taken for dead.
    static let probeTimeout: TimeInterval = 4
    /// How long after a working connection dropped the Mac is dialled again (where it answered
    /// first: a blip doesn't send the phone round its tailnet address).
    static let redialAfter: TimeInterval = 0.5

    init(address: String, routes: [String] = [], known: [String] = [], auth: Auth, transport: Transport,
         hostId: String? = nil, hostName: String? = nil) {
        self.address = address
        self.routes = routes
        self.known = known
        self.auth = auth
        self.transport = transport
        self.hostId = hostId
        self.hostName = hostName
    }

    /// Something a dial can be made to: a URL, or (Network.framework) the Mac's Bonjour service
    /// or an address scoped to an interface.
    static func dialable(_ address: String, encrypted: Bool) -> Bool {
        Address.needsInterface(address) ? NetworkLink.endpoint(for: address) != nil : url(for: address, encrypted: encrypted) != nil
    }

    /// `host:port` (any scheme the user typed is dropped) as a `wss://` or `ws://` URL.
    static func url(for address: String, encrypted: Bool) -> URL? {
        var host = address.trimmingCharacters(in: .whitespacesAndNewlines)
        for scheme in ["wss://", "ws://", "https://", "http://"] where host.lowercased().hasPrefix(scheme) {
            host = String(host.dropFirst(scheme.count))
        }
        while host.hasSuffix("/") { host.removeLast() }
        guard !host.isEmpty, !host.contains("/") else { return nil }
        return URL(string: "\(encrypted ? "wss" : "ws")://\(host)/")
    }

    /// The Mac's tailnet address, or its MagicDNS name.
    static func isTailnet(_ address: String) -> Bool { Address.isTailnet(address) }
    /// An address on the network the phone is on (not Tailscale's), or a `.local` name.
    static func isLocal(_ address: String) -> Bool { Address.isLocal(address) }

    /// What pairing ends with when nothing answered.
    static let unreachable = "Mac unreachable — retrying"
    static let tailnetUnreachable = "Can't reach your Mac over Tailscale — retrying. Is Tailscale on, on this iPhone?"

    // MARK: Triggers

    func start() {
        stopped = false
        paths.onChange = { [weak self] in self?.pathChanged($0) }
        paths.start()
        beginSearch()
    }

    func stop() {
        stopped = true
        abandonRound()
        beat += 1
        heartbeat = nil
        live?.close()
        live = nil
        authenticated = false
        finder?.cancel()
        finder = nil
        paths.cancel()
    }

    /// Back in front after a while away. A connection that looks open is pinged, and dropped if
    /// the Mac doesn't answer at once; without one, the Mac is looked for afresh now, not at the
    /// next retry.
    func wake() {
        guard !stopped else { return }
        if authenticated { probe() } else { beginSearch() }
    }

    /// The user asked (a tap, or pull to refresh found the connection not answering): drop this
    /// connection and find the Mac afresh now.
    func reconnect() {
        guard !stopped else { return }
        if authenticated { silent() } else { beginSearch() }
    }

    private func pathChanged(_ p: NetworkPath) {
        let was = path
        path = p
        // The first report says what the network is; it changes nothing.
        guard !stopped, let was else { updateStatus(); return }
        guard was != p else { return }
        // Another network: what Bonjour found was on the old one.
        found = []
        foundVia = [:]
        if authenticated {
            if p.up { probe() }
            return
        }
        finder?.cancel()
        finder = nil
        guard live == nil else { return }
        abandonRound()
        search = Search(started: timeline.now())
        backoff.reset()
        guard p.up else { updateStatus(); return }
        startFinder()
        dialRound()
    }

    // MARK: Sending

    @discardableResult
    func send(_ message: ClientMessage, id: String?) -> Bool {
        guard let live, authenticated || isAuthMessage(message) else { return false }
        return send(message, id: id, on: live)
    }

    private func send(_ message: ClientMessage, id: String?, on link: MacLink) -> Bool {
        guard let data = try? message.encoded(id: id), let text = String(data: data, encoding: .utf8) else { return false }
        link.send(text) { [weak self, weak link] ok in
            guard !ok, let self, let link else { return }
            self.writeFailed(link)
        }
        return true
    }

    /// A write on the open connection failed: it's gone, so say so now (what waits on an answer
    /// stops waiting) rather than when a read or a ping notices. A failed handshake is the read's
    /// to report.
    private func writeFailed(_ link: MacLink) {
        guard link === live, authenticated, !stopped else { return }
        live = nil
        link.close()
        dropped()
    }

    private func isAuthMessage(_ m: ClientMessage) -> Bool {
        switch m {
        case .pair, .hello: true
        default: false
        }
    }

    // MARK: Finding the Mac

    /// Every way to the Mac worth dialling now.
    private func candidates() -> [Candidate] {
        Candidates.gather(preferred: address, found: hostId == nil ? [] : found, via: foundVia, routes: routes, known: known,
                          onLAN: path?.local ?? true)
    }

    /// Look for the Mac afresh: everything dialled now, the retry cadence from its start, the
    /// words from "Looking for…".
    private func beginSearch() {
        guard !stopped, !authenticated, live == nil else { return }
        // Just begun (launch, then the app's first activation): leave it be.
        if round != nil, timeline.now() - search.started < 1 { return }
        abandonRound()
        search = Search(started: timeline.now())
        backoff.reset()
        startFinder()
        dialRound()
    }

    /// Dial every candidate (those leading at once, the rest a moment later); a round under way
    /// takes in the new ones instead.
    private func dialRound() {
        guard !stopped, !authenticated, live == nil else { return }
        if round != nil { addCandidates(); return }
        epoch += 1
        let onLAN = path?.local ?? true
        let all = candidates().filter { Self.dialable($0.address, encrypted: transport.isEncrypted) }
        guard !all.isEmpty else {
            setState(.offline("Bad address"))
            return
        }
        round = DialRound(all)
        roundStarted = timeline.now()
        startFinder()
        finder?.resolveAgain()
        let e = epoch
        for (c, delay) in zip(all, Candidates.delays(all, onLAN: onLAN)) {
            if delay == 0 {
                dial(c)
            } else {
                timeline.after(delay) { [weak self] in
                    guard let self, self.epoch == e else { return }
                    self.dial(c)
                }
            }
        }
        // The words move on after a few seconds even when nothing else happens.
        let wait = search.started + ReachStatus.isolationAfter - timeline.now()
        if wait > 0 { timeline.after(wait) { [weak self] in self?.updateStatus() } }
        updateStatus()
    }

    /// Mid-round, what's new (Bonjour found the Mac) is dialled at once.
    private func addCandidates() {
        for c in candidates() where Self.dialable(c.address, encrypted: transport.isEncrypted) {
            if round?.add(c) == true { dial(c) }
        }
    }

    private func dial(_ c: Candidate) {
        guard !stopped, round?.dialling(c.address) == true else { return }
        let link = makeLink(c.address, transport)
        links[c.address] = link
        link.onOpen = { [weak self, weak link] in
            guard let self, let link else { return }
            self.opened(link)
        }
        link.onMessage = { [weak self, weak link] message in
            guard let self, let link, link === self.live else { return }
            self.received(message)
        }
        link.onClose = { [weak self, weak link] why in
            guard let self, let link else { return }
            self.closed(link, why)
        }
        link.start(timeout: Self.dialTimeout + 2)
        timeline.after(Self.dialTimeout) { [weak self, weak link] in
            guard let self, let link, self.links[link.address] === link else { return }
            link.close()
            self.closed(link, .unreachable)
        }
    }

    /// A socket opened with the pinned certificate. The first authenticates; the others close.
    private func opened(_ link: MacLink) {
        guard !stopped, links[link.address] === link, let losers = round?.opened(link.address) else {
            if links[link.address] === link { links[link.address] = nil }
            link.close()
            return
        }
        links[link.address] = nil
        for l in losers { links.removeValue(forKey: l)?.close() }
        live = link
        let first: ClientMessage = switch auth {
        case .pair(let code, let deviceId, let deviceName): .pair(code: code, deviceId: deviceId, deviceName: deviceName)
        case .hello(let deviceId, let token): .hello(deviceId: deviceId, token: token)
        }
        _ = send(first, id: "auth", on: link)
        // A Mac that opens and never answers is given up on.
        timeline.after(Self.authTimeout) { [weak self, weak link] in
            guard let self, let link, self.live === link, !self.authenticated, !self.stopped else { return }
            link.close()
            self.closed(link, .dropped)
        }
    }

    private func closed(_ link: MacLink, _ why: LinkFailure) {
        guard !stopped else { return }
        if link === live {
            live = nil
            if authenticated {
                dropped()
                return
            }
            // Opened, then went before the Mac said welcome.
            noteFailure(link.address, why)
            if round != nil {
                round?.failed(link.address, why)
                roundProgress()
            } else {
                retryLater()
            }
            return
        }
        guard links[link.address] === link else { return }
        links[link.address] = nil
        noteFailure(link.address, why)
        round?.failed(link.address, why)
        roundProgress()
    }

    /// For the words: an address that answered with another certificate did answer.
    private func noteFailure(_ address: String, _ why: LinkFailure) {
        if case .mismatch = why { return }
        search.failed.insert(address)
    }

    private func roundProgress() {
        guard let r = round else { return }
        if r.lost {
            roundLost()
            return
        }
        // Everything started has failed already: the rest go now, not after their head start.
        if r.winner == nil, !r.anyDialling {
            for c in r.waiting { dial(c) }
        }
        updateStatus()
    }

    /// Nothing answered this time.
    private func roundLost() {
        guard let r = round else { return }
        round = nil
        for l in links.values { l.close() }
        links = [:]
        // Not a network problem: what answered had another certificate. Never trusted, never
        // fallen back to plain. When every address answers like that, or one that is surely the
        // Mac's (its Bonjour record led there, or its tailnet address), the Mac has changed (or
        // something poses as it): stop. Otherwise the one that did may simply be another
        // machine that has the Mac's old address on this network.
        if case .tls(let pin) = transport, r.everyMismatched || r.ownMismatched {
            stop()
            setState(.identityChanged(expected: pin.short, seen: r.mismatchSeen.map(Fingerprint.short)))
            return
        }
        if case .pair = auth {
            // Pairing, the Mac answered, but not with TLS (an older Trek, or TREK_REMOTE_PLAIN):
            // don't hammer it, and don't downgrade by ourselves either. A Mac this phone paired
            // with over TLS served it then: a failed handshake now (one cut off as Trek on the
            // Mac restarts) is a drop like any other, retried.
            if case .tls = transport, r.anyNotTLS {
                stop()
                setState(.offline("Couldn't open an encrypted connection to that Mac. Use its IP address, and check that Trek on the Mac is up to date."))
                return
            }
            setState(.offline(r.candidates.allSatisfy { $0.kind == .tailnet } ? Self.tailnetUnreachable : Self.unreachable))
        }
        retryLater()
        updateStatus()
    }

    /// Go round again after the next step of the cadence (sooner if something happens).
    private func retryLater() {
        let e = epoch
        timeline.after(backoff.next()) { [weak self] in
            guard let self, self.epoch == e else { return }
            self.dialRound()
        }
    }

    /// Give up the round under way: its dials close and what's scheduled for it is called off.
    private func abandonRound() {
        epoch += 1
        for l in links.values { l.close() }
        links = [:]
        round = nil
    }

    private func startFinder() {
        guard finder == nil, let hostId, !stopped else { return }
        let f = makeFinder(hostId)
        f.onEvent = { [weak self] in self?.finderEvent($0) }
        finder = f
        f.start()
    }

    private func finderEvent(_ event: FinderEvent) {
        guard !stopped, !authenticated else { return }
        switch event {
        case .browsing:
            guard denied else { return }
            denied = false
            updateStatus()
        case .seen:
            denied = false
            guard !search.seen else { return }
            search.seen = true
            updateStatus()
        case .denied:
            denied = true
            updateStatus()
        case .found(let a, let via):
            denied = false
            search.seen = true
            found.removeAll { $0 == a }
            found.insert(a, at: 0)
            if let via { foundVia[a] = via }
            // A fresh sighting: dialled now, failed before or not.
            search.failed.remove(a)
            guard live == nil else { return }
            if round != nil {
                addCandidates()
                // Each round resolves again: what that brings back within a second is what it's
                // dialling already. Starting over on it would go round as fast as an address
                // refuses (the Mac up, but not listening on the cable).
                let news = timeline.now() - roundStarted >= 1
                if news, round?.dials[a].map({ if case .failed = $0 { true } else { false } }) == true {
                    // Already dialled this round and failed: a round of its own, now.
                    abandonRound()
                    dialRound()
                }
            } else {
                backoff.reset()
                dialRound()
            }
            updateStatus()
        }
    }

    // MARK: Connected

    private func received(_ message: ServerMessage) {
        heartbeat?.heard(at: timeline.now())
        handle(message)
    }

    private func handle(_ message: ServerMessage) {
        switch message {
        case .paired(_, let token, let host, _, let routes):
            hostName = host.name
            welcomed(routes: routes)
            auth = .hello(deviceId: Device.id, token: token)
            // Typed pairing pinned only 16 hex characters; from now on, the whole certificate.
            if case .tls = transport, let seen = live?.fingerprint { transport = .tls(.full(seen)) }
            onPaired?(token, host, transport)
            // The model learns the Mac (what it serves) before it hears it's connected and asks.
            onMessage?(message)
            setState(.connected)
            return
        case .welcome(_, let host, _, let routes):
            hostName = host.name
            welcomed(routes: routes)
            onMessage?(message)
            setState(.connected)
            return
        case .error(_, let code, let text) where !authenticated:
            switch code {
            case .unauthorized:
                stop()
                setState(.unauthorized("This iPhone isn't paired with that Mac any more. Pair again."))
            case .pairingFailed:
                stop()
                setState(.unauthorized(text.isEmpty ? "That code didn't work. Codes last 10 minutes." : text))
            case .unsupportedProtocol:
                stop()
                setState(.unauthorized("Update Trek on your Mac or this app: they speak different versions."))
            default:
                setState(.offline(text))
            }
        default:
            break
        }
        onMessage?(message)
    }

    /// The Mac said welcome on the live link: that address is the one to dial first from now on,
    /// and the routes it names are kept.
    private func welcomed(routes named: [String]) {
        guard let link = live else { return }
        authenticated = true
        round = nil
        epoch += 1
        backoff.reset()
        finder?.cancel()
        finder = nil
        var changed = false
        if !named.isEmpty, named != routes {
            routes = named
            changed = true
        }
        // An address tied to an interface (the cable's link-local one, the service seen on it)
        // isn't kept: it may be named otherwise next time, and the Wi-Fi one stays the first.
        if link.address != address, Address.lasting(link.address) {
            known = Candidates.remember(address, in: known, except: [link.address])
            address = link.address
            changed = true
        }
        if changed { onRemember?(address, routes, known) }
        startHeartbeat()
        connectedOver = ReachStatus.link(address: link.address, used: link.via, seen: foundVia[link.address], path: path)
        setDetail(ReachStatus.connected(via: connectedOver))
    }

    /// A working connection went (the phone slept, the Wi-Fi blinked, Trek on the Mac
    /// restarted): look again, where it answered first.
    private func dropped() {
        authenticated = false
        beat += 1
        heartbeat = nil
        guard !stopped else { return }
        search = Search(started: timeline.now())
        backoff.reset()
        setDetail(ReachStatus.reconnecting(macName))
        setState(.connecting)
        startFinder()
        let e = epoch
        timeline.after(Self.redialAfter) { [weak self] in
            guard let self, self.epoch == e else { return }
            self.dialRound()
        }
    }

    private func startHeartbeat() {
        heartbeat = Heartbeat(now: timeline.now())
        beat += 1
        scheduleBeat(beat)
    }

    private func scheduleBeat(_ b: Int) {
        timeline.after(Heartbeat.tick) { [weak self] in
            guard let self, self.beat == b, self.authenticated, var hb = self.heartbeat else { return }
            let action = hb.check(at: self.timeline.now())
            self.heartbeat = hb
            switch action {
            case .drop:
                self.silent()
                return
            case .ping:
                self.send(.ping, id: nil)
            case .wait:
                break
            }
            self.scheduleBeat(b)
        }
    }

    /// Is the connection still there? A ping now, and the connection is dropped (and the Mac
    /// looked for afresh) if nothing comes back soon.
    private func probe() {
        guard authenticated, let hb = heartbeat else { return }
        let sent = timeline.now()
        if hb.silent(at: sent) {
            silent()
            return
        }
        send(.ping, id: nil)
        let b = beat
        timeline.after(Self.probeTimeout) { [weak self] in
            guard let self, self.beat == b, self.authenticated, let hb = self.heartbeat, hb.heard < sent else { return }
            self.silent()
        }
    }

    /// Nothing has come from the Mac for too long: drop this socket and connect afresh, rather
    /// than show a connection that no longer brings anything.
    private func silent() {
        guard let link = live, authenticated else { return }
        live = nil
        link.close()
        dropped()
    }

    // MARK: Words

    private var macName: String { hostName.flatMap { $0.isEmpty ? nil : $0 } ?? "your Mac" }

    /// What the words are made from.
    func facts() -> ReachFacts {
        let all = candidates()
        // What Bonjour found on a cable to the Mac (USB, Ethernet) is its own way, apart from the Wi-Fi.
        let cabled = all.filter(\.wired)
        let local = all.filter { $0.kind == .local && !$0.wired }.map(\.address)
        let tailnet = all.filter { $0.kind == .tailnet }.map(\.address)
        let answered = ([address] + known).filter { Address.kind($0) == .local }
        let onWiFi = found.filter { foundVia[$0]?.kind.wired != true }
        let cable: LinkKind? = cabled.contains { $0.via?.kind == .usb } ? .usb : cabled.first?.via?.kind
        return ReachFacts(
            mac: macName,
            path: path,
            localDenied: denied,
            bonjourSeen: !onWiFi.isEmpty || (search.seen && cabled.isEmpty),
            bonjourFound: !onWiFi.isEmpty,
            cable: cable,
            cableFailed: !cabled.isEmpty && cabled.map(\.address).allSatisfy(search.failed.contains),
            hasLocal: !local.isEmpty,
            hasTailnet: !tailnet.isEmpty,
            localFailed: !local.isEmpty && local.allSatisfy(search.failed.contains),
            tailnetFailed: !tailnet.isEmpty && tailnet.allSatisfy(search.failed.contains),
            macsNetwork: path?.subnet.map { s in answered.contains(where: s.contains) } ?? false,
            elapsed: timeline.now() - search.started)
    }

    private func updateStatus() {
        guard !stopped, !authenticated, live == nil else { return }
        // The network is reported a moment after launch: words that suit it, rather than a guess first.
        if path == nil, timeline.now() - search.started < 1 {
            if state == .idle { setState(.connecting) }
            return
        }
        let status = ReachStatus.searching(facts())
        setDetail(status.text)
        // Pairing ends on what the round says (`roundLost`), not on the words on the way.
        if case .pair = auth {
            if state == .idle { setState(.connecting) }
            return
        }
        setState(status.trying ? .connecting : .offline(status.text))
    }

    private func setState(_ s: ConnectionState) {
        guard s != state else { return }
        state = s
        onState?(s)
    }

    private func setDetail(_ text: String) {
        guard text != detail else { return }
        detail = text
        onDetail?(text)
    }
}

/// One WebSocket to one of the Mac's addresses (`URLSessionWebSocketTask`, its own session so
/// the pin's verdict is its own).
final class SocketLink: MacLink {
    let address: String
    var onOpen: (() -> Void)?
    var onMessage: ((ServerMessage) -> Void)?
    var onClose: ((LinkFailure) -> Void)?
    private let transport: Transport
    private var session: URLSession?
    private var task: URLSessionWebSocketTask?
    private var pinning: PinningDelegate?
    private var isOpen = false
    private var closed = false

    init(address: String, transport: Transport) {
        self.address = address
        self.transport = transport
    }

    var fingerprint: String? { pinning?.seen }

    func start(timeout: TimeInterval) {
        guard let url = TrekClient.url(for: address, encrypted: transport.isEncrypted) else {
            fail(.unreachable)
            return
        }
        let config = URLSessionConfiguration.ephemeral
        config.urlCache = nil
        config.httpCookieStorage = nil
        config.waitsForConnectivity = false
        let pin: CertificatePin? = if case .tls(let p) = transport { p } else { nil }
        let delegate = PinningDelegate(pin: pin) { [weak self] in
            Task { @MainActor in self?.didOpen() }
        }
        pinning = delegate
        let session = URLSession(configuration: config, delegate: delegate, delegateQueue: nil)
        self.session = session
        var request = URLRequest(url: url)
        request.timeoutInterval = timeout
        let task = session.webSocketTask(with: request)
        // Above anything the Mac sends (it clips an item's text well under this): a frame over
        // the limit ends the connection, and reconnecting would only meet it again.
        task.maximumMessageSize = 16 << 20
        self.task = task
        task.resume()
        Task { await receive(task) }
    }

    func send(_ text: String, done: @escaping @MainActor (Bool) -> Void) {
        guard let task, !closed else {
            done(false)
            return
        }
        task.send(.string(text)) { error in
            Task { @MainActor in done(error == nil) }
        }
    }

    func close() {
        guard !closed else { return }
        teardown()
    }

    private func didOpen() {
        guard !closed, !isOpen else { return }
        isOpen = true
        onOpen?()
    }

    private func receive(_ task: URLSessionWebSocketTask) async {
        while !closed {
            do {
                let frame = try await task.receive()
                let data: Data
                switch frame {
                case .string(let s): data = Data(s.utf8)
                case .data(let d): data = d
                @unknown default: continue
                }
                // Read off the main thread (a transcript can be large); handled back on it, in order.
                let message = await Perf.measureAsync("decode", "\(data.count / 1024) KB") { await ServerMessage.decodeInBackground(data) }
                guard !closed, let message else { continue }
                onMessage?(message)
            } catch {
                fail(failure(error))
                return
            }
        }
    }

    private func failure(_ error: Error) -> LinkFailure {
        if let rejected = pinning?.rejected { return .mismatch(rejected) }
        if isOpen { return .dropped }
        if case .tls = transport, Self.isTLSFailure(error) { return .notTLS }
        return .unreachable
    }

    /// A handshake that failed for a reason other than our pin (e.g. the server speaks plain HTTP).
    private static func isTLSFailure(_ error: Error) -> Bool {
        let code = (error as NSError).code
        return (error as NSError).domain == NSURLErrorDomain
            && [NSURLErrorSecureConnectionFailed, NSURLErrorCannotLoadFromNetwork].contains(code)
    }

    private func fail(_ why: LinkFailure) {
        guard !closed else { return }
        teardown()
        onClose?(why)
    }

    private func teardown() {
        closed = true
        task?.cancel(with: .goingAway, reason: nil)
        task = nil
        session?.invalidateAndCancel()
        session = nil
    }
}

/// One WebSocket over `NWConnection`, for the ways to the Mac a URL can't name: its Bonjour
/// service on one interface (`Address.service`: Network.framework resolves it there and picks the
/// address and its link-local scope itself), or an address scoped to one (`[fe80::1%en7]:7420`,
/// dialled as a host that carries the interface). The certificate is pinned as `SocketLink`'s is,
/// before any byte of ours goes out.
final class NetworkLink: MacLink {
    let address: String
    var onOpen: (() -> Void)?
    var onMessage: ((ServerMessage) -> Void)?
    var onClose: ((LinkFailure) -> Void)?
    private(set) var via: Via?
    private let transport: Transport
    private let pinning: PinningDelegate
    private var connection: NWConnection?
    private var isOpen = false
    private var closed = false
    private nonisolated static let queue = DispatchQueue(label: "trek.link")

    init(address: String, transport: Transport) {
        self.address = address
        self.transport = transport
        let pin: CertificatePin? = if case .tls(let p) = transport { p } else { nil }
        pinning = PinningDelegate(pin: pin)
    }

    var fingerprint: String? { pinning.seen }

    /// Where `address` leads: the service on its interface, or the scoped host and port. Nil
    /// for anything else (a plain `host:port` is `SocketLink`'s).
    nonisolated static func endpoint(for address: String) -> NWEndpoint? {
        if let s = Address.serviceParts(address) {
            return .service(name: s.name, type: s.type, domain: s.domain, interface: s.interface.flatMap(SeenInterfaces.named))
        }
        let (hostScoped, portText) = Address.split(address)
        let (host, name) = Address.unscoped(hostScoped)
        guard let name, let portText, let n = UInt16(portText), let port = NWEndpoint.Port(rawValue: n) else { return nil }
        let interface = SeenInterfaces.named(name)
        if let v6 = IPv6Address(host) {
            // Network.framework scopes a link-local address by its `%en7` itself for the
            // interfaces it lists; one only Bonjour saw is given its interface by hand.
            guard let interface, let scoped = IPv6Address(v6.rawValue, interface) else {
                return .hostPort(host: NWEndpoint.Host(hostScoped), port: port)
            }
            return .hostPort(host: .ipv6(scoped), port: port)
        }
        if let v4 = IPv4Address(host) {
            return .hostPort(host: .ipv4(interface.flatMap { IPv4Address(v4.rawValue, $0) } ?? v4), port: port)
        }
        return nil
    }

    nonisolated static func parameters(_ transport: Transport, pinning: PinningDelegate, interface: NWInterface?) -> NWParameters {
        let params: NWParameters
        if transport.isEncrypted {
            let tls = NWProtocolTLS.Options()
            // The pin is the whole of the trust decision (the certificate is self-signed).
            sec_protocol_options_set_verify_block(tls.securityProtocolOptions, { _, trust, complete in
                complete(pinning.check(sec_trust_copy_ref(trust).takeRetainedValue()))
            }, queue)
            params = NWParameters(tls: tls)
        } else {
            params = .tcp
        }
        let ws = NWProtocolWebSocket.Options()
        ws.autoReplyPing = true
        // As `SocketLink`: above anything the Mac sends.
        ws.maximumMessageSize = 16 << 20
        params.defaultProtocolStack.applicationProtocols.insert(ws, at: 0)
        params.includePeerToPeer = false
        if let interface { params.requiredInterface = interface }
        return params
    }

    func start(timeout: TimeInterval) {
        guard let endpoint = Self.endpoint(for: address) else {
            fail(.unreachable)
            return
        }
        let params = Self.parameters(transport, pinning: pinning, interface: Address.interface(of: address).flatMap(SeenInterfaces.named))
        if let tcp = params.defaultProtocolStack.transportProtocol as? NWProtocolTCP.Options {
            tcp.connectionTimeout = max(1, Int(timeout))
        }
        let c = NWConnection(to: endpoint, using: params)
        connection = c
        c.stateUpdateHandler = { [weak self] state in
            Task { @MainActor in self?.changed(state) }
        }
        c.start(queue: Self.queue)
    }

    func send(_ text: String, done: @escaping @MainActor (Bool) -> Void) {
        guard let connection, isOpen, !closed else {
            done(false)
            return
        }
        let frame = NWConnection.ContentContext(identifier: "text", metadata: [NWProtocolWebSocket.Metadata(opcode: .text)])
        connection.send(content: Data(text.utf8), contentContext: frame, isComplete: true, completion: .contentProcessed { error in
            Task { @MainActor in done(error == nil) }
        })
    }

    func close() {
        guard !closed else { return }
        teardown()
    }

    private func changed(_ state: NWConnection.State) {
        guard !closed else { return }
        switch state {
        case .ready:
            // TLS accepted the pinned certificate and the upgrade went through.
            guard !isOpen else { return }
            isOpen = true
            via = connection?.currentPath.flatMap { $0.availableInterfaces.first }.flatMap(MacFinder.via)
            receive()
            onOpen?()
        case .waiting(let error), .failed(let error):
            // Waiting means refused or no route, until the network changes: the client's next
            // round is the retry.
            fail(failure(error))
        default:
            break
        }
    }

    private func receive() {
        connection?.receiveMessage { [weak self] data, context, _, error in
            let meta = context?.protocolMetadata(definition: NWProtocolWebSocket.definition) as? NWProtocolWebSocket.Metadata
            let text = meta.map { $0.opcode == .text || $0.opcode == .binary } ?? false
            let ended = meta?.opcode == .close || (context?.isFinal ?? false)
            Task { @MainActor in self?.received(text ? data : nil, ended: ended, error) }
        }
    }

    private func received(_ data: Data?, ended: Bool, _ error: NWError?) {
        guard !closed else { return }
        if let error {
            fail(failure(error))
            return
        }
        guard let data, !data.isEmpty else {
            if ended { fail(.dropped) } else { receive() }
            return
        }
        // Read off the main thread (a transcript can be large); handled back on it, in order.
        Task { [weak self] in
            let message = await Perf.measureAsync("decode", "\(data.count / 1024) KB") { await ServerMessage.decodeInBackground(data) }
            guard let self, !self.closed else { return }
            if let message { self.onMessage?(message) }
            self.receive()
        }
    }

    private func failure(_ error: NWError) -> LinkFailure {
        if let rejected = pinning.rejected { return .mismatch(rejected) }
        if isOpen { return .dropped }
        if transport.isEncrypted, case .tls = error { return .notTLS }
        return .unreachable
    }

    private func fail(_ why: LinkFailure) {
        guard !closed else { return }
        teardown()
        onClose?(why)
    }

    private func teardown() {
        closed = true
        connection?.stateUpdateHandler = nil
        connection?.cancel()
        connection = nil
    }
}

/// A `trek://pair?host=…&code=…&name=…&hid=…&fp=…` link from the Mac's QR code.
struct PairingLink: Equatable, Identifiable {
    var address: String
    /// The Mac's other addresses (`alt`): its tailnet one beside its Wi-Fi one.
    var alternates: [String] = []
    var code: String
    var name: String?
    var hostId: String?
    /// The full certificate fingerprint; nil for Macs that don't serve TLS (plain `ws://`).
    var fingerprint: String?

    var id: String { "\(address)|\(code)|\(fingerprint ?? "")" }

    init(address: String, code: String, name: String? = nil, hostId: String? = nil, fingerprint: String?) {
        self.address = address
        self.code = code
        self.name = name
        self.hostId = hostId
        self.fingerprint = fingerprint
    }

    /// Nil for anything that isn't a well-formed pairing link, including one whose `fp` is
    /// present but not a SHA-256 (rather than quietly treating it as a plain-`ws://` Mac).
    init?(_ string: String) {
        guard let comps = URLComponents(string: string.trimmingCharacters(in: .whitespacesAndNewlines)),
              comps.scheme == "trek", comps.host == "pair" || comps.path.contains("pair") else { return nil }
        let q = Dictionary((comps.queryItems ?? []).map { ($0.name, $0.value ?? "") }, uniquingKeysWith: { a, _ in a })
        guard let host = q["host"], !host.isEmpty, let code = q["code"], !code.isEmpty else { return nil }
        address = host
        alternates = (q["alt"] ?? "").split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty && $0 != host }
        self.code = code
        name = q["name"].flatMap { $0.isEmpty ? nil : $0 }
        hostId = q["hid"].flatMap { $0.isEmpty ? nil : $0 }
        if let fp = q["fp"] {
            guard let full = Fingerprint.full(fp) else { return nil }
            fingerprint = full
        } else {
            fingerprint = nil
        }
    }

    var transport: Transport { fingerprint.map { .tls(.full($0)) } ?? .plain }
}
