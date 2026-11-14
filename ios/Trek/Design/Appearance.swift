import SwiftUI

/// The phone's own appearance settings (Settings › Appearance), kept on the phone: theme, text
/// size, background art, motion and row density. `RootView` applies them app-wide.
nonisolated enum AppearanceKey {
    static let theme = "appearance"
    static let textSize = "textSize"
    static let calm = "reduceMotion"
    static let compact = "compactRows"
    /// The ridge art behind the lists and the new-thread screen: the switch before the Mac's
    /// collections came, read once as Classic Night (`LookStore.artSetting`).
    static let backgroundArt = "backgroundArt"
    /// The Trek theme: `mac` (Match your Mac, the default) or a built-in's id (`Look.builtIn`).
    static let trekTheme = "trekTheme"
    /// The background art: `mac` (the default), `none`, or a piece (`builtin:paintings/oxbow`).
    static let background = "backgroundPiece"
    /// Where the phone's own Classic art shows (`ArtPlacement`): other collections say themselves.
    static let placement = "backgroundPlacement"
    /// How much the phone's own art is dimmed beyond what the text needs, 0 to 0.9.
    static let dim = "backgroundDim"
    /// The last look the Mac sent, as it came (`MacLook`).
    static let macLook = "macLook"
    /// The setting that follows the paired Mac.
    static let matchMac = "mac"
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

/// The app icons, as the Mac offers them (its Settings › Appearance › App icon): Night is the
/// app's own, and the alternatives are `AppIcon-Paper` and `AppIcon-Glass` in the asset catalog
/// (ios/scripts/app-icons.sh).
enum AppIconChoice: String, CaseIterable, Identifiable {
    case night, paper, glass
    var id: String { rawValue }
    var label: String {
        switch self {
        case .night: "Night"
        case .paper: "Paper"
        case .glass: "Glass"
        }
    }
    /// The name `setAlternateIconName` takes; nil for the primary icon.
    var iconName: String? {
        switch self {
        case .night: nil
        case .paper: "AppIcon-Paper"
        case .glass: "AppIcon-Glass"
        }
    }
    /// A small copy of it to pick from.
    var preview: String {
        switch self {
        case .night: "IconNight"
        case .paper: "IconPaper"
        case .glass: "IconGlass"
        }
    }

    /// The choice iOS is showing. A name this version has no icon for (`AppIcon-Night`, an
    /// alternative before Night became the app's own) is the app's own.
    static func of(_ iconName: String?) -> AppIconChoice {
        allCases.first { $0.iconName == iconName } ?? .night
    }

    /// Whether iOS still holds an alternative this version doesn't ship: it would show a blank
    /// icon for it.
    static func isRetired(_ iconName: String?) -> Bool {
        guard let iconName else { return false }
        return !allCases.contains { $0.iconName == iconName }
    }

    /// Go back to the app's own icon from an alternative that's gone. iOS says so with its own
    /// alert, once.
    @MainActor static func dropRetired() {
        let app = UIApplication.shared
        guard app.supportsAlternateIcons, isRetired(app.alternateIconName) else { return }
        app.setAlternateIconName(nil)
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
    /// Text here is the conversation (messages, answers, notes): set in the theme's conversation
    /// face (Journal's serif) at its size.
    @Entry var trekConversation = false
}

extension Font {
    /// Body text at `size`: the conversation's face where `conversation`, the interface's else.
    static func text(size: CGFloat, weight: Weight = .regular, conversation: Bool) -> Font {
        conversation ? .conversation(size: size, weight: weight) : .ui(size: size, weight: weight)
    }
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
