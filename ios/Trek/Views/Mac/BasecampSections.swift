import SwiftUI

// Basecamp as the Mac draws it now (`basecamp_v2`): what needs the user, what's ready for review,
// then the range in numbers: totals, turns over time, and tables by project and by model. Laid out
// for a phone: the Mac's row of figures in two rows of three, its tables as two-line rows.

/// The Mac's Basecamp, top to bottom (below the range picker).
struct BasecampSections: View {
    var recap: Basecamp
    var newThread: () -> Void

    var body: some View {
        NeedsSections(rows: recap.review)
        if recap.empty == true {
            NoActivity(range: recap.range, newThread: newThread)
        } else {
            if let totals = recap.totals { TotalsGrid(totals: totals) }
            if let chart = recap.chart, !chart.bars.isEmpty { TurnsCard(chart: chart) }
            if let rows = recap.projects, !rows.isEmpty { UsageTable(title: "Projects", rows: rows) }
            if let rows = recap.models, !rows.isEmpty { UsageTable(title: "Models", rows: rows) }
        }
    }
}

/// A section's title, with a count beside it (none at zero).
private struct SectionTitle: View {
    var title: String
    var count: Int? = nil

    var body: some View {
        HStack(spacing: 6) {
            Text(title).fontWeight(.semibold)
            if let count, count > 0 {
                Text("\(count)").monospacedDigit().foregroundStyle(Trek.muted)
            }
        }
        .font(.ui(.subheadline))
        .padding(.horizontal, 4)
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(.isHeader)
    }
}

// MARK: Needs you, Needs review

/// Rows of "Needs review" shown before "Show more", as on the Mac.
private let reviewRows = 8

/// "Needs you" (questions, approvals, plans, failures), then "Needs review" (finished threads not
/// looked at yet), split where the Mac splits them. A row opens its thread.
private struct NeedsSections: View {
    var rows: [ReviewRow]
    @State private var expanded = false

    var body: some View {
        let needs = rows.filter { $0.needsYou == true }
        let review = rows.filter { $0.needsYou != true }
        let shown = expanded ? review : Array(review.prefix(reviewRows))
        VStack(alignment: .leading, spacing: 8) {
            SectionTitle(title: "Needs you", count: needs.count)
            if needs.isEmpty {
                Text("Nothing is waiting on you.")
                    .font(.ui(.subheadline))
                    .foregroundStyle(Trek.muted)
                    .padding(.horizontal, 4)
            } else {
                ReviewList(rows: needs)
            }
        }
        if !review.isEmpty {
            VStack(alignment: .leading, spacing: 8) {
                SectionTitle(title: "Needs review", count: review.count)
                ReviewList(rows: shown)
                if review.count > reviewRows {
                    Button(expanded ? "Show less" : "Show \(review.count - reviewRows) more") {
                        withAnimation(.snappy) { expanded.toggle() }
                    }
                    .font(.ui(.footnote))
                    .foregroundStyle(Trek.muted)
                    .padding(.horizontal, 4)
                }
            }
        }
    }
}

/// Rows between hairlines, in a card.
private struct ReviewList: View {
    var rows: [ReviewRow]

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            ForEach(Array(rows.enumerated()), id: \.element.id) { i, row in
                if i > 0 { Divider().overlay(Trek.foreground.opacity(0.04)) }
                NavigationLink(value: BasecampRoute.thread(row.threadId)) {
                    ReviewRowView(row: row, showsAgent: false)
                }
                .buttonStyle(.plain)
            }
        }
        .card(padding: 14)
    }
}

// MARK: Totals

/// The range's totals: the Mac's row of figures, three to a row.
private struct TotalsGrid: View {
    var totals: BasecampTotals

    private var figures: [(label: String, value: String, alert: Bool)] {
        [
            ("Threads", "\(totals.threads)", false),
            ("Turns", "\(totals.turns)", false),
            ("Tokens", totals.tokensLabel, false),
            ("Cost", totals.cost, false),
            ("Failures", "\(totals.failed ?? 0)", (totals.failed ?? 0) > 0),
            ("Agent time", totals.agentTime, false),
        ]
    }

