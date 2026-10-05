import Foundation

/// What the demo Mac says about everything besides threads: usage, Basecamp, notes, git, slash
/// commands and settings. Realistic, made up, the same shapes a real Mac sends.
enum DemoMac {
    static var now: Int64 { Int64(Date().timeIntervalSince1970 * 1000) }
    static let claude = AgentRef(key: "claude-code", name: "Claude Code", logo: "claude-code")
    static let codex = AgentRef(key: "codex", name: "Codex", logo: "codex")

    // MARK: Thread details

    /// The details a Mac row carries, for the seeded threads (context, cost, sub-agents…).
    static func details(_ t: inout ThreadSummary) {
        let plan = Cost(label: "≈ $1.84 at API prices", billing: .plan, plan: "Claude Max", detail: "Included in your Claude Max plan")
        let metered = Cost(label: "$0.41", billing: .metered, detail: "Billed per token by your API provider")
        t.effortLabel = Effort.label(t.effort ?? "high")
        t.git = GitSummary(changed: 0, ahead: 0, behind: 0, defaultBranch: "main")
        if t.worktree { t.base = "main" }
        switch t.id {
        case "t-inbox":
            t.context = ContextUse(used: 171_000, window: 200_000, percent: 86)
            t.cost = plan
            t.subAgents = [
                SubAgent(agent: codex, model: "Sol", title: "Review the inbox section rules", state: .running, since: now - 95_000),
                SubAgent(agent: claude, title: "Find every place that reads snoozed_until", state: .running, since: now - 40_000),
            ]
            t.background = ["cargo watch -x 'test -p trek-core'"]
            t.git = GitSummary(changed: 3, ahead: 2, behind: 0, defaultBranch: "main")
        case "t-flaky":
            t.context = ContextUse(used: 61_200, window: 200_000, percent: 31)
            t.cost = plan
            t.git = GitSummary(changed: 2, ahead: 1, behind: 0, defaultBranch: "main")
        case "t-ratelimit":
            t.context = ContextUse(used: 22_400, window: 272_000, percent: 8)
            t.cost = metered
        case "t-offline":
            t.context = ContextUse(used: 88_000, window: 200_000, percent: 44)
            t.cost = plan
            t.background = ["xcodebuild -scheme Fieldnotes test"]
            t.git = GitSummary(changed: 2, ahead: 0, behind: 3, defaultBranch: "main")
        case "t-tailscale":
            t.context = ContextUse(used: 104_000, window: 200_000, percent: 52)
            t.cost = Cost(label: "≈ $3.10 at API prices", billing: .plan, plan: "Claude Max", detail: "Included in your Claude Max plan")
        default:
            break
        }
    }

    /// The files a finished turn changed, for the seeded transcripts.
    static func changes(for tid: String) -> TurnChanges? {
        let f = { (path: String, status: FileStatus, added: Int, removed: Int) in ChangedFile(path: path, status: status, added: added, removed: removed) }
        let files: [ChangedFile]
        switch tid {
        case "t-tailscale":
            files = [f("crates/trek-remote/src/pairing.rs", .modified, 41, 3), f("crates/trek-remote/tests/pairing.rs", .added, 13, 0),
                     ChangedFile(path: "crates/trek-remote/src/address.rs", status: .renamed, from: "crates/trek-remote/src/lan.rs", added: 6, removed: 2)]
        case "t-vite":
            files = [f("package.json", .modified, 6, 6), f("pnpm-lock.yaml", .modified, 412, 389), f("vite.config.ts", .modified, 14, 9),
                     f("src/legacy/polyfills.ts", .deleted, 0, 48), ChangedFile(path: "public/og.png", status: .modified, binary: true)]
        case "t-release":
            files = [f("CHANGELOG.md", .modified, 48, 2)]
        default:
            return nil
        }
        return TurnChanges(files: files, added: files.reduce(0) { $0 + $1.added }, removed: files.reduce(0) { $0 + $1.removed })
    }

    // MARK: Usage

