import Foundation
import Network
import dnssd

/// What the Bonjour browser tells the client.
enum FinderEvent: Equatable {
    /// The browse is up (Local Network is allowed).
    case browsing
    /// A record carrying the paired Mac's id is on this network (it may yet not answer).
    case seen
    /// A way to the Mac its record led to, on the interface it was seen on: a `host:port` it
    /// resolved to there (a link-local one scoped to it, `[fe80::1%en7]:7420`), or the service
    /// itself on it (`Address.service`), for Network.framework to dial.
    case found(String, via: Via? = nil)
    /// iOS doesn't let Trek look on the local network (Local Network is off for it).
    case denied
}

/// Looks for the paired Mac on this network, for as long as the phone isn't connected.
protocol MacBrowsing: AnyObject {
    var onEvent: ((FinderEvent) -> Void)? { get set }
    func start()
    /// Resolve what's been seen again (a new round: the Mac's address may have changed).
    func resolveAgain()
    func cancel()
}

/// Says when the phone's network changes (Wi-Fi joined, another interface, back online).
protocol PathWatching: AnyObject {
    var onChange: ((NetworkPath) -> Void)? { get set }
    func start()
    func cancel()
}

/// Finds the paired Mac on this network, wherever it was paired: Trek on the Mac announces
/// itself over Bonjour (`_trek._tcp`) with its lasting id in the record (`id=`); only records
/// with the paired Mac's id are taken, and the connection that follows is still pinned to its
/// certificate, so a record that lies gets nowhere. The browse stays open while the phone looks,
/// so a Mac that appears is dialled the moment it's seen.
final class MacFinder: MacBrowsing {
    static let service = "_trek._tcp"
    /// How long a resolve may take before it's given up (the next round tries again).
    static let resolveTimeout: TimeInterval = 4

    var onEvent: ((FinderEvent) -> Void)?
    private let hostId: String
    private var browser: NWBrowser?
    /// The paired Mac's service as seen on this network, and the interfaces it's seen on.
    private var seenOn: [NWEndpoint: [NWInterface]] = [:]
    private var resolves: [ServiceResolve] = []
    private let queue = DispatchQueue(label: "trek.macfinder")

    init(hostId: String) {
        self.hostId = hostId
    }

    func start() {
        guard browser == nil else { return }
        let params = NWParameters()
        params.includePeerToPeer = false
        let browser = NWBrowser(for: .bonjourWithTXTRecord(type: Self.service, domain: nil), using: params)
        let hostId = hostId
        browser.browseResultsChangedHandler = { [weak self] results, _ in
            let mine = results.filter { Self.id(in: $0) == hostId }.map { ($0.endpoint, $0.interfaces) }
            SeenInterfaces.note(mine.flatMap(\.1))
            Task { @MainActor in self?.seen(mine) }
        }
        browser.stateUpdateHandler = { [weak self] state in
            switch state {
            case .ready:
                Task { @MainActor in self?.onEvent?(.browsing) }
            case .waiting(let error), .failed(let error):
                let denied = Self.isDenied(error)
                Task { @MainActor in
                    if denied { self?.onEvent?(.denied) }
                    // Failed for good: the next search starts a new one.
                    if case .failed = state { self?.cancel() }
                }
            default:
                break
            }
        }
        self.browser = browser
        browser.start(queue: queue)
    }

    func resolveAgain() {
        for (e, interfaces) in seenOn {
            offer(e, on: interfaces)
            resolve(e, on: interfaces)
        }
    }

    func cancel() {
        browser?.cancel()
        browser = nil
        seenOn = [:]
        resolves.forEach { $0.cancel() }
        resolves = []
    }

    /// The `id=` entry of a result's TXT record.
    nonisolated static func id(in result: NWBrowser.Result) -> String? {
        guard case .bonjour(let txt) = result.metadata else { return nil }
        return txt["id"]
    }

    /// Bonjour refused for want of the Local Network permission (`kDNSServiceErr_PolicyDenied`).
    nonisolated static func isDenied(_ error: NWError) -> Bool {
        if case .dns(let code) = error { return code == -65570 }
        return false
    }

