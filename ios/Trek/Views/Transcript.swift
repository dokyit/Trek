import SwiftUI

struct UserBubble: View {
    var text: String
    var images: Int
    var threadID = ""
    var itemID = ""
    @Environment(AppModel.self) private var model
    @State private var showingAll = false

    var body: some View {
        // A pasted log or crash report can run to megabytes: the bubble shows its start, and the
        // whole of it opens in a sheet that lays it out a stretch at a time.
        let preview = LongText.preview(text)
        HStack {
            Spacer(minLength: 48)
            VStack(alignment: .trailing, spacing: 6) {
                if images > 0, model.serves(.attachment, on: threadID), !itemID.isEmpty {
                    MessageImages(threadID: threadID, itemID: itemID, count: images)
                } else if images > 0 {
                    Label("\(images) image\(images == 1 ? "" : "s")", systemImage: "photo")
                        .font(.ui(.caption)).foregroundStyle(Trek.muted)
                }
                RichText(markdown: preview ?? text, size: 16)
                    .lineSpacing(2)
                if preview != nil {
                    Button { showingAll = true } label: {
                        Label("Show all · \(LongText.size(text))", systemImage: "text.alignleft")
                            .font(.ui(.footnote).weight(.medium))
                            .foregroundStyle(Trek.link)
                    }
                    .buttonStyle(.plain)
                    .sheet(isPresented: $showingAll) { LongTextSheet(title: "Your message", text: text) }
                }
            }
            .padding(.horizontal, 15)
            .padding(.vertical, 11)
            .background(Trek.surface, in: RoundedRectangle(cornerRadius: 18, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 18, style: .continuous).strokeBorder(Trek.border, lineWidth: 0.5))
        }
    }
}

/// A folded run of thinking and tool calls; tap to open it. Open, it shows its newest calls
/// and how many came before, as the Mac's live group does: a turn that runs for hours makes
/// thousands, and laying them all out at once stalls SwiftUI (and in the end exhausts it).
struct ToolGroupView: View {
    var items: [TItem]
    var expanded: Bool
    var toggle: () -> Void
    @State private var shown = Self.shownAtFirst

    /// The calls an open group shows at first, and how many more each "Show earlier" adds.
    static let shownAtFirst = 40
    static let shownMore = 200

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Button {
                withAnimation(.snappy(duration: 0.22)) { toggle() }
            } label: {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Image(systemName: "chevron.right")
                        .font(.system(size: 11, weight: .semibold))
                        .rotationEffect(.degrees(expanded ? 90 : 0))
                        .frame(width: 14)
                    if running {
                        Text(summary).shimmering()
                    } else if failedCount > 0 {
                        // One run of text, so a summary that wraps carries its failures along.
                        Text("\(summary) \(Text("· \(failedCount) failed").foregroundStyle(Trek.failed))")
                    } else {
                        Text(summary)
                    }
                    Spacer(minLength: 0)
                }
                .font(.ui(.subheadline))
                .foregroundStyle(Trek.muted)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)

            if expanded {
                let hidden = max(0, items.count - shown)
                VStack(alignment: .leading, spacing: 2) {
                    if hidden > 0 {
                        Button {
                            shown += Self.shownMore
                        } label: {
                            Text(hidden > Self.shownMore
                                 ? "Show \(Self.shownMore) earlier · \(hidden) in all"
                                 : "Show \(hidden) earlier")
                                .font(.ui(.footnote).weight(.medium))
                                .foregroundStyle(Trek.link)
                                .padding(.leading, 19)
                                .padding(.vertical, 5)
                        }
                        .buttonStyle(.plain)
                    }
                    ForEach(items.suffix(shown)) { item in
                        ToolRow(item: item)
                    }
                }
                .padding(.leading, 6)
                .padding(.top, 8)
                .overlay(alignment: .leading) {
                    Rectangle().fill(Trek.border).frame(width: 1).padding(.vertical, 6).padding(.leading, 6)
                }
                .transition(.opacity.combined(with: .move(edge: .top)))
            }
        }
    }

    private var running: Bool {
        items.contains { if case .tool(let c) = $0.body { c.status == .running } else { false } }
    }

    private var failedCount: Int {
        items.filter { if case .tool(let c) = $0.body { c.status == .failed } else { false } }.count
    }

    var summary: String {
        var thought = 0, commands = 0, reads = 0, edits = 0, searches = 0, web = 0, agents = 0, other = 0
        var files = Set<String>()
        for item in items {
            switch item.body {
            case .reasoning: thought += 1
            case .tool(let c):
                switch c.tool {
                case .command: commands += 1
                case .read: reads += 1
                case .edit: edits += 1; files.insert(c.detail)
                case .search: searches += 1
                case .web: web += 1
                case .agent: agents += 1
                case .other: other += 1
                }
            default: break
            }
        }
        func n(_ count: Int, _ one: String, _ many: String) -> String { count == 1 ? one : many.replacingOccurrences(of: "#", with: "\(count)") }
        var parts: [String] = []
        if thought > 0 { parts.append(n(thought, "Thought once", "Thought # times")) }
        if commands > 0 { parts.append(n(commands, "ran 1 command", "ran # commands")) }
        if reads > 0 { parts.append(n(reads, "read 1 file", "read # files")) }
        if edits > 0 { parts.append(n(files.count, "edited 1 file", "edited # files")) }
        if searches > 0 { parts.append(n(searches, "searched once", "searched # times")) }
        if web > 0 { parts.append(n(web, "browsed once", "browsed # times")) }
        if agents > 0 { parts.append(n(agents, "ran 1 sub-agent", "ran # sub-agents")) }
        if other > 0 { parts.append(n(other, "used 1 tool", "used # tools")) }
        guard var s = parts.first else { return "Worked" }
        s = s.prefix(1).uppercased() + s.dropFirst()
        return ([s] + parts.dropFirst()).joined(separator: " · ")
    }
}

