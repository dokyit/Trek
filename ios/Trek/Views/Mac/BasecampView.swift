import SwiftUI

/// Where a Basecamp tap leads: a thread from "Ready for review", or plan usage.
enum BasecampRoute: Hashable {
    case thread(String)
    case usage
}

/// Basecamp, the Mac's activity dashboard: what's ready for review, the trek in sentences, the
/// elevation profile and the stat tiles, for today, this week or all time. Worded as the Mac
/// words it (the Mac sends the sentences); drawn here on glass.
struct BasecampView: View {
    @Environment(AppModel.self) private var model
    @Binding var path: [BasecampRoute]
    @Binding var showNew: Bool
    @AppStorage("basecampRange") private var range = BasecampRange.today

    private var recap: Basecamp? { model.basecamp[range] }

    var body: some View {
        NavigationStack(path: $path) {
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    Picker("Range", selection: $range.animation(.snappy)) {
                        ForEach(BasecampRange.allCases) { Text($0.label).tag($0) }
                    }
                    .pickerStyle(.segmented)
                    .padding(.bottom, 2)

                    if let recap {
                        if recap.empty == true {
                            if !recap.review.isEmpty { ReviewCard(rows: recap.review) }
                            EmptyTrekCard(recap: recap) { showNew = true }
                        } else {
                            if !recap.review.isEmpty { ReviewCard(rows: recap.review) }
                            TrekCard(recap: recap, loading: model.basecampLoading.contains(range))
                            TileGrid(tiles: recap.tiles)
                            if recap.review.isEmpty {
                                Label("Nothing waiting on you. Every thread is read.", systemImage: "checkmark.circle")
                                    .font(.subheadline)
                                    .foregroundStyle(Trek.muted)
                                    .padding(.horizontal, 4)
                            }
                        }
                    } else {
                        HStack(spacing: 10) {
                            ProgressView().controlSize(.small)
                            Text("Reading the trail…").foregroundStyle(Trek.muted)
                        }
                        .font(.subheadline)
                        .frame(maxWidth: .infinity)
                        .padding(.top, 60)
                    }
                }
                .padding(.horizontal, 16)
                .padding(.bottom, 24)
                .id(range)
                .transition(.opacity)
            }
            .scrollEdgeEffectStyle(.hard, for: .bottom)
            .background(RidgeBackdrop(height: 300))
            .navigationTitle("Basecamp")
            .navigationSubtitle(recap?.greeting ?? "")
            .refreshable { await reload() }
            .toolbar {
                ToolbarItemGroup(placement: .topBarTrailing) {
                    Button("Plan usage", systemImage: "gauge.with.dots.needle.67percent") { path.append(.usage) }
                    Button("Mark all read", systemImage: "checkmark.circle") {
                        for row in recap?.review ?? [] where row.unseen == true { model.markSeen(row.threadId) }
                        model.loadBasecamp(range)
                    }
                    .disabled(!(recap?.review.contains { $0.unseen == true } ?? false))
                }
            }
            .task(id: range) { model.loadBasecamp(range) }
            .navigationDestination(for: BasecampRoute.self) { route in
                switch route {
                case .thread(let id): ThreadView(threadID: id)
                case .usage: UsageView()
                }
            }
        }
    }

    /// Ask again and wait (a little) for the answer, for pull to refresh.
    private func reload() async {
        model.loadBasecamp(range)
        for _ in 0..<30 where model.basecampLoading.contains(range) {
            try? await Task.sleep(for: .milliseconds(100))
        }
    }
}

// MARK: Ready for review

/// "Ready for review": what needs the user first, then finished threads not looked at yet.
private struct ReviewCard: View {
    var rows: [ReviewRow]

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 7) {
                Text("Ready for review").font(.subheadline.weight(.semibold))
                Text("\(rows.count)").font(.subheadline.monospacedDigit()).foregroundStyle(Trek.muted)
            }
            .padding(.bottom, 6)
            ForEach(Array(rows.enumerated()), id: \.element.id) { i, row in
                if i > 0 { Divider().overlay(Trek.foreground.opacity(0.04)) }
                NavigationLink(value: BasecampRoute.thread(row.threadId)) {
                    ReviewRowView(row: row)
                }
                .buttonStyle(.plain)
            }
        }
        .glassCard(padding: 14)
    }
}

/// A row of "Ready for review": status, title, then agent, diff stat and project.
private struct ReviewRowView: View {
    var row: ReviewRow

