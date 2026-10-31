import SwiftUI

/// Under a message, what Trek added to it for the agent, as the Mac's transcript says it: who it
/// consults ("Consulting Sol · High · advice, then implement", with their logos), that it asked
/// for a restatement first, and the editor context it carried.
struct MessageNotes: View {
    var parts: MessageParts
    var threadID: String
    @Environment(AppModel.self) private var model

    var body: some View {
        if parts.consult != nil || parts.restate || parts.ask || !parts.context.isEmpty {
            VStack(alignment: .trailing, spacing: 5) {
                if !parts.context.isEmpty || parts.ask {
                    HStack(spacing: 5) {
                        if parts.ask { note("Ask", systemImage: "questionmark.bubble") }
                        ForEach(Array(parts.context.prefix(4).enumerated()), id: \.offset) { _, label in
                            note(label, systemImage: label == "Terminal" ? "apple.terminal" : "doc.text")
                        }
                        if parts.context.count > 4 { note("+\(parts.context.count - 4)", systemImage: nil) }
                    }
                }
                if let consult = parts.consult {
                    HStack(spacing: 6) {
                        HStack(spacing: -4) {
                            ForEach(Array(consult.agents.prefix(4).enumerated()), id: \.offset) { _, key in
                                AgentGlyph(key: logo(key), size: 13)
                            }
                        }
                        Text(consult.line(name: name))
                            .lineLimit(2)
                            .multilineTextAlignment(.trailing)
                    }
                    .accessibilityElement(children: .combine)
                }
                if parts.restate {
                    Label("Asked to restate it first", systemImage: "text.quote")
                        .labelStyle(NoteLabelStyle())
                }
            }
            .font(.ui(.caption))
            .foregroundStyle(Trek.muted)
            .frame(maxWidth: .infinity, alignment: .trailing)
            .padding(.leading, 48)
        }
    }

    private func note(_ text: String, systemImage: String?) -> some View {
        HStack(spacing: 4) {
            if let systemImage { Image(systemName: systemImage).font(.system(size: 10, weight: .medium)) }
            Text(text).lineLimit(1)
        }
        .padding(.horizontal, 7)
        .frame(height: 20)
        .background(Trek.surface, in: Capsule())
        .overlay(Capsule().strokeBorder(Trek.border, lineWidth: 0.5))
    }

    private var agents: [AgentOption] { model.link(for: threadID)?.agents ?? [] }

    private func logo(_ key: String) -> String {
        agents.first { $0.key == key }?.logo ?? key
    }

    /// A consultant's model by the name its agent gives it ("Sol"), else its id.
    private func name(_ c: MessageParts.Consultant) -> String {
        let models = agents.first { $0.key == c.agent }?.models ?? []
        return models.first { ModelNaming.same(c.model, $0.id) || ModelNaming.same($0.id, c.model) }?.label ?? c.model
    }
}

private struct NoteLabelStyle: LabelStyle {
    func makeBody(configuration: Configuration) -> some View {
        HStack(spacing: 6) {
            configuration.icon.font(.system(size: 10, weight: .medium))
            configuration.title
        }
    }
}

/// A wake-up message, folded as the Mac folds it: "↳ Sol reported back on “Review the cache”".
/// Tap to read what came back.
struct WakeRow: View {
    var text: String
    @State private var open = false

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Button {
                withAnimation(.snappy(duration: 0.2)) { open.toggle() }
            } label: {
                HStack(spacing: 8) {
                    Image(systemName: "arrow.turn.down.right")
                        .font(.system(size: 11, weight: .semibold))
                    Text(WakeNote.summary(text))
                        .lineLimit(1)
                        .truncationMode(.tail)
                    Image(systemName: "chevron.right")
                        .font(.system(size: 10, weight: .semibold))
                        .rotationEffect(.degrees(open ? 90 : 0))
                        .opacity(0.6)
                    Spacer(minLength: 0)
                }
                .font(.ui(.footnote))
                .foregroundStyle(WakeNote.failed(text) ? Trek.failed.opacity(0.85) : Trek.muted)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityHint(open ? "Folds what came back" : "Shows what came back")
            if open {
                ScrollView {
                    Text(LongText.preview(text) ?? text)
                        .font(.ui(.footnote))
                        .foregroundStyle(Trek.muted)
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                .frame(maxHeight: 320)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.leading, 14)
                .overlay(alignment: .leading) {
                    Rectangle().fill(Trek.border).frame(width: 1).padding(.leading, 5)
                }
                .transition(.opacity)
            }
        }
    }
}
