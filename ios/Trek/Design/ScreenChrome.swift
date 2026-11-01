import SwiftUI

/// The foot of a tab's list: rows fade out above the "New thread" pill and the tab bar and stay
/// hidden under their glass (as the transcript does under the composer), and the last row scrolls
/// clear of the fade.
struct BottomBarsFade: ViewModifier {
    /// The screen's own background, which the rows fade into.
    var color: Color = Trek.background
    /// How far above the bars the rows start to fade.
    static let fade: CGFloat = 28
    /// The bottom safe area: the tab bar and the pill over it (or the keyboard). The list's own
    /// frame ends where it begins.
    @State private var bars: CGFloat = 0

    func body(content: Content) -> some View {
        content
            .contentMargins(.bottom, Self.fade, for: .scrollContent)
            // The bars' own scroll edge effect is a blur that rows still read through; the fade
            // below covers that ground.
            .scrollEdgeEffectHidden(true, for: .bottom)
            .onGeometryChange(for: CGFloat.self) { $0.safeAreaInsets.bottom } action: { bars = $0 }
            .overlay(alignment: .bottom) {
                // Hung from the list's foot (the bars' top edge) down to the screen's.
                Self.cover(color: color, fade: Self.fade, bars: bars)
                    .offset(y: bars)
                    .allowsHitTesting(false)
                    .accessibilityHidden(true)
            }
    }

    /// The fade, ending where the bars begin, then the solid ground under them.
    static func cover(color: Color, fade: CGFloat, bars: CGFloat) -> some View {
        VStack(spacing: 0) {
            LinearGradient(colors: [color.opacity(0), color], startPoint: .top, endPoint: .bottom)
                .frame(height: fade)
            color.frame(height: bars)
        }
    }
}

extension View {
    /// Rows fade out under the tab bar and the "New thread" pill; see `BottomBarsFade`.
    func fadesUnderBottomBars(_ color: Color = Trek.background) -> some View {
        modifier(BottomBarsFade(color: color))
    }
}

/// A tab's large title drawn as the first row of its list, with whatever stacks under it (the
/// machine chips, the update card, connection banners) in the same scrolling column; the bar
/// keeps an inline title that shows once this one has scrolled under it. Threads draws its own
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

    /// Past this much scrolling the title row has gone under the bar, and the bar's inline title
    /// takes over.
    static let scrolledAway: CGFloat = 36

    /// Whether a list scrolled `offset` (its content offset, the top inset added back) has taken
    /// the title row under the bar.
    static func isGone(scrolled offset: CGFloat) -> Bool { offset > scrolledAway }
}
