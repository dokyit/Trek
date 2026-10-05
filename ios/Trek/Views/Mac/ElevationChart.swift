import SwiftUI

/// Basecamp's elevation profile, drawn as the Mac draws it: the climb so far as a mountain with
/// its ridge, the summit flagged in ember, the hiker standing at "now", and the trail ahead in
/// dots. Touch it to read a stretch ("2–3 PM · 4 prompts · 12m of agent time").
struct ElevationChart: View, Animatable {
    /// Nil: a trail with no climb yet (the empty state's).
    var profile: ElevationProfile?
    /// 0–1: how far the mountain has risen (it grows in when it appears).
    var progress: Double = 1
    @Binding var selected: Int?

    var animatableData: Double {
        get { progress }
        set { progress = newValue }
    }

    static let top: CGFloat = 26
    static let axis: CGFloat = 20

    @Environment(\.displayScale) private var scale

    var body: some View {
        GeometryReader { g in
            Canvas { ctx, size in draw(ctx, size) }
                .contentShape(Rectangle())
                .gesture(DragGesture(minimumDistance: 0)
                    .onChanged { pick($0.location.x, width: g.size.width) }
                    .onEnded { _ in selected = nil })
        }
        .sensoryFeedback(.selection, trigger: selected) { _, new in new != nil }
        .accessibilityElement()
        .accessibilityLabel(profile?.line ?? "A flat trail so far")
        .accessibilityValue(profile?.total ?? "")
    }

    /// The stretch under a finger.
    private func pick(_ x: CGFloat, width: CGFloat) {
        guard let n = profile?.buckets.count, n > 0, width > 0 else { return }
        let i = min(max(Int((x / width * CGFloat(n)).rounded(.down)), 0), n - 1)
        if selected != i { selected = i }
    }

    private var nowFraction: Double {
        guard let p = profile else { return 0.5 }
        if let at = p.nowAt { return min(max(at, 0), 1) }
        if let now = p.now, !p.buckets.isEmpty { return (Double(now) + 0.5) / Double(p.buckets.count) }
        return 1
    }

