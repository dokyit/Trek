import SwiftUI

/// What fills the context window, as the Mac's context card shows it (`context_popover.rs`): a
/// bar in segments (the agent's own prompt and tools, MCP servers, skills, messages, then what's
/// free and what the agent keeps back), a row for each with its tokens, and Compact now, which
/// sends the agent its own `/compact`. The split is the Mac's estimate, and says so.
struct ContextBreakdown: View {
    var threadID: String
    @Environment(AppModel.self) private var model
    @State private var card: ContextCard?
    @State private var failed = false
    @State private var compacting = false

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            if let card {
                bar(card)
                VStack(alignment: .leading, spacing: 6) {
                    ForEach(card.filled) { part in
                        HStack(spacing: 8) {
                            RoundedRectangle(cornerRadius: 2).fill(Self.color(part.kind)).frame(width: 8, height: 8)
                            Text(part.label).foregroundStyle(Trek.foreground.opacity(0.9))
                            Spacer(minLength: 8)
                            Text(Tokens.short(part.tokens)).monospacedDigit().foregroundStyle(Trek.muted)
                        }
                        .font(.ui(.footnote))
                        .accessibilityElement(children: .combine)
                    }
                    HStack(spacing: 8) {
                        RoundedRectangle(cornerRadius: 2).strokeBorder(Trek.border, lineWidth: 1).frame(width: 8, height: 8)
                        Text(card.parts.first { $0.kind == "free" }?.label ?? "Free until auto-compact").foregroundStyle(Trek.foreground.opacity(0.9))
                        Spacer(minLength: 8)
                        Text(Tokens.short(card.free)).monospacedDigit().foregroundStyle(Trek.muted)
                    }
                    .font(.ui(.footnote))
                }
                Text("Estimated from what Trek sends the agent; the total is the agent's own.")
                    .font(.ui(.caption))
                    .foregroundStyle(Trek.muted)
                    .fixedSize(horizontal: false, vertical: true)
                compactButton(card)
            } else if failed {
                EmptyView()
            } else {
                ProgressView().controlSize(.small).frame(maxWidth: .infinity)
            }
        }
        .task(id: threadID) { await load() }
    }

    private func bar(_ card: ContextCard) -> some View {
        GeometryReader { geo in
            let window = CGFloat(max(1, card.window))
            HStack(spacing: 1.5) {
                ForEach(card.filled) { part in
                    Rectangle().fill(Self.color(part.kind))
                        .frame(width: max(2, geo.size.width * CGFloat(part.tokens) / window))
                }
                Rectangle().fill(Trek.foreground.opacity(0.08))
                if card.held > 0 {
                    Rectangle().fill(Trek.foreground.opacity(0.18))
                        .frame(width: max(2, geo.size.width * CGFloat(card.held) / window))
                }
            }
        }
        .frame(height: 8)
        .clipShape(Capsule())
        .accessibilityHidden(true)
    }

    @ViewBuilder
    private func compactButton(_ card: ContextCard) -> some View {
        let busy = card.compact == "busy"
        let running = compacting || card.compact == "running"
        if card.compact != "none" {
            Button {
                compacting = true
                model.threadAction(threadID, .compact, done: "Compacting")
            } label: {
                HStack(spacing: 6) {
                    if running { ProgressView().controlSize(.mini) } else { Image(systemName: "arrow.down.right.and.arrow.up.left") }
                    Text(running ? "Compacting…" : "Compact now")
                }
                .font(.ui(.subheadline).weight(.medium))
                .frame(maxWidth: .infinity)
                .frame(height: 36)
            }
            .buttonStyle(.bordered)
            .tint(Trek.foreground)
            .disabled(busy || running)
            .accessibilityHint(busy ? "Waits for the turn to finish" : "Asks the agent to summarise the conversation so far")
            if busy {
                Text("Compacting waits for the turn to finish.").font(.ui(.caption)).foregroundStyle(Trek.muted)
            }
        }
    }

    private func load() async {
        guard model.serves(.contextCard, on: threadID) else { failed = true; return }
        let got: ContextCard? = await withCheckedContinuation { c in
            model.request(.contextCard(threadId: threadID)) { reply in
                if case .contextCard(_, let card) = reply { c.resume(returning: card) } else { c.resume(returning: nil) }
            }
        }
        withAnimation(.easeOut(duration: 0.2)) {
            card = got
            failed = got == nil
        }
    }

    /// The Mac's segment colours.
    static func color(_ kind: String) -> Color {
        switch kind {
        case "system": Color(hex: 0x9A9AA2)
        case "mcp": Color(hex: 0xA77BF0)
        case "skills": Color(hex: 0xF0A13C)
        case "messages": Color(hex: 0x5B92F2)
        default: Color(hex: 0x6BC4A6)
        }
    }
}

extension ContextCard {
    /// What fills the window: the parts before what's free and what the agent keeps back (a Mac
    /// lists those two last, so the parts add up to the window).
    var filled: [ContextPart] { parts.filter { $0.kind != "free" && $0.kind != "reserved" } }
    /// Free until auto-compact.
    var free: Int64 { parts.first { $0.kind == "free" }?.tokens ?? max(0, window - used - reserved) }
    /// Kept back so compacting has room.
    var held: Int64 { parts.first { $0.kind == "reserved" }?.tokens ?? reserved }
}
