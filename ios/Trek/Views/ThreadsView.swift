import SwiftUI

/// Home: every thread on the paired machines, laid out as the Mac's sidebar is. What waits on you
/// first, in the one tinted block; then Pinned; then what's at work, across projects (Active);
/// then each project with its quiet threads, one line each; Snoozed; and Settled folded away. Or
/// by status (Pinned · Working · In the background · Snoozed · Recent), under the same block.
/// Every row starts with a state column that says how its thread stands; colour only where
/// something wants action, is moving or broke, and the rest recedes.
struct ThreadsView: View {
    @Environment(AppModel.self) private var model
    @Binding var path: [String]
    @Binding var showNew: Bool
    @AppStorage(ThreadGrouping.key) private var groupingRaw = ThreadGrouping.project.rawValue
    @State private var collapsed: Set<String> = Launch.folded
    /// Groups showing all their threads, not just the first few: a project's id, "settled:" and
    /// one, or the Needs you block's key.
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
    /// The status bar's height: the navigation bar's row starts below it.
    @State private var statusBar: CGFloat = 0
    /// Where the list's head (title, chips, cards) is: the bar turns solid once it goes under.
    @State private var head = ListHead()
    @Environment(\.compactRows) private var compact

    /// Quiet threads a project shows before "Show more" (the Mac's sidebar: 3, and 5 settled).
    private static let quietShown = 3
    private static let settledShown = 5
    /// Threads the Needs you block shows before "Show more": the rest of the list stays in reach
    /// when a dozen wait.
    private static let needsShown = 4
    private static let needsKey = "needs"

    private var grouping: ThreadGrouping { ThreadGrouping(rawValue: Launch.grouping ?? groupingRaw) ?? .project }

