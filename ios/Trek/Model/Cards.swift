import Foundation

// The live cards a thread's agent leaves for work it pushed out: a pull request, a workflow run,
// a release (the Mac's `pr_card.rs`; on the wire a `card` item, see
// crates/trek-remote/src/protocol/cards.rs). The Mac sends a card again, under the same item id,
// whenever it moves on; the words it says are worked out here the way the Mac works them out.

nonisolated enum CardKind: String, LenientEnum {
    case pr, release, run, other
    static var fallback: CardKind { .other }
}

nonisolated enum CardPhase: String, LenientEnum {
    case creating, open, checking, conflicted, ready, merging, merged, closed, releasing, published, passed, failed, other
    static var fallback: CardPhase { .other }

    /// Nothing more to follow.
    var terminal: Bool { [.merged, .closed, .published, .passed, .failed].contains(self) }

    /// The Mac's badge words.
    var label: String {
        switch self {
        case .creating: "Creating"
        case .open: "Open"
        case .checking: "Checking"
        case .conflicted: "Conflicts"
        case .ready: "Ready to merge"
        case .merging: "Merging"
        case .merged: "Merged"
        case .closed: "Closed"
        case .releasing: "Releasing"
        case .published: "Published"
        case .passed: "Passed"
        case .failed: "Failed"
        case .other: "Updated"
        }
    }
}

nonisolated enum CheckState: String, LenientEnum {
    case queued, running, pass, fail, skipped
    static var fallback: CheckState { .skipped }
    var pending: Bool { self == .queued || self == .running }
}

/// A card's buttons, as on the Mac.
nonisolated enum CardAction: String, LenientEnum {
    case merge, resolve, rerun, unknown
    static var fallback: CardAction { .unknown }
}

nonisolated struct CardCheck: Codable, Hashable {
    var name: String
    var state: CheckState
}

/// A run's or release's job.
nonisolated struct CardStage: Codable, Hashable {
    var label: String
    var state: CheckState
    var startedMs: Int64? = nil
    var endedMs: Int64? = nil
}

nonisolated struct CardCommit: Codable, Hashable {
    var title: String
    var oid: String
    var author: String? = nil
    var committedMs: Int64? = nil
}

/// A card as the Mac sends it. Every field may be missing (the Mac leaves out empty ones).
nonisolated struct RemoteCard: Codable, Hashable {
    var key = ""
    var kind: CardKind = .pr
    var phase: CardPhase = .creating
    var title = ""
    var number: Int64? = nil
    var url: String? = nil
    var head = ""
    var base = ""
    var repo: String? = nil
    var commits: Int? = nil
    /// Newest first.
    var history: [CardCommit] = []
    var added: Int? = nil
    var removed: Int? = nil
    var checks: [CardCheck] = []
    var conflicts: [String] = []
    var resolved: [String] = []
    var resolving = false
    var merging = false
    var branchDeleted = false
    var step = ""
    var version = ""
    var channel = ""
    var stages: [CardStage] = []
    var subject = ""
    var event = ""
    var createdMs: Int64 = 0
    var phaseAtMs: Int64 = 0
    var startedMs: Int64 = 0
    var endedMs: Int64 = 0
    var actions: [CardAction] = []
    var notesUrl: String? = nil

    private enum CodingKeys: String, CodingKey {
        case key, kind, phase, title, number, url, head, base, repo, commits, history, added, removed, checks, conflicts, resolved
        case resolving, merging, branchDeleted, step, version, channel, stages, subject, event, createdMs, phaseAtMs
        case startedMs, endedMs, actions, notesUrl
    }

    init() {}

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        func v<T: Decodable>(_ k: CodingKeys, _ d: T) -> T { (try? c.decodeIfPresent(T.self, forKey: k)) ?? d }
        key = v(.key, "")
        kind = v(.kind, .other)
        phase = v(.phase, .creating)
        title = v(.title, "")
        number = v(.number, nil)
        url = v(.url, nil)
        head = v(.head, "")
        base = v(.base, "")
        repo = v(.repo, nil)
        commits = v(.commits, nil)
        history = (try? c.decodeIfPresent(Lossy<CardCommit>.self, forKey: .history))?.elements ?? []
        added = v(.added, nil)
        removed = v(.removed, nil)
        checks = (try? c.decodeIfPresent(Lossy<CardCheck>.self, forKey: .checks))?.elements ?? []
        conflicts = v(.conflicts, [])
        resolved = v(.resolved, [])
        resolving = v(.resolving, false)
        merging = v(.merging, false)
        branchDeleted = v(.branchDeleted, false)
        step = v(.step, "")
        version = v(.version, "")
        channel = v(.channel, "")
        stages = (try? c.decodeIfPresent(Lossy<CardStage>.self, forKey: .stages))?.elements ?? []
        subject = v(.subject, "")
        event = v(.event, "")
        createdMs = v(.createdMs, 0)
        phaseAtMs = v(.phaseAtMs, 0)
        startedMs = v(.startedMs, 0)
        endedMs = v(.endedMs, 0)
        actions = v(.actions, []).filter { $0 != .unknown }
        notesUrl = v(.notesUrl, nil)
        // A missing kind is a pull request (the Mac's default); an unknown one stays `other`.
        if !c.contains(.kind) { kind = .pr }
    }
}

