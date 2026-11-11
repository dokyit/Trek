import SwiftUI

/// Over a sub-agent's request pinned in its parent's thread: whose request it is, and the way into
/// its thread (the Mac's "Sub-agent “…” is asking" with "Open it"). The request under it is
/// answered here, and the answer goes to the sub-agent.
struct SubAgentAskLine: View {
    /// The sub-agent's thread.
    var threadID: String
    @Environment(AppModel.self) private var model
    @Environment(\.openThread) private var openThread

    var body: some View {
        let kid = model.thread(threadID)
        let name = kid?.title ?? ""
        // Side by side; at the accessibility sizes the button goes under the words.
        BesideOrOver(spacing: 8) {
            HStack(spacing: 8) {
                AgentGlyph(key: kid?.agent.logo ?? kid?.agent.key ?? "", size: 16)
                    .accessibilityHidden(true)
                // A long title gives way, not the words after it: "Sub-agent “Benchmark the scr…” is asking".
                HStack(spacing: 0) {
                    Text(name.isEmpty ? "A sub-agent" : "Sub-agent “\(name)")
                        .lineLimit(1)
                        .truncationMode(.tail)
                    Text(name.isEmpty ? " is asking" : "” is asking")
                        .lineLimit(1)
                        .fixedSize()
                }
                .font(.ui(.footnote).weight(.medium))
                .foregroundStyle(Trek.muted)
                .accessibilityElement(children: .ignore)
                .accessibilityLabel(name.isEmpty ? "A sub-agent is asking" : "Sub-agent \(name) is asking")
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            Button {
                if let openThread { openThread(threadID) } else { model.openRequest = threadID }
            } label: {
                Text("Open it")
                    .font(.ui(.footnote).weight(.semibold))
                    .foregroundStyle(Trek.foreground)
                    .padding(.horizontal, 12)
                    .frame(minHeight: 30)
                    .background(Trek.foreground.opacity(0.08), in: Capsule())
                    // A finger's worth of room around a small capsule.
                    .padding(.vertical, 7)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.press)
            .padding(.vertical, -7)
            .accessibilityLabel("Open the sub-agent's thread")
            .accessibilityHint("Its request can be answered here too")
        }
        .padding(.leading, 6)
        .padding(.trailing, 2)
    }
}

/// Above a parent's composer while one of its sub-agents waits on something that isn't answered
/// from here (a secret typed on the Mac, or a Mac that sends no request for it): what it's waiting
/// for and a way into its thread (as the Mac's card says "Sub-agent needs you").
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
