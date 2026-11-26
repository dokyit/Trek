import Foundation
import Observation

/// A transcript as the phone shows it: messages stand alone; runs of thinking and tool calls fold
/// into one group ("Thought 2 times · ran 4 commands · edited 1 file"). Pending requests are left
/// out: the thread pins them above the composer.
///
/// Two blocks are equal when they have the same id and their items the same seqs (the Mac gives
/// an item a new, higher seq whenever it changes), so SwiftUI compares a row in constant time.
nonisolated struct Block: Identifiable, Equatable {
    enum Kind {
        /// `at`: when it was sent (ms). `aside`: not a prompt, nothing to rewind to.
        case user(text: String, images: Int, at: Int64?, aside: Bool)
        /// `streaming`: still being written.
        case assistant(String, streaming: Bool)
        case group([TItem])
        case resolved(TItem)
        /// `at`: when the turn finished (ms).
        case turnEnd(secs: Int, at: Int64?)
        case notice(String)
        case error(String)
        case limit(String, resetsAt: Int64?)
        case handoff(from: String, to: String)
        /// A row that opens another thread.
        case threadLink(ThreadLink)
        /// The files a turn changed, under its end.
        case changes(TurnChanges)
        /// A live card: a pull request, a workflow run, a release.
        case card(RemoteCard)
        /// A sub-agent's call: a row of its own, as on the Mac, that opens it (a Mac that says
        /// which sub-agent a row stands for, `ToolCall.subAgent`).
        case subAgent(ToolCall)
        /// Items of a kind this app doesn't know (a newer Mac's), one after another: one muted
        /// row says there's something here an update would show.
        case unsupported
    }

    let id: String
    /// The first item's id (a group's id is derived from it).
    let itemID: String
    let kind: Kind
    /// The highest seq among its items, and how many there are: changes whenever an item does.
    let version: Int64
    let count: Int
    /// What a newer Mac adds to a single item (a turn's verification, a handoff's agents, a
    /// message's videos).
    var extras = ItemExtras()

    static func == (a: Block, b: Block) -> Bool { a.id == b.id && a.version == b.version && a.count == b.count }

    var isGroup: Bool { if case .group = kind { true } else { false } }
    var isTurnEnd: Bool { if case .turnEnd = kind { true } else { false } }
    var isUser: Bool { if case .user = kind { true } else { false } }
    /// A message the user sent (not a wake-up note Trek sent the agent).
    var isPrompt: Bool { if case .user(let text, _, _, _) = kind { !WakeNote.isWake(text) } else { false } }

    /// The block for a single item that isn't thinking or a tool call (nil for one not shown).
    static func single(_ item: TItem) -> Block? {
        let kind: Kind
        switch item.body {
        case .user(let text, let images, let aside): kind = .user(text: text, images: images, at: item.at, aside: aside)
        case .assistant(let text, let streaming): kind = .assistant(text, streaming: streaming)
        case .approval, .question, .plan: kind = .resolved(item)
        case .turnEnd(let secs): kind = .turnEnd(secs: secs, at: item.at)
        case .notice(let s): kind = .notice(s)
        case .error(let s): kind = .error(s)
        case .limit(let s, let at): kind = .limit(s, resetsAt: at)
        case .handoff(let a, let b): kind = .handoff(from: a, to: b)
        case .threadLink(let l): kind = .threadLink(l)
        case .changes(let c):
            guard !c.files.isEmpty else { return nil }
            kind = .changes(c)
        case .card(let c): kind = .card(c)
        case .tool(let c) where c.subAgent != nil: kind = .subAgent(c)
        case .reasoning, .tool, .unknown: return nil
        }
        return Block(id: item.id, itemID: item.id, kind: kind, version: item.seq, count: 1, extras: item.extras)
    }

    static func group(_ run: ArraySlice<TItem>) -> Block {
        let first = run[run.startIndex]
        return Block(id: "g-\(first.id)", itemID: first.id, kind: .group(Array(run)),
                     version: run.reduce(Int64.min) { max($0, $1.seq) }, count: run.count)
    }

    /// The blocks that end a turn, as the Mac sees them: its `turn_end`, else (a turn that failed,
    /// hit a limit or was interrupted) the error, limit or notice it stopped with. `open`: the last
    /// turn is still running, so it hasn't ended.
    static func turnEnds(_ blocks: [Block], open: Bool) -> [String] {
        var ends: [String] = []
        var ended = false, worked = false
        var stop: String?
        func close() {
            if !ended, let stop { ends.append(stop) }
            ended = false; worked = false; stop = nil
        }
        for b in blocks {
            switch b.kind {
            // An aside (a command Trek answered) doesn't start a turn.
            case .user(_, _, _, let aside): if !aside { close() }
            case .turnEnd: ends.append(b.id); ended = true
            case .group, .assistant, .resolved, .subAgent: worked = true; stop = nil
            case .error, .limit: stop = b.id
            case .notice: if worked { stop = b.id }
            case .changes, .handoff, .threadLink, .card, .unsupported: break
            }
        }
        if !open { close() }
        return ends
    }

    /// What the row for items this app doesn't know says.
    static let unsupportedText = "Update Trek on this iPhone to see this"

    /// The row for a run of items of kinds this app doesn't know.
    static func unsupported(_ run: [TItem]) -> Block {
        Block(id: "u-\(run[0].id)", itemID: run[0].id, kind: .unsupported,
              version: run.reduce(Int64.min) { max($0, $1.seq) }, count: run.count)
    }

    static func isUnknown(_ item: TItem) -> Bool {
        if case .unknown = item.body { true } else { false }
    }

    static func folds(_ item: TItem) -> Bool {
        switch item.body {
        case .reasoning: true
        case .tool(let c): c.subAgent == nil
        default: false
        }
    }

    /// Blocks for `items[from...]`, and the item index each starts at. `from` must be where a
    /// block starts (or 0).
    static func build(_ items: [TItem], from: Int = 0) -> (blocks: [Block], starts: [Int]) {
        var blocks: [Block] = []
        var starts: [Int] = []
        var runStart: Int?
        var run: [Int] = []
        func flush() {
            guard let s = runStart else { return }
            var slice: [TItem] = []
            slice.reserveCapacity(run.count)
            for i in run { slice.append(items[i]) }
            blocks.append(group(slice[...]))
            starts.append(s)
            runStart = nil
            run = []
        }
        // Items of kinds this app doesn't know, one after another: one row for the run. They were
        // left out without a word, and still cut a tool group in two around nothing.
        var strangers: [TItem] = []
        var strangersStart = 0
        func flushStrangers() {
            guard !strangers.isEmpty else { return }
            blocks.append(unsupported(strangers))
            starts.append(strangersStart)
            strangers = []
        }
        var i = from
        while i < items.count {
            let item = items[i]
            if isUnknown(item) {
                flush()
                if strangers.isEmpty { strangersStart = i }
                strangers.append(item)
            } else if folds(item) {
                flushStrangers()
                if runStart == nil { runStart = i }
                run.append(i)
            } else if !item.isPendingRequest {
                flush()
                flushStrangers()
                if let b = single(item) {
                    blocks.append(b)
                    starts.append(i)
                }
            }
            i += 1
        }
        flush()
        flushStrangers()
        return (blocks, starts)
    }
}