    var body: some View {
        let line = Trek.foreground.opacity(0.07)
        Grid(horizontalSpacing: 0, verticalSpacing: 0) {
            ForEach(0..<2, id: \.self) { r in
                if r > 0 {
                    Rectangle().fill(line).frame(height: 0.75).gridCellUnsizedAxes(.horizontal)
                }
                GridRow {
                    ForEach(0..<3, id: \.self) { c in
                        let f = figures[r * 3 + c]
                        VStack(alignment: .leading, spacing: 4) {
                            Text(f.label)
                                .font(.ui(.caption))
                                .foregroundStyle(Trek.muted)
                                .lineLimit(1)
                            Text(f.value)
                                .scaledFont(19, weight: .medium)
                                .monospacedDigit()
                                .lineLimit(1)
                                .minimumScaleFactor(0.6)
                                .foregroundStyle(f.alert ? Trek.failed : Trek.foreground)
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.vertical, 12)
                        .padding(.leading, c > 0 ? 14 : 0)
                        .overlay(alignment: .leading) {
                            if c > 0 { Rectangle().fill(line).frame(width: 0.75) }
                        }
                        .accessibilityElement(children: .combine)
                    }
                }
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 2)
        .frame(maxWidth: .infinity)
        .background(Trek.surface, in: RoundedRectangle(cornerRadius: 16, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).strokeBorder(Trek.border, lineWidth: 0.75))
    }
}

// MARK: Turns

/// Turns over the range, a bar a stretch; tap a bar to read it ("2–3 PM · 4 turns · 12K tokens").
private struct TurnsCard: View {
    var chart: BasecampChart
    @State private var selected: Int?
    @State private var grown = 0.0
    @MotionAllowed private var motion

    private var line: String {
        if let s = selected, s < chart.bars.count { return chart.bars[s].line }
        return "Turns per \(chart.step)"
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .firstTextBaseline) {
                Text(line).lineLimit(1).contentTransition(.opacity)
                Spacer(minLength: 8)
                Text(chart.total).monospacedDigit()
            }
            .font(.ui(.footnote))
            .foregroundStyle(selected == nil ? Trek.muted : Trek.foreground)
            TurnsChart(chart: chart, progress: grown, selected: $selected)
        }
        .card(padding: 16)
        .onAppear {
            guard motion else { grown = 1; return }
            grown = 0
            withAnimation(.smooth(duration: 0.8).delay(0.05)) { grown = 1 }
        }
    }
}

/// The bars, drawn as the Mac draws them: muted, at most 28 pt wide, the one being read darker,
/// the axis labels under their bars. They grow in one after another.
private struct TurnsChart: View, Animatable {
    var chart: BasecampChart
    var progress: Double
    /// The bar being read: a tap picks one, tapping it again puts it away.
    @Binding var selected: Int?

    static let height: CGFloat = 96
    /// Room under the bars for their labels, growing with the text.
    @ScaledMetric(relativeTo: .caption2) private var axis: CGFloat = 20
    /// A bar's widest: a week's seven stay bars, not blocks.
    private static let maxBar: CGFloat = 28

    var animatableData: Double {
        get { progress }
        set { progress = newValue }
    }

    private var gap: CGFloat { chart.bars.count > 40 ? 1 : 3 }

    var body: some View {
        GeometryReader { g in
            Canvas { ctx, size in draw(ctx, size) }
                .contentShape(Rectangle())
                .onTapGesture(coordinateSpace: .local) { at in
                    let i = bar(at: at.x, width: g.size.width)
                    withAnimation(.snappy(duration: 0.2)) { selected = selected == i ? nil : i }
                }
        }
        .frame(height: Self.height + axis)
        .sensoryFeedback(.selection, trigger: selected) { _, new in new != nil }
        .accessibilityElement()
        .accessibilityLabel("Turns per \(chart.step)")
        .accessibilityValue(selected.map { chart.bars[$0].line } ?? chart.total)
        .accessibilityAdjustableAction { direction in
            let n = chart.bars.count
            switch direction {
            case .increment: selected = min((selected ?? -1) + 1, n - 1)
            case .decrement: selected = max((selected ?? n) - 1, 0)
            @unknown default: break
            }
        }
    }

    /// The bar under `x`.
    private func bar(at x: CGFloat, width: CGFloat) -> Int {
        let n = chart.bars.count
        return min(max(Int((x / max(width, 1) * CGFloat(n)).rounded(.down)), 0), n - 1)
    }

