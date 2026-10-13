import CoreText
import Synchronization
import SwiftUI
import UIKit

// MARK: Drawing a formula

/// A formula drawn: an image in the template colour (it takes the text's colour, light or dark),
/// in points, and how far its baseline sits above its bottom edge.
final class MathImage {
    let image: UIImage
    let baseline: CGFloat

    init(image: UIImage, baseline: CGFloat) {
        self.image = image
        self.baseline = baseline
    }
}

/// Formulas typeset by `MathLayout` and drawn with the KaTeX fonts (Resources/Fonts, SIL Open
/// Font License), as the Mac draws them: glyphs in the fonts' own pieces, rules on whole pixels.
/// Drawn once per TeX, style and size; a formula with a command Trek doesn't know isn't drawn at
/// all (`nil`), so its source shows instead of a guess.
enum MathRender {
    /// Maths is set this much larger than the text around it: the KaTeX fonts' x-height is
    /// Computer Modern's, well below Inter's (the Mac's `SCALE`).
    static let scale: CGFloat = 1.21

    private static let cache: NSCache<NSString, MathImage> = {
        let c = NSCache<NSString, MathImage>()
        c.countLimit = 600
        return c
    }()

    /// Formulas known not to draw, by the same key.
    private static var failed: Set<String> = []

    /// `tex` drawn at `em` points per em: display maths, or inline. `nil` when it can't be.
    static func image(_ tex: String, display: Bool, em: CGFloat) -> MathImage? {
        let key = "\(display ? "D" : "T")\(em)|\(tex)"
        if let hit = cache.object(forKey: key as NSString) { return hit }
        if failed.contains(key) { return nil }
        let nodes = tex.count <= 20_000 ? MathParse.parse(tex) : nil
        guard let nodes, !nodes.isEmpty, !MathParse.hasUnknown(nodes) else {
            failed.insert(key)
            return nil
        }
        let box = MathLayout.typeset(nodes, display: display, measure: MathFonts.width)
        guard box.w > 0, box.h + box.d > 0, box.w * em < 8000 else {
            failed.insert(key)
            return nil
        }
        let drawn = draw(box, em: em, display: display)
        cache.setObject(drawn, forKey: key as NSString)
        return drawn
    }

    /// `box` as an image, `em` points per em, with a little room for ink past the box (an italic
    /// letter's tail, a hat): less in a line of text, where it would read as space.
    private static func draw(_ box: MathBox, em: CGFloat, display: Bool) -> MathImage {
        let padX = (em * (display ? 0.1 : 0.03)).rounded(.up)
        let padY = (em * (display ? 0.15 : 0.04)).rounded(.up)
        let size = CGSize(width: (box.w * em).rounded(.up) + 2 * padX, height: ((box.h + box.d) * em).rounded(.up) + 2 * padY)
        let format = UIGraphicsImageRendererFormat.preferred()
        format.opaque = false
        let scale = format.scale
        let baselineY = padY + box.h * em
        let image = UIGraphicsImageRenderer(size: size, format: format).image { context in
            let ctx = context.cgContext
            ctx.setFillColor(UIColor.black.cgColor)
            ctx.setStrokeColor(UIColor.black.cgColor)
            ctx.textMatrix = .identity
            let px = { (x: Double) in padX + x * em }
            let py = { (y: Double) in baselineY - y * em }
            // A rule at least a device pixel thick, on whole device pixels: crisp, never blurred away.
            func rule(_ x: Double, _ y: Double, _ w: Double, _ h: Double) {
                let thick = max((h * em * scale).rounded(), 1) / scale
                let top = (py(y + h) * scale).rounded() / scale
                ctx.fill(CGRect(x: px(x), y: top, width: w * em, height: thick))
            }
            for item in box.items {
                switch item {
                case .glyph(let x, let y, let text, let face, let size):
                    let font = MathFonts.font(face, size: size * em)
                    let line = CTLineCreateWithAttributedString(NSAttributedString(string: text, attributes: [
                        NSAttributedString.Key(kCTFontAttributeName as String): font,
                        NSAttributedString.Key(kCTForegroundColorAttributeName as String): UIColor.black.cgColor,
                    ]))
                    ctx.saveGState()
                    ctx.translateBy(x: px(x), y: py(y))
                    ctx.scaleBy(x: 1, y: -1)
                    ctx.textPosition = .zero
                    CTLineDraw(line, ctx)
                    ctx.restoreGState()
                case .rule(let x, let y, let w, let h):
                    rule(x, y, w, h)
                case .frame(let x, let y, let w, let h, let t):
                    rule(x, y, w, t)
                    rule(x, y + h - t, w, t)
                    let side = max((t * em * scale).rounded(), 1) / scale
                    ctx.fill(CGRect(x: px(x), y: py(y + h), width: side, height: h * em))
                    ctx.fill(CGRect(x: px(x + w) - side, y: py(y + h), width: side, height: h * em))
                case .strike(let x, let y, let w, let h, let t, let forward):
                    ctx.setLineWidth(max(t * em, 1 / scale))
                    ctx.setLineCap(.round)
                    ctx.move(to: CGPoint(x: px(x), y: forward ? py(y) : py(y + h)))
                    ctx.addLine(to: CGPoint(x: px(x + w), y: forward ? py(y + h) : py(y)))
                    ctx.strokePath()
                }
            }
        }
        return MathImage(image: image.withRenderingMode(.alwaysTemplate), baseline: size.height - baselineY)
    }
}

