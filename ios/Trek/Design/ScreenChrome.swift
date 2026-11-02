import SwiftUI

/// Where the bars at the foot of the screen begin, from the top of the window: the top edge of
/// the "New thread" pill, measured where it is drawn. On iOS 26 and later the tab bar folds into a
/// small pill of its own beside a floating "New thread" while a list scrolls down, lower on the
/// screen than the full bar, and the bottom safe area needn't follow it (on an iPhone on iOS 27.2
/// rows read under and around the folded bars), so the fade hangs from this edge where there is
/// one, and from the safe area where there isn't.
@MainActor @Observable
final class BottomBars {
    static let shared = BottomBars()
    /// The pill's top edge in the window, while it is on screen.
    var accessoryTop: CGFloat?
}

/// The foot of a tab's list: rows fade out above the "New thread" pill and the tab bar and stay
/// hidden under their glass (as the transcript does under the composer), and the last row scrolls
/// clear of the fade. The fade follows the bars as they fold and unfold (`BottomBars`).
struct BottomBarsFade: ViewModifier {
    /// The screen's own background, which the rows fade into.
    var color: Color = Trek.background
    /// How tall the fade is.
    static let fade: CGFloat = 32
    /// The solid ground the fade ends on, above the bars' top edge: nothing readable, a section
    /// header held at the foot included, sits right over the pill (on an iPhone on iOS 27.2 a
    /// "Direct" header still read through the foot of a fade that ended at the pill).
    static let lift: CGFloat = 10
    /// How much room the last row gets below it, to scroll clear of the fade.
    static var clearance: CGFloat { fade + lift }
    /// The most the bars' top edge is believed to sit above the list's foot: a pill measured
    /// higher than any bar is tall (or before it was laid out) is not believed.
    static let maxRise: CGFloat = 160
    /// The most it is believed to sit below the list's foot: folded, the bars drop by less than
    /// this (63 pt on an iPhone 16 while the safe area catches up); a pill further down is
    /// behind something else that ends the list, the keyboard, and the fade stays above that.
    static let maxDrop: CGFloat = 100

    /// The bottom safe area: the tab bar and the pill over it (or the keyboard). The list's own
    /// frame ends where it begins.
    @State private var bars: CGFloat = 0
    /// The list's foot, from the top of the window.
    @State private var foot: CGFloat = 0

    func body(content: Content) -> some View {
        let rise = Self.rise(listFoot: foot, bars: bars, barsTop: BottomBars.shared.accessoryTop)
        content
            .contentMargins(.bottom, Self.clearance + max(0, rise), for: .scrollContent)
            // The bars' own scroll edge effect is a blur that rows still read through; the fade
            // below covers that ground.
            .scrollEdgeEffectHidden(true, for: .bottom)
            .onGeometryChange(for: CGFloat.self) { $0.safeAreaInsets.bottom } action: { bars = $0 }
            .onGeometryChange(for: CGFloat.self) { $0.frame(in: .global).maxY } action: { foot = $0 }
            .overlay(alignment: .bottom) {
                // Hung from the bars' top edge down to the screen's foot. An overlay on the list
                // covers everything in it, the pinned section headers too.
                Self.cover(color: color, fade: Self.fade, bars: max(0, bars + rise) + Self.lift)
                    .offset(y: bars)
                    .animation(.smooth(duration: 0.25), value: rise)
                    .allowsHitTesting(false)
                    .accessibilityHidden(true)
            }
    }

    /// How far above the list's foot (`listFoot`, from the top of the window, with `bars` of safe
    /// area below it) the bars' top edge is: none without a measured one, negative where the bars
    /// have folded down below the safe area's edge, positive where they stand above it.
    static func rise(listFoot: CGFloat, bars: CGFloat, barsTop: CGFloat?) -> CGFloat {
        guard let top = barsTop, listFoot > 0, top > listFoot - maxRise,
              top < listFoot + min(bars, maxDrop) else { return 0 }
        return (listFoot - top).rounded()
    }

    /// Where the solid ground under the bars begins, from the top of the window: `lift` above
    /// their top edge (measured, or the safe area's).
    static func solidTop(listFoot: CGFloat, bars: CGFloat, barsTop: CGFloat?) -> CGFloat {
        listFoot - rise(listFoot: listFoot, bars: bars, barsTop: barsTop) - lift
    }

    /// The fade, then the solid ground from `lift` above the bars down under them.
    static func cover(color: Color, fade: CGFloat, bars: CGFloat) -> some View {
        VStack(spacing: 0) {
            // Eased: most of the way to solid by its middle, so what's in its lower half no
            // longer reads.
            LinearGradient(stops: [.init(color: color.opacity(0), location: 0),
                                   .init(color: color.opacity(0.75), location: 0.55),
                                   .init(color: color, location: 1)],
                           startPoint: .top, endPoint: .bottom)
                .frame(height: fade)
            color.frame(height: bars)
        }
    }
}

