import SwiftUI

/// The phone's own appearance settings (Settings › Appearance), kept on the phone: theme, text
/// size, motion and row density. `RootView` applies them app-wide.
enum AppearanceKey {
    static let theme = "appearance"
    static let textSize = "textSize"
    static let calm = "reduceMotion"
    static let compact = "compactRows"
}

/// Theme names as the Mac has them: Night is dark, Paper is light.
enum ThemeChoice: String, CaseIterable, Identifiable {
    case system, dark, light
    var id: String { rawValue }
    var label: String {
        switch self {
        case .system: "System"
        case .dark: "Night"
        case .light: "Paper"
        }
    }
    var scheme: ColorScheme? {
        switch self {
        case .system: nil
        case .dark: .dark
        case .light: .light
        }
    }
}

/// Text size steps. `system` follows the iPhone's own setting; the others pin a Dynamic Type size,
/// so every text style, and the fixed sizes scaled with `.scaledFont`, follow it.
enum TextSizeChoice: String, CaseIterable, Identifiable {
    case system, small, medium, large, xLarge, xxLarge
    var id: String { rawValue }
    var label: String {
        switch self {
        case .system: "Match iPhone"
        case .small: "Small"
        case .medium: "Medium"
        case .large: "Default"
        case .xLarge: "Large"
        case .xxLarge: "Larger"
        }
    }
    var dynamicType: DynamicTypeSize? {
        switch self {
        case .system: nil
        case .small: .small
        case .medium: .medium
        case .large: .large
        case .xLarge: .xLarge
        case .xxLarge: .xxLarge
        }
    }
}

extension EnvironmentValues {
    /// Trek's own "Reduce motion": the hiker stands still, nothing shimmers or breathes. The
    /// system setting does the same; this one is for Trek alone.
    @Entry var trekCalm = false
    /// Denser thread rows.
    @Entry var compactRows = false
    /// The open thread's project hue: folders named in it take the project's colour.
    @Entry var projectHue: Int? = nil
}

/// Whether animation is welcome here: neither iOS nor Trek's own setting asks for less motion.
@propertyWrapper
struct MotionAllowed: DynamicProperty {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.trekCalm) private var calm
    var wrappedValue: Bool { !reduceMotion && !calm }
}

extension View {
    /// A fixed point size that grows and shrinks with Dynamic Type (and Trek's text size).
    func scaledFont(_ size: CGFloat, weight: Font.Weight = .regular, design: Font.Design = .default,
                    relativeTo style: Font.TextStyle = .body) -> some View {
        modifier(ScaledFont(size: size, weight: weight, design: design, style: style))
    }
}

private struct ScaledFont: ViewModifier {
    var size: CGFloat
    var weight: Font.Weight
    var design: Font.Design
    var style: Font.TextStyle
    @Environment(\.dynamicTypeSize) private var dynamicType

    func body(content: Content) -> some View {
        content.font(.system(size: TextScale.factor(dynamicType, style: style) * size, weight: weight, design: design))
    }
}

enum TextScale {
    /// How much `style` grows at `size` relative to Default (Large), as UIKit scales it.
    static func factor(_ size: DynamicTypeSize, style: Font.TextStyle = .body) -> CGFloat {
        let traits = UITraitCollection(preferredContentSizeCategory: UIContentSizeCategory(size))
        let ui = uiStyle(style)
        return UIFontMetrics(forTextStyle: ui).scaledValue(for: 100, compatibleWith: traits) / 100
    }

    private static func uiStyle(_ style: Font.TextStyle) -> UIFont.TextStyle {
        switch style {
        case .largeTitle: .largeTitle
        case .title: .title1
        case .title2: .title2
        case .title3: .title3
        case .headline: .headline
        case .subheadline: .subheadline
        case .callout: .callout
        case .footnote: .footnote
        case .caption: .caption1
        case .caption2: .caption2
        default: .body
        }
    }
}
