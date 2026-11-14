import Observation
import SwiftUI
import UIKit

// Trek themes on the phone: the Mac's whole looks (crates/trek-core/src/themes.rs, the built-ins in
// crates/trek-core/themes/*.toml), drawn natively. A theme's nine colours become `Trek`'s palette
// for light and dark, its fonts the interface's and the conversation's, its sizes a scale on top
// of Dynamic Type, its corners a scale on the app's radii, compact density the compact rows, and
// still motion Trek's own Reduce motion. Settings › Appearance picks one, or "Match your Mac",
// which draws whatever theme the paired Mac has on (its own and agents' theme files too), from
// the snapshot's `look` (`MacLook`).

/// A Trek theme as the phone draws it.
nonisolated struct Look: Equatable, Sendable, Identifiable {
    enum Mode: String, Sendable { case dark, light, any }

    /// The families a theme may set text in: the ones Trek ships on every platform.
    enum Face: String, Sendable {
        case sans, mono, serif

        /// Its family as the app registers it.
        var family: String {
            switch self {
            case .sans: TrekFont.ui
            case .mono: TrekFont.code
            case .serif: TrekFont.serif
            }
        }

        /// The PostScript name of its cut at `weight`, for UIKit (the navigation bars).
        func postScript(_ weight: Font.Weight) -> String {
            let base = switch self {
            case .sans: "Inter"
            case .mono: "JetBrainsMonoNL"
            case .serif: "Spectral"
            }
            let cut = switch weight {
            case .bold, .heavy, .black: "Bold"
            case .semibold: "SemiBold"
            case .medium: "Medium"
            default: "Regular"
            }
            return "\(base)-\(cut)"
        }
    }

    enum Density: String, Sendable { case compact, regular, roomy }
    enum Motion: String, Sendable { case still, brisk, standard, calm }

    /// A theme's colours for light or dark, each `0xRRGGBBAA`. What it leaves out is Night's or
    /// Paper's.
    struct Palette: Equatable, Sendable {
        var accent: UInt32?
        var onAccent: UInt32?
        var window: UInt32?
        var sidebar: UInt32?
        var surface: UInt32?
        var raised: UInt32?
        var text: UInt32?
        var muted: UInt32?
        var border: UInt32?

        init(accent: UInt32? = nil, onAccent: UInt32? = nil, window: UInt32? = nil, sidebar: UInt32? = nil,
             surface: UInt32? = nil, raised: UInt32? = nil, text: UInt32? = nil, muted: UInt32? = nil, border: UInt32? = nil) {
            self.accent = accent
            self.onAccent = onAccent
            self.window = window
            self.sidebar = sidebar
            self.surface = surface
            self.raised = raised
            self.text = text
            self.muted = muted
            self.border = border
        }

        /// From the theme file's `"#RRGGBB"` and `"#RRGGBBAA"` strings (a key that isn't a colour
        /// is left to Night's or Paper's).
        init(_ hex: [String: String]) {
            // The wire's keys may come camel-cased: the decoder's snake_case conversion reaches
            // a dictionary's keys too.
            let c = { (key: String) in (hex[key] ?? hex[Look.camel(key)]).flatMap(Look.colour) }
            self.init(accent: c("accent"), onAccent: c("on_accent"), window: c("window"), sidebar: c("sidebar"),
                      surface: c("surface"), raised: c("raised"), text: c("text"), muted: c("muted"), border: c("border"))
        }

        var isEmpty: Bool { self == Palette() }
    }

    var id: String
    var name: String
    var description = ""
    var mode = Mode.any
    var uiFont = Face.sans
    var conversationFont = Face.sans
    /// Points on the Mac: Trek's own are 14 and 14.5.
    var uiSize = Look.trekUISize
    var conversationSize = Look.trekConversationSize
    /// Every corner's radius against Trek's: 0 square, 1 as designed, up to 2.
    var corners = 1.0
    var density = Density.regular
    var motion = Motion.standard
    var dark = Palette()
    var light = Palette()
    /// The art it comes with, for "Match your Mac" (Alpenglow's painting); the phone's own
    /// background setting decides otherwise.
    var art: String?

    static let trekUISize = 14.0
    static let trekConversationSize = 14.5

    func palette(dark: Bool) -> Palette { dark ? self.dark : light }

    /// The interface's text against Trek's, on top of Dynamic Type.
    var uiScale: CGFloat { CGFloat(uiSize / Self.trekUISize) }
    /// The conversation's text against Trek's.
    var conversationScale: CGFloat { CGFloat(conversationSize / Self.trekConversationSize) }

    /// `on_accent` as `onAccent`.
    static func camel(_ key: String) -> String {
        let parts = key.split(separator: "_")
        return parts.enumerated().map { $0.offset == 0 ? String($0.element) : $0.element.capitalized }.joined()
    }

    /// `#RRGGBB` or `#RRGGBBAA` as `0xRRGGBBAA`.
    static func colour(_ text: String) -> UInt32? {
        let t = text.trimmingCharacters(in: .whitespaces)
        guard t.hasPrefix("#") else { return nil }
        let hex = t.dropFirst()
        guard hex.allSatisfy(\.isHexDigit), let v = UInt32(hex, radix: 16) else { return nil }
        switch hex.count {
        case 6: return (v << 8) | 0xFF
        case 8: return v
        default: return nil
        }
    }
}

