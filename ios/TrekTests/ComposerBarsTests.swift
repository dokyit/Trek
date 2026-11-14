import Foundation
import Testing
@testable import Trek

/// What the Mac's composer and Settings gained, on the phone: the failing-checks bar, "Continue on
/// <account>", the Compact chip, a switch held for the next message, the permission rules and files
/// marked viewed. The wire as docs/MOBILE.md writes it, what goes back, and the logic that decides
/// what shows.
@MainActor
struct ComposerBarsTests {
    private static func row(_ extra: String) -> String {
        #"{"id":"t","title":"Fix the login","project":{"id":"p","name":"web","hue":10,"monogram":"WE"},"agent":{"key":"claude-code","name":"Claude Code"},"run_state":"idle","section":"inbox","unseen":false,"pinned":false,"worktree":false,"updated_at":1,"additions":0,"deletions":0"#
            + extra + "}"
    }

    private func summary(_ extra: String) throws -> ThreadSummary {
        try ServerMessage.decoder.decode(ThreadSummary.self, from: Data(Self.row(extra).utf8))
    }

    private func decode(_ json: String) throws -> ServerMessage {
        try ServerMessage.decode(Data(json.utf8))
    }

    private func object(_ m: ClientMessage) throws -> [String: Any] {
        try #require(JSONSerialization.jsonObject(with: m.encoded(id: "1")) as? [String: Any])
    }

    // MARK: Failing checks

