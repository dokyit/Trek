import PhotosUI
import SwiftUI

/// The floating glass composer. + attaches photos (and, from a Mac that takes them, videos and
/// files); while the agent works, a follow-up either steers the running turn or queues for after
/// it (the desktop's follow-up setting), and the button becomes Stop.
struct Composer: View {
    @Binding var text: String
    @Binding var mode: SendMode
    @Binding var photos: [PickedPhoto]
    @Binding var files: [PickedFile]
    var placeholder: String
    var working: Bool
    /// The Mac takes videos and files (`uploads`).
    var uploads = false
    /// They're going up: the message waits for them, as it is.
    var uploading = false
    /// Bumped to bring the keyboard up.
    var focusRequest = 0
    var send: () -> Void
    var stop: () -> Void
    @FocusState private var focused: Bool
    @State private var voice = Dictation()
    @Environment(AppModel.self) private var model: AppModel?
    @Environment(\.scenePhase) private var scenePhase
    @Environment(\.dynamicTypeSize) private var typeSize
    /// Where the caret goes back to if the dictation is dropped.
    @State private var wasFocused = false
    @MotionAllowed private var motion

    private var empty: Bool { text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && photos.isEmpty && files.isEmpty }

    var body: some View {
        GlassEffectContainer(spacing: 10) {
            HStack(alignment: .bottom, spacing: 10) {
                // While dictating the capsule takes the full width.
                if !voice.listening {
                    AttachPicker(photos: $photos, files: $files, uploads: uploads) { [ink = Trek.foreground] in
                        Image(systemName: "plus")
                            .font(.system(size: 17, weight: .medium))
                            .foregroundStyle(ink)
                            .frame(width: 44, height: 44)
                    }
                    .glassEffect(.regular.interactive(), in: .circle)
                    .disabled(uploading)
                    .transition(.opacity)
                }

                HStack(alignment: .bottom, spacing: 8) {
                    VStack(alignment: .leading, spacing: 2) {
                        if !photos.isEmpty || !files.isEmpty { AttachmentStrip(photos: $photos, files: $files, locked: uploading) }
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
                                .font(.ui(.caption2).weight(.semibold))
                                .foregroundStyle(Trek.muted)
                            }
                            .transition(.opacity)
                            .accessibilityLabel("Follow-up mode: \(mode.label)")
                        }
                        if voice.listening {
                            // A TextField can't style a range, so while dictating it steps aside
                            // for a Text run: typed words plain, interim words italic and muted.
                            Group {
                                if text.isEmpty && voice.heard.isEmpty {
                                    Text("Listening…")
                                        .foregroundStyle(Trek.muted)
                                } else {
                                    listeningText(voice.heard.text)
                                }
                            }
                            .font(.ui(size: 17, relativeTo: .body))
                            .lineLimit(typeSize.isAccessibilitySize ? 3 : 6)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            DictationWave(levels: voice.levels)
                                .frame(height: 22)
                                .padding(.top, 4)
                        } else {
                            // The agent's name left "Messa…" at the accessibility sizes.
                            TextField(typeSize.isAccessibilitySize ? "Message" : placeholder, text: $text, axis: .vertical)
                                // At the accessibility sizes six lines would leave no transcript.
                                .lineLimit(1...(typeSize.isAccessibilitySize ? 3 : 6))
                                .font(.ui(size: 17, relativeTo: .body))
                                .focused($focused)
                                // Going out as it is once its files are up.
                                .disabled(uploading)
                                .opacity(uploading ? 0.6 : 1)
                        }
                    }
                    .padding(.vertical, 11)
                    .padding(.leading, 16)

                    if voice.listening {
                        Button {
                            voice.drop()
                            focused = wasFocused
                        } label: {
                            Image(systemName: "xmark")
                                .font(.system(size: 13, weight: .bold))
                                .foregroundStyle(Trek.foreground)
                                .frame(width: 34, height: 34)
                                .background(Circle().fill(Trek.foreground.opacity(0.12)))
                        }
                        .buttonStyle(.plain)
                        .padding(5)
                        .accessibilityLabel("Discard dictation")
                        Button {
                            keepDictation()
                        } label: {
                            Image(systemName: "checkmark")
                                .font(.system(size: 15, weight: .bold))
                                .foregroundStyle(Trek.background)
                                .frame(width: 34, height: 34)
                                .background(Circle().fill(Trek.foreground))
                        }
                        .buttonStyle(.plain)
                        .padding(5)
                        .accessibilityLabel("Keep dictation")
                    } else {
                        Button {
                            wasFocused = focused
                            // Keyboard down first: the waveform takes the field's place.
                            focused = false
                            Task { await voice.start(failed: { model?.show($0, error: true) }) }
                        } label: {
                            // The send button's shape and padding, so the two line up and read as
                            // one pair: a soft round button beside the filled one.
                            Image(systemName: "mic")
                                .font(.system(size: 15, weight: .semibold))
                                .foregroundStyle(Trek.foreground.opacity(0.8))
                                .frame(width: 34, height: 34)
                                .background(Circle().fill(Trek.foreground.opacity(0.08)))
                        }
                        .buttonStyle(.plain)
                        .padding(.vertical, 5)
                        .padding(.trailing, -6)
                        .disabled(uploading)
                        .accessibilityLabel("Speak your message")
                        Button {
                            if working && empty { stop() } else { send() }
                        } label: {
                            Group {
                                if uploading {
                                    ProgressView().tint(Trek.background).controlSize(.small)
                                } else {
                                    Image(systemName: working && empty ? "stop.fill" : "arrow.up")
                                        .font(.system(size: working && empty ? 13 : 16, weight: .bold))
                                        .contentTransition(.symbolEffect(.replace))
                                }
                            }
                            .foregroundStyle(Trek.background)
                            .frame(width: 34, height: 34)
                            .background(Circle().fill(!working && empty ? Trek.muted.opacity(0.35) : Trek.foreground))
                        }
                        .buttonStyle(.plain)
                        .disabled(!working && empty || uploading)
                        .padding(5)
                        .accessibilityLabel(uploading ? "Sending its files" : working && empty ? "Stop" : "Send")
                    }
                }
                // Not `.interactive()`: its touch tracking took a tap on a menu row over the
                // capsule (the + menu's, the follow-up menu's) and the row never chose.
                .glassEffect(.regular, in: RoundedRectangle(cornerRadius: 22, style: .continuous))
            }
        }
        .animation(.snappy(duration: 0.2), value: working)
        .animation(.snappy(duration: 0.2), value: empty)
        .animation(motion ? .snappy(duration: 0.2) : nil, value: voice.listening)
        .onChange(of: focusRequest) { focused = true }
        // Listening can't carry on in the background; keep the words rather than lose them.
        .onChange(of: scenePhase) { _, phase in
            if phase != .active && voice.listening { keepDictation(focus: false) }
        }
        // Leaving the thread stops the engine and keeps the words in the draft.
        .onDisappear { if voice.listening { keepDictation(focus: false) } }
    }

    /// Typed words plain, the interim dictation italic and muted (a TextField can't style a range,
    /// which is why the field steps aside for this while listening).
    private func listeningText(_ heard: String) -> Text {
        let gap = text.isEmpty || heard.isEmpty || text.last?.isWhitespace == true ? "" : " "
        var said = AttributedString(gap + heard)
        said.inlinePresentationIntent = .emphasized
        said.foregroundColor = Trek.muted
        return Text(AttributedString(text) + said)
    }

    /// Fold what's been heard into the field — after whatever was typed — and stop listening.
    private func keepDictation(focus: Bool = true) {
        let heard = voice.heard
        let kept = voice.keep()
        if !kept.isEmpty { text = heard.merged(into: text) }
        if focus { focused = true }
    }
}

/// The dictation waveform: `levels` drawn newest-at-the-right as centred bars. Silence still draws
/// a minimum-height dot, so the strip reads as a continuous row rather than a blank gap. The left
/// edge fades out where old bars scroll off.
private struct DictationWave: View {
    var levels: [Float]

    var body: some View {
        Canvas { context, size in
            let bar: CGFloat = 2.5
            let gap: CGFloat = 2
            let step = bar + gap
            let shown = levels.suffix(Int(size.width / step))
            for (i, level) in shown.enumerated() {
                let height = max(bar, CGFloat(level) * size.height)
                let x = size.width - CGFloat(shown.count - i) * step
                let dot = height <= bar
                context.fill(
                    Path(roundedRect: CGRect(x: x, y: (size.height - height) / 2, width: bar, height: height),
                         cornerRadius: bar / 2),
                    with: .color(dot ? Trek.muted.opacity(0.5) : Trek.foreground.opacity(0.55)))
            }
        }
        .mask(LinearGradient(stops: [.init(color: .clear, location: 0), .init(color: .black, location: 0.15)],
                             startPoint: .leading, endPoint: .trailing))
        .accessibilityHidden(true)
    }
}
