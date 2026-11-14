import SwiftUI

/// Changed files and their diffs, from a turn's card or the Git panel: the files listed, each
/// opening its diff. Opened on a file, it starts at that file's diff (back goes to the list).
struct DiffSheet: View {
    var target: GitTarget
    var files: [ChangedFile]
    @State private var path: [String]
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss

    init(target: GitTarget, files: [ChangedFile], opening: String? = nil) {
        self.target = target
        self.files = files
        _path = State(initialValue: opening.map { [$0] } ?? [])
    }

    /// Files are ticked viewed as on the Mac's Git panel (a Mac that serves `diff_viewed`).
    private var marks: Bool { model.serves(.diffViewed, for: target) }
    private var status: GitStatus? { model.gitStatus[target] }

    var body: some View {
        NavigationStack(path: $path) {
            List {
                Section {
                    ForEach(files) { f in
                        let viewed = marks && status?.isViewed(f.path) == true
                        NavigationLink(value: f.path) {
                            HStack(spacing: 8) {
                                ChangedFileRow(file: f).opacity(viewed ? 0.6 : 1)
                                if viewed {
                                    Image(systemName: "checkmark.circle.fill")
                                        .font(.system(size: 14))
                                        .foregroundStyle(Trek.done)
                                        .accessibilityLabel("Viewed")
                                }
                            }
                        }
                        .swipeActions(edge: .leading) {
                            if marks, status?.files.contains(where: { $0.path == f.path }) == true {
                                Button(viewed ? "Not viewed" : "Viewed", systemImage: viewed ? "circle" : "checkmark.circle") {
                                    model.setViewed(target, path: f.path, !viewed)
                                }
                                .tint(viewed ? Trek.muted : Trek.done)
                            }
                        }
                    }
                } header: {
                    if marks, let n = status.map({ s in files.filter { s.isViewed($0.path) }.count }),
                       let said = GitStatus.viewedLabel(n, of: files.count) {
                        Text(said).font(.ui(.footnote))
                    }
                } footer: {
                    Group {
                        if let n = model.gitStatus[target]?.files.count, n != files.count {
                            Text("Diffs come from the folder's git status as it is now: a file committed or changed back since has none.")
                        }
                    }
                    .font(.ui(.footnote))
                }
            }
            .scrollContentBackground(.hidden)
            .background(Trek.grouped)
            .navigationTitle("Changed files")
            .navigationBarTitleDisplayMode(.inline)
            .navigationDestination(for: String.self) { p in
                DiffScreen(target: target, file: files.first { $0.path == p } ?? ChangedFile(path: p, status: .modified),
                           files: files, go: { path = [$0] })
                    .id(p)
            }
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button { dismiss() } label: { Image(systemName: "checkmark") }
                        .accessibilityLabel("Done")
                }
            }
        }
        .modifier(DiffCommenting(target: target))
        .onAppear { model.loadGitStatus(target) }
    }
}

/// A changed file in a list: badge, name (struck through when deleted), its tag, its folder under
/// it, and its lines on the right.
struct ChangedFileRow: View {
    var file: ChangedFile
    @Environment(\.dynamicTypeSize) private var typeSize

    var body: some View {
        let big = typeSize.isAccessibilitySize
        HStack(alignment: big ? .top : .center, spacing: 10) {
            FileBadge(path: file.path, size: 20)
                .padding(.top, big ? 6 : 0)
            VStack(alignment: .leading, spacing: 2) {
                BesideOrOver(spacing: big ? 2 : 6) {
                    Text(file.name)
                        .font(.ui(.subheadline).weight(.medium))
                        .strikethrough(file.status == .deleted)
                        .foregroundStyle(file.status == .deleted ? Trek.muted : Trek.foreground)
                        .lineLimit(big ? 3 : 1)
                        .truncationMode(.middle)
                        .layoutPriority(1)
                    FileTag(file: file)
                }
                let dir = (file.path as NSString).deletingLastPathComponent
                if !dir.isEmpty {
                    Text(dir)
                        .font(.ui(.caption))
                        .foregroundStyle(Trek.muted)
                        .lineLimit(big ? 2 : 1)
                        .truncationMode(.middle)
                }
                if big { FileLines(file: file) }
            }
            if !big {
                Spacer(minLength: 6)
                FileLines(file: file)
            }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(FileLines.spoken(file))
    }
}

/// One file's diff, as git has it in the folder now (`git_diff`).
struct DiffScreen: View {
    var target: GitTarget
    var file: ChangedFile
    /// The files around this one, to go through without going back to the list.
    var files: [ChangedFile] = []
    var go: (String) -> Void = { _ in }
    @Environment(AppModel.self) private var model
    @State private var diff: GitDiff?
    @State private var gaveUp = false

