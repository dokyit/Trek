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
        let cover = UIHostingController(rootView: BottomBarsFade.cover(color: Trek.background, fade: BottomBarsFade.fade,
                                                                       bars: bars + BottomBarsFade.lift))
            .sizeThatFits(in: CGSize(width: 393, height: 2000))
        #expect(cover.height == BottomBarsFade.fade + BottomBarsFade.lift + bars)
    }

    /// The fade ends on solid ground a little above the pill, not at its edge (on iOS 27.2 a
    /// section header read through the foot of the fade, just over the pill), and the last row
    /// scrolls clear of all of it.
    @Test func theBottomFadeIsSolidAboveThePill() {
        #expect(BottomBarsFade.lift >= 8, "solid ground above the pill's top edge")
        #expect(BottomBarsFade.fade >= 28)
        #expect(BottomBarsFade.clearance == BottomBarsFade.fade + BottomBarsFade.lift)
    }

    // MARK: The bar at the top

    /// iPhone 16 on iOS 27.0: a 59 pt status bar and the bar's row under it (113 pt in all); an
    /// iPhone 14-sized one on 27.2: 47 and 101.
    static let bars: [(status: CGFloat, bar: CGFloat)] = [(59, 113), (47, 101), (62, 116)]

    @Test func theBarHasNoGroundAtRestAndASolidOneOnceRowsAreUnderIt() {
        #expect(TopBar.groundOpacity(scrolled: 0) == 0, "at rest: nothing under the bar")
        #expect(TopBar.groundOpacity(scrolled: -80) == 0, "pulled down: the refresh spinner shows")
        #expect(TopBar.groundOpacity(scrolled: TopBar.fade) == 1)
        #expect(TopBar.groundOpacity(scrolled: 600) == 1)
        var last = -1.0
        for y in stride(from: CGFloat(-10), through: TopBar.fade + 10, by: 2) {
            let o = TopBar.groundOpacity(scrolled: y)
            #expect(o >= last, "never lighter as the rows go further under, at \(y)")
            last = o
        }
    }

    @Test func theSmallTitleComesOnlyOnceTheBarIsSolid() {
        // Before it, the big title would still show through a part-drawn ground beside it.
        #expect(ScreenTitle.scrolledAway >= TopBar.fade)
        for y in stride(from: CGFloat(-60), through: 300, by: 1) where ScreenTitle.isGone(scrolled: y) {
            #expect(TopBar.groundOpacity(scrolled: y) == 1, "small title over a solid bar at \(y)")
        }
    }

    @Test func theSmallTitleSitsHalfwayDownTheBarsRow() {
        for (status, bar) in Self.bars {
            let centre = TopBar.titleCentre(statusBar: status, bar: bar)
            #expect(centre > status && centre < bar, "inside the bar's row at \(status)/\(bar)")
            #expect(abs((centre - status) - (bar - centre)) < 0.5, "centred at \(status)/\(bar)")
        }
        // Before the geometry is known it stays at the top rather than going negative.
        #expect(TopBar.titleCentre(statusBar: 59, bar: 0) == 59)
    }

    @Test func theBarsGroundReachesTheBarsFootThenFades() {
        for (_, bar) in Self.bars {
            let mask = UIHostingController(rootView: TopBar.mask(bar: bar, fade: TopBar.fade))
                .sizeThatFits(in: CGSize(width: 393, height: 2000))
            #expect(mask.height == bar + TopBar.fade, "solid to \(bar) pt, then \(TopBar.fade) pt of fade")
        }
    }

    /// A list whose head rests at the bar's foot, scrolled `d` (negative: pulled down), as the
    /// tracker sees it: the head's place while its row is laid out, and the scroll view's offset
    /// with the inset it reports (iOS 27.0's, or none, as iOS 27.2's read as).
    private func tracker(bar: CGFloat, scrolled d: CGFloat, headShown: Bool = true, inset: CGFloat) -> HeadTracker {
        var t = HeadTracker()
        t.barFoot = bar
        t.inset = inset
        // At rest first: the row seen where it lies.
        t.offset = -bar
        t.head = ListHead(top: bar, shown: true)
        t.settle()
        t.offset = -bar + d
        t.head = ListHead(top: bar - d, shown: headShown)
        t.settle()
        return t
    }

    /// Nothing of the head (the big title, the chips) is ever drawn in the bar region uncovered:
    /// from the first point it goes above the bar's foot the ground there is solid. On an
    /// iPhone on iOS 27.2 the title and the chips sat under the clock and the bar's buttons, one
    /// swipe in, with no ground.
    @Test func theHeadIsNeverUncoveredInTheBarRegion() {
        for (_, bar) in Self.bars {
            for inset in [bar, 0] {
                var d: CGFloat = 0.02
                while d < 900 {
                    let t = tracker(bar: bar, scrolled: d, headShown: d < 400, inset: inset)
                    #expect(abs(t.scrolled - d) < 0.001, "tracked at \(d) (bar \(bar), inset \(inset))")
                    #expect(TopBar.groundOpacity(scrolled: t.scrolled) == 1, "solid at \(d)")
                    #expect(TopBar.uncovered(scrolled: t.scrolled, bar: bar) == 0, "nothing uncovered at \(d)")
                    d += d < 60 ? 0.25 : 7
                }
                // At rest and pulled down: no ground, so the refresh spinner shows.
                for d: CGFloat in [0, -0.000_000_1, -20, -120] {
                    let t = tracker(bar: bar, scrolled: d, inset: inset)
                    #expect(TopBar.groundOpacity(scrolled: t.scrolled) == 0, "no ground at \(d)")
                    #expect(TopBar.fadeOpacity(scrolled: t.scrolled) == 0)
                    #expect(!ScreenTitle.isGone(scrolled: t.scrolled))
                }
            }
        }
        // The rounding seen at rest on a simulator.
        #expect(TopBar.groundOpacity(scrolled: 2.842170943040401e-14) == 0)
    }

    /// Once the List drops the head's row, the offset carries on from where the two met, not
    /// from the head's last place (a List stops placing a row before it drops it).
    @Test func theHeadIsTrackedFromTheOffsetOnceItsRowIsGone() {
        var t = tracker(bar: 113, scrolled: 450, inset: 113)
        // The row's last place, then the list scrolls on and drops it.
        t.offset += 1400
        t.head.shown = false
        t.settle()
        #expect(abs(t.scrolled - 1850) < 0.001)
        // Back to the top: the row is laid out again where it rests.
        t.offset = -113
        t.head = ListHead(top: 113, shown: true)
        t.settle()
        #expect(abs(t.scrolled) < 0.001)
        // Before the head is ever seen: the scroll view's own numbers.
        var fresh = HeadTracker()
        fresh.offset = -100
        fresh.inset = 113
        #expect(fresh.scrolled == 13)
    }

    /// The bottom fade follows the bars: its solid ground starts `lift` above the bars' top edge
    /// whether they stand expanded (the pill over the tab bar, the safe area ending at the pill)
    /// or folded (a small tab button, the pill and Search on one row, lower), wherever the safe
    /// area ends meanwhile. Screens at 393 and 430 pt.
    @Test func theBottomFadeTracksTheBarsInBothStates() {
        struct Case { var name: String; var screen: CGFloat; var foot: CGFloat; var barsTop: CGFloat? }
        var cases: [Case] = []
        for screen: CGFloat in [852, 932] {  // 393 and 430 pt
            let expanded = screen - 139, folded = screen - 76
            cases += [
                Case(name: "expanded", screen: screen, foot: expanded, barsTop: expanded),
                // Folded, as iOS 27.0 has it: the safe area follows, nearly.
                Case(name: "folded, safe area followed", screen: screen, foot: screen - 83, barsTop: folded),
                // Folded, the safe area left where it was.
                Case(name: "folded, safe area kept", screen: screen, foot: expanded, barsTop: folded),
                // Folded, the safe area shrunk past the pill (rows read under and around it).
                Case(name: "folded, safe area shrunk", screen: screen, foot: screen - 34, barsTop: folded),
            ]
        }
        for c in cases {
            let bars = c.screen - c.foot
            let solid = BottomBarsFade.solidTop(listFoot: c.foot, bars: bars, barsTop: c.barsTop)
            #expect(solid == c.barsTop! - BottomBarsFade.lift, "\(c.name) at \(c.screen)")
            // The cover as drawn: hung from the screen's foot, its top the fade's.
            let rise = BottomBarsFade.rise(listFoot: c.foot, bars: bars, barsTop: c.barsTop)
            let cover = UIHostingController(rootView: BottomBarsFade.cover(color: Trek.background, fade: BottomBarsFade.fade,
                                                                           bars: max(0, bars + rise) + BottomBarsFade.lift))
                .sizeThatFits(in: CGSize(width: 393, height: 2000))
            #expect(abs((c.screen - cover.height) - (solid - BottomBarsFade.fade)) < 0.5, "cover from \(c.name) at \(c.screen)")
            // The last row still scrolls clear of the fade above the bars.
            #expect(c.foot - (BottomBarsFade.clearance + max(0, rise)) <= solid - BottomBarsFade.fade + 0.5, "\(c.name)")
        }
        // No pill (a Mac that refused this iPhone), the keyboard up, or a pill measured before
        // it was placed: the safe area's edge.
        for (foot, bars, top) in [(CGFloat(713), CGFloat(139), CGFloat?.none), (516, 336, 713), (713, 139, 0), (713, 139, 20)] {
            #expect(BottomBarsFade.solidTop(listFoot: foot, bars: bars, barsTop: top) == foot - BottomBarsFade.lift)
        }
    }
}
