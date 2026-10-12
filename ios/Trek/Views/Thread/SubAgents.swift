import SwiftUI

/// Above a parent's composer while one of its sub-agents waits on the user: what it's waiting for
/// and a way into its thread, where it's answered (as the Mac's card says "Sub-agent needs you").
struct SubAgentNeedsCard: View {
    var threadID: String
    var needs: Needs?
    @Environment(AppModel.self) private var model
    @Environment(\.openThread) private var openThread

    private var tint: Color {
        // Amber for anything waiting on you, as on the Mac; plans keep theirs.
        needs?.kind == .plan ? Trek.plan : Trek.approval
    }

    private var waitsFor: String {
        switch needs?.kind {
        case .question: "It has a question for you"
        case .plan: "Its plan is ready to review"
        default: "It's asking for approval"
        }
    }

    var body: some View {
        let kid = model.thread(threadID)
        Button { open() } label: {
            HStack(spacing: 12) {
                ZStack(alignment: .bottomTrailing) {
                    AgentGlyph(key: kid?.agent.logo ?? kid?.agent.key ?? "", size: 26)
                    Circle().fill(tint).frame(width: 9, height: 9)
                        .overlay(Circle().strokeBorder(Trek.elevated, lineWidth: 2))
                        .offset(x: 2, y: 2)
                }
                VStack(alignment: .leading, spacing: 2) {
                    Text("Sub-agent needs you").font(.ui(.subheadline).weight(.semibold))
                    Text(kid.map { "\($0.title) · \(waitsFor)" } ?? waitsFor)
                        .font(.ui(.footnote)).foregroundStyle(Trek.muted).lineLimit(2)
                }
                Spacer(minLength: 6)
                Text("Open")
                    .font(.ui(.subheadline).weight(.semibold))
                    .foregroundStyle(Trek.background)
                    .padding(.horizontal, 14)
                    .padding(.vertical, 7)
                    .background(Trek.foreground, in: Capsule())
            }
            .padding(12)
            .background(Trek.elevated.opacity(0.92), in: RoundedRectangle(cornerRadius: 22, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 22, style: .continuous).strokeBorder(tint.opacity(0.45), lineWidth: 1))
            .shadow(color: .black.opacity(0.12), radius: 18, y: 6)
            .contentShape(RoundedRectangle(cornerRadius: 22, style: .continuous))
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Sub-agent needs you. \(kid?.title ?? ""). \(waitsFor).")
        .accessibilityHint("Opens its thread")
    }

    private func open() {
        if let openThread { openThread(threadID) } else { model.openRequest = threadID }
    }
}

/// A sub-agent's state, as its row in the at-work sheet shows it.
extension SubAgentState {
    init(_ t: ThreadSummary) {
        switch t.runState {
        case .working: self = .running
        case .needsYou: self = .needsYou
        case .failed: self = .failed
        case .idle: self = .done
        }
    }
}
