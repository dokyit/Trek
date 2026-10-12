import Charts
import SwiftUI

// Charts of values: bars, lines, a donut, stat tiles, tables, heatmaps and treemaps. Tapping a
// mark picks it out and reads its value (the Mac's hover); tapping it again lets go.

private func caption(_ text: String, _ look: VizLook) -> some View {
    Text(text).font(.system(size: 12)).foregroundStyle(look.muted)
}

/// A plot that reveals left to right as `reveal` goes from 0 to 1 (overhanging a little, so the
/// dots at its ends aren't cut).
private nonisolated struct Reveal: Shape {
    var reveal: Double
    var animatableData: Double {
        get { reveal }
        set { reveal = newValue }
    }

    func path(in rect: CGRect) -> Path {
        Path(CGRect(x: rect.minX - 8, y: rect.minY - 8, width: (rect.width + 16) * reveal, height: rect.height + 16))
    }
}

// MARK: Bar

struct VizBars: View {
    var bars: [VizMark]
    var xLabel: String?
    var yLabel: String?
    var ctx: VizContext

    var body: some View {
        let look = ctx.look
        let lo = bars.map(\.value).reduce(0, min), hi = bars.map(\.value).reduce(0, max)
        let scale = VizScale.nice(lo, hi, target: 3)
        // Room past the scale for the value written at a bar's end.
        let room = (scale.hi - scale.lo) * 0.22
        let domain = (lo < 0 ? scale.lo - room : scale.lo)...(hi > 0 ? scale.hi + room : scale.hi)
        let picked = ctx.selected.wrappedValue
        VStack(alignment: .leading, spacing: 6) {
            if let yLabel { caption(yLabel, look) }
            // Labels in a column of their own, the bars on a track beside them, the scale under
            // the track: as the Mac lays a bar chart out.
            VStack(alignment: .leading, spacing: 6) {
                HStack(spacing: 0) {
                    VStack(alignment: .leading, spacing: 0) {
                        ForEach(bars.indices, id: \.self) { i in
                            Text(bars[i].label)
                                .font(.system(size: 12.5))
                                .foregroundStyle(look.fg.opacity(0.86 * ctx.presence(i)))
                                .lineLimit(1)
                                .truncationMode(.middle)
                                .padding(.trailing, 10)
                                .frame(width: 104, height: 30, alignment: .leading)
                                .contentShape(Rectangle())
                                .onTapGesture { ctx.pick(i) }
                        }
                    }
                    Chart(Array(bars.indices), id: \.self) { i in
                        let bar = bars[i]
                        let on = picked == i
                        BarMark(xStart: .value("Base", 0.0), xEnd: .value(xLabel ?? "Value", bar.value * ctx.enter),
                                y: .value("Label", "\(i)"), height: .fixed(15))
                            .cornerRadius(4)
                            .foregroundStyle(VizLook.tone(bar.tone).opacity(on ? 1 : 0.86 * ctx.presence(i)))
                            .annotation(position: bar.value < 0 ? .leading : .trailing, alignment: .center, spacing: 6) {
                                Text(VizFormat.number(bar.value))
                                    .font(.system(size: 11.5, design: .monospaced))
                                    .foregroundStyle(look.fg.opacity(on ? 1 : 0.78 * ctx.presence(i)))
                                    .opacity(ctx.enter)
                            }
                    }
                    .chartXScale(domain: domain)
                    .chartXAxis {
                        AxisMarks(values: scale.ticks) { v in
                            AxisGridLine(stroke: StrokeStyle(lineWidth: 1)).foregroundStyle((v.as(Double.self) ?? 0) == 0 ? look.hairline : look.grid)
                        }
                    }
                    .chartYAxis(.hidden)
                    .chartOverlay { proxy in
                        GeometryReader { g in
                            Rectangle().fill(.clear).contentShape(Rectangle())
                                .onTapGesture { location in
                                    guard let plot = proxy.plotFrame else { return }
                                    if let s: String = proxy.value(atY: location.y - g[plot].minY), let i = Int(s) { ctx.pick(i) }
                                }
                        }
                    }
                    .frame(height: CGFloat(bars.count) * 30)
                }
                HStack(spacing: 0) {
                    Color.clear.frame(width: 104, height: 1)
                    GeometryReader { g in
                        ForEach(scale.ticks, id: \.self) { t in
                            Text(VizFormat.tick(t, step: scale.step))
                                .font(.system(size: 10.5, design: .monospaced))
                                .foregroundStyle(look.muted)
                                .fixedSize()
                                .position(x: (t - domain.lowerBound) / (domain.upperBound - domain.lowerBound) * g.size.width, y: 7)
                        }
                    }
                    .frame(height: 14)
                }
            }
            .vizInset(look)
            if let xLabel {
                caption(xLabel, look).frame(maxWidth: .infinity)
            }
        }
    }
}