    private var status: GitStatus? { model.gitStatus[target] }
    /// The Mac serves a diff only for one of the folder's changed files.
    private var inStatus: Bool { status?.files.contains { $0.path == file.path } ?? false }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                ChangedFileRow(file: file)
                    .padding(.horizontal, 16)
                content
            }
            .padding(.vertical, 14)
        }
        .background(Trek.background)
        .navigationTitle(file.name)
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            // Ticked viewed, as on the Mac: it comes back unviewed if the file changes again.
            if model.serves(.diffViewed, for: target), inStatus {
                let viewed = status?.isViewed(file.path) == true
                ToolbarItem(placement: .topBarTrailing) {
                    Button {
                        model.setViewed(target, path: file.path, !viewed)
                    } label: {
                        Image(systemName: viewed ? "checkmark.circle.fill" : "circle")
                            .foregroundStyle(viewed ? Trek.done : Trek.foreground)
                    }
                    .accessibilityLabel(viewed ? "Viewed" : "Mark viewed")
                }
            }
            if files.count > 1, let at = files.firstIndex(where: { $0.path == file.path }) {
                ToolbarItemGroup(placement: .topBarTrailing) {
                    Button { go(files[at - 1].path) } label: { Image(systemName: "chevron.up") }
                        .disabled(at == 0)
                        .accessibilityLabel("Previous file")
                    Button { go(files[at + 1].path) } label: { Image(systemName: "chevron.down") }
                        .disabled(at == files.count - 1)
                        .accessibilityLabel("Next file")
                }
            }
        }
        .task(id: status != nil) {
            guard status != nil, inStatus, file.binary != true, diff == nil else { return }
            model.gitDiff(target, path: file.path) { diff = $0 }
            // An error comes as a toast; stop waiting after a while.
            try? await Task.sleep(for: .seconds(8))
            if diff == nil { gaveUp = true }
        }
    }

    @ViewBuilder
    private var content: some View {
        if file.binary == true {
            note("A binary file: there are no lines to compare.", symbol: "doc.zipper")
        } else if let diff {
            DiffText(diff: diff.diff, path: file.path)
            if diff.truncated == true {
                note("The diff is cut short here: it's longer than the Mac sends.", symbol: "scissors")
            }
        } else if status != nil && !inStatus {
            note("Git has no change to this file now: it was committed or changed back since.", symbol: "checkmark.circle")
        } else if gaveUp {
            note("Couldn't load the diff.", symbol: "exclamationmark.triangle")
        } else {
            ProgressView().frame(maxWidth: .infinity).padding(.top, 40)
        }
    }

    private func note(_ text: String, symbol: String) -> some View {
        Label(text, systemImage: symbol)
            .font(.ui(.subheadline))
            .foregroundStyle(Trek.muted)
            .padding(.horizontal, 16)
    }
}

/// A unified diff, a row per line: additions on green, removals on red, each hunk under a quiet
/// band, the line's number in a gutter. Long lines wrap, so nothing runs off the side. In a line
/// changed rather than added, what changed stands out from what didn't. Where there's an agent to
/// tell, tapping a line writes a comment on it.
struct DiffText: View {
    var diff: String
    /// The file: what a comment on a line says it's about.
    var path: String = ""
    @Environment(\.diffComments) private var comments
    @Environment(\.dynamicTypeSize) private var typeSize
    @ScaledMetric(relativeTo: .caption) private var digit: CGFloat = 7.5
    @ScaledMetric(relativeTo: .caption) private var sign: CGFloat = 16

    struct Line: Identifiable {
        enum Kind { case hunk, added, removed, context, note }
        var id: Int
        var kind: Kind
        var number: Int?
        var text: String
        /// The characters that differ from the line it replaces.
        var changed: Range<Int>? = nil
    }

    /// Lines longer than this aren't compared: nothing of them would fit on the screen at once.
    private static let longestCompared = 2000

