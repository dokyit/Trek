import Foundation

// What's typed and not sent, and telling the Mac a thread being read has been seen.

extension AppModel {
    // MARK: Composers and drafts

    /// The thread's composer, made on first use with the draft kept for it. One per thread for as
    /// long as the app runs (its photos and files with it), never swapped for another while a
    /// screen may hold it; the text is also kept on disk (`DraftStore`), so it's there after iOS
    /// ended the app.
    func composer(_ tid: String) -> ComposerState {
        if let c = composers[tid] { return c }
        let c = ComposerState()
        if let kept = drafts.text(tid) { c.put(kept) }
        c.onDraft = { [weak self] text in self?.draftChanged(tid, text) }
        composers[tid] = c
        return c
    }

    private func draftChanged(_ tid: String, _ text: String) {
        // A message on its way empties the composer; its text stays kept until the Mac has it,
        // whatever is typed meanwhile (`sendSettled` then keeps what the composer holds).
        if sending[tid, default: 0] > 0 { return }
        drafts.set(tid, text)
    }

    /// The thread is gone from its machine: its composer and its kept draft go too.
    func forgetComposer(_ tid: String) {
        if let c = composers.removeValue(forKey: tid) {
            c.onDraft = nil
            for f in c.files { f.discard() }
        }
        sending[tid] = nil
        seen.forget(tid)
        drafts.clear(tid)
    }

    /// Takes what the thread's composer holds, to send: the composer empties as the message goes,
    /// while the kept draft stays until the Mac has taken it (`send(…held:)` settles it).
    func takeComposed(_ tid: String) -> HeldMessage {
        let c = composer(tid)
        let held = HeldMessage(text: c.draft, photos: c.photos)
        sending[tid, default: 0] += 1
        c.cleared(after: c.draft)
        return held
    }

    /// The Mac answered a message taken with `takeComposed`. Refused (`restoring`): it goes back
    /// into the thread's composer, ahead of anything typed since, and so into its kept draft.
    /// Taken: the kept draft becomes whatever the composer holds now.
    func sendSettled(_ tid: String, restoring held: HeldMessage?) {
        // Its thread went meanwhile (`forgetComposer`): there's nowhere for it to come back to.
        guard let out = sending[tid] else { return }
        sending[tid] = out > 1 ? out - 1 : nil
        let c = composer(tid)
        if let held { c.restore(text: held.text, photos: held.photos) }
        drafts.set(tid, c.draft)
    }

    // MARK: Seen

    /// The thread's own screen came up: seen, here at once and on the Mac.
    func opened(_ tid: String) {
        if let i = threads.firstIndex(where: { $0.id == tid }), threads[i].unseen { threads[i].unseen = false }
        noteSeen(tid)
    }

    /// Something happened in `tid` that the Mac may count as unread (its turn ended, its row says
    /// unread, items came in, the app came back to the front on it): the Mac is told it's seen if
    /// its screen is the one being read, and not more often than `SeenPolicy.gap`.
    func noteSeen(_ tid: String) {
        switch seen.note(tid, showing: screens.last == tid, active: sceneActive, now: now()) {
        case .send: markSeen(tid)
        case .wait(let delay):
            after(delay) { [weak self] in
                guard let self else { return }
                if self.seen.due(tid, showing: self.screens.last == tid, active: self.sceneActive, now: self.now()) == .send {
                    self.markSeen(tid)
                }
            }
        case .no: break
        }
    }

    /// Items came in for `s`. While its turn runs they change nothing about read or unread (the
    /// row says so when they do, and each `mark_seen` is a write on the Mac): told for ones that
    /// come outside a turn, or while the row says unread.
    func itemsArrived(_ s: ThreadStore) {
        guard screens.last == s.id, let row = s.summary, row.unseen || row.runState != .working else { return }
        noteSeen(s.id)
    }

    /// The thread whose screen is in view, if any.
    var showing: String? { screens.last }

    // MARK: Icons

    /// The machine to ask for an agent's icon `hash`: one that lists `agent` with that icon (the
    /// picked machine when it's one of them, else one that's connected), found through its agents
    /// or a thread of its that runs the agent. With two machines paired, an agent only the other
    /// one has was asked of the picked machine, which doesn't know it, and got a neutral glyph.
    func iconHost(agent: String, hash: String) -> HostLink? {
        var has = links.filter { l in
            l.agents.contains { $0.key == agent && $0.icon == hash }
                || threads.contains { l.owns($0.id) && $0.agent.key == agent && $0.agent.icon == hash }
        }
        // None names that very icon (it came with an item, a handoff's agent): one that at least
        // lists the agent.
        if has.isEmpty { has = links.filter { l in l.agents.contains { $0.key == agent } } }
        if let picked = selected, has.contains(where: { $0 === picked }) { return picked }
        return has.first { $0.connection == .connected } ?? has.first
    }
}
