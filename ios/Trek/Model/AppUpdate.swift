import Foundation

/// A newer Trek on a paired Mac than this app. The app can't update itself (it's signed for this
/// iPhone, not from the App Store): the Mac installs the newer build over Wi-Fi
/// (`ios/scripts/phone-update.sh`, run by a LaunchAgent), and Settings says it's coming.
nonisolated enum AppUpdate {
    struct Available: Equatable {
        /// The Mac's name.
        var mac: String
        /// The Trek it runs ("0.4.21").
        var version: String
    }

    /// The newest Trek among the Macs this iPhone is connected to, when it's newer than this app.
    /// Windows and Linux machines don't install the iPhone app, so they don't count.
    @MainActor static func available(in links: [HostLink], app: String = ClientMessage.appVersion) -> Available? {
        links
            .filter { $0.kind == .mac || $0.kind == .unknown }
            .compactMap { link in link.host.map { Available(mac: link.name, version: $0.version) } }
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
}
