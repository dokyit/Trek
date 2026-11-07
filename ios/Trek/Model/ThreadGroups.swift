import Foundation

/// How the thread list is arranged: by project, as the Mac's sidebar is, or by status.
enum ThreadGrouping: String, CaseIterable, Identifiable {
    case project, status
    var id: String { rawValue }
    var label: String {
        switch self {
        case .project: "Project"
        case .status: "Status"
        }
    }
    static let key = "threadGrouping"
}

/// One project's threads, ordered as the Mac's sidebar orders them (`sidebar.rs`, `live_order`):
/// those that want something or are at work first, then the rest, unread first, newest first.
struct ProjectGroup: Identifiable, Equatable {
    /// The project's id, or "" for threads in no project.
    var id: String
    var project: ProjectRef?
    /// The rows that stay out however folded the group is: waiting on the user, working.
    var active: [ThreadSummary]
    /// The rest, unread first.
    var quiet: [ThreadSummary]

    var name: String { project?.name ?? "No project" }
    var needs: Int { active.count { $0.waitsOnYou } }
    var working: Int { active.count(where: \.atWork) }
    /// Done with their turns, a command of theirs still running (the sky dot).
    var inBackground: Int { active.count(where: \.onlyRunsInBackground) }
    var unread: Int { quiet.count { $0.unseen } }
    var count: Int { active.count + quiet.count }
    var newest: Int64 { (active + quiet).map(\.updatedAt).max() ?? 0 }
}

extension ThreadSummary {
    /// Sub-agents of its own at work right now (the Mac's sidebar pill counts these).
    var runningSubAgents: [SubAgent] { (subAgents ?? []).filter { $0.state == .running || $0.state == .needsYou } }

    /// Its turn is over but it still waits on what it started (sub-agents, its agent's background
    /// work, their reports on the way): at work, as the Mac's sidebar says "Waiting". The Mac says
    /// so; an older one doesn't, and then it's what that Mac's own sidebar works out for another
    /// machine's row (`Machines::waiting`): idle, nothing asked, sub-agents out.
    var waitsInBackground: Bool {
        busyInBackground ?? (runState == .idle && needs == nil && !runningSubAgents.isEmpty)
    }

    /// Working, or waiting on what it started: the Mac counts both as at work (its Working group
    /// and header).
    var atWork: Bool { runState == .working || waitsInBackground }

    /// What its agent left running in the background (a command, a dev server, a watcher), turn
    /// or no turn: the Mac's strip above the composer, and the still sky dot on its sidebar row.
    /// From a Mac that sends only their titles, tasks that can't be opened or stopped from here.
    var backgroundWork: [BackgroundTask] {
        if let tasks = backgroundTasks, !tasks.isEmpty { return tasks }
        return (background ?? []).enumerated().map { n, title in
            BackgroundTask(id: "\(Self.titleOnly)\(n)", kind: .shell, title: title)
        }
    }

    /// Something of its own still runs in the background.
    var runsInBackground: Bool { !(backgroundTasks ?? []).isEmpty || !(background ?? []).isEmpty }

    /// Its turn is over and nothing waits on the user, but a command or a server it started runs on.
    var onlyRunsInBackground: Bool { runsInBackground && !atWork && !waitsOnYou }

    /// The id of a task known only by its title (an older Mac's row).
    static let titleOnly = "title:"

    /// A row the Mac draws as a card: it waits on the user, works, or has sub-agents out. Here a
    /// thread with work running in the background is kept out with them too, under its project,
    /// so "Show more" never hides that something still runs (a folded project's header counts it).
    var isActive: Bool {
        waitsOnYou || runState == .working || !runningSubAgents.isEmpty || waitsInBackground || runsInBackground || cutShort != nil
    }
}

extension ProjectRef {
    /// The Mac's "No project", which it sends for a thread outside every project: id `""`,
    /// qualified here with its host (`"demo:"`).
    var isNone: Bool { id.isEmpty || id.hasSuffix(":") }
}