/// One step inside a group: a verb, then what it acted on (a file chip for files). Tap to see output.
struct ToolRow: View {
    var item: TItem
    @State private var open = false

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Button {
                if hasMore { withAnimation(.snappy(duration: 0.2)) { open.toggle() } }
            } label: {
                HStack(alignment: .center, spacing: 9) {
                    Rectangle().fill(Trek.border).frame(width: 10, height: 1)
                    icon
                        .font(.system(size: 13, weight: .medium))
                        .frame(width: 22, height: 22)
                        .foregroundStyle(tint)
                    content
                    Spacer(minLength: 0)
                    statusMark
                }
                .padding(.vertical, 5)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)

            if open {
                detailView
                    .padding(.leading, 41)
                    .transition(.opacity)
            }
            // A finished `watch_video` call has the video and its frames under it, as on the Mac.
            if let watch = item.extras.watch, let threadID, case .tool(let c) = item.body, c.status == .done {
                WatchedVideoView(watch: watch, threadID: threadID)
                    .padding(.leading, 41)
                    .padding(.bottom, 4)
            }
        }
    }

    @Environment(\.liveThreadID) private var threadID

    private var hasMore: Bool {
        switch item.body {
        case .reasoning(let t): !t.isEmpty
        case .tool(let c): !c.output.isEmpty || c.detail.count > 38
        default: false
        }
    }

    private var tint: Color {
        if case .tool(let c) = item.body {
            switch c.status {
            case .failed, .denied: return Trek.failed
            case .running: return Trek.working
            case .done: return Trek.muted
            }
        }
        return Trek.muted
    }

    @ViewBuilder
    private var icon: some View {
        switch item.body {
        case .reasoning: Image(systemName: "text.bubble")
        case .tool(let c): Image(systemName: Self.symbol(c.tool))
        default: Image(systemName: "circle")
        }
    }

    /// How the call went, at the row's end, as the Mac marks it: a spinner while it runs, a tick
    /// once done, a red cross when it failed or was denied.
    @ViewBuilder
    private var statusMark: some View {
        if case .tool(let c) = item.body {
            Group {
                switch c.status {
                case .running: ProgressView().controlSize(.mini).tint(Trek.working)
                case .done: Image(systemName: "checkmark").foregroundStyle(Trek.muted.opacity(0.7))
                case .failed, .denied: Image(systemName: "xmark.circle").foregroundStyle(Trek.failed)
                }
            }
            .font(.system(size: 11, weight: .semibold))
            .frame(width: 18, height: 18)
            .accessibilityLabel(c.status == .running ? "Running" : c.status == .done ? "Done" : c.status == .denied ? "Denied" : "Failed")
        }
    }

    static func symbol(_ kind: ToolKind) -> String {
        switch kind {
        case .command: "apple.terminal"
        case .read: "doc.text"
        case .edit: "square.and.pencil"
        case .search: "magnifyingglass"
        case .web: "globe"
        case .agent: "person.2"
        case .other: "wrench.adjustable"
        }
    }

    @ViewBuilder
    private var content: some View {
        switch item.body {
        case .reasoning:
            Text("Thought process").font(.ui(.subheadline)).foregroundStyle(Trek.muted)
        case .tool(let c):
            HStack(spacing: 8) {
                Text(verb(c)).font(.ui(.subheadline)).foregroundStyle(Trek.muted)
                if (c.tool == .read || c.tool == .edit), !c.detail.contains(" ") {
                    FileChip(path: c.detail, added: c.added, removed: c.removed)
                } else {
                    Text(c.detail)
                        .font(.code(.subheadline))
                        .foregroundStyle(Trek.foreground.opacity(0.85))
                        .lineLimit(1)
                        .truncationMode(.tail)
                }
            }
        default:
            EmptyView()
        }
    }

    private func verb(_ c: ToolCall) -> String {
        switch c.tool {
        case .command: "Run"
        case .read: "Read"
        case .edit: c.title.hasPrefix("Wr") ? "Write" : "Edit"
        case .search: "Search"
        case .web: "Fetch"
        case .agent: "Agent"
        case .other: c.title.isEmpty ? "Tool" : c.title
        }
    }

    @ViewBuilder
    private var detailView: some View {
        switch item.body {
        case .reasoning(let text):
            // Markdown, as the Mac draws a thought: lists, code and emphasis as written.
            MarkdownText(text: text, size: 15)
                .equatable()
                .foregroundStyle(Trek.muted)
                .textSelection(.enabled)
        case .tool(let c):
            VStack(alignment: .leading, spacing: 6) {
                if c.detail.count > 38 || c.tool == .command {
                    Text(c.detail).font(.code(.caption)).foregroundStyle(Trek.foreground)
                }
                if !c.output.isEmpty {
                    ScrollView(.horizontal, showsIndicators: false) {
                        Text(c.output)
                            .font(.code(size: 11.5))
                            .foregroundStyle(c.status == .failed ? Trek.failed : Trek.muted)
                            .padding(10)
                    }
                    .background(Trek.surface, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
                    .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(Trek.border, lineWidth: 0.5))
                }
            }
        default:
            EmptyView()
        }
    }
}