// MARK: The built-in themes

nonisolated extension Look {
    /// Trek's own: Night and Paper as designed.
    static let trek = Look(id: "trek", name: "Trek", description: "Night and Paper, as Trek comes.")

    /// crates/trek-core/themes/console.toml.
    static let console = Look(
        id: "console", name: "Console",
        description: "Every word in monospace, square corners, phosphor green on soft black.",
        mode: .dark, uiFont: .mono, conversationFont: .mono, uiSize: 13, conversationSize: 13.5,
        corners: 0.25, density: .compact, motion: .brisk,
        dark: Palette(["accent": "#6BDB9A", "on_accent": "#06140C", "window": "#0B0D0C", "sidebar": "#0E1110",
                       "surface": "#151A18", "raised": "#121615", "text": "#D4DDD7", "muted": "#7C8A82", "border": "#222A26"]),
        light: Palette(["accent": "#17804A", "on_accent": "#FFFFFF", "window": "#F6F6F1", "sidebar": "#EEEEE7",
                        "surface": "#E9EAE2", "raised": "#FBFBF7", "text": "#1A1E1B", "muted": "#646B65", "border": "#DADBD1"]))

    /// crates/trek-core/themes/journal.toml.
    static let journal = Look(
        id: "journal", name: "Journal",
        description: "Warm paper, the conversation set in a serif, terracotta accents.",
        mode: .light, uiFont: .sans, conversationFont: .serif, uiSize: 15, conversationSize: 16,
        corners: 1.25, density: .roomy, motion: .calm,
        dark: Palette(["accent": "#E0895A", "on_accent": "#1C0904", "window": "#14110E", "sidebar": "#191612",
                       "surface": "#221E19", "raised": "#1D1915", "text": "#ECE4D8", "muted": "#9B9082", "border": "#2E2922"]),
        light: Palette(["accent": "#B0552C", "on_accent": "#FFFFFF", "window": "#FBF8F2", "sidebar": "#F3EEE5",
                        "surface": "#F0EAE0", "raised": "#FFFDF9", "text": "#231E19", "muted": "#776C61", "border": "#E4DACC"]))

    /// crates/trek-core/themes/alpenglow.toml.
    static let alpenglow = Look(
        id: "alpenglow", name: "Alpenglow",
        description: "Sunrise on the Matterhorn behind the chat, dusk-blue chrome, a peach accent.",
        mode: .dark, uiFont: .sans, conversationFont: .sans, uiSize: 14, conversationSize: 14.5,
        corners: 1.5, density: .regular, motion: .calm,
        dark: Palette(["accent": "#FF9E6B", "on_accent": "#1E0C03", "window": "#0C0F1C", "sidebar": "#0F1324",
                       "surface": "#191E34", "raised": "#151A2D", "text": "#ECEAF3", "muted": "#9799B3", "border": "#272D48"]),
        light: Palette(["accent": "#B24A17", "on_accent": "#FFFFFF", "window": "#FBF6F0", "sidebar": "#F5EEE6",
                        "surface": "#EFE6DB", "raised": "#FFFBF6", "text": "#2A2320", "muted": "#7A6D63", "border": "#E3D7C8"]),
        art: "builtin:paintings/matterhorn")

    /// Trek's own first, then the ones it ships, as the Mac lists them.
    static let builtIn: [Look] = [.trek, .console, .journal, .alpenglow]

    static func builtIn(_ id: String) -> Look? { builtIn.first { $0.id == id } }
}

// MARK: The Mac's look on the wire

