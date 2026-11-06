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
