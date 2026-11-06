import SwiftUI

/// What runs wider than the screen and scrolls sideways (code, tables, a command's output), fading
/// out at whichever edge has more. Cut off clean at the edge, a line looked like the end of it.
struct SidewaysScroll<Content: View>: View {
    @ViewBuilder var content: Content
    @State private var more = SidewaysEdges()

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) { content }
            .onScrollGeometryChange(for: SidewaysEdges.self) { geo in
                SidewaysEdges(contentWidth: geo.contentSize.width, viewWidth: geo.containerSize.width, offset: geo.contentOffset.x)
            } action: { _, new in more = new }
            .mask {
                HStack(spacing: 0) {
                    LinearGradient(colors: [.clear, .black], startPoint: .leading, endPoint: .trailing)
                        .frame(width: more.before ? SidewaysEdges.fade : 0)
                    Color.black
                    LinearGradient(colors: [.black, .clear], startPoint: .leading, endPoint: .trailing)
                        .frame(width: more.after ? SidewaysEdges.fade : 0)
                }
            }
    }
}

/// Whether there's more to scroll to at either end.
nonisolated struct SidewaysEdges: Equatable {
    static let fade: CGFloat = 28

    var before = false
    var after = false

    init() {}

    init(contentWidth: CGFloat, viewWidth: CGFloat, offset: CGFloat) {
        // A point's slack: a width that just fits isn't "more".
        before = offset > 1
        after = contentWidth - offset - viewWidth > 1
    }
}
