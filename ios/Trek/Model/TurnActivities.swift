import ActivityKit
import Foundation
import UIKit
import os

private let activityLog = Logger(subsystem: "dev.trek.TrekMobile", category: "activity")

/// What the Live Activity shows for the threads as they are: what asks for the user first, then
/// the turn started last. Nil: nothing to show, and it ends.
///
/// Only what's moving or blocked holds it up. A failure that was never settled, a thread paused at
/// a limit or a dev server left running could keep it on the Lock Screen for days: they're only
/// counted in what's under it. So is anything snoozed, which the user put away on purpose.
enum TurnActivityPick {
    typealias State = TurnActivityAttributes.ContentState

    /// An approval nobody answered for this long no longer holds it up.
    static let asksFor: TimeInterval = 12 * 3600
    private static let longestTitle = 80
    private static let longestLine = 140

    static func state(threads: [ThreadSummary], now: Date, split: (String) -> (host: String, bare: String)) -> State? {
        let awake = threads.filter { $0.section != .snoozed && $0.parentId == nil }
        let asks = awake.filter { t in
            guard let kind = t.needs?.kind, kind == .approval || kind == .question || kind == .plan else { return false }
            return now.timeIntervalSince1970 - Double(t.updatedAt) / 1000 < asksFor
        }.sorted { ($0.updatedAt, $0.id) > ($1.updatedAt, $1.id) }
        let working = awake.filter { $0.atWork && !$0.waitsOnYou }
            .sorted { ($0.workingSince ?? 0, $0.id) > ($1.workingSince ?? 0, $1.id) }
        guard let t = asks.first ?? working.first else { return nil }
        let (host, bare) = split(t.id)
        var state = State(kind: asks.first != nil ? .asking : t.runState == .working ? .working : .waiting,
                          title: clip(t.title, longestTitle), project: t.project?.name, agent: t.agent.name, line: "",
                          since: t.workingSince.map { Date(timeIntervalSince1970: Double($0) / 1000) },
                          threadID: t.id, hostID: host, bareID: bare,
                          waitingOnYou: awake.count(where: \.waitsOnYou), atWork: awake.count(where: \.atWork),
                          inBackground: awake.count(where: \.onlyRunsInBackground), requestID: nil, warning: nil, heard: now)
        switch state.kind {
        case .asking:
            state.line = clip(plain(t.needs?.text ?? "Needs you"), longestLine)
            // A request of a sub-agent's is answered in its own thread. What looks destructive is
            // read in full first, as the card in the thread asks; its first line alone won't do.
            if t.needs?.kind == .approval, t.needsThread == nil, let ask = t.ask {
                if let warning = CommandRisk.warning(for: ask.detail) {
                    state.warning = warning
                } else {
                    state.requestID = ask.requestId
                }
            }
        case .working:
            state.line = clip(t.activity.map(plain) ?? "Working", longestLine)
        case .waiting:
            state.line = "Waiting on what it started"
        case .finished:
            break
        }
        return state
    }

    /// The words it ends on, for the thread it was last showing.
    static func finished(_ last: State, now: Date) -> State {
        var state = last
        state.kind = .finished
        state.line = "Nothing is at work now."
        state.requestID = nil
        state.warning = nil
        state.since = nil
        state.waitingOnYou = 0
        state.atWork = 0
        state.heard = now
        return state
    }

    /// Without the backticks an activity line or a request carries (the app draws them as chips).
    private static func plain(_ text: String) -> String {
        text.replacingOccurrences(of: "`", with: "").replacingOccurrences(of: "\n", with: " ")
    }

    private static func clip(_ text: String, _ most: Int) -> String {
        text.count > most ? String(text.prefix(most - 1)) + "…" : text
    }
}

/// Keeps Trek's Live Activity in step with the threads. Local updates only: a free signing has no
/// push, so while the app isn't running the card can't change, and past its stale date it says
/// when it last heard from the Mac. iOS lets an activity be started only from the front; updates
/// and ending work from the background too, in the moments the app still runs there.
final class TurnActivities {
    /// `@AppStorage` key: the phone's own setting, on unless turned off.
    static let settingKey = "liveActivity"
    /// How long a card is believed after the app last vouched for it.
    static let believedFor: TimeInterval = 120
    /// While in front, how often it's vouched for even if nothing changed.
    private static let vouchEvery: TimeInterval = 60
    /// Rows change several times a second while a thread works: the card follows every so often.
    private static let settle: TimeInterval = 1.5
    /// How long it shows "Nothing is at work now" before it goes.
    private static let lingers: TimeInterval = 90

