import SwiftUI

/// Home: every thread on the Mac, grouped like the desktop sidebar (Pinned · Needs you · Working ·
/// Recent). Colour only where something wants action, is moving or broke; the rest recedes.
struct ThreadsView: View {
    @Environment(AppModel.self) private var model
    @Binding var path: [String]
    @Binding var showNew: Bool
    @State private var collapsed: Set<String> = []
    @State private var projectFilter: String?

    var body: some View {
        NavigationStack(path: $path) {
            List {
                if model.mode == .live, model.connection != .connected {
                    Group {
                        if case .identityChanged(let expected, let seen) = model.connection {
                            IdentityChangedCard(host: PairedMac.load()?.hostName, expected: expected, seen: seen) { model.leave() }
                        } else {
                            ConnectionBanner(state: model.connection, host: model.host?.name ?? PairedMac.load()?.hostName) { model.leave() }
                        }
                    }
                    .listRowBackground(Color.clear)
                    .listRowSeparator(.hidden)
                    .listRowInsets(EdgeInsets(top: 4, leading: 16, bottom: 8, trailing: 16))
                }
                section("Pinned", filtered(model.pinned))
                section("Needs you", filtered(model.needsYou), tint: Trek.approval)
                section("Working", filtered(model.working), tint: Trek.working)
                section("Recent", filtered(model.recent))
                if model.threads.isEmpty, model.connection == .connected {
                    ContentUnavailableView("No threads yet", systemImage: "bubble.left.and.text.bubble.right",
                                           description: Text("Start one here or on your Mac."))
                        .listRowBackground(Color.clear)
                }
            }
            .listStyle(.plain)
            .listSectionSpacing(6)
            .environment(\.defaultMinListHeaderHeight, 0)
            .scrollContentBackground(.hidden)
            .background(RidgeBackdrop(height: 300))
            // Rows pass cleanly under the "New thread" pill and the tab bar (one hard edge for both
            // bars), and the last row clears them.
            .scrollEdgeEffectStyle(.hard, for: .bottom)
            .contentMargins(.bottom, 14, for: .scrollContent)
            .navigationTitle("Threads")
            .refreshable { await model.refresh() }
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Menu {
                        Section("Project") {
                            Picker("Project", selection: $projectFilter) {
                                Text("All projects").tag(String?.none)
                                ForEach(model.projects) { p in
                                    Text(p.name).tag(Optional(p.id))
                                }
                            }
                        }
                        Section {
                            Button("Mark all read", systemImage: "checkmark.circle") {
                                for t in model.threads where t.unseen { model.markSeen(t.id) }
                            }
                            Button("New thread", systemImage: "plus") { showNew = true }
                        }
                    } label: {
                        Image(systemName: projectFilter == nil ? "ellipsis" : "line.3.horizontal.decrease")
                    }
                }
            }
            .navigationDestination(for: String.self) { tid in
                ThreadView(threadID: tid)
            }
        }
    }

    private func filtered(_ list: [ThreadSummary]) -> [ThreadSummary] {
        guard let projectFilter else { return list }
        return list.filter { $0.project?.id == projectFilter }
    }

    @ViewBuilder
    private func section(_ title: String, _ threads: [ThreadSummary], tint: Color? = nil) -> some View {
        if !threads.isEmpty {
            Section {
                if !collapsed.contains(title) {
                    ForEach(threads) { t in
                        NavigationLink(value: t.id) {
                            ThreadRow(thread: t)
                        }
                        .navigationLinkIndicatorVisibility(.hidden)
                        .listRowBackground(Color.clear)
                        .listRowSeparator(.hidden)
                        .listRowInsets(EdgeInsets(top: 9, leading: 20, bottom: 9, trailing: 18))
                        .swipeActions(edge: .leading) {
                            if t.unseen {
                                Button("Read", systemImage: "checkmark") { model.markSeen(t.id) }.tint(Trek.done)
                            }
                        }
                        .contextMenu {
                            Button("Copy title", systemImage: "doc.on.doc") { UIPasteboard.general.string = t.title }
                            if t.runState == .working {
                                Button("Stop", systemImage: "stop.fill", role: .destructive) { model.interrupt(t.id) }
                            }
                        }
                    }
                }
            } header: {
                Button {
                    withAnimation(.snappy) {
                        if collapsed.contains(title) { collapsed.remove(title) } else { collapsed.insert(title) }
                    }
                } label: {
                    HStack(spacing: 7) {
                        Text(title).font(.subheadline.weight(.semibold)).foregroundStyle(Trek.muted)
                        Text("\(threads.count)").font(.subheadline.monospacedDigit()).foregroundStyle(tint ?? Trek.muted.opacity(0.7))
                        Spacer()
                        Image(systemName: "chevron.down")
                            .font(.caption.weight(.semibold))
                            .foregroundStyle(Trek.muted.opacity(0.8))
                            .rotationEffect(.degrees(collapsed.contains(title) ? -90 : 0))
                    }
                    .padding(.horizontal, 4)
                    .padding(.top, 10)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
            .listSectionSeparator(.hidden)
        }
    }
}

struct ThreadRow: View {
    var thread: ThreadSummary

    private var recedes: Bool { thread.runState == .idle && !thread.unseen && thread.needs == nil }

    var body: some View {
        HStack(alignment: .top, spacing: 13) {
            AgentGlyph(key: thread.agent.key, size: 24)
                .padding(.top, 1)
                .opacity(recedes ? 0.7 : 1)
            VStack(alignment: .leading, spacing: 5) {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text(thread.title)
                        .font(.system(size: 17, weight: thread.unseen ? .semibold : .regular))
                        .foregroundStyle(recedes ? Trek.foreground.opacity(0.72) : Trek.foreground)
                        .lineLimit(1)
                    Spacer(minLength: 4)
                    trailing
                }
                HStack(spacing: 6) {
                    if let p = thread.project {
                        ProjectBadge(project: p, size: 16)
                        Text(p.name).foregroundStyle(Trek.muted)
                    }
                    secondary
                        .lineLimit(1)
                        .transaction { $0.animation = nil }
                    Spacer(minLength: 0)
                    if thread.additions + thread.deletions > 0, thread.needs == nil {
                        DiffStat(additions: thread.additions, deletions: thread.deletions, font: .caption.monospacedDigit())
                            .opacity(recedes ? 0.6 : 0.9)
                    }
                }
                .font(.subheadline)
            }
        }
        .accessibilityElement(children: .combine)
    }

    @ViewBuilder
    private var trailing: some View {
        if let look = StatusLook.of(thread) {
            StatusPill(look: look)
        } else {
            HStack(spacing: 6) {
                if thread.unseen { Circle().fill(Trek.done).frame(width: 7, height: 7) }
                Text(When.short(thread.updatedAt))
                    .font(.subheadline.monospacedDigit())
                    .foregroundStyle(thread.unseen ? Trek.done : Trek.muted.opacity(0.8))
            }
        }
    }

    /// What follows the project: the request waiting on you, what the agent is doing, else the branch.
    @ViewBuilder
    private var secondary: some View {
        if let needs = thread.needs {
            Text(needs.text.replacingOccurrences(of: "`", with: "")).foregroundStyle(Trek.muted)
        } else if thread.runState == .working, let activity = thread.activity {
            Text(activity).foregroundStyle(Trek.muted)
        } else if let branch = thread.branch {
            Label {
                Text(branch)
            } icon: {
                Image(systemName: thread.worktree ? "arrow.triangle.branch" : "arrow.triangle.branch")
                    .font(.caption2)
            }
            .labelStyle(BranchLabelStyle())
            .foregroundStyle(Trek.muted.opacity(0.85))
        }
    }
}

