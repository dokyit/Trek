import SwiftUI

/// Above the composer: the follow-ups the Mac holds for this thread, after the running turn or
/// for its usage limit's reset. Each can be changed in place, or taken back to the composer.
struct QueuedStrip: View {
    var threadID: String
    var queued: [QueuedMessage]
    /// A message taken back: for the composer.
    var takeBack: (String) -> Void
    @Environment(AppModel.self) private var model
    @State private var editing: Editing?

    private struct Editing: Identifiable {
        var index: Int
        var message: QueuedMessage
        var text: String
        var id: String { "\(index)-\(message.reset ?? false)" }
    }

    var body: some View {
        VStack(spacing: 0) {
            ForEach(Array(rows.enumerated()), id: \.offset) { n, row in
                if n > 0 { Rectangle().fill(Trek.border).frame(height: 0.5).padding(.leading, 38) }
                line(row.index, row.message)
            }
        }
        .glassEffect(.regular, in: RoundedRectangle(cornerRadius: 18, style: .continuous))
        .sheet(item: $editing) { e in
            EditQueued(text: e.text, reset: e.message.reset ?? false) { text in
                model.queueAction(threadID, index: e.index, e.message, edit: text)
            }
        }
    }

    /// Each message with its place among those waiting for the same thing.
    private var rows: [(index: Int, message: QueuedMessage)] {
        var turn = 0, reset = 0
        return queued.map { q in
            if q.reset == true { reset += 1; return (reset - 1, q) }
            turn += 1
            return (turn - 1, q)
        }
    }

    private func line(_ index: Int, _ q: QueuedMessage) -> some View {
        HStack(spacing: 10) {
            Image(systemName: "clock")
                .font(.system(size: 13, weight: .medium))
                .foregroundStyle(Trek.muted)
                .frame(width: 18)
            VStack(alignment: .leading, spacing: 1) {
                Text(Self.preview(q))
                    .font(.ui(.subheadline))
                    .foregroundStyle(Trek.foreground.opacity(0.9))
                    .lineLimit(1)
                Text(q.reset == true ? "Sends when your limit resets" : "Sends after this turn")
                    .font(.ui(.caption))
                    .foregroundStyle(Trek.muted)
            }
            Spacer(minLength: 4)
            Menu {
                Button("Edit", systemImage: "pencil") { editing = Editing(index: index, message: q, text: q.text) }
                Button("Don't send", systemImage: "arrow.uturn.backward") {
                    model.queueAction(threadID, index: index, q) { takeBack($0) }
                }
            } label: {
                Image(systemName: "ellipsis")
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundStyle(Trek.muted)
                    .frame(width: 36, height: 36)
                    .contentShape(Rectangle())
            }
            .accessibilityLabel("Queued message actions")
        }
        .padding(.leading, 12)
        .padding(.trailing, 4)
        .padding(.vertical, 4)
        .accessibilityElement(children: .combine)
    }

    /// Its first line, or its photos when it has no words.
    static func preview(_ q: QueuedMessage) -> String {
        let first = q.text.split(separator: "\n", omittingEmptySubsequences: true).first.map(String.init) ?? ""
        if !first.isEmpty { return first }
        let n = q.images ?? 0
        return "\(n) image\(n == 1 ? "" : "s")"
    }
}

/// A queued message's words, changed before it goes.
private struct EditQueued: View {
    @State var text: String
    var reset: Bool
    var save: (String) -> Void
    @Environment(\.dismiss) private var dismiss
    @FocusState private var focused: Bool

    var body: some View {
        NavigationStack {
            TextEditor(text: $text)
                .focused($focused)
                .scrollContentBackground(.hidden)
                .padding(.horizontal, 16)
                .background(Trek.background)
                .navigationTitle("Queued message")
                .navigationSubtitle(reset ? "Sends when your limit resets" : "Sends after this turn")
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) {
                        Button(role: .cancel) { dismiss() }
                    }
                    ToolbarItem(placement: .confirmationAction) {
                        Button(role: .confirm) {
                            save(text)
                            dismiss()
                        }
                        .disabled(text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                    }
                }
                .onAppear { focused = true }
        }
        .presentationDetents([.medium, .large])
    }
}
