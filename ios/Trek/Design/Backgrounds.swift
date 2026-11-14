import SwiftUI

// The Mac's background art on the phone (crates/trek-app/src/backgrounds.rs): the same four
// collections, cut to a phone's shape in Backgrounds.xcassets (ios/scripts/backgrounds.py), and
// the same scrim: the theme's ground laid over the art just strongly enough that text keeps 4.5:1
// and grey text 3:1 against the art's most demanding pixel, with the user's Dim on top.
//
// Where it shows: the thread list is the phone's sidebar, a thread its chat. Art "across the
// chat" lies behind both; art "in the sidebar" (ASCII always) behind the thread list alone; art
// "behind a new thread" at the top of the lists and the new-thread sheet, fading into the ground,
// as the phone has always drawn the ridge.

/// A labelled group of pieces, as the Mac's gallery has them.
nonisolated enum ArtCollection: String, CaseIterable, Sendable {
    case classic, paintings, ascii, light

    var title: String {
        switch self {
        case .classic: "Classic"
        case .paintings: "Paintings"
        case .ascii: "ASCII"
        case .light: "Light"
        }
    }

    var note: String {
        switch self {
        case .classic: "Dithered ridges, wherever you put them."
        case .paintings: "Public-domain landscapes in Trek's colours, behind your threads and the chat."
        case .ascii: "Mountains drawn in text. Behind the thread list only: the chat stays plain."
        case .light: "Long-exposure light made of glyphs, behind your threads and the chat."
        }
    }

    /// Where a piece from here goes when it's picked; nil keeps the user's choice.
    var placement: ArtPlacement? {
        switch self {
        case .classic: nil
        case .paintings, .light: .chat
        case .ascii: .sidebar
        }
    }

    /// The pixel that asks the most of a scrim: the brightest of the Night files (`dark`), the
    /// darkest of the Paper ones (`Collection::extreme`; backgrounds.py holds the phone's files to
    /// it).
    func extreme(dark: Bool) -> UInt32 {
        switch (self, dark) {
        case (.paintings, true): 0xFF7A3D
        case (.paintings, false): 0x6A6058
        case (.ascii, true): 0xFF8448
        case (.ascii, false): 0x655A4E
        case (.light, true): 0xBEAAA0
        case (.light, false): 0x74604F
        case (.classic, true): 0xFFFFFF
        case (.classic, false): 0x000000
        }
    }
}

/// Where the art shows (the Mac's `BackgroundPlacement`, by the wire's names).
nonisolated enum ArtPlacement: String, CaseIterable, Sendable {
    /// Across the chat: behind the thread list and the thread.
    case chat
    /// Behind the sidebar: the thread list alone.
    case sidebar
    /// Behind a new thread: the top of the lists and the new-thread sheet.
    case newThread = "new_thread"

    var label: String {
        switch self {
        case .chat: "Everywhere"
        case .sidebar: "Thread list"
        case .newThread: "New thread"
        }
    }
}