/// One thread as the phone has it: its row, its transcript (items, and the blocks drawn from
/// them), and its paging state. Each thread has its own, so an update to one redraws only the
/// views that show it, and a row update doesn't touch the transcript.
@Observable
final class ThreadStore {
    /// The qualified id shared state knows this thread by (`"host:thread"`).
    let id: String
    /// Its host's id, for the transcript cache (each host's same-named thread keeps its own).
    let hostId: String

    /// The thread's row, as the Mac last sent it.
    private(set) var summary: ThreadSummary?
    /// What the transcript view draws.
    private(set) var blocks: [Block] = []
    /// Requests waiting on the user, pinned above the composer.
    private(set) var pending: [TItem] = []
    /// A transcript is in (from the Mac, or the cache on disk).
    private(set) var loaded = false
    /// Earlier items exist than the ones loaded.
    private(set) var more = false
    /// A page of earlier items is on its way.
    var loadingEarlier = false
    /// Why the transcript couldn't be fetched (the Mac refused, or never answered), while none of
    /// it is here: the thread says so, with Try again, where it would otherwise spin for ever.
    var failure: String?
    /// Bumped whenever the items before the earliest loaded one stop being the ones a page asked
    /// for would hold (the transcript rewritten, the list trimmed): a page that arrives after
    /// that is dropped (`prepend`).
    @ObservationIgnored private(set) var paging = 0
    /// Bumped when the transcript is replaced whole (the host rewrote it): the view draws it
    /// afresh, at the latest, rather than keep places that no longer fit it.
    private(set) var generation = 0
    /// The working line is out under the transcript: its turn runs, or has only just ended. A Mac
    /// says a turn is over and sends its last rows a moment apart: the line stays for those, so the
    /// two land together (`turnEndHold` at most: there may be none).
    private(set) var showsWorking = false
    /// Nothing to show under the transcript.
    var footerEmpty: Bool { !showsWorking && (summary?.runningSubAgents.isEmpty ?? true) }
    /// All of the transcript is in and there's nothing in it, nor anything at work: the list
    /// says so rather than stay blank.
    var nothingYet: Bool { loaded && !more && blocks.isEmpty && footerEmpty }

