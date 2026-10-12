import SwiftUI

// Diagrams of structure rather than values: timelines, flows and layer stacks, laid out as the
// Mac lays them out.

/// Mark `i` of `n`'s share of an entrance `enter` of the way through, eased: they arrive one after
/// another, the last starting a third of the way in.
func vizArrive(_ enter: Double, _ i: Int, _ n: Int) -> Double {
    if enter >= 1 { return 1 }
    let delay = n > 1 ? Double(i) / Double(n - 1) * 0.35 : 0
    let t = min(max((enter - delay) / 0.65, 0), 1)
    return 1 - pow(1 - t, 3)
}

// MARK: Timeline

struct VizTimeline: View {
    var events: [VizEvent]
    var ticks: [String]?
    var unit: String?
    var ctx: VizContext

    private let rowH: CGFloat = 30
    private let labelW: CGFloat = 100

    /// Where an event starts and ends on the axis, and whether it is a milestone (no end).
    private func span(_ e: VizEvent) -> (Double, Double, Bool) {
        let at = { (p: VizTime) -> Double in
            switch p {
            case .number(let n): n
            case .tick(let t): Double(ticks?.firstIndex(of: t) ?? 0)
            }
        }
        let start = at(e.start)
        switch (e.end, ticks) {
        case (nil, _?): return (start + 0.5, start + 0.5, true)
        case (nil, nil): return (start, start, true)
        // A tick names a whole slot: Oct to Nov runs to the end of November.
        case (let end?, _?): return (start, at(end) + 1, false)
        case (let end?, nil): return (start, at(end), false)
        }
    }

    private func when(_ p: VizTime) -> String {
        switch p {
        case .number(let v): VizFormat.withUnit(VizFormat.tick(v, step: 1), unit)
        case .tick(let t): t
        }
    }

