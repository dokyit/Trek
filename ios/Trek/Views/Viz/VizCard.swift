import SwiftUI

// A ```trek-viz block in an answer, drawn natively as the Mac draws it: a card with a hairline
// edge and the sheen of Trek's glass, an inset surface the marks sit on, and readouts over both.
// Marks wear Trek's status and series colours; text always wears text colours. A block still
// being written draws as far as it's written; one that can't be drawn says why over its JSON.

/// The colours every part of a visualization draws with (the Mac's `Look`).
struct VizLook {
    var dark: Bool

    var fg: Color { Trek.foreground }
    var muted: Color { Trek.muted }
    /// Hairline edges and rules.
    var hairline: Color { Trek.foreground.opacity(dark ? 0.10 : 0.11) }
    /// Gridlines: a step quieter than the hairline.
    var grid: Color { Trek.foreground.opacity(dark ? 0.06 : 0.07) }
    /// The card: a step lighter than the column at night, the paper itself by day.
    var card: Color { dark ? Trek.foreground.opacity(0.022) : Trek.background }
    /// The inset surface marks sit on.
    var inset: Color { dark ? Trek.background.opacity(0.6) : Trek.border.opacity(0.42) }
    /// What separates touching marks: opaque.
    var gap: Color { Trek.background }
    var popover: Color { Trek.elevated }
    static let mono = Font.Design.monospaced

    /// A mark's colour for its tone; no tone is the accent.
    static func tone(_ tone: VizTone?) -> Color {
        switch tone ?? .accent {
        case .neutral: Trek.muted
        case .accent: Trek.ember
        case .positive: Trek.done
        case .warning: Trek.approval
        case .negative: Trek.failed
        case .info: Trek.sky
        }
    }

    /// The Mac's sixth series hue: Trek has no token for it.
    private static let pink = Color(light: 0xDB2777, dark: 0xF472B6)

    /// Series `i`'s colour: its tone when it has one, else the next categorical hue (by its place
    /// in the data, so switching others off never repaints it).
    static func series(_ i: Int, tone: VizTone? = nil) -> Color {
        if let tone { return Self.tone(tone) }
        return [Trek.sky, Trek.ember, Trek.done, Trek.question, Trek.approval, pink][i % 6]
    }
}

/// What a kind's view is drawn with: the look, how far the entrance is, and what the reader picked.
struct VizContext {
    var look: VizLook
    /// The entrance, 0 to 1.
    var enter: Double
    var selected: Binding<Int?>
    /// Series or parts switched off from the legend.
    var hidden: Binding<Set<Int>>

    /// Picks mark `i`, or lets go of it when it's already picked.
    func pick(_ i: Int?) {
        withAnimation(.snappy(duration: 0.18)) { selected.wrappedValue = selected.wrappedValue == i ? nil : i }
    }

    /// How present mark `i` is while another is picked: the rest step back.
    func presence(_ i: Int) -> Double {
        if let s = selected.wrappedValue, s != i { return 0.38 }
        return 1
    }

    /// Switches series `i` on or off from the legend; never all of them off.
    func toggle(_ i: Int, of count: Int) {
        withAnimation(.snappy(duration: 0.22)) {
            if hidden.wrappedValue.remove(i) == nil, hidden.wrappedValue.count + 1 < count { hidden.wrappedValue.insert(i) }
        }
    }
}

/// Which visualizations have made their entrance, so scrolling back doesn't replay it.
@MainActor
enum VizEntrance {
    private static var seen: Set<Int> = []
    static func played(_ ident: Int) -> Bool { seen.contains(ident) }
    static func play(_ ident: Int) { seen.insert(ident) }
}

/// One `trek-viz` block: drawn, still arriving, or refused.
struct VizBlockView: View {
    var block: VizBlock
    /// The answer is still being written.
    var live: Bool
    var size: CGFloat

    var body: some View {
        Group {
            if let d = drawable {
                VizCard(viz: d.viz, drawing: d.drawing)
            } else if case .arriving(let p, _) = block, live {
                VizPending(kind: p.kind, title: p.title)
            } else if case .invalid(let reason, let source) = block {
                VizInvalid(reason: reason, source: source, size: size)
            } else if case .arriving(_, let source) = block {
                VizInvalid(reason: "the answer ended before the block was finished", source: source, size: size)
            }
        }
        .padding(.vertical, 4)
        .textSelection(.disabled)
    }

    /// What can be drawn, and whether its marks are still arriving.
    private var drawable: (viz: Visualization, drawing: Bool)? {
        switch block {
        case .drawn(let viz): (viz, false)
        case .arriving(let p, _): p.drawable.flatMap { live || p.complete ? ($0, live && !p.complete) : nil }
        case .invalid: nil
        }
    }
}