// MARK: Line

struct VizLine: View {
    var xLabels: [String]
    var series: [VizSeries]
    var area: Bool
    var yLabel: String?
    var unit: String?
    var ctx: VizContext

    var body: some View {
        let look = ctx.look
        let n = xLabels.count
        let visible = series.indices.filter { !ctx.hidden.wrappedValue.contains($0) }
        let all = visible.flatMap { series[$0].values }
        let mn = all.min() ?? 0, mx = all.max() ?? 1
        // Include zero unless the values sit far from it: a line's shape matters more than its base.
        let scale = VizScale.nice(mn >= 0 && mn <= mx * 0.5 ? 0 : mn, mx, target: 3)
        let colors = series.indices.map { VizLook.series($0, tone: series[$0].tone) }
        let base = min(max(0, scale.lo), scale.hi)
        // Overlapping washes muddy each other: the more lines, the fainter each.
        let wash = area ? (visible.count > 1 ? 0.06 : 0.14) : 0.06
        let washed = area || visible.count == 1
        let cursor = ctx.selected.wrappedValue.flatMap { $0 < n ? $0 : nil }
        VStack(alignment: .leading, spacing: 8) {
            if let yLabel {
                caption(yLabel + (unit.map { " (\($0))" } ?? ""), look)
            }
            if series.count > 1 { legend(colors, look) }
            Chart {
                ForEach(visible, id: \.self) { s in
                    ForEach(0..<n, id: \.self) { i in
                        if washed {
                            AreaMark(x: .value("x", i), yStart: .value("Base", base), yEnd: .value(series[s].label, series[s].values[i]),
                                     series: .value("Wash", "w\(s)"))
                                .interpolationMethod(.monotone)
                                .foregroundStyle(colors[s].opacity(wash))
                        }
                        LineMark(x: .value("x", i), y: .value(series[s].label, series[s].values[i]), series: .value("Series", "s\(s)"))
                            .interpolationMethod(.monotone)
                            .lineStyle(StrokeStyle(lineWidth: 2, lineCap: .round, lineJoin: .round))
                            .foregroundStyle(colors[s])
                    }
                }
                if let c = cursor {
                    RuleMark(x: .value("x", c)).lineStyle(StrokeStyle(lineWidth: 1)).foregroundStyle(look.fg.opacity(0.24))
                    ForEach(visible, id: \.self) { s in
                        PointMark(x: .value("x", c), y: .value("v", series[s].values[c])).symbolSize(120).foregroundStyle(look.gap)
                        PointMark(x: .value("x", c), y: .value("v", series[s].values[c])).symbolSize(52).foregroundStyle(colors[s])
                    }
                } else if ctx.enter >= 1 {
                    // The latest value, marked at each line's end.
                    ForEach(visible, id: \.self) { s in
                        PointMark(x: .value("x", n - 1), y: .value("v", series[s].values[n - 1])).symbolSize(84).foregroundStyle(look.gap)
                        PointMark(x: .value("x", n - 1), y: .value("v", series[s].values[n - 1])).symbolSize(30).foregroundStyle(colors[s])
                    }
                }
            }
            .chartLegend(.hidden)
            .chartXScale(domain: 0...max(n - 1, 1))
            .chartYScale(domain: scale.lo...scale.hi)
            .chartXAxis {
                AxisMarks(values: shownX(n)) { v in
                    let i = v.as(Int.self) ?? 0
                    AxisValueLabel(anchor: i == 0 ? .topLeading : i == n - 1 ? .topTrailing : .top) {
                        Text(xLabels[min(max(i, 0), n - 1)])
                            .font(.system(size: 11))
                            .foregroundStyle(cursor == i ? look.fg : look.muted)
                    }
                }
            }
            .chartYAxis {
                AxisMarks(position: .leading, values: scale.ticks) { v in
                    let t = v.as(Double.self) ?? 0
                    AxisGridLine(stroke: StrokeStyle(lineWidth: 1)).foregroundStyle(t == 0 ? look.hairline : look.grid)
                    AxisValueLabel {
                        Text(VizFormat.tick(t, step: scale.step)).font(.system(size: 10.5, design: .monospaced)).foregroundStyle(look.muted)
                    }
                }
            }
            .chartPlotStyle { $0.mask { Reveal(reveal: ctx.enter) } }
            .chartOverlay { proxy in
                GeometryReader { g in
                    Rectangle().fill(.clear).contentShape(Rectangle())
                        .onTapGesture { location in
                            guard let plot = proxy.plotFrame else { return }
                            let rect = g[plot]
                            let frac = (location.x - rect.minX) / max(rect.width, 1)
                            ctx.pick(min(max(Int((frac * CGFloat(n - 1)).rounded()), 0), n - 1))
                        }
                    if let c = cursor, let plot = proxy.plotFrame {
                        let rect = g[plot]
                        let x = rect.minX + (proxy.position(forX: c) ?? 0)
                        let left = Double(c) / Double(max(n - 1, 1)) <= 0.5
                        readout(c, colors, look)
                            .padding(left ? .leading : .trailing, left ? x + 12 : g.size.width - x + 12)
                            .padding(.top, rect.minY + 2)
                            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: left ? .topLeading : .topTrailing)
                    }
                }
            }
            .frame(height: 176)
            .padding(.top, 2)
            .vizInset(look, padding: 10)
        }
    }

    /// As many x labels as fit, evenly spread, always the first and the last.
    private func shownX(_ n: Int) -> [Int] {
        let room = max(Int((330 - 38 - 40) / 56.0), 2)
        let every = max(Int((Double(n) / Double(min(room, n))).rounded(.up)), 1)
        return (0..<n).filter { $0 % every == 0 || $0 == n - 1 }.filter { $0 == n - 1 || n - 1 - $0 >= every / 2 + 1 || every == 1 }
    }

    private func readout(_ c: Int, _ colors: [Color], _ look: VizLook) -> some View {
        VizReadout(look: look) {
            Text(xLabels[c]).font(.system(size: 11)).foregroundStyle(look.muted)
            ForEach(series.indices.filter { !ctx.hidden.wrappedValue.contains($0) }, id: \.self) { s in
                HStack(spacing: 8) {
                    Capsule().fill(colors[s]).frame(width: 10, height: 2)
                    Text(VizFormat.withUnit(VizFormat.number(series[s].values[c]), unit))
                        .font(.system(size: 12, weight: .semibold, design: .monospaced))
                        .foregroundStyle(look.fg)
                    Text(series[s].label).foregroundStyle(look.muted).lineLimit(1)
                }
            }
        }
        .frame(minWidth: 120, alignment: .leading)
    }

    /// Chips that switch a series on or off.
    private func legend(_ colors: [Color], _ look: VizLook) -> some View {
        VizWrap {
            ForEach(series.indices, id: \.self) { i in
                let off = ctx.hidden.wrappedValue.contains(i)
                Button {
                    ctx.toggle(i, of: series.count)
                } label: {
                    HStack(spacing: 6) {
                        Capsule().fill(off ? look.muted.opacity(0.4) : colors[i]).frame(width: 10, height: 2)
                        Text(series[i].label)
                            .font(.system(size: 12))
                            .strikethrough(off)
                            .foregroundStyle(off ? look.muted.opacity(0.7) : look.fg.opacity(0.85))
                    }
                    .padding(.horizontal, 9)
                    .frame(height: 26)
                    .overlay(Capsule().strokeBorder(off ? look.grid : look.hairline, lineWidth: 1))
                    .contentShape(Capsule())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(series[i].label)
                .accessibilityValue(off ? "Hidden" : "Shown")
            }
        }
    }
}