/// An answered request, folded into the transcript: "Allowed · rm -rf target/".
struct ResolvedRequestRow: View {
    var item: TItem

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Image(systemName: symbol).foregroundStyle(color)
            RichText(markdown: text, size: 15, style: .subheadline).lineLimit(2)
        }
        .font(.ui(.subheadline))
        .foregroundStyle(Trek.muted)
    }

    private var symbol: String {
        switch item.body {
        case .approval(let a): a.state == .denied ? "xmark.circle" : "checkmark.circle"
        case .question: "questionmark.bubble"
        case .plan(let p): p.state == .rejected ? "arrow.uturn.left.circle" : "checklist"
        default: "circle"
        }
    }

    private var color: Color {
        switch item.body {
        case .approval(let a): a.state == .denied ? Trek.failed : Trek.done
        case .question: Trek.muted
        case .plan: Trek.plan
        default: Trek.muted
        }
    }

    /// Markdown: the approved command in a code chip.
    private var text: String {
        switch item.body {
        case .approval(let a):
            let verb = switch a.state {
            case .allowed: "Allowed"
            case .allowedForSession: "Allowed for this session"
            case .denied: "Denied"
            default: "Settled by the agent"
            }
            return "\(verb) · `\(a.detail.replacingOccurrences(of: "`", with: "'"))`"
        case .question(let q):
            // A secret answer stays masked in the transcript too.
            let secret = Set(q.questions.filter(\.secret).map(\.question))
            let answers = (q.answers ?? []).map { secret.contains($0.question) ? "••••••" : $0.answer }.joined(separator: ", ")
            return answers.isEmpty ? "Question settled" : "Answered: \(answers)"
        case .plan(let p):
            return p.state == .approved ? "Plan approved" : p.state == .rejected ? "Sent the plan back" : "Plan settled"
        default:
            return ""
        }
    }
}