    @Test func aFailingPullRequestRidesOnItsRowAndShowsAsCIFailed() throws {
        let t = try summary(#","needs":{"kind":"failed","text":"CI failed: clippy and 1 more"},"checks":{"url":"https://github.com/acme/web/pull/128","number":128,"failing":["clippy","test (macos)"],"bar":true,"newer_mac_field":1}"#)
        let checks = try #require(t.checks)
        #expect(checks.failing == ["clippy", "test (macos)"])
        #expect(checks.bar && !checks.stopped && !checks.sending && !checks.auto)
        #expect(checks.title == "CI failed")
        #expect(checks.detail == "#128: clippy, test (macos)")
        // The row's state says what failed: CI, not its turn.
        #expect(StatusLook.of(t)?.label == "CI failed")
        var turnFailed = t
        turnFailed.runState = .failed
        #expect(StatusLook.of(turnFailed)?.label == "Failed")
        // An older Mac's row: no bar, a failure as before.
        #expect(try summary(#","needs":{"kind":"failed","text":"Build failed"}"#).checks == nil)
        let stopped = FailingChecks(url: "u", number: 3, failing: ["lint"], stopped: true)
        #expect(stopped.title == "CI still failing")
        #expect(stopped.detail == "#3: lint. Trek stopped sending it after 2 tries.")
    }

    @Test func theBarsButtonsAndTheProjectSwitchGoToTheMac() throws {
        let send = try object(.threadAction(threadId: "t", action: .sendFailedChecks))
        #expect((send["action"] as? [String: Any])?["kind"] as? String == "send_failed_checks")
        let dismiss = try object(.threadAction(threadId: "t", action: .dismissFailedChecks))
        #expect((dismiss["action"] as? [String: Any])?["kind"] as? String == "dismiss_failed_checks")
        let on = try object(.setSettings(SettingsChange(sendFailedChecks: ProjectSwitch(projectId: "p", on: true))))
        #expect((on["send_failed_checks"] as? [String: Any])?["project_id"] as? String == "p")
        #expect((on["send_failed_checks"] as? [String: Any])?["on"] as? Bool == true)
        // Shown at once, as the Mac will answer.
        var s = DemoMac.settings()
        s.sendFailedChecks = ["q"]
        let after = SettingsChange(sendFailedChecks: ProjectSwitch(projectId: "p", on: true)).applied(to: s)
        #expect(after.sendFailedChecks == ["q", "p"])
        #expect(SettingsChange(sendFailedChecks: ProjectSwitch(projectId: "q", on: false)).applied(to: after).sendFailedChecks == ["p"])
    }

    // MARK: Continue on another account

    @Test func theLimitBarNamesAnotherAccount() throws {
        let t = try summary(#","pause":{"resets_at":1791030000000,"resume":false,"wrapped":false,"other_account":{"account":"acct-2","label":"Work"}}"#)
        #expect(t.pause?.otherAccount == OtherAccount(account: "acct-2", label: "Work"))
        let go = try object(.threadAction(threadId: "t", action: .continueOnAccount("acct-2")))
        #expect((go["action"] as? [String: Any])?["account"] as? String == "acct-2")
        let login = try object(.threadAction(threadId: "t", action: .continueOnAccount(nil)))
        #expect((login["action"] as? [String: Any])?.keys.sorted() == ["kind"], "the normal login has no id")
        #expect(try summary(#","pause":{"resets_at":null,"resume":false,"wrapped":false}"#).pause?.otherAccount == nil)
    }

    // MARK: The Compact chip

    @Test func theCompactChipShowsOnceTheCacheGoesUntilDismissed() throws {
        let t = try summary(#","compact_offer":{"used":90000,"window":200000,"last_active":1791000000000,"lapses_at":1791000300000,"why":"The prompt cache has expired."}"#)
        let offer = try #require(t.compactOffer)
        let before = Date(timeIntervalSince1970: 1_791_000_299)
        let after = Date(timeIntervalSince1970: 1_791_000_301)
        #expect(!offer.shows(at: before, dismissed: nil), "the cache is still warm")
        #expect(offer.shows(at: after, dismissed: nil))
        #expect(!offer.shows(at: after, dismissed: offer.lastActive), "dismissed")
        #expect(offer.shows(at: after, dismissed: offer.lastActive - 1), "active again since")
        #expect(try summary("").compactOffer == nil)
    }

    // MARK: A switch held for the next message

    @Test func pickingAnotherAgentHoldsItForTheNextMessage() throws {
        let hold = try object(.holdSwitch(threadId: "t", agent: "codex", model: "gpt-5.6-sol"))
        #expect(hold["type"] as? String == "set_prefs")
        #expect(hold["hold"] as? Bool == true && hold["agent"] as? String == "codex" && hold["model"] as? String == "gpt-5.6-sol")
        let t = try summary(#","pending_switch":{"agent":{"key":"codex","name":"Codex","logo":"codex"},"model":"gpt-5.6-sol","model_label":"GPT-5.6 Sol","effort":"high","to":"Codex","from":"Claude Code"}"#)
        let p = try #require(t.pendingSwitch)
        #expect((p.to, p.from, p.modelLabel) == ("Codex", "Claude Code", "GPT-5.6 Sol"))
        #expect(PendingSwitch.sentence(to: "Codex", running: false, compact: false)
                == ("Your next message switches to Codex.", "The conversation so far goes with it."))
        #expect(PendingSwitch.sentence(to: "Codex", running: true, compact: false).rest
                == "It stops this turn, and the conversation so far goes with it.")
        #expect(PendingSwitch.sentence(to: "Codex", running: true, compact: true) == ("Next message stops this and switches to Codex", nil))
        let stay = try object(.threadAction(threadId: "t", action: .stayOnAgent))
        #expect((stay["action"] as? [String: Any])?["kind"] as? String == "stay_on_agent")
        guard case .switchPreview(let re, let preview) = try decode(#"{"type":"switch_preview","re":"11","thread_id":"t","text":"You're picking up a conversation from Claude Code.","tokens":3100,"budget":24000,"left_out":"Left out: 2 tool outputs."}"#) else {
            Issue.record("not a preview")
            return
        }
        #expect(re == "11" && preview.tokens == 3100 && preview.leftOut == "Left out: 2 tool outputs.")
        #expect(try object(.switchPreview(threadId: "t"))["type"] as? String == "switch_preview")
    }

    /// The phone qualifies ids for several Macs: what goes back is the bare id.
    @Test func whatGoesBackCarriesTheMacsOwnIds() throws {
        let backend = RecordingBackend()
        let model = AppModel()
        let link = HostLink(paired: .demo, backend: backend)
        model.links = [link]
        #expect(model.route(.holdSwitch(threadId: link.qualified("t"), agent: "codex", model: nil)) === link)
        #expect(model.route(.setViewed(.project(link.qualified("p")), path: "a", viewed: true)) === link)
        #expect(model.route(.alwaysAllow(threadId: link.qualified("t"), requestId: "r", pattern: "x")) === link)
        model.request(.holdSwitch(threadId: link.qualified("t"), agent: "codex", model: nil))
        model.request(.setViewed(.thread(link.qualified("t")), path: "a.rs", viewed: false))
        let sent = backend.sent.map { $0.message.json(id: nil) }
        #expect(sent.map { $0["thread_id"] as? String } == ["t", "t"])
    }

    // MARK: Permission rules

    @Test func theRulesAreReadTriedAndWritten() throws {
        guard case .rules(_, let list) = try decode(#"{"type":"rules","re":"3","rules":[{"kind":"command","pattern":"rm","action":"deny"},{"kind":"newer_kind","pattern":"x","action":"allow"}],"editable":false}"#) else {
            Issue.record("not the rules")
            return
        }
        #expect(list.rules.first == Rule(kind: .command, pattern: "rm", action: .deny))
        #expect(list.rules.first?.shown == "Deny command “rm”")
        #expect(list.rules.count == 2, "a kind it doesn't know still reads")
        #expect(!list.editable)
        let add = try object(.ruleAction(.add(Rule(kind: .edit, pattern: "~/.ssh/**", action: .deny))))
        #expect(add["type"] as? String == "rule_action" && add["action"] as? String == "add")
        #expect((add["rule"] as? [String: Any])?["kind"] as? String == "edit")
        let remove = try object(.ruleAction(.remove(index: 0, Rule(kind: .command, pattern: "rm", action: .deny))))
        #expect(remove["index"] as? Int == 0)
        guard case .ruleTest(_, let tried) = try decode(#"{"type":"rule_test","re":"6","kind":"command","text":"rm -rf build; echo done","decided_by":"Deny command “rm” decides.","rule":0,"levels":[{"level":"supervised","outcome":"refused"},{"level":"full-access","outcome":"refused"}]}"#) else {
            Issue.record("not a test")
            return
        }
        #expect(tried.rule == 0 && tried.levels.count == 2 && tried.levels[1].level == .fullAccess)
        #expect(RuleTest(kind: .command, text: "ls").said.contains("level does"))
        let always = try object(.alwaysAllow(threadId: "t", requestId: "perm-3", pattern: "cargo test"))
        #expect(always["type"] as? String == "always_allow" && always["pattern"] as? String == "cargo test")
    }

    /// "Always allow…" is offered only where the Mac lets this phone write rules.
    @Test func alwaysAllowIsTheMacsToOffer() throws {
        let item = try ServerMessage.decoder.decode(TItem.self, from: Data(#"{"id":"r3","seq":9,"at":null,"kind":"approval","request_id":"perm-3","title":"Run command","detail":"cargo test --workspace","state":"pending","always_allow":{"kind":"command","pattern":"cargo test"}}"#.utf8))
        guard case .approval(let a) = item.body else {
            Issue.record("not an approval")
            return
        }
        #expect(a.alwaysAllow == RuleDraft(kind: .command, pattern: "cargo test"))
        #expect(ApprovalCard.offersRule(a, served: true, macAllows: true))
        #expect(!ApprovalCard.offersRule(a, served: true, macAllows: false), "off on the Mac")
        #expect(!ApprovalCard.offersRule(a, served: true, macAllows: nil), "a Mac that doesn't say")
        #expect(!ApprovalCard.offersRule(a, served: false, macAllows: true))
        var asked = a
        asked.askedBy = "Ask command “cargo”"
        #expect(!ApprovalCard.offersRule(asked, served: true, macAllows: true), "a rule asks every time")
        // The transcript cache keeps the draft.
        let e = JSONEncoder()
        e.keyEncodingStrategy = .convertToSnakeCase
        #expect(try ServerMessage.decoder.decode(TItem.self, from: e.encode(item)) == item)
        // An older Mac's settings say nothing, and so nothing is offered.
        let s = try ServerMessage.decoder.decode(MacSettings.self, from: Data(#"{"default_agent":"codex","default_effort":"high","default_access":"supervised","follow_up":"steer","notifications":"banner","push":{"enabled":false,"when":"away","server":"https://ntfy.sh","topic":""},"auto_settle_days":3,"theme":"system"}"#.utf8))
        #expect(s.phoneRules == nil && s.sendFailedChecks == nil)
    }

    // MARK: Files marked viewed

    @Test func filesMarkedViewedComeWithTheStatus() throws {
        guard case .gitStatus(_, let s) = try decode(#"{"type":"git_status","re":"13","is_repo":true,"branch":"main","ahead":0,"behind":0,"has_upstream":false,"files":[{"path":"README.md","status":"modified","added":1,"removed":0},{"path":"new.txt","status":"untracked","added":2,"removed":0}],"can_switch":true,"viewed":["README.md"]}"#) else {
            Issue.record("not a status")
            return
        }
        #expect(s.isViewed("README.md") && !s.isViewed("new.txt"))
        #expect(GitStatus.viewedLabel(1, of: 2) == "1 of 2 viewed")
        #expect(GitStatus.viewedLabel(0, of: 2) == nil)
        let mark = try object(.setViewed(.project("p"), path: "README.md", viewed: true))
        #expect(mark["type"] as? String == "set_viewed" && mark["project_id"] as? String == "p" && mark["viewed"] as? Bool == true)
        #expect(GitStatus(files: []).isViewed("a") == false, "an older Mac's status")
    }

    // MARK: The demo Mac

    @Test func theDemoMacServesThemAll() {
        for f in ["failed_checks", "rules", "continue_on_account", "compact_offer", "switch_preview", "diff_viewed"] {
            #expect(MockHost.features.contains(f), "\(f)")
        }
        let tried = MockHost.test(.command, "rm -rf build; echo done")
        #expect(tried.decidedBy?.contains("rm -rf build") == true)
        #expect(tried.levels.allSatisfy { $0.outcome == .refused })
        #expect(MockHost.test(.command, "ls").decidedBy == nil)
    }
}
