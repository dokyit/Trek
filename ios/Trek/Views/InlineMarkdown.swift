import SwiftUI
import UIKit

/// Inline Markdown (bold, italics, links, `code`) as one `Text`, with code spans in chips: a file or
/// folder in a chip of its type's colour with its badge, as the Mac shows paths in answers; other
/// code in a quiet neutral chip. The chips are drawn by `ChipRenderer` behind the runs that carry
/// a `ChipAttribute`, so they wrap and select with the text around them.
struct RichText: View {
    var markdown: String
    var size: CGFloat = 16
    var weight: Font.Weight = .regular
    var style: Font.TextStyle = .body
    /// Chip path-like words in plain text too ("Reading store.rs"), for activity lines.
    var autoPaths = false
    @Environment(\.colorScheme) private var scheme
    @Environment(\.projectHue) private var projectHue
    @Environment(\.dynamicTypeSize) private var dynamicType

    var body: some View {
        let s = size * TextScale.factor(dynamicType, style: style)
        Inline.text(markdown, size: s, weight: weight, dark: scheme == .dark, projectHue: projectHue, autoPaths: autoPaths)
            .font(.system(size: s, weight: weight))
            .textRenderer(ChipRenderer())
    }
}

/// Marks the runs of a chip: its wash and rim.
struct ChipAttribute: TextAttribute {
    var fill: Color
    var edge: Color
}

/// Draws a rounded chip behind each stretch of runs that carries a `ChipAttribute` (one per line
/// a chip wraps onto), then the text.
struct ChipRenderer: TextRenderer {
    func draw(layout: Text.Layout, in ctx: inout GraphicsContext) {
        for line in layout {
            var current: (chip: ChipAttribute, rect: CGRect)?
            func flush(_ ctx: inout GraphicsContext) {
                guard let c = current else { return }
                let h = c.rect.height
                let r = c.rect.insetBy(dx: -max(3, h * 0.18), dy: -1)
                let shape = RoundedRectangle(cornerRadius: min(6, h * 0.32), style: .continuous).path(in: r)
                ctx.fill(shape, with: .color(c.chip.fill))
                ctx.stroke(shape, with: .color(c.chip.edge), lineWidth: 0.75)
                current = nil
            }
            for run in line {
                if let chip = run[ChipAttribute.self] {
                    let rect = run.typographicBounds.rect
                    if let c = current, c.chip == chip {
                        current = (chip, c.rect.union(rect))
                    } else {
                        flush(&ctx)
                        current = (chip, rect)
                    }
                } else {
                    flush(&ctx)
                }
            }
            flush(&ctx)
            for run in line { ctx.draw(run) }
        }
    }
}

enum Inline {
    /// A chip's padding from its neighbours: a thin space on each side, outside the chip.
    private static let pad = "\u{2009}"

