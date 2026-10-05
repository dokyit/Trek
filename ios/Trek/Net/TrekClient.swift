import CryptoKit
import Foundation
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

/// Checks the Mac's certificate against the pin during the TLS handshake. There's no CA here:
/// the certificate is self-signed, so its fingerprint is the whole of the trust decision. It runs
/// before any byte of ours goes out, so a wrong server never sees the pairing code or the token.
nonisolated final class PinningDelegate: NSObject, URLSessionDelegate, @unchecked Sendable {
    private let lock = NSLock()
    private let pin: CertificatePin
    private var _seen: String?
    private var _rejected: String?

    init(pin: CertificatePin) { self.pin = pin }

    /// The fingerprint of the last certificate accepted.
    var seen: String? { lock.withLock { _seen } }
    /// The fingerprint of a certificate refused for not matching the pin (nil if none was).
    var rejected: String? { lock.withLock { _rejected } }

    func urlSession(_ session: URLSession, didReceive challenge: URLAuthenticationChallenge,
                    completionHandler: @escaping (URLSession.AuthChallengeDisposition, URLCredential?) -> Void) {
        guard challenge.protectionSpace.authenticationMethod == NSURLAuthenticationMethodServerTrust,
              let trust = challenge.protectionSpace.serverTrust else {
            completionHandler(.performDefaultHandling, nil)
            return
        }
        guard let leaf = (SecTrustCopyCertificateChain(trust) as? [SecCertificate])?.first else {
            lock.withLock { _rejected = "" }
            completionHandler(.cancelAuthenticationChallenge, nil)
            return
        }
        let fp = Fingerprint.of(SecCertificateCopyData(leaf) as Data)
        if pin.matches(fp) {
            lock.withLock { _seen = fp; _rejected = nil }
            completionHandler(.useCredential, URLCredential(trust: trust))
        } else {
            lock.withLock { _rejected = fp }
            completionHandler(.cancelAuthenticationChallenge, nil)
        }
    }
}

/// The live connection to Trek on a Mac: a WebSocket speaking protocol v1 (docs/MOBILE.md), over
/// TLS pinned to the Mac's certificate unless the user chose an unencrypted connection.
///
/// It authenticates first (`pair` with a one-time code, or `hello` with this device's token),
/// then streams `ServerMessage`s to the model. Dropped connections retry with backoff; a refused
/// token or code, or a certificate that doesn't match the pin, stops it.
final class TrekClient: Backend {
    enum Auth {
        case pair(code: String, deviceId: String, deviceName: String)
        case hello(deviceId: String, token: String)
    }

    var onMessage: ((ServerMessage) -> Void)?
    var onState: ((ConnectionState) -> Void)?
    /// Called once when pairing succeeds, with the device token to keep and the transport to keep
    /// using (for TLS, pinned to the full fingerprint seen while pairing).
    var onPaired: ((String, HostInfo, Transport) -> Void)?

    private let address: String
    private var auth: Auth
    private(set) var transport: Transport
    private var task: URLSessionWebSocketTask?
    private var session: URLSession?
    private var pinning: PinningDelegate?
    private var stopped = false
    private var authenticated = false
    private var attempt = 0
    private var pingTimer: Timer?
    /// When the Mac was last heard from (it answers every ping): silence for two pings means
    /// the connection is dead in the Mac's direction, though writes to it still go through.
    private var heard = Date()
    private static let pingEvery: TimeInterval = 25
    private var generation = 0

