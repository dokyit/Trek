import SwiftUI

/// Over the composer, as the Mac's (`checks_bar`): a pull request the thread opened is failing.
/// What's failing, and the way to hand it to the agent; View on GitHub and Dismiss beside it, and
/// in its menu whether the project sends failing checks by itself.
struct ChecksBar: View {
    var thread: ThreadSummary
    var checks: FailingChecks
    @Environment(AppModel.self) private var model
    @Environment(\.openURL) private var openURL

    private var project: String? { thread.project?.id }
    private var projectName: String { thread.project?.name ?? "this project" }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                if checks.sending {
                    RingSpinner(size: 13)
                } else {
                    Image(systemName: "xmark.circle.fill").foregroundStyle(Trek.failed)
                }
                VStack(alignment: .leading, spacing: 2) {
                    Text(checks.title)
                        .font(.ui(.subheadline).weight(.semibold))
                        .foregroundStyle(Trek.foreground)
                    Text(checks.sending ? "Reading the log…" : checks.detail)
                        .font(.ui(.footnote))
                        .foregroundStyle(Trek.muted)
                        .lineLimit(3)
                }
                Spacer(minLength: 0)
                menu
            }
            .accessibilityElement(children: .combine)
            if !checks.sending {
                ViewThatFits(in: .horizontal) {
                    actions(compact: false)
                    actions(compact: true)
                }
            }
        }
        .padding(12)
        .background(Trek.surface, in: RoundedRectangle(cornerRadius: Trek.corner(16), style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: Trek.corner(16), style: .continuous).strokeBorder(Trek.failed.opacity(0.35), lineWidth: 0.75))
        .accessibilityElement(children: .contain)
    }

    private func actions(compact: Bool) -> some View {
        HStack(spacing: 8) {
            BarButton(title: "Send to agent", strong: true) { model.sendFailedChecks(thread.id) }
            if let url = URL(string: checks.url) {
                BarButton(title: compact ? "GitHub" : "View on GitHub", strong: false) { openURL(url) }
            }
            BarButton(title: "Dismiss", strong: false) { model.dismissFailedChecks(thread.id) }
            Spacer(minLength: 0)
        }
    }

    /// Whether the project sends them by itself: the Mac's project setting, off unless turned on
    /// (each go is a turn someone pays for).
    @ViewBuilder
    private var menu: some View {
        if let project {
            Menu {
                Toggle(isOn: Binding(get: { checks.auto }, set: { model.setSendsFailedChecks(project, $0) })) {
                    Label("Send by itself in \(projectName)", systemImage: "arrow.uturn.left.circle")
                    Text("Up to \(FailingChecks.autoTries) tries a pull request")
                }
            } label: {
                Image(systemName: "ellipsis")
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(Trek.muted)
                    .frame(width: 28, height: 28)
                    .contentShape(Rectangle())
            }
            .accessibilityLabel("More for the failing checks")
            .task { if model.sendsFailedChecks(project) == nil { model.loadSettings(on: model.link(for: thread.id)) } }
        }
    }
}

/// Over the composer while a move to another agent waits for the next message, as the Mac's
/// (`switch_bar`): what happens, what goes with it (its size, and the text a tap away), and the
/// way to stay.
struct SwitchBar: View {
    var thread: ThreadSummary
    var pending: PendingSwitch
    @Environment(AppModel.self) private var model
    @State private var previewing = false

    private var running: Bool { thread.runState == .working }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Image(systemName: "arrow.left.arrow.right").foregroundStyle(Trek.muted)
                // The whole sentence, wrapped: the phone's bar has the room the Mac's narrow one lacks.
                sentence(compact: false)
                Spacer(minLength: 0)
            }
            .accessibilityElement(children: .combine)
            HStack(spacing: 8) {
                BarButton(title: "Stay on \(pending.from)", strong: true) { model.stayOnAgent(thread.id) }
                BarButton(title: "What goes with it", strong: false) { previewing = true }
                Spacer(minLength: 0)
            }
        }
        .padding(12)
        .background(Trek.surface, in: RoundedRectangle(cornerRadius: Trek.corner(16), style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: Trek.corner(16), style: .continuous).strokeBorder(Trek.foreground.opacity(0.12), lineWidth: 0.75))
        .accessibilityElement(children: .contain)
        .onAppear { if Launch.sheet == "switch-preview" { previewing = true } }
        .sheet(isPresented: $previewing) { SwitchPreviewSheet(threadID: thread.id, pending: pending) }
    }

    private func sentence(compact: Bool) -> some View {
        let (lead, rest) = PendingSwitch.sentence(to: pending.to, running: running, compact: compact)
        return VStack(alignment: .leading, spacing: 2) {
            Text(lead).font(.ui(.subheadline).weight(.semibold)).foregroundStyle(Trek.foreground)
            if let rest { Text(rest).font(.ui(.footnote)).foregroundStyle(Trek.muted) }
        }
        .fixedSize(horizontal: false, vertical: true)
    }
}

