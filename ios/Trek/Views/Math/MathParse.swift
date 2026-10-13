import Foundation

/// TeX maths, as agents write it, into a tree `MathLayout` can set: the Mac's parser
/// (math/parse.rs), ported. Forgiving by design: a brace left open closes at the end, a missing
/// argument is empty, and a command Trek doesn't know becomes `.unknown` — which the phone shows
/// as the formula's source rather than guess.
nonisolated enum MathParse {
    /// How deep groups may nest before the rest is shown as written.
    static let maxDepth = 48

    /// Delimiter for "none" (`\left.`).
    static let noDelim: Unicode.Scalar = "."

    /// Parse `tex` (maths mode).
    static func parse(_ tex: String) -> [MathNode] {
        var p = Parser(src: Array(tex.unicodeScalars))
        var out = p.list(.end)
        // A stray `}` ends a list early: show it and carry on.
        while p.pos < p.src.count {
            p.pos += 1
            out.append(.unknown("}"))
            out.append(contentsOf: p.list(.end))
        }
        return out
    }

    /// Whether `nodes` hold anything the parser didn't know (a command, a stray brace).
    static func hasUnknown(_ nodes: [MathNode]) -> Bool {
        nodes.contains { $0.hasUnknown }
    }

    /// Split an environment's body into rows (at `\\`) and cells (at `&`), noting `\hline`s.
    static func rowsOf(_ body: [MathNode], kind: MathArray.Kind, cols: [MathArray.Align], delims: (Unicode.Scalar, Unicode.Scalar)) -> MathArray {
        var rows: [[[MathNode]]] = [[[]]]
        var rowRules: [Int] = []
        for node in body {
            switch node {
            case .tab:
                rows[rows.count - 1].append([])
            case .newline:
                rows.append([[]])
            case .unknown("\\hline"):
                let at = rows.count - 1
                let last = rows[at]
                rowRules.append(last.count == 1 && last[0].isEmpty ? at : at + 1)
            default:
                let r = rows.count - 1
                rows[r][rows[r].count - 1].append(node)
            }
        }
        // A trailing `\\` leaves an empty row.
        if rows.count > 1, let last = rows.last, last.count == 1, last[0].isEmpty { rows.removeLast() }
        rowRules = rowRules.map { min($0, rows.count) }
        return MathArray(rows: rows, cols: cols, colRules: [], rowRules: rowRules, kind: kind, left: delims.0, right: delims.1)
    }

    /// Math-mode symbols from KaTeX's table, by command or character.
    static let symbols: [String: (ch: Unicode.Scalar, face: MathFace, cls: MathClass, italic: Bool)] = {
        var out: [String: (ch: Unicode.Scalar, face: MathFace, cls: MathClass, italic: Bool)] = [:]
        for line in MathTables.symbols.components(separatedBy: "\n") {
            let f = line.components(separatedBy: "\t")
            guard f.count == 5, let code = UInt32(f[1], radix: 16), let ch = Unicode.Scalar(code) else { continue }
            let cls: MathClass = switch f[3] {
            case "Op": .op
            case "Bin": .bin
            case "Rel": .rel
            case "Open": .open
            case "Close": .close
            case "Punct": .punct
            case "Inner": .inner
            default: .ord
            }
            out[f[0]] = (ch, f[2] == "A" ? .ams : .main, cls, f[4] == "1")
        }
        return out
    }()
}

/// TeX's atom classes, which decide the space between neighbours.
nonisolated enum MathClass: Equatable, Sendable { case ord, op, bin, rel, open, close, punct, inner }

/// TeX's maths styles.
nonisolated enum MathStyle: Equatable, Sendable { case display, text, script, scriptScript }

/// A KaTeX font face.
nonisolated enum MathFace: Hashable, Sendable {
    case main, mainBold, mainItalic, mainBoldItalic, mathItalic, mathBoldItalic, ams, caligraphic, fraktur, script, sansSerif,
         typewriter, size1, size2, size3, size4

    var italic: Bool { self == .mainItalic || self == .mainBoldItalic || self == .mathItalic || self == .mathBoldItalic }
}

