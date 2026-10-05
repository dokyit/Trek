import Foundation

// What the phone asks the Mac for beyond threads and transcripts: usage, Basecamp, notes, git,
// slash commands and settings. Mirrors crates/trek-remote/src/protocol.rs (docs/MOBILE.md); the
// replies are decoded whole from their message (snake_case keys, the coder converts).

// MARK: Usage

/// Plan usage, as the Mac's Usage popover shows it.
nonisolated struct Usage: Codable, Hashable {
    /// Agents that reported their plan, in the Mac's agent order.
    var providers: [ProviderUsage] = []
    /// The Mac was still asking an agent when it answered: ask again shortly for the rest.
    var loading: Bool? = nil
}

nonisolated struct ProviderUsage: Codable, Hashable, Identifiable {
    var agent: AgentRef
    /// "Claude Max", "ChatGPT Plus".
    var plan: String? = nil
    /// 5-hour, weekly and per-model windows. Empty: the plan has none.
    var limits: [UsageLimit] = []
    /// Something the plan reports besides its limits (Devin's on-demand balance).
    var note: String? = nil
    var error: String? = nil
    var id: String { agent.key }
}

nonisolated struct UsageLimit: Codable, Hashable, Identifiable {
    /// "5-hour limit", "Weekly limit", "Weekly · Opus".
    var label: String
    /// 0–100 used.
    var percent: Double
    var resetsAt: Int64? = nil
    /// "5h", "7d".
    var window: String? = nil
    var id: String { label }
}

// MARK: Basecamp

nonisolated enum BasecampRange: String, Codable, CaseIterable, Identifiable {
    case today, week, all
    var id: String { rawValue }
    /// As the Mac's segmented control says it.
    var label: String {
        switch self {
        case .today: "Today"
        case .week: "This week"
        case .all: "All time"
        }
    }
}

/// The Basecamp recap, worded as the Mac words it.
nonisolated struct Basecamp: Codable, Hashable {
    var range: BasecampRange
    /// "Good evening, Monday 5 October", "Good evening — on the trail since 3 June".
    var greeting: String
    /// "Today's trek".
    var title: String
    var updatedAt: Int64
    /// "Ready for review": what needs the user first, then finished threads not looked at yet.
    var review: [ReviewRow] = []
    /// Nothing on the trail in the range: show `invitation` (and a New thread button).
    var empty: Bool? = nil
    var invitation: String? = nil
    /// The recap in sentences: text, numbers in bold, project and model badges.
    var narrative: [RecapSpan] = []
    var summary: RecapSummary? = nil
    var profile: ElevationProfile? = nil
    var tiles: [StatTile] = []
}

nonisolated enum ReviewStatus: String, LenientEnum {
    case needsYou = "needs_you", failed, paused, done
    static var fallback: ReviewStatus { .done }
}

nonisolated struct ReviewRow: Codable, Hashable, Identifiable {
    var threadId: String
    var title: String
    var status: ReviewStatus
    /// "Approval", "Question", "Plan to review", "Paused until 3 PM", "Failed"; nil when done.
    var label: String? = nil
    var agent: AgentRef
    var project: ProjectRef? = nil
    var additions: Int = 0
    var deletions: Int = 0
    var updatedAt: Int64
    var unseen: Bool? = nil
    var id: String { threadId }
}

/// A piece of the recap's sentences.
nonisolated struct RecapSpan: Codable, Hashable {
    /// `text`, `strong` (a number worth reading first), `project` (drawn as its badge), `model`
    /// (with its agent's logo).
    var kind: String
    var text: String
    var project: ProjectRef? = nil
    var agent: AgentRef? = nil
}

nonisolated struct RecapSummary: Codable, Hashable {
    var prompts: Int
    var threads: Int
    var turns: Int
    var agentSecs: Int
    /// "1h 2m", "under a minute".
    var agentTime: String
    var tokens: Int64
    var failed: Int? = nil
    var topProject: ProjectShare? = nil
    var bestModel: ModelShare? = nil
}

nonisolated struct ProjectShare: Codable, Hashable {
    var project: ProjectRef
    var prompts: Int
    var tokens: Int64? = nil
}

nonisolated struct ModelShare: Codable, Hashable {
    var agent: AgentRef
    /// "Claude Opus 5.5".
    var label: String
    var tokens: Int64? = nil
    var turns: Int? = nil
    /// Its share of the reported tokens, in percent.
    var share: Int? = nil
}

/// The elevation profile: activity drawn as a mountain, the summit flagged, the hiker at "now".
nonisolated struct ElevationProfile: Codable, Hashable {
    var buckets: [ProfileBucket]
    var summit: Int? = nil
    /// The stretch "now" is in, while the range is current.
    var now: Int? = nil
    /// Where "now" is across the range, 0–1.
    var nowAt: Double? = nil
    /// "Summit at 2 PM", "A flat trail so far".
    var line: String
    /// "18 prompts".
    var total: String
    var ticks: [Tick] = []
}