    /// What differs between a line and the one that replaces it: what's left once what they
    /// start and end with in common is set aside. Nil when they share too little for that to say
    /// anything, or nothing differs on that side.
    static func changedParts(_ old: String, _ new: String) -> (old: Range<Int>?, new: Range<Int>?)? {
        guard old.count <= longestCompared, new.count <= longestCompared else { return nil }
        let a = Array(old), b = Array(new)
        var start = 0
        while start < a.count, start < b.count, a[start] == b[start] { start += 1 }
        var end = 0
        while end < a.count - start, end < b.count - start, a[a.count - 1 - end] == b[b.count - 1 - end] { end += 1 }
        let shared = start + end
        guard shared * 3 >= min(a.count, b.count), shared > 0 else { return nil }
        let oldEnd = a.count - end, newEnd = b.count - end
        return (start < oldEnd ? start..<oldEnd : nil, start < newEnd ? start..<newEnd : nil)
    }

    /// Marks what changed, where a run of removed lines is followed by as many added ones.
    private static func markChanges(_ lines: inout [Line]) {
        var i = 0
        while i < lines.count {
            guard lines[i].kind == .removed else { i += 1; continue }
            var removed = i
            while removed < lines.count, lines[removed].kind == .removed { removed += 1 }
            var added = removed
            while added < lines.count, lines[added].kind == .added { added += 1 }
            if added - removed == removed - i {
                for k in 0..<(removed - i) {
                    if let parts = changedParts(lines[i + k].text, lines[removed + k].text) {
                        lines[i + k].changed = parts.old
                        lines[removed + k].changed = parts.new
                    }
                }
            }
            i = added
        }
    }

    static func parse(_ diff: String) -> [Line] {
        var out: [Line] = []
        var old = 0, new = 0
        var inHunk = false
        let rows = diff.split(separator: "\n", omittingEmptySubsequences: false)
        for (i, raw) in rows.enumerated() {
            let s = String(raw)
            if s.hasPrefix("@@") {
                inHunk = true
                // "@@ -212,9 +212,14 @@ impl Thread {"
                let parts = s.split(separator: " ")
                if parts.count > 2 {
                    old = Int(parts[1].dropFirst().split(separator: ",").first ?? "") ?? 0
                    new = Int(parts[2].dropFirst().split(separator: ",").first ?? "") ?? 0
                }
                out.append(Line(id: i, kind: .hunk, text: s))
                continue
            }
            // The file header before the first hunk says nothing the screen doesn't already.
            guard inHunk else { continue }
            if s.hasPrefix("+") {
                out.append(Line(id: i, kind: .added, number: new, text: String(s.dropFirst())))
                new += 1
            } else if s.hasPrefix("-") {
                out.append(Line(id: i, kind: .removed, number: old, text: String(s.dropFirst())))
                old += 1
            } else if s.hasPrefix("\\") {
                out.append(Line(id: i, kind: .note, text: String(s.dropFirst(2))))
            } else if s.hasPrefix("diff --git") {
                inHunk = false
            } else if !(s.isEmpty && i == rows.count - 1) {
                out.append(Line(id: i, kind: .context, number: new, text: String(s.dropFirst())))
                old += 1
                new += 1
            }
        }
        markChanges(&out)
        return out
    }

    var body: some View {
        let lines = Self.parse(diff)
        let gutter = CGFloat(String(lines.compactMap(\.number).max() ?? 0).count) * digit + 10
        LazyVStack(alignment: .leading, spacing: 0) {
            if lines.isEmpty {
                Text("No lines changed.").font(.ui(.subheadline)).foregroundStyle(Trek.muted).padding(.horizontal, 16)
            }
            ForEach(lines) { line in
                row(line, gutter: gutter)
            }
        }
        .font(.code(size: 12, relativeTo: .caption))
        .background(Trek.surface)
        .overlay(alignment: .top) { Rectangle().fill(Trek.border).frame(height: 0.5) }
        .overlay(alignment: .bottom) { Rectangle().fill(Trek.border).frame(height: 0.5) }
    }

