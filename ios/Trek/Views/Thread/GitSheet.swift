import SwiftUI

/// The thread's Git panel, as the Mac's: its branch and how far it is from its upstream (or, in a
/// worktree, its base), the changed files with their diffs, Commit and Push, and the branches to
/// switch to. A worktree thread can be merged into its base or have its worktree removed; when
/// removing would lose work the Mac says what, and nothing goes until the user agrees.
struct GitSheet: View {
    var threadID: String
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var message = ""
    @State private var committing = false
    @State private var removing = false
    /// The Mac's words for what removing would lose, and whether the branch goes too.
    @State private var lose: (text: String, deleteBranch: Bool)?
    @FocusState private var writing: Bool

    private var target: GitTarget { .thread(threadID) }
    private var thread: ThreadSummary? { model.thread(threadID) }
    private var status: GitStatus? { model.gitStatus[target] }
    private var branches: GitBranches? { model.gitBranches[target] }
    private var busy: Bool { model.gitBusy.contains(target) }
    private var worktree: WorktreeStatus? { status?.worktree }
    private var base: String? { worktree?.base ?? thread?.base }

    var body: some View {
        NavigationStack {
            Form {
                branchSection
                if status?.isRepo == false {
                    Section { Text("This folder isn't a git repository.").foregroundStyle(Trek.muted) }
                } else {
                    changesSection
                    commitSection
                    if base != nil { worktreeSection }
                    branchesSection
                }
            }
            .scrollContentBackground(.hidden)
            .background(Trek.background)
            .scrollDismissesKeyboard(.interactively)
            .navigationTitle("Git")
            .navigationBarTitleDisplayMode(.inline)
            .navigationDestination(for: String.self) { path in
                DiffScreen(target: target, file: status?.files.first { $0.path == path } ?? ChangedFile(path: path, status: .modified))
            }
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    if busy { ProgressView().accessibilityLabel("Working on it") }
                }
                ToolbarItem(placement: .topBarTrailing) {
                    Button { dismiss() } label: { Image(systemName: "checkmark") }
                        .accessibilityLabel("Done")
                }
            }
            .refreshable { load() }
        }
        .onAppear(perform: load)
        .onChange(of: busy) {
            // A commit went through: the message has done its job.
            if !busy, committing {
                committing = false
                if model.toast?.isError == false { message = "" }
            }
        }
        .confirmationDialog("Remove this thread's worktree?", isPresented: $removing, titleVisibility: .visible) {
            Button("Remove, keep the branch", role: .destructive) { remove(deleteBranch: false, force: false) }
            Button("Remove and delete the branch", role: .destructive) { remove(deleteBranch: true, force: false) }
        } message: {
            Text("The thread carries on in the project folder\(base.map { " on \($0)" } ?? "").")
        }
        .alert("Lose this work?", isPresented: Binding(get: { lose != nil }, set: { if !$0 { lose = nil } })) {
            Button("Remove anyway", role: .destructive) {
                if let lose { remove(deleteBranch: lose.deleteBranch, force: true) }
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text(lose?.text ?? "")
        }
    }

    private func load() {
        model.loadGitStatus(target)
        model.loadBranches(target)
    }

    private func remove(deleteBranch: Bool, force: Bool) {
        model.removeWorktree(threadID, deleteBranch: deleteBranch, force: force) { text in
            lose = (text, deleteBranch)
        }
    }

    // MARK: Sections

    private var branchSection: some View {
        Section {
            HStack(spacing: 12) {
                Image(systemName: base != nil ? "arrow.triangle.branch" : "point.topleft.down.to.point.bottomright.curvepath")
                    .font(.system(size: 17, weight: .medium))
                    .frame(width: 38, height: 38)
                    .background(Trek.foreground.opacity(0.07), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
                VStack(alignment: .leading, spacing: 4) {
                    HStack(spacing: 6) {
                        Text(status?.branch ?? thread?.branch ?? "—")
                            .font(.system(.body, design: .monospaced).weight(.semibold))
                            .lineLimit(1)
                            .truncationMode(.middle)
                        if let b = status?.branch, b == (status?.defaultBranch ?? thread?.git?.defaultBranch) {
                            Text("default")
                                .font(.caption2.weight(.semibold))
                                .foregroundStyle(Trek.muted)
                                .padding(.horizontal, 6)
                                .padding(.vertical, 1.5)
                                .background(Trek.foreground.opacity(0.07), in: Capsule())
                        }
                    }
                    HStack(spacing: 10) {
                        Text(base.map { "Worktree from \($0)" } ?? "In the project folder")
                        aheadBehind
                    }
                    .font(.footnote)
                    .foregroundStyle(Trek.muted)
                    .lineLimit(1)
                }
            }
            .padding(.vertical, 2)
            .accessibilityElement(children: .combine)
        }
    }

    @ViewBuilder
    private var aheadBehind: some View {
        let ahead = status?.ahead ?? thread?.git?.ahead ?? 0
        let behind = status?.behind ?? thread?.git?.behind ?? 0
        if status?.hasUpstream == false && base == nil {
            Text("· not pushed yet")
        } else {
            HStack(spacing: 6) {
                Label("\(ahead)", systemImage: "arrow.up").foregroundStyle(ahead > 0 ? Trek.foreground : Trek.muted)
                Label("\(behind)", systemImage: "arrow.down").foregroundStyle(behind > 0 ? Trek.approval : Trek.muted)
            }
            .labelStyle(TightLabel())
            .monospacedDigit()
            .accessibilityLabel("\(ahead) ahead, \(behind) behind \(base ?? "upstream")")
        }
    }

    private var changesSection: some View {
        Section {
            if let status {
                if status.files.isEmpty {
                    Label("Nothing to commit", systemImage: "checkmark.circle").foregroundStyle(Trek.muted)
                }
                ForEach(status.files) { f in
                    NavigationLink(value: f.path) { ChangedFileRow(file: f) }
                }
            } else {
                ProgressView().frame(maxWidth: .infinity)
            }
        } header: {
            HStack {
                Text("Changes\(status.map { " (\($0.files.count))" } ?? "")")
                Spacer()
                if let s = status, !s.files.isEmpty {
                    LineCounts(added: s.files.reduce(0) { $0 + $1.added }, removed: s.files.reduce(0) { $0 + $1.removed }, quiet: false)
                }
            }
        } footer: {
            if base != nil { Text("Against \(base ?? "its base"): its commits, and what isn't committed yet.") }
        }
    }

    @ViewBuilder
    private var commitSection: some View {
        Section {
            TextField("Commit message", text: $message, axis: .vertical)
                .lineLimit(1...5)
                .focused($writing)
        }
        // The buttons float under the message, outside its group.
        Section {
            HStack(spacing: 10) {
                Button {
                    writing = false
                    committing = true
                    model.commit(target, message: message.trimmingCharacters(in: .whitespacesAndNewlines))
                } label: {
                    Label("Commit", systemImage: "checkmark.circle").frame(maxWidth: .infinity)
                }
                .buttonStyle(.glassProminent)
                .disabled(busy || message.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || status?.files.isEmpty != false)
                Button {
                    model.push(target)
                } label: {
                    Label("Push", systemImage: "arrow.up.circle").frame(maxWidth: .infinity)
                }
                .buttonStyle(.glass)
                .disabled(busy)
            }
            .controlSize(.large)
            .listRowBackground(Color.clear)
            .listRowInsets(EdgeInsets(top: 0, leading: 0, bottom: 0, trailing: 0))
        } footer: {
            Text("Commit takes every change, as the Mac's Git panel does.")
        }
    }

    private var worktreeSection: some View {
        Section {
            Button {
                if let thread { model.mergeWorktree(thread.id) }
            } label: {
                Label("Merge into \(base ?? "its base")", systemImage: "arrow.triangle.merge")
                    .foregroundStyle(busy || worktree?.mergeBlocked != nil ? Trek.muted : Trek.foreground)
            }
            .disabled(busy || worktree?.mergeBlocked != nil)
            Button(role: .destructive) {
                removing = true
            } label: {
                Label("Remove worktree…", systemImage: "trash")
            }
            .disabled(busy)
        } header: {
            Text("Worktree")
        } footer: {
            if let why = worktree?.mergeBlocked {
                Text(why)
            } else if let n = worktree?.unmerged, n > 0 {
                Text("\(n) commit\(n == 1 ? "" : "s") \(base ?? "the base") doesn't have yet.")
            }
        }
    }

    private var branchesSection: some View {
        Section {
            if let branches {
                ForEach(branches.branches, id: \.self) { b in
                    let current = b == (status?.branch ?? branches.current)
                    Button {
                        model.switchBranch(target, to: b)
                    } label: {
                        HStack(spacing: 8) {
                            Text(b).font(.system(.subheadline, design: .monospaced)).lineLimit(1).truncationMode(.middle)
                            if b == branches.defaultBranch {
                                Text("default").font(.caption2.weight(.semibold)).foregroundStyle(Trek.muted)
                            }
                            Spacer()
                            if current { Image(systemName: "checkmark").font(.subheadline.weight(.semibold)) }
                        }
                        // Greyed where it can't be checked out now (the footer says why).
                        .foregroundStyle(current || (!busy && status?.canSwitch != false) ? Trek.foreground : Trek.muted)
                    }
                    .disabled(current || busy || status?.canSwitch == false)
                    .accessibilityAddTraits(current ? .isSelected : [])
                }
            } else {
                ProgressView().frame(maxWidth: .infinity)
            }
        } header: {
            Text("Branches")
        } footer: {
            if let why = status?.switchBlocked { Text(why) }
        }
    }
}

/// An icon and its title close together ("↑2").
private struct TightLabel: LabelStyle {
    func makeBody(configuration: Configuration) -> some View {
        HStack(spacing: 1) {
            configuration.icon.font(.system(size: 10, weight: .bold))
            configuration.title
        }
    }
}

/// The toolbar's Git button: a branch, and how many files are changed.
struct GitButtonLabel: View {
    var changed: Int

    var body: some View {
        HStack(spacing: 4) {
            Image(systemName: "arrow.triangle.branch")
            if changed > 0 {
                Text("\(changed)").font(.subheadline.weight(.semibold)).monospacedDigit()
            }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(changed > 0 ? "Git, \(changed) changed" : "Git")
    }
}
