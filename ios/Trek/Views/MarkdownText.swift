import Synchronization
import SwiftUI

/// Agent Markdown, block by block: headings, paragraphs, bullet and numbered lists, quotes, fenced
/// code, tables and display maths. Inline styling (bold, italics, links, `code` and file chips) is
/// `RichText`, with maths typeset in the line by `RichMathText`.
/// The text is parsed once (and may already be, in the background, as the transcript arrived).
struct MarkdownText: View, Equatable {
    var text: String
    var size: CGFloat = 16
    /// The answer is still being written: a visualization whose fence is still open is on its
    /// way, not one that was never finished.
    var live = false

    typealias Align = Markdown.Align
    typealias Block = Markdown.Block

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            ForEach(Array(Markdown.blocks(text).enumerated()), id: \.offset) { _, block in
                view(for: block)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    @ViewBuilder
    private func view(for block: Block) -> some View {
        switch block {
        case .heading(let level, let s):
            RichMathText(markdown: s, size: level <= 1 ? size * 1.3 : size * 1.12, weight: .semibold)
                .padding(.top, 4)
        case .paragraph(let s):
            RichMathText(markdown: s, size: size).lineSpacing(3)
        case .bullet(let items):
            VStack(alignment: .leading, spacing: 6) {
                ForEach(Array(items.enumerated()), id: \.offset) { _, item in
                    HStack(alignment: .firstTextBaseline, spacing: 9) {
                        Circle().fill(Trek.muted.opacity(0.8)).frame(width: 4.5, height: 4.5).alignmentGuide(.firstTextBaseline) { $0[.bottom] + 4 }
                        RichMathText(markdown: item, size: size).lineSpacing(3)
                    }
                }
            }
        case .numbered(let items):
            VStack(alignment: .leading, spacing: 6) {
                ForEach(Array(items.enumerated()), id: \.offset) { i, item in
                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                        Text("\(i + 1).").scaledFont(size).monospacedDigit().foregroundStyle(Trek.muted)
                        RichMathText(markdown: item, size: size).lineSpacing(3)
                    }
                }
            }
        case .quote(let s):
            HStack(spacing: 10) {
                RoundedRectangle(cornerRadius: 1).fill(Trek.border).frame(width: 3)
                RichMathText(markdown: s, size: size).foregroundStyle(Trek.muted)
            }
        case .math(let tex, let source):
            MathBlock(tex: tex, source: source, size: size)
        case .code(let lang, let code) where Markdown.isArtifact(lang):
            LivePageCard(title: Markdown.artifactTitle(code), html: code, size: size)
        case .code(let lang, let code):
            CodeBlockView(lang: lang, code: code, size: size)
        case .table(let header, let align, let rows):
            MarkdownTable(header: header, align: align, rows: rows, size: size)
        case .viz(let block):
            VizBlockView(block: block, live: live, size: size)
        case .images(let pictures):
            AnswerImages(pictures: pictures, size: size)
        case .video(let link):
            AnswerVideo(link: link, size: size)
        }
    }

}