/// Where a list's head (its first row: the big title, the chips, the cards) is, for `TopBarFade`.
struct ListHead: Equatable {
    /// The head row's top edge, from the top of the window.
    var top: CGFloat?
    /// Whether the row is laid out (a List drops it once it is well off screen).
    var shown = false
}

/// How far a list's head has gone above the bar's foot, from where it is drawn: its top edge
/// against the bar's foot while the row is on screen, and from the list's scroll offset once it
/// isn't, counted from the offset at which the two edges met. Nothing relies on how the scroll
/// view reports its insets: on an iPhone on iOS 27.2 the offset with the top inset added back
/// didn't read as scrolled while only the head had gone up, and the title and the chips were
/// drawn over the status bar with no ground under the bar.
struct HeadTracker: Equatable {
    /// The scroll view's content offset, as it reports it.
    var offset: CGFloat = 0
    /// Its top inset, as it reports it: used only before the head has been seen.
    var inset: CGFloat = 0
    /// The bar's foot, from the top of the window: where the list's frame begins.
    var barFoot: CGFloat = 0
    var head = ListHead()
    /// The content offset at which the head's top edge was at the bar's foot.
    private(set) var rest: CGFloat?

    /// How far the head's top edge is above the bar's foot (negative while pulled down).
    var scrolled: CGFloat {
        if head.shown, let top = head.top, barFoot > 0 { return barFoot - top }
        if let rest { return offset - rest }
        return offset + inset
    }

    /// Ties the offset to the head's place, for when the row is gone. Called as the head moves,
    /// not as the offset does: a List stops placing a row well off screen before it drops it, and
    /// that last place taken with a later offset tied them hundreds of points wrong.
    mutating func settle() {
        if head.shown, let top = head.top, barFoot > 0 { rest = offset - (barFoot - top) }
    }
}

/// The head of a list that draws its own title (see `ScreenTitle`): as soon as the head's top
/// edge goes above the bar's foot the bar region is solid, its ground the screen's own, with a
/// fade below it that comes in as the head goes further, so nothing scrolled away (the big title,
/// the chips, a banner) is ever drawn in the bar region, over the status bar or behind the bar's
/// buttons; and the bar shows a small title once the big one has gone under it. Drawn here from
/// where the head is (`HeadTracker`) rather than left to the bar's large-title collapse and
/// scroll edge effect, which iOS 27.2 changed under us.
struct TopBarFade<Ground: View>: ViewModifier {
    var title: String
    /// The status bar's foot, from the top of the window. The bar's own row runs from here down
    /// to the list's top edge.
    var statusBar: CGFloat
    /// Where the list's head is.
    var head: ListHead
    /// What the bar region is filled with: the screen's own background, so its edge doesn't show.
    var ground: Ground

    @State private var tracker = HeadTracker()
    /// The overlay's top, from the top of the window (the screen's, as it ignores the safe area).
    @State private var origin: CGFloat = 0

    func body(content: Content) -> some View {
        let scrolled = tracker.scrolled
        let shown = ScreenTitle.isGone(scrolled: scrolled)
        let bar = max(0, tracker.barFoot - origin)
        content
            .scrollEdgeEffectHidden(true, for: .top)
            .onScrollGeometryChange(for: [CGFloat].self) { [$0.contentOffset.y, $0.contentInsets.top] } action: { _, g in
                tracker.offset = g[0]
                tracker.inset = g[1]
            }
            .onGeometryChange(for: CGFloat.self) { $0.frame(in: .global).minY } action: {
                tracker.barFoot = $0
                tracker.settle()
            }
            .onChange(of: head, initial: true) {
                tracker.head = head
                tracker.settle()
            }
            .overlay {
                ZStack(alignment: .top) {
                    ground
                        .mask(alignment: .top) {
                            TopBar.mask(bar: bar, fade: TopBar.fade, fadeOpacity: TopBar.fadeOpacity(scrolled: scrolled))
                        }
                        .opacity(TopBar.groundOpacity(scrolled: scrolled))
                        .accessibilityHidden(true)
                    Text(title)
                        .font(.ui(.headline))
                        .foregroundStyle(Trek.foreground)
                        .lineLimit(1)
                        .padding(.horizontal, 110)  // clear of the bar's buttons
                        .frame(height: 0)
                        .padding(.top, TopBar.titleCentre(statusBar: max(0, statusBar - origin), bar: bar))
                        .opacity(shown ? 1 : 0)
                        .animation(.easeOut(duration: 0.15), value: shown)
                        .accessibilityAddTraits(.isHeader)
                        .accessibilityHidden(!shown)
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
                // Measured inside the safe area it ignores: outside, it reads as the list's frame.
                .onGeometryChange(for: CGFloat.self) { $0.frame(in: .global).minY } action: { origin = $0 }
                .ignoresSafeArea()
                .allowsHitTesting(false)
            }
    }
}

/// The measures of `TopBarFade`, apart for the tests.
enum TopBar {
    /// How tall the fade under the bar is, and how far the head goes under before it is all there.
    static let fade: CGFloat = 18
    /// Less than this above the bar's foot is the head at rest: the rounding in its place (seen
    /// as 3e-14), far under a pixel.
    static let tolerance: CGFloat = 0.01