    static func usage() -> Usage {
        let h = { (hours: Int64) in now + hours * 3_600_000 }
        return Usage(providers: [
            ProviderUsage(agent: claude, plan: "Claude Max", limits: [
                UsageLimit(label: "5-hour limit", percent: 42, resetsAt: h(2), window: "5h"),
                UsageLimit(label: "Weekly limit", percent: 18, resetsAt: h(96), window: "7d"),
                UsageLimit(label: "Weekly · Opus", percent: 64, resetsAt: h(96), window: "7d"),
            ]),
            ProviderUsage(agent: codex, plan: "ChatGPT Pro", limits: [
                UsageLimit(label: "5-hour limit", percent: 91, resetsAt: h(1), window: "5h"),
                UsageLimit(label: "Weekly limit", percent: 33, resetsAt: h(130), window: "7d"),
            ]),
            ProviderUsage(agent: AgentRef(key: "acp:devin", name: "Devin", logo: "devin"), plan: "Devin Core", limits: [],
                          note: "$18.40 of on-demand credit left"),
        ])
    }

    // MARK: Basecamp

    static func basecamp(_ range: BasecampRange, threads: [ThreadSummary], projects: [ProjectSummary]) -> Basecamp {
        let n: Int
        let label: (Int) -> String
        switch range {
        case .today:
            n = 24
            label = { i in "\((i + 11) % 12 + 1)–\((i + 12) % 12 + 1) \(i < 12 ? "AM" : "PM")" }
        case .week:
            n = 56
            label = { i in "\(["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"][i / 8]) \(["12–3 AM", "3–6 AM", "6–9 AM", "9 AM–12 PM", "12–3 PM", "3–6 PM", "6–9 PM", "9 PM–12 AM"][i % 8])" }
        case .all:
            n = 60
            label = { i in "Day \(i + 1)" }
        }
        let shape = { (i: Int) -> Double in
            let quiet = range == .today && !(9..<20).contains(i)
            return quiet ? 0 : sin(Double(i) * 0.7) * 0.5 + 0.5
        }
        let buckets = (0..<n).map { i -> ProfileBucket in
            let prompts = Int((shape(i) * 6).rounded())
            let secs = Int(shape(i) * 1500)
            let l = label(i)
            return ProfileBucket(value: Double(secs) / 60, label: l,
                                 line: prompts == 0 ? "\(l) · quiet" : "\(l) · \(prompts) prompts · \(secs / 60)m of agent time",
                                 prompts: prompts, agentSecs: secs, tokens: Int64(secs) * 900)
        }
        let summit = buckets.indices.max { buckets[$0].value < buckets[$1].value }
        let prompts = buckets.reduce(0) { $0 + ($1.prompts ?? 0) }
        let secs = buckets.reduce(0) { $0 + ($1.agentSecs ?? 0) }
        let tokens = buckets.reduce(Int64(0)) { $0 + ($1.tokens ?? 0) }
        let time = "\(secs / 3600)h \(secs % 3600 / 60)m"
        var sum: Int64 = 0
        let spark = buckets.map { b -> Double in sum += b.tokens ?? 0; return Double(sum) / Double(max(tokens, 1)) }
        let trek = projects.first { $0.id == "p-trek" }?.ref ?? ProjectRef(id: "p-trek", name: "trek", hue: 24, monogram: "TR")
        let review = threads.filter { $0.needs != nil || $0.unseen || $0.runState == .failed }.map { t -> ReviewRow in
            let (status, text): (ReviewStatus, String?) = switch (t.needs?.kind, t.runState) {
            case (.limit?, _): (.paused, "Paused until 4 PM")
            case (_, .failed): (.failed, "Failed")
            case (.question?, _): (.needsYou, "Question")
            case (.plan?, _): (.needsYou, "Plan to review")
            case (.approval?, _): (.needsYou, "Approval")
            default: (.done, nil)
            }
            return ReviewRow(threadId: t.id, title: t.title, status: status, label: text, agent: t.agent, project: t.project,
                             additions: t.additions, deletions: t.deletions, updatedAt: t.updatedAt, unseen: t.unseen)
        }
        let opening = switch range {
        case .today: "You sent "
        case .week: "This week you sent "
        case .all: "So far you've sent "
        }
        return Basecamp(
            range: range,
            greeting: range == .all ? "Good evening — on the trail since 3 June" : "Good evening, Monday 5 October",
            title: range == .today ? "Today's trek" : range == .week ? "This week's trek" : "Your trek so far",
            updatedAt: now,
            review: review,
            narrative: [
                RecapSpan(kind: "text", text: opening), RecapSpan(kind: "strong", text: "\(prompts) prompts"),
                RecapSpan(kind: "text", text: " across "), RecapSpan(kind: "strong", text: "9 threads"),
                RecapSpan(kind: "text", text: ". Most of it went into "), RecapSpan(kind: "project", text: trek.name, project: trek),
                RecapSpan(kind: "text", text: ", with "), RecapSpan(kind: "model", text: "Claude Opus 5.5", agent: claude),
                RecapSpan(kind: "text", text: " carrying "), RecapSpan(kind: "strong", text: "77%"),
                RecapSpan(kind: "text", text: " of the tokens, ahead of "), RecapSpan(kind: "model", text: "GPT-6 Astra", agent: codex),
                RecapSpan(kind: "text", text: ". Your agents were on the trail for "), RecapSpan(kind: "strong", text: time),
                RecapSpan(kind: "text", text: "."),
            ],
            summary: RecapSummary(prompts: prompts, threads: 9, turns: prompts + 4, agentSecs: secs, agentTime: time, tokens: tokens, failed: 1,
                                  topProject: ProjectShare(project: trek, prompts: prompts * 2 / 3, tokens: tokens / 2),
                                  bestModel: ModelShare(agent: claude, label: "Claude Opus 5.5", tokens: tokens * 77 / 100, turns: 14, share: 77)),
            profile: ElevationProfile(
                buckets: buckets, summit: summit, now: n * 3 / 4, nowAt: 0.75,
                line: summit.map { "Summit \(range == .today ? "at" : "on") \(buckets[$0].label)" } ?? "A flat trail so far",
                total: "\(prompts) prompts",
                ticks: range == .today ? [Tick(at: 0.25, label: "6 AM"), Tick(at: 0.5, label: "Noon"), Tick(at: 0.75, label: "6 PM")]
                    : [Tick(at: 0.1, label: "Sep 14"), Tick(at: 0.5, label: "Sep 21"), Tick(at: 0.9, label: "Oct 1")]),
            tiles: [
                StatTile(kind: .bestModel, label: "Your best model", figure: "Claude Opus 5.5", note: "77% of tokens · 14 turns", agent: claude),
                StatTile(kind: .workedMostOn, label: "You worked most on", figure: trek.name, note: "\(prompts * 2 / 3) prompts · 1.2M tokens", project: trek),
                StatTile(kind: .tokens, label: "You used", figure: String(format: "%.1fM tokens", Double(tokens) / 1e6),
                         note: "≈ $14.20 at API prices \(range == .today ? "today" : range == .week ? "this week" : "so far")", sparkline: spark),
                StatTile(kind: .agentTime, label: "Your agents worked for", figure: time, note: "1 turn failed \(range == .today ? "today" : range == .week ? "this week" : "so far")"),
                StatTile(kind: .planLeft, label: "Left on Claude Max", figure: "36%", note: "Weekly · Opus · resets in 4d", agent: claude, percent: 36, resetsAt: now + 96 * 3_600_000),
                StatTile(kind: .planLeft, label: "Left on ChatGPT Pro", figure: "9%", note: "5-hour limit · resets in 58m", agent: codex, percent: 9, resetsAt: now + 3_500_000),
            ])
    }

