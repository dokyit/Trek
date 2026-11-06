import Foundation
import Testing
@testable import Trek

/// The words at the top of Settings: what this iPhone is connected to, and how.
struct SettingsGlanceTests {
    private func mac(_ state: ConnectionState = .connected, name: String = "Toby's MacBook Air",
                     detail: String? = "Connected over USB", encrypted: Bool = true) -> MachineGlance {
        MachineGlance(name: name, connection: state, detail: detail, encrypted: encrypted)
    }

    @Test func oneMachineIsNamed() {
        #expect(SettingsGlance.headline([mac()], demo: false) == "Connected to Toby's MacBook Air")
        #expect(SettingsGlance.headline([mac(.connecting)], demo: false) == "Connecting to Toby's MacBook Air…")
        #expect(SettingsGlance.headline([mac(.offline("Offline"))], demo: false) == "Can't reach Toby's MacBook Air")
        #expect(SettingsGlance.headline([mac(.unauthorized("Refused"))], demo: false) == "Toby's MacBook Air needs pairing again")
    }

    @Test func severalAreCounted() {
        let both = [mac(), mac(name: "T-Book", detail: "Connected over Tailscale")]
        #expect(SettingsGlance.headline(both, demo: false) == "Connected to 2 machines")
        let one = [mac(), mac(.offline("Offline"), name: "T-Book")]
        #expect(SettingsGlance.headline(one, demo: false) == "1 of 2 machines connected")
    }

    @Test func theDemoSaysSo() {
        #expect(SettingsGlance.headline([mac()], demo: true) == "Exploring the demo")
        #expect(SettingsGlance.headline([], demo: false) == "Not connected")
    }

    @Test func theWayIsTheLinkItRunsOver() {
        #expect(SettingsGlance.way(mac()) == "USB")
        #expect(SettingsGlance.way(mac(detail: "Connected over Tailscale")) == "Tailscale")
        #expect(SettingsGlance.way(mac(detail: nil)) == "Connected")
        #expect(SettingsGlance.way(mac(.connecting, detail: "Looking for it on this Wi-Fi…")) == "Connecting")
        #expect(SettingsGlance.way(mac(.identityChanged(expected: "a", seen: "b"))) == "Pair again")
    }

    @Test func unencryptedConnectionsAreCounted() {
        #expect(SettingsGlance.unencrypted([mac(), mac(name: "T-Book")]) == 0)
        #expect(SettingsGlance.unencrypted([mac(), mac(name: "T-Book", encrypted: false)]) == 1)
    }

    @Test func storageSaysNoneForNothing() {
        #expect(PhoneStorage.count(0, 0) == "None")
        #expect(PhoneStorage.count(3, 2_000_000).hasPrefix("3 · "))
    }
}

/// Settings › About › Signed until: the profile's expiry, read from inside its signed envelope.
struct AppSigningTests {
    @Test func readsTheExpiryFromAProfile() throws {
        let plist = """
        <?xml version="1.0" encoding="UTF-8"?>
        <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
        <plist version="1.0"><dict>
        <key>AppIDName</key><string>Trek</string>
        <key>ExpirationDate</key><date>2026-11-12T17:49:53Z</date>
        </dict></plist>
        """
        // The signature's bytes sit on either side of the plist in a real profile.
        var data = Data([0x30, 0x80, 0x06, 0x09, 0x2A, 0x86, 0x48])
        data.append(Data(plist.utf8))
        data.append(Data([0x00, 0x00, 0xA0, 0x82]))
        let expiry = try #require(AppSigning.expiry(inProfile: data))
        #expect(expiry == ISO8601DateFormatter().date(from: "2026-11-12T17:49:53Z"))
    }

    @Test func somethingElseHasNone() {
        #expect(AppSigning.expiry(inProfile: Data("not a profile".utf8)) == nil)
    }
}
