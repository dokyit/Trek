import SwiftUI

/// Under the thread's title: how full its context is and what it cost, and who's at work for it
/// (sub-agents and background tasks). Glass chips, each opening the details; nothing when the Mac
/// hasn't said yet.
struct ThreadStatusStrip: View {
    var thread: ThreadSummary
    @Environment(AppModel.self) private var model
    @State private var showingCost = false
    @State private var showingWork = false

    private var agents: [SubAgent] { thread.subAgents ?? [] }
    private var background: [String] { thread.background ?? [] }
    /// Sub-agents done with their task, whose threads still open.
    private var earlier: Int {
        guard model.serves(.subAgents, on: thread.id) else { return 0 }
        let live = Set(agents.compactMap(\.threadId))
        return model.subAgentThreads(of: thread.id).count { !live.contains($0.id) }
    }

    var body: some View {
        let earlier = earlier
        if thread.context != nil || thread.cost != nil || !agents.isEmpty || !background.isEmpty || earlier > 0 {
            GlassEffectContainer(spacing: 8) {
                HStack(spacing: 8) {
                    if thread.context != nil || thread.cost != nil {
                        Button { showingCost = true } label: {
                            ContextCostLabel(context: thread.context, cost: thread.cost)
                        }
                        .buttonStyle(.plain)
                        .glassEffect(.regular.interactive(), in: Capsule())
                        .popover(isPresented: $showingCost) {
                            ContextCostDetail(context: thread.context, cost: thread.cost)
                                .presentationCompactAdaptation(.popover)
                        }
                        .accessibilityLabel(ContextCostLabel.spoken(thread.context, thread.cost))
                        .accessibilityHint("Shows context and cost")
                    }
                    if !agents.isEmpty || !background.isEmpty || earlier > 0 {
                        Button { showingWork = true } label: {
                            AtWorkLabel(agents: agents, background: background.count, earlier: earlier)
                        }
                        .buttonStyle(.plain)
                        .glassEffect(.regular.interactive(), in: Capsule())
                        .sheet(isPresented: $showingWork) {
                            AtWorkSheet(threadID: thread.id)
                        }
                        .accessibilityLabel(AtWorkLabel.spoken(agents, background.count, earlier: earlier))
                        .accessibilityHint("Lists them")
                    }
                    Spacer(minLength: 0)
                }
            }
            .onAppear {
                switch Launch.sheet {
                case "context": showingCost = true
                case "agents", "background": showingWork = true
                default: break
                }
            }
        }
    }
}

// MARK: Context and cost

/// Tokens as the Mac shortens them: 950, 12.3K, 171K, 1.2M.
enum Tokens {
    static func short(_ n: Int64) -> String {
        switch n {
        case ..<1000: return "\(n)"
        case ..<10_000: return String(format: "%.1fK", Double(n) / 1000).replacingOccurrences(of: ".0K", with: "K")
        case ..<1_000_000: return "\(n / 1000)K"
        default: return String(format: "%.1fM", Double(n) / 1e6).replacingOccurrences(of: ".0M", with: "M")
        }
    }
}

/// The context meter, as the Mac's composer draws it: a ring that fills as the window does,
/// amber from three quarters, red from nine tenths.
struct ContextRing: View {
    var percent: Int
    var size: CGFloat = 16
    var line: CGFloat = 2

    static func color(_ percent: Int) -> Color {
        percent >= 90 ? Trek.failed : percent >= 75 ? Trek.approval : Trek.foreground.opacity(0.75)
    }

    var body: some View {
        ZStack {
            Circle().stroke(Trek.foreground.opacity(0.14), lineWidth: line)
            Circle()
                .trim(from: 0, to: CGFloat(min(max(percent, 0), 100)) / 100)
                .stroke(Self.color(percent), style: StrokeStyle(lineWidth: line, lineCap: .round))
                .rotationEffect(.degrees(-90))
        }
        .frame(width: size, height: size)
        .accessibilityHidden(true)
    }
}

/// The chip: the ring and its percent, then the cost worded as the Mac words it.
struct ContextCostLabel: View {
    var context: ContextUse?
    var cost: Cost?