    var body: some View {
        let look = ctx.look
        let spans = events.map(span)
        // The axis: the ticks' slots, or a nice scale over the numbers.
        let lo: Double, hi: Double
        let marks: [(at: Double, label: String, grid: Double)]
        if let ticks {
            (lo, hi) = (0, Double(ticks.count))
            marks = ticks.enumerated().map { (Double($0.offset) + 0.5, $0.element, Double($0.offset)) }
        } else {
            let scale = VizScale.nice(spans.map(\.0).min() ?? 0, spans.map(\.1).max() ?? 1, target: 4)
            (lo, hi) = (scale.lo, scale.hi)
            marks = scale.ticks.map { ($0, VizFormat.withUnit(VizFormat.tick($0, step: scale.step), unit), $0) }
        }
        let frac = { (v: Double) in (v - lo) / max(hi - lo, .ulpOfOne) }
        var lanes: [String] = []
        for e in events { if let l = e.lane, !lanes.contains(l) { lanes.append(l) } }
        let laned = !lanes.isEmpty
        // Rows: one per event, or per lane with its events packed into as few rows as fit.
        var rows: [(label: String?, members: [Int])] = []
        if laned {
            for lane in lanes + (events.contains { $0.lane == nil } ? [""] : []) {
                let members = events.indices.filter { (events[$0].lane ?? "") == lane }
                // Touching spans share a row; a milestone keeps room for its label.
                let packed = VizLayout.pack(members.map { (spans[$0].0, spans[$0].2 ? spans[$0].0 + (hi - lo) * 0.18 : spans[$0].1) }, gap: 0)
                for r in 0..<((packed.max() ?? -1) + 1) {
                    rows.append((r == 0 ? (lane.isEmpty ? "Other" : lane) : nil, zip(members, packed).filter { $0.1 == r }.map(\.0)))
                }
            }
        } else {
            rows = events.indices.map { (events[$0].label, [$0]) }
        }
        let grid: [Double] = ticks.map { t in (0...t.count).map { Double($0) / Double(t.count) } } ?? marks.map { frac($0.grid) }
        let firsts = rows.map { $0.label != nil }
        let picked = ctx.selected.wrappedValue
        return VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 0) {
                Color.clear.frame(width: labelW, height: 1)
                GeometryReader { g in
                    ForEach(marks.indices, id: \.self) { m in
                        // Numbers in mono, so they line up; named ticks (months, phases) as words.
                        Text(marks[m].label)
                            .font(.system(size: ticks == nil ? 10.5 : 11.5, design: ticks == nil ? .monospaced : .default))
                            .foregroundStyle(look.muted)
                            .lineLimit(1)
                            .fixedSize()
                            .position(x: frac(marks[m].at) * g.size.width, y: 8)
                    }
                }
                .frame(height: 16)
            }
            HStack(alignment: .top, spacing: 0) {
                VStack(alignment: .leading, spacing: 0) {
                    ForEach(rows.indices, id: \.self) { r in
                        let solo = !laned && rows[r].members.count == 1
                        Text(rows[r].label ?? "")
                            .font(.system(size: 12.5, weight: laned ? .medium : .regular))
                            .foregroundStyle(laned ? look.fg : look.fg.opacity(0.86 * (solo ? ctx.presence(rows[r].members[0]) : 1)))
                            .lineLimit(1)
                            .padding(.trailing, 10)
                            .frame(width: labelW, height: rowH, alignment: .leading)
                            .overlay(alignment: .top) {
                                if laned, rows[r].label != nil, r > 0 { Rectangle().fill(look.hairline).frame(height: 1) }
                            }
                    }
                }
                GeometryReader { g in
                    let w = g.size.width
                    ZStack(alignment: .topLeading) {
                        Canvas { c, size in
                            for x in grid {
                                c.fill(Path(CGRect(x: x * size.width, y: 0, width: 1, height: size.height)), with: .color(look.grid))
                            }
                            if laned {
                                for (r, first) in firsts.enumerated() where first && r > 0 {
                                    c.fill(Path(CGRect(x: 0, y: CGFloat(r) * rowH, width: size.width, height: 1)), with: .color(look.hairline))
                                }
                            }
                        }
                        ForEach(rows.indices, id: \.self) { r in
                            ForEach(rows[r].members, id: \.self) { ix in
                                event(ix, spans[ix], laned: laned, frac: frac, width: w, look: look)
                                    .offset(y: CGFloat(r) * rowH)
                            }
                        }
                        if let p = picked, p < events.count, let r = rows.firstIndex(where: { $0.members.contains(p) }) {
                            let e = events[p]
                            let right = frac(spans[p].1) < 0.55
                            VizReadout(look: look) {
                                Text(e.label).fontWeight(.medium).foregroundStyle(look.fg)
                                Text(e.end.map { "\(when(e.start)) – \(when($0))" } ?? when(e.start)).foregroundStyle(look.muted)
                            }
                            // Beside the event, in the room it leaves: after it, or before it near the end.
                            .padding(right ? .leading : .trailing, right ? frac(spans[p].1) * w + 8 : w - frac(spans[p].0) * w + 8)
                            .frame(width: w, alignment: right ? .leading : .trailing)
                            .offset(y: CGFloat(r) * rowH + rowH / 2 - 26)
                        }
                    }
                }
                .frame(height: CGFloat(rows.count) * rowH)
            }
        }
        .padding(.trailing, 10)
        .vizInset(look)
    }

    @ViewBuilder
    private func event(_ ix: Int, _ s: (Double, Double, Bool), laned: Bool, frac: (Double) -> Double, width w: CGFloat, look: VizLook) -> some View {
        let e = events[ix]
        let color = VizLook.tone(e.tone)
        let presence = ctx.presence(ix)
        let on = ctx.selected.wrappedValue == ix
        let grown = ctx.enter
        Group {
            if s.2 {
                HStack(spacing: 5) {
                    Rectangle().fill(color.opacity(presence)).frame(width: 9, height: 9).rotationEffect(.degrees(45)).frame(width: 14, height: 14)
                    if laned { Text(e.label).font(.system(size: 12)).foregroundStyle(look.fg.opacity(0.86 * presence)).lineLimit(1).fixedSize() }
                }
                .frame(height: rowH)
                .offset(x: frac(s.0) * w - 7)
                .opacity(grown)
            } else {
                let x0 = frac(s.0) * w
                let full = (frac(s.1) - frac(s.0)) * w
                RoundedRectangle(cornerRadius: 4, style: .continuous)
                    .fill(color.opacity(laned ? (on ? 0.34 : 0.2 * presence) : (on ? 1 : 0.85 * presence)))
                    .overlay {
                        if laned {
                            RoundedRectangle(cornerRadius: 4, style: .continuous).strokeBorder(color.opacity(0.55 * presence), lineWidth: 1)
                        }
                    }
                    .overlay(alignment: .leading) {
                        if laned {
                            Text(e.label).font(.system(size: 11.5, weight: .medium)).foregroundStyle(look.fg.opacity(max(presence, 0.4)))
                                .lineLimit(1).padding(.horizontal, 6)
                        }
                    }
                    .clipShape(RoundedRectangle(cornerRadius: 4, style: .continuous))
                    .frame(width: max(full * grown - 2, 4), height: 20)
                    .frame(height: rowH)
                    .offset(x: x0 + 1)
            }
        }
        .contentShape(Rectangle())
        .onTapGesture { ctx.pick(ix) }
        .animation(.easeOut(duration: 0.5).delay(Double(ix) * 0.04), value: ctx.enter)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(e.end.map { "\(e.label), \(when(e.start)) to \(when($0))" } ?? "\(e.label), \(when(e.start))")
    }
}