// MARK: The card

struct VizCard: View {
    var viz: Visualization
    /// Marks still arriving: nothing whole to copy or tabulate yet.
    var drawing: Bool

    @State private var showData = Launch.vizData
    @State private var selected: Int? = Launch.vizSelect
    @State private var hidden: Set<Int> = []
    @State private var enter: Double
    @Environment(\.colorScheme) private var scheme
    @Environment(AppModel.self) private var model: AppModel?
    @MotionAllowed private var motion

    init(viz: Visualization, drawing: Bool) {
        self.viz = viz
        self.drawing = drawing
        _enter = State(initialValue: VizEntrance.played(viz.ident) ? 1 : 0)
    }

    private var look: VizLook { VizLook(dark: scheme == .dark) }
    private var data: VizDataTable? { VizDataTable.of(viz) }
    private var isTable: Bool { if case .table = viz.content { true } else { false } }

    var body: some View {
        let look = look
        let data = data
        VizShell(look: look) {
            VizHeader(kind: viz.kind, look: look) {
                Text(viz.title).vizTitle(look)
            } summary: {
                Text(viz.summary).vizSummary(look)
            } actions: {
                if drawing {
                    DrawingLabel()
                } else {
                    HStack(spacing: 2) {
                        if data != nil, !isTable { ChartDataSwitch(showData: $showData, selected: $selected, look: look) }
                        Menu {
                            copyItems(data)
                        } label: {
                            Image(systemName: "ellipsis")
                                .font(.system(size: 14, weight: .semibold))
                                .foregroundStyle(look.muted)
                                .frame(width: 30, height: 28)
                                .contentShape(Rectangle())
                        }
                        .accessibilityLabel("Copy")
                    }
                }
            }
            VStack(alignment: .trailing, spacing: 8) {
                if showData, let data {
                    VizTable(columns: data.columns, rows: data.rows.map { VizRow(cells: $0, tone: nil) }, look: look)
                    if !drawing {
                        Button {
                            copy(data.csv, "CSV copied")
                        } label: {
                            Label("CSV", systemImage: "doc.on.doc").font(.system(size: 12)).foregroundStyle(look.muted)
                        }
                        .buttonStyle(.plain)
                    }
                } else {
                    chart(VizContext(look: look, enter: enter, selected: $selected, hidden: $hidden))
                }
            }
            .padding(.horizontal, 14)
            .padding(.bottom, 14)
        }
        .contextMenu {
            if !drawing {
                if data != nil, !isTable {
                    Button(showData ? "Show chart" : "Show data", systemImage: showData ? "chart.bar.xaxis" : "tablecells") {
                        showData.toggle()
                        selected = nil
                    }
                }
                copyItems(data)
            }
        }
        .sensoryFeedback(.selection, trigger: selected) { _, new in new != nil }
        .accessibilityElement(children: .contain)
        .accessibilityLabel(viz.accessibleDescription)
        .onAppear(perform: entrance)
    }

    @ViewBuilder
    private func copyItems(_ data: VizDataTable?) -> some View {
        if let data {
            Button("Copy as Markdown table", systemImage: "tablecells") { copy(data.markdown, "Table copied") }
            Button("Copy as CSV", systemImage: "doc.on.doc") { copy(data.csv, "CSV copied") }
        }
        Button("Copy data as JSON", systemImage: "curlybraces") { copy(viz.json.pretty(), "Visualization data copied") }
    }

    private func copy(_ text: String, _ done: String) {
        UIPasteboard.general.string = text
        model?.show(done)
    }

    /// Marks arrive the first time a visualization is drawn, and only then.
    private func entrance() {
        guard enter < 1 else { return }
        VizEntrance.play(viz.ident)
        if motion {
            withAnimation(.easeOut(duration: 0.72)) { enter = 1 }
        } else {
            enter = 1
        }
    }

