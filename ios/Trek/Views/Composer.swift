import SwiftUI

/// The floating glass composer. While the agent works, a follow-up either steers the running turn
/// or queues for after it (the desktop's follow-up setting), and the button becomes Stop.
struct Composer: View {
    @Binding var text: String
    @Binding var mode: SendMode
    var placeholder: String
    var working: Bool
    var send: () -> Void
    var stop: () -> Void
    @FocusState private var focused: Bool

    private var empty: Bool { text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }

    var body: some View {
        GlassEffectContainer(spacing: 10) {
            HStack(alignment: .bottom, spacing: 10) {
                Menu {
                    Picker("Follow-ups", selection: $mode) {
                        ForEach(SendMode.allCases) { m in
                            Label {
                                Text(m.label)
                                Text(m.help)
                            } icon: {
                                Image(systemName: m == .steer ? "arrow.turn.down.right" : "text.line.last.and.arrowtriangle.forward")
                            }
                            .tag(m)
                        }
                    }
                    .pickerStyle(.inline)
                } label: {
                    Image(systemName: working ? (mode == .steer ? "arrow.turn.down.right" : "text.line.last.and.arrowtriangle.forward") : "plus")
                        .font(.system(size: 17, weight: .medium))
                        .foregroundStyle(working ? Trek.ember : Trek.foreground)
                        .frame(width: 44, height: 44)
                        .contentTransition(.symbolEffect(.replace))
                }
                .glassEffect(.regular.interactive(), in: .circle)
                .accessibilityLabel("Follow-up mode: \(mode.label)")

                HStack(alignment: .bottom, spacing: 8) {
                    VStack(alignment: .leading, spacing: 2) {
                        if working, !empty || focused {
                            Text(mode == .steer ? "Steer the running turn" : "Queue for after this turn")
                                .font(.caption2.weight(.semibold))
                                .foregroundStyle(Trek.ember)
                                .transition(.opacity)
                        }
                        TextField(placeholder, text: $text, axis: .vertical)
                            .lineLimit(1...6)
                            .font(.system(size: 17))
                            .focused($focused)
                    }
                    .padding(.vertical, 11)
                    .padding(.leading, 16)

                    Button {
                        if working && empty { stop() } else { send() }
                    } label: {
                        Image(systemName: working && empty ? "stop.fill" : "arrow.up")
                            .font(.system(size: working && empty ? 13 : 16, weight: .bold))
                            .foregroundStyle(Trek.background)
                            .frame(width: 34, height: 34)
                            .background(Circle().fill(!working && empty ? Trek.muted.opacity(0.35) : Trek.foreground))
                            .contentTransition(.symbolEffect(.replace))
                    }
                    .buttonStyle(.plain)
                    .disabled(!working && empty)
                    .padding(5)
                    .accessibilityLabel(working && empty ? "Stop" : "Send")
                }
                .glassEffect(.regular.interactive(), in: RoundedRectangle(cornerRadius: 22, style: .continuous))
            }
        }
        .animation(.snappy(duration: 0.2), value: working)
        .animation(.snappy(duration: 0.2), value: empty)
    }
}
