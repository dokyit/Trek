import SwiftUI

/// At the foot of the transcript while a thread has sub-agents out: who they are, what each is on,
/// on which agent and model, and for how long. A row opens the sub-agent's thread where the Mac
/// lets it; the header folds the list.
struct SubAgentsCard: View {
    var agents: [SubAgent]
    @Environment(AppModel.self) private var model
    @Environment(\.openThread) private var openThread
    @State private var open = true

    private var running: Int { agents.count { $0.state == .running } }
    private var waiting: Int { agents.count { $0.state == .needsYou } }
    private var failed: Int { agents.count { $0.state == .failed } }
    /// Since the first of those still at it started.
    private var since: Int64? {
        agents.filter { $0.state == .running || $0.state == .needsYou }.compactMap(\.since).min()
    }

    var body: some View {
        TimelineView(.periodic(from: .now, by: 1)) { ctx in
            VStack(alignment: .leading, spacing: 0) {
                header(now: ctx.date)
                if open {
                    Rectangle().fill(Trek.border).frame(height: 0.5).padding(.horizontal, 12)
                    VStack(spacing: 0) {
                        ForEach(Array(agents.enumerated()), id: \.offset) { i, s in
                            if i > 0 { Rectangle().fill(Trek.border.opacity(0.6)).frame(height: 0.5).padding(.leading, 34) }
                            row(s, now: ctx.date)
                        }
                    }
                    .padding(.vertical, 4)
                }
            }
            .background(Trek.surface, in: RoundedRectangle(cornerRadius: 16, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).strokeBorder(Trek.border, lineWidth: 0.5))
        }
    }

    private func header(now: Date) -> some View {
        Button {
            withAnimation(.snappy(duration: 0.22)) { open.toggle() }
        } label: {
            HStack(spacing: 10) {
                StackedLogos(agents: agents, size: 20)
                VStack(alignment: .leading, spacing: 1) {
                    Text("\(agents.count) sub-agent\(agents.count == 1 ? "" : "s")")
                        .font(.ui(.subheadline).weight(.semibold))
                        .foregroundStyle(Trek.foreground)
                    HStack(spacing: 5) {
                        if running > 0 { Text("\(running) working").foregroundStyle(Trek.working) }
                        if waiting > 0 { Text("\(waiting) need\(waiting == 1 ? "s" : "") you").foregroundStyle(Trek.approval) }
                        if failed > 0 { Text("\(failed) failed").foregroundStyle(Trek.failed) }
                        if running + waiting + failed == 0 { Text("All done").foregroundStyle(Trek.done) }
                    }
                    .font(.ui(.footnote).weight(.medium))
                }
                Spacer(minLength: 6)
                if let since {
                    Text(When.duration(max(0, Int(now.timeIntervalSince1970 - Double(since) / 1000))))
                        .font(.ui(.footnote).monospacedDigit())
                        .foregroundStyle(Trek.muted)
                }
                Image(systemName: "chevron.down")
                    .font(.ui(.caption).weight(.semibold))
                    .foregroundStyle(Trek.muted.opacity(0.8))
                    .rotationEffect(.degrees(open ? 180 : 0))
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 10)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(AtWorkLabel.spoken(agents, 0))
        .accessibilityHint(open ? "Folds the list" : "Shows each of them")
    }

    @ViewBuilder
    private func row(_ s: SubAgent, now: Date) -> some View {
        let opens = model.serves(.subAgents, on: s.threadId) ? s.threadId : nil
        Button {
            guard let kid = opens else { return }
            if let openThread { openThread(kid) } else { model.openRequest = kid }
        } label: {
            HStack(alignment: .top, spacing: 10) {
                BeaconDot(color: Self.color(s.state), pulses: s.state == .running, size: 7)
                    .frame(width: 12, height: 18)
                VStack(alignment: .leading, spacing: 3) {
                    HStack(spacing: 6) {
                        Text(s.title)
                            .font(.ui(.subheadline).weight(.medium))
                            .foregroundStyle(Trek.foreground)
                            .lineLimit(1)
                        Spacer(minLength: 4)
                        Text(Self.label(s.state))
                            .font(.ui(.footnote).weight(.medium))
                            .foregroundStyle(Self.color(s.state))
                            .fixedSize()
                        if let since = s.since, s.state == .running || s.state == .needsYou {
                            Text(When.duration(max(0, Int(now.timeIntervalSince1970 - Double(since) / 1000))))
                                .font(.ui(.footnote).monospacedDigit())
                                .foregroundStyle(Trek.muted)
                                .fixedSize()
                        }
                    }
                    HStack(spacing: 5) {
                        AgentGlyph(s.agent, size: 13)
                        Text(s.model.map { "\(s.agent.name) · \($0)" } ?? s.agent.name)
                            .lineLimit(1)
                    }
                    .font(.ui(.footnote))
                    .foregroundStyle(Trek.muted)
                }
                Image(systemName: "chevron.right")
                    .font(.ui(.caption).weight(.semibold))
                    .foregroundStyle(Trek.muted.opacity(0.7))
                    .frame(height: 18)
                    .opacity(opens == nil ? 0 : 1)
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        // Not `disabled`: a row with no thread to open reads the same, it just doesn't open.
        .allowsHitTesting(opens != nil)
        .contextMenu {
            if let kid = opens, s.state == .running || s.state == .needsYou {
                Button("Stop sub-agent", systemImage: "stop.fill", role: .destructive) { model.interrupt(kid) }
            }
        }
        .accessibilityElement(children: .combine)
        .accessibilityHint(opens == nil ? "" : "Opens its thread")
    }

    static func label(_ state: SubAgentState) -> String {
        switch state {
        case .running: "Working"
        case .needsYou: "Needs you"
        case .done: "Done"
        case .failed: "Failed"
        case .stopped: "Stopped"
        }
    }

    static func color(_ state: SubAgentState) -> Color {
        switch state {
        case .running: Trek.working
        case .needsYou: Trek.approval
        case .done: Trek.done
        case .failed: Trek.failed
        case .stopped: Trek.muted
        }
    }
}

/// Up to three agents' logos, overlapping, then "+N" for the rest.
struct StackedLogos: View {
    var agents: [SubAgent]
    var size: CGFloat = 18

    var body: some View {
        let shown = agents.prefix(3)
        HStack(spacing: -size * 0.32) {
            ForEach(Array(shown.enumerated()), id: \.offset) { _, s in
                AgentGlyph(s.agent, size: size)
                    .padding(2)
                    .background(Trek.surface, in: Circle())
                    .overlay(Circle().strokeBorder(Trek.border, lineWidth: 0.5))
            }
            if agents.count > shown.count {
                Text("+\(agents.count - shown.count)")
                    .font(.ui(size: size * 0.55, weight: .semibold).monospacedDigit())
                    .foregroundStyle(Trek.muted)
                    .frame(width: size + 4, height: size + 4)
                    .background(Trek.surface, in: Circle())
                    .overlay(Circle().strokeBorder(Trek.border, lineWidth: 0.5))
            }
        }
        .accessibilityHidden(true)
    }
}