/// Under the transcript while the agent is busy, as the Mac's working bar: the hiker walking its
/// dotted trail, then the agent's logo, a trail word ("Breaking trail…") that changes every few
/// seconds, how long the turn has run, and what it's doing now.
struct WorkingLine: View {
    var threadID: String
    var agentKey: String
    var since: Int64?
    var activity: String?
    /// The model at work ("Opus 5.5"), named beside the agent's logo.
    var model: String? = nil
    /// Sub-agents at work, of how many it has.
    var subAgents: (running: Int, total: Int) = (0, 0)
    @MotionAllowed private var motion

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            TrailView(id: threadID, still: !motion)
            if model != nil || subAgents.total > 0 {
                // Who's at it: the model, and the sub-agents out of how many.
                HStack(spacing: 6) {
                    if let model {
                        AgentGlyph(key: agentKey, size: 13)
                        Text(model).foregroundStyle(Trek.foreground.opacity(0.8))
                    }
                    if subAgents.total > 0 {
                        if model != nil { Text("·").foregroundStyle(Trek.muted.opacity(0.6)) }
                        Image(systemName: "person.2.fill").font(.system(size: 10, weight: .semibold))
                        Text("\(subAgents.running)/\(subAgents.total) sub-agents").monospacedDigit()
                    }
                }
                .font(.ui(.footnote).weight(.medium))
                .foregroundStyle(Trek.muted)
                .lineLimit(1)
            }
            TimelineView(.periodic(from: .now, by: 1)) { ctx in
                HStack(spacing: 7) {
                    AgentGlyph(key: agentKey, size: 15)
                    Text(TrailWord.line(threadID, since: since, now: ctx.date))
                        .foregroundStyle(Trek.foreground.opacity(0.88))
                        .shimmering()
                        .lineLimit(1)
                        .fixedSize()
                        // The new word in place at once: cross-faded, the old one (often longer)
                        // lay over the clock beside it while it went.
                        .id(TrailWord.line(threadID, since: since, now: ctx.date))
                        .transition(.identity)
                    if let since {
                        Text(When.duration(Int(ctx.date.timeIntervalSince1970 - Double(since) / 1000)))
                            .monospacedDigit()
                            .foregroundStyle(Trek.muted)
                            .fixedSize()
                    }
                    if let activity, !activity.isEmpty {
                        Text("·").foregroundStyle(Trek.muted.opacity(0.6))
                        RichText(markdown: activity, size: 15, style: .subheadline, autoPaths: true)
                            .foregroundStyle(Trek.muted)
                            .lineLimit(1)
                            .truncationMode(.tail)
                    }
                }
                .font(.ui(.subheadline))
            }
        }
        .accessibilityElement(children: .combine)
    }
}

extension View {
    /// The sunrise-tinted sweep the desktop uses for work under way.
    func shimmering() -> some View { modifier(Shimmer()) }
}

private struct Shimmer: ViewModifier {
    @State private var phase: CGFloat = -1
    @MotionAllowed private var motion

    func body(content: Content) -> some View {
        content
            .overlay {
                if motion {
                    GeometryReader { geo in
                        LinearGradient(colors: [.clear, Color(hex: 0xFFC56B).opacity(0.9), Color(hex: 0xFF8A3D).opacity(0.9), .clear],
                                       startPoint: .leading, endPoint: .trailing)
                            .frame(width: geo.size.width * 0.6)
                            .offset(x: phase * geo.size.width * 1.6)
                    }
                    .mask(content)
                    .allowsHitTesting(false)
                }
            }
            // From the start each time motion comes on: begun once at first sight, a shimmer
            // that motion was off for then never ran.
            .onChange(of: motion, initial: true) { _, on in
                var still = Transaction()
                still.disablesAnimations = true
                withTransaction(still) { phase = -1 }
                if on {
                    withAnimation(.linear(duration: 1.6).repeatForever(autoreverses: false)) { phase = 1 }
                }
            }
    }
}
