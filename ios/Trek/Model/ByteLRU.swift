import Foundation

/// A cache bounded by what it holds, not by how many things (the Mac's is `budget.rs`). A count
/// says nothing about memory: 3,000 one-line answers and 3,000 answers with a file in each are
/// the same 3,000. Here every entry says what it costs in bytes, the least recently used go
/// first once the budget is passed, and an entry over `entryCap` isn't kept at all, so one huge
/// one can't push everything else out. Whoever asked still has what it made, and makes it again
/// next time.
///
/// `supersedes`: whether a new key takes the place of one put in just before it. An answer
/// streaming in asks for its text again, a little longer, many times a second: with this each
/// version replaces the last instead of joining it, so a long answer leaves one entry behind and
/// not one for every length it had on the way.
nonisolated struct ByteLRU<Key: Hashable, Value> {
    fileprivate struct Entry {
        var value: Value
        var cost: Int
        /// When it was last used (`tick`).
        var used: UInt64
    }

    let budget: Int
    let entryCap: Int
    private let supersedes: (@Sendable (_ new: Key, _ old: Key) -> Bool)?
    private var entries: [Key: Entry] = [:]
    private var tick: UInt64 = 0
    /// The last few keys put in, newest last: what a new one may supersede. Several, since two
    /// texts can stream at once (a thought and an answer) and take turns.
    private var recent: [Key] = []
    private static var recentKept: Int { 4 }
    /// What the entries cost together.
    private(set) var cost = 0

    init(budget: Int, entryCap: Int, supersedes: (@Sendable (_ new: Key, _ old: Key) -> Bool)? = nil) {
        self.budget = budget
        self.entryCap = entryCap
        self.supersedes = supersedes
    }

    var count: Int { entries.count }

    /// Whether `key` is kept, without counting as a use.
    func contains(_ key: Key) -> Bool { entries[key] != nil }

    /// A hit: the value, now the most recently used.
    mutating func value(for key: Key) -> Value? {
        guard let i = entries.index(forKey: key) else { return nil }
        tick += 1
        entries.values[i].used = tick
        return entries.values[i].value
    }

    /// Keep `value`, `cost` bytes big, as the most recently used; the least recently used go to
    /// make room. False when it's over `entryCap` and so wasn't kept.
    @discardableResult
    mutating func insert(_ value: Value, for key: Key, cost: Int) -> Bool {
        if let supersedes {
            for old in recent where old != key && supersedes(key, old) { remove(old) }
            recent = recent.filter { $0 != key && entries[$0] != nil }
        }
        guard cost <= entryCap else {
            remove(key)
            return false
        }
        tick += 1
        if let old = entries.updateValue(Entry(value: value, cost: cost, used: tick), forKey: key) {
            self.cost -= old.cost
        }
        self.cost += cost
        if supersedes != nil {
            recent.append(key)
            if recent.count > Self.recentKept { recent.removeFirst() }
        }
        if self.cost > budget { trim() }
        return true
    }

    mutating func remove(_ key: Key) {
        if let old = entries.removeValue(forKey: key) { cost -= old.cost }
    }

    mutating func removeAll() {
        entries.removeAll()
        recent.removeAll()
        cost = 0
    }

    /// Oldest first, down to seven eighths of the budget, so this isn't done for every insert
    /// once the cache is full.
    private mutating func trim() {
        let target = budget - budget / 8
        for (key, _) in entries.map({ ($0.key, $0.value.used) }).sorted(by: { $0.1 < $1.1 }) {
            if cost <= target { break }
            remove(key)
        }
    }
}

extension ByteLRU.Entry: Sendable where Key: Sendable, Value: Sendable {}
extension ByteLRU: Sendable where Key: Sendable, Value: Sendable {}

nonisolated extension String {
    /// This text is `old` with more after it: the next version of a text that's streaming in.
    func extends(_ old: String) -> Bool {
        utf8.count > old.utf8.count && hasPrefix(old)
    }
}
