import Foundation

/// What the top of Settings says at a glance: what this iPhone is connected to and how. Plain
/// values, so the words can be checked without a Mac.
nonisolated struct MachineGlance: Equatable {
    var name: String
    var connection: ConnectionState
    /// What the connection is doing, in words ("Connected over USB"), when the client says.
    var detail: String?
    var encrypted: Bool
}

nonisolated enum SettingsGlance {
    /// "Connected to Toby's MacBook Air", "Connected to 2 machines", "1 of 2 machines connected",
    /// "Connecting to T-Book…", "Can't reach T-Book".
    static func headline(_ machines: [MachineGlance], demo: Bool) -> String {
        if demo { return "Exploring the demo" }
        guard let first = machines.first else { return "Not connected" }
        let connected = machines.count { $0.connection == .connected }
        if machines.count == 1 {
            switch first.connection {
            case .connected: return "Connected to \(first.name)"
            case .connecting: return "Connecting to \(first.name)…"
            case .idle, .offline: return "Can't reach \(first.name)"
            case .unauthorized, .identityChanged: return "\(first.name) needs pairing again"
            }
        }
        if connected == machines.count { return "Connected to \(machines.count) machines" }
        return "\(connected) of \(machines.count) machines connected"
    }

    /// How the connections are protected, for the line under the headline: nil when every one is
    /// encrypted and pinned, else how many aren't.
    static func unencrypted(_ machines: [MachineGlance]) -> Int {
        machines.count { !$0.encrypted }
    }

    /// The way to a machine in a word or two, for its chip: "USB", "Wi-Fi", "Tailscale" while
    /// connected (from "Connected over …"), else what's wrong.
    static func way(_ m: MachineGlance) -> String {
        switch m.connection {
        case .connected:
            if let detail = m.detail, let r = detail.range(of: "Connected over ") { return String(detail[r.upperBound...]) }
            return "Connected"
        case .connecting: return "Connecting"
        case .idle, .offline: return "Offline"
        case .unauthorized: return "Refused"
        case .identityChanged: return "Pair again"
        }
    }
}

/// What Trek keeps on this iPhone to open things at once (Settings › Storage): the last threads
/// opened (`TranscriptCache`) and files fetched from notes and threads (`NoteFileStore`). Both
/// are copies; the threads and notes themselves stay on the machines.
nonisolated enum PhoneStorage {
    struct Usage: Equatable, Sendable {
        var threads = 0
        var threadBytes: Int64 = 0
        var files = 0
        var fileBytes: Int64 = 0
        var bytes: Int64 { threadBytes + fileBytes }
    }

    /// Reads the folders off the main thread.
    @concurrent static func measure() async -> Usage {
        let t = TranscriptCache.usage()
        let f = NoteFileStore.shared.summary()
        return Usage(threads: t.threads, threadBytes: t.bytes, files: f.files, fileBytes: f.bytes)
    }

    @concurrent static func clear() async {
        TranscriptCache.removeEverything()
        NoteFileStore.shared.removeAll()
    }

    /// "12 · 1.2 MB", or "None".
    static func count(_ n: Int, _ bytes: Int64) -> String {
        n == 0 && bytes == 0 ? "None" : "\(n) · \(size(bytes))"
    }

    /// "4.2 MB", "Zero KB" as "None".
    static func size(_ bytes: Int64) -> String {
        bytes == 0 ? "None" : ByteCountFormatter.string(fromByteCount: bytes, countStyle: .file)
    }
}