    @ViewBuilder
    private func chart(_ ctx: VizContext) -> some View {
        switch viz.content {
        case .bar(let bars, let x, let y): VizBars(bars: bars, xLabel: x, yLabel: y, ctx: ctx)
        case .line(let xs, let series, let area, let y, let unit): VizLine(xLabels: xs, series: series, area: area, yLabel: y, unit: unit, ctx: ctx)
        case .donut(let parts, let unit): VizDonut(parts: parts, unit: unit, ctx: ctx)
        case .stats(let tiles): VizStats(tiles: tiles, ctx: ctx)
        case .table(let columns, let rows): VizTable(columns: columns, rows: rows, look: ctx.look)
        case .heatmap(let xs, let ys, let values): VizHeatmap(xLabels: xs, yLabels: ys, values: values, ctx: ctx)
        case .treemap(let items): VizTreemap(items: items, ctx: ctx)
        case .timeline(let events, let ticks, let unit): VizTimeline(events: events, ticks: ticks, unit: unit, ctx: ctx)
        case .flow(let nodes, let edges): VizFlowChart(nodes: nodes, edges: edges, ctx: ctx)
        case .layers(let layers): VizLayers(layers: layers, ctx: ctx)
        case .mockup(let nodes): VizMockup(nodes: nodes, look: ctx.look, enter: ctx.enter)
        }
    }
}

/// The card every visualization sits in: a hairline edge, raised a step off the column, and the
/// light a glass pane catches along its top.
struct VizShell<Content: View>: View {
    var look: VizLook
    @ViewBuilder var content: Content

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: 14, style: .continuous)
        VStack(alignment: .leading, spacing: 0) { content }
            .frame(maxWidth: .infinity, alignment: .leading)
            .background {
                ZStack(alignment: .top) {
                    look.card
                    LinearGradient(colors: [.white.opacity(look.dark ? 0.022 : 0), .white.opacity(0)], startPoint: .top, endPoint: .bottom)
                        .frame(height: 64)
                    Rectangle().fill(.white.opacity(look.dark ? 0.07 : 0.85)).frame(height: 1)
                }
                .clipShape(shape)
            }
            .overlay(shape.strokeBorder(look.hairline, lineWidth: 0.75))
            .shadow(color: .black.opacity(look.dark ? 0 : 0.04), radius: 1.5, y: 1)
    }
}

/// The title row (its kind, its name, what can be done with it), with the summary the full width
/// beneath so a narrow column doesn't squeeze it beside the actions.
struct VizHeader<Title: View, Summary: View, Actions: View>: View {
    var kind: String?
    var look: VizLook
    @ViewBuilder var title: Title
    @ViewBuilder var summary: Summary
    @ViewBuilder var actions: Actions

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 10) {
                Image(systemName: VizHeader.icon(kind))
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(look.muted)
                    .frame(width: 24, height: 24)
                    .background(look.inset, in: RoundedRectangle(cornerRadius: 6.5, style: .continuous))
                    .overlay(RoundedRectangle(cornerRadius: 6.5, style: .continuous).strokeBorder(look.hairline, lineWidth: 0.75))
                title.frame(maxWidth: .infinity, alignment: .leading)
                actions
            }
            summary.padding(.leading, 34)
        }
        .padding(.leading, 14)
        .padding(.trailing, 8)
        .padding(.vertical, 12)
    }

    /// A type's symbol by its wire name; one not known (or not yet written) gets a plain chart.
    static func icon(_ kind: String?) -> String {
        switch kind {
        case "bar": "chart.bar.xaxis"
        case "line": "chart.xyaxis.line"
        case "donut": "chart.pie"
        case "stats": "gauge.medium"
        case "table": "tablecells"
        case "heatmap": "square.grid.3x3"
        case "treemap": "rectangle.3.group"
        case "timeline": "calendar.day.timeline.left"
        case "flow": "flowchart"
        case "layers": "square.3.layers.3d"
        case "mockup": "macwindow"
        default: "chart.bar"
        }
    }
}

extension Text {
    func vizTitle(_ look: VizLook) -> some View {
        scaledFont(15, weight: .semibold, relativeTo: .subheadline).foregroundStyle(look.fg).lineLimit(2).fixedSize(horizontal: false, vertical: true)
    }

    func vizSummary(_ look: VizLook) -> some View {
        scaledFont(13, relativeTo: .footnote).foregroundStyle(look.muted).lineSpacing(2).fixedSize(horizontal: false, vertical: true)
    }
}

/// Chart | Data: the two views of the same values, as a segmented control.
private struct ChartDataSwitch: View {
    @Binding var showData: Bool
    @Binding var selected: Int?
    var look: VizLook

