import Foundation
import Network

/// A tiny HTTP server on the iPhone's loopback: the Mac reaches it over the USB cable through
/// usbmuxd (which connects to the phone's own 127.0.0.1), and nothing on the network can. Each
/// request is one connection; `handle` runs on the main thread, where XCUITest wants its calls.
final class Server {
    let port: UInt16
    private let token: String
    private let handle: (Request) -> Response
    private var listener: NWListener?
    private let queue = DispatchQueue(label: "dev.trek.driver.server")

    init(port: UInt16, token: String, handle: @escaping (Request) -> Response) {
        self.port = port
        self.token = token
        self.handle = handle
    }

    func start() throws {
        let params = NWParameters.tcp
        params.requiredInterfaceType = .loopback
        params.allowLocalEndpointReuse = true
        let listener = try NWListener(using: params, on: NWEndpoint.Port(rawValue: port)!)
        listener.newConnectionHandler = { [weak self] connection in self?.accept(connection) }
        listener.stateUpdateHandler = { state in
            if case .failed(let error) = state { NSLog("TrekDriver: listener failed: \(error)") }
            if case .ready = state { NSLog("TrekDriver: listening on 127.0.0.1:\(self.port)") }
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
                DispatchQueue.main.async {
                    let response = self.handle(request)
                    self.queue.async { self.send(connection, response) }
                }
            }
        }
    }

    private func send(_ connection: NWConnection, _ response: Response) {
        connection.send(content: response.encoded, completion: .contentProcessed { _ in
            connection.cancel()
        })
    }
}