    /// The widget extension is in the app: `install-device.sh` leaves it out where a free signing
    /// has no room for it, and the setting goes with it.
    static var installed: Bool {
        Bundle.main.builtInPlugInsURL.map {
            FileManager.default.fileExists(atPath: $0.appending(path: "TrekActivity.appex").path)
        } ?? false
    }

    private static var turnedOn: Bool { UserDefaults.standard.object(forKey: settingKey) as? Bool ?? true }

    /// Reads the threads as they are now.
    var read: (Date) -> TurnActivityPick.State? = { _ in nil }
    /// Every machine has said what its threads are. Until then no threads doesn't mean nothing is
    /// at work: a launch out of reach of the Mac must not end the card that says otherwise.
    var knowsAll: () -> Bool = { true }
    /// The machine of this thread can be heard now. Only then may the card say it was just now.
    var hears: (String) -> Bool = { _ in true }

    private var activity: Activity<TurnActivityAttributes>?
    private var shown: TurnActivityPick.State?
    /// The thread and kind the user swiped away: not put back until there's something else to say.
    private var swipedAway: String?
    /// The request just answered from the card: until the Mac's row moves on, it isn't offered again.
    private var answered: String?
    private var flushScheduled = false
    private var vouchScheduled = false
    private var backgroundTask = UIBackgroundTaskIdentifier.invalid

    /// At launch: what an earlier run left is taken over, or ended if there are several.
    func adopt() {
        let left = Activity<TurnActivityAttributes>.activities
        activity = left.first { $0.activityState == .active || $0.activityState == .stale }
        shown = activity?.content.state
        for other in left where other.id != activity?.id {
            // ActivityKit's own types aren't marked Sendable, though they're made to be used so.
            nonisolated(unsafe) let other = other
            Task { await other.end(nil, dismissalPolicy: .immediate) }
        }
    }

    /// The threads changed.
    func threadsChanged() {
        guard !flushScheduled else { return }
        flushScheduled = true
        DispatchQueue.main.asyncAfter(deadline: .now() + Self.settle) { [weak self] in
            self?.flushScheduled = false
            self?.flush()
        }
    }

    /// The setting changed, or the app came to the front: what can start only there starts now.
    func refresh() {
        endBackgroundTask()
        flush()
    }

    /// Leaving the front: the card is brought up to date, and the app asks for the moments iOS
    /// gives it to go on hearing from the Mac, so a turn ending just then still shows.
    func backgrounded() {
        flush()
        guard backgroundTask == .invalid else { return }
        backgroundTask = UIApplication.shared.beginBackgroundTask(withName: "Trek: last words") { [weak self] in
            self?.endBackgroundTask()
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 25) { [weak self] in self?.endBackgroundTask() }
    }

    private func endBackgroundTask() {
        guard backgroundTask != .invalid else { return }
        UIApplication.shared.endBackgroundTask(backgroundTask)
        backgroundTask = .invalid
    }

    /// An answer given on the card is on its way: the buttons go at once, and it says so.
    func answering(_ line: String) {
        guard var state = shown else { return }
        answered = state.requestID ?? answered
        state.line = line
        state.requestID = nil
        state.warning = nil
        state.heard = .now
        push(state)
    }

    private func flush() {
        let now = Date.now
        // The user took it off the Lock Screen, or the eight hours iOS allows ran out.
        if let a = activity, a.activityState == .dismissed || a.activityState == .ended {
            if a.activityState == .dismissed, let shown { swipedAway = Self.key(shown) }
            activity = nil
            shown = nil
        }
        guard Self.turnedOn else {
            finish(now)
            return
        }
        guard let next = read(now) else {
            if knowsAll() { finish(now) }
            return
        }
        if activity != nil {
            if let answered, next.requestID == answered { return }
            answered = nil
            if let shown, next.looksLike(shown),
               now.timeIntervalSince(shown.heard) < Self.vouchEvery || !hears(shown.threadID) { return }
            push(next)
        } else if Self.key(next) != swipedAway {
            start(next)
        }
        if UIApplication.shared.applicationState == .active { vouchLater() }
    }

    private func start(_ state: TurnActivityPick.State) {
        guard ActivityAuthorizationInfo().areActivitiesEnabled, UIApplication.shared.applicationState == .active else { return }
        do {
            activity = try Activity.request(attributes: TurnActivityAttributes(),
                                            content: Self.content(state), pushType: nil)
            shown = state
            swipedAway = nil
        } catch {
            activityLog.info("no Live Activity: \(error.localizedDescription, privacy: .public)")
        }
    }