nonisolated indirect enum MathNode: Equatable, Sendable {
    /// One glyph.
    case sym(Unicode.Scalar, MathFace, MathClass)
    /// A run of text, set as one line (`\text{…}`, an operator's name).
    case text(String, MathFace)
    /// `{…}`: an ordinary atom.
    case group([MathNode])
    /// A base with scripts. `limits`: set above and below (`true`), beside (`false`), or as the
    /// base decides (`nil`).
    case scripts(base: MathNode, sup: [MathNode]?, sub: [MathNode]?, limits: Bool?)
    /// `\frac` and friends. `style`: forced (`\dfrac`, `\tfrac`); `delims`: `\binom`'s.
    case frac(num: [MathNode], den: [MathNode], rule: Bool, style: MathStyle?, delims: [Unicode.Scalar]?)
    case sqrt(index: [MathNode]?, body: [MathNode])
    /// A large operator (`\sum`, `\int`: `glyph`) or a named one (`\lim`, `\sin`: `name`).
    case op(glyph: Unicode.Scalar?, name: [MathNode], limits: Bool)
    case accent(MathAccent, [MathNode])
    /// `\left( … \middle| … \right)`: the delimiters, the parts between them.
    case leftRight(delims: [Unicode.Scalar], parts: [[MathNode]])
    /// `\big(`, `\Bigg]`: a delimiter at a fixed size, 1 to 4 (0: `\middle`, while parsing).
    case big(Unicode.Scalar, size: Int, MathClass)
    /// Space, in ems (negative: back).
    case space(Double)
    case boxed([MathNode])
    /// The rest of a group in another style (`\displaystyle`).
    case styled(MathStyle, [MathNode])
    /// An environment: rows of cells.
    case array(MathArray)
    case phantom([MathNode], width: Bool, height: Bool)
    case cancel([MathNode], forward: Bool, back: Bool)
    /// `\overset{over}{base}`, `\underset{under}{base}`, `\stackrel`.
    case stack(base: [MathNode], over: [MathNode]?, under: [MathNode]?)
    /// `\xrightarrow[under]{over}`.
    case arrow(right: Bool, over: [MathNode], under: [MathNode])
    /// A slash through the next atom (`\not=`, `\neq`).
    case not(MathNode)
    /// The body as an atom of another class (`\mathrel{…}`).
    case classed(MathClass, [MathNode])
    /// `\tag{…}`: shown after the formula, in parentheses.
    case tag([MathNode])
    /// A command Trek doesn't know, shown as written.
    case unknown(String)
    /// `&` and `\\` outside an environment, for layout to set the formula as aligned rows.
    case tab
    case newline

    /// Space, `&` or `\\`: not an atom scripts can sit on.
    var isSpacing: Bool {
        switch self {
        case .space, .tab, .newline: true
        default: false
        }
    }

    var hasUnknown: Bool {
        switch self {
        // `\hline` outside an array is harmless.
        case .unknown(let name): name != "\\hline"
        case .sym, .text, .space, .big, .tab, .newline: false
        case .group(let b), .boxed(let b), .styled(_, let b), .phantom(let b, _, _), .cancel(let b, _, _), .classed(_, let b), .tag(let b),
             .accent(_, let b):
            MathParse.hasUnknown(b)
        case .scripts(let base, let sup, let sub, _): base.hasUnknown || MathParse.hasUnknown(sup ?? []) || MathParse.hasUnknown(sub ?? [])
        case .frac(let n, let d, _, _, _): MathParse.hasUnknown(n) || MathParse.hasUnknown(d)
        case .sqrt(let i, let b): MathParse.hasUnknown(i ?? []) || MathParse.hasUnknown(b)
        case .op(_, let name, _): MathParse.hasUnknown(name)
        case .leftRight(_, let parts): parts.contains { MathParse.hasUnknown($0) }
        case .array(let a): a.rows.contains { $0.contains { MathParse.hasUnknown($0) } }
        case .stack(let b, let o, let u): MathParse.hasUnknown(b) || MathParse.hasUnknown(o ?? []) || MathParse.hasUnknown(u ?? [])
        case .arrow(_, let o, let u): MathParse.hasUnknown(o) || MathParse.hasUnknown(u)
        case .not(let n): n.hasUnknown
        }
    }
}

nonisolated enum MathAccent: Equatable, Sendable {
    case glyph(Unicode.Scalar)
    /// `\vec`: a small arrow.
    case vec
    case hat, tilde, overline, underline
    case overArrow(left: Bool, right: Bool)
    case overBrace, underBrace
}

nonisolated struct MathArray: Equatable, Sendable {
    enum Align: Equatable, Sendable { case left, center, right }

    enum Kind: Equatable, Sendable {
        /// `matrix`, `array`: centred cells, text style.
        case matrix
        /// `smallmatrix`, `substack`: script style, tight.
        case small
        /// `aligned`, `align`: right-left pairs, display style.
        case aligned
        /// `gathered`, `equation`: centred rows, display style.
        case gathered
        /// `cases`: left-aligned, a quad apart.
        case cases(display: Bool)
    }

    var rows: [[[MathNode]]]
    var cols: [Align]
    /// Vertical rules: before which column (`cols.count` for after the last).
    var colRules: [Int]
    /// Horizontal rules: above which row (`rows.count` for below the last).
    var rowRules: [Int]
    var kind: Kind
    var left: Unicode.Scalar
    var right: Unicode.Scalar
}

/// The font letters and digits are set in.
private nonisolated enum MathFont { case normal, roman, bold, italic, blackboard, calligraphic, fraktur, script, sans, mono, boldSymbol }

private nonisolated enum Stop {
    /// The end of the text, or a `}` that isn't ours.
    case end
    /// `}`.
    case brace
    /// `]` at this nesting level (an optional argument).
    case bracket
    /// `\right`.
    case right
    /// `\end`.
    case endEnv
}

