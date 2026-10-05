import SwiftUI

struct UserBubble: View {
    var text: String
    var images: Int

    var body: some View {
        HStack {
            Spacer(minLength: 48)
            VStack(alignment: .trailing, spacing: 6) {
                if images > 0 {
                    Label("\(images) image\(images == 1 ? "" : "s")", systemImage: "photo")
                        .font(.caption).foregroundStyle(Trek.muted)
                }
                RichText(markdown: text, size: 16)
                    .lineSpacing(2)
            }
            .padding(.horizontal, 15)
            .padding(.vertical, 11)
            .background(Trek.surface, in: RoundedRectangle(cornerRadius: 18, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 18, style: .continuous).strokeBorder(Trek.border, lineWidth: 0.5))
        }
    }
}

/// A folded run of thinking and tool calls; tap to open it.
struct ToolGroupView: View {
    var items: [TItem]
    var expanded: Bool
    var toggle: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Button {
                withAnimation(.snappy(duration: 0.22)) { toggle() }
            } label: {
                HStack(spacing: 8) {
                    Image(systemName: "chevron.right")
                        .font(.system(size: 11, weight: .semibold))
                        .rotationEffect(.degrees(expanded ? 90 : 0))
                        .frame(width: 14)
                    if running {
                        Text(summary).shimmering()
                    } else {
                        Text(summary)
                    }
                    if failedCount > 0 {
                        Text("· \(failedCount) failed").foregroundStyle(Trek.failed)
                    }
                    Spacer(minLength: 0)
                }
                .font(.subheadline)
                .foregroundStyle(Trek.muted)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)

            if expanded {
                VStack(alignment: .leading, spacing: 2) {
                    ForEach(items) { item in
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
        }
    }

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
        case .tool(let c):
            if c.status == .running {
                ProgressView().controlSize(.mini).tint(Trek.working)
            } else {
                Image(systemName: Self.symbol(c.tool))
            }
        default: Image(systemName: "circle")
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
            Text("Thought process").font(.subheadline).foregroundStyle(Trek.muted)
        case .tool(let c):
            HStack(spacing: 8) {
                Text(verb(c)).font(.subheadline).foregroundStyle(Trek.muted)
                if (c.tool == .read || c.tool == .edit), !c.detail.contains(" ") {
                    FileChip(path: c.detail, added: c.added, removed: c.removed)
                } else {
                    Text(c.detail)
                        .font(.system(.subheadline, design: .monospaced))
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
            Text(text).font(.subheadline).italic().foregroundStyle(Trek.muted)
        case .tool(let c):
            VStack(alignment: .leading, spacing: 6) {
                if c.detail.count > 38 || c.tool == .command {
                    Text(c.detail).font(.system(.caption, design: .monospaced)).foregroundStyle(Trek.foreground)
                }
                if !c.output.isEmpty {
                    ScrollView(.horizontal, showsIndicators: false) {
                        Text(c.output)
                            .font(.system(size: 11.5, design: .monospaced))
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
        .font(.subheadline)
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
        case .question: Trek.question
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
            let answers = (q.answers ?? []).map(\.answer).joined(separator: ", ")
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
    @MotionAllowed private var motion

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            TrailView(id: threadID, still: !motion)
            TimelineView(.periodic(from: .now, by: 1)) { ctx in
                HStack(spacing: 7) {
                    AgentGlyph(key: agentKey, size: 15)
                    Text(TrailWord.line(threadID, since: since, now: ctx.date))
                        .foregroundStyle(Trek.foreground.opacity(0.88))
                        .shimmering()
                        .lineLimit(1)
                        .fixedSize()
                        .contentTransition(.opacity)
                        .animation(motion ? .easeInOut(duration: 0.35) : nil, value: TrailWord.line(threadID, since: since, now: ctx.date))
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
                .font(.subheadline)
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