    init(address: String, auth: Auth, transport: Transport) {
        self.address = address
        self.auth = auth
        self.transport = transport
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

    func start() {
        stopped = false
        connect()
    }

    func stop() {
        stopped = true
        generation += 1
        pingTimer?.invalidate()
        task?.cancel(with: .goingAway, reason: nil)
        task = nil
        session?.invalidateAndCancel()
        session = nil
    }

    @discardableResult
    func send(_ message: ClientMessage, id: String?) -> Bool {
        guard let task, authenticated || isAuthMessage(message) else { return false }
        guard let data = try? message.encoded(id: id), let text = String(data: data, encoding: .utf8) else { return false }
        task.send(.string(text)) { _ in }
        return true
    }

    private func isAuthMessage(_ m: ClientMessage) -> Bool {
        switch m {
        case .pair, .hello: true
        default: false
        }
    }

    /// A session for the current transport: TLS ones carry the pinning delegate.
    private func makeSession() -> URLSession {
        session?.invalidateAndCancel()
        let config = URLSessionConfiguration.ephemeral
        config.urlCache = nil
        config.httpCookieStorage = nil
        config.waitsForConnectivity = false
        if case .tls(let pin) = transport {
            let delegate = PinningDelegate(pin: pin)
            pinning = delegate
            return URLSession(configuration: config, delegate: delegate, delegateQueue: nil)
        }
        pinning = nil
        return URLSession(configuration: config)
    }

    private func connect() {
        guard !stopped, let url = Self.url(for: address, encrypted: transport.isEncrypted) else {
            onState?(.offline("Bad address"))
            return
        }
        generation += 1
        let gen = generation
        authenticated = false
        onState?(.connecting)
        let session = makeSession()
        self.session = session
        var request = URLRequest(url: url)
        request.timeoutInterval = 10
        let task = session.webSocketTask(with: request)
        // Above anything the Mac sends (it clips an item's text well under this): a frame over
        // the limit ends the connection, and reconnecting would only meet it again.
        task.maximumMessageSize = 16 << 20
        self.task = task
        task.resume()
        // Queued until the socket opens, which for TLS is after the pin was checked.
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
                if gen == generation { heard = Date() }
                let data: Data
                switch frame {
                case .string(let s): data = Data(s.utf8)
                case .data(let d): data = d
                @unknown default: continue
                }
                // Read off the main thread (a transcript can be large); handled back on it, in order.
                let message = await Perf.measureAsync("decode", "\(data.count / 1024) KB") { await ServerMessage.decodeInBackground(data) }
                guard gen == generation, let message else { continue }
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
            // Typed pairing pinned only 16 hex characters; from now on, the whole certificate.
            if case .tls = transport, let seen = pinning?.seen { transport = .tls(.full(seen)) }
            onPaired?(token, host, transport)
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
        if case .tls(let pin) = transport, let rejected = pinning?.rejected {
            // Not a network problem: something answered with a different certificate. Never retry
            // into it and never fall back to plain.
            stopped = true
            onState?(.identityChanged(expected: pin.short, seen: rejected.isEmpty ? nil : Fingerprint.short(rejected)))
            return
        }
        if case .tls = transport, Self.isTLSFailure(error) {
            // The Mac answered, but not with TLS (an older Trek, or TREK_REMOTE_PLAIN): don't
            // hammer it, and don't downgrade by ourselves either.
            stopped = true
            onState?(.offline("Couldn't open an encrypted connection to that Mac. Use its IP address, and check that Trek on the Mac is up to date."))
            return
        }
        attempt += 1
        let delay = min(30, pow(2, Double(min(attempt, 5))) / 2)
        onState?(.offline("Mac unreachable — retrying"))
        DispatchQueue.main.asyncAfter(deadline: .now() + delay) { [weak self] in
            guard let self, !self.stopped else { return }
            self.connect()
        }
    }

    /// A handshake that failed for a reason other than our pin (e.g. the server speaks plain HTTP).
    private static func isTLSFailure(_ error: Error) -> Bool {
        let code = (error as NSError).code
        return (error as NSError).domain == NSURLErrorDomain
            && [NSURLErrorSecureConnectionFailed, NSURLErrorCannotLoadFromNetwork].contains(code)
    }

    private func startPings() {
        pingTimer?.invalidate()
        heard = Date()
        pingTimer = Timer.scheduledTimer(withTimeInterval: Self.pingEvery, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated {
                guard let self else { return }
                if Date().timeIntervalSince(self.heard) > Self.pingEvery * 2.5 {
                    self.silent()
                } else {
                    self.send(.ping, id: nil)
                }
            }
        }
    }

    /// Nothing has come from the Mac for too long: drop this socket and connect afresh, rather
    /// than show a connection that no longer brings anything.
    private func silent() {
        generation += 1
        task?.cancel(with: .goingAway, reason: nil)
        task = nil
        dropped(URLError(.timedOut))
    }
}

/// A `trek://pair?host=…&code=…&name=…&hid=…&fp=…` link from the Mac's QR code.
struct PairingLink: Equatable, Identifiable {
    var address: String
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