/// The snapshot's `look`: the Mac's Trek theme, whole, and its background art (docs/MOBILE.md,
/// "The Mac's look"). Enumerations come as strings: one this version doesn't know reads as the
/// nearest it does.
nonisolated struct MacLook: Codable, Equatable, Sendable {
    struct Theme: Codable, Equatable, Sendable {
        var id: String
        var name: String
        var description: String? = nil
        var mode: String? = nil
        var uiFont: String? = nil
        var conversationFont: String? = nil
        var uiSize: Double? = nil
        var conversationSize: Double? = nil
        var corners: Double? = nil
        var density: String? = nil
        var motion: String? = nil
        var dark: [String: String]? = nil
        var light: [String: String]? = nil
    }

    var theme: Theme
    /// Built-in art (`builtin:…`); none for no art or a picture of the user's own.
    var background: String? = nil
    /// `chat`, `sidebar` or `new_thread`.
    var placement: String? = nil
    var dim: Double? = nil

    /// The theme as the phone draws it: values out of the Mac's ranges are brought into them.
    var look: Look {
        let t = theme
        func clamp(_ v: Double?, _ lo: Double, _ hi: Double, _ fallback: Double) -> Double {
            guard let v, v.isFinite else { return fallback }
            return min(max(v, lo), hi)
        }
        return Look(
            id: t.id, name: t.name, description: t.description ?? "",
            mode: t.mode.flatMap(Look.Mode.init) ?? .any,
            uiFont: t.uiFont.flatMap(Look.Face.init) ?? .sans,
            conversationFont: t.conversationFont.flatMap(Look.Face.init) ?? .sans,
            uiSize: clamp(t.uiSize, 11, 18, Look.trekUISize),
            conversationSize: clamp(t.conversationSize, 12, 22, Look.trekConversationSize),
            corners: clamp(t.corners, 0, 2, 1),
            density: t.density.flatMap(Look.Density.init) ?? .regular,
            motion: t.motion.flatMap(Look.Motion.init) ?? .standard,
            dark: Look.Palette(t.dark ?? [:]), light: Look.Palette(t.light ?? [:]),
            art: background)
    }

    /// The art and where it shows, as the Mac has it.
    var art: ArtChoice? {
        guard let background, let piece = ArtPiece.named(background) else { return nil }
        return ArtChoice(piece: piece, placement: placement.flatMap(ArtPlacement.init) ?? .newThread,
                         dim: min(max(dim ?? 0.35, 0), 0.9))
    }
}

nonisolated extension MacLook {
    /// The look, or none when it can't be read: a look a newer Mac shapes differently never
    /// costs the phone its snapshot.
    struct Lenient: Decodable {
        var value: MacLook?
        init(from decoder: Decoder) throws { value = try? MacLook(from: decoder) }
    }

    /// A built-in theme and art as a Mac sends them (the demo Mac).
    init(_ look: Look, art: ArtChoice?) {
        func hex(_ p: Look.Palette) -> [String: String]? {
            var out: [String: String] = [:]
            let keys: [(String, UInt32?)] = [("accent", p.accent), ("on_accent", p.onAccent), ("window", p.window),
                                             ("sidebar", p.sidebar), ("surface", p.surface), ("raised", p.raised),
                                             ("text", p.text), ("muted", p.muted), ("border", p.border)]
            for (k, v) in keys { if let v { out[k] = String(format: "#%08X", v) } }
            return out.isEmpty ? nil : out
        }
        self.init(theme: Theme(id: look.id, name: look.name, description: look.description, mode: look.mode.rawValue,
                               uiFont: look.uiFont.rawValue, conversationFont: look.conversationFont.rawValue,
                               uiSize: look.uiSize, conversationSize: look.conversationSize, corners: look.corners,
                               density: look.density.rawValue, motion: look.motion.rawValue,
                               dark: hex(look.dark), light: hex(look.light)),
                  background: art?.piece.spec, placement: art?.placement.rawValue, dim: art?.dim)
    }
}

// MARK: The colours a look gives the app

