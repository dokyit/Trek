import Foundation
import Network

/// A tiny HTTP server on the iPhone's loopback: the Mac reaches it over the USB cable through
/// usbmuxd (which connects to the phone's own 127.0.0.1), and nothing on the network can. Each
/// request is one connection; `handle` runs on the main thread, where XCUITest wants its calls.
///
/// Over Wi-Fi (no cable) usbmuxd connects to the phone's address on the network instead, so Trek
/// passes `peers`, the Mac's own addresses there: the server then listens on the local network
/// too (never cellular), and serves a connection from it only when it comes from one of those
/// addresses and carries the token.
final class Server {
    let port: UInt16
    private let token: String
    /// The Mac's addresses (their bytes) a network connection must come from; `nil`: the loopback
    /// only, as over the cable.
    private let peers: [Data]?
    private let handle: (Request) -> Response
    private var listener: NWListener?
    private let queue = DispatchQueue(label: "dev.trek.driver.server")

    init(port: UInt16, token: String, peers: [String]? = nil, handle: @escaping (Request) -> Response) {
        self.port = port
        self.token = token
        self.peers = peers.map { $0.compactMap(Server.raw) }
        self.handle = handle
    }

    /// Where it takes connections from, for its log.
    var reach: String { peers == nil ? "127.0.0.1" : "the local network" }

    func start() throws {
        let params = NWParameters.tcp
        if peers == nil {
            params.requiredInterfaceType = .loopback
        } else {
            params.prohibitedInterfaceTypes = [.cellular]
            params.acceptLocalOnly = true
        }
        params.allowLocalEndpointReuse = true
        let listener = try NWListener(using: params, on: NWEndpoint.Port(rawValue: port)!)
        listener.newConnectionHandler = { [weak self] connection in self?.accept(connection) }
        listener.stateUpdateHandler = { state in
            if case .failed(let error) = state { NSLog("TrekDriver: listener failed: \(error)") }
            if case .ready = state { NSLog("TrekDriver: listening on \(self.reach), port \(self.port)") }
        }
        listener.start(queue: queue)
        self.listener = listener
    }

    func stop() {
        listener?.cancel()
        listener = nil
    }

    private func accept(_ connection: NWConnection) {
        connection.start(queue: queue)
        read(connection, Data())
    }

    private func read(_ connection: NWConnection, _ buffer: Data) {
        connection.receive(minimumIncompleteLength: 1, maximumLength: 64 * 1024) { [weak self] data, _, done, error in
            guard let self else { return }
            var buffer = buffer
            if let data { buffer.append(data) }
            switch Request.parse(buffer) {
            case .incomplete:
                if done || error != nil { connection.cancel() } else { self.read(connection, buffer) }
            case .bad(let why):
                self.send(connection, .error(why, status: 400))
            case .done(let request, _):
                guard self.token.isEmpty || request.headers["x-trek-token"] == self.token else {
                    self.send(connection, .error("wrong token", status: 401))
                    return
                }
                if let peer = self.refused(connection) {
                    NSLog("TrekDriver: refused a connection from \(peer)")
                    self.send(connection, .error("not from the Mac that started it (\(peer))", status: 403))
                    return
                }
                DispatchQueue.main.async {
                    let response = self.handle(request)
                    self.queue.async { self.send(connection, response) }
                }
            }
        }
    }

    /// Where a connection that must not be served came from (neither the loopback nor one of
    /// `peers`); `nil` to serve it.
    private func refused(_ connection: NWConnection) -> String? {
        guard let peers else { return nil }
        guard case let .hostPort(host, _) = connection.endpoint else { return "\(connection.endpoint)" }
        var loopback = false
        var bytes: Data?
        switch host {
        case .ipv4(let a):
            (loopback, bytes) = (a.isLoopback, a.rawValue)
        case .ipv6(let a):
            if let v4 = a.asIPv4 {
                (loopback, bytes) = (v4.isLoopback, v4.rawValue)
            } else {
                (loopback, bytes) = (a.isLoopback, a.rawValue)
            }
        default:
            break
        }
        if loopback { return nil }
        if let bytes, peers.contains(bytes) { return nil }
        return "\(host)"
    }

    /// An address's bytes (IPv4 inside IPv6 as IPv4), without its interface (`%en0`).
    static func raw(_ text: String) -> Data? {
        let plain = String(text.split(separator: "%").first ?? "").trimmingCharacters(in: .whitespaces)
        if let v4 = IPv4Address(plain) { return v4.rawValue }
        if let v6 = IPv6Address(plain) { return v6.asIPv4?.rawValue ?? v6.rawValue }
        return nil
    }

    private func send(_ connection: NWConnection, _ response: Response) {
        // The answer ends the connection: as the last message, so the close follows the last
        // byte over the wire. Cancelling as soon as the bytes were taken dropped the tail of a
        // long answer (a screenshot) over Wi-Fi, where they're still in flight; the loopback
        // never showed it.
        connection.send(content: response.encoded, contentContext: .finalMessage, isComplete: true,
                        completion: .contentProcessed { _ in
            self.queue.asyncAfter(deadline: .now() + 5) { connection.cancel() }
        })
    }
}
