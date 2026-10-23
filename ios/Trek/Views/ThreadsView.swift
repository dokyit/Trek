import SwiftUI

/// Home: every thread on the Mac. By project, as the Mac's sidebar has them (Pinned, then each
/// project with what wants you or is at work on top, Snoozed, and Settled folded away), or by
/// status (Pinned · Needs you · Working · Snoozed · Recent). Colour only where something wants
/// action, is moving or broke; the rest recedes.
struct ThreadsView: View {
    @Environment(AppModel.self) private var model
    @Binding var path: [String]
    @Binding var showNew: Bool
    @AppStorage(ThreadGrouping.key) private var groupingRaw = ThreadGrouping.project.rawValue
    @State private var collapsed: Set<String> = Launch.folded
    /// Project groups showing all their quiet threads, not just the first few.
    @State private var showingAll: Set<String> = []
    @State private var settledOpen = Launch.folded.contains("-Settled")
    @State private var projectFilter: String?
    @State private var pickedMachine: String?
    /// The picked machine, while it is still paired and the picker that clears it is on offer:
    /// a machine unpaired meanwhile must not leave the list filtered to nothing.
    private var machineFilter: String? {
        guard let id = pickedMachine, model.links.count > 1, model.links.contains(where: { $0.id == id }) else { return nil }
        return id
    }
    @State private var deleteAsk: DeleteAsk?
    /// A thread whose swipe asked for the snooze menu, or a time of one's own.
    @State private var snoozeChoice: ThreadSummary?
    @State private var snoozePicking: ThreadSummary?
    @State private var showingUsage = false
    @Environment(\.compactRows) private var compact

    /// Quiet threads a project shows before "Show more" (the Mac's sidebar: 3, and 5 settled).
    private static let quietShown = 3
    private static let settledShown = 5

    private var grouping: ThreadGrouping { ThreadGrouping(rawValue: Launch.grouping ?? groupingRaw) ?? .project }