/// `Trek`'s palette for a look: its colours where it sets them, Night's and Paper's where not.
nonisolated struct LookColors {
    /// One role's colour in Paper (light) and Night (dark), `0xRRGGBB` with its alpha apart.
    struct Pair {
        var light: UInt32
        var dark: UInt32
        var lightAlpha: Double = 1
        var darkAlpha: Double = 1

        func rgb(dark d: Bool) -> UInt32 { d ? dark : light }

        var color: Color {
            let p = self
            return Color(uiColor: UIColor { tc in
                let dark = tc.userInterfaceStyle == .dark
                return UIColor(hex: dark ? p.dark : p.light).withAlphaComponent(dark ? p.darkAlpha : p.lightAlpha)
            })
        }
    }

    var background: Pair
    var grouped: Pair
    var surface: Pair
    var elevated: Pair
    var border: Pair
    var foreground: Pair
    var muted: Pair
    var ember: Pair
    var onEmber: Pair
    var link: Pair

    /// Trek's own, from the Mac's theme (crates/trek-app/assets/themes/trek.json): (Paper, Night).
    static let trek = LookColors(
        background: Pair(light: 0xFFFFFF, dark: 0x0A0A0B),
        // Behind grouped lists (Settings): the Mac's sidebar, so the rows' cards stand out.
        grouped: Pair(light: 0xF6F5F3, dark: 0x0A0A0B),
        surface: Pair(light: 0xF4F3F1, dark: 0x161618),
        elevated: Pair(light: 0xFFFFFF, dark: 0x161618),
        border: Pair(light: 0xE7E5E1, dark: 0x232327),
        foreground: Pair(light: 0x1A1A1C, dark: 0xE4E4E7),
        muted: Pair(light: 0x6E6C68, dark: 0x8B8B94),
        ember: Pair(light: 0xE85D1F, dark: 0xFF7A3D),
        onEmber: Pair(light: 0xFFFFFF, dark: 0x160A04),
        // Links in answers: ember, a shade darker on Paper so it reads at body size.
        link: Pair(light: 0xC2410C, dark: 0xFF7A3D))

    init(background: Pair, grouped: Pair, surface: Pair, elevated: Pair, border: Pair, foreground: Pair,
         muted: Pair, ember: Pair, onEmber: Pair, link: Pair) {
        self.background = background
        self.grouped = grouped
        self.surface = surface
        self.elevated = elevated
        self.border = border
        self.foreground = foreground
        self.muted = muted
        self.ember = ember
        self.onEmber = onEmber
        self.link = link
    }

    /// `look`'s colours, as the Mac makes its kit theme from them (`themes.rs`'s `colours`): the
    /// window is the ground, the sidebar what grouped lists stand on in light, the accent the
    /// working beacon's and links' colour, with text on it in `on_accent` (else white or near-black,
    /// whichever reads).
    init(_ look: Look) {
        let t = LookColors.trek
        func pick(_ base: Pair, _ key: KeyPath<Look.Palette, UInt32?>, opaque: Bool = false) -> Pair {
            var p = base
            if let c = look.light[keyPath: key] {
                p.light = c >> 8
                p.lightAlpha = opaque ? 1 : Double(c & 0xFF) / 255
            }
            if let c = look.dark[keyPath: key] {
                p.dark = c >> 8
                p.darkAlpha = opaque ? 1 : Double(c & 0xFF) / 255
            }
            return p
        }
        background = pick(t.background, \.window, opaque: true)
        // Grouped lists stand on the sidebar's colour in light, on the window in dark (as Trek's
        // own do).
        var grouped = pick(t.grouped, \.sidebar, opaque: true)
        grouped.dark = background.dark
        self.grouped = grouped
        surface = pick(t.surface, \.surface)
        elevated = pick(t.elevated, \.raised)
        border = pick(t.border, \.border)
        foreground = pick(t.foreground, \.text)
        muted = pick(t.muted, \.muted)
        ember = pick(t.ember, \.accent, opaque: true)
        link = pick(t.link, \.accent, opaque: true)
        var on = pick(t.onEmber, \.onAccent, opaque: true)
        // An accent with no colour for text on it: white if that reads, else near-black.
        func readable(_ accent: UInt32) -> UInt32 { Scrim.contrast(accent, 0xFFFFFF) >= 3 ? 0xFFFFFF : 0x111111 }
        if look.light.accent != nil, look.light.onAccent == nil { on.light = readable(ember.light) }
        if look.dark.accent != nil, look.dark.onAccent == nil { on.dark = readable(ember.dark) }
        onEmber = on
    }
}

/// A look's colours as SwiftUI's, each following light and dark.
struct Paint {
    var background, grouped, surface, elevated, border, foreground, muted, ember, onEmber, link: Color

