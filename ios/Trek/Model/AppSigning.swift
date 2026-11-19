import Foundation

/// When this app's signing lapses. A free Apple ID signs it for 7 days from when Xcode made the
/// profile; past that iOS won't open it until a Mac installs it again (`ios/scripts/phone-update.sh`
/// does when two days are left). Settings › About says when, so a lapse never comes as a surprise.
nonisolated enum AppSigning {
    /// Nil in the simulator and in a build without a profile (App Store, TestFlight): nothing lapses.
    static let expiry: Date? = {
        guard let url = Bundle.main.url(forResource: "embedded", withExtension: "mobileprovision"),
              let data = try? Data(contentsOf: url) else { return nil }
        return expiry(inProfile: data)
    }()

    /// Under this long left, About says so in amber: the Mac's updater should have renewed it by now.
    static let soon: TimeInterval = 2 * 86_400

    /// The profile's `ExpirationDate`. A profile is a signed envelope with the plist inside it as
    /// plain XML, so the plist is read from between its own markers.
    static func expiry(inProfile data: Data) -> Date? {
        guard let start = data.range(of: Data("<?xml".utf8)),
              let end = data.range(of: Data("</plist>".utf8), in: start.lowerBound..<data.endIndex) else { return nil }
        let plist = data.subdata(in: start.lowerBound..<end.upperBound)
        let dict = (try? PropertyListSerialization.propertyList(from: plist, format: nil)) as? [String: Any]
        return dict?["ExpirationDate"] as? Date
    }
}

/// How this build reached the phone. A development build is signed for this iPhone by Xcode and
/// kept current by the paired Mac (`AppUpdate`, `ios/scripts/phone-update.sh`); a build from the
/// App Store or TestFlight is updated by the store, and the Mac's update flow must not exist in it.
nonisolated enum Distribution {
    enum Channel: Equatable, Sendable { case development, store }

    /// Set by the `TREK_APP_STORE` compilation condition, which `ios/AppStore.xcconfig` adds and
    /// `ios/scripts/testflight.sh` builds with.
    static let compiledForStore: Bool = {
        #if TREK_APP_STORE
        true
        #else
        false
        #endif
    }()

    /// The flag decides. Without it, a build on a phone with no provisioning profile inside it
    /// can only have come from the store (which strips the profile; Xcode's and ad hoc installs
    /// keep theirs), so an archive made without the flag is still treated as the store's.
    static func channel(compiledForStore: Bool, simulator: Bool, hasProfile: Bool) -> Channel {
        if compiledForStore { return .store }
        if simulator { return .development }
        return hasProfile ? .development : .store
    }

    static let channel: Channel = {
        #if targetEnvironment(simulator)
        let simulator = true
        #else
        let simulator = false
        #endif
        return channel(compiledForStore: compiledForStore, simulator: simulator,
                       hasProfile: Bundle.main.url(forResource: "embedded", withExtension: "mobileprovision") != nil)
    }()

    /// Whether a paired Mac builds and installs this app's updates.
    static var updatesFromMac: Bool { channel == .development }
}
