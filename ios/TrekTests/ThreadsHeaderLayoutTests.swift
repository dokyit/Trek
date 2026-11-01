import SwiftUI
import Testing
import UIKit
@testable import Trek

/// The head of the thread list: the update card and the connection banners each take their own
/// room under the title and the machine chips, never drawn over them (seen on an iPhone on iOS
/// 27.2: the card over the large title, the chips up under the status bar).
@MainActor
struct ThreadsHeaderLayoutTests {
    /// iPhone 16 / 17, 16 Plus, 17 Pro Max: the widths the header was checked at.
    static let widths: [CGFloat] = [393, 430, 440]
    static let sizes: [DynamicTypeSize] = [.large, .xxxLarge]

    private let update = AppUpdate.Available(
        mac: "Tobias’s MacBook Pro", version: "0.4.34",
        note: "iPhone tool: taps over USB through Trek Driver while the iPhone is unlocked, and the real clock over iOS's 9:41",
        hostId: "demo-mac", canUpdate: true)

    /// A Mac, and a PC that can't be reached, both with long names so the chips wrap.
    private func model() -> (AppModel, HostLink) {
        let model = AppModel()
        var mac = PairedMac.demo
        mac.hostName = "Tobias’s MacBook Pro with a long name"
        let pc = HostLink(paired: .demoPC, backend: MockHost(name: "T-Book", id: "demo-pc", platform: "windows"))
        pc.connection = .offline("")
        pc.connectionDetail = "This Wi-Fi may not let devices see each other. Tailscale didn't answer either: is it on, on this iPhone and on the Mac?"
        model.links = [HostLink(paired: mac, backend: MockHost()), pc]
        return (model, pc)
    }

    private func size<V: View>(_ view: V, width: CGFloat, model: AppModel, type: DynamicTypeSize) -> CGSize {
        let host = UIHostingController(rootView: view.environment(model).dynamicTypeSize(type))
        return host.sizeThatFits(in: CGSize(width: width, height: .greatestFiniteMagnitude))
    }

    @Test func theCardAndBannersStackUnderTheTitleAndChips() {
        for width in Self.widths {
            for type in Self.sizes {
                let (model, pc) = model()
                func header(_ update: AppUpdate.Available?, _ unreachable: [HostLink]) -> CGSize {
                    size(ThreadsHeader(picked: .constant(nil), update: update, unreachable: unreachable),
                         width: width, model: model, type: type)
                }
                let bare = header(nil, [])
                let withCard = header(update, [])
                let withBoth = header(update, [pc])
                // Each part on its own, at the width it gets in the header (16 pt each side).
                let card = size(UpdateCard(update: update).frame(width: width - 32), width: width, model: model, type: type)
                let banner = size(ConnectionBanner(state: pc.connection, detail: pc.connectionText, host: pc.name, retry: {}, pairAgain: {})
                    .frame(width: width - 32), width: width, model: model, type: type)
                let at = "\(Int(width)) pt, \(type)"
                // The card adds all of its height (and its 4 + 8 pt of room) below the chips:
                // nothing of it lies over what's above.
                #expect(abs(withCard.height - bare.height - (card.height + 12)) < 1, "the card takes its own room at \(at)")
                #expect(abs(withBoth.height - withCard.height - (banner.height + 12)) < 1, "the banner takes its own room at \(at)")
                // The title row and the chips are there before either.
                #expect(bare.height > 70, "title and chips at \(at)")
                // Nothing reaches past the screen's sides.
                #expect(withBoth.width <= width + 0.5, "fits \(at)")
            }
        }
    }

    @Test func chipsWrapRatherThanRunOffTheEdge() {
        let (model, _) = model()
        let one = size(ThreadsHeader(picked: .constant(nil), update: nil), width: 1000, model: model, type: .large)
        let narrow = size(ThreadsHeader(picked: .constant(nil), update: nil), width: 393, model: model, type: .xxxLarge)
        #expect(narrow.width <= 393.5)
        #expect(narrow.height > one.height, "a second line of chips at 393 pt and the largest text")
    }

    @Test func theBarTakesTheTitleOnceTheRowHasGoneUnderIt() {
        #expect(!ScreenTitle.isGone(scrolled: 0))
        #expect(!ScreenTitle.isGone(scrolled: -60), "pulled down to refresh")
        #expect(!ScreenTitle.isGone(scrolled: ScreenTitle.scrolledAway))
        #expect(ScreenTitle.isGone(scrolled: ScreenTitle.scrolledAway + 1))
    }

    @Test func theFadeCoversTheBarsDownToTheFootOfTheScreen() {
        let bars: CGFloat = 139  // the pill and the tab bar on a 393 pt iPhone
        let cover = UIHostingController(rootView: BottomBarsFade.cover(color: Trek.background, fade: BottomBarsFade.fade, bars: bars))
            .sizeThatFits(in: CGSize(width: 393, height: 2000))
        #expect(cover.height == BottomBarsFade.fade + bars)
    }
}
