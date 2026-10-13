import Foundation

/// Typesetting: the parsed formula as boxes — glyphs, rules and drawn shapes placed in ems around
/// a baseline — following TeX's rules as KaTeX implements them, with the KaTeX fonts' metrics:
/// the Mac's layout (math/layout.rs), ported. Coordinates: x to the right, y up from the baseline;
/// one em is the formula's font size.
nonisolated enum MathLayout {
    /// Typeset `nodes`: display maths, or inline (text style). `measure` gives the width in ems of
    /// text the font tables can't (`\text{…}` runs, characters outside the KaTeX fonts).
    static func typeset(_ nodes: [MathNode], display: Bool, measure: @escaping (String, MathFace) -> Double?) -> MathBox {
        let env = Env(style: display ? .display : .text, cramped: false)
        let t = Typesetter(measure: measure)
        let tags = nodes.compactMap { if case .tag(let b) = $0 { b } else { nil } }
        let body = nodes.filter { if case .tag = $0 { false } else { true } }
        // `\\` and `&` outside an environment: rows, aligned at the `&`s when there are any.
        var b: MathBox
        if body.contains(where: { $0 == .newline || $0 == .tab }) {
            let aligned = body.contains(.tab)
            let array = MathParse.rowsOf(body, kind: aligned ? .aligned : .gathered, cols: aligned ? [] : [.center],
                                         delims: (MathParse.noDelim, MathParse.noDelim))
            b = t.array(array, env)
        } else {
            b = t.list(body, env)
        }
        for inner in tags {
            b.append(.kern(2))
            b.append(t.list([.sym("(", .main, .open)] + inner + [.sym(")", .main, .close)], env.with(.text)))
        }
        return sanitize(b)
    }

    /// Depth, height, italic correction, skew and width of `ch` in `face`, when the face has it.
    static func metrics(_ face: MathFace, _ ch: Unicode.Scalar) -> Metrics? {
        tables[face]?[ch.value]
    }

    struct Metrics: Sendable {
        var d, h, italic, skew, w: Double
    }

    private static let tables: [MathFace: [UInt32: Metrics]] = {
        func read(_ text: String) -> [UInt32: Metrics] {
            var out: [UInt32: Metrics] = [:]
            for line in text.split(separator: "\n") {
                let v = line.split(separator: " ")
                guard v.count == 6, let cp = UInt32(v[0]) else { continue }
                let n = v.dropFirst().map { Double($0) ?? 0 }
                out[cp] = Metrics(d: n[0], h: n[1], italic: n[2], skew: n[3], w: n[4])
            }
            return out
        }
        let t = MathTables.self
        return [.main: read(t.mainRegular), .mainBold: read(t.mainBold), .mainItalic: read(t.mainItalic), .mainBoldItalic: read(t.mainBoldItalic),
                .mathItalic: read(t.mathItalic), .mathBoldItalic: read(t.mathBoldItalic), .ams: read(t.amsRegular),
                .caligraphic: read(t.caligraphicRegular), .fraktur: read(t.frakturRegular), .script: read(t.scriptRegular),
                .sansSerif: read(t.sansSerifRegular), .typewriter: read(t.typewriterRegular), .size1: read(t.size1Regular),
                .size2: read(t.size2Regular), .size3: read(t.size3Regular), .size4: read(t.size4Regular)]
    }()

    /// Whatever went wrong upstream, a box the renderer can trust: finite and not negative.
    private static func sanitize(_ box: MathBox) -> MathBox {
        var b = box
        let ok = { (v: Double) in v.isFinite ? v : 0 }
        b.w = max(ok(b.w), 0)
        b.h = ok(b.h)
        b.d = ok(b.d)
        if b.h + b.d < 0 { b.d = -b.h }
        b.items.removeAll { item in
            switch item {
            case .glyph(let x, let y, _, _, let size): !(x.isFinite && y.isFinite && size.isFinite && size > 0)
            case .rule(let x, let y, let w, let h), .frame(let x, let y, let w, let h, _), .strike(let x, let y, let w, let h, _, _):
                ![x, y, w, h].allSatisfy(\.isFinite) || w < 0 || h < 0
            }
        }
        return b
    }
}

/// What a box draws.
nonisolated enum MathItem: Sendable {
    /// Text on a baseline at (x, y), `size` ems tall.
    case glyph(x: Double, y: Double, text: String, face: MathFace, size: Double)
    /// A filled rectangle; `y` its bottom.
    case rule(x: Double, y: Double, w: Double, h: Double)
    /// A rectangle's outline, `t` thick; `y` its bottom.
    case frame(x: Double, y: Double, w: Double, h: Double, t: Double)
    /// A stroke corner to corner, `t` thick: bottom-left to top-right (`forward`) or top-left to
    /// bottom-right; `y` the rectangle's bottom.
    case strike(x: Double, y: Double, w: Double, h: Double, t: Double, forward: Bool)

    func shifted(_ dx: Double, _ dy: Double) -> MathItem {
        switch self {
        case .glyph(let x, let y, let text, let face, let size): .glyph(x: x + dx, y: y + dy, text: text, face: face, size: size)
        case .rule(let x, let y, let w, let h): .rule(x: x + dx, y: y + dy, w: w, h: h)
        case .frame(let x, let y, let w, let h, let t): .frame(x: x + dx, y: y + dy, w: w, h: h, t: t)
        case .strike(let x, let y, let w, let h, let t, let f): .strike(x: x + dx, y: y + dy, w: w, h: h, t: t, forward: f)
        }
    }
}

