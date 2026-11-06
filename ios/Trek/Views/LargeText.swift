import SwiftUI

/// Side by side, or one over the other once text runs at the accessibility sizes: side by side
/// there, each part left the other room for a letter or two ("Mi…", "Rea/dy to/mer/ge").
struct BesideOrOver<Content: View>: View {
    var alignment: VerticalAlignment = .center
    /// How the parts line up when they're one over the other.
    var overAlignment: HorizontalAlignment = .leading
    var spacing: CGFloat = 8
    @ViewBuilder var content: Content
    @Environment(\.dynamicTypeSize) private var size

    var body: some View {
        // One layout that changes, not two views: what's inside keeps its state and its sheets.
        let layout = size.isAccessibilitySize
            ? AnyLayout(VStackLayout(alignment: overAlignment, spacing: spacing))
            : AnyLayout(HStackLayout(alignment: alignment, spacing: spacing))
        layout { content }
    }
}

extension View {
    /// For what has a height of its own (a chip, the pill over the tab bar): it stops growing here,
    /// as the system's own bars do, rather than run out of its capsule and over what's beside it.
    /// What the chip says is also said where it opens.
    func barText() -> some View { dynamicTypeSize(...DynamicTypeSize.xxLarge) }
}
