import Foundation
import Observation
import SwiftUI

/// What the phone talks to: a Mac over the network (`TrekClient`) or the in-process `MockHost`.
/// Both speak the same messages, so demo mode exercises the same handling as a real Mac.
protocol Backend: AnyObject {
    var onMessage: ((ServerMessage) -> Void)? { get set }
    var onState: ((ConnectionState) -> Void)? { get set }
    func start()
    func send(_ message: ClientMessage, id: String?)
    func stop()
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
    var mode: AppMode = .unpaired
    var connection: ConnectionState = .idle
    var host: HostInfo?
    var threads: [ThreadSummary] = []
    var projects: [ProjectSummary] = []
    var agents: [AgentOption] = []
    var transcripts: [String: [TItem]] = [:]
    var loadedTranscripts: Set<String> = []
    var toast: Toast?
    var pairing = false
    var pairingError: String?
    /// A pairing link (deep link or QR code) waiting for the user to confirm it.
    var pendingLink: PairingLink?
    /// How the live connection is carried, for the Settings indicator.
    var transport: Transport?

    var followUpMode: SendMode {
        didSet { UserDefaults.standard.set(followUpMode.rawValue, forKey: "followUpMode") }
    }

    private var backend: Backend?
    private var nextRequest = 1
    private var replies: [String: (ServerMessage) -> Void] = [:]
    private var subscribed: Set<String> = []
    private var seqs: [String: Int64] = [:]

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
        attach(TrekClient(address: paired.address, auth: .hello(deviceId: Device.id, token: paired.token), transport: transport),
               mode: .live)
        self.transport = transport
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
    func pair(address: String, code: String, transport: Transport) {
        pendingLink = nil
        pairing = true
        pairingError = nil
        let client = TrekClient(address: address, auth: .pair(code: code, deviceId: Device.id, deviceName: Device.name),
                                transport: transport)
        client.onPaired = { [weak self] token, host, kept in
            PairedMac(address: address, token: token, hostName: host.name, hostId: host.id, transport: kept).save()
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
        b.onState = { [weak self] state in
            guard let self else { return }
            self.connection = state
            if state == .connected {
                // Resubscribe after a reconnect, from where we were.
                for id in self.subscribed { self.request(.subscribe(threadId: id, afterSeq: self.seqs[id])) }
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
        case .offline(let why) where why == "Mac unreachable — retrying":
            "Couldn't reach that Mac. Check the address, and that this iPhone is on the same network or tailnet."
        case .offline(let why): why
        default: nil
        }
    }

    private func reset() {
        threads = []
        projects = []
        agents = []
        transcripts = [:]
        loadedTranscripts = []
        subscribed = []
        seqs = [:]
        replies = [:]
        host = nil
        transport = nil
        connection = .idle
    }

    // MARK: Incoming

    func handle(_ message: ServerMessage) {
        switch message {
        case .paired(_, _, let host), .welcome(_, let host):
            self.host = host
        case .snapshot(let s):
            withAnimation(.snappy) {
                threads = s.threads
                projects = s.projects
                agents = s.agents
            }
        case .thread(let t):
            withAnimation(.snappy) {
                if let i = threads.firstIndex(where: { $0.id == t.id }) { threads[i] = t } else { threads.insert(t, at: 0) }
            }
        case .threadRemoved(let id):
            threads.removeAll { $0.id == id }
        case .transcript(let re, let tid, let reset, let seq, let items):
            if reset { transcripts[tid] = items } else { for item in items { upsert(item, in: tid) } }
            seqs[tid] = max(seqs[tid] ?? 0, seq)
            loadedTranscripts.insert(tid)
            reply(re, message)
        case .item(let tid, let item):
            guard subscribed.contains(tid) else { return }
            withAnimation(.smooth(duration: 0.25)) { upsert(item, in: tid) }
            seqs[tid] = max(seqs[tid] ?? 0, item.seq)
        case .transcriptReset(let tid):
            if subscribed.contains(tid) { request(.subscribe(threadId: tid, afterSeq: nil)) }
        case .ack(let re, _), .pong(let re):
            reply(re, message)
        case .error(let re, let code, let text):
            if let re, replies[re] != nil { reply(re, message) } else if code != .unauthorized { show(text, error: true) }
        case .unknown:
            break
        }
    }

    private func upsert(_ item: TItem, in tid: String) {
        var list = transcripts[tid] ?? []
        if let i = list.firstIndex(where: { $0.id == item.id }) {
            list[i] = item
        } else {
            list.append(item)
        }
        transcripts[tid] = list
    }

    private func reply(_ re: String?, _ message: ServerMessage) {
        guard let re, let f = replies.removeValue(forKey: re) else { return }
        f(message)
    }

    // MARK: Outgoing

    @discardableResult
    private func request(_ message: ClientMessage, then: ((ServerMessage) -> Void)? = nil) -> String {
        let id = "\(nextRequest)"
        nextRequest += 1
        if let then { replies[id] = then }
        backend?.send(message, id: id)
        return id
    }

    /// Failures of an action become a toast; `ok` runs on success.
    private func act(_ message: ClientMessage, ok: ((ServerMessage) -> Void)? = nil) {
        request(message) { [weak self] reply in
            if case .error(_, _, let text) = reply { self?.show(text, error: true) } else { ok?(reply) }
        }
    }

    func subscribe(_ tid: String) {
        subscribed.insert(tid)
        request(.subscribe(threadId: tid, afterSeq: loadedTranscripts.contains(tid) ? seqs[tid] : nil))
        markSeen(tid)
    }

    func unsubscribe(_ tid: String) {
        subscribed.remove(tid)
        backend?.send(.unsubscribe(threadId: tid), id: nil)
    }

    func markSeen(_ tid: String) {
        if let i = threads.firstIndex(where: { $0.id == tid }), threads[i].unseen { threads[i].unseen = false }
        backend?.send(.markSeen(threadId: tid), id: nil)
    }

    func send(_ text: String, to tid: String, mode: SendMode?) {
        act(.send(threadId: tid, text: text, mode: mode))
    }

    func interrupt(_ tid: String) {
        act(.interrupt(threadId: tid))
    }

    func answer(_ tid: String, requestId: String, _ response: AnswerResponse) {
        act(.answer(threadId: tid, requestId: requestId, response: response))
    }

    func newThread(project: String, agent: String, model: String?, text: String, worktree: Bool,
                   opened: @escaping (String) -> Void) {
        act(.newThread(projectId: project, agent: agent, model: model, text: text, worktree: worktree)) { reply in
            if case .ack(_, let tid?) = reply { opened(tid) }
        }
    }

    func refresh() async {
        // The Mac pushes every change, so pull-to-refresh only proves the link is alive (a ping);
        // a dead link shows in the connection banner and reconnects on its own.
        await withCheckedContinuation { (c: CheckedContinuation<Void, Never>) in
            var done = false
            request(.ping) { _ in if !done { done = true; c.resume() } }
            DispatchQueue.main.asyncAfter(deadline: .now() + 2) { if !done { done = true; c.resume() } }
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

    func thread(_ id: String) -> ThreadSummary? { threads.first { $0.id == id } }

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

    var workingCount: Int { threads.filter { $0.runState == .working }.count }
    var needsYouCount: Int { threads.filter { $0.waitsOnYou && $0.section != .snoozed }.count }
}

extension ThreadSummary {
    var waitsOnYou: Bool { needs != nil || runState == .needsYou || runState == .failed }
}

// MARK: Pairing storage

struct PairedMac: Codable {
    var address: String
    var token: String
    var hostName: String
    var hostId: String
    /// Pinned TLS (the full fingerprint) or plain `ws://` (chosen by the user when pairing).
    var transport: Transport

    init(address: String, token: String, hostName: String, hostId: String, transport: Transport) {
        self.address = address
        self.token = token
        self.hostName = hostName
        self.hostId = hostId
        self.transport = transport
    }

    private enum CodingKeys: String, CodingKey { case address, token, hostName, hostId, transport }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        address = try c.decode(String.self, forKey: .address)
        token = try c.decode(String.self, forKey: .token)
        hostName = try c.decode(String.self, forKey: .hostName)
        hostId = try c.decode(String.self, forKey: .hostId)
        // Records from before TLS were paired over plain ws:// and stay that way (Settings says
        // "Unencrypted") until paired again.
        transport = try c.decodeIfPresent(Transport.self, forKey: .transport) ?? .plain
    }

    static func load() -> PairedMac? {
        guard let data = Keychain.read("paired-mac") else { return nil }
        return try? JSONDecoder().decode(PairedMac.self, from: data)
    }

    func save() {
        if let data = try? JSONEncoder().encode(self) { Keychain.write("paired-mac", data) }
    }

    static func clear() { Keychain.delete("paired-mac") }
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