private nonisolated struct Parser {
    var src: [Unicode.Scalar]
    var pos = 0
    var depth = 0
    var font = MathFont.normal

    func peek() -> Unicode.Scalar? { pos < src.count ? src[pos] : nil }

    mutating func bump() -> Unicode.Scalar? {
        guard let c = peek() else { return nil }
        pos += 1
        return c
    }

    func peek(_ offset: Int) -> Unicode.Scalar? { pos + offset < src.count ? src[pos + offset] : nil }

    static func isAlpha(_ c: Unicode.Scalar) -> Bool { (c >= "a" && c <= "z") || (c >= "A" && c <= "Z") }

    mutating func skipSpace() {
        while let c = peek(), c.properties.isWhitespace { pos += 1 }
    }

    /// A control sequence's name, after its backslash (letters, or one other character).
    mutating func commandName() -> String {
        var n = ""
        while let c = peek(), Self.isAlpha(c) {
            n.unicodeScalars.append(c)
            pos += 1
        }
        if !n.isEmpty {
            // `\operatorname*`, `\hspace*`, `\tag*`.
            if peek() == "*", ["operatorname", "hspace", "tag"].contains(n) {
                pos += 1
                n += "*"
            }
            skipSpace()
            return n
        }
        return bump().map { String($0) } ?? ""
    }

    /// Whether the next thing is `\name`, without consuming it.
    func atCommand(_ name: String) -> Bool {
        let scalars = Array(name.unicodeScalars)
        guard peek() == "\\", pos + 1 + scalars.count <= src.count, Array(src[(pos + 1)..<(pos + 1 + scalars.count)]) == scalars else { return false }
        return !(peek(1 + scalars.count).map(Self.isAlpha) ?? false)
    }

    mutating func list(_ stop: Stop) -> [MathNode] {
        depth += 1
        defer { depth -= 1 }
        return depth > MathParse.maxDepth ? restAsText() : listInner(stop)
    }

    /// Too deep to be maths: the rest as written.
    mutating func restAsText() -> [MathNode] {
        var rest = ""
        rest.unicodeScalars.append(contentsOf: src[pos...])
        pos = src.count
        return [.text(rest, .main)]
    }

    mutating func listInner(_ stop: Stop) -> [MathNode] {
        var out: [MathNode] = []
        let saved = font
        defer { font = saved }
        while true {
            skipSpace()
            guard let c = peek() else { break }
            switch c {
            case "}":
                if stop == .brace { pos += 1 }
                return out
            case "]" where stop == .bracket:
                pos += 1
                return out
            case "^", "_":
                pos += 1
                let a = arg()
                attach(&out, sup: c == "^", a)
            case "'":
                var primes: [MathNode] = []
                while peek() == "'" {
                    pos += 1
                    primes.append(.sym("\u{2032}", .main, .ord))
                }
                attach(&out, sup: true, primes)
            case "\\":
                if stop == .right && atCommand("right") { return out }
                if stop == .endEnv && atCommand("end") { return out }
                pos += 1
                let name = commandName()
                // Infix fractions take everything before and after them in the group.
                let infix: (Bool, [Unicode.Scalar]?)? = switch name {
                case "over": (true, nil)
                case "atop": (false, nil)
                case "choose": (false, ["(", ")"])
                case "brack": (false, ["[", "]"])
                case "brace": (false, ["{", "}"])
                default: nil
                }
                if let (rule, delims) = infix {
                    let num = out
                    let den = list(stop)
                    return [.frac(num: num, den: den, rule: rule, style: nil, delims: delims)]
                }
                if let style = styleSwitch(name) {
                    let body = list(stop)
                    out.append(.styled(style, body))
                    return out
                }
                if let f = fontSwitch(name) {
                    font = f
                    continue
                }
                if name == "color" {
                    // `\color{red}`: the rest of the group, in the text's own colour.
                    _ = argText()
                    continue
                }
                if name == "limits" || name == "nolimits" || name == "displaylimits" {
                    setLimits(&out, name == "limits")
                    continue
                }
                if name == "not" {
                    let next = atom()
                    out.append(.not(next.first ?? .group([])))
                    continue
                }
                out.append(contentsOf: command(name))
            case "{":
                pos += 1
                out.append(.group(list(.brace)))
            case "&":
                pos += 1
                out.append(.tab)
            case "~":
                pos += 1
                out.append(.space(0.25))
            default:
                pos += 1
                out.append(symbol(c))
            }
        }
        return out
    }

    /// One argument: a group, or a single token.
    mutating func arg() -> [MathNode] {
        skipSpace()
        switch peek() {
        case nil, "}", "&": return []
        case "{":
            pos += 1
            return list(.brace)
        default: return atom()
        }
    }

    /// One token as nodes: a symbol, a group, or a command with its arguments.
    mutating func atom() -> [MathNode] {
        skipSpace()
        switch peek() {
        case nil, "}": return []
        case "{":
            pos += 1
            return [.group(list(.brace))]
        case "\\":
            pos += 1
            let name = commandName()
            return command(name)
        case let c?:
            pos += 1
            return [symbol(c)]
        }
    }

    /// An argument read as plain text: a colour name, an environment name, a column spec.
    mutating func argText() -> String {
        skipSpace()
        switch peek() {
        case "{":
            pos += 1
            let start = pos
            var depth = 0
            while let c = bump() {
                if c == "{" {
                    depth += 1
                } else if c == "}" {
                    if depth == 0 { return string(start..<(pos - 1)) }
                    depth -= 1
                }
            }
            return string(start..<src.count)
        case let c? where c != "}":
            pos += 1
            return String(c)
        default:
            return ""
        }
    }

    func string(_ r: Range<Int>) -> String {
        var s = ""
        s.unicodeScalars.append(contentsOf: src[r])
        return s
    }

    /// `[…]`, when it comes next: an optional argument.
    mutating func optional() -> [MathNode]? {
        skipSpace()
        guard peek() == "[" else { return nil }
        pos += 1
        return list(.bracket)
    }

    /// A delimiter after `\left`, `\big` and the like.
    mutating func delim() -> Unicode.Scalar {
        skipSpace()
        switch peek() {
        case nil: return MathParse.noDelim
        case "\\":
            pos += 1
            return delimNamed(commandName()) ?? MathParse.noDelim
        case let c?:
            pos += 1
            switch c {
            case "<": return "\u{27e8}"
            case ">": return "\u{27e9}"
            default: return c
            }
        }
    }

    /// Text-mode content: `\text{…}`'s argument, with `$…$` inside it back in maths.
    mutating func textArg(_ face: MathFace) -> [MathNode] {
        skipSpace()
        guard peek() == "{" else {
            return bump().map { [.text(String($0), face)] } ?? []
        }
        pos += 1
        depth += 1
        defer { depth -= 1 }
        return depth > MathParse.maxDepth ? restAsText() : textInner(face)
    }

    mutating func textInner(_ face: MathFace) -> [MathNode] {
        var out: [MathNode] = []
        var run = ""
        func flush() {
            if !run.isEmpty {
                out.append(.text(run, face))
                run = ""
            }
        }
        while let c = bump() {
            switch c {
            case "}":
                flush()
                return out
            case "{":
                flush()
                out.append(contentsOf: textInner(face))
            case "$":
                flush()
                let start = pos
                let end = src[start...].firstIndex(of: "$") ?? src.count
                var sub = Parser(src: Array(src[start..<end]), depth: depth)
                out.append(.group(sub.list(.end)))
                pos = min(end + 1, src.count)
            case "\\":
                let name = commandName()
                switch name {
                case "$", "%", "&", "#", "_", "{", "}": run += name
                case " ", ",", ";", ":": run += " "
                case "textbackslash": run += "\\"
                case "textbf", "bf", "textit", "it", "emph", "texttt", "textrm", "textup", "textnormal", "textsf":
                    flush()
                    let bold = name == "textbf" || name == "bf"
                    let italic = name == "textit" || name == "it" || name == "emph"
                    let inner: MathFace = if bold && face == .mainItalic || italic && face == .mainBold {
                        .mainBoldItalic
                    } else if bold {
                        .mainBold
                    } else if italic {
                        .mainItalic
                    } else {
                        name == "texttt" ? .typewriter : name == "textsf" ? .sansSerif : .main
                    }
                    out.append(contentsOf: textArg(inner))
                case "ldots", "dots", "textellipsis": run += "…"
                case "quad": run += "\u{2003}"
                case "qquad": run += "\u{2003}\u{2003}"
                case "": run += "\\"
                default:
                    flush()
                    out.append(.unknown(name))
                }
            case "~": run += "\u{a0}"
            case _ where c.properties.isWhitespace:
                if !run.hasSuffix(" ") { run += " " }
            case "-" where peek() == "-" && peek(1) == "-":
                pos += 2
                run += "—"
            case "-" where peek() == "-":
                pos += 1
                run += "–"
            case "`" where peek() == "`":
                pos += 1
                run += "“"
            case "'" where peek() == "'":
                pos += 1
                run += "”"
            case "`": run += "‘"
            case "'": run += "’"
            default: run.unicodeScalars.append(c)
            }
        }
        flush()
        return out
    }

    /// A character in maths mode.
    func symbol(_ c: Unicode.Scalar) -> MathNode {
        if c.isASCII, Self.isAlpha(c) || (c >= "0" && c <= "9") { return letter(c, font) }
        if let s = MathParse.symbols[String(c)] { return sym(s.ch, s.face, s.cls, s.italic, font) }
        switch c {
        case "%", "#": return .sym(c, .main, .ord)
        case "|": return .sym("\u{2223}", .main, .ord)
        // Anything else (accented letters, CJK) from the system's fonts.
        default: return .text(String(c), .main)
        }
    }

    mutating func command(_ name: String) -> [MathNode] {
        switch name {
        case "": return [.unknown("\\")]
        case "\\", "newline", "cr":
            // `\\[2pt]`: the extra space doesn't matter here.
            let save = pos
            if optional() == nil { pos = save }
            return [.newline]
        case "frac", "dfrac", "tfrac", "cfrac":
            let num = arg()
            let den = arg()
            let style: MathStyle? = name == "dfrac" || name == "cfrac" ? .display : name == "tfrac" ? .text : nil
            return [.frac(num: num, den: den, rule: true, style: style, delims: nil)]
        case "binom", "dbinom", "tbinom":
            let num = arg()
            let den = arg()
            let style: MathStyle? = name == "dbinom" ? .display : name == "tbinom" ? .text : nil
            return [.frac(num: num, den: den, rule: false, style: style, delims: ["(", ")"])]
        case "genfrac":
            let left = argText()
            let right = argText()
            let thickness = argText()
            let style = argText()
            let num = arg()
            let den = arg()
            func d(_ s: String) -> Unicode.Scalar {
                var t = Substring(s)
                while t.hasPrefix("\\") { t = t.dropFirst() }
                return delimNamed(String(t)) ?? s.unicodeScalars.first ?? MathParse.noDelim
            }
            let delims: [Unicode.Scalar]? = !left.isEmpty || !right.isEmpty ? [d(left), d(right)] : nil
            let st: MathStyle? = switch style.trimmingCharacters(in: .whitespaces) {
            case "0": .display
            case "1": .text
            case "2": .script
            case "3": .scriptScript
            default: nil
            }
            return [.frac(num: num, den: den, rule: !thickness.trimmingCharacters(in: .whitespaces).hasPrefix("0"), style: st, delims: delims)]
        case "sqrt":
            let index = optional()
            let body = arg()
            return [.sqrt(index: index, body: body)]
        case "text", "textrm", "textnormal", "textup", "textmd", "mbox", "hbox", "mathrm_text": return [.group(textArg(.main))]
        case "textbf": return [.group(textArg(.mainBold))]
        case "textit", "emph", "textsl": return [.group(textArg(.mainItalic))]
        case "texttt": return [.group(textArg(.typewriter))]
        case "textsf": return [.group(textArg(.sansSerif))]
        case "fbox": return [.boxed(textArg(.main))]
        case "boxed": return [.boxed([.styled(.display, arg())])]
        case "operatorname", "operatorname*", "mathop":
            let saved = font
            if name != "mathop" { font = .roman }
            let body = arg()
            font = saved
            return [.op(glyph: nil, name: body, limits: name != "operatorname")]
        case "mathbin", "mathrel", "mathord", "mathopen", "mathclose", "mathpunct", "mathinner":
            let cls: MathClass = switch name {
            case "mathbin": .bin
            case "mathrel": .rel
            case "mathopen": .open
            case "mathclose": .close
            case "mathpunct": .punct
            case "mathinner": .inner
            default: .ord
            }
            return [.classed(cls, arg())]
        case "left":
            var delims = [delim()]
            var parts: [[MathNode]] = [[]]
            // `\middle` arrives as a marker in the list.
            for node in list(.right) {
                if case .big(let d, 0, _) = node {
                    parts.append([])
                    delims.append(d)
                } else {
                    parts[parts.count - 1].append(node)
                }
            }
            if atCommand("right") {
                pos += 6
                delims.append(delim())
            } else {
                delims.append(MathParse.noDelim)
            }
            return [.leftRight(delims: delims, parts: parts)]
        case "middle": return [.big(delim(), size: 0, .rel)]
        case "right":
            // A `\right` with no `\left`: its delimiter, as written.
            return [.sym(delim(), .main, .close)]
        case "big", "Big", "bigg", "Bigg", "bigl", "Bigl", "biggl", "Biggl", "bigr", "Bigr", "biggr", "Biggr", "bigm", "Bigm", "biggm", "Biggm":
            var base = Substring(name)
            while let l = base.last, "lrm".contains(l) { base = base.dropLast() }
            let size = switch base {
            case "big": 1
            case "Big": 2
            case "bigg": 3
            default: 4
            }
            let cls: MathClass = switch name.last {
            case "l": .open
            case "r": .close
            case "m": .rel
            default: .ord
            }
            return [.big(delim(), size: size, cls)]
        case ",", "thinspace": return [.space(3.0 / 18)]
        case ":", ">", "medspace": return [.space(4.0 / 18)]
        case ";", "thickspace": return [.space(5.0 / 18)]
        case "!", "negthinspace": return [.space(-3.0 / 18)]
        case "negmedspace": return [.space(-4.0 / 18)]
        case "negthickspace": return [.space(-5.0 / 18)]
        case " ", "space", "nobreakspace": return [.space(0.25)]
        case "enspace": return [.space(0.5)]
        case "quad": return [.space(1)]
        case "qquad": return [.space(2)]
        case "hspace", "hspace*": return [.space(dimension(argText()))]
        case "kern", "mkern", "hskip", "mskip", "mspace":
            let dim = peek() == "{" ? argText() : dimensionText()
            return [.space(dimension(dim))]
        case "displaystyle", "textstyle", "scriptstyle", "scriptscriptstyle": return []
        case "hat", "widehat", "check", "widecheck", "tilde", "widetilde", "bar", "overline", "underline", "vec", "dot", "ddot", "dddot", "acute",
             "grave", "breve", "mathring", "overrightarrow", "overleftarrow", "overleftrightarrow", "overbrace", "underbrace":
            let accent: MathAccent = switch name {
            case "hat": .glyph("\u{2c6}")
            case "check", "widecheck": .glyph("\u{2c7}")
            case "tilde": .glyph("\u{2dc}")
            case "bar": .glyph("\u{2c9}")
            case "dot": .glyph("\u{2d9}")
            case "ddot": .glyph("\u{a8}")
            case "dddot": .glyph("\u{20db}")
            case "acute": .glyph("\u{2ca}")
            case "grave": .glyph("\u{2cb}")
            case "breve": .glyph("\u{2d8}")
            case "mathring": .glyph("\u{2da}")
            case "widehat": .hat
            case "widetilde": .tilde
            case "vec": .vec
            case "overline": .overline
            case "underline": .underline
            case "overrightarrow": .overArrow(left: false, right: true)
            case "overleftarrow": .overArrow(left: true, right: false)
            case "overleftrightarrow": .overArrow(left: true, right: true)
            case "overbrace": .overBrace
            default: .underBrace
            }
            let body = arg()
            // A brace takes its label as a script set above or below it.
            let node = MathNode.accent(accent, body)
            if accent == .overBrace || accent == .underBrace { return [.scripts(base: node, sup: nil, sub: nil, limits: true)] }
            return [node]
        case "phantom", "hphantom", "vphantom":
            return [.phantom(arg(), width: name != "vphantom", height: name != "hphantom")]
        case "mathstrut": return [.phantom([symbol("(")], width: false, height: true)]
        case "smash":
            _ = optional()
            return [.group(arg())]
        case "cancel", "bcancel", "xcancel", "sout":
            return [.cancel(arg(), forward: name != "bcancel", back: name == "bcancel" || name == "xcancel")]
        case "overset", "stackrel":
            let over = arg()
            let base = arg()
            let stack = MathNode.stack(base: base, over: over, under: nil)
            return [name == "stackrel" ? .classed(.rel, [stack]) : stack]
        case "underset":
            let under = arg()
            let base = arg()
            return [.stack(base: base, over: nil, under: under)]
        case "xrightarrow", "xleftarrow", "xLeftarrow", "xRightarrow", "xleftrightarrow", "xmapsto":
            let under = optional() ?? []
            let over = arg()
            return [.classed(.rel, [.arrow(right: !name.contains("left"), over: over, under: under)])]
        case "textcolor", "colorbox":
            _ = argText()
            return name == "colorbox" ? [.group(textArg(.main))] : [.group(arg())]
        case "pmod":
            let a = arg()
            return [.group([.space(1), symbol("("), .text("mod", .main), .space(6.0 / 18)] + a + [symbol(")")])]
        case "pod":
            let a = arg()
            return [.group([.space(1), symbol("(")] + a + [symbol(")")])]
        case "bmod": return [.classed(.bin, [.text("mod", .main)])]
        case "mod": return [.group([.space(1), .text("mod", .main), .space(6.0 / 18)])]
        case "tag", "tag*": return [.tag(textArg(.main))]
        case "label", "ref", "eqref":
            let text = argText()
            return name == "label" ? [] : [.text("(\(text))", .main)]
        case "hline", "hdashline": return [.unknown("\\hline")]
        case "nonumber", "notag", "allowbreak", "nobreak", "relax", "displaybreak", "strut": return []
        case "substack":
            return [.array(MathParse.rowsOf(arg(), kind: .small, cols: [.center], delims: (MathParse.noDelim, MathParse.noDelim)))]
        case "begin": return environment()
        case "end":
            // An `\end` with no `\begin`.
            return [.unknown("\\end{\(argText())}")]
        case "neq", "ne": return [.not(.sym("=", .main, .rel))]
        case "notin": return [.not(.sym("\u{2208}", .main, .rel))]
        case "implies": return [.space(5.0 / 18), .sym("\u{27f9}", .main, .rel), .space(5.0 / 18)]
        case "impliedby": return [.space(5.0 / 18), .sym("\u{27f8}", .main, .rel), .space(5.0 / 18)]
        case "iff": return [.space(5.0 / 18), .sym("\u{27fa}", .main, .rel), .space(5.0 / 18)]
        case "dots", "dotsc", "dotso", "ldots", "mathellipsis": return [.sym("\u{2026}", .main, .inner)]
        case "cdots", "dotsb", "dotsm", "dotsi": return [.sym("\u{22ef}", .main, .inner)]
        case "vdots": return [.sym("\u{22ee}", .main, .ord)]
        case "colon": return [.sym(":", .main, .punct)]
        case "R", "N", "Z", "Q", "C", "Reals", "Complex", "natnums", "reals", "Bbbk":
            let c: Unicode.Scalar = switch name {
            case "Reals", "reals": "R"
            case "Complex": "C"
            case "natnums": "N"
            case "Bbbk": "k"
            default: name.unicodeScalars.first ?? "R"
            }
            return [.sym(c, .ams, .ord)]
        case "lt": return [.sym("<", .main, .rel)]
        case "gt": return [.sym(">", .main, .rel)]
        case "argmax", "argmin": return [.op(glyph: nil, name: [.text("arg\u{2009}\(name.dropFirst(3))", .main)], limits: true)]
        default:
            if let text = namedOperator(name) {
                let limits = ["lim", "liminf", "limsup", "max", "min", "sup", "inf", "det", "gcd", "Pr", "injlim", "projlim", "varliminf", "varlimsup"]
                    .contains(name)
                return [.op(glyph: nil, name: [.text(text, .main)], limits: limits)]
            }
            if let f = fontCommand(name) {
                let saved = font
                font = f
                let body = arg()
                font = saved
                return [.group(body)]
            }
            if let s = MathParse.symbols["\\" + name] {
                if s.cls == .op {
                    let limits = !(s.ch.value >= 0x222b && s.ch.value <= 0x2233)
                    return [.op(glyph: s.ch, name: [], limits: limits)]
                }
                return [sym(s.ch, s.face, s.cls, s.italic, font)]
            }
            if let d = delimNamed(name) { return [.sym(d, .main, .ord)] }
            return [.unknown("\\" + name)]
        }
    }

    /// A TeX dimension written without braces (`\kern 3mu`, `\mkern-2mu`).
    mutating func dimensionText() -> String {
        skipSpace()
        let start = pos
        while let c = peek(), (c >= "0" && c <= "9") || c == "." || c == "-" || c == "+" || c == " " { pos += 1 }
        var unit = 0
        while unit < 2, let c = peek(), Self.isAlpha(c) {
            pos += 1
            unit += 1
        }
        return string(start..<pos)
    }

    mutating func environment() -> [MathNode] {
        let env = argText()
        var base = Substring(env)
        while base.hasSuffix("*") { base = base.dropLast() }
        var spec: String?
        if ["array", "subarray", "alignat", "alignedat"].contains(base) { spec = argText() }
        var body = list(.endEnv)
        if atCommand("end") {
            pos += 4
            _ = argText()
        }
        let none = MathParse.noDelim
        var kind = MathArray.Kind.matrix
        var delims = (none, none)
        var cols: [MathArray.Align] = []
        switch base {
        case "matrix": break
        case "pmatrix": delims = ("(", ")")
        case "bmatrix": delims = ("[", "]")
        case "Bmatrix": delims = ("{", "}")
        case "vmatrix": delims = ("|", "|")
        case "Vmatrix": delims = ("\u{2016}", "\u{2016}")
        case "smallmatrix": kind = .small
        case "cases": (kind, delims, cols) = (.cases(display: false), ("{", none), [.left, .left])
        case "dcases": (kind, delims, cols) = (.cases(display: true), ("{", none), [.left, .left])
        case "rcases": (kind, delims, cols) = (.cases(display: false), (none, "}"), [.left, .left])
        case "aligned", "align", "alignat", "alignedat", "split", "eqnarray", "flalign": kind = .aligned
        case "gathered", "gather", "equation", "multline", "displaymath", "math": (kind, cols) = (.gathered, [.center])
        case "array", "subarray":
            kind = base == "subarray" ? .small : .matrix
            cols = columnSpec(spec ?? "c").cols
        default: break
        }
        if base == "eqnarray" { body.removeAll { if case .tag = $0 { true } else { false } } }
        var array = MathParse.rowsOf(body, kind: kind, cols: cols, delims: delims)
        if base == "array" { array.colRules = columnSpec(spec ?? "c").rules }
        return [.array(array)]
    }
}