    private func push(_ state: TurnActivityPick.State) {
        guard let activity else { return }
        shown = state
        let content = Self.content(state)
        nonisolated(unsafe) let live = activity
        Task { await live.update(content) }
    }

    private func finish(_ now: Date) {
        guard let activity, let shown else { return }
        self.activity = nil
        self.shown = nil
        nonisolated(unsafe) let live = activity
        // Put away on purpose (the setting): gone at once. Over by itself: it says so for a moment.
        let content = ActivityContent(state: TurnActivityPick.finished(shown, now: now), staleDate: nil)
        let policy: ActivityUIDismissalPolicy = Self.turnedOn ? .after(now.addingTimeInterval(Self.lingers)) : .immediate
        Task { await live.end(content, dismissalPolicy: policy) }
    }

    /// While the app is in front the card stays believed, whatever else happens.
    private func vouchLater() {
        guard !vouchScheduled else { return }
        vouchScheduled = true
        DispatchQueue.main.asyncAfter(deadline: .now() + Self.vouchEvery) { [weak self] in
            self?.vouchScheduled = false
            if UIApplication.shared.applicationState == .active { self?.flush() }
        }
    }

    private static func content(_ state: TurnActivityPick.State) -> ActivityContent<TurnActivityPick.State> {
        ActivityContent(state: state, staleDate: state.heard.addingTimeInterval(believedFor))
    }

    private static func key(_ state: TurnActivityPick.State) -> String { "\(state.threadID)/\(state.kind.rawValue)" }
}

extension AppModel {
    /// Resume or Retry on the Interrupted bar. It says "Resuming…" at once rather than when the
    /// Mac's row comes back, so a second tap can't be taken for another resume (the Mac refuses
    /// that, and the refusal would read as if the first had failed).
    func resumeCutShort(_ tid: String) {
        guard let i = threads.firstIndex(where: { $0.id == tid }), let before = threads[i].cutShort, before.state != .resuming else { return }
        threads[i].cutShort?.state = .resuming
        request(.threadAction(threadId: tid, action: .resumeInterrupted)) { [weak self] reply in
            guard case .error(_, _, let text) = reply, let self else { return }
            self.show(text, error: true)
            if let i = self.threads.firstIndex(where: { $0.id == tid }), self.threads[i].cutShort?.state == .resuming {
                self.threads[i].cutShort = before
            }
        }
    }

    /// Dismiss on the Interrupted bar: the bar goes at once, and comes back if the Mac refuses.
    func dismissCutShort(_ tid: String) {
        guard let i = threads.firstIndex(where: { $0.id == tid }), let before = threads[i].cutShort else { return }
        threads[i].cutShort = nil
        request(.threadAction(threadId: tid, action: .dismissInterrupted)) { [weak self] reply in
            guard case .error(_, _, let text) = reply, let self else { return }
            self.show(text, error: true)
            if let i = self.threads.firstIndex(where: { $0.id == tid }) { self.threads[i].cutShort = before }
        }
    }

    /// What `TurnActivities` reads.
    func activityState(now: Date) -> TurnActivityPick.State? {
        TurnActivityPick.state(threads: threads, now: now) { qid in
            let link = link(for: qid)
            return (link?.id ?? "", link?.bare(qid) ?? qid)
        }
    }

    /// Allow or Deny from the Lock Screen or the Dynamic Island. iOS may have just started the
    /// app for this, or woken it with its connection long gone: it boots if it hasn't, and the
    /// answer waits for the Mac for as long as iOS is likely to wait for it.
    func answerFromActivity(thread qid: String, request rid: String, allow: Bool) async {
        if !booted { boot(demo: Launch.demo) }
        activities.answering(allow ? "Allowing…" : "Denying…")
        guard let link = link(for: qid) else {
            activities.answering("That Mac isn't paired any more.")
            return
        }
        if link.connection != .connected { link.backend.wake() }
        let deadline = Date.now.addingTimeInterval(20)
        while link.connection != .connected, Date.now < deadline {
            try? await Task.sleep(for: .milliseconds(250))
        }
        guard link.connection == .connected else {
            activities.answering("Couldn't reach \(link.name). Open Trek to answer.")
            return
        }
        let failed: String? = await withCheckedContinuation { done in
            request(.answer(threadId: qid, requestId: rid, response: .approval(allow ? .allow : .deny))) { reply in
                if case .error(_, _, let text) = reply { done.resume(returning: text) } else { done.resume(returning: nil) }
            }
        }
        if let failed {
            activities.answering(failed)
        } else {
            activities.answering(allow ? "Allowed" : "Denied")
        }
    }
}