// MARK: Flow

struct VizFlowChart: View {
    var nodes: [VizNode]
    var edges: [VizEdge]
    var ctx: VizContext
    @State private var width: CGFloat = 330

    var body: some View {
        let look = ctx.look
        let index = { (id: String) in nodes.firstIndex { $0.id == id } ?? 0 }
        let pairs = edges.map { (index($0.from), index($0.to)) }
        var names: [String] = []
        for n in nodes { if let g = n.group, !names.contains(g) { names.append(g) } }
        let groups = nodes.map { n in n.group.flatMap { names.firstIndex(of: $0) } }
        let layout = VizLayout.flow(nodes: nodes.count, edges: pairs, groups: groups)
        let grouped = !names.isEmpty
        let backs = layout.back.contains(true)
        let inner = width - 24 - (backs ? 26 : 0)
        let slots = CGFloat(layout.slots)
        let slotW = inner / slots
        let nodeW = min(slotW - 12, 220)
        // Details when a node has room for them.
        let details = nodeW >= 118 && nodes.contains { $0.detail != nil }
        let nodeH: CGFloat = details ? 46 : 34
        let gap: CGFloat = grouped ? 52 : 38
        let top: CGFloat = grouped ? 26 : 6
        let rankTop = { (r: Int) in top + CGFloat(r) * (nodeH + gap) }
        let height = rankTop(layout.ranks - 1) + nodeH + (grouped ? 14 : 8) + (backs ? 4 : 0)
        // A group gets a container when one fits around its nodes and nobody else's; groups that
        // interleave with others are named on their nodes instead.
        let boxed = names.indices.map { g -> Bool in
            let members = nodes.indices.filter { groups[$0] == g }
            let lo = members.map { layout.x[$0] }.min() ?? 0, hi = members.map { layout.x[$0] }.max() ?? 0
            let r0 = members.map { layout.rank[$0] }.min() ?? 0, r1 = members.map { layout.rank[$0] }.max() ?? 0
            return !nodes.indices.contains { v in groups[v] != g && (r0...r1).contains(layout.rank[v]) && layout.x[v] > lo - 0.99 && layout.x[v] < hi + 0.99 }
        }
        let colors = nodes.map { $0.tone != nil ? VizLook.tone($0.tone) : look.muted }
        let picked = ctx.selected.wrappedValue.flatMap { $0 < nodes.count ? $0 : nil }
        let linked = { (e: Int) in picked.map { pairs[e].0 == $0 || pairs[e].1 == $0 } ?? false }
        let near = { (v: Int) in picked.map { h in h == v || pairs.contains { ($0.0 == h && $0.1 == v) || ($0.1 == h && $0.0 == v) } } ?? true }
        return ZStack(alignment: .topLeading) {
            Canvas { c, _ in
                let cx = { (v: Int) in slotW * (layout.x[v] + 0.5) }
                let edgeColor = look.fg.opacity(look.dark ? 0.26 : 0.24)
                let lit = picked.map { nodes[$0].tone != nil ? colors[$0] : look.fg.opacity(0.75) } ?? edgeColor
                for g in names.indices where boxed[g] {
                    let members = nodes.indices.filter { groups[$0] == g }
                    guard !members.isEmpty else { continue }
                    let lo = members.map { layout.x[$0] }.min()!, hi = members.map { layout.x[$0] }.max()!
                    let r0 = members.map { layout.rank[$0] }.min()!, r1 = members.map { layout.rank[$0] }.max()!
                    let rect = CGRect(x: slotW * lo + 2, y: rankTop(r0) - 22, width: slotW * (hi + 1 - lo) - 4, height: rankTop(r1) - rankTop(r0) + nodeH + 30)
                    let box = Path(roundedRect: rect, cornerRadius: 10, style: .continuous)
                    c.fill(box, with: .color(look.fg.opacity(look.dark ? 0.022 : 0.03)))
                    c.stroke(box, with: .color(look.hairline), lineWidth: 1)
                }
                for (e, (from, to)) in pairs.enumerated() {
                    // The picked node's edges light up in its tone (or ink, untoned).
                    let color = linked(e) ? lit : picked != nil ? edgeColor.opacity(0.45) : edgeColor
                    var path = Path()
                    let tip: CGPoint
                    let dir: (CGFloat, CGFloat)
                    if layout.back[e] || layout.rank[to] <= layout.rank[from] {
                        // Back up the ranks: out of the source's right side and round into the target's.
                        let s = CGPoint(x: cx(from) + nodeW / 2, y: rankTop(layout.rank[from]) + nodeH / 2)
                        let t = CGPoint(x: cx(to) + nodeW / 2 + 2, y: rankTop(layout.rank[to]) + nodeH / 2)
                        path.move(to: s)
                        path.addCurve(to: t, control1: CGPoint(x: s.x + 26, y: s.y), control2: CGPoint(x: t.x + 26, y: t.y))
                        (tip, dir) = (t, (-1, 0))
                    } else {
                        // Down through each rank it crosses, in its own slot there.
                        var way = [CGPoint(x: cx(from), y: rankTop(layout.rank[from]) + nodeH)]
                        for (r, x) in layout.routes[e] {
                            way.append(CGPoint(x: slotW * (x + 0.5), y: rankTop(r)))
                            way.append(CGPoint(x: slotW * (x + 0.5), y: rankTop(r) + nodeH))
                        }
                        let t = CGPoint(x: cx(to), y: rankTop(layout.rank[to]) - 2)
                        way.append(t)
                        path.move(to: way[0])
                        for i in 0..<(way.count - 1) {
                            let (s, t) = (way[i], way[i + 1])
                            if i % 2 == 1 {
                                path.addLine(to: t)
                            } else {
                                let dy = (t.y - s.y) / 2
                                path.addCurve(to: t, control1: CGPoint(x: s.x, y: s.y + dy), control2: CGPoint(x: t.x, y: t.y - dy))
                            }
                        }
                        (tip, dir) = (t, (0, 1))
                    }
                    c.stroke(path, with: .color(color), style: StrokeStyle(lineWidth: linked(e) ? 1.75 : 1.25, lineCap: .round))
                    // The arrowhead, pointing along the curve's end.
                    let (l, hw): (CGFloat, CGFloat) = (6.5, 3.6)
                    let base = CGPoint(x: tip.x - dir.0 * l, y: tip.y - dir.1 * l)
                    var head = Path()
                    head.move(to: tip)
                    head.addLine(to: CGPoint(x: base.x - dir.1 * hw, y: base.y + dir.0 * hw))
                    head.addLine(to: CGPoint(x: base.x + dir.1 * hw, y: base.y - dir.0 * hw))
                    head.closeSubpath()
                    c.fill(head, with: .color(color))
                }
            }
            .opacity(ctx.enter)
            ForEach(names.indices.filter { boxed[$0] }, id: \.self) { g in
                let members = nodes.indices.filter { groups[$0] == g }
                if let r0 = members.map({ layout.rank[$0] }).min() {
                    Text(names[g])
                        .font(.system(size: 11, weight: .medium))
                        .foregroundStyle(look.muted)
                        .fixedSize()
                        .offset(x: slotW * (members.map { layout.x[$0] }.min() ?? 0) + 12, y: rankTop(r0) - 19)
                }
            }
            ForEach(nodes.indices, id: \.self) { v in
                let node = nodes[v]
                let toned = node.tone != nil
                let on = picked == v
                let tag = groups[v].flatMap { boxed[$0] || nodeW < 170 ? nil : names[$0] }
                HStack(spacing: 6) {
                    VStack(alignment: .leading, spacing: 1) {
                        Text(node.label).font(.system(size: 12.5, weight: .medium)).foregroundStyle(look.fg).lineLimit(1)
                        if details { Text(node.detail ?? "").font(.system(size: 11)).foregroundStyle(look.muted).lineLimit(1) }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    if let tag { Text(tag).font(.system(size: 10)).foregroundStyle(look.muted) }
                }
                .padding(.leading, toned ? 13 : 10)
                .padding(.trailing, 8)
                .frame(width: max(nodeW, 40), height: nodeH)
                .background(look.popover, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                .overlay(alignment: .leading) {
                    if toned { Capsule().fill(colors[v]).frame(width: 3).padding(.vertical, 8).padding(.leading, 4) }
                }
                .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous)
                    .strokeBorder(on ? colors[v].opacity(0.8) : look.fg.opacity(look.dark ? 0.13 : 0.12), lineWidth: 1))
                .shadow(color: .black.opacity(look.dark ? 0.35 : 0.06), radius: 1.5, y: 1)
                .opacity(vizArrive(ctx.enter, layout.rank[v], layout.ranks) * (near(v) ? 1 : 0.4))
                .contentShape(Rectangle())
                .onTapGesture { ctx.pick(v) }
                .offset(x: slotW * (layout.x[v] + 0.5) - max(nodeW, 40) / 2, y: rankTop(layout.rank[v]))
                .animation(.easeOut(duration: 0.5).delay(Double(layout.rank[v]) * 0.08), value: ctx.enter)
                .accessibilityElement(children: .combine)
            }
            ForEach(edges.indices, id: \.self) { e in
                let (from, to) = pairs[e]
                if let label = edges[e].label, layout.rank[to] > layout.rank[from] {
                    // Halfway down its first stretch, just under the source.
                    let next = layout.routes[e].first?.1 ?? layout.x[to]
                    Text(label)
                        .font(.system(size: 11))
                        .foregroundStyle(linked(e) ? look.fg : look.muted)
                        .lineLimit(1)
                        .fixedSize()
                        .padding(.horizontal, 6)
                        .frame(height: 19)
                        .background(look.popover, in: Capsule())
                        .overlay(Capsule().strokeBorder(look.hairline, lineWidth: 1))
                        .frame(width: 140)
                        .offset(x: slotW * ((layout.x[from] + next) / 2 + 0.5) - 70, y: rankTop(layout.rank[from]) + nodeH + gap / 2 - 9.5)
                        .opacity(ctx.enter)
                        .allowsHitTesting(false)
                }
            }
        }
        .frame(width: max(inner, 1), height: height, alignment: .topLeading)
        .padding(.trailing, backs ? 26 : 0)
        .vizInset(look)
        .onGeometryChange(for: CGFloat.self) { $0.size.width } action: { width = $0 }
    }
}