/// Columns of an `array` spec (`{c|cc}`), and where its vertical rules go.
private nonisolated func columnSpec(_ spec: String) -> (cols: [MathArray.Align], rules: [Int]) {
    var cols: [MathArray.Align] = []
    var rules: [Int] = []
    var it = spec.makeIterator()
    while let c = it.next() {
        switch c {
        case "l": cols.append(.left)
        case "c": cols.append(.center)
        case "r": cols.append(.right)
        case "|": rules.append(cols.count)
        case "p", "m", "b", "@", ">", "<":
            // A braced argument follows; `p{…}` is a left-aligned column.
            var depth = 0
            while let d = it.next() {
                if d == "{" {
                    depth += 1
                } else if d == "}" {
                    depth -= 1
                    if depth <= 0 { break }
                }
            }
            if "pmb".contains(c) { cols.append(.left) }
        default: break
        }
    }
    return (cols, rules)
}

/// A TeX length in ems (`3mu`, `1em`, `2pt`, `0.5ex`).
private nonisolated func dimension(_ text: String) -> Double {
    let t = text.replacingOccurrences(of: " ", with: "")
    let split = t.firstIndex { $0.isASCII && $0.isLetter } ?? t.endIndex
    let value = Double(t[..<split]) ?? 0
    let em: Double = switch t[split...] {
    case "em": 1
    case "ex": 0.431
    case "mu": 1.0 / 18
    case "pt": 0.1
    case "pc": 1.2
    case "px": 0.1 * 0.75
    case "bp": 0.1 * 72.27 / 72
    case "mm": 0.1 * 2.845
    case "cm": 0.1 * 28.45
    case "in": 0.1 * 72.27
    default: 0.1
    }
    return min(max(value * em, -20), 20)
}

