import SwiftUI

/// The card under a finished turn when it changed files, as the Mac's (`changes_card.rs`): "CHANGED
/// FILES (3) · +54 / −5" with Collapse all and View diff, then the files grouped by folder, each
/// with its type's badge and the lines it gained and lost. Tapping a file shows its diff.
struct ChangesCard: View {
    var changes: TurnChanges
    var threadID: String
    /// What the top folder is called (the project's name).
    var rootName: String
    /// The folders folded, by path ("" for the top folder).
    @Binding var folded: Set<String>
    @State private var diff: DiffRequest?

    private var groups: [(dir: String, files: [ChangedFile])] { Self.folders(changes.files) }
    private var allFolded: Bool { groups.allSatisfy { folded.contains($0.dir) } }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            Rectangle().fill(Trek.foreground.opacity(0.07)).frame(height: 0.5)
            VStack(alignment: .leading, spacing: 0) {
                ForEach(groups, id: \.dir) { group in
                    folderRow(group.dir, count: group.files.count)
                    if !folded.contains(group.dir) {
                        ForEach(group.files) { file in
                            fileRow(file)
                        }
                    }
                }
            }
            .padding(4)
        }
        .background(Trek.foreground.opacity(0.025), in: RoundedRectangle(cornerRadius: 14, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(Trek.foreground.opacity(0.1), lineWidth: 0.75))
        .sheet(item: $diff) { request in
            DiffSheet(target: .thread(threadID), files: changes.files, opening: request.path)
        }
    }

    /// The top folder's own files first, then the folders by path (`changes_card::folders`).
    static func folders(_ files: [ChangedFile]) -> [(dir: String, files: [ChangedFile])] {
        var out: [(dir: String, files: [ChangedFile])] = []
        for f in files {
            let dir = (f.path as NSString).deletingLastPathComponent
            if let i = out.firstIndex(where: { $0.dir == dir }) { out[i].files.append(f) } else { out.append((dir, [f])) }
        }
        return out.sorted { a, b in a.dir.isEmpty != b.dir.isEmpty ? a.dir.isEmpty : a.dir < b.dir }
    }

    private var header: some View {
        HStack(spacing: 6) {
            HStack(spacing: 5) {
                Text("CHANGED FILES (\(changes.files.count))").tracking(0.3)
                if changes.added + changes.removed > 0 {
                    Text("·")
                    LineCounts(added: changes.added, removed: changes.removed, quiet: false)
                }
            }
            .font(.caption.weight(.medium))
            .foregroundStyle(Trek.muted)
            .lineLimit(1)
            .layoutPriority(1)
            Spacer(minLength: 4)
            // The buttons keep their words when there's room for them, and are icons when not.
            ViewThatFits(in: .horizontal) {
                HStack(spacing: 2) { foldButton(labelled: true); diffButton(labelled: true) }
                HStack(spacing: 0) { foldButton(labelled: false); diffButton(labelled: false) }
            }
        }
        .padding(.leading, 12)
        .padding(.trailing, 4)
        .frame(minHeight: 40)
    }

    private func foldButton(labelled: Bool) -> some View {
        Button {
            withAnimation(.snappy(duration: 0.22)) { folded = allFolded ? [] : Set(groups.map(\.dir)) }
        } label: {
            headerLabel(allFolded ? "Expand all" : "Collapse all",
                        symbol: allFolded ? "arrow.up.and.line.horizontal.and.arrow.down" : "arrow.down.and.line.horizontal.and.arrow.up", labelled: labelled)
        }
        .buttonStyle(.plain)
        .accessibilityLabel(allFolded ? "Expand all" : "Collapse all")
    }

    private func diffButton(labelled: Bool) -> some View {
        Button {
            diff = DiffRequest(path: nil)
        } label: {
            headerLabel("View diff", symbol: "doc.text.magnifyingglass", labelled: labelled)
        }
        .buttonStyle(.plain)
        .accessibilityLabel("View diff")
    }

    private func headerLabel(_ title: String, symbol: String, labelled: Bool) -> some View {
        HStack(spacing: 4) {
            Image(systemName: symbol).font(.system(size: 11, weight: .semibold))
            if labelled { Text(title).font(.caption.weight(.semibold)) }
        }
        .foregroundStyle(Trek.foreground.opacity(0.85))
        .fixedSize()
        .padding(.horizontal, 8)
        .frame(minWidth: 34, minHeight: 34)
        .contentShape(Rectangle())
    }

    private func folderRow(_ dir: String, count: Int) -> some View {
        let open = !folded.contains(dir)
        return Button {
            withAnimation(.snappy(duration: 0.2)) {
                if open { folded.insert(dir) } else { folded.remove(dir) }
            }
        } label: {
            HStack(spacing: 6) {
                Image(systemName: "chevron.right")
                    .font(.system(size: 9, weight: .bold))
                    .rotationEffect(.degrees(open ? 90 : 0))
                    .frame(width: 12)
                Image(systemName: open ? "folder" : "folder.fill").font(.system(size: 12))
                Text(dir.isEmpty ? rootName : dir)
                    .lineLimit(1)
                    .truncationMode(.middle)
                if !open {
                    Text("\(count)").foregroundStyle(Trek.muted.opacity(0.7))
                }
                Spacer(minLength: 0)
            }
            .font(.footnote)
            .foregroundStyle(Trek.muted)
            .padding(.horizontal, 8)
            .frame(minHeight: 32)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("\(dir.isEmpty ? rootName : dir), \(count) file\(count == 1 ? "" : "s"), \(open ? "open" : "folded")")
    }

    private func fileRow(_ f: ChangedFile) -> some View {
        Button {
            diff = DiffRequest(path: f.path)
        } label: {
            HStack(spacing: 8) {
                FileBadge(path: f.path, size: 16)
                Text(f.name)
                    .font(.subheadline)
                    .strikethrough(f.status == .deleted)
                    .foregroundStyle(f.status == .deleted ? Trek.muted : Trek.foreground.opacity(0.92))
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .layoutPriority(1)
                FileTag(file: f)
                Spacer(minLength: 6)
                FileLines(file: f)
            }
            .padding(.leading, 30)
            .padding(.trailing, 8)
            .frame(minHeight: 34)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(FileLines.spoken(f))
        .accessibilityHint("Shows the diff")
    }
}

/// Which file's diff to show (none: the list).
struct DiffRequest: Identifiable {
    var path: String?
    var id: String { path ?? "" }
}

/// "+6 / −5" in green and red, monospaced; quiet (muted) where a side is zero.
struct LineCounts: View {
    var added: Int
    var removed: Int
    /// Zeros in muted grey, as the Mac's file rows have them.
    var quiet = true

    var body: some View {
        HStack(spacing: 3) {
            Text("+\(added)").foregroundStyle(quiet && added == 0 ? Trek.muted.opacity(0.6) : Trek.additions)
            Text("/").foregroundStyle(Trek.muted.opacity(0.6))
            Text("−\(removed)").foregroundStyle(quiet && removed == 0 ? Trek.muted.opacity(0.6) : Trek.deletions)
        }
        .font(.system(.caption, design: .monospaced))
        .monospacedDigit()
        .fixedSize()
    }
}

/// A file's lines on the right of its row: "+12 / −3", or "binary".
struct FileLines: View {
    var file: ChangedFile

    var body: some View {
        if file.binary == true {
            Text("binary").font(.system(.caption, design: .monospaced)).foregroundStyle(Trek.muted).fixedSize()
        } else {
            LineCounts(added: file.added, removed: file.removed)
        }
    }

    /// The row for VoiceOver: "pairing.rs, new, 13 lines added".
    static func spoken(_ f: ChangedFile) -> String {
        var parts = [f.name]
        switch f.status {
        case .added, .untracked: parts.append("new")
        case .deleted: parts.append("deleted")
        case .renamed: if let from = f.from { parts.append("renamed from \((from as NSString).lastPathComponent)") }
        case .modified: break
        }
        if f.binary == true { parts.append("binary") } else { parts.append("\(f.added) added, \(f.removed) removed") }
        return parts.joined(separator: ", ")
    }
}

/// The tag after a file's name: "new", "deleted", or "from <old name>" for a renamed one.
struct FileTag: View {
    var file: ChangedFile

    var body: some View {
        switch file.status {
        case .added, .untracked: tag("new", Trek.additions)
        case .deleted: tag("deleted", Trek.deletions)
        case .renamed: if let from = file.from { tag("from \((from as NSString).lastPathComponent)", Trek.muted) }
        case .modified: EmptyView()
        }
    }

    private func tag(_ text: String, _ color: Color) -> some View {
        Text(text)
            .font(.caption2.weight(.medium))
            .foregroundStyle(color)
            .lineLimit(1)
            .truncationMode(.middle)
            .padding(.horizontal, 5)
            .padding(.vertical, 1.5)
            .background(color.opacity(0.12), in: RoundedRectangle(cornerRadius: 4, style: .continuous))
    }
}
