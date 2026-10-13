import Foundation

/// Where an answer writes maths: `$…$` and `\(…\)` in a line of text, `$$…$$` and `\[…\]` set on
/// their own. Found in the Markdown source before Markdown reads it, since Markdown takes `\[` for
/// an escaped bracket and `\\` for an escaped backslash. The Mac's rules (math/detect.rs), byte for
/// byte, so the phone finds the formulas the Mac does.
///
/// Dollars are also money and shell variables, so a single `$` only opens maths the way Pandoc
/// has it: not followed by a space, closed by a `$` that isn't preceded by a space or followed by
/// a digit ("costs $5 and $10" stays prose). Code — fenced blocks and code spans — is never maths,
/// and an escaped `\$` is a dollar.
nonisolated enum MathDetect {
    /// How a formula is delimited.
    enum Delim: Equatable, Sendable {
        /// `$…$`
        case dollar
        /// `$$…$$`
        case dollars
        /// `\(…\)`
        case paren
        /// `\[…\]`
        case bracket

        /// Display maths: set on its own line, centred.
        var display: Bool { self == .dollars || self == .bracket }

        var open: [UInt8] {
            switch self {
            case .dollar: [36]
            case .dollars: [36, 36]
            case .paren: [92, 40]
            case .bracket: [92, 91]
            }
        }

        var close: [UInt8] {
            switch self {
            case .dollar: [36]
            case .dollars: [36, 36]
            case .paren: [92, 41]
            case .bracket: [92, 93]
            }
        }
    }

    /// A formula in the source, as a range of its UTF-8 bytes (delimiters included).
    struct Region: Equatable, Sendable {
        var range: Range<Int>
        var delim: Delim
        /// Its closing delimiter has arrived. An open one runs to the end of the text: an answer
        /// still being written, whose formula shows as plain text until it closes.
        var closed: Bool

        /// The region's text, delimiters and all.
        func source(_ b: [UInt8]) -> String {
            String(decoding: b[range], as: UTF8.self)
        }

        /// The TeX between the delimiters, in `b` (the text the region was found in). Lines of a
        /// formula inside a block quote lose their `>`.
        func tex(_ b: [UInt8]) -> String {
            var inner = Array(b[range])
            if inner.starts(with: delim.open) { inner.removeFirst(delim.open.count) }
            if closed, inner.count >= delim.close.count, Array(inner.suffix(delim.close.count)) == delim.close {
                inner.removeLast(delim.close.count)
            }
            let text = String(decoding: inner, as: UTF8.self)
            let lineStart = b[..<range.lowerBound].lastIndex(of: 10).map { $0 + 1 } ?? 0
            let before = String(decoding: b[lineStart..<range.lowerBound], as: UTF8.self)
            guard before.drop(while: { $0 == " " || $0 == "\t" }).hasPrefix(">"), inner.contains(10) else { return text }
            return text.components(separatedBy: "\n").enumerated()
                .map { i, line in i == 0 ? line : MathDetect.unquote(line) }
                .joined(separator: "\n")
        }
    }

    static func unquote(_ line: String) -> String {
        var line = Substring(line)
        while true {
            let trimmed = line.drop(while: \.isWhitespace)
            guard trimmed.hasPrefix(">") else { return String(line) }
            let rest = trimmed.dropFirst()
            line = rest.hasPrefix(" ") ? rest.dropFirst() : rest
        }
    }

    /// `text` is exactly one formula, delimiters included.
    static func whole(_ text: String) -> Region? {
        let b = Array(text.utf8)
        let found = regions(b)
        guard found.count == 1, let r = found.first else { return nil }
        return r.range == 0..<b.count || (!r.closed && r.range.lowerBound == 0) ? r : nil
    }

    /// Inline maths longer than this isn't inline maths.
    private static let inlineMax = 2000

    static func regions(_ source: String) -> [Region] { regions(Array(source.utf8)) }

    /// Every formula in `b` (UTF-8), in order.
    static func regions(_ b: [UInt8]) -> [Region] {
        let fences = fences(b)
        var f = 0
        var out: [Region] = []
        var i = 0
        while i < b.count {
            while f < fences.count, fences[f].upperBound <= i { f += 1 }
            if f < fences.count, fences[f].lowerBound <= i {
                i = fences[f].upperBound
                continue
            }
            let nextFence = f < fences.count ? fences[f].lowerBound : b.count
            switch b[i] {
            case 92: // backslash
                let delim: Delim? = i + 1 < b.count ? (b[i + 1] == 40 ? .paren : b[i + 1] == 91 ? .bracket : nil) : nil
                if let delim {
                    if let region = closing(b, i, delim, nextFence) {
                        i = region.range.upperBound
                        out.append(region)
                    } else {
                        i += 2
                    }
                } else {
                    // An escape: whatever it escapes is literal.
                    i += 1 + (i + 1 < b.count ? charLen(b, i + 1) : 0)
                }
            case 96: // backtick
                let run = runOf(b, i, 96)
                i = codeSpanEnd(b, i, run, nextFence) ?? i + run
            case 36: // dollar
                let delim: Delim = i + 1 < b.count && b[i + 1] == 36 ? .dollars : .dollar
                if let region = closing(b, i, delim, nextFence) {
                    i = region.range.upperBound
                    out.append(region)
                } else {
                    i += delim.open.count
                }
            default:
                i += charLen(b, i)
            }
        }
        return out
    }

    /// The length of the UTF-8 sequence starting at `i` (1 for a stray continuation byte).
    private static func charLen(_ b: [UInt8], _ i: Int) -> Int {
        let c = b[i]
        let n = c < 0x80 ? 1 : c >= 0xF0 ? 4 : c >= 0xE0 ? 3 : c >= 0xC0 ? 2 : 1
        return min(n, b.count - i)
    }

    private static func runOf(_ b: [UInt8], _ i: Int, _ c: UInt8) -> Int {
        var n = 0
        while i + n < b.count, b[i + n] == c { n += 1 }
        return n
    }

    private static func isSpace(_ c: UInt8) -> Bool { c == 32 || c == 9 || c == 10 || c == 12 || c == 13 }

    /// Where the code span opened by `run` backticks at `i` ends: past a run of exactly as many,
    /// before a blank line or `limit` (a fence).
    private static func codeSpanEnd(_ b: [UInt8], _ i: Int, _ run: Int, _ limit: Int) -> Int? {
        var j = i + run
        while j < limit {
            if b[j] == 96 {
                let n = runOf(b, j, 96)
                if n == run { return j + n }
                j += n
            } else if b[j] == 10, blankLineAfter(b, j) {
                return nil
            } else {
                j += 1
            }
        }
        return nil
    }

    /// The line after the line ending at `nl` is blank (or a quote's empty line), or there is none.
    private static func blankLineAfter(_ b: [UInt8], _ nl: Int) -> Bool {
        var k = nl + 1
        while k < b.count, b[k] != 10 {
            if !(b[k] == 32 || b[k] == 9 || b[k] == 13 || b[k] == 62) { return false }
            k += 1
        }
        return true
    }

    /// The line starting at `from`.
    private static func line(_ b: [UInt8], from: Int) -> ArraySlice<UInt8> {
        let end = b[from...].firstIndex(of: 10) ?? b.count
        return b[from..<end]
    }

    /// A line that starts a list item or heading: inline maths can't run into one.
    private static func startsBlock(_ line: ArraySlice<UInt8>) -> Bool {
        var l = line.drop(while: isSpace)
        if l.first == 62 { l = l.dropFirst().drop(while: isSpace) }
        let a = Array(l)
        if a.count >= 2, a[0] == 45 || a[0] == 42 || a[0] == 43, a[1] == 32 { return true }
        if a.first == 35 { return true }
        let digits = a.prefix { $0 >= 48 && $0 <= 57 }.count
        return digits > 0 && digits + 1 < a.count && (a[digits] == 46 || a[digits] == 41) && a[digits + 1] == 32
    }

    private static func trimmedEmpty(_ b: [UInt8], _ r: Range<Int>) -> Bool {
        String(decoding: b[r], as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    /// The formula `delim` opens at `i`, if it closes — or, when the text ends first (an answer
    /// still being written), the open one running to its end. `$` never stays open: it's as often
    /// money as maths.
    private static func closing(_ b: [UInt8], _ i: Int, _ delim: Delim, _ limit: Int) -> Region? {
        let start = i + delim.open.count
        if delim == .dollar {
            guard start < b.count, !isSpace(b[start]) else { return nil }
        }
        let inline = !delim.display
        var j = start
        while j < limit {
            if inline, j - start > inlineMax { return nil }
            switch b[j] {
            case 92:
                if delim.close.first == 92, j + 1 < b.count, b[j + 1] == delim.close[1] {
                    if trimmedEmpty(b, start..<j) { return nil }
                    return Region(range: i..<(j + 2), delim: delim, closed: true)
                }
                j += 1 + (j + 1 < b.count ? charLen(b, j + 1) : 0)
            case 36 where delim == .dollar || delim == .dollars:
                if delim == .dollars {
                    if j + 1 < b.count, b[j + 1] == 36 {
                        if trimmedEmpty(b, start..<j) { return nil }
                        return Region(range: i..<(j + 2), delim: delim, closed: true)
                    }
                    j += 1
                    continue
                }
                // A single dollar closes, or it was never maths.
                return dollarCloses(b, start, j) ? Region(range: i..<(j + 1), delim: delim, closed: true) : nil
            case 10:
                if blankLineAfter(b, j) {
                    // A paragraph can't hold a formula past its end; if this is the end of the
                    // text, the formula is still being written.
                    let restBlank = b[j...].allSatisfy(isSpace)
                    return restBlank && delim != .dollar && limit == b.count ? openRegion(b, i, delim) : nil
                }
                if inline, startsBlock(line(b, from: j + 1)) { return nil }
                j += 1
            default:
                j += charLen(b, j)
            }
        }
        return delim != .dollar && limit == b.count ? openRegion(b, i, delim) : nil
    }

    /// The formula `delim` opened at `i` and hasn't closed, to the end of the text.
    private static func openRegion(_ b: [UInt8], _ i: Int, _ delim: Delim) -> Region {
        var end = b.count
        while end > 0, isSpace(b[end - 1]) { end -= 1 }
        return Region(range: i..<max(end, i + delim.open.count), delim: delim, closed: false)
    }

    /// The `$` at `j` closes the one before `start`: something between, no space before it, and no
    /// digit after it ("$5 and $10"). A letter right after it is only allowed after a letter, digit
    /// or bracket — `$n$th`, not `$PATH:$HOME`.
    private static func dollarCloses(_ b: [UInt8], _ start: Int, _ j: Int) -> Bool {
        if j == start { return false }
        let before = b[j - 1]
        if isSpace(before) { return false }
        guard j + 1 < b.count else { return true }
        let c = b[j + 1]
        if c >= 48 && c <= 57 { return false }
        let alpha = { (c: UInt8) in (c >= 65 && c <= 90) || (c >= 97 && c <= 122) }
        if alpha(c) || c == 95 {
            return alpha(before) || (before >= 48 && before <= 57) || before == 125 || before == 41 || before == 93 || before == 39
        }
        return true
    }

    /// The fenced code blocks in `b`, as byte ranges; one never closed runs to the end.
    private static func fences(_ b: [UInt8]) -> [Range<Int>] {
        var out: [Range<Int>] = []
        var open: (from: Int, c: UInt8, n: Int)?
        var offset = 0
        while offset < b.count {
            let start = offset
            let nl = b[start...].firstIndex(of: 10)
            offset = nl.map { $0 + 1 } ?? b.count
            let body = fenceBody(b[start..<offset])
            if let o = open {
                if let (c2, n2) = fenceMarker(body), c2 == o.c, n2 >= o.n, body.dropFirst(n2).allSatisfy(isSpace) {
                    out.append(o.from..<offset)
                    open = nil
                }
            } else if let (c, n) = fenceMarker(body) {
                // A backtick fence's info string has no backticks.
                if c == 96, body.dropFirst(n).contains(96) { continue }
                open = (start, c, n)
            }
        }
        if let o = open { out.append(o.from..<b.count) }
        return out
    }

    /// A line without its quote markers, list marker and indent.
    private static func fenceBody(_ line: ArraySlice<UInt8>) -> ArraySlice<UInt8> {
        var s = line.drop(while: isSpace)
        while s.first == 62 { s = s.dropFirst().drop(while: isSpace) }
        if s.count >= 2, s.first == 45 || s.first == 42 || s.first == 43, s[s.startIndex + 1] == 32 {
            return s.dropFirst(2).drop(while: isSpace)
        }
        let digits = s.prefix { $0 >= 48 && $0 <= 57 }.count
        if digits > 0, s.count >= digits + 2 {
            let a = s[s.startIndex + digits], b2 = s[s.startIndex + digits + 1]
            if (a == 46 || a == 41) && b2 == 32 { return s.dropFirst(digits + 2).drop(while: isSpace) }
        }
        return s
    }

    private static func fenceMarker(_ body: ArraySlice<UInt8>) -> (UInt8, Int)? {
        guard let c = body.first, c == 96 || c == 126 else { return nil }
        let n = body.prefix { $0 == c }.count
        return n >= 3 ? (c, n) : nil
    }
}