/// The Markdown parser behind `MarkdownText`, with a cache: safe off the main thread, so a
/// transcript's answers can be parsed in the background before they're drawn.
nonisolated enum Markdown {
    /// A fence's language says it's an interactive artifact (`trek-html`).
    static func isArtifact(_ lang: String) -> Bool {
        lang.trimmingCharacters(in: .whitespaces).lowercased() == "trek-html"
    }

    /// An artifact's title, as the Mac shows it: its `<title>`, else its first `<h1>`–`<h3>`, as
    /// plain text.
    static func artifactTitle(_ html: String) -> String? {
        for tag in ["title", "h1", "h2", "h3"] {
            guard let open = html.range(of: "<\(tag)", options: .caseInsensitive),
                  let start = html.range(of: ">", range: open.upperBound..<html.endIndex),
                  let close = html.range(of: "</\(tag)", options: .caseInsensitive, range: start.upperBound..<html.endIndex)
            else { continue }
            var text = String(html[start.upperBound..<close.lowerBound])
            text = text.replacingOccurrences(of: "<[^>]*>", with: " ", options: .regularExpression)
            for (entity, char) in [("&amp;", "&"), ("&lt;", "<"), ("&gt;", ">"), ("&quot;", "\""), ("&#39;", "'"), ("&nbsp;", " ")] {
                text = text.replacingOccurrences(of: entity, with: char)
            }
            text = text.split(whereSeparator: \.isWhitespace).joined(separator: " ")
            if !text.isEmpty { return text.count > 80 ? String(text.prefix(79)) + "…" : text }
        }
        return nil
    }

    enum Align: Equatable, Sendable { case leading, center, trailing }

    enum Block: Equatable, Sendable {
        case heading(Int, String)
        case paragraph(String)
        case bullet([String])
        case numbered([String])
        case quote(String)
        case code(String, String)
        case table(header: [String], align: [Align], rows: [[String]])
        /// A ```trek-viz fence: drawn natively.
        case viz(VizBlock)
        /// Display maths (`$$…$$`, `\[…\]`): its TeX, and its source as written.
        case math(tex: String, source: String)
        /// A line of nothing but pictures (`![shot](docs/shot.png)`): drawn, as the Mac does.
        case images([MediaLink])
        /// A line that's one link to a video file (`[demo](clip.mp4)`): a player, as the Mac's.
        case video(MediaLink)

        /// The inline Markdown in it, for `RichText` (code isn't).
        var inlines: [String] {
            switch self {
            case .heading(_, let s), .paragraph(let s), .quote(let s): [s]
            case .bullet(let items), .numbered(let items): items
            case .code, .viz, .math, .images, .video: []
            case .table(let header, _, let rows): header + rows.flatMap { $0 }
            }
        }
    }

    private static let cache = Mutex<[String: [Block]]>([:])

    /// `text`'s blocks, parsed once.
    static func blocks(_ text: String) -> [Block] {
        if let hit = cache.withLock({ $0[text] }) { return hit }
        let parsed = parse(text)
        cache.withLock { c in
            if c.count > 3000 { c.removeAll(keepingCapacity: true) }
            c[text] = parsed
        }
        return parsed
    }

    /// Parses `texts` (and their inline Markdown) in the background, so they draw at once.
    static func prewarm(_ texts: [String]) {
        guard !texts.isEmpty else { return }
        Task.detached(priority: .userInitiated) {
            for text in texts {
                for block in blocks(text) {
                    for inline in block.inlines { _ = Inline.parsed(inline) }
                }
            }
        }
    }

    static func parse(_ text: String) -> [Block] {
        var out: [Block] = []
        var para: [String] = []
        var bullets: [String] = []
        var numbers: [String] = []
        var table: [String] = []
        var code: [String]? = nil
        var lang = ""

        /// A fenced block as it ends: a ```trek-viz one is a visualization (`closed`: its fence is).
        func fence(_ lines: [String], closed: Bool) -> Block {
            lang.split(separator: " ").first == "trek-viz"
                ? .viz(VizBlock.make(payload: lines.joined(separator: "\n"), closed: closed))
                : .code(lang, lines.joined(separator: "\n"))
        }

        func flushTable() {
            guard !table.isEmpty else { return }
            if table.count >= 2, isSeparator(table[1]) {
                let header = cells(table[0])
                let align = cells(table[1]).map { c -> Align in
                    let c = c.trimmingCharacters(in: .whitespaces)
                    if c.hasPrefix(":") && c.hasSuffix(":") { return .center }
                    return c.hasSuffix(":") ? .trailing : .leading
                }
                let width = header.count
                let rows = table.dropFirst(2).map { line -> [String] in
                    let c = cells(line)
                    return Array((c + Array(repeating: "", count: max(0, width - c.count))).prefix(width))
                }
                out.append(.table(header: header, align: Array((align + Array(repeating: .leading, count: width)).prefix(width)), rows: rows))
            } else {
                // Pipes, but not a table: keep the lines as text.
                out.append(.paragraph(table.joined(separator: " ")))
            }
            table = []
        }

        func flush() {
            if !para.isEmpty {
                let text = para.joined(separator: " ")
                // Display maths in a paragraph is set on its own, between the text around it.
                if let parts = MathDetect.splitDisplay(text) {
                    for part in parts {
                        switch part {
                        case .text(let t): out.append(.paragraph(t))
                        case .formula(let tex, let source): out.append(.math(tex: tex, source: source))
                        }
                    }
                } else if para.count == 1, let pictures = MediaLink.images(text) {
                    out.append(.images(pictures))
                } else if para.count == 1, let video = MediaLink.video(text) {
                    out.append(.video(video))
                } else {
                    out.append(.paragraph(text))
                }
                para = []
            }
            if !bullets.isEmpty { out.append(.bullet(bullets)); bullets = [] }
            if !numbers.isEmpty { out.append(.numbered(numbers)); numbers = [] }
            flushTable()
        }

        // A formula over several lines is read as one line: `$$`, the formula, `$$` stays whole.
        for raw in MathDetect.joinLines(text).components(separatedBy: "\n") {
            let line = raw.trimmingCharacters(in: .whitespaces)
            if var c = code {
                if line.hasPrefix("```") {
                    out.append(fence(c, closed: true))
                    code = nil
                } else {
                    c.append(raw)
                    code = c
                }
                continue
            }
            if line.hasPrefix("|") {
                if table.isEmpty { flush() }
                table.append(line)
                continue
            } else if !table.isEmpty {
                flushTable()
            }
            if line.hasPrefix("```") {
                flush()
                lang = line.dropFirst(3).trimmingCharacters(in: .whitespaces)
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
        if let c = code { out.append(fence(c, closed: false)) }
        flush()
        return out
    }

    /// `|---|:--:|`: the line under a table's header.
    static func isSeparator(_ line: String) -> Bool {
        line.contains("-") && line.allSatisfy { "|-: \t".contains($0) }
    }

    /// A table row's cells: split on pipes outside `code`, `\|` kept as a pipe.
    static func cells(_ line: String) -> [String] {
        var s = line.trimmingCharacters(in: .whitespaces)
        if s.hasPrefix("|") { s.removeFirst() }
        if s.hasSuffix("|") && !s.hasSuffix("\\|") { s.removeLast() }
        var out: [String] = []
        var cell = ""
        var inCode = false
        var escaped = false
        for ch in s {
            if escaped {
                cell.append(ch)
                escaped = false
            } else if ch == "\\" {
                escaped = true
            } else if ch == "`" {
                inCode.toggle()
                cell.append(ch)
            } else if ch == "|" && !inCode {
                out.append(cell.trimmingCharacters(in: .whitespaces))
                cell = ""
            } else {
                cell.append(ch)
            }
        }
        if escaped { cell.append("\\") }
        out.append(cell.trimmingCharacters(in: .whitespaces))
        return out
    }
}

/// A Markdown table: a header row, aligned columns, hairline rules, scrolling sideways when it's
/// wider than the screen. Cells keep their inline styling and chips.
struct MarkdownTable: View {
    var header: [String]
    var align: [MarkdownText.Align]
    var rows: [[String]]
    var size: CGFloat

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            Grid(alignment: .topLeading, horizontalSpacing: 0, verticalSpacing: 0) {
                GridRow {
                    ForEach(header.indices, id: \.self) { c in
                        cell(header[c], column: c, row: -1)
                            .gridColumnAlignment(horizontal(align[c]))
                    }
                }
                .background(Trek.foreground.opacity(0.045))
                ForEach(rows.indices, id: \.self) { r in
                    GridRow {
                        ForEach(header.indices, id: \.self) { c in
                            cell(rows[r][c], column: c, row: r)
                        }
                    }
                }
            }
            .fixedSize()
            .background(Trek.surface.opacity(0.6))
            .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(Trek.border, lineWidth: 0.75))
            .padding(.vertical, 1)
        }
        .scrollBounceBehavior(.basedOnSize, axes: .horizontal)
        .scrollClipDisabled()
    }

    private func cell(_ text: String, column c: Int, row r: Int) -> some View {
        CapWidth(cap: 260) {
            RichMathText(markdown: text, size: size * 0.88, weight: r < 0 ? .semibold : .regular, style: .subheadline)
                .multilineTextAlignment(textAlignment(align[c]))
                .lineSpacing(2)
        }
        .padding(.horizontal, 11)
        .padding(.vertical, 8)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: Alignment(horizontal: horizontal(align[c]), vertical: .top))
        .overlay(alignment: .bottom) {
            if r < rows.count - 1 { Rectangle().fill(Trek.border).frame(height: r < 0 ? 0.75 : 0.5) }
        }
        .overlay(alignment: .trailing) {
            if c < header.count - 1 { Rectangle().fill(Trek.border.opacity(0.8)).frame(width: 0.5) }
        }
    }

    private func horizontal(_ a: MarkdownText.Align) -> HorizontalAlignment {
        switch a {
        case .leading: .leading
        case .center: .center
        case .trailing: .trailing
        }
    }

    private func textAlignment(_ a: MarkdownText.Align) -> TextAlignment {
        switch a {
        case .leading: .leading
        case .center: .center
        case .trailing: .trailing
        }
    }
}

/// Lays its content out at its natural width, but no wider than `cap`: long cells wrap, short ones
/// stay snug, even with no width on offer (inside a sideways scroll view).
private struct CapWidth: Layout {
    var cap: CGFloat

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        guard let child = subviews.first else { return .zero }
        let ideal = child.sizeThatFits(.unspecified)
        let width = min(ideal.width, cap)
        return child.sizeThatFits(ProposedViewSize(width: width, height: nil))
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        subviews.first?.place(at: bounds.origin, proposal: ProposedViewSize(width: bounds.width, height: bounds.height))
    }
}