    private var look: (symbol: String, color: Color) {
        switch row.status {
        case .needsYou:
            switch row.label {
            case "Question": ("questionmark.bubble.fill", Trek.approval)
            case "Plan to review": ("list.bullet.clipboard.fill", Trek.approval)
            default: ("hand.raised.fill", Trek.approval)
            }
        case .failed: ("xmark.circle.fill", Trek.failed)
        case .paused: ("clock.fill", Trek.approval)
        case .done: ("checkmark.circle.fill", Trek.done)
        }
    }

    var body: some View {
        HStack(alignment: .top, spacing: 11) {
            Image(systemName: look.symbol)
                .font(.system(size: 15, weight: .semibold))
                .foregroundStyle(look.color)
                .frame(width: 20)
                .padding(.top, 1)
            VStack(alignment: .leading, spacing: 4) {
                Text(row.title)
                    .font(.body.weight(row.unseen == true ? .semibold : .regular))
                    .lineLimit(1)
                HStack(spacing: 6) {
                    AgentGlyph(key: row.agent.key, size: 14)
                    if row.additions + row.deletions > 0 {
                        DiffStat(additions: row.additions, deletions: row.deletions)
                    }
                    if let p = row.project {
                        ProjectBadge(project: p, size: 14)
                        Text(p.name).lineLimit(1)
                    }
                }
                .font(.footnote)
                .foregroundStyle(Trek.muted)
            }
            Spacer(minLength: 6)
            Group {
                if let label = row.label {
                    Text(label).fontWeight(.semibold).foregroundStyle(look.color)
                } else {
                    Text(When.short(row.updatedAt)).monospacedDigit().foregroundStyle(Trek.muted)
                }
            }
            .font(.footnote)
            .lineLimit(1)
            .fixedSize()
            .padding(.top, 2)
        }
        .padding(.vertical, 9)
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
    }
}

// MARK: The trek

/// The recap: what the trek came to in sentences, then the elevation profile.
private struct TrekCard: View {
    var recap: Basecamp
    var loading: Bool
    @State private var selected: Int?
    @State private var risen = 0.0
    @MotionAllowed private var motion

    private var line: String {
        guard let p = recap.profile else { return "" }
        if let s = selected, s < p.buckets.count { return p.buckets[s].line }
        return p.line
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(alignment: .firstTextBaseline) {
                Text(recap.title).font(.subheadline.weight(.semibold))
                Spacer()
                Text(loading ? "Updating…" : "Updated \(Resets.clock(recap.updatedAt))")
                    .font(.caption)
                    .foregroundStyle(Trek.muted)
            }
            Narrative(spans: recap.narrative)
            if let profile = recap.profile {
                Divider().overlay(Trek.foreground.opacity(0.05)).padding(.top, 4)
                HStack(alignment: .firstTextBaseline) {
                    Text(line).lineLimit(1).contentTransition(.opacity)
                    Spacer(minLength: 8)
                    Text(profile.total).monospacedDigit()
                }
                .font(.footnote)
                .foregroundStyle(Trek.muted)
                ElevationChart(profile: profile, progress: risen, selected: $selected)
                    .frame(height: 150)
            }
        }
        .glassCard(padding: 18)
        .onAppear {
            guard motion else { risen = 1; return }
            risen = 0
            withAnimation(.smooth(duration: 1.1).delay(0.1)) { risen = 1 }
        }
    }
}

/// The recap's sentences: quiet words, numbers in bold, projects and models as badges. Punctuation
/// stays with the word or badge before it.
private struct Narrative: View {
    var spans: [RecapSpan]

    private enum Piece {
        case word(String, strong: Bool)
        case project(ProjectRef?, String)
        case model(AgentRef?, String)
    }

    /// Each group wraps as one: a word, or a badge and the punctuation after it.
    private var groups: [[Piece]] {
        var groups: [[Piece]] = []
        var spaceBefore = true
        func push(_ p: Piece, newWord: Bool) {
            if newWord || groups.isEmpty { groups.append([p]) } else { groups[groups.count - 1].append(p) }
        }
        for span in spans {
            switch span.kind {
            case "strong":
                push(.word(span.text, strong: true), newWord: spaceBefore)
                spaceBefore = false
            case "project":
                push(.project(span.project, span.text), newWord: spaceBefore)
                spaceBefore = false
            case "model":
                push(.model(span.agent, span.text), newWord: spaceBefore)
                spaceBefore = false
            default:
                let words = span.text.split(whereSeparator: \.isWhitespace)
                for (i, w) in words.enumerated() {
                    push(.word(String(w), strong: false), newWord: i > 0 || spaceBefore || span.text.first?.isWhitespace == true)
                }
                spaceBefore = span.text.last?.isWhitespace ?? spaceBefore
            }
        }
        return groups
    }