    private func draw(_ ctx: GraphicsContext, _ size: CGSize) {
        let left: CGFloat = 0
        let width = size.width
        let top = Self.top
        let height = max(size.height - Self.top - Self.axis, 8)
        let base = top + height
        let heights = scaledHeights
        let n = heights.count
        let nowX = left + CGFloat(nowFraction) * width
        let xOf = { (i: Int) in left + (CGFloat(i) + 0.5) / CGFloat(max(n, 1)) * width }
        let yOf = { (h: Double) in base - CGFloat(h) * (height - 2) }
        let ink = Trek.foreground

        // The climb so far: every stretch whose middle is behind us, then where we stand now.
        var pts: [CGPoint] = [CGPoint(x: left, y: base)]
        if n > 0 {
            let current = min(Int(nowFraction * Double(n)), n - 1)
            for i in 0..<current { pts.append(CGPoint(x: xOf(i), y: yOf(heights[i]))) }
            pts.append(CGPoint(x: nowX, y: yOf(heights[current])))
        } else {
            pts.append(CGPoint(x: nowX, y: base))
        }
        let ground = pts.last?.y ?? base

        // Catmull-Rom through the points, as cubic Béziers, kept from dipping below the ground.
        func curve(_ path: inout Path) {
            for k in 0..<(pts.count - 1) {
                let p0 = pts[max(k - 1, 0)], p1 = pts[k], p2 = pts[k + 1], p3 = pts[min(k + 2, pts.count - 1)]
                let c1 = CGPoint(x: p1.x + (p2.x - p0.x) / 6, y: min(p1.y + (p2.y - p0.y) / 6, base))
                let c2 = CGPoint(x: p2.x - (p3.x - p1.x) / 6, y: min(p2.y - (p3.y - p1.y) / 6, base))
                path.addCurve(to: p2, control1: c1, control2: c2)
            }
        }
        if pts.count > 1, pts.contains(where: { $0.y < base - 0.5 }) {
            var land = Path()
            land.move(to: CGPoint(x: left, y: base))
            curve(&land)
            land.addLine(to: CGPoint(x: nowX, y: base))
            land.closeSubpath()
            ctx.fill(land, with: .linearGradient(Gradient(colors: [ink.opacity(0.2), ink.opacity(0.02)]),
                                                 startPoint: CGPoint(x: 0, y: top), endPoint: CGPoint(x: 0, y: base)))
            var ridge = Path()
            ridge.move(to: CGPoint(x: left, y: base))
            curve(&ridge)
            ctx.stroke(ridge, with: .color(ink.opacity(0.45)), style: StrokeStyle(lineWidth: 1.5, lineCap: .round, lineJoin: .round))
        }

        // The ground walked, and the trail ahead in dots.
        ctx.fill(Path(CGRect(x: left, y: base, width: max(nowX - left, 0), height: 1)), with: .color(ink.opacity(0.18)))
        var x = nowX + 10
        while x < left + width - 2 {
            ctx.fill(Path(roundedRect: CGRect(x: x, y: base - 0.5, width: 2, height: 2), cornerRadius: 1), with: .color(ink.opacity(0.18)))
            x += 7
        }

        // The stretch being read.
        if let h = selected, h < n {
            let x = xOf(h)
            ctx.fill(Path(CGRect(x: x - 0.5, y: top - 4, width: 1, height: base - top + 4)), with: .color(ink.opacity(0.22)))
            if x <= nowX + 1 {
                let y = yOf(heights[h])
                ctx.fill(Path(ellipseIn: CGRect(x: x - 3.5, y: y - 3.5, width: 7, height: 7)), with: .color(ink.opacity(0.85)))
            }
        }

        // The summit flag: a pole and an ember pennant.
        if let i = profile?.summit, i < n, heights[i] > 0 {
            let fx = min(xOf(i), nowX), fy = yOf(heights[i])
            ctx.fill(Path(CGRect(x: fx - 0.5, y: fy - 20, width: 1, height: 20)), with: .color(ink.opacity(0.45)))
            var pennant = Path()
            pennant.move(to: CGPoint(x: fx + 0.5, y: fy - 20))
            pennant.addLine(to: CGPoint(x: fx + 12, y: fy - 16))
            pennant.addLine(to: CGPoint(x: fx + 0.5, y: fy - 12))
            pennant.closeSubpath()
            ctx.fill(pennant, with: .color(Trek.ember))
        }

        // The hiker, where the day stands.
        let cell = (1.25 * scale).rounded(.up) / scale
        let spriteW = CGFloat(HikerSprite.width) * cell
        let hikerLeft = ((min(max(nowX - spriteW / 2, left), left + width - spriteW)) / cell).rounded() * cell
        HikerSprite.draw(ctx, left: hikerLeft, ground: ground + 1, cell: cell, frame: 1, right: true)

        // Axis labels.
        for tick in profile?.ticks ?? [] {
            let label = ctx.resolve(Text(tick.label).font(.caption2).foregroundStyle(Trek.muted))
            let w = label.measure(in: size).width
            let tx = min(max(left + CGFloat(tick.at) * width, left + w / 2), left + width - w / 2)
            ctx.draw(label, at: CGPoint(x: tx, y: base + 5), anchor: .top)
        }
    }

    /// Each stretch's height, 0–1 of the tallest, risen by `progress`.
    private var scaledHeights: [Double] {
        let values = profile?.buckets.map(\.value) ?? []
        let top = values.max() ?? 0
        return values.map { top > 0 ? $0 / top * progress : 0 }
    }
}

/// The tokens tile's line: tokens used so far through the range, rising left to right.
struct Sparkline: View {
    var points: [Double]

    var body: some View {
        Canvas { ctx, size in
            guard points.count > 1 else { return }
            let step = size.width / CGFloat(points.count - 1)
            var line = Path()
            for (i, v) in points.enumerated() {
                let p = CGPoint(x: CGFloat(i) * step, y: size.height - 1 - CGFloat(min(max(v, 0), 1)) * (size.height - 2))
                if i == 0 { line.move(to: p) } else { line.addLine(to: p) }
            }
            var fill = line
            fill.addLine(to: CGPoint(x: size.width, y: size.height))
            fill.addLine(to: CGPoint(x: 0, y: size.height))
            fill.closeSubpath()
            ctx.fill(fill, with: .linearGradient(Gradient(colors: [Trek.foreground.opacity(0.14), .clear]),
                                                 startPoint: .zero, endPoint: CGPoint(x: 0, y: size.height)))
            ctx.stroke(line, with: .color(Trek.foreground.opacity(0.6)), style: StrokeStyle(lineWidth: 1.5, lineCap: .round, lineJoin: .round))
        }
        .accessibilityHidden(true)
    }
}