    @ObservationIgnored private(set) var items: [TItem] = []
    @ObservationIgnored private var index: [String: Int] = [:]
    /// The item each block starts at.
    @ObservationIgnored private var starts: [Int] = []
    /// The highest seq seen from the subscription: where a resubscribe carries on from.
    @ObservationIgnored var seq: Int64 = 0
    @ObservationIgnored var subscribed = false
    /// Items that arrived and aren't in `blocks` yet: applied together, once per run loop pass.
    @ObservationIgnored private var queued: [TItem] = []
    @ObservationIgnored private var flushScheduled = false
    /// Called after the items change (to save them to the cache).
    @ObservationIgnored var onChange: (() -> Void)?
    /// Called when something came from the Mac: a reply, live items (not a page of earlier ones).
    @ObservationIgnored var onArrived: (() -> Void)?
    /// Bumped as a hold on the working line starts and ends: a timer only ends its own.
    @ObservationIgnored private var hold: Int?
    @ObservationIgnored private var holds = 0
    nonisolated static let turnEndHold: TimeInterval = 0.2

    init(id: String, host: String, summary: ThreadSummary?) {
        self.id = id
        self.hostId = host
        self.summary = summary
        showsWorking = summary?.runState == .working
    }

    /// The id as its host knows it (the prefix off).
    var bareId: String { id.hasPrefix("\(hostId):") ? String(id.dropFirst(hostId.count + 1)) : id }

    func setSummary(_ s: ThreadSummary?) {
        if s != summary { summary = s }
        if s?.runState == .working {
            hold = nil
            if !showsWorking { showsWorking = true }
        } else if showsWorking, hold == nil {
            holds += 1
            let mine = holds
            hold = mine
            DispatchQueue.main.asyncAfter(deadline: .now() + Self.turnEndHold) { [weak self] in
                if self?.hold == mine { self?.endHold() }
            }
        }
    }

    private func endHold() {
        hold = nil
        if showsWorking { showsWorking = false }
    }