nonisolated struct ProfileBucket: Codable, Hashable {
    /// Height: agent minutes when turns were timed, else prompts.
    var value: Double
    /// "2–3 PM", "Tue 3–6 PM", "Sep 14".
    var label: String
    /// What the Mac says over it when it's hovered: "2–3 PM · 4 prompts · 12m of agent time".
    var line: String
    var prompts: Int? = nil
    var agentSecs: Int? = nil
    var tokens: Int64? = nil
}

nonisolated struct Tick: Codable, Hashable {
    /// 0–1 across the profile.
    var at: Double
    var label: String
}

nonisolated enum TileKind: String, LenientEnum {
    case bestModel = "best_model", workedMostOn = "worked_most_on", tokens, agentTime = "agent_time", planLeft = "plan_left"
    case unknown
    static var fallback: TileKind { .unknown }
}

/// A stat tile: a quiet label, the figure, a note under it.
nonisolated struct StatTile: Codable, Hashable, Identifiable {
    var kind: TileKind
    var label: String
    var figure: String
    var note: String
    /// The logo beside the figure (best model, plan left).
    var agent: AgentRef? = nil
    /// The badge beside the figure (worked most on).
    var project: ProjectRef? = nil
    /// Tokens used so far through the range, 0–1 a stretch (tokens).
    var sparkline: [Double]? = nil
    /// How much is left, 0–100 (plan left).
    var percent: Double? = nil
    var resetsAt: Int64? = nil
    var id: String { label }
}

// MARK: Notes

nonisolated struct NoteSummary: Codable, Hashable, Identifiable {
    var id: String
    /// The first line with words in it, without markdown; "Untitled".
    var title: String
    /// The text after the title, on one line.
    var preview: String
    /// Last changed (ms).
    var modified: Int64
}

/// A note, whole: markdown (colour and highlights are inline `<span style="color: …">` and
/// `<mark>`).
nonisolated struct Note: Codable, Hashable, Identifiable {
    var id: String
    var title: String
    var body: String
    /// Pass it back when saving: if the note changed on the Mac since, the Mac refuses.
    var modified: Int64
}

// MARK: Git

/// Whose folder a git request is about: a thread's (its worktree, for one in a worktree), or a
/// project's.
nonisolated enum GitTarget: Hashable {
    case thread(String)
    case project(String)
    var json: [String: Any] {
        switch self {
        case .thread(let id): ["thread_id": id]
        case .project(let id): ["project_id": id]
        }
    }
}

/// A folder's git status, as the Mac's Git panel shows it. For a worktree thread: its changes
/// against its base (its commits and what isn't committed yet).
nonisolated struct GitStatus: Codable, Hashable {
    var isRepo: Bool? = nil
    var branch: String? = nil
    var defaultBranch: String? = nil
    /// Against the upstream, or for a worktree, against its base.
    var ahead: Int? = nil
    var behind: Int? = nil
    var hasUpstream: Bool? = nil
    var files: [ChangedFile] = []
    var worktree: WorktreeStatus? = nil
    /// Another branch can be checked out now; else `switchBlocked` says why.
    var canSwitch: Bool? = nil
    var switchBlocked: String? = nil
}

nonisolated struct WorktreeStatus: Codable, Hashable {
    var branch: String
    var base: String
    var uncommitted: Int? = nil
    var unpushed: Int? = nil
    /// Why it can't be merged into its base now; nil: it can.
    var mergeBlocked: String? = nil
    /// Commits its base doesn't have (lost with the branch, if it's deleted).
    var unmerged: Int? = nil
    var missing: Bool? = nil
}

nonisolated struct GitDiff: Codable, Hashable {
    var path: String
    /// Unified diff (an untracked file: all its lines, `+`).
    var diff: String
    var truncated: Bool? = nil
}

nonisolated struct GitBranches: Codable, Hashable {
    var current: String? = nil
    var defaultBranch: String? = nil
    /// Local branches, most recently committed first.
    var branches: [String] = []
}

// MARK: Commands

nonisolated enum CommandKind: String, LenientEnum {
    case command, skill, agent
    static var fallback: CommandKind { .command }
}

/// A slash command, as the Mac composer's `/` picker lists it.
nonisolated struct CommandInfo: Codable, Hashable, Identifiable {
    /// Without the slash: "permissions full", "compact".
    var name: String
    var description: String
    var kind: CommandKind
    /// One of Trek's own (the Mac answers it, not the agent).
    var trek: Bool? = nil
    var id: String { name }
}

// MARK: Settings

nonisolated enum NotifyMode: String, LenientEnum, CaseIterable, Identifiable {
    case off, banner, sound, bannerAndSound = "banner_and_sound"
    static var fallback: NotifyMode { .bannerAndSound }
    var id: String { rawValue }
    var label: String {
        switch self {
        case .off: "Off"
        case .banner: "Banner"
        case .sound: "Sound"
        case .bannerAndSound: "Banner and sound"
        }
    }
}