// MARK: The words, as the Mac says them (`pr_card.rs`)

nonisolated extension RemoteCard {
    /// A run's name: its workflow's, once known.
    var runName: String { title.isEmpty ? "Workflow" : title }

    /// Something is still under way (a spinner, the state word in grey).
    var busy: Bool {
        resolving || [.creating, .checking, .merging, .releasing].contains(phase) || (kind == .run && !phase.terminal)
    }

    /// Until when the composer's chip shows it (ms): a good finish lingers a few seconds; a
    /// failure stays until a message is sent; nil while it's live.
    var chipUntil: Int64? {
        [.merged, .published, .passed].contains(phase) ? max(phaseAtMs, createdMs) + 4_000 : nil
    }

    /// The chip's two halves: "#128" and "Ready to merge".
    var chipParts: (what: String, state: String) {
        let what: String
        switch kind {
        case .pr: what = number.map { "#\($0)" } ?? "PR"
        case .release:
            let v = version.split(whereSeparator: { $0 == "-" || $0 == "+" }).first.map(String.init) ?? ""
            what = v.isEmpty ? "Release" : "v\(v)"
        case .run, .other: what = runName
        }
        let state: String
        switch kind {
        case .pr:
            if resolving { state = "Resolving" }
            else if phase == .conflicted, conflicts.count == 1 { state = "1 conflict" }
            else if phase == .conflicted, conflicts.count > 1 { state = "\(conflicts.count) conflicts" }
            else { state = phase.label }
        default:
            if [.failed, .closed].contains(phase), stages.contains(where: { $0.state == .fail }) { state = failedJobs }
            else { state = pipelineState }
        }
        return (what, state)
    }

    /// One line for where the card isn't drawn: "Pull request #128 · Merged".
    var summary: String {
        switch kind {
        case .pr: "Pull request\(number.map { " #\($0)" } ?? "") · \(phase.label)"
        case .release: "Release \(version) · \(phase.label)"
        case .run, .other: "\(runName) run · \(phase.label)"
        }
    }

    /// "Building 2 of 3", "2 of 3 passed", "Publishing", "1 failed".
    var pipelineState: String {
        let total = stages.count
        let passed = stages.filter { $0.state == .pass }.count
        let failed = stages.filter { $0.state == .fail }.count
        switch phase {
        case .published: return "Published"
        case .passed: return "Passed"
        case .failed, .closed:
            if step == "Cancelled" { return "Cancelled" }
            if kind == .release { return "Failed" }
            return failed > 0 ? "\(failed) failed" : "Failed"
        default:
            if kind == .release {
                if stages.contains(where: { $0.isPublish && $0.state == .running }) { return "Publishing" }
                return total > 1 ? "Building \(min(passed + 1, total)) of \(total)" : "Releasing"
            }
            return total > 0 ? "\(passed) of \(total) passed" : "Starting"
        }
    }

    /// "Windows build failed", "2 jobs failed".
    var failedJobs: String {
        let failed = stages.filter { $0.state == .fail }
        switch failed.count {
        case 0: return "Something failed"
        case 1: return "\(failed[0].label) failed"
        default: return "\(failed.count) jobs failed"
        }
    }

    /// A step that only restates the failure.
    private var genericFailure: Bool {
        let s = step.trimmingCharacters(in: .whitespaces)
        return s.isEmpty || s.hasPrefix("Failed: ") || s == "Run failed" || s == "Release workflow failed"
    }

    /// A failed run's or release's title: the job that failed, else the watcher's own words.
    var failureTitle: String {
        stages.contains(where: { $0.state == .fail }) || genericFailure ? failedJobs : step.trimmingCharacters(in: .whitespaces)
    }

    /// Under it: what the step adds (the error), or how the other jobs did.
    var failureDetail: String {
        let s = step.trimmingCharacters(in: .whitespaces)
        if !genericFailure, s != failureTitle { return s }
        let total = stages.count, passed = stages.filter { $0.state == .pass }.count
        if total == 0 { return "" }
        if passed + 1 == total { return "The other jobs passed" }
        if passed == 0 { return "No job passed" }
        return "\(passed) of \(total) jobs passed"
    }

    /// Under Ready to merge.
    var readyDetail: String {
        let names = checks.filter { $0.state == .pass }.map(\.name)
        switch names.count {
        case 0: return "No conflicts"
        case 1...3: return "No conflicts · \(Self.andList(names)) passed"
        default: return "No conflicts · all \(names.count) checks passed"
        }
    }

    /// "Mac + Windows" from a release's jobs; "3 jobs" when the names don't say.
    var platforms: String {
        func has(_ w: String) -> Bool { stages.contains { $0.label.lowercased().contains(w) } }
        var names: [String] = []
        if has("mac") { names.append("Mac") }
        if has("windows") { names.append("Windows") }
        if has("linux") { names.append("Linux") }
        return names.isEmpty ? Self.jobs(stages.count) : names.joined(separator: " + ")
    }

    /// "Mac and Windows" for a published release's line; "The" when its jobs don't say.
    var platformsForSub: String {
        let p = platforms
        if p.contains("+") { return p.replacingOccurrences(of: " + ", with: " and ") }
        if p.hasSuffix("job") || p.hasSuffix("jobs") || p.isEmpty { return "The" }
        return p
    }

    /// The run's trigger as its badge says it: "Push", "Pull request", "Manual".
    var eventLabel: String {
        switch event {
        case "": ""
        case "push": "Push"
        case "pull_request", "pull_request_target": "Pull request"
        case "workflow_dispatch": "Manual"
        case "schedule": "Scheduled"
        case "release": "Release"
        default: event.replacingOccurrences(of: "_", with: " ").capitalizedFirst
        }
    }

    var channelLabel: String { channel.capitalizedFirst }

    /// How long the run has taken (to its end once it has one), or the release (to when it
    /// settled); 0 before it starts.
    func tookMs(now: Int64) -> Int64 {
        if kind == .release {
            let began = startedMs > 0 ? startedMs : createdMs
            let end = phase.terminal ? max(phaseAtMs, createdMs) : now
            return max(0, end - began)
        }
        guard startedMs > 0 else { return 0 }
        return max(0, (endedMs > 0 ? endedMs : now) - startedMs)
    }

    /// A pull request's conflicts left, as the footer counts them.
    var conflictsTitle: String {
        let base = base.isEmpty ? "main" : base
        if resolving, conflicts.isEmpty, !resolved.isEmpty { return "Conflicts resolved" }
        return "\(Self.conflictsLabel(conflicts.count)) with \(base)"
    }

    static func conflictsLabel(_ n: Int) -> String {
        switch n {
        case 0: "Conflicts"
        case 1: "1 conflict"
        default: "\(n) conflicts"
        }
    }

    static func jobs(_ n: Int) -> String { n == 0 ? "" : n == 1 ? "1 job" : "\(n) jobs" }

    /// "Clippy, Test (macOS) and Test (Windows)".
    static func andList(_ names: [String]) -> String {
        switch names.count {
        case 0: ""
        case 1: names[0]
        default: names.dropLast().joined(separator: ", ") + " and " + names[names.count - 1]
        }
    }

    /// "45s", "3m 12s", "1h 4m".
    static func duration(_ ms: Int64) -> String {
        let s = max(0, ms / 1000)
        if s < 60 { return "\(s)s" }
        if s < 3600 { return "\(s / 60)m \(String(format: "%02d", s % 60))s" }
        return "\(s / 3600)h \(s % 3600 / 60)m"
    }

    /// "just now", "2m ago".
    static func ago(_ ms: Int64, now: Int64) -> String {
        let s = max(0, (now - ms) / 1000)
        if s < 60 { return "just now" }
        if s < 3600 { return "\(s / 60)m ago" }
        if s < 86400 { return "\(s / 3600)h ago" }
        return "\(s / 86400)d ago"
    }

    static func nowMs(_ date: Date = Date()) -> Int64 { Int64(date.timeIntervalSince1970 * 1000) }
}

nonisolated extension CardStage {
    var isPublish: Bool {
        let l = label.lowercased()
        return l.contains("publish") || l.contains("upload") || l.contains("deploy")
    }

    /// How long it took, or has taken so far; 0 before it starts.
    func tookMs(now: Int64) -> Int64 {
        guard let start = startedMs, start > 0 else { return 0 }
        if let end = endedMs, end > 0 { return max(0, end - start) }
        return state.pending ? max(0, now - start) : 0
    }

    /// Its time on the right of its row: "9m 12s", or "4m" while it runs; nothing before it starts.
    func timeLabel(now: Int64) -> String {
        let ms = tookMs(now: now)
        guard ms > 0 else { return "" }
        if state == .running { return ms >= 60_000 ? "\(ms / 60_000)m" : "\(ms / 1000)s" }
        return RemoteCard.duration(ms)
    }
}

nonisolated private extension String {
    var capitalizedFirst: String { prefix(1).uppercased() + dropFirst() }
}