/// `^` or `_` on the last atom of `out` (or on nothing).
private nonisolated func attach(_ out: inout [MathNode], sup: Bool, _ arg: [MathNode]) {
    var base = MathNode.group([])
    switch out.popLast() {
    case .scripts(let b, let s, let sub, let limits)?:
        // `x^a_b`, `x_b^a`, `x'^2`: one set of scripts. A second of the same kind starts a new
        // atom, as TeX would complain and show it anyway.
        let taken = sup ? s != nil : sub != nil
        let primes = s?.allSatisfy { if case .sym("\u{2032}", _, _) = $0 { true } else { false } } ?? false
        if !taken || (sup && primes) {
            out.append(sup ? .scripts(base: b, sup: (s ?? []) + arg, sub: sub, limits: limits) : .scripts(base: b, sup: s, sub: arg, limits: limits))
            return
        }
        out.append(.scripts(base: b, sup: s, sub: sub, limits: limits))
    case let node?:
        // Scripts on space or a line break go on nothing, after it.
        if node.isSpacing { out.append(node) } else { base = node }
    case nil:
        break
    }
    out.append(.scripts(base: base, sup: sup ? arg : nil, sub: sup ? nil : arg, limits: nil))
}

/// `\limits` or `\nolimits` after an operator.
private nonisolated func setLimits(_ out: inout [MathNode], _ on: Bool) {
    switch out.last {
    case .op(let g, let n, _)?: out[out.count - 1] = .op(glyph: g, name: n, limits: on)
    case .scripts(let b, let s, let sub, _)?: out[out.count - 1] = .scripts(base: b, sup: s, sub: sub, limits: on)
    default: break
    }
}