/// The KaTeX faces, loaded from the app's fonts without registering them (nothing else sets
/// text in them), and their widths for `MathLayout`.
nonisolated enum MathFonts {
    private static let graphics = Mutex<[MathFace: CGFont]>([:])
    private static let fonts = Mutex<[String: Held]>([:])

    /// A font in the cache: CoreText fonts are immutable, safe on any thread.
    private final class Held: @unchecked Sendable {
        let font: CTFont
        init(_ font: CTFont) { self.font = font }
    }

    private static func file(_ face: MathFace) -> String {
        switch face {
        case .main: "KaTeX_Main-Regular"
        case .mainBold: "KaTeX_Main-Bold"
        case .mainItalic: "KaTeX_Main-Italic"
        case .mainBoldItalic: "KaTeX_Main-BoldItalic"
        case .mathItalic: "KaTeX_Math-Italic"
        case .mathBoldItalic: "KaTeX_Math-BoldItalic"
        case .ams: "TrekMath_AMS-Regular"
        case .caligraphic: "TrekMath_Caligraphic-Regular"
        case .fraktur: "KaTeX_Fraktur-Regular"
        case .script: "TrekMath_Script-Regular"
        case .sansSerif: "KaTeX_SansSerif-Regular"
        case .typewriter: "KaTeX_Typewriter-Regular"
        case .size1: "TrekMath_Size1-Regular"
        case .size2: "TrekMath_Size2-Regular"
        case .size3: "TrekMath_Size3-Regular"
        case .size4: "TrekMath_Size4-Regular"
        }
    }

    /// `face` at `size` points (Times, should the font file be missing).
    static func font(_ face: MathFace, size: CGFloat) -> CTFont {
        let key = "\(file(face))@\(size)"
        if let hit = fonts.withLock({ $0[key] }) { return hit.font }
        let cg = graphics.withLock { g -> CGFont? in
            if let hit = g[face] { return hit }
            let name = file(face)
            let url = Bundle.main.url(forResource: name, withExtension: "ttf")
                ?? Bundle.main.url(forResource: name, withExtension: "ttf", subdirectory: "Fonts")
            let font = url.flatMap { CGDataProvider(url: $0 as CFURL) }.flatMap { CGFont($0) }
            g[face] = font
            return font
        }
        let font = cg.map { CTFontCreateWithGraphicsFont($0, size, nil, nil) } ?? CTFontCreateWithName("Times New Roman" as CFString, size, nil)
        fonts.withLock { f in
            if f.count > 400 { f.removeAll() }
            f[key] = Held(font)
        }
        return font
    }

    /// The width of `text` in `face`, in ems.
    static func width(_ text: String, _ face: MathFace) -> Double? {
        let size: CGFloat = 100
        let line = CTLineCreateWithAttributedString(NSAttributedString(string: text, attributes: [
            NSAttributedString.Key(kCTFontAttributeName as String): font(face, size: size),
        ]))
        let w = CTLineGetTypographicBounds(line, nil, nil, nil)
        return w.isFinite ? w / size : nil
    }
}

// MARK: Maths in a line of text

/// Inline Markdown, as `RichText` sets it, with its formulas typeset in the line: `$…$` and
/// `\(…\)` (and `$$…$$` or `\[…\]` where a block can't go, in a list item or a table cell). A
/// formula sits on the text's baseline in the text's colour; one Trek can't draw shows its source
/// in a code chip, and one still being written shows as written until it closes. Text with no
/// maths is just `RichText`.
struct RichMathText: View {
    var markdown: String
    var size: CGFloat = 16
    var weight: Font.Weight = .regular
    var style: Font.TextStyle = .body
    @Environment(\.colorScheme) private var scheme
    @Environment(\.projectHue) private var projectHue
    @Environment(\.dynamicTypeSize) private var dynamicType

    var body: some View {
        if MathInline.hasMath(markdown) {
            let s = size * TextScale.factor(dynamicType, style: style)
            MathInline.cachedText(markdown, size: s, weight: weight, dark: scheme == .dark, projectHue: projectHue)
                .font(.ui(size: s, weight: weight))
                .textRenderer(ChipRenderer())
                .tint(Trek.link)
        } else {
            RichText(markdown: markdown, size: size, weight: weight, style: style)
        }
    }
}

