import Foundation
import Testing
@testable import Trek

/// The Threads page as the Mac's sidebar lays it out: what waits on you and what's at work have
/// places of their own, projects hold the quiet threads, and a project (or "No project") is one
/// section however many machines it's on. Seen twice over on an iPhone with a Mac and a PC.
@MainActor
struct ThreadsHomeTests {
    /// A Mac and a PC, as the demo pairs them.
    private func model() -> AppModel {
        let model = AppModel()
        model.links = [HostLink(paired: .demo, backend: MockHost()),
                       HostLink(paired: .demoPC, backend: MockHost(name: "T-Book", id: "demo-pc", platform: "windows"))]
        return model
    }

    /// A row as it is once qualified with its host: its id and its project's both carry the prefix.
    private func thread(_ id: String, on host: String, project: (id: String, name: String)?, state: RunState = .idle,
                        needs: Needs? = nil, section: ThreadSection = .inbox, unseen: Bool = false) -> ThreadSummary {
        ThreadSummary(id: "\(host):\(id)", title: id,
                      project: project.map { ProjectRef(id: "\(host):\($0.id)", name: $0.name, hue: 24, monogram: "TR") },
                      agent: AgentRef(key: "claude-code", name: "Claude Code"), model: nil, modelLabel: "Opus 5.5", runState: state,
                      needs: needs, section: section, unseen: unseen, pinned: section == .pinned, branch: "main", worktree: false,
                      activity: state == .working ? "Reading store.rs" : nil, workingSince: state == .working ? 1_800_000_000_000 : nil,
                      updatedAt: 1_800_000_000_000, additions: 0, deletions: 0)
    }

    @Test func noProjectIsOneSectionWhateverTheMachine() {
        // Every Mac sends a thread in no project with the project id "", which the phone
        // qualifies per host: "demo:" and "demo-pc:" were two "No project" sections.
        let model = model()
        let groups = model.quietGroups(of: [thread("a", on: "demo", project: ("", "No project")),
                                            thread("b", on: "demo-pc", project: ("", "No project")),
                                            thread("c", on: "demo", project: nil)])
        #expect(groups.count == 1)
        #expect(groups.first?.id == "")
        #expect(groups.first?.name == "No project")
        #expect(groups.first?.count == 3)
        #expect(groups.first?.project?.id == "", "no one machine's id on the merged section")
    }

    @Test func aProjectOnTwoMachinesIsOneSection() {
        let model = model()
        let groups = model.quietGroups(of: [thread("a", on: "demo", project: ("p-trek", "trek")),
                                            thread("b", on: "demo-pc", project: ("p-trek", "trek")),
                                            thread("c", on: "demo", project: ("p-atlas", "atlas-web"))])
        #expect(groups.map(\.name).sorted() == ["atlas-web", "trek"])
        #expect(groups.first { $0.name == "trek" }?.count == 2)
        // The fold and "Show more" keep to the section, whichever machine's row is asked about.
        let a = thread("a", on: "demo", project: ("p-trek", "trek")), b = thread("b", on: "demo-pc", project: ("p-trek", "Trek"))
        #expect(model.groupKey(of: a) == model.groupKey(of: b))
    }

    @Test func settledHistoryIsGroupedTheSameWay() {
        let model = model()
        let groups = model.settledGroups(of: [thread("a", on: "demo", project: ("", "No project"), section: .settled),
                                              thread("b", on: "demo-pc", project: ("", "No project"), section: .settled)])
        #expect(groups.count == 1)
    }

    @Test func whatWaitsAndWhatWorksLeaveTheProjects() {
        let model = model()
        let asks = thread("asks", on: "demo", project: ("p-trek", "trek"), state: .needsYou,
                          needs: Needs(kind: .approval, text: "Run cargo test"))
        let works = thread("works", on: "demo-pc", project: ("p-trek", "trek"), state: .working, section: .working)
        let quiet = thread("quiet", on: "demo", project: ("p-trek", "trek"))
        let pinned = thread("pinned", on: "demo", project: ("p-trek", "trek"), state: .working, section: .pinned)
        let list = [asks, works, quiet, pinned]
        #expect(model.active(of: list).map(\.id) == ["demo-pc:works"], "not what asks (Needs you), not a pinned thread")
        #expect(model.quietGroups(of: list).flatMap { $0.active + $0.quiet }.map(\.id) == ["demo:quiet"])
    }

    @Test func theReasonGivesWayFirstAndGoesWhenItWouldSayNothing() {
        // Words, why, where: whole when they fit.
        #expect(StandingLayout.widths(available: 300, ideal: [100, 80, 60], leastReason: 56) == [100, 80, 60])
        // Why is cut to what's left…
        #expect(StandingLayout.widths(available: 220, ideal: [100, 80, 60], leastReason: 56) == [100, 60, 60])
        // …and left out under the least (it read "Working 1m 13s ·… · atlas-web").
        #expect(StandingLayout.widths(available: 200, ideal: [100, 80, 60], leastReason: 56) == [100, 0, 60])
        // Where is cut only once why has gone.
        #expect(StandingLayout.widths(available: 150, ideal: [100, 80, 120], leastReason: 56) == [100, 0, 50])
    }

    @Test func eachRowSaysWhereItStandsOnce() {
        let quiet = RowState.of(thread("q", on: "demo", project: nil))
        #expect(!quiet.says && quiet.mark == .none, "one line, an empty column")
        let unread = RowState.of(thread("u", on: "demo", project: nil, unseen: true))
        #expect(!unread.says && unread.mark == .dot && unread.spoken == "Unread")
        let working = RowState.of(thread("w", on: "demo", project: nil, state: .working))
        #expect(working.words == "Working" && working.mark == .beacon)
        let asks = RowState.of(thread("a", on: "demo", project: nil, state: .needsYou, needs: Needs(kind: .approval, text: "Run it")))
        #expect(asks.words == "Approval" && asks.mark == .dot)
        let failed = RowState.of(thread("f", on: "demo", project: nil, state: .failed, needs: Needs(kind: .failed, text: "Build failed")))
        #expect(failed.words == "Failed")
        var waiting = thread("wt", on: "demo", project: nil)
        waiting.busyInBackground = true
        #expect(RowState.of(waiting).words == "Waiting")
        var running = thread("r", on: "demo", project: nil)
        running.busyInBackground = false
        running.background = ["pnpm dev"]
        #expect(RowState.of(running).words == "Running" && RowState.of(running).mark == .dot)
        let snoozed = RowState.of(thread("s", on: "demo", project: nil, section: .snoozed))
        #expect(!snoozed.says && snoozed.mark == .snoozed)
        let pinned = RowState.of(thread("p", on: "demo", project: nil, section: .pinned))
        #expect(!pinned.says && pinned.mark == .pin)
    }
}