private nonisolated func styleSwitch(_ name: String) -> MathStyle? {
    switch name {
    case "displaystyle": .display
    case "textstyle": .text
    case "scriptstyle": .script
    case "scriptscriptstyle": .scriptScript
    default: nil
    }
}

/// Old-style switches that change the font for the rest of the group.
private nonisolated func fontSwitch(_ name: String) -> MathFont? {
    switch name {
    case "rm": .roman
    case "bf": .bold
    case "it": .italic
    case "cal": .calligraphic
    case "sf": .sans
    case "tt": .mono
    default: nil
    }
}

/// Commands that set their argument in another font.
private nonisolated func fontCommand(_ name: String) -> MathFont? {
    switch name {
    case "mathrm", "mathup": .roman
    case "mathbf", "bold", "mathbfup": .bold
    case "mathit": .italic
    case "mathbb", "Bbb", "mathds": .blackboard
    case "mathcal": .calligraphic
    case "mathfrak", "frak": .fraktur
    case "mathscr": .script
    case "mathsf", "mathsfup": .sans
    case "mathtt": .mono
    case "boldsymbol", "bm", "pmb", "mathbfit": .boldSymbol
    case "mathnormal": .normal
    default: nil
    }
}

/// Upright operator names: their text.
private nonisolated func namedOperator(_ name: String) -> String? {
    switch name {
    case "sin", "cos", "tan", "cot", "sec", "csc", "arcsin", "arccos", "arctan", "arctg", "arcctg", "sinh", "cosh", "tanh", "coth", "sh", "ch",
         "th", "tg", "ctg", "cotg", "log", "ln", "lg", "exp", "ker", "dim", "hom", "deg", "arg", "lim", "max", "min", "sup", "inf", "det", "gcd",
         "Pr", "tr", "Tr", "rank", "sgn", "sign", "diag", "span", "Var", "Cov", "erf":
        name
    case "liminf": "lim\u{2009}inf"
    case "limsup": "lim\u{2009}sup"
    case "injlim": "inj\u{2009}lim"
    case "projlim": "proj\u{2009}lim"
    default: nil
    }
}

