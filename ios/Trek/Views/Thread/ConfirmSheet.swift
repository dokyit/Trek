import SwiftUI

/// Undo, retry or rewind, asked first as the Mac's confirmation card asks (`thread_view.rs`
/// `confirm_card`): what leaves the conversation, and the files restoring would put back, each
/// with what happens to it ("revert", "delete", "bring back"), under "Also restore files".
struct ConfirmSheet: View {
    var title: String
    var message: String
    var verb: String
    var threadID: String
    var itemID: String
    var action: TurnAction
    var agentName: String
    var run: (_ restoreFiles: Bool) -> Void
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var preview: TurnPreview?
    @State private var failed: String?
    @State private var restore = true

    /// The most files listed; the rest are counted.
    static let shown = 8

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text(title).font(.ui(.title3).weight(.semibold))
            Text(bodyText)
                .font(.ui(.subheadline))
                .foregroundStyle(Trek.muted)
                .fixedSize(horizontal: false, vertical: true)
            files
            HStack(spacing: 10) {
                Button { dismiss() } label: {
                    Text("Cancel").font(.ui(.body).weight(.medium)).frame(maxWidth: .infinity).frame(height: 46)
                }
                .buttonStyle(.bordered)
                .tint(Trek.foreground)
                Button {
                    let keep = restore && !(preview?.files ?? []).isEmpty
                    dismiss()
                    run(keep)
                } label: {
                    Text(verb).font(.ui(.body).weight(.semibold)).frame(maxWidth: .infinity).frame(height: 46)
                }
                .buttonStyle(.borderedProminent)
                .tint(Trek.failed)
                .disabled(preview == nil && failed == nil)
            }
            .padding(.top, 4)
        }
        .padding(.horizontal, 20)
        .padding(.top, 28)
        // From the top of the sheet: the files coming in grow it downwards, nothing above moves.
        .frame(maxHeight: .infinity, alignment: .top)
        .presentationDetents([.medium, .large])
        .presentationDragIndicator(.visible)
        .task { await load() }
    }

    private var bodyText: String {
        var text = message
        if preview?.recap == true {
            text += " \(agentName) can't take its own session back to this point, so it continues in a new one with a recap."
        }
        return text
    }

    @ViewBuilder
    private var files: some View {
        if let preview {
            if let list = preview.files {
                if list.isEmpty {
                    note("Nothing to put back: the files these turns changed are as they were.")
                } else {
                    VStack(alignment: .leading, spacing: 8) {
                        Toggle(isOn: $restore) {
                            Text("Also restore files").font(.ui(.subheadline).weight(.medium))
                        }
                        .toggleStyle(CheckToggle())
                        VStack(alignment: .leading, spacing: 4) {
                            ForEach(list.prefix(Self.shown)) { f in
                                HStack(spacing: 8) {
                                    Text(f.path)
                                        .font(.code(size: 12))
                                        .foregroundStyle(Trek.foreground)
                                        .lineLimit(1)
                                        .truncationMode(.head)
                                    Spacer(minLength: 6)
                                    Text(f.verb).font(.ui(.caption)).foregroundStyle(Trek.muted).fixedSize()
                                }
                            }
                            if list.count > Self.shown {
                                Text("and \(list.count - Self.shown) more").font(.ui(.caption)).foregroundStyle(Trek.muted)
                            }
                        }
                        .padding(.leading, 28)
                        .opacity(restore ? 1 : 0.5)
                    }
                }
            } else {
                note(preview.unavailable ?? "Trek has no checkpoint for this point: the files stay as they are.")
            }
        } else if let failed {
            note(failed)
        } else {
            HStack(spacing: 8) {
                ProgressView().controlSize(.small)
                Text("Checking which files changed…").font(.ui(.footnote)).foregroundStyle(Trek.muted)
            }
        }
    }

    private func note(_ text: String) -> some View {
        Text(text).font(.ui(.footnote)).foregroundStyle(Trek.muted).fixedSize(horizontal: false, vertical: true)
    }

    private func load() async {
        let got: Got = await withCheckedContinuation { c in
            model.request(.turnPreview(threadId: threadID, itemId: itemID, action: action)) { reply in
                switch reply {
                case .turnPreview(_, let p): c.resume(returning: .preview(p))
                case .error(_, _, let text): c.resume(returning: .failed(text))
                default: c.resume(returning: .failed("The Mac didn't say which files changed."))
                }
            }
        }
        withAnimation(.easeOut(duration: 0.2)) {
            switch got {
            case .preview(let p):
                preview = p
                restore = !(p.files ?? []).isEmpty
            case .failed(let why): failed = why
            }
        }
    }
}

private enum Got {
    case preview(TurnPreview)
    case failed(String)
}

/// A checkbox, as the Mac's `check_row`.
private struct CheckToggle: ToggleStyle {
    func makeBody(configuration: Configuration) -> some View {
        Button { configuration.isOn.toggle() } label: {
            HStack(spacing: 10) {
                Image(systemName: configuration.isOn ? "checkmark.square.fill" : "square")
                    .font(.system(size: 18))
                    .foregroundStyle(configuration.isOn ? Trek.ember : Trek.muted)
                configuration.label.foregroundStyle(Trek.foreground)
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}