    private func seen(_ mine: [(NWEndpoint, [NWInterface])]) {
        guard browser != nil else { return }
        let new = mine.filter { e, interfaces in seenOn[e].map(\.count) != interfaces.count }
        seenOn = Dictionary(mine, uniquingKeysWith: { a, _ in a })
        // The service on the interfaces it's on first, so the words know a cable from the Wi-Fi.
        for (e, interfaces) in new { offer(e, on: interfaces) }
        if !mine.isEmpty { onEvent?(.seen) }
        for (e, interfaces) in new { resolve(e, on: interfaces) }
    }

    /// The service itself, on each interface it was seen on, is a way to the Mac too: dialled as
    /// a service, Network.framework resolves it there and scopes a link-local address itself
    /// (the USB cable to the Mac has nothing else).
    private func offer(_ endpoint: NWEndpoint, on interfaces: [NWInterface]) {
        guard case .service(let name, let type, let domain, _) = endpoint else { return }
        let real = interfaces.filter { $0.type != .loopback }
        if real.isEmpty {
            onEvent?(.found(Address.service(name: name, type: type, domain: domain.isEmpty ? "local." : domain, interface: nil)))
        }
        for i in real {
            onEvent?(.found(Address.service(name: name, type: type, domain: domain.isEmpty ? "local." : domain, interface: i.name),
                            via: Self.via(i)))
        }
    }

    /// What kind of link an interface is (Wi-Fi, the cable to the Mac, Ethernet…).
    nonisolated static func via(_ i: NWInterface) -> Via? {
        let type: Via.InterfaceType = switch i.type {
        case .wifi: .wifi
        case .cellular: .cellular
        case .wiredEthernet: .wiredEthernet
        case .loopback: .loopback
        default: .other
        }
        return Via.kind(name: i.name, type: type, routableIPv4: routableIPv4(on: i.name)).map { Via(name: i.name, kind: $0) }
    }

    /// The interface named `name` has an IPv4 address of its own (not a self-assigned 169.254 one).
    nonisolated static func routableIPv4(on name: String) -> Bool {
        var list: UnsafeMutablePointer<ifaddrs>?
        guard getifaddrs(&list) == 0, let first = list else { return false }
        defer { freeifaddrs(list) }
        for ifa in sequence(first: first, next: { $0.pointee.ifa_next }) {
            let i = ifa.pointee
            guard String(cString: i.ifa_name) == name, let addr = i.ifa_addr, addr.pointee.sa_family == UInt8(AF_INET) else { continue }
            let a = addr.withMemoryRebound(to: sockaddr_in.self, capacity: 1) { UInt32(bigEndian: $0.pointee.sin_addr.s_addr) }
            if a >> 16 != 0xA9FE, a != 0 { return true }
        }
        return false
    }

    /// A service becomes addresses on each interface it was seen on: its SRV record names a host
    /// and port, the host's A and AAAA records on that interface the addresses (dns_sd, without
    /// connecting: whether they answer is for the dials to find out).
    private func resolve(_ endpoint: NWEndpoint, on interfaces: [NWInterface]) {
        guard case .service(let name, let type, let domain, _) = endpoint else { return }
        resolves.removeAll(where: \.finished)
        let real = interfaces.filter { $0.type != .loopback }
        let on: [NWInterface?] = real.isEmpty ? [nil] : real
        for i in on {
            let via = i.flatMap(Self.via)
            let r = ServiceResolve(queue: queue, interfaceName: i?.name) { [weak self] address in
                Task { @MainActor in self?.found(address, via) }
            }
            resolves.append(r)
            r.start(name: name, type: type, domain: domain.isEmpty ? "local." : domain, interface: i.map { UInt32($0.index) } ?? 0,
                    timeout: Self.resolveTimeout)
        }
    }

    private func found(_ address: String, _ via: Via?) {
        guard browser != nil else { return }
        onEvent?(.found(address, via: via))
    }
}

/// The interfaces Bonjour saw the Mac on, by name, for dialling there: Network.framework doesn't
/// list every one (the links to a computer over USB among them) for `fe80::1%en7` to find, and
/// an `NWInterface` can't be made from a name.
nonisolated enum SeenInterfaces {
    private static let lock = NSLock()
    nonisolated(unsafe) private static var byName: [String: NWInterface] = [:]

    static func note(_ interfaces: [NWInterface]) {
        lock.withLock { for i in interfaces { byName[i.name] = i } }
    }

    static func named(_ name: String) -> NWInterface? {
        lock.withLock { byName[name] }
    }
}

