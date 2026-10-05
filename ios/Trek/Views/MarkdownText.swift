import SwiftUI

/// Agent Markdown, block by block: headings, paragraphs, bullet and numbered lists, quotes and
/// fenced code. Inline styling (bold, italics, `code`, links) comes from `AttributedString`.
struct MarkdownText: View {
    var text: String
    var size: CGFloat = 16

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            ForEach(Array(Self.blocks(text).enumerated()), id: \.offset) { _, block in
                view(for: block)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    enum Block: Equatable {
        case heading(Int, String)
        case paragraph(String)
        case bullet([String])
        case numbered([String])
        case quote(String)
        case code(String, String)
    }

    @ViewBuilder
    private func view(for block: Block) -> some View {
        switch block {
        case .heading(let level, let s):
            Text(Self.inline(s))
                .font(.system(size: level <= 1 ? size * 1.3 : size * 1.12, weight: .semibold))
                .padding(.top, 4)
        case .paragraph(let s):
            Text(Self.inline(s)).font(.system(size: size)).lineSpacing(3)
        case .bullet(let items):
            VStack(alignment: .leading, spacing: 6) {
                ForEach(Array(items.enumerated()), id: \.offset) { _, item in
                    HStack(alignment: .firstTextBaseline, spacing: 9) {
                        Circle().fill(Trek.muted.opacity(0.8)).frame(width: 4.5, height: 4.5).alignmentGuide(.firstTextBaseline) { $0[.bottom] + 4 }
                        Text(Self.inline(item)).font(.system(size: size)).lineSpacing(3)
                    }
                }
            }
        case .numbered(let items):
            VStack(alignment: .leading, spacing: 6) {
                ForEach(Array(items.enumerated()), id: \.offset) { i, item in
                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                        Text("\(i + 1).").font(.system(size: size).monospacedDigit()).foregroundStyle(Trek.muted)
                        Text(Self.inline(item)).font(.system(size: size)).lineSpacing(3)
                    }
                }
            }
        case .quote(let s):
            HStack(spacing: 10) {
                RoundedRectangle(cornerRadius: 1).fill(Trek.border).frame(width: 3)
                Text(Self.inline(s)).font(.system(size: size)).foregroundStyle(Trek.muted)
            }
        case .code(_, let code):
            ScrollView(.horizontal, showsIndicators: false) {
                Text(code)
                    .font(.system(size: size * 0.8, design: .monospaced))
                    .foregroundStyle(Trek.foreground)
                    .padding(12)
            }
            .background(Trek.surface, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(Trek.border, lineWidth: 0.5))
        }
    }

    static func inline(_ s: String) -> AttributedString {
        var out = (try? AttributedString(markdown: s, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace)))
            ?? AttributedString(s)
        for run in out.runs {
            if run.inlinePresentationIntent?.contains(.code) == true {
                out[run.range].font = .system(size: 14, design: .monospaced)
                out[run.range].backgroundColor = Trek.foreground.opacity(0.07)
            }
        }
        return out
    }

    static func blocks(_ text: String) -> [Block] {
        var out: [Block] = []
        var para: [String] = []
        var bullets: [String] = []
        var numbers: [String] = []
        var code: [String]? = nil
        var lang = ""

        func flush() {
            if !para.isEmpty { out.append(.paragraph(para.joined(separator: " "))); para = [] }
            if !bullets.isEmpty { out.append(.bullet(bullets)); bullets = [] }
            if !numbers.isEmpty { out.append(.numbered(numbers)); numbers = [] }
        }

        for raw in text.components(separatedBy: "\n") {
            let line = raw.trimmingCharacters(in: .whitespaces)
            if var c = code {
                if line.hasPrefix("```") {
                    out.append(.code(lang, c.joined(separator: "\n")))
                    code = nil
                } else {
                    c.append(raw)
                    code = c
                }
                continue
            }
            if line.hasPrefix("```") {
                flush()
                lang = String(line.dropFirst(3))
                code = []
            } else if line.isEmpty {
                flush()
            } else if line.hasPrefix("#") {
                flush()
                let level = line.prefix { $0 == "#" }.count
                out.append(.heading(level, line.dropFirst(level).trimmingCharacters(in: .whitespaces)))
            } else if line.hasPrefix("- ") || line.hasPrefix("* ") || line.hasPrefix("• ") {
                if !para.isEmpty || !numbers.isEmpty { flush() }
                bullets.append(String(line.dropFirst(2)))
            } else if let dot = line.firstIndex(of: "."), line[..<dot].allSatisfy(\.isNumber), !line[..<dot].isEmpty,
                      line[line.index(after: dot)...].hasPrefix(" ") {
                if !para.isEmpty || !bullets.isEmpty { flush() }
                numbers.append(line[line.index(after: dot)...].trimmingCharacters(in: .whitespaces))
            } else if line.hasPrefix(">") {
                flush()
                out.append(.quote(line.dropFirst().trimmingCharacters(in: .whitespaces)))
            } else {
                if !bullets.isEmpty || !numbers.isEmpty { flush() }
                para.append(line)
            }
        }
        if let c = code { out.append(.code(lang, c.joined(separator: "\n"))) }
        flush()
        return out
    }
}