extension AppModel {
    /// Every thread waiting on the user, newest first: what the hand on the "New thread" pill and
    /// the tab's badge count.
    var waitingOnYou: [ThreadSummary] {
        threads.filter { $0.waitsOnYou && $0.section != .snoozed }.sorted { $0.updatedAt > $1.updatedAt }
    }

    /// The inbox and what's working in `list`, by project: projects with something waiting or at
    /// work first, then those with unread threads, then the most recently active (the Mac's order).
    func liveGroups(of list: [ThreadSummary]) -> [ProjectGroup] {
        groups(list.filter { $0.section == .inbox || $0.section == .working }) { a, b in
            if (a.needs > 0) != (b.needs > 0) { return a.needs > 0 }
            if a.active.isEmpty != b.active.isEmpty { return !a.active.isEmpty }
            if (a.unread > 0) != (b.unread > 0) { return a.unread > 0 }
            if a.newest != b.newest { return a.newest > b.newest }
            return a.name.localizedCaseInsensitiveCompare(b.name) == .orderedAscending
        }
    }

    /// The quiet inbox by project, for the list's Projects: what waits on the user and what's at
    /// work have places of their own above it (`needsYou`, `active`), as in the Mac's sidebar.
    func quietGroups(of list: [ThreadSummary]) -> [ProjectGroup] {
        liveGroups(of: list.filter { !$0.isActive })
    }

    /// At work across projects, as the Mac's Active: working, waiting on what it started, running
    /// in the background, cut short. Not what waits on the user (`needsYou`), nor pinned threads.
    func active(of list: [ThreadSummary]) -> [ThreadSummary] {
        list.filter { ($0.section == .inbox || $0.section == .working) && $0.isActive && !$0.waitsOnYou }
            .sorted(by: Self.activeOrder)
    }

    /// Settled threads in `list` by project, the project settled in most recently first.
    func settledGroups(of list: [ThreadSummary]) -> [ProjectGroup] {
        groups(list.filter { $0.section == .settled }) { $0.newest > $1.newest }
    }

    /// The group a thread is listed under: one for a project's name whichever machines it's on,
    /// and one for threads in no project. Keyed by id, each machine's "No project" (which every
    /// Mac sends as `""`, qualified here by host) and each machine's copy of a project made a
    /// section of its own, two "No project"s and two "trek"s, told apart only by their rows.
    func groupKey(of t: ThreadSummary) -> String {
        guard let p = t.project, !p.isNone else { return "" }
        return "name:" + p.name.lowercased()
    }

    private func groups(_ list: [ThreadSummary], by order: (ProjectGroup, ProjectGroup) -> Bool) -> [ProjectGroup] {
        var byProject: [String: [ThreadSummary]] = [:]
        var refs: [String: ProjectRef] = [:]
        for t in list {
            let key = groupKey(of: t)
            byProject[key, default: []].append(t)
            if let p = t.project, refs[key] == nil { refs[key] = p }
        }
        return byProject.map { key, threads in
            let active = threads.filter(\.isActive).sorted(by: Self.activeOrder)
            let quiet = threads.filter { !$0.isActive }.sorted {
                $0.unseen != $1.unseen ? $0.unseen : $0.updatedAt > $1.updatedAt
            }
            // The one "No project" keeps the Mac's name for it, but not one machine's id.
            let ref = key.isEmpty ? refs[key].map { ProjectRef(id: "", name: $0.name, hue: $0.hue, monogram: $0.monogram) } : refs[key]
            return ProjectGroup(id: key, project: ref, active: active, quiet: quiet)
        }
        .sorted(by: order)
    }

    /// Waiting on the user first, then working, then running in the background, then the rest.
    static func activeOrder(_ a: ThreadSummary, _ b: ThreadSummary) -> Bool {
        if a.waitsOnYou != b.waitsOnYou { return a.waitsOnYou }
        let aw = a.atWork, bw = b.atWork
        if aw != bw { return aw }
        if a.runsInBackground != b.runsInBackground { return a.runsInBackground }
        return a.updatedAt > b.updatedAt
    }
}