private struct BranchLabelStyle: LabelStyle {
    func makeBody(configuration: Configuration) -> some View {
        HStack(spacing: 3) {
            configuration.icon
            configuration.title
        }
        .padding(.leading, 4)
    }
}

/// "Mac unreachable — retrying" above the list, with the cached threads still shown below it.
struct ConnectionBanner: View {
    var state: ConnectionState
    var host: String?
    /// Offered when the Mac refused this iPhone (revoked, or a code that didn't work).
    var pairAgain: (() -> Void)? = nil

    var body: some View {
        HStack(spacing: 10) {
            if state == .connecting {
                ProgressView().controlSize(.small)
            } else {
                Image(systemName: "wifi.exclamationmark").foregroundStyle(Trek.approval)
            }
            VStack(alignment: .leading, spacing: 1) {
                Text(host ?? "Your Mac").font(.subheadline.weight(.semibold))
                Text(state.label).font(.caption).foregroundStyle(Trek.muted)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Spacer()
            if case .unauthorized = state, let pairAgain {
                Button("Pair again", action: pairAgain)
                    .font(.subheadline.weight(.semibold))
                    .buttonStyle(.glass)
            }
        }
        .padding(12)
        .glassEffect(.regular, in: RoundedRectangle(cornerRadius: 16, style: .continuous))
    }
}

/// The Mac answered with a certificate other than the pinned one. Nothing reconnects until the
/// user pairs again: that's the only way a new certificate becomes trusted.
struct IdentityChangedCard: View {
    var host: String?
    var expected: String
    var seen: String?
    var pairAgain: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Label {
                Text("This Mac's identity changed").font(.headline)
            } icon: {
                Image(systemName: "exclamationmark.shield.fill").foregroundStyle(Trek.failed)
            }
            Text("\(host ?? "Your Mac") answered with a different certificate than the one this iPhone paired with. Trek may have been reinstalled on it, or something else on the network is posing as it. Trek won't connect until you pair again.")
                .font(.subheadline)
                .foregroundStyle(Trek.foreground.opacity(0.85))
                .fixedSize(horizontal: false, vertical: true)
            HStack(spacing: 14) {
                LabeledContent("Paired") { Text(expected).monospaced() }
                if let seen { LabeledContent("Now") { Text(seen).monospaced().foregroundStyle(Trek.failed) } }
            }
            .labeledContentStyle(FingerprintLabelStyle())
            Button(action: pairAgain) {
                Text("Pair again").fontWeight(.semibold).frame(maxWidth: .infinity)
            }
            .buttonStyle(.glassProminent)
            .tint(Trek.foreground)
            .controlSize(.large)
            .padding(.top, 2)
        }
        .padding(16)
        .background(Trek.failed.opacity(0.08), in: RoundedRectangle(cornerRadius: 20, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 20, style: .continuous).strokeBorder(Trek.failed.opacity(0.4), lineWidth: 1))
    }
}

private struct FingerprintLabelStyle: LabeledContentStyle {
    func makeBody(configuration: Configuration) -> some View {
        HStack(spacing: 6) {
            configuration.label.foregroundStyle(Trek.muted)
            configuration.content
        }
        .font(.footnote)
    }
}