// MARK: Layers

struct VizLayers: View {
    var layers: [VizLayer]
    var ctx: VizContext
    @State private var rows: [Int: CGRect] = [:]

    private let column: CGFloat = 116

    var body: some View {
        let look = ctx.look
        let colors = layers.indices.map { layers[$0].tone != nil ? VizLook.tone(layers[$0].tone) : VizLook.series($0) }
        VStack(alignment: .leading, spacing: 4) {
            ForEach(layers.indices, id: \.self) { i in
                let layer = layers[i]
                let on = ctx.selected.wrappedValue == i
                VStack(alignment: .leading, spacing: 5) {
                    HStack(spacing: 8) {
                        Text(String(format: "%02d", i + 1)).font(.system(size: 10.5, design: .monospaced)).foregroundStyle(look.muted)
                        Text(layer.label).font(.system(size: 13.5, weight: .semibold)).foregroundStyle(look.fg).lineLimit(1)
                    }
                    if let d = layer.detail {
                        Text(d).font(.system(size: 12)).foregroundStyle(look.muted).lineSpacing(1.5).fixedSize(horizontal: false, vertical: true)
                    }
                    if !layer.items.isEmpty {
                        VizWrap(spacing: 4, lineSpacing: 4) {
                            ForEach(layer.items, id: \.self) { item in
                                Text(item)
                                    .font(.system(size: 11))
                                    .foregroundStyle(look.fg.opacity(0.82))
                                    .padding(.horizontal, 6)
                                    .frame(height: 20)
                                    .background(look.fg.opacity(0.03), in: RoundedRectangle(cornerRadius: 5, style: .continuous))
                                    .overlay(RoundedRectangle(cornerRadius: 5, style: .continuous).strokeBorder(look.hairline, lineWidth: 1))
                            }
                        }
                    }
                }
                .padding(.horizontal, 11)
                .padding(.vertical, 9)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(on ? look.fg.opacity(0.035) : .clear, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).strokeBorder(on ? colors[i].opacity(0.45) : .clear, lineWidth: 1))
                .opacity(max(ctx.presence(i), 0.45) * (0.3 + 0.7 * vizArrive(ctx.enter, layers.count - 1 - i, layers.count)))
                .contentShape(Rectangle())
                .onTapGesture { ctx.pick(i) }
                .onGeometryChange(for: CGRect.self) { $0.frame(in: .named("viz-layers")) } action: { rows[i] = $0 }
                .accessibilityElement(children: .combine)
            }
        }
        .padding(.vertical, 22)
        .padding(.leading, column)
        .background(alignment: .leading) {
            LayerSlabs(rows: rows, colors: colors, items: layers.map(\.items.count), picked: ctx.selected.wrappedValue,
                       enter: ctx.enter, look: look)
                .frame(width: column)
                .allowsHitTesting(false)
        }
        .coordinateSpace(.named("viz-layers"))
        .vizInset(look, padding: 10)
    }
}

