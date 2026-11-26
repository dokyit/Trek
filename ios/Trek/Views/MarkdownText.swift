import Synchronization
import SwiftUI

/// Agent Markdown, block by block, as the Mac's transcript reads it: headings, paragraphs, rules,
/// bullet, numbered and task lists (nested, with the number they start at), quotes, fenced code,
/// tables, display maths and pictures. Inline styling (bold, italics, links, `code` and file
/// chips) is `RichText`, with maths typeset in the line by `RichMathText`.
/// The text is parsed once and kept (`Markdown.blocks`): a transcript's answers are parsed in the
/// background as they arrive (`Markdown.prewarm`), and an answer still being written is parsed
/// only from its last settled block on.
struct MarkdownText: View, nonisolated Equatable {
    // Read by `==`, which SwiftUI may call off the main thread (as `BlockRow`'s): a conformance
    // left to the target's default isolation stops the app there.
    nonisolated(unsafe) var text: String
    nonisolated(unsafe) var size: CGFloat = 16
    /// The answer is still being written: a visualization or page whose fence is still open is
    /// on its way, not one that was never finished.
    nonisolated(unsafe) var live = false

    nonisolated static func == (a: MarkdownText, b: MarkdownText) -> Bool {
        a.size == b.size && a.live == b.live && a.text == b.text
    }

    typealias Align = Markdown.Align
    typealias Block = Markdown.Block

    var body: some View {
        MarkdownBlocks(blocks: Markdown.blocks(text), size: size, live: live)
            .frame(maxWidth: .infinity, alignment: .leading)
            // Answers, thoughts and notes are the conversation: set in the theme's face for it.
            .environment(\.trekConversation, true)
    }
}

/// Blocks one under the other: an answer's, a list item's or a quote's.
struct MarkdownBlocks: View {
    var blocks: [Markdown.Block]
    var size: CGFloat
    var live: Bool
    var spacing: CGFloat = 10

    var body: some View {
        VStack(alignment: .leading, spacing: spacing) {
            ForEach(Array(blocks.enumerated()), id: \.offset) { _, block in
                MarkdownBlockView(block: block, size: size, live: live)
            }
        }
    }
}

struct MarkdownBlockView: View {
    var block: Markdown.Block
    var size: CGFloat
    var live: Bool