nonisolated enum PushWhen: String, LenientEnum, CaseIterable, Identifiable {
    case away, always
    static var fallback: PushWhen { .away }
    var id: String { rawValue }
    var label: String { self == .away ? "When I'm away" : "Always" }
}

nonisolated enum MacTheme: String, LenientEnum, CaseIterable, Identifiable {
    case system, night, paper
    static var fallback: MacTheme { .system }
    var id: String { rawValue }
    var label: String {
        switch self {
        case .system: "Match macOS"
        case .night: "Night"
        case .paper: "Paper"
        }
    }
}

/// The Mac's settings the phone may see and change.
nonisolated struct MacSettings: Codable, Hashable {
    /// New threads' agent, model (nil: the agent's default), effort and access.
    var defaultAgent: String
    var defaultModel: String? = nil
    var defaultEffort: String
    var defaultAccess: Access
    /// What a message to a working thread does.
    var followUp: SendMode
    /// The Mac's own notifications.
    var notifications: NotifyMode
    var push: PushSettings
    /// Settle finished threads after this many idle days (0: never).
    var autoSettleDays: Int
    var theme: MacTheme
    /// Full access is unlocked on the Mac (only the Mac changes that).
    var fullAccess: Bool? = nil
}

/// Notifications on the phone through ntfy.
nonisolated struct PushSettings: Codable, Hashable {
    var enabled: Bool
    var when: PushWhen
    /// "https://ntfy.sh", or the user's own server.
    var server: String
    /// The topic (empty until push is first on). Anyone who has it can read the notes.
    var topic: String
    /// The topic on the web, `https://ntfy.sh/<topic>` (ntfy's web app).
    var topicUrl: String? = nil
    /// `ntfy://<host>/<topic>`: ntfy's subscribe link. ntfy documents it for Android only; its iOS
    /// app registers no URL scheme, so on an iPhone offer "Copy topic" and have the user add it
    /// in ntfy (+, paste the topic; another server under "Use another server").
    var subscribeUrl: String? = nil
}

/// What to change in the Mac's settings; nil leaves it as it is.
nonisolated struct SettingsChange: Hashable {
    var defaultAgent: String? = nil
    /// "" goes back to the agent's default.
    var defaultModel: String? = nil
    var defaultEffort: String? = nil
    var defaultAccess: Access? = nil
    var followUp: SendMode? = nil
    var notifications: NotifyMode? = nil
    var push: Bool? = nil
    var pushWhen: PushWhen? = nil
    var pushServer: String? = nil
    /// Make a new random topic (the old one stops getting notes).
    var newPushTopic = false
    var autoSettleDays: Int? = nil
    var theme: MacTheme? = nil

    var json: [String: Any] {
        var o: [String: Any] = [:]
        if let defaultAgent { o["default_agent"] = defaultAgent }
        if let defaultModel { o["default_model"] = defaultModel }
        if let defaultEffort { o["default_effort"] = defaultEffort }
        if let defaultAccess { o["default_access"] = defaultAccess.rawValue }
        if let followUp { o["follow_up"] = followUp.rawValue }
        if let notifications { o["notifications"] = notifications.rawValue }
        if let push { o["push"] = push }
        if let pushWhen { o["push_when"] = pushWhen.rawValue }
        if let pushServer { o["push_server"] = pushServer }
        if newPushTopic { o["new_push_topic"] = true }
        if let autoSettleDays { o["auto_settle_days"] = autoSettleDays }
        if let theme { o["theme"] = theme.rawValue }
        return o
    }

    /// The settings with this change made (for showing it before the Mac answers).
    func applied(to s: MacSettings) -> MacSettings {
        var s = s
        if let defaultAgent, defaultAgent != s.defaultAgent { s.defaultAgent = defaultAgent; s.defaultModel = nil }
        if let defaultModel { s.defaultModel = defaultModel.isEmpty ? nil : defaultModel }
        if let defaultEffort { s.defaultEffort = defaultEffort }
        if let defaultAccess { s.defaultAccess = defaultAccess }
        if let followUp { s.followUp = followUp }
        if let notifications { s.notifications = notifications }
        if let push { s.push.enabled = push }
        if let pushWhen { s.push.when = pushWhen }
        if let pushServer { s.push.server = pushServer }
        if let autoSettleDays { s.autoSettleDays = autoSettleDays }
        if let theme { s.theme = theme }
        return s
    }
}

// MARK: Acks that ask for something

/// A screen the phone should open, as an ack asks (`/new` sent to a thread).
nonisolated struct OpenScreen: Codable, Hashable {
    /// `new_thread`.
    var screen: String
    var projectId: String? = nil
}
