import Foundation

/// The live connection to Trek on a Mac: a WebSocket speaking protocol v1 (docs/MOBILE.md).
///
/// It authenticates first (`pair` with a one-time code, or `hello` with this device's token),
/// then streams `ServerMessage`s to the model. Dropped connections retry with backoff; a refused
/// token or code stops it (the user has to pair again).
final class TrekClient: Backend {
    enum Auth {
        case pair(code: String, deviceId: String, deviceName: String)
        case hello(deviceId: String, token: String)
    }

    var onMessage: ((ServerMessage) -> Void)?
    var onState: ((ConnectionState) -> Void)?
    /// Called once when pairing succeeds, with the device token to keep.
    var onPaired: ((String, HostInfo) -> Void)?

    private let address: String
    private var auth: Auth
    private var task: URLSessionWebSocketTask?
    private var session = URLSession(configuration: .default)
    private var stopped = false
    private var authenticated = false
    private var attempt = 0
    private var pingTimer: Timer?
    private var generation = 0

    init(address: String, auth: Auth) {
        self.address = address
        self.auth = auth
    }

    static func url(for address: String) -> URL? {
        let trimmed = address.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.hasPrefix("ws://") || trimmed.hasPrefix("wss://") { return URL(string: trimmed) }
        return URL(string: "ws://\(trimmed)/")
    }

    func start() {
        stopped = false
        connect()
    }

    func stop() {
        stopped = true
        pingTimer?.invalidate()
        task?.cancel(with: .goingAway, reason: nil)
        task = nil
    }

    func send(_ message: ClientMessage, id: String?) {
        guard let task, authenticated || isAuthMessage(message) else { return }
        guard let data = try? message.encoded(id: id), let text = String(data: data, encoding: .utf8) else { return }
        task.send(.string(text)) { _ in }
    }

    private func isAuthMessage(_ m: ClientMessage) -> Bool {
        switch m {
        case .pair, .hello: true
        default: false
        }
    }

    private func connect() {
        guard !stopped, let url = Self.url(for: address) else {
            onState?(.offline("Bad address"))
            return
        }
        generation += 1
        let gen = generation
        authenticated = false
        onState?(.connecting)
        var request = URLRequest(url: url)
        request.timeoutInterval = 10
        let task = session.webSocketTask(with: request)
        task.maximumMessageSize = 4 << 20
        self.task = task
        task.resume()
        let first: ClientMessage = switch auth {
        case .pair(let code, let deviceId, let deviceName): .pair(code: code, deviceId: deviceId, deviceName: deviceName)
        case .hello(let deviceId, let token): .hello(deviceId: deviceId, token: token)
        }
        send(first, id: "auth")
        Task { await receive(task, gen: gen) }
    }

    private func receive(_ task: URLSessionWebSocketTask, gen: Int) async {
        while gen == generation {
            do {
                let frame = try await task.receive()
                let data: Data
                switch frame {
                case .string(let s): data = Data(s.utf8)
                case .data(let d): data = d
                @unknown default: continue
                }
                guard let message = try? ServerMessage.decode(data) else { continue }
                handle(message)
            } catch {
                if gen == generation { dropped(error) }
                return
            }
        }
    }

    private func handle(_ message: ServerMessage) {
        switch message {
        case .paired(_, let token, let host):
            authenticated = true
            attempt = 0
            auth = .hello(deviceId: Device.id, token: token)
            onPaired?(token, host)
            onState?(.connected)
            startPings()
        case .welcome:
            authenticated = true
            attempt = 0
            onState?(.connected)
            startPings()
        case .error(_, let code, let text) where !authenticated:
            switch code {
            case .unauthorized:
                stopped = true
                onState?(.unauthorized("This iPhone isn't paired with that Mac any more. Pair again."))
            case .pairingFailed:
                stopped = true
                onState?(.unauthorized(text.isEmpty ? "That code didn't work. Codes last 10 minutes." : text))
            case .unsupportedProtocol:
                stopped = true
                onState?(.unauthorized("Update Trek on your Mac or this app: they speak different versions."))
            default:
                onState?(.offline(text))
            }
        default:
            break
        }
        onMessage?(message)
    }

    private func dropped(_ error: Error) {
        pingTimer?.invalidate()
        authenticated = false
        guard !stopped else { return }
        attempt += 1
        let delay = min(30, pow(2, Double(min(attempt, 5))) / 2)
        onState?(.offline("Mac unreachable — retrying"))
        DispatchQueue.main.asyncAfter(deadline: .now() + delay) { [weak self] in
            guard let self, !self.stopped else { return }
            self.connect()
        }
    }

    private func startPings() {
        pingTimer?.invalidate()
        pingTimer = Timer.scheduledTimer(withTimeInterval: 25, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.send(.ping, id: nil) }
        }
    }
}

/// A `trek://pair?host=…&code=…&name=…` link from the Mac's QR code.
struct PairingLink: Equatable {
    var address: String
    var code: String
    var name: String?

    init?(_ string: String) {
        guard let comps = URLComponents(string: string.trimmingCharacters(in: .whitespacesAndNewlines)),
              comps.scheme == "trek", comps.host == "pair" || comps.path.contains("pair") else { return nil }
        let q = Dictionary((comps.queryItems ?? []).map { ($0.name, $0.value ?? "") }, uniquingKeysWith: { a, _ in a })
        guard let host = q["host"], !host.isEmpty, let code = q["code"], !code.isEmpty else { return nil }
        address = host
        self.code = code
        name = q["name"]
    }
}
