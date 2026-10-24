import Foundation

// How the phone finds the paired Mac and says what it's doing (docs/MOBILE.md, "Finding the
// Mac"). Plain values with no networking in them: `TrekClient` drives them, and a script can
// exercise them without a device.

/// `host:port` strings: the Mac's Wi-Fi address, its tailnet one, one Bonjour resolved (a
/// link-local one scoped to its interface, `[fe80::1%en7]:7420`), or the Mac's Bonjour service on
/// one interface (`bonjour:en7/local./_trek._tcp/Toby's MacBook Air`).
nonisolated enum Address {
    enum Kind: Equatable { case local, tailnet, other }

    static let servicePrefix = "bonjour:"

    /// The Mac's Bonjour service seen on `interface` ("" for any), dialled as itself: Network.framework
    /// resolves it and picks the interface and the link-local scoping.
    static func service(name: String, type: String, domain: String, interface: String?) -> String {
        "\(servicePrefix)\(interface ?? "")/\(domain)/\(type)/\(name)"
    }

    /// The parts of a `service(…)` address; nil for any other.
    static func serviceParts(_ address: String) -> (name: String, type: String, domain: String, interface: String?)? {
        guard address.hasPrefix(servicePrefix) else { return nil }
        let parts = address.dropFirst(servicePrefix.count).split(separator: "/", maxSplits: 3, omittingEmptySubsequences: false)
        guard parts.count == 4, !parts[3].isEmpty else { return nil }
        return (String(parts[3]), String(parts[2]), String(parts[1]), parts[0].isEmpty ? nil : String(parts[0]))
    }

    static func isService(_ address: String) -> Bool { serviceParts(address) != nil }

    /// A host without its interface scope (`fe80::1%en7` → `fe80::1`, `en7`).
    static func unscoped(_ host: String) -> (host: String, interface: String?) {
        guard let i = host.firstIndex(of: "%") else { return (host, nil) }
        let name = String(host[host.index(after: i)...])
        return (String(host[..<i]), name.isEmpty ? nil : name)
    }

    /// The interface an address is tied to: a scoped one's, or a service's.
    static func interface(of address: String) -> String? {
        if let s = serviceParts(address) { return s.interface }
        return unscoped(split(address).host).interface
    }

    /// Dialled with Network.framework rather than as a URL: a service, or an address scoped to an
    /// interface (a URL can't carry the scope).
    static func needsInterface(_ address: String) -> Bool {
        isService(address) || interface(of: address) != nil
    }

    /// Dialled with `NetworkLink` rather than a URL session: a Bonjour service or an address
    /// tied to an interface (a URL can't name them), and a tailnet IP address. App Transport
    /// Security holds URL sessions to its certificate rules on any address it doesn't count as
    /// local, and a tailnet one (reached through the Tailscale tunnel) isn't: it refused the
    /// Mac's self-signed certificate there (-9802) before the pin was asked, so the phone never
    /// reached a Mac over Tailscale. Network.framework leaves the decision to the pin.
    static func needsNetworkLink(_ address: String) -> Bool {
        if needsInterface(address) { return true }
        guard isTailnet(address) else { return false }
        let host = unscoped(split(address).host).host
        return ipv4(host) != nil || host.contains(":")
    }

    /// Worth keeping as where the Mac answered, for another day: not one tied to an interface
    /// (the USB cable's link-local address, the service seen on it), which may be named otherwise
    /// next time.
    static func lasting(_ address: String) -> Bool { !needsInterface(address) }

    /// Host and port of `host:port`, `[v6]:port` or a bare host (a bare IPv6 address has no port).
    static func split(_ address: String) -> (host: String, port: String?) {
        let a = address.trimmingCharacters(in: .whitespacesAndNewlines)
        if a.hasPrefix("["), let close = a.firstIndex(of: "]") {
            let host = String(a[a.index(after: a.startIndex)..<close])
            let rest = a[a.index(after: close)...]
            return (host, rest.hasPrefix(":") ? String(rest.dropFirst()) : nil)
        }
        let parts = a.split(separator: ":", omittingEmptySubsequences: false)
        return parts.count == 2 ? (String(parts[0]), String(parts[1])) : (a, nil)
    }

    /// `host:port`, an IPv6 host in brackets.
    static func join(_ host: String, _ port: UInt16) -> String {
        host.contains(":") ? "[\(host)]:\(port)" : "\(host):\(port)"
    }

    static func ipv4(_ host: String) -> [UInt8]? {
        let parts = host.split(separator: ".", omittingEmptySubsequences: false)
        let octets = parts.compactMap { UInt8($0) }
        return parts.count == 4 && octets.count == 4 ? octets : nil
    }

    /// The Mac's tailnet address (100.64/10, or Tailscale's fd7a:115c:a1e0::/48), or its MagicDNS name.
    static func isTailnet(_ address: String) -> Bool {
        guard !isService(address) else { return false }
        let host = unscoped(split(address).host).host.lowercased()
        if host.hasSuffix(".ts.net") || host.hasPrefix("fd7a:115c:a1e0:") { return true }
        guard let o = ipv4(host) else { return false }
        return o[0] == 100 && (64..<128).contains(o[1])
    }

    /// An address on the network the phone is on: a private or link-local IPv4 one (not
    /// Tailscale's), an IPv6 unique-local or link-local one, a `.local` name, or the Mac's
    /// Bonjour service.
    static func isLocal(_ address: String) -> Bool {
        if isService(address) { return true }
        let host = unscoped(split(address).host).host.lowercased()
        if host.hasSuffix(".local") { return true }
        if host.contains(":") {
            guard !isTailnet(address) else { return false }
            return host.hasPrefix("fe80:") || host.hasPrefix("fc") || host.hasPrefix("fd")
        }
        guard let o = ipv4(host) else { return false }
        switch (o[0], o[1]) {
        case (10, _), (192, 168), (172, 16...31), (169, 254): return true
        default: return false
        }
    }

    static func kind(_ address: String) -> Kind {
        isTailnet(address) ? .tailnet : isLocal(address) ? .local : .other
    }
}

