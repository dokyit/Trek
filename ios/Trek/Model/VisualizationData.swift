import CoreGraphics
import Foundation

// What the visualizations draw with besides their marks: numbers as the Mac writes them, the
// table behind every chart (the Data view, and what the copy actions copy), and the geometry the
// Mac lays them out with (nice scales, squarified treemaps, layered flows, timeline lanes).

nonisolated enum VizFormat {
    /// A number for reading: grouped thousands, compact from ten thousand, at most two decimals.
    static func number(_ value: Double) -> String {
        let a = abs(value)
        func compact(_ div: Double, _ suffix: String) -> String {
            let v = value / div
            let s = (v * 10).rounded().truncatingRemainder(dividingBy: 10) == 0 ? String(format: "%.0f", v) : String(format: "%.1f", v)
            return s + suffix
        }
        if a >= 1e12 { return compact(1e12, "T") }
        if a >= 1e9 { return compact(1e9, "B") }
        if a >= 1e6 { return compact(1e6, "M") }
        if a >= 1e4 { return compact(1e3, "K") }
        let decimals = abs(value.truncatingRemainder(dividingBy: 1)) < 1e-9 ? 0
            : abs((value * 10).truncatingRemainder(dividingBy: 1)) < 1e-9 ? 1 : 2
        return group(String(format: "%.\(decimals)f", value))
    }

    /// A number exactly as given, thousands grouped: for tables, where figures are looked up.
    static func exact(_ value: Double) -> String {
        if abs(value.truncatingRemainder(dividingBy: 1)) < 1e-9, abs(value) < 1e15 { return group(String(format: "%.0f", value)) }
        let text = "\(value)"
        return text.contains("e") ? text : group(text)
    }

    /// A tick label: as many decimals as the step needs, and no more.
    static func tick(_ value: Double, step: Double) -> String {
        if abs(value) >= 1e4 { return number(value) }
        let decimals = (0..<6).first { abs((step * pow(10, Double($0))).truncatingRemainder(dividingBy: 1)) < 1e-6 } ?? 6
        return group(String(format: "%.\(decimals)f", value))
    }

    static func group(_ s: String) -> String {
        let (sign, rest) = s.hasPrefix("-") ? ("-", String(s.dropFirst())) : ("", s)
        let parts = rest.split(separator: ".", maxSplits: 1, omittingEmptySubsequences: false)
        let int = Array(parts[0])
        var out = ""
        for (i, c) in int.enumerated() {
            if i > 0, (int.count - i) % 3 == 0 { out.append(",") }
            out.append(c)
        }
        return parts.count > 1 ? "\(sign)\(out).\(parts[1])" : "\(sign)\(out)"
    }

    /// `text` with its unit: currency before, a percent sign tight after, anything else spaced.
    static func withUnit(_ text: String, _ unit: String?) -> String {
        guard let unit else { return text }
        switch unit {
        case "$", "€", "£", "¥": return text.hasPrefix("-") ? "-\(unit)\(text.dropFirst())" : "\(unit)\(text)"
        case "%": return "\(text)%"
        default: return "\(text) \(unit)"
        }
    }

    /// A share of a whole: "25.4%".
    static func share(_ part: Double, of total: Double, decimals: Int = 1) -> String {
        String(format: "%.\(decimals)f%%", part / total * 100)
    }
}

// MARK: The table behind every chart