/// A box: its width, height above the baseline and depth below, and what's drawn in it.
nonisolated struct MathBox: Sendable {
    var w: Double = 0
    var h: Double = 0
    var d: Double = 0
    var items: [MathItem] = []
    /// The italic correction of a lone glyph, which a subscript tucks under.
    var italic: Double = 0
    /// A lone glyph's skew, which centres an accent over its slant.
    var skew: Double = 0
    /// A single character: scripts sit at fixed heights rather than from its edges.
    var charBox = false

    static func kern(_ w: Double) -> MathBox { MathBox(w: w) }

    /// Place `other` with its origin at (x, y) in this box, without changing this box's size.
    mutating func put(_ other: MathBox, _ x: Double, _ y: Double) {
        items.append(contentsOf: other.items.map { $0.shifted(x, y) })
    }

    mutating func append(_ other: MathBox) {
        let x = w
        h = max(h, other.h)
        d = max(d, other.d)
        w += other.w
        put(other, x, 0)
    }

    func shifted(_ dy: Double) -> MathBox {
        var b = self
        b.h += dy
        b.d -= dy
        b.items = items.map { $0.shifted(0, dy) }
        return b
    }
}

/// TeX's font dimensions, for text, script and scriptscript sizes (KaTeX's `sigmasAndXis`).
private nonisolated enum Sigma {
    static let xHeight = [0.431, 0.431, 0.431]
    static let num1 = [0.677, 0.732, 0.925]
    static let num2 = [0.394, 0.384, 0.387]
    static let num3 = [0.444, 0.471, 0.504]
    static let denom1 = [0.686, 0.752, 1.025]
    static let denom2 = [0.345, 0.344, 0.532]
    static let sup1 = [0.413, 0.503, 0.504]
    static let sup2 = [0.363, 0.431, 0.404]
    static let sup3 = [0.289, 0.286, 0.294]
    static let sub1 = [0.150, 0.143, 0.200]
    static let sub2 = [0.247, 0.286, 0.400]
    static let supDrop = [0.386, 0.353, 0.494]
    static let subDrop = [0.050, 0.071, 0.100]
    static let delim1 = [2.390, 1.700, 1.980]
    static let delim2 = [1.010, 1.157, 1.420]
    static let axis = [0.25, 0.25, 0.25]
    static let rule = [0.04, 0.049, 0.049]
    static let bigOp1 = [0.111, 0.111, 0.111]
    static let bigOp2 = [0.166, 0.166, 0.166]
    static let bigOp3 = [0.2, 0.2, 0.2]
    static let bigOp4 = [0.6, 0.611, 0.611]
    static let bigOp5 = [0.1, 0.143, 0.143]
}

/// The style a box is set in.
private nonisolated struct Env {
    var style: MathStyle
    var cramped: Bool

    var size: Double {
        switch style {
        case .display, .text: 1
        case .script: 0.7
        case .scriptScript: 0.5
        }
    }

    /// A font dimension at this style, in ems of the formula.
    func get(_ table: [Double]) -> Double {
        let i = switch style {
        case .display, .text: 0
        case .script: 1
        case .scriptScript: 2
        }
        return table[i] * size
    }

    func with(_ style: MathStyle) -> Env { Env(style: style, cramped: cramped) }
    func cramp() -> Env { Env(style: style, cramped: true) }
    func sup() -> Env { with(style == .display || style == .text ? .script : .scriptScript) }
    func sub() -> Env { sup().cramp() }

    func num() -> Env {
        with(style == .display ? .text : style == .text ? .script : .scriptScript)
    }

    func den() -> Env { num().cramp() }
    var tight: Bool { style == .script || style == .scriptScript }
    var mu: Double { size / 18 }
}

/// The space TeX puts between atoms of classes `l` and `r`, in mu.
private nonisolated func spacing(_ l: MathClass, _ r: MathClass, tight: Bool) -> Double {
    let (mu, always): (Double, Bool) = switch (l, r) {
    case (.ord, .op), (.op, .ord), (.op, .op), (.close, .op), (.inner, .op): (3, true)
    case (.ord, .bin), (.bin, .ord), (.bin, .op), (.bin, .open), (.bin, .inner), (.close, .bin), (.inner, .bin), (.op, .bin): (4, false)
    case (.ord, .rel), (.op, .rel), (.rel, .ord), (.rel, .op), (.rel, .open), (.rel, .inner), (.close, .rel), (.inner, .rel): (5, false)
    case (.ord, .inner), (.inner, .ord), (.inner, .open), (.inner, .punct), (.inner, .inner), (.close, .inner): (3, false)
    case (.punct, .ord), (.punct, .op), (.punct, .rel), (.punct, .open), (.punct, .close), (.punct, .punct), (.punct, .inner): (3, false)
    default: (0, true)
    }
    return always || !tight ? mu : 0
}