/// The IPv4 network the phone's Wi-Fi is on (its address and mask), to tell the Mac's own Wi-Fi
/// (an address that answered before is on it) from somewhere else.
nonisolated struct Subnet: Equatable {
    var address: UInt32
    var mask: UInt32

    init(address: UInt32, mask: UInt32) {
        (self.address, self.mask) = (address, mask)
    }

    init?(address: String, mask: String) {
        guard let a = Self.number(address), let m = Self.number(mask), m != 0 else { return nil }
        self.init(address: a, mask: m)
    }

    func contains(_ address: String) -> Bool {
        guard let n = Self.number(Address.split(address).host) else { return false }
        return n & mask == self.address & mask
    }

    static func number(_ host: String) -> UInt32? {
        Address.ipv4(host).map { $0.reduce(UInt32(0)) { $0 << 8 | UInt32($1) } }
    }
}

/// What a way to the Mac runs over, as the words name it.
nonisolated enum LinkKind: Equatable, Hashable {
    case wifi, ethernet, usb, cellular, tailscale

    /// "Connected over USB".
    var name: String {
        switch self {
        case .wifi: "Wi-Fi"
        case .ethernet: "Ethernet"
        case .usb: "USB"
        case .cellular: "cellular"
        case .tailscale: "Tailscale"
        }
    }

    /// A cable to the Mac, or a wired network: when the Mac's record is seen on one, it's dialled
    /// there first.
    var wired: Bool { self == .usb || self == .ethernet }
}

/// An interface of the phone (`en0`, `en7`, `anpi0`), and what kind of link it is.
nonisolated struct Via: Equatable, Hashable {
    /// `NWInterface.InterfaceType`, without the framework.
    enum InterfaceType: Equatable { case wifi, cellular, wiredEthernet, loopback, other }

    var name: String
    var kind: LinkKind

    /// What an interface is. Wi-Fi and cellular say so; Tailscale's is a tunnel (`utun`); one
    /// whose name says USB, or Apple's private links to a computer (`anpi`, `anri`), is the cable
    /// to the Mac; a wired one with an address of its own (not 169.254/16) is an Ethernet network,
    /// one with link-local addresses alone the cable (the Mac's "iPhone USB" link self-assigns).
    /// Anything else is taken for the cable too. Nil for loopback.
    static func kind(name: String, type: InterfaceType, routableIPv4: Bool) -> LinkKind? {
        let n = name.lowercased()
        switch type {
        case .loopback: return nil
        case .wifi: return .wifi
        case .cellular: return .cellular
        case .wiredEthernet, .other:
            if n.contains("usb") || n.hasPrefix("anpi") || n.hasPrefix("anri") { return .usb }
            if n.hasPrefix("utun") || n.hasPrefix("ipsec") { return .tailscale }
            return type == .wiredEthernet && routableIPv4 ? .ethernet : .usb
        }
    }
}

