import Foundation

/// The demo Mac's live cards (a pull request, a CI run, a release), in every state the Mac draws
/// them in: the same contents as the Figma frames on "Phone · cards", so the two can be put side
/// by side. `-TrekDemoCard <state>` holds the `t-card` thread's card in one state; without it the
/// card moves through them by itself, as a real one does.
extension DemoMac {
    /// Every state, in the order the live demo plays them (each family from its start).
    static let cardStates = [
        "pr-creating", "pr-checking", "pr-conflicted", "pr-resolving", "pr-ready", "pr-merging", "pr-merged",
        "run-running", "run-passed", "run-failed",
        "release-building", "release-publishing", "release-published", "release-failed",
    ]

    /// `-TrekDemoCard <state>`.
    static var heldCard: String? { UserDefaults.standard.string(forKey: "TrekDemoCard").flatMap { cardStates.contains($0) ? $0 : nil } }

    /// The card in `state`, as a real Mac would send it then.
    static func card(_ state: String, now: Int64 = DemoMac.now) -> RemoteCard {
        let m: Int64 = 60_000, s: Int64 = 1000
        var pr = RemoteCard()
        pr.key = "https://github.com/dokyit/Trek/pull/128"
        pr.kind = .pr
        pr.title = "Refresh sessions in the auth middleware"
        pr.number = 128
        pr.url = pr.key
        pr.head = "feat/session-refresh"
        pr.base = "main"
        pr.repo = "dokyit/Trek"
        pr.commits = 3
        pr.history = [
            CardCommit(title: "Add session refresh tests", oid: "d29f50", author: "You", committedMs: now - 20 * s),
            CardCommit(title: "Refresh sessions near expiry", oid: "c70a92", author: "You", committedMs: now - 25 * m),
            CardCommit(title: "Read session expiry from the token", oid: "1f8e3b", author: "You", committedMs: now - 40 * m),
        ]
        pr.added = 128
        pr.removed = 36
        pr.createdMs = now - 5 * m
        pr.phaseAtMs = now
        let passed = ["build (macos)", "build (windows)", "clippy"].map { CardCheck(name: $0, state: .pass) }
        let conflicts = ["src/middleware.ts", "src/auth.ts"]

        var run = RemoteCard()
        run.key = "run-39779843418"
        run.kind = .run
        run.title = "CI"
        run.number = 39_779_843_418
        run.url = "https://github.com/dokyit/Trek/actions/runs/39779843418"
        run.head = "main"
        run.subject = "Side chat and AI side bar"
        run.event = "push"
        run.createdMs = now - 24 * m
        run.phaseAtMs = now
        func job(_ label: String, _ state: CheckState, took: Int64?, started: Int64) -> CardStage {
            CardStage(label: label, state: state, startedMs: started, endedMs: took.map { started + $0 })
        }

        var rel = RemoteCard()
        rel.key = "release-0.4.28-nightly.20261030"
        rel.kind = .release
        rel.title = "Release v0.4.28-nightly.20261030"
        rel.version = "0.4.28-nightly.20261030"
        rel.channel = "nightly"
        rel.head = "main"
        rel.url = "https://github.com/dokyit/Trek/actions/runs/39780000001"
        rel.createdMs = now - 14 * m
        rel.phaseAtMs = now

        switch state {
        case "pr-creating":
            pr.phase = .creating
            pr.number = nil
            pr.url = nil
            pr.commits = nil
            pr.added = nil
            pr.removed = nil
            pr.history = []
            pr.step = "Pushing 3 commits to origin"
            return pr
        case "pr-checking":
            pr.phase = .checking
            pr.checks = [CardCheck(name: "build (macos)", state: .pass), CardCheck(name: "build (windows)", state: .running),
                         CardCheck(name: "clippy", state: .queued)]
            return pr
        case "pr-conflicted":
            pr.phase = .conflicted
            pr.checks = passed
            pr.conflicts = conflicts
            pr.actions = [.resolve]
            return pr
        case "pr-resolving":
            pr.phase = .conflicted
            pr.checks = passed
            pr.conflicts = ["src/auth.ts"]
            pr.resolved = ["src/middleware.ts"]
            pr.resolving = true
            pr.step = "Resolving the conflicts"
            return pr
        case "pr-ready":
            pr.phase = .ready
            pr.checks = passed
            pr.actions = [.merge]
            return pr
        case "pr-merging":
            pr.phase = .merging
            pr.merging = true
            pr.checks = passed
            pr.step = "Merging the pull request"
            return pr
        case "pr-merged":
            pr.phase = .merged
            pr.checks = passed
            pr.branchDeleted = true
            return pr
        case "run-running":
            run.phase = .checking
            run.startedMs = now - 21 * m - 5 * s
            run.stages = [job("Clippy", .pass, took: 4 * m + 10 * s, started: run.startedMs),
                          job("Test (macOS)", .pass, took: 18 * m + 22 * s, started: run.startedMs),
                          job("Test (Windows)", .running, took: nil, started: run.startedMs)]
            return run
        case "run-passed":
            run.phase = .passed
            run.startedMs = now - 23 * m - 40 * s
            run.endedMs = now
            run.stages = [job("Clippy", .pass, took: 4 * m + 10 * s, started: run.startedMs),
                          job("Test (macOS)", .pass, took: 18 * m + 22 * s, started: run.startedMs),
                          job("Test (Windows)", .pass, took: 23 * m + 31 * s, started: run.startedMs)]
            return run
        case "run-failed":
            run.phase = .failed
            run.startedMs = now - 24 * m - 10 * s
            run.endedMs = now
            run.step = "2 of 214 tests failed in trek-remote"
            run.stages = [job("Clippy", .pass, took: 4 * m + 10 * s, started: run.startedMs),
                          job("Test (macOS)", .pass, took: 18 * m + 22 * s, started: run.startedMs),
                          job("Test (Windows)", .fail, took: 24 * m + 2 * s, started: run.startedMs)]
            run.actions = [.rerun]
            return run
        case "release-building":
            rel.phase = .releasing
            rel.startedMs = now - 9 * m - 40 * s
            rel.stages = [job("macOS app", .pass, took: 9 * m + 12 * s, started: rel.startedMs),
                          job("Windows build", .running, took: nil, started: rel.startedMs),
                          CardStage(label: "Publish", state: .queued)]
            return rel
        case "release-publishing":
            rel.phase = .releasing
            rel.startedMs = now - 12 * m - 21 * s
            rel.stages = [job("macOS app", .pass, took: 9 * m + 12 * s, started: rel.startedMs),
                          job("Windows build", .pass, took: 11 * m + 40 * s, started: rel.startedMs),
                          job("Publish", .running, took: nil, started: now - 40 * s)]
            return rel
        case "release-published":
            rel.phase = .published
            rel.startedMs = now - 13 * m - 11 * s
            rel.phaseAtMs = now
            rel.stages = [job("macOS app", .pass, took: 9 * m + 12 * s, started: rel.startedMs),
                          job("Windows build", .pass, took: 11 * m + 40 * s, started: rel.startedMs),
                          job("Publish", .pass, took: 1 * m + 31 * s, started: rel.startedMs + 11 * m + 40 * s)]
            rel.notesUrl = "https://github.com/dokyit/Trek/releases/tag/v0.4.28-nightly.20261030"
            return rel
        default: // "release-failed"
            rel.phase = .failed
            rel.startedMs = now - 11 * m - 52 * s
            rel.phaseAtMs = now
            rel.step = "Release workflow failed"
            rel.stages = [job("macOS app", .pass, took: 9 * m + 12 * s, started: rel.startedMs),
                          job("Windows build", .fail, took: 11 * m + 40 * s, started: rel.startedMs),
                          CardStage(label: "Publish", state: .skipped)]
            rel.actions = [.rerun]
            return rel
        }
    }

    /// The state a card moves on to by itself in the live demo (nil: it stays), and after how long.
    static func nextCard(after state: String) -> (String, Double)? {
        switch state {
        case "pr-creating": ("pr-checking", 2.5)
        case "pr-checking": ("pr-conflicted", 3)
        case "pr-resolving": ("pr-ready", 4)
        case "pr-merging": ("pr-merged", 2)
        case "pr-merged": ("run-running", 5)
        case "run-running": ("run-failed", 4)
        case "run-passed": ("release-building", 5)
        case "release-building": ("release-publishing", 4)
        case "release-publishing": ("release-published", 3)
        default: nil
        }
    }

    /// What a card's button does to it in the demo.
    static func cardAfter(_ action: CardAction, on state: String) -> String? {
        switch (action, state) {
        case (.resolve, "pr-conflicted"): "pr-resolving"
        case (.merge, "pr-ready"): "pr-merging"
        case (.rerun, "run-failed"): "run-running"
        case (.rerun, "release-failed"): "release-building"
        default: nil
        }
    }
}