    var body: some View {
        // The cache's share too, where there's room for it (it's in the details either way).
        ViewThatFits(in: .horizontal) {
            line(cached: true)
            line(cached: false)
        }
        .font(.ui(.footnote).weight(.medium))
        .padding(.horizontal, 11)
        .frame(height: 30)
        .contentShape(Capsule())
    }

    private func line(cached: Bool) -> some View {
        HStack(spacing: 6) {
            if let context {
                ContextRing(percent: context.percent, size: 14, line: 2)
                Text("\(context.percent)%")
                    .monospacedDigit()
                    .foregroundStyle(context.percent >= 75 ? ContextRing.color(context.percent) : Trek.foreground)
            }
            if context != nil, cost != nil {
                Text("·").foregroundStyle(Trek.muted.opacity(0.7))
            }
            if let cost {
                Text(cost.label).foregroundStyle(Trek.muted).lineLimit(1).truncationMode(.tail)
            }
            if cached, let text = cost?.cached {
                Text("·").foregroundStyle(Trek.muted.opacity(0.7))
                Text(text).foregroundStyle(Trek.muted).lineLimit(1).fixedSize()
            }
        }
    }

    static func spoken(_ context: ContextUse?, _ cost: Cost?) -> String {
        [context.map { "\($0.percent)% of context used" }, cost?.label, cost?.cached].compactMap { $0 }.joined(separator: ", ")
    }
}

/// What the chip opens: tokens used of the window, and the cost with how it's billed.
struct ContextCostDetail: View {
    var context: ContextUse?
    var cost: Cost?

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            if let context {
                HStack(spacing: 12) {
                    ContextRing(percent: context.percent, size: 34, line: 3.5)
                    VStack(alignment: .leading, spacing: 2) {
                        Text("\(context.percent)% context used").font(.ui(.headline))
                        Text("\(Tokens.short(context.used)) of \(Tokens.short(context.window)) tokens")
                            .font(.ui(.subheadline)).monospacedDigit().foregroundStyle(Trek.muted)
                    }
                }
            }
            if context != nil, cost != nil {
                Rectangle().fill(Trek.border).frame(height: 0.5)
            }
            if let cost {
                VStack(alignment: .leading, spacing: 8) {
                    Text(cost.label).font(.ui(.headline))
                    if let billing = cost.billing, billing != .unknown {
                        row(Self.billingLabel(billing), symbol: Self.billingSymbol(billing))
                    }
                    if let plan = cost.plan { row(plan, symbol: "person.crop.circle") }
                    if let detail = cost.detail {
                        Text(detail).font(.ui(.footnote)).foregroundStyle(Trek.muted).fixedSize(horizontal: false, vertical: true)
                    }
                    if let cached = cost.cached {
                        row(cached, symbol: "bolt.horizontal.circle")
                        if let detail = cost.cachedDetail {
                            Text(detail).font(.ui(.footnote)).foregroundStyle(Trek.muted).fixedSize(horizontal: false, vertical: true)
                        }
                    }
                }
            }
        }
        .padding(18)
        .frame(idealWidth: 290, maxWidth: 320, alignment: .leading)
    }

    private func row(_ text: String, symbol: String) -> some View {
        Label(text, systemImage: symbol).font(.ui(.subheadline)).foregroundStyle(Trek.foreground.opacity(0.9))
    }

    static func billingLabel(_ b: Billing) -> String {
        switch b {
        case .plan: "Covered by a plan"
        case .metered: "Billed per token"
        case .local: "Runs on the Mac: not billed"
        case .unknown: ""
        }
    }

    static func billingSymbol(_ b: Billing) -> String {
        switch b {
        case .plan: "checkmark.seal"
        case .metered: "creditcard"
        case .local: "desktopcomputer"
        case .unknown: "questionmark.circle"
        }
    }
}

// MARK: Sub-agents and background tasks

/// The at-work pill, as the Mac's: one logo per agent with how many it runs, then the background
/// tasks.
struct AtWorkLabel: View {
    var agents: [SubAgent]
    var background: Int
    /// Sub-agents that are done, shown when nothing is at work.
    var earlier = 0

    /// The agents at work, in order of first appearance, with how many each runs.
    static func byAgent(_ agents: [SubAgent]) -> [(agent: AgentRef, count: Int)] {
        var out: [(agent: AgentRef, count: Int)] = []
        for s in agents {
            let key = s.agent.logo ?? s.agent.key
            if let i = out.firstIndex(where: { ($0.agent.logo ?? $0.agent.key) == key }) { out[i].count += 1 } else { out.append((s.agent, 1)) }
        }
        return out
    }