/// What the phone is connected through, as `NWPathMonitor` sees it.
nonisolated struct NetworkPath: Equatable {
    enum Kind: Equatable { case wifi, wired, cellular, other, none }
    var kind: Kind
    /// The interfaces up (`en0`, `pdp_ip0`, `utun4`…): Tailscale turning on adds one.
    var interfaces: [String] = []
    /// The Wi-Fi's IPv4 network, when the phone is on one.
    var subnet: Subnet? = nil

    var up: Bool { kind != .none }
    /// On a network the Mac may share (Wi-Fi or wired), so it's looked for there first.
    var local: Bool { kind == .wifi || kind == .wired }
}

/// One address the Mac may answer at, and how the phone came to know it.
nonisolated struct Candidate: Equatable, Hashable {
    enum Source: Equatable, Hashable {
        /// Found on this network just now, by Bonjour, under the paired Mac's id.
        case bonjour
        /// Where the Mac answered last.
        case preferred
        /// An address the Mac named in its last `welcome` (or the pairing code's `alt`).
        case route
        /// An address the Mac answered at before (another network it was on).
        case known
    }

    var address: String
    var source: Source
    /// The interface Bonjour saw the Mac on, for what it found.
    var via: Via? = nil
    var kind: Address.Kind { Address.kind(address) }
    /// Found on a cable to the Mac (USB, Ethernet), not the Wi-Fi.
    var wired: Bool { via?.kind.wired == true }
}

nonisolated enum Candidates {
    /// How long the first of them (the ones on the network the phone is on) is dialled alone
    /// before the rest start, as in Happy Eyeballs: on the Mac's Wi-Fi the answer comes well
    /// within it, so the tailnet isn't dialled for nothing.
    static let headStart: TimeInterval = 0.25

    /// Every way to the Mac worth dialling now, each once, in the order they're started: what
    /// Bonjour found on this network (on a cable to the Mac the service itself first, then its
    /// addresses there; on Wi-Fi the addresses, then the service), then on Wi-Fi the local
    /// addresses (where it answered last first) and then the tailnet; off Wi-Fi the tailnet first.
    /// `via` says which interface Bonjour found each of `found` on.
    static func gather(preferred: String, found: [String] = [], via: [String: Via] = [:], routes: [String] = [],
                       known: [String] = [], onLAN: Bool = true) -> [Candidate] {
        var out: [Candidate] = []
        func add(_ address: String, _ source: Candidate.Source) {
            let a = address.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !a.isEmpty, !out.contains(where: { $0.address == a }) else { return }
            out.append(Candidate(address: a, source: source, via: source == .bonjour ? via[a] : nil))
        }
        for f in found { add(f, .bonjour) }
        add(preferred, .preferred)
        for r in routes { add(r, .route) }
        for k in known { add(k, .known) }
        // A stable sort: within a rank, the order above.
        return out.enumerated().sorted { (rank($0.element, onLAN), $0.offset) < (rank($1.element, onLAN), $1.offset) }.map(\.element)
    }

    private static func rank(_ c: Candidate, _ onLAN: Bool) -> Int {
        if c.source == .bonjour {
            let service = Address.isService(c.address)
            return c.wired ? (service ? 0 : 1) : (service ? 3 : 2)
        }
        let local = c.kind == .local
        return onLAN ? (local ? 4 : 5) : (local ? 5 : 4)
    }

    /// When each is dialled, from the start of the round: those leading (on Wi-Fi the local
    /// ones, off it the rest) at once, the others `headStart` later.
    static func delays(_ candidates: [Candidate], onLAN: Bool = true) -> [TimeInterval] {
        let leads = { (c: Candidate) in c.source == .bonjour || (onLAN ? c.kind == .local : c.kind != .local) }
        let anyLeads = candidates.contains(where: leads)
        return candidates.enumerated().map { i, c in leads(c) || (!anyLeads && i == 0) ? 0 : headStart }
    }

    /// `known` with `address` put first (each once, at most `limit`, none of `except`).
    static func remember(_ address: String, in known: [String], except: [String] = [], limit: Int = 6) -> [String] {
        var out = [address]
        for k in known where !out.contains(k) { out.append(k) }
        return Array(out.filter { !except.contains($0) }.prefix(limit))
    }
}

