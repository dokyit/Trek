import SwiftUI

// Small pieces the Mac screens share: the glass card, the usage bar, reset times and a layout
// that wraps words and chips like a paragraph.

extension View {
    /// A card of Liquid Glass, the dashboard's surface.
    func glassCard(radius: CGFloat = 22, padding: CGFloat = 16) -> some View {
        self
            .padding(padding)
            .frame(maxWidth: .infinity, alignment: .leading)
            .glassEffect(.regular, in: RoundedRectangle(cornerRadius: radius, style: .continuous))
    }
}

/// How much of a limit is used, in the Mac's colours: red from 90%, amber from 70%.
struct UsageBar: View {
    /// 0–100.
    var percent: Double
    /// The colour by what's left (the "Left on" tiles) rather than what's used.
    var left = false
    var height: CGFloat = 5

    var body: some View {
        GeometryReader { g in
            ZStack(alignment: .leading) {
                Capsule().fill(Trek.foreground.opacity(0.09))
                Capsule().fill(color)
                    .frame(width: max(height, g.size.width * min(max(percent, 0), 100) / 100))
                    .opacity(percent > 0 ? 1 : 0)
            }
        }
        .frame(height: height)
        .accessibilityHidden(true)
    }

    private var color: Color {
        if left {
            percent <= 10 ? Trek.failed : percent <= 30 ? Trek.approval : Trek.foreground.opacity(0.85)
        } else {
            percent >= 90 ? Trek.failed : percent >= 70 ? Trek.approval : Trek.foreground.opacity(0.85)
        }
    }
}

enum Resets {
    /// When a limit resets, as the Mac's Usage popover says it: "in 42m", "in 2h 10m", "Fri 9:41 AM
    /// (in 4d)".
    static func until(_ ms: Int64, now: Date = .now) -> String {
        let s = max(0, Int((Double(ms) / 1000 - now.timeIntervalSince1970).rounded()))
        switch s {
        case ..<60: return "in under a minute"
        case ..<3600: return "in \(s / 60)m"
        case ..<86_400: return "in \(s / 3600)h \(s % 3600 / 60)m"
        default:
            let day = Date(timeIntervalSince1970: Double(ms) / 1000).formatted(.dateTime.weekday(.abbreviated).hour().minute())
            return "\(day) (in \(s / 86_400)d)"
        }
    }

    /// "7:41 PM" today, "Oct 1, 7:41 PM" another day (the Mac's `clock`).
    static func clock(_ ms: Int64) -> String {
        let date = Date(timeIntervalSince1970: Double(ms) / 1000)
        return Calendar.current.isDateInToday(date)
            ? date.formatted(date: .omitted, time: .shortened)
            : date.formatted(.dateTime.month(.abbreviated).day().hour().minute())
    }
}

/// Lays its children out like words in a paragraph: left to right, wrapping onto new lines, each
/// line's children centred on one another.
struct WordFlow: Layout {
    var spacing: CGFloat = 5
    var lineSpacing: CGFloat = 4

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let lines = lines(width: proposal.width ?? .infinity, subviews: subviews)
        let height = lines.reduce(0) { $0 + $1.height } + CGFloat(max(lines.count - 1, 0)) * lineSpacing
        let width = lines.map(\.width).max() ?? 0
        return CGSize(width: proposal.width ?? width, height: height)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        var y = bounds.minY
        for line in lines(width: bounds.width, subviews: subviews) {
            var x = bounds.minX
            for i in line.items {
                let size = subviews[i].sizeThatFits(.unspecified)
                subviews[i].place(at: CGPoint(x: x, y: y + (line.height - size.height) / 2), proposal: ProposedViewSize(size))
                x += size.width + spacing
            }
            y += line.height + lineSpacing
        }
    }

    private struct Line {
        var items: [Int] = []
        var width: CGFloat = 0
        var height: CGFloat = 0
    }

    private func lines(width: CGFloat, subviews: Subviews) -> [Line] {
        var out: [Line] = []
        var line = Line()
        for (i, view) in subviews.enumerated() {
            let size = view.sizeThatFits(.unspecified)
            let extra = line.items.isEmpty ? size.width : spacing + size.width
            if !line.items.isEmpty, line.width + extra > width {
                out.append(line)
                line = Line()
            }
            line.width += line.items.isEmpty ? size.width : spacing + size.width
            line.height = max(line.height, size.height)
            line.items.append(i)
        }
        if !line.items.isEmpty { out.append(line) }
        return out
    }
}