enum MathInline {
    /// What stands in for the `n`th formula while Markdown reads the text: a character from the
    /// supplementary private use area, which nothing writes and Markdown leaves alone.
    private static func placeholder(_ n: Int) -> Character { Character(Unicode.Scalar(0xF0000 + UInt32(n))!) }

    private static func index(_ c: Character) -> Int? {
        guard c.unicodeScalars.count == 1, let v = c.unicodeScalars.first?.value, v >= 0xF0000, v < 0xFFFFE else { return nil }
        return Int(v - 0xF0000)
    }

    private nonisolated static let found = Mutex<[String: Bool]>([:])

    /// Whether `markdown` holds a formula (closed or still being written), found once.
    nonisolated static func hasMath(_ markdown: String) -> Bool {
        guard markdown.contains("$") || markdown.contains("\\(") || markdown.contains("\\[") else { return false }
        if let hit = found.withLock({ $0[markdown] }) { return hit }
        let any = !MathDetect.regions(markdown).isEmpty
        found.withLock { c in
            if c.count > 4000 { c.removeAll(keepingCapacity: true) }
            c[markdown] = any
        }
        return any
    }

    private struct Key: Hashable {
        var markdown: String
        var size: CGFloat
        var weight: Font.Weight
        var dark: Bool
        var projectHue: Int?
    }

    private static var texts: [Key: Text] = [:]

    /// `text(…)`, built once per Markdown, size and look.
    static func cachedText(_ markdown: String, size: CGFloat, weight: Font.Weight, dark: Bool, projectHue: Int?) -> Text {
        let key = Key(markdown: markdown, size: size, weight: weight, dark: dark, projectHue: projectHue)
        if let hit = texts[key] { return hit }
        let t = text(markdown, size: size, weight: weight, dark: dark, projectHue: projectHue)
        if texts.count > 2000 { texts.removeAll(keepingCapacity: true) }
        texts[key] = t
        return t
    }

    /// The Markdown with each formula masked by a placeholder, read by `Inline` as usual (bold,
    /// links, code chips), and the placeholders then replaced by the formulas.
    static func text(_ markdown: String, size: CGFloat, weight: Font.Weight, dark: Bool, projectHue: Int?) -> Text {
        let b = Array(markdown.utf8)
        let regions = MathDetect.regions(b)
        var masked = [UInt8]()
        masked.reserveCapacity(b.count)
        var at = 0
        for (n, r) in regions.enumerated() {
            masked.append(contentsOf: b[at..<r.range.lowerBound])
            masked.append(contentsOf: Array(String(placeholder(n)).utf8))
            at = r.range.upperBound
        }
        masked.append(contentsOf: b[at...])
        let attributed = Inline.parsed(String(decoding: masked, as: UTF8.self))

        var parts: [Text] = []
        for run in attributed.runs {
            let slice = attributed[run.range]
            if run.inlinePresentationIntent?.contains(.code) == true {
                parts.append(Inline.chip(String(slice.characters), size: size, dark: dark, projectHue: projectHue))
                continue
            }
            var start = slice.startIndex
            var i = slice.startIndex
            while i < slice.endIndex {
                let next = slice.characters.index(after: i)
                if let n = index(slice.characters[i]), n < regions.count {
                    if start < i { parts.append(Text(AttributedString(slice[start..<i]))) }
                    parts.append(formula(regions[n], in: b, size: size))
                    start = next
                }
                i = next
            }
            if start < slice.endIndex { parts.append(Text(AttributedString(slice[start..<slice.endIndex]))) }
        }
        return joined(parts[...])
    }

    /// `parts` as one `Text`, nested about log₂(n) deep rather than n: each `Text("\(a)\(b)")` is
    /// a level SwiftUI resolves recursively.
    private static func joined(_ parts: ArraySlice<Text>) -> Text {
        switch parts.count {
        case 0: return Text(verbatim: "")
        case 1: return parts[parts.startIndex]
        default:
            let mid = parts.startIndex + parts.count / 2
            return Text("\(joined(parts[..<mid]))\(joined(parts[mid...]))")
        }
    }

    /// A formula as it sits in the line: typeset, or its source.
    private static func formula(_ region: MathDetect.Region, in b: [UInt8], size: CGFloat) -> Text {
        let source = region.source(b)
        // Still being written: as written, until it closes.
        guard region.closed else { return Text(verbatim: source) }
        guard let drawn = MathRender.image(region.tex(b), display: false, em: size * MathRender.scale) else { return sourceChip(source, size: size) }
        return Text(Image(uiImage: drawn.image).renderingMode(.template)).baselineOffset(-drawn.baseline)
    }