    /// The Mac's reply to a subscribe: the whole (last part of the) transcript, or what changed.
    ///
    /// A page sent whole (`reset`) doesn't say why: the Mac sends one after a rewind, and also
    /// whenever it can't serve what's newer (this phone fell behind on events, a reconnect, a
    /// restart). The page itself tells the two apart (`Fit`): one that lines up with what's here
    /// continues it, and the earlier pages stay, with the reader's place; only a transcript that
    /// was rewritten replaces everything and is drawn afresh.
    func applyReply(reset: Bool, seq: Int64, items new: [TItem], more: Bool) {
        flushQueued()
        if failure != nil { failure = nil }
        if !reset {
            upsert(new)
        } else {
            switch loaded ? Self.fit(new, onto: items) : .rewritten {
            case .continues(let keep):
                // The page carries every request still open: one kept from before that isn't
                // among them was closed while this phone wasn't listening (it went on asking for
                // an answer the Mac had long had).
                let kept = Self.closingMissed(Array(items[..<keep]), open: new)
                replaceAll(kept + new, more: self.more)
            case .covers:
                replaceAll(new, more: more)
            case .rewritten:
                replaceAll(new, more: more)
                // A page of earlier items asked for before this isn't this transcript's.
                paging += 1
                if loadingEarlier { loadingEarlier = false }
                if loaded, !items.isEmpty { generation += 1 }
            }
        }
        self.seq = max(reset ? seq : self.seq, seq)
        if !loaded { loaded = true }
        onChange?()
        onArrived?()
    }

    /// `kept` with each request still waiting there but not open in `open` (a page sent whole,
    /// which carries every open request) closed.
    nonisolated static func closingMissed(_ kept: [TItem], open: [TItem]) -> [TItem] {
        let stillOpen = Set(open.filter(\.isPendingRequest).map(\.id))
        return kept.map { $0.isPendingRequest && !stillOpen.contains($0.id) ? $0.closed : $0 }
    }

    /// How a page sent whole sits on what's here.
    nonisolated enum Fit: Equatable {
        /// It carries on from the item at this index here: the items before it stay.
        case continues(keep: Int)
        /// It starts before everything here and holds all of it: it replaces what's here, in place.
        case covers
        /// It isn't what's here any more (a rewind, a rewrite), or shares nothing with it.
        case rewritten
    }

    /// Whether `page` (the last items of the transcript as the Mac has it now) is the same
    /// transcript as `here`, further on. Compared on the items that stand in order on both sides:
    /// not requests (the Mac puts every open one at a page's end, wherever it was asked) nor a
    /// turn's changed files (counted a moment after its end, so one side may not have them yet).
    ///
    /// Where the two overlap, each pair must be the same item (`same`), not just the same id:
    /// the Mac's ids are positions (`i<n>`), so after a rewind new items take the old ones' ids.
    /// And the page must reach at least as far as what's here: the Mac's transcript shorter than
    /// this one was rewound.
    nonisolated static func fit(_ page: [TItem], onto here: [TItem]) -> Fit {
        func stands(_ item: TItem) -> Bool {
            switch item.body {
            case .approval, .question, .plan, .changes: false
            default: true
            }
        }
        let mine = here.indices.filter { stands(here[$0]) }
        let theirs = page.filter(stands)
        guard let first = theirs.first, let oldest = mine.first else { return .rewritten }
        if let at = mine.firstIndex(where: { here[$0].id == first.id }) {
            let tail = mine[at...]
            guard tail.count <= theirs.count, zip(tail, theirs).allSatisfy({ same(here[$0], $1) }) else { return .rewritten }
            return .continues(keep: mine[at])
        }
        if let at = theirs.firstIndex(where: { $0.id == here[oldest].id }) {
            let tail = theirs[at...]
            guard mine.count <= tail.count, zip(mine, tail).allSatisfy({ same(here[$0], $1) }) else { return .rewritten }
            return .covers
        }
        return .rewritten
    }