/// What the next message carries to the new agent, as the Mac's preview card shows it.
struct SwitchPreviewSheet: View {
    var threadID: String
    var pending: PendingSwitch
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var preview: SwitchPreview?

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 14) {
                    if let preview {
                        Text("\(Tokens.short(preview.tokens)) of \(Tokens.short(preview.budget)) tokens")
                            .font(.ui(.footnote))
                            .foregroundStyle(Trek.muted)
                        ForEach(Array(preview.text.components(separatedBy: "\n\n").enumerated()), id: \.offset) { _, p in
                            Text(p)
                                .font(.ui(.subheadline))
                                .foregroundStyle(Trek.foreground.opacity(0.85))
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .textSelection(.enabled)
                        }
                        if let leftOut = preview.leftOut {
                            Divider()
                            Text(leftOut).font(.ui(.footnote)).foregroundStyle(Trek.muted)
                        }
                    } else {
                        ProgressView().frame(maxWidth: .infinity).padding(.top, 40)
                    }
                }
                .padding(16)
            }
            .background(Trek.background)
            .navigationTitle("Goes with your next message")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button { dismiss() } label: { Image(systemName: "checkmark") }
                        .accessibilityLabel("Done")
                }
            }
        }
        .presentationDetents([.medium, .large])
        .task { model.loadSwitchPreview(threadID) { preview = $0 } }
    }
}

/// "Compact", once a large thread's prompt cache has gone: the next message would re-read the
/// whole context at full price. Why is a tap on the info mark away; ✕ dismisses it until the
/// thread is next active (as the Mac's `compact_chip`).
struct CompactChip: View {
    var threadID: String
    var offer: CompactOffer
    var dismiss: () -> Void
    @Environment(AppModel.self) private var model
    @State private var why = false

    var body: some View {
        HStack(spacing: 6) {
            Button {
                model.threadAction(threadID, .compact, done: "Compacting")
                dismiss()
            } label: {
                HStack(spacing: 6) {
                    Image(systemName: "arrow.down.right.and.arrow.up.left")
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundStyle(Trek.muted)
                    Text("Compact").foregroundStyle(Trek.foreground)
                }
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Compact the conversation")
            .accessibilityHint(offer.why)
            Button { why = true } label: {
                Image(systemName: "info.circle").font(.system(size: 12)).foregroundStyle(Trek.muted)
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Why compact")
            .popover(isPresented: $why) {
                Text(offer.why)
                    .font(.ui(.footnote))
                    .foregroundStyle(Trek.foreground)
                    .padding(14)
                    .frame(maxWidth: 300)
                    .fixedSize(horizontal: false, vertical: true)
                    .presentationCompactAdaptation(.popover)
            }
            Button(action: dismiss) {
                Image(systemName: "xmark").font(.system(size: 9, weight: .bold)).foregroundStyle(Trek.muted.opacity(0.8))
                    .frame(width: 18, height: 18)
                    .contentShape(Circle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Dismiss")
        }
        .font(.ui(size: 13, relativeTo: .footnote))
        .padding(.leading, 12)
        .padding(.trailing, 6)
        .frame(minHeight: 32)
        .barText()
        .background(CardInk.chipFill, in: Capsule())
        .overlay(Capsule().strokeBorder(CardInk.chipBorder, lineWidth: 1))
        .shadow(color: .black.opacity(0.05), radius: 1, y: 1)
    }
}

/// A bar's button, as the limit bar's: the strong one on the card's button colour, the rest quiet.
struct BarButton: View {
    var title: String
    var strong: Bool
    var action: () -> Void

    var body: some View {
        Button(action: action) {
            Text(title)
                .font(.ui(.footnote).weight(strong ? .semibold : .medium))
                .foregroundStyle(strong ? CardInk.onButton : Trek.foreground)
                .padding(.horizontal, 12)
                .frame(minHeight: 32)
                .background(strong ? CardInk.button : Trek.foreground.opacity(0.06), in: Capsule())
                .fixedSize()
        }
        .buttonStyle(CardPress())
    }
}