/// Why one dial didn't end with the Mac.
nonisolated enum LinkFailure: Equatable {
    /// Nothing answered in time, or the address refused (the Mac isn't there, or the network
    /// keeps the phone from it).
    case unreachable
    /// Something answered with a certificate other than the pinned one ("" when it had none).
    case mismatch(String)
    /// Something answered, but not with TLS.
    case notTLS
    /// The socket opened and then went.
    case dropped
}

/// One round of dialling every candidate at once: the first socket to open (TLS accepted the
/// pinned certificate, the WebSocket upgrade went through) is the one that authenticates, and
/// every other dial is cancelled. Only one authenticates: the Mac replaces a device's older
/// connection with its newer one, and a pairing code works once.
nonisolated struct DialRound {
    enum Dial: Equatable { case waiting, dialling, open, failed(LinkFailure), cancelled }

    private(set) var candidates: [Candidate] = []
    private(set) var dials: [String: Dial] = [:]
    /// The dial that opened first, until it fails.
    private(set) var winner: String?

    init(_ candidates: [Candidate]) {
        for c in candidates { add(c) }
    }

    /// A candidate seen mid-round (Bonjour found the Mac): false when it's already in it.
    @discardableResult
    mutating func add(_ c: Candidate) -> Bool {
        guard dials[c.address] == nil else { return false }
        candidates.append(c)
        dials[c.address] = .waiting
        return true
    }

    /// `address` is being dialled now; false when it's no longer wanted (another won).
    mutating func dialling(_ address: String) -> Bool {
        guard winner == nil, dials[address] == .waiting else { return false }
        dials[address] = .dialling
        return true
    }

    /// The socket at `address` opened. The first wins: the dials to cancel come back (those
    /// under way or not yet started). Nil when another won first: this one is to close.
    mutating func opened(_ address: String) -> [String]? {
        guard winner == nil, dials[address] == .dialling else { return nil }
        winner = address
        dials[address] = .open
        let losers = candidates.map(\.address).filter { dials[$0] == .dialling || dials[$0] == .waiting }
        for l in losers { dials[l] = .cancelled }
        return losers
    }

    mutating func failed(_ address: String, _ why: LinkFailure) {
        guard let d = dials[address], d == .dialling || d == .open || d == .waiting else { return }
        dials[address] = .failed(why)
        if winner == address { winner = nil }
    }

    /// Candidates not started yet: when everything started has already failed, they go now
    /// rather than wait out the head start.
    var waiting: [Candidate] { candidates.filter { dials[$0.address] == .waiting } }
    var anyDialling: Bool { dials.values.contains(.dialling) }

    /// Nothing won and nothing is left to wait for.
    var lost: Bool {
        winner == nil && !dials.isEmpty && !dials.values.contains { $0 == .waiting || $0 == .dialling || $0 == .open }
    }

    var failures: [String: LinkFailure] {
        dials.compactMapValues { if case .failed(let f) = $0 { f } else { nil } }
    }

    /// Every candidate answered with another certificate. With one that merely didn't answer,
    /// the one that answered wrongly may be another machine that has the Mac's old address on
    /// this network: that isn't the Mac's identity changing.
    var everyMismatched: Bool {
        let f = failures
        return lost && !f.isEmpty && f.count == candidates.count && f.values.allSatisfy { if case .mismatch = $0 { true } else { false } }
    }

    /// The fingerprint of a certificate that didn't match, when one was seen.
    var mismatchSeen: String? {
        for case .mismatch(let fp) in failures.values where !fp.isEmpty { return fp }
        return nil
    }

    /// An address that is surely the Mac's answered with another certificate: one its own
    /// Bonjour record led to, or its tailnet address (the user's own network: no other machine
    /// has it). That's the Mac's identity changing, whatever the others did.
    var ownMismatched: Bool {
        candidates.contains { c in
            guard c.source == .bonjour || c.kind == .tailnet, case .failed(.mismatch) = dials[c.address] else { return false }
            return true
        }
    }

    var anyNotTLS: Bool { failures.values.contains(.notTLS) }
}

