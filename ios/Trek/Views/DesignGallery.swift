#if DEBUG
import SwiftUI

/// Debug builds only (`-TrekGallery YES`): Trek's phone components on one page, with content the
/// demo transcripts don't carry (tables, every file type), for screenshots and design review.
struct DesignGallery: View {
    static let markdown = """
    ## What changed

    The refresh now holds `self.lock` across the check, so two refreshes can't both see the stale token. \
    Touched `crates/trek-api/src/session.rs`, `web/src/api.ts` and `Cargo.toml`; the fixtures live in `tests/fixtures/`.

    | File | Change | Lines |
    |:-----|:------:|------:|
    | `session.rs` | Check under the lock | +12 −3 |
    | `concurrent_refresh.rs` | New regression test, 50 refreshes at once | +31 |
    | `api.ts` | Retry once on **401** | +4 −1 |
    | `config.json` | Shorter expiry in tests | +1 −1 |

    Run `cargo test -p auth session_refresh` to check it.
    """

    static let files = ["main.rs", "App.tsx", "index.js", "build.py", "server.go", "Thread.swift", "README.md",
                        "package.json", "Cargo.toml", "ci.yml", "index.html", "app.css", "release.sh", "schema.sql",
                        "logo.png", "Cargo.lock", "LICENSE", "src/net/"]

    @State private var effort: String? = "high"

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 22) {
                    MarkdownText(text: Self.markdown)
                    section("Files") {
                        FlowLayout(spacing: 6) {
                            ForEach(Self.files, id: \.self) { FileChip(path: $0) }
                        }
                    }
                    section("Status") {
                        FlowLayout(spacing: 6) {
                            StatusPill(look: StatusLook(label: "Working", color: Trek.working, symbol: nil, pulses: true))
                            StatusPill(look: StatusLook(label: "Needs you", color: Trek.approval, symbol: "hand.raised.fill", pulses: false))
                            StatusPill(look: StatusLook(label: "Approval", color: Trek.approval, symbol: "hand.raised.fill", pulses: false))
                            StatusPill(look: StatusLook(label: "Question", color: Trek.question, symbol: "questionmark.bubble.fill", pulses: false))
                            StatusPill(look: StatusLook(label: "Plan", color: Trek.plan, symbol: "list.bullet.clipboard.fill", pulses: false))
                            StatusPill(look: StatusLook(label: "Failed", color: Trek.failed, symbol: "exclamationmark.triangle.fill", pulses: false))
                            StatusPill(look: StatusLook(label: "Done", color: Trek.done, symbol: "checkmark", pulses: false))
                        }
                    }
                    section("Working") {
                        WorkingLine(threadID: "t-gallery", agentKey: "claude-code", since: Int64(Date.now.timeIntervalSince1970 * 1000) - 72_000,
                                    activity: "Editing session.rs")
                    }
                    section("Effort") {
                        EffortPicker(efforts: ["low", "medium", "high", "xhigh", "max"], selection: effort) { effort = $0 }
                    }
                }
                .padding(20)
            }
            .background(Trek.background)
            .navigationTitle("Gallery")
            .environment(\.projectHue, 210)
        }
    }

    private func section<Content: View>(_ title: String, @ViewBuilder _ content: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(title).font(.footnote.weight(.semibold)).foregroundStyle(Trek.muted)
            content()
        }
    }
}

/// Wraps its children onto as many rows as they need.
private struct FlowLayout: Layout {
    var spacing: CGFloat

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let rows = arrange(proposal.width ?? .infinity, subviews)
        return CGSize(width: proposal.width ?? rows.map(\.width).max() ?? 0, height: rows.last.map { $0.y + $0.height } ?? 0)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        for row in arrange(bounds.width, subviews) {
            var x = bounds.minX
            for i in row.items {
                let size = subviews[i].sizeThatFits(.unspecified)
                subviews[i].place(at: CGPoint(x: x, y: bounds.minY + row.y + (row.height - size.height) / 2), proposal: .unspecified)
                x += size.width + spacing
            }
        }
    }

    private struct Row { var items: [Int] = []; var width: CGFloat = 0; var height: CGFloat = 0; var y: CGFloat = 0 }

    private func arrange(_ width: CGFloat, _ subviews: Subviews) -> [Row] {
        var rows: [Row] = [Row()]
        for i in subviews.indices {
            let size = subviews[i].sizeThatFits(.unspecified)
            if !rows[rows.count - 1].items.isEmpty, rows[rows.count - 1].width + spacing + size.width > width {
                let last = rows[rows.count - 1]
                rows.append(Row(y: last.y + last.height + spacing))
            }
            var row = rows[rows.count - 1]
            row.width += (row.items.isEmpty ? 0 : spacing) + size.width
            row.height = max(row.height, size.height)
            row.items.append(i)
            rows[rows.count - 1] = row
        }
        return rows
    }
}
#endif