    var body: some View {
        HStack(spacing: 8) {
            ForEach(Self.byAgent(agents), id: \.agent.key) { entry in
                HStack(spacing: 3) {
                    AgentGlyph(entry.agent, size: 15)
                    Text("\(entry.count)").monospacedDigit()
                }
            }
            if background > 0 {
                HStack(spacing: 3) {
                    Image(systemName: "apple.terminal").font(.system(size: 12, weight: .medium)).foregroundStyle(Trek.muted)
                    Text("\(background)").monospacedDigit()
                }
            }
            if agents.isEmpty, background == 0, earlier > 0 {
                HStack(spacing: 4) {
                    Image(systemName: "person.2").font(.system(size: 12, weight: .medium)).foregroundStyle(Trek.muted)
                    Text("\(earlier)").monospacedDigit()
                }
            }
        }
        .font(.ui(.footnote).weight(.semibold))
        .foregroundStyle(Trek.foreground)
        .padding(.horizontal, 11)
        .frame(height: 30)
        .contentShape(Capsule())
    }

    static func spoken(_ agents: [SubAgent], _ background: Int, earlier: Int = 0) -> String {
        var parts: [String] = []
        if agents.isEmpty, background == 0, earlier > 0 { parts.append("\(earlier) sub-agent\(earlier == 1 ? "" : "s") done") }
        if !agents.isEmpty { parts.append("\(agents.count) sub-agent\(agents.count == 1 ? "" : "s") at work") }
        if background > 0 { parts.append("\(background) background task\(background == 1 ? "" : "s")") }
        return parts.joined(separator: ", ")
    }
}

/// Who's at work for the thread: its sub-agents (logo, title, model, state, how long), then what
/// its agent runs in the background.
struct AtWorkSheet: View {
    var threadID: String
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @Environment(\.openThread) private var openThread
    @State private var path: [String] = []

    private var thread: ThreadSummary? { model.thread(threadID) }
    /// Sub-agents' threads open from here (where the Mac says they can).
    private var opens: Bool { model.serves(.subAgents, on: threadID) }