/// The retry cadence while nothing answers: 1, 2, 4, 8 seconds, then every 10.
nonisolated struct Backoff {
    static let steps: [TimeInterval] = [1, 2, 4, 8]
    static let cap: TimeInterval = 10
    private(set) var tries = 0

    mutating func next() -> TimeInterval {
        defer { tries += 1 }
        return tries < Self.steps.count ? Self.steps[tries] : Self.cap
    }

    mutating func reset() { tries = 0 }
}

/// Keeping an open connection honest: a ping every 10 s, and the connection is taken for dead
/// after 25 s without a word from the Mac (it answers every ping). The Mac pings every 20 s and
/// drops a phone silent for 60 s; the phone's pings keep it well inside that.
nonisolated struct Heartbeat {
    static let every: TimeInterval = 10
    static let deadAfter: TimeInterval = 25
    /// How often it's checked.
    static let tick: TimeInterval = 2.5

    enum Action: Equatable { case wait, ping, drop }

    private(set) var heard: TimeInterval
    private(set) var pinged: TimeInterval

    init(now: TimeInterval) { (heard, pinged) = (now, now) }

    mutating func heard(at now: TimeInterval) { heard = now }

    mutating func check(at now: TimeInterval) -> Action {
        if now - heard > Self.deadAfter { return .drop }
        guard now - pinged >= Self.every else { return .wait }
        pinged = now
        return .ping
    }

    func silent(at now: TimeInterval) -> Bool { now - heard > Self.deadAfter }
}

/// What the phone knows while it looks for the Mac, for the words it shows.
nonisolated struct ReachFacts: Equatable {
    var mac: String
    var path: NetworkPath? = nil
    /// iOS doesn't let Trek look on the local network (Settings › Privacy › Local Network).
    var localDenied = false
    /// A Bonjour record with the paired Mac's id is on this network (the Wi-Fi, not a cable)…
    var bonjourSeen = false
    /// …and the Mac answered at an address it resolved to.
    var bonjourFound = false
    /// The record is seen on a cable to the Mac (USB, Ethernet) rather than the Wi-Fi: which.
    var cable: LinkKind? = nil
    /// Every way over that cable dialled since the search began has failed.
    var cableFailed = false
    /// Local candidates other than those on the cable: the Wi-Fi's.
    var hasLocal = false
    var hasTailnet = false
    /// Every local candidate (not on the cable) dialled since the search began has failed.
    var localFailed = false
    /// Every tailnet candidate dialled since the search began has failed.
    var tailnetFailed = false
    /// The phone's Wi-Fi is the network the Mac answered on before.
    var macsNetwork = false
    /// Seconds since the search began (the connection went, the network changed, the app came
    /// to the front, the user asked).
    var elapsed: TimeInterval = 0
}

