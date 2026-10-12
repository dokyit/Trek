import Foundation
import Network

/// Finds the paired Mac on this network when the address it was paired at stops answering: the
/// router gave it another one, or it was paired on another network. Trek on the Mac announces
/// itself over Bonjour (`_trek._tcp`) with its lasting id in the record (`id=`); only a record with
/// the paired Mac's id is taken, and the connection that follows is still pinned to its
/// certificate, so a record that lies gets nowhere.
nonisolated final class MacFinder: @unchecked Sendable {
    static let service = "_trek._tcp"

    private let hostId: String
    private var browser: NWBrowser?
    private var probes: [NWConnection] = []
    private var done: (@MainActor (String?) -> Void)?
    private let queue = DispatchQueue(label: "trek.macfinder")

    init(hostId: String) {
        self.hostId = hostId
    }

    /// Looks for up to `timeout` seconds; `done` gets `host:port` (IPv4, the address the Mac
    /// listens on) or nil, once, on the main queue.
    func find(timeout: TimeInterval = 5, done: @escaping @MainActor (String?) -> Void) {
        queue.async { [self] in
            self.done = done
            let browser = NWBrowser(for: .bonjourWithTXTRecord(type: Self.service, domain: nil), using: .tcp)
            browser.browseResultsChangedHandler = { [weak self] results, _ in
                guard let self else { return }
                for result in results where Self.id(in: result) == self.hostId {
                    self.resolve(result.endpoint)
                }
            }
            browser.stateUpdateHandler = { [weak self] state in
                if case .failed = state { self?.finish(nil) }
            }
            self.browser = browser
            browser.start(queue: queue)
            queue.asyncAfter(deadline: .now() + timeout) { [weak self] in self?.finish(nil) }
        }
    }

    func cancel() {
        queue.async { [self] in
            done = nil
            finish(nil)
        }
    }

    /// The `id=` entry of a result's TXT record.
    static func id(in result: NWBrowser.Result) -> String? {
        guard case .bonjour(let txt) = result.metadata else { return nil }
        return txt["id"]
    }

    /// A service endpoint becomes an address by connecting to it: the path's remote end is the
    /// IPv4 address and port it resolved to. Nothing is sent on this connection.
    private func resolve(_ endpoint: NWEndpoint) {
        let params = NWParameters.tcp
        if let ip = params.defaultProtocolStack.internetProtocol as? NWProtocolIP.Options {
            ip.version = .v4
        }
        let probe = NWConnection(to: endpoint, using: params)
        probe.stateUpdateHandler = { [weak self, weak probe] state in
            guard let self, let probe else { return }
            switch state {
            case .ready:
                if case .hostPort(let host, let port)? = probe.currentPath?.remoteEndpoint, let address = Self.address(host, port) {
                    self.finish(address)
                }
                probe.cancel()
            case .failed, .waiting:
                probe.cancel()
            default:
                break
            }
        }
        probes.append(probe)
        probe.start(queue: queue)
    }

    static func address(_ host: NWEndpoint.Host, _ port: NWEndpoint.Port) -> String? {
        switch host {
        case .ipv4(let ip):
            // Without the interface suffix (`%en0`) the address string can carry.
            let text = "\(ip)".split(separator: "%").first.map(String.init) ?? "\(ip)"
            return "\(text):\(port.rawValue)"
        default:
            return nil
        }
    }

    private func finish(_ address: String?) {
        browser?.cancel()
        browser = nil
        probes.forEach { $0.cancel() }
        probes = []
        guard let done else { return }
        self.done = nil
        Task { @MainActor in done(address) }
    }
}
