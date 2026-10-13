import SwiftUI

/// Text too long to lay out in one piece: a message with a pasted log, a crash report.
nonisolated enum LongText {
    /// Folded past either of these. A `Text` is drawn in one layer, and one much taller than a
    /// screen or two comes out blank: the fold keeps the bubble well inside that.
    static let foldLines = 24
    static let foldBytes = 1_500

    /// The start of `text` to show in its place, or nil when it's short enough to show whole.
    /// Counts bytes and lines only as far as it needs to.
    static func preview(_ text: String) -> String? {
        let utf8 = text.utf8
        var lines = 0
        var end = utf8.startIndex
        var bytes = 0
        for i in utf8.indices {
            if bytes >= foldBytes || lines >= foldLines { break }
            if utf8[i] == UInt8(ascii: "\n") { lines += 1 }
            bytes += 1
            end = utf8.index(after: i)
        }
        guard end < utf8.endIndex else { return nil }
        // Back to the end of a line where there's one close by, else to a whole character.
        var cut = String(Substring(text[..<end]))
        if let nl = cut.lastIndex(of: "\n"), cut.distance(from: nl, to: cut.endIndex) < 400 { cut = String(cut[..<nl]) }
        return cut + " …"
    }

    /// "12 KB", "2.1 MB": how much there is in all.
    static func size(_ text: String) -> String {
        ByteCountFormatter.string(fromByteCount: Int64(text.utf8.count), countStyle: .file)
    }

    /// `text` in stretches of at most `lines` lines and about `characters` characters (a log
    /// may be one enormous line), each short enough to draw, for a lazy stack.
    static func chunks(_ text: String, lines: Int = 20, characters: Int = 2_000) -> [String] {
        var out: [String] = []
        var start = text.startIndex
        var count = 0
        var length = 0
        var i = text.startIndex
        while i < text.endIndex {
            let newline = text[i].isNewline
            if newline { count += 1 }
            length += 1
            if count == lines || (length >= characters && (newline || length >= characters * 2)) {
                // At a line's end where one comes soon enough, else mid-line.
                out.append(String(text[start..<(newline ? i : text.index(after: i))]))
                start = text.index(after: i)
                count = 0
                length = 0
            }
            i = text.index(after: i)
        }
        if start < text.endIndex { out.append(String(text[start...])) }
        return out
    }
}

/// The whole of a long text, plain and selectable, laid out as it scrolls into view.
struct LongTextSheet: View {
    var title: String
    var text: String
    @Environment(\.dismiss) private var dismiss
    @Environment(AppModel.self) private var model
    @State private var chunks: [String]?

    var body: some View {
        NavigationStack {
            Group {
                if let chunks {
                    ScrollView {
                        LazyVStack(alignment: .leading, spacing: 0) {
                            ForEach(chunks.indices, id: \.self) { i in
                                Text(verbatim: chunks[i])
                                    .font(.code(.footnote))
                                    .foregroundStyle(Trek.foreground.opacity(0.92))
                                    .frame(maxWidth: .infinity, alignment: .leading)
                                    .textSelection(.enabled)
                            }
                        }
                        .padding(16)
                    }
                } else {
                    ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
                }
            }
            .background(Trek.background)
            .navigationTitle(title)
            .navigationSubtitle(LongText.size(text))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button("Copy", systemImage: "doc.on.doc") {
                        UIPasteboard.general.string = text
                        model.show("Copied")
                    }
                }
                ToolbarItem(placement: .topBarTrailing) {
                    Button { dismiss() } label: { Image(systemName: "checkmark") }
                        .accessibilityLabel("Done")
                }
            }
        }
        .task {
            let text = text
            chunks = await Task.detached(priority: .userInitiated) { LongText.chunks(text) }.value
        }
    }
}