    var body: some View {
        switch block {
        case .heading(let level, let s):
            RichMathText(markdown: s, size: level <= 1 ? size * 1.3 : size * 1.12, weight: .semibold)
                .padding(.top, 4)
                .accessibilityAddTraits(.isHeader)
        case .paragraph(let s):
            RichMathText(markdown: s, size: size).lineSpacing(3)
        case .rule:
            Rectangle().fill(Trek.border).frame(height: 1)
                .padding(.vertical, 4)
                .accessibilityHidden(true)
        case .list(let list):
            MarkdownListView(list: list, size: size, live: live)
        case .quote(let blocks):
            HStack(spacing: 10) {
                RoundedRectangle(cornerRadius: Trek.corner(1)).fill(Trek.border).frame(width: 3)
                MarkdownBlocks(blocks: blocks, size: size, live: live, spacing: 8)
                    .foregroundStyle(Trek.muted)
            }
        case .math(let tex, let source):
            MathBlock(tex: tex, source: source, size: size)
        case .code(let lang, let code, let closed) where Markdown.isArtifact(lang):
            // The page runs only once it's whole, and only if it's one the Mac would run.
            if !closed, live {
                LivePageBuilding(title: Markdown.artifactTitle(code), bytes: code.utf8.count, size: size)
            } else if let reason = LivePage.refusal(code, closed: closed) {
                BlockNote(what: "Couldn't run this interactive page", reason: reason, source: LivePage.shownSource(code), sourceName: "source", size: size)
            } else {
                LivePageCard(title: Markdown.artifactTitle(code), html: code, size: size)
            }
        case .code(let lang, let code, false) where live && Markdown.isStudy(lang):
            // A tutor's question on its way: its JSON isn't something to read.
            LivePageBuilding(title: "Study", bytes: code.utf8.count, size: size, interactive: false)
        case .code(let lang, let code, _):
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

/// A list: each item's marker (a dot, its number, or a task's box) beside what the item holds,
/// which may be more lists, code or pictures. A list inside an item sits under the item's text,
/// so nesting indents by itself.
struct MarkdownListView: View {
    var list: Markdown.List
    var size: CGFloat
    var live: Bool

    var body: some View {
        // A list written with blank lines between its items is set with the same air.
        VStack(alignment: .leading, spacing: list.loose ? 10 : 6) {
            ForEach(Array(list.items.enumerated()), id: \.offset) { i, item in
                HStack(alignment: .firstTextBaseline, spacing: list.ordered && item.task == nil ? 8 : 9) {
                    marker(i, item)
                    MarkdownBlocks(blocks: item.blocks, size: size, live: live, spacing: 8)
                }
            }
        }
    }

    @ViewBuilder
    private func marker(_ i: Int, _ item: Markdown.Item) -> some View {
        if let done = item.task {
            // Read-only: an answer's boxes say what's done, they don't take taps.
            Image(systemName: done ? "checkmark.square.fill" : "square")
                .scaledFont(size * 0.95)
                .foregroundStyle(done ? Trek.done : Trek.muted)
                .accessibilityLabel(done ? "done" : "to do")
        } else if list.ordered {
            Text(verbatim: "\(list.start + i).").scaledFont(size).monospacedDigit().foregroundStyle(Trek.muted)
        } else {
            Circle().fill(Trek.muted.opacity(0.8)).frame(width: 4.5, height: 4.5).alignmentGuide(.firstTextBaseline) { $0[.bottom] + 4 }
        }
    }
}

/// The Markdown parser behind `MarkdownText`, with a cache: safe off the main thread, so a
/// transcript's answers are parsed in the background before they're drawn (`prewarm`).
nonisolated enum Markdown {
    /// A fence's language says it's an interactive artifact (`trek-html`).
    static func isArtifact(_ lang: String) -> Bool {
        lang.trimmingCharacters(in: .whitespaces).lowercased() == "trek-html"
    }

    /// A fence's language says it's a tutor's question (`trek-study`, as models write it: the
    /// Mac's `study::is_question_fence`).
    static func isStudy(_ lang: String) -> Bool {
        let word = lang.split(whereSeparator: \.isWhitespace).first.map(String.init) ?? ""
        let name = word.trimmingCharacters(in: CharacterSet(charactersIn: "{}.")).lowercased().replacingOccurrences(of: "_", with: "-")
        return name == "trek-study" || name == "trekstudy"
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
        /// `---`, `***` or `___` on a line of its own.
        case rule
        case list(List)
        /// Consecutive `>` lines: one quote, holding blocks of its own.
        case quote([Block])
        /// A fenced block: what follows its fence, its text, and whether its closing fence is
        /// written (an answer still streaming, or one cut short, leaves it open).
        case code(lang: String, code: String, closed: Bool)
        case table(header: [String], align: [Align], rows: [[String]])
        /// A ```trek-viz fence: drawn natively.
        case viz(VizBlock)
        /// Display maths (`$$…$$`, `\[…\]`): its TeX, and its source as written.
        case math(tex: String, source: String)
        /// Pictures (`![shot](docs/shot.png)`), on a line of their own or under the text they
        /// were written in: drawn, as the Mac does.
        case images([MediaLink])
        /// A line that's one link to a video file (`[demo](clip.mp4)`): a player, as the Mac's.
        case video(MediaLink)

        /// The inline Markdown in it, for `RichText` (code isn't).
        var inlines: [String] {
            switch self {
            case .heading(_, let s), .paragraph(let s): [s]
            case .list(let list): list.items.flatMap { $0.blocks.flatMap(\.inlines) }
            case .quote(let blocks): blocks.flatMap(\.inlines)
            case .rule, .code, .viz, .math, .images, .video: []
            case .table(let header, _, let rows): header + rows.flatMap { $0 }
            }
        }
    }

    /// A bullet or numbered list. `start`: the number its first item was written with.
    /// `loose`: blank lines between its items, or inside one.
    struct List: Equatable, Sendable {
        var ordered: Bool
        var start = 1
        var loose = false
        var items: [Item]
    }

    /// One item: its blocks (text, and whatever is nested under it), and for a task
    /// (`- [x]`) whether it's done.
    struct Item: Equatable, Sendable {
        var task: Bool?
        var blocks: [Block]
    }

    /// An answer's blocks as kept, with where a longer version of the same text picks up: the
    /// UTF-8 offset of a line that starts a block of its own with nothing open before it, and how
    /// many blocks come before that line. Those blocks can't change, whatever is written after.
    struct Kept: Equatable, Sendable {
        var blocks: [Block]
        var settled: Settled?
    }

    struct Settled: Equatable, Sendable {
        var offset: Int
        var blocks: Int
    }

    /// Parsed answers kept: 8 MiB by what they cost, twice their text (the key, and the blocks
    /// that hold it again). That's some 400 answers of 10 KB or 2,000 of 2 KB: the threads read
    /// lately (the phone holds 80 items of a thread). It was 3,000 answers of any size, all
    /// dropped at once at 3,001. A text streaming in replaces its shorter self here.
    static let cacheBudget = 8 << 20
    /// One answer's most to be kept: 512 KB of text. A longer one is parsed each time it's drawn.
    static let cacheEntryCap = 1 << 20

    private static let cache = Mutex(ByteLRU<String, Kept>(budget: cacheBudget, entryCap: cacheEntryCap) { new, old in
        new.extends(old)
    })

    /// What `text`'s blocks cost to keep.
    static func cost(_ text: String) -> Int { text.utf8.count * 2 + 64 }

    /// `text`'s blocks, parsed once. A text that carries on one parsed just before (an answer
    /// streaming in) is parsed from that one's last settled block on, not from its start.
    static func blocks(_ text: String) -> [Block] {
        let (hit, earlier) = cache.withLock { c -> (Kept?, Kept?) in
            if let hit = c.value(for: text) { return (hit, nil) }
            return (nil, c.superseded(by: text))
        }
        if let hit { return hit.blocks }
        let kept = earlier.flatMap { carried(text, on: $0) } ?? keep(text)
        cache.withLock { _ = $0.insert(kept, for: text, cost: cost(text)) }
        return kept.blocks
    }

    /// Whether `text`'s blocks are kept (for tests).
    static func isCached(_ text: String) -> Bool { cache.withLock { $0.contains(text) } }

    /// Parses `texts` (and their inline Markdown) in the background, so they draw at once: the
    /// answers of a transcript as it arrives. A long answer is drawn in parts (`AnswerPart`), so
    /// it's parsed in those.
    static func prewarm(_ texts: [String]) {
        guard !texts.isEmpty else { return }
        Task.detached(priority: .userInitiated) {
            for text in texts {
                for part in text.utf8.count >= AnswerPart.threshold ? AnswerPart.cut(text) : [text] {
                    for block in blocks(part) {
                        for inline in block.inlines { _ = Inline.parsed(inline) }
                    }
                }
            }
        }
    }

    /// `text`'s blocks.
    static func parse(_ text: String) -> [Block] { read(text).blocks }

    /// `text` parsed whole, with where a longer version of it can pick up.
    static func keep(_ text: String) -> Kept {
        let reading = read(text)
        return Kept(blocks: reading.blocks, settled: settled(reading, in: text))
    }

    /// `text` parsed from where `earlier` (a shorter version of it) settled: the blocks before
    /// that line are kept as they are, and only the rest is read. Nil when nothing had settled.
    static func carried(_ text: String, on earlier: Kept) -> Kept? {
        guard let s = earlier.settled, s.offset > 0, s.offset <= text.utf8.count, s.blocks <= earlier.blocks.count else { return nil }
        let from = text.utf8.index(text.utf8.startIndex, offsetBy: s.offset)
        let tail = keep(String(text[from...]))
        return Kept(blocks: Array(earlier.blocks[..<s.blocks]) + tail.blocks,
                    settled: tail.settled.map { Settled(offset: s.offset + $0.offset, blocks: s.blocks + $0.blocks) } ?? s)
    }

    /// The last place in `text` its parse can be picked up from: a line after a blank line that
    /// began a block with no list, quote or fence open, and that the maths reader, too, takes to
    /// be outside a fence (it joins a formula's lines over the whole text, by its own reckoning).
    private static func settled(_ reading: Reading, in text: String) -> Settled? {
        guard !reading.restarts.isEmpty else { return nil }
        let bytes = Array(text.utf8)
        for restart in reading.restarts.reversed().prefix(4) where restart.blanks > 0 && restart.blocks > 0 {
            guard let offset = lineStart(afterBlank: restart.blanks, in: bytes), offset < bytes.count else { continue }
            if !MathDetect.endsInFence(bytes[..<offset]) { return Settled(offset: offset, blocks: restart.blocks) }
        }
        return nil
    }

    /// Where the line after `text`'s `n`th blank line starts. Blank lines are counted, not
    /// bytes, since a formula over several lines is read as one and the text shifts under it;
    /// no formula holds a blank line.
    static func lineStart(afterBlank n: Int, in bytes: [UInt8]) -> Int? {
        var seen = 0
        var start = 0
        while start <= bytes.count {
            let end = bytes[start...].firstIndex(of: 10) ?? bytes.count
            var last = end
            if last > start, bytes[last - 1] == 13 { last -= 1 }
            if bytes[start..<last].allSatisfy({ $0 == 32 || $0 == 9 }) {
                seen += 1
                if seen == n { return end < bytes.count ? end + 1 : nil }
            }
            if end >= bytes.count { break }
            start = end + 1
        }
        return nil
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
        SidewaysScroll {
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
            .clipShape(RoundedRectangle(cornerRadius: Trek.corner(12), style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: Trek.corner(12), style: .continuous).strokeBorder(Trek.border, lineWidth: 0.75))
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