    init(_ c: LookColors) {
        background = c.background.color
        grouped = c.grouped.color
        surface = c.surface.color
        elevated = c.elevated.color
        border = c.border.color
        foreground = c.foreground.color
        muted = c.muted.color
        ember = c.ember.color
        onEmber = c.onEmber.color
        link = c.link.color
    }
}

// MARK: The look on screen

/// The look the app is drawn in, and the art behind it: what Settings › Appearance picked, or the
/// paired Mac's. `Trek`'s colours and `Font.ui` read it, so every view that draws with them is
/// redrawn when it changes.
@Observable
final class LookStore {
    static let shared = LookStore()

    private(set) var look: Look = .trek
    private(set) var colors = LookColors.trek
    /// `colors` as SwiftUI colours, made once per look.
    private(set) var paint = Paint(.trek)
    /// Counts the looks put up: what's cached in one (text built with its colours) is told apart.
    private(set) var generation = 0
    /// The art behind the lists and threads, and where it shows; none for none.
    private(set) var art: ArtChoice?
    /// The last look the paired Mac sent (kept, so the phone opens in it before it reconnects).
    private(set) var mac: MacLook?

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        if let data = defaults.data(forKey: AppearanceKey.macLook) {
            mac = try? JSONDecoder.snake.decode(MacLook.self, from: data)
        }
        refresh()
    }

    @ObservationIgnored private let defaults: UserDefaults

    /// What Settings says: a built-in's id or `mac`.
    var themeSetting: String { defaults.string(forKey: AppearanceKey.trekTheme) ?? AppearanceKey.matchMac }
    /// `mac`, `none`, or a piece's setting (`builtin:paintings/oxbow`).
    var artSetting: String { Self.artSetting(defaults) }

    static func artSetting(_ defaults: UserDefaults) -> String {
        if let s = defaults.string(forKey: AppearanceKey.background) { return s }
        // The phone's earlier switch, on: the ridge at dusk it showed.
        if defaults.bool(forKey: AppearanceKey.backgroundArt) { return "builtin:night" }
        return AppearanceKey.matchMac
    }

    /// The Mac sent its look (none from an older Mac, or before its snapshot: the last one stays).
    func macSent(_ look: MacLook?) {
        guard let look, look != mac else { return }
        mac = look
        if let data = try? JSONEncoder.snake.encode(look) { defaults.set(data, forKey: AppearanceKey.macLook) }
        refresh()
    }

    /// Settings changed: work out the look and art again.
    func refresh() {
        let (look, art) = Self.resolve(theme: themeSetting, art: artSetting, mac: mac,
                                       placement: defaults.string(forKey: AppearanceKey.placement),
                                       dim: defaults.object(forKey: AppearanceKey.dim) as? Double)
        if look != self.look {
            self.look = look
            colors = LookColors(look)
            paint = Paint(colors)
            generation += 1
            TrekFont.styleBars(look)
        }
        if art != self.art { self.art = art }
    }

    /// The look and art for these settings: `mac` takes the Mac's (Trek's own and no art before a
    /// Mac has said, or from one too old to); a built-in id its theme; a piece its art, where the
    /// collection puts it unless `placement` says otherwise.
    nonisolated static func resolve(theme: String, art: String, mac: MacLook?, placement: String?, dim: Double?) -> (Look, ArtChoice?) {
        let look = theme == AppearanceKey.matchMac ? (mac?.look ?? .trek) : (Look.builtIn(theme) ?? .trek)
        let choice: ArtChoice?
        switch art {
        case AppearanceKey.matchMac: choice = mac?.art
        case "none": choice = nil
        default:
            choice = ArtPiece.named(art).map { piece in
                ArtChoice(piece: piece,
                          placement: piece.collection.placement ?? placement.flatMap(ArtPlacement.init) ?? .newThread,
                          dim: min(max(dim ?? 0.35, 0), 0.9))
            }
        }
        return (look, choice)
    }
}

nonisolated extension JSONDecoder {
    /// The wire's snake_case keys.
    static var snake: JSONDecoder {
        let d = JSONDecoder()
        d.keyDecodingStrategy = .convertFromSnakeCase
        return d
    }
}

nonisolated extension JSONEncoder {
    static var snake: JSONEncoder {
        let e = JSONEncoder()
        e.keyEncodingStrategy = .convertToSnakeCase
        return e
    }
}

extension Trek {
    /// A radius the app is designed with, in the look's corners (Console's are near square).
    static func corner(_ r: CGFloat) -> CGFloat { r * CGFloat(LookStore.shared.look.corners) }
}
