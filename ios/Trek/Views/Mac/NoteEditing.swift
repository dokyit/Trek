import Foundation

/// The Mac's note formatting, done to markdown text and a selection (`trek_core::notes`): wrap
/// the selection in markers, turn lines into a list or a heading, carry a list on at a new line.
/// Offsets are in characters. Whatever the phone can't edit stays in the text as it is.
nonisolated enum NoteEditing {
    struct Edit: Equatable {
        var text: String
        var selection: Range<Int>
    }

    enum Block: Equatable {
        case heading(Int), bullets, numbers, checklist, quote

        var prefix: String {
            switch self {
            case .heading(let n): String(repeating: "#", count: n) + " "
            case .bullets: "- "
            case .numbers: "1. "
            case .checklist: "- [ ] "
            case .quote: "> "
            }
        }
    }

    /// Text colours and highlights a note can use, as the Mac offers them: name, text colour,
    /// highlight.
    static let colors: [(name: String, text: String, mark: String)] = [
        ("Red", "#ef4444", "#ef444440"), ("Orange", "#f97316", "#f9731640"), ("Yellow", "#eab308", "#facc1550"),
        ("Green", "#22c55e", "#22c55e40"), ("Teal", "#14b8a6", "#14b8a640"), ("Blue", "#3b82f6", "#3b82f640"),
        ("Purple", "#a855f7", "#a855f740"), ("Pink", "#ec4899", "#ec489940"),
    ]

    /// Wrap the selection in `open` and `close`, or take them off when it already has them. With
    /// nothing selected, the cursor lands between the two.
    static func wrap(_ text: String, _ sel: Range<Int>, open: String, close: String) -> Edit {
        var chars = Array(text)
        let o = Array(open), c = Array(close)
        let lo = sel.lowerBound, hi = sel.upperBound
        // Already wrapped just outside the selection: unwrap.
        if lo >= o.count, hi + c.count <= chars.count, Array(chars[(lo - o.count)..<lo]) == o, Array(chars[hi..<(hi + c.count)]) == c {
            chars.removeSubrange(hi..<(hi + c.count))
            chars.removeSubrange((lo - o.count)..<lo)
            return Edit(text: String(chars), selection: (lo - o.count)..<(hi - o.count))
        }
        // The selection itself carries them: unwrap.
        if hi - lo >= o.count + c.count, Array(chars[lo..<(lo + o.count)]) == o, Array(chars[(hi - c.count)..<hi]) == c {
            chars.removeSubrange((hi - c.count)..<hi)
            chars.removeSubrange(lo..<(lo + o.count))
            return Edit(text: String(chars), selection: lo..<(hi - o.count - c.count))
        }
        chars.insert(contentsOf: c, at: hi)
        chars.insert(contentsOf: o, at: lo)
        return Edit(text: String(chars), selection: (lo + o.count)..<(hi + o.count))
    }

    static func color(_ text: String, _ sel: Range<Int>, css: String, highlight: Bool) -> Edit {
        highlight ? wrap(text, sel, open: "<mark style=\"background: \(css)\">", close: "</mark>")
            : wrap(text, sel, open: "<span style=\"color: \(css)\">", close: "</span>")
    }

    /// The list, heading or quote markers any line may start with.
    private static var marker: Regex<(Substring, Substring, Substring)> { /^(\s*)(#{1,6}\s+|[-*+]\s+\[[ xX]\]\s+|[-*+]\s+|\d+[.)]\s+|>\s?)/ }

    /// Make the selected lines `block`, or plain text again when they all are already.
    static func toggle(_ text: String, _ sel: Range<Int>, _ block: Block) -> Edit {
        var lines = text.components(separatedBy: "\n")
        let first = line(at: sel.lowerBound, lines)
        let last = max(line(at: sel.isEmpty ? sel.lowerBound : sel.upperBound - 1, lines), first)
        let isBlock = { (l: String) -> Bool in
            let s = l.trimmingCharacters(in: .whitespaces)
            switch block {
            case .heading(let n): return s.hasPrefix(String(repeating: "#", count: n) + " ")
            case .bullets: return s.wholeMatch(of: /[-*+]\s+(?!\[[ xX]\]).*/) != nil
            case .numbers: return s.wholeMatch(of: /\d+[.)]\s+.*/) != nil
            case .checklist: return s.wholeMatch(of: /[-*+]\s+\[[ xX]\].*/) != nil
            case .quote: return s.hasPrefix(">")
            }
        }
        // Blank lines inside a selection of several are left alone.
        let touched = (first...last).filter { first == last || !lines[$0].trimmingCharacters(in: .whitespaces).isEmpty }
        let off = touched.allSatisfy { isBlock(lines[$0]) }
        var (oldMarker, newMarker) = (0, 0)
        for (n, i) in touched.enumerated() {
            let s = lines[i]
            let spaces = s.prefix { $0 == " " }.count
            let old = s.firstMatch(of: marker).map { (indent: $0.1.count, length: $0.0.count) } ?? (indent: spaces, length: spaces)
            let prefix = off ? "" : block == .numbers ? "\(n + 1). " : block.prefix
            lines[i] = String(repeating: " ", count: old.indent) + prefix + s.dropFirst(old.length)
            if i == first { (oldMarker, newMarker) = (old.length, old.indent + prefix.count) }
        }
        let joined = lines.joined(separator: "\n")
        // Lines before the first are as they were, so it starts where it did.
        let start = lines[..<first].reduce(0) { $0 + $1.count + 1 }
        if first == last {
            let end = start + lines[first].count
            let col = max(sel.lowerBound - start - oldMarker + newMarker, newMarker)
            let lo = min(start + col, end)
            return Edit(text: joined, selection: lo..<min(lo + sel.count, end))
        }
        let end = lines[..<(last + 1)].reduce(0) { $0 + $1.count + 1 } - 1
        return Edit(text: joined, selection: start..<end)
    }

    /// Return was typed at `at` (the new line is already in `text`, ending at `at`): carry the
    /// line's list on, or end the list when the line was an empty item.
    static func newline(_ text: String, at: Int) -> Edit? {
        var chars = Array(text)
        guard at > 0, at <= chars.count, chars[at - 1] == "\n" else { return nil }
        var start = at - 1
        while start > 0, chars[start - 1] != "\n" { start -= 1 }
        let previous = String(chars[start..<(at - 1)])
        guard let m = previous.firstMatch(of: marker), m.0.count > 0 else { return nil }
        let marker = String(m.0)
        let trimmed = marker.trimmingCharacters(in: .whitespaces)
        guard !trimmed.hasPrefix("#") else { return nil }
        if previous.count == marker.count || previous.trimmingCharacters(in: .whitespaces) == trimmed {
            // An empty item: the list ends here, and the marker goes.
            chars.removeSubrange(start..<at)
            return Edit(text: String(chars), selection: start..<start)
        }
        var next = marker
        if let num = trimmed.firstMatch(of: /(\d+)([.)])/), let n = Int(num.1) {
            next = String(repeating: " ", count: String(m.1).count) + "\(n + 1)\(num.2) "
        } else if trimmed.contains("[") {
            next = String(repeating: " ", count: String(m.1).count) + "- [ ] "
        }
        chars.insert(contentsOf: next, at: at)
        return Edit(text: String(chars), selection: (at + next.count)..<(at + next.count))
    }

    /// Tick or untick the check box on line `index`.
    static func toggleCheck(_ text: String, line index: Int) -> String? {
        var lines = text.components(separatedBy: "\n")
        guard index < lines.count, let m = lines[index].firstMatch(of: /^(\s*[-*+]\s+\[)([ xX])(\])/) else { return nil }
        let mark = m.2 == " " ? "x" : " "
        lines[index].replaceSubrange(m.range, with: m.1 + mark + m.3)
        return lines.joined(separator: "\n")
    }

    /// The note's title as the Mac takes it: its first line with words in it, without markdown.
    static func title(of body: String) -> String {
        for line in body.split(separator: "\n") {
            var s = String(line)
            if let m = s.firstMatch(of: marker) { s = String(s.dropFirst(m.0.count)) }
            s = s.replacing(/<[^>]+>/, with: "").replacing(/[*_~`]/, with: "").trimmingCharacters(in: .whitespaces)
            if !s.isEmpty { return String(s.prefix(80)) }
        }
        return "Untitled"
    }

    private static func line(at offset: Int, _ lines: [String]) -> Int {
        var at = 0
        for (i, l) in lines.enumerated() {
            if offset <= at + l.count { return i }
            at += l.count + 1
        }
        return max(lines.count - 1, 0)
    }
}