/// One Bonjour service resolved on one interface to the addresses it answers at:
/// `DNSServiceResolve` for its host and port, then `DNSServiceGetAddrInfo` for the host's IPv4
/// and IPv6 addresses. Everything runs on `queue`, and stops after `timeout`.
nonisolated final class ServiceResolve: @unchecked Sendable {
    private let queue: DispatchQueue
    /// The interface resolved on (`en7`), which a link-local address is scoped to.
    private let interfaceName: String?
    private let found: @Sendable (String) -> Void
    private var refs: [DNSServiceRef] = []
    private var hosts: Set<String> = []
    private var port: UInt16 = 0
    private var stopped = false
    private let lock = NSLock()
    private var _finished = false

    init(queue: DispatchQueue, interfaceName: String? = nil, found: @escaping @Sendable (String) -> Void) {
        self.queue = queue
        self.interfaceName = interfaceName
        self.found = found
    }

    var finished: Bool { lock.withLock { _finished } }

    func start(name: String, type: String, domain: String, interface: UInt32, timeout: TimeInterval) {
        queue.async { [self] in
            var ref: DNSServiceRef?
            let context = Unmanaged.passUnretained(self).toOpaque()
            let err = DNSServiceResolve(&ref, 0, interface, name, type, domain, { _, _, index, err, _, host, port, _, _, context in
                guard err == kDNSServiceErr_NoError, let host, let context else { return }
                Unmanaged<ServiceResolve>.fromOpaque(context).takeUnretainedValue()
                    .resolved(host: String(cString: host), port: UInt16(bigEndian: port), interface: index)
            }, context)
            guard err == kDNSServiceErr_NoError, let ref else { return stop() }
            add(ref)
            // Holds this object until it stops: the callbacks' context isn't retained.
            queue.asyncAfter(deadline: .now() + timeout) { [self] in stop() }
        }
    }

    func cancel() {
        queue.async { [self] in stop() }
    }

    private func add(_ ref: DNSServiceRef) {
        guard DNSServiceSetDispatchQueue(ref, queue) == kDNSServiceErr_NoError else {
            DNSServiceRefDeallocate(ref)
            return
        }
        refs.append(ref)
    }

    private func resolved(host: String, port: UInt16, interface: UInt32) {
        guard !stopped, !hosts.contains(host) else { return }
        hosts.insert(host)
        self.port = port
        var ref: DNSServiceRef?
        let context = Unmanaged.passUnretained(self).toOpaque()
        let err = DNSServiceGetAddrInfo(&ref, 0, interface, DNSServiceProtocol(kDNSServiceProtocol_IPv4 | kDNSServiceProtocol_IPv6), host, { _, flags, index, err, _, addr, _, context in
            guard err == kDNSServiceErr_NoError, flags & kDNSServiceFlagsAdd != 0, let addr, let context else { return }
            Unmanaged<ServiceResolve>.fromOpaque(context).takeUnretainedValue().address(addr, interface: index)
        }, context)
        if err == kDNSServiceErr_NoError, let ref { add(ref) }
    }

    /// An address the host has, on the interface numbered `index` (what it answered on, when
    /// the resolve wasn't on one: a 169.254 address carries no scope of its own).
    private func address(_ sa: UnsafePointer<sockaddr>, interface index: UInt32) {
        guard !stopped, let host = Self.numeric(sa, interface: interfaceName ?? Self.name(of: index)) else { return }
        found(Address.join(host, port))
    }

    /// The name of the interface numbered `index` (`en7`); nil for 0 (any).
    static func name(of index: UInt32) -> String? {
        guard index != 0 else { return nil }
        var buf = [CChar](repeating: 0, count: Int(IF_NAMESIZE) + 1)
        guard if_indextoname(index, &buf) != nil else { return nil }
        return String(decoding: buf.prefix { $0 != 0 }.map { UInt8(bitPattern: $0) }, as: UTF8.self)
    }

    /// The address as text; nil for loopback. A link-local one (`fe80::/10`, `169.254/16`) keeps
    /// the interface it's on (`fe80::1%en7`): it means nothing without it, and it's how the USB
    /// cable to the Mac is reached. Any other loses its suffix.
    static func numeric(_ sa: UnsafePointer<sockaddr>, interface: String? = nil) -> String? {
        var buf = [CChar](repeating: 0, count: Int(NI_MAXHOST))
        guard getnameinfo(sa, socklen_t(sa.pointee.sa_len), &buf, socklen_t(buf.count), nil, 0, NI_NUMERICHOST) == 0 else { return nil }
        let full = String(decoding: buf.prefix { $0 != 0 }.map { UInt8(bitPattern: $0) }, as: UTF8.self)
        return scoped(full, interface: interface)
    }

    /// `numeric`'s text decision, apart from the socket address.
    nonisolated static func scoped(_ full: String, interface: String?) -> String? {
        let (text, scope) = Address.unscoped(full)
        let lower = text.lowercased()
        if text.isEmpty || lower.hasPrefix("127.") || lower == "::1" { return nil }
        let linkLocal = lower.hasPrefix("fe8") || lower.hasPrefix("fe9") || lower.hasPrefix("fea") || lower.hasPrefix("feb")
            || lower.hasPrefix("169.254.")
        guard linkLocal else { return text }
        guard let name = scope ?? interface else { return nil }
        return "\(text)%\(name)"
    }

    private func stop() {
        guard !stopped else { return }
        stopped = true
        refs.forEach { DNSServiceRefDeallocate($0) }
        refs = []
        lock.withLock { _finished = true }
    }
}