    /// Whether `b` is `a` as it is now: the same id and kind, and what an item keeps for life the
    /// same (a message's words and time, a tool call's id, a turn end's time, how an answer
    /// starts). What changes as an item goes on (an answer growing, a tool finishing) doesn't count.
    nonisolated static func same(_ a: TItem, _ b: TItem) -> Bool {
        guard a.id == b.id else { return false }
        switch (a.body, b.body) {
        case (.user(let x, _, _), .user(let y, _, _)): return x == y && a.at == b.at
        case (.tool(let x), .tool(let y)): return x.callId == y.callId
        case (.assistant(let x, _), .assistant(let y, _)), (.reasoning(let x), .reasoning(let y)):
            let n = min(x.count, y.count, 48)
            return x.prefix(n) == y.prefix(n)
        case (.turnEnd, .turnEnd): return a.at == b.at
        case (.notice, .notice), (.error, .error), (.limit, .limit), (.handoff, .handoff), (.threadLink, .threadLink),
             (.card, .card), (.unknown, .unknown), (.approval, .approval), (.question, .question), (.plan, .plan),
             (.changes, .changes):
            return true
        default: return false
        }
    }

    /// From the cache on disk: shown at once, and the subscription asks only for what's newer.
    func restore(items cached: [TItem], seq: Int64, more: Bool) {
        replaceAll(cached, more: more)
        self.seq = seq
        loaded = true
    }

    /// A live item: queued, and applied with the others that arrive in the same pass. `seq`
    /// moves only once the item is among `items`: the cache saves the two together, and a seq
    /// ahead of the items would ask the Mac only for what's newer than a message never kept,
    /// which then never came back (the phone showed a turn's work without the message that
    /// started it).
    func enqueue(_ item: TItem) {
        queued.append(item)
        guard !flushScheduled else { return }
        flushScheduled = true
        DispatchQueue.main.async { [weak self] in self?.flushQueued() }
    }

    func flushQueued() {
        flushScheduled = false
        guard !queued.isEmpty else { return }
        let batch = queued
        queued = []
        upsert(batch)
        seq = batch.reduce(seq) { max($0, $1.seq) }
        if hold != nil { endHold() }
        onChange?()
        onArrived?()
    }

    /// A page of earlier items, put before the ones loaded. `asked`: `paging` as it was when the
    /// page was asked for. A page from before the transcript was rewritten (or trimmed) holds
    /// items of a transcript that isn't this one any more, or ones that no longer meet the
    /// earliest here: it's dropped, and the next scroll to the top asks afresh. False then.
    @discardableResult
    func prepend(_ page: [TItem], more: Bool, asked: Int? = nil) -> Bool {
        if let asked, asked != paging { return false }
        let fresh = page.filter { index[$0.id] == nil }
        loadingEarlier = false
        if self.more != more { self.more = more }
        guard !fresh.isEmpty else { return true }
        replaceAll(fresh + items, more: more)
        onChange?()
        return true
    }

    /// Keeps the last `count` items (and requests still pending): a thread left after scrolling
    /// far back opens again as fast as the first time. The rest come back a page at a time.
    func trim(to count: Int) {
        guard items.count > count else { return }
        let cut = items.count - count
        replaceAll(items.prefix(cut).filter(\.isPendingRequest) + items.suffix(count), more: true)
        // A page on its way was for before the old earliest item: it would leave a hole now.
        paging += 1
        if loadingEarlier { loadingEarlier = false }
    }

    /// The item to ask for earlier items before: the first one loaded that isn't a request
    /// pinned from further back.
    var earliestID: String? {
        (items.first { $0.id.hasPrefix("i") } ?? items.first { !$0.isPendingRequest } ?? items.first)?.id
    }

    func item(_ id: String) -> TItem? { index[id].map { items[$0] } }

