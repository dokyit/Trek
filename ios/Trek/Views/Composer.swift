import PhotosUI
import SwiftUI

/// The floating glass composer. + attaches photos; while the agent works, a follow-up either
/// steers the running turn or queues for after it (the desktop's follow-up setting), and the
/// button becomes Stop.
struct Composer: View {
    @Binding var text: String
    @Binding var mode: SendMode
    @Binding var photos: [PickedPhoto]
    var placeholder: String
    var working: Bool
    /// Bumped to bring the keyboard up.
    var focusRequest = 0
    var send: () -> Void
    var stop: () -> Void
    @FocusState private var focused: Bool
    @State private var picked: [PhotosPickerItem] = []

    private var empty: Bool { text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && photos.isEmpty }

    var body: some View {
        GlassEffectContainer(spacing: 10) {
            HStack(alignment: .bottom, spacing: 10) {
                PhotosPicker(selection: $picked, maxSelectionCount: 4, matching: .images) { [ink = Trek.foreground] in
                    Image(systemName: "plus")
                        .font(.system(size: 17, weight: .medium))
                        .foregroundStyle(ink)
                        .frame(width: 44, height: 44)
                }
                .glassEffect(.regular.interactive(), in: .circle)
                .accessibilityLabel("Add photos")

                HStack(alignment: .bottom, spacing: 8) {
                    VStack(alignment: .leading, spacing: 2) {
                        if !photos.isEmpty { PhotoStrip(photos: $photos) }
                        // While a turn runs: where a follow-up goes, and a menu to change it.
                        if working {
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
                                HStack(spacing: 3) {
                                    Image(systemName: mode == .steer ? "arrow.turn.down.right" : "text.line.last.and.arrowtriangle.forward")
                                    Text(mode == .steer ? "Steer the running turn" : "Queue for after this turn")
                                    Image(systemName: "chevron.down").font(.system(size: 8, weight: .bold))
                                }
                                .font(.caption2.weight(.semibold))
                                .foregroundStyle(Trek.muted)
                            }
                            .transition(.opacity)
                            .accessibilityLabel("Follow-up mode: \(mode.label)")
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
        .onChange(of: focusRequest) { focused = true }
        .onChange(of: picked) {
            let items = picked
            picked = []
            Task {
                let loaded = await PickedPhoto.load(items)
                withAnimation(.snappy) { photos.append(contentsOf: loaded) }
            }
        }
    }
}
