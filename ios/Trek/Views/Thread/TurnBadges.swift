import SwiftUI

/// On a turn's footer, as the Mac's: the turn ran the project's verification CLI. "Verified" in
/// green when its last run passed, "Verification failed" in amber when not; a tap says what ran.
struct VerifiedBadge: View {
    var verification: Verification
    /// Where the footer is crowded (the latest turn, with its actions out): the mark only.
    var compact = false
    @State private var showing = false

    private var color: Color { verification.passed ? Trek.done : Trek.approval }
    private var word: String { verification.passed ? "Verified" : "Verification failed" }

    var body: some View {
        Button { showing = true } label: {
            HStack(spacing: 4) {
                Image(systemName: verification.passed ? "checkmark.seal.fill" : "exclamationmark.triangle.fill")
                    .font(.system(size: 11, weight: .semibold))
                if !compact { Text(verification.passed ? "Verified" : "Failed").font(.ui(.caption).weight(.medium)) }
            }
            .foregroundStyle(color)
            .frame(minHeight: 32)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .fixedSize()
        .accessibilityLabel(word)
        .accessibilityHint("Says what ran")
        .popover(isPresented: $showing) {
            VStack(alignment: .leading, spacing: 8) {
                Label(word, systemImage: verification.passed ? "checkmark.seal.fill" : "exclamationmark.triangle.fill")
                    .font(.ui(.subheadline).weight(.semibold))
                    .foregroundStyle(color)
                Text(verification.passed
                     ? "Checked with the project's verification skill (\(verification.commands.count) run\(verification.commands.count == 1 ? "" : "s"))."
                     : "The last check with the project's verification skill failed.")
                    .font(.ui(.footnote))
                    .foregroundStyle(Trek.muted)
                    .fixedSize(horizontal: false, vertical: true)
                ForEach(Array(verification.commands.suffix(3).reversed().enumerated()), id: \.offset) { _, c in
                    Text("$ \(c)")
                        .font(.code(size: 12))
                        .foregroundStyle(Trek.foreground)
                        .lineLimit(2)
                        .truncationMode(.middle)
                }
            }
            .padding(14)
            .frame(maxWidth: 320, alignment: .leading)
            .presentationCompactAdaptation(.popover)
        }
    }
}

/// "Handed over from Codex to Claude Code", with the two agents' logos, as the Mac's row.
struct HandoffRow: View {
    var from: String
    var to: String
    var fromAgent: AgentRef?
    var toAgent: AgentRef?

    var body: some View {
        HStack(spacing: 6) {
            if let fromAgent { AgentGlyph(fromAgent, size: 14) }
            Image(systemName: "arrow.right")
                .font(.system(size: 10, weight: .semibold))
                .foregroundStyle(Trek.muted.opacity(0.8))
            if let toAgent { AgentGlyph(toAgent, size: 14) }
            Text("Handed over from \(from) to \(to)")
                .lineLimit(2)
        }
        .font(.ui(.subheadline))
        .foregroundStyle(Trek.muted)
        .frame(maxWidth: .infinity, alignment: .center)
        .accessibilityElement(children: .combine)
    }
}
