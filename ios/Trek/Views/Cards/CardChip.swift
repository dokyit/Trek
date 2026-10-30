import SwiftUI

/// The composer's status chip for the thread's newest card, as the Mac's (`pr_card::status_chip`):
/// a pill of the page's own colour with a hairline and a faint shadow, a mark (a spinner while
/// it works, a dot while it waits or once it's done, the merge glyph once merged), the name in
/// ink and the state, in colour only when it's a verdict. A good finish lingers a few seconds
/// and goes; a failure stays until the next message. Tapping it shows the card.
struct CardChip: View {
    var card: RemoteCard

    var body: some View {
        if card.kind == .pr, card.number == nil, card.phase == .creating {
            creating
        } else {
            status
        }
    }

    /// A pull request on its way: the Mac's dark "Creating PR" chip.
    private var creating: some View {
        HStack(spacing: 6) {
            RingSpinner(size: 12, color: CardInk.onButton)
            Text("Creating PR").font(.ui(size: 13, weight: .medium, relativeTo: .footnote))
        }
        .foregroundStyle(CardInk.onButton)
        .padding(.horizontal, 12)
        .frame(minHeight: 32)
        .background(CardInk.button, in: Capsule())
        .accessibilityElement(children: .combine)
        .accessibilityHint("Shows the card")
    }

    private var status: some View {
        let (what, state) = card.chipParts
        let hue = Self.hue(card)
        return HStack(spacing: 6) {
            if card.busy {
                RingSpinner(size: 12)
            } else if card.phase == .merged {
                CardIcon(name: "git-merge", size: 12).foregroundStyle(hue?.mark ?? Trek.muted)
            } else {
                Circle().fill(hue?.mark ?? Trek.muted).frame(width: 6, height: 6)
            }
            Text(what).foregroundStyle(Trek.foreground)
            Text(state)
                .foregroundStyle(hue?.text ?? Trek.muted)
                .contentTransition(.numericText())
        }
        .font(.ui(size: 13, relativeTo: .footnote))
        .lineLimit(1)
        .padding(.horizontal, 12)
        .frame(minHeight: 32)
        .background(CardInk.chipFill, in: Capsule())
        .overlay(Capsule().strokeBorder(CardInk.chipBorder, lineWidth: 1))
        .shadow(color: .black.opacity(0.05), radius: 1, y: 1)
        .animation(.snappy(duration: 0.25), value: state)
        .accessibilityElement(children: .combine)
        .accessibilityLabel("\(what) \(state)")
        .accessibilityHint("Shows the card")
    }

    /// The verdict's colour, or none while work is under way.
    static func hue(_ card: RemoteCard) -> CardHue? {
        if card.busy { return nil }
        switch card.phase {
        case .conflicted: return .amber
        case .ready, .passed, .published: return .green
        case .merged: return .purple
        case .failed: return .red
        default: return nil
        }
    }

    /// The card the chip shows for `blocks`, if any: the newest card, unless it finished well
    /// more than a few seconds ago (`now`), or failed and a message has been sent since.
    static func card(in blocks: [Block], now: Int64) -> (id: String, card: RemoteCard)? {
        guard let i = blocks.lastIndex(where: { if case .card = $0.kind { true } else { false } }),
              case .card(let card) = blocks[i].kind else { return nil }
        if let until = card.chipUntil, now >= until { return nil }
        if card.phase == .failed || card.phase == .closed,
           blocks[(i + 1)...].contains(where: { if case .user(_, _, _, let aside) = $0.kind { !aside } else { false } }) {
            return nil
        }
        return (blocks[i].itemID, card)
    }
}

/// Over the composer, on the right: the chip of the thread's newest card, while it's news. Tap
/// it for the card itself, live, with its buttons.
struct CardChipRow: View {
    let store: ThreadStore
    @Environment(AppModel.self) private var model
    @State private var showing = false
    /// Ticks the chip away once a finished card has lingered.
    @State private var now = RemoteCard.nowMs()

    var body: some View {
        let found = CardChip.card(in: store.blocks, now: now)
        Group {
            if let card = found?.card {
                HStack {
                    Spacer(minLength: 0)
                    Button { showing = true } label: { CardChip(card: card) }
                        .buttonStyle(CardPress())
                }
                .transition(.opacity.combined(with: .scale(scale: 0.96, anchor: .trailing)))
                .task(id: card.chipUntil) {
                    guard let until = card.chipUntil else { return }
                    let wait = max(0, until - RemoteCard.nowMs())
                    try? await Task.sleep(for: .milliseconds(wait))
                    withAnimation(.easeOut(duration: 0.25)) { now = RemoteCard.nowMs() }
                }
                .sheet(isPresented: $showing) {
                    CardSheet(store: store)
                        .presentationDetents([.medium, .large])
                        .presentationDragIndicator(.visible)
                }
            }
        }
        .animation(.snappy(duration: 0.2), value: found?.card.phase)
        .onChange(of: store.blocks.count) { now = RemoteCard.nowMs() }
    }
}

/// The newest card, live, in a sheet over the thread.
private struct CardSheet: View {
    let store: ThreadStore
    @Environment(AppModel.self) private var model

    var body: some View {
        let card = store.blocks.reversed().lazy.compactMap { b -> RemoteCard? in
            if case .card(let c) = b.kind { c } else { nil }
        }.first
        ScrollView {
            if let card {
                LiveCard(card: card, act: model.serves(.cardActions, on: store.id) ? { model.cardAction(store.id, key: card.key, $0) } : nil)
                    .padding(16)
            }
        }
        .background(Trek.background)
    }
}
