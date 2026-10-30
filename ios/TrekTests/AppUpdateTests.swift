import Foundation
import Testing
@testable import Trek

struct UpdateLaterTests {
    private let at = Date(timeIntervalSince1970: 1_791_000_000)
    private var later: AppUpdate.Dismissal { AppUpdate.Dismissal(version: "0.4.28", at: at, launch: "L1") }

    @Test func hidesThatVersionForTheRestOfTheLaunch() {
        #expect(later.hides("0.4.28", now: at, launch: "L1"))
        #expect(later.hides("0.4.28", now: at.addingTimeInterval(23 * 3600), launch: "L1"))
    }

    @Test func aDayLaterItShowsAgain() {
        #expect(!later.hides("0.4.28", now: at.addingTimeInterval(24 * 3600), launch: "L1"))
    }

    @Test func theNextLaunchShowsItAgain() {
        #expect(!later.hides("0.4.28", now: at.addingTimeInterval(60), launch: "L2"))
    }

    @Test func aNewerVersionShowsAtOnce() {
        #expect(!later.hides("0.4.29", now: at.addingTimeInterval(60), launch: "L1"))
    }

    @Test func itIsKeptAcrossReads() throws {
        let defaults = try #require(UserDefaults(suiteName: "trek-tests-\(UUID().uuidString)"))
        #expect(AppUpdate.Dismissal.load(defaults) == nil)
        later.save(defaults)
        #expect(AppUpdate.Dismissal.load(defaults) == later)
    }
}

struct UpdateOfferTests {
    private let mac = HostInfo(id: "h", name: "Studio", version: "0.4.27")

    @Test func theReleaseTheMacFoundWinsOverTheOneItRuns() {
        let offer = AppUpdate.offered(host: mac, release: TrekRelease(version: "0.4.28", note: "Update now from the phone"))
        #expect(offer == TrekRelease(version: "0.4.28", note: "Update now from the phone"))
    }

    @Test func aMacBeforeReleasesOffersWhatItRuns() {
        #expect(AppUpdate.offered(host: mac, release: nil) == TrekRelease(version: "0.4.27"))
        #expect(AppUpdate.offered(host: nil, release: nil) == nil)
    }

    @Test func neverOlderThanTheMacItself() {
        #expect(AppUpdate.offered(host: mac, release: TrekRelease(version: "0.4.26", note: "old")) == TrekRelease(version: "0.4.27"))
        #expect(AppUpdate.offered(host: mac, release: TrekRelease(version: "")) == TrekRelease(version: "0.4.27"))
    }

    @Test func theNewestOfferNewerThanTheAppIsShown() {
        let offers = [AppUpdate.Available(mac: "A", version: "0.4.27"), AppUpdate.Available(mac: "B", version: "0.4.28"),
                      AppUpdate.Available(mac: "C", version: "0.4.26")]
        #expect(AppUpdate.newest(offers, app: "0.4.26")?.mac == "B")
        #expect(AppUpdate.newest(offers, app: "0.4.28") == nil)
        #expect(AppUpdate.newest([], app: "0.4.26") == nil)
    }
}

struct PhoneUpdateWireTests {
    private func decode(_ json: String) throws -> ServerMessage {
        try ServerMessage.decode(Data(json.utf8))
    }

    @Test func progressPushesDecode() throws {
        guard case .phoneUpdate(let u) = try decode(#"{"type":"phone_update","state":"building","version":"0.4.28"}"#) else {
            Issue.record("not a phone_update"); return
        }
        #expect(u == PhoneUpdate(state: .building, version: "0.4.28"))
        #expect(u.running)
        guard case .phoneUpdate(let failed) = try decode(#"{"type":"phone_update","state":"failed","message":"Your Mac couldn't reach the iPhone"}"#) else {
            Issue.record("not a phone_update"); return
        }
        #expect(failed.text == "Your Mac couldn't reach the iPhone")
        #expect(!failed.running)
    }

    @Test func aStateFromANewerMacIsHarmless() throws {
        guard case .phoneUpdate(let u) = try decode(#"{"type":"phone_update","state":"signing"}"#) else {
            Issue.record("not a phone_update"); return
        }
        #expect(u.state == .unknown)
    }

    @Test func theSnapshotCarriesTheReleaseAndOlderOnesStillRead() throws {
        guard case .snapshot(let s) = try decode(#"{"type":"snapshot","threads":[],"projects":[],"agents":[],"release":{"version":"0.4.28","note":"Faster"}}"#) else {
            Issue.record("not a snapshot"); return
        }
        #expect(s.release == TrekRelease(version: "0.4.28", note: "Faster"))
        guard case .snapshot(let old) = try decode(#"{"type":"snapshot","threads":[],"projects":[],"agents":[]}"#) else {
            Issue.record("not a snapshot"); return
        }
        #expect(old.release == nil)
    }

    @Test func theRequestNamesTheVersion() {
        #expect(ClientMessage.updatePhone(version: "0.4.28").json(id: "7") as NSDictionary == ["type": "update_phone", "version": "0.4.28", "id": "7"] as NSDictionary)
        #expect(ClientMessage.updatePhone(version: nil).json(id: nil) as NSDictionary == ["type": "update_phone"] as NSDictionary)
    }
}
