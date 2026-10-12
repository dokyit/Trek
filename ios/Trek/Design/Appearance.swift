import SwiftUI

/// The phone's own appearance settings (Settings › Appearance), kept on the phone: theme, text
/// size, background art, motion and row density. `RootView` applies them app-wide.
enum AppearanceKey {
    static let theme = "appearance"
    static let textSize = "textSize"
    static let calm = "reduceMotion"
    static let compact = "compactRows"
    /// The ridge art behind the lists and the new-thread screen: off unless chosen, as on the Mac.
    static let backgroundArt = "backgroundArt"
}

/// Theme names as the Mac has them: Night is dark, Paper is light.
enum ThemeChoice: String, CaseIterable, Identifiable {
    case system, dark, light
    var id: String { rawValue }
    var label: String {
        switch self {
        case .system: "Match iPhone"
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

/// The app icons, as the Mac offers them (its Settings › Appearance › App icon): the alternatives
/// are `AppIcon-Night` and `AppIcon-Glass` in the asset catalog (ios/scripts/app-icons.sh).
enum AppIconChoice: String, CaseIterable, Identifiable {
    case ember, night, glass
    var id: String { rawValue }
    var label: String {
        switch self {
        case .ember: "Ember"
        case .night: "Night"
        case .glass: "Glass"
        }
    }
    /// The name `setAlternateIconName` takes; nil for the primary icon.
    var iconName: String? {
        switch self {
        case .ember: nil
        case .night: "AppIcon-Night"
        case .glass: "AppIcon-Glass"
        }
    }
    /// A small copy of it to pick from.
    var preview: String {
        switch self {
        case .ember: "IconEmber"
        case .night: "IconNight"
        case .glass: "IconGlass"
        }
    }

    static func of(_ iconName: String?) -> AppIconChoice {
        allCases.first { $0.iconName == iconName } ?? .ember
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
        let s = TextScale.factor(dynamicType, style: style) * size
        content.font(design == .monospaced ? .code(size: s, weight: weight) : .ui(size: s, weight: weight))
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