    /// Everything the agent said in the turn that ends at `endID`, for "Copy response".
    func responseText(endingAt endID: String) -> String {
        guard let end = index[endID] else { return "" }
        var parts: [String] = []
        var i = end - 1
        while i >= 0 {
            switch items[i].body {
            case .user: i = -1
            case .assistant(let text, _): parts.append(text)
            default: break
            }
            i -= 1
        }
        return parts.reversed().joined(separator: "\n\n")
    }

    // MARK: Building

    private func replaceAll(_ list: [TItem], more: Bool) {
        // A reply may repeat an id (a pending request also among the last items): keep the last.
        var seen: [String: Int] = [:]
        var out: [TItem] = []
        out.reserveCapacity(list.count)
        for item in list {
            if let i = seen[item.id] { out[i] = item } else { seen[item.id] = out.count; out.append(item) }
        }
        items = out
        index = seen
        let built = Perf.measure("blocks", "\(out.count) items") { Block.build(out) }
        starts = built.starts
        blocks = built.blocks
        if self.more != more { self.more = more }
        refreshPending()
    }

    private func upsert(_ batch: [TItem]) {
        guard !batch.isEmpty else { return }
        var dirty = items.count
        var requests = false
        for item in batch {
            if case .approval = item.body { requests = true }
            if case .question = item.body { requests = true }
            if case .plan = item.body { requests = true }
            if let i = index[item.id] {
                items[i] = item
                dirty = min(dirty, i)
            } else if more, let key = Self.order(item.id), let first = items.first.flatMap({ Self.order($0.id) }), key < first {
                // An update to an item before the loaded ones: it comes with its page.
                continue
            } else if let at = place(for: item.id), at < items.count {
                // One that belongs before the end (a turn's changed files, counted after later
                // turns were already here): put where it goes, not under the newest message.
                items.insert(item, at: at)
                for i in at..<items.count { index[items[i].id] = i }
                dirty = min(dirty, at)
            } else {
                index[item.id] = items.count
                dirty = min(dirty, items.count)
                items.append(item)
            }
        }
        rebuild(from: dirty)
        if requests { refreshPending() }
    }

    /// Rebuilds the blocks from the one holding item `from` on; the ones before stay as they are.
    private func rebuild(from: Int) {
        guard from < items.count else { return }
        // The last block starting at or before `from`.
        var lo = 0, hi = starts.count
        while lo < hi {
            let mid = (lo + hi) / 2
            if starts[mid] <= from { lo = mid + 1 } else { hi = mid }
        }
        let k = max(0, lo - 1)
        let start = starts.isEmpty ? 0 : (lo == 0 ? 0 : starts[k])
        let keep = lo == 0 ? 0 : k
        let tail = Block.build(items, from: start)
        // One mutation: SwiftUI sees the list change once.
        var next = Array(blocks.prefix(keep))
        next.append(contentsOf: tail.blocks)
        var nextStarts = Array(starts.prefix(keep))
        nextStarts.append(contentsOf: tail.starts)
        starts = nextStarts
        if next != blocks { blocks = next }
    }

    private func refreshPending() {
        let list = items.filter(\.isPendingRequest)
        if list != pending { pending = list }
    }

    /// Where a new item with `id` goes among the loaded ones: after the last one the Mac orders
    /// at or before it (`order`). Nil for ids it doesn't order (requests), which go at the end.
    private func place(for id: String) -> Int? {
        guard let key = Self.order(id) else { return nil }
        var i = items.count - 1
        while i >= 0 {
            if let k = Self.order(items[i].id), k <= key { return i + 1 }
            i -= 1
        }
        return nil
    }

    /// Where an item id sits in the transcript, for the Mac's `i<n>` / `c<n>` ids (a turn's
    /// changes follow its end); nil for any other id.
    static func order(_ id: String) -> Double? {
        guard let head = id.first, head == "i" || head == "c", let n = Int(id.dropFirst()) else { return nil }
        return Double(n) + (head == "c" ? 0.5 : 0)
    }
}