// MARK: Donut

struct VizDonut: View {
    var parts: [VizMark]
    var unit: String?
    var ctx: VizContext

    private let diameter: CGFloat = 164
    private let thick: CGFloat = 19

    var body: some View {
        let look = ctx.look
        let colors = parts.indices.map { VizLook.series($0, tone: parts[$0].tone) }
        let total = parts.reduce(0) { $0 + $1.value }
        let picked = ctx.selected.wrappedValue.flatMap { $0 < parts.count ? $0 : nil }
        let outer = diameter / 2 - 4
        // The parts sweep round as the entrance plays: an unseen filler holds the rest of the turn.
        let filler = total * (1 - ctx.enter) / max(ctx.enter, 0.001)
        VStack(spacing: 14) {
            ZStack {
                Circle().strokeBorder(look.grid, lineWidth: thick).padding(4)
                Chart {
                    ForEach(parts.indices, id: \.self) { i in
                        SectorMark(angle: .value("Value", parts[i].value), innerRadius: .fixed(outer - thick),
                                   outerRadius: .fixed(picked == i ? outer + 3 : outer), angularInset: 1.2)
                            .cornerRadius(1.5)
                            .foregroundStyle(colors[i].opacity(picked == nil || picked == i ? 1 : 0.4))
                    }
                    if filler > 0 {
                        SectorMark(angle: .value("Value", filler), innerRadius: .fixed(outer - thick), outerRadius: .fixed(outer))
                            .foregroundStyle(.clear)
                    }
                }
                .chartLegend(.hidden)
                VStack(spacing: 1) {
                    if let p = picked {
                        Text(VizFormat.withUnit(VizFormat.number(parts[p].value), unit)).font(.system(size: 21, weight: .semibold)).foregroundStyle(look.fg)
                        Text(parts[p].label).font(.system(size: 11.5)).foregroundStyle(look.muted).lineLimit(1)
                        Text(VizFormat.share(parts[p].value, of: total)).font(.system(size: 11, design: .monospaced)).foregroundStyle(look.muted)
                    } else {
                        Text(VizFormat.withUnit(VizFormat.number(total), unit)).font(.system(size: 21, weight: .semibold)).foregroundStyle(look.fg)
                        Text("Total").font(.system(size: 11.5)).foregroundStyle(look.muted)
                    }
                }
                .frame(width: diameter - thick * 2 - 20)
                .contentTransition(.numericText())
            }
            .frame(width: diameter, height: diameter)
            .contentShape(Circle())
            .onTapGesture { location in ctx.pick(part(at: location, total: total)) }
            VStack(spacing: 2) {
                ForEach(parts.indices, id: \.self) { i in
                    HStack(spacing: 10) {
                        RoundedRectangle(cornerRadius: 2.5).fill(colors[i]).frame(width: 10, height: 10)
                        Text(parts[i].label).font(.system(size: 13)).foregroundStyle(look.fg).lineLimit(1)
                            .frame(maxWidth: .infinity, alignment: .leading)
                        Text(VizFormat.withUnit(VizFormat.number(parts[i].value), unit))
                            .font(.system(size: 12.5, design: .monospaced)).foregroundStyle(look.fg.opacity(0.85))
                        Text(VizFormat.share(parts[i].value, of: total, decimals: 0))
                            .font(.system(size: 12, design: .monospaced)).foregroundStyle(look.muted)
                            .frame(width: 40, alignment: .trailing)
                    }
                    .padding(.horizontal, 8)
                    .frame(height: 30)
                    .background(picked == i ? look.fg.opacity(0.05) : .clear, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
                    .opacity(max(ctx.presence(i), 0.5))
                    .contentShape(Rectangle())
                    .onTapGesture { ctx.pick(i) }
                }
            }
        }
        .padding(4)
        .vizInset(look, padding: 14)
    }

    /// Which part a tap at `p` landed on: by its angle, within the ring.
    private func part(at p: CGPoint, total: Double) -> Int? {
        let (dx, dy) = (p.x - diameter / 2, p.y - diameter / 2)
        let r = hypot(dx, dy), outer = diameter / 2 - 4
        guard r <= outer + 8, r >= outer - thick - 8 else { return nil }
        var angle = (atan2(dy, dx) + .pi / 2) / (2 * .pi)
        if angle < 0 { angle += 1 }
        var acc = 0.0
        for (i, part) in parts.enumerated() {
            acc += part.value / total
            if angle <= acc { return i }
        }
        return parts.count - 1
    }
}

// MARK: Stats

struct VizStats: View {
    var tiles: [VizTile]
    var ctx: VizContext