    var body: some View {
        NavigationStack(path: $path.onSet { if Perf.enabled { Perf.navigated = Perf.now } }) {
            ScrollViewReader { proxy in
                List {
                    ThreadsHeader(picked: $pickedMachine, update: model.offeredUpdate,
                                  unreachable: model.mode == .live || Launch.demoOffline != nil || Launch.demoConnection != nil
                                      ? model.links.filter { $0.connection != .connected } : [])
                        .reportsListHead($head)
                        .listRowBackground(Color.clear)
                        .listRowSeparator(.hidden)
                        .listRowInsets(EdgeInsets())
                    needsBlock
                    switch grouping {
                    case .project: byProject
                    case .status: byStatus
                    }
                    if model.threads.isEmpty, model.connection == .connected {
                        ContentUnavailableView("No threads yet", systemImage: "bubble.left.and.text.bubble.right",
                                               description: Text("Start one here or on your Mac."))
                            .listRowBackground(Color.clear)
                            .listRowSeparator(.hidden)
                    }
                }
                .listStyle(.plain)
                .listSectionSpacing(6)
                .environment(\.defaultMinListHeaderHeight, 0)
                // A quiet thread is one 44 pt line (the HIG's least), not the taller rows the
                // list gives by default.
                .environment(\.defaultMinListRowHeight, 44)
                .scrollContentBackground(.hidden)
                .background(RidgeBackdrop(height: 300))
                // Rows fade out above the "New thread" pill and the tab bar, and the last row clears them.
                .fadesUnderBottomBars()
                // The bar turns solid as soon as the head goes under it, and says "Threads" once
                // the list's own title has gone (drawn from where the head is: see `TopBarFade`).
                .fadesUnderTopBar(title: "Threads", statusBar: statusBar, head: head, ground: RidgeBackdrop(height: 300))
                .navigationTitle("Threads")
                .navigationBarTitleDisplayMode(.inline)
                .toolbar(removing: .title)
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
        // The status bar's foot in the window: the stack's top edge (it keeps out of the status bar).
        .onGeometryChange(for: CGFloat.self) { $0.frame(in: .global).minY } action: { statusBar = $0 }
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

    // MARK: Needs you

    /// What waits on you, first and the one tinted block on the page (the Mac's sidebar has it so):
    /// the first few, then "Show more". The tab's badge and the hand on the "New thread" pill count
    /// these. A pinned thread that asks stays under Pinned, as on the Mac.
    @ViewBuilder
    private var needsBlock: some View {
        let needs = filtered(model.needsYou)
        if !needs.isEmpty {
            let all = showingAll.contains(Self.needsKey)
            let shown = all ? needs : Array(needs.prefix(Self.needsShown))
            let more = needs.count > Self.needsShown
            Section {
                NeedsHeader(count: needs.count)
                    .listRowBackground(NeedsWash(part: .top))
                    .listRowSeparator(.hidden)
                    .listRowInsets(EdgeInsets(top: 0, leading: 20, bottom: 0, trailing: 18))
                ForEach(shown) { t in
                    row(t, showsProject: true, wash: !more && t.id == shown.last?.id ? .bottom : .middle)
                }
                if more {
                    moreRow(hidden: needs.count - shown.count, wash: .bottom) {
                        if all { showingAll.remove(Self.needsKey) } else { showingAll.insert(Self.needsKey) }
                    }
                }
            }
            .listSectionSeparator(.hidden)
        }
    }

    // MARK: By project

    @ViewBuilder
    private var byProject: some View {
        let list = filtered(model.threads)
        let pinned = filtered(model.pinned)
        let active = model.active(of: list)
        let groups = model.quietGroups(of: list)
        section("Pinned", pinned)
        section("Active", active)
        if !groups.isEmpty {
            // What the groups under it are; each project's header then stays put above its rows.
            SectionLabel(title: "Projects", count: groups.count)
                .listRowBackground(Color.clear)
                .listRowSeparator(.hidden)
                .listRowInsets(EdgeInsets(top: 0, leading: 20, bottom: 0, trailing: 18))
            ForEach(groups) { group in
                projectSection(group)
            }
        }
        if groups.isEmpty, active.isEmpty, pinned.isEmpty, filtered(model.needsYou).isEmpty, !model.threads.isEmpty,
           model.connection == .connected || model.mode == .demo {
            // The Mac's words for an empty inbox.
            ContentUnavailableView("Inbox zero", systemImage: "checkmark.circle",
                                   description: Text("Running agents and anything that needs you will show up here."))
                .listRowBackground(Color.clear)
                .listRowSeparator(.hidden)
        }
        section("Snoozed", filtered(model.snoozed))
        settledSection(model.settledGroups(of: list))
    }

    /// A project's quiet threads under its badge: the first few, then "Show more". What waits on
    /// you and what's at work are above, in Needs you and Active.
    @ViewBuilder
    private func projectSection(_ g: ProjectGroup) -> some View {
        let key = "project:\(g.id)"
        let folded = collapsed.contains(key)
        let all = showingAll.contains(g.id)
        let rows = g.active + g.quiet
        let shown = all ? rows : Array(rows.prefix(Self.quietShown))
        Section {
            if !folded {
                ForEach(shown) { t in row(t, showsProject: false) }
                if rows.count > Self.quietShown {
                    moreRow(hidden: rows.count - shown.count) {
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

    private func moreRow(hidden: Int, wash: NeedsWash.Part? = nil, toggle: @escaping () -> Void) -> some View {
        Button {
            withAnimation(.snappy) { toggle() }
        } label: {
            Text(hidden > 0 ? "Show \(hidden) more" : "Show less")
                .font(.ui(.subheadline).weight(.medium))
                .foregroundStyle(Trek.muted)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.leading, ThreadRow.titleInset)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .listRowBackground(RowGround(wash: wash))
        .listRowSeparator(.hidden)
        .listRowInsets(EdgeInsets(top: 2, leading: 20, bottom: wash == nil ? 6 : 10, trailing: 18))
    }

    // MARK: By status

    @ViewBuilder
    private var byStatus: some View {
        section("Pinned", filtered(model.pinned))
        section("Working", filtered(model.working))
        section("In the background", filtered(model.inBackground))
        section("Snoozed", filtered(model.snoozed))
        section("Recent", filtered(model.recent))
    }

    @ViewBuilder
    private func section(_ title: String, _ threads: [ThreadSummary]) -> some View {
        if !threads.isEmpty {
            Section {
                if !collapsed.contains(title) {
                    ForEach(threads) { t in row(t, showsProject: true) }
                }
            } header: {
                SectionHeader(title: title, count: threads.count, folded: collapsed.contains(title)) {
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

    /// The machine a row says it's on: with two or more paired and the list showing them all.
    /// Picked to one, every row is on it and none says so.
    private func host(of t: ThreadSummary) -> String? {
        model.links.count > 1 && machineFilter == nil ? model.hostName(of: t.id) : nil
    }

    private func row(_ t: ThreadSummary, showsProject: Bool, wash: NeedsWash.Part? = nil) -> some View {
        // A quiet thread is one 44 pt line; one with something to say is two.
        let quiet = compact || !RowState.of(t).says
        return NavigationLink(value: t.id) {
            ThreadRow(thread: t, showsProject: showsProject, host: host(of: t), hostKind: model.machineKind(of: t.id))
        }
        .id(t.id)
        .navigationLinkIndicatorVisibility(.hidden)
        .listRowBackground(RowGround(wash: wash))
        .listRowSeparator(.hidden)
        .listRowInsets(EdgeInsets(top: quiet ? (compact ? 7 : 10) : 8, leading: 20, bottom: quiet ? (compact ? 7 : 10) : 8, trailing: 18))
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
            collapsed.remove("project:\(model.groupKey(of: t))")
            for title in ["Pinned", "Active", "Working", "In the background", "Snoozed", "Recent"] { collapsed.remove(title) }
            if model.needsYou.contains(where: { $0.id == id }) { showingAll.insert(Self.needsKey) }
            if t.section == .settled { settledOpen = true }
        }
        DispatchQueue.main.async {
            withAnimation(.snappy) { proxy.scrollTo(id, anchor: .center) }
        }
    }
}

/// The ground behind a row: the list's own, or a slice of the Needs you block's wash.
private struct RowGround: View {
    var wash: NeedsWash.Part?

    var body: some View {
        if let wash { NeedsWash(part: wash) } else { Color.clear }
    }
}

/// The Needs you block's amber wash, drawn row by row (a list draws each row's ground on its
/// own): rounded at the block's top and foot, inset from the screen's edges so it reads as one
/// block. 8 %, as the Mac's sidebar has it; the only fill on the page.
struct NeedsWash: View {
    enum Part { case top, middle, bottom }
    var part: Part
    static let radius: CGFloat = 18

    var body: some View {
        UnevenRoundedRectangle(topLeadingRadius: part == .top ? Self.radius : 0,
                               bottomLeadingRadius: part == .bottom ? Self.radius : 0,
                               bottomTrailingRadius: part == .bottom ? Self.radius : 0,
                               topTrailingRadius: part == .top ? Self.radius : 0,
                               style: .continuous)
            .fill(Trek.approval.opacity(0.08))
            .padding(.horizontal, 12)
            .padding(.top, part == .top ? 6 : 0)
            .accessibilityHidden(true)
    }
}

/// The head of the Needs you block: amber words and how many.
private struct NeedsHeader: View {
    var count: Int

    var body: some View {
        HStack(spacing: 8) {
            Text("Needs you").font(.ui(.subheadline).weight(.semibold))
            Text("\(count)")
                .font(.ui(.footnote).weight(.semibold).monospacedDigit())
                // One more or one fewer rolls to it, as a row comes in or goes.
                .contentTransition(.numericText(value: Double(count)))
                .padding(.horizontal, 7)
                .padding(.vertical, 1)
                .background(Trek.approval.opacity(0.16), in: Capsule())
            Spacer()
        }
        .foregroundStyle(Trek.approval)
        .padding(.top, 16)
        .padding(.bottom, 2)
        .accessibilityElement(children: .combine)
        .accessibilityLabel("Needs you, \(count)")
        .accessibilityAddTraits(.isHeader)
    }
}

/// What the groups under it are, as a section's title is drawn, without a fold: "Projects 4".
private struct SectionLabel: View {
    var title: String
    var count: Int

    var body: some View {
        HStack(spacing: 7) {
            Text(title).font(.ui(.subheadline).weight(.semibold)).foregroundStyle(Trek.muted)
            Text("\(count)").font(.ui(.subheadline).monospacedDigit()).foregroundStyle(Trek.muted.opacity(0.7))
            Spacer()
        }
        .padding(.top, 14)
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(.isHeader)
    }
}

/// A section's title, how many it holds, and its fold chevron.
private struct SectionHeader: View {
    var title: String
    var count: Int
    var folded: Bool
    var toggle: () -> Void

    var body: some View {
        Button(action: toggle) {
            HStack(spacing: 7) {
                Text(title).font(.ui(.subheadline).weight(.semibold)).foregroundStyle(Trek.muted)
                Text("\(count)").font(.ui(.subheadline).monospacedDigit()).foregroundStyle(Trek.muted.opacity(0.7))
                    .contentTransition(.numericText(value: Double(count)))
                Spacer()
                FoldChevron(folded: folded)
            }
            .padding(.horizontal, 4)
            .padding(.top, 10)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .modifier(HeaderGround())
        .accessibilityLabel("\(title), \(count)")
        .accessibilityHint(folded ? "Shows them" : "Folds them away")
    }
}

/// A project's header, as the Mac's sidebar draws it: its badge and name, what's new in it, how
/// many, and the fold chevron. One for a name, whichever machines it's on (`groupKey`).
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
        .modifier(HeaderGround())
        .accessibilityElement(children: .combine)
        .accessibilityHint(folded ? "Shows its threads" : "Folds its threads away")
    }
}

/// A section header held at the top of the list stands on the list's own ground, edge to edge,
/// so the rows passing under it don't show through its words. The ground reaches up over the
/// room the list leaves above a header, so one held at the top meets the bar's solid ground
/// with no strip between where a row scrolling away still showed, faded (`TopBarFade`).
private struct HeaderGround: ViewModifier {
    func body(content: Content) -> some View {
        content.background {
            Trek.background
                .padding(.horizontal, -24)
                .padding(.top, -12)
                .padding(.bottom, -4)
                .allowsHitTesting(false)
        }
    }
}

/// Inside Settled: the project the threads under it are in.
private struct SettledProjectLabel: View {
    var group: ProjectGroup

    var body: some View {
        HStack(spacing: 7) {
            if let p = group.project, !p.isNone { ProjectBadge(project: p, size: 15) }
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

/// How a row's thread stands, said once: the mark in its state column and, for a thread with
/// something to say, the words its second line opens with, in the state's colour (the Mac's
/// sidebar's one state column). A quiet thread has no words and is one line.
struct RowState {
    enum Mark: Equatable {
        /// At work: the ember beacon, breathing.
        case beacon
        /// Asked, failed, interrupted, background work, finished and unread.
        case dot
        case pin
        case snoozed
        case none
    }

    var mark: Mark
    var color: Color
    /// "Approval", "Working", "Running"…; nil for a quiet thread.
    var words: String?
    /// What VoiceOver says for a mark with no words beside it ("Unread", "Pinned").
    var spoken: String?

    /// Whether it has something to say: two lines.
    var says: Bool { words != nil }

    static func of(_ t: ThreadSummary) -> RowState {
        if t.runState == .working, t.needs == nil {
            return RowState(mark: .beacon, color: Trek.working, words: "Working")
        }
        // Asked (approval, question, plan, paused at a limit), failed, interrupted.
        if let look = StatusLook.of(t) {
            return RowState(mark: .dot, color: look.color, words: look.label)
        }
        // Its turn is over, but it waits on what it started (the Mac's "Waiting").
        if t.waitsInBackground {
            return RowState(mark: .beacon, color: Trek.working, words: "Waiting")
        }
        if t.runsInBackground {
            return RowState(mark: .dot, color: Trek.sky, words: "Running")
        }
        if t.section == .snoozed { return RowState(mark: .snoozed, color: Trek.muted, spoken: "Snoozed") }
        if t.unseen { return RowState(mark: .dot, color: Trek.done, spoken: "Unread") }
        if t.section == .pinned { return RowState(mark: .pin, color: Trek.muted, spoken: "Pinned") }
        return RowState(mark: .none, color: Trek.muted)
    }
}

/// A row's state column (20 pt): where every row starts, so how each thread stands reads down one
/// edge of the list. Empty for a quiet thread.
struct StateColumn: View {
    var state: RowState
    static let width: CGFloat = 20

    var body: some View {
        Group {
            switch state.mark {
            case .beacon: BeaconDot(color: state.color, pulses: true, size: 8)
            case .dot: Circle().fill(state.color).frame(width: 8, height: 8)
            case .pin:
                Image(systemName: "pin.fill").font(.system(size: 10, weight: .semibold)).foregroundStyle(Trek.muted.opacity(0.8))
            case .snoozed:
                Image(systemName: "moon.zzz").font(.system(size: 11, weight: .medium)).foregroundStyle(Trek.muted)
            case .none: Color.clear.frame(width: 8, height: 8)
            }
        }
        .frame(width: Self.width)
        .accessibilityHidden(state.spoken == nil)
        .accessibilityLabel(state.spoken ?? "")
    }
}

/// A thread's row. Its state column, then: a quiet thread is one line, its title and how long
/// ago (with two machines, which one, by its mark); a thread with something to say is two, its
/// title with its agent's logo at the end, then where it stands in its state's colour, why, its
/// project and its machine, muted, the reason giving way first.
struct ThreadRow: View {
    var thread: ThreadSummary
    /// Its project's name on the second line (its badge on a quiet line): left out under the
    /// project's own header.
    var showsProject = true
    /// Which machine it's on, when several are paired and the list shows them all, and what that
    /// machine is.
    var host: String? = nil
    var hostKind: MachineKind = .unknown
    @Environment(\.compactRows) private var compact
    @Environment(\.dynamicTypeSize) private var typeSize

    /// Where a row's title starts, from its leading edge: past the state column.
    static let titleInset: CGFloat = StateColumn.width + 10

    private var state: RowState { RowState.of(thread) }

    /// Read, idle, nothing to say: its title steps back.
    private var recedes: Bool { !state.says && !thread.unseen }

    var body: some View {
        if typeSize.isAccessibilitySize { accessibleBody } else if compact { compactBody } else if state.says { twoLines } else { oneLine }
    }

    private var title: some View {
        Text(thread.title)
            .scaledFont(17, weight: thread.unseen ? .semibold : .regular)
            .foregroundStyle(recedes ? Trek.foreground.opacity(0.72) : Trek.foreground)
    }

    /// Something to say: the title (and its agent), then the state's words, why, and where.
    private var twoLines: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(alignment: .center, spacing: 10) {
                StateColumn(state: state)
                title.lineLimit(1)
                Spacer(minLength: 4)
                SubAgentsPill(agents: thread.runningSubAgents)
                AgentGlyph(thread.agent, size: 18)
            }
            standing
                .font(.ui(.subheadline))
                .lineLimit(1)
                .padding(.leading, Self.titleInset)
                .transaction { $0.animation = nil }
        }
        .accessibilityElement(children: .combine)
    }

    /// Quiet: the title, then how long ago (and, with two machines, which).
    private var oneLine: some View {
        HStack(alignment: .center, spacing: 10) {
            StateColumn(state: state)
            title.lineLimit(1)
            Spacer(minLength: 4)
            HStack(spacing: 6) {
                if showsProject, let p = thread.project, !p.isNone {
                    ProjectBadge(project: p, size: 15)
                }
                if let host { MachineMark(kind: hostKind, size: 10).accessibilityLabel("On \(host)") }
                Text(when)
                    .font(.ui(.subheadline).monospacedDigit())
                    .foregroundStyle(Trek.muted.opacity(0.8))
            }
            .fixedSize()
        }
        .accessibilityElement(children: .combine)
    }

    /// The setting for one line a row: the state column, the title, the state's words or the time.
    private var compactBody: some View {
        HStack(alignment: .center, spacing: 10) {
            StateColumn(state: state)
            if showsProject, let p = thread.project, !p.isNone { ProjectBadge(project: p, size: 15) }
            title.lineLimit(1)
            Spacer(minLength: 4)
            SubAgentsPill(agents: thread.runningSubAgents)
            if let words = state.words {
                Group {
                    if state.mark == .beacon, thread.runState == .working, let since = thread.workingSince {
                        Ticking(since: since) { "\(words) \($0)" }
                    } else {
                        Text(words)
                    }
                }
                .font(.ui(.footnote).weight(.medium))
                .foregroundStyle(state.color)
                .fixedSize()
            } else {
                if let host { MachineMark(kind: hostKind, size: 10).accessibilityLabel("On \(host)") }
                Text(when).font(.ui(.subheadline).monospacedDigit()).foregroundStyle(Trek.muted.opacity(0.8)).fixedSize()
            }
        }
        .accessibilityElement(children: .combine)
    }

    /// At the accessibility sizes, a line for each: beside the status, the title was squeezed down
    /// to nothing.
    private var accessibleBody: some View {
        HStack(alignment: .top, spacing: 10) {
            StateColumn(state: state)
                .padding(.top, 8)
            VStack(alignment: .leading, spacing: 4) {
                title.lineLimit(3)
                WordFlow(spacing: 8, lineSpacing: 4) {
                    if state.says { standingWords.foregroundStyle(state.color) }
                    SubAgentsPill(agents: thread.runningSubAgents)
                    if !state.says { Text(when).foregroundStyle(Trek.muted) }
                }
                .font(.ui(.subheadline))
                if let reason {
                    Text(reason).font(.ui(.subheadline)).foregroundStyle(Trek.muted).lineLimit(2)
                        .transaction { $0.animation = nil }
                }
                if showsProject, let p = thread.project {
                    HStack(spacing: 6) {
                        if !p.isNone { ProjectBadge(project: p, size: 16) }
                        Text(p.name).font(.ui(.subheadline)).foregroundStyle(Trek.muted).lineLimit(1)
                    }
                }
                if let host { MachineTag(name: host, kind: hostKind, compact: true) }
            }
        }
        .accessibilityElement(children: .combine)
    }

    /// The second line: the state's words (and its clock), why, then the project and machine.
    /// The reason gives way first; the project and machine stay whole as long as they can.
    private var standing: some View {
        StandingLayout {
            standingWords
                .fontWeight(.medium)
                .foregroundStyle(state.color)
            Group {
                if let reason {
                    reasonText(reason).foregroundStyle(Trek.muted)
                } else {
                    Color.clear.frame(width: 0, height: 0)
                }
            }
            .clipped()
            Text(context.map { " · " + $0 } ?? "")
                .foregroundStyle(Trek.muted)
        }
    }

    /// Why, after a dot: a command still running in the background in code's face.
    private func reasonText(_ reason: String) -> Text {
        state.words == "Running" ? Text(" · ") + Text(reason).font(.code(.footnote)) : Text(" · " + reason)
    }

    /// "Working 3m 05s" and "Running 1h 2m" (ticking), "Approval", "Waiting"…
    @ViewBuilder
    private var standingWords: some View {
        let words = state.words ?? ""
        if state.mark == .beacon, thread.runState == .working, let since = thread.workingSince {
            Ticking(since: since) { "\(words) \($0)" }
        } else if words == "Running", let since = thread.backgroundWork.compactMap(\.since).min() {
            Ticking(since: since) { "\(words) \($0)" }
        } else {
            Text(words)
        }
    }

    /// Why it stands so: what it asks or why it failed, what it's doing or runs, when it wakes.
    private var reason: String? {
        if let needs = thread.needs { return needs.text.replacingOccurrences(of: "`", with: "") }
        if state.words == "Running", let first = thread.backgroundWork.first {
            // What it runs: the first command, and how many more.
            let more = thread.backgroundWork.count - 1
            return BackgroundLine.oneLine(first.title) + (more > 0 ? " +\(more)" : "")
        }
        if thread.cutShort != nil, thread.runState != .working { return "Cut short when Trek closed" }
        if thread.runState == .working, let activity = thread.activity, !activity.isEmpty {
            return activity.replacingOccurrences(of: "`", with: "")
        }
        if thread.section == .snoozed, let until = SnoozeWords.until(thread.snoozedUntil) { return until }
        return nil
    }

    /// Where it is: its project (unless under the project's header) and, with two machines, which.
    private var context: String? {
        let parts = [showsProject ? thread.project?.name : nil, host.map(MachineTag.short)].compactMap { $0 }
        return parts.isEmpty ? nil : parts.joined(separator: " · ")
    }

    /// How long ago, or when a snoozed thread wakes ("Sat 9:00 AM": the moon beside it says
    /// until, and the title keeps the room).
    private var when: String {
        if thread.section == .snoozed, let until = SnoozeWords.until(thread.snoozedUntil) {
            return until.hasPrefix("Until ") ? String(until.dropFirst(6)) : until
        }
        return When.short(thread.updatedAt)
    }
}

/// A row's second line, in three parts: the state's words, why, and where (project, machine).
/// The words and where keep their width; why takes what's left, cut short, and is left out when
/// that's less than `leastReason` (a dot and an ellipsis said nothing: "Working 1m 13s ·… · trek").
struct StandingLayout: Layout {
    var leastReason: CGFloat = 56

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let ideal = subviews.map { $0.sizeThatFits(.unspecified) }
        let total = ideal.reduce(0) { $0 + $1.width }
        return CGSize(width: min(proposal.width ?? total, total), height: ideal.map(\.height).max() ?? 0)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        let widths = Self.widths(available: bounds.width, ideal: subviews.map { $0.sizeThatFits(.unspecified).width },
                                 leastReason: leastReason)
        var x = bounds.minX
        for (i, view) in subviews.enumerated() {
            let w = i < widths.count ? widths[i] : 0
            // What it takes of `w`: a reason cut short ends at its ellipsis, and where follows it.
            let size = view.sizeThatFits(ProposedViewSize(width: w, height: nil))
            view.place(at: CGPoint(x: x, y: bounds.midY - size.height / 2), proposal: ProposedViewSize(width: w, height: size.height))
            x += min(w, size.width)
        }
    }

    /// Each part's width in `available`, from their ideal widths (words, why, where): the words
    /// whole, then where, then why in what's left, or none of it under `leastReason`.
    static func widths(available: CGFloat, ideal: [CGFloat], leastReason: CGFloat) -> [CGFloat] {
        guard ideal.count == 3 else { return ideal }
        let words = min(ideal[0], available)
        let place = min(ideal[2], max(0, available - words))
        let left = max(0, available - words - place)
        let reason = ideal[1] <= left ? ideal[1] : (left >= leastReason ? left : 0)
        return [words, reason, place]
    }
}

/// Text that ticks every second with how long since `since` (ms): "Working 3m 05s".
private struct Ticking: View {
    var since: Int64
    var words: (String) -> String

    var body: some View {
        TimelineView(.periodic(from: .now, by: 1)) { ctx in
            Text(words(When.duration(max(0, Int(ctx.date.timeIntervalSince1970 - Double(since) / 1000)))))
                .monospacedDigit()
        }
    }
}

/// A command a thread left running, as its row and the strip above the composer name it.
enum BackgroundLine {
    /// A command's first line, its runs of spaces closed up.
    static func oneLine(_ title: String) -> String {
        let first = title.split(separator: "\n", omittingEmptySubsequences: true).first.map(String.init) ?? title
        return first.split(separator: " ", omittingEmptySubsequences: true).joined(separator: " ")
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
                // Changes in place: a cross-fade drew the old words and the new ones over each
                // other (seen on iOS 27.2, the row resizing under it), and they change often.
                Text(detail ?? state.label).font(.ui(.caption)).foregroundStyle(Trek.muted)
                    .fixedSize(horizontal: false, vertical: true)
                    .contentTransition(.identity)
                    .transaction { $0.animation = nil }
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

/// The head of the thread list, one row of it: the title, the machine chips (with more than one
/// machine), the update card and a banner for each machine that can't be reached. They stack,
/// each in its own room, and scroll together; the bar's small title takes over once the title
/// has gone under it (see `TopBarFade`).
struct ThreadsHeader: View {
    @Environment(AppModel.self) private var model
    @Binding var picked: String?
    var update: AppUpdate.Available?
    /// The machines to say can't be reached, each under its own name.
    var unreachable: [HostLink] = []

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            ScreenTitle(title: "Threads")
                .padding(.leading, 20)
                .padding(.trailing, 16)
                .padding(.bottom, 6)
            if model.links.count > 1 {
                // Each chip's target reaches 6 pt above and below its capsule (44 pt, as the HIG
                // asks); the bar's own room is that much less.
                MachineFilterBar(picked: $picked)
                    .padding(.horizontal, 16)
                    .padding(.top, -4)
            }
            if let update {
                UpdateCard(update: update)
                    .padding(.horizontal, 16)
                    .padding(.top, 4)
                    .padding(.bottom, 8)
                    // Fades where it stands: sliding up, it passed over the title and the chips.
                    .transition(.asymmetric(insertion: .opacity,
                                            removal: .opacity.animation(.easeOut(duration: 0.12))))
            }
            ForEach(unreachable) { link in
                Group {
                    if case .identityChanged(let expected, let seen) = link.connection {
                        IdentityChangedCard(host: link.name, expected: expected, seen: seen) { model.leave(link) }
                    } else {
                        ConnectionBanner(state: link.connection, detail: link.connectionText,
                                         host: link.name,
                                         retry: { model.retryNow(link) }) { model.leave(link) }
                    }
                }
                .padding(.horizontal, 16)
                .padding(.top, 4)
                .padding(.bottom, 8)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .animation(.snappy, value: update?.version)
    }
}

/// Which machines' threads the list shows: all, or one, each by its tag (its platform's mark
/// and colour), so a Mac's threads and a PC's read apart.
private struct MachineFilterBar: View {
    @Binding var picked: String?
    @Environment(AppModel.self) private var model

    var body: some View {
        // Wrapping rather than scrolling sideways: on iOS 27.2 a scroll view at the head of the
        // list was taken for the list's own, and the bar's large title stopped following the list.
        WordFlow(spacing: 8, lineSpacing: -4) {
            Button {
                withAnimation(.snappy(duration: 0.2)) { picked = nil }
            } label: {
                Text("All")
                    .font(.ui(.subheadline).weight(.medium))
                    .foregroundStyle(picked == nil ? Trek.foreground : Trek.foreground.opacity(0.75))
                    .padding(.horizontal, 14)
                    .frame(height: 32)
                    .background(Trek.foreground.opacity(picked == nil ? 0.14 : 0.06), in: Capsule())
                    .padding(.vertical, 6)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.borderless)
            ForEach(model.links) { link in
                Button {
                    withAnimation(.snappy(duration: 0.2)) { picked = picked == link.id ? nil : link.id }
                } label: {
                    // Its short name: every machine of one person begins with the same "Toby's".
                    MachineTag(name: MachineTag.short(link.name), kind: link.kind, selected: picked == link.id)
                        .overlay(alignment: .topTrailing) {
                            if link.connection != .connected {
                                Circle().fill(Trek.failed).frame(width: 7, height: 7).offset(x: 1, y: -1)
                            }
                        }
                        .padding(.vertical, 6)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.borderless)
                .accessibilityLabel("\(link.name)\(link.kind.word.map { ", \($0)" } ?? "")")
            }
        }
    }
}