/// The words the Threads list and Settings show while the phone looks for the Mac.
nonisolated enum ReachStatus {
    /// How long the local network gets before the phone says it may be keeping devices apart.
    static let isolationAfter: TimeInterval = 3

    struct Status: Equatable {
        var text: String
        /// Still trying something that may work (a spinner), rather than waiting to retry.
        var trying: Bool
    }

    static func connected(over kind: Address.Kind) -> String {
        switch kind {
        case .local: "Connected over Wi-Fi"
        case .tailnet: "Connected over Tailscale"
        case .other: "Connected"
        }
    }

    /// "Connected over USB", "over Ethernet", "over Wi-Fi", "over Tailscale".
    static func connected(via link: LinkKind?) -> String {
        link.map { "Connected over \($0.name)" } ?? "Connected"
    }

    /// What the connection that won runs over: the tailnet for a tailnet address; else the
    /// interface the dial ended up on (`used`, when the link knows it) or the one Bonjour saw the
    /// Mac on; else, for a local address, the phone's own network (Wi-Fi, or Ethernet when wired).
    static func link(address: String, used: Via?, seen: Via?, path: NetworkPath?) -> LinkKind? {
        switch Address.kind(address) {
        case .tailnet: return .tailscale
        case .local:
            if let v = used ?? seen { return v.kind }
            return path?.kind == .wired ? .ethernet : .wifi
        case .other:
            return (used ?? seen)?.kind
        }
    }

    static func reconnecting(_ mac: String) -> String { "Reconnecting to \(mac)…" }

    static func searching(_ f: ReachFacts) -> Status {
        var s = words(f)
        // "your Mac isn't answering…": a sentence starts with a capital.
        s.text = s.text.prefix(1).uppercased() + s.text.dropFirst()
        return s
    }

    private static func words(_ f: ReachFacts) -> Status {
        let mac = f.mac
        let tailscale = "Connecting to \(mac) over Tailscale…"
        let tailscaleDown = "Can't reach \(mac) over Tailscale. Is Tailscale on, on this iPhone and on the Mac?"
        guard let path = f.path else {
            return Status(text: "Looking for \(mac)…", trying: true)
        }
        // The Mac's record is on a cable to it (USB, Ethernet): that works whatever the Wi-Fi
        // does, and with no Wi-Fi at all.
        if let cable = f.cable, !f.cableFailed {
            let over = cable == .usb ? "the USB cable" : cable.name
            if path.local, f.localFailed {
                return Status(text: "This Wi-Fi keeps devices apart, but \(over) should work. Connecting to \(mac) over \(cable.name)…", trying: true)
            }
            return Status(text: "Found \(mac) over \(cable.name), connecting…", trying: true)
        }
        if !path.up {
            return Status(text: "No network. Trek connects to \(mac) when this iPhone is back online.", trying: false)
        }
        if !path.local {
            guard f.hasTailnet else {
                return Status(text: "Not on Wi-Fi. Join the Wi-Fi \(mac) is on, or use Tailscale on both.", trying: false)
            }
            return f.tailnetFailed ? Status(text: tailscaleDown, trying: false) : Status(text: tailscale, trying: true)
        }
        if f.localDenied {
            let fix = "Turn on Local Network for Trek in iOS Settings › Privacy & Security."
            guard f.hasTailnet else { return Status(text: "Trek can't look on this Wi-Fi. \(fix)", trying: false) }
            return f.tailnetFailed
                ? Status(text: "Trek can't look on this Wi-Fi and Tailscale didn't answer. \(fix)", trying: false)
                : Status(text: "Trek can't look on this Wi-Fi. Trying Tailscale…", trying: true)
        }
        // Paired over Tailscale alone, and nothing of the Mac on this Wi-Fi: no Wi-Fi to talk of.
        if !f.hasLocal, !f.bonjourSeen, f.hasTailnet {
            return f.tailnetFailed ? Status(text: tailscaleDown, trying: false) : Status(text: tailscale, trying: true)
        }
        if f.bonjourFound, !f.localFailed {
            return Status(text: "Found on Wi-Fi, connecting…", trying: true)
        }
        if f.elapsed < isolationAfter, !(f.localFailed && f.tailnetFailed) {
            return Status(text: f.bonjourSeen ? "Found on Wi-Fi, connecting…" : "Looking for \(mac) on this Wi-Fi…", trying: true)
        }
        let lead = if let cable = f.cable {
            "This Wi-Fi keeps devices apart, and \(mac) doesn't answer over \(cable.name) either. Is Trek on the Mac up to date?"
        } else if f.bonjourSeen {
            "\(mac) is on this Wi-Fi but doesn't answer. The network may keep devices apart."
        } else if f.macsNetwork {
            "\(mac) isn't answering on this Wi-Fi. Is it awake, with Trek open?"
        } else {
            "This Wi-Fi may not let devices see each other."
        }
        guard f.hasTailnet else {
            let fix = f.macsNetwork && !f.bonjourSeen ? "" : " Use the same Wi-Fi at home, a hotspot, or Tailscale on both."
            return Status(text: lead + fix, trying: false)
        }
        return f.tailnetFailed
            ? Status(text: lead + " Tailscale didn't answer either: is it on, on this iPhone and on the Mac?", trying: false)
            : Status(text: lead + " Trying Tailscale…", trying: true)
    }
}