    var body: some View {
        let rows = stride(from: 0, to: tiles.count, by: 2).map { Array($0..<min($0 + 2, tiles.count)) }
        VStack(spacing: 8) {
            ForEach(rows, id: \.self) { row in
                HStack(alignment: .top, spacing: 8) {
                    ForEach(row, id: \.self) { i in tile(i) }
                }
                .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    /// A delta's direction from its sign: up, down, or flat (unsigned).
    static func direction(_ delta: String) -> Int {
        switch delta.trimmingCharacters(in: .whitespaces).first {
        case "+", "↑", "▲": 1
        case "-", "−", "–", "↓", "▼": -1
        default: 0
        }
    }

    private func tile(_ i: Int) -> some View {
        let look = ctx.look
        let tile = tiles[i]
        let status = tile.tone.map { VizLook.tone($0) } ?? look.muted
        let accent = tile.tone == nil || tile.tone == .neutral ? Trek.sky : VizLook.tone(tile.tone)
        return VStack(alignment: .leading, spacing: 4) {
            Text(tile.label).font(.system(size: 12.5)).foregroundStyle(look.muted).lineLimit(1)
            VizWrap(spacing: 8, lineSpacing: 2) {
                Text(tile.value).font(.system(size: 24, weight: .semibold)).foregroundStyle(look.fg).lineLimit(1).minimumScaleFactor(0.7)
                if let delta = tile.delta {
                    let dir = Self.direction(delta)
                    HStack(spacing: 2) {
                        if dir != 0 { Image(systemName: dir > 0 ? "arrow.up.right" : "arrow.down.right").font(.system(size: 9.5, weight: .bold)) }
                        Text(delta).font(.system(size: 12, weight: .medium))
                    }
                    .foregroundStyle(status)
                    .padding(.horizontal, 7)
                    .frame(height: 20)
                    .background(status.opacity(0.12), in: Capsule())
                    .padding(.top, 6)
                }
            }
            if let trend = tile.trend {
                VizSparkline(values: trend, line: look.muted.opacity(0.85), accent: accent, gap: look.gap, reveal: ctx.enter)
                    .frame(height: 30)
            }
        }
        .padding(.horizontal, 13)
        .padding(.top, 12)
        .padding(.bottom, tile.trend == nil ? 12 : 8)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .vizInset(look, padding: 0)
        .opacity(0.25 + 0.75 * ctx.enter)
        .animation(.easeOut(duration: 0.5).delay(Double(i) * 0.06), value: ctx.enter)
        .accessibilityElement(children: .combine)
    }
}

/// A trend of recent values: a quiet line over a wash of the tile's colour, its latest value marked.
struct VizSparkline: View {
    var values: [Double]
    var line: Color
    var accent: Color
    var gap: Color
    var reveal: Double

    var body: some View {
        let n = values.count
        let lo = values.min() ?? 0, hi = values.max() ?? 1
        let pad = hi > lo ? (hi - lo) * 0.1 : 1
        Chart {
            ForEach(0..<n, id: \.self) { i in
                AreaMark(x: .value("i", i), yStart: .value("Base", lo - pad), yEnd: .value("v", values[i]))
                    .interpolationMethod(.monotone)
                    .foregroundStyle(accent.opacity(0.09))
                LineMark(x: .value("i", i), y: .value("v", values[i]))
                    .interpolationMethod(.monotone)
                    .lineStyle(StrokeStyle(lineWidth: 1.5, lineCap: .round))
                    .foregroundStyle(line)
            }
            if reveal >= 1 {
                PointMark(x: .value("i", n - 1), y: .value("v", values[n - 1])).symbolSize(48).foregroundStyle(gap)
                PointMark(x: .value("i", n - 1), y: .value("v", values[n - 1])).symbolSize(20).foregroundStyle(accent)
            }
        }
        .chartXAxis(.hidden)
        .chartYAxis(.hidden)
        .chartLegend(.hidden)
        .chartXScale(domain: 0...max(n - 1, 1))
        .chartYScale(domain: (lo - pad)...(hi + pad))
        .chartPlotStyle { $0.mask { Reveal(reveal: reveal) } }
        .accessibilityHidden(true)
    }
}

// MARK: Table

/// A table's rows (or a chart's values, on its Data view): figures right-aligned in mono, exact;
/// it scrolls sideways rather than wrapping a number.
struct VizTable: View {
    var columns: [String]
    var rows: [VizRow]
    var look: VizLook
    @State private var width: CGFloat = 0

    var body: some View {
        let numeric = columns.indices.map { c in
            rows.contains { if case .number? = $0.cells[safe: c] { true } else { false } }
                && rows.allSatisfy { if case .text(let t)? = $0.cells[safe: c] { t.isEmpty || t == "—" } else { true } }
        }
        let widths = columns.indices.map { c -> CGFloat in
            let longest = rows.map { r -> Int in
                switch r.cells[safe: c] {
                case .number(let n)?: VizFormat.exact(n).count
                case .text(let t)?: min(t.count, 26)
                case nil: 0
                }
            }.max() ?? 4
            return min(max(CGFloat(max(longest, min(columns[c].count, 18))) * 7.6 + 24, 60), 250)
        }
        let total = widths.reduce(0, +) + 8
        let stretch = width > total ? (width - 8) / (total - 8) : 1
        let shape = RoundedRectangle(cornerRadius: 10, style: .continuous)
        ScrollView(.horizontal, showsIndicators: false) {
            VStack(alignment: .leading, spacing: 0) {
                HStack(spacing: 0) {
                    ForEach(columns.indices, id: \.self) { c in
                        Text(columns[c])
                            .font(.system(size: 12, weight: .medium))
                            .foregroundStyle(look.muted)
                            .lineLimit(1)
                            .frame(width: widths[c] * stretch - 20, alignment: numeric[c] ? .trailing : .leading)
                            .padding(.horizontal, 10)
                    }
                }
                .padding(.vertical, 8)
                .overlay(alignment: .bottom) { Rectangle().fill(look.hairline).frame(height: 1) }
                ForEach(rows.indices, id: \.self) { r in
                    let tint = rows[r].tone.map { VizLook.tone($0) }
                    HStack(alignment: .top, spacing: 0) {
                        ForEach(columns.indices, id: \.self) { c in
                            cell(rows[r].cells[safe: c], column: c)
                                .frame(width: widths[c] * stretch - 20, alignment: numeric[c] ? .trailing : .leading)
                                .padding(.horizontal, 10)
                        }
                    }
                    .padding(.vertical, 8)
                    .background(tint?.opacity(0.07) ?? .clear)
                    .overlay(alignment: .leading) {
                        if let tint { Capsule().fill(tint).frame(width: 2).padding(.vertical, 5) }
                    }
                    .overlay(alignment: .bottom) {
                        if r + 1 < rows.count { Rectangle().fill(look.grid).frame(height: 1) }
                    }
                }
            }
            .padding(.horizontal, 4)
            .padding(.bottom, 4)
            .background(look.inset, in: shape)
            .overlay(shape.strokeBorder(look.fg.opacity(look.dark ? 0.05 : 0.06), lineWidth: 0.75))
        }
        .scrollBounceBehavior(.basedOnSize, axes: .horizontal)
        .onGeometryChange(for: CGFloat.self) { $0.size.width } action: { width = $0 }
        .overlay(alignment: .trailing) {
            // Wider than the column: it scrolls sideways, and a fade at the edge says so.
            if width > 0, total > width + 1 {
                LinearGradient(colors: [look.gap.opacity(0), look.gap.opacity(0.9)], startPoint: .leading, endPoint: .trailing)
                    .frame(width: 28)
                    .clipShape(UnevenRoundedRectangle(bottomTrailingRadius: 10, topTrailingRadius: 10, style: .continuous))
                    .allowsHitTesting(false)
            }
        }
    }

    @ViewBuilder
    private func cell(_ value: VizCell?, column c: Int) -> some View {
        switch value {
        case .number(let n)?:
            // A table is where the exact figure lives: grouped, never abbreviated.
            Text(VizFormat.exact(n)).font(.system(size: 12.5, design: .monospaced)).foregroundStyle(look.fg.opacity(c == 0 ? 1 : 0.86)).lineLimit(1)
        case .text(let t)?:
            Text(t).font(.system(size: 13)).foregroundStyle(look.fg.opacity(c == 0 ? 1 : 0.86)).fixedSize(horizontal: false, vertical: true)
        case nil:
            Text("")
        }
    }
}

// MARK: Heatmap

struct VizHeatmap: View {
    var xLabels: [String]
    var yLabels: [String]
    var values: [[Double]]
    var ctx: VizContext
    @State private var width: CGFloat = 330

    var body: some View {
        let look = ctx.look
        let flat = values.flatMap { $0 }
        let mn = flat.min() ?? 0, mx = flat.max() ?? 1
        // Values on both sides of zero diverge from it (two hues); otherwise one hue, light to dark.
        let diverging = mn < 0 && mx > 0
        let reach = max(abs(mn), abs(mx), .ulpOfOne)
        let span = max(mx - mn, .ulpOfOne)
        let labelW: CGFloat = 72
        let cols = xLabels.count
        let cellW = (width - 24 - labelW - CGFloat(cols) * 3) / CGFloat(max(cols, 1))
        let every = max(Int((40 / max(cellW, 1)).rounded(.up)), 1)
        let numbers = cellW >= 30
        let picked = ctx.selected.wrappedValue.map { ($0 / max(cols, 1), $0 % max(cols, 1)) }
        let paint = { (v: Double) -> (Color, Double) in
            diverging ? (v < 0 ? Trek.failed : Trek.sky, abs(v) / reach) : (Trek.sky, (v - mn) / span)
        }
        VStack(alignment: .trailing, spacing: 8) {
            VStack(spacing: 3) {
                HStack(spacing: 3) {
                    Color.clear.frame(width: labelW, height: 1)
                    ForEach(xLabels.indices, id: \.self) { c in
                        Text(c % every == 0 ? xLabels[c] : "")
                            .font(.system(size: 11))
                            .foregroundStyle(picked?.1 == c ? look.fg : look.muted)
                            .lineLimit(1)
                            .frame(maxWidth: .infinity)
                    }
                }
                ForEach(values.indices, id: \.self) { r in
                    HStack(spacing: 3) {
                        Text(yLabels[safe: r] ?? "")
                            .font(.system(size: 12))
                            .foregroundStyle(picked?.0 == r ? look.fg : look.muted)
                            .lineLimit(1)
                            .frame(width: labelW - 8, alignment: .leading)
                            .padding(.trailing, 8)
                        ForEach(values[r].indices, id: \.self) { c in
                            let (color, strength) = paint(values[r][c])
                            let s = min(max(strength, 0), 1)
                            let onDark = look.dark == (s < 0.55)
                            let ix = r * cols + c
                            RoundedRectangle(cornerRadius: 4, style: .continuous)
                                .fill(color.opacity((0.07 + s * 0.85) * ctx.enter))
                                .overlay {
                                    if numbers {
                                        Text(VizFormat.number(values[r][c]))
                                            .font(.system(size: 11, design: .monospaced))
                                            .foregroundStyle(onDark ? Color.white.opacity(0.92) : Color.black.opacity(0.8))
                                            .opacity(ctx.enter)
                                    }
                                }
                                .overlay {
                                    if ctx.selected.wrappedValue == ix {
                                        RoundedRectangle(cornerRadius: 4, style: .continuous).strokeBorder(look.fg.opacity(0.7), lineWidth: 1.5)
                                    }
                                }
                                .frame(minWidth: 8, maxWidth: .infinity)
                                .frame(height: 26)
                                .contentShape(Rectangle())
                                .onTapGesture { ctx.pick(ix) }
                                .animation(.easeOut(duration: 0.45).delay(Double(c) * 0.05), value: ctx.enter)
                                .accessibilityLabel("\(yLabels[safe: r] ?? "") · \(xLabels[safe: c] ?? ""): \(VizFormat.number(values[r][c]))")
                        }
                    }
                }
            }
            .vizInset(look)
            .onGeometryChange(for: CGFloat.self) { $0.size.width } action: { width = $0 }
            HStack(spacing: 8) {
                if let p = picked, p.0 < values.count, p.1 < values[p.0].count {
                    let value = Text(VizFormat.number(values[p.0][p.1])).foregroundStyle(look.fg).fontWeight(.semibold)
                    Text("\(yLabels[safe: p.0] ?? "") · \(xLabels[safe: p.1] ?? ""): \(value)").foregroundStyle(look.muted)
                }
                Spacer(minLength: 8)
                Text(VizFormat.number(diverging ? -reach : mn))
                Capsule()
                    .fill(LinearGradient(colors: diverging ? [Trek.failed.opacity(0.9), Trek.sky.opacity(0.9)] : [Trek.sky.opacity(0.07), Trek.sky.opacity(0.92)],
                                         startPoint: .leading, endPoint: .trailing))
                    .frame(width: 104, height: 6)
                Text(VizFormat.number(diverging ? reach : mx))
            }
            .font(.system(size: 11, design: .monospaced))
            .foregroundStyle(look.muted)
            .lineLimit(1)
        }
    }
}

// MARK: Treemap

struct VizTreemap: View {
    var items: [VizMark]
    var ctx: VizContext

    private let height: CGFloat = 230

    var body: some View {
        let look = ctx.look
        let total = items.reduce(0) { $0 + $1.value }
        let biggest = max(items.map(\.value).max() ?? 1, .ulpOfOne)
        let byWeight = items.indices.sorted { items[$0].value > items[$1].value }
        GeometryReader { g in
            let tiles = VizLayout.squarify(items.map(\.value), in: CGRect(origin: .zero, size: CGSize(width: g.size.width, height: height)))
            ZStack(alignment: .topLeading) {
                ForEach(items.indices, id: \.self) { i in
                    tile(i, tiles[i], look: look, total: total, biggest: biggest, rank: byWeight.firstIndex(of: i) ?? 0)
                }
                if let s = ctx.selected.wrappedValue, s < items.count {
                    let r = tiles[s]
                    VizReadout(look: look) {
                        Text(items[s].label).fontWeight(.medium).foregroundStyle(look.fg)
                        Text("\(VizFormat.number(items[s].value)) · \(VizFormat.share(items[s].value, of: total))")
                            .font(.system(size: 11.5, design: .monospaced)).foregroundStyle(look.muted)
                    }
                    // Clear of the tile it reads: in the other half, on the tile's side.
                    .padding(8)
                    .frame(width: g.size.width, height: height,
                           alignment: Alignment(horizontal: r.midX < g.size.width / 2 ? .leading : .trailing,
                                                vertical: r.midY < height / 2 ? .bottom : .top))
                }
            }
        }
        .frame(height: height)
    }

    private func tile(_ i: Int, _ r: CGRect, look: VizLook, total: Double, biggest: Double, rank: Int) -> some View {
        let item = items[i]
        // Untoned tiles are quiet greys stepped by weight, so a toned one stands out.
        let toned = item.tone != nil
        let color = toned ? VizLook.tone(item.tone) : look.fg
        let k = (item.value / biggest).squareRoot()
        let picked = ctx.selected.wrappedValue == i
        let strength = (toned ? 0.16 + 0.22 * k : 0.03 + 0.07 * k) + (picked ? (toned ? 0.1 : 0.04) : 0)
        let (w, h) = (r.width - 3, r.height - 3)
        return VStack(alignment: .leading, spacing: 3) {
            if w > 54, h > 30 {
                Text(item.label).font(.system(size: 12, weight: .medium)).foregroundStyle(look.fg).lineLimit(1)
                if h > 48 {
                    HStack(spacing: 6) {
                        Text(VizFormat.number(item.value)).foregroundStyle(look.fg.opacity(0.8))
                        if w > 96 { Text(VizFormat.share(item.value, of: total, decimals: 0)).foregroundStyle(look.muted) }
                    }
                    .font(.system(size: 11, design: .monospaced))
                    .lineLimit(1)
                }
            }
        }
        .padding(8)
        .frame(width: max(w, 0), height: max(h, 0), alignment: .topLeading)
        .background(color.opacity(strength * ctx.enter), in: RoundedRectangle(cornerRadius: 6, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 6, style: .continuous).strokeBorder(color.opacity(picked ? 0.6 : toned ? 0.32 : 0.07), lineWidth: 1))
        .clipped()
        .opacity(max(ctx.presence(i), 0.45))
        .contentShape(Rectangle())
        .onTapGesture { ctx.pick(i) }
        .offset(x: r.minX + 1.5, y: r.minY + 1.5)
        .animation(.easeOut(duration: 0.5).delay(Double(rank) * 0.03), value: ctx.enter)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(item.label), \(VizFormat.number(item.value)), \(VizFormat.share(item.value, of: total))")
    }
}