nonisolated struct VizDataTable: Equatable, Sendable {
    var columns: [String]
    var rows: [[VizCell]]

    static func text(_ cell: VizCell) -> String {
        switch cell {
        case .number(let n): n.truncatingRemainder(dividingBy: 1) == 0 && abs(n) < 1e15 ? String(format: "%.0f", n) : "\(n)"
        case .text(let t): t
        }
    }

    var markdown: String {
        let escape = { (s: String) in s.replacingOccurrences(of: "|", with: "\\|") }
        var out = "| " + columns.map(escape).joined(separator: " | ") + " |\n"
        let numeric = columns.indices.map { c in rows.allSatisfy { if case .number? = $0[safe: c] { true } else { false } } }
        out += "| " + numeric.map { $0 ? "---:" : "---" }.joined(separator: " | ") + " |\n"
        for row in rows { out += "| " + row.map { escape(Self.text($0)) }.joined(separator: " | ") + " |\n" }
        return out
    }

    var csv: String {
        let field = { (s: String) in s.contains(where: { $0 == "," || $0 == "\"" || $0 == "\n" }) ? "\"\(s.replacingOccurrences(of: "\"", with: "\"\""))\"" : s }
        var out = columns.map(field).joined(separator: ",") + "\n"
        for row in rows { out += row.map { field(Self.text($0)) }.joined(separator: ",") + "\n" }
        return out
    }

    /// The values behind `viz`, for every type that has values (a mockup has none).
    static func of(_ viz: Visualization) -> VizDataTable? {
        func time(_ t: VizTime) -> VizCell {
            switch t {
            case .number(let n): .number(n)
            case .tick(let s): .text(s)
            }
        }
        switch viz.content {
        case .bar(let bars, let x, let y):
            return VizDataTable(columns: [y ?? "Label", x ?? "Value"], rows: bars.map { [.text($0.label), .number($0.value)] })
        case .line(let xs, let series, _, _, _):
            return VizDataTable(columns: [""] + series.map(\.label),
                                rows: xs.indices.map { i in [.text(xs[i])] + series.map { .number($0.values[i]) } })
        case .donut(let parts, _):
            let total = parts.reduce(0) { $0 + $1.value }
            return VizDataTable(columns: ["Part", "Value", "Share"],
                                rows: parts.map { [.text($0.label), .number($0.value), .text(VizFormat.share($0.value, of: total))] })
        case .stats(let tiles):
            return VizDataTable(columns: ["Figure", "Value", "Change"], rows: tiles.map { [.text($0.label), .text($0.value), .text($0.delta ?? "")] })
        case .table(let columns, let rows):
            return VizDataTable(columns: columns, rows: rows.map(\.cells))
        case .heatmap(let xs, let ys, let values):
            return VizDataTable(columns: [""] + xs, rows: zip(ys, values).map { y, row in [.text(y)] + row.map { .number($0) } })
        case .treemap(let items):
            let total = items.reduce(0) { $0 + $1.value }
            return VizDataTable(columns: ["Item", "Weight", "Share"],
                                rows: items.map { [.text($0.label), .number($0.value), .text(VizFormat.share($0.value, of: total))] })
        case .timeline(let events, _, _):
            let lanes = events.contains { $0.lane != nil }
            return VizDataTable(columns: ["Event"] + (lanes ? ["Lane"] : []) + ["Start", "End"], rows: events.map { e in
                [.text(e.label)] + (lanes ? [.text(e.lane ?? "")] : []) + [time(e.start), e.end.map(time) ?? .text("—")]
            })
        case .flow(let nodes, let edges):
            let label = { (id: String) in nodes.first { $0.id == id }?.label ?? "" }
            let grouped = nodes.contains { $0.group != nil }
            return VizDataTable(columns: ["Step"] + (grouped ? ["Group"] : []) + ["Leads to", "Detail"], rows: nodes.map { n in
                let next = edges.filter { $0.from == n.id }.map { e in e.label.map { "\(label(e.to)) (\($0))" } ?? label(e.to) }.joined(separator: ", ")
                return [.text(n.label)] + (grouped ? [.text(n.group ?? "")] : []) + [.text(next), .text(n.detail ?? "")]
            })
        case .layers(let layers):
            return VizDataTable(columns: ["Layer", "Detail", "Contains"],
                                rows: layers.map { [.text($0.label), .text($0.detail ?? ""), .text($0.items.joined(separator: ", "))] })
        case .mockup:
            return nil
        }
    }
}

extension Array {
    nonisolated subscript(safe i: Int) -> Element? { indices.contains(i) ? self[i] : nil }
}