    /// A formula Trek can't draw: its TeX in a quiet code chip, as `code` in an answer looks.
    static func sourceChip(_ source: String, size: CGFloat) -> Text {
        let n = FileTint.neutral
        // Word joiners after the opening delimiter and before the closing one: `$\foo$` never
        // breaks next to its dollars.
        let open = source.hasPrefix("$$") ? 2 : source.hasPrefix("$") ? 1 : 2
        let close = source.hasSuffix("$$") ? 2 : source.hasSuffix("$") ? 1 : 2
        let joined = source.count > open + close
            ? "\(source.prefix(open))\u{2060}\(source.dropFirst(open).dropLast(close))\u{2060}\(source.suffix(close))" : source
        let body = Text(verbatim: joined).font(.code(size: size * 0.86, weight: .medium)).foregroundStyle(Trek.foreground.opacity(0.92))
        return Text("\u{2009}\(body.customAttribute(ChipAttribute(fill: n.fill, edge: n.edge)))\u{2009}")
    }
}

// MARK: Maths on its own

/// Display maths: a formula of its own, centred, scrolling sideways when it's wider than the
/// column. One Trek can't draw shows its source, as code.
struct MathBlock: View {
    var tex: String
    /// As written, delimiters and all.
    var source: String
    var size: CGFloat
    @Environment(\.dynamicTypeSize) private var dynamicType

    var body: some View {
        let em = size * TextScale.factor(dynamicType) * MathRender.scale
        if let drawn = MathRender.image(tex, display: true, em: em) {
            let image = Image(uiImage: drawn.image)
                .renderingMode(.template)
                .foregroundStyle(Trek.foreground)
                .accessibilityLabel(tex.trimmingCharacters(in: .whitespacesAndNewlines))
            ViewThatFits(in: .horizontal) {
                image.frame(maxWidth: .infinity)
                ScrollView(.horizontal, showsIndicators: false) { image }
                    .scrollBounceBehavior(.basedOnSize, axes: .horizontal)
            }
            .padding(.vertical, 2)
        } else {
            ScrollView(.horizontal, showsIndicators: false) {
                Text(verbatim: source)
                    .scaledFont(size * 0.8, design: .monospaced)
                    .foregroundStyle(Trek.foreground)
                    .padding(12)
            }
            .background(Trek.surface, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(Trek.border, lineWidth: 0.5))
        }
    }
}

// MARK: Display maths in the Markdown source

nonisolated extension MathDetect {
    /// `text` with each formula that spans lines put on one (TeX reads a line break as a space),
    /// so a line-by-line Markdown reader keeps it whole: `$$` on a line of its own, then the
    /// formula, then `$$`, becomes one line that is all formula.
    static func joinLines(_ text: String) -> String {
        guard text.contains("\n"), text.contains("$") || text.contains("\\(") || text.contains("\\[") else { return text }
        let b = Array(text.utf8)
        let multi = regions(b).filter { $0.closed && b[$0.range].contains(10) }
        guard !multi.isEmpty else { return text }
        var out = [UInt8]()
        out.reserveCapacity(b.count)
        var at = 0
        for r in multi {
            out.append(contentsOf: b[at..<r.range.lowerBound])
            let tex = r.tex(b).split(whereSeparator: \.isNewline).map { $0.trimmingCharacters(in: .whitespaces) }.joined(separator: " ")
            out.append(contentsOf: r.delim.open)
            out.append(contentsOf: Array(tex.utf8))
            out.append(contentsOf: r.delim.close)
            at = r.range.upperBound
        }
        out.append(contentsOf: b[at...])
        return String(decoding: out, as: UTF8.self)
    }

    /// A part of a paragraph: text, or a display formula set on its own.
    enum Part: Equatable {
        case text(String)
        case formula(tex: String, source: String)
    }

    /// A paragraph split around the display formulas in it, when it holds any: the text before
    /// each, the formula, and the text after the last.
    static func splitDisplay(_ paragraph: String) -> [Part]? {
        guard paragraph.contains("$$") || paragraph.contains("\\[") else { return nil }
        let b = Array(paragraph.utf8)
        let display = regions(b).filter { $0.closed && $0.delim.display }
        guard !display.isEmpty else { return nil }
        var out: [Part] = []
        func text(_ r: Range<Int>) {
            let t = String(decoding: b[r], as: UTF8.self).trimmingCharacters(in: .whitespaces)
            if !t.isEmpty { out.append(.text(t)) }
        }
        var at = 0
        for r in display {
            text(at..<r.range.lowerBound)
            out.append(.formula(tex: r.tex(b), source: r.source(b)))
            at = r.range.upperBound
        }
        text(at..<b.count)
        return out
    }
}