    static func text(_ markdown: String, size: CGFloat, weight: Font.Weight = .regular, dark: Bool,
                     projectHue: Int? = nil, autoPaths: Bool = false) -> Text {
        let attributed = (try? AttributedString(markdown: markdown, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace)))
            ?? AttributedString(markdown)
        var out = Text(verbatim: "")
        var empty = true
        func append(_ t: Text) {
            out = empty ? t : Text("\(out)\(t)")
            empty = false
        }
        for run in attributed.runs {
            let slice = attributed[run.range]
            let string = String(slice.characters)
            if run.inlinePresentationIntent?.contains(.code) == true {
                append(chip(string, size: size, dark: dark, projectHue: projectHue))
            } else if autoPaths {
                for (word, isPath) in words(string) {
                    if isPath { append(chip(word, size: size, dark: dark, projectHue: projectHue)) } else { append(Text(verbatim: word)) }
                }
            } else {
                append(Text(AttributedString(slice)))
            }
        }
        return out
    }

    /// `code`, or a file or folder in its type's chip.
    static func chip(_ code: String, size: CGFloat, dark: Bool, projectHue: Int?) -> Text {
        let mono = Font.system(size: size * 0.86, weight: .medium, design: .monospaced)
        guard FileType.looksLikePath(code) else {
            let n = FileTint.neutral
            let body = Text(verbatim: code).font(mono).foregroundStyle(Trek.foreground.opacity(0.92))
            return Text("\(pad)\(body.customAttribute(ChipAttribute(fill: n.fill, edge: n.edge)))\(pad)")
        }
        let tint = FileTint.of(code, projectHue: projectHue)
        let attr = ChipAttribute(fill: tint.fill, edge: tint.edge)
        let badgeSize = (size * 0.84).rounded()
        let badge = Text(Image(uiImage: BadgeImage.make(code, size: badgeSize, dark: dark)))
            .baselineOffset(-badgeSize * 0.14)
        // A narrow no-break space: the badge never wraps away from the name.
        let gap = Text(verbatim: "\u{202F}").font(mono)
        // Word joiners after slashes and hyphens: a path breaks only where it must, not at every `/`.
        let unbroken = code.replacingOccurrences(of: "/", with: "/\u{2060}").replacingOccurrences(of: "-", with: "\u{2060}-\u{2060}")
        let name = Text(verbatim: unbroken).font(mono).foregroundStyle(tint.ink)
        return Text("\(pad)\(badge.customAttribute(attr))\(gap.customAttribute(attr))\(name.customAttribute(attr))\(pad)")
    }

    /// Plain text split into words and the spaces between, with path-like words marked (trailing
    /// punctuation stays outside the chip).
    static func words(_ s: String) -> [(String, Bool)] {
        var out: [(String, Bool)] = []
        var word = ""
        func flush() {
            guard !word.isEmpty else { return }
            var core = word
            var tail = ""
            while let last = core.last, ",.:;)!?".contains(last) {
                tail.insert(last, at: tail.startIndex)
                core.removeLast()
            }
            if FileType.looksLikePath(core) {
                out.append((core, true))
                if !tail.isEmpty { out.append((tail, false)) }
            } else {
                out.append((word, false))
            }
            word = ""
        }
        for ch in s {
            if ch.isWhitespace {
                flush()
                out.append((String(ch), false))
            } else {
                word.append(ch)
            }
        }
        flush()
        return out
    }
}

/// A file type's badge as an image, to sit inline in text (`Text(Image)`). Cached per type, size
/// and theme.
enum BadgeImage {
    @MainActor private static var cache: [String: UIImage] = [:]

    @MainActor static func make(_ path: String, size: CGFloat, dark: Bool) -> UIImage {
        let type = FileType.of(path)
        let key = "\(type.mark)-\(type.fill ?? 0)-\(size)-\(dark)"
        if let hit = cache[key] { return hit }
        let neutralFill = dark ? UIColor(white: 1, alpha: 0.12) : UIColor(white: 0, alpha: 0.08)
        let neutralInk = dark ? UIColor(hex: 0x8A93A3) : UIColor(hex: 0x6B6558)
        let fill = type.fill.map(UIColor.init(hex:)) ?? neutralFill
        let ink: UIColor = type.fill == nil ? neutralInk : (type.darkMark ? UIColor(hex: 0x1F1F1F) : .white)
        let image = UIGraphicsImageRenderer(size: CGSize(width: size, height: size)).image { _ in
            let rect = CGRect(x: 0, y: 0, width: size, height: size)
            fill.setFill()
            UIBezierPath(roundedRect: rect, cornerRadius: size * 0.24).fill()
            switch type.mark {
            case .text(let label):
                let base = UIFont.systemFont(ofSize: size * (label.count > 1 ? 0.5 : 0.62), weight: .bold)
                let font = base.fontDescriptor.withDesign(.rounded).map { UIFont(descriptor: $0, size: 0) } ?? base
                let attrs: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: ink]
                let s = NSAttributedString(string: label, attributes: attrs)
                let b = s.size()
                s.draw(at: CGPoint(x: (size - b.width) / 2, y: (size - b.height) / 2))
            case .lock, .image, .folder, .file:
                let name = switch type.mark {
                case .lock: "lock.fill"
                case .image: "photo.fill"
                case .folder: "folder.fill"
                default: "doc.text"
                }
                let config = UIImage.SymbolConfiguration(pointSize: size * 0.52, weight: .semibold)
                if let symbol = UIImage(systemName: name, withConfiguration: config)?.withTintColor(ink, renderingMode: .alwaysOriginal) {
                    let s = symbol.size
                    symbol.draw(in: CGRect(x: (size - s.width) / 2, y: (size - s.height) / 2, width: s.width, height: s.height))
                }
            }
        }
        cache[key] = image
        return image
    }
}