/// Delimiters by name, for `\left` and `\big`.
private nonisolated func delimNamed(_ name: String) -> Unicode.Scalar? {
    switch name {
    case "{", "lbrace": "{"
    case "}", "rbrace": "}"
    case "|", "Vert", "lVert", "rVert": "\u{2016}"
    case "vert", "lvert", "rvert", "mid": "|"
    case "langle", "lang": "\u{27e8}"
    case "rangle", "rang": "\u{27e9}"
    case "lfloor": "\u{230a}"
    case "rfloor": "\u{230b}"
    case "lceil": "\u{2308}"
    case "rceil": "\u{2309}"
    case "lbrack": "["
    case "rbrack": "]"
    case "backslash": "\\"
    case "uparrow": "\u{2191}"
    case "downarrow": "\u{2193}"
    case "updownarrow": "\u{2195}"
    case "Uparrow": "\u{21d1}"
    case "Downarrow": "\u{21d3}"
    case "lgroup": "("
    case "rgroup": ")"
    case ".": MathParse.noDelim
    default: nil
    }
}

private nonisolated func sym(_ ch: Unicode.Scalar, _ face: MathFace, _ cls: MathClass, _ italic: Bool, _ font: MathFont) -> MathNode {
    let bold = font == .bold || font == .boldSymbol
    if italic { return .sym(ch, bold ? .mathBoldItalic : .mathItalic, cls) }
    return .sym(ch, bold && face == .main ? .mainBold : face, cls)
}

/// A letter or digit in the current font.
private nonisolated func letter(_ c: Unicode.Scalar, _ font: MathFont) -> MathNode {
    let digit = c >= "0" && c <= "9"
    let upper = c >= "A" && c <= "Z"
    let face: MathFace = switch font {
    case .normal: digit ? .main : .mathItalic
    case .roman: .main
    case .bold: .mainBold
    case .italic: digit ? .main : .mainItalic
    case .blackboard where upper || c == "k": .ams
    case .calligraphic where upper: .caligraphic
    case .script where upper: .script
    case .fraktur: .fraktur
    case .sans: .sansSerif
    case .mono: .typewriter
    case .boldSymbol: digit ? .mainBold : .mathBoldItalic
    case .blackboard, .calligraphic, .script: .main
    }
    return .sym(c, face, .ord)
}