/// The phone's network as `NWPathMonitor` sees it: Wi-Fi or not, and which Wi-Fi (its subnet).
final class PathWatcher: PathWatching {
    var onChange: ((NetworkPath) -> Void)?
    private var monitor: NWPathMonitor?

    func start() {
        guard monitor == nil else { return }
        let monitor = NWPathMonitor()
        monitor.pathUpdateHandler = { [weak self] path in
            let described = Self.describe(path)
            Task { @MainActor in self?.onChange?(described) }
        }
        self.monitor = monitor
        monitor.start(queue: DispatchQueue(label: "trek.path"))
    }

    func cancel() {
        monitor?.cancel()
        monitor = nil
    }

    nonisolated static func describe(_ path: NWPath) -> NetworkPath {
        guard path.status == .satisfied else { return NetworkPath(kind: .none) }
        let kind: NetworkPath.Kind = if path.usesInterfaceType(.wifi) { .wifi }
            else if path.usesInterfaceType(.wiredEthernet) { .wired }
            else if path.usesInterfaceType(.cellular) { .cellular }
            else { .other }
        let wifi = path.availableInterfaces.first { $0.type == .wifi || $0.type == .wiredEthernet }?.name
        // AirDrop's interfaces come and go all the time: not a change of network.
        let interfaces = path.availableInterfaces.map(\.name).filter { name in !fleeting.contains { name.hasPrefix($0) } }
        return NetworkPath(kind: kind, interfaces: interfaces.sorted(), subnet: wifi.flatMap(subnet(of:)))
    }

    nonisolated static let fleeting = ["awdl", "llw", "anpi"]

    /// The IPv4 address and mask of the interface named `name` (`en0`).
    nonisolated static func subnet(of name: String) -> Subnet? {
        var list: UnsafeMutablePointer<ifaddrs>?
        guard getifaddrs(&list) == 0, let first = list else { return nil }
        defer { freeifaddrs(list) }
        for ifa in sequence(first: first, next: { $0.pointee.ifa_next }) {
            let i = ifa.pointee
            guard String(cString: i.ifa_name) == name, let addr = i.ifa_addr, let mask = i.ifa_netmask,
                  addr.pointee.sa_family == UInt8(AF_INET) else { continue }
            let a = addr.withMemoryRebound(to: sockaddr_in.self, capacity: 1) { UInt32(bigEndian: $0.pointee.sin_addr.s_addr) }
            let m = mask.withMemoryRebound(to: sockaddr_in.self, capacity: 1) { UInt32(bigEndian: $0.pointee.sin_addr.s_addr) }
            return m == 0 ? nil : Subnet(address: a, mask: m)
        }
        return nil
    }
}
