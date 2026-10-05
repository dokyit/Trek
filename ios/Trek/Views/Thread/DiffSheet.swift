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

    var body: some View {
        NavigationStack(path: $path) {
            List {
                Section {
                    ForEach(files) { f in
                        NavigationLink(value: f.path) { ChangedFileRow(file: f) }
                    }
                } footer: {
                    if let n = model.gitStatus[target]?.files.count, n != files.count {
                        Text("Diffs come from the folder's git status as it is now: a file committed or changed back since has none.")
                    }
                }
            }
            .scrollContentBackground(.hidden)
            .background(Trek.background)
            .navigationTitle("Changed files")
            .navigationBarTitleDisplayMode(.inline)
            .navigationDestination(for: String.self) { p in
                DiffScreen(target: target, file: files.first { $0.path == p } ?? ChangedFile(path: p, status: .modified))
            }
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button { dismiss() } label: { Image(systemName: "checkmark") }
                        .accessibilityLabel("Done")
                }
            }
        }
        .onAppear { model.loadGitStatus(target) }
    }
}

/// A changed file in a list: badge, name (struck through when deleted), its tag, its folder under
/// it, and its lines on the right.
struct ChangedFileRow: View {
    var file: ChangedFile

    var body: some View {
        HStack(spacing: 10) {
            FileBadge(path: file.path, size: 20)
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 6) {
                    Text(file.name)
                        .font(.subheadline.weight(.medium))
                        .strikethrough(file.status == .deleted)
                        .foregroundStyle(file.status == .deleted ? Trek.muted : Trek.foreground)
                        .lineLimit(1)
                        .truncationMode(.middle)
                        .layoutPriority(1)
                    FileTag(file: file)
                }
                let dir = (file.path as NSString).deletingLastPathComponent
                if !dir.isEmpty {
                    Text(dir)
                        .font(.caption)
                        .foregroundStyle(Trek.muted)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
            }
            Spacer(minLength: 6)
            FileLines(file: file)
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(FileLines.spoken(file))
    }
}

/// One file's diff, as git has it in the folder now (`git_diff`).
struct DiffScreen: View {
    var target: GitTarget
    var file: ChangedFile
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
            DiffText(diff: diff.diff)
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
            .font(.subheadline)
            .foregroundStyle(Trek.muted)
            .padding(.horizontal, 16)
    }
}

/// A unified diff, a row per line: additions on green, removals on red, each hunk under a quiet
/// band, the line's number in a gutter. Long lines wrap, so nothing runs off the side.
struct DiffText: View {
    var diff: String

    struct Line: Identifiable {
        enum Kind { case hunk, added, removed, context, note }
        var id: Int
        var kind: Kind
        var number: Int?
        var text: String
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
        return out
    }

    var body: some View {
        let lines = Self.parse(diff)
        let gutter = CGFloat(String(lines.compactMap(\.number).max() ?? 0).count) * 7.5 + 10
        LazyVStack(alignment: .leading, spacing: 0) {
            if lines.isEmpty {
                Text("No lines changed.").font(.subheadline).foregroundStyle(Trek.muted).padding(.horizontal, 16)
            }
            ForEach(lines) { line in
                row(line, gutter: gutter)
            }
        }
        .font(.system(size: 12, design: .monospaced))
        .background(Trek.surface)
        .overlay(alignment: .top) { Rectangle().fill(Trek.border).frame(height: 0.5) }
        .overlay(alignment: .bottom) { Rectangle().fill(Trek.border).frame(height: 0.5) }
    }

    @ViewBuilder
    private func row(_ line: Line, gutter: CGFloat) -> some View {
        switch line.kind {
        case .hunk:
            Text(line.text)
                .foregroundStyle(Trek.question)
                .lineLimit(1)
                .truncationMode(.tail)
                .padding(.horizontal, 12)
                .padding(.vertical, 5)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Trek.question.opacity(0.08))
        case .note:
            Text(line.text).italic().foregroundStyle(Trek.muted).padding(.horizontal, 12).padding(.vertical, 2)
        case .added, .removed, .context:
            let tint: Color? = line.kind == .added ? Trek.additions : line.kind == .removed ? Trek.deletions : nil
            HStack(alignment: .firstTextBaseline, spacing: 0) {
                Text(line.number.map(String.init) ?? "")
                    .foregroundStyle(Trek.muted.opacity(0.7))
                    .frame(width: gutter, alignment: .trailing)
                Text(line.kind == .added ? "+" : line.kind == .removed ? "−" : " ")
                    .foregroundStyle(tint ?? Trek.muted)
                    .frame(width: 16)
                Text(line.text.isEmpty ? " " : line.text)
                    .foregroundStyle(Trek.foreground.opacity(line.kind == .context ? 0.78 : 0.95))
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .padding(.trailing, 10)
            .padding(.vertical, 1.5)
            .background(tint.map { $0.opacity(0.11) } ?? .clear)
            .overlay(alignment: .leading) {
                if let tint { Rectangle().fill(tint.opacity(0.7)).frame(width: 2) }
            }
        }
    }
}