    /// Solid down to the bar's foot (`bar`, from the top of the screen), then the fade, as strong
    /// as `fadeOpacity`.
    static func mask(bar: CGFloat, fade: CGFloat, fadeOpacity: Double = 1) -> some View {
        VStack(spacing: 0) {
            Color.black.frame(height: bar)
            LinearGradient(colors: [.black, .black.opacity(0)], startPoint: .top, endPoint: .bottom)
                .frame(height: fade)
                .opacity(fadeOpacity)
        }
    }

    /// The bar region's ground: none at rest or while pulled down (the refresh spinner shows),
    /// and solid as soon as the head's top edge is above the bar's foot, so no part of the head
    /// is drawn in the bar region. It is the screen's own ground, so it comes in unseen.
    static func groundOpacity(scrolled: CGFloat) -> Double {
        scrolled > tolerance ? 1 : 0
    }

    /// The fade under the bar comes in over the first `fade` points the head goes under, so its
    /// edge doesn't jump onto the title.
    static func fadeOpacity(scrolled: CGFloat) -> Double {
        Double(min(1, max(0, scrolled / fade)))
    }

    /// How much of the head is drawn in the bar region uncovered, once its top edge has gone
    /// `scrolled` above the bar's foot (`bar` from the top of the screen): what's above the foot,
    /// less what the ground hides. The rule is none, at every offset.
    static func uncovered(scrolled: CGFloat, bar: CGFloat) -> CGFloat {
        min(max(0, scrolled), bar) * CGFloat(1 - groundOpacity(scrolled: scrolled))
    }

    /// Where the bar's small title is centred, from the top of the screen: halfway down the
    /// bar's row, as its buttons are.
    static func titleCentre(statusBar: CGFloat, bar: CGFloat) -> CGFloat {
        statusBar + max(0, bar - statusBar) / 2
    }
}

extension View {
    /// Rows fade out under the tab bar and the "New thread" pill; see `BottomBarsFade`.
    func fadesUnderBottomBars(_ color: Color = Trek.background) -> some View {
        modifier(BottomBarsFade(color: color))
    }

    /// The bar region turns solid once the list's head goes under it, with a fade below, and
    /// shows `title` once the list's own has gone; see `TopBarFade`.
    func fadesUnderTopBar<Ground: View>(title: String, statusBar: CGFloat, head: ListHead, ground: Ground) -> some View {
        modifier(TopBarFade(title: title, statusBar: statusBar, head: head, ground: ground))
    }

    /// Reports this view's top edge in the window, and whether it is laid out, as a list's head
    /// (`ListHead`), for `fadesUnderTopBar`.
    func reportsListHead(_ head: Binding<ListHead>) -> some View {
        onGeometryChange(for: CGFloat.self) { $0.frame(in: .global).minY } action: { head.wrappedValue.top = $0 }
            .onAppear { head.wrappedValue.shown = true }
            .onDisappear { head.wrappedValue.shown = false }
    }

    /// Reports this view's top edge in the window as the bars' (`BottomBars`): the "New thread"
    /// pill, expanded or folded in beside the tab bar.
    func reportsBottomBarsTop() -> some View {
        onGeometryChange(for: CGFloat.self) { $0.frame(in: .global).minY } action: { BottomBars.shared.accessoryTop = $0 }
            .onDisappear { BottomBars.shared.accessoryTop = nil }
    }
}

/// A tab's large title drawn as the first row of its list, with whatever stacks under it (the
/// machine chips, the update card, connection banners) in the same scrolling column; the bar
/// shows a small title once this one has scrolled under it (`TopBarFade`). Threads draws its own
/// because on iOS 27.2 the bar's large title stopped following that list: the chips and the
/// cards slid up under a title that stayed put.
struct ScreenTitle: View {
    var title: String

    var body: some View {
        Text(title)
            .font(.ui(size: 34, weight: .bold, relativeTo: .largeTitle))
            .foregroundStyle(Trek.foreground)
            .lineLimit(1)
            .minimumScaleFactor(0.7)
            .accessibilityAddTraits(.isHeader)
            .frame(maxWidth: .infinity, alignment: .leading)
    }

    /// Past this much scrolling the title row has gone under the bar's solid ground, and the
    /// bar's small title takes over.
    static let scrolledAway: CGFloat = 36

    /// Whether a list whose head has gone `offset` above the bar's foot (`HeadTracker`) has
    /// taken the title row under the bar.
    static func isGone(scrolled offset: CGFloat) -> Bool { offset > scrolledAway }
}
