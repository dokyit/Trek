import SwiftUI

/// "Send to Claude Code?": before the first message leaves this iPhone (and before the first to an
/// agent whose provider hasn't been named yet), where it goes, in a few plain lines: the paired
/// computer, the agent and whose service that is, and Trek's own part, which is none. Allow sends
/// what waited; Not now sends nothing and leaves it where it was typed. Settings › Privacy shows
/// the choice and takes it back. Drawn like the pairing confirmation, the app's other "are you
/// sure where this goes" sheet.
struct SharingSheet: View {
    var ask: SharingAsk
    @Environment(AppModel.self) private var model
    @Environment(\.openURL) private var openURL
    @State private var height: CGFloat = 520
    @State private var dictationOnDevice = true

    private var words: SharingWords {
        SharingWords(ask.recipient, machine: ask.machine, dictationOnDevice: dictationOnDevice)
    }

    var body: some View {
        let words = words
        ScrollView {
            VStack(spacing: 18) {
                Image(systemName: "paperplane")
                    .font(.system(size: 32, weight: .medium))
                    .foregroundStyle(Trek.foreground)
                    .padding(.top, 30)
                    .accessibilityHidden(true)
                Text(words.title)
                    .font(.ui(.title2).weight(.semibold))
                    .multilineTextAlignment(.center)
                    .fixedSize(horizontal: false, vertical: true)

                VStack(alignment: .leading, spacing: 14) {
                    line(ask.recipient.local ? "desktopcomputer" : "laptopcomputer.and.iphone", words.computer)
                    line(ask.recipient.local ? "cpu" : "cloud", words.agent)
                    line("lock", words.trek)
                    if let dictation = words.dictation { line("mic", dictation) }
                }
                .padding(16)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Trek.foreground.opacity(0.05), in: RoundedRectangle(cornerRadius: 16, style: .continuous))

                VStack(spacing: 10) {
                    Button {
                        model.answerSharing(ask, allow: true)
                    } label: {
                        Text("Allow").fontWeight(.semibold).foregroundStyle(Trek.background).frame(maxWidth: .infinity)
                    }
                    .buttonStyle(.glassProminent)
                    .tint(Trek.foreground)
                    .controlSize(.extraLarge)
                    Button {
                        model.answerSharing(ask, allow: false)
                    } label: {
                        Text("Not now").frame(maxWidth: .infinity)
                    }
                    .buttonStyle(.glass)
                    .controlSize(.extraLarge)
                }
                .padding(.top, 4)

                VStack(spacing: 6) {
                    Text("Not now sends nothing. You can change this in Settings › Privacy.")
                        .foregroundStyle(Trek.muted)
                        .multilineTextAlignment(.center)
                        .fixedSize(horizontal: false, vertical: true)
                    Button {
                        openURL(SharingConsent.policy)
                    } label: {
                        HStack(spacing: 4) {
                            Text("Privacy policy")
                            Image(systemName: "arrow.up.right").font(.ui(.caption).weight(.semibold))
                        }
                        .foregroundStyle(Trek.link)
                        .frame(minHeight: 32)
                    }
                    .buttonStyle(.plain)
                }
                .font(.ui(.footnote))
            }
            .padding(.horizontal, 24)
            .padding(.bottom, 12)
            .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { height = $0 }
        }
        .scrollBounceBehavior(.basedOnSize)
        .font(.ui(.body))
        .tint(Trek.foreground)
        .presentationDetents([.height(height)])
        .presentationDragIndicator(.visible)
        .presentationBackground(Trek.background)
        .onAppear { dictationOnDevice = SharingWords.dictationOnDevice }
    }

    private func line(_ symbol: String, _ text: String) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 12) {
            Image(systemName: symbol)
                .font(.ui(.subheadline).weight(.medium))
                .foregroundStyle(Trek.muted)
                .frame(width: 24)
                .accessibilityHidden(true)
            Text(text)
                .font(.ui(.subheadline))
                .foregroundStyle(Trek.foreground)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}

/// Shows `SharingSheet` from wherever it's put. A sheet can only come up over the topmost thing
/// on screen, so each place that can send (the app itself, and each sheet with a composer) has
/// one, and the one that appeared last shows it.
private struct SharingAsker: ViewModifier {
    @Environment(AppModel.self) private var model
    @State private var id = UUID()

    func body(content: Content) -> some View {
        content
            .onAppear {
                model.sharingPresenters.removeAll { $0 == id }
                model.sharingPresenters.append(id)
            }
            .onDisappear { model.sharingPresenters.removeAll { $0 == id } }
            .sheet(item: Binding(get: { model.sharingPresenters.last == id ? model.sharingAsk : nil },
                                 set: { if $0 == nil { model.sharingDismissed() } })) { ask in
                SharingSheet(ask: ask)
            }
    }
}

extension View {
    /// This view can show the question asked before a message goes to an agent (`SharingSheet`).
    func asksBeforeSharing() -> some View { modifier(SharingAsker()) }
}