/// One piece of built-in art.
nonisolated struct ArtPiece: Identifiable, Equatable, Hashable, Sendable {
    /// Its file's name in its collection (and the setting's id).
    var key: String
    var name: String
    var collection: ArtCollection
    /// Who made it, for a painting.
    var credit = ""

    /// The setting that names it: `builtin:dawn` for Classic, `builtin:<collection>/<key>` for
    /// the rest (as the Mac's).
    var spec: String { collection == .classic ? "builtin:\(key)" : "builtin:\(collection.rawValue)/\(key)" }
    var id: String { spec }
    /// Its image set: a Night and a Paper version, but for Classic's.
    var asset: String { "bg-\(collection.rawValue)-\(key)" }
    var thumbnail: String { asset + "-thumb" }

    /// Every built-in piece, in the gallery's order.
    static let all: [ArtPiece] = [
        ArtPiece(key: "dawn", name: "Dawn", collection: .classic),
        ArtPiece(key: "night", name: "Night", collection: .classic),
        ArtPiece(key: "paper", name: "Paper", collection: .classic),
        ArtPiece(key: "matterhorn", name: "Matterhorn", collection: .paintings,
                 credit: "Albert Bierstadt, Sunrise on the Matterhorn (after 1875). The Metropolitan Museum of Art."),
        ArtPiece(key: "kauterskill", name: "Kauterskill", collection: .paintings,
                 credit: "Sanford Robinson Gifford, A Gorge in the Mountains (Kauterskill Clove) (1862). The Metropolitan Museum of Art."),
        ArtPiece(key: "oxbow", name: "Oxbow", collection: .paintings,
                 credit: "Thomas Cole, View from Mount Holyoke after a Thunderstorm (The Oxbow) (1836). The Metropolitan Museum of Art."),
        ArtPiece(key: "notch", name: "Notch", collection: .paintings,
                 credit: "Thomas Cole, A View of the Mountain Pass Called the Notch of the White Mountains (1839). National Gallery of Art."),
        ArtPiece(key: "green-river", name: "Green River", collection: .paintings,
                 credit: "Thomas Moran, Green River Cliffs, Wyoming (1881). National Gallery of Art."),
        ArtPiece(key: "aosta", name: "Aosta", collection: .paintings,
                 credit: "J. M. W. Turner, Valley of Aosta: Snowstorm, Avalanche, and Thunderstorm (1836–37). Art Institute of Chicago."),
        ArtPiece(key: "keelmen", name: "Moonlight", collection: .paintings,
                 credit: "J. M. W. Turner, Keelmen Heaving in Coals by Moonlight (1835). National Gallery of Art."),
        ArtPiece(key: "merced", name: "Merced", collection: .paintings,
                 credit: "Albert Bierstadt, Merced River, Yosemite Valley (1866). The Metropolitan Museum of Art."),
        ArtPiece(key: "ramp", name: "Ramp", collection: .ascii),
        ArtPiece(key: "braille", name: "Braille", collection: .ascii),
        ArtPiece(key: "halftone", name: "Halftone", collection: .ascii),
        ArtPiece(key: "dense", name: "Dense", collection: .ascii),
        ArtPiece(key: "led", name: "LED", collection: .ascii),
        ArtPiece(key: "streaks", name: "Streaks", collection: .light),
        ArtPiece(key: "aurora", name: "Aurora", collection: .light),
        ArtPiece(key: "window", name: "Window", collection: .light),
        ArtPiece(key: "switchbacks", name: "Switchbacks", collection: .light),
    ]

    /// The piece a setting names (`builtin:paintings/oxbow`, or a bare `builtin:oxbow`, as the
    /// Mac reads them), if it names one this version has.
    static func named(_ spec: String) -> ArtPiece? {
        guard spec.hasPrefix("builtin:") else { return nil }
        let name = String(spec.dropFirst("builtin:".count))
        return all.first { $0.spec == spec || $0.key == name }
    }
}

/// The art in use: which piece, where it shows, and how much it's dimmed beyond what the text
/// needs.
nonisolated struct ArtChoice: Equatable, Sendable {
    var piece: ArtPiece
    var placement: ArtPlacement
    var dim: Double

    /// Whether it lies behind `surface`, and how: full (under a scrim) or as the fading header.
    func shows(on surface: ArtSurface) -> ArtStyle? {
        // ASCII art is the sidebar's, wherever it's put.
        let placement = piece.collection == .ascii ? .sidebar : placement
        switch (placement, surface) {
        case (.chat, _): return .full
        case (.sidebar, .threadList): return .full
        case (.sidebar, _): return nil
        case (.newThread, .thread): return nil
        case (.newThread, _): return .header
        }
    }
}

/// Where a backdrop is drawn.
enum ArtSurface {
    /// The thread list: the phone's sidebar.
    case threadList
    /// A thread's transcript: the phone's chat.
    case thread
    /// Basecamp and Notes.
    case pane
    /// The new-thread sheet.
    case newThread
}

enum ArtStyle {
    /// Edge to edge, under the scrim.
    case full
    /// At the top, fading into the ground.
    case header
}