extension Visualization {
    /// A text equivalent for VoiceOver: the values too, so nothing is told by colour alone.
    var accessibleDescription: String {
        var out = "Visualization: \(title). \(summary)"
        func push(_ s: String) {
            if out.count < 4096 { out += "; " + s }
        }
        switch content {
        case .mockup(let nodes):
            func visit(_ nodes: [MockNode]) {
                for n in nodes {
                    if let d = n.device { push("\(d.rawValue) frame") }
                    if let t = n.text { push(t) }
                    if let v = n.value { push(v) }
                    visit(n.children)
                }
            }
            visit(nodes)
        case .flow(let nodes, let edges):
            let label = { (id: String) in nodes.first { $0.id == id }?.label ?? "" }
            for n in nodes {
                switch (n.group, n.detail) {
                case let (g?, d?): push("\(n.label) (\(g)): \(d)")
                case let (g?, nil): push("\(n.label) (\(g))")
                case let (nil, d?): push("\(n.label): \(d)")
                case (nil, nil): push(n.label)
                }
            }
            for e in edges {
                push(e.label.map { "\(label(e.from)) to \(label(e.to)) (\($0))" } ?? "\(label(e.from)) to \(label(e.to))")
            }
        default:
            if let data = VizDataTable.of(self) {
                for row in data.rows {
                    push(zip(row, data.columns).map { cell, column in
                        let value = switch cell {
                        case .number(let n): VizFormat.number(n)
                        case .text(let t): t
                        }
                        return column.isEmpty ? value : "\(column) \(value)"
                    }.joined(separator: ", "))
                }
            }
        }
        return String(out.prefix(4096))
    }
}

// MARK: Geometry

/// A linear axis rounded out to "nice" 1-2-5 steps.
nonisolated struct VizScale: Equatable {
    var lo: Double
    var hi: Double
    var step: Double

    /// `min...max` widened to whole steps of about `target` ticks. A flat range still gets an
    /// axis: around zero when it is zero, else from zero to it.
    static func nice(_ a: Double, _ b: Double, target: Int) -> VizScale {
        var (lo, hi) = (min(a, b), max(a, b))
        if abs(hi - lo) < .ulpOfOne {
            if hi == 0 { hi = 1 } else if hi > 0 { lo = 0 } else { hi = 0 }
        }
        let step = niceRound((hi - lo) / Double(max(target, 1)))
        return VizScale(lo: (lo / step).rounded(.down) * step, hi: (hi / step).rounded(.up) * step, step: step)
    }

    var ticks: [Double] {
        let n = Int(((hi - lo) / step).rounded())
        return (0...min(n, 64)).map { i in
            let v = lo + Double(i) * step
            return abs(v) < step * 1e-9 ? 0 : v
        }
    }

    /// Where `value` falls on the axis, 0 at `lo` to 1 at `hi`.
    func frac(_ value: Double) -> Double { (value - lo) / max(hi - lo, .ulpOfOne) }

    private static func niceRound(_ raw: Double) -> Double {
        guard raw.isFinite, raw > 0 else { return 1 }
        let magnitude = pow(10, log10(raw).rounded(.down))
        let n = raw / magnitude
        return (n < 1.5 ? 1 : n < 3 ? 2 : n < 7 ? 5 : 10) * magnitude
    }
}

