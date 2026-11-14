import SwiftUI

// What the Mac's composer and Settings gained, from here (BarsProtocol.swift): the failing-checks
// bar, "Continue on <account>", a switch held for the next message, the permission rules, and
// files marked viewed. Each change shows here at once; the Mac's own row or reply follows, and puts
// it back (saying why) when it refuses.

extension AppModel {
    // MARK: Failing checks

    func sendFailedChecks(_ tid: String) {
        let agent = thread(tid)?.agent.name ?? "the agent"
        threadAction(tid, .sendFailedChecks, done: "Sent to \(agent)")
    }

    func dismissFailedChecks(_ tid: String) {
        if let i = threads.firstIndex(where: { $0.id == tid }) {
            withAnimation(.snappy) {
                threads[i].checks = nil
                if threads[i].needs?.kind == .failed, threads[i].runState != .failed { threads[i].needs = nil }
            }
        }
        threadAction(tid, .dismissFailedChecks)
    }

    /// Whether `project` (qualified) sends its failing checks by itself, as its Mac's settings say;
    /// nil when that Mac doesn't say.
    func sendsFailedChecks(_ project: String) -> Bool? {
        guard let link = link(for: project), let on = link.macSettings?.sendFailedChecks else { return nil }
        return on.contains(link.bare(project))
    }

    /// Turn `project`'s (qualified) sending failing checks by itself on or off.
    func setSendsFailedChecks(_ project: String, _ on: Bool) {
        guard let link = link(for: project) else { return }
        for i in threads.indices where threads[i].project?.id == project { threads[i].checks?.auto = on }
        changeSettings(SettingsChange(sendFailedChecks: ProjectSwitch(projectId: link.bare(project), on: on)), on: link)
    }

    // MARK: Continue on another account

    func continueOnAccount(_ tid: String, _ other: OtherAccount) {
        threadAction(tid, .continueOnAccount(other.account), done: "Carrying on with \(other.label)")
    }

    // MARK: A switch held for the next message

    /// Pick another agent as the Mac's composer does: it waits for the next message.
    func holdSwitch(_ tid: String, agent key: String, model modelID: String?) {
        let before = thread(tid)
        if let i = threads.firstIndex(where: { $0.id == tid }), let a = link(for: tid)?.agents.first(where: { $0.key == key }) {
            let id = modelID ?? a.defaultModel
            threads[i].pendingSwitch = PendingSwitch(agent: a.ref, model: id, modelLabel: a.models.first { $0.id == id }?.label,
                                                     to: a.name, from: threads[i].agent.name)
        }
        request(.holdSwitch(threadId: tid, agent: key, model: modelID)) { [weak self] reply in
            guard case .error(_, _, let text) = reply, let self else { return }
            self.show(text, error: true)
            if let before, let i = self.threads.firstIndex(where: { $0.id == tid }) { self.threads[i] = before }
        }
    }

    func stayOnAgent(_ tid: String) {
        if let i = threads.firstIndex(where: { $0.id == tid }) { withAnimation(.snappy) { threads[i].pendingSwitch = nil } }
        threadAction(tid, .stayOnAgent)
    }

    func loadSwitchPreview(_ tid: String, done: @escaping (SwitchPreview) -> Void) {
        request(.switchPreview(threadId: tid)) { [weak self] reply in
            switch reply {
            case .switchPreview(_, let p): done(p)
            case .error(_, _, let text): self?.show(text, error: true)
            default: break
            }
        }
    }

    // MARK: Permission rules

    func loadRules(on link: HostLink?, done: @escaping (RulesList) -> Void) {
        guard let link = link ?? selected else { return }
        request(.rules, on: link) { [weak self] reply in
            switch reply {
            case .rules(_, let list): done(list)
            case .error(_, _, let text): self?.show(text, error: true)
            default: break
            }
        }
    }

    /// Add or remove a rule; `done` gets the list as it is after (or nil when the Mac refused).
    func changeRule(_ change: RuleChange, on link: HostLink?, done: @escaping (RulesList?) -> Void) {
        guard let link = link ?? selected else { return }
        request(.ruleAction(change), on: link) { [weak self] reply in
            switch reply {
            case .rules(_, let list): done(list)
            case .error(_, _, let text):
                self?.show(text, error: true)
                done(nil)
            default: break
            }
        }
    }

    func testRule(_ kind: RuleKind, _ text: String, on link: HostLink?, done: @escaping (RuleTest) -> Void) {
        guard let link = link ?? selected else { return }
        request(.ruleTest(kind: kind, text: text), on: link) { reply in
            if case .ruleTest(_, let t) = reply { done(t) }
        }
    }

    /// "Add rule and allow": `done(true)` once the card is allowed and the rule kept.
    func alwaysAllow(_ tid: String, requestId: String, pattern: String, done: @escaping (Bool) -> Void) {
        request(.alwaysAllow(threadId: tid, requestId: requestId, pattern: pattern)) { [weak self] reply in
            if case .error(_, _, let text) = reply {
                self?.show(text, error: true)
                done(false)
            } else {
                self?.show("Rule added. It runs without asking from now on.")
                done(true)
            }
        }
    }

    // MARK: Files marked viewed

    /// Whether the machine `target` is on serves `feature`.
    func serves(_ feature: MacFeature, for target: GitTarget) -> Bool { serves(feature, on: target.id) }

    /// Tick `path` viewed in `target`'s review, or not: the Mac keeps the mark with its diff, and
    /// answers with the folder's status.
    func setViewed(_ target: GitTarget, path: String, _ viewed: Bool) {
        if var s = gitStatus[target] {
            var marked = s.viewed ?? []
            marked.removeAll { $0 == path }
            if viewed { marked.append(path) }
            s.viewed = marked
            gitStatus[target] = s
        }
        request(.setViewed(target, path: path, viewed: viewed)) { [weak self] reply in
            switch reply {
            case .gitStatus(_, let s): self?.gitStatus[target] = s
            case .error(_, _, let text):
                self?.show(text, error: true)
                self?.loadGitStatus(target)
            default: break
            }
        }
    }
}
