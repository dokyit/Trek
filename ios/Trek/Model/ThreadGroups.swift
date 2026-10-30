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

    /// A row the Mac draws as a card: it waits on the user, works, or has sub-agents out.
    var isActive: Bool { waitsOnYou || runState == .working || !runningSubAgents.isEmpty || waitsInBackground }
}

extension AppModel {
    /// Every thread waiting on the user, newest first: what the hand on the "New thread" pill and
    /// the tab's badge count.
    var waitingOnYou: [ThreadSummary] {
        threads.filter { $0.waitsOnYou && $0.section != .snoozed }.sorted { $0.updatedAt > $1.updatedAt }
    }

    /// The inbox and what's working, by project: projects with something waiting or at work first,
    /// then those with unread threads, then the most recently active (the Mac's order).
    var liveGroups: [ProjectGroup] {
        groups(threads.filter { $0.section == .inbox || $0.section == .working }) { a, b in
            if (a.needs > 0) != (b.needs > 0) { return a.needs > 0 }
            if a.active.isEmpty != b.active.isEmpty { return !a.active.isEmpty }
            if (a.unread > 0) != (b.unread > 0) { return a.unread > 0 }
            if a.newest != b.newest { return a.newest > b.newest }
            return a.name.localizedCaseInsensitiveCompare(b.name) == .orderedAscending
        }
    }

    /// Settled threads by project, the project settled in most recently first.
    var settledGroups: [ProjectGroup] {
        groups(threads.filter { $0.section == .settled }) { $0.newest > $1.newest }
    }

    private func groups(_ list: [ThreadSummary], by order: (ProjectGroup, ProjectGroup) -> Bool) -> [ProjectGroup] {
        var byProject: [String: [ThreadSummary]] = [:]
        var refs: [String: ProjectRef] = [:]
        for t in list {
            let id = t.project?.id ?? ""
            byProject[id, default: []].append(t)
            if let p = t.project, refs[id] == nil { refs[id] = p }
        }
        return byProject.map { id, threads in
            let active = threads.filter(\.isActive).sorted(by: Self.activeOrder)
            let quiet = threads.filter { !$0.isActive }.sorted {
                $0.unseen != $1.unseen ? $0.unseen : $0.updatedAt > $1.updatedAt
            }
            return ProjectGroup(id: id, project: refs[id] ?? project(id)?.ref, active: active, quiet: quiet)
        }
        .sorted(by: order)
    }

    /// Waiting on the user first, then working (longest at it first), then the rest.
    private static func activeOrder(_ a: ThreadSummary, _ b: ThreadSummary) -> Bool {
        if a.waitsOnYou != b.waitsOnYou { return a.waitsOnYou }
        let aw = a.atWork, bw = b.atWork
        if aw != bw { return aw }
        return a.updatedAt > b.updatedAt
    }
}
