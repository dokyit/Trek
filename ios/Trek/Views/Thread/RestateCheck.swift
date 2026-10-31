import SwiftUI

/// The Mac's restate check (`restate.rs`): a turn that was asked to restate its request first
/// ends with "Is that what you meant?". Yes sends the go-ahead, with whatever the asking message
/// held back for it (a consult, an arena); no brings the keyboard up to correct it.
nonisolated enum RestateCheck {
    /// `restate::GO_AHEAD`.
    static let goAhead = "That's right — go ahead."

    /// The go-ahead for the turn ending at block `end`, or nil when its message didn't ask for a
    /// restatement (`restate::asked_in_turn`, `go_ahead_after`).
    static func goAhead(_ blocks: [Block], endingAt end: String) -> String? {
        guard let i = blocks.firstIndex(where: { $0.id == end }) else { return nil }
        // The user's messages before it, newest first (Trek's own wake-ups and asides aren't).
        let asked = blocks[..<i].reversed().compactMap { b -> String? in
            if case .user(let text, _, _, let aside) = b.kind, !aside, !WakeNote.isWake(text) { return text }
            return nil
        }
        guard let last = asked.first, MessageParts.of(last).restate else { return nil }
        // The exchange runs back through the messages that asked for a restatement; the newest of
        // them that asked for more (a consult) goes ahead with it.
        for text in asked {
            guard MessageParts.of(text).restate else { break }
            if MessageParts.of(text).consult != nil, let at = text.range(of: "\n\n\(MessageParts.consultOpen)", options: .backwards) {
                return goAhead + text[at.lowerBound...]
            }
        }
        return goAhead
    }
}

/// "Is that what you meant?" with its two answers, under the turn.
struct RestateCheckRow: View {
    var yes: () -> Void
    var no: () -> Void

    var body: some View {
        HStack(spacing: 8) {
            Text("Is that what you meant?")
                .font(.ui(.footnote))
                .foregroundStyle(Trek.muted)
                .lineLimit(1)
                .layoutPriority(-1)
            Spacer(minLength: 0)
            Button(action: no) {
                Text("Not quite…")
                    .font(.ui(.footnote).weight(.medium))
                    .foregroundStyle(Trek.foreground)
                    .padding(.horizontal, 10)
                    .frame(minHeight: 32)
                    .background(Trek.foreground.opacity(0.06), in: Capsule())
                    .fixedSize()
            }
            .buttonStyle(CardPress())
            Button(action: yes) {
                Text("Go ahead")
                    .font(.ui(.footnote).weight(.semibold))
                    .foregroundStyle(CardInk.onButton)
                    .padding(.horizontal, 12)
                    .frame(minHeight: 32)
                    .background(CardInk.button, in: Capsule())
                    .fixedSize()
            }
            .buttonStyle(CardPress())
            .accessibilityLabel("That's right, go ahead")
        }
        .padding(.top, 2)
    }
}