    var body: some View {
        NavigationStack(path: $path) {
            List {
                let agents = thread?.subAgents ?? []
                let background = thread?.background ?? []
                let live = Set(agents.compactMap(\.threadId))
                let earlier = opens ? model.subAgentThreads(of: threadID).filter { !live.contains($0.id) } : []
                if !agents.isEmpty {
                    Section("Sub-agents") {
                        TimelineView(.periodic(from: .now, by: 1)) { ctx in
                            ForEach(Array(agents.enumerated()), id: \.offset) { _, s in
                                subAgent(s, threadID: opens ? s.threadId : nil, now: ctx.date)
                            }
                        }
                    }
                }
                if !earlier.isEmpty {
                    Section(agents.isEmpty ? "Sub-agents" : "Done") {
                        ForEach(earlier) { t in
                            subAgent(SubAgent(agent: t.agent, model: t.modelLabel, title: t.title, state: SubAgentState(t), threadId: t.id),
                                     threadID: t.id, now: .now)
                        }
                    }
                }
                if let tasks = thread?.backgroundTasks, !tasks.isEmpty, model.serves(.background, on: threadID) {
                    Section {
                        TimelineView(.periodic(from: .now, by: 1)) { ctx in
                            ForEach(tasks) { task in
                                NavigationLink(value: task.id) { BackgroundTaskRow(task: task, now: ctx.date) }
                            }
                        }
                    } header: {
                        Text("In the background")
                    } footer: {
                        Text("Dev servers, watchers and other work \(thread?.agent.name ?? "the agent") left running. Open one for its output.")
                            .font(.ui(.footnote))
                    }
                } else if !background.isEmpty {
                    Section {
                        ForEach(background, id: \.self) { title in
                            Label {
                                Text(title).font(.code(.subheadline)).lineLimit(2)
                            } icon: {
                                Image(systemName: "apple.terminal").foregroundStyle(Trek.muted)
                            }
                        }
                    } header: {
                        Text("In the background")
                    } footer: {
                        Text("Dev servers, watchers and other work \(thread?.agent.name ?? "the agent") left running. Stop them on the Mac.")
                            .font(.ui(.footnote))
                    }
                }
                if agents.isEmpty && background.isEmpty && earlier.isEmpty {
                    Text("Nothing at work now.").foregroundStyle(Trek.muted)
                }
            }
            .scrollContentBackground(.hidden)
            .background(Trek.grouped)
            .navigationTitle("At work")
            .navigationBarTitleDisplayMode(.inline)
            .navigationDestination(for: String.self) { BackgroundTaskView(threadID: threadID, taskID: $0) }
            .onAppear { if Launch.sheet == "background", let first = thread?.backgroundTasks?.first { path = [first.id] } }
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button { dismiss() } label: { Image(systemName: "checkmark") }
                        .accessibilityLabel("Done")
                }
            }
        }
        .presentationDetents([.medium, .large])
        .presentationDragIndicator(.visible)
    }

    /// A sub-agent's row: with a thread, it opens it (on top of its parent), and one at work can
    /// be stopped, as from its own window on the Mac.
    @ViewBuilder
    private func subAgent(_ s: SubAgent, threadID kid: String?, now: Date) -> some View {
        if let kid {
            Button { open(kid) } label: {
                HStack(spacing: 8) {
                    SubAgentRow(sub: s, now: now)
                    Image(systemName: "chevron.right").font(.ui(.caption).weight(.semibold)).foregroundStyle(Trek.muted.opacity(0.7))
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityHint("Opens its thread")
            .swipeActions {
                if s.state == .running || s.state == .needsYou {
                    Button("Stop", systemImage: "stop.fill", role: .destructive) { model.interrupt(kid) }
                }
            }
            .contextMenu {
                Button("Open", systemImage: "arrow.up.right") { open(kid) }
                if s.state == .running || s.state == .needsYou {
                    Button("Stop sub-agent", systemImage: "stop.fill", role: .destructive) { model.interrupt(kid) }
                }
            }
        } else {
            // The chevron's room, kept: its state lines up with the rows that open.
            HStack(spacing: 8) {
                SubAgentRow(sub: s, now: now)
                Image(systemName: "chevron.right").font(.ui(.caption).weight(.semibold)).hidden()
            }
        }
    }

    private func open(_ kid: String) {
        dismiss()
        // Once the sheet is down, so the push shows.
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.35) { [openThread, model] in
            if let openThread { openThread(kid) } else { model.openRequest = kid }
        }
    }
}

/// A sub-agent: its logo, what it's on, its model and how long it's been at it, and its state.
struct SubAgentRow: View {
    var sub: SubAgent
    var now: Date

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            AgentGlyph(sub.agent, size: 24)
            VStack(alignment: .leading, spacing: 3) {
                Text(sub.title).font(.ui(.subheadline).weight(.medium)).lineLimit(3)
                HStack(spacing: 5) {
                    Text(sub.model.map { "\(sub.agent.name) · \($0)" } ?? sub.agent.name)
                    if let since = sub.since, sub.state == .running || sub.state == .needsYou {
                        Text("·")
                        Text(When.duration(max(0, Int(now.timeIntervalSince1970 - Double(since) / 1000)))).monospacedDigit()
                    }
                }
                .font(.ui(.footnote))
                .foregroundStyle(Trek.muted)
                .lineLimit(1)
            }
            Spacer(minLength: 6)
            StatusPill(look: look, compact: false)
        }
        .padding(.vertical, 2)
        .accessibilityElement(children: .combine)
    }

    private var look: StatusLook {
        switch sub.state {
        case .running: StatusLook(label: "Working", color: Trek.working, symbol: nil, pulses: true)
        case .needsYou: StatusLook(label: "Needs you", color: Trek.approval, symbol: "hand.raised.fill", pulses: false)
        case .done: StatusLook(label: "Done", color: Trek.done, symbol: "checkmark", pulses: false)
        case .failed: StatusLook(label: "Failed", color: Trek.failed, symbol: "exclamationmark.triangle.fill", pulses: false)
        case .stopped: StatusLook(label: "Stopped", color: Trek.muted, symbol: "stop.fill", pulses: false)
        }
    }
}