/// The contrast the art leaves the text (`backgrounds.rs`'s `scrim`, `with_dim` and WCAG's maths).
nonisolated enum Scrim {
    /// Transcript text over art keeps WCAG AA for body text.
    static let textRatio = 4.5
    /// Grey text (times, counts, labels).
    static let mutedRatio = 3.0
    /// The most a scrim takes: the art nearly gone.
    static let most = 0.92

    /// The least alpha of `ground` over `art` (both `0xRRGGBB`) at which `text` keeps `ratio`.
    static func alpha(art: UInt32, ground: UInt32, text: UInt32, ratio: Double) -> Double {
        func reads(_ a: Double) -> Bool { contrast(rgb(text), mix(rgb(art), rgb(ground), a)) >= ratio }
        if reads(0) { return 0 }
        if !reads(most) { return most }
        var lo = 0.0, hi = most
        for _ in 0..<24 {
            let mid = (lo + hi) / 2
            if reads(mid) { hi = mid } else { lo = mid }
        }
        // Rounded up a little, so the text keeps its ratio after the GPU's rounding.
        return min(hi + 0.01, most)
    }

    /// The scrim `need`ed, with the user's Dim on top: Dim darkens the art further, it never
    /// takes away what the text needs.
    static func withDim(_ need: Double, _ dim: Double) -> Double {
        1 - (1 - min(max(need, 0), 1)) * (1 - min(max(dim, 0), 0.9))
    }

    /// The scrim over `choice`'s art on `ground` for `text` (4.5:1) and `muted` (3:1), Dim on top.
    static func over(_ choice: ArtChoice, dark: Bool, ground: UInt32, text: UInt32, muted: UInt32) -> Double {
        let extreme = choice.piece.collection.extreme(dark: dark)
        let need = max(alpha(art: extreme, ground: ground, text: text, ratio: textRatio),
                       alpha(art: extreme, ground: ground, text: muted, ratio: mutedRatio))
        return withDim(need, choice.dim)
    }

    static func rgb(_ hex: UInt32) -> [Double] {
        [Double((hex >> 16) & 0xFF) / 255, Double((hex >> 8) & 0xFF) / 255, Double(hex & 0xFF) / 255]
    }

    /// `art` with `ground` over it at `alpha`, as the GPU blends them.
    static func mix(_ art: [Double], _ ground: [Double], _ alpha: Double) -> [Double] {
        (0..<3).map { art[$0] * (1 - alpha) + ground[$0] * alpha }
    }

    static func luminance(_ c: [Double]) -> Double {
        func lin(_ v: Double) -> Double { v <= 0.04045 ? v / 12.92 : pow((v + 0.055) / 1.055, 2.4) }
        return 0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2])
    }

    static func contrast(_ a: [Double], _ b: [Double]) -> Double {
        let la = luminance(a), lb = luminance(b)
        return (max(la, lb) + 0.05) / (min(la, lb) + 0.05)
    }

    static func contrast(_ a: UInt32, _ b: UInt32) -> Double { contrast(rgb(a), rgb(b)) }
}

/// The art behind a surface, as the look and Settings › Appearance have it: edge to edge under
/// the scrim its text needs, or the fading header. Nothing when no art goes there.
struct ArtBackdrop: View {
    var surface: ArtSurface
    /// The header's height, when it's the header.
    var height: CGFloat = 320
    @Environment(\.colorScheme) private var scheme

    var body: some View {
        let store = LookStore.shared
        if let art = store.art, let style = art.shows(on: surface) {
            let dark = scheme == .dark
            switch style {
            case .full:
                let c = store.colors
                let alpha = Scrim.over(art, dark: dark, ground: c.background.rgb(dark: dark),
                                       text: c.foreground.rgb(dark: dark), muted: c.muted.rgb(dark: dark))
                Color.clear
                    .overlay { Image(art.piece.asset).resizable().interpolation(.medium).scaledToFill() }
                    .overlay(Trek.background.opacity(alpha))
                    .clipped()
                    .accessibilityHidden(true)
            case .header:
                Color.clear
                    .frame(height: height)
                    .overlay { Image(art.piece.asset).resizable().interpolation(.none).scaledToFill() }
                    .clipped()
                    .opacity(0.9)
                    .mask(LinearGradient(stops: [.init(color: .black, location: 0), .init(color: .black.opacity(0.6), location: 0.55),
                                                 .init(color: .clear, location: 1)], startPoint: .top, endPoint: .bottom))
                    .frame(maxHeight: .infinity, alignment: .top)
                    .accessibilityHidden(true)
            }
        }
    }
}