    var body: some View {
        NavigationStack(path: $path.onSet { if Perf.enabled { Perf.navigated = Perf.now } }) {
            ScrollViewReader { proxy in
                List {
                    if model.mode == .live {
                        // Each machine that isn't connected says so under its own name.
                        ForEach(model.links.filter { $0.connection != .connected }) { link in
                            Group {
                                if case .identityChanged(let expected, let seen) = link.connection {
                                    IdentityChangedCard(host: link.name, expected: expected, seen: seen) { model.leave(link) }
                                } else {
                                    ConnectionBanner(state: link.connection, detail: link.connectionText,
                                                     host: link.name,
                                                     retry: { model.retryNow(link) }) { model.leave(link) }
                                }
                            }
                            .listRowBackground(Color.clear)
                            .listRowSeparator(.hidden)
                            .listRowInsets(EdgeInsets(top: 4, leading: 16, bottom: 8, trailing: 16))
                        }
                    }
                    switch grouping {
                    case .project: byProject
                    case .status: byStatus
                    }
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
                .toolbar { toolbar }
                .navigationDestination(for: String.self) { tid in
                    ThreadView(threadID: tid)
                }
                .confirmationDialog("Snooze", isPresented: Binding(get: { snoozeChoice != nil }, set: { if !$0 { snoozeChoice = nil } }),
                                    titleVisibility: .visible, presenting: snoozeChoice) { t in
                    ForEach(SnoozePreset.allCases) { preset in
                        Button(preset.label) { model.snooze(t.id, preset) }
                    }
                    Button("Pick a time…") { snoozePicking = t }
                } message: { t in
                    Text(t.title)
                }
                .sheet(item: $snoozePicking) { SnoozePicker(thread: $0) }
                .sheet(isPresented: $showingUsage) { UsageSheet() }
                .task(id: model.connection == .connected || model.mode == .demo) {
                    if model.connection == .connected || model.mode == .demo {
                        for link in model.links { model.loadUsage(quiet: true, on: link) }
                    }
                }
                .deleteConfirmation($deleteAsk, model: model)
                .onChange(of: model.revealRequest) {
                    // The hand on the "New thread" pill: bring the thread that waits into view.
                    guard let id = model.revealRequest else { return }
                    model.revealRequest = nil
                    reveal(id, proxy: proxy)
                }
            }
        }
        .environment(\.openThread, OpenThreadAction { path.append($0) })
    }

    @ToolbarContentBuilder
    private var toolbar: some ToolbarContent {
        // Plan usage, always in reach, as the Usage button in the Mac's sidebar.
        ToolbarItem(placement: .topBarTrailing) {
            UsageButton { showingUsage = true }
        }
        ToolbarSpacer(.fixed, placement: .topBarTrailing)
        ToolbarItem(placement: .topBarTrailing) {
            Menu {
                Section("Group by") {
                    Picker("Group by", selection: $groupingRaw) {
                        ForEach(ThreadGrouping.allCases) { g in
                            Label(g.label, systemImage: g == .project ? "folder" : "circle.grid.2x1").tag(g.rawValue)
                        }
                    }
                }
                if model.links.count > 1 {
                    Section("Machine") {
                        Picker("Machine", selection: $pickedMachine) {
                            Text("All machines").tag(String?.none)
                            ForEach(model.links) { l in
                                Text(l.name).tag(Optional(l.id))
                            }
                        }
                    }
                }
                Section("Project") {
                    Picker("Project", selection: $projectFilter) {
                        Text("All projects").tag(String?.none)
                        ForEach(model.projects) { p in
                            Text(model.links.count > 1 ? "\(p.name) · \(model.hostName(of: p.id) ?? "")" : p.name).tag(Optional(p.id))
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
                Image(systemName: projectFilter == nil && machineFilter == nil ? "ellipsis" : "line.3.horizontal.decrease")
            }
        }
    }

    // MARK: By project

    @ViewBuilder
    private var byProject: some View {
        section("Pinned", filtered(model.pinned))
        let live = filtered(model.liveGroups)
        ForEach(live) { group in
            projectSection(group)
        }
        if live.isEmpty, model.pinned.isEmpty, !model.threads.isEmpty, model.connection == .connected || model.mode == .demo {
            // The Mac's words for an empty inbox.
            ContentUnavailableView("Inbox zero", systemImage: "checkmark.circle",
                                   description: Text("Running agents and anything that needs you will show up here."))
                .listRowBackground(Color.clear)
                .listRowSeparator(.hidden)
        }
        section("Snoozed", filtered(model.snoozed))
        settledSection(filtered(model.settledGroups))
    }

    /// A project's live threads under its badge: what wants you and what's at work always out,
    /// then the first few quiet ones and "Show more".
    @ViewBuilder
    private func projectSection(_ g: ProjectGroup) -> some View {
        let key = "project:\(g.id)"
        let folded = collapsed.contains(key)
        let all = showingAll.contains(g.id)
        let quiet = all ? g.quiet : Array(g.quiet.prefix(Self.quietShown))
        Section {
            if !folded {
                ForEach(g.active + quiet) { t in row(t, showsProject: false) }
                if g.quiet.count > Self.quietShown {
                    moreRow(hidden: g.quiet.count - quiet.count) {
                        if all { showingAll.remove(g.id) } else { showingAll.insert(g.id) }
                    }
                }
            }
        } header: {
            ProjectHeader(group: g, folded: folded) {
                withAnimation(.snappy) { if folded { collapsed.remove(key) } else { collapsed.insert(key) } }
            }
            .id(key)
        }
        .listSectionSeparator(.hidden)
    }

    /// History, folded away: by project, the first few of each.
    @ViewBuilder
    private func settledSection(_ groups: [ProjectGroup]) -> some View {
        let total = groups.reduce(0) { $0 + $1.count }
        if total > 0 {
            Section {
                if settledOpen {
                    ForEach(groups) { g in
                        let key = "settled:\(g.id)"
                        let all = showingAll.contains(key)
                        let shown = all ? g.active + g.quiet : Array((g.active + g.quiet).prefix(Self.settledShown))
                        SettledProjectLabel(group: g)
                            .listRowBackground(Color.clear)
                            .listRowSeparator(.hidden)
                            .listRowInsets(EdgeInsets(top: 10, leading: 20, bottom: 2, trailing: 18))
                        ForEach(shown) { t in row(t, showsProject: false) }
                        if g.count > Self.settledShown {
                            moreRow(hidden: g.count - shown.count) {
                                if all { showingAll.remove(key) } else { showingAll.insert(key) }
                            }
                        }
                    }
                }
            } header: {
                SectionHeader(title: "Settled", count: total, folded: !settledOpen) {
                    withAnimation(.snappy) { settledOpen.toggle() }
                }
            }
            .listSectionSeparator(.hidden)
        }
    }

    private func moreRow(hidden: Int, toggle: @escaping () -> Void) -> some View {
        Button {
            withAnimation(.snappy) { toggle() }
        } label: {
            Text(hidden > 0 ? "Show \(hidden) more" : "Show less")
                .font(.ui(.subheadline).weight(.medium))
                .foregroundStyle(Trek.muted)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.leading, compact ? 28 : 37)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .listRowBackground(Color.clear)
        .listRowSeparator(.hidden)
        .listRowInsets(EdgeInsets(top: 2, leading: 20, bottom: 6, trailing: 18))
    }

    // MARK: By status

    @ViewBuilder
    private var byStatus: some View {
        section("Pinned", filtered(model.pinned))
        section("Needs you", filtered(model.needsYou), tint: Trek.approval)
        section("Working", filtered(model.working), tint: Trek.working)
        section("Snoozed", filtered(model.snoozed))
        section("Recent", filtered(model.recent))
    }

    @ViewBuilder
    private func section(_ title: String, _ threads: [ThreadSummary], tint: Color? = nil) -> some View {
        if !threads.isEmpty {
            Section {
                if !collapsed.contains(title) {
                    ForEach(threads) { t in row(t, showsProject: true) }
                }
            } header: {
                SectionHeader(title: title, count: threads.count, tint: tint, folded: collapsed.contains(title)) {
                    withAnimation(.snappy) {
                        if collapsed.contains(title) { collapsed.remove(title) } else { collapsed.insert(title) }
                    }
                }
            }
            .listSectionSeparator(.hidden)
        }
    }

    // MARK: Rows

    private func filtered(_ list: [ThreadSummary]) -> [ThreadSummary] {
        var list = list
        if let machineFilter { list = list.filter { model.link(for: $0.id)?.id == machineFilter } }
        if let projectFilter { list = list.filter { $0.project?.id == projectFilter } }
        return list
    }

    private func filtered(_ groups: [ProjectGroup]) -> [ProjectGroup] {
        var groups = groups
        if let machineFilter {
            groups = groups.compactMap { g in
                var g = g
                g.active = g.active.filter { model.link(for: $0.id)?.id == machineFilter }
                g.quiet = g.quiet.filter { model.link(for: $0.id)?.id == machineFilter }
                return g.count > 0 ? g : nil
            }
        }
        if let projectFilter { groups = groups.filter { $0.id == projectFilter } }
        return groups
    }

    private func row(_ t: ThreadSummary, showsProject: Bool) -> some View {
        NavigationLink(value: t.id) {
            ThreadRow(thread: t, showsProject: showsProject,
                      host: model.links.count > 1 ? model.hostName(of: t.id) : nil)
        }
        .id(t.id)
        .navigationLinkIndicatorVisibility(.hidden)
        .listRowBackground(Color.clear)
        .listRowSeparator(.hidden)
        .listRowInsets(EdgeInsets(top: compact ? 7 : 9, leading: 20, bottom: compact ? 7 : 9, trailing: 18))
        .swipeActions(edge: .leading) {
            if t.unseen {
                Button("Read", systemImage: "checkmark") { model.markSeen(t.id) }.tint(Trek.done)
            } else if model.serves(.markUnread, on: t.id) {
                Button("Unread", systemImage: "circlebadge.fill") { model.markUnread(t.id) }.tint(Trek.working)
            }
        }
        .swipeActions(edge: .trailing) {
            if model.serves(.snooze, on: t.id) {
                if t.section == .snoozed {
                    Button("Wake", systemImage: "sun.max") { model.wake(t.id) }.tint(Trek.approval)
                } else {
                    Button("Snooze", systemImage: "moon.zzz") { snoozeChoice = t }.tint(.indigo)
                }
            }
        }
        .contextMenu {
            Button("Copy title", systemImage: "doc.on.doc") { UIPasteboard.general.string = t.title }
            if t.runState == .working {
                Button("Stop", systemImage: "stop.fill", role: .destructive) { model.interrupt(t.id) }
            }
            if t.section == .settled {
                Button("Back to the inbox", systemImage: "tray.and.arrow.down") { model.threadAction(t.id, .unsettle) }
            } else if t.section != .pinned {
                Button("Settle", systemImage: "checkmark.circle") { model.threadAction(t.id, .settle, done: "Settled") }
            }
            if model.serves(.snooze, on: t.id) {
                SnoozeMenu(thread: t) { snoozePicking = t }
            }
            if t.unseen {
                Button("Mark read", systemImage: "checkmark.circle") { model.markSeen(t.id) }
            } else if model.serves(.markUnread, on: t.id) {
                Button("Mark unread", systemImage: "circlebadge") { model.markUnread(t.id) }
            }
            if model.serves(.regenerateTitle, on: t.id) {
                Button("Regenerate title", systemImage: "text.badge.star") { model.regenerateTitle(t.id) }
            }
            if model.serves(.delete, on: t.id) {
                Divider()
                Button("Delete…", systemImage: "trash", role: .destructive) { model.askToDelete(t, into: $deleteAsk) }
            }
        }
    }

    /// Unfold whatever hides `id` and scroll to it.
    private func reveal(_ id: String, proxy: ScrollViewProxy) {
        guard let t = model.thread(id) else { return }
        projectFilter = nil
        pickedMachine = nil
        withAnimation(.snappy) {
            collapsed.remove("project:\(t.project?.id ?? "")")
            for title in ["Pinned", "Needs you", "Working", "Snoozed", "Recent"] { collapsed.remove(title) }
            if t.section == .settled { settledOpen = true }
        }
        DispatchQueue.main.async {
            withAnimation(.snappy) { proxy.scrollTo(id, anchor: .center) }
        }
    }
}

/// A section's title, how many it holds, and its fold chevron.
private struct SectionHeader: View {
    var title: String
    var count: Int
    var tint: Color? = nil
    var folded: Bool
    var toggle: () -> Void

    var body: some View {
        Button(action: toggle) {
            HStack(spacing: 7) {
                Text(title).font(.ui(.subheadline).weight(.semibold)).foregroundStyle(Trek.muted)
                Text("\(count)").font(.ui(.subheadline).monospacedDigit()).foregroundStyle(tint ?? Trek.muted.opacity(0.7))
                Spacer()
                FoldChevron(folded: folded)
            }
            .padding(.horizontal, 4)
            .padding(.top, 10)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("\(title), \(count)")
        .accessibilityHint(folded ? "Shows them" : "Folds them away")
    }
}

/// A project's header, as the Mac's sidebar draws it: its badge and name, then what's in it (what
/// waits and what works only while folded: unfolded, the rows say so), and the fold chevron.
private struct ProjectHeader: View {
    var group: ProjectGroup
    var folded: Bool
    var toggle: () -> Void

    var body: some View {
        Button(action: toggle) {
            HStack(spacing: 8) {
                if let p = group.project {
                    ProjectBadge(project: p, size: 18)
                } else {
                    Image(systemName: "bubble.left")
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundStyle(Trek.muted)
                        .frame(width: 22, height: 18)
                        .background(Trek.foreground.opacity(0.08), in: RoundedRectangle(cornerRadius: 4.5))
                }
                Text(group.name)
                    .font(.ui(.subheadline).weight(.semibold))
                    .foregroundStyle(Trek.foreground.opacity(0.88))
                    .lineLimit(1)
                if folded, group.needs > 0 {
                    Label("\(group.needs)", systemImage: "hand.raised.fill")
                        .labelStyle(TightLabelStyle())
                        .foregroundStyle(Trek.approval)
                }
                if folded, group.working > 0 {
                    HStack(spacing: 4) {
                        BeaconDot(color: Trek.working, pulses: true, size: 6)
                        Text("\(group.working)")
                    }
                    .foregroundStyle(Trek.working)
                }
                if group.unread > 0 {
                    Text("\(group.unread) new").foregroundStyle(Trek.done)
                }
                Spacer(minLength: 4)
                Text("\(group.count)").foregroundStyle(Trek.muted.opacity(0.7))
                FoldChevron(folded: folded)
            }
            .font(.ui(.footnote).weight(.medium).monospacedDigit())
            .padding(.horizontal, 4)
            .padding(.top, 14)
            .padding(.bottom, 2)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityElement(children: .combine)
        .accessibilityHint(folded ? "Shows its threads" : "Folds its threads away")
    }
}

/// Inside Settled: the project the threads under it are in.
private struct SettledProjectLabel: View {
    var group: ProjectGroup

    var body: some View {
        HStack(spacing: 7) {
            if let p = group.project { ProjectBadge(project: p, size: 15) }
            Text(group.name).font(.ui(.footnote).weight(.semibold)).foregroundStyle(Trek.muted)
            Text("\(group.count)").font(.ui(.footnote).monospacedDigit()).foregroundStyle(Trek.muted.opacity(0.7))
        }
        .accessibilityElement(children: .combine)
    }
}

private struct FoldChevron: View {
    var folded: Bool

    var body: some View {
        Image(systemName: "chevron.down")
            .font(.ui(.caption).weight(.semibold))
            .foregroundStyle(Trek.muted.opacity(0.8))
            .rotationEffect(.degrees(folded ? -90 : 0))
    }
}

private struct TightLabelStyle: LabelStyle {
    func makeBody(configuration: Configuration) -> some View {
        HStack(spacing: 3) {
            configuration.icon.font(.system(size: 10, weight: .semibold))
            configuration.title
        }
    }
}

struct ThreadRow: View {
    var thread: ThreadSummary
    /// Its project's badge and name: left out under the project's own header.
    var showsProject = true
    /// Which machine it's on, when several are paired.
    var host: String? = nil
    @Environment(\.compactRows) private var compact

    private var recedes: Bool { thread.runState == .idle && !thread.unseen && thread.needs == nil && thread.runningSubAgents.isEmpty }

    var body: some View {
        if compact { compactBody } else { fullBody }
    }

    /// One line: logo, project badge, title, sub-agents, status.
    private var compactBody: some View {
        HStack(alignment: .center, spacing: 10) {
            AgentGlyph(thread.agent, size: 18)
                .opacity(recedes ? 0.7 : 1)
            if showsProject, let p = thread.project { ProjectBadge(project: p, size: 15) }
            if let host { Text(host).font(.ui(.caption)).foregroundStyle(Trek.muted).lineLimit(1) }
            Text(thread.title)
                .scaledFont(16, weight: thread.unseen ? .semibold : .regular)
                .foregroundStyle(recedes ? Trek.foreground.opacity(0.72) : Trek.foreground)
                .lineLimit(1)
            Spacer(minLength: 4)
            SubAgentsPill(agents: thread.runningSubAgents)
            trailing
        }
        .accessibilityElement(children: .combine)
    }

    private var fullBody: some View {
        HStack(alignment: .top, spacing: 13) {
            AgentGlyph(thread.agent, size: 24)
                .padding(.top, 1)
                .opacity(recedes ? 0.7 : 1)
            VStack(alignment: .leading, spacing: 5) {
                HStack(alignment: .center, spacing: 8) {
                    Text(thread.title)
                        .scaledFont(17, weight: thread.unseen ? .semibold : .regular)
                        .foregroundStyle(recedes ? Trek.foreground.opacity(0.72) : Trek.foreground)
                        .lineLimit(1)
                    Spacer(minLength: 4)
                    trailing
                }
                HStack(spacing: 6) {
                    if showsProject, let p = thread.project {
                        ProjectBadge(project: p, size: 16)
                        // At work, the model and what it's doing need the room: the badge says
                        // which project.
                        if thread.runState != .working {
                            Text(p.name).foregroundStyle(Trek.muted).lineLimit(1).layoutPriority(-1)
                        }
                    }
                    secondary
                        .lineLimit(1)
                        .transaction { $0.animation = nil }
                    Spacer(minLength: 0)
                    if !thread.runningSubAgents.isEmpty {
                        SubAgentsPill(agents: thread.runningSubAgents)
                    } else if thread.additions + thread.deletions > 0, thread.needs == nil {
                        DiffStat(additions: thread.additions, deletions: thread.deletions, font: .ui(.caption).monospacedDigit())
                            .opacity(recedes ? 0.6 : 0.9)
                    }
                }
                .font(.ui(.subheadline))
            }
        }
        .accessibilityElement(children: .combine)
    }

    @ViewBuilder
    private var trailing: some View {
        if thread.runState == .working, thread.needs == nil {
            // Working, and for how long, as the Mac's sidebar card says it.
            WorkingPill(since: thread.workingSince)
        } else if let look = StatusLook.of(thread) {
            StatusPill(look: look)
        } else if !thread.runningSubAgents.isEmpty {
            // Its turn is over, but sub-agents it started are still at it.
            StatusPill(look: StatusLook(label: "Waiting", color: Trek.working.opacity(0.8), symbol: nil, pulses: true))
        } else if thread.unseen {
            // Done and not read yet: an emerald dot by the time, as in the Mac's sidebar.
            HStack(spacing: 6) {
                Circle().fill(Trek.done).frame(width: 7, height: 7)
                Text(When.short(thread.updatedAt)).font(.ui(.footnote).weight(.medium).monospacedDigit())
                    .foregroundStyle(Trek.muted)
            }
            .fixedSize()
            .accessibilityLabel("Done, unread, \(When.short(thread.updatedAt))")
        } else {
            Text(When.short(thread.updatedAt))
                .font(.ui(.subheadline).monospacedDigit())
                .foregroundStyle(Trek.muted.opacity(0.8))
        }
    }

    /// The model at work, then the request waiting on you, what the agent is doing, or the branch.
    @ViewBuilder
    private var secondary: some View {
        HStack(spacing: 5) {
            if let host {
                Label(host, systemImage: "laptopcomputer")
                    .labelStyle(BranchLabelStyle())
                    .foregroundStyle(Trek.muted.opacity(0.9))
                    .fixedSize()
                Text("·").foregroundStyle(Trek.muted.opacity(0.6))
            }
            if !showsProject || thread.runState == .working {
                Text(thread.modelLabel ?? thread.agent.name)
                    .foregroundStyle(thread.runState == .working ? Trek.foreground.opacity(0.8) : Trek.muted)
                    .fixedSize()
                if hasDetail { Text("·").foregroundStyle(Trek.muted.opacity(0.6)) }
            }
            detail
        }
    }

    private var hasDetail: Bool {
        thread.needs != nil || (thread.section == .snoozed && thread.snoozedUntil != nil)
            || (thread.runState == .working && thread.activity != nil) || thread.branch != nil
    }

    @ViewBuilder
    private var detail: some View {
        if let needs = thread.needs {
            Text(needs.text.replacingOccurrences(of: "`", with: "")).foregroundStyle(Trek.muted)
        } else if thread.section == .snoozed, let until = SnoozeWords.until(thread.snoozedUntil) {
            Label(until, systemImage: "moon.zzz").labelStyle(BranchLabelStyle()).foregroundStyle(Trek.muted.opacity(0.85))
        } else if thread.runState == .working, let activity = thread.activity {
            RichText(markdown: activity, size: 15, style: .subheadline, autoPaths: true)
                .foregroundStyle(Trek.muted)
                .environment(\.projectHue, thread.project?.hue)
        } else if let branch = thread.branch {
            Label {
                Text(branch)
            } icon: {
                Image(systemName: "arrow.triangle.branch")
                    .font(.ui(.caption2))
            }
            .labelStyle(BranchLabelStyle())
            .foregroundStyle(Trek.muted.opacity(0.85))
        }
    }
}

/// "Working 3m 05s": the beacon and how long the turn has run, ticking.
struct WorkingPill: View {
    var since: Int64?

    var body: some View {
        HStack(spacing: 6) {
            BeaconDot(color: Trek.working, pulses: true, size: 7)
            if let since {
                TimelineView(.periodic(from: .now, by: 1)) { ctx in
                    Text("Working \(When.duration(max(0, Int(ctx.date.timeIntervalSince1970 - Double(since) / 1000))))")
                        .monospacedDigit()
                }
            } else {
                Text("Working")
            }
        }
        .font(.ui(.footnote).weight(.medium))
        .foregroundStyle(Trek.working)
        .fixedSize()
        .accessibilityElement(children: .combine)
    }
}

/// The sub-agents at work for a thread, as the Mac's sidebar pill shows them: their agents' logos,
/// overlapping (three at most), and how many there are.
struct SubAgentsPill: View {
    var agents: [SubAgent]
    var size: CGFloat = 15

    var body: some View {
        if !agents.isEmpty {
            let logos = AtWorkLabel.byAgent(agents).prefix(3)
            HStack(spacing: 4) {
                HStack(spacing: -size * 0.3) {
                    ForEach(Array(logos.enumerated()), id: \.offset) { _, entry in
                        AgentGlyph(entry.agent, size: size)
                            .padding(1)
                            .background(Trek.background, in: RoundedRectangle(cornerRadius: size * 0.3))
                    }
                }
                Text("\(agents.count)")
                    .font(.ui(.caption).weight(.semibold).monospacedDigit())
                    .foregroundStyle(Trek.foreground.opacity(0.85))
            }
            .padding(.leading, 3)
            .padding(.trailing, 7)
            .padding(.vertical, 2)
            .background(Trek.working.opacity(0.12), in: Capsule())
            .overlay(Capsule().strokeBorder(Trek.working.opacity(0.28), lineWidth: 0.5))
            .fixedSize()
            .accessibilityElement(children: .ignore)
            .accessibilityLabel("\(agents.count) sub-agent\(agents.count == 1 ? "" : "s") at work")
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

/// What the connection is doing, above the list ("Looking for … on this Wi-Fi…", "This Wi-Fi
/// may not let devices see each other. Trying Tailscale…"), with the cached threads still shown
/// below it. While it waits to retry, "Try now" (or a tap on it) looks for the Mac at once.
struct ConnectionBanner: View {
    var state: ConnectionState
    /// The words; the state's own when nil.
    var detail: String? = nil
    var host: String?
    var retry: (() -> Void)? = nil
    /// Offered when the Mac refused this iPhone (revoked, or a code that didn't work).
    var pairAgain: (() -> Void)? = nil

    private var waiting: Bool { if case .offline = state { true } else { false } }

    var body: some View {
        HStack(spacing: 10) {
            if state == .connecting {
                ProgressView().controlSize(.small)
            } else {
                Image(systemName: "wifi.exclamationmark").foregroundStyle(Trek.approval)
            }
            VStack(alignment: .leading, spacing: 1) {
                Text(host ?? "Your Mac").font(.ui(.subheadline).weight(.semibold))
                Text(detail ?? state.label).font(.ui(.caption)).foregroundStyle(Trek.muted)
                    .fixedSize(horizontal: false, vertical: true)
                    .contentTransition(.opacity)
                    .animation(.smooth, value: detail ?? state.label)
            }
            Spacer()
            if case .unauthorized = state, let pairAgain {
                Button("Pair again", action: pairAgain)
                    .font(.ui(.subheadline).weight(.semibold))
                    .buttonStyle(.glass)
            } else if waiting, let retry {
                Button("Try now", action: retry)
                    .font(.ui(.subheadline).weight(.semibold))
                    .buttonStyle(.glass)
            }
        }
        .padding(12)
        .contentShape(Rectangle())
        .onTapGesture { if waiting { retry?() } }
        .glassEffect(.regular, in: RoundedRectangle(cornerRadius: 16, style: .continuous))
        .accessibilityElement(children: .combine)
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
                Text("This Mac's identity changed").font(.ui(.headline))
            } icon: {
                Image(systemName: "exclamationmark.shield.fill").foregroundStyle(Trek.failed)
            }
            Text("\(host ?? "Your Mac") answered with a different certificate than the one this iPhone paired with. Trek may have been reinstalled on it, or something else on the network is posing as it. Trek won't connect until you pair again.")
                .font(.ui(.subheadline))
                .foregroundStyle(Trek.foreground.opacity(0.85))
                .fixedSize(horizontal: false, vertical: true)
            HStack(spacing: 14) {
                LabeledContent("Paired") { Text(expected).font(.code(.footnote)) }
                if let seen { LabeledContent("Now") { Text(seen).font(.code(.footnote)).foregroundStyle(Trek.failed) } }
            }
            .labeledContentStyle(FingerprintLabelStyle())
            Button(action: pairAgain) {
                Text("Pair again").fontWeight(.semibold).foregroundStyle(Trek.background).frame(maxWidth: .infinity)
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
        .font(.ui(.footnote))
    }
}
