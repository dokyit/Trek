import Foundation

/// A newer Trek on a paired Mac than this app. The app can't update itself (it's signed for this
/// iPhone, not from the App Store): the Mac builds it and installs it over USB or Wi-Fi
/// (`ios/scripts/phone-update.sh`), every half hour by itself, or now when the phone asks
/// (`update_phone`, on a Mac that serves it).
nonisolated enum AppUpdate {
    struct Available: Equatable {
        /// The Mac's name.
        var mac: String
        /// The Trek it offers ("0.4.21"): a release it found, else the one it runs.
        var version: String
        /// The first line of that release's notes, when the Mac has them.
        var note: String? = nil
        /// The Mac's host id, to ask it.
        var hostId: String = ""
        /// It builds and installs on request (`update_phone`); else it does by itself, in time.
        var canUpdate: Bool = false
    }

    /// The newest Trek among the Macs this iPhone is connected to, when it's newer than this app.
    /// Windows and Linux machines don't install the iPhone app, so they don't count.
    @MainActor static func available(in links: [HostLink], app: String = ClientMessage.appVersion) -> Available? {
        newest(links
            .filter { $0.kind == .mac || $0.kind == .unknown }
            .compactMap { link in
                offered(host: link.host, release: link.release).map {
                    Available(mac: link.name, version: $0.version, note: $0.note, hostId: link.id,
                              canUpdate: link.serves(.updatePhone))
                }
            }, app: app)
    }

    /// What a Mac offers: the release its snapshot names (newer than itself when its updater found
    /// one it hasn't installed), else, from a Mac before that, the version it runs, with no note.
    static func offered(host: HostInfo?, release: TrekRelease?) -> TrekRelease? {
        let running = host.map { TrekRelease(version: $0.version) }
        guard let release, !release.version.isEmpty else { return running }
        if let running, isNewer(running.version, than: release.version) { return running }
        return release
    }

    /// The newest of `offers` that's newer than `app`.
    static func newest(_ offers: [Available], app: String) -> Available? {
        offers
            .filter { isNewer($0.version, than: app) }
            .max { isNewer($1.version, than: $0.version) }
    }

    /// Versions as Trek numbers them: "0.4.21" is newer than "0.4.20"; a pre-release of a version
    /// ("0.4.21-nightly.20261027") is newer than the one before it and older than its release.
    /// Build metadata ("+…") doesn't count.
    static func isNewer(_ a: String, than b: String) -> Bool {
        let (ca, pa) = parts(a), (cb, pb) = parts(b)
        for i in 0..<max(ca.count, cb.count) {
            let x = i < ca.count ? ca[i] : 0, y = i < cb.count ? cb[i] : 0
            if x != y { return x > y }
        }
        switch (pa, pb) {
        case (nil, nil): return false
        case (nil, _): return true
        case (_, nil): return false
        case let (x?, y?): return x.compare(y, options: .numeric) == .orderedDescending
        }
    }

    private static func parts(_ v: String) -> ([Int], String?) {
        let v = v.trimmingCharacters(in: .whitespaces).drop { $0 == "v" }.split(separator: "+", maxSplits: 1).first.map(String.init) ?? ""
        let pieces = v.split(separator: "-", maxSplits: 1)
        let core = (pieces.first ?? "").split(separator: ".").map { Int($0) ?? 0 }
        return (core, pieces.count > 1 ? String(pieces[1]) : nil)
    }

    /// "Later" on an offer: that version stays out of sight until the app is launched again or a
    /// day has passed, whichever comes first. A newer version is offered at once.
    struct Dismissal: Codable, Equatable {
        var version: String
        var at: Date
        /// The launch it was dismissed in (`Self.launch`).
        var launch: String

        static let lasts: TimeInterval = 24 * 3600
        /// This run of the app; a new one each launch.
        static let launch = UUID().uuidString
        private static let key = "updateDismissal"

        func hides(_ version: String, now: Date = Date(), launch: String = Self.launch) -> Bool {
            self.version == version && self.launch == launch && now.timeIntervalSince(at) < Self.lasts
        }

        static func load(_ defaults: UserDefaults = .standard) -> Dismissal? {
            defaults.data(forKey: key).flatMap { try? JSONDecoder().decode(Dismissal.self, from: $0) }
        }

        func save(_ defaults: UserDefaults = .standard) {
            if let data = try? JSONEncoder().encode(self) { defaults.set(data, forKey: Self.key) }
        }
    }
}

/// The newest Trek a Mac knows of (the snapshot's `release`).
nonisolated struct TrekRelease: Codable, Equatable, Sendable {
    var version: String
    /// The first line of its release notes.
    var note: String? = nil
}

/// How an iPhone update the Mac runs is going (`phone_update`, to every phone).
nonisolated struct PhoneUpdate: Decodable, Equatable, Sendable {
    nonisolated enum State: String, LenientEnum {
        /// The phone asked; the Mac hasn't said it started yet. Never sent by the Mac.
        case asking
        case building, installing, installed, failed
        case unknown
        static var fallback: State { .unknown }
    }

    var state: State
    var version: String? = nil
    /// Why it failed, in the Mac's words; otherwise a detail.
    var message: String? = nil

    /// What the phone says about it.
    var text: String {
        switch state {
        case .asking: "Asking your Mac…"
        case .building: "Building on your Mac…"
        case .installing: "Installing… Trek will close and open again"
        case .installed: "Installed: Trek will restart"
        case .failed: message.flatMap { $0.isEmpty ? nil : $0 } ?? "The update didn't go through."
        case .unknown: "Updating…"
        }
    }

    /// Still going: nothing to press but wait.
    var running: Bool { [.asking, .building, .installing].contains(state) }
}