private nonisolated struct Typesetter {
    var measure: (String, MathFace) -> Double?

    func m(_ face: MathFace, _ ch: Unicode.Scalar) -> MathLayout.Metrics? { MathLayout.metrics(face, ch) }

    func list(_ nodes: [MathNode], _ env: Env) -> MathBox {
        // Atoms with their classes; `nil` is space, which doesn't change the neighbours.
        var atoms: [(MathClass?, MathBox)] = []
        atoms.reserveCapacity(nodes.count)
        for node in nodes {
            switch node {
            case .space(let em): atoms.append((nil, .kern(em * env.size)))
            case .tab, .newline: atoms.append((nil, .kern(0.5 * env.size)))
            case .styled(let style, let body): atoms.append((.ord, list(body, Env(style: style, cramped: env.cramped))))
            default: atoms.append(atom(node, env))
            }
        }
        // A binary operator with nothing to operate on is an ordinary symbol (TeX's rules 5, 6).
        let classed = atoms.indices.filter { atoms[$0].0 != nil }
        for (k, i) in classed.enumerated() where atoms[i].0 == .bin {
            let prev = k > 0 ? atoms[classed[k - 1]].0 : nil
            let next = k + 1 < classed.count ? atoms[classed[k + 1]].0 : nil
            let lonely = [nil, .bin, .op, .rel, .open, .punct].contains(prev) || [nil, .rel, .close, .punct].contains(next)
            if lonely { atoms[i].0 = .ord }
        }
        var out = MathBox()
        var prev: MathClass?
        for (cls, b) in atoms {
            if let cls {
                if let p = prev { out.append(.kern(spacing(p, cls, tight: env.tight) * env.mu)) }
                prev = cls
            }
            out.append(b)
            // A lone glyph stays one, for the scripts and accents set on it.
            if atoms.count == 1 {
                out.italic = b.italic
                out.skew = b.skew
                out.charBox = b.charBox
            }
        }
        return out
    }

    func atom(_ node: MathNode, _ env: Env) -> (MathClass?, MathBox) {
        switch node {
        case .sym(let ch, let face, let cls): return (cls, glyph(ch, face, env))
        case .text(let text, let face): return (.ord, self.text(text, face, env))
        case .group(let body): return (.ord, list(body, env))
        case .scripts(let base, let sup, let sub, let limits): return scripts(base, sup, sub, limits, env)
        case .frac(let num, let den, let rule, let style, let delims): return (.ord, frac(num, den, rule, style, delims, env))
        case .sqrt(let index, let body): return (.ord, sqrt(index, body, env))
        case .op(let g, let name, _): return (.op, op(g, name, env))
        case .accent(let accent, let body): return (.ord, self.accent(accent, body, env))
        case .leftRight(let delims, let parts): return (.inner, leftRight(delims, parts, env))
        case .big(let delim, let size, let cls):
            let heights = [1.0, 1.2, 1.8, 2.4, 3.0]
            return (cls, delimiter(delim, heights[min(max(size, 0), 4)] * env.size, env))
        case .boxed(let body): return (.ord, boxed(body, env))
        case .array(let a): return (.ord, array(a, env))
        case .phantom(let body, let width, let height):
            let b = list(body, env)
            return (.ord, MathBox(w: width ? b.w : 0, h: height ? b.h : 0, d: height ? b.d : 0))
        case .cancel(let body, let forward, let back): return (.ord, cancel(body, forward, back, env))
        case .stack(let base, let over, let under):
            var cls = MathClass.ord
            if base.count == 1, case .sym(_, _, .rel) = base[0] { cls = .rel }
            return (cls, limits(list(base, env), 0, over, under, env, 0.1))
        case .arrow(let right, let over, let under): return (.rel, xArrow(right, over, under, env))
        case .not(let inner):
            let (cls, b) = atom(inner, env)
            return (cls ?? .rel, not(b, env))
        case .classed(let cls, let body): return (cls, list(body, env))
        case .tag(let body): return (.ord, list(body, env))
        case .unknown(let name): return (.ord, text(name, .main, env))
        case .space, .tab, .newline, .styled: return (.ord, MathBox())
        }
    }

    func glyph(_ ch: Unicode.Scalar, _ face: MathFace, _ env: Env) -> MathBox {
        let s = env.size
        let found = m(face, ch).map { (face, $0) } ?? m(.main, ch).map { (MathFace.main, $0) }
        guard let (face, x) = found else { return text(String(ch), face, env) }
        return MathBox(w: (x.w + x.italic) * s, h: x.h * s, d: x.d * s, items: [.glyph(x: 0, y: 0, text: String(ch), face: face, size: s)],
                       italic: x.italic * s, skew: x.skew * s, charBox: true)
    }

    /// A run of text in one face, set as a line.
    func text(_ text: String, _ face: MathFace, _ env: Env) -> MathBox {
        let s = env.size
        var h = 0.0, d = 0.0, sum = 0.0
        for c in text.unicodeScalars {
            if let x = m(face, c) ?? m(.main, c) {
                h = max(h, x.h)
                d = max(d, x.d)
                sum += x.w + (face.italic ? x.italic : 0)
            } else {
                h = max(h, 0.72)
                d = max(d, 0.2)
                sum += c.utf8.count > 2 ? 1.0 : 0.55
            }
        }
        if text.trimmingCharacters(in: .whitespaces).isEmpty {
            h = 0
            d = 0
        }
        let w = measure(text, face).flatMap { $0.isFinite && $0 >= 0 ? $0 : nil } ?? sum
        return MathBox(w: w * s, h: h * s, d: d * s, items: [.glyph(x: 0, y: 0, text: text, face: face, size: s)])
    }

    func scripts(_ base: MathNode, _ sup: [MathNode]?, _ sub: [MathNode]?, _ limits: Bool?, _ env: Env) -> (MathClass, MathBox) {
        let (c, b): (MathClass?, MathBox)
        if case .group(let g) = base, g.count == 1 { (c, b) = atom(g[0], env) } else { (c, b) = atom(base, env) }
        let cls = c ?? .ord
        // Operators set their limits above and below in display style, or when asked.
        var opLimits = limits == true
        var isOpGlyph = false
        if case .op(let g, _, let l) = base {
            opLimits = limits ?? (l && env.style == .display)
            isOpGlyph = g != nil
        }
        if opLimits {
            let slant = isOpGlyph ? b.italic : 0
            var gap = 0.0
            if case .accent = base { gap = 0.1 * env.size }
            return (cls, self.limits(b, slant, sup, sub, env, gap))
        }
        let charBox = b.charBox && !isOpGlyph
        let italic = charBox || isOpGlyph ? b.italic : 0
        let supEnv = env.sup()
        let subEnv = env.sub()
        let supBox = sup.map { list($0, supEnv) }
        let subBox = sub.map { list($0, subEnv) }
        var supShift = charBox ? 0 : b.h - supEnv.get(Sigma.supDrop)
        var subShift = charBox ? 0 : b.d + subEnv.get(Sigma.subDrop)
        let minSup = env.style == .display ? env.get(Sigma.sup1) : env.cramped ? env.get(Sigma.sup3) : env.get(Sigma.sup2)
        let xh = env.get(Sigma.xHeight)
        let rule = env.get(Sigma.rule)
        switch (supBox, subBox) {
        case (let p?, let q?):
            supShift = max(supShift, minSup, p.d + 0.25 * xh)
            subShift = max(subShift, env.get(Sigma.sub2))
            let gap = (supShift - p.d) - (q.h - subShift)
            if gap < 4 * rule {
                subShift = 4 * rule - (supShift - p.d) + q.h
                let psi = 0.8 * xh - (supShift - p.d)
                if psi > 0 {
                    supShift += psi
                    subShift -= psi
                }
            }
        case (nil, let q?): subShift = max(subShift, env.get(Sigma.sub1), q.h - 0.8 * xh)
        case (let p?, nil): supShift = max(supShift, minSup, p.d + 0.25 * xh)
        case (nil, nil): break
        }
        let scriptSpace = 0.05
        let x = b.w
        var out = MathBox(w: b.w, h: b.h, d: b.d)
        out.put(b, 0, 0)
        var right = x
        if let p = supBox {
            out.h = max(out.h, supShift + p.h)
            out.d = max(out.d, p.d - supShift)
            right = max(right, x + p.w)
            out.put(p, x, supShift)
        }
        if let q = subBox {
            out.d = max(out.d, subShift + q.d)
            out.h = max(out.h, q.h - subShift)
            right = max(right, x - italic + q.w)
            out.put(q, x - italic, -subShift)
        }
        out.w = right + scriptSpace
        return (cls, out)
    }

    /// `base` with `sup` centred above it and `sub` below, as for `\sum` in display style.
    func limits(_ base: MathBox, _ slant: Double, _ sup: [MathNode]?, _ sub: [MathNode]?, _ env: Env, _ gap: Double) -> MathBox {
        let supBox = sup.map { list($0, env.sup()) }
        let subBox = sub.map { list($0, env.sub()) }
        let w = max(base.w, supBox?.w ?? 0, subBox?.w ?? 0)
        var out = MathBox(w: w, h: base.h, d: base.d)
        out.put(base, (w - base.w) / 2, 0)
        if let p = supBox {
            let kern = max(env.get(Sigma.bigOp1), env.get(Sigma.bigOp3) - p.d, gap)
            let y = base.h + kern + p.d
            out.h = y + p.h + env.get(Sigma.bigOp5)
            out.put(p, (w - p.w) / 2 + slant / 2, y)
        }
        if let q = subBox {
            let kern = max(env.get(Sigma.bigOp2), env.get(Sigma.bigOp4) - q.h, gap)
            let y = base.d + kern + q.h
            out.d = y + q.d + env.get(Sigma.bigOp5)
            out.put(q, (w - q.w) / 2 - slant / 2, -y)
        }
        return out
    }

    func frac(_ num: [MathNode], _ den: [MathNode], _ rule: Bool, _ style: MathStyle?, _ delims: [Unicode.Scalar]?, _ outer: Env) -> MathBox {
        let env = style.map { Env(style: $0, cramped: outer.cramped) } ?? outer
        let n = list(num, env.num())
        let d = list(den, env.den())
        let ruleSpacing = env.get(Sigma.rule)
        let t = rule ? ruleSpacing : 0
        let axis = env.get(Sigma.axis)
        let display = env.style == .display
        var numShift: Double, clearance: Double, denShift: Double
        if display {
            (numShift, clearance, denShift) = (env.get(Sigma.num1), rule ? 3 * t : 7 * ruleSpacing, env.get(Sigma.denom1))
        } else {
            (numShift, clearance, denShift) = (rule ? env.get(Sigma.num2) : env.get(Sigma.num3), rule ? t : 3 * ruleSpacing, env.get(Sigma.denom2))
        }
        if rule {
            let above = (numShift - n.d) - (axis + t / 2)
            if above < clearance { numShift += clearance - above }
            let below = (axis - t / 2) - (d.h - denShift)
            if below < clearance { denShift += clearance - below }
        } else {
            let candidate = (numShift - n.d) - (d.h - denShift)
            if candidate < clearance {
                numShift += 0.5 * (clearance - candidate)
                denShift += 0.5 * (clearance - candidate)
            }
        }
        let width = max(n.w, d.w)
        let pad = 0.12 * env.size
        var out = MathBox(w: width + 2 * pad, h: numShift + n.h, d: denShift + d.d)
        out.put(n, pad + (width - n.w) / 2, numShift)
        out.put(d, pad + (width - d.w) / 2, -denShift)
        if rule { out.items.append(.rule(x: pad, y: axis - t / 2, w: width, h: t)) }
        guard let delims, delims.count == 2 else { return out }
        let size = display ? env.get(Sigma.delim1) : env.get(Sigma.delim2)
        var wrapped = delimiter(delims[0], size, env)
        // The delimiters stand in for the null space on either side.
        out.w -= 2 * pad
        out.items = out.items.map { $0.shifted(-pad, 0) }
        wrapped.append(out)
        wrapped.append(delimiter(delims[1], size, env))
        return wrapped
    }

    func sqrt(_ index: [MathNode]?, _ body: [MathNode], _ env: Env) -> MathBox {
        var inner = list(body, env.cramp())
        if inner.h <= 0 { inner.h = env.get(Sigma.xHeight) }
        let theta = env.get(Sigma.rule)
        let phi = env.style == .display ? env.get(Sigma.xHeight) : theta
        var clearance = theta + phi / 4
        let minHeight = inner.h + inner.d + clearance + theta
        let s = env.size
        // The radical sign: the font's, at the first size tall enough, else built from its
        // pieces (a hook at the bottom, a stem as long as it must be).
        let surd = [MathFace.main, .size1, .size2, .size3, .size4].lazy
            .compactMap { f in self.m(f, "\u{221a}").map { (f, $0) } }
            .first { ($0.1.h + $0.1.d) * s >= minHeight }
        let height = surd.map { ($0.1.h + $0.1.d) * s } ?? minHeight
        let advance = (surd?.1.w ?? 1.056) * s
        let rule = 0.04 * s
        let delimDepth = height - rule
        if delimDepth > inner.h + inner.d + clearance { clearance = (clearance + delimDepth - inner.h - inner.d) / 2 }
        let top = inner.h + clearance + rule
        let bottom = top - height
        var out = MathBox(w: advance + inner.w, h: top + min(theta, rule), d: max(-bottom, inner.d))
        if let (face, x) = surd {
            out.items.append(.glyph(x: 0, y: top - x.h * s, text: "\u{221a}", face: face, size: s))
        } else {
            out.items.append(contentsOf: stacked(["\u{e001}", "\u{e000}", "\u{e000}", "\u{23b7}"], top, bottom, env).items)
        }
        out.items.append(.rule(x: advance - 0.0001, y: top - rule, w: inner.w + 0.0001, h: rule))
        out.put(inner, advance, 0)
        guard let index else { return out }
        let ix = list(index, Env(style: .scriptScript, cramped: false))
        let raise = 0.6 * (out.h - out.d)
        let before = 5 * env.mu
        let back = -10 * env.mu
        var root = MathBox(w: before)
        root.h = max(root.h, raise + ix.h)
        root.d = max(root.d, ix.d - raise)
        root.put(ix, before, raise)
        root.w = max(before + ix.w + back, 0)
        root.append(out)
        return root
    }

    /// A delimiter built from the largest font's pieces, as TeX builds tall ones: `top` and
    /// `bottom` ends (and a `middle`, for braces, when it isn't the `repeat`), with the `repeat`
    /// piece overlapping itself to fill between them. It spans `top` to `bottom`.
    func stacked(_ pieces: [Unicode.Scalar], _ top: Double, _ bottom: Double, _ env: Env) -> MathBox {
        let (topC, repeatC, middle, bottomC) = (pieces[0], pieces[1], pieces[2], pieces[3])
        let s = env.size
        let face = MathFace.size4
        let mm = { (c: Unicode.Scalar) in self.m(face, c) ?? MathLayout.Metrics(d: 0, h: 0.5, italic: 0, skew: 0, w: 0.5) }
        let glyph = { (c: Unicode.Scalar, y: Double) in MathItem.glyph(x: 0, y: y, text: String(c), face: face, size: s) }
        let tm = mm(topC), bm = mm(bottomC)
        var ends = [glyph(topC, top - tm.h * s), glyph(bottomC, bottom + bm.d * s)]
        let topEnd = top - (tm.h + tm.d) * s
        let bottomStart = bottom + (bm.h + bm.d) * s
        var gaps: [(Double, Double)] = []
        if middle != repeatC {
            let md = mm(middle)
            let base = (top + bottom) / 2 - (md.h - md.d) / 2 * s
            ends.append(glyph(middle, base))
            gaps.append((topEnd, base + md.h * s))
            gaps.append((base - md.d * s, bottomStart))
        } else {
            gaps.append((topEnd, bottomStart))
        }
        let rm = mm(repeatC)
        let r = (rm.h + rm.d) * s
        var items: [MathItem] = []
        for (hi, lo) in gaps where hi > lo && r > 0 {
            // Copies that overlap a little, so no seam shows; the last one ends at `lo`.
            let n = min(Int(max(((hi - lo - r) / (r * 0.9)).rounded(.up), 0)) + 1, 200)
            let step = n > 1 ? (hi - lo - r) / Double(n - 1) : 0
            for i in 0..<n {
                let copyTop = n > 1 ? hi - Double(i) * step : (hi + lo + r) / 2
                items.append(glyph(repeatC, copyTop - rm.h * s))
            }
        }
        // The ends go on top of the stem.
        return MathBox(w: tm.w * s, h: top, d: -bottom, items: items + ends)
    }

    func op(_ g: Unicode.Scalar?, _ name: [MathNode], _ env: Env) -> MathBox {
        guard let ch = g else {
            var b = list(name, env)
            b.charBox = false
            return b
        }
        var face: MathFace = env.style == .display ? .size2 : .size1
        if m(face, ch) == nil { face = .main }
        var b = glyph(ch, face, env)
        let shift = (b.h - b.d) / 2 - env.get(Sigma.axis)
        b = b.shifted(-shift)
        b.charBox = false
        return b
    }

    /// `ch` in `face` at `size` ems, as a box.
    func sizedGlyph(_ ch: Unicode.Scalar, _ face: MathFace, _ size: Double) -> MathBox {
        let x = m(face, ch) ?? MathLayout.Metrics(d: 0, h: 0.7, italic: 0, skew: 0, w: 0.5)
        return MathBox(w: x.w * size, h: x.h * size, d: x.d * size, items: [.glyph(x: 0, y: 0, text: String(ch), face: face, size: size)])
    }

    func accent(_ accent: MathAccent, _ body: [MathNode], _ env: Env) -> MathBox {
        let b = list(body, env.cramp())
        let s = env.size
        let single = b.charBox
        let inkW = b.w - (single ? b.italic : 0)
        let theta = env.get(Sigma.rule)
        let xh = env.get(Sigma.xHeight)
        let (bh, bd) = (b.h, b.d)
        var out = MathBox(w: b.w, h: b.h, d: b.d, italic: b.italic)
        let skew = single ? b.skew : 0
        out.put(b, 0, 0)
        switch accent {
        case .glyph, .hat, .tilde:
            // The font's accent; a wide hat or tilde from the first size wide enough.
            let mark: MathBox
            if case .glyph(let c) = accent {
                mark = glyph(c, .main, env)
            } else {
                let c: Unicode.Scalar = accent == .hat ? "\u{2c6}" : "\u{2dc}"
                let face = [MathFace.main, .size1, .size2, .size3, .size4].first { f in m(f, c).map { $0.w * s >= inkW * 0.9 } ?? false } ?? .size4
                mark = glyph(c, face, env)
            }
            let clearance = min(bh, xh)
            let y = bh - clearance + mark.d
            let x = (inkW - mark.w) / 2 + skew
            out.h = max(out.h, y + mark.h)
            out.put(mark, x, y)
        case .vec:
            // A small arrow just above the letter.
            let k = 0.62 * s
            let mark = sizedGlyph("\u{2192}", .main, k)
            let y = max(bh, xh) + 0.06 * s + 0.133 * k
            let x = (inkW - mark.w) / 2 + skew
            out.h = max(out.h, y + mark.h)
            out.put(mark, x, y)
        case .overline:
            let y = bh + 3 * theta
            out.h = y + 2 * theta
            out.items.append(.rule(x: 0, y: y, w: inkW, h: theta))
        case .underline:
            let y = -(bd + 3 * theta) - theta
            out.d = bd + 5 * theta
            out.items.append(.rule(x: 0, y: y, w: inkW, h: theta))
        case .overArrow(let left, let right):
            // The font's arrowheads with a rule between them, as long as the body.
            let k = 0.8 * s
            let base = bh + 0.08 * s + 0.133 * k
            let t = 0.04 * k
            let head = 1.0 * k
            let w = max(inkW, head)
            var (from, to) = (0.0, w)
            if right {
                out.put(sizedGlyph("\u{2192}", .main, k), w - head, base)
                to = w - head * 0.5
            }
            if left {
                out.put(sizedGlyph("\u{2190}", .main, k), 0, base)
                from = head * 0.5
            }
            if to > from { out.items.append(.rule(x: from, y: base + 0.25 * k - t / 2, w: to - from, h: t)) }
            out.w = max(out.w, w)
            out.h = base + 0.367 * k
        case .overBrace, .underBrace:
            // A brace of rules: a bar, ends turned towards the body, a point away from it.
            let over = accent == .overBrace
            let t = 0.05 * s
            let h = 0.22 * s
            let gap = 0.1 * s
            let (bar, end, tip) = over ? (bh + gap + h / 2, bh + gap, bh + gap + h / 2) : (-(bd + gap + h / 2), -(bd + gap + h / 2), -(bd + gap + h))
            let half = h / 2
            out.items.append(.rule(x: 0, y: bar - t / 2, w: inkW, h: t))
            for x in [0, inkW - t] { out.items.append(.rule(x: x, y: end, w: t, h: half)) }
            out.items.append(.rule(x: (inkW - t) / 2, y: tip, w: t, h: half))
            if over { out.h = bh + gap + h } else { out.d = bd + gap + h }
        }
        return out
    }

    func leftRight(_ delims: [Unicode.Scalar], _ parts: [[MathNode]], _ env: Env) -> MathBox {
        let boxes = parts.map { list($0, env) }
        let h = boxes.map(\.h).reduce(0, max)
        let d = boxes.map(\.d).reduce(0, max)
        let height = delimHeight(h, d, env)
        var out = MathBox()
        for (i, b) in boxes.enumerated() {
            let delim = i < delims.count ? delims[i] : MathParse.noDelim
            // `\middle`: a relation's space either side.
            if i > 0 { out.append(.kern(5.0 / 18 * env.size * 0.6)) }
            out.append(delimiter(delim, height, env))
            if i > 0 { out.append(.kern(5.0 / 18 * env.size * 0.6)) }
            out.append(b)
        }
        out.append(delimiter(delims.last ?? MathParse.noDelim, height, env))
        return out
    }

    /// How tall `\left`/`\right` delimiters must be around content `h` high and `d` deep (TeX's
    /// \delimiterfactor and \delimitershortfall).
    func delimHeight(_ h: Double, _ d: Double, _ env: Env) -> Double {
        let axis = env.get(Sigma.axis)
        let dist = max(h - axis, d + axis)
        return max(dist / 500 * 901, 2 * dist - 0.5 * env.size)
    }

    /// The delimiter `ch` at least `height` tall, centred on the maths axis: the font's own glyph
    /// at the first size that's tall enough, or drawn.
    func delimiter(_ ch: Unicode.Scalar, _ height: Double, _ env: Env) -> MathBox {
        let s = env.size
        if ch == MathParse.noDelim { return .kern(0.12 * s) }
        let axis = env.get(Sigma.axis)
        let g: Unicode.Scalar = ch == "|" ? "\u{2223}" : ch == "\u{2016}" ? "\u{2225}" : ch
        let faces: [MathFace] = g == "\u{2223}" || g == "\u{2225}" ? [.main] : [.main, .size1, .size2, .size3, .size4]
        var last: MathFace?
        for face in faces {
            guard let x = m(face, g) else { continue }
            last = face
            if (x.h + x.d) * s >= height - 0.01 {
                let b = glyph(g, face, env)
                var shifted = b.shifted(axis - (b.h - b.d) / 2)
                shifted.charBox = false
                shifted.italic = 0
                return shifted
            }
        }
        let (top, bottom) = (axis + height / 2, axis - height / 2)
        // Taller than any glyph: built from the pieces of the largest size, as TeX does.
        let pieces: [Unicode.Scalar]? = switch ch {
        case "(": ["\u{239b}", "\u{239c}", "\u{239c}", "\u{239d}"]
        case ")": ["\u{239e}", "\u{239f}", "\u{239f}", "\u{23a0}"]
        case "[": ["\u{23a1}", "\u{23a2}", "\u{23a2}", "\u{23a3}"]
        case "]": ["\u{23a4}", "\u{23a5}", "\u{23a5}", "\u{23a6}"]
        case "\u{230a}": ["\u{23a2}", "\u{23a2}", "\u{23a2}", "\u{23a3}"]
        case "\u{230b}": ["\u{23a5}", "\u{23a5}", "\u{23a5}", "\u{23a6}"]
        case "\u{2308}": ["\u{23a1}", "\u{23a2}", "\u{23a2}", "\u{23a2}"]
        case "\u{2309}": ["\u{23a4}", "\u{23a5}", "\u{23a5}", "\u{23a5}"]
        case "{": ["\u{23a7}", "\u{23aa}", "\u{23a8}", "\u{23a9}"]
        case "}": ["\u{23ab}", "\u{23aa}", "\u{23ac}", "\u{23ad}"]
        default: nil
        }
        if let pieces { return stacked(pieces, top, bottom, env) }
        // Bars are straight: rules as long as they must be.
        let bars: [Double] = ch == "|" ? [0.139] : ch == "\u{2016}" ? [0.139, 0.361] : []
        if !bars.isEmpty {
            let t = 0.056 * s
            let w = (bars.count == 1 ? 0.278 : 0.5) * s
            return MathBox(w: w, h: top, d: -bottom, items: bars.map { .rule(x: $0 * s - t / 2, y: bottom, w: t, h: height) })
        }
        // Nothing to build it from: the largest glyph there is.
        guard let face = last else { return glyph(ch, .main, env) }
        let b = glyph(g, face, env)
        return b.shifted(axis - (b.h - b.d) / 2)
    }

    func boxed(_ body: [MathNode], _ env: Env) -> MathBox {
        let b = list(body, env)
        let s = env.size
        let pad = 0.3 * s
        let t = 0.04 * s
        let (w, h, d) = (b.w + 2 * (pad + t), b.h + pad + t, b.d + pad + t)
        var out = MathBox(w: w, h: h, d: d)
        out.put(b, pad + t, 0)
        out.items.append(.frame(x: 0, y: -d, w: w, h: h + d, t: t))
        return out
    }

    func cancel(_ body: [MathNode], _ forward: Bool, _ back: Bool, _ env: Env) -> MathBox {
        var b = list(body, env)
        let s = env.size
        let over = 0.1 * s
        let (x, y, w, h) = (0.0, -b.d - over * 0.5, b.w, b.h + b.d + over)
        if forward { b.items.append(.strike(x: x, y: y, w: w, h: h, t: 0.045 * s, forward: true)) }
        if back { b.items.append(.strike(x: x, y: y, w: w, h: h, t: 0.045 * s, forward: false)) }
        b.charBox = false
        return b
    }

    /// A slash through `b` (`\not`).
    func not(_ box: MathBox, _ env: Env) -> MathBox {
        // The font's own `\not` slash: no width, drawn over a relation 0.778 em wide.
        var b = box
        let s = env.size
        let slash = sizedGlyph("\u{e020}", .main, s)
        b.h = max(b.h, slash.h)
        b.d = max(b.d, slash.d)
        b.put(slash, (b.w - 0.778 * s) / 2, 0)
        b.charBox = false
        return b
    }

    func xArrow(_ right: Bool, _ over: [MathNode], _ under: [MathNode], _ env: Env) -> MathBox {
        let s = env.size
        let o = list(over, env.sup())
        let u = list(under, env.sub())
        let w = max(max(o.w, u.w) + 0.9 * s, 1.4 * s)
        let axis = env.get(Sigma.axis)
        let ah = 0.5 * s
        var out = MathBox(w: w, h: axis + ah / 2, d: max(ah / 2 - axis, 0))
        // The font's arrowhead (its shaft sits on the axis) and a rule for the rest.
        let t = 0.04 * s
        let head = sizedGlyph(right ? "\u{2192}" : "\u{2190}", .main, s)
        let (from, to) = right ? (0, w - 0.5 * s) : (0.5 * s, w)
        out.items.append(.rule(x: from, y: axis - t / 2, w: to - from, h: t))
        out.put(head, right ? w - s : 0, 0)
        if o.w > 0 {
            let y = axis + ah / 2 + 0.111 * s + o.d
            out.h = max(out.h, y + o.h)
            out.put(o, (w - o.w) / 2, y)
        }
        if u.w > 0 {
            let y = axis - ah / 2 - 0.111 * s - u.h
            out.d = max(out.d, -y + u.d)
            out.put(u, (w - u.w) / 2, y)
        }
        return out
    }

    func array(_ array: MathArray, _ env: Env) -> MathBox {
        let s = env.size
        let cellEnv: Env = switch array.kind {
        case .matrix, .cases(display: false): env.with(env.style == .display ? .text : env.style)
        case .small: env.with(.script)
        case .aligned, .gathered, .cases(display: true): env.with(.display)
        }
        let cellS = cellEnv.size
        let rows: [[MathBox]] = array.rows.map { row in
            row.enumerated().map { i, cell in
                // `aligned`'s left-hand columns start with the relation they align on; set them
                // as if something came before it, so it keeps its space.
                array.kind == .aligned && i % 2 == 1 ? list([.group([])] + cell, cellEnv) : list(cell, cellEnv)
            }
        }
        let ncols = max(rows.map(\.count).max() ?? 0, 1)
        var widths = [Double](repeating: 0, count: ncols)
        for row in rows {
            for (i, cell) in row.enumerated() { widths[i] = max(widths[i], cell.w) }
        }
        func align(_ i: Int) -> MathArray.Align {
            switch array.kind {
            case .aligned: i % 2 == 0 ? .right : .left
            case .cases: .left
            default: i < array.cols.count ? array.cols[i] : .center
            }
        }
        // Space before column `i`.
        func gap(_ i: Int) -> Double {
            if i == 0 { return 0 }
            return switch array.kind {
            case .small: 0.5556 * cellS
            case .aligned: i % 2 == 0 ? 1.0 * s : 0
            default: 1.0 * s
            }
        }
        let (stretch, jot): (Double, Double) = switch array.kind {
        case .small: (0.5, 0)
        case .aligned, .gathered: (1.0, 0.3 * s)
        default: (1.0, 0)
        }
        let strutH = 0.7 * 1.2 * stretch * s
        let strutD = 0.3 * 1.2 * stretch * s
        let rowDims = rows.map { row in (row.map(\.h).reduce(strutH, max), row.map(\.d).reduce(strutD, max)) }
        let total = rowDims.reduce(0) { $0 + $1.0 + $1.1 } + jot * Double(max(rowDims.count - 1, 0))
        let axis = env.get(Sigma.axis)
        let top = total / 2 + axis
        let outer = array.kind == .matrix && !array.colRules.isEmpty ? 0.5 * s : 0
        var xs: [Double] = []
        var x = outer
        for (i, w) in widths.enumerated() {
            x += gap(i)
            xs.append(x)
            x += w
        }
        let width = x + outer
        var out = MathBox(w: width, h: top, d: total - top)
        var y = top
        var rowTops: [Double] = []
        for (r, row) in rows.enumerated() {
            let (h, d) = rowDims[r]
            if r > 0 { y -= jot }
            rowTops.append(y)
            let baseline = y - h
            for (i, cell) in row.enumerated() {
                let cx = switch align(i) {
                case .left: xs[i]
                case .center: xs[i] + (widths[i] - cell.w) / 2
                case .right: xs[i] + widths[i] - cell.w
                }
                out.put(cell, cx, baseline)
            }
            y = baseline - d
        }
        rowTops.append(y)
        let t = 0.04 * s
        for r in array.rowRules where r < rowTops.count {
            out.items.append(.rule(x: 0, y: rowTops[r] - t / 2, w: width, h: t))
        }
        for c in array.colRules {
            let cx = c == 0 ? 0 : c >= ncols ? width - t : xs[c] - gap(c) / 2 - t / 2
            out.items.append(.rule(x: cx, y: -(total - top), w: t, h: total))
        }
        let (l, r) = (array.left, array.right)
        if l == MathParse.noDelim && r == MathParse.noDelim { return out }
        let height = delimHeight(out.h, out.d, env)
        var wrapped = l == MathParse.noDelim ? MathBox() : delimiter(l, height, env)
        if case .cases = array.kind { wrapped.append(.kern(0.17 * s)) }
        wrapped.append(out)
        if r != MathParse.noDelim { wrapped.append(delimiter(r, height, env)) }
        return wrapped
    }
}
