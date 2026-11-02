import SwiftUI

/// The foot of a tab's list: rows fade out above the "New thread" pill and the tab bar and stay
/// hidden under their glass (as the transcript does under the composer), and the last row scrolls
/// clear of the fade.
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
    /// The bottom safe area: the tab bar and the pill over it (or the keyboard). The list's own
    /// frame ends where it begins.
    @State private var bars: CGFloat = 0

    func body(content: Content) -> some View {
        content
            .contentMargins(.bottom, Self.clearance, for: .scrollContent)
            // The bars' own scroll edge effect is a blur that rows still read through; the fade
            // below covers that ground.
            .scrollEdgeEffectHidden(true, for: .bottom)
            .onGeometryChange(for: CGFloat.self) { $0.safeAreaInsets.bottom } action: { bars = $0 }
            .overlay(alignment: .bottom) {
                // Hung from the list's foot (the bars' top edge) down to the screen's. An overlay
                // on the list covers everything in it, the pinned section headers too.
                Self.cover(color: color, fade: Self.fade, bars: bars + Self.lift)
                    .offset(y: bars)
                    .allowsHitTesting(false)
                    .accessibilityHidden(true)
            }
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

/// The head of a list that draws its own title (see `ScreenTitle`): once rows go under the
/// navigation bar the bar region turns solid, its ground the screen's own, with a fade below it,
/// so nothing scrolled away (the big title, the chips, a banner) shows behind the bar's buttons;
/// and the bar shows a small title once the big one has gone under it. Drawn here from the
/// list's scroll offset rather than left to the bar's large-title collapse and scroll edge
/// effect, which iOS 27.2 changed under us (the scrolled-away header showed through the glass
/// and no small title came).
struct TopBarFade<Ground: View>: ViewModifier {
    var title: String
    /// The screen's top inset outside the navigation stack: the status bar. The bar's own row
    /// runs from here down to the list's top inset.
    var statusBar: CGFloat
    /// What the bar region is filled with: the screen's own background, so its edge doesn't show.
    var ground: Ground

    /// How far the list has scrolled from rest (negative while pulled down to refresh).
    @State private var scrolled: CGFloat = 0
    /// The list's top safe area: the status bar and the navigation bar. Its frame starts there.
    @State private var top: CGFloat = 0

    func body(content: Content) -> some View {
        let shown = ScreenTitle.isGone(scrolled: scrolled)
        content
            .scrollEdgeEffectHidden(true, for: .top)
            .onScrollGeometryChange(for: CGFloat.self) { $0.contentOffset.y + $0.contentInsets.top } action: { _, offset in
                scrolled = offset
            }
            .onGeometryChange(for: CGFloat.self) { $0.safeAreaInsets.top } action: { top = $0 }
            .overlay {
                ZStack(alignment: .top) {
                    ground
                        .mask(alignment: .top) { TopBar.mask(bar: top, fade: TopBar.fade) }
                        .opacity(TopBar.groundOpacity(scrolled: scrolled))
                        .accessibilityHidden(true)
                    Text(title)
                        .font(.ui(.headline))
                        .foregroundStyle(Trek.foreground)
                        .lineLimit(1)
                        .padding(.horizontal, 110)  // clear of the bar's buttons
                        .frame(height: 0)
                        .padding(.top, TopBar.titleCentre(statusBar: statusBar, bar: top))
                        .opacity(shown ? 1 : 0)
                        .animation(.easeOut(duration: 0.15), value: shown)
                        .accessibilityAddTraits(.isHeader)
                        .accessibilityHidden(!shown)
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
                .ignoresSafeArea()
                .allowsHitTesting(false)
            }
    }
}

/// The measures of `TopBarFade`, apart for the tests.
enum TopBar {
    /// How tall the fade under the bar is, and how far the rows scroll before the bar is solid.
    static let fade: CGFloat = 18

    /// Solid down to the bar's foot (`bar`, from the top of the screen), then the fade.
    static func mask(bar: CGFloat, fade: CGFloat) -> some View {
        VStack(spacing: 0) {
            Color.black.frame(height: bar)
            LinearGradient(colors: [.black, .black.opacity(0)], startPoint: .top, endPoint: .bottom)
                .frame(height: fade)
        }
    }

    /// The bar region's ground: none at rest or while pulled down (the refresh spinner shows),
    /// then fully there once the rows have gone `fade` under the bar.
    static func groundOpacity(scrolled: CGFloat) -> Double {
        Double(min(1, max(0, scrolled / fade)))
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

    /// The bar region turns solid over the rows, with a fade under it, and shows `title` once the
    /// list's own has gone under it; see `TopBarFade`.
    func fadesUnderTopBar<Ground: View>(title: String, statusBar: CGFloat, ground: Ground) -> some View {
        modifier(TopBarFade(title: title, statusBar: statusBar, ground: ground))
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
    /// bar's small title takes over. Never before the ground is solid: the two titles would show
    /// at once, the big one ghosted through the bar.
    static let scrolledAway: CGFloat = 36

    /// Whether a list scrolled `offset` (its content offset, the top inset added back) has taken
    /// the title row under the bar.
    static func isGone(scrolled offset: CGFloat) -> Bool { offset > scrolledAway }
}
