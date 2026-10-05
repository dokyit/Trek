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
        /// `at`: when it was sent (ms).
        case user(text: String, images: Int, at: Int64?)
        case assistant(String)
        case group([TItem])
        case resolved(TItem)
        /// `at`: when the turn finished (ms).
        case turnEnd(secs: Int, at: Int64?)
        case notice(String)
        case error(String)
        case limit(String, resetsAt: Int64?)
        case handoff(from: String, to: String)
        /// The files a turn changed, under its end.
        case changes(TurnChanges)
    }

    let id: String
    /// The first item's id (a group's id is derived from it).
    let itemID: String
    let kind: Kind
    /// The highest seq among its items, and how many there are: changes whenever an item does.
    let version: Int64
    let count: Int

    static func == (a: Block, b: Block) -> Bool { a.id == b.id && a.version == b.version && a.count == b.count }

    var isGroup: Bool { if case .group = kind { true } else { false } }
    var isTurnEnd: Bool { if case .turnEnd = kind { true } else { false } }
    var isUser: Bool { if case .user = kind { true } else { false } }

    /// The block for a single item that isn't thinking or a tool call (nil for one not shown).
    static func single(_ item: TItem) -> Block? {
        let kind: Kind
        switch item.body {
        case .user(let text, let images): kind = .user(text: text, images: images, at: item.at)
        case .assistant(let text, _): kind = .assistant(text)
        case .approval, .question, .plan: kind = .resolved(item)
        case .turnEnd(let secs): kind = .turnEnd(secs: secs, at: item.at)
        case .notice(let s): kind = .notice(s)
        case .error(let s): kind = .error(s)
        case .limit(let s, let at): kind = .limit(s, resetsAt: at)
        case .handoff(let a, let b): kind = .handoff(from: a, to: b)
        case .changes(let c):
            guard !c.files.isEmpty else { return nil }
            kind = .changes(c)
        case .reasoning, .tool, .unknown: return nil
        }
        return Block(id: item.id, itemID: item.id, kind: kind, version: item.seq, count: 1)
    }

    static func group(_ run: ArraySlice<TItem>) -> Block {
        let first = run[run.startIndex]
        return Block(id: "g-\(first.id)", itemID: first.id, kind: .group(Array(run)),
                     version: run.reduce(Int64.min) { max($0, $1.seq) }, count: run.count)
    }

    static func folds(_ item: TItem) -> Bool {
        switch item.body {
        case .reasoning, .tool: true
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
        var i = from
        while i < items.count {
            let item = items[i]
            if folds(item) {
                if runStart == nil { runStart = i }
                run.append(i)
            } else if !item.isPendingRequest {
                flush()
                if let b = single(item) {
                    blocks.append(b)
                    starts.append(i)
                }
            }
            i += 1
        }
        flush()
        return (blocks, starts)
    }
}

/// One thread as the phone has it: its row, its transcript (items, and the blocks drawn from
/// them), and its paging state. Each thread has its own, so an update to one redraws only the
/// views that show it, and a row update doesn't touch the transcript.
@Observable
final class ThreadStore {
    let id: String

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

    init(id: String, summary: ThreadSummary?) {
        self.id = id
        self.summary = summary
    }

    func setSummary(_ s: ThreadSummary?) {
        if s != summary { summary = s }
    }

    /// The Mac's reply to a subscribe: the whole (last part of the) transcript, or what changed.
    func applyReply(reset: Bool, seq: Int64, items new: [TItem], more: Bool) {
        flushQueued()
        if reset {
            replaceAll(new, more: more)
        } else {
            upsert(new)
        }
        self.seq = max(reset ? seq : self.seq, seq)
        if !loaded { loaded = true }
        onChange?()
    }

    /// From the cache on disk: shown at once, and the subscription asks only for what's newer.
    func restore(items cached: [TItem], seq: Int64, more: Bool) {
        replaceAll(cached, more: more)
        self.seq = seq
        loaded = true
    }

    /// A live item: queued, and applied with the others that arrive in the same pass.
    func enqueue(_ item: TItem) {
        queued.append(item)
        seq = max(seq, item.seq)
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
        onChange?()
    }

    /// A page of earlier items, put before the ones loaded.
    func prepend(_ page: [TItem], more: Bool) {
        let fresh = page.filter { index[$0.id] == nil }
        loadingEarlier = false
        if self.more != more { self.more = more }
        guard !fresh.isEmpty else { return }
        replaceAll(fresh + items, more: more)
        onChange?()
    }

    /// Keeps the last `count` items (and requests still pending): a thread left after scrolling
    /// far back opens again as fast as the first time. The rest come back a page at a time.
    func trim(to count: Int) {
        guard items.count > count else { return }
        let cut = items.count - count
        replaceAll(items.prefix(cut).filter(\.isPendingRequest) + items.suffix(count), more: true)
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

    /// Where an item id sits in the transcript, for the Mac's `i<n>` / `c<n>` ids (a turn's
    /// changes follow its end); nil for any other id.
    static func order(_ id: String) -> Double? {
        guard let head = id.first, head == "i" || head == "c", let n = Int(id.dropFirst()) else { return nil }
        return Double(n) + (head == "c" ? 0.5 : 0)
    }
}