/// The exploded stack beside the layers' rows: each a slab seen from above at an angle, its items
/// as blocks on its face, a leader across to its row. It builds itself bottom first, as a cairn does.
private struct LayerSlabs: View, Animatable {
    var rows: [Int: CGRect]
    var colors: [Color]
    var items: [Int]
    var picked: Int?
    var enter: Double
    var look: VizLook

    var animatableData: Double {
        get { enter }
        set { enter = newValue }
    }

    var body: some View {
        Canvas { c, size in
            let n = colors.count
            let pw = size.width * 0.8
            let ph = pw * 0.44
            let cx = size.width * 0.45
            let thick: CGFloat = 6
            let base = look.gap
            for i in (0..<n).reversed() {
                guard let row = rows[i] else { continue }
                let on = picked == i
                let k = vizArrive(enter, n - 1 - i, n)
                let cy = row.midY - (1 - k) * 28 - (on ? 9 : 0)
                let alpha = k * (picked != nil && !on ? 0.4 : 1)
                guard alpha > 0 else { continue }
                let color = colors[i]
                let (l, t, r, b) = (CGPoint(x: cx - pw / 2, y: cy), CGPoint(x: cx, y: cy - ph / 2), CGPoint(x: cx + pw / 2, y: cy), CGPoint(x: cx, y: cy + ph / 2))
                let down = { (p: CGPoint) in CGPoint(x: p.x, y: p.y + thick) }
                func poly(_ pts: [CGPoint], _ fill: Color, _ a: Double) {
                    var p = Path()
                    p.addLines(pts)
                    p.closeSubpath()
                    // Over the opaque surface, so a slab hides what's behind it.
                    c.fill(p, with: .color(base.opacity(alpha)))
                    c.fill(p, with: .color(fill.opacity(a * alpha)))
                }
                // The slab's two visible sides, then its face.
                poly([l, b, down(b), down(l)], color, look.dark ? 0.42 : 0.5)
                poly([b, r, down(r), down(b)], color, look.dark ? 0.3 : 0.38)
                poly([l, t, r, b], color, on ? 0.34 : look.dark ? 0.2 : 0.16)
                // Its rim: the back edges catch the light, the front ones the tone.
                var rim = Path()
                rim.addLines([l, t, r])
                c.stroke(rim, with: .color(look.fg.opacity((look.dark ? 0.22 : 0.18) * alpha)), lineWidth: 1)
                var front = Path()
                front.addLines([l, b, r])
                c.stroke(front, with: .color(color.opacity((on ? 1 : 0.75) * alpha)), lineWidth: on ? 1.5 : 1)
                // What it holds, as blocks on its face.
                let count = min(items[i], 8)
                if count > 0 {
                    let cols = Int(Double(count).squareRoot().rounded(.up))
                    let lines = (count + cols - 1) / cols
                    let on = { (u: CGFloat, v: CGFloat) in
                        CGPoint(x: l.x + (t.x - l.x) * u + (b.x - l.x) * v, y: l.y + (t.y - l.y) * u + (b.y - l.y) * v)
                    }
                    for j in 0..<count {
                        let (col, line) = (CGFloat(j % cols), CGFloat(j / cols))
                        let (du, dv) = (0.64 / CGFloat(cols), 0.64 / CGFloat(lines))
                        let (u0, v0) = (0.18 + col * du, 0.18 + line * dv)
                        let (u1, v1) = (u0 + du * 0.7, v0 + dv * 0.7)
                        let quad = [on(u0, v0), on(u1, v0), on(u1, v1), on(u0, v1)]
                        let raised = quad.map { CGPoint(x: $0.x, y: $0.y - 2.5) }
                        poly([quad[3], quad[2], quad[1], raised[1], raised[2], raised[3]], color, 0.55)
                        poly(raised, color, look.dark ? 0.75 : 0.62)
                    }
                }
                // A leader from its right corner across to its row.
                if size.width > r.x + 6 {
                    c.fill(Path(CGRect(x: r.x + 4, y: cy - 0.5, width: size.width - r.x - 4, height: 1)),
                           with: .color(on ? color.opacity(0.7) : look.fg.opacity(0.18 * alpha)))
                    c.fill(Path(ellipseIn: CGRect(x: r.x, y: cy - 2, width: 4, height: 4)), with: .color(color.opacity(alpha)))
                }
            }
        }
    }
}