    var body: some View {
        HStack(spacing: 1) {
            segment("Chart", active: !showData) { showData = false }
            segment("Data", active: showData) { showData = true }
        }
        .padding(2)
        .background(look.fg.opacity(0.055), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
    }

    private func segment(_ label: String, active: Bool, _ action: @escaping () -> Void) -> some View {
        Button {
            withAnimation(.snappy(duration: 0.2)) {
                action()
                selected = nil
            }
        } label: {
            Text(label)
                .font(.system(size: 12, weight: active ? .medium : .regular))
                .foregroundStyle(active ? look.fg : look.muted)
                .padding(.horizontal, 9)
                .frame(height: 24)
                .background {
                    if active {
                        RoundedRectangle(cornerRadius: 6, style: .continuous)
                            .fill(look.dark ? Trek.elevated : Trek.surface)
                            .shadow(color: .black.opacity(look.dark ? 0.4 : 0.08), radius: 1, y: 1)
                    }
                }
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(active ? .isSelected : [])
    }
}

/// "Drawing…", breathing, in a card's corner while its marks are arriving.
private struct DrawingLabel: View {
    @MotionAllowed private var motion
    @State private var dim = false

    var body: some View {
        Text("Drawing…")
            .font(.system(size: 12))
            .foregroundStyle(Trek.muted)
            .opacity(dim ? 0.55 : 1)
            .frame(height: 28)
            .padding(.trailing, 6)
            .onChange(of: motion, initial: true) { _, on in
                if on { withAnimation(.easeInOut(duration: 0.8).repeatForever()) { dim = true } } else { dim = false }
            }
    }
}

// MARK: Arriving and refused

/// A visualization still being written with nothing whole to draw yet: its card, with its type
/// and title as soon as they're written, and a quiet skeleton of its marks where they will go.
struct VizPending: View {
    var kind: String?
    var title: String?
    @Environment(\.colorScheme) private var scheme
    @MotionAllowed private var motion
    @State private var phase: CGFloat = 0

    var body: some View {
        let look = VizLook(dark: scheme == .dark)
        let stub = look.fg.opacity(look.dark ? 0.07 : 0.06)
        VizShell(look: look) {
            VizHeader(kind: kind, look: look) {
                if let title {
                    Text(title).vizTitle(look)
                } else {
                    Capsule().fill(stub).frame(width: 140, height: 10).frame(height: 19, alignment: .leading)
                }
            } summary: {
                Capsule().fill(stub).frame(width: 190, height: 8).frame(height: 17, alignment: .leading)
            } actions: {
                DrawingLabel()
            }
            marks(stub)
                .frame(maxWidth: .infinity, alignment: .topLeading)
                .frame(height: VizPending.height(kind), alignment: .topLeading)
                .background(look.inset)
                .overlay {
                    if motion {
                        GeometryReader { g in
                            LinearGradient(colors: [.clear, look.fg.opacity(look.dark ? 0.05 : 0.055), .clear], startPoint: .leading, endPoint: .trailing)
                                .frame(width: g.size.width * 0.4)
                                .offset(x: (phase * 1.6 - 0.4) * g.size.width)
                        }
                        .allowsHitTesting(false)
                    }
                }
                .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(look.fg.opacity(look.dark ? 0.05 : 0.06), lineWidth: 0.75))
                .padding(.horizontal, 14)
                .padding(.bottom, 14)
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(title.map { "Visualization being drawn: \($0)" } ?? "Visualization being drawn")
        .onChange(of: motion, initial: true) { _, on in
            var still = Transaction()
            still.disablesAnimations = true
            withTransaction(still) { phase = 0 }
            if on { withAnimation(.linear(duration: 1.6).repeatForever(autoreverses: false)) { phase = 1 } }
        }
    }

    /// A hint of the marks to come, in the type's own shape.
    @ViewBuilder
    private func marks(_ stub: Color) -> some View {
        switch kind {
        case "bar", "timeline", "layers", "table":
            VStack(alignment: .leading, spacing: 12) {
                ForEach(Array([0.82, 0.58, 0.7, 0.36, 0.5].enumerated()), id: \.offset) { _, w in
                    HStack(spacing: 10) {
                        Capsule().fill(stub).frame(width: 56, height: 8)
                        GeometryReader { g in Capsule().fill(stub).frame(width: g.size.width * w, height: 12) }.frame(height: 12)
                    }
                }
            }
            .padding(12)
        case "stats":
            HStack(spacing: 12) {
                ForEach(0..<2, id: \.self) { _ in
                    VStack(alignment: .leading, spacing: 10) {
                        Capsule().fill(stub).frame(width: 60, height: 8)
                        Capsule().fill(stub).frame(width: 90, height: 16)
                        Capsule().fill(stub).frame(width: 44, height: 8)
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
            .padding(12)
        default:
            Color.clear
        }
    }

    /// Roughly how tall a type's marks draw, so the card doesn't jump when they replace this.
    static func height(_ kind: String?) -> CGFloat {
        switch kind {
        case "stats": 104
        case "bar", "heatmap", "timeline": 170
        case "donut": 300
        case "table": 200
        case "line", "treemap", "flow": 230
        case "layers": 250
        case "mockup": 320
        default: 180
        }
    }
}

/// A block that closed but can't be drawn: a short note saying why, over its JSON (folded away).
struct VizInvalid: View {
    var reason: String
    var source: String
    var size: CGFloat
    @State private var open = false
    @Environment(\.colorScheme) private var scheme

    var body: some View {
        let look = VizLook(dark: scheme == .dark)
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(Trek.approval).font(.system(size: 12))
                Text("Couldn't draw this visualization: \(reason)")
                    .scaledFont(13, relativeTo: .footnote)
                    .foregroundStyle(look.muted)
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            .padding(.horizontal, 11)
            .padding(.vertical, 8)
            .background(Trek.approval.opacity(look.dark ? 0.06 : 0.07), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(look.hairline, lineWidth: 0.75))
            Button {
                withAnimation(.snappy(duration: 0.22)) { open.toggle() }
            } label: {
                HStack(spacing: 6) {
                    Image(systemName: "chevron.right")
                        .font(.system(size: 10, weight: .semibold))
                        .rotationEffect(.degrees(open ? 90 : 0))
                    Text(open ? "Hide JSON" : "Show JSON")
                }
                .font(.system(size: 13))
                .foregroundStyle(look.muted)
                .padding(.vertical, 4)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            if open {
                ScrollView(.horizontal, showsIndicators: false) {
                    Text(source)
                        .scaledFont(size * 0.8, design: .monospaced)
                        .foregroundStyle(Trek.foreground)
                        .textSelection(.enabled)
                        .padding(12)
                }
                .background(Trek.surface, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(Trek.border, lineWidth: 0.5))
                .transition(.opacity.combined(with: .move(edge: .top)))
            }
        }
        .accessibilityElement(children: .contain)
    }
}

// MARK: Shared parts

/// The inset surface marks sit on.
struct VizInset: ViewModifier {
    var look: VizLook
    var padding: CGFloat = 12

    func body(content: Content) -> some View {
        content
            .padding(padding)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(look.inset, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(look.fg.opacity(look.dark ? 0.05 : 0.06), lineWidth: 0.75))
    }
}

extension View {
    func vizInset(_ look: VizLook, padding: CGFloat = 12) -> some View { modifier(VizInset(look: look, padding: padding)) }
}

/// A floating readout: the top layer, over the marks.
struct VizReadout<Content: View>: View {
    var look: VizLook
    @ViewBuilder var content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 3) { content }
            .font(.system(size: 12))
            .padding(.horizontal, 10)
            .padding(.vertical, 7)
            .background(look.popover, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 9, style: .continuous).strokeBorder(look.fg.opacity(look.dark ? 0.14 : 0.10), lineWidth: 0.75))
            .shadow(color: .black.opacity(look.dark ? 0.45 : 0.10), radius: 9, y: 5)
            .fixedSize()
            .allowsHitTesting(false)
            .transition(.opacity.combined(with: .scale(scale: 0.96)))
    }
}

/// Lays its children out in rows, wrapping: legend chips, tags.
struct VizWrap: Layout {
    var spacing: CGFloat = 6
    var lineSpacing: CGFloat = 6

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let width = proposal.width ?? .infinity
        var x: CGFloat = 0, y: CGFloat = 0, line: CGFloat = 0, widest: CGFloat = 0
        for s in subviews {
            let size = s.sizeThatFits(.unspecified)
            if x > 0, x + size.width > width {
                y += line + lineSpacing
                x = 0
                line = 0
            }
            x += size.width + spacing
            widest = max(widest, x - spacing)
            line = max(line, size.height)
        }
        return CGSize(width: proposal.width ?? widest, height: y + line)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        var x = bounds.minX, y = bounds.minY, line: CGFloat = 0
        for s in subviews {
            let size = s.sizeThatFits(.unspecified)
            if x > bounds.minX, x + size.width > bounds.maxX {
                y += line + lineSpacing
                x = bounds.minX
                line = 0
            }
            s.place(at: CGPoint(x: x, y: y), proposal: ProposedViewSize(size))
            x += size.width + spacing
            line = max(line, size.height)
        }
    }
}

extension Launch {
    /// Screenshots: open every visualization on its Data view (`-TrekVizData YES`), or with a
    /// mark picked (`-TrekVizSelect 2`).
    static var vizData: Bool { UserDefaults.standard.bool(forKey: "TrekVizData") }
    static var vizSelect: Int? { UserDefaults.standard.object(forKey: "TrekVizSelect").map { _ in UserDefaults.standard.integer(forKey: "TrekVizSelect") } }
}