    // MARK: Notes

    static func notes() -> [Note] {
        [
            Note(id: "n-release", title: "0.3.5 release", body: "# 0.3.5 release\n\n- [x] Phone: usage and Basecamp\n- [ ] Changelog\n- [ ] Tag and publish\n\nAsk **Mara** about the <mark>pricing page</mark> copy.", modified: now - 35 * 60_000),
            Note(id: "n-ideas", title: "Ideas", body: "Ideas\n\n- Rate limit per account *and* per IP\n- Cache the dashboard query for 30 s\n- <span style=\"color: #ef4444\">Drop</span> the Docker image", modified: now - 20 * 3_600_000),
            Note(id: "n-groceries", title: "Groceries", body: "Groceries\n\n- [ ] Oat milk\n- [ ] Coffee beans\n- [x] Bread", modified: now - 3 * 86_400_000),
        ]
    }

    static func title(of body: String) -> String {
        let line = body.split(separator: "\n").map { $0.trimmingCharacters(in: CharacterSet(charactersIn: "#-*[] x").union(.whitespaces)) }.first { !$0.isEmpty }
        return line.map { String($0.prefix(80)) } ?? "Untitled"
    }

    /// A line without its markdown, as the Mac's list shows it (`trek_core::notes::plain`).
    static func plain(_ line: String) -> String {
        line.replacing(/^\s*(#+\s*|>\s?|[-*+]\s+\[[ xX]\]\s+|[-*+]\s+|\d+\.\s+)/, with: "")
            .replacing(/<[^>]*>/, with: "")
            .replacing(/[*_~`]/, with: "")
            .trimmingCharacters(in: .whitespaces)
    }

    static func summary(_ n: Note) -> NoteSummary {
        let preview = n.body.split(separator: "\n").map { plain(String($0)) }.filter { !$0.isEmpty }.dropFirst().prefix(3).joined(separator: " ")
        return NoteSummary(id: n.id, title: n.title, preview: preview, modified: n.modified)
    }

    // MARK: Git

    static func gitStatus(thread t: ThreadSummary?) -> GitStatus {
        let f = { (path: String, status: FileStatus, added: Int, removed: Int) in ChangedFile(path: path, status: status, added: added, removed: removed) }
        // A finished turn's files are still uncommitted, so their diffs can be shown.
        let turn = t.flatMap { changes(for: $0.id)?.files } ?? []
        guard let t, t.worktree else {
            return GitStatus(isRepo: true, branch: t?.branch ?? "main", defaultBranch: "main", ahead: 0, behind: 2, hasUpstream: true,
                             files: turn + [f("README.md", .modified, 12, 4), f("docs/phone.md", .untracked, 38, 0)],
                             canSwitch: t?.runState != .working,
                             switchBlocked: t?.runState == .working ? "A thread is working in this folder: switch once it's done." : nil)
        }
        let branch = t.branch ?? "trek/work"
        return GitStatus(isRepo: true, branch: branch, defaultBranch: "main", ahead: 2, behind: 0, hasUpstream: false,
                         files: turn.isEmpty ? [f("crates/trek-core/src/store.rs", .modified, 18, 4), f("crates/trek-app/src/sidebar.rs", .modified, 22, 6),
                                                f("crates/trek-core/tests/inbox.rs", .untracked, 46, 0)] : turn,
                         worktree: WorktreeStatus(branch: branch, base: t.base ?? "main", uncommitted: 2, unpushed: nil,
                                                  mergeBlocked: "2 files aren't committed yet. Commit or revert them first.", unmerged: 2, missing: false),
                         canSwitch: false,
                         switchBlocked: "This thread works in a worktree: \(branch) stays checked out there. Merge it into \(t.base ?? "main") instead.")
    }

    static func diff(_ path: String) -> GitDiff {
        GitDiff(path: path, diff: """
        diff --git a/\(path) b/\(path)
        --- a/\(path)
        +++ b/\(path)
        @@ -212,9 +212,14 @@ impl Thread {
             pub fn own_section(&self, now: i64) -> Section {
        -        if self.snoozed_until.is_some_and(|t| t > now) {
        -            return Section::Snoozed;
        -        }
        +        // A thread that needs the user raises its hand, snoozed or not.
        +        if self.needs_you() {
        +            return Section::Inbox;
        +        }
        +        if self.snoozed_until.is_some_and(|t| t > now) {
        +            return Section::Snoozed;
        +        }
                 if self.run_state == RunState::Working {
        """)
    }

    static let branches = GitBranches(current: "main", defaultBranch: "main", branches: ["main", "inbox/raise-hand", "remote/tailscale", "perf/transcript", "release/0.3"])

    // MARK: Commands and settings

    static func commands() -> [CommandInfo] {
        let c = { (name: String, description: String, kind: CommandKind, trek: Bool) in CommandInfo(name: name, description: description, kind: kind, trek: trek) }
        return [
            c("new", "Start a new thread in this project", .command, true),
            c("usage", "Show plan usage and reset times", .command, true),
            c("context", "Show how much of the context window is used", .command, true),
            c("cost", "Show this session's estimated cost", .command, true),
            c("model", "Show the model this thread uses", .command, true),
            c("permissions", "Show or change how much the agent asks first", .command, true),
            c("permissions supervised", "Ask before every edit and command", .command, true),
            c("permissions edits", "Apply file edits; ask before commands", .command, true),
            c("permissions auto", "Work on its own; check before risky actions", .command, true),
            c("permissions full", "No prompts and no sandbox", .command, true),
            c("consult", "Ask other models first: /consult sol high, opus max: your message", .command, true),
            c("restate", "Have the agent say back what you asked before it starts", .command, true),
            c("compact", "Clear the conversation but keep a summary in context", .command, false),
            c("review", "Review a pull request", .command, false),
            c("frontend-design", "Create distinctive, production-grade frontend interfaces", .skill, false),
            c("code-reviewer", "Reviews code for bugs, style and missed edge cases", .agent, false),
        ]
    }

    /// How the demo Mac answers one of Trek's own commands, as a notice.
    static func reply(to command: String) -> String? {
        switch command {
        case "usage": "**Claude Code** · Claude Max\n- 5-hour limit: 42% used, resets in 2h 10m\n- Weekly limit: 18% used, resets in 4d"
        case "context": "171K of 200K tokens in context (86%)."
        case "cost": "≈ $1.84 at API prices (included in your Claude Max plan)."
        case "model": "Claude Code · claude-opus-5-5"
        case "permissions", "access", "mode": "Hand-holding is **Auto-accept edits**. Switch with `/permissions supervised`, `/permissions edits`, `/permissions auto` or `/permissions full`."
        default: nil
        }
    }

    static func settings() -> MacSettings {
        MacSettings(defaultAgent: "claude-code", defaultModel: nil, defaultEffort: "high", defaultAccess: .autoAcceptEdits, followUp: .steer,
                    notifications: .bannerAndSound,
                    push: PushSettings(enabled: true, when: .away, server: "https://ntfy.sh", topic: "trek-q7Hx2VbK9pL3mN8RtY4wZ6cE1fJ5aD0s",
                                       topicUrl: "https://ntfy.sh/trek-q7Hx2VbK9pL3mN8RtY4wZ6cE1fJ5aD0s",
                                       subscribeUrl: "ntfy://ntfy.sh/trek-q7Hx2VbK9pL3mN8RtY4wZ6cE1fJ5aD0s"),
                    autoSettleDays: 3, theme: .system, fullAccess: true)
    }

    /// ntfy's links for a topic, as the Mac makes them.
    static func ntfyLinks(server: String, topic: String) -> (String?, String?) {
        guard !topic.isEmpty else { return (nil, nil) }
        let server = server.hasSuffix("/") ? String(server.dropLast()) : server
        let host = server.components(separatedBy: "://").last ?? server
        return ("\(server)/\(topic)", "ntfy://\(host)/\(topic)\(server.hasPrefix("http://") ? "?secure=false" : "")")
    }
}
