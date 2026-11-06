import SwiftUI

/// Comments on a diff's lines, gathered while reading, then sent to the thread's agent in one
/// message: a plain follow-up, so it works with any Mac, steers or queues as the composer's would,
/// and shows in the transcript as what it is.
@Observable
final class DiffComments {
    /// One comment, on a line as the diff showed it.
    struct Comment: Identifiable, Equatable {
        var path: String
        /// The diff's row for the line: unique within the file.
        var row: Int
        var number: Int?
        var removed: Bool
        var quote: String
        var text: String

        var id: String { "\(path)#\(row)" }
    }

    private(set) var list: [Comment] = []
    /// The line being commented on (its sheet is up).
    var composing: Comment?

    func comment(path: String, row: Int) -> Comment? { list.first { $0.path == path && $0.row == row } }

    /// Starts a comment on a line, or takes up the one it has.
    func compose(path: String, line: DiffText.Line) {
        composing = comment(path: path, row: line.id)
            ?? Comment(path: path, row: line.id, number: line.number, removed: line.kind == .removed, quote: line.text, text: "")
    }

    /// Keeps what was written, in the order of the file; nothing written takes the comment away.
    func save(_ comment: Comment) {
        list.removeAll { $0.id == comment.id }
        guard !comment.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
        list.append(comment)
        list.sort { ($0.path, $0.row) < ($1.path, $1.row) }
    }

    func clear() { list = [] }

    /// Why the comments can't go yet, or nil: the agent waits on a question of its own (they'd be
    /// taken as its answer) or on a secret (no message goes past one). They stay till it's answered.
    nonisolated static func heldBack(_ thread: ThreadSummary?) -> String? {
        guard let thread, thread.needsThread == nil, thread.needs?.kind == .question else { return nil }
        return "Answer what \(thread.agent.name) asked first, then send the comments."
    }

    /// The message to the agent: each comment under where it is and the line it's about.
    nonisolated static func message(_ list: [Comment]) -> String {
        var out = ["Comments on the diff:"]
        for c in list {
            let place = c.number.map { "\(c.path):\($0)" } ?? c.path
            var block = ["`\(place)`\(c.removed ? " (a line it removes)" : "")"]
            let quote = c.quote.trimmingCharacters(in: .whitespaces)
            if !quote.isEmpty { block.append("> \(quote.count > 200 ? String(quote.prefix(199)) + "…" : quote)") }
            block.append(c.text.trimmingCharacters(in: .whitespacesAndNewlines))
            out.append(block.joined(separator: "\n"))
        }
        return out.joined(separator: "\n\n")
    }
}

extension EnvironmentValues {
    /// Where a diff's lines take comments: nil where there's no agent to send them to (a project's
    /// Git panel).
    @Entry var diffComments: DiffComments? = nil
}

/// Lets the diffs under it take comments: the sheet to write one, and the bar that sends them.
struct DiffCommenting: ViewModifier {
    var target: GitTarget
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var comments = DiffComments()

    private var threadID: String? {
        if case .thread(let id) = target { id } else { nil }
    }

    func body(content: Content) -> some View {
        @Bindable var comments = comments
        content
            .environment(\.diffComments, threadID == nil ? nil : comments)
            .safeAreaInset(edge: .bottom, spacing: 0) {
                if let threadID, !comments.list.isEmpty {
                    bar(threadID)
                        .transition(.move(edge: .bottom).combined(with: .opacity))
                }
            }
            .animation(.snappy(duration: 0.2), value: comments.list.isEmpty)
            .sheet(item: $comments.composing) { c in
                LineCommentSheet(comment: c) { comments.save($0) }
            }
    }

    private func bar(_ threadID: String) -> some View {
        let thread = model.thread(threadID)
        let connected = model.link(for: threadID)?.connection == .connected
        let n = comments.list.count
        return HStack(spacing: 10) {
            Button { comments.clear() } label: {
                Image(systemName: "xmark")
                    .font(.system(size: 13, weight: .semibold))
                    .frame(width: 44, height: 44)
            }
            .buttonStyle(.glass)
            .buttonBorderShape(.circle)
            .accessibilityLabel("Discard the comments")
            Button {
                if let why = DiffComments.heldBack(thread) {
                    model.show(why, error: true)
                    return
                }
                let working = thread?.runState == .working
                model.send(DiffComments.message(comments.list), to: threadID, mode: working ? model.followUpMode : nil)
                comments.clear()
                model.show("Sent to \(thread?.agent.name ?? "the agent")")
                dismiss()
            } label: {
                Text(connected ? "Send \(n) comment\(n == 1 ? "" : "s") to \(thread?.agent.name ?? "the agent")" : AppModel.notConnected)
                    .font(.ui(.subheadline).weight(.semibold))
                    .foregroundStyle(connected ? Trek.onEmber : Trek.muted)
                    .lineLimit(1)
                    .minimumScaleFactor(0.8)
                    .frame(maxWidth: .infinity, minHeight: 34)
            }
            .buttonStyle(.glassProminent)
            .tint(Trek.ember)
            .disabled(!connected)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 8)
    }
}

/// Writing a comment on one line: the line, and the field.
private struct LineCommentSheet: View {
    @State var comment: DiffComments.Comment
    var done: (DiffComments.Comment) -> Void
    @Environment(\.dismiss) private var dismiss
    @FocusState private var writing: Bool
    @Environment(\.dynamicTypeSize) private var typeSize

    private var title: String {
        let file = (comment.path as NSString).lastPathComponent
        return comment.number.map { "\(file):\($0)" } ?? file
    }

    var body: some View {
        NavigationStack {
            VStack(alignment: .leading, spacing: 12) {
                Text(comment.quote.trimmingCharacters(in: .whitespaces).isEmpty ? " " : comment.quote.trimmingCharacters(in: .whitespaces))
                    .font(.code(.footnote))
                    .foregroundStyle(Trek.foreground.opacity(0.85))
                    .lineLimit(4)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(10)
                    .background((comment.removed ? Trek.deletions : Trek.foreground).opacity(0.08),
                                in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                TextField(typeSize.isAccessibilitySize ? "Your comment" : "What should change here?", text: $comment.text, axis: .vertical)
                    .font(.ui(.body))
                    .lineLimit(2...6)
                    .focused($writing)
                    .accessibilityLabel("Your comment on \(title)")
                Spacer(minLength: 0)
            }
            .padding(16)
            .background(Trek.background)
            .navigationTitle(title)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button { dismiss() } label: { Image(systemName: "xmark") }
                        .accessibilityLabel("Cancel")
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button {
                        done(comment)
                        dismiss()
                    } label: { Image(systemName: "checkmark") }
                        .accessibilityLabel("Keep the comment")
                }
            }
        }
        .presentationDetents([.height(280), .large])
        .presentationDragIndicator(.visible)
        .onAppear { writing = true }
    }
}