nonisolated enum VizLayout {
    /// Squarified treemap (Bruls, Huizing & van Wijk): tiles of area proportional to `weights`
    /// filling `bounds`, as close to square as the order allows. In input order.
    static func squarify(_ weights: [Double], in bounds: CGRect) -> [CGRect] {
        var out = Array(repeating: CGRect.zero, count: weights.count)
        let total = weights.filter { $0 > 0 }.reduce(0, +)
        guard total > 0, bounds.width > 0, bounds.height > 0 else { return out }
        let scale = Double(bounds.width * bounds.height) / total
        let order = weights.indices.filter { weights[$0] > 0 }.sorted { weights[$0] != weights[$1] ? weights[$0] > weights[$1] : $0 < $1 }
        let area = { (i: Int) in CGFloat(weights[i] * scale) }
        func worst(_ row: [Int], _ side: CGFloat) -> CGFloat {
            let sum = row.reduce(0) { $0 + area($1) }
            let lo = row.map(area).min() ?? 0, hi = row.map(area).max() ?? 0
            let (s2, side2) = (sum * sum, side * side)
            return max(side2 * hi / s2, s2 / (side2 * max(lo, .ulpOfOne)))
        }
        var rect = bounds
        var row: [Int] = []
        func place(_ row: [Int]) {
            let sum = row.reduce(0) { $0 + area($1) }
            if rect.width >= rect.height {
                // A column down the left edge.
                let thick = rect.height > 0 ? sum / rect.height : 0
                var y = rect.minY
                for i in row {
                    let h = thick > 0 ? area(i) / thick : 0
                    out[i] = CGRect(x: rect.minX, y: y, width: thick, height: h)
                    y += h
                }
                rect = CGRect(x: rect.minX + thick, y: rect.minY, width: max(rect.width - thick, 0), height: rect.height)
            } else {
                // A row along the top edge.
                let thick = rect.width > 0 ? sum / rect.width : 0
                var x = rect.minX
                for i in row {
                    let w = thick > 0 ? area(i) / thick : 0
                    out[i] = CGRect(x: x, y: rect.minY, width: w, height: thick)
                    x += w
                }
                rect = CGRect(x: rect.minX, y: rect.minY + thick, width: rect.width, height: max(rect.height - thick, 0))
            }
        }
        for i in order {
            let side = min(rect.width, rect.height)
            let grown = row + [i]
            if row.isEmpty || worst(grown, side) <= worst(row, side) {
                row = grown
            } else {
                place(row)
                row = [i]
            }
        }
        if !row.isEmpty { place(row) }
        return out
    }

    /// A layered (Sugiyama-style) layout of a directed graph, ranks running top to bottom.
    struct Flow: Equatable {
        /// Each node's rank (row), 0 at the top.
        var rank: [Int]
        /// Each node's centre across, in slots (0 is the middle of the first slot).
        var x: [Double]
        /// Slots across: the widest rank.
        var slots: Int
        var ranks: Int
        /// Edges that point back up a rank (a cycle): drawn around the side.
        var back: [Bool]
        /// Where each edge crosses the ranks between its ends: (rank, x in slots), top down.
        var routes: [[(Int, Double)]]

        static func == (a: Flow, b: Flow) -> Bool {
            a.rank == b.rank && a.x == b.x && a.slots == b.slots && a.ranks == b.ranks && a.back == b.back
        }
    }

    /// `trek-app`'s `flow_layout`: rank by longest path from the sources (cycles broken where a
    /// depth-first walk meets them), order each rank by group then by the barycentre of its
    /// neighbours, and place nodes over their parents where the order allows.
    static func flow(nodes n: Int, edges: [(Int, Int)], groups inGroups: [Int?]) -> Flow {
        var back = Array(repeating: false, count: edges.count)
        var state = Array(repeating: 0, count: n)
        let out: [[(Int, Int)]] = (0..<n).map { v in edges.enumerated().filter { $0.element.0 == v }.map { ($0.offset, $0.element.1) } }
        for root in 0..<n where state[root] == 0 {
            var stack = [(root, 0)]
            state[root] = 1
            while let (v, next) = stack.last {
                if next < out[v].count {
                    stack[stack.count - 1].1 += 1
                    let (edge, w) = out[v][next]
                    switch state[w] {
                    case 0:
                        state[w] = 1
                        stack.append((w, 0))
                    case 1: back[edge] = true
                    default: break
                    }
                } else {
                    state[v] = 2
                    stack.removeLast()
                }
            }
        }
        var forward = zip(edges, back).filter { !$0.1 }.map(\.0)
        // Longest path: relax in topological order (Kahn's).
        var indegree = Array(repeating: 0, count: n)
        for (_, to) in forward { indegree[to] += 1 }
        var queue = (0..<n).filter { indegree[$0] == 0 }
        var rank = Array(repeating: 0, count: n)
        var head = 0
        while head < queue.count {
            let v = queue[head]
            head += 1
            for (from, to) in forward where from == v {
                rank[to] = max(rank[to], rank[from] + 1)
                indegree[to] -= 1
                if indegree[to] == 0 { queue.append(to) }
            }
        }
        let ranks = (rank.max() ?? -1) + 1
        // An edge across several ranks passes through a virtual node in each rank between.
        let real = n
        var groups: [Int?] = (0..<n).map { $0 < inGroups.count ? inGroups[$0] : nil }
        var chains = Array(repeating: [Int](), count: edges.count)
        var through: [(Int, Int)] = []
        for (e, (from, to)) in edges.enumerated() where !back[e] {
            var prev = from
            if rank[from] + 1 < rank[to] {
                for r in (rank[from] + 1)..<rank[to] {
                    let v = rank.count
                    rank.append(r)
                    groups.append(groups[from])
                    chains[e].append(v)
                    through.append((prev, v))
                    prev = v
                }
            }
            through.append((prev, to))
        }
        forward = through
        let all = rank.count
        var rows = Array(repeating: [Int](), count: max(ranks, 0))
        for v in 0..<all { rows[rank[v]].append(v) }
        let groupKey = { (v: Int) in groups[v] ?? Int.max }
        for r in rows.indices {
            rows[r] = rows[r].enumerated().sorted { (groupKey($0.element), $0.offset) < (groupKey($1.element), $1.offset) }.map(\.element)
        }
        func position() -> [Double] {
            var pos = Array(repeating: 0.0, count: all)
            for row in rows {
                for (i, v) in row.enumerated() { pos[v] = Double(i) - Double(row.count - 1) / 2 }
            }
            return pos
        }
        // A few sweeps down and up: by group, then the mean position of the neighbours just placed.
        for sweep in 0..<4 {
            var pos = position()
            let down = sweep % 2 == 0
            let order: [Int] = down ? Array(1..<max(ranks, 1)) : Array((0..<max(ranks - 1, 0)).reversed())
            for r in order {
                let keyed: [(Int, Double, Int)] = rows[r].enumerated().map { i, v in
                    let near = forward.compactMap { a, b -> Double? in
                        if down, b == v { return pos[a] }
                        if !down, a == v { return pos[b] }
                        return nil
                    }
                    let bary = near.isEmpty ? pos[v] : near.reduce(0, +) / Double(near.count)
                    return (groupKey(v), bary, i)
                }
                let sorted = keyed.sorted { a, b in
                    if a.0 != b.0 { return a.0 < b.0 }
                    if a.1 != b.1 { return a.1 < b.1 }
                    return a.2 < b.2
                }
                rows[r] = sorted.map { rows[r][$0.2] }
                for (i, v) in rows[r].enumerated() { pos[v] = Double(i) - Double(rows[r].count - 1) / 2 }
            }
        }
        // Coordinates in slots: each rank over the mean of its parents, a slot apart.
        let slots = max(rows.map(\.count).max() ?? 1, 1)
        let maxX = Double(slots - 1)
        var x = Array(repeating: 0.0, count: all)
        func settle(_ want: [Double]) -> [Double] {
            var at = want
            for i in at.indices {
                at[i] = min(max(at[i], 0), maxX)
                if i > 0 { at[i] = max(at[i], at[i - 1] + 1) }
            }
            for i in at.indices.reversed() {
                at[i] = min(at[i], i + 1 < at.count ? at[i + 1] - 1 : maxX)
            }
            return at
        }
        for (r, row) in rows.enumerated() {
            let want = row.enumerated().map { i, v -> Double in
                let parents = forward.filter { $0.1 == v && rank[$0.0] < r }.map { x[$0.0] }
                return parents.isEmpty ? Double(i) + Double(slots - row.count) / 2 : parents.reduce(0, +) / Double(parents.count)
            }
            for (v, at) in zip(row, settle(want)) { x[v] = at }
        }
        // Then once back up: a parent moves toward the middle of its children where it can.
        for row in rows.reversed() {
            let want = row.map { v -> Double in
                let kids = forward.filter { $0.0 == v }.map { x[$0.1] }
                return kids.isEmpty ? x[v] : (x[v] + kids.reduce(0, +) / Double(kids.count)) / 2
            }
            for (v, at) in zip(row, settle(want)) { x[v] = at }
        }
        let routes = chains.map { $0.map { (rank[$0], x[$0]) } }
        return Flow(rank: Array(rank.prefix(real)), x: Array(x.prefix(real)), slots: slots, ranks: max(ranks, 1), back: back, routes: routes)
    }

    /// Rows for spans `(start, end)` so none overlap within a row: each takes the first row free
    /// by its start. `gap` keeps touching spans apart.
    static func pack(_ spans: [(Double, Double)], gap: Double) -> [Int] {
        let order = spans.indices.sorted { spans[$0].0 != spans[$1].0 ? spans[$0].0 < spans[$1].0 : $0 < $1 }
        var ends: [Double] = []
        var row = Array(repeating: 0, count: spans.count)
        for i in order {
            let (start, end) = spans[i]
            if let r = ends.firstIndex(where: { $0 + gap <= start }) {
                ends[r] = end
                row[i] = r
            } else {
                row[i] = ends.count
                ends.append(end)
            }
        }
        return row
    }
}