    var body: some View {
        WordFlow(spacing: 5, lineSpacing: 5) {
            ForEach(Array(groups.enumerated()), id: \.offset) { _, group in
                HStack(spacing: 0) {
                    ForEach(Array(group.enumerated()), id: \.offset) { _, piece in view(piece) }
                }
            }
        }
        .scaledFont(17)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(spans.map(\.text).joined())
    }

    @ViewBuilder
    private func view(_ piece: Piece) -> some View {
        switch piece {
        case .word(let text, let strong):
            Text(text)
                .fontWeight(strong ? .semibold : .regular)
                .foregroundStyle(strong ? Trek.foreground : Trek.foreground.opacity(0.62))
        case .project(let project, let name):
            HStack(spacing: 5) {
                if let project { ProjectBadge(project: project, size: 18) }
                Text(name).fontWeight(.semibold).foregroundStyle(Trek.foreground)
            }
        case .model(let agent, let label):
            HStack(spacing: 5) {
                if let agent { AgentGlyph(key: agent.key, size: 17) }
                Text(label).fontWeight(.semibold).foregroundStyle(Trek.foreground)
            }
        }
    }
}

/// Nothing on the trail in this range yet: a flat trail, the invitation and New thread.
private struct EmptyTrekCard: View {
    var recap: Basecamp
    var newThread: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            ElevationChart(profile: recap.profile.map { ElevationProfile(buckets: [], now: $0.now, nowAt: $0.nowAt, line: $0.line, total: $0.total, ticks: $0.ticks) },
                           selected: .constant(nil))
                .frame(height: 70)
            Text(recap.invitation ?? "Nothing on the trail yet — start a thread.")
                .scaledFont(17)
                .foregroundStyle(Trek.foreground.opacity(0.85))
            Button(action: newThread) {
                Label("New thread", systemImage: "plus").fontWeight(.semibold).foregroundStyle(Trek.background)
            }
            .buttonStyle(.glassProminent)
            .tint(Trek.foreground)
        }
        .glassCard(padding: 18)
    }
}

// MARK: Tiles

/// The stat tiles, two to a row: best model, worked most on, tokens, agent time, and what's
/// left on each plan.
private struct TileGrid: View {
    var tiles: [StatTile]

    var body: some View {
        LazyVGrid(columns: [GridItem(.flexible(), spacing: 12), GridItem(.flexible(), spacing: 12)], spacing: 12) {
            ForEach(tiles) { tile in
                if tile.kind == .planLeft {
                    NavigationLink(value: BasecampRoute.usage) { TileView(tile: tile) }
                        .buttonStyle(.plain)
                } else {
                    TileView(tile: tile)
                }
            }
        }
    }
}

/// A stat tile: a quiet label, the figure, a note under it.
private struct TileView: View {
    var tile: StatTile

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(tile.label)
                .font(.footnote)
                .foregroundStyle(Trek.muted)
                .lineLimit(1)
            figure
            Spacer(minLength: 0)
            Text(tile.note)
                .font(.caption)
                .foregroundStyle(Trek.muted)
                .lineLimit(2, reservesSpace: true)
                .fixedSize(horizontal: false, vertical: true)
        }
        .frame(maxWidth: .infinity, minHeight: 104, alignment: .topLeading)
        .glassCard(radius: 20, padding: 14)
        .accessibilityElement(children: .combine)
    }

    private var figureText: some View {
        Text(tile.figure)
            .scaledFont(19, weight: .semibold)
            .lineLimit(1)
            .minimumScaleFactor(0.7)
            .monospacedDigit()
    }

    @ViewBuilder
    private var figure: some View {
        switch tile.kind {
        case .bestModel:
            HStack(spacing: 7) {
                if let agent = tile.agent { AgentGlyph(key: agent.key, size: 18) }
                figureText
            }
        case .workedMostOn:
            HStack(spacing: 7) {
                if let project = tile.project { ProjectBadge(project: project, size: 19) }
                figureText
            }
        case .tokens:
            VStack(alignment: .leading, spacing: 6) {
                figureText
                if let spark = tile.sparkline, spark.count > 1 {
                    Sparkline(points: spark).frame(height: 18)
                }
            }
        case .planLeft:
            VStack(alignment: .leading, spacing: 8) {
                HStack(spacing: 7) {
                    if let agent = tile.agent { AgentGlyph(key: agent.key, size: 18) }
                    figureText
                }
                UsageBar(percent: tile.percent ?? 0, left: true, height: 4)
            }
        case .agentTime, .unknown:
            figureText
        }
    }
}