    private func draw(_ ctx: GraphicsContext, _ size: CGSize) {
        let n = chart.bars.count
        guard n > 0 else { return }
        let ink = Trek.foreground
        let base = Self.height
        let slot = (size.width - gap * CGFloat(n - 1)) / CGFloat(n)
        let top = Double(chart.bars.map(\.turns).max() ?? 0)
        let x0 = { (i: Int) in CGFloat(i) * (slot + gap) }

        // The stretch being read.
        if let s = selected, s < n {
            ctx.fill(Path(CGRect(x: x0(s) - gap / 2, y: 0, width: slot + gap, height: base)), with: .color(ink.opacity(0.05)))
        }
        for (i, b) in chart.bars.enumerated() where b.turns > 0 && top > 0 {
            // One after another: each starts a little after the one before.
            let spread = 0.6
            let t = min(max(progress * (1 + spread) - Double(i) / Double(n) * spread, 0), 1)
            let h = max(CGFloat(Double(b.turns) / top * t) * (base - 2), 2 * t)
            let w = min(slot, Self.maxBar)
            let rect = CGRect(x: x0(i) + (slot - w) / 2, y: base - h, width: w, height: h)
            ctx.fill(Path(roundedRect: rect, cornerRadius: min(1.5, w / 2)), with: .color(ink.opacity(selected == i ? 0.8 : 0.42)))
        }
        // The baseline.
        ctx.fill(Path(CGRect(x: 0, y: base, width: size.width, height: 0.75)), with: .color(ink.opacity(0.12)))
        // Axis labels, each centred under its bar, kept inside the edges; one that would run into
        // the one before (large text, a narrow phone) is left out.
        var taken: CGFloat = -.infinity
        for tick in chart.ticks where tick.bar < n {
            let label = ctx.resolve(Text(tick.label).font(.ui(.caption2)).foregroundStyle(Trek.muted))
            let w = label.measure(in: size).width
            let x = min(max(x0(tick.bar) + slot / 2, w / 2), size.width - w / 2)
            guard x - w / 2 >= taken + 6 else { continue }
            taken = x + w / 2
            ctx.draw(label, at: CGPoint(x: x, y: base + 5), anchor: .top)
        }
    }
}

// MARK: Tables

/// A table by project or by model, a two-line row each: badge and name with its tokens, then its
/// threads, turns and failures with its cost.
private struct UsageTable: View {
    var title: String
    var rows: [BasecampRow]

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            SectionTitle(title: title)
            VStack(alignment: .leading, spacing: 0) {
                ForEach(Array(rows.enumerated()), id: \.element.id) { i, row in
                    if i > 0 { Divider().overlay(Trek.foreground.opacity(0.04)) }
                    UsageTableRow(row: row)
                }
            }
            .card(padding: 14)
        }
    }
}

private struct UsageTableRow: View {
    var row: BasecampRow

    private var figures: Text {
        // Verbatim: the Mac's figures, unlocalized ("1872 turns").
        var text = Text(verbatim: "\(row.threads) \(row.threads == 1 ? "thread" : "threads") · \(row.turns) \(row.turns == 1 ? "turn" : "turns")")
        if let failed = row.failed, failed > 0 {
            text = Text("\(text)\(Text(verbatim: " · "))\(Text(verbatim: "\(failed) failed").foregroundStyle(Trek.failed))")
        }
        return text
    }

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Group {
                if let agent = row.agent {
                    AgentGlyph(agent, size: 18)
                } else if let project = row.project {
                    ProjectBadge(project: project, size: 18)
                } else {
                    Image(systemName: "tray").font(.system(size: 13)).foregroundStyle(Trek.muted)
                }
            }
            .frame(minWidth: 22, minHeight: 20)
            VStack(alignment: .leading, spacing: 3) {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text(row.label).font(.ui(.body)).lineLimit(1)
                    Spacer(minLength: 6)
                    Text("\(row.tokensLabel) tokens").font(.ui(.subheadline).weight(.medium)).monospacedDigit().fixedSize()
                }
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    figures.lineLimit(2)
                    Spacer(minLength: 6)
                    Text(row.cost).fixedSize()
                }
                .font(.ui(.footnote).monospacedDigit())
                .foregroundStyle(Trek.muted)
            }
        }
        .padding(.vertical, 9)
        .accessibilityElement(children: .combine)
    }
}

// MARK: Empty

/// Nothing done in this range yet, and New thread.
private struct NoActivity: View {
    var range: BasecampRange
    var newThread: () -> Void

    private var text: String {
        switch range {
        case .today: "No activity today."
        case .week: "No activity this week."
        case .all: "No activity yet."
        }
    }

    var body: some View {
        HStack(spacing: 12) {
            Text(text).font(.ui(.subheadline)).foregroundStyle(Trek.muted)
            Spacer(minLength: 8)
            Button(action: newThread) {
                Label("New thread", systemImage: "square.and.pencil").fontWeight(.semibold)
            }
            .buttonStyle(.glass)
        }
        .card(padding: 14)
    }
}
