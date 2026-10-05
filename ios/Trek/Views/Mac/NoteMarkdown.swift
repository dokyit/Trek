import SwiftUI

/// A note as it reads, line by line as the Mac's preview draws it: headings, bullets, numbered
/// and check lists, quotes, code, and inline bold, italics, underline, strikethrough, `code`,
/// text colour and highlights (`<span style="color: …">`, `<mark>`, `<u>`). Ticking a box
/// changes that line in the note.
struct NoteMarkdown: View {
    var text: String
    var size: CGFloat = 17
    /// A check box was tapped: the line it's on (0-based).
    var toggle: ((Int) -> Void)? = nil
    @Environment(\.dynamicTypeSize) private var dynamicType

    var body: some View {
        let s = size * TextScale.factor(dynamicType)
        VStack(alignment: .leading, spacing: 7) {
            ForEach(NoteLine.parse(text)) { line in
                view(line, s)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    @ViewBuilder
    private func view(_ line: NoteLine, _ s: CGFloat) -> some View {
        switch line.kind {
        case .blank:
            Color.clear.frame(height: s * 0.2)
        case .heading(let level):
            inline(line.text, s * (level == 1 ? 1.4 : level == 2 ? 1.2 : 1.08), weight: .bold)
                .padding(.top, level == 1 ? 4 : 2)
        case .paragraph:
            inline(line.text, s)
        case .bullet(let indent):
            HStack(alignment: .firstTextBaseline, spacing: 9) {
                Circle().fill(Trek.foreground.opacity(0.55)).frame(width: 5, height: 5)
                    .alignmentGuide(.firstTextBaseline) { $0[.bottom] + s * 0.28 }
                inline(line.text, s)
            }
            .padding(.leading, CGFloat(indent) * 18 + 4)
        case .numbered(let n, let indent):
            HStack(alignment: .firstTextBaseline, spacing: 7) {
                Text("\(n).").font(.system(size: s)).monospacedDigit().foregroundStyle(Trek.muted)
                inline(line.text, s)
            }
            .padding(.leading, CGFloat(indent) * 18)
        case .check(let done, let indent):
            HStack(alignment: .firstTextBaseline, spacing: 9) {
                Button {
                    toggle?(line.index)
                } label: {
                    Image(systemName: done ? "checkmark.circle.fill" : "circle")
                        .font(.system(size: s * 1.05, weight: .regular))
                        .foregroundStyle(done ? Trek.done : Trek.muted)
                }
                .buttonStyle(.plain)
                .disabled(toggle == nil)
                .accessibilityLabel(done ? "Done" : "Not done")
                inline(line.text, s)
                    .strikethrough(done, color: Trek.muted)
                    .foregroundStyle(done ? Trek.muted : Trek.foreground)
            }
            .padding(.leading, CGFloat(indent) * 18)
        case .quote:
            HStack(spacing: 10) {
                RoundedRectangle(cornerRadius: 1.5).fill(Trek.foreground.opacity(0.2)).frame(width: 3)
                inline(line.text, s).foregroundStyle(Trek.muted)
            }
        case .code:
            ScrollView(.horizontal, showsIndicators: false) {
                Text(line.text)
                    .font(.system(size: s * 0.8, design: .monospaced))
                    .padding(12)
            }
            .background(Trek.foreground.opacity(0.05), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        case .rule:
            Divider().padding(.vertical, 4)
        }
    }

    private func inline(_ text: String, _ size: CGFloat, weight: Font.Weight = .regular) -> some View {
        Text(NoteInline.attributed(text))
            .font(.system(size: size, weight: weight))
            .fixedSize(horizontal: false, vertical: true)
            .lineSpacing(2)
    }
}

/// A line of a note, as the preview draws it.
struct NoteLine: Identifiable {
    enum Kind: Equatable {
        case blank, paragraph, quote, code, rule
        case heading(Int)
        case bullet(indent: Int)
        case numbered(Int, indent: Int)
        case check(Bool, indent: Int)
    }

    /// The first line it came from (0-based), for ticking boxes.
    var index: Int
    var kind: Kind
    var text: String
    var id: Int { index }

    static func parse(_ body: String) -> [NoteLine] {
        var out: [NoteLine] = []
        let lines = body.components(separatedBy: "\n")
        var fence: (start: Int, lines: [String])?
        for (i, raw) in lines.enumerated() {
            let trimmed = raw.trimmingCharacters(in: .whitespaces)
            if trimmed.hasPrefix("```") {
                if let f = fence {
                    out.append(NoteLine(index: f.start, kind: .code, text: f.lines.joined(separator: "\n")))
                    fence = nil
                } else {
                    fence = (i, [])
                }
                continue
            }
            if fence != nil { fence?.lines.append(raw); continue }
            let indent = raw.prefix { $0 == " " || $0 == "\t" }.reduce(0) { $0 + ($1 == "\t" ? 2 : 1) } / 2
            out.append(line(i, trimmed, indent: indent))
        }
        if let f = fence { out.append(NoteLine(index: f.start, kind: .code, text: f.lines.joined(separator: "\n"))) }
        return out
    }

    private static func line(_ i: Int, _ s: String, indent: Int) -> NoteLine {
        if s.isEmpty { return NoteLine(index: i, kind: .blank, text: "") }
        if s == "---" || s == "***" { return NoteLine(index: i, kind: .rule, text: "") }
        if let m = s.wholeMatch(of: /(#{1,6})\s+(.*)/) { return NoteLine(index: i, kind: .heading(m.1.count), text: String(m.2)) }
        if let m = s.wholeMatch(of: /[-*+]\s+\[([ xX])\]\s?(.*)/) {
            return NoteLine(index: i, kind: .check(m.1 != " ", indent: indent), text: String(m.2))
        }
        if let m = s.wholeMatch(of: /[-*+]\s+(.*)/) { return NoteLine(index: i, kind: .bullet(indent: indent), text: String(m.1)) }
        if let m = s.wholeMatch(of: /(\d+)[.)]\s+(.*)/) {
            return NoteLine(index: i, kind: .numbered(Int(m.1) ?? 1, indent: indent), text: String(m.2))
        }
        if let m = s.wholeMatch(of: />\s?(.*)/) { return NoteLine(index: i, kind: .quote, text: String(m.1)) }
        return NoteLine(index: i, kind: .paragraph, text: s)
    }
}

/// Inline markdown with the HTML the Mac's notes use for colour, highlights and underline.
enum NoteInline {
    private enum Style {
        case color(Color?)
        case mark(Color)
        case underline
    }

    /// The tags are swapped for private-use characters before the markdown is read, so emphasis
    /// may run across them; the characters are then taken out again, styling what's between.
    static func attributed(_ line: String) -> AttributedString {
        var styles: [Style] = []
        var source = ""
        var rest = Substring(line)
        let open = Character(UnicodeScalar(0xE000)!)
        let close = Character(UnicodeScalar(0xE001)!)
        while let m = rest.firstMatch(of: /<(\/?)(span|mark|u)((?:\s[^>]*)?)>/.ignoresCase()) {
            source += rest[..<m.range.lowerBound]
            if m.1.isEmpty {
                styles.append(style(tag: m.2.lowercased(), attributes: String(m.3)))
                // The style's number rides along as the private-use character after the marker.
                source.append(open)
                source.append(Character(UnicodeScalar(0xE100 + styles.count - 1)!))
            } else {
                source.append(close)
            }
            rest = rest[m.range.upperBound...]
        }
        source += rest
        let parsed = (try? AttributedString(markdown: source, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace)))
            ?? AttributedString(source)
        guard !styles.isEmpty else { return parsed }

        var out = AttributedString()
        var stack: [Style] = []
        var expectStyle = false
        for run in parsed.runs {
            var chunk = ""
            func flush() {
                guard !chunk.isEmpty else { return }
                var piece = AttributedString(chunk)
                piece.mergeAttributes(run.attributes)
                for s in stack {
                    switch s {
                    case .color(let c): if let c { piece.foregroundColor = c }
                    case .mark(let c): piece.backgroundColor = c
                    case .underline: piece.underlineStyle = .single
                    }
                }
                out += piece
                chunk = ""
            }
            for ch in parsed[run.range].characters {
                if expectStyle, let v = ch.unicodeScalars.first?.value, v >= 0xE100, Int(v - 0xE100) < styles.count {
                    stack.append(styles[Int(v - 0xE100)])
                    expectStyle = false
                } else if ch == open {
                    flush()
                    expectStyle = true
                } else if ch == close {
                    flush()
                    if !stack.isEmpty { stack.removeLast() }
                } else {
                    chunk.append(ch)
                }
            }
            flush()
        }
        return out
    }

    private static func style(tag: String, attributes: String) -> Style {
        switch tag {
        case "u": return .underline
        case "mark":
            let css = value(of: "background(?:-color)?", in: attributes)
            return .mark(css.flatMap(color) ?? Color(hex: 0xFACC15, alpha: 0.32))
        default:
            return .color(value(of: "color", in: attributes).flatMap(color))
        }
    }

    /// A CSS property's value in a `style="…"` attribute.
    private static func value(of property: String, in attributes: String) -> String? {
        guard let regex = try? Regex("(?:^|[\\s;\"'])\(property)\\s*:\\s*([^;\"']+)").ignoresCase(),
              let m = attributes.firstMatch(of: regex), let v = m.output[1].substring else { return nil }
        return v.trimmingCharacters(in: .whitespaces)
    }

    /// `#rgb`, `#rrggbb`, `#rrggbbaa`, or one of the Mac's colour names.
    static func color(_ css: String) -> Color? {
        let named: [String: UInt32] = ["red": 0xEF4444, "orange": 0xF97316, "yellow": 0xEAB308, "green": 0x22C55E,
                                       "teal": 0x14B8A6, "blue": 0x3B82F6, "purple": 0xA855F7, "pink": 0xEC4899, "gray": 0x9CA3AF, "grey": 0x9CA3AF]
        let s = css.lowercased()
        if let hex = named[s] { return Color(hex: hex) }
        guard s.hasPrefix("#") else { return nil }
        var digits = String(s.dropFirst())
        if digits.count == 3 || digits.count == 4 { digits = digits.map { "\($0)\($0)" }.joined() }
        guard let v = UInt64(digits, radix: 16) else { return nil }
        switch digits.count {
        case 6: return Color(hex: UInt32(v))
        case 8: return Color(hex: UInt32(v >> 8), alpha: Double(v & 0xFF) / 255)
        default: return nil
        }
    }
}
