import Foundation

// The block parser behind `Markdown.parse`: CommonMark's block structure as the Mac's transcript
// reads it (`md.rs`), as far as answers use it. Rules, ATX and setext headings, lists with their
// start number, nesting, loose items and task boxes, quotes over several lines, fences that close
// only on their own kind and length, tables, display maths, and pictures wherever they're written.
// Not read: indented code (four spaces), HTML blocks and link reference definitions.

nonisolated extension Markdown {
    /// One line of the text, within the container being read: what's left of its indentation,
    /// and the text after it.
    struct Line {
        /// The line as written (no line break), from which a code block takes its text.
        var raw: Substring
        /// Where its text starts in `raw`, past the leading spaces and tabs.
        var start: Substring.Index
        /// Columns of leading whitespace left, after what the containers around it took.
        var indent: Int
        /// Columns the containers took.
        var taken = 0
        /// It carries on a paragraph from outside its container (CommonMark's lazy continuation).
        var lazy = false

        init(_ raw: Substring) {
            var raw = raw
            if raw.utf8.last == 13 { raw = raw.dropLast() }
            var col = 0
            var at = raw.utf8.startIndex
            let end = raw.utf8.endIndex
            while at < end {
                let c = raw.utf8[at]
                if c == 32 { col += 1 } else if c == 9 { col += 4 - col % 4 } else { break }
                at = raw.utf8.index(after: at)
            }
            self.raw = raw
            start = at
            indent = col
            if text.utf8.allSatisfy({ $0 == 32 || $0 == 9 || $0 == 11 || $0 == 12 }) {
                // Blank: nothing of it counts.
                start = raw.endIndex
                indent = 0
            }
        }

        /// The text after the indentation (trailing spaces kept: two of them break a line).
        var text: Substring { raw[start...] }
        var isBlank: Bool { start == raw.endIndex }

        /// The line inside a container that takes `columns` of its indentation.
        func dropping(_ columns: Int) -> Line {
            var l = self
            let n = min(columns, indent)
            l.indent -= n
            l.taken += n
            return l
        }

        /// The line as a code block shows it: as written, less the columns its containers and its
        /// fence's own indentation took (a tab cut through becomes the spaces left of it).
        func literal(dropping columns: Int) -> String {
            let total = taken + min(columns, indent)
            if total == 0 { return String(raw) }
            var col = 0
            var at = raw.utf8.startIndex
            while at < start, col < total {
                col += raw.utf8[at] == 9 ? 4 - col % 4 : 1
                at = raw.utf8.index(after: at)
            }
            return String(repeating: " ", count: max(0, col - total)) + raw[at...]
        }
    }

    /// A list item's marker: `-`, `*`, `+`, `•`, or a number and `.` or `)`.
    struct Marker {
        var ordered: Bool
        var number: Int
        /// The bullet, or the number's `.` or `)`: a list is the items that share it.
        var delimiter: UInt8
        /// Columns from the marker to the item's content.
        var width: Int
        /// The first line of the item, after the marker.
        var content: Line
    }

    /// An opening fence: its character, its length and what follows it.
    struct Fence {
        var char: UInt8
        var count: Int
        var info: String
    }

    static func fenceOpen(_ s: Substring) -> Fence? {
        guard let c = s.utf8.first, c == 96 || c == 126 else { return nil }
        let n = s.utf8.prefix { $0 == c }.count
        guard n >= 3 else { return nil }
        let info = s.dropFirst(n).trimmingCharacters(in: .whitespaces)
        // A backtick fence's info string has no backticks: that's code in a line.
        if c == 96, info.contains("`") { return nil }
        return Fence(char: c, count: n, info: info)
    }

    /// `s` closes `fence`: its character alone, at least as many of it.
    static func fenceCloses(_ s: Substring, _ fence: Fence) -> Bool {
        let n = s.utf8.prefix { $0 == fence.char }.count
        return n >= fence.count && s.utf8.dropFirst(n).allSatisfy { $0 == 32 || $0 == 9 }
    }

    /// `# Title`: one to six `#`, then a space (or nothing). `#128` is text.
    static func heading(_ s: Substring) -> (level: Int, text: String)? {
        let level = s.utf8.prefix { $0 == 35 }.count
        guard (1...6).contains(level) else { return nil }
        let rest = s.dropFirst(level)
        guard rest.isEmpty || rest.utf8.first == 32 || rest.utf8.first == 9 else { return nil }
        var text = rest.trimmingCharacters(in: .whitespaces)
        // A closing run of `#` is decoration: "## Title ##".
        let closing = text.utf8.reversed().prefix { $0 == 35 }.count
        if closing > 0 {
            let before = text.dropLast(closing)
            if before.isEmpty || before.last == " " || before.last == "\t" { text = before.trimmingCharacters(in: .whitespaces) }
        }
        return (level, text)
    }

    /// `---`, `***`, `___` (spaces between allowed): a rule.
    static func isRule(_ s: Substring) -> Bool {
        guard let c = s.utf8.first, c == 45 || c == 42 || c == 95 else { return false }
        var n = 0
        for b in s.utf8 {
            if b == c { n += 1 } else if b != 32 && b != 9 { return false }
        }
        return n >= 3
    }

    /// The line under a paragraph that makes it a heading: `===` (1) or `---` (2).
    static func setext(_ s: Substring) -> Int? {
        guard let c = s.utf8.first, c == 61 || c == 45 else { return nil }
        let n = s.utf8.prefix { $0 == c }.count
        return s.utf8.dropFirst(n).allSatisfy { $0 == 32 || $0 == 9 } ? (c == 61 ? 1 : 2) : nil
    }

    static func marker(_ line: Line) -> Marker? {
        let s = line.text
        let u = s.utf8
        guard let first = u.first else { return nil }
        var ordered = false
        var number = 0
        var delimiter = first
        var length = 1
        if first == 45 || first == 42 || first == 43 {
            // A bullet.
        } else if s.hasPrefix("•") {
            length = "•".utf8.count
            delimiter = 0
        } else if first >= 48, first <= 57 {
            let digits = u.prefix { $0 >= 48 && $0 <= 57 }.count
            guard digits <= 9, digits < u.count else { return nil }
            let d = u[u.index(u.startIndex, offsetBy: digits)]
            guard d == 46 || d == 41 else { return nil }
            ordered = true
            number = Int(s.prefix(digits)) ?? 0
            delimiter = d
            length = digits + 1
        } else {
            return nil
        }
        let after = s.dropFirst(0).utf8.dropFirst(length)
        guard after.isEmpty || after.first == 32 || after.first == 9 else { return nil }
        // The content starts after the spaces that follow the marker: up to four; with more
        // (or none, on an empty item), one.
        var spaces = 0
        var col = line.indent + line.taken + length
        var at = after.startIndex
        while at < after.endIndex, after[at] == 32 || after[at] == 9 {
            let w = after[at] == 9 ? 4 - col % 4 : 1
            spaces += w
            col += w
            at = after.index(after: at)
        }
        let blank = at == after.endIndex
        if blank || spaces > 4 {
            var content = Line(blank ? s[s.endIndex...] : s[after.index(after: after.startIndex)...])
            if !blank { content.indent = spaces - 1 }
            return Marker(ordered: ordered, number: number, delimiter: delimiter, width: length + 1, content: content)
        }
        return Marker(ordered: ordered, number: number, delimiter: delimiter, width: length + spaces, content: Line(s[at...]))
    }

    /// `line` starts a block of its own, so it can't carry on the paragraph before it.
    static func startsBlock(_ line: Line) -> Bool {
        let s = line.text
        if fenceOpen(s) != nil || heading(s) != nil || isRule(s) || s.hasPrefix(">") || s.hasPrefix("|") { return true }
        if let m = marker(line) { return interrupts(m) }
        return false
    }

    /// A list may start right under a paragraph only with a bullet or a `1.`, and with something in it.
    static func interrupts(_ m: Marker) -> Bool {
        !m.content.isBlank && (!m.ordered || m.number == 1)
    }

    /// A paragraph could be running through `line`: it isn't a heading, a rule, a table row or
    /// a fence.
    static func carriesText(_ line: Line) -> Bool {
        let s = line.text
        return !line.isBlank && fenceOpen(s) == nil && heading(s) == nil && !isRule(s) && !s.hasPrefix("|")
    }

    /// Tracks whether the lines fed to it leave a fence open.
    struct FenceState {
        var open: Fence?
        /// The last line closed a fence.
        var closedLast = false

        mutating func take(_ line: Line) {
            closedLast = false
            if let f = open {
                if fenceCloses(line.text, f) {
                    open = nil
                    closedLast = true
                }
            } else if let f = fenceOpen(line.text) {
                open = f
            }
        }
    }

    /// What a parse found: the blocks, and the places a later, longer version of the same text
    /// can pick up from.
    struct Reading {
        var blocks: [Block] = []
        /// Lines that start a block at the top level after a blank line, with nothing open: the
        /// line's number, the blank lines before it, and how many blocks were out before it.
        var restarts: [(line: Int, blanks: Int, blocks: Int)] = []
    }

    struct Parser {
        var restarts: [(line: Int, blanks: Int, blocks: Int)] = []
        /// How many quotes and list items the lines being read sit inside.
        var depth = 0
        /// Deeper than this, what's written is a paragraph: a line of a few thousand `>` would
        /// otherwise take the stack with it, and nothing nests this far on a phone's width.
        static let maxDepth = 16

        /// The blocks inside a quote or a list item.
        mutating func nested(_ lines: [Line], open: Bool) -> [Block] {
            guard depth < Self.maxDepth else {
                let text = Markdown.joined(lines)
                return text.isEmpty ? [] : [.paragraph(text)]
            }
            depth += 1
            defer { depth -= 1 }
            return blocks(lines, top: false, open: open)
        }

        /// The blocks of `lines`. `top`: these are the text's own lines (not a list item's or a
        /// quote's). `open`: the last line may still be being written (the text ends there).
        mutating func blocks(_ lines: [Line], top: Bool, open: Bool) -> [Block] {
            var out: [Block] = []
            var para: [Line] = []
            var table: [String] = []
            var blanks = 0

            func flushTable() {
                guard !table.isEmpty else { return }
                out.append(Markdown.table(table))
                table = []
            }
            func flush() {
                if !para.isEmpty {
                    out.append(contentsOf: Markdown.paragraph(para))
                    para = []
                }
                flushTable()
            }

            var i = 0
            while i < lines.count {
                let line = lines[i]
                if line.isBlank {
                    flush()
                    blanks += 1
                    i += 1
                    continue
                }
                if top, para.isEmpty, table.isEmpty, i == 0 || lines[i - 1].isBlank {
                    restarts.append((i, blanks, out.count))
                }
                let s = line.text
                let last = open && i == lines.count - 1

                if s.hasPrefix("|") {
                    if table.isEmpty { flush() }
                    table.append(s.trimmingCharacters(in: .whitespaces))
                    i += 1
                    continue
                } else if !table.isEmpty {
                    flushTable()
                }

                if let fence = fenceOpen(s) {
                    flush()
                    var body: [String] = []
                    var closed = false
                    i += 1
                    while i < lines.count {
                        if fenceCloses(lines[i].text, fence) {
                            closed = true
                            i += 1
                            break
                        }
                        body.append(lines[i].literal(dropping: line.indent))
                        i += 1
                    }
                    out.append(Markdown.fenced(fence.info, body, closed: closed))
                    continue
                }

                // A paragraph with `===` or `---` under it is a heading. Not while that line is
                // still being written: "-" may be a bullet on its way.
                if !para.isEmpty, !line.lazy, !last, let level = setext(s) {
                    let text = Markdown.joined(para)
                    para = []
                    out.append(.heading(level, text))
                    i += 1
                    continue
                }

                if let h = heading(s) {
                    flush()
                    if !h.text.isEmpty { out.append(.heading(h.level, h.text)) }
                    i += 1
                    continue
                }

                if isRule(s) {
                    flush()
                    out.append(.rule)
                    i += 1
                    continue
                }

                if s.hasPrefix(">") {
                    flush()
                    var inner: [Line] = []
                    var state = FenceState()
                    while i < lines.count {
                        let l = lines[i]
                        if l.text.hasPrefix(">") {
                            var rest = l.text.dropFirst()
                            if rest.first == " " { rest = rest.dropFirst() }
                            let quoted = Line(rest)
                            state.take(quoted)
                            inner.append(quoted)
                        } else if !l.isBlank, state.open == nil, !state.closedLast, let before = inner.last, carriesText(before), !startsBlock(l) {
                            // A quote's paragraph may run on without its `>`.
                            var lazy = Line(l.text)
                            lazy.lazy = true
                            inner.append(lazy)
                        } else {
                            break
                        }
                        i += 1
                    }
                    out.append(.quote(nested(inner, open: open && i == lines.count)))
                    continue
                }

                if let m = marker(line), para.isEmpty || interrupts(m) {
                    flush()
                    out.append(.list(list(lines, &i, first: m, open: open)))
                    continue
                }

                para.append(line)
                i += 1
            }
            flush()
            return out
        }

        /// The list that starts at `lines[i]`; leaves `i` at the first line after it.
        mutating func list(_ lines: [Line], _ i: inout Int, first: Marker, open: Bool) -> List {
            var items: [Item] = []
            var loose = false
            var marker = first
            while true {
                let base = lines[i].indent
                var body: [Line] = [marker.content]
                var state = FenceState()
                state.take(marker.content)
                var j = i + 1
                var blanks = 0
                var spaced = false
                while j < lines.count {
                    let l = lines[j]
                    if l.isBlank {
                        blanks += 1
                        j += 1
                        continue
                    }
                    // In the item: indented to its content. Two columns past the marker is taken
                    // for that too, as agents write sub-bullets and code under "1." (CommonMark
                    // wants three there, and would end the list).
                    if l.indent - base >= 2 {
                        if blanks > 0 {
                            if state.open == nil { spaced = true }
                            body.append(contentsOf: repeatElement(Line(""), count: blanks))
                            blanks = 0
                        }
                        let inside = l.dropping(base + marker.width)
                        state.take(inside)
                        body.append(inside)
                        j += 1
                        continue
                    }
                    if blanks == 0, state.open == nil, !state.closedLast, let before = body.last, carriesText(before), !siblingOrBlock(l, of: first) {
                        var lazy = Line(l.text)
                        lazy.lazy = true
                        body.append(lazy)
                        j += 1
                        continue
                    }
                    break
                }
                var task: Bool?
                if let box = Markdown.taskBox(body[0].text) {
                    task = box.done
                    body[0] = Line(box.rest)
                }
                let ends = j == lines.count && blanks == 0
                items.append(Item(task: task, blocks: nested(body, open: open && ends)))
                if spaced { loose = true }
                // Another item of this list?
                if j < lines.count, lines[j].indent - base < 2, !Markdown.isRule(lines[j].text),
                   let next = Markdown.marker(lines[j]), next.ordered == first.ordered, next.delimiter == first.delimiter {
                    if blanks > 0 { loose = true }
                    marker = next
                    i = j
                    continue
                }
                i = j - blanks
                break
            }
            return List(ordered: first.ordered, start: first.number, loose: loose, items: items)
        }

        /// `line`, right under an item's paragraph, is the list's next item or a block of its own
        /// rather than more of the paragraph.
        private func siblingOrBlock(_ line: Line, of list: Marker) -> Bool {
            if let m = Markdown.marker(line), !Markdown.isRule(line.text), m.ordered == list.ordered, m.delimiter == list.delimiter { return true }
            return Markdown.startsBlock(line)
        }
    }

    /// `[ ]` or `[x]` at the start of a list item: a task, and the text after its box.
    static func taskBox(_ s: Substring) -> (done: Bool, rest: Substring)? {
        let u = Array(s.utf8.prefix(4))
        guard u.count >= 3, u[0] == 91, u[2] == 93, u[1] == 32 || u[1] == 120 || u[1] == 88 else { return nil }
        guard u.count == 3 || u[3] == 32 || u[3] == 9 else { return nil }
        return (u[1] != 32, s.dropFirst(3).drop { $0 == " " || $0 == "\t" })
    }

    /// A paragraph's lines as one text: a space where a line ends, a line break where it ends
    /// in two spaces or a backslash.
    static func joined(_ lines: [Line]) -> String {
        var out = ""
        for (n, line) in lines.enumerated() {
            var s = line.text
            let isLast = n == lines.count - 1
            var hard = false
            if !isLast {
                if s.hasSuffix("  ") {
                    hard = true
                } else if s.hasSuffix("\\"), !s.hasSuffix("\\\\") {
                    hard = true
                    s = s.dropLast()
                }
            }
            out += s.trimmingCharacters(in: .whitespaces)
            if !isLast { out += hard ? "\n" : " " }
        }
        return out
    }

    /// A paragraph's blocks: its text, with display maths set on its own and its pictures drawn
    /// under the text they follow; a line that is one link to a video is the video.
    static func paragraph(_ lines: [Line]) -> [Block] {
        let text = joined(lines)
        var out: [Block] = []
        func add(_ t: String) { out.append(.paragraph(t)) }
        // Display maths in a paragraph is set on its own, between the text around it.
        if let parts = MathDetect.splitDisplay(text) {
            for part in parts {
                switch part {
                case .text(let t): add(t)
                case .formula(let tex, let source): out.append(.math(tex: tex, source: source))
                }
            }
        } else if lines.count == 1, let pictures = MediaLink.images(text) {
            out.append(.images(pictures))
        } else if lines.count == 1, let video = MediaLink.video(text) {
            out.append(.video(video))
        } else if lines.count > 1, let last = lines.last, let pictures = MediaLink.images(joined([last])) {
            // Pictures on the line after some text, with no blank line between: drawn under it
            // (only their alt text showed before).
            add(joined(Array(lines.dropLast())))
            out.append(.images(pictures))
        } else {
            add(text)
        }
        return out
    }

    /// A fenced block as it ends: a ```trek-viz one is a visualization (`closed`: its fence is).
    static func fenced(_ info: String, _ lines: [String], closed: Bool) -> Block {
        let body = lines.joined(separator: "\n")
        return info.split(separator: " ").first == "trek-viz"
            ? .viz(VizBlock.make(payload: body, closed: closed))
            : .code(lang: info, code: body, closed: closed)
    }

    /// Lines that start with a pipe: a table when the second is its `|---|` line, else text.
    static func table(_ lines: [String]) -> Block {
        guard lines.count >= 2, isSeparator(lines[1]) else {
            // Pipes, but not a table: keep the lines as text.
            return .paragraph(lines.joined(separator: " "))
        }
        let header = cells(lines[0])
        let align = cells(lines[1]).map { c -> Align in
            let c = c.trimmingCharacters(in: .whitespaces)
            if c.hasPrefix(":") && c.hasSuffix(":") { return .center }
            return c.hasSuffix(":") ? .trailing : .leading
        }
        let width = header.count
        let rows = lines.dropFirst(2).map { line -> [String] in
            let c = cells(line)
            return Array((c + Array(repeating: "", count: max(0, width - c.count))).prefix(width))
        }
        return .table(header: header, align: Array((align + Array(repeating: .leading, count: width)).prefix(width)), rows: rows)
    }

    /// `text`'s blocks and where a longer version of it can pick up.
    static func read(_ text: String) -> Reading {
        // A formula over several lines is read as one line: `$$`, the formula, `$$` stays whole.
        // Split on the line feed's byte: "\r\n" is one Character, which a split on "\n" never
        // finds (a PC's text came out as a single paragraph); `Line` drops the "\r".
        let lines = MathDetect.joinLines(text).utf8.split(separator: 10, omittingEmptySubsequences: false).map { Line(Substring($0)) }
        var parser = Parser()
        let blocks = parser.blocks(lines, top: true, open: true)
        // A restart counts only once its line is whole: the text has gone on to another.
        return Reading(blocks: blocks, restarts: parser.restarts.filter { $0.line < lines.count - 1 })
    }
}