    @ViewBuilder
    private func row(_ line: Line, gutter: CGFloat) -> some View {
        switch line.kind {
        case .hunk:
            Text(line.text)
                .foregroundStyle(Trek.indigo)
                .lineLimit(1)
                .truncationMode(.tail)
                .padding(.horizontal, 12)
                .padding(.vertical, 5)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Trek.indigo.opacity(0.08))
        case .note:
            Text(line.text).italic().foregroundStyle(Trek.muted).padding(.horizontal, 12).padding(.vertical, 2)
        case .added, .removed, .context:
            let tint: Color? = line.kind == .added ? Trek.additions : line.kind == .removed ? Trek.deletions : nil
            let code = Text(Self.attributed(line, tint: tint))
                .foregroundStyle(Trek.foreground.opacity(line.kind == .context ? 0.78 : 0.95))
                .frame(maxWidth: .infinity, alignment: .leading)
                .fixedSize(horizontal: false, vertical: true)
            let mark = Text(line.kind == .added ? "+" : line.kind == .removed ? "−" : " ")
                .foregroundStyle(tint ?? Trek.muted)
            let number = Text(line.number.map(String.init) ?? "")
                .foregroundStyle(Trek.muted.opacity(0.7))
            let body = Group {
                if typeSize.isAccessibilitySize {
                    // The number over the line: beside it, with the indent, they left the code a
                    // few letters a line.
                    VStack(alignment: .leading, spacing: 0) {
                        HStack(spacing: 6) {
                            number
                            mark
                        }
                        .dynamicTypeSize(...DynamicTypeSize.xxLarge)
                        code.padding(.leading, CGFloat(min(Self.indent(of: line.text).columns, 8)) * 2)
                    }
                    .padding(.leading, 12)
                } else {
                    HStack(alignment: .firstTextBaseline, spacing: 0) {
                        number.frame(width: gutter, alignment: .trailing)
                        mark.frame(width: sign)
                        code.padding(.leading, CGFloat(Self.indent(of: line.text).columns) * digit * Self.columnWidth)
                    }
                }
            }
            .padding(.trailing, 10)
            .padding(.vertical, 1.5)
            .background(tint.map { $0.opacity(0.11) } ?? .clear)
            .overlay(alignment: .leading) {
                if let tint { Rectangle().fill(tint.opacity(0.7)).frame(width: 2) }
            }
            .contentShape(Rectangle())
            .accessibilityElement(children: .ignore)
            .accessibilityLabel(Self.spoken(line))
            if let comments {
                VStack(alignment: .leading, spacing: 0) {
                    Button { comments.compose(path: path, line: line) } label: { body }
                        .buttonStyle(.plain)
                        .accessibilityHint("Writes a comment on this line")
                    if let comment = comments.comment(path: path, row: line.id) {
                        Button { comments.compose(path: path, line: line) } label: {
                            Label(comment.text, systemImage: "text.bubble")
                                .font(.ui(.footnote))
                                .foregroundStyle(Trek.foreground)
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .padding(.horizontal, 12)
                                .padding(.vertical, 8)
                                .background(Trek.ember.opacity(0.1))
                                .overlay(alignment: .leading) { Rectangle().fill(Trek.ember).frame(width: 2) }
                        }
                        .buttonStyle(.plain)
                        .accessibilityLabel("Your comment: \(comment.text)")
                        .accessibilityHint("Changes it")
                    }
                }
            } else {
                body
            }
        }
    }

    /// How much of a column's width each column of indent takes: on a phone, nesting has to read
    /// without using up the line. A wrapped line stays under its own indent.
    private static let columnWidth: CGFloat = 0.5
    /// The most columns of indent drawn: past this, a line would have no room left.
    private static let deepestIndent = 24

    /// A line's indent, drawn as room rather than spaces: how many characters it is, and how many
    /// columns they come to (a tab being four).
    static func indent(of text: String) -> (characters: Int, columns: Int) {
        var characters = 0, columns = 0
        for c in text {
            if c == " " { columns += 1 } else if c == "\t" { columns += 4 } else { break }
            characters += 1
        }
        return (characters, min(columns, deepestIndent))
    }

    /// The line without its indent, with what changed in it standing out.
    private static func attributed(_ line: Line, tint: Color?) -> AttributedString {
        let skip = indent(of: line.text).characters
        let body = String(line.text.dropFirst(skip))
        var text = AttributedString(body.isEmpty ? " " : body)
        if let range = line.changed, let tint, !body.isEmpty {
            // What changed may start inside the indent: only what's drawn can stand out.
            let lower = max(range.lowerBound - skip, 0), upper = max(range.upperBound - skip, 0)
            if lower < upper {
                let start = text.index(text.startIndex, offsetByCharacters: lower)
                let end = text.index(text.startIndex, offsetByCharacters: upper)
                text[start..<end].backgroundColor = tint.opacity(0.32)
            }
        }
        return text
    }

    /// The row for VoiceOver: "Line 42, added: let x = 1".
    static func spoken(_ line: Line) -> String {
        let what = line.kind == .added ? "added" : line.kind == .removed ? "removed" : "unchanged"
        return [line.number.map { "Line \($0)" }, what].compactMap { $0 }.joined(separator: ", ") + ": " + line.text
    }
}
